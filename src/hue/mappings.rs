//! CueEngine → Hue RGB mapping
//!
//! Translates CueEngine state (palette, zone, energy, tension, polarity,
//! dominant pitch, tonality, energy slope, triggers) into per-light RGB values
//! based on each light's assigned role (Wash, Accent, Ambient, Strip).
//!
//! ## UI ↔ Code field glossary
//!
//! The Harmonic Field panel in the UI displays metrics that differ from the
//! CueEngine/CueSnapshot fields used here. When the user references these by
//! name, this is the mapping:
//!
//! | UI label (Harmonic Field) | Source                              | CueSnapshot field       | Typical range |
//! |---------------------------|-------------------------------------|-------------------------|---------------|
//! | **Tension: N%**           | Sum of positive chroma deviations   | *(UI-only, not in snap)* | 0–100%       |
//! | **Balance: N%**           | 1 − normalized chroma variance      | `balance`               | 0–100%        |
//! | **Energy bar**            | Perceptual energy (remapped)        | `energy`                | 0.0–1.0       |
//! | **Stability bar**         | Spectral stability                  | `stability`             | 0.0–1.0       |
//! | **Dominance: N%**         | Key detector confidence             | `dominant_pc`           | 0–11 (pc)     |
//!
//! CueEngine fields NOT shown in the UI but used for lighting decisions:
//! - `tension` (CueSnapshot) — `instability * 0.5 + tonal_separation * 0.5`, stays 0.05–0.15
//! - `tension_bank` — accumulated "buildup currency", reaches 0.6–0.7 before drops
//! - `excitement` — slow-building musical arc (seconds), reaches 1.0 at drops
//! - `energy_slope` — rate of energy change (positive = building, negative = decaying)
//! - `bass_pump` — kick-triggered brightness envelope, decays ~150ms

use super::{CueSnapshot, HueLightAssignment, LightRole};
use super::streaming::HueLightColor;
use crate::emerge::{EnergyZone, Palette, PaletteBias, AutoBlinderPhase};

// ============================================================================
// SPATIAL ANIMATION PRIMITIVES
// ============================================================================

/// Spatial effect type, driven by macro phase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpatialEffect {
    /// No spatial modulation (current behavior)
    None,
    /// Sequential sweep, direction from energy_slope (Cool phase)
    Chase,
    /// Bright cursor ping-pongs across lights, speed ramps with excitement (Buildup phase)
    Bounce,
    /// Center-outward radial burst on each kick (Pump phase)
    Explode,
}

/// Select spatial effect based on current macro phase
fn select_spatial_effect(snapshot: &CueSnapshot) -> SpatialEffect {
    match snapshot.macro_phase {
        AutoBlinderPhase::BlindersHold => SpatialEffect::Bounce,
        AutoBlinderPhase::OverdriveStrobe => SpatialEffect::Explode,
        AutoBlinderPhase::Recover => SpatialEffect::Chase,
        AutoBlinderPhase::Normal => SpatialEffect::None,
    }
}

/// Compute spatial brightness modulation for a single light.
///
/// Returns a multiplier in [0.15, 1.0] — Hue bulbs look bad fully off.
///
/// `norm_pos`: normalized position [0, 1] along the spatial axis.
///   With v2 position data: derived from (x, y, z).
///   Without position data: falls back to `idx / num_lights`.
fn compute_spatial_modulation(
    effect: SpatialEffect,
    norm_pos: f32,
    snapshot: &CueSnapshot,
) -> f32 {
    const FLOOR: f32 = 0.15;

    match effect {
        SpatialEffect::None => 1.0,

        SpatialEffect::Bounce => {
            // Ping-pong cursor along norm_pos axis
            // Speed ramps with excitement: 0.5 Hz at low → 2.5 Hz at high
            let speed = 0.5 + snapshot.excitement * 2.0;
            // Triangle wave: cursor position 0-1-0 over time
            let phase = (snapshot.frame_time * speed).rem_euclid(2.0);
            let cursor = if phase < 1.0 { phase } else { 2.0 - phase };
            // Gaussian-ish falloff from cursor position (width ~0.25)
            let dist = (norm_pos - cursor).abs();
            let intensity = (-dist * dist / (2.0 * 0.06)).exp(); // sigma^2 = 0.06
            lerp(FLOOR, 1.0, intensity)
        }

        SpatialEffect::Explode => {
            // Center-outward radial burst. Trigger intensity from strobe_env/bass_pump.
            let trigger = snapshot.strobe_env.max(snapshot.bass_pump);
            if trigger < 0.01 {
                return FLOOR;
            }
            // Radial distance from center (norm_pos 0.5 = center)
            let radial = (norm_pos - 0.5).abs() * 2.0; // 0 at center, 1 at edges
            // Expanding wavefront: wavefront_pos moves outward with trigger decay
            // At trigger=1.0, wavefront is at center; as it decays, wavefront expands
            let wavefront = 1.0 - trigger;
            let dist = (radial - wavefront).abs();
            let intensity = (-dist * dist / (2.0 * 0.04)).exp() * trigger; // tight gaussian, scaled by trigger
            lerp(FLOOR, 1.0, intensity)
        }

        SpatialEffect::Chase => {
            // Gentle sequential sweep. Direction from energy_slope (positive = L→R, negative = R→L).
            let speed = 0.3 + snapshot.excitement * 0.5; // Gentle 0.3–0.8 Hz
            let direction = if snapshot.energy_slope >= 0.0 { 1.0 } else { -1.0 };
            let phase = (snapshot.frame_time * speed * direction).rem_euclid(1.0);
            let dist = ((norm_pos - phase).abs()).min((norm_pos - phase + 1.0).abs()).min((norm_pos - phase - 1.0).abs());
            let intensity = (-dist * dist / (2.0 * 0.08)).exp(); // sigma^2 = 0.08, wider for gentle effect
            lerp(FLOOR, 1.0, intensity)
        }
    }
}

/// Get normalized position for a light. Uses v2 position data (x axis) if available,
/// otherwise falls back to index-based ordering.
fn get_normalized_position(assignment: &HueLightAssignment, idx: usize, num_lights: usize) -> f32 {
    if let Some(pos) = assignment.position {
        // v2 position: x ranges from -1 to +1 in the entertainment area
        // Normalize to 0-1
        (pos.x + 1.0) * 0.5
    } else {
        // Fallback: evenly spaced by index
        if num_lights <= 1 { 0.5 } else { idx as f32 / (num_lights - 1) as f32 }
    }
}

/// Get normalized radial distance from center for a light (used by Explode).
/// With v2: uses sqrt(x^2 + y^2). Without: falls back to linear distance from center index.
fn get_normalized_radial(assignment: &HueLightAssignment, idx: usize, num_lights: usize) -> f32 {
    if let Some(pos) = assignment.position {
        // Radial distance from center (0,0) normalized to [0, 1]
        // Position ranges: x [-1,1], y [-1,1], z [-1,1]
        let r = (pos.x * pos.x + pos.y * pos.y).sqrt();
        (r / std::f32::consts::SQRT_2).clamp(0.0, 1.0) // max distance is sqrt(2)
    } else {
        // Fallback: distance from center index
        let center = (num_lights as f32 - 1.0) * 0.5;
        let dist = (idx as f32 - center).abs();
        let max_dist = center.max(0.5);
        (dist / max_dist).clamp(0.0, 1.0)
    }
}

// ============================================================================
// MULTI-BAND FREQUENCY TINTING
// ============================================================================

/// Compute multi-band frequency tinting for a light based on spatial position.
///
/// Maps frequency bands to positions: bass (left) → mids (center) → highs (right).
/// Uses palette-relative hue shifts (warm/cool) instead of hardcoded RGB.
/// Active during Normal phase to add spatial frequency separation across the room.
///
/// Band reactivity is derived from CueSnapshot:
/// - **Bass**: `bass_pump` (kick detection) + energy
/// - **Mids**: `dominant_env` (pitched content) + energy
/// - **Highs**: `strobe_env` (transient energy) + tension (harmonic complexity)
fn compute_multiband_blend(
    base_rgb: (f32, f32, f32),
    norm_pos: f32,
    snapshot: &CueSnapshot,
) -> (f32, f32, f32) {
    // Band reactivity from CueSnapshot (0–1 each)
    let bass = (snapshot.bass_pump + snapshot.energy * 0.2).clamp(0.0, 1.0);
    let mids = (snapshot.dominant_env * 0.5 + snapshot.energy * 0.5).clamp(0.0, 1.0);
    let highs = (snapshot.strobe_env * 0.6 + snapshot.tension * 0.3).clamp(0.0, 1.0);

    // Band weights from position (triangular crossfade, 3 equal sections)
    // norm_pos: 0.0 = far left (bass), 0.5 = center (mids), 1.0 = far right (highs)
    let bass_w = (1.0 - norm_pos * 3.0).clamp(0.0, 1.0);
    let high_w = ((norm_pos - 0.67) * 3.0).clamp(0.0, 1.0);
    let mid_w = (1.0 - bass_w - high_w).max(0.0);

    // Weighted reactivity for this light's position
    let band_react = bass_w * bass + mid_w * mids + high_w * highs;

    // Hue shift: warm for bass-heavy positions, cool for highs-heavy
    // Bass lights → warm shift (-0.08 on color wheel ≈ toward red/amber)
    // High lights → cool shift (+0.12 on color wheel ≈ toward blue/cyan)
    let hue_shift = -0.08 * bass_w + 0.12 * high_w;
    let tinted = rotate_hue(base_rgb, hue_shift);

    // Brightness modulation by band reactivity (gentle for Hue bulbs)
    let bri_mod = 0.7 + 0.3 * band_react;

    (
        tinted.0 * bri_mod,
        tinted.1 * bri_mod,
        tinted.2 * bri_mod,
    )
}

// ============================================================================
// PUBLIC API
// ============================================================================

/// MultiBand mapping — matches HueFlow's MultiBandEffect algorithm.
///
/// Band-to-color mapping:
///   Bass → Red,  Mids → Green,  Highs → Blue
///
/// Spatial assignment:
///   Lights sorted by X position, divided into 3 equal sections (left/center/right).
///   If no spatial data, falls back to channel_id % 3.
///
/// Brightness:
///   brightness = band_value.clamp(0.0, 1.0)  — direct, no scaling, no energy multiply.
///
/// No smoothing, no gamma, no gain — HueManager's interpolation handles easing.
pub fn map_multiband_to_lights(
    snapshot: &CueSnapshot,
    assignments: &[HueLightAssignment],
) -> Vec<HueLightColor> {
    // Periodic diagnostic logging (~every 2s at 50fps)
    static FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let frame = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let diag = frame % 100 == 0;

    if assignments.is_empty() {
        return Vec::new();
    }

    let num_lights = assignments.len();

    // Check if any light has spatial position data
    let has_positions = assignments.iter().any(|a| {
        a.position.as_ref().map_or(false, |p| p.x.abs() > 0.001 || p.y.abs() > 0.001)
    });

    // Band values are pre-normalized 0-1 from AudioAnalysis
    let bass = snapshot.bass.clamp(0.0, 1.0);
    let mid = snapshot.mid.clamp(0.0, 1.0);
    let high = snapshot.high.clamp(0.0, 1.0);

    if diag {
        log::info!(
            "[MultiBand] bass={:.3} mid={:.3} high={:.3} | lights={} spatial={}",
            bass, mid, high, num_lights, has_positions,
        );
    }

    let mut result = vec![HueLightColor::from_f32(0, 0.0, 0.0, 0.0); num_lights];

    if has_positions {
        // Spatial mode: sort by X position, divide into thirds
        let mut sorted: Vec<(usize, f32)> = assignments.iter().enumerate().map(|(idx, a)| {
            let x = get_normalized_position(a, idx, num_lights);
            (idx, x)
        }).collect();
        sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        for (i, &(orig_idx, _x)) in sorted.iter().enumerate() {
            let channel_id = assignments[orig_idx].channel_id;
            // HueFlow partition: section = (i * 3) / count
            let section = if num_lights < 3 { i } else { (i * 3) / num_lights };
            let (r, g, b) = match section {
                0 => (bass, 0.0, 0.0),  // Bass → Red
                1 => (0.0, mid, 0.0),   // Mids → Green
                _ => (0.0, 0.0, high),  // Highs → Blue
            };
            result[orig_idx] = HueLightColor::from_f32(channel_id, r, g, b);
        }
    } else {
        // No position data: assign by channel_id % 3 (HueFlow fallback)
        for (idx, assignment) in assignments.iter().enumerate() {
            let channel_id = assignment.channel_id;
            let (r, g, b) = match channel_id % 3 {
                0 => (bass, 0.0, 0.0),  // Bass → Red
                1 => (0.0, mid, 0.0),   // Mids → Green
                _ => (0.0, 0.0, high),  // Highs → Blue
            };
            result[idx] = HueLightColor::from_f32(channel_id, r, g, b);
        }
    }

    result
}

/// Map a CueSnapshot to RGB colors for each assigned light.
///
/// The `diag_frame` counter enables periodic diagnostic logging (~every 2s at 25fps).
/// Pass the frame counter from HueManager; logging fires when `diag_frame % 50 == 1`.
pub fn map_snapshot_to_lights(
    snapshot: &CueSnapshot,
    assignments: &[HueLightAssignment],
    diag_frame: u64,
) -> Vec<HueLightColor> {
    let diag = diag_frame % 50 == 1;

    // Handle manual triggers first (override everything)
    if snapshot.trigger_blackout {
        if diag { log::info!("[HueMap] OVERRIDE: trigger_blackout → all black"); }
        return assignments.iter().map(|a| {
            HueLightColor::from_f32(a.channel_id, 0.0, 0.0, 0.0)
        }).collect();
    }

    // Manual blinder (user holding button) → red
    let manual_blinder = snapshot.trigger_blinder && snapshot.macro_phase != AutoBlinderPhase::BlindersHold;
    if manual_blinder {
        if diag { log::info!("[HueMap] OVERRIDE: manual_blinder → all red"); }
        return assignments.iter().map(|a| {
            HueLightColor::from_f32(a.channel_id, 1.0, 0.0, 0.0)
        }).collect();
    }

    // Buildup (BlindersHold) → 8Hz white strobe
    if snapshot.macro_phase == AutoBlinderPhase::BlindersHold {
        let strobe_hz = 8.0;
        let strobe_phase = (snapshot.frame_time * strobe_hz).fract();
        let strobe_on = strobe_phase < 0.5;
        let v = if strobe_on { 1.0 } else { 0.05 };
        if diag {
            log::info!("[HueMap] MACRO: Buildup → 8Hz strobe v={:.2}", v);
        }
        return assignments.iter().map(|a| {
            HueLightColor::from_f32(a.channel_id, v, v, v)
        }).collect();
    }

    // UV mode → all black: Hue bulbs have no UV emitter, and UV mode means
    // "only lights that support UV are on". Checked AFTER the blinder paths
    // (a blinder hold beats UV; UV resumes on release) and BEFORE strobe
    // (auto/manual strobes are suppressed during UV mode).
    if snapshot.trigger_uv {
        if diag { log::info!("[HueMap] OVERRIDE: trigger_uv → all black (no UV emitter)"); }
        return assignments.iter().map(|a| {
            HueLightColor::from_f32(a.channel_id, 0.0, 0.0, 0.0)
        }).collect();
    }

    if snapshot.trigger_strobe {
        // Flash on the engine's wall-clock square wave (`strobe_on`, advanced
        // at the operator's strobe rate on the producer's own 100Hz clock) —
        // live with or without audio, at the SAME RATE as the DMX rig (which
        // strobes off its own in-frame clock; relative phase is free-running,
        // imperceptible at typical 8-20Hz given Hue transport latency). The
        // old `strobe_active` read was CueEngine state, which is FROZEN while
        // a manual trigger is held (engine-freeze-by-design), so a manual
        // STROBE rendered a static level on Hue instead of flashing.
        let v = if snapshot.strobe_on { 1.0 } else { 0.0 };
        if diag { log::info!("[HueMap] OVERRIDE: trigger_strobe → white={:.0}", v); }
        return assignments.iter().map(|a| {
            HueLightColor::from_f32(a.channel_id, v, v, v)
        }).collect();
    }

    // Automatic blackout from CueEngine
    if snapshot.blackout_active {
        if diag { log::info!("[HueMap] OVERRIDE: blackout_active → all black"); }
        return assignments.iter().map(|a| {
            HueLightColor::from_f32(a.channel_id, 0.0, 0.0, 0.0)
        }).collect();
    }

    // ── Macro phase effects ─────────────────────────────────────────
    // Pump (OverdriveStrobe) and Cool (Recover) fall through to normal mapping
    // with modified beat_scale via compute_brightness().
    if diag_frame % 25 == 0 && snapshot.macro_phase != AutoBlinderPhase::Normal {
        log::info!(
            "[HueMap] phase={} excitement={:.3} tension_bank={:.3} energy={:.3} bass={:.3}",
            snapshot.macro_phase.as_str(), snapshot.excitement, snapshot.tension_bank, snapshot.energy, snapshot.bass,
        );
    }

    // Core computations (shared across roles)
    let base_color = compute_base_color(snapshot, diag);
    let brightness = compute_brightness(snapshot, diag);

    if diag {
        log::info!(
            "[HueMap] envelopes: strobe_env={:.3} dominant_env={:.3} polarity_env={:.3} | excitement={:.2} frame_t={:.1}",
            snapshot.strobe_env, snapshot.dominant_env, snapshot.polarity_env,
            snapshot.excitement, snapshot.frame_time,
        );
    }

    // Per-light hue offset: spread lights across the color wheel so the room
    // isn't monotone. Each light gets a unique angular offset from the base color.
    let num_lights = assignments.len().max(1) as f32;
    let num_lights_usize = assignments.len();

    // Select spatial effect based on macro phase (None during Normal → multiplier 1.0)
    let spatial_effect = select_spatial_effect(snapshot);
    if diag && spatial_effect != SpatialEffect::None {
        log::info!("[HueMap] Spatial effect: {:?} (phase={:?})", spatial_effect, snapshot.macro_phase);
    }

    assignments.iter().enumerate().map(|(idx, assignment)| {
        // Compute per-light color offset (hue rotation)
        let light_hue_offset = (idx as f32) / num_lights; // evenly spaced 0..1

        let (r, g, b) = match assignment.role {
            LightRole::Wash => compute_wash_color(snapshot, base_color, brightness, light_hue_offset, diag && idx == 0),
            LightRole::Accent => compute_accent_color(snapshot, brightness, idx, light_hue_offset, diag),
            LightRole::Strip => compute_strip_color(snapshot, base_color, brightness, light_hue_offset, diag && idx == 0),
            LightRole::Ambient => compute_ambient_color(snapshot, base_color, light_hue_offset, diag && idx == 0),
        };

        // Spatial effects:
        // Normal phase → multi-band frequency tinting (color + brightness per band)
        // Macro phases → brightness-only spatial modulation (Chase/Bounce/Explode)
        let (r, g, b) = if spatial_effect == SpatialEffect::None {
            let norm_pos = get_normalized_position(assignment, idx, num_lights_usize);
            compute_multiband_blend((r, g, b), norm_pos, snapshot)
        } else {
            let norm_pos = get_normalized_position(assignment, idx, num_lights_usize);
            let spatial = compute_spatial_modulation(spatial_effect, norm_pos, snapshot);
            (r * spatial, g * spatial, b * spatial)
        };

        // Global Hue output scale. Macro phases (blinder, overdrive) return early and bypass this.
        let hue_dim = 0.85_f32;
        let r = r * hue_dim;
        let g = g * hue_dim;
        let b = b * hue_dim;

        // Perceptual gamma correction: Hue bulbs have non-linear perception,
        // so 40% brightness looks like 20%. Gamma < 1.0 lifts mid-range values.
        let gamma = 0.85_f32; // Perceptual correction for Hue bulb non-linearity
        let r = r.powf(gamma);
        let g = g.powf(gamma);
        let b = b.powf(gamma);

        if diag {
            log::info!(
                "[HueMap]   #{} '{}' {:>7} hue_off={:.2} → RGB({:.3}, {:.3}, {:.3})",
                assignment.channel_id, assignment.name, assignment.role.as_str(),
                light_hue_offset, r, g, b,
            );
        }

        HueLightColor::from_f32(assignment.channel_id, r, g, b)
    }).collect()
}

// ============================================================================
// COLOR COMPUTATION
// ============================================================================

/// Compute base color from palette, bias, dominant pitch, and tonality.
fn compute_base_color(snapshot: &CueSnapshot, diag: bool) -> (f32, f32, f32) {
    let palette_rgb = palette_to_rgb(snapshot.palette, snapshot.palette_bias);
    let (mut r, mut g, mut b) = palette_rgb;

    // Tonality shift: major = warmer, minor = cooler
    // Capped at ±0.12 per channel to avoid wrecking the palette color
    let tonality_factor = snapshot.major_smooth;
    if tonality_factor > 0.55 {
        let shift = (tonality_factor - 0.55) * 2.2; // 0–1
        r = (r + shift * 0.12).min(1.0);
        g = (g + shift * 0.04).min(1.0);
        b = (b - shift * 0.10).max(0.0);
    } else if tonality_factor < 0.45 {
        let shift = (0.45 - tonality_factor) * 2.2;
        b = (b + shift * 0.12).min(1.0);
        g = (g + shift * 0.04).min(1.0);
        r = (r - shift * 0.10).max(0.0);
    }
    let post_tonality = (r, g, b);

    // Dominant pitch → hue rotation
    // Only apply when there's real musical energy (zone >= Mid), otherwise palette stays clean.
    // Base influence 0.05 (was 0.15), scaling up to 0.35 at full tension (was 0.45).
    let pitch_hue = pitch_target(snapshot);
    let energy_gate = match snapshot.zone {
        EnergyZone::Silent | EnergyZone::Still => 0.0,
        EnergyZone::Low => 0.3,  // partial
        EnergyZone::Mid => 0.7,
        EnergyZone::High => 1.0,
    };
    let pitch_influence = (0.05 + 0.30 * snapshot.tension) * energy_gate;
    if pitch_influence > 0.001 {
        r = lerp(r, pitch_hue.0, pitch_influence);
        g = lerp(g, pitch_hue.1, pitch_influence);
        b = lerp(b, pitch_hue.2, pitch_influence);
    }

    // Tension → saturation boost (unchanged)
    if snapshot.tension > 0.4 {
        let sat_boost = (snapshot.tension - 0.4) * 0.5;
        let max = r.max(g).max(b);
        if max > 0.0 {
            r = lerp(r, if r == max { 1.0 } else { r * 0.5 }, sat_boost);
            g = lerp(g, if g == max { 1.0 } else { g * 0.5 }, sat_boost);
            b = lerp(b, if b == max { 1.0 } else { b * 0.5 }, sat_boost);
        }
    }

    let final_color = (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0));

    if diag {
        log::info!(
            "[HueMap] base_color: palette={:?}/{:?} raw=({:.2},{:.2},{:.2}) → tonality({:.2})=({:.2},{:.2},{:.2}) → pitch(pc={} inf={:.2})=({:.2},{:.2},{:.2})",
            snapshot.palette, snapshot.palette_bias,
            palette_rgb.0, palette_rgb.1, palette_rgb.2,
            tonality_factor,
            post_tonality.0, post_tonality.1, post_tonality.2,
            snapshot.dominant_pc, pitch_influence,
            final_color.0, final_color.1, final_color.2,
        );
    }

    final_color
}

/// Compute brightness from zone, energy, energy slope, reactivity, and macro phase.
///
/// Zone sets a floor. Energy modulates within that zone's range using a
/// zone-relative formula so brightness never goes negative within any zone.
/// Reactivity applies a guaranteed brightness floor and audio drive gain
/// so lights are never fully off during music, and bass drops always punch.
/// Macro phase modifies beat modulation: Pump = bass³ × 1.5, Cool = 75%.
fn compute_brightness(snapshot: &CueSnapshot, diag: bool) -> f32 {
    let r = snapshot.reactivity;

    // Reactivity-derived scalars
    let base_floor = 0.06 + (0.18 - 0.06) * r; // lerp(0.06, 0.18, r)
    let drive_gain = 0.8 + (1.8 - 0.8) * r;     // lerp(0.8, 1.8, r)

    // Zone floor and ceiling — brightness stays within this band.
    // Silent/Still have a warm glow floor so Hue bulbs never snap fully off
    // (manual blackout trigger handles true darkness).
    let (zone_floor, zone_ceil): (f32, f32) = match snapshot.zone {
        EnergyZone::Silent => (0.03, 0.08),
        EnergyZone::Still  => (0.06, 0.18),
        EnergyZone::Low    => (0.12, 0.50),
        EnergyZone::Mid    => (0.25, 0.80),
        EnergyZone::High   => (0.35, 1.0),
    };

    // Zone thresholds (matching cue_engine defaults for mapping energy → 0-1 within zone)
    let (zone_energy_lo, zone_energy_hi): (f32, f32) = match snapshot.zone {
        EnergyZone::Silent => (0.0, 0.02),
        EnergyZone::Still  => (0.02, 0.15),
        EnergyZone::Low    => (0.15, 0.40),
        EnergyZone::Mid    => (0.40, 0.70),
        EnergyZone::High   => (0.70, 1.0),
    };

    // Normalize energy within the zone's own range → [0, 1]
    let range = (zone_energy_hi - zone_energy_lo).max(0.01);
    let zone_t = ((snapshot.energy - zone_energy_lo) / range).clamp(0.0, 1.0);

    // Interpolate between floor and ceiling
    let zone_base = lerp(zone_floor, zone_ceil, zone_t);

    // Energy slope anticipation: build → brighten, drop → dim (±15%)
    let slope_mod = snapshot.energy_slope * 0.15;

    // Tension-aware brightness boost: lifts mid-energy passages when musical tension is high
    let tension_boost = if snapshot.tension > 0.5 {
        (snapshot.tension - 0.5) * 0.24  // max +0.12 at tension=1.0
    } else {
        0.0
    };

    let zone_result = (zone_base + slope_mod + tension_boost).clamp(0.0, 1.0);

    // Reactivity floor: guaranteed minimum brightness from audio energy.
    // Capped low so normal operation stays cinematic/dim — macro phases handle the bright moments.
    let react_floor = (base_floor + drive_gain * snapshot.energy * 0.20).clamp(base_floor, 0.35);

    // Take the max of zone-based brightness and reactivity floor
    let mut result = zone_result.max(react_floor);

    // Bass pump: kick-triggered brightness boost
    if snapshot.bass_pump > 0.01 {
        result = (result + snapshot.bass_pump * 0.18).min(1.0);
    }

    // Raw audio beat modulation: multiplicative scaling from raw bass energy.
    // The percentile normalizer keeps bass centered ~0.4-0.6, so we re-center:
    // subtract the midpoint and amplify so beats punch to 1.0 and gaps hit 0.0.
    let bass_centered = ((snapshot.bass - 0.35) * 2.5).clamp(0.0, 1.0);
    // Pump (OverdriveStrobe): bass³ × 1.5 for aggressive pumping
    // Cool (Recover): 75% brightness ease-back
    // Normal / Buildup: default swing (Buildup returns early before reaching here)
    let beat_scale = match snapshot.macro_phase {
        AutoBlinderPhase::OverdriveStrobe => {
            // Pump: brightness IS the bass, cubed for razor-sharp punch.
            // Cubing makes peaks pop and valleys go nearly black.
            // Boosted 1.5x overall so the drop feels BRIGHT + punchy.
            let b = snapshot.bass.clamp(0.0, 1.0);
            ((b * b * b) * 1.5).clamp(0.10, 1.0)
        }
        AutoBlinderPhase::Recover => {
            // Cool: easing down from Pump brightness. Still responsive but dimmer.
            let scale = 0.05 + 0.95 * bass_centered;
            scale * 0.75 // 75% of full brightness — noticeably calmer than Pump
        }
        _ => 0.10 + 0.90 * bass_centered,  // 0.10–1.0: big default swing
    };
    result *= beat_scale;

    let result = result.clamp(0.0, 1.0);

    if diag {
        log::info!(
            "[HueMap] brightness: zone={:?} energy={:.3} zone_t={:.2} zone_base={:.3} slope={:.3} tension={:.3} zone_bri={:.3} react_floor={:.3} bass_pump={:.3} bass={:.3} beat_scale={:.3} → {:.3} (reactivity={:.2})",
            snapshot.zone, snapshot.energy, zone_t, zone_base,
            snapshot.energy_slope, tension_boost, zone_result,
            react_floor, snapshot.bass_pump, snapshot.bass, beat_scale, result, r,
        );
    }

    result
}

// ============================================================================
// PER-ROLE COLOR FUNCTIONS
// ============================================================================

/// Wash: palette + zone + energy + breathing + strobe bloom + per-light hue offset
fn compute_wash_color(snapshot: &CueSnapshot, base_color: (f32, f32, f32), brightness: f32, hue_offset: f32, diag: bool) -> (f32, f32, f32) {
    // Apply per-light hue rotation for room color variety
    let (br, bg, bb) = rotate_hue(base_color, hue_offset * 0.35); // ±35% spread

    // Breathing: smooth and restrained — Hue bulbs look choppy with big jumps.
    let wash_brightness = if snapshot.excitement < 0.4 {
        // Low excitement: gentle sine breathing (~5s cycle, smooth)
        let cycle = (snapshot.frame_time * 0.20 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        let breath_range = 0.06_f32.max(brightness * 0.15);
        (brightness - breath_range * 0.5 + breath_range * cycle).clamp(0.0, 1.0)
    } else {
        // Active: modest energy-reactive lift
        (brightness + snapshot.energy * 0.15).min(1.0)
    };

    // Drop impact: scale minimum brightness by impact level (smooth decay, not hard cutoff)
    let impact_floor = 0.75 * snapshot.drop_impact;
    let wash_brightness = wash_brightness.max(impact_floor);

    let mut rgb = (br * wash_brightness, bg * wash_brightness, bb * wash_brightness);

    // Strobe envelope: gentle desaturated lift (Hue can't strobe like DMX)
    if snapshot.strobe_env > 0.01 {
        let amt = (snapshot.strobe_env * 0.55).clamp(0.0, 1.0); // Restrained for Hue
        let lifted = strobe_lift_color(rgb, amt);
        rgb = (
            lerp(rgb.0, lifted.0, amt),
            lerp(rgb.1, lifted.1, amt),
            lerp(rgb.2, lifted.2, amt),
        );
    }

    rgb = enforce_perceptual_floor(snapshot.zone, rgb);

    if diag {
        log::info!(
            "[HueMap]   wash: brightness={:.3} wash_bri={:.3} strobe_env={:.3} drop_impact={:.2}",
            brightness, wash_brightness, snapshot.strobe_env, snapshot.drop_impact,
        );
    }

    rgb
}

/// Accent: complementary colors, dominant pitch reactive, per-light offset
fn compute_accent_color(snapshot: &CueSnapshot, brightness: f32, light_index: usize, hue_offset: f32, diag: bool) -> (f32, f32, f32) {
    let base = compute_base_color(snapshot, false);

    // Accents use complementary hue (180° + per-light offset) for color contrast
    let complementary = rotate_hue(base, 0.5 + hue_offset * 0.25);

    // Accent brightness: modest lift above base for Hue
    let accent_bri = (brightness * 1.20 + 0.04).min(1.0);

    // Drop impact: scale minimum brightness by impact level
    let impact_floor = 0.80 * snapshot.drop_impact;
    let accent_bri = accent_bri.max(impact_floor);

    let mut rgb = (complementary.0 * accent_bri, complementary.1 * accent_bri, complementary.2 * accent_bri);

    // Strobe envelope: moderate lift (Hue accents shouldn't flash hard)
    if snapshot.strobe_env > 0.01 {
        let amt = (snapshot.strobe_env * 0.60).clamp(0.0, 1.0);
        let lifted = strobe_lift_color(rgb, amt);
        rgb = (
            lerp(rgb.0, lifted.0, amt),
            lerp(rgb.1, lifted.1, amt),
            lerp(rgb.2, lifted.2, amt),
        );
    }

    // Dominant pitch envelope: stronger influence on accents (they're the "reactive" lights)
    if snapshot.dominant_env > 0.01 {
        let (pr, pg, pb) = pitch_target(snapshot);
        let amt = (snapshot.dominant_env * 0.70).clamp(0.0, 1.0); // Boosted from 0.55
        rgb = (lerp(rgb.0, pr, amt), lerp(rgb.1, pg, amt), lerp(rgb.2, pb, amt));
    }

    // Polarity envelope: warm tint on quadrant-matched accents
    if snapshot.polarity_env > 0.01 {
        let quadrant = snapshot.polarity_quadrant;
        let my_quadrant = (light_index % 4) as u8;
        if my_quadrant == quadrant {
            let amt = (snapshot.polarity_env * 0.55).clamp(0.0, 1.0); // Boosted from 0.45
            let warm = (1.0, 0.92, 0.78);
            rgb = (lerp(rgb.0, warm.0, amt), lerp(rgb.1, warm.1, amt), lerp(rgb.2, warm.2, amt));
        }
    }

    rgb = enforce_perceptual_floor(snapshot.zone, rgb);

    if diag && light_index == 0 {
        log::info!(
            "[HueMap]   accent: bri={:.3} strobe_env={:.3} dominant_env={:.3}(pc={}) pol_env={:.3}(q={}) drop_impact={:.2}",
            accent_bri, snapshot.strobe_env, snapshot.dominant_env, snapshot.dominant_pc,
            snapshot.polarity_env, snapshot.polarity_quadrant, snapshot.drop_impact,
        );
    }

    (rgb.0.clamp(0.0, 1.0), rgb.1.clamp(0.0, 1.0), rgb.2.clamp(0.0, 1.0))
}

/// Strip: architectural underglow with per-light hue offset + kick punch
fn compute_strip_color(snapshot: &CueSnapshot, base_color: (f32, f32, f32), brightness: f32, hue_offset: f32, diag: bool) -> (f32, f32, f32) {
    // Strips get a subtle hue shift for color variation along the strip
    let (br, bg, bb) = rotate_hue(base_color, hue_offset * 0.20); // ±20% spread (subtle)

    // Breathing: smooth and restrained for strip underglow
    let cycle = (snapshot.frame_time * 0.18 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    let breath_range = 0.05_f32.max(brightness * 0.10);
    let base_bri = if snapshot.excitement < 0.45 {
        (brightness - breath_range * 0.5 + breath_range * cycle).clamp(0.0, 1.0)
    } else {
        (brightness + snapshot.energy * 0.14).min(1.0)
    };

    let base_bri = base_bri.max(0.70 * snapshot.drop_impact);

    let mut rgb = (br * base_bri, bg * base_bri, bb * base_bri);

    // Kick punch: restrained for Hue strips
    if snapshot.strobe_env > 0.01 {
        let amt = (snapshot.strobe_env * 0.45).clamp(0.0, 1.0);
        let lifted = strobe_lift_color(rgb, amt);
        rgb = (
            lerp(rgb.0, lifted.0, amt),
            lerp(rgb.1, lifted.1, amt),
            lerp(rgb.2, lifted.2, amt),
        );
    }

    // Dominant pitch: moderate hue bloom
    if snapshot.dominant_env > 0.01 {
        let (pr, pg, pb) = pitch_target(snapshot);
        let amt = (snapshot.dominant_env * 0.45).clamp(0.0, 1.0); // Boosted from 0.35
        rgb = (lerp(rgb.0, pr, amt), lerp(rgb.1, pg, amt), lerp(rgb.2, pb, amt));
    }

    // Polarity: warm/cool tint
    if snapshot.polarity_env > 0.01 {
        let amt = (snapshot.polarity_env * 0.30).clamp(0.0, 1.0); // Boosted from 0.25
        let warm = (1.0, 0.92, 0.78);
        rgb = (lerp(rgb.0, warm.0, amt), lerp(rgb.1, warm.1, amt), lerp(rgb.2, warm.2, amt));
    }

    rgb = enforce_perceptual_floor(snapshot.zone, rgb);

    if diag {
        log::info!(
            "[HueMap]   strip: base_bri={:.3} strobe_env={:.3} dom_env={:.3} pol_env={:.3}",
            base_bri, snapshot.strobe_env, snapshot.dominant_env, snapshot.polarity_env,
        );
    }

    (rgb.0.clamp(0.0, 1.0), rgb.1.clamp(0.0, 1.0), rgb.2.clamp(0.0, 1.0))
}

/// Ambient: never fully off, slow breathing, warm-shifted + per-light offset
fn compute_ambient_color(snapshot: &CueSnapshot, base_color: (f32, f32, f32), hue_offset: f32, diag: bool) -> (f32, f32, f32) {
    // Ambient lights get a warm-shifted hue with per-light offset for variety
    let warmed = rotate_hue(base_color, hue_offset * 0.15 - 0.05); // Slight warm shift
    let (br, bg, bb) = warmed;

    let ambient_brightness = if snapshot.excitement < 0.3 {
        // Slow breathing sine wave (~3.3s cycle), gentle atmospheric glow
        let breath_cycle = (snapshot.frame_time * 0.3 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        lerp(0.10, 0.22, breath_cycle)
    } else {
        // Active: modest bass-reactive lift
        0.18 + 0.30 * snapshot.energy
    };

    let ambient_brightness = ambient_brightness.max(0.60 * snapshot.drop_impact);

    if diag {
        log::info!(
            "[HueMap]   ambient: bri={:.3} excitement={:.2} drop_impact={:.2}",
            ambient_brightness, snapshot.excitement, snapshot.drop_impact,
        );
    }

    let rgb = (br * ambient_brightness, bg * ambient_brightness, bb * ambient_brightness);
    enforce_perceptual_floor(snapshot.zone, rgb)
}

// ============================================================================
// COLOR HELPERS
// ============================================================================

/// Map Palette + PaletteBias to base RGB color
fn palette_to_rgb(palette: Palette, bias: PaletteBias) -> (f32, f32, f32) {
    match (palette, bias) {
        // ICE palette (sparse/ambient)
        (Palette::Ice, PaletteBias::Cool) =>    (0.6, 0.75, 1.0),
        (Palette::Ice, PaletteBias::Warm) =>    (0.85, 0.15, 0.45),
        (Palette::Ice, PaletteBias::Neutral) => (0.85, 0.85, 0.85),
        (Palette::Ice, PaletteBias::Vivid) =>   (0.9, 0.4, 0.8),

        // COOL_A palette (moderate activity)
        (Palette::CoolA, PaletteBias::Cool) =>    (0.3, 0.7, 1.0),
        (Palette::CoolA, PaletteBias::Warm) =>    (0.75, 0.10, 0.60),
        (Palette::CoolA, PaletteBias::Neutral) => (0.7, 0.7, 0.75),
        (Palette::CoolA, PaletteBias::Vivid) =>   (0.2, 0.4, 1.0),

        // COOL_B palette (dense/saturated)
        (Palette::CoolB, PaletteBias::Cool) =>    (0.15, 0.5, 1.0),
        (Palette::CoolB, PaletteBias::Warm) =>    (0.55, 0.05, 0.85),
        (Palette::CoolB, PaletteBias::Neutral) => (0.5, 0.5, 0.55),
        (Palette::CoolB, PaletteBias::Vivid) =>   (0.6, 0.1, 1.0),
    }
}

/// Map dominant pitch class (0–11) to color wheel position
fn dominant_pitch_to_hue(pc: u8) -> (f32, f32, f32) {
    let hue = (pc as f32) / 12.0;
    hsv_to_rgb(hue, 0.8, 0.9)
}

/// The pitch colour the rig keys off this frame: the single dominant pitch
/// (default / "Dom" checked), or the all-notes chromatic blend when the
/// operator un-checks "Dom" (spread chord washes whiter via `all_notes_sat`).
fn pitch_target(snapshot: &CueSnapshot) -> (f32, f32, f32) {
    if snapshot.all_notes_color {
        hsv_to_rgb(snapshot.all_notes_hue, 0.8 * snapshot.all_notes_sat, 0.9)
    } else {
        dominant_pitch_to_hue(snapshot.dominant_pc)
    }
}

/// Convert HSV (0–1 range) to RGB (0–1 range)
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

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Rotate the hue of an RGB color by a fraction of the color wheel (0-1).
/// Preserves saturation and value, only changes hue.
fn rotate_hue(rgb: (f32, f32, f32), offset: f32) -> (f32, f32, f32) {
    if offset.abs() < 0.001 {
        return rgb;
    }
    let (h, s, v) = rgb_to_hsv(rgb.0, rgb.1, rgb.2);
    let new_h = (h + offset).rem_euclid(1.0);
    hsv_to_rgb(new_h, s, v)
}

/// Convert RGB (0–1 range) to HSV (0–1 range)
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let v = max;
    if max == 0.0 {
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
    let h = (h / 6.0).rem_euclid(1.0);

    (h, s, v)
}

/// Desaturated base lift for strobe bloom — never pure white, always tinted.
fn strobe_lift_color(rgb: (f32, f32, f32), amt: f32) -> (f32, f32, f32) {
    let (h, s, v) = rgb_to_hsv(rgb.0, rgb.1, rgb.2);
    let s = lerp(s, 0.18, amt);
    let v = (v + amt * 0.4).min(1.0);
    hsv_to_rgb(h, s, v)
}

/// Prevent Hue bulbs from going invisible/gray at low luma.
fn enforce_perceptual_floor(zone: EnergyZone, rgb: (f32, f32, f32)) -> (f32, f32, f32) {
    let (mut r, mut g, mut b) = rgb;
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;

    let floor = match zone {
        EnergyZone::Silent => 0.0,
        EnergyZone::Still  => 0.04,
        EnergyZone::Low    => 0.06,
        EnergyZone::Mid    => 0.03,
        EnergyZone::High   => 0.02,
    };

    if floor > 0.0 && luma < floor {
        if luma > 1e-6 {
            let scale = floor / luma;
            r *= scale;
            g *= scale;
            b *= scale;
        } else {
            // No color info — emit neutral at floor level
            r = floor;
            g = floor;
            b = floor;
        }
    }

    // In Still/Low zones, prevent gray wash by boosting the dominant channel
    if matches!(zone, EnergyZone::Still | EnergyZone::Low) {
        let (_, s, _) = rgb_to_hsv(r, g, b);
        if s < 0.15 {
            let boost = 0.04;
            if r >= g && r >= b {
                r = (r + boost).min(1.0);
            } else if g >= r && g >= b {
                g = (g + boost).min(1.0);
            } else {
                b = (b + boost).min(1.0);
            }
        }
    }

    (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}
