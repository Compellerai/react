//! Key Detection and Chroma Analysis
//!
//! Detects the musical key (root note) from chroma energy over multiple bars.
//! Key changes are slow and deliberate - we don't chase peaks.
//!
//! ## Architecture
//!
//! - **Chroma**: 12-bin pitch class energy (C, C#, D, ... B)
//! - **Key Root**: Stable tonic (0-11), only changes after N beats of confidence
//! - **Held Dominant**: Latched dominant pitch class with hysteresis (UI/shader-friendly)
//! - **Pitch Bias**: Current dominant note relative to key (nudges, doesn't steer)
//!
//! ## Dominant Pitch Latching
//!
//! The held dominant only changes when:
//! 1. New dominant confidence is SWITCH_BOOST (25%) better than held
//! 2. New dominant has been pending for SWITCH_HOLD_SECS (0.6s)
//! 3. New dominant passes MIN_DOM_PCT (0.22) threshold
//!
//! This prevents "jittery" dominant switches when harmony is stable but multi-centered.
//!
//! ## Harmonic Labels
//!
//! - **Stable**: confidence >= 0.25 (clear dominant)
//! - **Rotating**: 0.12 <= confidence < 0.25 (multi-lobe harmony, not ambiguous)
//! - **Ambiguous**: confidence < 0.12 (no clear center)
//! - **Atonal**: dominance < 0.20 with high entropy (noise/silence)
//!
//! ## Tuning Support
//!
//! - 440 Hz: Standard concert pitch
//! - 432 Hz: Alternative tuning (optional offset in chroma extraction)

/// Tuning reference for chroma extraction
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuningReference {
    /// Standard concert pitch (A4 = 440 Hz)
    Hz440,
    /// Alternative tuning (A4 = 432 Hz)
    Hz432,
}

impl Default for TuningReference {
    fn default() -> Self {
        TuningReference::Hz440
    }
}

impl TuningReference {
    /// Get the frequency offset in semitones for chroma extraction
    /// 432 Hz is approximately 0.318 semitones flat from 440 Hz
    pub fn semitone_offset(&self) -> f32 {
        match self {
            TuningReference::Hz440 => 0.0,
            TuningReference::Hz432 => -0.318, // 12 * log2(432/440)
        }
    }

    /// Get A4 frequency
    pub fn a4_frequency(&self) -> f32 {
        match self {
            TuningReference::Hz440 => 440.0,
            TuningReference::Hz432 => 432.0,
        }
    }

    /// Apply tuning correction to a chroma array by shifting bins
    /// The analyzer assumes 440Hz, so for 432Hz we shift to compensate
    pub fn apply_to_chroma(&self, chroma: &[f32; 12]) -> [f32; 12] {
        let offset = -self.semitone_offset(); // Negate: 432Hz offset is -0.318, we shift +0.318
        if offset.abs() < 0.001 {
            return *chroma; // No shift needed for 440Hz
        }

        let mut shifted = [0.0f32; 12];
        for i in 0..12 {
            // Source position with fractional offset
            let src = (i as f32 - offset).rem_euclid(12.0);
            let src_low = src.floor() as usize % 12;
            let src_high = (src_low + 1) % 12;
            let frac = src - src.floor();

            // Linear interpolation between adjacent bins
            shifted[i] = chroma[src_low] * (1.0 - frac) + chroma[src_high] * frac;
        }
        shifted
    }
}

// ============================================================================
// DOMINANT PITCH LATCHING CONSTANTS
// ============================================================================

/// New dominant must be this much stronger (multiplicative) than held to trigger switch
/// 1.25 = 25% better confidence required
const DOM_SWITCH_BOOST: f32 = 1.0;  // Was 1.25 - no boost required, just different

/// Seconds the new dominant must remain pending before switching
const DOM_SWITCH_HOLD_SECS: f32 = 0.25;  // Was 0.6 - faster switching

/// Minimum dominance (top1 value) required to consider switching
/// Prevents switching on weak signals
const DOM_MIN_PCT: f32 = 0.15;  // Was 0.22 - lower threshold for responsiveness

// ============================================================================
// CHROMA FIFO BUFFER CONSTANTS
// ============================================================================

/// Size of the chroma FIFO ring buffer (frames)
/// At ~47fps, 30 frames = ~0.6 seconds of history
/// Reduced from 120 (2.5s) for more responsive note tracking during sustain
const CHROMA_BUFFER_SIZE: usize = 30;

// ============================================================================
// HARMONIC LABEL THRESHOLDS
// ============================================================================

/// Confidence threshold for "Stable Harmony" label
pub const LABEL_STABLE_THRESHOLD: f32 = 0.25;

/// Confidence threshold for "Rotating Harmony" label (multi-lobe, not ambiguous)
pub const LABEL_ROTATING_THRESHOLD: f32 = 0.12;

/// Below this = "Ambiguous" (no clear center)
/// Atonal is when dominance < 0.20 AND confidence is low

/// Harmonic state label based on confidence and dominance
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarmonicLabel {
    /// Clear dominant pitch class (confidence >= 0.25)
    Stable,
    /// Multi-lobe harmony, rotating center (0.12 <= confidence < 0.25)
    Rotating,
    /// No clear harmonic center (confidence < 0.12)
    Ambiguous,
    /// No pitched content / noise / silence
    Atonal,
}

impl HarmonicLabel {
    /// Get display string for UI
    pub fn as_str(&self) -> &'static str {
        match self {
            HarmonicLabel::Stable => "Stable Harmony",
            HarmonicLabel::Rotating => "Rotating Harmony",
            HarmonicLabel::Ambiguous => "Ambiguous",
            HarmonicLabel::Atonal => "Atonal Field",
        }
    }
}

/// Key detection with slow hysteresis
///
/// Only changes key when:
/// 1. Confidence is high (above threshold)
/// 2. Same key wins for N consecutive beats
#[derive(Clone, Debug)]
pub struct KeyDetector {
    /// Current detected key root (0-11, where 0=C, 1=C#, etc.) - HELD (stable)
    key_root: u8,

    /// Confidence in current key (0.0 - 1.0) - margin-based
    confidence: f32,

    /// Smoothed chroma bins (12 pitch classes)
    chroma: [f32; 12],

    /// Display-ready chroma (mean-subtracted, normalized)
    display_chroma: [f32; 12],

    /// Average chroma energy (for bias calculation)
    avg_chroma: f32,

    /// Candidate key (waiting for confirmation) - LIVE (what we're seeing now)
    candidate_key: u8,

    /// How many beats the candidate has been winning
    candidate_beats: u32,

    /// Beats required to confirm a key change
    confirmation_beats: u32,

    /// Confidence threshold for key changes
    confidence_threshold: f32,

    /// Tuning reference
    tuning: TuningReference,

    /// Beat accumulator for tracking beats
    beat_accumulator: f32,

    /// Last beat phase (to detect beat crossings)
    last_beat_phase: f32,

    /// Whether we're in silence (RMS below threshold)
    is_silent: bool,

    /// Silence threshold for noise gate
    silence_threshold: f32,

    /// Second strongest pitch class (for margin calculation)
    second_strongest: u8,

    /// Major scale likelihood (0.0 - 1.0)
    major_likelihood: f32,

    /// Minor scale likelihood (0.0 - 1.0)
    minor_likelihood: f32,

    /// Tonal energy (0.0 - 1.0) - "Do we have pitched content?"
    /// RMS of chroma bins, indicates presence of tonal material
    tonal_energy: f32,

    /// Key strength (0.0 - 1.0) - "Is one bin clearly winning?"
    /// The normalized value of the top pitch class
    key_strength: f32,

    // ========================================================================
    // DOMINANT PITCH LATCHING STATE
    // ========================================================================

    /// Held dominant pitch class (0-11) - LATCHED, only changes with hysteresis
    /// This is what UI and shaders should read, NOT the raw argmax
    held_dom_pc: u8,

    /// Confidence when held dominant was latched
    held_dom_confidence: f32,

    /// Pending new dominant pitch class (waiting for confirmation)
    pending_dom_pc: u8,

    /// Accumulated time (seconds) that pending_dom_pc has been the raw winner
    pending_dom_time: f32,

    /// Raw dominant pitch class (instant argmax, NOT latched)
    /// For debugging/visualization of what the detector sees before latching
    raw_dom_pc: u8,

    // ========================================================================
    // CHROMA FIFO RING BUFFER
    // ========================================================================

    /// Ring buffer of chroma frames for FIFO averaging
    /// Each entry is a 12-bin chroma snapshot
    chroma_buffer: Vec<[f32; 12]>,

    /// Current write position in the ring buffer
    chroma_buffer_pos: usize,

    /// Number of valid frames in the buffer (for startup)
    chroma_buffer_count: usize,
}

impl Default for KeyDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyDetector {
    pub fn new() -> Self {
        Self {
            key_root: 0,  // Start at C
            confidence: 0.0,
            chroma: [0.0; 12],
            display_chroma: [0.0; 12],
            avg_chroma: 0.0,
            candidate_key: 0,
            candidate_beats: 0,
            confirmation_beats: 8,  // Need 8 beats of consistency (2 bars at 4/4)
            confidence_threshold: 0.6,
            tuning: TuningReference::Hz440,
            beat_accumulator: 0.0,
            last_beat_phase: 0.0,
            is_silent: true,
            silence_threshold: 0.02, // RMS threshold for noise gate
            second_strongest: 1,
            major_likelihood: 0.5,
            minor_likelihood: 0.5,
            tonal_energy: 0.0,
            key_strength: 0.0,
            // Dominant pitch latching state
            held_dom_pc: 0,
            held_dom_confidence: 0.0,
            pending_dom_pc: 0,
            pending_dom_time: 0.0,
            raw_dom_pc: 0,
            // Chroma FIFO ring buffer
            chroma_buffer: vec![[0.0; 12]; CHROMA_BUFFER_SIZE],
            chroma_buffer_pos: 0,
            chroma_buffer_count: 0,
        }
    }

    /// Set the confirmation beats required for key change
    pub fn set_confirmation_beats(&mut self, beats: u32) {
        self.confirmation_beats = beats.max(1);
    }

    /// Set the confidence threshold for key changes
    pub fn set_confidence_threshold(&mut self, threshold: f32) {
        self.confidence_threshold = threshold.clamp(0.1, 0.95);
    }

    /// Set the tuning reference
    pub fn set_tuning(&mut self, tuning: TuningReference) {
        self.tuning = tuning;
    }

    /// Get current key root (0-11)
    pub fn key_root(&self) -> u8 {
        self.key_root
    }

    /// Get confidence in current key (0.0 - 1.0)
    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    /// Get smoothed chroma array
    pub fn chroma(&self) -> &[f32; 12] {
        &self.chroma
    }

    /// Get average chroma energy
    pub fn avg_chroma(&self) -> f32 {
        self.avg_chroma
    }

    /// Get the current tuning reference
    pub fn tuning(&self) -> TuningReference {
        self.tuning
    }

    /// Get live candidate key (what detector is currently seeing)
    pub fn candidate_key(&self) -> u8 {
        self.candidate_key
    }

    /// Get beats accumulated for candidate
    pub fn candidate_beats(&self) -> u32 {
        self.candidate_beats
    }

    /// Get display-ready chroma (mean-subtracted, normalized)
    pub fn display_chroma(&self) -> &[f32; 12] {
        &self.display_chroma
    }

    /// Get raw averaged chroma (for variance/balance calculation)
    pub fn raw_chroma(&self) -> &[f32; 12] {
        &self.chroma
    }

    /// Check if in silence
    pub fn is_silent(&self) -> bool {
        self.is_silent
    }

    /// Get second strongest pitch class
    pub fn second_strongest(&self) -> u8 {
        self.second_strongest
    }

    /// Get major scale likelihood
    pub fn major_likelihood(&self) -> f32 {
        self.major_likelihood
    }

    /// Get minor scale likelihood
    pub fn minor_likelihood(&self) -> f32 {
        self.minor_likelihood
    }

    /// Get tonal energy (0.0 - 1.0) - "Do we have pitched content?"
    pub fn tonal_energy(&self) -> f32 {
        self.tonal_energy
    }

    /// Get key strength (0.0 - 1.0) - "Is one bin clearly winning?"
    pub fn key_strength(&self) -> f32 {
        self.key_strength
    }

    // ========================================================================
    // DOMINANT PITCH LATCHING GETTERS
    // ========================================================================

    /// Get the HELD dominant pitch class (0-11) - LATCHED with hysteresis
    /// This is what UI and shaders should read for stable visualization
    pub fn held_dom_pc(&self) -> u8 {
        self.held_dom_pc
    }

    /// Get the RAW dominant pitch class (0-11) - instant argmax, no latching
    /// For debugging or when you want to see the unfiltered detection
    pub fn raw_dom_pc(&self) -> u8 {
        self.raw_dom_pc
    }

    /// Get confidence when held dominant was latched
    pub fn held_dom_confidence(&self) -> f32 {
        self.held_dom_confidence
    }

    /// Get the harmonic label based on current confidence and tonal state
    /// Uses the held dominant's confidence for stability
    pub fn harmonic_label(&self) -> HarmonicLabel {
        // Atonal: silence or no pitched content
        if self.is_silent || self.tonal_energy < 0.1 {
            return HarmonicLabel::Atonal;
        }

        // Use current confidence (margin between top1 and top2)
        if self.confidence >= LABEL_STABLE_THRESHOLD {
            HarmonicLabel::Stable
        } else if self.confidence >= LABEL_ROTATING_THRESHOLD {
            HarmonicLabel::Rotating
        } else {
            // Low confidence + low dominance = atonal
            if self.key_strength < 0.20 {
                HarmonicLabel::Atonal
            } else {
                HarmonicLabel::Ambiguous
            }
        }
    }

    /// Calculate pitch bias for a given note index
    ///
    /// Returns a value in [-0.15, +0.15] indicating how prominent
    /// this pitch class is relative to average.
    /// Use this to NUDGE parameters, not steer the visualization.
    pub fn pitch_bias(&self, note_index: u8) -> f32 {
        let idx = (note_index % 12) as usize;
        let bias = self.chroma[idx] - self.avg_chroma;
        bias.clamp(-0.15, 0.15)
    }

    /// Update with new chroma data and optional RMS for silence detection
    ///
    /// Arguments:
    /// - `raw_chroma`: 12-bin chroma energy from audio analysis
    /// - `beat_phase`: Current beat phase (0.0 - 1.0) for beat detection
    pub fn update(&mut self, raw_chroma: &[f32; 12], beat_phase: f32) {
        self.update_with_dt(raw_chroma, beat_phase, None, 1.0 / 60.0);
    }

    /// Update with new chroma data and RMS for silence detection
    pub fn update_with_rms(&mut self, raw_chroma: &[f32; 12], beat_phase: f32, rms: Option<f32>) {
        self.update_with_dt(raw_chroma, beat_phase, rms, 1.0 / 60.0);
    }

    /// Full update with delta time for dominant pitch latching
    ///
    /// Arguments:
    /// - `raw_chroma`: 12-bin chroma energy from audio analysis
    /// - `beat_phase`: Current beat phase (0.0 - 1.0) for beat detection
    /// - `rms`: Optional RMS for silence detection
    /// - `dt`: Delta time in seconds since last update (for dominant latching)
    pub fn update_with_dt(&mut self, raw_chroma: &[f32; 12], beat_phase: f32, rms: Option<f32>, dt: f32) {
        // Apply tuning correction (shifts chroma bins for 432Hz reference)
        let tuned_chroma = self.tuning.apply_to_chroma(raw_chroma);

        // === TONAL ENERGY: RMS of raw chroma (before normalization) ===
        // "Do we have pitched content?"
        let sum_sq: f32 = tuned_chroma.iter().map(|x| x * x).sum();
        self.tonal_energy = (sum_sq / 12.0).sqrt().min(1.0);

        // Check silence gate
        let total_raw: f32 = raw_chroma.iter().sum();
        let rms_val = rms.unwrap_or(total_raw / 12.0);
        self.is_silent = rms_val < self.silence_threshold;

        // If silent, zero out display values but keep buffer intact
        if self.is_silent {
            self.tonal_energy = 0.0;
            self.key_strength = 0.0;
            self.confidence = 0.0;
            // Zero display chroma so UI shows empty bars
            self.display_chroma = [0.0; 12];
            // Note: Don't clear chroma buffer - it will naturally update when audio returns
            return;
        }

        // === FIFO RING BUFFER: Store new frame and compute average ===
        // Write tuning-corrected chroma to ring buffer
        self.chroma_buffer[self.chroma_buffer_pos] = tuned_chroma;
        self.chroma_buffer_pos = (self.chroma_buffer_pos + 1) % CHROMA_BUFFER_SIZE;
        if self.chroma_buffer_count < CHROMA_BUFFER_SIZE {
            self.chroma_buffer_count += 1;
        }

        // Average over the FIFO buffer (only valid frames)
        self.chroma = [0.0; 12];
        for frame_idx in 0..self.chroma_buffer_count {
            let frame = &self.chroma_buffer[frame_idx];
            for i in 0..12 {
                self.chroma[i] += frame[i];
            }
        }
        let count = self.chroma_buffer_count as f32;
        for i in 0..12 {
            self.chroma[i] /= count;
        }

        // Calculate average chroma
        self.avg_chroma = self.chroma.iter().sum::<f32>() / 12.0;

        // === Create display chroma (mean-subtracted, clamped, normalized) ===
        for i in 0..12 {
            self.display_chroma[i] = (self.chroma[i] - self.avg_chroma).max(0.0);
        }
        let max_display = self.display_chroma.iter().copied().fold(0.0f32, f32::max);
        if max_display > 0.001 {
            for c in &mut self.display_chroma {
                *c /= max_display;
            }
        }

        // Detect beat crossing (phase wrap from high to low)
        let beat_crossed = beat_phase < self.last_beat_phase &&
                          (self.last_beat_phase - beat_phase) > 0.5;
        self.last_beat_phase = beat_phase;

        // Find top 2 pitch classes for margin-based confidence
        let (top1_idx, top1_val, top2_idx, top2_val) = self.find_top_two();
        self.second_strongest = top2_idx;

        // === KEY STRENGTH: top1 value (normalized) ===
        // "Is one bin clearly winning?"
        self.key_strength = top1_val.min(1.0);

        // === MARGIN-BASED CONFIDENCE ===
        // confidence = (top1 - top2) / top1 (clamped)
        // This measures how dominant the winner is vs the runner-up
        self.confidence = if top1_val > 0.001 {
            ((top1_val - top2_val) / top1_val).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // === SCALE DETECTION (Major vs Minor) ===
        self.calculate_scale_likelihood(top1_idx);

        // === DOMINANT PITCH ===
        // Argmax. That's it.
        self.raw_dom_pc = top1_idx;
        self.held_dom_pc = top1_idx;
        self.held_dom_confidence = self.confidence;

        // === KEY CHANGE LOGIC (fixed candidate tracking) ===
        // Only process on beat crossings with sufficient confidence AND tonal energy
        if beat_crossed && self.confidence >= self.confidence_threshold && self.tonal_energy > 0.1 {
            // Check if this matches the PREVIOUS candidate (before updating it)
            if top1_idx == self.candidate_key {
                // Same as previous candidate - accumulate beats
                self.candidate_beats += 1;
            } else {
                // New candidate - reset counter
                self.candidate_beats = 1;
            }

            // NOW update the candidate (after using the previous value for comparison)
            self.candidate_key = top1_idx;

            // Confirm key change after enough consistent beats
            if self.candidate_beats >= self.confirmation_beats && self.candidate_key != self.key_root {
                self.key_root = self.candidate_key;
                log::debug!("[KeyDetector] Key changed to {} (conf={:.2})",
                    Self::note_name(self.key_root), self.confidence);
            }
        } else {
            // Always update live candidate even when not crossing beats
            // (so UI shows current detection)
            self.candidate_key = top1_idx;
        }
    }

    /// Find top two pitch classes
    fn find_top_two(&self) -> (u8, f32, u8, f32) {
        let mut top1_idx = 0u8;
        let mut top1_val = self.chroma[0];
        let mut top2_idx = 1u8;
        let mut top2_val = self.chroma[1];

        if top2_val > top1_val {
            std::mem::swap(&mut top1_idx, &mut top2_idx);
            std::mem::swap(&mut top1_val, &mut top2_val);
        }

        for i in 2..12 {
            let val = self.chroma[i];
            if val > top1_val {
                top2_idx = top1_idx;
                top2_val = top1_val;
                top1_idx = i as u8;
                top1_val = val;
            } else if val > top2_val {
                top2_idx = i as u8;
                top2_val = val;
            }
        }

        (top1_idx, top1_val, top2_idx, top2_val)
    }

    /// Calculate major/minor scale likelihood based on detected root
    fn calculate_scale_likelihood(&mut self, root: u8) {
        // Major scale intervals from root: 0, 2, 4, 5, 7, 9, 11 (whole-whole-half-whole-whole-whole-half)
        // Minor scale intervals from root: 0, 2, 3, 5, 7, 8, 10 (natural minor)
        let major_intervals = [0, 2, 4, 5, 7, 9, 11];
        let minor_intervals = [0, 2, 3, 5, 7, 8, 10];

        let mut major_energy = 0.0f32;
        let mut minor_energy = 0.0f32;

        for &interval in &major_intervals {
            let idx = ((root as usize + interval) % 12) as usize;
            major_energy += self.chroma[idx];
        }

        for &interval in &minor_intervals {
            let idx = ((root as usize + interval) % 12) as usize;
            minor_energy += self.chroma[idx];
        }

        let total = major_energy + minor_energy;
        if total > 0.01 {
            self.major_likelihood = major_energy / total;
            self.minor_likelihood = minor_energy / total;
        } else {
            self.major_likelihood = 0.5;
            self.minor_likelihood = 0.5;
        }
    }

    /// Get note name for a pitch class
    pub fn note_name(pitch_class: u8) -> &'static str {
        match pitch_class % 12 {
            0 => "C",
            1 => "C#",
            2 => "D",
            3 => "D#",
            4 => "E",
            5 => "F",
            6 => "F#",
            7 => "G",
            8 => "G#",
            9 => "A",
            10 => "A#",
            11 => "B",
            _ => "?",
        }
    }

    /// Get the key-relative note index
    ///
    /// This is the core function for key-aware visualization:
    /// `note_index = (gate_index + key_root) % 12`
    ///
    /// When the key is C, note_index == gate_index
    /// When the key is G, the whole mapping shifts by 7
    pub fn key_relative_index(&self, gate_index: u8) -> u8 {
        (gate_index + self.key_root) % 12
    }

    /// Manually set key (for testing or user override)
    pub fn set_key(&mut self, key_root: u8) {
        self.key_root = key_root % 12;
        self.candidate_key = self.key_root;
        self.candidate_beats = self.confirmation_beats;
        self.confidence = 1.0;
    }
}

/// Chroma extraction helpers
impl KeyDetector {
    /// Generate empty chroma (for when no audio analysis is available)
    pub fn empty_chroma() -> [f32; 12] {
        [0.0; 12]
    }

    /// Simple chroma estimation from frequency bands
    /// This is a rough approximation - real chroma should come from proper FFT analysis
    pub fn estimate_chroma_from_bands(bass: f32, mid: f32, high: f32) -> [f32; 12] {
        // This is a placeholder - proper chroma requires spectral analysis
        // For now, distribute energy somewhat evenly with slight variations
        let mut chroma = [0.0f32; 12];
        let base_energy = (bass + mid * 2.0 + high) / 4.0;

        for i in 0..12 {
            // Slight variation based on position
            let variation = 1.0 + 0.1 * (i as f32 / 12.0 - 0.5);
            chroma[i] = base_energy * variation;
        }

        chroma
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_relative_index() {
        let mut detector = KeyDetector::new();

        // In key of C
        detector.set_key(0);
        assert_eq!(detector.key_relative_index(0), 0);  // Gate 0 -> C (I)
        assert_eq!(detector.key_relative_index(7), 7);  // Gate 7 -> G (V)

        // In key of G
        detector.set_key(7);
        assert_eq!(detector.key_relative_index(0), 7);  // Gate 0 -> G (I)
        assert_eq!(detector.key_relative_index(5), 0);  // Gate 5 -> C (IV)
    }

    #[test]
    fn test_pitch_bias_clamping() {
        let detector = KeyDetector::new();
        let bias = detector.pitch_bias(0);
        assert!(bias >= -0.15 && bias <= 0.15);
    }

    #[test]
    fn test_tuning_offset() {
        assert_eq!(TuningReference::Hz440.semitone_offset(), 0.0);
        assert!(TuningReference::Hz432.semitone_offset() < 0.0);
    }
}
