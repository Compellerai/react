//! Harmonic Field - Core Data Structure
//!
//! The harmonic field is the fundamental primitive for REG-aligned geometry.
//! It represents a 12-point radial profile where each spoke is controlled by
//! a pitch class (C through B) from the chroma vector.
//!
//! ## Philosophy
//!
//! Geometry emerges from number fields, not named solids.
//! The viewer should be able to "see harmony" - symmetry emerges from resonance,
//! disorder appears only when the music is noisy.

use std::f32::consts::TAU;

// ============================================================================
// RENDER AND SYMMETRY MODES
// ============================================================================

/// How the 2D harmonic profile is realized in 3D
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// Extrude 2D profile along Z axis
    /// Creates readable "harmonic column"
    #[default]
    Extrusion,

    /// Revolve profile around Y axis
    /// Creates smooth "audio sculpture"
    Lathe,

    /// Both extrusion and lathe visible simultaneously
    Both,
}

impl RenderMode {
    pub fn as_i32(&self) -> i32 {
        match self {
            RenderMode::Extrusion => 0,
            RenderMode::Lathe => 1,
            RenderMode::Both => 2,
        }
    }

    pub fn from_i32(v: i32) -> Self {
        match v {
            0 => RenderMode::Extrusion,
            1 => RenderMode::Lathe,
            _ => RenderMode::Both,
        }
    }
}

/// Symmetry order for the harmonic field
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SymmetryMode {
    /// Base 12-fold symmetry from raw chroma
    #[default]
    Twelve,

    /// 24-fold symmetry with octave reinforcement
    /// Creates inner + outer ring or phase-shifted duplicate
    TwentyFour,
}

impl SymmetryMode {
    pub fn as_i32(&self) -> i32 {
        match self {
            SymmetryMode::Twelve => 12,
            SymmetryMode::TwentyFour => 24,
        }
    }

    pub fn from_i32(v: i32) -> Self {
        if v >= 24 {
            SymmetryMode::TwentyFour
        } else {
            SymmetryMode::Twelve
        }
    }
}

// ============================================================================
// HARMONIC FIELD - Core Data Structure
// ============================================================================

/// The harmonic field - a 12-point radial profile driven by chroma
///
/// Each pitch class (C..B) controls one spoke of the radial profile:
/// ```
/// radius[i] = base_radius + chroma[i] * gain
/// ```
///
/// The 2D profile is smoothly interpolated between spokes (no hard corners).
/// This 2D shape is then extruded or lathed into 3D.
#[derive(Clone, Debug)]
pub struct HarmonicField {
    /// Raw 12-bin chroma from audio analysis (normalized 0-1)
    /// Index 0 = C, 1 = C#, 2 = D, ... 11 = B
    chroma: [f32; 12],

    /// Smoothed chroma values (hysteresis applied)
    chroma_smooth: [f32; 12],

    /// Slew-limited chroma (final output for geometry)
    /// This is what gets sent to the shader - prevents flopping
    chroma_slew: [f32; 12],

    /// Base radius when all chroma bins are zero (neutral circle)
    base_radius: f32,

    /// Gain multiplier for chroma → radius mapping
    gain: f32,

    /// Tonal confidence from key detector (0-1)
    /// When low, shape relaxes toward neutral circle
    tonal_confidence: f32,

    /// Overall tonal energy (sum of chroma magnitudes)
    /// Controls extrusion thickness
    tonal_energy: f32,

    /// Temporal stability (0-1) from analyzer
    /// Low stability = slow down geometry changes
    stability: f32,

    /// Trust factor: smoothstep(confidence) * smoothstep(stability)
    /// Geometry deformation is multiplied by this
    trust: f32,

    /// Smoothing time constant for chroma hysteresis (seconds)
    /// This is the BASE tau - actual tau adapts to stability
    smoothing_tau: f32,
}

impl Default for HarmonicField {
    fn default() -> Self {
        Self {
            chroma: [0.0; 12],
            chroma_smooth: [0.0; 12],
            chroma_slew: [0.0; 12],
            base_radius: 1.0,
            gain: 0.5,
            tonal_confidence: 0.0,
            tonal_energy: 0.0,
            stability: 0.5,
            trust: 0.0,
            smoothing_tau: 0.1, // 100ms hysteresis (base rate)
        }
    }
}

impl HarmonicField {
    /// Create a new harmonic field with specified parameters
    pub fn new(base_radius: f32, gain: f32) -> Self {
        Self {
            base_radius,
            gain,
            ..Default::default()
        }
    }

    /// Update the field with new chroma data
    ///
    /// Applies stability-adaptive smoothing + slew limiting to prevent flopping
    pub fn update(&mut self, chroma: &[f32; 12], tonal_confidence: f32, tonal_energy: f32, dt: f32) {
        self.update_with_stability(chroma, tonal_confidence, tonal_energy, 0.5, dt);
    }

    /// Full update with stability from analyzer
    ///
    /// Stability-adaptive smoothing:
    /// - Stable (>0.7): fast response (10-14 Hz)
    /// - Ambiguous (<0.5): slow response (2-4 Hz) to stop flopping
    ///
    /// Slew limiter caps per-frame change based on stability
    pub fn update_with_stability(
        &mut self,
        chroma: &[f32; 12],
        tonal_confidence: f32,
        tonal_energy: f32,
        stability: f32,
        dt: f32,
    ) {
        self.chroma = *chroma;
        self.tonal_confidence = tonal_confidence;
        self.tonal_energy = tonal_energy;
        self.stability = stability;

        // === COMPUTE TRUST FACTOR ===
        // trust = smoothstep(confidence) × smoothstep(stability)
        // Geometry deformation is gated by this
        let conf_trust = smoothstep(0.15, 0.35, tonal_confidence);
        let stab_trust = smoothstep(0.45, 0.75, stability);
        self.trust = conf_trust * stab_trust;

        // === STABILITY-ADAPTIVE SMOOTHING RATE ===
        // Stable: rate = 6 Hz (smooth, controlled)
        // Ambiguous: rate = 1.5 Hz (very slow, geological)
        let rate_stable = 6.0;
        let rate_ambiguous = 1.5;
        let rate = lerp(rate_ambiguous, rate_stable, stab_trust);
        let alpha = 1.0 - (-dt * rate).exp();

        // === FIRST PASS: EMA smooth chroma bins ===
        for i in 0..12 {
            self.chroma_smooth[i] += (self.chroma[i] - self.chroma_smooth[i]) * alpha;
        }

        // Apply relaxation toward neutral when confidence is low
        self.apply_relaxation(dt);

        // === SLEW LIMITER: Cap per-frame change ===
        // Stable: max_step = 0.04 (smooth but responsive)
        // Ambiguous: max_step = 0.008 (VERY slow, no flopping)
        let max_step_stable = 0.04;
        let max_step_ambiguous = 0.008;
        let max_step = lerp(max_step_ambiguous, max_step_stable, stab_trust);

        for i in 0..12 {
            let target = self.chroma_smooth[i];
            let current = self.chroma_slew[i];
            let delta = (target - current).clamp(-max_step, max_step);
            self.chroma_slew[i] = current + delta;
        }
    }

    /// Apply relaxation toward neutral circle when tonal confidence is low
    fn apply_relaxation(&mut self, dt: f32) {
        // Calculate average chroma (what neutral should converge to)
        let avg: f32 = self.chroma_smooth.iter().sum::<f32>() / 12.0;

        // Relaxation rate increases as confidence decreases
        // relax_rate = 0 at confidence = 1, ~2.0 at confidence = 0
        let relax_rate = (1.0 - self.tonal_confidence) * 2.0;
        let relax_alpha = 1.0 - (-dt * relax_rate).exp();

        for i in 0..12 {
            // Move each bin toward average when confidence is low
            self.chroma_smooth[i] += (avg - self.chroma_smooth[i]) * relax_alpha;
        }
    }

    /// Get radius at angle theta with smooth cubic interpolation
    ///
    /// theta is in radians, 0 = C position (right/east)
    /// Uses slew-limited chroma and applies trust factor to deformation
    pub fn radius_at(&self, theta: f32) -> f32 {
        // Normalize theta to [0, TAU)
        let theta_norm = theta.rem_euclid(TAU);

        // Map theta to bin index (0 = C at angle 0)
        // Each bin spans TAU/12 radians (30 degrees)
        let bin_angle = TAU / 12.0;
        let idx_f = theta_norm / bin_angle;

        // Get the two surrounding bins
        let idx0 = idx_f.floor() as usize % 12;
        let idx1 = (idx0 + 1) % 12;

        // Fractional position between bins
        let t = idx_f.fract();

        // Get radii for surrounding bins (using slew-limited chroma)
        // Apply trust factor: radius = base + chroma * gain * trust
        // When trust is low (ambiguous), geometry stays closer to base circle
        let effective_gain = self.gain * self.trust;
        let r0 = self.base_radius + self.chroma_slew[idx0] * effective_gain;
        let r1 = self.base_radius + self.chroma_slew[idx1] * effective_gain;

        // For cubic interpolation, we need 4 points
        let idx_m1 = (idx0 + 11) % 12; // Previous bin
        let idx2 = (idx1 + 1) % 12;     // Next-next bin

        let r_m1 = self.base_radius + self.chroma_slew[idx_m1] * effective_gain;
        let r2 = self.base_radius + self.chroma_slew[idx2] * effective_gain;

        // Catmull-Rom spline interpolation for smooth curves
        catmull_rom(r_m1, r0, r1, r2, t)
    }

    /// Generate a 2D profile as N points (for mesh generation or debugging)
    pub fn profile(&self, segments: usize) -> Vec<[f32; 2]> {
        let mut points = Vec::with_capacity(segments);

        for i in 0..segments {
            let theta = (i as f32 / segments as f32) * TAU;
            let r = self.radius_at(theta);
            points.push([r * theta.cos(), r * theta.sin()]);
        }

        points
    }

    /// Get the slew-limited chroma array (for shader upload)
    /// This is what should drive geometry - smooth and stable
    pub fn chroma_smooth(&self) -> &[f32; 12] {
        &self.chroma_slew
    }

    /// Get the EMA-smoothed chroma (before slew limiting)
    pub fn chroma_ema(&self) -> &[f32; 12] {
        &self.chroma_smooth
    }

    /// Get raw chroma array
    pub fn chroma_raw(&self) -> &[f32; 12] {
        &self.chroma
    }

    /// Get current stability value
    pub fn stability(&self) -> f32 {
        self.stability
    }

    /// Get computed trust factor
    /// This gates geometry deformation - multiply deformation by this
    pub fn trust(&self) -> f32 {
        self.trust
    }

    /// Get base radius
    pub fn base_radius(&self) -> f32 {
        self.base_radius
    }

    /// Set base radius
    pub fn set_base_radius(&mut self, r: f32) {
        self.base_radius = r.max(0.1);
    }

    /// Get gain
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Set gain
    pub fn set_gain(&mut self, g: f32) {
        self.gain = g.clamp(0.0, 2.0);
    }

    /// Get tonal confidence
    pub fn tonal_confidence(&self) -> f32 {
        self.tonal_confidence
    }

    /// Get tonal energy (for extrusion thickness)
    pub fn tonal_energy(&self) -> f32 {
        self.tonal_energy
    }

    /// Set smoothing time constant
    pub fn set_smoothing_tau(&mut self, tau: f32) {
        self.smoothing_tau = tau.max(0.01);
    }

    /// Set chroma directly (bypasses smoothing, for frozen playback)
    pub fn set_chroma_direct(&mut self, chroma: &[f32; 12]) {
        self.chroma = *chroma;
        self.chroma_smooth = *chroma;
        self.chroma_slew = *chroma; // Also set slew-limited to avoid jumps
    }

    /// Get the maximum radius (for bounding/camera)
    pub fn max_radius(&self) -> f32 {
        let max_chroma = self.chroma_slew.iter().cloned().fold(0.0f32, f32::max);
        self.base_radius + max_chroma * self.gain
    }

    /// Get the minimum radius (for bounding)
    pub fn min_radius(&self) -> f32 {
        let min_chroma = self.chroma_slew.iter().cloned().fold(1.0f32, f32::min);
        self.base_radius + min_chroma * self.gain
    }
}

// ============================================================================
// INTERPOLATION HELPERS
// ============================================================================

/// Linear interpolation
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smoothstep function (Hermite interpolation)
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Catmull-Rom spline interpolation for smooth curves
///
/// Given 4 control points p0, p1, p2, p3 and parameter t in [0,1],
/// returns smoothly interpolated value between p1 and p2.
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;

    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_circle() {
        let field = HarmonicField::default();

        // With all zeros, should be a perfect circle of base_radius
        for i in 0..12 {
            let theta = (i as f32 / 12.0) * TAU;
            let r = field.radius_at(theta);
            assert!((r - field.base_radius).abs() < 0.001,
                "Expected {}, got {} at theta {}", field.base_radius, r, theta);
        }
    }

    #[test]
    fn test_single_peak() {
        let mut field = HarmonicField::new(1.0, 0.5);

        // Set only C (index 0) to 1.0
        let mut chroma = [0.0; 12];
        chroma[0] = 1.0;

        // Drive a stable signal long enough for the adaptive EMA and slew limiter to converge.
        // `smoothing_tau` is no longer the update control path; geometry deformation is now
        // gated by stability-derived trust and capped by the slew limiter.
        for _ in 0..120 {
            field.update_with_stability(&chroma, 1.0, 1.0, 1.0, 1.0 / 60.0);
        }

        // Radius at C (theta = 0) should converge to base + gain
        let r_at_c = field.radius_at(0.0);
        assert!((r_at_c - 1.5).abs() < 0.1, "Expected ~1.5, got {}", r_at_c);

        // Radius at opposite side (F#, theta = PI) should be closer to base
        let r_at_fsharp = field.radius_at(std::f32::consts::PI);
        assert!(r_at_fsharp < r_at_c, "Expected radius at F# < radius at C");
    }

    #[test]
    fn test_relaxation() {
        let mut field = HarmonicField::new(1.0, 0.5);
        field.smoothing_tau = 0.001; // Fast smoothing

        // Set uneven chroma
        let chroma = [1.0, 0.0, 0.5, 0.0, 0.8, 0.0, 0.3, 0.0, 0.6, 0.0, 0.4, 0.0];
        field.update(&chroma, 1.0, 1.0, 1.0);

        let r_before = field.radius_at(0.0);

        // Now set low confidence and update many times
        for _ in 0..100 {
            field.update(&chroma, 0.0, 0.5, 0.016);
        }

        let r_after = field.radius_at(0.0);
        let r_opposite = field.radius_at(std::f32::consts::PI);

        // With low confidence, values should converge (relax toward average)
        assert!((r_after - r_opposite).abs() < (r_before - 1.0).abs(),
            "Expected relaxation, but delta is {} (was {})", r_after - r_opposite, r_before - 1.0);
    }

    #[test]
    fn test_profile_generation() {
        let field = HarmonicField::default();
        let profile = field.profile(360);

        assert_eq!(profile.len(), 360);

        // All points should be at base_radius distance from origin
        for [x, y] in &profile {
            let r = (x * x + y * y).sqrt();
            assert!((r - field.base_radius).abs() < 0.001);
        }
    }
}
