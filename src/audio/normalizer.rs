//! Percentile-based control signal normalizer for lighting.
//!
//! Replaces fixed scale factors (bass×300, etc.) that saturate everything to 1.0
//! with adaptive normalization using running p10/p90 percentile estimates.
//!
//! Shared by both Hue and OSC output paths — one normalization, two consumers.
//!
//! ## Algorithm
//! 1. Convert raw FFT magnitudes to log domain: `x = ln(1 + k * raw)`
//! 2. Track running p10 and p90 percentile estimates with asymmetric EMA
//! 3. Normalize: `norm = clamp((x - p10) / (p90 - p10), 0, 1)`
//! 4. Anti-clip: if too many outputs hit 1.0, expand headroom automatically
//!
//! ## Guardrails
//! - **Silence floor lock**: percentile updates freeze during silence to prevent
//!   p10 collapse → post-silence overreaction
//! - **Warmup cap**: energy capped during first seconds to prevent instant High zone
//! - **Floor ratio**: tracks fraction at 0.0, not just fraction at 1.0

/// Number of signals tracked
const NUM_SIGNALS: usize = 4; // bass, mid, high, energy

/// Signal indices
const SIG_BASS: usize = 0;
const SIG_MID: usize = 1;
const SIG_HIGH: usize = 2;
const SIG_ENERGY: usize = 3;

/// Log-compression factor (controls dynamic range mapping).
/// Higher = more compression of loud signals, better separation of quiet ones.
/// With k=200, raw 0.001 → ln(1.2)=0.18, raw 0.1 → ln(21)=3.04
const LOG_K: f32 = 200.0;

/// Base adaptation alpha per FFT frame (~170ms at 48kHz/8192).
/// Targets ~5s time constant: 1 - exp(-0.17/5) ≈ 0.033
const ADAPT_ALPHA: f32 = 0.033;

/// During warmup, adapt 10x faster so percentiles converge quickly
const WARMUP_ALPHA_MULT: f32 = 10.0;

/// Frames before warmup ends (~30 frames at ~5.9Hz ≈ 5 seconds)
const WARMUP_FRAMES: u32 = 30;

/// Asymmetric ratio for percentile tracking.
/// p10 moves fast downward (toward lower values), slow upward.
/// p90 moves fast upward (toward higher values), slow downward.
const ASYM_FAST: f32 = 0.9;
const ASYM_SLOW: f32 = 0.1;

/// Minimum effective normalization range (log domain).
///
/// When p90-p10 is narrower than this, the range is widened symmetrically
/// around the midpoint of p10/p90. This prevents amplifying frame-to-frame
/// noise to full 0–1 swings during steady music (where RMS varies ±10%
/// but that's just FFT window jitter, not musical dynamics).
///
/// At 0.8, steady music at ~0.6 RMS maps to ~0.3–0.7 normalized (good
/// dynamic range without noise amplification). Real quiet→loud transitions
/// (RMS 0.05→0.6) produce >2.0 log span and aren't affected.
const MIN_RANGE: f32 = 0.8;

/// Smoothing alpha for clip/floor ratio tracking
const RATIO_ALPHA: f32 = 0.02;

/// If clip_ratio exceeds this, expand headroom to reduce clipping
const CLIP_RATIO_THRESH: f32 = 0.15;

/// Headroom expansion factor when clipping detected
const HEADROOM_EXPAND: f32 = 1.05;

/// Rate at which headroom contracts back toward 1.0 (per frame)
const HEADROOM_CONTRACT: f32 = 0.998;

/// Maximum headroom expansion
const HEADROOM_MAX: f32 = 3.0;

/// RMS below this threshold = silence (freeze percentile updates)
const SILENCE_RMS_THRESH: f32 = 0.003;

/// Percentile update rate during silence (10x slower than normal slow rate)
/// Prevents p10 from collapsing during brief pauses
const SILENCE_ALPHA_MULT: f32 = 0.05;

/// Maximum energy_norm during warmup (ramps from this to 1.0)
/// Prevents instant High-zone lock on startup
const WARMUP_ENERGY_CAP_START: f32 = 0.55;

/// Percentile-based signal normalizer for lighting control.
///
/// Produces stable 0–1 control signals with real headroom from raw FFT data.
/// Both Hue and OSC consume the same normalized output.
pub struct ControlSignalNormalizer {
    /// p10 estimate per signal (log domain)
    p10: [f32; NUM_SIGNALS],
    /// p90 estimate per signal (log domain)
    p90: [f32; NUM_SIGNALS],
    /// Headroom multiplier per signal (≥1.0, expands range when clipping)
    headroom: [f32; NUM_SIGNALS],
    /// Smoothed clip ratio per signal (fraction of frames at 1.0)
    clip_ratio: [f32; NUM_SIGNALS],
    /// Smoothed floor ratio per signal (fraction of frames at 0.0)
    floor_ratio: [f32; NUM_SIGNALS],
    /// Frames processed (for warmup logic)
    frames_seen: u32,
    /// Whether first frame has been seen (for initialization)
    initialized: bool,
    /// Warmup blend factor: 0.0 at start, ramps to 1.0 over WARMUP_FRAMES
    warmup_phase: f32,
}

impl ControlSignalNormalizer {
    pub fn new() -> Self {
        Self {
            p10: [0.0; NUM_SIGNALS],
            p90: [1.0; NUM_SIGNALS],
            headroom: [1.0; NUM_SIGNALS],
            clip_ratio: [0.0; NUM_SIGNALS],
            floor_ratio: [0.0; NUM_SIGNALS],
            frames_seen: 0,
            initialized: false,
            warmup_phase: 0.0,
        }
    }

    /// Normalize raw audio features into stable 0–1 control signals.
    ///
    /// # Arguments
    /// * `bass_raw` - Raw FFT bass band energy (typically 0.001–0.1)
    /// * `mid_raw` - Raw FFT mid band energy
    /// * `high_raw` - Raw FFT high band energy
    /// * `rms` - Raw RMS energy
    ///
    /// # Returns
    /// `NormalizedSignals` with bass/mid/high/energy_raw/energy_vis in 0–1
    pub fn normalize(
        &mut self,
        bass_raw: f32,
        mid_raw: f32,
        high_raw: f32,
        rms: f32,
    ) -> NormalizedSignals {
        self.frames_seen += 1;

        // Ramp warmup_phase from 0→1 over WARMUP_FRAMES
        if self.frames_seen <= WARMUP_FRAMES {
            self.warmup_phase = self.frames_seen as f32 / WARMUP_FRAMES as f32;
        } else {
            self.warmup_phase = 1.0;
        }

        // Step 1: Convert to log domain for better dynamic range
        let log_vals = [
            log1p_k(bass_raw),
            log1p_k(mid_raw),
            log1p_k(high_raw),
            log1p_k(rms),
        ];

        // Step 2: Initialize percentiles from first frame
        if !self.initialized {
            for i in 0..NUM_SIGNALS {
                let x = log_vals[i];
                if x > 0.001 {
                    // Center percentiles around first observation with wide range
                    self.p10[i] = x * 0.3;
                    self.p90[i] = x * 3.0;
                } else {
                    // Near-silence: use safe defaults
                    self.p10[i] = 0.0;
                    self.p90[i] = 0.5;
                }
            }
            self.initialized = true;
        }

        // Step 3: Determine adaptation rate
        //
        // Three regimes:
        //   - Silence (rms < threshold): nearly frozen (prevents p10 collapse)
        //   - Warmup (first ~5s): 10x faster convergence
        //   - Normal: slow 5s time constant
        let is_silent = rms < SILENCE_RMS_THRESH;

        let alpha = if is_silent {
            // Silence: nearly freeze percentiles to prevent p10 collapse.
            // Without this, a 2-second pause would drop p10 so far that the
            // next sound maps to full scale.
            ADAPT_ALPHA * SILENCE_ALPHA_MULT
        } else if self.frames_seen < WARMUP_FRAMES {
            ADAPT_ALPHA * WARMUP_ALPHA_MULT
        } else {
            ADAPT_ALPHA
        };

        // Step 4: Update percentile estimates with asymmetric EMA
        for i in 0..NUM_SIGNALS {
            let x = log_vals[i];

            // p10: fast downward, slow upward (tracks low end)
            if x < self.p10[i] {
                self.p10[i] += alpha * ASYM_FAST * (x - self.p10[i]);
            } else {
                self.p10[i] += alpha * ASYM_SLOW * (x - self.p10[i]);
            }

            // p90: fast upward, slow downward (tracks high end)
            if x > self.p90[i] {
                self.p90[i] += alpha * ASYM_FAST * (x - self.p90[i]);
            } else {
                self.p90[i] += alpha * ASYM_SLOW * (x - self.p90[i]);
            }

            // Note: we no longer force p90 upward here. Instead, Step 5
            // centers the MIN_RANGE expansion around the midpoint of p10/p90,
            // so steady music maps to ~0.5 instead of ~0.16.
        }

        // Step 5: Normalize with centered range expansion
        //
        // When p90-p10 is narrower than MIN_RANGE (steady music), we expand
        // symmetrically around the midpoint so the typical signal lands at ~0.5.
        // This prevents the old problem where extending from p10 mapped active
        // music to 0.16 (too low) or amplified jitter to wild 0-1 swings.
        let mut normed = [0.0f32; NUM_SIGNALS];
        for i in 0..NUM_SIGNALS {
            let mid = (self.p10[i] + self.p90[i]) / 2.0;
            let actual_range = (self.p90[i] - self.p10[i]) * self.headroom[i];
            let effective_range = actual_range.max(MIN_RANGE);
            let effective_low = mid - effective_range / 2.0;
            normed[i] = ((log_vals[i] - effective_low) / effective_range).clamp(0.0, 1.0);

            // Step 6: Anti-clip headroom adjustment
            let is_clipped = if normed[i] >= 0.99 { 1.0f32 } else { 0.0 };
            self.clip_ratio[i] += RATIO_ALPHA * (is_clipped - self.clip_ratio[i]);

            let is_floored = if normed[i] <= 0.01 { 1.0f32 } else { 0.0 };
            self.floor_ratio[i] += RATIO_ALPHA * (is_floored - self.floor_ratio[i]);

            if self.clip_ratio[i] > CLIP_RATIO_THRESH {
                self.headroom[i] *= HEADROOM_EXPAND;
            } else {
                // Slowly contract back toward 1.0
                self.headroom[i] = 1.0 + (self.headroom[i] - 1.0) * HEADROOM_CONTRACT;
            }
            self.headroom[i] = self.headroom[i].clamp(1.0, HEADROOM_MAX);
        }

        // Step 7: Warmup energy cap
        // During the first few seconds, cap energy_norm to prevent instant High zone.
        // Ramps from WARMUP_ENERGY_CAP_START → 1.0 over warmup period.
        let energy_cap = if self.warmup_phase < 1.0 {
            WARMUP_ENERGY_CAP_START + (1.0 - WARMUP_ENERGY_CAP_START) * self.warmup_phase
        } else {
            1.0
        };
        normed[SIG_ENERGY] = normed[SIG_ENERGY].min(energy_cap);

        // Energy vis: perceptual curve on normalized energy (expand low values)
        let energy_vis = normed[SIG_ENERGY].powf(0.7);

        NormalizedSignals {
            bass: normed[SIG_BASS],
            mid: normed[SIG_MID],
            high: normed[SIG_HIGH],
            energy_raw: normed[SIG_ENERGY],
            energy_vis,
            // Diagnostics
            clip_ratio_avg: self.clip_ratio.iter().sum::<f32>() / NUM_SIGNALS as f32,
            floor_ratio_avg: self.floor_ratio.iter().sum::<f32>() / NUM_SIGNALS as f32,
            p10_energy: self.p10[SIG_ENERGY],
            p90_energy: self.p90[SIG_ENERGY],
            warmup_phase: self.warmup_phase,
        }
    }

    /// Diagnostic: average clip ratio across all signals (0 = never clips, 1 = always clips)
    pub fn clip_ratio_avg(&self) -> f32 {
        self.clip_ratio.iter().sum::<f32>() / NUM_SIGNALS as f32
    }

    /// Diagnostic: average floor ratio across all signals (0 = never floored, 1 = always 0)
    pub fn floor_ratio_avg(&self) -> f32 {
        self.floor_ratio.iter().sum::<f32>() / NUM_SIGNALS as f32
    }

    /// Diagnostic: per-signal percentile ranges (for logging)
    pub fn percentile_ranges(&self) -> [(f32, f32); NUM_SIGNALS] {
        let mut ranges = [(0.0f32, 0.0f32); NUM_SIGNALS];
        for i in 0..NUM_SIGNALS {
            ranges[i] = (self.p10[i], self.p90[i]);
        }
        ranges
    }
}

impl Default for ControlSignalNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalized output signals for lighting control.
///
/// All values in 0.0–1.0 with real headroom (should NOT be pinned at 1.0).
#[derive(Clone, Copy, Debug)]
pub struct NormalizedSignals {
    /// Normalized bass band energy
    pub bass: f32,
    /// Normalized mid band energy
    pub mid: f32,
    /// Normalized high band energy
    pub high: f32,
    /// Normalized overall energy (linear)
    pub energy_raw: f32,
    /// Normalized perceptual energy (power curve for visual display)
    pub energy_vis: f32,
    // === Diagnostics ===
    /// Average clip ratio across signals (0 = healthy, >0.15 = expanding headroom)
    pub clip_ratio_avg: f32,
    /// Average floor ratio across signals (0 = healthy, high = everything floored)
    pub floor_ratio_avg: f32,
    /// p10 energy percentile (log domain, for debug logging)
    pub p10_energy: f32,
    /// p90 energy percentile (log domain, for debug logging)
    pub p90_energy: f32,
    /// Warmup phase (0.0 = just started, 1.0 = fully converged)
    pub warmup_phase: f32,
}

/// Log-compress with factor k: ln(1 + k * x)
#[inline]
fn log1p_k(x: f32) -> f32 {
    (1.0 + LOG_K * x.max(0.0)).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log1p_k_monotonic() {
        // Higher raw values should produce higher log values
        assert!(log1p_k(0.01) < log1p_k(0.1));
        assert!(log1p_k(0.1) < log1p_k(1.0));
        assert!(log1p_k(0.0) >= 0.0);
    }

    #[test]
    fn test_normalizer_not_stuck_at_one() {
        let mut norm = ControlSignalNormalizer::new();

        // Simulate 100 frames of varying input (not constant)
        let mut results = Vec::new();
        for i in 0..100 {
            let t = i as f32 / 100.0;
            // Simulate bass varying between 0.01 and 0.05
            let bass = 0.01 + 0.04 * (t * std::f32::consts::TAU).sin().abs();
            let mid = 0.002 + 0.008 * (t * 2.0 * std::f32::consts::TAU).sin().abs();
            let high = 0.0001 + 0.001 * (t * 3.0 * std::f32::consts::TAU).sin().abs();
            let rms = 0.02 + 0.08 * (t * std::f32::consts::TAU).sin().abs();

            let sig = norm.normalize(bass, mid, high, rms);
            results.push(sig);
        }

        // After convergence, values should NOT all be 1.0
        let last_20: Vec<_> = results[80..].to_vec();
        let avg_bass: f32 = last_20.iter().map(|s| s.bass).sum::<f32>() / 20.0;
        let avg_energy: f32 = last_20.iter().map(|s| s.energy_raw).sum::<f32>() / 20.0;

        // Average should be well below 1.0 (good distribution)
        assert!(avg_bass < 0.9, "Bass average too high: {}", avg_bass);
        assert!(avg_energy < 0.9, "Energy average too high: {}", avg_energy);

        // Should also not all be 0.0
        assert!(avg_bass > 0.1, "Bass average too low: {}", avg_bass);
        assert!(avg_energy > 0.1, "Energy average too low: {}", avg_energy);
    }

    #[test]
    fn test_normalizer_handles_silence() {
        let mut norm = ControlSignalNormalizer::new();

        // Feed silence
        for _ in 0..50 {
            let sig = norm.normalize(0.0, 0.0, 0.0, 0.0);
            // Should not panic or produce NaN
            assert!(!sig.bass.is_nan());
            assert!(!sig.energy_raw.is_nan());
            assert!(sig.bass >= 0.0 && sig.bass <= 1.0);
        }
    }

    #[test]
    fn test_normalizer_handles_constant_input() {
        let mut norm = ControlSignalNormalizer::new();

        // Feed constant values (the old fixed-scale scenario)
        for _ in 0..100 {
            let sig = norm.normalize(0.03, 0.004, 0.0002, 0.06);
            assert!(!sig.bass.is_nan());
            assert!(sig.bass >= 0.0 && sig.bass <= 1.0);
        }

        // After convergence with constant input, should settle near middle
        // (p10 and p90 will converge toward the same value, but MIN_RANGE prevents crash)
        let sig = norm.normalize(0.03, 0.004, 0.0002, 0.06);
        assert!(sig.bass >= 0.0 && sig.bass <= 1.0);
    }

    #[test]
    fn test_silence_does_not_collapse_p10() {
        let mut norm = ControlSignalNormalizer::new();

        // Feed music for 50 frames to establish percentiles
        for _ in 0..50 {
            norm.normalize(0.03, 0.004, 0.0002, 0.06);
        }
        let p10_before = norm.p10[SIG_ENERGY];

        // Feed 50 frames of silence
        for _ in 0..50 {
            norm.normalize(0.0, 0.0, 0.0, 0.0);
        }
        let p10_after = norm.p10[SIG_ENERGY];

        // p10 should NOT have collapsed dramatically (silence floor lock)
        // Allow some drift but not more than 50% drop
        assert!(
            p10_after > p10_before * 0.5,
            "p10 collapsed too much during silence: before={:.4} after={:.4}",
            p10_before, p10_after
        );
    }

    #[test]
    fn test_post_silence_not_overreaction() {
        let mut norm = ControlSignalNormalizer::new();

        // Establish normal levels
        for _ in 0..60 {
            norm.normalize(0.03, 0.004, 0.0002, 0.06);
        }

        // Brief silence (10 frames ≈ 1.7s)
        for _ in 0..10 {
            norm.normalize(0.0, 0.0, 0.0, 0.0);
        }

        // Resume at same level — should NOT jump to 1.0
        let sig = norm.normalize(0.03, 0.004, 0.0002, 0.06);
        assert!(
            sig.energy_raw < 0.95,
            "Post-silence energy overreaction: {:.3} (expected < 0.95)",
            sig.energy_raw
        );
    }

    #[test]
    fn test_warmup_caps_energy() {
        let mut norm = ControlSignalNormalizer::new();

        // First frame with loud input — should be capped by warmup
        let sig = norm.normalize(0.1, 0.02, 0.005, 0.5);
        assert!(
            sig.energy_raw <= WARMUP_ENERGY_CAP_START + 0.05,
            "Warmup didn't cap energy on first frame: {:.3}",
            sig.energy_raw
        );
    }
}
