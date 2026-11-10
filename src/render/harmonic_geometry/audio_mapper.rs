//! Audio Forces and Uniform Mapping
//!
//! ## Architecture (v2)
//!
//! Audio is split into two layers:
//! - **AUTHORITATIVE** (slow, structural): beat_phase, macro_energy
//! - **EXPRESSIVE** (fast, shape-bending): bass_force, mid_force, high_force, kick_impulse
//!
//! **Key principle**: Audio bends shapes, not time. beat_phase comes from transport clock.

use super::harmonic_states::{InterpolatedState};

// ============================================================================
// AUDIO FORCES - Two-layer audio processing (authoritative + expressive)
// ============================================================================

/// Audio forces with macro/micro separation
///
/// - **Authoritative** forces drive structure (beat_phase, macro_energy)
/// - **Expressive** forces bend shapes (bass, mid, high, kick)
#[derive(Clone, Debug)]
pub struct AudioForces {
    // BPM for authoritative timing (default 120)
    bpm: f32,

    // === AUTHORITATIVE (slow, structural) ===

    /// Beat phase 0..1, advances at BPM rate from transport clock
    /// NOT from FFT - this is the clock that rules all timing
    pub beat_phase: f32,

    /// Macro energy smoothed over ~1 bar (very slow)
    /// Drives lock_strength in GateState
    pub macro_energy: f32,

    /// Global complexity (0..1) - "how mathematically rich is this moment"
    /// Drives softmin k, warp strength, surface frequency
    pub complexity: f32,

    // === EXPRESSIVE (fast, shape-bending) ===

    /// Bass force (smoothed, fast response)
    pub bass_force: f32,

    /// Mid force (smoothed, fast response)
    pub mid_force: f32,

    /// High force (smoothed, fast response)
    pub high_force: f32,

    /// Kick impulse (1.0 on kick, decays quickly)
    pub kick_impulse: f32,

    // Internal smoothing params
    fast_attack: f32,
    fast_decay: f32,
    macro_smooth: f32,  // Very slow smoothing for macro_energy

    // === HYSTERESIS STATE ===
    /// Whether we're in "active" (not silent) state - uses hysteresis
    is_active: bool,
}

impl Default for AudioForces {
    fn default() -> Self {
        Self::new(120.0)  // Default 120 BPM
    }
}

impl AudioForces {
    pub fn new(bpm: f32) -> Self {
        Self {
            bpm,
            beat_phase: 0.0,
            macro_energy: 0.0,
            complexity: 0.0,
            bass_force: 0.0,
            mid_force: 0.0,
            high_force: 0.0,
            kick_impulse: 0.0,
            fast_attack: 0.4,   // Fast attack for micro forces
            fast_decay: 0.15,   // Moderate decay
            macro_smooth: 0.02, // Very slow for macro (smoothed over ~1 bar)
            is_active: false,   // Start inactive, enter on energy > threshold
        }
    }

    /// Set BPM for beat phase calculation
    pub fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.clamp(60.0, 200.0);
    }

    /// Get current BPM
    pub fn bpm(&self) -> f32 {
        self.bpm
    }

    /// Update all audio forces from raw audio analysis
    ///
    /// **Critical**: beat_phase advances from transport clock (BPM), not FFT jitter
    pub fn update(&mut self, bass: f32, mid: f32, high: f32, energy: f32, kick: bool, dt: f32) {
        // === SILENCE GATE WITH HYSTERESIS ===
        // Enter active state at high threshold, exit only when below low threshold
        // Prevents flip-flop on borderline energy levels
        const SILENCE_THRESH_HI: f32 = 0.015;  // Enter active above this
        const SILENCE_THRESH_LO: f32 = 0.008;  // Exit active below this

        if self.is_active {
            if energy < SILENCE_THRESH_LO {
                self.is_active = false;
            }
        } else {
            if energy > SILENCE_THRESH_HI {
                self.is_active = true;
            }
        }

        if self.is_active {
            // === AUTHORITATIVE: beat_phase from transport clock ===
            // This is THE clock - audio timing jitter must NOT affect it
            let beats_per_second = self.bpm / 60.0;
            self.beat_phase = (self.beat_phase + dt * beats_per_second) % 1.0;
        }
        // If not active: beat_phase stays frozen at last value

        // === AUTHORITATIVE: macro_energy with exponential smoothing ===
        // tau = 0.15s gives smooth, non-abrupt changes
        let macro_target = energy;
        self.macro_energy = Self::exp_smooth(self.macro_energy, macro_target, dt, 0.15);

        // === AUTHORITATIVE: complexity from macro_energy ===
        // When music is intense, mathematics become rich
        let complexity_target = self.macro_energy.powf(0.8);
        self.complexity = Self::exp_smooth(self.complexity, complexity_target, dt, 0.12);

        // === EXPRESSIVE: micro forces with envelope + delta clamping ===
        self.bass_force = self.envelope_follow_clamped(self.bass_force, bass, dt);
        self.mid_force = self.envelope_follow_clamped(self.mid_force, mid, dt);
        self.high_force = self.envelope_follow_clamped(self.high_force, high, dt);

        // === EXPRESSIVE: kick impulse ===
        if kick {
            self.kick_impulse = 1.0;
        } else {
            self.kick_impulse *= 0.85;  // Fast decay
        }

        // Clean up floating point noise
        if self.kick_impulse < 0.001 {
            self.kick_impulse = 0.0;
        }
    }

    /// Exponential smoothing with time constant (critically damped feel)
    /// tau: time constant in seconds (0.12-0.25 feels good for visuals)
    fn exp_smooth(current: f32, target: f32, dt: f32, tau: f32) -> f32 {
        let alpha = 1.0 - (-dt / tau).exp();
        current + (target - current) * alpha
    }

    /// Envelope follower with attack/decay AND delta clamping
    /// Prevents sudden jumps while maintaining responsiveness
    fn envelope_follow_clamped(&self, current: f32, target: f32, _dt: f32) -> f32 {
        const MAX_STEP: f32 = 0.03;  // Max change per frame

        let raw_delta = if target > current {
            (target - current) * self.fast_attack
        } else {
            (target - current) * self.fast_decay
        };

        // Clamp the delta to prevent abrupt changes
        let clamped_delta = raw_delta.clamp(-MAX_STEP, MAX_STEP);
        current + clamped_delta
    }

    /// Envelope follower with attack/decay (unclamped, for backwards compat)
    fn envelope_follow(&self, current: f32, target: f32) -> f32 {
        if target > current {
            current + (target - current) * self.fast_attack
        } else {
            current + (target - current) * self.fast_decay
        }
    }

    /// Manually set beat phase (for sync with external transport)
    pub fn set_beat_phase(&mut self, phase: f32) {
        self.beat_phase = phase % 1.0;
    }

    /// Get beat phase (0..1)
    pub fn beat_phase(&self) -> f32 {
        self.beat_phase
    }

    /// Get macro energy (0..1, slow-smoothed)
    pub fn macro_energy(&self) -> f32 {
        self.macro_energy
    }

    /// Get complexity (0..1)
    pub fn complexity(&self) -> f32 {
        self.complexity
    }
}

// ============================================================================
// SHADER UNIFORMS - Values ready for GPU upload
// ============================================================================

/// Shader uniform values ready for upload to GPU
#[derive(Clone, Copy, Debug)]
pub struct ShaderUniforms {
    // Time
    pub time: f32,

    // Phase wheel
    pub theta: f32,
    pub gate_mode: i32,       // 0=off, 12, 24
    pub gate_sharpness: f32,
    pub gate_index: i32,
    pub gate_blend: f32,

    // Shape weights (sum to ~1)
    pub w_torus: f32,
    pub w_hyper: f32,
    pub w_gyroid: f32,

    // Morph value (0 = torus, 1 = hyperboloid) - latched and smoothed
    pub morph: f32,

    // Shape parameters
    pub torus_r: f32,
    pub torus_r2: f32,
    pub hyper_a: f32,
    pub hyper_b: f32,
    pub gyroid_freq: f32,
    pub gyroid_iso: f32,

    // Warp amounts
    pub twist: f32,
    pub bend: f32,
    pub ripple: f32,
    pub quad_warp: f32,

    // Audio bands (0-1)
    pub bass: f32,
    pub mid: f32,
    pub high: f32,
    pub energy: f32,

    // Effects
    pub kick: f32,            // Kick flash intensity (decays)
    pub glow: f32,            // Overall glow amount
    pub complexity: f32,      // Global complexity (0..1) - "mathematical richness"

    // Camera
    pub cam_dist: f32,
    pub cam_height: f32,
    pub cam_rotation: f32,

    // Polygon overlay
    pub poly_sides: f32,      // Number of polygon sides (0 = none)
    pub poly_rotation: f32,
    pub poly_scale: f32,
    pub poly_glow: f32,
}

impl Default for ShaderUniforms {
    fn default() -> Self {
        Self {
            time: 0.0,
            theta: 0.0,
            gate_mode: 12,
            gate_sharpness: 0.6,
            gate_index: 0,
            gate_blend: 0.0,

            // Default to torus-dominant (state A)
            w_torus: 0.85,
            w_hyper: 0.15,
            w_gyroid: 0.0,

            // Morph value (latched, smoothed by renderer)
            morph: 0.0,  // Start as pure torus

            // Torus params - exaggerate hole for clarity (R/r ~5:1)
            torus_r: 1.2,    // Major radius (was 1.0)
            torus_r2: 0.25,  // Tube radius (was 0.35) - thinner!
            hyper_a: 0.8,
            hyper_b: 1.2,
            gyroid_freq: 3.0,
            gyroid_iso: 0.1,

            // Minimal warps
            twist: 0.0,
            bend: 0.0,
            ripple: 0.0,
            quad_warp: 0.0,

            // No audio
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            energy: 0.0,

            // No effects
            kick: 0.0,
            glow: 0.3,
            complexity: 0.3,  // Start with moderate complexity

            // CRITICAL: Camera must be positioned to see the shape!
            cam_dist: 4.0,      // 4 units away from origin
            cam_height: 0.5,    // Slightly above origin
            cam_rotation: 0.0,

            // Polygon overlay
            poly_sides: 3.0,
            poly_rotation: 0.0,
            poly_scale: 0.3,
            poly_glow: 0.2,
        }
    }
}

/// Audio mapper with envelope following
///
/// Uses dual-speed smoothing for organic motion:
/// - Fast values (bass, mid, high, energy) respond quickly to audio
/// - Slow values (bass_slow, mid_slow, etc.) lag behind for smooth, organic feel
///
/// Use fast for immediate effects (twist, ripple)
/// Use slow for larger movements (bend, camera)
#[derive(Clone, Debug)]
pub struct AudioMapper {
    // Fast smoothed audio values (immediate response)
    bass: f32,
    mid: f32,
    high: f32,
    energy: f32,

    // Slow smoothed audio values (lagged for organic motion)
    bass_slow: f32,
    mid_slow: f32,
    high_slow: f32,
    energy_slow: f32,

    // Kick flash with decay
    kick_flash: f32,
    kick_intensity: f32,

    // Envelope parameters
    attack: f32,   // How fast values rise (0-1)
    decay: f32,    // How fast values fall (0-1)

    // Kick decay rate
    kick_decay: f32,

    // Time accumulator
    time: f32,

    // Audio modulation strengths
    pub bass_to_bend: f32,
    pub bass_to_torus_r2: f32,
    pub bass_to_hyper_b: f32,
    pub mid_to_twist: f32,
    pub mid_to_rotation: f32,
    pub high_to_ripple: f32,
    pub high_to_gyroid_freq: f32,
    pub high_to_glow: f32,
    pub energy_to_morph: f32,
    pub energy_to_smin_k: f32,
}

impl Default for AudioMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioMapper {
    pub fn new() -> Self {
        Self {
            // Fast values
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            energy: 0.0,
            // Slow values (start same as fast)
            bass_slow: 0.0,
            mid_slow: 0.0,
            high_slow: 0.0,
            energy_slow: 0.0,
            kick_flash: 0.0,
            kick_intensity: 0.0,
            attack: 0.35,      // Fast attack for responsiveness
            decay: 0.12,       // Slower decay for smoothness
            kick_decay: 0.85,  // Kick decays per frame
            time: 0.0,

            // Default modulation strengths (tunable per preset)
            bass_to_bend: 0.8,
            bass_to_torus_r2: 0.3,
            bass_to_hyper_b: 0.4,
            mid_to_twist: 0.6,
            mid_to_rotation: 0.5,
            high_to_ripple: 0.7,
            high_to_gyroid_freq: 0.5,
            high_to_glow: 0.8,
            energy_to_morph: 0.6,
            energy_to_smin_k: 0.5,
        }
    }

    /// Set envelope parameters
    pub fn set_envelope(&mut self, attack: f32, decay: f32) {
        self.attack = attack.clamp(0.01, 1.0);
        self.decay = decay.clamp(0.01, 1.0);
    }

    /// Update smoothed values from raw audio analysis
    ///
    /// Uses dual-speed smoothing:
    /// - Fast: immediate response for tight audio sync (twist, ripple)
    /// - Slow: lagged response for organic motion (bend, camera)
    ///
    /// The slow values follow the fast values, creating micro-lag that makes
    /// audio feel like the "cause" and geometry the "response".
    pub fn update(&mut self, bass: f32, mid: f32, high: f32, energy: f32, kick: bool, dt: f32) {
        self.time += dt;

        // === FAST LAYER: immediate audio response ===
        // Envelope following for each band (attack/decay smoothing)
        self.bass = self.envelope_follow(self.bass, bass);
        self.mid = self.envelope_follow(self.mid, mid);
        self.high = self.envelope_follow(self.high, high);
        self.energy = self.envelope_follow(self.energy, energy);

        // === SLOW LAYER: organic motion (follows fast with additional lag) ===
        // tau = 0.15s gives smooth, non-jumpy secondary response
        let slow_alpha = 1.0 - (-dt / 0.15).exp();
        self.bass_slow += (self.bass - self.bass_slow) * slow_alpha;
        self.mid_slow += (self.mid - self.mid_slow) * slow_alpha;
        self.high_slow += (self.high - self.high_slow) * slow_alpha;
        self.energy_slow += (self.energy - self.energy_slow) * slow_alpha;

        // Kick flash with decay
        if kick {
            self.kick_flash = 1.0;
            self.kick_intensity = 1.0;
        } else {
            self.kick_flash *= self.kick_decay;
            self.kick_intensity *= 0.95;
        }

        // Clamp kick flash to avoid floating point noise
        if self.kick_flash < 0.001 {
            self.kick_flash = 0.0;
        }
    }

    /// Envelope follower with attack/decay
    fn envelope_follow(&self, current: f32, target: f32) -> f32 {
        if target > current {
            // Attack
            current + (target - current) * self.attack
        } else {
            // Decay
            current + (target - current) * self.decay
        }
    }

    /// Get current smoothed values (for debugging/HUD)
    pub fn smoothed_values(&self) -> (f32, f32, f32, f32) {
        (self.bass, self.mid, self.high, self.energy)
    }

    /// Get kick flash intensity
    pub fn kick_flash(&self) -> f32 {
        self.kick_flash
    }

    /// Apply audio modulation to a harmonic state, producing final shader uniforms
    ///
    /// This is where the magic happens - audio drives the topology evolution
    pub fn apply_to_state(
        &self,
        state: &InterpolatedState,
        gate_mode: i32,
        gate_sharpness: f32,
        gate_index: i32,
        gate_blend: f32,
        theta: f32,
    ) -> ShaderUniforms {
        // Start with base state values
        let mut params = state.params;
        let mut warps = state.warps;

        // === AUDIO MODULATION (dual-speed) ===
        // Fast values: immediate effects (twist, ripple, glow)
        // Slow values: larger movements (bend, camera, shape params)
        // This creates organic motion where audio feels like "cause" and geometry "response"

        // Bass (SLOW) → bend, torus tube radius, hyperboloid b
        // Using slow bass for smooth, organic bend response
        warps.bend += self.bass_slow * self.bass_to_bend * warps.bend;
        params.torus_r2 += self.bass_slow * self.bass_to_torus_r2 * 0.2;
        params.hyper_b += self.bass_slow * self.bass_to_hyper_b * 0.3;

        // Mid (FAST) → twist (immediate response feels tight)
        // Mid (SLOW) → rotation speed (smooth camera movement)
        warps.twist += self.mid * self.mid_to_twist * warps.twist;
        let rotation_speed = self.mid_slow * self.mid_to_rotation;

        // High (FAST) → ripple, glow (immediate sparkle)
        // High (SLOW) → gyroid frequency (smooth complexity change)
        warps.ripple += self.high * self.high_to_ripple * warps.ripple;
        params.gyroid_freq += self.high_slow * self.high_to_gyroid_freq * 2.0;
        let glow = self.high * self.high_to_glow;

        // Energy (SLOW) → morph factor (smooth blending response)
        let morph_factor = 1.0 + self.energy_slow * self.energy_to_morph;

        // Kick flash (FAST) → boost quad_warp momentarily
        warps.quad_warp += self.kick_flash * 0.3;

        // 24-gate mode bonus: boost quadWarp and symmetry snap
        let quad_warp_boost = if gate_mode == 24 { 1.2 } else { 1.0 };
        warps.quad_warp *= quad_warp_boost;

        // Calculate camera (using SLOW values for smooth movement)
        let cam_dist = 4.0 - self.energy_slow * 0.5;  // Camera moves closer with energy
        let cam_height = 0.5 + self.mid_slow * 0.3;
        let cam_rotation = self.time * rotation_speed * 0.5;

        // Polygon overlay parameters
        let poly_sides = if state.symmetry == 0 {
            0.0  // Continuous = no polygon
        } else {
            state.symmetry as f32
        };
        let poly_rotation = self.time * 0.1 + self.mid * 0.5;
        let poly_scale = 0.3 + self.bass * 0.1;
        let poly_glow = glow * 0.5;

        ShaderUniforms {
            time: self.time,
            theta,
            gate_mode,
            gate_sharpness,
            gate_index,
            gate_blend,

            // Shape weights (modulated by morph factor)
            w_torus: state.shape_mix.torus / morph_factor,
            w_hyper: state.shape_mix.hyper / morph_factor,
            w_gyroid: state.shape_mix.gyroid / morph_factor,

            // Morph value (set to 0.0 here - renderer will override with latched/smoothed value)
            morph: 0.0,

            // Shape parameters
            torus_r: params.torus_r,
            torus_r2: params.torus_r2.clamp(0.1, 0.8),
            hyper_a: params.hyper_a,
            hyper_b: params.hyper_b.clamp(0.5, 2.0),
            gyroid_freq: params.gyroid_freq.clamp(1.0, 10.0),
            gyroid_iso: params.gyroid_iso,

            // Warp amounts (clamped to reasonable ranges)
            twist: warps.twist.clamp(0.0, 2.0),
            bend: warps.bend.clamp(0.0, 1.5),
            ripple: warps.ripple.clamp(0.0, 1.5),
            quad_warp: warps.quad_warp.clamp(0.0, 1.5),

            // Audio bands (raw smoothed values)
            bass: self.bass,
            mid: self.mid,
            high: self.high,
            energy: self.energy,

            // Effects
            kick: self.kick_flash,
            glow,
            complexity: 0.3,  // Default - renderer will override from AudioForces

            // Camera
            cam_dist,
            cam_height,
            cam_rotation,

            // Polygon overlay
            poly_sides,
            poly_rotation,
            poly_scale,
            poly_glow,
        }
    }
}

/// Preset-specific audio mapping configuration
#[derive(Clone, Debug)]
pub struct AudioMappingConfig {
    pub attack: f32,
    pub decay: f32,
    pub bass_to_bend: f32,
    pub bass_to_torus_r2: f32,
    pub bass_to_hyper_b: f32,
    pub mid_to_twist: f32,
    pub mid_to_rotation: f32,
    pub high_to_ripple: f32,
    pub high_to_gyroid_freq: f32,
    pub high_to_glow: f32,
    pub energy_to_morph: f32,
    pub energy_to_smin_k: f32,
}

impl Default for AudioMappingConfig {
    fn default() -> Self {
        Self {
            attack: 0.35,
            decay: 0.12,
            bass_to_bend: 0.8,
            bass_to_torus_r2: 0.3,
            bass_to_hyper_b: 0.4,
            mid_to_twist: 0.6,
            mid_to_rotation: 0.5,
            high_to_ripple: 0.7,
            high_to_gyroid_freq: 0.5,
            high_to_glow: 0.8,
            energy_to_morph: 0.6,
            energy_to_smin_k: 0.5,
        }
    }
}

impl AudioMapper {
    /// Apply a mapping configuration
    pub fn apply_config(&mut self, config: &AudioMappingConfig) {
        self.attack = config.attack;
        self.decay = config.decay;
        self.bass_to_bend = config.bass_to_bend;
        self.bass_to_torus_r2 = config.bass_to_torus_r2;
        self.bass_to_hyper_b = config.bass_to_hyper_b;
        self.mid_to_twist = config.mid_to_twist;
        self.mid_to_rotation = config.mid_to_rotation;
        self.high_to_ripple = config.high_to_ripple;
        self.high_to_gyroid_freq = config.high_to_gyroid_freq;
        self.high_to_glow = config.high_to_glow;
        self.energy_to_morph = config.energy_to_morph;
        self.energy_to_smin_k = config.energy_to_smin_k;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_envelope_follow() {
        let mut mapper = AudioMapper::new();

        // Rising
        mapper.update(1.0, 0.0, 0.0, 0.0, false, 0.016);
        assert!(mapper.bass > 0.0 && mapper.bass < 1.0);

        // Multiple frames should approach target
        for _ in 0..60 {
            mapper.update(1.0, 0.0, 0.0, 0.0, false, 0.016);
        }
        assert!(mapper.bass > 0.9);
    }

    #[test]
    fn test_kick_flash_decay() {
        let mut mapper = AudioMapper::new();

        // Trigger kick
        mapper.update(0.0, 0.0, 0.0, 0.0, true, 0.016);
        assert_eq!(mapper.kick_flash, 1.0);

        // Should decay
        mapper.update(0.0, 0.0, 0.0, 0.0, false, 0.016);
        assert!(mapper.kick_flash < 1.0);

        // After many frames, should be near zero
        for _ in 0..100 {
            mapper.update(0.0, 0.0, 0.0, 0.0, false, 0.016);
        }
        assert!(mapper.kick_flash < 0.01);
    }
}
