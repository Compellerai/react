//! Harmonic States Lookup Table
//!
//! The 12 harmonic states based on REG's unified mathematics of geometry and music.
//! Each state corresponds to a musical note (A through G#) with specific:
//! - **TopologyIntent** - what the state wants to be (Stable, Transitional, Emergent, SymmetryLock)
//! - Shape mixing weights (torus/hyperboloid/gyroid)
//! - Geometric parameters (torusR, torusr, hyperAB, gyroidFreq/Iso)
//! - Warp amounts (twist, bend, ripple, quadWarp)
//! - Polygon symmetry for 2D overlay
//!
//! ## Architecture (v2)
//!
//! States now declare **intent**, not just parameter bundles. The renderer
//! derives behavior from intent, making the visualization feel *inevitable*.

// ============================================================================
// TOPOLOGY INTENT - What the state wants to be
// ============================================================================

/// Topology intent - what a harmonic state wants to become
///
/// This is the key architectural change: states declare intent,
/// and the renderer derives behavior from intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopologyIntent {
    /// Stable - torus-dominant, grounded, low warp
    /// Response rate: slow (0.05)
    Stable,

    /// Transitional - morphing between shapes, blending
    /// Response rate: moderate (0.15)
    Transitional,

    /// Emergent - complexity rising, gyroid emerging
    /// Response rate: fast (0.35)
    Emergent,

    /// SymmetryLock - hard structure, polyhedral forms, 12/24 gate snap
    /// Response rate: instant (1.0)
    SymmetryLock,
}

impl TopologyIntent {
    /// Get the response rate for hysteresis calculations
    ///
    /// This determines how fast morph parameters change:
    /// - Stable: slow, grounded feel
    /// - Transitional: moderate flow
    /// - Emergent: fast, rising
    /// - SymmetryLock: instant snap
    pub fn response_rate(&self) -> f32 {
        match self {
            TopologyIntent::Stable => 0.05,
            TopologyIntent::Transitional => 0.15,
            TopologyIntent::Emergent => 0.35,
            TopologyIntent::SymmetryLock => 1.0,
        }
    }

    /// Get the warp budget (max deformation allowed)
    pub fn warp_budget(&self) -> f32 {
        match self {
            TopologyIntent::Stable => 0.3,
            TopologyIntent::Transitional => 0.6,
            TopologyIntent::Emergent => 0.9,
            TopologyIntent::SymmetryLock => 0.4,  // Structured, not chaotic
        }
    }
}

// ============================================================================
// SHAPE TYPES
// ============================================================================

/// Shape mixing weights (should sum to ~1.0)
#[derive(Clone, Copy, Debug)]
pub struct ShapeMix {
    pub torus: f32,
    pub hyper: f32,
    pub gyroid: f32,
}

impl ShapeMix {
    pub const fn new(torus: f32, hyper: f32, gyroid: f32) -> Self {
        Self { torus, hyper, gyroid }
    }

    /// Interpolate between two shape mixes
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            torus: self.torus + (other.torus - self.torus) * t,
            hyper: self.hyper + (other.hyper - self.hyper) * t,
            gyroid: self.gyroid + (other.gyroid - self.gyroid) * t,
        }
    }
}

/// Geometric shape parameters
#[derive(Clone, Copy, Debug)]
pub struct ShapeParams {
    /// Torus major radius
    pub torus_r: f32,
    /// Torus minor (tube) radius
    pub torus_r2: f32,
    /// Hyperboloid a parameter
    pub hyper_a: f32,
    /// Hyperboloid b parameter
    pub hyper_b: f32,
    /// Gyroid frequency
    pub gyroid_freq: f32,
    /// Gyroid iso-surface level
    pub gyroid_iso: f32,
}

impl ShapeParams {
    pub const fn new(torus_r: f32, torus_r2: f32, hyper_a: f32, hyper_b: f32, gyroid_freq: f32, gyroid_iso: f32) -> Self {
        Self { torus_r, torus_r2, hyper_a, hyper_b, gyroid_freq, gyroid_iso }
    }

    /// Interpolate between two param sets
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            torus_r: self.torus_r + (other.torus_r - self.torus_r) * t,
            torus_r2: self.torus_r2 + (other.torus_r2 - self.torus_r2) * t,
            hyper_a: self.hyper_a + (other.hyper_a - self.hyper_a) * t,
            hyper_b: self.hyper_b + (other.hyper_b - self.hyper_b) * t,
            gyroid_freq: self.gyroid_freq + (other.gyroid_freq - self.gyroid_freq) * t,
            gyroid_iso: self.gyroid_iso + (other.gyroid_iso - self.gyroid_iso) * t,
        }
    }
}

/// Warp deformation parameters
#[derive(Clone, Copy, Debug)]
pub struct WarpParams {
    /// Y-axis twist amount (driven by mid)
    pub twist: f32,
    /// X-axis bend amount (driven by bass)
    pub bend: f32,
    /// Radial ripple amount (driven by high)
    pub ripple: f32,
    /// Quadripolar field warp (signature REG effect)
    pub quad_warp: f32,
}

impl WarpParams {
    pub const fn new(twist: f32, bend: f32, ripple: f32, quad_warp: f32) -> Self {
        Self { twist, bend, ripple, quad_warp }
    }

    /// Interpolate between two warp param sets
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            twist: self.twist + (other.twist - self.twist) * t,
            bend: self.bend + (other.bend - self.bend) * t,
            ripple: self.ripple + (other.ripple - self.ripple) * t,
            quad_warp: self.quad_warp + (other.quad_warp - self.quad_warp) * t,
        }
    }
}

/// Optional polyhedra type for 2D overlay
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Polyhedra {
    Tetrahedron,    // 4 faces
    Cube,           // 6 faces
    Octahedron,     // 8 faces
    Dodecahedron,   // 12 faces
    Icosahedron,    // 20 faces
}

impl Polyhedra {
    /// Number of vertices for polygon overlay
    pub fn vertices(&self) -> u8 {
        match self {
            Polyhedra::Tetrahedron => 3,
            Polyhedra::Cube => 4,
            Polyhedra::Octahedron => 6,
            Polyhedra::Dodecahedron => 5,
            Polyhedra::Icosahedron => 3,
        }
    }
}

/// A single harmonic state (one of 12 notes)
#[derive(Clone, Copy, Debug)]
pub struct HarmonicState {
    /// Index (0-11)
    pub index: u8,
    /// Musical note name
    pub note: &'static str,
    /// Topology intent - what this state wants to become
    pub intent: TopologyIntent,
    /// Polygon symmetry (3, 4, 5, 6, ... 24, or 0 for continuous)
    pub symmetry: u8,
    /// Shape mixing weights
    pub shape_mix: ShapeMix,
    /// Optional polyhedra for overlay
    pub poly: Option<Polyhedra>,
    /// Geometric parameters
    pub params: ShapeParams,
    /// Warp deformation amounts
    pub warps: WarpParams,
}

impl HarmonicState {
    /// Interpolate between this state and another
    pub fn lerp(&self, other: &Self, t: f32) -> InterpolatedState {
        InterpolatedState {
            shape_mix: self.shape_mix.lerp(&other.shape_mix, t),
            params: self.params.lerp(&other.params, t),
            warps: self.warps.lerp(&other.warps, t),
            symmetry: if t < 0.5 { self.symmetry } else { other.symmetry },
        }
    }
}

/// Interpolated state between two harmonic states
#[derive(Clone, Copy, Debug)]
pub struct InterpolatedState {
    pub shape_mix: ShapeMix,
    pub params: ShapeParams,
    pub warps: WarpParams,
    pub symmetry: u8,
}

/// The 12 harmonic states based on REG's unified mathematics
/// Progression: Pure torus → torus/hyper blend → hyper dominant → gyroid emerging → pure gyroid
pub const HARMONIC_STATES: [HarmonicState; 12] = [
    // i=0: A - Pure torus, minimal warps, triangle symmetry (STABLE)
    HarmonicState {
        index: 0,
        note: "A",
        intent: TopologyIntent::Stable,
        symmetry: 3,
        shape_mix: ShapeMix::new(0.85, 0.15, 0.00),
        poly: None,
        params: ShapeParams::new(0.95, 0.32, 0.85, 1.15, 2.0, 0.05),
        warps: WarpParams::new(0.20, 0.40, 0.10, 0.25),
    },
    // i=1: A# - Octahedron skin, square symmetry (STABLE)
    HarmonicState {
        index: 1,
        note: "A#",
        intent: TopologyIntent::Stable,
        symmetry: 4,
        shape_mix: ShapeMix::new(0.70, 0.28, 0.02),
        poly: Some(Polyhedra::Octahedron),
        params: ShapeParams::new(0.90, 0.35, 0.80, 1.20, 2.2, 0.08),
        warps: WarpParams::new(0.25, 0.35, 0.15, 0.30),
    },
    // i=2: B - Dodecahedron reference, pentagon symmetry, more hyper (TRANSITIONAL)
    HarmonicState {
        index: 2,
        note: "B",
        intent: TopologyIntent::Transitional,
        symmetry: 5,
        shape_mix: ShapeMix::new(0.55, 0.40, 0.05),
        poly: Some(Polyhedra::Dodecahedron),
        params: ShapeParams::new(0.85, 0.38, 0.75, 1.25, 2.5, 0.10),
        warps: WarpParams::new(0.30, 0.30, 0.20, 0.35),
    },
    // i=3: C - Cube reference, hexagon symmetry, balanced torus/hyper (TRANSITIONAL)
    HarmonicState {
        index: 3,
        note: "C",
        intent: TopologyIntent::Transitional,
        symmetry: 6,
        shape_mix: ShapeMix::new(0.45, 0.45, 0.10),
        poly: Some(Polyhedra::Cube),
        params: ShapeParams::new(0.80, 0.40, 0.70, 1.30, 2.8, 0.12),
        warps: WarpParams::new(0.35, 0.25, 0.25, 0.40),
    },
    // i=4: C# - Heptagon symmetry, hyper-dominant (TRANSITIONAL)
    HarmonicState {
        index: 4,
        note: "C#",
        intent: TopologyIntent::Transitional,
        symmetry: 7,
        shape_mix: ShapeMix::new(0.30, 0.55, 0.15),
        poly: None,
        params: ShapeParams::new(0.75, 0.42, 0.65, 1.35, 3.0, 0.15),
        warps: WarpParams::new(0.40, 0.22, 0.30, 0.45),
    },
    // i=5: D - Icosahedron reference, octagon symmetry, gyroid emerging (EMERGENT)
    HarmonicState {
        index: 5,
        note: "D",
        intent: TopologyIntent::Emergent,
        symmetry: 8,
        shape_mix: ShapeMix::new(0.20, 0.50, 0.30),
        poly: Some(Polyhedra::Icosahedron),
        params: ShapeParams::new(0.70, 0.44, 0.60, 1.40, 3.5, 0.18),
        warps: WarpParams::new(0.45, 0.20, 0.35, 0.50),
    },
    // i=6: D# - Nonagon symmetry, gyroid/hyper mix (EMERGENT)
    HarmonicState {
        index: 6,
        note: "D#",
        intent: TopologyIntent::Emergent,
        symmetry: 9,
        shape_mix: ShapeMix::new(0.15, 0.40, 0.45),
        poly: None,
        params: ShapeParams::new(0.65, 0.45, 0.55, 1.45, 4.0, 0.20),
        warps: WarpParams::new(0.50, 0.18, 0.40, 0.55),
    },
    // i=7: E - Dodecahedron, decagon symmetry, gyroid-dominant (EMERGENT)
    HarmonicState {
        index: 7,
        note: "E",
        intent: TopologyIntent::Emergent,
        symmetry: 10,
        shape_mix: ShapeMix::new(0.10, 0.30, 0.60),
        poly: Some(Polyhedra::Dodecahedron),
        params: ShapeParams::new(0.60, 0.46, 0.50, 1.50, 4.5, 0.22),
        warps: WarpParams::new(0.55, 0.15, 0.45, 0.60),
    },
    // i=8: F - Icosahedron, hendecagon symmetry, heavy gyroid (SYMMETRYLOCK)
    HarmonicState {
        index: 8,
        note: "F",
        intent: TopologyIntent::SymmetryLock,
        symmetry: 11,
        shape_mix: ShapeMix::new(0.05, 0.20, 0.75),
        poly: Some(Polyhedra::Icosahedron),
        params: ShapeParams::new(0.55, 0.48, 0.45, 1.55, 5.0, 0.25),
        warps: WarpParams::new(0.60, 0.12, 0.50, 0.65),
    },
    // i=9: F# - Dodecagon symmetry (12 = Flower of Life), near-pure gyroid (SYMMETRYLOCK)
    // This state references 186.32 Hz / 186,282 miles/sec (speed of light)
    HarmonicState {
        index: 9,
        note: "F#",
        intent: TopologyIntent::SymmetryLock,
        symmetry: 12,
        shape_mix: ShapeMix::new(0.02, 0.10, 0.88),
        poly: None,
        params: ShapeParams::new(0.50, 0.50, 0.40, 1.60, 5.5, 0.28),
        warps: WarpParams::new(0.65, 0.10, 0.55, 0.70),
    },
    // i=10: G - Icositetragon (24-wheel), gyroid cathedral (SYMMETRYLOCK)
    HarmonicState {
        index: 10,
        note: "G",
        intent: TopologyIntent::SymmetryLock,
        symmetry: 24,
        shape_mix: ShapeMix::new(0.00, 0.05, 0.95),
        poly: None,
        params: ShapeParams::new(0.45, 0.52, 0.35, 1.65, 6.0, 0.30),
        warps: WarpParams::new(0.70, 0.08, 0.60, 0.75),
    },
    // i=11: G# - Continuous symmetry (0), return to origin (STABLE)
    HarmonicState {
        index: 11,
        note: "G#",
        intent: TopologyIntent::Stable,
        symmetry: 0, // 0 = continuous/infinite symmetry
        shape_mix: ShapeMix::new(0.00, 0.00, 1.00),
        poly: None,
        params: ShapeParams::new(0.40, 0.55, 0.30, 1.70, 6.5, 0.32),
        warps: WarpParams::new(0.75, 0.05, 0.65, 0.80),
    },
];

/// Get the next harmonic state (wraps around)
pub fn next_state(index: u8) -> &'static HarmonicState {
    &HARMONIC_STATES[((index + 1) % 12) as usize]
}

/// Get the complement state (REG's mirror across 6)
pub fn complement_state(index: u8) -> &'static HarmonicState {
    &HARMONIC_STATES[((12 - index) % 12) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_count() {
        assert_eq!(HARMONIC_STATES.len(), 12);
    }

    #[test]
    fn test_shape_mix_sums() {
        for state in HARMONIC_STATES.iter() {
            let sum = state.shape_mix.torus + state.shape_mix.hyper + state.shape_mix.gyroid;
            assert!((sum - 1.0).abs() < 0.01, "State {} shape mix sums to {}", state.note, sum);
        }
    }

    #[test]
    fn test_interpolation() {
        let state0 = &HARMONIC_STATES[0];
        let state1 = &HARMONIC_STATES[1];
        let interp = state0.lerp(state1, 0.5);

        // Midpoint should be between the two
        assert!(interp.shape_mix.torus > state1.shape_mix.torus);
        assert!(interp.shape_mix.torus < state0.shape_mix.torus);
    }
}
