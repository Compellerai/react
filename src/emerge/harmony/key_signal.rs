//! Fallback key/harmonic signal derived from AudioAnalysis.
//!
//! When HarmonicGeometry's KeyDetector is not available, this module provides
//! a renderer-independent key signal with latching, hysteresis, and hold timers.
//! This ensures lighting runs coherently in any visualizer mode.
//!
//! ## Design
//!
//! - **KeySignal**: Lightweight output struct carrying the same data that CueEngine
//!   and OSC continuous output need. Built from either HG's KeyDetector (premium)
//!   or KeySignalState (fallback).
//!
//! - **KeySignalState**: Stateful fallback processor. Derives dominant pitch class
//!   from chroma with latching (no jitter), polarity from spectral mood with wide
//!   hysteresis, and harmonic label from tonal confidence.

use crate::audio::AudioAnalysis;
use crate::render::{KeyDetector, HarmonicLabel};

// ============================================================================
// KEY SIGNAL (lightweight output)
// ============================================================================

/// Key/harmonic data consumed by CueEngine and OSC output.
/// Populated from either HG's KeyDetector or the audio fallback.
#[derive(Clone, Copy, Debug)]
pub struct KeySignal {
    pub harmonic_label: HarmonicLabel,
    pub key_root: u8,
    pub confidence: f32,
    pub major_likelihood: f32,
    pub minor_likelihood: f32,
}

impl KeySignal {
    /// Build from HG's real KeyDetector (premium source)
    pub fn from_key_detector(kd: &KeyDetector) -> Self {
        Self {
            harmonic_label: kd.harmonic_label(),
            key_root: kd.key_root(),
            confidence: kd.confidence(),
            major_likelihood: kd.major_likelihood(),
            minor_likelihood: kd.minor_likelihood(),
        }
    }

    /// Build from fallback state (audio-derived)
    pub fn from_fallback(state: &KeySignalState) -> Self {
        let harmonic_label = if state.is_tonal_smooth > 0.6 && state.dominant_pc_conf > 0.4 {
            HarmonicLabel::Stable
        } else if state.is_tonal_smooth > 0.3 {
            HarmonicLabel::Ambiguous
        } else {
            HarmonicLabel::Atonal
        };

        Self {
            harmonic_label,
            key_root: state.dominant_pc_latched,
            confidence: state.dominant_pc_conf,
            major_likelihood: if state.polarity_warm { 0.65 } else { 0.35 },
            minor_likelihood: if state.polarity_warm { 0.35 } else { 0.65 },
        }
    }
}

// ============================================================================
// KEY SIGNAL STATE (fallback processor with latching)
// ============================================================================

/// Latch thresholds
const TONAL_THRESH: f32 = 0.40;
const STABILITY_THRESH: f32 = 0.25;
const STRENGTH_THRESH: f32 = 0.20;
const ENERGY_GATE: f32 = 0.05;

/// Latch switch parameters
const SWITCH_FLOOR: f32 = 0.30;     // confidence must drop below this
const MIN_HOLD_SECS: f32 = 0.80;    // 800ms minimum hold before allowing switch
const CONFIRM_SECS: f32 = 0.15;     // 150ms candidate must persist

/// Polarity parameters
const POLARITY_MIN_HOLD: f32 = 3.0; // 3s minimum between polarity flips
const POLARITY_STAB_THRESH: f32 = 0.40;

/// Fallback key signal state with latching and hold timers.
/// Updated every frame from AudioAnalysis. Provides stable, jitter-free
/// dominant pitch class and polarity even from noisy chroma input.
pub struct KeySignalState {
    // === Latched dominant pitch class ===
    /// Current latched dominant pitch class (0-11)
    pub dominant_pc_latched: u8,
    /// Confidence in current latch (0-1)
    pub dominant_pc_conf: f32,
    /// Seconds since last latch change
    dominant_pc_hold: f32,

    // === Candidate tracking for latch switch ===
    candidate_pc: u8,
    candidate_persist: f32,

    // === Polarity (warm/cool mood proxy) ===
    /// true = major-ish (warm), false = minor-ish (cool)
    pub polarity_warm: bool,
    polarity_hold: f32,

    // === Smoothed tonal signals ===
    pub is_tonal_smooth: f32,
    stability_smooth: f32,

    // === One-frame event pulses ===
    /// True for one frame when dominant_pc latch switches
    pub dominant_changed_pulse: bool,
    /// True for one frame when polarity latch flips
    pub polarity_changed_pulse: bool,
}

impl KeySignalState {
    pub fn new() -> Self {
        Self {
            dominant_pc_latched: 0,
            dominant_pc_conf: 0.0,
            dominant_pc_hold: 0.0,
            candidate_pc: 0,
            candidate_persist: 0.0,
            polarity_warm: false,
            polarity_hold: 0.0,
            is_tonal_smooth: 0.0,
            stability_smooth: 0.5,
            dominant_changed_pulse: false,
            polarity_changed_pulse: false,
        }
    }

    /// Update fallback key signal from audio analysis.
    /// Call once per frame. Produces latched, stable key data.
    pub fn update(&mut self, analysis: &AudioAnalysis, dt: f32) {
        // Clear one-frame pulses from previous frame
        self.dominant_changed_pulse = false;
        self.polarity_changed_pulse = false;

        // Smooth tonal signals
        let tonal_target = if analysis.is_tonal { 1.0 } else { 0.0 };
        self.is_tonal_smooth += (tonal_target - self.is_tonal_smooth) * 0.08;
        self.stability_smooth += (analysis.stability - self.stability_smooth) * 0.06;

        // Increment hold timers
        self.dominant_pc_hold += dt;
        self.polarity_hold += dt;

        // === DOMINANT PC LATCH ===
        let (pc_candidate, pc_strength) = chroma_dominant(&analysis.chroma);

        // Gate on RAW energy, never `energy_vis`: the adaptive normalizer lifts
        // the quiet-input floor of `energy_vis` to ~0.5, so a gate at 0.05 was
        // effectively always open and tonal bleed (hum, monitor spill, fade
        // tails) could march the latch around between songs. `energy` is the
        // raw RMS field whose documented range (0.02-0.15) the 0.05 constant
        // was written for.
        let valid = self.is_tonal_smooth > TONAL_THRESH
            && self.stability_smooth > STABILITY_THRESH
            && pc_strength > STRENGTH_THRESH
            && analysis.energy > ENERGY_GATE;

        if valid {
            if pc_candidate == self.dominant_pc_latched {
                // Same as current latch: increase confidence
                self.dominant_pc_conf = (self.dominant_pc_conf + 0.05 * dt * 60.0).min(1.0);
                self.candidate_persist = 0.0;
            } else {
                // Different: track candidate
                if pc_candidate == self.candidate_pc {
                    self.candidate_persist += dt;
                } else {
                    self.candidate_pc = pc_candidate;
                    self.candidate_persist = 0.0;
                }

                // Decay current confidence
                self.dominant_pc_conf = (self.dominant_pc_conf - 0.02 * dt * 60.0).max(0.0);

                // Switch latch when confidence dropped, hold expired, candidate confirmed
                if self.dominant_pc_conf < SWITCH_FLOOR
                    && self.dominant_pc_hold > MIN_HOLD_SECS
                    && self.candidate_persist > CONFIRM_SECS
                {
                    self.dominant_pc_latched = self.candidate_pc;
                    self.dominant_pc_conf = pc_strength;
                    self.dominant_pc_hold = 0.0;
                    self.candidate_persist = 0.0;
                    self.dominant_changed_pulse = true;

                    log::debug!(
                        "[KeySignal] Dominant latch: pc={} conf={:.2}",
                        self.dominant_pc_latched, self.dominant_pc_conf
                    );
                }
            }
        } else {
            // Not valid (silence, noise, atonal): freeze state, no drift
            self.candidate_persist = 0.0;
        }

        // === POLARITY (warm/cool mood proxy) ===
        // Based on major vs minor third interval from the latched dominant.
        // Wide hysteresis and long hold — polarity flips should be rare.
        if valid
            && self.polarity_hold > POLARITY_MIN_HOLD
            && self.stability_smooth > POLARITY_STAB_THRESH
        {
            let root = self.dominant_pc_latched as usize;
            let major_third = analysis.chroma[(root + 4) % 12];
            let minor_third = analysis.chroma[(root + 3) % 12];

            // Only flip if the difference is clear (hysteresis)
            let diff = major_third - minor_third;
            let should_be_warm = diff > 0.08;  // clear major bias
            let should_be_cool = diff < -0.08; // clear minor bias

            if should_be_warm && !self.polarity_warm {
                self.polarity_warm = true;
                self.polarity_hold = 0.0;
                self.polarity_changed_pulse = true;
                log::debug!("[KeySignal] Polarity: cool → warm");
            } else if should_be_cool && self.polarity_warm {
                self.polarity_warm = false;
                self.polarity_hold = 0.0;
                self.polarity_changed_pulse = true;
                log::debug!("[KeySignal] Polarity: warm → cool");
            }
        }
    }
}

// ============================================================================
// HELPERS
// ============================================================================

/// Find dominant pitch class and its strength from chroma.
/// Returns (pc: u8, strength: f32 0-1).
fn chroma_dominant(chroma: &[f32; 12]) -> (u8, f32) {
    let mut max_idx = 0usize;
    let mut max_val = 0.0f32;
    let mut runner_up = 0.0f32;

    for (i, &v) in chroma.iter().enumerate() {
        if v > max_val {
            runner_up = max_val;
            max_val = v;
            max_idx = i;
        } else if v > runner_up {
            runner_up = v;
        }
    }

    // Strength: how much the dominant stands out from the runner-up
    let strength = ((max_val - runner_up) * 3.0).clamp(0.0, 1.0);
    (max_idx as u8, strength)
}
