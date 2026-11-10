//! FFT-based Audio Analyzer with Beat Detection
//!
//! Provides accurate, low-latency audio analysis:
//! - FFT-based frequency band analysis (bass, mid, high)
//! - Kick detection using ratio gate + envelope rise detection
//! - Click/transient detection for hi-hats

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;

use super::normalizer::ControlSignalNormalizer;

const FFT_SIZE: usize = 8192; // Larger for bass pitch accuracy (~170ms latency, 5.9 Hz/bin)
const SAMPLE_RATE: f32 = 48000.0;

// Frequency band ranges (in Hz)
const BASS_LOW: f32 = 20.0;
const BASS_HIGH: f32 = 400.0;   // Cleaner kick focus
const MID_LOW: f32 = 300.0;
const MID_HIGH: f32 = 1200.0;
const HIGH_LOW: f32 = 1400.0;
const HIGH_HIGH: f32 = 20000.0;

// Kick detection bands - focused on the "punch" frequency range
// Kicks have their fundamental at 40-100Hz with attack transients around 80-150Hz
const KICK_BODY_LOW: f32 = 40.0;   // Kick drum fundamental
const KICK_BODY_HIGH: f32 = 150.0; // Kick attack/punch region
const BASS_TAIL_LOW: f32 = 150.0;  // Upper bass that sustains longer
const BASS_TAIL_HIGH: f32 = 300.0;

// Kick detection parameters
const KICK_RATIO_GATE: f32 = 1.28;      // Body must be this much stronger than tail
const KICK_ENVELOPE_FC: f32 = 60.0;     // Fast envelope follower cutoff (Hz)
const KICK_FLOOR_RISE_FC: f32 = 0.5;    // Floor follower rise rate (slow)
const KICK_FLOOR_FALL_FC: f32 = 0.2;    // Floor follower fall rate (even slower)
const KICK_FLOOR_MARGIN: f32 = 1.4;     // Envelope must exceed floor * this
const KICK_RISE_THRESHOLD: f32 = 0.0005; // Minimum rise rate to trigger

// Kick timing constants - tuned for most electronic music
const KICK_REFRACTORY_MS: f32 = 80.0;   // Allows up to ~180 BPM
const KICK_HYSTERESIS: f32 = 0.35;      // Rearm when env drops below threshold * (1 - hysteresis)

const DEFAULT_BPM: f32 = 120.0;

/// Audio analysis results
/// PERFORMANCE: Copy instead of Clone - just primitive types (~120 bytes with chroma)
#[derive(Clone, Copy, Debug)]
pub struct AudioAnalysis {
    /// FAST smoothed bass - responsive UI meters, shader animation
    pub bass: f32,
    /// FAST smoothed mid - responsive UI meters, shader animation
    pub mid: f32,
    /// FAST smoothed high - responsive UI meters, shader animation
    pub high: f32,
    /// SLOW smoothed bass - stable LED wall, sustained energy
    pub stable_bass: f32,
    /// SLOW smoothed mid - stable LED wall, sustained energy
    pub stable_mid: f32,
    /// SLOW smoothed high - stable LED wall, sustained energy
    pub stable_high: f32,
    /// Raw energy (RMS) - for gates and silence detection (typically 0.02-0.15)
    pub energy: f32,
    /// Perceptual energy - for UI and shaders (remapped to 0-1 visible range)
    pub energy_vis: f32,
    pub kick: bool,
    pub kick_intensity: f32,
    pub click: bool,        // Deprecated - kept for compatibility
    pub click_intensity: f32, // Deprecated - kept for compatibility
    pub bpm: f32,
    pub beat_phase: f32,
    /// 12-bin chroma (pitch class energy): C, C#, D, D#, E, F, F#, G, G#, A, A#, B
    /// SLOW smoothing for stable dominant detection (800ms hold)
    /// Computed by folding FFT bins into pitch classes
    pub chroma: [f32; 12],
    /// 12-bin chroma with FAST smoothing for responsive UI display
    /// Use this for shader/HUD, not for dominant latch logic
    pub display_chroma: [f32; 12],
    /// Whether chroma passes tonal validity gates (energy + peak separation)
    /// When false, chroma is noise - don't trust dominant pitch class
    pub is_tonal: bool,
    /// Dominant pitch class index (0-11) - only valid when is_tonal=true
    pub dominant_pc: usize,
    /// Peak separation ratio (0-1) - how much the dominant stands out
    pub tonal_separation: f32,
    /// Total tonal energy (sum of chroma bins before normalization)
    pub tonal_energy: f32,
    /// Temporal stability (0-1) - how stable chroma is frame-to-frame
    /// High = smooth transitions, Low = erratic changes
    pub stability: f32,
    /// Live frequency centroids for 6 spectral bands (Hz)
    /// [Sub, Kick, Low-mid, Mid, High-mid, Air]
    /// Computed as energy-weighted average frequency per band
    pub band_centroids: [f32; 6],
    /// Normalizer clip ratio (0 = healthy, >0.15 = expanding headroom)
    pub norm_clip_ratio: f32,
    /// Normalizer floor ratio (0 = healthy, high = everything floored at 0)
    pub norm_floor_ratio: f32,
    /// Normalizer p10 energy percentile (log domain, for debug)
    pub norm_p10_energy: f32,
    /// Normalizer p90 energy percentile (log domain, for debug)
    pub norm_p90_energy: f32,
    /// Normalizer warmup phase (0.0 = just started, 1.0 = converged)
    pub norm_warmup: f32,
    /// Raw FFT bass band energy (pre-normalization, for MultiBand mapping)
    pub raw_bass: f32,
    /// Raw FFT mid band energy (pre-normalization, for MultiBand mapping)
    pub raw_mid: f32,
    /// Raw FFT high band energy (pre-normalization, for MultiBand mapping)
    pub raw_high: f32,
    /// Continuous low-band onset strength — the kick-envelope flux (`rise`),
    /// floor-normalized for loudness-invariance. This is the per-frame onset
    /// detection function the tempo autocorrelator consumes; distinct from the
    /// held boolean `kick`.
    pub onset_strength: f32,
}

impl Default for AudioAnalysis {
    fn default() -> Self {
        Self {
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            stable_bass: 0.0,
            stable_mid: 0.0,
            stable_high: 0.0,
            energy: 0.0,
            energy_vis: 0.0,
            kick: false,
            kick_intensity: 0.0,
            click: false,
            click_intensity: 0.0,
            bpm: 120.0,
            beat_phase: 0.0,
            chroma: [0.0; 12],
            display_chroma: [0.0; 12],
            is_tonal: false,
            dominant_pc: 0,
            tonal_separation: 0.0,
            tonal_energy: 0.0,
            stability: 0.0,
            // Default band centroids: geometric means (log-center of each band)
            band_centroids: [35.0, 112.0, 355.0, 1122.0, 3550.0, 11225.0],
            norm_clip_ratio: 0.0,
            norm_floor_ratio: 0.0,
            norm_p10_energy: 0.0,
            norm_p90_energy: 0.0,
            norm_warmup: 0.0,
            raw_bass: 0.0,
            raw_mid: 0.0,
            raw_high: 0.0,
            onset_strength: 0.0,
        }
    }
}

// Tonal confidence thresholds with hysteresis
// Enter tonal at higher threshold, exit at lower (prevents flip-flopping)
const TONAL_ENTER_THRESHOLD: f32 = 0.18;  // Must exceed this to become tonal (was 0.25, lowered for faster re-entry)
const TONAL_EXIT_THRESHOLD: f32 = 0.06;   // Must drop below this to become atonal (was 0.12, lowered to stay tonal during transitions)

// Frame counts for hysteresis (at ~47 fps = FFT_SIZE/SAMPLE_RATE)
const TONAL_ENTER_FRAMES: u32 = 2;   // Must be above threshold for 2 frames to enter (was 3)
const TONAL_EXIT_FRAMES: u32 = 15;   // Must be below threshold for 15 frames to exit (was 8, increased to prevent dropout during note changes)

// Dominant note latch (time-based with strength hysteresis)
const DOM_SWITCH_DELTA: f32 = 0.08;   // New winner must be 8% stronger than current (was 12%, lowered for faster response)
const DOM_MIN_HOLD_MS: f32 = 300.0;   // Minimum 300ms between dominant switches (was 800ms, reduced for faster response)

// Stability threshold for "Stable Harmony" badge
const STABILITY_THRESHOLD: f32 = 0.6;  // Frame-to-frame chroma stability required

/// FFT-based audio analyzer with beat detection
pub struct AudioAnalyzer {
    /// Cached FFT plan - creating this is expensive, reuse it!
    fft: Arc<dyn Fft<f32>>,
    fft_buffer: Vec<Complex<f32>>,
    window: Vec<f32>,

    // Sample accumulation buffer (to collect enough samples for FFT)
    sample_buffer: Vec<f32>,

    // Smoothed output values
    smooth_bass: f32,           // FAST - responsive UI meters
    smooth_mid: f32,
    smooth_high: f32,
    stable_bass: f32,           // SLOW - stable LED wall
    stable_mid: f32,
    stable_high: f32,
    smooth_energy: f32,         // Raw energy for gates
    smooth_energy_vis: f32,     // Perceptual energy for UI/shaders
    smooth_chroma: [f32; 12],    // SLOW smoothing for stable dominant detection
    display_chroma: [f32; 12],   // FAST smoothing for responsive UI display
    /// Smoothed band centroid frequencies (Hz) for spectral ring labels
    /// 6 bands: [Sub, Kick, Low-mid, Mid, High-mid, Air]
    smooth_band_centroids: [f32; 6],
    /// Band energies for hold-on-silence logic (don't update centroid when silent)
    prev_band_energies: [f32; 6],

    // Tonal state tracking (with hysteresis)
    is_tonal: bool,
    dominant_pc: usize,
    tonal_confidence: f32,       // Smoothed confidence value
    tonal_energy: f32,
    // Previous frame's normalized chroma (for stability calculation)
    prev_chroma_norm: [f32; 12],
    // Temporal stability (0-1, how much chroma changed from prev frame)
    stability: f32,
    // Hysteresis counters
    tonal_enter_counter: u32,    // Frames above TONAL_ENTER_THRESHOLD
    tonal_exit_counter: u32,     // Frames below TONAL_EXIT_THRESHOLD
    // Dominant note latch (time-based, not frame-based)
    last_dom_switch_ms: f32,     // Time when dominant was last switched
    last_dom_strength: f32,      // Strength of current dominant when latched

    // Kick detection state
    kick_envelope_lp: f32,      // Fast envelope follower for kick body energy
    kick_floor: f32,            // Slow noise floor follower
    prev_envelope: f32,         // Previous envelope for rise detection
    kick_last_time_ms: f32,
    kick_armed: bool,
    kick_hold_samples: u32,     // Hold kick=true for multiple analysis frames

    // Click detection state
    click_envelope: f32,
    click_threshold: f32,
    prev_spectrum: Vec<f32>,
    click_flux_history: VecDeque<f32>,
    click_cooldown: u32,

    // Reusable magnitude buffer (avoids allocation per FFT frame)
    magnitude_buffer: Vec<f32>,

    // Time tracking
    sample_count: u64,

    // BPM tracking
    estimated_bpm: f32,
    beat_period_ms: f32,
    beat_anchor_ms: f32,

    // Sensitivity
    onset_threshold_k: f32,

    // Audio gain
    gain: f32,

    // Percentile-based normalizer for lighting control signals
    normalizer: ControlSignalNormalizer,

    // Raw FFT band energies (pre-normalization, for MultiBand Hue mapping)
    last_raw_bass: f32,
    last_raw_mid: f32,
    last_raw_high: f32,
}

impl AudioAnalyzer {
    pub fn new() -> Self {
        // Create and CACHE the FFT plan - planning is expensive!
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);

        // Hann window for smooth FFT
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos())
            })
            .collect();

        let beat_period_ms = 60000.0 / DEFAULT_BPM;

        Self {
            fft,
            fft_buffer: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            window,
            sample_buffer: Vec::with_capacity(FFT_SIZE * 2),
            smooth_bass: 0.0,
            smooth_mid: 0.0,
            smooth_high: 0.0,
            stable_bass: 0.0,
            stable_mid: 0.0,
            stable_high: 0.0,
            smooth_energy: 0.0,
            smooth_energy_vis: 0.0,
            smooth_chroma: [0.0; 12],
            display_chroma: [0.0; 12],
            // Initialize band centroids to geometric means (log-center of each band)
            // sqrt(low * high) for each band with geometric spacing
            smooth_band_centroids: [35.0, 112.0, 355.0, 1122.0, 3550.0, 11225.0],
            prev_band_energies: [0.0; 6],
            is_tonal: false,
            dominant_pc: 0,
            tonal_confidence: 0.0,
            tonal_energy: 0.0,
            prev_chroma_norm: [0.0; 12],
            stability: 0.0,  // Start at 0 (no audio = empty meter)
            tonal_enter_counter: 0,
            tonal_exit_counter: 0,
            last_dom_switch_ms: 0.0,
            last_dom_strength: 0.0,
            kick_envelope_lp: 0.0,
            kick_floor: 0.0,
            prev_envelope: 0.0,
            kick_last_time_ms: -1000.0,
            kick_armed: true,
            kick_hold_samples: 0,
            click_envelope: 0.0,
            click_threshold: 0.2,
            prev_spectrum: vec![0.0; FFT_SIZE / 2],
            click_flux_history: VecDeque::with_capacity(32),
            click_cooldown: 0,
            magnitude_buffer: vec![0.0; FFT_SIZE / 2],
            sample_count: 0,
            estimated_bpm: DEFAULT_BPM,
            beat_period_ms,
            beat_anchor_ms: 0.0,
            onset_threshold_k: 0.58,
            gain: 1.0,
            normalizer: ControlSignalNormalizer::new(),
            last_raw_bass: 0.0,
            last_raw_mid: 0.0,
            last_raw_high: 0.0,
        }
    }

    /// Set audio gain
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain.max(0.0).min(4.0);
    }

    fn freq_to_bin(&self, freq: f32) -> usize {
        ((freq * FFT_SIZE as f32 / SAMPLE_RATE) as usize).min(FFT_SIZE / 2 - 1)
    }

    fn band_energy(&self, magnitudes: &[f32], low_freq: f32, high_freq: f32) -> f32 {
        let low_bin = self.freq_to_bin(low_freq);
        let high_bin = self.freq_to_bin(high_freq);

        if high_bin <= low_bin {
            return 0.0;
        }

        let sum: f32 = magnitudes[low_bin..=high_bin].iter().map(|x| x * x).sum();
        (sum / (high_bin - low_bin + 1) as f32).sqrt()
    }

    /// Compute spectral centroid (energy-weighted average frequency) for a band
    /// Returns the centroid frequency in Hz, clamped to band bounds
    fn band_centroid(&self, magnitudes: &[f32], low_freq: f32, high_freq: f32) -> f32 {
        let low_bin = self.freq_to_bin(low_freq);
        let high_bin = self.freq_to_bin(high_freq);

        if high_bin <= low_bin {
            return (low_freq + high_freq) * 0.5; // Return center if invalid range
        }

        let mut weighted_sum = 0.0f32;
        let mut mag_sum = 0.0f32;

        for bin in low_bin..=high_bin {
            let freq = bin as f32 * SAMPLE_RATE / FFT_SIZE as f32;
            let mag = magnitudes[bin];
            weighted_sum += freq * mag;
            mag_sum += mag;
        }

        if mag_sum < 1e-10 {
            // No energy - return band center
            return (low_freq + high_freq) * 0.5;
        }

        // Compute centroid and clamp to band bounds
        let centroid = weighted_sum / mag_sum;
        centroid.clamp(low_freq, high_freq)
    }

    fn spectral_flux(&self, magnitudes: &[f32], prev: &[f32], low_freq: f32, high_freq: f32) -> f32 {
        let low_bin = self.freq_to_bin(low_freq);
        let high_bin = self.freq_to_bin(high_freq);

        let mut flux = 0.0f32;
        for i in low_bin..=high_bin {
            let diff = magnitudes[i] - prev[i];
            if diff > 0.0 {
                flux += diff * diff;
            }
        }
        flux.sqrt()
    }

    fn smooth(&self, current: f32, target: f32, attack: f32, decay: f32) -> f32 {
        if target > current {
            current + (target - current) * attack
        } else {
            current + (target - current) * decay
        }
    }

    fn compute_alpha(&self, fc: f32, dt_s: f32) -> f32 {
        1.0 - (-2.0 * std::f32::consts::PI * fc * dt_s).exp()
    }

    /// Compute 12-bin chroma from FFT magnitudes
    /// Returns (normalized_chroma, raw_energy)
    ///
    /// ALWAYS normalizes to max=1.0 when there's signal.
    /// Tonal confidence is computed later from SMOOTHED chroma in analyze().
    ///
    /// Pitch classes: 0=C, 1=C#, 2=D, 3=D#, 4=E, 5=F, 6=F#, 7=G, 8=G#, 9=A, 10=A#, 11=B
    fn compute_chroma(&self, magnitudes: &[f32]) -> ([f32; 12], f32) {
        let mut chroma = [0.0f32; 12];

        // Frequency range for chroma: A0 (27.5 Hz) to ~B6 (2000 Hz)
        // Captures full piano range including lowest fundamentals
        let min_freq = 27.5;
        let max_freq = 2000.0;

        let min_bin = self.freq_to_bin(min_freq).max(3);
        let max_bin = self.freq_to_bin(max_freq).min(FFT_SIZE / 2 - 1);

        // Noise floor
        let noise_floor = 0.0005;

        // Process local maxima with parabolic interpolation for accurate pitch
        for k in (min_bin + 1)..max_bin {
            let mag = magnitudes[k];
            let prev = magnitudes[k - 1];
            let next = magnitudes[k + 1];

            // Skip if not a local maximum or below noise floor
            if mag < noise_floor || mag < prev || mag < next {
                continue;
            }

            // Parabolic interpolation to find true peak frequency
            let denom = prev - 2.0 * mag + next;
            let interpolated_bin = if denom.abs() > 1e-10 && denom < 0.0 {
                // Only interpolate if it's a true peak (negative second derivative)
                let delta = 0.5 * (prev - next) / denom;
                k as f32 + delta.clamp(-0.5, 0.5)
            } else {
                k as f32
            };

            let freq = interpolated_bin * SAMPLE_RATE / FFT_SIZE as f32;

            // Convert to MIDI note
            let midi_note = 69.0 + 12.0 * (freq / 440.0).log2();

            // Soft voting from interpolated frequency
            let pc_float = midi_note.rem_euclid(12.0);
            let pc_lower = pc_float.floor() as usize % 12;
            let pc_upper = (pc_lower + 1) % 12;
            let frac = pc_float - pc_float.floor();

            // Gentle 1/sqrt(octave) weighting
            let octave = (freq / 27.5).log2().max(0.0) + 1.0;
            let weight = mag / octave.sqrt();

            chroma[pc_lower] += weight * (1.0 - frac);
            chroma[pc_upper] += weight * frac;
        }

        // Get raw sum before normalization (for energy metric)
        let raw_sum: f32 = chroma.iter().sum();
        let max_val: f32 = chroma.iter().copied().fold(0.0f32, f32::max);

        // Silence check - return zeros
        if raw_sum < 1e-6 || max_val < 1e-6 {
            return ([0.0; 12], 0.0);
        }

        // ALWAYS normalize to max=1.0 so the field has shape
        let inv = 1.0 / max_val;
        for c in &mut chroma {
            *c *= inv;
        }

        (chroma, raw_sum)
    }

    /// Compute tonal confidence using entropy-based clarity + temporal stability
    /// Returns (dominant_idx, confidence, top1, top2, mean) for logging and hysteresis
    ///
    /// New formula designed to be chord-friendly:
    /// - peak_norm: how much does winner stand out from mean (single notes + chords)
    /// - clarity: 1 - entropy (low entropy = tonal, even for chords)
    /// - stability: temporal consistency with prev frame (smooth = tonal)
    ///
    /// IMPORTANT: Call this on SMOOTHED chroma, not raw!
    fn compute_tonal_state(&self, chroma: &[f32; 12], prev_chroma: &[f32; 12]) -> (usize, f32, f32, f32, f32) {
        // Find dominant and compute stats
        let mut dominant_idx = 0;
        let mut max_val = 0.0f32;
        let mut sum = 0.0f32;

        for (i, &v) in chroma.iter().enumerate() {
            sum += v;
            if v > max_val {
                max_val = v;
                dominant_idx = i;
            }
        }

        let mean = sum / 12.0;

        // Sort to find top 2 (for logging)
        let mut sorted = *chroma;
        sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let top1 = sorted[0];
        let top2 = sorted[1];

        // === CHORD-FRIENDLY CONFIDENCE METRIC ===

        // 1. Peak norm: how much does winner stand out from mean?
        // Works for both single notes AND chords (doesn't punish close top1/top2)
        let peak = (top1 - mean).max(0.0);
        let peak_norm = (peak / (top1 + 1e-6)).clamp(0.0, 1.0);

        // 2. Clarity: 1 - normalized entropy
        // Low entropy = energy concentrated in few bins = tonal (single note OR chord)
        // High entropy = energy spread evenly = noise
        let clarity = if sum > 1e-6 {
            // Normalize chroma to probability distribution (sum = 1)
            let mut entropy = 0.0f32;
            for &v in chroma.iter() {
                let p = v / sum;
                if p > 1e-9 {
                    entropy -= p * p.ln();
                }
            }
            // Normalize by max entropy (ln(12) = uniform distribution)
            let max_entropy = 12.0f32.ln();
            let normalized_entropy = (entropy / max_entropy).clamp(0.0, 1.0);
            1.0 - normalized_entropy
        } else {
            0.0 // Silence = no clarity
        };

        // 3. Stability: how similar is this frame to the previous?
        // Smooth transitions = tonal music, erratic jumps = noise/percussion
        // Note: If there's no audio (sum near zero), stability should be 0 (no data to measure)
        let stability = if sum > 1e-6 {
            let mut diff_sum = 0.0f32;
            for i in 0..12 {
                diff_sum += (chroma[i] - prev_chroma[i]).abs();
            }
            let mean_diff = diff_sum / 12.0;
            (1.0 - (mean_diff / 0.15).clamp(0.0, 1.0)).max(0.0)
        } else {
            0.0 // Silence = no data to measure stability
        };

        // Combined confidence:
        // 45% peak_norm (tonal has prominent notes)
        // 35% clarity (tonal has low entropy, even chords)
        // 20% stability (tonal changes smoothly)
        let confidence = (0.45 * peak_norm + 0.35 * clarity + 0.20 * stability).clamp(0.0, 1.0);

        (dominant_idx, confidence, top1, top2, mean)
    }

    /// Process audio samples and return analysis
    /// Accumulates samples until we have enough for FFT
    pub fn analyze(&mut self, samples: &[f32]) -> AudioAnalysis {
        // Accumulate incoming samples
        self.sample_buffer.extend_from_slice(samples);

        // Keep buffer tight - only keep exactly what we need for FFT
        // This ensures we respond immediately to silence
        if self.sample_buffer.len() > FFT_SIZE {
            let excess = self.sample_buffer.len() - FFT_SIZE;
            self.sample_buffer.drain(..excess);
        }

        // If we don't have enough samples yet, return current smoothed values
        if self.sample_buffer.len() < FFT_SIZE {
            return AudioAnalysis {
                bass: self.smooth_bass,
                mid: self.smooth_mid,
                high: self.smooth_high,
                stable_bass: self.stable_bass,
                stable_mid: self.stable_mid,
                stable_high: self.stable_high,
                energy: self.smooth_energy,
                energy_vis: self.smooth_energy_vis,
                bpm: self.estimated_bpm,
                kick: false,
                kick_intensity: (self.kick_envelope_lp * 15.0).min(1.0),
                click: false,
                click_intensity: (self.click_envelope / 100.0).min(1.0),
                beat_phase: 0.0,
                chroma: self.smooth_chroma,
                display_chroma: self.display_chroma,
                is_tonal: self.is_tonal,
                dominant_pc: self.dominant_pc,
                tonal_separation: self.tonal_confidence,  // Use smoothed confidence
                tonal_energy: self.tonal_energy,
                stability: self.stability,
                band_centroids: self.smooth_band_centroids,
                norm_clip_ratio: self.normalizer.clip_ratio_avg(),
                norm_floor_ratio: self.normalizer.floor_ratio_avg(),
                norm_p10_energy: 0.0,
                norm_p90_energy: 0.0,
                norm_warmup: 0.0,
                raw_bass: self.last_raw_bass,
                raw_mid: self.last_raw_mid,
                raw_high: self.last_raw_high,
                onset_strength: 0.0,
            };
        }

        // Use the last FFT_SIZE samples for analysis
        let analysis_samples = &self.sample_buffer[self.sample_buffer.len() - FFT_SIZE..];
        let block_samples = analysis_samples.len();
        let dt_s = block_samples as f32 / SAMPLE_RATE;
        let now_ms = (self.sample_count as f32 / SAMPLE_RATE) * 1000.0;
        self.sample_count += block_samples as u64;

        // Apply window and gain to analysis samples
        for (i, &sample) in analysis_samples.iter().enumerate() {
            self.fft_buffer[i] = Complex::new(sample * self.gain * self.window[i], 0.0);
        }

        // Perform FFT using cached plan (creating plans is expensive!)
        self.fft.process(&mut self.fft_buffer);

        // Calculate magnitudes into reusable buffer (avoids allocation)
        for (i, c) in self.fft_buffer[..FFT_SIZE / 2].iter().enumerate() {
            self.magnitude_buffer[i] = c.norm() / FFT_SIZE as f32;
        }

        // Calculate band energies
        let bass_raw = self.band_energy(&self.magnitude_buffer, BASS_LOW, BASS_HIGH);
        let mid_raw = self.band_energy(&self.magnitude_buffer, MID_LOW, MID_HIGH);
        let high_raw = self.band_energy(&self.magnitude_buffer, HIGH_LOW, HIGH_HIGH);

        // Store raw band energies for MultiBand Hue mapping
        self.last_raw_bass = bass_raw;
        self.last_raw_mid = mid_raw;
        self.last_raw_high = high_raw;

        // Total RMS energy
        let rms: f32 = (analysis_samples.iter().map(|s| s * s).sum::<f32>() / analysis_samples.len() as f32).sqrt();

        // === ADAPTIVE NORMALIZATION (replaces fixed scale factors) ===
        // Old code used fixed multipliers (bass×300, mid×1200, high×12000) that
        // clipped everything to 1.0 — killing all contrast for lighting.
        // New code: percentile-based normalization with anti-clip headroom.
        let norm = self.normalizer.normalize(bass_raw, mid_raw, high_raw, rms);

        let bass_target = norm.bass;
        let mid_target = norm.mid;
        let high_target = norm.high;
        let energy_raw_target = norm.energy_raw;
        let energy_vis_target = norm.energy_vis;

        // Debug: log raw + normalized values once per ~1 second
        if self.sample_count % (SAMPLE_RATE as u64 * 1) < FFT_SIZE as u64 {
            log::info!("[Bands RAW] bass={:.5} mid={:.5} high={:.5} rms={:.5}",
                bass_raw, mid_raw, high_raw, rms);
            log::info!("[Bands NORM] bass={:.2} mid={:.2} high={:.2} energy_raw={:.2} energy_vis={:.2} clip={:.3} p10e={:.2} p90e={:.2}",
                bass_target, mid_target, high_target, energy_raw_target, energy_vis_target,
                norm.clip_ratio_avg, norm.p10_energy, norm.p90_energy);
        }

        // FAST smoothing for display (responsive UI meters, shader animation)
        let attack = 0.95;  // Near-instant attack
        let decay = 0.85;   // Quick decay
        self.smooth_bass = self.smooth(self.smooth_bass, bass_target, attack, decay);
        self.smooth_mid = self.smooth(self.smooth_mid, mid_target, attack, decay);
        self.smooth_high = self.smooth(self.smooth_high, high_target, attack, decay);
        self.smooth_energy = self.smooth(self.smooth_energy, energy_raw_target, attack, decay);
        self.smooth_energy_vis = self.smooth(self.smooth_energy_vis, energy_vis_target, attack, decay);

        // SLOW smoothing for stable streams (LED wall, sustained energy)
        let stable_attack = 0.25;  // Gradual rise
        let stable_decay = 0.15;   // Slow decay
        self.stable_bass = self.smooth(self.stable_bass, bass_target, stable_attack, stable_decay);
        self.stable_mid = self.smooth(self.stable_mid, mid_target, stable_attack, stable_decay);
        self.stable_high = self.smooth(self.stable_high, high_target, stable_attack, stable_decay);

        // === BAND CENTROID COMPUTATION (for spectral ring frequency labels) ===
        // 6 bands with GEOMETRIC spacing (follows octave/logarithmic symmetry)
        // 20Hz to 20kHz = 10 octaves, 6 bands = ~1.67 octaves each
        // Ratio per band: 2^(10/6) ≈ 3.17
        // This ensures ring spacing is proportional to musical intervals
        const BAND_RANGES: [(f32, f32); 6] = [
            (20.0, 63.0),       // Sub (~1.67 octaves)
            (63.0, 200.0),      // Bass (~1.67 octaves)
            (200.0, 630.0),     // Low-mid (~1.67 octaves)
            (630.0, 2000.0),    // Mid (~1.67 octaves)
            (2000.0, 6300.0),   // High-mid (~1.67 octaves)
            (6300.0, 20000.0),  // Air (~1.67 octaves)
        ];
        const CENTROID_HOLD_THRESHOLD: f32 = 0.03; // Don't update if band energy below this

        for (i, (low_freq, high_freq)) in BAND_RANGES.iter().enumerate() {
            // Compute band energy for hold logic
            let band_energy = self.band_energy(&self.magnitude_buffer, *low_freq, *high_freq);
            // Scale to 0-1 similar to bass/mid/high
            let band_energy_scaled = (band_energy * 1000.0).min(1.0);

            // Only update centroid if band has enough energy (prevents random jumps in silence)
            if band_energy_scaled > CENTROID_HOLD_THRESHOLD {
                let raw_centroid = self.band_centroid(&self.magnitude_buffer, *low_freq, *high_freq);

                // EMA smoothing ~250ms (alpha = 1 - exp(-dt * 6.0) ≈ 0.5 at 85ms frame time)
                let centroid_alpha = 1.0 - (-dt_s * 6.0).exp();
                self.smooth_band_centroids[i] +=
                    (raw_centroid - self.smooth_band_centroids[i]) * centroid_alpha;
            }
            // If band is silent, hold the last centroid value (no update)

            self.prev_band_energies[i] = band_energy_scaled;
        }

        // === CHROMA COMPUTATION (12 pitch classes) ===
        // Step 1: Get raw normalized chroma from FFT (always max=1.0)
        let (raw_chroma, raw_energy) = self.compute_chroma(&self.magnitude_buffer);
        self.tonal_energy = raw_energy;
        let is_silent = raw_energy < 1e-6;  // Only true silence (was 0.001 - too aggressive)

        // Step 2: Smooth the chroma with DUAL STREAMS
        // - display_chroma: FAST smoothing for responsive UI (shader/HUD)
        // - smooth_chroma: SLOW smoothing for stable dominant latch (800ms hold)
        if is_silent {
            // True silence - fade both streams to zeros gradually
            let fade_rate = 0.08;  // 8% per frame = decay to ~1% in ~55 frames (~1s)
            for i in 0..12 {
                self.smooth_chroma[i] *= 1.0 - fade_rate;
                self.display_chroma[i] *= 1.0 - fade_rate;
                // Snap to zero when very small to avoid lingering values
                if self.smooth_chroma[i] < 0.01 {
                    self.smooth_chroma[i] = 0.0;
                }
                if self.display_chroma[i] < 0.01 {
                    self.display_chroma[i] = 0.0;
                }
            }
        } else {
            // Have signal - apply dual smoothing
            // FAST smoothing for display (responsive UI)
            // Keep consistent smoothing regardless of tonal state to prevent display dropout during note changes
            let display_smoothing = 0.45;  // Always fast for responsive UI
            for i in 0..12 {
                self.display_chroma[i] += (raw_chroma[i] - self.display_chroma[i]) * display_smoothing;
            }

            // SLOW smoothing for dominant detection (stable latch)
            // Slightly faster when not tonal to help re-acquire the new note
            let latch_smoothing = if self.is_tonal { 0.20 } else { 0.15 };
            for i in 0..12 {
                self.smooth_chroma[i] += (raw_chroma[i] - self.smooth_chroma[i]) * latch_smoothing;
            }
        }

        // Step 3: Compute tonal state from SMOOTHED chroma (critical!)
        // Pass prev_chroma_norm for stability calculation
        let (raw_dominant, raw_confidence, top1, top2, mean) = self.compute_tonal_state(&self.smooth_chroma, &self.prev_chroma_norm);

        // Step 3b: Compute temporal STABILITY (frame-to-frame chroma change)
        // Normalize chroma to sum=1 for proper stability calculation
        let chroma_sum: f32 = self.smooth_chroma.iter().sum();
        let raw_stability = if chroma_sum > 1e-6 {
            let mut diff_sum = 0.0f32;
            let prev_sum: f32 = self.prev_chroma_norm.iter().sum();
            for i in 0..12 {
                let curr_norm = self.smooth_chroma[i] / chroma_sum;
                let prev_norm = if prev_sum > 1e-6 { self.prev_chroma_norm[i] / prev_sum } else { 0.0 };
                diff_sum += (curr_norm - prev_norm).abs();
            }
            // diff_sum is 0..2 (if completely different distributions)
            // Scale so diff of 0.24 (12% change total) = stability 0
            let diff_norm = (diff_sum / 2.0) / 0.12;
            (1.0 - diff_norm).clamp(0.0, 1.0)
        } else {
            0.0 // Silence = no data to measure, show empty
        };

        // Smooth stability for UI display
        let stability_smoothing = 0.2;
        self.stability += (raw_stability - self.stability) * stability_smoothing;

        // Update prev_chroma_norm for next frame's stability calculation
        self.prev_chroma_norm = self.smooth_chroma;

        // Smooth the confidence value itself for stability
        let conf_smoothing = 0.15;
        self.tonal_confidence += (raw_confidence - self.tonal_confidence) * conf_smoothing;

        // Step 4: Apply HYSTERESIS to is_tonal state
        // Must stay above/below threshold for N frames before state changes
        if self.is_tonal {
            // Currently tonal - check for exit condition
            if self.tonal_confidence < TONAL_EXIT_THRESHOLD {
                self.tonal_exit_counter += 1;
                self.tonal_enter_counter = 0;
                if self.tonal_exit_counter >= TONAL_EXIT_FRAMES {
                    self.is_tonal = false;
                    self.tonal_exit_counter = 0;
                    log::debug!("[Tonal] → ATONAL (conf={:.2} < {:.2} for {} frames)",
                        self.tonal_confidence, TONAL_EXIT_THRESHOLD, TONAL_EXIT_FRAMES);
                }
            } else {
                self.tonal_exit_counter = 0;
            }
        } else {
            // Currently atonal - check for enter condition
            if self.tonal_confidence > TONAL_ENTER_THRESHOLD {
                self.tonal_enter_counter += 1;
                self.tonal_exit_counter = 0;
                if self.tonal_enter_counter >= TONAL_ENTER_FRAMES {
                    self.is_tonal = true;
                    self.tonal_enter_counter = 0;
                    log::debug!("[Tonal] → TONAL (conf={:.2} > {:.2} for {} frames)",
                        self.tonal_confidence, TONAL_ENTER_THRESHOLD, TONAL_ENTER_FRAMES);
                }
            } else {
                self.tonal_enter_counter = 0;
            }
        }

        // Step 5: Apply DOMINANT NOTE LATCH with time-based hysteresis
        // Only change dominant if:
        // 1. Enough time has passed since last switch (DOM_MIN_HOLD_MS)
        // 2. New winner is significantly stronger than current (DOM_SWITCH_DELTA)
        let current_dom_val = self.smooth_chroma[self.dominant_pc];
        let new_top_val = self.smooth_chroma[raw_dominant];
        let time_since_switch = now_ms - self.last_dom_switch_ms;

        if raw_dominant != self.dominant_pc && time_since_switch > DOM_MIN_HOLD_MS {
            // Check if new winner is meaningfully better than current
            // Either: new is 12% stronger, OR current has dropped significantly
            let strength_threshold = self.last_dom_strength * (1.0 + DOM_SWITCH_DELTA);
            if new_top_val > strength_threshold || current_dom_val < self.last_dom_strength * 0.5 {
                let note_names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
                log::debug!("[Dominant] {} → {} (new={:.2} > old*1.12={:.2}, held {:.0}ms)",
                    note_names[self.dominant_pc], note_names[raw_dominant],
                    new_top_val, strength_threshold, time_since_switch);
                self.dominant_pc = raw_dominant;
                self.last_dom_switch_ms = now_ms;
                self.last_dom_strength = new_top_val;
            }
        }

        // If we haven't set last_dom_strength yet, initialize it
        if self.last_dom_strength == 0.0 && current_dom_val > 0.1 {
            self.last_dom_strength = current_dom_val;
        }

        // Step 6: Periodic logging with all 12 bins + stats
        if self.sample_count % (SAMPLE_RATE as u64 * 2) < FFT_SIZE as u64 {
            let note_names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
            log::debug!("[Chroma] {} | conf={:.2} top1={:.2} top2={:.2} mean={:.2} | tonal={}",
                self.smooth_chroma.iter().enumerate()
                    .map(|(i, v)| format!("{}:{:.2}", note_names[i], v))
                    .collect::<Vec<_>>().join(" "),
                self.tonal_confidence, top1, top2, mean, self.is_tonal);
        }

        // === KICK DETECTION (Ratio Gate + Envelope Rise) ===
        // Uses RAW FFT energy (not UI-scaled) for consistent detection across tracks

        let since_last = now_ms - self.kick_last_time_ms;

        // 1. Measure two bands (raw FFT energy, not UI-scaled)
        let kick_body_raw = self.band_energy(&self.magnitude_buffer, KICK_BODY_LOW, KICK_BODY_HIGH);
        let bass_tail_raw = self.band_energy(&self.magnitude_buffer, BASS_TAIL_LOW, BASS_TAIL_HIGH);

        // Log-compress for stability across loudness levels: x = ln(1 + k*x)
        let k = 50.0;  // Compression factor
        let kick_body = (1.0 + k * kick_body_raw).ln();
        let bass_tail = (1.0 + k * bass_tail_raw).ln();

        // 2. Ratio gate: kick body must dominate bass tail
        let eps = 0.001;
        let ratio = kick_body / (bass_tail + eps);
        let ratio_passed = ratio > KICK_RATIO_GATE;

        // 3. Fast envelope follower on kick body energy
        let alpha_env = self.compute_alpha(KICK_ENVELOPE_FC, dt_s);
        self.kick_envelope_lp += alpha_env * (kick_body - self.kick_envelope_lp);

        // 4. Rise detection: compute per-frame rise
        let rise = (self.kick_envelope_lp - self.prev_envelope).max(0.0);
        self.prev_envelope = self.kick_envelope_lp;

        // 5. Dynamic threshold via slow noise floor follower
        // Floor rises slowly, falls even slower
        if self.kick_envelope_lp > self.kick_floor {
            let alpha_rise = self.compute_alpha(KICK_FLOOR_RISE_FC, dt_s);
            self.kick_floor += alpha_rise * (self.kick_envelope_lp - self.kick_floor);
        } else {
            let alpha_fall = self.compute_alpha(KICK_FLOOR_FALL_FC, dt_s);
            self.kick_floor += alpha_fall * (self.kick_envelope_lp - self.kick_floor);
        }

        // Compute dynamic threshold from floor
        let env_threshold = self.kick_floor * KICK_FLOOR_MARGIN;
        let env_passed = self.kick_envelope_lp > env_threshold;
        let rise_passed = rise > KICK_RISE_THRESHOLD;

        // 6. Hysteresis rearm: rearm when envelope drops below threshold * (1 - hysteresis)
        let rearm_level = env_threshold * (1.0 - KICK_HYSTERESIS);
        if !self.kick_armed && self.kick_envelope_lp < rearm_level {
            self.kick_armed = true;
        }

        // 7. Trigger kick if all conditions met
        // Hold kick=true for ~5 analysis frames (~100ms) so UI can catch it
        const KICK_HOLD_FRAMES: u32 = 5;

        if since_last > KICK_REFRACTORY_MS && self.kick_armed && ratio_passed && env_passed && rise_passed {
            self.kick_hold_samples = KICK_HOLD_FRAMES;
            self.kick_last_time_ms = now_ms;
            self.kick_armed = false;  // Disarm until envelope drops
            log::debug!("[Kick] TRIGGERED! ratio={:.2} env={:.4} floor={:.4} rise={:.5}",
                ratio, self.kick_envelope_lp, self.kick_floor, rise);
        }

        // Decrement hold counter
        let kick = if self.kick_hold_samples > 0 {
            self.kick_hold_samples -= 1;
            true
        } else {
            false
        };

        // Kick intensity based on envelope (for UI visualization)
        let kick_intensity = (self.kick_envelope_lp * 3.0).min(1.0);

        // Click detection disabled
        let click = false;
        self.prev_spectrum.copy_from_slice(&self.magnitude_buffer);

        // Beat phase
        let elapsed = now_ms - self.beat_anchor_ms;
        let beat_phase = if self.beat_period_ms > 0.0 {
            (elapsed / self.beat_period_ms).fract()
        } else {
            0.0
        };

        AudioAnalysis {
            bass: self.smooth_bass,              // FAST - for UI display
            mid: self.smooth_mid,
            high: self.smooth_high,
            stable_bass: self.stable_bass,      // SLOW - for LED wall
            stable_mid: self.stable_mid,
            stable_high: self.stable_high,
            energy: self.smooth_energy,
            energy_vis: self.smooth_energy_vis,
            kick,
            kick_intensity,
            click,
            click_intensity: 0.0,  // Disabled
            bpm: self.estimated_bpm,
            beat_phase,
            chroma: self.smooth_chroma,            // SLOW - for dominant latch
            display_chroma: self.display_chroma,  // FAST - for shader/UI
            is_tonal: self.is_tonal,
            dominant_pc: self.dominant_pc,
            tonal_separation: self.tonal_confidence,  // Use smoothed confidence
            tonal_energy: self.tonal_energy,
            stability: self.stability,
            band_centroids: self.smooth_band_centroids,
            norm_clip_ratio: norm.clip_ratio_avg,
            norm_floor_ratio: norm.floor_ratio_avg,
            norm_p10_energy: norm.p10_energy,
            norm_p90_energy: norm.p90_energy,
            norm_warmup: norm.warmup_phase,
            raw_bass: self.last_raw_bass,
            raw_mid: self.last_raw_mid,
            raw_high: self.last_raw_high,
            // Continuous onset-detection function for the tempo autocorrelator:
            // the half-wave-rectified flux of the (log-compressed) kick envelope.
            // Autocorrelation normalizes by variance, so raw scale is fine.
            onset_strength: rise,
        }
    }

    /// Get current levels (for UI display)
    pub fn get_levels(&self) -> (f32, f32, f32) {
        (self.smooth_bass, self.smooth_mid, self.smooth_high)
    }
}

impl Default for AudioAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
