//! WLED visualization renderers
//!
//! Three strip visualization modes:
//! - **Spectrum**: frequency band scrolling timeline (center-mirrored)
//! - **Beat**: onset envelope spatial spread from center
//! - **Pulse**: whole-strip energy-reactive wash
//!
//! Each renderer produces a `Vec<(u8, u8, u8)>` of per-LED RGB values.

use std::collections::VecDeque;
use crate::hue::CueSnapshot;
use crate::emerge::{Palette, PaletteBias, EnergyZone};

use super::FrequencyBands;

// ============================================================================
// ENVELOPE TYPES
// ============================================================================

/// Envelope with decay rate proportional to current value (natural fade-out)
pub struct DynamicDecay {
    pub value: f32,
    decay_rate: f32,
}

impl DynamicDecay {
    pub fn new(decay_rate: f32) -> Self {
        Self { value: 0.0, decay_rate }
    }

    /// Trigger with an intensity (takes the max of current and new)
    pub fn trigger(&mut self, intensity: f32) {
        self.value = self.value.max(intensity);
    }

    /// Tick the envelope and return the current value
    pub fn tick(&mut self, dt: f32) -> f32 {
        self.value = (self.value - self.decay_rate * dt).max(0.0);
        self.value
    }
}

/// Envelope with fixed duration (linear decay to zero)
pub struct FixedDecay {
    value: f32,
    duration: f32,
    elapsed: f32,
}

impl FixedDecay {
    pub fn new(duration_secs: f32) -> Self {
        Self { value: 0.0, duration: duration_secs, elapsed: 0.0 }
    }

    /// Trigger with an intensity (restarts the decay)
    pub fn trigger(&mut self, intensity: f32) {
        self.value = intensity;
        self.elapsed = 0.0;
    }

    /// Tick the envelope and return the current value
    pub fn tick(&mut self, dt: f32) -> f32 {
        self.elapsed += dt;
        if self.elapsed >= self.duration {
            self.value = 0.0;
        } else {
            let t = self.elapsed / self.duration;
            self.value *= 1.0 - t.min(1.0);
        }
        self.value
    }
}

// ============================================================================
// COLOR UTILITIES
// ============================================================================

/// Force saturate an RGB color: convert to HSV, set S=1.0, convert back.
/// Returns (r, g, b) in 0.0-1.0 range.
fn force_saturate(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (h, _s, v) = rgb_to_hsv(r, g, b);
    if v < 0.01 {
        return (0.0, 0.0, 0.0);
    }
    hsv_to_rgb(h, 1.0, v)
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let v = max;
    if max <= 0.0 {
        return (0.0, 0.0, v);
    }
    let s = delta / max;
    if delta < 1e-6 {
        return (0.0, s, v);
    }

    let h = if r >= max {
        (g - b) / delta
    } else if g >= max {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    let h = h / 6.0;
    let h = if h < 0.0 { h + 1.0 } else { h };

    (h, s, v)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    if s <= 0.0 {
        return (v, v, v);
    }

    let h = h * 6.0;
    let i = h.floor() as u32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));

    match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        5 => (v, p, q),
        _ => (v, v, v),
    }
}

/// Convert float RGB to u8 RGB
fn to_u8(r: f32, g: f32, b: f32) -> (u8, u8, u8) {
    (
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Blend all 12 pitch class colors weighted by chroma energy
pub fn chroma_to_rgb(chroma: &[f32; 12]) -> (f32, f32, f32) {
    let mut r_sum = 0.0f32;
    let mut g_sum = 0.0f32;
    let mut b_sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for (i, &energy) in chroma.iter().enumerate() {
        if energy > 0.001 {
            let hue = i as f32 / 12.0;
            let (pr, pg, pb) = hsv_to_rgb(hue, 0.9, 1.0);
            r_sum += pr * energy;
            g_sum += pg * energy;
            b_sum += pb * energy;
            weight_sum += energy;
        }
    }

    if weight_sum > 0.001 {
        (r_sum / weight_sum, g_sum / weight_sum, b_sum / weight_sum)
    } else {
        (0.5, 0.5, 0.5)
    }
}

/// Map Palette + PaletteBias to base RGB color (duplicated from hue/mappings.rs)
fn palette_to_rgb(palette: Palette, bias: PaletteBias) -> (f32, f32, f32) {
    match (palette, bias) {
        (Palette::Ice, PaletteBias::Cool) =>    (0.6, 0.75, 1.0),
        (Palette::Ice, PaletteBias::Warm) =>    (1.0, 0.85, 0.65),
        (Palette::Ice, PaletteBias::Neutral) => (0.85, 0.85, 0.85),
        (Palette::Ice, PaletteBias::Vivid) =>   (0.9, 0.4, 0.8),

        (Palette::CoolA, PaletteBias::Cool) =>    (0.3, 0.7, 1.0),
        (Palette::CoolA, PaletteBias::Warm) =>    (1.0, 0.6, 0.3),
        (Palette::CoolA, PaletteBias::Neutral) => (0.7, 0.7, 0.75),
        (Palette::CoolA, PaletteBias::Vivid) =>   (0.2, 0.4, 1.0),

        (Palette::CoolB, PaletteBias::Cool) =>    (0.15, 0.5, 1.0),
        (Palette::CoolB, PaletteBias::Warm) =>    (1.0, 0.35, 0.15),
        (Palette::CoolB, PaletteBias::Neutral) => (0.5, 0.5, 0.55),
        (Palette::CoolB, PaletteBias::Vivid) =>   (0.6, 0.1, 1.0),
    }
}

/// Map dominant pitch class (0-11) to color wheel RGB
fn pitch_to_rgb(pc: u8) -> (f32, f32, f32) {
    let hue = (pc as f32) / 12.0;
    hsv_to_rgb(hue, 0.9, 1.0)
}

/// The pitch colour the strip keys off this frame: dominant pitch ("Dom"
/// checked), or the all-notes chromatic blend when un-checked (spread chord
/// washes whiter via `all_notes_sat`).
fn pitch_target(snapshot: &CueSnapshot) -> (f32, f32, f32) {
    if snapshot.all_notes_color {
        hsv_to_rgb(snapshot.all_notes_hue, 0.9 * snapshot.all_notes_sat, 1.0)
    } else {
        pitch_to_rgb(snapshot.dominant_pc)
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ============================================================================
// RENDERERS
// ============================================================================

/// Spectrum mode: frequency band scrolling timeline, center-mirrored.
///
/// Each frame computes a color from bass(R) / mid(G) / high(B), force-saturates it,
/// pushes to center of the buffer, and scrolls outward (mirrored).
pub fn render_spectrum(
    buffer: &mut VecDeque<(f32, f32, f32)>,
    bands: &FrequencyBands,
    led_count: usize,
) -> Vec<(u8, u8, u8)> {
    let half = led_count / 2;

    // Ensure buffer is correctly sized for the half
    while buffer.len() < half {
        buffer.push_back((0.0, 0.0, 0.0));
    }
    while buffer.len() > half {
        buffer.pop_back();
    }

    // Compute new color from frequency bands
    let raw_r = bands.bass;
    let raw_g = bands.mid;
    let raw_b = bands.high;

    // Force saturate for vivid strip colors
    let (r, g, b) = force_saturate(raw_r, raw_g, raw_b);

    // Brighten: ensure we always have some visible color when audio is present
    let max_band = bands.bass.max(bands.mid).max(bands.high);
    let brightness = max_band.max(0.05); // floor so it's visible
    let (r, g, b) = (
        (r * brightness).min(1.0),
        (g * brightness).min(1.0),
        (b * brightness).min(1.0),
    );

    // Push new color to front (center), scroll outward
    buffer.pop_back();
    buffer.push_front((r, g, b));

    // Build mirrored output: left half = reversed buffer, right half = buffer
    let mut leds = Vec::with_capacity(led_count);
    // Left side: buffer reversed (center → left edge)
    for i in (0..half).rev() {
        if let Some(&color) = buffer.get(i) {
            leds.push(to_u8(color.0, color.1, color.2));
        } else {
            leds.push((0, 0, 0));
        }
    }
    // Right side: buffer as-is (center → right edge)
    for i in 0..half {
        if let Some(&color) = buffer.get(i) {
            leds.push(to_u8(color.0, color.1, color.2));
        } else {
            leds.push((0, 0, 0));
        }
    }
    // If odd LED count, insert center pixel
    if led_count % 2 != 0 {
        let center = to_u8(r, g, b);
        leds.insert(half, center);
    }

    leds
}

/// Beat mode: onset envelope spatial spread from center.
///
/// Three envelopes:
/// - Kick → red spread from center
/// - Dominant pitch change → pitch-colored spread (chroma → color wheel)
/// - Polarity flash → white spread
///
/// Envelope value = fraction of strip lit from center outward.
pub fn render_beat(
    kick_env: &mut DynamicDecay,
    dominant_env: &mut DynamicDecay,
    polarity_env: &mut FixedDecay,
    snapshot: &CueSnapshot,
    bands: &FrequencyBands,
    led_count: usize,
    dt: f32,
) -> Vec<(u8, u8, u8)> {
    let half = (led_count / 2) as f32;

    // Trigger envelopes from snapshot events
    if bands.kick {
        kick_env.trigger(bands.kick_intensity.max(0.7));
    }
    if snapshot.dominant_flash_active {
        dominant_env.trigger(0.9);
    }
    if snapshot.polarity_flash_active {
        polarity_env.trigger(1.0);
    }

    // Tick all envelopes
    let kick_val = kick_env.tick(dt);
    let dominant_val = dominant_env.tick(dt);
    let polarity_val = polarity_env.tick(dt);

    // Pitch color: "Dom" checked → single dominant pitch; un-checked → the
    // vivid all-notes chroma blend (raw chroma when present, else the
    // smoothed all-notes target).
    let (pitch_r, pitch_g, pitch_b) = if !snapshot.all_notes_color {
        pitch_to_rgb(snapshot.dominant_pc)
    } else if bands.chroma.iter().any(|&c| c > 0.01) {
        let (cr, cg, cb) = chroma_to_rgb(&bands.chroma);
        force_saturate(cr, cg, cb)
    } else {
        pitch_target(snapshot)
    };

    // Build LED array: for each LED, compute distance from center,
    // check which envelopes cover it, blend colors
    let mut leds = Vec::with_capacity(led_count);
    for i in 0..led_count {
        let center = (led_count as f32 - 1.0) / 2.0;
        let dist = ((i as f32) - center).abs() / half; // 0.0 at center, ~1.0 at edges

        let mut r = 0.0f32;
        let mut g = 0.0f32;
        let mut b = 0.0f32;

        // Kick: red spread
        if kick_val > 0.01 && dist < kick_val {
            let fade = 1.0 - (dist / kick_val).min(1.0);
            r += 1.0 * fade * kick_val;
        }

        // Dominant pitch change: pitch-colored spread
        if dominant_val > 0.01 && dist < dominant_val {
            let fade = 1.0 - (dist / dominant_val).min(1.0);
            r += pitch_r * fade * dominant_val;
            g += pitch_g * fade * dominant_val;
            b += pitch_b * fade * dominant_val;
        }

        // Polarity flash: white spread
        if polarity_val > 0.01 && dist < polarity_val {
            let fade = 1.0 - (dist / polarity_val).min(1.0);
            r += fade * polarity_val;
            g += fade * polarity_val;
            b += fade * polarity_val;
        }

        leds.push(to_u8(r.min(1.0), g.min(1.0), b.min(1.0)));
    }

    leds
}

/// Pulse mode: whole-strip energy-reactive wash.
///
/// Uses the same palette/zone/energy/tonality/pitch as Hue wash but force-saturated
/// for vivid strip colors. Bass drives brightness pulsing.
/// Chroma-weighted hue blends pitch class colors for richer harmonic color.
pub fn render_pulse(
    snapshot: &CueSnapshot,
    bands: &FrequencyBands,
    led_count: usize,
) -> Vec<(u8, u8, u8)> {
    // Base color from palette
    let (mut r, mut g, mut b) = palette_to_rgb(snapshot.palette, snapshot.palette_bias);

    // Tonality shift (same as Hue): major = warmer, minor = cooler
    let tonality = snapshot.major_smooth;
    if tonality > 0.55 {
        let shift = (tonality - 0.55) * 2.2;
        r = (r + shift * 0.25).min(1.0);
        g = (g + shift * 0.08).min(1.0);
        b = (b - shift * 0.20).max(0.0);
    } else if tonality < 0.45 {
        let shift = (0.45 - tonality) * 2.2;
        b = (b + shift * 0.25).min(1.0);
        g = (g + shift * 0.08).min(1.0);
        r = (r - shift * 0.20).max(0.0);
    }

    // Blend in chroma-weighted pitch color for richer harmonic color
    let (cr, cg, cb) = chroma_to_rgb(&bands.chroma);
    let chroma_weight = 0.15 + 0.30 * snapshot.tension;
    r = lerp(r, cr, chroma_weight);
    g = lerp(g, cg, chroma_weight);
    b = lerp(b, cb, chroma_weight);

    // Pitch hue rotation — dominant pitch, or the all-notes blend when "Dom"
    // is un-checked (the :383 chroma blend above stays as added richness).
    let (pr, pg, pb) = pitch_target(snapshot);
    let pitch_weight = 0.10 + 0.20 * snapshot.tension;
    r = lerp(r, pr, pitch_weight);
    g = lerp(g, pg, pitch_weight);
    b = lerp(b, pb, pitch_weight);

    // Force saturate for vivid strip colors
    let (r, g, b) = force_saturate(r, g, b);

    // Brightness from zone + energy
    let zone_brightness = match snapshot.zone {
        EnergyZone::Silent => 0.05,
        EnergyZone::Still => 0.20,
        EnergyZone::Low => 0.40,
        EnergyZone::Mid => 0.70,
        EnergyZone::High => 0.95,
    };

    // Bass-driven brightness pulsing
    let bass_pulse = bands.bass * 0.35;
    let brightness = (zone_brightness + bass_pulse + snapshot.energy * 0.15).clamp(0.05, 1.0);

    let pixel = to_u8(r * brightness, g * brightness, b * brightness);
    vec![pixel; led_count]
}
