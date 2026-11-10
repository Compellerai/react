//! 12/24 Harmonic Gate System
//!
//! The gate system is the core mathematical engine that maps beat phase to
//! harmonic states. Based on REG's icositetragon (24-gon) and 12-tone music theory.
//!
//! Key concepts:
//! - Gates are quantization points on the phase wheel (0..1)
//! - 12-gate mode: Each gate = 30 degrees = one semitone
//! - 24-gate mode: Each gate = 15 degrees = half-semitone (more granular)
//! - Magnetic snap: Phase "sticks" to gates with adjustable sharpness
//! - Shapes EVOLVE continuously - never discrete switching
//!
//! The 12/24 logic is explicit and inspectable for future HUD overlay.

use std::f32::consts::TAU;

/// Gate quantization mode
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateCount {
    /// No quantization - continuous flow
    Off,
    /// 12 gates (30 degrees each) - semitone resolution
    Twelve,
    /// 24 gates (15 degrees each) - half-semitone resolution
    TwentyFour,
}

impl GateCount {
    /// Number of gates (0 = continuous)
    pub fn count(&self) -> u8 {
        match self {
            GateCount::Off => 0,
            GateCount::Twelve => 12,
            GateCount::TwentyFour => 24,
        }
    }

    /// Angle per gate in radians
    pub fn angle_per_gate(&self) -> f32 {
        match self {
            GateCount::Off => TAU,
            GateCount::Twelve => TAU / 12.0,  // 30 degrees
            GateCount::TwentyFour => TAU / 24.0, // 15 degrees
        }
    }

    /// Angle per gate in degrees
    pub fn degrees_per_gate(&self) -> f32 {
        match self {
            GateCount::Off => 360.0,
            GateCount::Twelve => 30.0,
            GateCount::TwentyFour => 15.0,
        }
    }
}

impl Default for GateCount {
    fn default() -> Self {
        GateCount::Twelve
    }
}

/// Transition behavior state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionState {
    /// Locked to current gate (no phase movement)
    Locked,
    /// Flowing with beat phase (smooth evolution)
    Flow,
    /// Drop mode - instant snap to gate on kick
    Drop,
}

impl Default for TransitionState {
    fn default() -> Self {
        TransitionState::Flow
    }
}

/// The gate system state machine
///
/// Manages the phase wheel position, gate quantization, and smooth
/// state transitions. All internal state is inspectable for HUD display.
///
/// ## Lock Strength (v2 Architecture)
///
/// The gate system now has `lock_strength` driven by `macro_energy`:
/// - Low energy → soft magnetic pull (states blend smoothly)
/// - High energy → authoritative snapping (states click into place)
///
/// This makes transitions feel *inevitable*, not reactive.
#[derive(Clone, Debug)]
pub struct GateSystem {
    /// Number of gates (12 or 24, or Off for continuous)
    gate_count: GateCount,

    /// Current gate index (0..gate_count-1)
    /// For 12-gate: 0=A, 1=A#, 2=B, ... 11=G#
    /// For 24-gate: subdivides each semitone
    current_gate: u8,

    /// Blend factor to next gate (0.0 = at current, 1.0 = at next)
    /// This enables smooth evolution between states
    blend: f32,

    /// Raw phase wheel position (0.0 to 1.0)
    /// Represents position on the circle (0 = 12 o'clock, 0.5 = 6 o'clock)
    theta: f32,

    /// Quantized theta after magnetic snap
    quantized_theta: f32,

    /// Transition behavior
    state: TransitionState,

    /// Magnetic snap sharpness (0.0 = no snap, 1.0 = hard snap)
    /// Higher values make phase "stick" to gates more
    sharpness: f32,

    /// Lock strength (0.0 - 1.0), driven by macro_energy
    /// Low energy → soft magnetic pull
    /// High energy → authoritative snap
    lock_strength: f32,

    /// Phase velocity (for momentum-based smoothing)
    phase_velocity: f32,

    /// Time accumulator for animation
    time: f32,

    /// Last kick time for gate advance
    last_kick_time: f32,

    /// Kick cooldown to prevent rapid re-triggering
    kick_cooldown: f32,
}

impl Default for GateSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl GateSystem {
    pub fn new() -> Self {
        Self {
            gate_count: GateCount::Twelve,
            current_gate: 0,
            blend: 0.0,
            theta: 0.0,
            quantized_theta: 0.0,
            state: TransitionState::Flow,
            sharpness: 0.7,
            lock_strength: 0.0,
            phase_velocity: 0.0,
            time: 0.0,
            last_kick_time: -1.0,
            kick_cooldown: 0.0,
        }
    }

    // === Configuration ===

    /// Set gate count (12, 24, or Off)
    pub fn set_gate_count(&mut self, count: GateCount) {
        self.gate_count = count;
        // Recalculate current gate for new count
        self.update_gate_from_theta();
    }

    /// Set magnetic snap sharpness (0.0 - 1.0)
    pub fn set_sharpness(&mut self, sharpness: f32) {
        self.sharpness = sharpness.clamp(0.0, 1.0);
    }

    /// Set transition state
    pub fn set_transition_state(&mut self, state: TransitionState) {
        self.state = state;
    }

    // === Inspectable State (for HUD) ===

    /// Current gate index (0-11 for 12-gate, 0-23 for 24-gate)
    pub fn current_gate(&self) -> u8 {
        self.current_gate
    }

    /// Gate count mode
    pub fn gate_count(&self) -> GateCount {
        self.gate_count
    }

    /// Blend factor to next gate (0.0 - 1.0)
    pub fn blend(&self) -> f32 {
        self.blend
    }

    /// Raw phase (0.0 - 1.0)
    pub fn raw_theta(&self) -> f32 {
        self.theta
    }

    /// Quantized phase after magnetic snap (0.0 - 1.0)
    pub fn quantized_theta(&self) -> f32 {
        self.quantized_theta
    }

    /// Current sharpness setting
    pub fn sharpness(&self) -> f32 {
        self.sharpness
    }

    /// Lock strength (0.0 - 1.0), derived from macro_energy
    /// Low = soft magnetic pull, High = authoritative snap
    pub fn lock_strength(&self) -> f32 {
        self.lock_strength
    }

    /// Current transition state
    pub fn transition_state(&self) -> TransitionState {
        self.state
    }

    /// Phase in degrees (0 - 360)
    pub fn phase_degrees(&self) -> f32 {
        self.quantized_theta * 360.0
    }

    /// Phase in radians (0 - TAU)
    pub fn phase_radians(&self) -> f32 {
        self.quantized_theta * TAU
    }

    /// Current harmonic state index (0-11, maps to HARMONIC_STATES)
    /// For 24-gate mode, this is floor(gate / 2)
    pub fn harmonic_state_index(&self) -> u8 {
        match self.gate_count {
            GateCount::Off => (self.theta * 12.0).floor() as u8 % 12,
            GateCount::Twelve => self.current_gate % 12,
            GateCount::TwentyFour => (self.current_gate / 2) % 12,
        }
    }

    /// Sub-gate blend for 24-gate mode (0.0 - 1.0 within harmonic state)
    /// For 12-gate mode, this equals self.blend
    pub fn sub_gate_blend(&self) -> f32 {
        match self.gate_count {
            GateCount::Off => (self.theta * 12.0).fract(),
            GateCount::Twelve => self.blend,
            GateCount::TwentyFour => {
                // In 24-gate, each harmonic state spans 2 gates
                // Blend within the pair
                let gate_in_pair = self.current_gate % 2;
                if gate_in_pair == 0 {
                    self.blend * 0.5  // First half
                } else {
                    0.5 + self.blend * 0.5  // Second half
                }
            }
        }
    }

    // === Update Logic ===

    /// Main update function - call once per frame
    ///
    /// Arguments:
    /// - beat_phase: 0.0-1.0 from transport clock (AUTHORITATIVE, not from FFT)
    /// - macro_energy: 0.0-1.0 slow-smoothed energy level (drives lock_strength)
    /// - kick: true if kick detected this frame
    /// - dt: delta time in seconds
    pub fn update(&mut self, beat_phase: f32, macro_energy: f32, kick: bool, dt: f32) {
        self.time += dt;
        self.kick_cooldown = (self.kick_cooldown - dt).max(0.0);

        // Calculate lock_strength from macro_energy
        // Power curve for better feel: low energy = soft pull, high energy = hard snap
        // Using 0.7 power gives a nice curve where energy has to build up
        self.lock_strength = macro_energy.powf(0.7);

        // Handle different transition states
        match self.state {
            TransitionState::Locked => {
                // Don't update theta - stay at current gate
                self.quantized_theta = self.current_gate as f32 / self.gate_count.count().max(1) as f32;
            }
            TransitionState::Flow => {
                // Smoothly follow beat phase
                self.update_theta_flow(beat_phase, dt);
            }
            TransitionState::Drop => {
                // Flow normally, but snap to gate on kick
                self.update_theta_flow(beat_phase, dt);
                if kick && self.kick_cooldown <= 0.0 {
                    self.advance_gate();
                    self.kick_cooldown = 0.1; // 100ms cooldown
                    self.last_kick_time = self.time;
                }
            }
        }

        // Apply magnetic snap using lock_strength (derived from macro_energy)
        self.quantized_theta = self.apply_magnetic_snap(self.theta);

        // Update current gate and blend from quantized theta
        self.update_gate_from_theta();
    }

    /// Update theta in Flow mode - smooth following with momentum
    fn update_theta_flow(&mut self, beat_phase: f32, dt: f32) {
        // Calculate target theta from beat phase
        let target = beat_phase.fract();

        // Handle wrap-around (e.g., 0.99 -> 0.01)
        let mut delta = target - self.theta;
        if delta > 0.5 {
            delta -= 1.0;
        } else if delta < -0.5 {
            delta += 1.0;
        }

        // Smooth following with attack/decay
        let follow_rate = if delta.abs() > 0.1 { 0.3 } else { 0.15 };
        self.phase_velocity = self.phase_velocity * 0.8 + delta * follow_rate;
        self.theta = (self.theta + self.phase_velocity * dt * 60.0).rem_euclid(1.0);
    }

    /// Apply magnetic snap - makes phase "stick" to gates
    ///
    /// This is the key "REG feel" function. The snap uses a smoothstep
    /// that gets sharper with higher lock_strength (snappier when energy builds).
    ///
    /// ## Lock Strength Behavior (v2 Architecture)
    ///
    /// - lock_strength = 0 → soft magnetic pull (states blend smoothly)
    /// - lock_strength = 1 → authoritative snap (gates click into place)
    ///
    /// This makes transitions feel *inevitable*, not reactive.
    fn apply_magnetic_snap(&self, theta: f32) -> f32 {
        self.apply_magnetic_snap_with_lock(theta, self.lock_strength)
    }

    /// Apply magnetic snap with explicit lock_strength (for timeline generation)
    fn apply_magnetic_snap_with_lock(&self, theta: f32, lock_strength: f32) -> f32 {
        if self.gate_count == GateCount::Off {
            return theta;  // No quantization
        }

        let gates = self.gate_count.count() as f32;

        // Dynamic sharpness: gets snappier with lock_strength
        // lock_strength = 0 → soft (k = 0.5 * (1 - sharpness))
        // lock_strength = 1 → hard snap (k approaches 0)
        let dynamic_k = self.sharpness * (1.0 + lock_strength * 0.5);
        let k = 0.5 * (1.0 - dynamic_k.min(0.95));  // k shrinks as sharpness rises, clamped to avoid division issues

        // The magnetic snap formula
        // Quantizes theta to gates with soft edges
        Self::gate_quantize(theta, gates, k)
    }

    /// Gate quantization with magnetic snap
    ///
    /// Mathematical formula: smoothstep-based soft quantization
    /// k = 0.5 → linear (no snap)
    /// k = 0.0 → hard snap
    fn gate_quantize(t: f32, gates: f32, k: f32) -> f32 {
        // Scale to gate space
        let scaled = t * gates;
        let gate_idx = scaled.floor();
        let fract = scaled - gate_idx;

        // Apply smoothstep-like snap
        let snapped = if k > 0.0 {
            // Soft snap with configurable sharpness
            let edge0 = 0.5 - k;
            let edge1 = 0.5 + k;
            if fract < edge0 {
                0.0
            } else if fract > edge1 {
                1.0
            } else {
                // Smoothstep in the middle region
                let t = (fract - edge0) / (edge1 - edge0);
                t * t * (3.0 - 2.0 * t)
            }
        } else {
            // Hard snap
            if fract < 0.5 { 0.0 } else { 1.0 }
        };

        // Convert back to 0-1 range
        ((gate_idx + snapped) / gates).rem_euclid(1.0)
    }

    /// Update current gate and blend from quantized theta
    fn update_gate_from_theta(&mut self) {
        if self.gate_count == GateCount::Off {
            self.current_gate = 0;
            self.blend = self.theta * 12.0;
            return;
        }

        let gates = self.gate_count.count() as f32;
        let scaled = self.quantized_theta * gates;

        self.current_gate = (scaled.floor() as u8) % self.gate_count.count();
        self.blend = scaled.fract();
    }

    /// Advance to next gate (for Drop mode on kick)
    fn advance_gate(&mut self) {
        if self.gate_count == GateCount::Off {
            return;
        }

        self.current_gate = (self.current_gate + 1) % self.gate_count.count();
        self.theta = self.current_gate as f32 / self.gate_count.count() as f32;
        self.blend = 0.0;
    }

    // === Manual Control (for scrubbing) ===

    /// Set theta directly (for freeze-frame scrubbing)
    pub fn set_theta(&mut self, theta: f32) {
        self.theta = theta.rem_euclid(1.0);
        self.quantized_theta = self.theta;
        self.update_gate_from_theta();
    }

    /// Set to specific gate (for manual selection)
    pub fn set_gate(&mut self, gate: u8) {
        let max_gate = match self.gate_count {
            GateCount::Off => 12,
            GateCount::Twelve => 12,
            GateCount::TwentyFour => 24,
        };
        self.current_gate = gate % max_gate;
        self.theta = self.current_gate as f32 / max_gate as f32;
        self.quantized_theta = self.theta;
        self.blend = 0.0;
    }
}

/// Timeline entry for export
#[derive(Clone, Debug)]
pub struct TimelineEntry {
    pub time: f32,
    pub phase: f32,
    pub gate: u8,
    pub blend: f32,
    pub harmonic_state: u8,
    pub topology_name: String,
}

impl GateSystem {
    /// Generate timeline entries for a given duration and step
    /// Useful for exporting phase → shape mapping
    pub fn generate_timeline(&self, duration: f32, step: f32) -> Vec<TimelineEntry> {
        let mut entries = Vec::new();
        let mut t = 0.0;

        while t <= duration {
            let phase = (t / duration).fract();
            let quantized = self.apply_magnetic_snap_with_lock(phase, 0.5);
            let gates = self.gate_count.count().max(1) as f32;
            let scaled = quantized * gates;
            let gate = (scaled.floor() as u8) % self.gate_count.count().max(1);
            let blend = scaled.fract();
            let harmonic_state = match self.gate_count {
                GateCount::Off => (phase * 12.0).floor() as u8 % 12,
                GateCount::Twelve => gate % 12,
                GateCount::TwentyFour => (gate / 2) % 12,
            };

            let topology_name = match harmonic_state {
                0 => "A: Torus Prime",
                1 => "A#: Octahedral Skin",
                2 => "B: Dodecahedral Blend",
                3 => "C: Cubic Balance",
                4 => "C#: Hyperbolic Rise",
                5 => "D: Icosahedral Emergence",
                6 => "D#: Gyroid Awakening",
                7 => "E: Gyroid Dominant",
                8 => "F: Gyroid Cathedral",
                9 => "F#: Light Speed Reference",
                10 => "G: 24-Wheel Cathedral",
                11 => "G#: Infinite Gyroid",
                _ => "Unknown",
            }.to_string();

            entries.push(TimelineEntry {
                time: t,
                phase,
                gate,
                blend,
                harmonic_state,
                topology_name,
            });

            t += step;
        }

        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_quantize_hard() {
        // Hard snap (k=0)
        assert_eq!(GateSystem::gate_quantize(0.04, 12.0, 0.0), 0.0);
        assert_eq!(GateSystem::gate_quantize(0.06, 12.0, 0.0), 1.0 / 12.0);
    }

    #[test]
    fn test_gate_quantize_soft() {
        // Soft snap (k=0.5) should be more gradual
        let soft = GateSystem::gate_quantize(0.04, 12.0, 0.5);
        assert!(soft > 0.0 && soft < 1.0 / 12.0);
    }

    #[test]
    fn test_12_gate_cycle() {
        let mut gs = GateSystem::new();
        gs.set_gate_count(GateCount::Twelve);

        for i in 0..12 {
            gs.set_gate(i);
            assert_eq!(gs.harmonic_state_index(), i);
        }
    }

    #[test]
    fn test_24_gate_mapping() {
        let mut gs = GateSystem::new();
        gs.set_gate_count(GateCount::TwentyFour);

        // Gates 0,1 -> harmonic 0; Gates 2,3 -> harmonic 1; etc.
        gs.set_gate(0);
        assert_eq!(gs.harmonic_state_index(), 0);
        gs.set_gate(1);
        assert_eq!(gs.harmonic_state_index(), 0);
        gs.set_gate(2);
        assert_eq!(gs.harmonic_state_index(), 1);
    }
}
