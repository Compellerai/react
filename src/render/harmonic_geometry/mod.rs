//! REG-Aligned Harmonic Geometry Renderer
//!
//! Geometry emerges from harmonic number fields, not named solids.
//! The 12-bin chroma vector directly controls the radial profile.
//!
//! ## Philosophy
//!
//! - **Number → Symmetry → Geometry** (not "3D shapes reacting to audio")
//! - The viewer should be able to SEE harmony
//! - Symmetry emerges from resonance
//! - Disorder appears only when the music is noisy
//!
//! ## Core Primitive: Harmonic Field
//!
//! Each pitch class (C through B) controls one spoke of a 12-point radial profile:
//! ```
//! radius[i] = base_radius + chroma[i] * gain
//! ```
//!
//! The 2D profile is smoothly interpolated (Catmull-Rom spline) between spokes,
//! then realized in 3D via extrusion or lathe/revolution.
//!
//! ## 3D Realization
//!
//! - **Extrusion**: 2D profile extruded along Z (readable "harmonic column")
//! - **Lathe**: Profile revolved around Y (smooth "audio sculpture")
//! - **Both**: Combined for full expression
//!
//! ## Symmetry Layers
//!
//! - **12-Fold**: Base harmonic field from chroma
//! - **24-Fold**: Octave reinforcement via phase-shifted duplicate
//!
//! ## Motion
//!
//! - Slow continuous rotation (geological, NOT beat-synced)
//! - Audio controls deformation and thickness, NOT timing
//! - When tonal confidence drops, shape relaxes toward neutral circle

mod harmonic_field;
mod key_detector;
mod presets;
mod quaternion_charges;
mod renderer;
mod shaders;
mod skin;

// Legacy modules preserved for reference (not used by new system)
// These will be removed in a future cleanup pass
#[allow(dead_code)]
mod audio_mapper;
#[allow(dead_code)]
mod gate_system;
#[allow(dead_code)]
mod harmonic_states;

// Public exports - Hero's Journey system (no presets, single instrument)
pub use harmonic_field::{HarmonicField, RenderMode, SymmetryMode};
pub use key_detector::{HarmonicLabel, KeyDetector, TuningReference};
pub use presets::{HeroConfig, VisualConfig};
pub use quaternion_charges::{Charge, Invariants, QuaternionChargesState};
pub use renderer::{HarmonicFieldState, HarmonicGeometryRenderer, ShaderInitStep};
pub use skin::{FieldSkin, SkinMode};

// ============================================================================
// HARMONIC RESONATOR VISUAL SYSTEM
// Temporal Acts Defined by Mathematical Invariants and Trigger Conditions
// ============================================================================
//
// THEORETICAL BASIS (Wave Theory of Numbers, Grant & Ghannam)
//
// 1. Structure emerges from interference, not imposition.
// 2. The 12-based fractal is fundamental (multiples of 12 in wave matrix).
// 3. Interior vs exterior is mathematically meaningful (angles vs arcs).
// 4. Binary doubling creates hierarchical emergence (octave relationships).
//
// ============================================================================
// ACT I — BIRTH (Field Emergence)
// ============================================================================
//
// MATHEMATICAL INVARIANT:
//   - Geometry exists only as continuous wave field with no stable angular
//     partitions.
//   - Radial displacement governed by mean-deviation ≤ ε, preventing
//     polygonal fixation.
//   - No dominant pitch satisfies coherence criteria across chroma vector.
//
// TRIGGER CONDITIONS (Birth → Lock):
//   - Harmonic coherence rises above minimum threshold for continuous window
//   - At least one chroma bin exceeds mean energy by defined ratio
//   - Minimum temporal duration elapsed (BIRTH_MIN_TIME)
//
// RESULTING STATE:
//   - Initial lobes form through constructive interference
//   - Geometry transitions from isotropic noise toward anisotropic structure
//
// ============================================================================
// ACT II — LOCK (Standing Wave Formation)
// ============================================================================
//
// MATHEMATICAL INVARIANT:
//   - Standing wave forms with stable angular symmetry
//   - Dominant pitch latched via hysteresis, preserving angular continuity
//   - Polygonal identity persists independent of transient amplitude variation
//
// TRIGGER CONDITIONS (Lock → Dive):
//   - Transient energy impulse (kick) exceeds threshold
//   - Section energy within sweet spot range [MIN, MAX]
//   - Stability above minimum confidence threshold
//
// RESULTING STATE:
//   - Geometry resolves into readable harmonic sculpture
//   - Color and symmetry stabilize without retargeting artifacts
//
// ============================================================================
// ACT III — DIVE (Interior Structural Revelation)
// ============================================================================
//
// MATHEMATICAL INVARIANT:
//   - Exterior geometry unchanged; no alteration of standing wave topology
//   - Interior space reveals nested interference structures isomorphic to
//     exterior wave field
//   - Internal lattice obeys identical angular/harmonic constraints as outer
//
// TRIGGER CONDITIONS (Dive → Afterglow):
//   - Overall energy falls below threshold
//   - Minimum temporal duration in interior elapsed (DIVE_MIN_TIME)
//
// RESULTING STATE:
//   - Viewpoint transitions across implicit surface boundary
//   - Internal wave interference patterns observable without breaking coherence
//
// ============================================================================
// ACT IV — AFTERGLOW (Residual Coherence Persistence)
// ============================================================================
//
// MATHEMATICAL INVARIANT:
//   - Standing wave remains intact despite decreasing energy input
//   - Harmonic identity persists via residual interference
//   - Motion continues through low-frequency rotational/orbital modulation
//
// TRIGGER CONDITIONS (Afterglow → Birth or Lock):
//   - Very low energy → Birth (full reset)
//   - Energy rises + stability → Lock (ready for another dive)
//
// RESULTING STATE:
//   - Geometry slowly relaxes without collapsing into noise
//   - System returns to exterior observational state with preserved structure
//
// ============================================================================
// SYSTEM-LEVEL CLAIM
// ============================================================================
//
// The above acts are NOT narrative states, but emergent temporal phases
// defined by invariant mathematical conditions governing wave interference,
// harmonic coherence, and angular symmetry.
//
// The perceived "journey" arises from time-domain evolution of the same
// underlying mathematical field, without discrete scene scripting or
// geometry substitution.
//
// ============================================================================

/// Temporal phase defined by mathematical invariants (not narrative state)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeroAct {
    /// Field Emergence: continuous wave field, no stable angular partitions
    Birth,
    /// Standing Wave Formation: stable angular symmetry, dominant pitch latched
    Lock,
    /// Interior Structural Revelation: viewpoint crosses implicit surface
    Dive,
    /// Residual Coherence Persistence: standing wave intact, energy decreasing
    Afterglow,
}

impl Default for HeroAct {
    fn default() -> Self {
        HeroAct::Birth
    }
}

impl HeroAct {
    /// Numeric value for shader uniform
    pub fn as_f32(&self) -> f32 {
        match self {
            HeroAct::Birth => 0.0,
            HeroAct::Lock => 1.0,
            HeroAct::Dive => 2.0,
            HeroAct::Afterglow => 3.0,
        }
    }

    /// Human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            HeroAct::Birth => "Birth",
            HeroAct::Lock => "Lock",
            HeroAct::Dive => "Dive",
            HeroAct::Afterglow => "Afterglow",
        }
    }
}

// JourneyPhase is now an alias for HeroAct
pub type JourneyPhase = HeroAct;

/// Interior traversal state - binary latch, not blend
/// This is the "permission" to be inside. Once granted, rules change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteriorState {
    /// Outside the geometry - surface rendering dominant
    Outside,
    /// Crossing the threshold - brief transition
    Entering,
    /// Fully inside - volume rendering owns the frame, surface disabled
    Inside,
    /// Pulling back out - brief transition
    Exiting,
}

impl Default for InteriorState {
    fn default() -> Self {
        InteriorState::Outside
    }
}

impl InteriorState {
    /// Is camera inside the geometry?
    pub fn is_inside(&self) -> bool {
        matches!(self, InteriorState::Inside | InteriorState::Entering)
    }

    /// Human-readable name for UI
    pub fn name(&self) -> &'static str {
        match self {
            InteriorState::Outside => "Outside",
            InteriorState::Entering => "Entering",
            InteriorState::Inside => "Inside",
            InteriorState::Exiting => "Exiting",
        }
    }
}

// ============================================================================
// PRESENTATION MODE - Museum/Idle vs Live/Performance
// ============================================================================
// Smooth crossfade between calm "screensaver" rendering and active performance.
// Activity detection with hysteresis prevents mode flapping.
// ============================================================================

/// Presentation mode for museum/installation vs live performance
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PresentationMode {
    /// Museum/screensaver mode - calm, deliberate, hero scaffold
    #[default]
    Idle,
    /// Live performance mode - responsive, energetic
    Live,
}

impl PresentationMode {
    /// Numeric value for shader uniform
    pub fn as_f32(&self) -> f32 {
        match self {
            PresentationMode::Idle => 0.0,
            PresentationMode::Live => 1.0,
        }
    }

    /// Human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            PresentationMode::Idle => "Idle",
            PresentationMode::Live => "Live",
        }
    }
}

// ============================================================================
// VORTEX TOPOLOGY SYSTEM - 24-Fold Harmonic Topology Routing
// ============================================================================
// Explicit topology selection for the 24-fold harmonic geometry system.
// Routes between discrete topology states based on bar-edge evaluated metrics.
// ============================================================================

/// Topology identifier for the 24-fold harmonic geometry system
/// Each topology represents a distinct mathematical structure
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TopologyId {
    /// Base 24-fold harmonic scaffold (default state)
    #[default]
    Scaffold24 = 0,
    /// Torus knot cymatics - standing wave on torus surface
    TorusKnotCymatics = 1,
    /// Vortex topology - spiral flow convergent/divergent
    Vortex = 2,
    /// Radial interference shell - concentric standing waves
    RadialInterferenceShell = 3,
    /// Braided standing wave - intertwined wave patterns
    BraidedStandingWave = 4,
    /// Afterglow residual - fading coherence pattern
    AfterglowResidual = 5,
    /// Dodecahedral resonance - crystalline harmonic arrival (12 pentagonal faces)
    DodecahedralResonance = 6,
}

impl TopologyId {
    /// Convert to i32 for shader uniform
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    /// Human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            TopologyId::Scaffold24 => "Scaffold24",
            TopologyId::TorusKnotCymatics => "TorusKnotCymatics",
            TopologyId::Vortex => "Vortex",
            TopologyId::RadialInterferenceShell => "RadialInterferenceShell",
            TopologyId::BraidedStandingWave => "BraidedStandingWave",
            TopologyId::AfterglowResidual => "AfterglowResidual",
            TopologyId::DodecahedralResonance => "DodecahedralResonance",
        }
    }
}

/// Topology router phase - transition state between topologies
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TopologyPhase {
    /// No transition active, holding current topology
    #[default]
    Idle,
    /// Crossfading between two topologies
    Crossfade,
}

impl TopologyPhase {
    pub fn as_i32(&self) -> i32 {
        match self {
            TopologyPhase::Idle => 0,
            TopologyPhase::Crossfade => 1,
        }
    }
}

/// Vortex flow direction - inward (convergent) or outward (divergent)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VortexDirection {
    /// Flow converging toward center (contractive)
    #[default]
    Inward = 0,
    /// Flow diverging from center (expansive)
    Outward = 1,
}

impl VortexDirection {
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    /// Flip direction
    pub fn flip(&self) -> Self {
        match self {
            VortexDirection::Inward => VortexDirection::Outward,
            VortexDirection::Outward => VortexDirection::Inward,
        }
    }
}

// ============================================================================
// REG-LOCKED CAMERA SYSTEM - Quantized Camera Motion
// ============================================================================
// Camera rides the same angular grid as the geometry:
// - Only targets discrete, symmetry-valid poses (azimuth anchors, elevation tiers)
// - Audio events select targets; camera eases toward them
// - "Going inside" is a pre-authored PORTAL_DIVE that locks to a single wedge
// ============================================================================

/// REG-locked camera rig types
/// Each rig has different motion constraints that preserve symmetry readability
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CameraRig {
    /// Default: orbits on constant radius tier, azimuth snaps to N-fold anchors
    /// Great for 12-fold and as default 24-fold viewing
    #[default]
    Orbit = 0,
    /// Dolly in/out on principal axis, minimal rotation
    /// Preserves symmetry scaffold perfectly, good for stability recovery
    AxialPush = 1,
    /// Pre-authored dive through locked wedge (24-fold only)
    /// Picks one wedge (anchored to QC port) and dives straight through it
    PortalDive = 2,
}

impl CameraRig {
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    pub fn name(&self) -> &'static str {
        match self {
            CameraRig::Orbit => "Orbit",
            CameraRig::AxialPush => "AxialPush",
            CameraRig::PortalDive => "PortalDive",
        }
    }
}

/// Camera transition phase
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CameraRegPhase {
    /// No transition active, holding current discrete state
    #[default]
    Idle,
    /// Transitioning between discrete states (smoothing in progress)
    Transitioning,
}

impl CameraRegPhase {
    pub fn as_i32(&self) -> i32 {
        match self {
            CameraRegPhase::Idle => 0,
            CameraRegPhase::Transitioning => 1,
        }
    }
}

// ============================================================================
// PORTAL FPS - Rail Shooter Mode Inside Portal
// ============================================================================
// Transforms portal dive into first-person traversal while staying REG-pure:
// - Wedge-locked forward direction (no free yaw)
// - Discrete lane switching (±1 step per onset/downbeat)
// - Instanced video rails bound to 24-fold seams
// - Bounded near-miss bar events
// - VR-safe (no roll, no continuous drift)
// ============================================================================

/// Portal traversal path presets - determines corridor visual style
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PortalPath {
    /// Simple corridor with wedge seams as walls
    #[default]
    Corridor = 0,
    /// Grid tunnel with perpendicular struts - best default, insane parallax
    TunnelGrid = 1,
    /// Ribbed lanes with curved supports
    RibLanes = 2,
    /// Torus knot pass-through for high harmonic confidence
    KnotPass = 3,
}

impl PortalPath {
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    pub fn name(&self) -> &'static str {
        match self {
            PortalPath::Corridor => "Corridor",
            PortalPath::TunnelGrid => "TunnelGrid",
            PortalPath::RibLanes => "RibLanes",
            PortalPath::KnotPass => "KnotPass",
        }
    }
}

/// Near-miss bar event for FPS "dodge" visuals
#[derive(Clone, Copy, Debug, Default)]
pub struct NearMissBar {
    /// Is this slot active?
    pub active: bool,
    /// Wedge index the bar is attached to (0..23)
    pub wedge_k: u8,
    /// Remaining lifetime (seconds)
    pub lifetime: f32,
    /// Radial position tier (0=inner, 1=mid, 2=outer)
    pub radius_tier: u8,
}

/// Maximum concurrent near-miss bars (bounded for performance)
const PORTAL_FPS_MAX_NEAR_MISS: usize = 6;

/// Forward motion rate (segments per second at speed_tier=1)
const PORTAL_FPS_FORWARD_RATE: f32 = 2.5;  // Must be high enough for 3-4 segment advances per 1.2s dive

/// Minimum time in lane before switching allowed
const PORTAL_FPS_LANE_HOLD_MIN: f32 = 0.3;

/// Cooldown after lane switch
const PORTAL_FPS_LANE_COOLDOWN: f32 = 0.25;

/// Lane transition smoothing rate (per second)
const PORTAL_FPS_LANE_TRANSITION_RATE: f32 = 8.0;

/// Lane impulse decay rate (achieves ~150ms decay)
const PORTAL_FPS_IMPULSE_DECAY: f32 = 12.0;

/// Lane impulse magnitude per speed tier [slow, normal, fast]
const PORTAL_FPS_IMPULSE_MAG: [f32; 3] = [0.3, 0.5, 0.8];

/// Vista scaffold emphasis duration (seconds)
const PORTAL_FPS_VISTA_DURATION: f32 = 0.25;

/// Vista scaffold emphasis peak intensity
const PORTAL_FPS_VISTA_PEAK: f32 = 1.0;

/// Hit emphasis decay rate
const PORTAL_FPS_HIT_DECAY: f32 = 8.0;

/// Rail group pulse decay rate
const PORTAL_FPS_RAIL_PULSE_DECAY: f32 = 6.0;

/// Segment type for narrative pacing
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentType {
    Runway,     // Entry/exit - calm, establish direction
    Accelerate, // Speed ramp - build momentum
    Threat,     // Near-miss zone - danger, focus
    Vista,      // Chamber widen - glimpse the bigger structure
    Needle,     // Thread the needle - tight precision
    Decompress, // Cool down - release tension
}

/// Portal segment configuration - discrete parameters per segment
#[derive(Clone, Copy, Debug)]
pub struct PortalSegment {
    /// Segment narrative type
    pub seg_type: SegmentType,
    /// Duration of this segment in seconds
    pub duration: f32,
    /// Speed tier: 0=slow, 1=normal, 2=fast
    pub speed_tier: u8,
    /// Rail density tier: 0=sparse(24), 1=normal(48), 2=dense(72)
    pub density_tier: u8,
    /// Near-miss budget: max bars that can spawn this segment
    pub near_miss_budget: u8,
    /// Corridor radius tier: 0=tight, 1=normal, 2=wide (vista)
    pub corridor_tier: u8,
}

/// "Make it hit" segment table - runway/accelerate/threat1/vista/threat2/needle/decompress/exit
/// Total duration: ~2.8s (tuned for one phrase of music at 120-130 BPM)
const PORTAL_SEGMENTS: [PortalSegment; 8] = [
    // Seg 0: Entry runway - establish direction, calm start
    PortalSegment { seg_type: SegmentType::Runway, duration: 0.30, speed_tier: 1, density_tier: 1, near_miss_budget: 0, corridor_tier: 1 },
    // Seg 1: Accelerate - build momentum, rails intensify
    PortalSegment { seg_type: SegmentType::Accelerate, duration: 0.35, speed_tier: 2, density_tier: 2, near_miss_budget: 0, corridor_tier: 1 },
    // Seg 2: Threat 1 - first danger, single near-miss
    PortalSegment { seg_type: SegmentType::Threat, duration: 0.35, speed_tier: 2, density_tier: 2, near_miss_budget: 1, corridor_tier: 0 },
    // Seg 3: Vista - corridor widens, glimpse the dodeca shell
    PortalSegment { seg_type: SegmentType::Vista, duration: 0.40, speed_tier: 1, density_tier: 1, near_miss_budget: 0, corridor_tier: 2 },
    // Seg 4: Threat 2 - double danger, symmetric pair
    PortalSegment { seg_type: SegmentType::Threat, duration: 0.35, speed_tier: 2, density_tier: 2, near_miss_budget: 2, corridor_tier: 0 },
    // Seg 5: Thread the needle - precision, single bar
    PortalSegment { seg_type: SegmentType::Needle, duration: 0.30, speed_tier: 2, density_tier: 2, near_miss_budget: 1, corridor_tier: 0 },
    // Seg 6: Decompress - release tension, slow down
    PortalSegment { seg_type: SegmentType::Decompress, duration: 0.35, speed_tier: 1, density_tier: 1, near_miss_budget: 0, corridor_tier: 1 },
    // Seg 7: Exit runway - return to outer space
    PortalSegment { seg_type: SegmentType::Runway, duration: 0.30, speed_tier: 1, density_tier: 1, near_miss_budget: 0, corridor_tier: 1 },
];

/// Speed multipliers for each tier
const PORTAL_SPEED_MULT: [f32; 3] = [0.6, 1.0, 1.5];

/// Rail counts per density tier
const PORTAL_RAIL_COUNTS: [u8; 3] = [24, 48, 72];

/// Portal FPS state - rail shooter mode inside portal
#[derive(Clone, Debug)]
pub struct PortalFpsState {
    /// Is FPS mode active (true during PortalDive)
    pub active: bool,

    // === PATH SELECTION ===
    /// Current path preset (determines visual style)
    pub path_id: PortalPath,

    // === LANE STATE (discrete lateral position) ===
    /// Current lane index: -2, -1, 0, +1, +2
    pub lane_idx: i8,
    /// Target lane (for smooth transitions)
    pub lane_target_idx: i8,
    /// Time since last lane change
    pub lane_hold_timer: f32,
    /// Cooldown preventing rapid lane changes
    pub lane_cooldown_timer: f32,
    /// Lane transition progress [0..1]
    pub lane_transition_t: f32,

    // === FORWARD MOTION (segment-based) ===
    /// Current segment index (0..7)
    pub segment_id: u8,
    /// Progress within current segment [0..1]
    pub forward_phase: f32,
    /// Current speed tier from segment table
    pub speed_tier: u8,
    /// Current density tier from segment table
    pub density_tier: u8,

    // === MICRO SWAY (bounded positional offset) ===
    /// Sway tier: 0=none, 1=subtle, 2=noticeable
    pub sway_tier: u8,
    /// Sway phase [0..1] for time-based oscillation
    pub sway_phase: f32,

    // === VIDEO RAILS ===
    /// Seed for deterministic rail variation (set on entry)
    pub line_set_seed: u32,
    /// UV scroll phase for rail textures [0..1]
    pub line_scroll_phase: f32,

    // === NEAR-MISS EVENTS (bounded buffer) ===
    /// Fixed ring buffer of active near-miss bars
    pub near_miss_bars: [NearMissBar; PORTAL_FPS_MAX_NEAR_MISS],
    /// Count of currently active bars
    pub near_miss_active_count: u8,
    /// Near-miss spawns remaining this segment
    pub near_miss_budget_remaining: u8,

    // === LOCKED FRAME (computed on entry) ===
    /// Forward direction X (cos of locked azimuth)
    pub frame_forward_x: f32,
    /// Forward direction Z (sin of locked azimuth)
    pub frame_forward_z: f32,
    /// Right direction X (perpendicular to forward)
    pub frame_right_x: f32,
    /// Right direction Z (perpendicular to forward)
    pub frame_right_z: f32,

    // === TRACKING ===
    /// Previous dominant chroma index for shift detection
    pub prev_dominant_chroma: u8,
    /// Previous beat phase for downbeat detection
    pub prev_beat_phase: f32,

    // === PARALLAX & ACCENT SCALARS (presentation only) ===
    /// Lane switch impulse [0..1] - decays over 120-180ms for parallax snap
    pub lane_impulse: f32,
    /// Lane impulse direction: -1 (left) or +1 (right)
    pub lane_impulse_dir: i8,
    /// Vista pulse [0..1] - time-based, only during Vista segment
    pub vista_pulse: f32,
    /// Hit emphasis [0..1] - short decay on near-miss spawn
    pub hit_emphasis: f32,
    /// Current corridor tier (may differ from segment during transitions)
    pub corridor_tier: u8,
    /// Scaffold emphasis [0..1] - brightens during Vista to show shell
    pub scaffold_emphasis: f32,

    // === VIDEO RAIL GROUPING ===
    /// Rail group pulse values [3] - groups by k mod 3
    /// [0]=downbeat pulse, [1]=offbeat pulse, [2]=onset pulse
    pub rail_group_pulse: [f32; 3],

    // === ROBUST LANE INTENT (computed once per beat) ===
    /// Lane intent from chroma peak: -1 (left), 0 (hold), +1 (right)
    pub lane_intent: i8,
    /// Previous beat's intent (for hysteresis)
    pub lane_intent_prev: i8,
    /// Intent confirmed (same for 2 consecutive beats)
    pub lane_intent_confirmed: bool,
    /// Beat ID when intent was last updated
    pub lane_intent_beat_id: u64,

    // === THREAT DEBT FALLBACK ===
    /// Debt counter: missed spawns in threat segments
    pub threat_debt: u8,
    /// Track if any near-miss spawned in current segment
    pub nm_spawned_in_segment: bool,
    /// Beat count since last spawn attempt (for fallback timing)
    pub beats_since_spawn_attempt: u8,

    // === INSIDEVIEW CAMERA STATE ===
    /// Cumulative Z offset (forward distance from portal entry)
    pub z_offset: f32,
    /// Entry aperture effect [0..1], 1.0 on entry, decays over ~200ms
    pub entry_aperture: f32,
}

impl Default for PortalFpsState {
    fn default() -> Self {
        Self {
            active: false,
            path_id: PortalPath::TunnelGrid,
            lane_idx: 0,
            lane_target_idx: 0,
            lane_hold_timer: 0.0,
            lane_cooldown_timer: 0.0,
            lane_transition_t: 1.0,
            segment_id: 0,
            forward_phase: 0.0,
            speed_tier: 1,
            density_tier: 1,
            sway_tier: 1,
            sway_phase: 0.0,
            line_set_seed: 0,
            line_scroll_phase: 0.0,
            near_miss_bars: [NearMissBar::default(); PORTAL_FPS_MAX_NEAR_MISS],
            near_miss_active_count: 0,
            near_miss_budget_remaining: 0,
            frame_forward_x: 1.0,
            frame_forward_z: 0.0,
            frame_right_x: 0.0,
            frame_right_z: 1.0,
            prev_dominant_chroma: 0,
            prev_beat_phase: 0.0,
            // Parallax & accent scalars
            lane_impulse: 0.0,
            lane_impulse_dir: 0,
            vista_pulse: 0.0,
            hit_emphasis: 0.0,
            corridor_tier: 1,
            scaffold_emphasis: 0.0,
            rail_group_pulse: [0.0; 3],
            // Robust lane intent
            lane_intent: 0,
            lane_intent_prev: 0,
            lane_intent_confirmed: false,
            lane_intent_beat_id: 0,
            // Threat debt fallback
            threat_debt: 0,
            nm_spawned_in_segment: false,
            beats_since_spawn_attempt: 0,
            // InsideView camera state
            z_offset: 0.0,
            entry_aperture: 0.0,
        }
    }
}

impl PortalFpsState {
    /// Debug string for HUD display
    pub fn debug_string(&self) -> String {
        if self.active {
            let seg_type = PORTAL_SEGMENTS[self.segment_id as usize].seg_type;
            let seg_char = match seg_type {
                SegmentType::Runway => 'R',
                SegmentType::Accelerate => 'A',
                SegmentType::Threat => 'T',
                SegmentType::Vista => 'V',
                SegmentType::Needle => 'N',
                SegmentType::Decompress => 'D',
            };
            format!(
                "FPS: {} L{:+} {}{}/8 c{} nm{} imp{:.0}",
                self.path_id.name(),
                self.lane_idx,
                seg_char,
                self.segment_id + 1,
                self.corridor_tier,
                self.near_miss_active_count,
                self.lane_impulse * 100.0
            )
        } else {
            "FPS: inactive".to_string()
        }
    }
}

// ============================================================================
// RING DIVE - Cinematic Inside Transition Sequence
// ============================================================================
// Single continuous curve ringdive_t in [0..1] with subranges:
// - lock_on:       0.00 - 0.18  (rings gain depth, target becomes portal)
// - warp_out:      0.18 - 0.35  (accelerate through, streaks aligned to dom_pc)
// - guided_flight: 0.35 - 0.80  (field lines visible, camera inside)
// - warp_back:     0.80 - 1.00  (decelerate, rings re-form)
// ============================================================================

/// GPU quality tier for RingDive effects
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RingDiveQuality {
    Low,   // warp<=6, lines<=4, no portal interior
    #[default]
    Med,   // warp<=10, lines<=8, cheap interior
    High,  // warp<=14, lines<=12, full interior
}

impl RingDiveQuality {
    pub fn warp_samples(&self) -> i32 {
        match self {
            RingDiveQuality::Low => 6,
            RingDiveQuality::Med => 10,
            RingDiveQuality::High => 14,
        }
    }

    pub fn field_lines(&self) -> i32 {
        match self {
            RingDiveQuality::Low => 4,
            RingDiveQuality::Med => 8,
            RingDiveQuality::High => 12,
        }
    }

    pub fn as_int(&self) -> i32 {
        match self {
            RingDiveQuality::Low => 0,
            RingDiveQuality::Med => 1,
            RingDiveQuality::High => 2,
        }
    }
}

// ============================================================================
// INSIDE 24-FOLD QUALITY SYSTEM
// ============================================================================
// Dynamic quality tier for expensive inside rendering.
// Degrades gracefully when GPU can't keep up:
//   High: Full fog, sparkle, labels, SDF detail
//   Med:  No fog, reduced sparkle, minimal labels
//   Low:  No fog/sparkle, no labels, reduced shells
// ============================================================================

/// Inside 24-fold rendering quality tier
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InsideQuality {
    /// Minimal: no fog/sparkle, no labels, 2 shells
    Low,
    /// Balanced: no fog, sparse sparkle, ticks only
    #[default]
    Med,
    /// Full: fog, sparkle, labels (only when stable)
    High,
}

impl InsideQuality {
    pub fn as_i32(&self) -> i32 {
        match self {
            InsideQuality::Low => 0,
            InsideQuality::Med => 1,
            InsideQuality::High => 2,
        }
    }

    /// Shell count for interior rendering
    pub fn shell_count(&self) -> i32 {
        match self {
            InsideQuality::Low => 2,
            InsideQuality::Med => 3,
            InsideQuality::High => 4,
        }
    }

    /// Max sparkle accents per frame
    pub fn accent_budget(&self) -> i32 {
        match self {
            InsideQuality::Low => 0,
            InsideQuality::Med => 6,
            InsideQuality::High => 12,
        }
    }

    /// Label density: 0=none, 1=ticks, 2=sparse numbers
    pub fn label_density(&self) -> i32 {
        match self {
            InsideQuality::Low => 0,
            InsideQuality::Med => 1,
            InsideQuality::High => 2,
        }
    }

    /// Fog enabled?
    pub fn fog_enabled(&self) -> bool {
        matches!(self, InsideQuality::High)
    }

    /// Sparkle enabled?
    pub fn sparkle_enabled(&self) -> bool {
        !matches!(self, InsideQuality::Low)
    }
}

/// Inside quality auto-scaler state
#[derive(Clone, Debug)]
pub struct InsideQualityState {
    /// Current quality tier
    pub quality: InsideQuality,
    /// Auto-scale enabled
    pub auto_scale: bool,
    /// Smoothed perf pressure (0-1, rises when over budget)
    pub perf_pressure: f32,
    /// Time at high pressure (for downgrade hysteresis)
    pub high_pressure_time: f32,
    /// Time at low pressure (for upgrade hysteresis)
    pub low_pressure_time: f32,
    /// Target frame time (ms)
    pub target_ms: f32,
    /// Margin before pressure rises (ms)
    pub margin_ms: f32,
}

impl Default for InsideQualityState {
    fn default() -> Self {
        Self {
            quality: InsideQuality::Med,
            auto_scale: true,
            perf_pressure: 0.0,
            high_pressure_time: 0.0,
            low_pressure_time: 0.0,
            target_ms: 16.0,  // 60fps target
            margin_ms: 2.0,   // Pressure rises above 18ms
        }
    }
}

impl InsideQualityState {
    /// Update quality based on frame time (call each frame when inside)
    pub fn update(&mut self, frame_time_ms: f32, dt: f32) {
        if !self.auto_scale {
            return;
        }

        let threshold = self.target_ms + self.margin_ms;

        // Update pressure (fast rise, slow fall)
        if frame_time_ms > threshold {
            // Pressure rises quickly
            let excess = (frame_time_ms - threshold) / 10.0;
            self.perf_pressure = (self.perf_pressure + excess * dt * 3.0).min(1.0);
            self.high_pressure_time += dt;
            self.low_pressure_time = 0.0;
        } else if frame_time_ms < self.target_ms - 2.0 {
            // Pressure falls slowly
            self.perf_pressure = (self.perf_pressure - dt * 0.3).max(0.0);
            self.low_pressure_time += dt;
            self.high_pressure_time = 0.0;
        } else {
            // In the safe zone
            self.high_pressure_time = 0.0;
            self.low_pressure_time = 0.0;
        }

        // Downgrade with hysteresis (0.7s sustained high pressure)
        if self.high_pressure_time > 0.7 && self.perf_pressure > 0.5 {
            match self.quality {
                InsideQuality::High => {
                    self.quality = InsideQuality::Med;
                    self.high_pressure_time = 0.0;
                    self.perf_pressure *= 0.5;
                }
                InsideQuality::Med => {
                    self.quality = InsideQuality::Low;
                    self.high_pressure_time = 0.0;
                    self.perf_pressure *= 0.5;
                }
                InsideQuality::Low => {}
            }
        }

        // Upgrade with hysteresis (2.0s sustained low pressure)
        if self.low_pressure_time > 2.0 && self.perf_pressure < 0.2 {
            match self.quality {
                InsideQuality::Low => {
                    self.quality = InsideQuality::Med;
                    self.low_pressure_time = 0.0;
                }
                InsideQuality::Med => {
                    self.quality = InsideQuality::High;
                    self.low_pressure_time = 0.0;
                }
                InsideQuality::High => {}
            }
        }
    }
}

/// Configuration for RingDive triggering and visuals
#[derive(Clone, Debug)]
pub struct RingDiveConfig {
    // Trigger thresholds (with hysteresis) - LOWERED for real music
    pub energy_enter_thresh: f32,      // 0.45 (was 0.55)
    pub tension_enter_thresh: f32,     // 0.58 (was 0.65)
    pub energy_enter_hold: f32,        // 0.4s (was 0.6s)
    pub tension_enter_hold: f32,       // 0.3s (was 0.4s)

    // Exit thresholds - only checked in LockOn phase now
    pub tension_exit_thresh: f32,      // 0.35
    pub energy_exit_thresh: f32,       // 0.25

    pub cooldown_duration: f32,        // 10.0s

    // Minimum progression rates per phase (ensures sequence completes)
    pub min_rate_lock_on: f32,         // 0.45 - t progresses at least this fast
    pub min_rate_warp_out: f32,        // 0.75
    pub min_rate_guided: f32,          // 0.18
    pub min_rate_warp_back: f32,       // 0.55

    // Minimum dwell times (hold before advancing to next phase)
    pub dwell_lock_on: f32,            // 0.35s
    pub dwell_warp_out: f32,           // 0.25s
    pub dwell_warp_back: f32,          // 0.35s

    // Visual tuning
    pub warp_strength: f32,            // 0.8
    pub portal_depth: f32,             // 0.15
    pub interior_alpha_budget: f32,    // 0.85 (clamp accumulated alpha)
}

impl Default for RingDiveConfig {
    fn default() -> Self {
        Self {
            // MIDI-friendly thresholds (13 notches: 0=none, 1=birth, 2-5=12fold, 6-8=24fold, 8-13=geo)
            // Notch 2 (~15%) should enter 12-fold (RingDive)
            energy_enter_thresh: 0.12,   // Was 0.35 - now ~notch 2
            tension_enter_thresh: 0.05,  // Lowered further
            energy_enter_hold: 0.6,      // Slightly faster response
            tension_enter_hold: 0.6,

            // Exit thresholds - stay inside longer
            tension_exit_thresh: 0.15,
            energy_exit_thresh: 0.08,    // Need to drop very low to exit

            cooldown_duration: 10.0,

            // Min rates - guided needs to progress enough to cross 0.60 for unfold24
            min_rate_lock_on: 0.45,
            min_rate_warp_out: 0.75,
            min_rate_guided: 0.25,       // ~18s for guided phase (0.35→0.80)
            min_rate_warp_back: 0.55,

            // Dwell times so you feel the beats
            dwell_lock_on: 0.35,
            dwell_warp_out: 0.25,
            dwell_warp_back: 0.35,

            warp_strength: 0.8,
            portal_depth: 0.15,
            interior_alpha_budget: 0.85,
        }
    }
}

/// RingDive state - single continuous curve [0..1]
#[derive(Clone, Debug)]
pub struct RingDiveState {
    pub active: bool,
    pub t: f32,                         // Master curve [0..1]
    pub target_band: usize,             // 0=bass, 1=mid, 2=high
    pub cooldown: f32,                  // Cooldown after completion

    // Trigger hysteresis accumulators
    pub energy_above_thresh_time: f32,  // Time section_energy > threshold
    pub tension_above_thresh_time: f32, // Time tension > threshold

    // Phase timing (for dwell times)
    pub phase_enter_time: f32,          // Time when current phase started
    pub current_phase: u8,              // 0=lock_on, 1=warp_out, 2=guided, 3=warp_back
    pub last_prog_rate: f32,            // Last progression rate (for debug)

    // Abort tracking (only in LockOn)
    pub abort_reason: Option<&'static str>,

    // Inside camera control
    pub guided_time: f32,               // Time spent in guided phase (for min 2s)
    pub orbit_suppress_timer: f32,      // Disable orbit for 0.5s on entry to guided

    // Smoothed camera targets (separate from position)
    pub look_target: [f32; 3],
    pub look_target_vel: [f32; 3],

    // Spline control points for Catmull-Rom
    pub path_points: [[f32; 3]; 5],

    // Track inside duration for validation
    pub inside_duration: f32,

    // Quality tier (auto-scaled based on frame time)
    pub quality: RingDiveQuality,

    // Quality auto-scaling timers
    pub quality_downgrade_time: f32,    // Time spent > 20ms frame time
    pub quality_upgrade_time: f32,      // Time spent < 14ms frame time

    // Config
    pub config: RingDiveConfig,
}

impl Default for RingDiveState {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            target_band: 1, // Default to mid band
            cooldown: 0.0,

            energy_above_thresh_time: 0.0,
            tension_above_thresh_time: 0.0,

            phase_enter_time: 0.0,
            current_phase: 0,
            last_prog_rate: 0.0,

            abort_reason: None,

            guided_time: 0.0,
            orbit_suppress_timer: 0.0,

            look_target: [0.0, 0.0, 0.0],
            look_target_vel: [0.0, 0.0, 0.0],

            path_points: [[0.0; 3]; 5],

            inside_duration: 0.0,

            quality: RingDiveQuality::Med,
            quality_downgrade_time: 0.0,
            quality_upgrade_time: 0.0,

            config: RingDiveConfig::default(),
        }
    }
}

/// Entry reason for 24-fold engagement (debug/telemetry)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fold24EntryReason {
    #[default]
    None,
    RingDive,
    TopologyRequest,
    User,
}

/// Fold24 state - secondary inside transition (12-fold → 24-fold)
/// Now supports dual-path engagement: RingDive or TopologyRequest
#[derive(Clone, Debug)]
pub struct Fold24State {
    pub active: bool,                   // DEPRECATED - use `engaged` instead
    pub t: f32,                         // Smoothed fold24_t [0..1], tau ~0.6s
    pub t_raw: f32,                     // Raw t (unsmoothed)
    pub cooldown: f32,                  // 8-12s cooldown after completion

    // === PERMISSION vs ENGAGEMENT (new model) ===
    /// Permission: macro narrative allows 24-fold (Journey, user override)
    pub permitted: bool,
    /// Engaged: we are currently rendering in 24-fold space
    pub engaged: bool,
    /// Engagement ramp (0..1) for visual intensity, smoothed
    pub engage_t: f32,
    /// How long the topology router has been requesting 24-fold (bar-edge accumulated)
    pub request_latch_bars: u32,
    /// Cooldown after disengagement to prevent spam
    pub request_cooldown_s: f32,
    /// Bar index when engagement started (for min hold enforcement)
    pub engaged_bar_index: u64,
    /// Why we entered 24-fold (debug/telemetry)
    pub entry_reason: Fold24EntryReason,
    /// Entry funnel: bars remaining before topology switching allowed
    pub entry_funnel_bars_left: u32,
    /// Which topology triggered the request (for preference on first switch)
    pub entry_requested_topology: Option<TopologyId>,

    // Trigger hysteresis
    pub tension_above_time: f32,        // Time tension > 0.7 during Guided
    pub energy_spike_time: f32,         // Time energy derivative > threshold

    // Camera witness beat
    pub witness_timer: f32,             // 250ms freeze timer
    pub aim_offset_angle: f32,          // +15° rotation during onset

    // Debug
    pub trigger_reason: Option<&'static str>,
}

impl Default for Fold24State {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            t_raw: 0.0,
            cooldown: 0.0,

            // Permission vs Engagement
            permitted: true,  // Default: 24-fold allowed
            engaged: false,
            engage_t: 0.0,
            request_latch_bars: 0,
            request_cooldown_s: 0.0,
            engaged_bar_index: 0,
            entry_reason: Fold24EntryReason::None,
            entry_funnel_bars_left: 0,
            entry_requested_topology: None,

            tension_above_time: 0.0,
            energy_spike_time: 0.0,

            witness_timer: 0.0,
            aim_offset_angle: 0.0,

            trigger_reason: None,
        }
    }
}

impl Fold24State {
    /// Inside mode PERMISSION: 0 = 12-fold permission, 1 = 24-fold permission
    /// NOTE: This is a PERMISSION flag for expensive 24-only passes, NOT a visual driver.
    /// For visual blending, use u_fold24_t (fold24.t) which is already smoothed.
    /// DO NOT use t > 0.1 threshold - that causes visual pops!
    pub fn inside_mode(&self) -> i32 {
        if self.active { 1 } else { 0 }
    }

    /// Debug string showing timeline state
    pub fn debug_string(&self) -> String {
        if self.active {
            format!("Fold24: ON t={:.2} raw={:.2} wit={:.0}ms",
                self.t, self.t_raw, self.witness_timer * 1000.0)
        } else if self.t > 0.01 || self.t_raw > 0.01 {
            // Transitioning (ramping up or down)
            format!("Fold24: ramp t={:.2} raw={:.2}", self.t, self.t_raw)
        } else {
            "Fold24: 12-fold".to_string()
        }
    }
}

// ============================================================================
// TOROIDAL KNOT CYMATICS - Tertiary Inside Transition (24-fold → Knot)
// ============================================================================
//
// When 24-fold is stable and sustained, the 24 spokes curl into a torus ring,
// then thread into a (p,q) torus knot. Nodes march along the knot strand.
//
// Subphases:
//   CurlToTorus (0.0-0.35): 24 spokes bend into donut ring
//   ThreadKnot  (0.35-0.75): Ring morphs into torus knot path
//   Stabilize   (0.75-1.0): Damped settle, nodes begin marching
//
// Audio mapping: interval between dominant + secondary pitch → (p,q) ratio
// ============================================================================

/// Torus knot transition subphase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TorusKnotSubphase {
    /// 24-fold spokes curl into torus ring (0.0 - 0.35)
    CurlToTorus,
    /// Ring threads into knot path (0.35 - 0.75)
    ThreadKnot,
    /// Damped settle, nodes march (0.75 - 1.0)
    Stabilize,
    /// Fully active, knot rendering
    Active,
    /// Not in torus knot mode
    Idle,
}

impl Default for TorusKnotSubphase {
    fn default() -> Self {
        Self::Idle
    }
}

impl TorusKnotSubphase {
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Idle => -1,
            Self::CurlToTorus => 0,
            Self::ThreadKnot => 1,
            Self::Stabilize => 2,
            Self::Active => 3,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::CurlToTorus => "CurlToTorus",
            Self::ThreadKnot => "ThreadKnot",
            Self::Stabilize => "Stabilize",
            Self::Active => "Active",
        }
    }
}

/// Torus knot quality tier (auto-scaled based on frame time)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TorusKnotQuality {
    /// 64 nodes, 64 segments
    Low,
    /// 128 nodes, 128 segments
    #[default]
    Med,
    /// 256 nodes, 256 segments
    High,
}

impl TorusKnotQuality {
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Low => 0,
            Self::Med => 1,
            Self::High => 2,
        }
    }

    pub fn node_count(&self) -> i32 {
        match self {
            Self::Low => 64,
            Self::Med => 128,
            Self::High => 256,
        }
    }
}

/// Configuration for torus knot rendering
#[derive(Clone, Debug)]
pub struct TorusKnotConfig {
    /// Major radius of torus (donut size)
    pub major_radius: f32,
    /// Minor radius of torus (tube thickness)
    pub minor_radius: f32,
    /// Line thickness for strand rendering
    pub line_thickness: f32,
    /// Node glow radius
    pub node_radius: f32,
    /// Node march speed multiplier
    pub march_speed: f32,
    /// Transition durations (seconds)
    pub curl_duration: f32,
    pub thread_duration: f32,
    pub stabilize_duration: f32,
    /// Trigger thresholds
    pub stability_thresh: f32,
    pub stability_hold_time: f32,
    pub energy_thresh: f32,
    /// Cooldown after exit
    pub cooldown_duration: f32,
    /// Video fade amount during ThreadKnot (0 = full fade, 1 = no fade)
    pub video_fade_min: f32,

    // === TWO-STAGE LATCH SYSTEM (Stage A: 24-fold latch, Stage B: knot charge) ===

    // Stage A: 24-fold latch with hysteresis
    /// fold24.t threshold to ENTER latched state (must exceed this)
    pub fold24_enter_t: f32,
    /// fold24.t threshold to EXIT latched state (must drop below this)
    pub fold24_exit_t: f32,
    /// Seconds above enter threshold before latching ON
    pub fold24_enter_hold: f32,
    /// Seconds below exit threshold before latching OFF
    pub fold24_exit_hold: f32,

    // Stage B: Knot charge accumulation
    /// Harmonic confidence threshold for charging (tonal_confidence_smooth)
    pub harmonic_conf_required: f32,
    /// Charge accumulation rate (per second)
    pub charge_rate: f32,
    /// Charge decay rate when conditions not met (per second) - SOFT decay, not reset
    pub decay_rate: f32,
    /// Section energy threshold for charging
    pub energy_required: f32,

    // Polarity flip bonus (NOT a hard requirement)
    /// Minimum semitone distance to count as a flip (e.g. 3 = minor 3rd)
    pub flip_distance: i32,
    /// Flip score bonus rate (knot_charge += flip_score * bonus_rate * dt)
    pub flip_bonus_rate: f32,
    /// Flip score decay rate (seconds to decay from 1.0 to 0)
    pub flip_decay_tau: f32,

    // Activation conditions
    /// Minimum fold24 hold time for activation (seconds)
    pub charge_min_hold: f32,
    /// Reentry lockout after exit (seconds) - prevents ping-pong
    pub reentry_lockout_secs: f32,

    // === PERCEPTUAL SUSTAIN SYSTEM ===
    // Once torus is active, it's sustained by support score, not 24-fold purity

    // Support score weights (must sum to ~1.0 for intuitive thresholds)
    /// Weight for slow energy in support score
    pub support_w_energy: f32,
    /// Weight for stability in support score
    pub support_w_stability: f32,
    /// Weight for harmonic confidence in support score
    pub support_w_harmonic: f32,
    /// Weight for tension penalty in support score (subtracted)
    pub support_w_tension: f32,

    // Support score dynamics (attack faster than release)
    /// Support attack rate (reaches 1.0 in ~0.8s)
    pub support_attack: f32,
    /// Support release rate (decays over ~3s)
    pub support_release: f32,

    // Stay/exit thresholds
    /// Support threshold to stay in torus (forgiving)
    pub stay_threshold: f32,
    /// Support threshold for hard exit (decisive)
    pub hard_exit_threshold: f32,
    /// Minimum time torus must stay visible (seconds)
    pub torus_min_on_secs: f32,
    /// Grace period before exiting when support drops
    pub exit_grace_secs: f32,

    // Hard drop override (24-fold collapses completely)
    /// fold24.t below which triggers hard drop check
    pub fold24_hard_drop_t: f32,
    /// Grace period for hard drop (seconds)
    pub hard_drop_grace: f32,

    // Silence/instability freeze gates
    /// Stability threshold for charging (derived from 1-tension_slow)
    pub stability_required: f32,
    /// If true, freeze (don't decay) charge during silence
    pub freeze_on_silence: bool,
    /// Silence threshold (energy below this = silence)
    pub silence_threshold: f32,

    // === ENHANCED VISUAL PARAMETERS ===
    /// Visual tube radius for SDF shading
    pub tube_radius: f32,
    /// Video rim light intensity
    pub rim_intensity: f32,
    /// Specular highlight power (sharpness)
    pub spec_power: f32,
    /// Specular highlight intensity
    pub spec_intensity: f32,
    /// Depth haze / fog strength
    pub depth_haze: f32,
    /// Bloom threshold
    pub bloom_thresh: f32,
    /// Particle spawn rate multiplier
    pub particle_rate: f32,
    /// Camera orbit speed
    pub cam_orbit: f32,
    /// Camera distance
    pub cam_dist: f32,
    /// Debug flags bitfield
    pub debug_flags: u32,

    // === PRO-LEVEL VORTEX VISUAL PARAMETERS ===
    /// Chromatic fringing intensity (High tier only, 0.0-1.0)
    pub chromatic_intensity: f32,
    /// Subsurface scattering approximation intensity (0.0-1.0)
    pub sss_intensity: f32,

    // === PREMIUM 3D QUANTIZATION CONFIG ===
    /// Minimum beats between camera orbit steps
    pub camera_step_min_beats: f32,
    /// Minimum beats between tube radius changes
    pub tube_step_min_beats: f32,
    /// Minimum beats between vortex strength changes
    pub vortex_step_min_beats: f32,
    /// Tube radius discrete levels [thin, normal, thick, bold]
    pub tube_radius_levels: [f32; 4],
}

impl Default for TorusKnotConfig {
    fn default() -> Self {
        Self {
            major_radius: 0.75,  // Sized to wrap around QS Wheel
            minor_radius: 0.25,
            line_thickness: 0.008,
            node_radius: 0.025,
            march_speed: 0.3,
            curl_duration: 1.2,
            thread_duration: 1.5,
            stabilize_duration: 0.8,
            stability_thresh: 0.40,      // Stable harmonic content
            stability_hold_time: 2.0,    // Must sustain for 2.0 seconds
            energy_thresh: 0.58,         // MIDI notches 8-13 (~62%+) for geometric
            cooldown_duration: 8.0,
            video_fade_min: 0.15,

            // === TWO-STAGE LATCH DEFAULTS ===

            // Stage A: 24-fold latch with hysteresis (prevents oscillation)
            fold24_enter_t: 0.50,        // Enter when fold24.t >= 0.50
            fold24_exit_t: 0.35,         // Exit when fold24.t <= 0.35
            fold24_enter_hold: 0.30,     // Hold 300ms above enter threshold
            fold24_exit_hold: 0.60,      // Hold 600ms below exit threshold

            // Stage B: Knot charge accumulation
            harmonic_conf_required: 0.50, // Require decent harmonic confidence
            charge_rate: 1.5,            // Moderate charge rate
            decay_rate: 0.12,            // Moderate decay
            energy_required: 0.25,       // Need some energy

            // Polarity flip bonus (adds to charge, not required)
            flip_distance: 3,            // Minor 3rd or larger counts
            flip_bonus_rate: 0.10,       // Flip score * 0.10 added to charge/sec
            flip_decay_tau: 2.0,         // Flip score decays over 2 seconds

            // Activation
            charge_min_hold: 0.40,       // Need 400ms at latched 24-fold
            reentry_lockout_secs: 4.0,   // 4s lockout after exit

            // === PERCEPTUAL SUSTAIN DEFAULTS ===

            // Support score weights (sum to ~1.0)
            support_w_energy: 0.30,      // Slow energy contributes 30%
            support_w_stability: 0.30,   // Stability contributes 30%
            support_w_harmonic: 0.25,    // Harmonic confidence contributes 25%
            support_w_tension: 0.15,     // Tension penalty (subtracted)

            // Support dynamics (attack faster than release for "sticky" feel)
            support_attack: 1.2,         // Ramps up in ~0.8s
            support_release: 0.35,       // Falls over ~3s

            // Stay/exit thresholds
            stay_threshold: 0.55,        // Forgiving - stay if support >= 55%
            hard_exit_threshold: 0.40,   // Decisive - exit if support <= 40%
            torus_min_on_secs: 5.0,      // Minimum 5s visible (no flicker)
            exit_grace_secs: 1.0,        // 1s grace before exiting

            // Hard drop override (24-fold completely collapses)
            fold24_hard_drop_t: 0.35,    // Very low threshold
            hard_drop_grace: 0.25,       // 250ms grace for hard drop

            // Freeze gates
            stability_required: 0.35,    // Easy to satisfy
            freeze_on_silence: true,     // Don't decay charge during silence
            silence_threshold: 0.10,     // Below 10% energy = silence

            // Enhanced visual defaults (cinematic, luminous, stable)
            tube_radius: 0.04,       // Thick enough to read as tube, not line
            rim_intensity: 0.8,      // Strong video rim light
            spec_power: 24.0,        // Medium-sharp specular
            spec_intensity: 0.6,     // Subtle but visible
            depth_haze: 0.35,        // Mild depth fog
            bloom_thresh: 0.7,       // Bloom only on bright features
            particle_rate: 1.0,      // Full particle density
            cam_orbit: 1.0,          // Slow, controlled orbit
            cam_dist: 2.0,           // Medium distance
            debug_flags: 0,          // All features on by default

            // Pro-level vortex visual defaults
            chromatic_intensity: 0.5,   // Moderate chromatic fringing (High tier only)
            sss_intensity: 0.4,         // Subtle subsurface scattering approximation

            // Premium 3D quantization defaults
            camera_step_min_beats: 2.0,   // Orbit steps only every 2 beats
            tube_step_min_beats: 4.0,     // Tube radius changes every 4 beats
            vortex_step_min_beats: 2.0,   // Vortex strength changes every 2 beats
            tube_radius_levels: [0.015, 0.025, 0.04, 0.06],  // [thin, normal, thick, bold]
        }
    }
}

/// Interval to (p,q) ratio mapping for musically coherent torus knots
/// Maps semitone intervals to simple consonant ratios
#[derive(Clone, Copy, Debug)]
pub struct IntervalRatio {
    pub p: i32,
    pub q: i32,
}

impl IntervalRatio {
    pub const fn new(p: i32, q: i32) -> Self {
        Self { p, q }
    }

    /// Get ratio for a semitone interval (0-11)
    pub fn from_interval(interval: usize) -> Self {
        // Map intervals to simple consonant ratios (reduced to co-prime)
        match interval % 12 {
            0 => Self::new(1, 1),    // Unison - circle
            1 => Self::new(16, 15),  // Minor 2nd
            2 => Self::new(9, 8),    // Major 2nd
            3 => Self::new(6, 5),    // Minor 3rd
            4 => Self::new(5, 4),    // Major 3rd
            5 => Self::new(4, 3),    // Perfect 4th
            6 => Self::new(7, 5),    // Tritone (simplified from 45:32)
            7 => Self::new(3, 2),    // Perfect 5th - trefoil!
            8 => Self::new(8, 5),    // Minor 6th
            9 => Self::new(5, 3),    // Major 6th
            10 => Self::new(9, 5),   // Minor 7th (simplified from 16:9)
            11 => Self::new(15, 8),  // Major 7th
            _ => Self::new(3, 2),    // Default to perfect 5th
        }
    }

    /// Get EXOTIC ratio for interval - uses 7-limit and 11-limit alternatives
    /// These create more complex, visually interesting torus knots
    /// Called when harmonic confidence is high and we want "spice"
    pub fn from_interval_exotic(interval: usize) -> Self {
        match interval % 12 {
            0 => Self::new(1, 1),    // Unison - keep simple
            1 => Self::new(16, 15),  // Minor 2nd - keep (already interesting)
            2 => Self::new(8, 7),    // Major 2nd → septimal whole tone (7-limit)
            3 => Self::new(7, 6),    // Minor 3rd → septimal minor 3rd (7-limit)
            4 => Self::new(9, 7),    // Major 3rd → septimal major 3rd (7-limit)
            5 => Self::new(11, 8),   // Perfect 4th → undecimal tritone (11-limit)
            6 => Self::new(11, 8),   // Tritone → undecimal (11-limit)
            7 => Self::new(7, 4),    // Perfect 5th → harmonic 7th (7-limit) - dramatic!
            8 => Self::new(11, 7),   // Minor 6th → undecimal (11-limit)
            9 => Self::new(12, 7),   // Major 6th → septimal (7-limit)
            10 => Self::new(7, 4),   // Minor 7th → harmonic 7th (7-limit) - THE classic
            11 => Self::new(11, 6),  // Major 7th → undecimal neutral 7th (11-limit)
            _ => Self::new(7, 4),    // Default to harmonic 7th
        }
    }

    /// Golden carrier knot for unstable/ambiguous intervals
    /// T(34, 55) - Fibonacci-based, creates beautiful patterns
    pub const fn golden_carrier() -> Self {
        Self::new(34, 55)
    }

    /// Simplified golden carrier for lower GPU load
    /// T(8, 13) - smaller Fibonacci pair
    pub const fn golden_carrier_simple() -> Self {
        Self::new(8, 13)
    }
}

/// Recognize the torus knot family from actual (p,q) topology.
/// Returns a human-readable name only if the topology matches a known family.
/// This is recognition-only labeling - it never drives topology selection.
pub fn recognize_family(p: i32, q: i32) -> &'static str {
    match (p, q) {
        // Simple consonant ratios (intervals)
        (1, 1) => "Unison",
        (3, 2) => "P5 (Trefoil)",
        (4, 3) => "P4",
        (5, 4) => "Maj3",
        (5, 3) => "Maj6",
        (6, 5) => "Min3",
        (8, 5) => "Min6",
        (9, 8) => "Maj2",
        (16, 15) => "Min2",
        (7, 5) => "Tritone",
        (9, 5) => "Min7",
        (15, 8) => "Maj7",
        // Golden family (Fibonacci)
        (8, 13) => "Golden",
        (13, 21) => "Golden",
        (21, 34) => "Golden",
        (34, 55) => "Golden",
        (55, 89) => "Golden",
        (89, 144) => "Golden",
        // Lucas family
        (7, 11) => "Lucas",
        (11, 18) => "Lucas",
        (18, 29) => "Lucas",
        (29, 47) => "Lucas",
        (47, 76) => "Lucas",
        (76, 123) => "Lucas",
        // Silver family (Pell)
        (5, 12) => "Silver",
        (12, 29) => "Silver",
        (29, 70) => "Silver",
        // Bronze family
        (9, 13) => "Bronze",
        (13, 22) => "Bronze",
        (22, 35) => "Bronze",
        // Unknown - return empty (no misleading label)
        _ => "",
    }
}

/// Emergence state machine for torus knot activation
/// Prevents frame spikes by gating expensive work
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EmergenceState {
    /// Not emerging - normal 24-fold rendering
    #[default]
    Idle,
    /// Intent detected, showing cheap preview cue (scaffold only)
    /// Waiting for admission control to allow prewarm
    Candidate,
    /// Invisible prep work in progress (budgeted, multi-frame)
    /// active=false, no torus shader work yet
    Prewarm,
    /// Fully active torus knot rendering
    Active,
}

impl EmergenceState {
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::Candidate => 1,
            Self::Prewarm => 2,
            Self::Active => 3,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Candidate => "Candidate",
            Self::Prewarm => "Prewarm",
            Self::Active => "Active",
        }
    }
}

// ============================================================================
// TORUS CANVAS/PAINT SYSTEM - Quantized Cymatic Topology
// ============================================================================
//
// Two-layer discrete system for stable, readable torus knot cymatics:
//
// CANVAS = T(p,q) torus knot preset (rarely changes)
//   - Golden family: Fibonacci-based (8,13), (13,21), (21,34), (34,55)
//   - Lucas family: Lucas-based (7,11), (11,18), (18,29), (29,47)
//   - Silver family: Pell-based (5,12), (12,29), (29,70)
//   - Bronze family: Bronze-ratio (9,13), (13,22), (22,35)
//
// PAINT = Point frequency / node pattern (can change more often, but quantized)
//   - Just Intonation ratios for stable, non-moving node patterns
//   - Optional 12-TET ratios with stepped phase offsets (never continuous)
//
// 24-FOLD SYMMETRY enforced via replica method: render knot at 24 anchor angles
// ============================================================================

/// Torus Canvas preset - the knot topology T(p,q)
/// Uses curated families from "Mathematics of Toroidal Knot Cymatics"
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TorusCanvas {
    // === GOLDEN FAMILY (Fibonacci ratios, φ-based) ===
    /// T(8,13) - Simple golden, low complexity
    #[default]
    Golden_8_13,
    /// T(13,21) - Medium golden
    Golden_13_21,
    /// T(21,34) - Rich golden
    Golden_21_34,
    /// T(34,55) - Full golden carrier
    Golden_34_55,
    /// T(55,89) - Extended golden (high complexity)
    Golden_55_89,
    /// T(89,144) - Standard golden carrier from PDF (very high complexity)
    Golden_89_144,

    // === LUCAS FAMILY (Lucas numbers) ===
    /// T(7,11) - Simple Lucas
    Lucas_7_11,
    /// T(11,18) - Medium Lucas
    Lucas_11_18,
    /// T(18,29) - Rich Lucas
    Lucas_18_29,
    /// T(29,47) - Full Lucas
    Lucas_29_47,
    /// T(47,76) - Extended Lucas (high complexity)
    Lucas_47_76,
    /// T(76,123) - Full Lucas carrier (very high complexity)
    Lucas_76_123,

    // === SILVER FAMILY (Pell ratios, √2-based) ===
    /// T(5,12) - Simple silver
    Silver_5_12,
    /// T(12,29) - Medium silver
    Silver_12_29,
    /// T(29,70) - Full silver
    Silver_29_70,

    // === BRONZE FAMILY (Bronze ratio) ===
    /// T(9,13) - Simple bronze
    Bronze_9_13,
    /// T(13,22) - Medium bronze
    Bronze_13_22,
    /// T(22,35) - Full bronze
    Bronze_22_35,

    // === SIMPLE RATIOS (consonant intervals) ===
    /// T(3,2) - Perfect fifth / trefoil
    Simple_3_2,
    /// T(5,3) - Major sixth
    Simple_5_3,
    /// T(5,4) - Major third
    Simple_5_4,
    /// T(4,3) - Perfect fourth
    Simple_4_3,
}

impl TorusCanvas {
    /// Get (p, q) parameters for this canvas
    pub fn pq(&self) -> (i32, i32) {
        match self {
            // Golden family
            Self::Golden_8_13 => (8, 13),
            Self::Golden_13_21 => (13, 21),
            Self::Golden_21_34 => (21, 34),
            Self::Golden_34_55 => (34, 55),
            Self::Golden_55_89 => (55, 89),
            Self::Golden_89_144 => (89, 144),
            // Lucas family
            Self::Lucas_7_11 => (7, 11),
            Self::Lucas_11_18 => (11, 18),
            Self::Lucas_18_29 => (18, 29),
            Self::Lucas_29_47 => (29, 47),
            Self::Lucas_47_76 => (47, 76),
            Self::Lucas_76_123 => (76, 123),
            // Silver family
            Self::Silver_5_12 => (5, 12),
            Self::Silver_12_29 => (12, 29),
            Self::Silver_29_70 => (29, 70),
            // Bronze family
            Self::Bronze_9_13 => (9, 13),
            Self::Bronze_13_22 => (13, 22),
            Self::Bronze_22_35 => (22, 35),
            // Simple ratios
            Self::Simple_3_2 => (3, 2),
            Self::Simple_5_3 => (5, 3),
            Self::Simple_5_4 => (5, 4),
            Self::Simple_4_3 => (4, 3),
        }
    }

    /// Get link count (gcd of p,q) - 1 = knot, >1 = link
    pub fn link_count(&self) -> i32 {
        let (p, q) = self.pq();
        gcd(p, q)
    }

    /// Is this a true knot (link_count == 1)?
    pub fn is_knot(&self) -> bool {
        self.link_count() == 1
    }

    /// Complexity tier (0=simple, 1=medium, 2=rich, 3=full, 4=extreme)
    pub fn complexity(&self) -> i32 {
        match self {
            Self::Golden_8_13 | Self::Lucas_7_11 | Self::Silver_5_12 | Self::Bronze_9_13 => 0,
            Self::Golden_13_21 | Self::Lucas_11_18 | Self::Silver_12_29 | Self::Bronze_13_22 => 1,
            Self::Golden_21_34 | Self::Lucas_18_29 | Self::Bronze_22_35 => 2,
            Self::Golden_34_55 | Self::Lucas_29_47 | Self::Silver_29_70 => 3,
            Self::Golden_55_89 | Self::Lucas_47_76 => 4,
            Self::Golden_89_144 | Self::Lucas_76_123 => 4,
            Self::Simple_3_2 | Self::Simple_5_3 | Self::Simple_5_4 | Self::Simple_4_3 => 0,
        }
    }

    /// Human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Golden_8_13 => "Golden 8:13",
            Self::Golden_13_21 => "Golden 13:21",
            Self::Golden_21_34 => "Golden 21:34",
            Self::Golden_34_55 => "Golden 34:55",
            Self::Golden_55_89 => "Golden 55:89",
            Self::Golden_89_144 => "Golden 89:144",
            Self::Lucas_7_11 => "Lucas 7:11",
            Self::Lucas_11_18 => "Lucas 11:18",
            Self::Lucas_18_29 => "Lucas 18:29",
            Self::Lucas_29_47 => "Lucas 29:47",
            Self::Lucas_47_76 => "Lucas 47:76",
            Self::Lucas_76_123 => "Lucas 76:123",
            Self::Silver_5_12 => "Silver 5:12",
            Self::Silver_12_29 => "Silver 12:29",
            Self::Silver_29_70 => "Silver 29:70",
            Self::Bronze_9_13 => "Bronze 9:13",
            Self::Bronze_13_22 => "Bronze 13:22",
            Self::Bronze_22_35 => "Bronze 22:35",
            Self::Simple_3_2 => "Trefoil 3:2",
            Self::Simple_5_3 => "Maj6 5:3",
            Self::Simple_5_4 => "Maj3 5:4",
            Self::Simple_4_3 => "P4 4:3",
        }
    }

    /// Get as integer ID for shader
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Golden_8_13 => 0,
            Self::Golden_13_21 => 1,
            Self::Golden_21_34 => 2,
            Self::Golden_34_55 => 3,
            Self::Golden_55_89 => 4,
            Self::Golden_89_144 => 5,
            Self::Lucas_7_11 => 6,
            Self::Lucas_11_18 => 7,
            Self::Lucas_18_29 => 8,
            Self::Lucas_29_47 => 9,
            Self::Lucas_47_76 => 10,
            Self::Lucas_76_123 => 11,
            Self::Silver_5_12 => 12,
            Self::Silver_12_29 => 13,
            Self::Silver_29_70 => 14,
            Self::Bronze_9_13 => 15,
            Self::Bronze_13_22 => 16,
            Self::Bronze_22_35 => 17,
            Self::Simple_3_2 => 18,
            Self::Simple_5_3 => 19,
            Self::Simple_5_4 => 20,
            Self::Simple_4_3 => 21,
        }
    }
    /// Select canvas based on quality tier and confidence (Golden family only)
    /// Quality: 0=Low, 1=Med, 2=High
    /// Confidence: 0.0-1.0 (stability/confidence of audio detection)
    pub fn for_quality_and_confidence(quality: i32, confidence: f32) -> Self {
        // Default: no lattice alignment, no invariants
        Self::for_reg_context(quality, confidence, 7, false, 0)
    }

    // ========================================================================
    // REG MATHEMATICAL CONSTANTS
    // ========================================================================

    /// φ (phi) - Golden ratio, limit of Fibonacci ratios F(n+1)/F(n)
    const PHI: f32 = 1.618033988749895;
    /// 1/φ = φ-1 ≈ 0.618 - limit of p/q ratios for Golden/Lucas families
    const PHI_INV: f32 = 0.618033988749895;
    /// δ_S (silver ratio) = √2 - 1 ≈ 0.414 - limit of Pell ratios
    const SILVER: f32 = 0.41421356237;
    /// Bronze ratio ≈ 0.303
    const BRONZE: f32 = 0.30277563773;

    // ========================================================================
    // INTERVAL TO RATIO MAPPING (Just Intonation)
    // ========================================================================

    /// Musical interval → frequency ratio (Just Intonation)
    /// These ratios come from the harmonic series
    fn interval_ratio(semitones: usize) -> f32 {
        match semitones % 12 {
            0 => 1.0,           // Unison (1:1)
            1 => 16.0 / 15.0,   // m2 - chromatic semitone
            2 => 9.0 / 8.0,     // M2 - major whole tone
            3 => 6.0 / 5.0,     // m3 - minor third
            4 => 5.0 / 4.0,     // M3 - major third
            5 => 4.0 / 3.0,     // P4 - perfect fourth
            6 => 7.0 / 5.0,     // Tritone - septimal
            7 => 3.0 / 2.0,     // P5 - perfect fifth
            8 => 8.0 / 5.0,     // m6 - minor sixth
            9 => 5.0 / 3.0,     // M6 - major sixth
            10 => 9.0 / 5.0,    // m7 - minor seventh
            11 => 15.0 / 8.0,   // M7 - major seventh
            _ => 1.0,
        }
    }

    /// Compute affinity score using logarithmic (musical) distance
    /// Returns 0.0-1.0 where 1.0 = perfect match
    fn family_affinity(interval_ratio: f32, family_ratio: f32) -> f32 {
        let log_diff = (interval_ratio.ln() - family_ratio.ln()).abs();
        // Gaussian: e^(-d²/σ²) with σ = 0.3 (tuned for musical perception)
        (-log_diff * log_diff / 0.09).exp()
    }

    // ========================================================================
    // REG-DRIVEN CANVAS SELECTION
    // ========================================================================
    //
    // In REG's framework:
    // - When QC invariants are satisfied, we're in a "harmonically locked" state
    // - lattice_ok_24 means alignment with the 24-fold (φ-based) symmetry grid
    // - More invariants satisfied = higher harmonic coherence = allow more complexity
    //
    // Family selection via interval ratio proximity:
    // - Golden (Fibonacci): p/q → 1/φ ≈ 0.618, major/bright character
    // - Lucas ("twin" of Fibonacci): same limit, minor/reflective character
    // - Silver (Pell): p/q → √2-1 ≈ 0.414, angular/dissonant
    // - Bronze: p/q → 0.303, alternative tension
    // ========================================================================

    /// Select canvas using full REG context
    /// - quality: GPU quality tier (0-2)
    /// - confidence: Audio stability/confidence (0.0-1.0)
    /// - interval: Detected interval in semitones (0-11)
    /// - lattice_ok: True if QC lattice_ok_24 invariant is satisfied (on 24-fold grid)
    /// - invariant_count: Number of QC invariants satisfied (0-4)
    pub fn for_reg_context(
        quality: i32,
        confidence: f32,
        interval: usize,
        lattice_ok: bool,
        invariant_count: usize,
    ) -> Self {
        // Base complexity from quality + confidence
        let effective = (quality as f32 * 0.5 + confidence * 0.5).clamp(0.0, 1.0);

        // REG coherence bonus: more invariants = more complexity allowed
        // 0 invariants = 0 bonus, 4 invariants = 0.2 bonus
        let coherence_bonus = invariant_count as f32 * 0.05;
        let effective = (effective + coherence_bonus).clamp(0.0, 1.0);

        // Complexity 4 only at High quality AND (high confidence OR high coherence)
        let allow_extreme = quality >= 2 && (confidence > 0.8 || invariant_count >= 3);

        // Get interval's frequency ratio
        let ratio = Self::interval_ratio(interval);
        let ratio_inv = 1.0 / ratio; // p/q form

        // ====================================================================
        // SIMPLE RATIOS: Exact matches at very high stability
        // These create the most stable, readable patterns
        // ====================================================================
        if confidence > 0.85 && effective > 0.3 {
            match interval % 12 {
                7 => return Self::Simple_3_2,  // P5 = 3:2 exactly (trefoil)
                5 => return Self::Simple_4_3,  // P4 = 4:3 exactly
                4 => return Self::Simple_5_4,  // M3 = 5:4 exactly
                9 => return Self::Simple_5_3,  // M6 = 5:3 exactly
                _ => {}
            }
        }

        // ====================================================================
        // FAMILY AFFINITY SCORES
        // ====================================================================
        let golden_affinity = Self::family_affinity(ratio_inv, Self::PHI_INV);
        let silver_affinity = Self::family_affinity(ratio_inv, Self::SILVER);
        let bronze_affinity = Self::family_affinity(ratio_inv, Self::BRONZE);

        // Interval character
        let is_minor = matches!(interval % 12, 3 | 8 | 10); // m3, m6, m7
        let is_dissonant = matches!(interval % 12, 1 | 2 | 6 | 10 | 11); // 2nds, tritone, 7ths

        // ====================================================================
        // FAMILY SCORING with REG context
        // ====================================================================
        // Golden: favored when on lattice (24-fold alignment)
        // Lucas: favored for minor intervals (REG's "twin" relationship)
        // Silver: favored for dissonant intervals
        // Bronze: fallback alternative
        let lattice_boost = if lattice_ok { 1.3 } else { 1.0 };

        let golden_score = golden_affinity * lattice_boost * if is_minor { 0.8 } else { 1.2 };
        let lucas_score = golden_affinity * if is_minor { 1.4 } else { 0.8 };
        let silver_score = silver_affinity * if is_dissonant { 1.3 } else { 0.9 };
        let bronze_score = bronze_affinity;

        // Find winning family
        let max_score = golden_score.max(lucas_score).max(silver_score).max(bronze_score);

        // ====================================================================
        // FAMILY SELECTION with complexity scaling
        // ====================================================================
        if max_score == lucas_score || (is_minor && golden_affinity > 0.3) {
            // LUCAS FAMILY - minor/reflective character
            // REG: Lucas is the "twin" of Fibonacci, same φ limit
            if effective < 0.25 {
                Self::Lucas_7_11
            } else if effective < 0.5 {
                Self::Lucas_11_18
            } else if effective < 0.75 {
                Self::Lucas_18_29
            } else if allow_extreme && confidence > 0.9 {
                Self::Lucas_76_123
            } else if allow_extreme {
                Self::Lucas_47_76
            } else {
                Self::Lucas_29_47
            }
        } else if max_score == silver_score && silver_affinity > 0.35 {
            // SILVER FAMILY - angular/dissonant character
            // Based on √2, the diagonal of the unit square
            if effective < 0.3 {
                Self::Silver_5_12
            } else if effective < 0.6 {
                Self::Silver_12_29
            } else {
                Self::Silver_29_70
            }
        } else if max_score == bronze_score && bronze_affinity > 0.4 {
            // BRONZE FAMILY - alternative tension
            if effective < 0.3 {
                Self::Bronze_9_13
            } else if effective < 0.6 {
                Self::Bronze_13_22
            } else {
                Self::Bronze_22_35
            }
        } else {
            // GOLDEN FAMILY - major/bright character (default)
            // REG: Fibonacci ratios converge to φ, nature's organizing principle
            if effective < 0.2 {
                Self::Golden_8_13
            } else if effective < 0.4 {
                Self::Golden_13_21
            } else if effective < 0.6 {
                Self::Golden_21_34
            } else if effective < 0.8 {
                Self::Golden_34_55
            } else if allow_extreme && invariant_count >= 3 {
                // T(89,144): Only when REG invariants confirm coherence
                Self::Golden_89_144
            } else if allow_extreme {
                Self::Golden_55_89
            } else {
                Self::Golden_34_55
            }
        }
    }

    /// Convenience wrapper that doesn't require QC state
    pub fn for_audio_context(quality: i32, confidence: f32, interval: usize) -> Self {
        Self::for_reg_context(quality, confidence, interval, false, 0)
    }

    /// Returns all variants for UI dropdown
    pub fn all_variants() -> &'static [Self] {
        &[
            Self::Golden_8_13,
            Self::Golden_13_21,
            Self::Golden_21_34,
            Self::Golden_34_55,
            Self::Golden_55_89,
            Self::Golden_89_144,
            Self::Lucas_7_11,
            Self::Lucas_11_18,
            Self::Lucas_18_29,
            Self::Lucas_29_47,
            Self::Lucas_47_76,
            Self::Lucas_76_123,
            Self::Silver_5_12,
            Self::Silver_12_29,
            Self::Silver_29_70,
            Self::Bronze_9_13,
            Self::Bronze_13_22,
            Self::Bronze_22_35,
            Self::Simple_3_2,
            Self::Simple_5_3,
            Self::Simple_5_4,
            Self::Simple_4_3,
        ]
    }

    /// Winding interval in degrees: WI = p * 360 / q
    /// This is the angular advance per revolution, key to cymatic pattern
    pub fn winding_interval(&self) -> f32 {
        let (p, q) = self.pq();
        (p as f32 * 360.0) / q as f32
    }

    /// Returns true if this is a high-complexity variant that may impact performance
    pub fn is_high_complexity(&self) -> bool {
        self.complexity() >= 4
    }
}

/// Torus Paint preset - the cymatic node pattern
/// Determines where nodes appear along the canvas via stable ratios
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TorusPaint {
    // === JUST INTONATION (stable, non-moving patterns) ===
    /// 1:1 - Resonant unison (static line/wave)
    #[default]
    Just_1_1,
    /// 2:1 - Octave (two-ring feel)
    Just_2_1,
    /// 3:2 - Perfect fifth
    Just_3_2,
    /// 4:3 - Perfect fourth
    Just_4_3,
    /// 5:4 - Major third
    Just_5_4,
    /// 5:3 - Major sixth
    Just_5_3,
    /// 6:5 - Minor third
    Just_6_5,
    /// 8:5 - Minor sixth
    Just_8_5,

    // === 12-TET (stepped phase offsets, never continuous) ===
    /// Minor 3rd in 12-TET (3 semitones)
    TET_m3,
    /// Major 3rd in 12-TET (4 semitones)
    TET_M3,
    /// Perfect 5th in 12-TET (7 semitones)
    TET_P5,
    /// Tritone in 12-TET (6 semitones)
    TET_Tritone,
}

impl TorusPaint {
    /// Get frequency ratio (numerator, denominator) for this paint
    pub fn ratio(&self) -> (i32, i32) {
        match self {
            // Just intonation
            Self::Just_1_1 => (1, 1),
            Self::Just_2_1 => (2, 1),
            Self::Just_3_2 => (3, 2),
            Self::Just_4_3 => (4, 3),
            Self::Just_5_4 => (5, 4),
            Self::Just_5_3 => (5, 3),
            Self::Just_6_5 => (6, 5),
            Self::Just_8_5 => (8, 5),
            // 12-TET approximations (expressed as simple ratios)
            Self::TET_m3 => (6, 5),   // ~1.189 vs 1.200
            Self::TET_M3 => (5, 4),   // ~1.260 vs 1.250
            Self::TET_P5 => (3, 2),   // ~1.498 vs 1.500
            Self::TET_Tritone => (7, 5), // ~1.414 vs 1.400
        }
    }

    /// Is this a 12-TET paint? (allows stepped phase wobble)
    pub fn is_tet(&self) -> bool {
        matches!(self, Self::TET_m3 | Self::TET_M3 | Self::TET_P5 | Self::TET_Tritone)
    }

    /// Compute node frequency F from canvas base p
    /// F determines how many nodes appear along the knot
    pub fn node_frequency(&self, canvas_p: i32) -> i32 {
        let (num, den) = self.ratio();
        // F = round(base * ratio)
        ((canvas_p * num) as f32 / den as f32).round() as i32
    }

    /// Human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Just_1_1 => "Unison",
            Self::Just_2_1 => "Octave",
            Self::Just_3_2 => "P5",
            Self::Just_4_3 => "P4",
            Self::Just_5_4 => "M3",
            Self::Just_5_3 => "M6",
            Self::Just_6_5 => "m3",
            Self::Just_8_5 => "m6",
            Self::TET_m3 => "TET m3",
            Self::TET_M3 => "TET M3",
            Self::TET_P5 => "TET P5",
            Self::TET_Tritone => "Tritone",
        }
    }

    /// Get as integer ID for shader
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Just_1_1 => 0,
            Self::Just_2_1 => 1,
            Self::Just_3_2 => 2,
            Self::Just_4_3 => 3,
            Self::Just_5_4 => 4,
            Self::Just_5_3 => 5,
            Self::Just_6_5 => 6,
            Self::Just_8_5 => 7,
            Self::TET_m3 => 8,
            Self::TET_M3 => 9,
            Self::TET_P5 => 10,
            Self::TET_Tritone => 11,
        }
    }

    /// Select paint based on detected interval
    pub fn from_interval(interval: usize) -> Self {
        match interval % 12 {
            0 => Self::Just_1_1,   // Unison
            1 | 2 => Self::Just_2_1,  // Seconds → octave feel
            3 => Self::Just_6_5,  // Minor 3rd
            4 => Self::Just_5_4,  // Major 3rd
            5 => Self::Just_4_3,  // Perfect 4th
            6 => Self::TET_Tritone, // Tritone
            7 => Self::Just_3_2,  // Perfect 5th
            8 => Self::Just_8_5,  // Minor 6th
            9 => Self::Just_5_3,  // Major 6th
            10 | 11 => Self::Just_2_1, // 7ths → octave feel
            _ => Self::Just_3_2,
        }
    }

    /// Returns all variants for UI dropdown
    pub fn all_variants() -> &'static [Self] {
        &[
            Self::Just_1_1,
            Self::Just_2_1,
            Self::Just_3_2,
            Self::Just_4_3,
            Self::Just_5_4,
            Self::Just_5_3,
            Self::Just_6_5,
            Self::Just_8_5,
            Self::TET_m3,
            Self::TET_M3,
            Self::TET_P5,
            Self::TET_Tritone,
        ]
    }

    /// Discrete bead multiplier (1, 2, or 3)
    /// Paint can only scale density by these discrete amounts, never arbitrary ratios
    pub fn bead_multiplier(&self) -> i32 {
        match self {
            // Just intonation: base density (1x)
            Self::Just_1_1 => 1,  // Unison: base density
            Self::Just_3_2 => 1,  // P5: base
            Self::Just_4_3 => 1,  // P4: base
            Self::Just_5_4 => 1,  // M3: base
            Self::Just_5_3 => 1,  // M6: base
            Self::Just_6_5 => 1,  // m3: base
            Self::Just_8_5 => 1,  // m6: base
            // Octave: double density for visual emphasis
            Self::Just_2_1 => 2,
            // 12-TET paints: 2x for accent/tension
            Self::TET_m3 | Self::TET_M3 | Self::TET_P5 | Self::TET_Tritone => 2,
        }
    }
}

/// GCD helper for link count calculation
pub(crate) fn gcd(a: i32, b: i32) -> i32 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Reduce p,q by their GCD to get coprime form (overflow safety)
/// Returns (pR, qR) where both are >= 1
pub(crate) fn reduced_pq(p: i32, q: i32) -> (i32, i32) {
    let g = gcd(p.abs(), q.abs()).max(1);
    let pr = (p / g).max(1);
    let qr = (q / g).max(1);
    (pr, qr)
}

/// Safe LCM computation using i64 to prevent overflow for large p,q
fn safe_lcm(p: i32, q: i32) -> i64 {
    let (pr, qr) = reduced_pq(p, q);
    let pa = pr.abs() as i64;
    let qa = qr.abs() as i64;
    let g = gcd(pr.abs(), qr.abs()).max(1) as i64;
    (pa * qa) / g
}

/// Spice boost factor lookup - gated by spice_level for cinematic reveals
///
/// When spice_gate is off (level 0), exotic intervals blend in with common ones.
/// As spice_level increases, 7-limit and 11-limit intervals get progressive boosts,
/// creating "reveal" moments where exotic harmonies suddenly pop.
fn spice_boost(max_pq: i32, spice_level: u8) -> i64 {
    match max_pq {
        7 => match spice_level {
            0 => 1,   // No boost - 7-limit blends in
            1 => 2,   // Warming - slight lift
            _ => 3,   // Full reveal - cinematic
        },
        11 => match spice_level {
            0 => 1,   // No boost
            1 => 2,   // Warming
            _ => 4,   // Full reveal - dramatic
        },
        _ => 1,  // No boost for other primes (2/3/5-limit, large primes)
    }
}

/// Get the maximum prime factor of a number (for spice eligibility)
/// Returns the largest prime that divides n. Used to determine "limit" of an interval.
/// e.g. 15 = 3×5 → returns 5 (5-limit), 7 → returns 7 (7-limit)
fn max_prime_factor(n: i32) -> i32 {
    if n <= 1 { return 1; }
    let mut n = n.abs();
    let mut max_prime = 1;

    // Check for 2
    while n % 2 == 0 {
        max_prime = 2;
        n /= 2;
    }

    // Check odd factors from 3
    let mut i = 3;
    while i * i <= n {
        while n % i == 0 {
            max_prime = i;
            n /= i;
        }
        i += 2;
    }

    // If n is still > 1, it's a prime factor
    if n > 1 {
        max_prime = n;
    }

    max_prime
}

/// Get the "limit" of a ratio (p, q) - the maximum prime factor across both
/// e.g. T(15:8) → max(max_prime(15), max_prime(8)) = max(5, 2) = 5 (5-limit)
/// e.g. T(7:4) → max(7, 2) = 7 (7-limit)
fn ratio_limit(p: i32, q: i32) -> i32 {
    let (pr, qr) = reduced_pq(p, q);
    max_prime_factor(pr).max(max_prime_factor(qr))
}

/// Compute chroma entropy (0..1) - low entropy = stable pitch content
/// Used for spice gate activation (exotic intervals reveal on stable harmony)
fn chroma_entropy(chroma: &[f32; 12]) -> f32 {
    let sum: f32 = chroma.iter().sum();
    if sum < 0.001 { return 1.0; }  // Silent = max entropy (unstable)

    let mut entropy = 0.0f32;
    for &c in chroma {
        let p = c / sum;
        if p > 0.001 {
            entropy -= p * p.ln();
        }
    }
    // Normalize to 0..1 (max entropy = ln(12) ≈ 2.48)
    (entropy / 2.48).clamp(0.0, 1.0)
}

/// Compute chroma coherence metrics (p_max, p2, entropy)
/// Multiple metrics with OR logic because entropy alone is too strict for real music.
/// - p_max: max probability in any chroma bin (one pitch dominates)
/// - p2: sum of squared probabilities (concentration measure)
/// - entropy: normalized entropy (0 = focused, 1 = spread)
fn chroma_coherence(chroma: &[f32; 12]) -> (f32, f32, f32) {
    let sum: f32 = chroma.iter().sum();
    if sum < 0.001 { return (0.0, 0.0, 1.0); }  // Silent = incoherent

    let mut p_max = 0.0f32;
    let mut p2 = 0.0f32;
    let mut entropy = 0.0f32;

    for &c in chroma {
        let p = c / sum;
        p_max = p_max.max(p);
        p2 += p * p;
        if p > 0.001 {
            entropy -= p * p.ln();
        }
    }

    let h_norm = (entropy / 2.48).clamp(0.0, 1.0);
    (p_max, p2, h_norm)
}

/// Complexity buckets - tuned for musical distribution with gated spice
///
/// Uses LCM as base complexity, with spice_level gating 7/11-limit boosts.
/// This creates "reveal" moments where exotic intervals suddenly pop when
/// harmonic conditions are right (high confidence + stable chroma).
///
/// Distribution (MED tier, spice_level=0):
/// - Level 0: octave (2:1), fifth (3:2) → 6 beads
/// - Level 1: fourth (4:3), thirds (5:4) → 8 beads
/// - Level 2: minor third (6:5), 7-limit base → 12 beads
/// - Level 3: whole step (9:8), golden (8:13) → 18 beads
/// - Level 4: semitone (16:15) → 26 beads
/// - Level 5: 11-limit spiced, extreme → 36 beads
///
/// With spice_level=2: 7:5 jumps bucket 2→4, 11:8 jumps bucket 3→5
fn complexity_bucket(p: i32, q: i32, spice_level: u8) -> u8 {
    let c = safe_lcm(p, q);
    let (pr, qr) = reduced_pq(p, q);
    let max_pq = pr.max(qr);

    // Spice boost is now gated by spice_level (cinematic reveals)
    let boost = spice_boost(max_pq, spice_level);
    let effective_c = c * boost;

    // Revised thresholds for iconic feel:
    // - Consonances stay low (buckets 0-1)
    // - Semitone at bucket 4 (not 5) to avoid constant maxing
    // - 7/11-limit get clear lift with spice (jump 2+ buckets)
    match effective_c {
        0..=8     => 0,   // Octave (2), Fifth (6) - rock solid
        9..=24    => 1,   // Fourth (12), Major third (20) - common consonance
        25..=55   => 2,   // Minor third (30), 7-limit base (28, 35) - everyday
        56..=110  => 3,   // Whole step (72), 7-limit spiced, 11-limit base (88) - active
        111..=220 => 4,   // Semitone (240→fits!), 7-limit full (105), Golden (104) - rich
        221..=500 => 5,   // 11-limit full (352), extreme contexts - rare exotic
        _         => 6,   // Extreme coprime (17:19=323+)
    }
}

// === DENSITY LEVEL CONSTANTS ===
const DENSITY_HOLD_TIME: f32 = 0.5;  // 500ms stability required before level change
const DENSITY_MAX_LOW: u8 = 3;
const DENSITY_MAX_MED: u8 = 5;
const DENSITY_MAX_HIGH: u8 = 6;

/// Bead counts by density level - LOW tier (conservative, 4 levels)
/// Increments: +2, +2, +4
const BEADS_LOW: [i32; 4] = [4, 6, 8, 12];

/// Bead counts by density level - MED tier (balanced, 6 levels)
/// Increments: +2, +4, +6, +8, +10 (accelerating for richer high-complexity feel)
const BEADS_MED: [i32; 6] = [6, 8, 12, 18, 26, 36];

/// Bead counts by density level - HIGH tier (ambitious, 7 levels - perf doesn't matter)
/// Increments: +4, +6, +10, +14, +20, +26 (exponential feel for premium visuals)
const BEADS_HIGH: [i32; 7] = [8, 12, 18, 28, 42, 62, 88];

// === PHRASE ENVELOPE CONSTANTS ===
const PHRASE_STEP_UP: f32 = 0.12;       // Envelope increment on strong event
const PHRASE_STEP_DOWN: f32 = 0.06;     // Envelope decrement on weak tick
const PHRASE_MIN_HOLD_BEATS: f32 = 0.8; // Min beats before bias can change (lowered)
const PHRASE_DECAY_RATE: f32 = 0.02;    // Per-beat decay toward neutral

// === SPICE GATING CONSTANTS ===
const SPICE_GATE_ON_TIME: f32 = 1.5;        // Seconds of stable coherence to activate
const SPICE_GATE_OFF_TIME: f32 = 0.5;       // Seconds of instability before gate closes
const SPICE_LEVEL_HOLD: f32 = 0.4;          // Seconds between level steps
const SPICE_COHERENCE_LATCH_TIME: f32 = 0.3; // 300ms hold before coherence state changes

// Confidence thresholds - MUCH lower than topology-commit (which uses ~0.65+)
// This is spice-specific: we want stable_time to charge on "pretty good" harmony
const SPICE_HC_OPEN: f32 = 0.45;            // Confidence opens at 45% stability
const SPICE_HC_CLOSE: f32 = 0.35;           // Confidence closes at 35% stability
const SPICE_CONFIDENT_LATCH_TIME: f32 = 0.3; // 300ms hold before confidence state changes

// Coherence thresholds - RELAXED to be reachable in real music
// OPEN: ANY metric passing = coherent (OR logic)
// Based on observed values: pMax ~0.09-0.13, p2 ~0.08-0.09
const SPICE_P_MAX_OPEN: f32 = 0.14;    // One bin has 14%+ of mass (was 0.22)
const SPICE_P2_OPEN: f32 = 0.10;       // Distribution concentrated (was 0.13)
const SPICE_ENTROPY_OPEN: f32 = 0.98;  // Entropy only vetoes extreme noise (was 0.70)

// CLOSE: ALL metrics must fail (AND logic for closing)
const SPICE_P_MAX_CLOSE: f32 = 0.11;   // (was 0.18)
const SPICE_P2_CLOSE: f32 = 0.08;      // (was 0.11)
const SPICE_ENTROPY_CLOSE: f32 = 0.995; // Only close on pure noise (was 0.80)

// === ORNAMENT CONSTANTS ===
const ORNAMENT_MIN_HOLD_SECS: f32 = 0.6;    // Min seconds before ornament can change
const ORNAMENT_INTENSITY_HOLD: f32 = 0.2;   // Seconds before intensity can change

// === WI ACCENT CONSTANTS ===
const WI_ACCENT_DURATION: f32 = 0.2;    // 200ms accent flash
const WI_ACCENT_COOLDOWN: f32 = 0.3;    // Min time between accents

/// Get max density level for a quality tier
fn max_density_level(quality_tier: i32) -> u8 {
    match quality_tier {
        0 => DENSITY_MAX_LOW,
        1 => DENSITY_MAX_MED,
        _ => DENSITY_MAX_HIGH,
    }
}

/// Get bead count for a density level and quality tier
fn beads_for_level(level: u8, quality_tier: i32) -> i32 {
    match quality_tier {
        0 => BEADS_LOW.get(level as usize).copied().unwrap_or(*BEADS_LOW.last().unwrap()),
        1 => BEADS_MED.get(level as usize).copied().unwrap_or(*BEADS_MED.last().unwrap()),
        _ => BEADS_HIGH.get(level as usize).copied().unwrap_or(*BEADS_HIGH.last().unwrap()),
    }
}

/// Torus Knot Cymatics state - tertiary inside transition (24-fold → Knot)
#[derive(Clone, Debug)]
pub struct TorusKnotState {
    /// Is torus knot mode active or transitioning
    pub active: bool,
    /// Master transition progress [0..1]
    pub t: f32,
    /// Current subphase
    pub subphase: TorusKnotSubphase,
    /// Seed phase captured from 24-fold (prevents orientation jump)
    pub seed_phase: f32,
    /// Current (p,q) parameters (integers for logic)
    pub p: i32,
    pub q: i32,
    /// Target (p,q) - may differ during transition
    pub target_p: i32,
    pub target_q: i32,
    /// Smooth interpolated p,q for rendering (floats for easing)
    pub p_smooth: f32,
    pub q_smooth: f32,
    /// Reference frequency for phase advance (Hz)
    pub ref_hz: f32,
    /// Live detected frequency (Hz)
    pub live_hz: f32,
    /// Current phase advance (marching nodes)
    pub phase_advance: f32,
    /// Stability value (for blending ratio vs golden carrier)
    pub stability: f32,
    /// Golden carrier blend (0 = ratio knot, 1 = golden carrier)
    pub golden_blend: f32,
    /// Quality tier (auto-scaled)
    pub quality: TorusKnotQuality,
    /// Cooldown timer
    pub cooldown: f32,
    /// Video fade amount (for shader)
    pub video_fade: f32,

    // === TWO-STAGE LATCH STATE ===

    // Stage A: 24-fold latch with hysteresis
    /// Is 24-fold currently latched (entered and held)
    pub fold24_latched: bool,
    /// Time spent above enter threshold (before latching)
    pub fold24_enter_hold_s: f32,
    /// Time spent below exit threshold (before unlatching)
    pub fold24_exit_hold_s: f32,
    /// Time spent in latched state (for charge_min_hold check)
    pub fold24_hold_secs: f32,

    // Stage B: Knot charge
    /// Charge integrator [0..1] - triggers at 1.0
    pub charge: f32,
    /// Previous dominant pitch class (for flip detection)
    pub prev_dom_pc: i32,
    /// Debounce timer for flip detection
    pub flip_debounce_secs: f32,
    /// Rolling flip score [0..1] - decays over time, boosted by flips
    pub flip_score: f32,

    // === PERCEPTUAL SUSTAIN STATE ===

    /// Support score [0..1] - perceptual sustain accumulator
    /// Driven by slow envelopes of energy/stability/harmonic conf
    pub support: f32,
    /// Minimum-on timer (seconds remaining before exit allowed)
    pub min_on_timer: f32,
    /// Exit grace timer (seconds since support fell below stay threshold)
    pub exit_grace_timer: f32,
    /// Hard drop timer (seconds since fold24 dropped below hard_drop_t)
    pub hard_drop_timer: f32,
    /// Reentry lockout timer (seconds remaining before re-trigger allowed)
    pub reentry_lockout: f32,
    /// Total time active (for logging/debug)
    pub active_time: f32,

    // Quality auto-scaling
    quality_downgrade_time: f32,
    quality_upgrade_time: f32,

    // === EMERGENCE STATE MACHINE (prevents frame spikes) ===
    // Phases: Idle → Candidate → Prewarm → Active
    // Never allocate or dual-render on the emergence frame

    /// Current emergence phase
    pub emerge_state: EmergenceState,
    /// Time in current emergence state
    pub emerge_state_time: f32,
    /// Prewarm progress [0..1] - tracks prep work completion
    pub prewarm_progress: f32,
    /// Frames spent in prewarm (for GPU settle)
    pub prewarm_frames: u32,
    /// Frame time EMA for admission control
    pub admission_frame_ms: f32,
    /// Perf pressure slow EMA (for admission gate)
    pub perf_pressure_slow: f32,

    // === CANVAS/PAINT SYSTEM (quantized cymatic topology) ===
    // Canvas = knot topology T(p,q), Paint = node pattern

    /// Current canvas preset (knot topology)
    pub canvas: TorusCanvas,
    /// Canvas hold timer (must hold MIN_HOLD before switching)
    pub canvas_hold_timer: f32,
    /// Canvas transition state (idle vs transitioning)
    pub canvas_transitioning: bool,

    /// Current paint preset (node pattern)
    pub paint: TorusPaint,
    /// Paint hold timer (minimum hold before switching)
    pub paint_hold_timer: f32,
    /// Paint cooldown timer (after switch, prevent rapid changes)
    pub paint_cooldown: f32,
    /// Quantized phase step index [0..23] for 24-fold anchored phase
    pub paint_phase_step: i32,

    /// Link count from canvas (1 = knot, >1 = multi-loop link)
    pub link_count: i32,
    /// Bead count (sampling density) computed from density level + paint multiplier
    pub bead_count: i32,
    /// Current discrete density level index (0..MAX_LEVEL)
    pub density_level: u8,
    /// Target density level before hysteresis
    pub density_level_target: u8,
    /// Stability timer for density level changes (must hold DENSITY_HOLD_TIME before stepping)
    pub density_hold_timer: f32,
    /// Snapshot of (p,q) used for last density target computation (prevents flickering)
    pub density_pq_snapshot: (i32, i32),
    /// Previous quality tier (for detecting tier changes and resetting hold timer)
    pub density_prev_tier: i32,

    // === PHRASE ENVELOPE (density breathing with musical phrasing) ===
    /// Phrase envelope value [0.0..1.0] - drives density bias
    pub phrase_env: f32,
    /// Accumulated beats at current phrase level (for hold timing)
    pub phrase_hold_beats: f32,
    /// Last beat phase for detecting beat crossings
    pub phrase_last_beat_phase: f32,
    /// Derived phrase bias {-1, 0, +1} for density modulation
    pub phrase_bias: i8,

    // === SPICE GATING (7/11-limit boost as cinematic reveals) ===
    /// Is spice gate currently active (harmonic conditions met)
    pub spice_gate_active: bool,
    /// Current spice level [0, 1, 2] - discrete steps
    pub spice_level: u8,
    /// Target spice level (level steps toward this)
    pub spice_level_target: u8,
    /// Is current topology eligible for spice (7-limit or 11-limit)
    pub spice_eligible: bool,
    /// Prime limit of current reduced ratio (for debug)
    pub spice_prime_limit: i32,
    /// Last (pR,qR) used for eligibility computation (snapshot-based, avoids every-frame recompute)
    pub spice_eligibility_snapshot: (i32, i32),
    /// Accumulated stable+coherent time (for gate activation)
    pub spice_stable_time: f32,
    /// Timer for gate-close condition (must hold before closing)
    pub spice_close_timer: f32,
    /// Hold timer between spice level changes
    pub spice_hold_timer: f32,
    /// Last computed chroma entropy (for debug display)
    pub spice_last_entropy: f32,
    /// Last computed p_max (max bin probability) for debug
    pub spice_last_p_max: f32,
    /// Last computed p2 (sum of squared probs) for debug
    pub spice_last_p2: f32,
    /// Latched coherence state (prevents flicker)
    pub spice_coherence_latched: bool,
    /// Timer for coherence latch hysteresis
    pub spice_coherence_latch_timer: f32,
    /// Latched confidence state for spice (separate from topology-commit confidence)
    pub spice_confident_latched: bool,
    /// Timer for confidence latch hysteresis
    pub spice_confident_latch_timer: f32,

    // === ORNAMENT LAYER (premium accents locked to symmetry) ===
    /// Current ornament type [0=none, 1=filigree, 2=notches, 3=halo]
    pub ornament_id: u8,
    /// Ornament intensity [0.0, 0.5, 1.0] - quantized
    pub ornament_intensity: f32,
    /// Hold timer before ornament can change
    pub ornament_hold_timer: f32,
    /// Sub-pattern within ornament type [0..2]
    pub ornament_pattern: u8,
    /// Spoke mask for filigree (which spokes light)
    pub ornament_spoke_mask: u32,
    /// Timer for intensity change hysteresis
    pub ornament_intensity_timer: f32,

    // === WI SECTOR ACCENT (winding interval as sector highlights) ===
    /// Quantized sector index from WI [0..N-1]
    pub wi_accent_step: u8,
    /// Countdown timer for accent duration (negative = cooldown)
    pub wi_accent_timer: f32,
    /// Is WI accent currently showing
    pub wi_accent_active: bool,

    // === PREMIUM 3D QUANTIZED STATE ===
    // Quantized camera orbit locked to 24-fold
    /// Camera orbit step index [0..23] for 24-fold locked orbit
    pub camera_orbit_step: i32,
    /// Hold timer preventing rapid orbit stepping
    pub camera_orbit_hold_timer: f32,

    // Quantized tube geometry
    /// Tube radius discrete level [0..3]
    pub tube_radius_step: u8,
    /// Hold timer for tube radius changes
    pub tube_radius_hold_timer: f32,

    // Quantized vortex volume
    /// Latched vortex strength [0.0, 0.33, 0.66, 1.0]
    pub vortex_strength_latched: f32,
    /// Hold timer for vortex changes
    pub vortex_hold_timer: f32,
    /// Vortex N-fold symmetry (locked to p value)
    pub vortex_fold_n: i32,

    // Quantized lighting latches
    /// Rim light gain [0.0, 0.5, 1.0]
    pub rim_gain_latched: f32,
    /// Depth separation [0.0, 0.5, 1.0]
    pub depth_sep_latched: f32,
    /// Specular boost [0.0, 0.5, 1.0]
    pub spec_boost_latched: f32,

    // Stability gates
    /// When true, state updates are frozen (audio unstable/silent)
    pub state_frozen: bool,

    // Beat tracking for quantized updates
    /// Previous beat phase for crossing detection
    pub prev_beat_phase: f32,

    // === DEBUG OVERLAY ===
    /// Enable debug overlay for tuning
    pub debug_overlay_enabled: bool,

    /// Configuration
    pub config: TorusKnotConfig,
}

impl Default for TorusKnotState {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            subphase: TorusKnotSubphase::Idle,
            seed_phase: 0.0,
            p: 3,
            q: 2,
            target_p: 3,
            target_q: 2,
            p_smooth: 3.0,
            q_smooth: 2.0,
            ref_hz: 440.0,
            live_hz: 440.0,
            phase_advance: 0.0,
            stability: 0.0,
            golden_blend: 0.0,
            quality: TorusKnotQuality::Med,
            cooldown: 0.0,
            video_fade: 1.0,

            // Two-stage latch state
            fold24_latched: false,
            fold24_enter_hold_s: 0.0,
            fold24_exit_hold_s: 0.0,
            fold24_hold_secs: 0.0,
            charge: 0.0,
            prev_dom_pc: -1,
            flip_debounce_secs: 0.0,
            flip_score: 0.0,

            // Perceptual sustain state
            support: 0.0,
            min_on_timer: 0.0,
            exit_grace_timer: 0.0,
            hard_drop_timer: 0.0,
            reentry_lockout: 0.0,
            active_time: 0.0,

            quality_downgrade_time: 0.0,
            quality_upgrade_time: 0.0,

            // Emergence state machine
            emerge_state: EmergenceState::Idle,
            emerge_state_time: 0.0,
            prewarm_progress: 0.0,
            prewarm_frames: 0,
            admission_frame_ms: 0.0,
            perf_pressure_slow: 0.0,

            // Canvas/paint system
            canvas: TorusCanvas::default(),
            canvas_hold_timer: 0.0,
            canvas_transitioning: false,
            paint: TorusPaint::default(),
            paint_hold_timer: 0.0,
            paint_cooldown: 0.0,
            paint_phase_step: 0,
            link_count: 1,
            bead_count: 8,
            density_level: 0,
            density_level_target: 0,
            density_hold_timer: 0.0,
            density_pq_snapshot: (3, 2),  // Default T(3:2)
            density_prev_tier: 1,  // Default Med tier

            // Phrase envelope (density breathing)
            phrase_env: 0.5,  // Start neutral
            phrase_hold_beats: 0.0,
            phrase_last_beat_phase: 0.0,
            phrase_bias: 0,

            // Spice gating (cinematic reveals)
            spice_gate_active: false,
            spice_level: 0,
            spice_level_target: 0,
            spice_eligible: false,
            spice_prime_limit: 3,  // T(3:2) default
            spice_eligibility_snapshot: (0, 0),  // Force first-frame compute
            spice_stable_time: 0.0,
            spice_close_timer: 0.0,
            spice_hold_timer: 0.0,
            spice_last_entropy: 1.0,
            spice_last_p_max: 0.0,
            spice_last_p2: 0.0,
            spice_coherence_latched: false,
            spice_coherence_latch_timer: 0.0,
            spice_confident_latched: false,
            spice_confident_latch_timer: 0.0,

            // Ornament layer
            ornament_id: 0,
            ornament_intensity: 0.0,
            ornament_hold_timer: 0.0,
            ornament_pattern: 0,
            ornament_spoke_mask: 0xFFFFFF,  // All 24 spokes initially
            ornament_intensity_timer: 0.0,

            // WI sector accent
            wi_accent_step: 0,
            wi_accent_timer: -WI_ACCENT_COOLDOWN,  // Start in cooldown
            wi_accent_active: false,

            // Premium 3D quantized state
            camera_orbit_step: 0,
            camera_orbit_hold_timer: 0.0,
            tube_radius_step: 1,  // Start at "normal" (index 1)
            tube_radius_hold_timer: 0.0,
            vortex_strength_latched: 0.0,
            vortex_hold_timer: 0.0,
            vortex_fold_n: 3,  // Default to p=3
            rim_gain_latched: 0.5,
            depth_sep_latched: 0.5,
            spec_boost_latched: 0.0,
            state_frozen: false,
            prev_beat_phase: 0.0,

            // Debug
            debug_overlay_enabled: false,

            config: TorusKnotConfig::default(),
        }
    }
}

impl TorusKnotState {
    /// Get interval from two pitch classes (0-11)
    pub fn interval_from_pitches(pc1: usize, pc2: usize) -> usize {
        ((pc2 as i32 - pc1 as i32).rem_euclid(12)) as usize
    }

    /// Update (p,q) from detected interval with stability blending
    /// Three-tier system:
    /// - Low stability (<0.3): golden carrier (ambiguous, flowing)
    /// - Medium stability (0.3-0.75): normal ratios (3/5-limit, consonant)
    /// - High stability (>0.75): exotic ratios (7/11-limit, spicy!)
    pub fn update_ratio(&mut self, dominant_pc: usize, secondary_pc: usize, stability: f32) {
        let interval = Self::interval_from_pitches(dominant_pc, secondary_pc);

        // Blend toward golden carrier when stability is low
        let stability_factor = stability.clamp(0.0, 1.0);
        self.golden_blend = (1.0 - stability_factor).powf(2.0); // Quadratic falloff

        // Three-tier ratio selection:
        // - Very unstable (golden_blend > 0.7, stability < ~0.45): golden carrier
        // - Stable but not exceptional: normal consonant ratios
        // - Exceptionally stable (stability > 0.88): EXOTIC 7/11-limit ratios for spice!
        const EXOTIC_THRESHOLD: f32 = 0.88;  // Raised to make exotic ratios rarer

        if self.golden_blend > 0.7 {
            // Use golden carrier for unstable intervals
            let golden = IntervalRatio::golden_carrier_simple();
            self.target_p = golden.p;
            self.target_q = golden.q;
        } else if stability_factor > EXOTIC_THRESHOLD {
            // HIGH STABILITY: Use exotic 7/11-limit ratios
            // This triggers spice eligibility!
            let exotic = IntervalRatio::from_interval_exotic(interval);
            self.target_p = exotic.p;
            self.target_q = exotic.q;
        } else {
            // Normal stability: use standard consonant ratios
            let ratio = IntervalRatio::from_interval(interval);
            self.target_p = ratio.p;
            self.target_q = ratio.q;
        }

        // Smoothly transition p,q (only when not actively transitioning)
        if self.subphase == TorusKnotSubphase::Active {
            self.p = self.target_p;
            self.q = self.target_q;
        }
    }

    /// Debug string for HUD - shows charge gate state and perceptual sustain
    pub fn debug_string(&self, fold24_t: f32) -> String {
        if self.active || self.t > 0.01 {
            // Show active state with support score
            format!(
                "TK: {} t={:.2} sup={:.0}% p:q={}:{} min={:.1}s",
                self.subphase.name(),
                self.t,
                self.support * 100.0,
                self.p,
                self.q,
                self.min_on_timer
            )
        } else if self.reentry_lockout > 0.0 {
            format!("TK: lockout {:.1}s", self.reentry_lockout)
        } else if self.cooldown > 0.0 {
            format!("TK: cooldown {:.1}s", self.cooldown)
        } else {
            // Show entry progress (two-stage latch + charge)
            let latch_indicator = if self.fold24_latched { "L" } else { "-" };
            format!(
                "TK: f24={:.2}[{}] hold={:.1}s chg={:.0}%",
                fold24_t,
                latch_indicator,
                self.fold24_hold_secs,
                self.charge * 100.0
            )
        }
    }
}

// ============================================================================
// VORTEX STATE - Spiral Flow Topology
// ============================================================================
//
// Vortex is a sibling to TorusKnotState, representing spiral convergent/divergent
// flow through the 24-fold geometry. It inherits gate_center from the current
// topology when activated, ensuring smooth visual continuity.
//
// Key Features:
// - Gate tracking: 24-fold quantized, ±1 circulation per beat
// - Direction: Inward (convergent) or Outward (divergent), rare flips
// - Pressure envelope: Attack/decay drive intensity
// - Shell sets: Radii bands from stability-based selection
// ============================================================================

/// Vortex configuration constants
#[derive(Clone, Debug)]
pub struct VortexConfig {
    /// Transition time from inactive to active (seconds)
    pub enter_duration: f32,
    /// Transition time from active to inactive (seconds)
    pub exit_duration: f32,
    /// Minimum time before exit allowed (seconds)
    pub min_on_time: f32,
    /// Cooldown after exiting (seconds)
    pub cooldown_duration: f32,
    /// Pressure envelope attack rate (units/sec)
    pub pressure_attack: f32,
    /// Pressure envelope decay rate (units/sec)
    pub pressure_decay: f32,
    /// Direction flip cooldown (seconds)
    pub direction_cooldown: f32,
    /// Circulation step probability threshold (0-1)
    pub circulation_threshold: f32,
}

impl Default for VortexConfig {
    fn default() -> Self {
        Self {
            enter_duration: 2.5,
            exit_duration: 2.0,
            min_on_time: 5.0,
            cooldown_duration: 3.0,
            pressure_attack: 2.0,
            pressure_decay: 1.5,
            direction_cooldown: 4.0,
            circulation_threshold: 0.4,
        }
    }
}

/// Vortex state - spiral flow topology as sibling to TorusKnotState
#[derive(Clone, Debug)]
pub struct VortexState {
    /// Is vortex topology active or transitioning
    pub active: bool,
    /// Master transition progress [0..1]
    pub t: f32,
    /// Seed phase captured from 24-fold (prevents orientation jump)
    pub seed_phase: f32,

    // === GATE TRACKING (24-fold quantized) ===
    /// Gate center index (0..23) - inherited on entry, circulates during active
    pub gate_center: u8,
    /// Circulation step per beat: -1, 0, or +1
    pub circulation_step: i8,
    /// Inherited gate from previous topology (preserved on entry)
    pub inherited_gate: u8,

    // === DIRECTION ===
    /// Flow direction: Inward (convergent) or Outward (divergent)
    pub direction: VortexDirection,
    /// Cooldown timer for direction flips (seconds)
    pub direction_cooldown: f32,

    // === SHELL/PITCH ===
    /// Current shell set ID (stability-based band selection)
    pub shell_set_id: u8,
    /// Pitch quantization (0..23) from dominant pitch class
    pub pitch_q: u8,

    // === PRESSURE ENVELOPE ===
    /// Current pressure envelope value [0..1]
    pub pressure_env: f32,
    /// Target pressure (drives envelope toward this)
    pub pressure_target: f32,

    // === TIMERS ===
    /// Minimum-on timer (seconds remaining before exit allowed)
    pub min_on_timer: f32,
    /// Cooldown timer (seconds remaining before re-entry allowed)
    pub cooldown: f32,
    /// Total time active (for logging)
    pub active_time: f32,

    // === GRANT TRIANGLE (REG alignment) ===
    /// Active grant triangle ID (0..7) - latched, changes only at commits
    pub grant_triangle_id: u8,
    /// Characteristic angle quantized (0..23)
    pub grant_characteristic_angle_q: u8,
    /// Shell radii from grant triangle computation (up to 9 shells)
    pub grant_shell_radii: [f32; 9],
    /// Number of active shells
    pub grant_shell_count: u8,

    // === GRANT TRIANGLE SELECTION (hardened) ===
    /// Time held at current triangle (seconds) - must exceed MIN_HOLD_SEC for change
    pub triangle_hold_timer: f32,
    /// Last requested triangle ID (for debug/logging)
    pub triangle_last_request_id: u8,
    /// Low-passed confidence for selection gating (smoothed stability)
    pub triangle_confidence_lp: f32,

    // === 3D VORTEX / DRAIN PARAMETERS ===
    /// Torus depth illusion [0..1] — how "thick" the torus reads
    pub vortex_depth: f32,
    /// Sink/drain strength [0..1] — how hard the drain pulls the wheel inward
    pub vortex_sink_strength: f32,
    /// Target sink strength (for smooth interpolation)
    pub vortex_sink_strength_target: f32,
    /// Quantized sink radius preset index (0..4, maps to preset radii)
    pub vortex_sink_radius_q: u8,
    /// Quantized bulge phase offset (0..23) — shapes torus bulge alignment
    pub vortex_bulge_phase_q: u8,
    /// Backface occlusion amount [0..1] — how much back dims
    pub vortex_occlusion: f32,
    /// Minimum bars before sink preset can change
    pub sink_hold_bars: u32,
    /// Last bar index when sink changed (for hold tracking)
    pub sink_last_change_bar: u64,
    /// Onset-driven sink envelope [0..1] — "gulp" pulses
    pub sink_env: f32,

    // === QS WHEEL DRAIN SPRING ANIMATION ===
    /// QS wheel drain pull position [0..1] — 0=normal, 1=fully pulled into vortex
    pub qs_drain_pull: f32,
    /// QS wheel drain velocity — for spring physics / ripple oscillation
    pub qs_drain_vel: f32,
    /// Target pull position (1.0 when active, 0.0 when releasing)
    pub qs_drain_target: f32,

    // === BEAT TRACKING ===
    /// Previous beat phase for crossing detection
    pub prev_beat_phase: f32,

    /// Configuration
    pub config: VortexConfig,
}

impl Default for VortexState {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            seed_phase: 0.0,

            gate_center: 0,
            circulation_step: 0,
            inherited_gate: 0,

            direction: VortexDirection::default(),
            direction_cooldown: 0.0,

            shell_set_id: 0,
            pitch_q: 0,

            pressure_env: 0.0,
            pressure_target: 0.0,

            min_on_timer: 0.0,
            cooldown: 0.0,
            active_time: 0.0,

            grant_triangle_id: 0,
            grant_characteristic_angle_q: 0,
            grant_shell_radii: [0.0; 9],
            grant_shell_count: 9,

            // Grant triangle selection (hardened)
            triangle_hold_timer: 0.0,
            triangle_last_request_id: 0,
            triangle_confidence_lp: 0.0,

            // 3D vortex/drain defaults
            vortex_depth: 0.0,
            vortex_sink_strength: 0.0,
            vortex_sink_strength_target: 0.0,
            vortex_sink_radius_q: 2, // Middle preset
            vortex_bulge_phase_q: 0,
            vortex_occlusion: 0.5,   // Moderate backface dimming
            sink_hold_bars: 4,       // Min 4 bars before change
            sink_last_change_bar: 0,
            sink_env: 0.0,

            // QS drain spring animation
            qs_drain_pull: 0.0,
            qs_drain_vel: 0.0,
            qs_drain_target: 0.0,

            prev_beat_phase: 0.0,

            config: VortexConfig::default(),
        }
    }
}

impl VortexState {
    /// Debug string for HUD display
    pub fn debug_string(&self) -> String {
        if self.active || self.t > 0.01 {
            let dir_str = match self.direction {
                VortexDirection::Inward => "In",
                VortexDirection::Outward => "Out",
            };
            format!(
                "Vortex: t={:.2} gate={} dir={} press={:.0}%",
                self.t,
                self.gate_center,
                dir_str,
                self.pressure_env * 100.0
            )
        } else if self.cooldown > 0.0 {
            format!("Vortex: cooldown {:.1}s", self.cooldown)
        } else {
            "Vortex: idle".to_string()
        }
    }
}

// ============================================================================
// REG-LOCKED CAMERA STATE - Quantized Camera Control
// ============================================================================
// Camera state that rides the same angular grid as the geometry.
// All targets are discrete: azimuth anchors, elevation tiers, radius steps.
// Smoothing is applied AFTER target selection, preserving REG alignment.
// ============================================================================

// === REG CAMERA CONSTANTS ===

/// Radius steps: [far, mid-far, mid, near, core] - discrete distance tiers
const CAM_RADIUS_STEPS: [f32; 5] = [3.2, 2.6, 2.1, 1.6, 1.2];

/// Elevation levels: [-18°, -9°, 0°, +9°, +18°] in radians
const CAM_ELEV_LEVELS: [f32; 5] = [-0.314, -0.157, 0.0, 0.157, 0.314];

/// FOV steps: [narrow, normal, wide]
const CAM_FOV_STEPS: [f32; 3] = [48.0, 55.0, 68.0];

/// Portal radius sequence: R2 → R1 → R0 → R0 → R1 (in, hold, ease-out)
const PORTAL_RADIUS_SEQ: [u8; 5] = [2, 1, 0, 0, 1];
const PORTAL_PHASE_THRESHOLDS: [f32; 5] = [0.0, 0.22, 0.48, 0.72, 0.90];

// === TIMING CONSTANTS ===
/// Min time before allowing camera commit
const CAM_T_HOLD: f32 = 2.5;
/// Cooldown after camera commit
const CAM_T_COOLDOWN: f32 = 1.25;
/// Total portal dive duration
const CAM_T_DIVE: f32 = 5.0;  // Extended from 1.2s for DJ visibility
/// Min hold after portal exit
const CAM_T_EXIT_HOLD: f32 = 0.6;
/// QC port hold time
const CAM_T_PORT_HOLD: f32 = 1.2;
/// QC port cooldown
const CAM_T_PORT_COOLDOWN: f32 = 0.8;
/// Beats per phrase for elevation changes
const CAM_BEATS_PER_PHRASE: u32 = 8;

// === PORTAL ELIGIBILITY CONSTANTS (two-key mechanism with hysteresis) ===
/// Sustained readiness time to arm portal entry
const PORTAL_T_ARM: f32 = 0.5;  // Reduced from 1.0 for faster portal entry
/// Minimum interval between portal entries (cinematic rarity)
const PORTAL_T_MIN_INTERVAL: f32 = 5.0;  // Reduced for more fluid 2D/3D transitions
/// Block timer after portal exit (prevents immediate re-entry)
const PORTAL_T_BLOCK: f32 = 2.0;
/// Time below exit threshold before disarming (hysteresis)
const PORTAL_T_EXIT_HOLD: f32 = 0.3;
/// LP filter tau for harmonic confidence
const PORTAL_LP_TAU_HARM: f32 = 0.8;
/// LP filter tau for gate stability
const PORTAL_LP_TAU_GATE: f32 = 0.6;
/// Harmonic confidence threshold to ENTER ready state (hysteresis high)
const PORTAL_HARM_ENTER: f32 = 0.25;  // Lowered for DJ use (was 0.40)
/// Harmonic confidence threshold to EXIT ready state (hysteresis low)
const PORTAL_HARM_EXIT: f32 = 0.15;   // Lowered (was 0.30)
/// Gate stability threshold for portal eligibility
const PORTAL_GATE_OK: f32 = 0.20;     // Lowered for DJ use (was 0.40)

// === THRESHOLD CONSTANTS ===
/// Harmonic confidence threshold for camera commits
const CAM_HARM_OK: f32 = 0.70;
/// Beat confidence threshold for camera commits
const CAM_BEAT_OK: f32 = 0.40;
/// Gate stability threshold for camera commits
const CAM_GATE_OK: f32 = 0.60;
/// QC confidence threshold for port-driven anchors
const CAM_QC_OK: f32 = 0.70;
/// Energy silence threshold - below this, camera freezes
const CAM_SILENCE: f32 = 0.08;

/// REG-locked camera state: quantized layer on top of smooth systems
/// All motion is constrained to discrete, symmetry-valid poses
#[derive(Clone, Debug)]
pub struct CameraRegState {
    // === RIG SELECTION ===
    /// Current camera rig mode
    pub rig: CameraRig,
    /// Transition phase (Idle or Transitioning)
    pub phase: CameraRegPhase,

    // === DISCRETE TARGETS (quantized) ===
    /// Azimuth anchor index: 0..N-1 where N = symmetry_mode (12 or 24)
    pub anchor_k: u8,
    /// Elevation tier: -2..+2 maps to CAM_ELEV_LEVELS
    pub elev_level: i8,
    /// Radius step: 0..4 maps to CAM_RADIUS_STEPS
    pub radius_step: u8,
    /// FOV step: 0..2 maps to CAM_FOV_STEPS
    pub fov_step: u8,

    // === DERIVED TARGET POSE (computed from discrete state) ===
    /// Target azimuth in radians = anchor_k * (TAU / N)
    pub target_azimuth: f32,
    /// Target elevation in radians from CAM_ELEV_LEVELS
    pub target_elevation: f32,
    /// Target radius from CAM_RADIUS_STEPS
    pub target_radius: f32,
    /// Target FOV from CAM_FOV_STEPS
    pub target_fov: f32,

    // === TIMERS ===
    /// Time since last commit (seconds)
    pub hold_timer: f32,
    /// Cooldown remaining after commit (seconds)
    pub cooldown_timer: f32,

    // === PORTAL DIVE STATE (24-fold only) ===
    /// Portal dive progress [0..1], time-based
    pub portal_phase: f32,
    /// Locked azimuth anchor for dive (doesn't change during dive)
    pub portal_anchor_k_locked: u8,
    /// Current position in portal radius sequence
    pub portal_radius_seq_idx: u8,

    // === PORTAL ELIGIBILITY (two-key mechanism with hysteresis) ===
    /// Low-passed harmonic confidence for portal readiness
    pub portal_ready_lp: f32,
    /// Low-passed gate stability for portal readiness
    pub gate_stability_lp: f32,
    /// Time eligibility conditions have been continuously met (with decay)
    pub portal_ready_timer: f32,
    /// True when timer matures; consumed on downbeat entry
    pub portal_enter_armed: bool,
    /// Cooldown blocking new portal attempts (seconds)
    pub portal_block_timer: f32,
    /// Time below exit threshold (for hysteresis disarm)
    pub portal_ready_exit_timer: f32,
    /// Total runtime when last portal was entered (for minimum interval)
    pub last_portal_time: f32,

    // === BEAT TRACKING ===
    /// Previous beat phase for crossing detection
    pub prev_beat_phase: f32,
    /// True for one frame when downbeat crosses
    pub beat_cross_detected: bool,

    // === PHRASE TRACKING ===
    /// Beats since last phrase boundary
    pub beat_count: u32,
    /// True for one frame at phrase boundary
    pub phrase_edge: bool,
}

impl Default for CameraRegState {
    fn default() -> Self {
        Self {
            rig: CameraRig::Orbit,
            phase: CameraRegPhase::Idle,
            anchor_k: 0,
            elev_level: 0,
            radius_step: 2,             // Start at mid-distance
            fov_step: 1,                // Normal FOV
            target_azimuth: 0.0,
            target_elevation: 0.0,
            target_radius: CAM_RADIUS_STEPS[2],
            target_fov: CAM_FOV_STEPS[1],
            hold_timer: 0.0,
            cooldown_timer: 0.0,
            portal_phase: 0.0,
            portal_anchor_k_locked: 0,
            portal_radius_seq_idx: 0,
            portal_ready_lp: 0.0,
            gate_stability_lp: 0.0,
            portal_ready_timer: 0.0,
            portal_enter_armed: false,
            portal_block_timer: 0.0,
            portal_ready_exit_timer: 0.0,
            last_portal_time: -20.0,  // Allow immediate first portal
            prev_beat_phase: 0.0,
            beat_cross_detected: false,
            beat_count: 0,
            phrase_edge: false,
        }
    }
}

impl CameraRegState {
    /// Debug string for HUD display
    pub fn debug_string(&self, symmetry_mode: u8) -> String {
        let rig_name = self.rig.name();
        if self.rig == CameraRig::PortalDive {
            format!(
                "Cam: {} k={}/{} phase={:.0}%",
                rig_name,
                self.portal_anchor_k_locked,
                symmetry_mode,
                self.portal_phase * 100.0
            )
        } else {
            format!(
                "Cam: {} k={}/{} R{} E{:+} F{}",
                rig_name,
                self.anchor_k,
                symmetry_mode,
                self.radius_step,
                self.elev_level,
                self.fov_step
            )
        }
    }
}

// ============================================================================
// QC PORT STATE - Quaternion Charge to Camera Wedge Mapping
// ============================================================================
// Deterministic mapping from QC state to a 24-fold wedge anchor for camera.
// Uses quadrant/polarity to select from 4 port angles, then quantizes to
// nearest 24-fold anchor with hysteresis and ±1 step limiting.
// ============================================================================

/// QC-to-camera port mapping state
/// Maps quaternion charge ports to discrete camera wedge anchors
#[derive(Clone, Debug)]
pub struct QcPortState {
    // === PORT SELECTION ===
    /// Active port index: 0=W, 1=π-W, 2=π+W, 3=τ-W
    pub port_id_active: u8,
    /// Target wedge index (0..23) computed from active port
    pub port_k_target: u8,
    /// Latched wedge index (0..23) - hysteresis-approved, ±1 step limited
    pub port_k_latched: u8,
    /// Low-passed confidence for QC eligibility
    pub port_conf_lp: f32,

    // === TIMERS ===
    /// Time since last latch update
    pub hold_timer: f32,
    /// Cooldown remaining after latch update
    pub cooldown_timer: f32,

    // === CLOSENESS SCORES (for stability) ===
    /// How close each port is to its quantized anchor [0..1]
    pub closeness: [f32; 4],
}

impl Default for QcPortState {
    fn default() -> Self {
        Self {
            port_id_active: 0,
            port_k_target: 0,
            port_k_latched: 0,
            port_conf_lp: 0.0,
            hold_timer: 0.0,
            cooldown_timer: 0.0,
            closeness: [0.0; 4],
        }
    }
}

impl QcPortState {
    /// Debug string for HUD display
    pub fn debug_string(&self) -> String {
        let port_names = ["W", "π-W", "π+W", "τ-W"];
        format!(
            "QcPort: {} k={} (tgt={}) conf={:.0}%",
            port_names[self.port_id_active as usize],
            self.port_k_latched,
            self.port_k_target,
            self.port_conf_lp * 100.0
        )
    }
}

/// Compute shortest wrapped delta in modular arithmetic
/// Returns value in range [-(modulus/2), modulus/2]
fn shortest_wrapped_delta(target: i16, current: i16, modulus: i16) -> i16 {
    let raw = target - current;
    let wrapped = ((raw % modulus) + modulus) % modulus;
    if wrapped > modulus / 2 {
        wrapped - modulus
    } else {
        wrapped
    }
}

// ============================================================================
// REG VALIDATION STATE - Event Logging & Invariant Checking
// ============================================================================
// Low-overhead validation for REG camera/QC/portal invariants.
// Logs events only (not per-frame), validates discrete state contracts.
// ============================================================================

/// REG validation counters and state tracking
#[derive(Clone, Debug)]
pub struct RegValidationState {
    // === COUNTERS ===
    pub cam_commits: u32,
    pub qc_commits: u32,
    pub portal_enters: u32,
    pub portal_exits: u32,
    pub portal_steps: u32,
    pub freeze_enters: u32,
    pub freeze_exits: u32,
    pub rig_changes: u32,

    // === VIOLATION COUNTERS ===
    pub inv1_discrete_range: u32,
    pub inv2_step_limit: u32,
    pub inv3_beat_gating: u32,
    pub inv4_portal_lock: u32,
    pub inv5_freeze_rule: u32,
    pub inv6_unit_contract: u32,

    // === BEAT TRACKING ===
    /// Monotonic beat ID for tracking double-commits
    pub beat_id: u64,
    /// Last beat ID that had a CAM_COMMIT
    pub last_cam_commit_beat_id: u64,
    /// Last beat ID that had a QC_COMMIT
    pub last_qc_commit_beat_id: u64,

    // === PREVIOUS STATE (for delta checks) ===
    pub prev_anchor_k: u8,
    pub prev_radius_step: u8,
    pub prev_elev_level: i8,
    pub prev_qc_k_latched: u8,
    pub prev_rig: CameraRig,
    pub prev_portal_radius_seq_idx: u8,
    pub prev_portal_anchor_k_locked: u8,

    // === FREEZE STATE ===
    pub was_frozen: bool,
    pub frozen_anchor_k: u8,  // anchor_k when freeze started

    // === PORTAL STATE ===
    pub was_in_portal: bool,
    pub portal_entry_anchor_k: u8,

    // === PORTAL FPS COUNTERS ===
    pub fps_lane_switches: u32,
    pub fps_segment_advances: u32,
    pub fps_near_miss_spawns: u32,
    pub fps_max_active_bars: u8,

    // === TIMING ===
    pub start_time: std::time::Instant,
    pub total_runtime_s: f32,
    pub last_summary_s: f32,
    pub last_state_dump_s: f32,

    // === RECENT EVENTS BUFFER (for HUD) ===
    pub recent_events: [RegEvent; 8],
    pub recent_event_idx: usize,
}

/// Event type for debug display
#[derive(Clone, Copy, Debug, Default)]
pub struct RegEvent {
    pub timestamp_s: f32,
    pub event_type: RegEventType,
    pub detail: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegEventType {
    #[default]
    None,
    CamCommit,
    QcCommit,
    PortalEnter,
    PortalExit,
    LaneSwitch,
    SegmentAdvance,
    NearMissSpawn,
    FreezeEnter,
    FreezeExit,
}

impl Default for RegValidationState {
    fn default() -> Self {
        Self {
            cam_commits: 0,
            qc_commits: 0,
            portal_enters: 0,
            portal_exits: 0,
            portal_steps: 0,
            freeze_enters: 0,
            freeze_exits: 0,
            rig_changes: 0,
            inv1_discrete_range: 0,
            inv2_step_limit: 0,
            inv3_beat_gating: 0,
            inv4_portal_lock: 0,
            inv5_freeze_rule: 0,
            inv6_unit_contract: 0,
            beat_id: 0,
            last_cam_commit_beat_id: 0,
            last_qc_commit_beat_id: 0,
            prev_anchor_k: 0,
            prev_radius_step: 2,  // Start at mid-distance (same as CameraRegState)
            prev_elev_level: 0,
            prev_qc_k_latched: 0,
            prev_rig: CameraRig::Orbit,
            prev_portal_radius_seq_idx: 0,
            prev_portal_anchor_k_locked: 0,
            was_frozen: false,
            frozen_anchor_k: 0,
            was_in_portal: false,
            portal_entry_anchor_k: 0,
            fps_lane_switches: 0,
            fps_segment_advances: 0,
            fps_near_miss_spawns: 0,
            fps_max_active_bars: 0,
            start_time: std::time::Instant::now(),
            total_runtime_s: 0.0,
            last_summary_s: 0.0,
            last_state_dump_s: 0.0,
            recent_events: [RegEvent::default(); 8],
            recent_event_idx: 0,
        }
    }
}

impl RegValidationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get elapsed time since start
    pub fn elapsed_s(&self) -> f32 {
        self.start_time.elapsed().as_secs_f32()
    }

    /// Get total violation count
    pub fn total_violations(&self) -> u32 {
        self.inv1_discrete_range
            + self.inv2_step_limit
            + self.inv3_beat_gating
            + self.inv4_portal_lock
            + self.inv5_freeze_rule
            + self.inv6_unit_contract
    }

    /// Add event to recent events buffer
    pub fn add_event(&mut self, event_type: RegEventType, detail: i32) {
        let event = RegEvent {
            timestamp_s: self.elapsed_s(),
            event_type,
            detail,
        };
        self.recent_events[self.recent_event_idx] = event;
        self.recent_event_idx = (self.recent_event_idx + 1) % self.recent_events.len();
    }

    /// Print end-of-run summary with coverage checks
    pub fn print_summary(&self) {
        let elapsed = self.elapsed_s();
        log::info!("========================================");
        log::info!("REG VALIDATION SUMMARY ({:.1}s runtime)", elapsed);
        log::info!("========================================");
        log::info!("REG Camera Events:");
        log::info!("  CAM_COMMIT:    {}", self.cam_commits);
        log::info!("  QC_COMMIT:     {}", self.qc_commits);
        log::info!("  PORTAL_ENTER:  {}", self.portal_enters);
        log::info!("  PORTAL_EXIT:   {}", self.portal_exits);
        log::info!("  PORTAL_STEP:   {}", self.portal_steps);
        log::info!("  FREEZE_ENTER:  {}", self.freeze_enters);
        log::info!("  FREEZE_EXIT:   {}", self.freeze_exits);
        log::info!("  RIG_CHANGE:    {}", self.rig_changes);
        log::info!("----------------------------------------");
        log::info!("Portal FPS Events:");
        log::info!("  LANE_SWITCH:      {}", self.fps_lane_switches);
        log::info!("  SEGMENT_ADVANCE:  {}", self.fps_segment_advances);
        log::info!("  NEAR_MISS_SPAWN:  {}", self.fps_near_miss_spawns);
        log::info!("  MAX_ACTIVE_BARS:  {}", self.fps_max_active_bars);
        log::info!("----------------------------------------");
        log::info!("Invariant Violations:");
        log::info!("  INV-1 Discrete range: {}", self.inv1_discrete_range);
        log::info!("  INV-2 Step limit:     {}", self.inv2_step_limit);
        log::info!("  INV-3 Beat gating:    {}", self.inv3_beat_gating);
        log::info!("  INV-4 Portal lock:    {}", self.inv4_portal_lock);
        log::info!("  INV-5 Freeze rule:    {}", self.inv5_freeze_rule);
        log::info!("  INV-6 Unit contract:  {}", self.inv6_unit_contract);
        log::info!("----------------------------------------");
        let total = self.total_violations();
        if total == 0 {
            log::info!("✓ ALL INVARIANTS HELD - REG OK");
        } else {
            log::warn!("✗ {} TOTAL VIOLATIONS - REG VIOLATED", total);
        }
        log::info!("========================================");

        // === COVERAGE CHECKS ===
        log::info!("COVERAGE CHECK (3-minute targets):");
        let scale = (180.0 / elapsed.max(1.0)).min(1.0);  // Scale for shorter runs

        let cam_target = (50.0 * scale) as u32;
        let qc_target = (10.0 * scale) as u32;
        let portal_target = 1u32;
        let lane_target = 3u32;
        let seg_target = 8u32;
        let nm_target = 3u32;

        let cam_ok = self.cam_commits >= cam_target;
        let qc_ok = self.qc_commits >= qc_target || elapsed < 60.0;  // QC needs fold24 time
        let portal_ok = self.portal_enters >= portal_target && self.portal_exits >= portal_target;
        let fps_ok = portal_ok && self.fps_lane_switches >= lane_target
            && self.fps_segment_advances >= seg_target
            && self.fps_near_miss_spawns >= nm_target;

        log::info!("  CAM_COMMIT >= {}: {} ({})", cam_target, if cam_ok { "PASS" } else { "FAIL" }, self.cam_commits);
        log::info!("  QC_COMMIT >= {}: {} ({})", qc_target, if qc_ok { "PASS" } else { "FAIL" }, self.qc_commits);
        log::info!("  PORTAL_ENTER/EXIT >= 1: {} ({}/{})",
            if portal_ok { "PASS" } else { "FAIL" }, self.portal_enters, self.portal_exits);

        if portal_ok {
            // Updated targets: LANE >= 2, NM >= 2 (robust triggers should hit these)
            let lane_target_robust = 2u32;
            let nm_target_robust = 2u32;

            let lane_ok = self.fps_lane_switches >= lane_target_robust;
            let seg_ok = self.fps_segment_advances >= seg_target;
            let nm_ok = self.fps_near_miss_spawns >= nm_target_robust;

            log::info!("  FPS LANE_SWITCH >= {}: {} ({})", lane_target_robust,
                if lane_ok { "PASS" } else { "FAIL" }, self.fps_lane_switches);
            log::info!("  FPS SEGMENT_ADVANCE >= {}: {} ({})", seg_target,
                if seg_ok { "PASS" } else { "FAIL" }, self.fps_segment_advances);
            log::info!("  FPS NEAR_MISS_SPAWN >= {}: {} ({})", nm_target_robust,
                if nm_ok { "PASS" } else { "FAIL" }, self.fps_near_miss_spawns);

            // Diagnostic hints if triggers didn't fire
            if !lane_ok && self.fps_lane_switches == 0 {
                log::warn!("  HINT: Lane switches need chroma peak to hold same intent (bins 0-3/8-11) for 2 beats");
            }
            if !nm_ok && self.fps_near_miss_spawns == 0 {
                log::warn!("  HINT: Near-miss needs threat segment + (onset OR downbeat fallback)");
                log::warn!("  HINT: Check if threat segments (Threat/Needle) are being reached");
            }
        } else {
            log::warn!("  FPS coverage skipped - portal never triggered");
            if self.portal_enters == 0 {
                log::warn!("  HINT: Check portal eligibility (harmonic_conf, gate_stability thresholds)");
            }
        }

        log::info!("========================================");
        if total == 0 && cam_ok && qc_ok && portal_ok && fps_ok {
            log::info!("✓ REG OK + FULL COVERAGE");
        } else if total == 0 {
            log::warn!("⚠ REG OK but NOT FULLY EXERCISED");
        } else {
            log::error!("✗ REG VIOLATED - {} violations", total);
        }
        log::info!("========================================");
    }

    /// Check if periodic summary should be printed (every 60s during validation)
    /// Returns true if summary was printed
    pub fn maybe_print_periodic_summary(&mut self) -> bool {
        let elapsed = self.elapsed_s();
        const SUMMARY_INTERVAL_S: f32 = 60.0;

        if elapsed - self.last_summary_s >= SUMMARY_INTERVAL_S {
            self.last_summary_s = elapsed;
            log::info!("--- PERIODIC REG VALIDATION UPDATE ---");
            self.print_summary();
            true
        } else {
            false
        }
    }

    /// Focused state dump every 5s for REG math validation.
    /// Validates: gate quantization, polarity-degree semantics, fold24 gating,
    /// topology hold, and cross-mode indexing.
    #[allow(clippy::too_many_arguments)]
    pub fn maybe_dump_state(
        &mut self,
        gate_mode: u8,           // 12 or 24
        current_gate: u8,        // gate_system.current_gate()
        gate_ok: f32,            // gate confidence [0..1]
        stability: f32,          // stability_smooth
        fold24_permitted: bool,
        fold24_engaged: bool,
        fold24_t: f32,
        fold24_engage_t: f32,
        reg_degree_a: i32,       // REG harmonic integer (1-360) — INFO, not symmetry angle
        polarity_quadrant: u8,   // 0-3
        qc_active: bool,         // whether quaternion charge compute is active
        lattice_ok_24: bool,     // quaternion ports on 15° grid (only valid when qc_active)
        topology_name: &str,
        section_energy: f32,
        dominant_pc: usize,
        harmonic_state_idx: u8,  // gate → harmonic state (0-11)
    ) {
        let elapsed = self.elapsed_s();
        const DUMP_INTERVAL_S: f32 = 5.0;

        if elapsed - self.last_state_dump_s < DUMP_INTERVAL_S {
            return;
        }
        self.last_state_dump_s = elapsed;

        // === GATE QUANTIZATION CHECK ===
        let gate_max = gate_mode;
        let gate_in_range = current_gate < gate_max;
        let gate_degrees = current_gate as f32 * (360.0 / gate_mode as f32);
        let quantized_to_grid = (gate_degrees % (360.0 / gate_mode as f32)).abs() < 0.01;

        // === CROSS-MODE INDEX CHECK ===
        // harmonic_state_idx must always be 0-11 regardless of gate_mode
        let harmonic_idx_ok = harmonic_state_idx < 12;
        // In 24-mode, harmonic = gate/2; verify mapping
        let cross_mode_ok = if gate_mode == 24 {
            harmonic_state_idx == current_gate / 2
        } else {
            harmonic_state_idx == current_gate % 12
        };

        // === REG DEGREE A CHECK ===
        // reg_degree_a is the REG harmonic integer (1-360), NOT a symmetry angle.
        // It should NOT be expected to be a multiple of 30° or 15°.
        let deg_is_on_30_grid = reg_degree_a > 0 && (reg_degree_a % 30 == 0);
        let deg_is_on_15_grid = reg_degree_a > 0 && (reg_degree_a % 15 == 0);

        // === LATTICE TRI-STATE ===
        // When QC is inactive, lattice_ok_24 is N/A (not a violation)
        let lattice_label = if !qc_active {
            "N/A(qc_off)"
        } else if lattice_ok_24 {
            "true"
        } else {
            "FALSE(!!)"
        };

        // === FOLD24 GATING CHECK ===
        // fold24 should not be engaged when stability is very low
        let fold24_gating_ok = !fold24_engaged || stability > 0.10;

        // === LOG ===
        let gate_label = if gate_ok > 0.5 { "OK" } else { "AMB" };
        let fold_label = if fold24_engaged {
            "ENGAGED"
        } else if fold24_permitted {
            "PERMITTED"
        } else {
            "OFF"
        };

        log::info!("--- REG STATE DUMP ({:.0}s) ---", elapsed);
        log::info!(
            "  Gate: mode={} k={}/{} ({:.0}°) state={} stab={:.2} | harmonic_idx={}/12 {}",
            gate_mode, current_gate, gate_max, gate_degrees, gate_label, stability,
            harmonic_state_idx,
            if harmonic_idx_ok && cross_mode_ok { "OK" } else { "CROSS-MODE BUG" }
        );
        log::info!(
            "  Fold24: {} t={:.2} engage_t={:.2} | gating={}",
            fold_label, fold24_t, fold24_engage_t,
            if fold24_gating_ok { "OK" } else { "VIOLATION(engaged@low_stab)" }
        );
        log::info!(
            "  REG A={} quad={} (INFO, not symmetry angle) | on_30°={} on_15°={}",
            reg_degree_a, polarity_quadrant, deg_is_on_30_grid, deg_is_on_15_grid
        );
        log::info!(
            "  Topology: {} | energy={:.2} | dom_pc={} | lattice_24={}",
            topology_name, section_energy, dominant_pc, lattice_label
        );

        // === INVARIANT CHECKS ===
        if !gate_in_range {
            log::error!("  !! GATE OUT OF RANGE: k={} but mode={}", current_gate, gate_mode);
        }
        if !quantized_to_grid {
            log::error!("  !! GATE NOT ON GRID: {:.1}° not multiple of {:.1}°", gate_degrees, 360.0 / gate_mode as f32);
        }
        if !harmonic_idx_ok || !cross_mode_ok {
            log::error!("  !! CROSS-MODE: harmonic_idx={} gate={} mode={}", harmonic_state_idx, current_gate, gate_mode);
        }
        if !fold24_gating_ok {
            log::error!("  !! FOLD24 ENGAGED AT LOW STABILITY: stab={:.3}", stability);
        }
        log::info!("---");
    }
}

// ============================================================================
// DODECAHEDRAL RESONANCE - Crystalline Harmonic Arrival
// ============================================================================
//
// The dodecahedron represents "harmonic arrival" - when music settles into
// coherent, stable harmony. It emerges from Vortex topology when:
//   - Chroma entropy is low (single key dominance)
//   - Stability is high (coherent pitch relationships)
//   - Mid-range energy is sustained (groove locked in)
//
// Geometry mapping (Grant Projection Theorem alignment):
//   - 12 pentagonal faces × 2 orientations = 24-fold symmetry
//   - Face depths driven by Grant shell radii (9 means)
//   - 20 vertices breathe with dominant pitch class
//   - 30 edges pulse on beat transients
//   - Golden ratio (Phi) embedded in pentagonal geometry
//
// ============================================================================

/// Dodecahedral resonance state - crystalline arrival topology
#[derive(Clone, Debug)]
pub struct DodecahedralState {
    /// Is dodecahedral topology active or transitioning
    pub active: bool,
    /// Master transition progress [0..1]
    pub t: f32,

    // === FACE TRACKING (12 pentagonal faces × 2 = 24-fold) ===
    /// Face activation levels [0..1] for each of 12 faces
    pub face_activation: [f32; 12],
    /// Face depth offsets (driven by Grant shell radii)
    pub face_depths: [f32; 12],
    /// Dominant face index (0..11) - brightest face
    pub dominant_face: u8,

    // === VERTEX BREATHING (20 vertices) ===
    /// Vertex breath phase (cycles with dominant pitch class)
    pub vertex_breath_phase: f32,
    /// Vertex breath amplitude [0..1]
    pub vertex_breath_amp: f32,

    // === EDGE PULSES (30 edges) ===
    /// Edge pulse envelope [0..1] - triggers on beat transients
    pub edge_pulse_env: f32,
    /// Edge glow intensity [0..1]
    pub edge_glow: f32,

    // === ROTATION & DRIFT ===
    /// Slow rotation angle (radians) - geological drift
    pub rotation_angle: f32,
    /// Rotation velocity (radians/sec) - very slow
    pub rotation_velocity: f32,
    /// Face lock strength [0..1] - how strongly faces align to 24-fold gates
    pub face_lock_strength: f32,

    // === GOLDEN RATIO RESONANCE ===
    /// Phi resonance pulse [0..1] - golden ratio harmonic emphasis
    pub phi_resonance: f32,
    /// Phi pulse target (driven by harmonic coherence)
    pub phi_target: f32,

    // === INHERITED STATE ===
    /// Gate inherited from previous topology (24-fold alignment)
    pub inherited_gate: u8,
    /// Grant triangle inherited (shell radii continuity)
    pub inherited_grant_id: u8,
    /// Grant shell radii (inherited and updated)
    pub grant_shell_radii: [f32; 9],

    // === TIMERS ===
    /// Minimum-on timer (seconds remaining before exit allowed)
    pub min_on_timer: f32,
    /// Cooldown timer (seconds remaining before re-entry allowed)
    pub cooldown: f32,
    /// Total time active
    pub active_time: f32,

    // === ENTRY CONDITIONS (latched) ===
    /// Chroma entropy at entry (latched for stability)
    pub entry_entropy: f32,
    /// Stability at entry (latched threshold)
    pub entry_stability: f32,

    // === BEAT TRACKING ===
    /// Previous beat phase for crossing detection
    pub prev_beat_phase: f32,
}

/// Dodecahedral resonance constants
pub mod dodeca_const {
    /// Minimum time before exit allowed (seconds)
    pub const MIN_ON_TIME: f32 = 4.0;
    /// Cooldown after exit before re-entry (seconds)
    pub const COOLDOWN_TIME: f32 = 3.0;
    /// Transition duration (seconds)
    pub const TRANSITION_TIME: f32 = 2.5;
    /// Chroma entropy threshold for entry (lower = more coherent)
    /// Real music is typically 0.85-0.98, so 0.95 gives margin for fluctuation
    pub const ENTRY_ENTROPY_MAX: f32 = 0.95;
    /// Stability threshold for entry (lowered to 0.35 for real-world music)
    pub const ENTRY_STABILITY_MIN: f32 = 0.35;
    /// Minimum mid-range energy for entry
    pub const ENTRY_MID_ENERGY_MIN: f32 = 0.3;
    /// Bars required at stable conditions for entry
    pub const ENTRY_STABLE_BARS: u8 = 2;
    /// Exit entropy threshold (hysteresis - must be HIGHER than entry to stay in)
    pub const EXIT_ENTROPY_MAX: f32 = 0.98;
    /// Exit stability threshold (hysteresis - must be LOWER than entry to stay in)
    pub const EXIT_STABILITY_MIN: f32 = 0.20;
    /// Rotation velocity (radians/sec) - very slow
    pub const ROTATION_VEL: f32 = 0.03;
    /// Face lock lerp rate
    pub const FACE_LOCK_RATE: f32 = 2.0;
    /// Vertex breath frequency (Hz relative to beat)
    pub const VERTEX_BREATH_FREQ: f32 = 0.5;
    /// Edge pulse attack rate
    pub const EDGE_PULSE_ATTACK: f32 = 15.0;
    /// Edge pulse decay rate
    pub const EDGE_PULSE_DECAY: f32 = 4.0;
    /// Phi resonance lerp rate
    pub const PHI_LERP_RATE: f32 = 1.5;
    /// Golden ratio constant
    pub const PHI: f32 = 1.618033988749895;
}

impl Default for DodecahedralState {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,

            face_activation: [0.0; 12],
            face_depths: [0.0; 12],
            dominant_face: 0,

            vertex_breath_phase: 0.0,
            vertex_breath_amp: 0.0,

            edge_pulse_env: 0.0,
            edge_glow: 0.0,

            rotation_angle: 0.0,
            rotation_velocity: dodeca_const::ROTATION_VEL,
            face_lock_strength: 0.0,

            phi_resonance: 0.0,
            phi_target: 0.0,

            inherited_gate: 0,
            inherited_grant_id: 0,
            grant_shell_radii: [0.0; 9],

            min_on_timer: 0.0,
            cooldown: 0.0,
            active_time: 0.0,

            entry_entropy: 1.0,
            entry_stability: 0.0,

            prev_beat_phase: 0.0,
        }
    }
}

impl DodecahedralState {
    /// Debug string for HUD display
    pub fn debug_string(&self) -> String {
        if self.active || self.t > 0.01 {
            format!(
                "Dodeca: t={:.2} face={} phi={:.0}%",
                self.t,
                self.dominant_face,
                self.phi_resonance * 100.0
            )
        } else if self.cooldown > 0.0 {
            format!("Dodeca: cooldown {:.1}s", self.cooldown)
        } else {
            "Dodeca: idle".to_string()
        }
    }
}

// ============================================================================
// HYPERSPACE JUMP DRIVE - Cinematic Transition Effect
// ============================================================================
//
// Three-phase hyperspace transition centered on QS wheel center circle:
//   Charge (0.0-0.2): Energy rings converge, center glows
//   Jump (0.2-0.6): Radial streaks, tunnel effect, blue-shift
//   Emerge (0.6-1.0): Expansion ring, reveal new state
//
// Triggered automatically by fold24 transitions, or manually for hero's journey.
// ============================================================================

/// Warp Jump QS Phase - QS wheel IS the portal
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpPhase {
    /// QS wheel locks to center, enlarges, becomes focus (0.00 - 0.25)
    Acquire,
    /// Portal shrinks to tiny disk, hyperspace tracers outside (0.25 - 0.60)
    Tunnel,
    /// Tiny disk expands via ripple to new scene (0.60 - 1.00)
    Burst,
    /// Not active
    Idle,
}

impl Default for WarpPhase {
    fn default() -> Self {
        Self::Idle
    }
}

impl WarpPhase {
    pub fn as_i32(&self) -> i32 {
        match self {
            WarpPhase::Idle => -1,
            WarpPhase::Acquire => 0,
            WarpPhase::Tunnel => 1,
            WarpPhase::Burst => 2,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            WarpPhase::Idle => "Idle",
            WarpPhase::Acquire => "Acquire",
            WarpPhase::Tunnel => "Tunnel",
            WarpPhase::Burst => "Burst",
        }
    }
}

/// Warp Jump QS State - The QS wheel IS the portal
///
/// Timeline:
/// 1. ACQUIRE (0.00-0.25): QS wheel locks to center, becomes hero focus
/// 2. TUNNEL (0.25-0.60): Portal shrinks, hyperspace tracers outside, flash at 0.52
/// 3. BURST (0.60-1.00): Ripple expands from tiny portal to full screen
///
/// Key insight: The QS wheel is NOT faded out - it IS the aperture.
#[derive(Clone, Debug)]
pub struct WarpJump {
    /// Is the warp transition active?
    pub active: bool,
    /// Master timeline progress [0..1]
    pub t: f32,
    /// Current phase
    pub phase: WarpPhase,
    /// Progress within current phase [0..1]
    pub phase_t: f32,

    // === PORTAL PARAMETERS (QS wheel = portal) ===

    /// Portal center in screen coords (lerps to 0.5, 0.5)
    pub portal_center: [f32; 2],
    /// Portal radius in normalized screen units
    pub portal_radius: f32,
    /// Portal spin (corkscrew effect during tunnel)
    pub portal_spin: f32,
    /// Camera zoom factor (affects FOV)
    pub zoom: f32,
    /// Flash intensity (peaks at t≈0.52)
    pub flash: f32,
    /// Ripple expansion radius [0..1]
    pub ripple: f32,
    /// Tracer intensity (hyperspace streaks outside portal)
    pub tracer_intensity: f32,

    // === GEOMETRY PARAMETERS ===

    /// Camera Z offset (push forward during tunnel)
    pub camera_z_offset: f32,
    /// Geometry warp strength (bend toward portal)
    pub warp_strength: f32,

    // === QS WHEEL ENHANCEMENTS ===

    /// QS wheel contrast boost during acquire
    pub qs_contrast: f32,
    /// QS wheel ring thickness multiplier
    pub qs_thickness: f32,
    /// QS wheel seam glow intensity
    pub qs_seam_glow: f32,
    /// QS wheel scale (shrink/pop/jiggle animation)
    pub qs_scale: f32,

    // === DISTORTION EFFECTS ===

    /// Chromatic aberration intensity (RGB split)
    pub chromatic_aberration: f32,
    /// Vortex UV distortion strength (spiral pull toward center)
    pub vortex_strength: f32,

    // Phase timing boundaries
    acquire_end: f32,  // 0.25
    tunnel_end: f32,   // 0.60

    /// Manual trigger override - prevents sync_to_fold24 from overriding
    manual_override: bool,
}

impl Default for WarpJump {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            phase: WarpPhase::Idle,
            phase_t: 0.0,

            portal_center: [0.5, 0.5],
            portal_radius: 0.25,  // Normal QS wheel size
            portal_spin: 0.0,
            zoom: 1.0,
            flash: 0.0,
            ripple: 0.0,
            tracer_intensity: 0.0,

            camera_z_offset: 0.0,
            warp_strength: 0.0,

            qs_contrast: 1.0,
            qs_thickness: 1.0,
            qs_seam_glow: 0.0,
            qs_scale: 1.0,

            chromatic_aberration: 0.0,
            vortex_strength: 0.0,

            acquire_end: 0.25,
            tunnel_end: 0.60,

            manual_override: false,
        }
    }
}

impl WarpJump {
    // Easing functions
    fn ease_in_quad(t: f32) -> f32 { t * t }
    fn ease_in_cubic(t: f32) -> f32 { t * t * t }
    fn ease_out_quad(t: f32) -> f32 { 1.0 - (1.0 - t) * (1.0 - t) }
    fn ease_out_cubic(t: f32) -> f32 { 1.0 - (1.0 - t).powi(3) }
    fn ease_in_out_cubic(t: f32) -> f32 {
        if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
    }
    fn ease_out_exp(t: f32) -> f32 { 1.0 - (-10.0 * t).exp() }
    fn ease_in_exp(t: f32) -> f32 { (10.0 * (t - 1.0)).exp() }

    /// Damped oscillation for jiggle/settle effect
    /// Returns 0 at t=0, oscillates with decaying amplitude, settles to 0
    fn damped_oscillate(t: f32, frequency: f32, damping: f32) -> f32 {
        let decay = (-damping * t).exp();
        decay * (t * frequency * std::f32::consts::TAU).sin()
    }

    /// Sync with fold24 transition (12-fold → 24-fold)
    pub fn sync_to_fold24(&mut self, fold24_t: f32, fold24_active: bool) {
        // Don't override manual trigger
        if self.manual_override {
            return;
        }
        if fold24_active && fold24_t > 0.01 {
            self.active = true;
            self.t = fold24_t;
            self.update_phase();
        } else if !fold24_active && self.active && self.t > 0.9 {
            // Transition complete
            self.active = false;
            self.phase = WarpPhase::Idle;
            self.reset_params();
        } else if !fold24_active {
            self.active = false;
            self.phase = WarpPhase::Idle;
            self.reset_params();
        }
    }

    /// Manual trigger (for torus exit, etc.)
    pub fn trigger(&mut self) {
        self.active = true;
        self.t = 0.0;
        self.phase = WarpPhase::Acquire;
        self.phase_t = 0.0;
        self.manual_override = true;  // Prevent sync_to_fold24 from overriding
        log::info!("[WarpJump] Manual trigger activated");
    }

    /// Update warp animation (call each frame when manually triggered)
    pub fn update(&mut self, dt: f32) {
        if !self.active || !self.manual_override {
            return;
        }
        // Advance t (complete in ~1 second)
        self.t += dt * 1.0;
        if self.t >= 1.0 {
            self.t = 1.0;
            self.active = false;
            self.manual_override = false;
            self.phase = WarpPhase::Idle;
            self.reset_params();
            log::info!("[WarpJump] Manual trigger complete");
        } else {
            self.update_phase();
        }
    }

    /// Update phase and compute all parameters
    fn update_phase(&mut self) {
        // Determine current phase
        if self.t < self.acquire_end {
            self.phase = WarpPhase::Acquire;
            self.phase_t = self.t / self.acquire_end;
        } else if self.t < self.tunnel_end {
            self.phase = WarpPhase::Tunnel;
            self.phase_t = (self.t - self.acquire_end) / (self.tunnel_end - self.acquire_end);
        } else {
            self.phase = WarpPhase::Burst;
            self.phase_t = (self.t - self.tunnel_end) / (1.0 - self.tunnel_end);
        }

        self.compute_params();
    }

    /// Compute all parameters based on current phase
    fn compute_params(&mut self) {
        // Base QS wheel radius (in normalized screen coords)
        let base_radius = 0.25;

        match self.phase {
            WarpPhase::Acquire => {
                // PHASE 1: ACQUIRE/SHRINK (0.00 → 0.25)
                // QS wheel shrinks toward center with ease-out (slows as it shrinks)
                let pt = self.phase_t;

                // Portal center at screen center
                self.portal_center = [0.5, 0.5];

                // Portal radius - keep at base (the QS scale does the work now)
                self.portal_radius = base_radius;

                // No spin
                self.portal_spin = 0.0;

                // No zoom (keeps wheel circular)
                self.zoom = 1.0;

                // No flash
                self.flash = 0.0;

                // No ripple
                self.ripple = 0.0;

                // No tracers (removed ring effect)
                self.tracer_intensity = 0.0;

                // No camera offset
                self.camera_z_offset = 0.0;

                // Subtle geometry warp
                self.warp_strength = 0.15 * Self::ease_in_quad(pt);

                // === QS WHEEL SHRINK ===
                // Scale from 1.0 → 0.3 with ease_out_cubic (slows down as it shrinks)
                self.qs_scale = 1.0 - 0.7 * Self::ease_out_cubic(pt);  // 1.0 → 0.3

                // QS wheel enhancements: increase contrast as it shrinks (concentrating energy)
                self.qs_contrast = 1.0 + 0.5 * Self::ease_out_cubic(pt);  // 1.0 → 1.5
                self.qs_thickness = 1.0 + 0.2 * Self::ease_out_cubic(pt); // 1.0 → 1.2
                self.qs_seam_glow = 0.6 * Self::ease_out_cubic(pt);        // 0 → 0.6

                // Subtle chromatic (building tension)
                self.chromatic_aberration = 0.05 * Self::ease_in_quad(pt);
                self.vortex_strength = 0.1 * Self::ease_in_quad(pt);
            }

            WarpPhase::Tunnel => {
                // PHASE 2: HOLD/CHARGE (0.25 → 0.60)
                // QS wheel stays compressed, building energy for the pop
                let pt = self.phase_t;

                // Portal center stays at screen center
                self.portal_center = [0.5, 0.5];

                // Portal radius at base
                self.portal_radius = base_radius;

                // No spin
                self.portal_spin = 0.0;

                // No zoom (keeps wheel circular)
                self.zoom = 1.0;

                // Flash at end of tunnel (the "bang" moment) - at pt ≈ 0.9
                let flash_center = 0.9;
                let flash_width = 0.1;
                let flash_dist = (pt - flash_center).abs();
                self.flash = 0.8 * (-flash_dist * flash_dist / (flash_width * flash_width) * 30.0).exp();

                // No ripple
                self.ripple = 0.0;

                // No tracers
                self.tracer_intensity = 0.0;

                // No camera offset
                self.camera_z_offset = 0.0;

                // Subtle warp
                self.warp_strength = 0.15;

                // === QS WHEEL HOLD AT SMALL ===
                // Stay at 0.3 with slight pulse (breathing at compressed size)
                let pulse = 0.03 * (pt * 8.0 * std::f32::consts::PI).sin();
                self.qs_scale = 0.3 + pulse;

                // Maintain enhanced contrast (energy concentrated)
                self.qs_contrast = 1.5 + 0.2 * pt;  // 1.5 → 1.7 (building)
                self.qs_thickness = 1.2;
                self.qs_seam_glow = 0.6 + 0.4 * pt;  // 0.6 → 1.0 (glowing brighter)

                // Subtle chromatic (tension)
                self.chromatic_aberration = 0.05 + 0.1 * pt;
                self.vortex_strength = 0.1;
            }

            WarpPhase::Burst => {
                // PHASE 3: POP & JIGGLE (0.60 → 1.00)
                // QS wheel pops back with ease-in (gradual speed up) then jiggles into place
                let pt = self.phase_t;

                // Portal center stays at screen center
                self.portal_center = [0.5, 0.5];

                // Portal radius at base
                self.portal_radius = base_radius;

                // No spin
                self.portal_spin = 0.0;

                // No zoom (keeps wheel circular)
                self.zoom = 1.0;

                // Quick flash at start of pop (the second "bang")
                self.flash = 0.6 * (1.0 - pt) * (1.0 - pt);

                // No ripple (removed ring effect)
                self.ripple = 0.0;

                // No tracers
                self.tracer_intensity = 0.0;

                // No camera offset
                self.camera_z_offset = 0.0;

                // Warp fades
                self.warp_strength = 0.15 * (1.0 - pt);

                // === QS WHEEL POP & JIGGLE ===
                // Two sub-phases within burst:
                // - First 40% (pt 0-0.4): Pop from 0.3 → 1.1 with ease_in_cubic (speeds up)
                // - Last 60% (pt 0.4-1.0): Settle from 1.1 → 1.0 with damped oscillation

                if pt < 0.4 {
                    // POP phase: 0.3 → 1.1 with ease_in (gradual speed up)
                    let pop_t = pt / 0.4;  // Normalize to 0..1
                    let eased = Self::ease_in_cubic(pop_t);
                    self.qs_scale = 0.3 + 0.8 * eased;  // 0.3 → 1.1 (overshoot)
                } else {
                    // JIGGLE/SETTLE phase: 1.1 → 1.0 with damped oscillation
                    let settle_t = (pt - 0.4) / 0.6;  // Normalize to 0..1
                    // Start at 1.1, oscillate toward 1.0
                    // Overshoot amount that decays: 0.1 * damped_oscillate
                    let jiggle = Self::damped_oscillate(settle_t, 2.5, 4.0);  // 2.5 oscillations, fast decay
                    let settle_base = 1.1 - 0.1 * Self::ease_out_quad(settle_t);  // 1.1 → 1.0
                    self.qs_scale = settle_base + 0.08 * jiggle;
                }

                // QS wheel parameters return to normal
                self.qs_contrast = 1.7 - 0.7 * Self::ease_out_quad(pt);  // 1.7 → 1.0
                self.qs_thickness = 1.2 - 0.2 * Self::ease_out_quad(pt);  // 1.2 → 1.0
                self.qs_seam_glow = 1.0 - Self::ease_out_quad(pt);  // 1.0 → 0

                // Chromatic fades quickly
                self.chromatic_aberration = 0.15 * (1.0 - Self::ease_out_exp(pt));
                self.vortex_strength = 0.1 * (1.0 - Self::ease_out_exp(pt));
            }

            WarpPhase::Idle => {
                self.reset_params();
            }
        }
    }

    fn reset_params(&mut self) {
        self.portal_center = [0.5, 0.5];
        self.portal_radius = 0.25;
        self.portal_spin = 0.0;
        self.zoom = 1.0;
        self.flash = 0.0;
        self.ripple = 0.0;
        self.tracer_intensity = 0.0;
        self.camera_z_offset = 0.0;
        self.warp_strength = 0.0;
        self.qs_contrast = 1.0;
        self.qs_thickness = 1.0;
        self.qs_seam_glow = 0.0;
        self.qs_scale = 1.0;
        self.chromatic_aberration = 0.0;
        self.vortex_strength = 0.0;
    }

    /// Debug string for HUD overlay
    pub fn debug_string(&self) -> String {
        format!(
            "Warp: {} t={:.2} r={:.3} tracer={:.2} flash={:.2} ripple={:.2}",
            self.phase.name(),
            self.t,
            self.portal_radius,
            self.tracer_intensity,
            self.flash,
            self.ripple
        )
    }
}

// Keep HyperspaceState as alias for backwards compatibility
pub type HyperspaceState = WarpJump;

// ============================================================================
// QUADRIPOLAR TRAVEL - REG Icositetragon Quaternion Symmetry Journey
// ============================================================================
//
// Implements coherent 12↔24 fold travel following REG's quadripolar symmetry.
// From a seed angle A, defines 4 "ports" via quaternion reflection:
//   {A, 180-A, 180+A, 360-A} (normalized to [0,360))
//
// Journey phases:
//   guided_12  : Follow 12-fold seam (k*30°) toward first port A
//   refract    : Hop through quaternion ports in order
//   unfold_24  : Split 12 seams into 24, align to nearest 15° grid
//   resolve    : Settle to symmetry anchor, ease back to orbit
//
// Key invariant: NO INERTIA SNAPS. All transitions use damped harmonic
// oscillators that conserve position/velocity continuity.
//
// ============================================================================

/// Quadripolar travel phase within the inside journey
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuadripolarPhase {
    /// Following a 12-fold seam toward first port
    Guided12,
    /// Hopping through quaternion ports
    Refract,
    /// Splitting 12→24 seams, camera aligns to 24-fold grid
    Unfold24,
    /// Settling to symmetry anchor
    Resolve,
    /// Not active
    Inactive,
}

impl Default for QuadripolarPhase {
    fn default() -> Self {
        QuadripolarPhase::Inactive
    }
}

impl QuadripolarPhase {
    pub fn as_f32(&self) -> f32 {
        match self {
            QuadripolarPhase::Inactive => 0.0,
            QuadripolarPhase::Guided12 => 1.0,
            QuadripolarPhase::Refract => 2.0,
            QuadripolarPhase::Unfold24 => 3.0,
            QuadripolarPhase::Resolve => 4.0,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            QuadripolarPhase::Inactive => "Inactive",
            QuadripolarPhase::Guided12 => "Guided12",
            QuadripolarPhase::Refract => "Refract",
            QuadripolarPhase::Unfold24 => "Unfold24",
            QuadripolarPhase::Resolve => "Resolve",
        }
    }
}

/// Damped spring for smooth angle transitions (wrap-aware)
#[derive(Clone, Debug)]
pub struct AngleSpring {
    /// Current angle (radians, wrapped to [0, 2π))
    pub value: f32,
    /// Current angular velocity (rad/s)
    pub velocity: f32,
    /// Target angle (radians)
    pub target: f32,
    /// Natural frequency (rad/s)
    pub omega: f32,
    /// Damping ratio (1.0 = critical, <1 = underdamped)
    pub zeta: f32,
}

impl AngleSpring {
    pub fn new(initial: f32, omega: f32, zeta: f32) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
            omega,
            zeta,
        }
    }

    /// Update with wrap-aware shortest path
    pub fn update(&mut self, dt: f32) {
        // Find shortest angular distance (wrap-aware)
        let mut delta = self.target - self.value;
        // Wrap to [-π, π]
        while delta > std::f32::consts::PI { delta -= std::f32::consts::TAU; }
        while delta < -std::f32::consts::PI { delta += std::f32::consts::TAU; }

        // Critically damped spring physics
        let w = self.omega;
        let z = self.zeta.min(1.0); // Clamp to critical

        // Spring force toward target
        let accel = w * w * delta - 2.0 * z * w * self.velocity;
        self.velocity += accel * dt;
        self.value += self.velocity * dt;

        // Wrap value to [0, 2π)
        while self.value < 0.0 { self.value += std::f32::consts::TAU; }
        while self.value >= std::f32::consts::TAU { self.value -= std::f32::consts::TAU; }
    }

    /// Inject energy (velocity impulse) without resetting position
    pub fn inject_velocity(&mut self, impulse: f32) {
        self.velocity += impulse;
    }

    /// Set new target (wrap-aware)
    pub fn set_target(&mut self, target: f32) {
        self.target = target % std::f32::consts::TAU;
        if self.target < 0.0 { self.target += std::f32::consts::TAU; }
    }
}

impl Default for AngleSpring {
    fn default() -> Self {
        Self::new(0.0, 8.0, 0.85) // Slightly underdamped for organic feel
    }
}

/// Quadripolar Travel State
/// Manages the journey through REG's quaternion symmetry
#[derive(Clone, Debug)]
pub struct QuadripolarTravelState {
    /// Is travel active?
    pub active: bool,

    /// Current phase
    pub phase: QuadripolarPhase,

    /// Timeline progress within current phase [0,1]
    pub phase_t: f32,

    /// Total journey progress [0,1]
    pub total_t: f32,

    /// Seed angle A (radians) - derived from dominant pitch or chosen seam
    pub seed_angle: f32,

    /// Quaternion ports: {A, 360-A, 180+A, 180-A} (radians, normalized)
    pub ports: [f32; 4],

    /// Current port index during refract phase
    pub current_port: usize,

    /// Camera angle spring (smooth transitions)
    pub camera_angle: AngleSpring,

    /// Seam basis interpolation [0=12-fold, 1=24-fold]
    pub seam_basis: f32,

    /// Seam basis target (for smooth transitions)
    pub seam_basis_target: f32,

    /// Video density multiplier (reduced during unfold for clarity)
    pub video_density: f32,

    /// Spoke flash intensity (brief visibility during unfold)
    pub spoke_flash: f32,

    /// Wedge segmentation intensity [0=fog, 1=24 distinct wedges]
    pub wedge_intensity: f32,

    /// Energy burst for inertia injection
    pub energy_burst: f32,

    /// Time spent in current phase
    pub phase_time: f32,

    /// Cooldown after completion
    pub cooldown: f32,

    // Phase durations (seconds)
    pub guided_12_duration: f32,
    pub refract_duration: f32,
    pub unfold_24_duration: f32,
    pub resolve_duration: f32,
}

impl Default for QuadripolarTravelState {
    fn default() -> Self {
        Self {
            active: false,
            phase: QuadripolarPhase::Inactive,
            phase_t: 0.0,
            total_t: 0.0,
            seed_angle: 0.0,
            ports: [0.0; 4],
            current_port: 0,
            camera_angle: AngleSpring::default(),
            seam_basis: 0.0,
            seam_basis_target: 0.0,
            video_density: 1.0,
            spoke_flash: 0.0,
            wedge_intensity: 0.0,
            energy_burst: 0.0,
            phase_time: 0.0,
            cooldown: 0.0,
            // Phase durations
            guided_12_duration: 2.0,
            refract_duration: 3.0,
            unfold_24_duration: 2.5,
            resolve_duration: 1.5,
        }
    }
}

impl QuadripolarTravelState {
    /// Start a new quadripolar journey from a seed pitch class (0-11)
    pub fn start_from_pitch(&mut self, pitch_class: usize) {
        // Convert pitch class to 12-fold seam angle
        // k*30° = k*(π/6), where k = pitch_class
        let seed = (pitch_class as f32) * std::f32::consts::PI / 6.0;
        self.start_from_angle(seed);
    }

    /// Start from a specific seed angle (radians)
    pub fn start_from_angle(&mut self, seed: f32) {
        self.active = true;
        self.phase = QuadripolarPhase::Guided12;
        self.phase_t = 0.0;
        self.total_t = 0.0;
        self.phase_time = 0.0;

        // Normalize seed to [0, 2π)
        let mut a = seed % std::f32::consts::TAU;
        if a < 0.0 { a += std::f32::consts::TAU; }
        self.seed_angle = a;

        // === REG QUATERNION SET ALGORITHM ===
        // Per PDF: reduce A to w < 90° (w = A mod 90°), then build {w, 180-w, 180+w, 360-w}
        // In radians: w = A mod (π/2), ports = {w, π-w, π+w, 2π-w}
        let pi = std::f32::consts::PI;
        let tau = std::f32::consts::TAU;
        let half_pi = std::f32::consts::FRAC_PI_2;

        // Reduce to first quadrant: w = A mod 90°
        let w = a % half_pi;

        // Build quaternion ports following REG rule
        self.ports = [
            w,                              // Port 0: w (first quadrant)
            (pi - w) % tau,                 // Port 1: 180° - w
            (pi + w) % tau,                 // Port 2: 180° + w
            (tau - w) % tau,                // Port 3: 360° - w
        ];
        self.current_port = 0;

        // Initialize camera angle to first port (w)
        self.camera_angle.value = w;
        self.camera_angle.target = w;
        self.camera_angle.velocity = 0.0;

        // Reset visual parameters
        self.seam_basis = 0.0;
        self.seam_basis_target = 0.0;
        self.video_density = 1.0;
        self.spoke_flash = 0.0;
        self.wedge_intensity = 0.0;

        // Energy burst on start
        self.energy_burst = 1.0;
        self.camera_angle.inject_velocity(2.0); // Initial momentum

        log::info!("[QuadripolarTravel] Started from angle {:.1}° (pitch seam {})",
            a.to_degrees(), (a / (pi / 6.0)).round() as i32 % 12);
    }

    /// Update the travel state
    pub fn update(&mut self, dt: f32) {
        if !self.active {
            // Decay cooldown
            self.cooldown = (self.cooldown - dt).max(0.0);
            return;
        }

        self.phase_time += dt;

        // Update camera angle spring
        self.camera_angle.update(dt);

        // Smooth seam basis transition
        let basis_rate = 2.0; // 0.5s to transition
        self.seam_basis += (self.seam_basis_target - self.seam_basis) * (1.0 - (-basis_rate * dt).exp());

        // Decay energy burst
        self.energy_burst *= (-3.0 * dt).exp();

        // Decay spoke flash
        self.spoke_flash *= (-5.0 * dt).exp();

        // Update based on current phase
        match self.phase {
            QuadripolarPhase::Guided12 => self.update_guided_12(dt),
            QuadripolarPhase::Refract => self.update_refract(dt),
            QuadripolarPhase::Unfold24 => self.update_unfold_24(dt),
            QuadripolarPhase::Resolve => self.update_resolve(dt),
            QuadripolarPhase::Inactive => {}
        }

        // Update total progress
        let total_duration = self.guided_12_duration + self.refract_duration
            + self.unfold_24_duration + self.resolve_duration;
        self.total_t = match self.phase {
            QuadripolarPhase::Guided12 =>
                self.phase_t * self.guided_12_duration / total_duration,
            QuadripolarPhase::Refract =>
                (self.guided_12_duration + self.phase_t * self.refract_duration) / total_duration,
            QuadripolarPhase::Unfold24 =>
                (self.guided_12_duration + self.refract_duration
                    + self.phase_t * self.unfold_24_duration) / total_duration,
            QuadripolarPhase::Resolve =>
                (self.guided_12_duration + self.refract_duration + self.unfold_24_duration
                    + self.phase_t * self.resolve_duration) / total_duration,
            QuadripolarPhase::Inactive => 0.0,
        };
    }

    fn update_guided_12(&mut self, dt: f32) {
        self.phase_t = (self.phase_time / self.guided_12_duration).min(1.0);

        // Stay on 12-fold seam, camera approaches first port
        self.camera_angle.set_target(self.ports[0]);
        self.seam_basis_target = 0.0; // Stay 12-fold
        self.video_density = 1.0;
        self.wedge_intensity = 0.0;

        // Transition to refract
        if self.phase_t >= 1.0 {
            self.phase = QuadripolarPhase::Refract;
            self.phase_time = 0.0;
            self.current_port = 0;
            self.energy_burst = 0.8; // Burst on phase change
            log::trace!("[QuadripolarTravel] -> Refract phase");
        }
    }

    fn update_refract(&mut self, dt: f32) {
        self.phase_t = (self.phase_time / self.refract_duration).min(1.0);

        // Hop through ports: A → 360-A → 180+A → 180-A
        let port_duration = self.refract_duration / 4.0;
        let port_idx = ((self.phase_time / port_duration) as usize).min(3);

        if port_idx != self.current_port {
            // Port hop!
            self.current_port = port_idx;
            self.camera_angle.set_target(self.ports[port_idx]);
            self.camera_angle.inject_velocity(1.5); // Momentum on hop
            self.energy_burst = 0.6;
            self.spoke_flash = 0.8; // Brief flash on hop
            log::trace!("[QuadripolarTravel] Refract -> Port {} ({:.1}°)",
                port_idx, self.ports[port_idx].to_degrees());
        }

        // Keep 12-fold during refract
        self.seam_basis_target = 0.0;
        self.video_density = 0.9; // Slight reduction
        self.wedge_intensity = 0.2; // Hint of wedges

        // Transition to unfold_24
        if self.phase_t >= 1.0 {
            self.phase = QuadripolarPhase::Unfold24;
            self.phase_time = 0.0;
            self.energy_burst = 1.0;
            self.spoke_flash = 1.0;
            log::trace!("[QuadripolarTravel] -> Unfold24 phase");
        }
    }

    fn update_unfold_24(&mut self, dt: f32) {
        self.phase_t = (self.phase_time / self.unfold_24_duration).min(1.0);

        // Smoothly transition from 12-fold to 24-fold
        self.seam_basis_target = self.phase_t; // 0→1 over duration

        // Snap camera to nearest seam (respects current seam_basis)
        // seam_basis < 0.5 → 12-fold (30°), seam_basis >= 0.5 → 24-fold (15°)
        let nearest = self.nearest_seam(self.camera_angle.value);
        self.camera_angle.set_target(nearest);

        // Reduce video density for structure clarity
        self.video_density = 1.0 - 0.4 * self.phase_t; // 1.0 → 0.6

        // Increase wedge intensity
        self.wedge_intensity = self.phase_t; // 0 → 1

        // Spoke flash during middle of unfold
        let flash_center = 0.5;
        let flash_width = 0.3;
        let flash_t = (-(self.phase_t - flash_center).powi(2) / (2.0 * flash_width * flash_width)).exp();
        self.spoke_flash = self.spoke_flash.max(flash_t * 0.7);

        // Transition to resolve
        if self.phase_t >= 1.0 {
            self.phase = QuadripolarPhase::Resolve;
            self.phase_time = 0.0;
            self.energy_burst = 0.5;
            log::trace!("[QuadripolarTravel] -> Resolve phase");
        }
    }

    fn update_resolve(&mut self, dt: f32) {
        self.phase_t = (self.phase_time / self.resolve_duration).min(1.0);

        // Hold at 24-fold
        self.seam_basis_target = 1.0;

        // Restore video density gradually
        self.video_density = 0.6 + 0.4 * self.phase_t; // 0.6 → 1.0

        // Reduce wedge intensity
        self.wedge_intensity = 1.0 - self.phase_t; // 1 → 0

        // Lock to nearest symmetry anchor
        let anchor = self.nearest_symmetry_anchor();
        self.camera_angle.set_target(anchor);

        // Complete journey
        if self.phase_t >= 1.0 {
            self.complete();
        }
    }

    /// Find nearest seam based on current seam_basis
    /// seam_basis < 0.5 → 12-fold (30° = π/6 steps, chroma-driven)
    /// seam_basis >= 0.5 → 24-fold (15° = π/12 steps, geometry-driven)
    /// This matches the PDF: half-step = 30° on a 360° unit circle
    fn nearest_seam(&self, angle: f32) -> f32 {
        let step = if self.seam_basis < 0.5 {
            std::f32::consts::PI / 6.0  // 30° = 12-fold (chroma)
        } else {
            std::f32::consts::PI / 12.0 // 15° = 24-fold (geometry)
        };
        let k = (angle / step).round();
        k * step
    }

    /// Find nearest 12-fold seam (k * 30°) - chroma-driven
    fn nearest_12_fold_seam(&self, angle: f32) -> f32 {
        let step = std::f32::consts::PI / 6.0; // 30°
        let k = (angle / step).round();
        k * step
    }

    /// Find nearest 24-fold seam (k * 15°) - geometry-driven
    fn nearest_24_fold_seam(&self, angle: f32) -> f32 {
        let step = std::f32::consts::PI / 12.0; // 15°
        let k = (angle / step).round();
        k * step
    }

    /// Find nearest symmetry anchor (nearest seam that's also close to a port)
    /// Uses seam_basis to determine 12-fold vs 24-fold snapping
    fn nearest_symmetry_anchor(&self) -> f32 {
        // Find the port closest to current camera angle
        let mut best_port = self.ports[0];
        let mut best_dist = f32::MAX;

        for &port in &self.ports {
            let mut dist = (port - self.camera_angle.value).abs();
            // Wrap distance
            if dist > std::f32::consts::PI {
                dist = std::f32::consts::TAU - dist;
            }
            if dist < best_dist {
                best_dist = dist;
                best_port = port;
            }
        }

        // Snap port to nearest seam (respects seam_basis)
        self.nearest_seam(best_port)
    }

    /// Complete the journey
    fn complete(&mut self) {
        self.active = false;
        self.phase = QuadripolarPhase::Inactive;
        self.cooldown = 12.0; // 12 second cooldown
        self.seam_basis = 0.0; // Back to 12-fold
        self.seam_basis_target = 0.0;
        self.video_density = 1.0;
        self.wedge_intensity = 0.0;
        self.spoke_flash = 0.0;
        log::info!("[QuadripolarTravel] Journey complete, cooldown {}s", self.cooldown);
    }

    /// Check if ready to start (not active and cooldown expired)
    pub fn is_ready(&self) -> bool {
        !self.active && self.cooldown <= 0.0
    }

    /// Get camera angle (radians)
    pub fn camera_angle_rad(&self) -> f32 {
        self.camera_angle.value
    }

    /// Debug string
    pub fn debug_string(&self) -> String {
        if self.active {
            format!("Quad({} t={:.2} cam={:.0}° basis={:.2})",
                self.phase.name(),
                self.phase_t,
                self.camera_angle.value.to_degrees(),
                self.seam_basis)
        } else if self.cooldown > 0.0 {
            format!("Quad(cd={:.1}s)", self.cooldown)
        } else {
            "Quad(ready)".to_string()
        }
    }
}

// ============================================================================
// TOPOLOGY ROUTER - Bar-Edge Deterministic Routing Between 24-Fold Topologies
// ============================================================================
//
// The topology router manages transitions between discrete topology states
// based on bar-edge evaluated metrics. Routes are evaluated only at bar
// boundaries (every 4 beats) to ensure musical coherence.
//
// Routing Philosophy:
// - Hysteresis + dwell: Enter/exit thresholds differ, conditions must hold
// - Crossfade not morph: Two stable projections, alpha blend only
// - Gate preservation: gate_center inherited on entry, never reset
// - fold24 engagement: TorusKnot requires fold24.engaged; Vortex can trigger engagement
// ============================================================================

/// Number of topology routes in the routing table
const NUM_TOPOLOGY_ROUTES: usize = 12;

// ============================================================================
// GRANT PROJECTION THEOREM - Shell Radii from Pythagorean Triples
// ============================================================================
// Based on Robert Edward Grant's "Grant Projection Theorem" (2025)
// Nine Generative Means from a right triangle (a, b, c) provide shell radii:
//   1. DM  = a           (Differential Mean - short leg)
//   2. GM  = b           (Geometric Mean - middle leg)
//   3. AM  = c           (Arithmetic Mean - hypotenuse)
//   4. HM  = b²/c        (Harmonic Mean)
//   5. QM  = c²/b        (Quadratic Mean)
//   6. LBM = b²/a        (Log-Baseline Mean)
//   7. LGM = c*√b        (Log-Growth Mean)
//   8. DQM = ac/b        (Diff-Quadratic Mean)
//   9. DHM = ab/c        (Diff-Harmonic Mean)
// ============================================================================

/// Grant triangle - Pythagorean triple (a, b, c) where a² + b² = c²
#[derive(Clone, Copy, Debug)]
pub struct GrantTriangle {
    /// Short leg (Differential Mean)
    pub a: f32,
    /// Middle leg (Geometric Mean)
    pub b: f32,
    /// Hypotenuse (Arithmetic Mean)
    pub c: f32,
}

impl GrantTriangle {
    /// Create from the consecutive-leg family: (2n+1, 2n(n+1), 2n(n+1)+1)
    pub const fn consecutive_leg(n: u32) -> Self {
        let a = 2 * n + 1;
        let b = 2 * n * (n + 1);
        let c = b + 1;
        Self {
            a: a as f32,
            b: b as f32,
            c: c as f32,
        }
    }

    /// Compute the Nine Generative Means as shell radii (normalized to 0..1 range)
    ///
    /// NOTE (REG contract): The returned array is SORTED by value (innermost to outermost).
    /// After sorting, index i does NOT correspond to a specific named mean (e.g., index 0
    /// is not always DHM). This is intentional: downstream code uses radii for geometric
    /// shell layering, not to identify specific mean types. Do not assume index identity.
    pub fn compute_shell_radii(&self) -> [f32; 9] {
        let a = self.a;
        let b = self.b;
        let c = self.c;

        // Raw means (per Grant Projection Theorem)
        let dm = a;                    // Differential Mean
        let gm = b;                    // Geometric Mean
        let am = c;                    // Arithmetic Mean
        let hm = b * b / c;            // Harmonic Mean
        let qm = c * c / b;            // Quadratic Mean
        let lbm = b * b / a;           // Log-Baseline Mean
        let lgm = c * b.sqrt();        // Log-Growth Mean
        let dqm = a * c / b;           // Diff-Quadratic Mean
        let dhm = a * b / c;           // Diff-Harmonic Mean

        // Sort for shell ordering (innermost to outermost) - destroys named identity
        let mut radii = [dhm, dm, dqm, hm, gm, am, qm, lbm, lgm];

        // Simple bubble sort (9 elements, compile-time friendly)
        for i in 0..9 {
            for j in 0..(8 - i) {
                if radii[j] > radii[j + 1] {
                    let tmp = radii[j];
                    radii[j] = radii[j + 1];
                    radii[j + 1] = tmp;
                }
            }
        }

        // Normalize to 0..1 range (divide by max)
        let max_val = radii[8];
        if max_val > 0.0 {
            for r in &mut radii {
                *r /= max_val;
            }
        }

        radii
    }

    /// Characteristic angle θ = arccos(b/c)
    pub fn characteristic_angle(&self) -> f32 {
        (self.b / self.c).acos()
    }

    /// Self-similar ratio r = c/b
    pub fn self_similar_ratio(&self) -> f32 {
        self.c / self.b
    }
}

/// Predefined Grant triangles from the consecutive-leg Pythagorean family
/// Index 0 = (3,4,5), Index 7 = (17,144,145)
/// Higher indices = more vertices, more complex topology
pub const GRANT_TRIANGLES: [GrantTriangle; 8] = [
    GrantTriangle { a: 3.0, b: 4.0, c: 5.0 },        // n=1: V=16, tight
    GrantTriangle { a: 5.0, b: 12.0, c: 13.0 },      // n=2: V=42, classic
    GrantTriangle { a: 7.0, b: 24.0, c: 25.0 },      // n=3: V=80
    GrantTriangle { a: 9.0, b: 40.0, c: 41.0 },      // n=4: V=130
    GrantTriangle { a: 11.0, b: 60.0, c: 61.0 },     // n=5: V=192
    GrantTriangle { a: 13.0, b: 84.0, c: 85.0 },     // n=6: V=266
    GrantTriangle { a: 15.0, b: 112.0, c: 113.0 },   // n=7: V=352
    GrantTriangle { a: 17.0, b: 144.0, c: 145.0 },   // n=8: V=450, expansive
];

/// Golden triangle (Kepler): (1, √φ, φ) - special case for φ-based recursion
pub const GOLDEN_TRIANGLE: GrantTriangle = GrantTriangle {
    a: 1.0,
    b: 1.272019649514069, // √φ ≈ 1.272
    c: 1.618033988749895, // φ ≈ 1.618
};

// ============================================================================
// GRANT TRIANGLE SELECTION CONSTANTS (Hardened)
// ============================================================================
// These control the latched, quantized, confidence-locked triangle swaps.
// Audio selects which "harmonic universe" is active, not deformation.
// ============================================================================

/// Minimum hold duration before triangle change is allowed (seconds)
pub const GRANT_MIN_HOLD_SEC: f32 = 2.0;

/// Energy threshold below which we consider silence (freeze triangle)
pub const GRANT_SILENCE_ENERGY: f32 = 0.05;

/// Minimum confidence required for triangle selection (arm threshold)
pub const GRANT_CONF_MIN: f32 = 0.65;

/// Hysteresis width for confidence arm/disarm
pub const GRANT_CONF_HYST: f32 = 0.05;

/// Minimum stability required for triangle change
pub const GRANT_STAB_MIN: f32 = 0.3;

/// Smoothing alpha for confidence low-pass (per-second rate)
pub const GRANT_CONF_SMOOTH_RATE: f32 = 2.0;

/// Hysteresis width for complexity bands
pub const GRANT_BAND_HYST: f32 = 0.06;

/// Complexity band thresholds (UP direction) for triangle IDs 0..7
/// To move UP to ID N, complexity must exceed BAND_THRESHOLDS_UP[N-1]
const GRANT_BAND_THRESHOLDS_UP: [f32; 7] = [0.12, 0.24, 0.36, 0.48, 0.60, 0.72, 0.84];

/// Compute complexity score from energy + stability
#[inline]
pub fn grant_complexity_score(energy: f32, stability: f32) -> f32 {
    // Weight energy more than stability for responsiveness
    (energy * 0.6 + stability * 0.4).clamp(0.0, 1.0)
}

/// Hysteresis-based triangle selection (returns requested target ID, NOT committed)
/// current_id: the currently active triangle
/// complexity: the complexity score (0..1)
/// Returns: the requested triangle ID (0..7), respecting hysteresis bands
pub fn grant_hysteresis_select(current_id: u8, complexity: f32) -> u8 {
    let current = current_id as usize;

    // Check if we should move UP
    if current < 7 {
        let up_threshold = GRANT_BAND_THRESHOLDS_UP[current];
        if complexity >= up_threshold {
            return (current + 1) as u8;
        }
    }

    // Check if we should move DOWN
    if current > 0 {
        // Down threshold is lower than up threshold (hysteresis gap)
        let down_threshold = GRANT_BAND_THRESHOLDS_UP[current - 1] - GRANT_BAND_HYST;
        if complexity < down_threshold {
            return (current - 1) as u8;
        }
    }

    // Stay at current
    current_id
}

/// Legacy function for initial selection (non-hysteresis, used only on first activation)
/// Maps directly to a band without hysteresis - use only when no current_id exists
pub fn grant_initial_select(energy: f32, stability: f32) -> u8 {
    let complexity = grant_complexity_score(energy, stability);
    // Direct mapping to bands
    for (i, &threshold) in GRANT_BAND_THRESHOLDS_UP.iter().enumerate() {
        if complexity < threshold {
            return i as u8;
        }
    }
    7 // Max complexity
}

/// Topology router configuration
#[derive(Clone, Debug)]
pub struct TopologyRouterConfig {
    /// Crossfade duration (seconds)
    pub xfade_duration: f32,
    /// Cooldown after transition (seconds)
    pub post_transition_cooldown: f32,
    /// Beats per bar (for bar boundary detection)
    pub beats_per_bar: u8,
    /// Dwell times per route (seconds condition must hold)
    pub route_dwell_times: [f32; NUM_TOPOLOGY_ROUTES],
}

impl Default for TopologyRouterConfig {
    fn default() -> Self {
        Self {
            xfade_duration: 1.6,
            post_transition_cooldown: 1.0,
            beats_per_bar: 4,
            // Route dwell times (seconds):
            // 0: Scaffold24 → TorusKnot, 1: TorusKnot → Scaffold24,
            // 2: Scaffold24 → Vortex, 3: TorusKnot → Vortex,
            // 4: Vortex → RadialShell, 5: Vortex → Scaffold24,
            // 6: Vortex → BraidedWave, 7: RadialShell → Scaffold24,
            // 8: Vortex → TorusKnot, 9: BraidedWave → Scaffold24,
            // 10: Vortex → Dodecahedral, 11: Dodecahedral → Scaffold24
            route_dwell_times: [2.0, 1.5, 2.0, 1.8, 2.5, 1.5, 3.0, 2.0, 2.0, 2.0, 1.0, 2.0],
        }
    }
}

/// Topology router state - manages transitions between 24-fold topologies
#[derive(Clone, Debug)]
pub struct TopologyRouterState {
    /// Current active topology
    pub topology_id: TopologyId,
    /// Previous topology (for crossfade)
    pub topology_prev_id: TopologyId,
    /// Current transition phase
    pub topology_phase: TopologyPhase,
    /// Crossfade progress [0..1]
    pub topology_xfade_t: f32,
    /// Hold time in current topology (seconds)
    pub topology_hold_s: f32,
    /// Cooldown after transition (seconds remaining)
    pub topology_cooldown_s: f32,

    /// Dwell timers per route (seconds condition has held)
    pub route_dwell_s: [f32; NUM_TOPOLOGY_ROUTES],

    // === BAR TRACKING ===
    /// Current bar index (increments at bar boundaries)
    pub bar_index: u64,
    /// Last bar index when routing was evaluated
    pub last_routed_bar_index: u64,
    /// Beat accumulator within bar
    pub beat_accumulator: f32,
    /// Previous beat phase for crossing detection
    pub prev_beat_phase: f32,
    /// Beats per bar setting
    pub beats_per_bar: u8,

    // === BAR-AVERAGED METRICS ===
    /// Onset rate averaged over current bar
    pub onset_rate_bar: f32,
    /// Energy averaged over current bar
    pub energy_bar: f32,
    /// Onset accumulator for bar averaging
    pub onset_accumulator: f32,
    /// Energy accumulator for bar averaging
    pub energy_accumulator: f32,
    /// Frame count in current bar (for averaging)
    pub bar_frame_count: u32,
    /// Tension at bar start (for slope calculation)
    pub tension_bar_start: f32,
    /// Tension slope (change per bar)
    pub tension_slope: f32,

    // === FOLD24 REQUEST (router output) ===
    /// Router requests 24-fold engagement (computed at bar-edge)
    pub fold24_request: bool,
    /// Stability-based confidence for engagement (0..1)
    pub fold24_stability: f32,

    /// Configuration
    pub config: TopologyRouterConfig,
}

impl Default for TopologyRouterState {
    fn default() -> Self {
        Self {
            topology_id: TopologyId::default(),
            topology_prev_id: TopologyId::default(),
            topology_phase: TopologyPhase::default(),
            topology_xfade_t: 0.0,
            topology_hold_s: 0.0,
            topology_cooldown_s: 0.0,

            route_dwell_s: [0.0; NUM_TOPOLOGY_ROUTES],

            bar_index: 0,
            last_routed_bar_index: 0,
            beat_accumulator: 0.0,
            prev_beat_phase: 0.0,
            beats_per_bar: 4,

            onset_rate_bar: 0.0,
            energy_bar: 0.0,
            onset_accumulator: 0.0,
            energy_accumulator: 0.0,
            bar_frame_count: 0,
            tension_bar_start: 0.0,
            tension_slope: 0.0,

            fold24_request: false,
            fold24_stability: 0.0,

            config: TopologyRouterConfig::default(),
        }
    }
}

impl TopologyRouterState {
    /// Start a topology transition via crossfade
    pub fn begin_transition(&mut self, target: TopologyId) {
        if target == self.topology_id {
            return;
        }
        log::info!(
            "[TopologyRouter] Transition: {} -> {}",
            self.topology_id.name(),
            target.name()
        );
        self.topology_prev_id = self.topology_id;
        self.topology_id = target;
        self.topology_phase = TopologyPhase::Crossfade;
        self.topology_xfade_t = 0.0;
        self.topology_hold_s = 0.0;
    }

    /// Complete the current crossfade
    pub fn complete_transition(&mut self) {
        self.topology_phase = TopologyPhase::Idle;
        self.topology_xfade_t = 1.0;
        self.topology_cooldown_s = self.config.post_transition_cooldown;
        log::trace!(
            "[TopologyRouter] Transition complete, now {}",
            self.topology_id.name()
        );
    }

    /// Check if a specific route should activate (predicate + dwell)
    /// Returns true if conditions are met and dwell threshold exceeded
    pub fn check_route(&self, route_idx: usize, condition_met: bool) -> bool {
        if !condition_met {
            return false;
        }
        let dwell_required = self.config.route_dwell_times.get(route_idx).copied().unwrap_or(2.0);
        self.route_dwell_s.get(route_idx).copied().unwrap_or(0.0) >= dwell_required
    }

    /// Debug string for HUD
    pub fn debug_string(&self) -> String {
        match self.topology_phase {
            TopologyPhase::Idle => {
                format!(
                    "Topo: {} bar={} hold={:.1}s",
                    self.topology_id.name(),
                    self.bar_index,
                    self.topology_hold_s
                )
            }
            TopologyPhase::Crossfade => {
                format!(
                    "Topo: {}→{} xf={:.0}%",
                    self.topology_prev_id.name(),
                    self.topology_id.name(),
                    self.topology_xfade_t * 100.0
                )
            }
        }
    }
}

// ============================================================================
// REST RELEASE - Cymatic Shakeout Animation (v2: Offset-Based)
// ============================================================================
//
// When audio stops (entering inertia), stored vibrational energy releases as
// a physically-plausible "shakeout" - like a cymbal continuing to ring after
// being struck, or a standing wave pattern dissolving on a Chladni plate.
//
// KEY DESIGN PRINCIPLES (v2):
// 1. Oscillators produce OFFSETS added to moving targets (not absolute values)
// 2. NO PHASE RESETS - capture current error as initial conditions
// 3. ζ crossfades smoothly to critical damping (no mode switch)
// 4. Seam-locked ripple effects: cos(N*θ + phase), not camera shake
// 5. N-fold tracks dynamically with wrap-safe anchor smoothing
// 6. Amplitude envelope: A = released_energy^1.2
//
// The oscillators ring down naturally from wherever they are. On trigger,
// we capture the current pose error and add it to the oscillator state,
// so the system continues smoothly from its current position.
//
// ============================================================================

/// Damped oscillator for offset-based animation
/// Produces a time-varying OFFSET that gets added to any target
#[derive(Clone, Debug)]
struct DampedOscillator {
    /// Current offset value (added to target)
    x: f32,
    /// Current velocity
    v: f32,
    /// Natural frequency (rad/s)
    omega: f32,
    /// Base damping ratio (stored for crossfade)
    zeta_base: f32,
    /// Current effective damping ratio
    zeta: f32,
}

impl DampedOscillator {
    fn new(omega: f32, zeta: f32) -> Self {
        Self { x: 0.0, v: 0.0, omega, zeta_base: zeta, zeta }
    }

    /// Inject energy: capture error as initial displacement, add velocity impulse
    /// This is called on trigger with the CURRENT error between pose and target
    fn inject(&mut self, error: f32, velocity_impulse: f32) {
        // Add error to current offset (no reset!)
        self.x += error;
        self.v += velocity_impulse;
        // Reset damping to base (will crossfade later)
        self.zeta = self.zeta_base;
    }

    /// Update oscillator with optional zeta override for crossfade
    fn update(&mut self, dt: f32, zeta_override: Option<f32>) {
        let w = self.omega;
        let z = zeta_override.unwrap_or(self.zeta);
        let x0 = self.x; // Offset oscillates around 0
        let v0 = self.v;

        if z < 0.999 {
            // Underdamped - oscillates while decaying
            let z_clamped = z.min(0.999); // Prevent sqrt of negative
            let wd = w * (1.0 - z_clamped * z_clamped).sqrt();
            let decay = (-z_clamped * w * dt).exp();
            let cos_wd = (wd * dt).cos();
            let sin_wd = (wd * dt).sin();

            // Target is 0 for offsets
            self.x = decay * (x0 * cos_wd + (v0 + z_clamped * w * x0) / wd * sin_wd);
            self.v = decay * (v0 * cos_wd - (v0 * z_clamped * w + w * w * x0) / wd * sin_wd);
        } else {
            // Critically damped or overdamped - smooth decay to zero
            let decay = (-w * dt).exp();
            self.x = (x0 + (v0 + w * x0) * dt) * decay;
            self.v = (v0 - w * (v0 + w * x0) * dt) * decay;
        }

        // Snap to zero if converged (prevent float drift)
        if self.x.abs() < 1e-6 && self.v.abs() < 1e-5 {
            self.x = 0.0;
            self.v = 0.0;
        }
    }

    /// Get current offset value
    fn offset(&self) -> f32 {
        self.x
    }
}

/// Wrap angle to [-π, π]
fn wrap_angle(a: f32) -> f32 {
    let mut x = a % std::f32::consts::TAU;
    if x > std::f32::consts::PI {
        x -= std::f32::consts::TAU;
    } else if x < -std::f32::consts::PI {
        x += std::f32::consts::TAU;
    }
    x
}

/// RestRelease state - cymatic shakeout when entering inertia (v2)
#[derive(Clone, Debug)]
pub struct RestReleaseState {
    /// Is the release animation active?
    pub active: bool,
    /// Progress through release [0..1]
    pub t: f32,
    /// Total duration (scales with energy)
    pub duration: f32,
    /// Stored energy that drives animation intensity
    pub released_energy: f32,

    // === SYMMETRY (DYNAMIC) ===
    /// Target anchor angle (continuously tracks nearest seam)
    pub theta_anchor: f32,
    /// Smoothed theta toward anchor (for smooth locking)
    theta_smooth: f32,
    /// Current n-fold (tracks dynamically from fold24_t)
    pub n_fold: u32,

    // === OFFSET OSCILLATORS ===
    /// Rotation offset oscillator (precession + torsion combined)
    rot_osc: DampedOscillator,
    /// Scale offset oscillator (breathing)
    scale_osc: DampedOscillator,
    /// Tilt offset oscillator (nutation)
    tilt_osc: DampedOscillator,
    /// Ripple phase oscillator (drives seam pattern)
    ripple_osc: DampedOscillator,

    // === SEAM-LOCKED EFFECTS (replaces camera shake) ===
    /// Seam ripple amplitude (modulates cos(N*θ + phase))
    pub seam_ripple_amp: f32,
    /// Seam ripple phase (from ripple oscillator)
    pub seam_ripple_phase: f32,

    // === PULSE EFFECTS ===
    /// Ring bloom intensity (0-1)
    pub ring_bloom: f32,
    /// Seam glow intensity
    pub seam_intensity: f32,
    /// Orbit speed boost
    pub orbit_boost: f32,

    // === TRACKING ===
    /// Energy accumulator while audio active
    energy_accumulator: f32,
    /// Was in inertia last frame
    was_inertia: bool,
    /// Time in current state
    state_time: f32,
    /// Previous frame's pose values (for capturing error on trigger)
    prev_rot: f32,
    prev_scale: f32,
    prev_tilt: f32,
}

impl Default for RestReleaseState {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            duration: 2.5,
            released_energy: 0.0,

            theta_anchor: 0.0,
            theta_smooth: 0.0,
            n_fold: 12,

            // Offset oscillators with different frequencies for rich motion
            // Lower frequencies = slower wobble, more "resonant" feel
            rot_osc: DampedOscillator::new(3.5, 0.12),    // ~0.55 Hz, very underdamped
            scale_osc: DampedOscillator::new(6.0, 0.18),  // ~0.95 Hz, underdamped
            tilt_osc: DampedOscillator::new(8.0, 0.22),   // ~1.27 Hz, underdamped
            ripple_osc: DampedOscillator::new(5.0, 0.15), // ~0.8 Hz, feeds seam pattern

            seam_ripple_amp: 0.0,
            seam_ripple_phase: 0.0,

            ring_bloom: 0.0,
            seam_intensity: 0.0,
            orbit_boost: 0.0,

            energy_accumulator: 0.0,
            was_inertia: true,
            state_time: 0.0,

            prev_rot: 0.0,
            prev_scale: 1.0,
            prev_tilt: 0.18,
        }
    }
}

impl RestReleaseState {
    const INERTIA_ENERGY: f32 = 0.08;
    const INERTIA_TENSION: f32 = 0.05;
    const MIN_AMPLITUDE: f32 = 0.02;

    /// Check if in inertia state
    pub fn is_inertia(energy: f32, tension: f32) -> bool {
        energy < Self::INERTIA_ENERGY && tension < Self::INERTIA_TENSION
    }

    /// Amplitude envelope: A = clamp(energy^1.2, MIN, 1.0)
    fn amplitude_envelope(&self) -> f32 {
        self.released_energy.powf(1.2).clamp(Self::MIN_AMPLITUDE, 1.0)
    }

    /// Compute zeta for current t (crossfade to critical in last 20%)
    fn effective_zeta(&self, base_zeta: f32) -> f32 {
        // Smoothly blend to critical damping (ζ=1) starting at t=0.8
        let lock_blend = smoothstep(0.8, 1.0, self.t);
        base_zeta + (1.0 - base_zeta) * lock_blend
    }

    /// Nearest symmetry anchor with wrap-safe calculation
    fn nearest_anchor(theta: f32, n_fold: u32) -> f32 {
        let step = std::f32::consts::TAU / n_fold as f32;
        let wrapped = wrap_angle(theta);
        (wrapped / step).round() * step
    }

    /// Trigger the release animation (v2: no phase resets)
    fn trigger(
        &mut self,
        current_theta: f32,
        fold24_t: f32,
        current_rot: f32,
        current_scale: f32,
        current_tilt: f32,
        target_rot: f32,
        target_scale: f32,
        target_tilt: f32,
    ) {
        self.active = true;
        self.t = 0.0;
        self.state_time = 0.0;

        // Clamp and store released energy
        let e = self.released_energy.clamp(0.1, 1.0);
        self.released_energy = e;
        let amp = self.amplitude_envelope();

        // Duration: 2.0s (low energy) to 4.0s (high energy)
        self.duration = 2.0 + e * 2.0;

        // Initial symmetry (will track dynamically)
        self.n_fold = if fold24_t > 0.5 { 24 } else { 12 };
        self.theta_anchor = Self::nearest_anchor(current_theta, self.n_fold);
        self.theta_smooth = current_theta;

        // === CAPTURE CURRENT ERROR AS INITIAL CONDITIONS ===
        // This is the key to no-phase-reset: we add the current error
        // to whatever the oscillators are doing, so motion continues smoothly

        // Rotation error (wrap-safe)
        let rot_error = wrap_angle(current_rot - target_rot);
        self.rot_osc.inject(rot_error, amp * 1.8);

        // Scale error
        let scale_error = current_scale - target_scale;
        self.scale_osc.inject(scale_error, amp * 0.8);

        // Tilt error
        let tilt_error = current_tilt - target_tilt;
        self.tilt_osc.inject(tilt_error, amp * 1.2);

        // Ripple gets fresh impulse (phase doesn't need continuity)
        self.ripple_osc.inject(0.0, amp * 3.0);

        // === PULSE EFFECTS ===
        self.ring_bloom = 0.4 + amp * 0.6;
        self.seam_intensity = 0.3 + amp * 0.7;
        self.seam_ripple_amp = amp;
        self.orbit_boost = amp * 0.2;
    }

    /// Update the release animation (v2: offset-based with crossfade)
    pub fn update(
        &mut self,
        dt: f32,
        energy: f32,
        tension: f32,
        current_theta: f32,
        fold24_t: f32,
    ) -> bool {
        let is_inertia_now = Self::is_inertia(energy, tension);
        let mut triggered = false;

        // === ENERGY TRACKING ===
        if !is_inertia_now {
            let e = energy * 0.6 + tension * 0.4;
            self.energy_accumulator = lerp(self.energy_accumulator, e, 0.15);
            self.released_energy = self.energy_accumulator;
        }

        // === TRIGGER DETECTION ===
        if !self.was_inertia && is_inertia_now && !self.active {
            if self.released_energy > 0.08 {
                // Trigger with current pose state
                // Note: we pass current values = prev values since we haven't moved yet
                self.trigger(
                    current_theta,
                    fold24_t,
                    self.prev_rot,
                    self.prev_scale,
                    self.prev_tilt,
                    0.0,  // target rot (will be overridden by blend)
                    1.0,  // target scale (neutral)
                    0.18, // target tilt (base)
                );
                triggered = true;
            }
        }

        self.was_inertia = is_inertia_now;

        // === DYNAMIC N-FOLD TRACKING ===
        // Switch between 12 and 24-fold based on fold24_t
        let new_n_fold = if fold24_t > 0.5 { 24 } else { 12 };
        if new_n_fold != self.n_fold {
            // N-fold changed - find new anchor but smooth the transition
            self.n_fold = new_n_fold;
        }
        // Always track nearest anchor (will smooth toward it)
        self.theta_anchor = Self::nearest_anchor(current_theta, self.n_fold);

        // Smooth theta toward anchor (wrap-safe)
        let theta_diff = wrap_angle(self.theta_anchor - self.theta_smooth);
        self.theta_smooth += theta_diff * (1.0 - (-dt * 2.5).exp());

        if !self.active {
            // Decay accumulator while in inertia
            if is_inertia_now {
                self.energy_accumulator *= 0.995;
            }

            // Even when inactive, oscillators can still ring down
            self.rot_osc.update(dt, Some(1.0)); // Critical damping when idle
            self.scale_osc.update(dt, Some(1.0));
            self.tilt_osc.update(dt, Some(1.0));
            self.ripple_osc.update(dt, Some(1.0));

            return triggered;
        }

        // === ANIMATION UPDATE ===
        self.t += dt / self.duration;
        self.state_time += dt;

        let amp = self.amplitude_envelope();

        // === UPDATE OSCILLATORS WITH ζ CROSSFADE ===
        // Each oscillator gets its effective ζ based on progress
        let z_rot = self.effective_zeta(self.rot_osc.zeta_base);
        let z_scale = self.effective_zeta(self.scale_osc.zeta_base);
        let z_tilt = self.effective_zeta(self.tilt_osc.zeta_base);
        let z_ripple = self.effective_zeta(self.ripple_osc.zeta_base);

        self.rot_osc.update(dt, Some(z_rot));
        self.scale_osc.update(dt, Some(z_scale));
        self.tilt_osc.update(dt, Some(z_tilt));
        self.ripple_osc.update(dt, Some(z_ripple));

        // === SEAM RIPPLE (replaces camera shake) ===
        // Phase accumulates from ripple oscillator
        self.seam_ripple_phase += self.ripple_osc.offset() * dt * 8.0;
        // Amplitude decays with progress and energy
        self.seam_ripple_amp = amp * (1.0 - self.t * 0.7);

        // === PULSE EFFECTS DECAY ===
        // Smooth decay curves instead of phase-based switching
        let decay_fast = (-dt * 3.0).exp();
        let decay_med = (-dt * 1.5).exp();
        let decay_slow = (-dt * 0.8).exp();

        self.ring_bloom *= decay_fast;
        self.seam_intensity *= decay_med;
        self.orbit_boost *= decay_slow;

        // === COMPLETION ===
        if self.t >= 1.0 {
            self.active = false;
            self.t = 1.0; // Keep at 1.0 so offsets continue to zero smoothly
            self.ring_bloom = 0.0;
            self.seam_intensity = 0.0;
            self.orbit_boost = 0.0;
            self.seam_ripple_amp = 0.0;
            self.energy_accumulator *= 0.5;
        }

        triggered
    }

    // === OUTPUT ACCESSORS (OFFSETS to be ADDED to targets) ===

    /// Rotation offset (radians) - add to aim_angle target
    pub fn rot_offset(&self) -> f32 {
        self.rot_osc.offset() * 0.15 * self.amplitude_envelope()
    }

    /// Scale offset - multiply with radial scale (1.0 + offset)
    pub fn scale_offset(&self) -> f32 {
        self.scale_osc.offset() * 0.06 * self.amplitude_envelope()
    }

    /// Tilt offset (radians) - add to tilt_angle target
    pub fn tilt_offset(&self) -> f32 {
        self.tilt_osc.offset() * 0.08 * self.amplitude_envelope()
    }

    /// Seam-locked ripple value: cos(N*θ + phase) * amplitude
    /// Use this instead of camera shake for "REG feel"
    pub fn seam_ripple(&self, theta: f32) -> f32 {
        if self.seam_ripple_amp < 0.001 {
            return 0.0;
        }
        let n = self.n_fold as f32;
        self.seam_ripple_amp * (n * theta + self.seam_ripple_phase).cos()
    }

    /// Smoothed theta (for smooth anchor locking)
    pub fn theta_smoothed(&self) -> f32 {
        self.theta_smooth
    }

    /// Update tracking of previous pose (call at end of frame)
    pub fn track_pose(&mut self, rot: f32, scale: f32, tilt: f32) {
        self.prev_rot = rot;
        self.prev_scale = scale;
        self.prev_tilt = tilt;
    }

    // === LEGACY ACCESSORS (for compatibility during transition) ===

    /// Precession angle - maps to rot_offset
    pub fn precession(&self) -> f32 {
        self.rot_offset()
    }

    /// Nutation angle - maps to tilt_offset
    pub fn nutation(&self) -> f32 {
        self.tilt_offset()
    }

    /// Breathing factor - maps to 1.0 + scale_offset
    pub fn breath_factor(&self) -> f32 {
        1.0 + self.scale_offset()
    }

    /// Torsion - maps to rot_offset * 0.3
    pub fn torsion(&self) -> f32 {
        self.rot_offset() * 0.3
    }

    /// Combined wobble - maps to rot_offset
    pub fn wobble_offset(&self) -> f32 {
        self.rot_offset()
    }

    /// Ripple phase - maps to seam_ripple_phase
    pub fn ripple_phase(&self) -> f32 {
        self.seam_ripple_phase
    }

    /// Current sub-phase (for debug/UI)
    pub fn subphase(&self) -> &'static str {
        match self.t {
            t if t < 0.15 => "Burst",
            t if t < 0.50 => "Ring",
            t if t < 0.80 => "Decay",
            _ => "Lock",
        }
    }
}

impl RingDiveState {
    /// Get current subrange name
    pub fn subrange(&self) -> &'static str {
        match self.t {
            t if t < 0.18 => "lock_on",
            t if t < 0.35 => "warp_out",
            t if t < 0.80 => "guided_flight",
            _ => "warp_back",
        }
    }

    /// Normalized t within current subrange [0..1]
    pub fn subrange_t(&self) -> f32 {
        match self.t {
            t if t < 0.18 => t / 0.18,
            t if t < 0.35 => (t - 0.18) / 0.17,
            t if t < 0.80 => (t - 0.35) / 0.45,
            _ => (self.t - 0.80) / 0.20,
        }
    }

    /// Check if in warp phase (warp_out or warp_back)
    pub fn is_warping(&self) -> bool {
        let sub = self.subrange();
        sub == "warp_out" || sub == "warp_back"
    }

    /// Check if in guided flight (inside the geometry)
    pub fn is_inside(&self) -> bool {
        self.subrange() == "guided_flight"
    }

    /// Phase name from phase number
    pub fn phase_name(&self) -> &'static str {
        match self.current_phase {
            0 => "LockOn",
            1 => "WarpOut",
            2 => "Guided",
            3 => "WarpBack",
            _ => "Unknown",
        }
    }

    /// Debug string for overlay - shows phase, t, subrange, prog rate
    pub fn debug_string(&self, section_energy: f32, tension: f32) -> String {
        if self.active {
            format!(
                "RingDive: {} t={:.2} sub={} prog={:.2}/s",
                self.phase_name(),
                self.t,
                self.subrange(),
                self.last_prog_rate
            )
        } else if self.cooldown > 0.0 {
            format!("RingDive: cooldown {:.1}s", self.cooldown)
        } else {
            format!(
                "RingDive: ready E={:.2}>{:.2}s T={:.2}>{:.2}s",
                section_energy,
                self.energy_above_thresh_time,
                tension,
                self.tension_above_thresh_time
            )
        }
    }
}

// ============================================================================
// GLOBAL SILENCE STATE - System-wide silence override
// ============================================================================
//
// When silence is active, ALL topology states (torus, inside, fold24) freeze.
// Only the GLB spins. This is the "idle but alive" state.
//
// Key principles:
// 1. Silence overrides ALL modes/topologies
// 2. All accumulators freeze (charge, support, emerge timers)
// 3. Blackout alpha fades all layers except GLB
// 4. Hysteresis prevents flicker on audio edge cases
// 5. Clean restoration on exit with grace period
// ============================================================================

/// Pre-silence snapshot for clean restoration
#[derive(Clone, Debug, Default)]
pub struct PreSilenceSnapshot {
    /// Fold24 active state
    pub fold24_active: bool,
    /// Fold24 t value
    pub fold24_t: f32,
    /// Torus knot active state
    pub torus_active: bool,
    /// Torus knot t value
    pub torus_t: f32,
    /// Torus knot emerge state (as i32 for simple storage)
    pub emerge_state: i32,
    /// Torus knot charge at time of silence
    pub torus_charge: f32,
    /// Torus knot support at time of silence
    pub torus_support: f32,
    /// Inside quality tier
    pub inside_quality_tier: i32,
    /// Interior state (as i32)
    pub interior_state: i32,
}

/// Global silence state - overrides all topology/mode states
#[derive(Clone, Debug)]
pub struct GlobalSilenceState {
    /// Is global silence active?
    pub active: bool,

    /// Timer accumulating while audio is quiet (for enter hysteresis)
    pub silence_timer: f32,

    /// Timer accumulating while audio is loud (for exit hysteresis)
    pub release_timer: f32,

    /// Snapshot of state when entering silence (for clean restoration)
    pub pre_snapshot: PreSilenceSnapshot,

    /// Blackout alpha [0..1] - fades scene to black
    pub blackout_alpha: f32,

    /// GLB alpha [0..1] - fades GLB in
    pub glb_alpha: f32,

    /// GLB spin phase (quantized slow rotation during silence)
    pub glb_spin_phase: f32,

    /// Grace period timer after exiting silence (prevent topology switches)
    pub resume_grace_timer: f32,

    // === DETECTION THRESHOLDS (configurable) ===

    /// Energy threshold for entering silence
    pub quiet_enter_threshold: f32,

    /// Energy threshold for exiting silence (higher = hysteresis)
    pub quiet_exit_threshold: f32,

    /// How long audio must be quiet to enter silence (seconds)
    pub quiet_hold_enter_secs: f32,

    /// How long audio must be loud to exit silence (seconds)
    pub quiet_hold_exit_secs: f32,

    /// Grace period after exit before topology changes allowed (seconds)
    pub resume_grace_secs: f32,
}

impl Default for GlobalSilenceState {
    fn default() -> Self {
        Self {
            active: false,
            silence_timer: 0.0,
            release_timer: 0.0,
            pre_snapshot: PreSilenceSnapshot::default(),
            blackout_alpha: 0.0,
            glb_alpha: 0.0,
            glb_spin_phase: 0.0,
            resume_grace_timer: 0.0,

            // Thresholds with hysteresis
            quiet_enter_threshold: 0.045,  // Enter at 4.5% energy
            quiet_exit_threshold: 0.085,   // Exit at 8.5% energy (hysteresis)
            quiet_hold_enter_secs: 0.45,   // Hold quiet for 450ms to enter
            quiet_hold_exit_secs: 0.35,    // Hold loud for 350ms to exit
            resume_grace_secs: 0.45,       // 450ms grace before topology changes
        }
    }
}

// ============================================================================
// PRESENTATION STATE - Activity Detection and Mode Latching
// ============================================================================

/// Museum-mode presentation state with activity detection and mode latching.
/// Provides smooth crossfade between Idle (museum/screensaver) and Live (performance).
#[derive(Clone, Debug)]
pub struct PresentationState {
    /// Current presentation mode
    pub mode: PresentationMode,

    /// Smoothed activity level [0..1]
    /// Combines audio_activity and video_activity (if available)
    pub activity_level: f32,

    /// Audio activity component (smoothed energy + onset)
    pub audio_activity: f32,

    /// Video activity component (motion/luma variance) - optional
    pub video_activity: f32,

    /// Timer for mode hold (prevents flapping)
    pub mode_hold_timer: f32,

    /// Timer tracking sustained activity above enter threshold
    pub live_enter_timer: f32,

    /// Timer tracking sustained activity below exit threshold
    pub idle_enter_timer: f32,

    /// Derived blend weight for rendering [0..1]
    /// 0.0 = pure Live, 1.0 = pure Idle
    pub idle_mix: f32,

    /// Latched dominant chroma index [0..11]
    pub dominant_chroma_idx_latched: i32,

    /// Timer for chroma latch hold
    pub chroma_hold_timer: f32,

    /// Confidence accumulator for chroma latching
    pub chroma_confidence_time: f32,

    // === CONFIGURATION ===

    /// Activity threshold to enter Live mode
    pub live_enter_threshold: f32,

    /// Activity threshold to exit Live mode (enter Idle)
    pub idle_enter_threshold: f32,

    /// Sustained time above threshold to enter Live
    pub live_enter_hold_secs: f32,

    /// Sustained time below threshold to enter Idle
    pub idle_enter_hold_secs: f32,

    /// Minimum time to hold mode before allowing switch
    pub min_mode_hold_secs: f32,

    /// Crossfade duration for idle_mix transition
    pub crossfade_secs: f32,

    /// Smoothing tau for activity detection (ms)
    pub activity_tau_ms: f32,

    /// Harmonic confidence threshold for chroma latching
    pub chroma_latch_threshold: f32,

    /// Time above confidence to latch chroma
    pub chroma_latch_hold_secs: f32,

    /// Minimum chroma hold time
    pub chroma_min_hold_secs: f32,
}

impl Default for PresentationState {
    fn default() -> Self {
        Self {
            mode: PresentationMode::Idle,
            activity_level: 0.0,
            audio_activity: 0.0,
            video_activity: 0.0,
            mode_hold_timer: 0.0,
            live_enter_timer: 0.0,
            idle_enter_timer: 0.0,
            idle_mix: 1.0, // Start in Idle (idle_mix = 1.0 means full idle)
            dominant_chroma_idx_latched: 0,
            chroma_hold_timer: 0.0,
            chroma_confidence_time: 0.0,

            // Thresholds
            live_enter_threshold: 0.18,
            idle_enter_threshold: 0.08,
            live_enter_hold_secs: 0.35,
            idle_enter_hold_secs: 1.25,
            min_mode_hold_secs: 2.0,
            crossfade_secs: 0.5,
            activity_tau_ms: 300.0,
            chroma_latch_threshold: 0.45,
            chroma_latch_hold_secs: 0.5,
            chroma_min_hold_secs: 2.0,
        }
    }
}

impl PresentationState {
    /// Update activity detection and mode latching
    pub fn update(
        &mut self,
        dt: f32,
        energy: f32,
        onset_confidence: f32,
        harmonic_confidence: f32,
        dominant_pc: i32,
        video_motion: Option<f32>,
    ) {
        // === ACTIVITY DETECTION ===
        // audio_activity = smooth(max(energy, onset_confidence * 0.8), τ)
        let raw_audio = energy.max(onset_confidence * 0.8);
        let alpha = 1.0 - (-dt * 1000.0 / self.activity_tau_ms).exp();
        self.audio_activity += (raw_audio - self.audio_activity) * alpha;

        // video_activity (if available)
        if let Some(motion) = video_motion {
            self.video_activity += (motion - self.video_activity) * alpha;
        }

        // Combined activity level
        self.activity_level = self.audio_activity.max(self.video_activity);

        // === MODE LATCHING ===
        self.mode_hold_timer += dt;

        match self.mode {
            PresentationMode::Idle => {
                // Check for Live entry
                if self.activity_level > self.live_enter_threshold {
                    self.live_enter_timer += dt;
                    self.idle_enter_timer = 0.0;
                } else {
                    self.live_enter_timer = (self.live_enter_timer - dt * 0.5).max(0.0);
                }

                // Enter Live if sustained and mode hold elapsed
                if self.live_enter_timer >= self.live_enter_hold_secs
                    && self.mode_hold_timer >= self.min_mode_hold_secs
                {
                    self.mode = PresentationMode::Live;
                    self.mode_hold_timer = 0.0;
                    self.live_enter_timer = 0.0;
                }
            }
            PresentationMode::Live => {
                // Check for Idle entry
                if self.activity_level < self.idle_enter_threshold {
                    self.idle_enter_timer += dt;
                    self.live_enter_timer = 0.0;
                } else {
                    self.idle_enter_timer = (self.idle_enter_timer - dt * 0.5).max(0.0);
                }

                // Enter Idle if sustained and mode hold elapsed
                if self.idle_enter_timer >= self.idle_enter_hold_secs
                    && self.mode_hold_timer >= self.min_mode_hold_secs
                {
                    self.mode = PresentationMode::Idle;
                    self.mode_hold_timer = 0.0;
                    self.idle_enter_timer = 0.0;
                }
            }
        }

        // === IDLE_MIX CROSSFADE ===
        let target_mix = match self.mode {
            PresentationMode::Idle => 1.0,
            PresentationMode::Live => 0.0,
        };
        let crossfade_rate = 1.0 / self.crossfade_secs;
        self.idle_mix +=
            (target_mix - self.idle_mix).signum() * crossfade_rate * dt;
        self.idle_mix = self.idle_mix.clamp(0.0, 1.0);

        // === CHROMA LATCHING (Simplified) ===
        // Update dominant chroma when confidence is reasonable and pitch changes
        // Uses shorter debounce to allow responsive updates without jitter
        self.chroma_hold_timer = (self.chroma_hold_timer - dt).max(0.0);

        // Update if: new pitch is different, confidence is decent, and debounce expired
        let chroma_changed = dominant_pc != self.dominant_chroma_idx_latched;
        let confidence_ok = harmonic_confidence > 0.25;  // Lower threshold for responsiveness

        if chroma_changed && confidence_ok && self.chroma_hold_timer <= 0.0 {
            self.dominant_chroma_idx_latched = dominant_pc;
            self.chroma_hold_timer = 0.3;  // Short debounce (300ms)
        }

        // Silence freezes the latched chroma (don't reset)
    }
}

/// Dynamic camera state driven by the Journey Phase system
#[derive(Clone, Debug)]
pub struct DynamicCameraState {
    // === JOURNEY STATE ===

    /// Current journey phase
    pub phase: JourneyPhase,

    /// Time spent in current phase (seconds)
    pub phase_time: f32,

    /// Smooth phase blend (0-1 for transition animation)
    pub phase_blend: f32,

    /// Confidence accumulator (rising triggers phase advances)
    confidence_accumulator: f32,

    /// Peak energy tracker for Performer detection
    peak_energy: f32,

    // === INTERIOR LATCH STATE ===
    // Binary state machine - once inside, rules change completely

    /// Interior traversal state (Outside/Entering/Inside/Exiting)
    pub interior_state: InteriorState,

    /// Time spent in current interior state
    pub interior_state_time: f32,

    /// Cooldown before can enter again (prevents oscillation)
    interior_cooldown: f32,

    /// Flag: interior is forced to Inside by RingDive (not HeroAct::Dive)
    pub interior_forced_by_ring_dive: bool,

    // === RENDER BLEND WEIGHTS (deterministic, derived per-frame) ===
    // These are NOT drifting states - they're functions of (state, state_time)

    /// Render blend: how "inside" we are visually [0..1]
    pub inside_mix: f32,

    /// Render blend: how "24-fold" we are visually [0..1]
    pub fold24_mix: f32,

    /// Latched fold mode outside RingDive (12 or 24)
    pub latched_fold_mode: i32,

    /// Time since fold mode changed (for crossfade outside RingDive)
    pub fold_mode_change_time: f32,

    /// Previous fold24.active state as seen by camera (for domain remap on transition)
    pub prev_camera_fold24_active: bool,
    /// Cooldown after mode switch to prevent rapid remap oscillation (seconds)
    pub camera_mode_switch_cooldown: f32,

    // === CAMERA PARAMETERS (driven by phase) ===

    /// Distance from center
    pub distance: f32,

    /// FOV in degrees
    pub fov: f32,

    /// Orbit speed (rad/sec)
    pub orbit_speed: f32,

    /// Orbit impulse from kicks
    pub orbit_impulse: f32,

    /// Warp intensity (0-1) for hyperspace transition effect
    pub warp_intensity: f32,

    /// X-axis tilt angle (radians)
    pub tilt_angle: f32,

    /// Rotational noise amplitude
    pub noise_rot: f32,

    /// Positional noise amplitude
    pub noise_pos: f32,

    /// Inside-SDF dip amount (0 = surface, 1 = deep inside)
    pub inside_dip: f32,

    /// Aim offset for framing [x, y]
    pub aim_offset: [f32; 2],

    /// Current aim angle (radians)
    aim_angle: f32,

    /// Dynamic rim strength
    pub rim_boost: f32,

    // === AUDIO TRACKING ===

    /// Slow RMS (~3s tau) for macro journey
    pub section_energy: f32,

    /// Stability tracker
    stability_smooth: f32,

    /// Dominance tracker
    dominance_smooth: f32,

    /// Kick dip cooldown (seconds since last dip)
    kick_dip_cooldown: f32,

    // === LEGACY MODE WEIGHTS (for UI display) ===

    pub observer_weight: f32,
    pub immersive_weight: f32,
    pub performer_weight: f32,

    // === INTERIOR RENDERING PARAMETERS ===

    /// Cell size for chopped video (0.2-1.0)
    pub interior_cell_size: f32,

    /// Harmonic lattice frequency (3.0-12.0)
    pub interior_lattice_freq: f32,

    /// Number of volume layers (4-8)
    pub interior_layers: f32,

    /// Distance between layers (0.05-0.2)
    pub interior_layer_step: f32,

    /// Video brightness boost inside (0.5-2.0)
    pub interior_video_boost: f32,

    /// Seam glow between cells (0.0-1.0)
    pub interior_seam_strength: f32,

    /// Fractal fold iterations (0-6)
    pub interior_fold_iters: f32,

    /// Density field scale (0.5-2.0)
    pub interior_density_scale: f32,

    // === INSIDE 24-FOLD QUALITY (dynamic perf scaling) ===
    /// Inside quality auto-scaler state
    pub inside_quality: InsideQualityState,

    // === VISUAL STATE (driven by phase) ===

    /// Field density (0 = hints only, 1 = solid)
    pub field_density: f32,

    /// Surface opacity (1 = solid, 0 = see-through)
    pub surface_opacity: f32,

    /// Interior visibility (0 = hidden, 1 = full volumetric)
    pub interior_enable: f32,

    /// Skin intensity (0 = subtle, 1 = full)
    pub skin_intensity: f32,

    /// Video visibility on geometry (0 = hidden, 1 = full)
    pub video_visibility: f32,

    // === COMPEL NARRATIVE STATE (driven by act) ===

    /// Compel exterior binding (0 = everywhere, 1 = 12-fold harmonic seams only)
    pub compel_exterior_bind: f32,

    /// Compel interior reveal (0 = hidden, 1 = full lattice revelation)
    pub compel_interior_reveal: f32,

    /// Compel emissive persistence (0 = off, 1 = afterglow glow)
    pub compel_emissive_persist: f32,

    /// Compel flicker gate (0 = stable, 1 = Birth flicker suppression)
    pub compel_flicker_gate: f32,

    // === RING DIVE STATE ===
    /// RingDive transition sequence state
    pub ring_dive: RingDiveState,

    // === FOLD24 STATE (secondary inside transition) ===
    /// Fold24 state - 12-fold → 24-fold transition
    pub fold24: Fold24State,

    // === TORUS KNOT STATE (tertiary inside transition) ===
    /// TorusKnot state - 24-fold → toroidal knot cymatics
    pub torus_knot: TorusKnotState,

    // === VORTEX STATE (spiral flow topology) ===
    /// Vortex state - spiral convergent/divergent flow (sibling to TorusKnot)
    pub vortex: VortexState,

    // === DODECAHEDRAL STATE (crystalline harmonic arrival) ===
    /// Dodecahedral resonance - emerges when harmony stabilizes
    pub dodecahedral: DodecahedralState,

    // === OUTER BACKDROP STATE (premium cathedral background) ===
    /// Outer style ID: 0=Aperture+Halo (default), 1=SeamArcConstellation, 2=FacetedShell
    pub outer_style_id: u8,
    /// Aperture radius preset (0..3) - controls rim halo size
    pub outer_aperture_q: u8,
    /// Beat/onset pulse envelope [0..1] - fast attack
    pub outer_pulse_env: f32,
    /// Slow pulse envelope [0..1] - bar-length decay for premium bloom feel
    pub outer_pulse_slow: f32,
    /// Last bar when outer style changed (for hold tracking)
    pub outer_last_change_bar: u64,
    /// Whether outer can update (frozen when ambiguous)
    pub outer_can_update: bool,
    /// Gate emphasis fade [0..1] - smooth fade based on gate confidence
    pub outer_gate_fade: f32,
    /// Is current topology TorusKnot (for secondary rim)
    pub outer_is_torusknot: bool,
    /// Is current topology Vortex (for tighter aperture)
    pub outer_is_vortex: bool,

    // === TOPOLOGY ROUTER STATE (bar-edge deterministic routing) ===
    /// Topology router - manages transitions between 24-fold topologies
    pub topology_router: TopologyRouterState,

    // === QUADRIPOLAR TRAVEL STATE (REG icositetragon journey) ===
    /// Quadripolar travel through 12↔24 fold symmetry
    pub quadripolar: QuadripolarTravelState,

    // === QUATERNION CHARGES STATE (REG charge visualization) ===
    /// QuaternionCharges - active during fold24 transition
    pub qc: QuaternionChargesState,

    // === POLARITY CAMERA EFFECTS ===
    /// Yaw nudge from polarity change (decays over ~0.3s)
    pub polarity_yaw_nudge: f32,
    /// Orbit direction sign: +1.0 or -1.0, flips on polarity change
    pub polarity_orbit_sign: f32,

    // === REST RELEASE STATE (cymatic shakeout) ===
    /// RestRelease animation state
    pub rest_release: RestReleaseState,

    // === TENSION TRACKING (spectral flux + energy slope) ===
    /// Slow tension EMA for triggering (tau ~1.5s)
    pub tension_slow: f32,
    /// Fast tension EMA for visuals (tau ~0.2s)
    pub tension_fast: f32,
    /// Spectral flux (sum of positive chroma changes)
    spectral_flux: f32,
    /// Previous chroma values for flux calculation
    prev_chroma: [f32; 12],
    /// Smoothed chroma for tension calculation (matches UI display)
    smoothed_chroma: [f32; 12],
    /// Section energy slope (derivative)
    energy_slope: f32,
    /// Previous section_energy for slope calculation
    prev_section_energy: f32,
    /// Kick impulse (small, suppressed during ringdive)
    kick_impulse: f32,
    /// Beat phase accumulator [0..1] - resets on kick onset, ramps toward 1
    beat_phase_accum: f32,
    /// Previous kick level for onset detection
    prev_kick_for_beat: f32,
    /// Harmonic balance (0-1): uniform chroma distribution = 1.0, single-note = 0.0
    pub balance: f32,

    // === GATE CONFIDENCE (authoritative kill switch for gate-dependent effects) ===
    /// Gate OK scalar [0..1]: 1 = gate confident (bias allowed), 0 = ambiguous (no bias)
    /// Multiplied by all gate-dependent effects in shaders to enforce uniform seam weighting
    pub gate_ok: f32,
    /// Gate OK target (instant computation, gate_ok lerps toward this)
    gate_ok_target: f32,

    // === DIVE PHASE TRACKING ===
    /// Time energy has been below exit threshold during Dive (for hysteresis)
    dive_low_energy_time: f32,

    // === HUD CONTROLS ===
    /// Symmetry HUD visibility (toggle with key)
    pub hud_visible: bool,
    /// QS Wheel (Quaternion Symmetry Wheel) visibility
    pub qs_wheel_visible: bool,
    /// Hide numbers on QS Wheel
    pub hide_qs_numbers: bool,
    /// QS Wheel shows only dominant note (true) or all detected notes (false)
    pub qs_dominant_only: bool,

    // === HYPERSPACE JUMP DRIVE ===
    /// Hyperspace transition effect state (syncs with fold24)
    pub hyperspace: HyperspaceState,

    // === CYLINDER MOTION - Anchor-based snapping ===
    /// Mechanical anchor-snapping motion (replaces direct audio-to-angle mapping)
    pub cylinder_motion: CylinderMotion,

    // === JOURNEY BLEND - Smooth parameter system ===
    /// All render params as critically damped springs (THE source of truth)
    pub blend: JourneyBlend,

    // === GLOBAL SILENCE STATE ===
    /// Silence override - freezes all topology and shows only GLB
    pub silence: GlobalSilenceState,

    // === PRESENTATION MODE STATE ===

    /// Museum-mode presentation state (Idle vs Live with smooth crossfade)
    pub presentation: PresentationState,

    // === REG-LOCKED CAMERA STATE ===
    /// REG-locked camera - quantized motion on symmetry grid
    pub camera_reg: CameraRegState,
    /// QC port mapping - deterministic QC-to-wedge selection for camera
    pub qc_port: QcPortState,
    /// REG validation - event logging and invariant checking
    pub reg_validation: RegValidationState,
    /// Portal FPS mode - rail shooter during portal dive
    pub portal_fps: PortalFpsState,
}

impl Default for DynamicCameraState {
    fn default() -> Self {
        Self {
            // Journey starts at Birth (first act)
            phase: HeroAct::Birth,
            phase_time: 0.0,
            phase_blend: 0.0,
            confidence_accumulator: 0.0,
            peak_energy: 0.0,

            // Interior latch starts Outside
            interior_state: InteriorState::Outside,
            interior_state_time: 0.0,
            interior_cooldown: 0.0,
            interior_forced_by_ring_dive: false,

            // Render blend weights (deterministic, derived per-frame)
            inside_mix: 0.0,
            fold24_mix: 0.0,
            latched_fold_mode: 12,
            fold_mode_change_time: 0.0,
            prev_camera_fold24_active: false,
            camera_mode_switch_cooldown: 0.0,

            // Camera starts Immersive by default (not distant/static)
            distance: 2.8,          // Moderately close (Immersive default)
            fov: 45.0,
            orbit_speed: 0.03,      // Slow orbit (always moving)
            orbit_impulse: 0.0,
            warp_intensity: 0.0,    // No warp by default
            tilt_angle: 0.18,
            noise_rot: 0.003,
            noise_pos: 0.004,
            inside_dip: 0.0,
            aim_offset: [0.0, 0.0],
            aim_angle: 0.0,
            rim_boost: 0.0,

            // Audio tracking
            section_energy: 0.0,
            stability_smooth: 0.0,
            dominance_smooth: 0.0,
            kick_dip_cooldown: 0.0,

            // Legacy weights (Immersive dominant)
            observer_weight: 0.2,
            immersive_weight: 0.8,
            performer_weight: 0.0,

            // Interior rendering - video OBVIOUSLY visible (reduced complexity for performance)
            interior_cell_size: 0.4,      // Slightly larger cells
            interior_lattice_freq: 5.0,   // Reduced frequency
            interior_layers: 4.0,         // Reduced from 6 for better performance
            interior_layer_step: 0.12,    // Slightly larger steps
            interior_video_boost: 1.9,    // Keep video visible
            interior_seam_strength: 0.5,  // Slightly reduced
            interior_fold_iters: 2.0,     // Reduced from 3 for better performance
            interior_density_scale: 1.0,

            // Inside 24-fold quality (auto-scales based on GPU load)
            inside_quality: InsideQualityState::default(),

            // Visual state (Birth = field forming)
            field_density: 0.3,       // Lobes just appearing
            surface_opacity: 0.5,     // Semi-transparent
            interior_enable: 0.0,     // Not yet visible
            skin_intensity: 0.4,      // Moderate
            video_visibility: 0.8,    // Video always visible on geometry (even at rest)

            // Compel state (Birth = latent narrative)
            compel_exterior_bind: 0.0,     // Not bound to seams yet
            compel_interior_reveal: 0.0,   // Interior hidden
            compel_emissive_persist: 0.0,  // No afterglow
            compel_flicker_gate: 0.2,      // Light flicker, video shows

            // RingDive state
            ring_dive: RingDiveState::default(),

            // Fold24 state (secondary inside transition)
            fold24: Fold24State::default(),

            // TorusKnot state (tertiary inside transition)
            torus_knot: TorusKnotState::default(),

            // Vortex state (spiral flow topology)
            vortex: VortexState::default(),
            dodecahedral: DodecahedralState::default(),

            // Outer backdrop state (premium cathedral background)
            outer_style_id: 0,           // Style A: Aperture+Halo (default)
            outer_aperture_q: 1,         // Middle aperture preset
            outer_pulse_env: 0.0,
            outer_pulse_slow: 0.0,
            outer_last_change_bar: 0,
            outer_can_update: true,
            outer_gate_fade: 0.0,
            outer_is_torusknot: false,
            outer_is_vortex: false,

            // Topology router state (bar-edge deterministic routing)
            topology_router: TopologyRouterState::default(),

            // Quadripolar travel state (REG icositetragon journey)
            quadripolar: QuadripolarTravelState::default(),

            // QuaternionCharges state (REG charge visualization)
            qc: QuaternionChargesState::default(),

            // Polarity camera effects
            polarity_yaw_nudge: 0.0,
            polarity_orbit_sign: 1.0,  // Start with positive orbit direction

            // RestRelease state (cymatic shakeout)
            rest_release: RestReleaseState::default(),

            // Tension tracking (spectral flux + energy slope)
            tension_slow: 0.0,
            tension_fast: 0.0,
            spectral_flux: 0.0,
            prev_chroma: [0.0; 12],
            smoothed_chroma: [0.0; 12],
            energy_slope: 0.0,
            prev_section_energy: 0.0,
            kick_impulse: 0.0,
            beat_phase_accum: 0.0,
            prev_kick_for_beat: 0.0,
            balance: 0.0,

            // Gate confidence
            gate_ok: 0.0,
            gate_ok_target: 0.0,

            // Dive phase tracking
            dive_low_energy_time: 0.0,

            // HUD controls
            hud_visible: true,  // On by default, toggle with 'H' key
            qs_wheel_visible: true,  // QS Wheel visible by default
            hide_qs_numbers: false,  // Show numbers by default
            qs_dominant_only: true,  // Default to dominant-only mode

            // Hyperspace jump drive
            hyperspace: HyperspaceState::default(),

            // Cylinder motion - anchor-based snapping
            cylinder_motion: CylinderMotion::default(),

            // Journey blend - smooth parameter system
            blend: JourneyBlend::default(),

            // Global silence state
            silence: GlobalSilenceState::default(),
            presentation: PresentationState::default(),

            // REG-locked camera state
            camera_reg: CameraRegState::default(),
            qc_port: QcPortState::default(),
            reg_validation: RegValidationState::new(),
            portal_fps: PortalFpsState::default(),
        }
    }
}

impl DynamicCameraState {
    // ========================================================================
    // PHASE TRANSITION TRIGGER THRESHOLDS
    // ========================================================================
    //
    // Transitions triggered solely by changes in harmonic coherence,
    // dominance, and energy — not by predefined animation states.
    // ========================================================================

    /// Minimum temporal duration before phase transition (prevents flapping)
    const BIRTH_MIN_TIME: f32 = 1.5;    // Shortened for faster journey
    const LOCK_MIN_TIME: f32 = 2.0;     // Shortened - enter Dive faster
    const DIVE_MIN_TIME: f32 = 10.0;    // 10 seconds minimum inside - see the 24-fold!
    const AFTERGLOW_MIN_TIME: f32 = 2.0;

    /// Maximum time in each phase (ensures journey cycles even with continuous audio)
    const BIRTH_MAX_TIME: f32 = 6.0;
    const LOCK_MAX_TIME: f32 = 8.0;     // Shortened - force Dive entry sooner
    const DIVE_MAX_TIME: f32 = 30.0;    // Extended to allow full 24-fold experience
    const AFTERGLOW_MAX_TIME: f32 = 8.0;

    /// Birth → Lock: harmonic coherence rises above threshold
    const BIRTH_TO_LOCK_ENERGY: f32 = 0.10;  // Minimum energy for coherence

    /// Lock → Dive: energy + stability + dominance (NO KICK - kick causes flapping)
    const LOCK_TO_DIVE_ENERGY_MIN: f32 = 0.15;  // Lower bound - enter Dive easily
    const LOCK_TO_DIVE_ENERGY_MAX: f32 = 1.00;  // No upper limit - peaks can enter too
    const LOCK_TO_DIVE_STABILITY: f32 = 0.20;   // Lower stability threshold
    const LOCK_TO_DIVE_DOMINANCE: f32 = 0.15;   // Lower dominance threshold

    /// Dive → Afterglow: energy falls below threshold (VERY LOW - true silence only)
    const DIVE_TO_AFTERGLOW_ENERGY: f32 = 0.12;  // Very low - only exit on true quiet
    const DIVE_TO_AFTERGLOW_STABILITY: f32 = 0.10; // Not used anymore

    /// Afterglow → Birth: coherence approaches zero (full reset)
    const AFTERGLOW_TO_BIRTH_ENERGY: f32 = 0.15;

    /// Interior traversal cooldown (prevents continuous dipping)
    const DIP_COOLDOWN: f32 = 1.0;
    const DIP_DEPTH_MIN: f32 = 0.14;
    const DIP_DEPTH_MAX: f32 = 0.22;

    // ========================================================================
    // HERO PRESET: 4-ACT CAMERA PARAMETERS
    // ========================================================================
    //
    // Camera is IMMERSIVE by default (always somewhat close, connected)
    // section_energy drives macro distance (far ↔ close)
    // kick drives micro impulse (dip into geometry)
    // ========================================================================

    /// Get target parameters for each act
    fn phase_targets(&self) -> PhaseTargets {
        match self.phase {
            // ACT 1: BIRTH - Field Emergence
            // Mathematical Invariant: No stable angular partitions exist yet
            // Compels: Latent state - visually suppressed, flicker gate active
            HeroAct::Birth => PhaseTargets {
                distance: 3.0,          // Moderately distant, watching formation
                fov: 44.0,
                orbit_speed: 0.025,     // Slow orbit, contemplative
                tilt: 0.15,
                noise_rot: 0.003,
                noise_pos: 0.004,
                inside_dip: 0.0,        // Outside, observing
                aim_radius: 0.03,
                field_density: 0.4,     // Lobes emerging
                surface_opacity: 0.75,  // More solid so skin shows
                interior_enable: 0.0,   // Interior hidden
                skin_intensity: 0.8,    // Strong skin presence
                video_visibility: 0.7,  // Video visible on surface (show the compel!)
                // Compel: LATENT - narrative potential exists but not perceptible
                compel_exterior_bind: 0.0,     // Not bound yet
                compel_interior_reveal: 0.0,   // Interior hidden
                compel_emissive_persist: 0.0,  // No persistence
                compel_flicker_gate: 0.2,      // Light flicker, video visible
            },

            // ACT 2: LOCK - Standing Wave Formation (Narrative Binding)
            // Mathematical Invariant: Angular symmetry stable, dominant harmonic latched
            // Compels: Bind to 12-fold harmonic seams using symmetry-preserving projection
            HeroAct::Lock => PhaseTargets {
                distance: 2.4,          // Closer, engaged
                fov: 47.0,
                orbit_speed: 0.04,      // Steady orbit
                tilt: 0.20,
                noise_rot: 0.004,
                noise_pos: 0.005,
                inside_dip: 0.08,       // Teasing the surface
                aim_radius: 0.06,
                field_density: 1.0,     // Full solid geometry
                surface_opacity: 0.9,   // Solid so skin shows well
                interior_enable: 0.15,  // Hint of interior
                skin_intensity: 0.9,    // Strong skin presence
                video_visibility: 0.85, // Video clearly visible, bound to seams
                // Compel: BOUND - story elements inseparable from geometry
                compel_exterior_bind: 1.0,     // Bind to 12-fold seams
                compel_interior_reveal: 0.1,   // Teasing interior
                compel_emissive_persist: 0.0,  // No persistence yet
                compel_flicker_gate: 0.0,      // Stable, no flicker
            },

            // ACT 3: DIVE - Interior Structural Revelation
            // THE BIG ROOM: Camera is inside a vast, contemplative space
            // The room should feel like it's opening up around you
            // Mathematical Invariant: Viewpoint crosses implicit surface boundary
            // Compels: Revealed within interior interference cells mirroring external symmetry
            HeroAct::Dive => PhaseTargets {
                distance: 2.2,          // Further back to see more of the room
                fov: 58.0,              // WIDE FOV for big room feeling
                orbit_speed: 0.018,     // Very slow, contemplative drift
                tilt: 0.20,             // Less tilt for stability
                noise_rot: 0.002,       // Minimal noise - calm wonder
                noise_pos: 0.003,
                inside_dip: 0.70,       // DEEP inside (2.2 - 0.70*1.2 ≈ 1.36 effective)
                aim_radius: 0.06,       // Minimal aim offset - centered view
                field_density: 1.0,
                surface_opacity: 0.15,  // Surface mostly gone, we're IN the room
                interior_enable: 1.0,   // FULL volumetric interior
                skin_intensity: 1.0,    // Strong video presence
                video_visibility: 1.0,  // VIDEO FULLY VISIBLE in harmonic lattice
                // Compel: REVEALED - viewer enters the story space
                compel_exterior_bind: 0.0,     // Not bound to seams (inside now)
                compel_interior_reveal: 1.0,   // Full interior lattice
                compel_emissive_persist: 0.0,  // No persistence yet
                compel_flicker_gate: 0.0,      // Stable
            },

            // ACT 4: AFTERGLOW - Residual Coherence Persistence
            // Mathematical Invariant: Standing wave persists as residual coherence
            // Compels: Remain visible in softened, emissive form - no new fragmentation
            HeroAct::Afterglow => PhaseTargets {
                distance: 2.8,          // Pulling back
                fov: 45.0,
                orbit_speed: 0.035,     // Slow but present (keep motion alive)
                tilt: 0.16,
                noise_rot: 0.003,
                noise_pos: 0.004,
                inside_dip: 0.12,       // Mostly outside, memory of interior
                aim_radius: 0.04,
                field_density: 0.95,    // Still mostly solid
                surface_opacity: 0.85,  // Solid surface with video
                interior_enable: 0.25,  // Interior fading
                skin_intensity: 0.7,
                video_visibility: 0.6,  // Video still present, fading glow
                // Compel: PERSISTENT - story lingers without escalation
                compel_exterior_bind: 0.5,     // Partial seam binding
                compel_interior_reveal: 0.3,   // Fading interior
                compel_emissive_persist: 1.0,  // Full emissive persistence
                compel_flicker_gate: 0.0,      // Stable
            },
        }
    }

    /// Update camera state based on audio signals (JOURNEY-DRIVEN)
    pub fn update(
        &mut self,
        energy: f32,
        stability: f32,
        kick: f32,
        dominance: f32,
        dominant_pc: usize,
        chroma: &[f32; 12],
        dt: f32,
    ) {
        // === Update smooth audio trackers ===
        // Faster smoothing so phase transitions respond better to energy changes
        let slow_tau = 0.8;  // Was 3.0 - much faster now
        let slow_alpha = 1.0 - (-dt / slow_tau).exp();
        self.section_energy = lerp(self.section_energy, energy, slow_alpha);

        // === TENSION COMPUTATION (spectral flux + energy slope) ===
        // Tension drives RingDive triggering and visual intensity
        // NOT based on kick or instantaneous energy - those cause flopping

        // 1. Spectral flux: sum of positive chroma changes (onset detection)
        let mut flux = 0.0f32;
        for i in 0..12 {
            let diff = chroma[i] - self.prev_chroma[i];
            if diff > 0.0 {
                flux += diff;
            }
            self.prev_chroma[i] = chroma[i];
        }
        // Smooth spectral flux
        let flux_tau = 0.15;
        let flux_alpha = 1.0 - (-dt / flux_tau).exp();
        self.spectral_flux = lerp(self.spectral_flux, flux, flux_alpha);

        // Update smoothed chroma for consistent tension calculation (matches UI display)
        // Key_detector uses a 120-frame FIFO (~2s at 60fps), so use ~1.0s tau for similar effect
        let chroma_tau = 1.0;
        let chroma_alpha = 1.0 - (-dt / chroma_tau).exp();
        for i in 0..12 {
            self.smoothed_chroma[i] = lerp(self.smoothed_chroma[i], chroma[i], chroma_alpha);
        }

        // 2. Section energy slope (rising or falling)
        let slope = (self.section_energy - self.prev_section_energy) / dt.max(0.001);
        self.prev_section_energy = self.section_energy;
        let slope_tau = 0.5;
        let slope_alpha = 1.0 - (-dt / slope_tau).exp();
        self.energy_slope = lerp(self.energy_slope, slope, slope_alpha);

        // 3. Kick impulse (small, decays quickly, suppressed during ringdive AND fold24 transitions)
        let kick_suppression = if self.ring_dive.active || self.blend.fold24_t.value > 0.3 { 0.1 } else { 1.0 };
        if kick > 0.3 {
            self.kick_impulse = (self.kick_impulse + kick * 0.15 * kick_suppression).min(0.4);
        }
        self.kick_impulse *= (-dt * 6.0).exp(); // Fast decay

        // Combine into raw tension:
        // - Spectral flux (harmonic activity) weighted 0.5
        // - Energy slope magnitude weighted 0.3
        // - Kick impulse weighted 0.2
        let flux_component = (self.spectral_flux * 2.0).min(1.0);
        let slope_component = (self.energy_slope.abs() * 0.5).min(1.0);
        // Decouple kick from tension during fold24 to prevent inside_dip jumping
        let kick_component = if self.blend.fold24_t.value > 0.3 { 0.0 } else { self.kick_impulse };

        let raw_tension = flux_component * 0.5 + slope_component * 0.3 + kick_component * 0.2;

        // Two EMAs: slow for triggering, fast for visuals
        let slow_tension_tau = 1.5;
        let slow_tension_alpha = 1.0 - (-dt / slow_tension_tau).exp();
        self.tension_slow = lerp(self.tension_slow, raw_tension.clamp(0.0, 1.0), slow_tension_alpha);

        let fast_tension_tau = 0.2;
        let fast_tension_alpha = 1.0 - (-dt / fast_tension_tau).exp();
        self.tension_fast = lerp(self.tension_fast, raw_tension.clamp(0.0, 1.0), fast_tension_alpha);

        // === BALANCE COMPUTATION (chroma variance → uniformity measure) ===
        // Balance = 1.0 when chroma is uniformly distributed, 0.0 when single-note dominant
        // Used as qualifier for torus mode entry (require balance > 50%)
        let chroma_sum: f32 = chroma.iter().sum();
        let chroma_mean = chroma_sum / 12.0;
        let variance: f32 = chroma.iter()
            .map(|&c| (c - chroma_mean).powi(2))
            .sum::<f32>() / 12.0;
        // Normalize: max variance ~0.076 for single-note, so /0.08 gives 0-1 range
        let raw_balance = 1.0 - (variance / 0.08).min(1.0);
        // Gate by energy (silence = 0 balance to prevent false positives)
        let balance_target = if chroma_sum < 0.01 { 0.0 } else { raw_balance };
        // Smooth with same tau as tension_slow for consistency
        self.balance = lerp(self.balance, balance_target, slow_tension_alpha);

        let fast_tau = 0.5;
        let fast_alpha = 1.0 - (-dt / fast_tau).exp();

        // Gate stability and dominance by energy - silence means no meaningful stability/dominance
        // Use smooth energy gate so these decay during quiet periods
        let energy_gate = (self.section_energy * 5.0).clamp(0.0, 1.0);  // Full gate at energy > 0.2
        let gated_stability = stability * energy_gate;
        let gated_dominance = dominance * energy_gate;

        self.stability_smooth = lerp(self.stability_smooth, gated_stability, fast_alpha);
        self.dominance_smooth = lerp(self.dominance_smooth, gated_dominance, fast_alpha);

        // === GATE_OK COMPUTATION (authoritative kill switch for gate-dependent effects) ===
        // gate_ok = 1 when gate is confident (bias allowed), 0 when ambiguous (no bias)
        // Uses hysteresis: entry threshold higher than exit to prevent oscillation
        const GATE_OK_ENTER: f32 = 0.35;   // Must exceed to become confident
        const GATE_OK_EXIT: f32 = 0.20;    // Must drop below to become ambiguous
        const GATE_OK_RAMP_UP: f32 = 0.4;  // Seconds to ramp from 0→1 (visual smoothing)
        const GATE_OK_RAMP_DOWN: f32 = 0.2; // Seconds to ramp from 1→0 (faster fade out)

        // Hysteresis logic: sticky target
        if self.stability_smooth >= GATE_OK_ENTER {
            self.gate_ok_target = 1.0;
        } else if self.stability_smooth < GATE_OK_EXIT {
            self.gate_ok_target = 0.0;
        }
        // else: keep current target (hysteresis band)

        // Smooth visual transition (different rates for up/down)
        let prev_gate_ok = self.gate_ok;
        let gate_ok_tau = if self.gate_ok_target > self.gate_ok { GATE_OK_RAMP_UP } else { GATE_OK_RAMP_DOWN };
        let gate_ok_alpha = 1.0 - (-dt / gate_ok_tau).exp();
        self.gate_ok = lerp(self.gate_ok, self.gate_ok_target, gate_ok_alpha);

        // Track peak energy (for Performer detection)
        self.peak_energy = (self.peak_energy * 0.95).max(energy);

        // Accumulate confidence over time
        if dominance > 0.1 {
            self.confidence_accumulator += dominance * dt;
        } else {
            self.confidence_accumulator *= 0.95;
        }

        // ====================================================================
        // PRESENTATION MODE UPDATE (Idle vs Live)
        // ====================================================================
        // Runs before silence detection to track activity at all times.
        // Provides idle_mix for museum-mode polish rendering.
        // ====================================================================
        self.presentation.update(
            dt,
            energy,
            self.spectral_flux,   // onset_confidence proxy
            dominance,            // harmonic_confidence
            dominant_pc as i32,
            None,                 // video_motion (future)
        );

        // ====================================================================
        // GLOBAL SILENCE DETECTION AND UPDATE
        // ====================================================================
        // This runs BEFORE any topology/mode logic.
        // When active, all subsequent updates are skipped.
        // ====================================================================

        // Detect quiet audio state
        let audio_quiet = self.section_energy < self.silence.quiet_enter_threshold;
        let audio_loud = self.section_energy > self.silence.quiet_exit_threshold;

        if !self.silence.active {
            // === NOT IN SILENCE: Check for entry ===
            if audio_quiet {
                self.silence.silence_timer += dt;

                // Enter silence after sustained quiet
                if self.silence.silence_timer >= self.silence.quiet_hold_enter_secs {
                    self.silence.active = true;
                    self.silence.silence_timer = 0.0;
                    self.silence.release_timer = 0.0;

                    // Snapshot current state for restoration
                    self.silence.pre_snapshot = PreSilenceSnapshot {
                        fold24_active: self.fold24.active,
                        fold24_t: self.fold24.t,
                        torus_active: self.torus_knot.active,
                        torus_t: self.torus_knot.t,
                        emerge_state: self.torus_knot.emerge_state.as_i32(),
                        torus_charge: self.torus_knot.charge,
                        torus_support: self.torus_knot.support,
                        inside_quality_tier: self.inside_quality.quality.as_i32(),
                        interior_state: match self.interior_state {
                            InteriorState::Outside => 0,
                            InteriorState::Entering => 1,
                            InteriorState::Inside => 2,
                            InteriorState::Exiting => 3,
                        },
                    };
                }
            } else {
                // Audio not quiet, reset timer
                self.silence.silence_timer = 0.0;
            }
        } else {
            // === IN SILENCE: Check for exit ===
            if audio_loud {
                self.silence.release_timer += dt;

                // Exit silence after sustained loud
                if self.silence.release_timer >= self.silence.quiet_hold_exit_secs {
                    self.silence.active = false;
                    self.silence.release_timer = 0.0;
                    self.silence.resume_grace_timer = self.silence.resume_grace_secs;

                    // State is NOT forcibly restored - let the topology logic
                    // resume naturally. The grace timer prevents immediate re-triggers.
                }
            } else {
                // Audio not loud enough, reset release timer
                self.silence.release_timer = 0.0;
            }
        }

        // Update resume grace timer (ticks down after exiting silence)
        if self.silence.resume_grace_timer > 0.0 {
            self.silence.resume_grace_timer -= dt;
        }

        // === BLACKOUT/GLB ALPHA ANIMATION (runs always for smooth fade) ===
        let blackout_target = if self.silence.active { 1.0 } else { 0.0 };
        let glb_target = if self.silence.active { 1.0 } else { 0.0 };

        // Blackout: slower in, faster out (premium feel)
        let blackout_tau = if self.silence.active { 0.4 } else { 0.25 };
        let blackout_alpha = 1.0 - (-dt / blackout_tau).exp();
        self.silence.blackout_alpha = lerp(self.silence.blackout_alpha, blackout_target, blackout_alpha);

        // GLB: slightly faster in than blackout, slower out
        let glb_tau = if self.silence.active { 0.3 } else { 0.35 };
        let glb_alpha_rate = 1.0 - (-dt / glb_tau).exp();
        self.silence.glb_alpha = lerp(self.silence.glb_alpha, glb_target, glb_alpha_rate);

        // GLB spin: slow quantized stepping during silence
        if self.silence.active {
            // Step every ~200ms for mechanical feel
            self.silence.glb_spin_phase += dt * 0.5;  // Very slow
        }

        // === IF SILENCE ACTIVE: Skip ALL topology/mode logic ===
        if self.silence.active {
            // All accumulators FREEZE - no charge, support, or timer updates
            // Blackout/GLB animation already updated above
            // Skip to end of update function

            // Only update the blend springs (for smooth visual fade)
            self.blend.update(dt);

            // Copy smoothed values to state fields
            self.distance = self.blend.distance.value;
            self.fov = self.blend.fov.value.clamp(35.0, 130.0);
            self.orbit_speed = self.blend.orbit_speed.value;
            self.tilt_angle = self.blend.tilt_angle.value;
            self.noise_rot = self.blend.noise_rot.value;

            return;  // EXIT EARLY - no topology updates during silence
        }

        // === Update phase time ===
        self.phase_time += dt;

        // === PHASE TRANSITION STATE MACHINE ===
        let new_phase = self.check_phase_transition(kick);
        if new_phase != self.phase {
            self.phase = new_phase;
            self.phase_time = 0.0;
            self.phase_blend = 0.0;
        }

        // Smooth phase blend (for visual transitions)
        let blend_speed = 0.8; // ~1.25s to full blend
        self.phase_blend = (self.phase_blend + dt * blend_speed).min(1.0);

        // === INTERIOR LATCH STATE MACHINE ===
        // This is the "permission" to be inside. Binary, not blended.
        self.interior_state_time += dt;
        self.interior_cooldown = (self.interior_cooldown - dt).max(0.0);

        let new_interior_state = self.check_interior_transition(kick);
        if new_interior_state != self.interior_state {
            self.interior_state = new_interior_state;
            self.interior_state_time = 0.0;

            // Set cooldown when exiting to prevent oscillation
            if new_interior_state == InteriorState::Outside {
                self.interior_cooldown = 3.0;  // 3 second cooldown before can enter again
            }
        }

        // === COMPUTE INSIDE_MIX (deterministic blend weight) ===
        self.inside_mix = self.compute_inside_mix();

        // Track whether interior is forced by RingDive (for debug HUD)
        self.interior_forced_by_ring_dive = self.ring_dive.active && self.ring_dive.t >= 0.18;

        // === GET PHASE TARGETS ===
        let targets = self.phase_targets();

        // === SET TARGETS ON JOURNEY BLEND ===
        // State machines set TARGETS, springs smooth toward them.
        // NO direct value assignments - everything goes through the blend.

        // Check for witness freeze (hold camera during 24-fold onset)
        let witness_freeze = self.fold24.witness_timer > 0.0;

        // === INTERIOR STATE DRIVES TARGETS ===
        match self.interior_state {
            InteriorState::Outside => {
                // Normal phase-driven rendering
                self.blend.set_phase_targets(&targets);
                self.blend.warp_intensity.set_target(0.0);

                // If resting, drift aim to nearest symmetry anchor
                if self.blend.is_resting {
                    let n_fold = if self.fold24.t > 0.1 { 24 } else { 12 };
                    self.blend.aim_angle.drift_to_nearest_anchor(n_fold);

                    // Check if resting complete
                    if self.blend.resting_complete() {
                        self.blend.exit_resting();
                    }
                }
            }
            InteriorState::Entering => {
                // HYPERSPACE ENTRY - compute targets for dramatic 1.2s transition
                let t = (self.interior_state_time / 1.2).min(1.0);
                self.blend.set_entering_targets(t);
            }
            InteriorState::Inside => {
                // THE BIG ROOM - set stable inside targets
                self.blend.set_inside_targets();

                // Room slowly expands over 8s
                let room_expand = (self.interior_state_time / 8.0).min(1.0);
                let expanded_fov = 58.0 + room_expand * 4.0;  // 58 → 62
                let expanded_dist = 2.2 + room_expand * 0.4;  // 2.2 → 2.6
                self.blend.fov.set_target(expanded_fov);
                self.blend.distance.set_target(expanded_dist);
            }
            InteriorState::Exiting => {
                // REVERSE HYPERSPACE - dramatic exit over 1.2s
                let t = (self.interior_state_time / 1.2).min(1.0);
                self.blend.set_exiting_targets(t, &targets);

                // When exit completes, enter resting phase for smooth alignment
                if t >= 0.95 && !self.blend.is_resting {
                    let n_fold = if self.fold24.t > 0.1 { 24 } else { 12 };
                    self.blend.enter_resting(n_fold);
                }
            }
        }

        // === UPDATE ALL SPRINGS (THE SMOOTHING HAPPENS HERE) ===
        // Don't update if frozen (witness beat)
        if !witness_freeze {
            self.blend.update(dt);
        }

        // === COPY SMOOTHED VALUES TO STATE FIELDS (for backward compatibility) ===
        // The blend is the source of truth, but other code reads from these fields.
        self.distance = self.blend.distance.value;
        self.fov = self.blend.fov.value.clamp(35.0, 130.0);  // Clamp for safety
        self.orbit_speed = self.blend.orbit_speed.value;
        self.tilt_angle = self.blend.tilt_angle.value;
        self.noise_rot = self.blend.noise_rot.value;
        self.noise_pos = self.blend.noise_pos.value;
        self.inside_dip = self.blend.inside_dip.value;
        self.aim_angle = self.blend.aim_angle.value;

        self.warp_intensity = self.blend.warp_intensity.value;

        self.field_density = self.blend.field_density.value;
        self.surface_opacity = self.blend.surface_opacity.value;
        self.interior_enable = self.blend.interior_enable.value;
        self.skin_intensity = self.blend.skin_intensity.value;
        self.video_visibility = self.blend.video_visibility.value;

        self.compel_exterior_bind = self.blend.compel_exterior_bind.value;
        self.compel_interior_reveal = self.blend.compel_interior_reveal.value;
        self.compel_emissive_persist = self.blend.compel_emissive_persist.value;
        self.compel_flicker_gate = self.blend.compel_flicker_gate.value;

        self.interior_video_boost = self.blend.interior_video_boost.value;

        // === ORBIT IMPULSE DISABLED ===
        // Beat/kick detection removed - motion is now purely harmonic
        // Orbit is driven only by section_energy and harmonic content
        self.blend.orbit_impulse.set_target(0.0);
        self.orbit_impulse = 0.0;

        // === CYLINDER SNAP CONTROLLER ===
        // STRICT anchor-based snapping with:
        // - Dead zone + mandatory hold (no micro-adjustments)
        // - Quantized targets only (12-fold or 24-fold)
        // - Rate limited (max ±1 anchor per move, cooldown enforced)
        // - One-shot transitions (target LOCKED during move)
        // - Hysteresis triggering (high to arm, low to reset)
        // - 2-layer EMA filtering (slow for decisions, fast for intensity)

        // Set mode based on interior state
        let is_inside = matches!(self.interior_state, InteriorState::Inside);
        self.cylinder_motion.set_mode(is_inside);

        // Update cylinder snap controller
        // Uses internal EMAs - raw values filtered internally
        self.cylinder_motion.update(
            dt,
            energy,                 // Raw energy (filtered internally)
            self.tension_slow,      // Raw tension (filtered internally)
            dominant_pc,            // Pitch class (stability checked internally)
        );

        // Use cylinder motion output for aim angle
        // This replaces the direct dominant_pc → aim_angle mapping
        self.aim_angle = self.cylinder_motion.angle;

        // Also update the blend system so other code that reads blend.aim_angle works
        self.blend.aim_angle.snap(self.aim_angle);

        self.aim_offset[0] = self.aim_angle.cos() * targets.aim_radius;
        self.aim_offset[1] = self.aim_angle.sin() * targets.aim_radius * 0.5 - 0.03;

        // === Dynamic rim boost ===
        let close_factor = smoothstep(3.0, 1.5, self.distance);
        self.rim_boost = close_factor * 0.5;

        // === Update legacy mode weights (for UI display) ===
        // Hero Preset: Immersive-dominant, with Performer peaks during Dive
        let weight_smooth = 1.0 - (-dt * 8.0).exp();
        let (obs, imm, perf) = match self.phase {
            HeroAct::Birth => (0.3, 0.7, 0.0),       // Immersive + observing
            HeroAct::Lock => (0.1, 0.9, 0.0),        // Very Immersive
            HeroAct::Dive => (0.0, 0.5, 0.5),        // Immersive + Performer inside
            HeroAct::Afterglow => (0.2, 0.8, 0.0),   // Mostly Immersive, slight observer
        };
        self.observer_weight = lerp(self.observer_weight, obs, weight_smooth);
        self.immersive_weight = lerp(self.immersive_weight, imm, weight_smooth);
        self.performer_weight = lerp(self.performer_weight, perf, weight_smooth);

        // === Update kick dip cooldown ===
        self.kick_dip_cooldown += dt;

        // === UPDATE TOPOLOGY ROUTER (bar-edge evaluated) ===
        // Beat phase accumulator: time-based cycling ~120 BPM
        // This ensures beat crossings happen regularly for REG camera commits
        const BEAT_RATE: f32 = 2.0;  // 120 BPM = 2 beats/sec = 0.5s per beat
        self.beat_phase_accum += dt * BEAT_RATE;

        // Wrap phase to [0, 1) - this creates the 0.99→0.01 crossing that beat_crossed detects
        while self.beat_phase_accum >= 1.0 {
            self.beat_phase_accum -= 1.0;
        }

        // Optional: kick onset can nudge phase toward 0 for better sync (future improvement)
        // For now, time-based cycling guarantees regular beat crossings
        let beat_phase_approx = self.beat_phase_accum;
        // Determine dominance from smoothed chroma
        let chroma_mean: f32 = self.smoothed_chroma.iter().sum::<f32>() / 12.0;
        let mid_sum: f32 = self.smoothed_chroma[2..8].iter().sum();
        let bass_sum: f32 = self.smoothed_chroma[0..3].iter().sum();
        let high_sum: f32 = self.smoothed_chroma[8..12].iter().sum();
        let mid_dominant = mid_sum > bass_sum && mid_sum > high_sum;
        let high_dominant = high_sum > bass_sum && high_sum > mid_sum;
        self.update_topology_router(dt, beat_phase_approx, stability, mid_dominant, high_dominant);

        // === UPDATE RING DIVE ===
        self.update_ring_dive(dt, dominant_pc);

        // === UPDATE FOLD24 (independent of RingDive) ===
        // Fold24 emerges whenever we're inside, based on time + tension
        self.update_fold24_timeline(dt);

        // === COMPUTE FOLD24_MIX AND TRACK LATCHED MODE ===
        // Track previous fold24_mix for gate continuity
        let old_fold24_mix = self.fold24_mix;

        // Update latched fold mode (12 or 24) when RingDive inactive
        // This determines which mode we fade TOWARD outside RingDive
        let desired_mode = if self.fold24.t > 0.5 || self.torus_knot.active { 24 } else { 12 };
        if desired_mode != self.latched_fold_mode && !self.ring_dive.active {
            self.latched_fold_mode = desired_mode;
            self.fold_mode_change_time = 0.0;
        }
        self.fold_mode_change_time += dt;

        // Compute fold24_mix (deterministic blend weight)
        self.fold24_mix = self.compute_fold24_mix();

        // === GATE CONTINUITY: Prevent anchor jumps during 12↔24 crossfade ===
        // When crossing 0.5 threshold, the effective n_fold changes.
        // The aim_angle should NOT search for nearest anchor during this window.
        // Instead, maintain current angle - springs handle the smooth transition.
        let in_fold_crossfade = self.fold24_mix > 0.01 && self.fold24_mix < 0.99;
        let crossed_threshold = (old_fold24_mix <= 0.5 && self.fold24_mix > 0.5)
            || (old_fold24_mix >= 0.5 && self.fold24_mix < 0.5);

        // If we just crossed the threshold, bump aim_angle target to maintain alignment
        // 12-fold anchor at index i → 24-fold anchor at index 2*i (even indices)
        // This prevents the nearest-anchor search from jumping to odd anchors
        if crossed_threshold && !in_fold_crossfade {
            // No action needed - the drift_to_nearest_anchor uses the current n_fold
            // and our smoothstep-based fold24_mix ensures gradual visual transition
        }

        // === UPDATE TORUS KNOT CYMATICS (tertiary inside transition) ===
        // Triggers when fold24 is stable; 24 spokes curl into torus knot
        self.update_torus_knot(dt, dominant_pc, chroma);

        // === UPDATE QUADRIPOLAR TRAVEL (REG icositetragon journey) ===
        // Coherent 12↔24 fold journey following quaternion symmetry
        self.update_quadripolar(dt, dominant_pc);

        // === UPDATE VORTEX (spiral flow topology) ===
        // Sibling to TorusKnot, triggered by topology router
        self.update_vortex(dt, dominant_pc, beat_phase_approx);

        // === UPDATE DODECAHEDRAL (crystalline harmonic arrival) ===
        // Emerges from Vortex when harmony stabilizes (low entropy, high stability)
        self.update_dodecahedral(dt, dominant_pc, beat_phase_approx, chroma);

        // === UPDATE OUTER BACKDROP (premium cathedral background) ===
        // Aperture + rim halo with beat-quantized pulses
        self.update_outer_backdrop(dt, beat_phase_approx, self.kick_impulse);

        // === UPDATE QUATERNION CHARGES (REG charge visualization) ===
        // Active during fold24 transition window
        self.update_quaternion_charges(dt, dominant_pc, self.section_energy);

        // === UPDATE QC PORT SELECTION (QC-to-camera wedge mapping) ===
        // Deterministic mapping from QC state to 24-fold wedge anchor
        let beat_crossed = self.camera_reg.beat_cross_detected;
        self.update_qc_port(dt, beat_crossed);

        // === UPDATE REG-LOCKED CAMERA (quantized camera motion) ===
        // Camera rides the same angular grid as the geometry
        let dominant_gate_k = ((dominant_pc * 2) % 24) as u8;  // 12-fold PC → 24-fold gate
        // Use dominance_smooth as beat confidence proxy (rhythmic clarity correlates with harmonic dominance)
        self.update_camera_reg(
            dt,
            beat_phase_approx,
            self.dominance_smooth,    // Beat confidence proxy
            self.stability_smooth,    // Harmonic confidence
            self.gate_ok_target,      // Gate stability
            dominant_gate_k,
            self.section_energy,
        );

        // === REG VALIDATION: Periodic summary (every 60s) ===
        self.reg_validation.maybe_print_periodic_summary();

        // === REG STATE DUMP: Focused validation (every 5s) ===
        {
            // Use prev_camera_fold24_active: this tracks the domain the camera is actually
            // operating in (only updates when mode switch fires, not blocked by cooldown).
            let gate_mode = if self.prev_camera_fold24_active { 24u8 } else { 12u8 };
            let current_gate = self.camera_reg.anchor_k;
            let harmonic_state_idx = if gate_mode == 24 {
                current_gate / 2
            } else {
                current_gate % 12
            };
            self.reg_validation.maybe_dump_state(
                gate_mode,
                current_gate,
                self.gate_ok,
                self.stability_smooth,
                self.fold24.permitted,
                self.fold24.engaged,
                self.fold24.t,
                self.fold24.engage_t,
                self.qc.current_degree,
                self.qc.current_quadrant,
                self.qc.active,                       // QC active (tri-state lattice)
                self.qc.invariants.lattice_ok_24,
                self.topology_router.topology_id.name(),
                self.section_energy,
                dominant_pc,
                harmonic_state_idx,
            );
        }

        // === APPLY POLARITY CAMERA EFFECTS ===
        // Yaw nudge and orbit direction flip on polarity/quadrant changes
        self.apply_polarity_camera_effects(dt);

        // === UPDATE REST RELEASE (cymatic shakeout on inertia entry) ===
        let _release_triggered = self.rest_release.update(
            dt,
            self.section_energy,
            self.tension_slow,
            self.aim_angle,
            self.fold24.t,
        );

        // Apply rest release OFFSETS to blend (not absolute targets)
        // This adds oscillator offsets on top of whatever targets were already set
        self.blend.apply_rest_release_offsets(&self.rest_release, self.aim_angle);

        // Track current pose for next frame's error capture
        self.rest_release.track_pose(
            self.aim_angle,
            self.blend.radial_breath.value,
            self.blend.tilt_angle.value,
        );

        // Add orbit boost from rest release (through spring for smooth motion)
        if self.rest_release.active {
            let boost = self.rest_release.orbit_boost * 0.3;
            let current = self.blend.orbit_impulse.target;
            self.blend.orbit_impulse.set_target((current + boost).min(0.12));
        }
    }

    // ========================================================================
    // RING DIVE - Cinematic Inside Transition Sequence
    // ========================================================================

    /// Update RingDive state based on energy and tension
    /// Uses tension_slow for triggering, min rates ensure completion
    fn update_ring_dive(&mut self, dt: f32, dominant_pc: usize) {
        let cfg = self.ring_dive.config.clone();

        // Update cooldown
        if self.ring_dive.cooldown > 0.0 {
            self.ring_dive.cooldown -= dt;
            self.ring_dive.energy_above_thresh_time = 0.0;
            self.ring_dive.tension_above_thresh_time = 0.0;
            self.ring_dive.abort_reason = None;
            return;
        }

        if !self.ring_dive.active {
            // === ENTRY CONDITIONS: Energy + Tension (NO INSIDE GATE) ===
            // RingDive IS the cinematic "go inside" sequence.
            // It triggers from energy/tension alone, then FORCES interior state.
            // This is the authoritative inside transition - not HeroAct::Dive.

            // Accumulate energy above threshold
            if self.section_energy >= cfg.energy_enter_thresh {
                self.ring_dive.energy_above_thresh_time += dt;
            } else {
                self.ring_dive.energy_above_thresh_time =
                    (self.ring_dive.energy_above_thresh_time - dt * 1.5).max(0.0);
            }

            // Accumulate tension above threshold
            if self.tension_slow >= cfg.tension_enter_thresh {
                self.ring_dive.tension_above_thresh_time += dt;
            } else {
                self.ring_dive.tension_above_thresh_time =
                    (self.ring_dive.tension_above_thresh_time - dt * 1.5).max(0.0);
            }

            // Trigger when both conditions met (energy + tension sustained)
            if self.ring_dive.energy_above_thresh_time > cfg.energy_enter_hold
               && self.ring_dive.tension_above_thresh_time > cfg.tension_enter_hold
            {
                log::info!("[RingDive] *** TRIGGERED *** energy={:.2}, tension={:.2}",
                    self.section_energy, self.tension_slow);
                self.ring_dive.active = true;
                self.ring_dive.t = 0.0;
                self.ring_dive.current_phase = 0; // LockOn
                self.ring_dive.phase_enter_time = 0.0;
                self.ring_dive.target_band = self.select_ring_dive_target_band();
                self.ring_dive.inside_duration = 0.0;
                self.ring_dive.abort_reason = None;
                self.generate_ring_dive_path(dominant_pc);
            }
        } else {
            // === ACTIVE: Advance t with MIN RATES (ensures completion) ===

            // Determine current phase from t
            let new_phase: u8 = if self.ring_dive.t < 0.18 { 0 }       // LockOn
                else if self.ring_dive.t < 0.35 { 1 }                  // WarpOut
                else if self.ring_dive.t < 0.80 { 2 }                  // Guided
                else { 3 };                                            // WarpBack

            // Detect phase transitions
            if new_phase != self.ring_dive.current_phase {
                log::trace!("[RingDive] Phase: {} → {} (t={:.2})",
                    self.ring_dive.phase_name(),
                    match new_phase { 0 => "LockOn", 1 => "WarpOut", 2 => "Guided", _ => "WarpBack" },
                    self.ring_dive.t);
                self.ring_dive.current_phase = new_phase;
                self.ring_dive.phase_enter_time = 0.0;

                // On entering Guided: reset timers, suppress orbit
                if new_phase == 2 {
                    self.ring_dive.guided_time = 0.0;
                    self.ring_dive.orbit_suppress_timer = 0.5;  // Disable orbit for 0.5s
                }
            }
            self.ring_dive.phase_enter_time += dt;

            // Track time in guided phase
            if self.ring_dive.current_phase == 2 {
                self.ring_dive.guided_time += dt;
                self.ring_dive.orbit_suppress_timer = (self.ring_dive.orbit_suppress_timer - dt).max(0.0);
            }

            // NOTE: fold24 update is called AFTER update_ring_dive in main update()
            // so it sees the NEW ring_dive.t value. Don't call here (causes stale read).

            // Get min rate and dwell time for current phase
            let (min_rate, dwell_time) = match self.ring_dive.current_phase {
                0 => (cfg.min_rate_lock_on, cfg.dwell_lock_on),
                1 => (cfg.min_rate_warp_out, cfg.dwell_warp_out),
                2 => (cfg.min_rate_guided, 0.0), // No dwell for guided - it's long
                _ => (cfg.min_rate_warp_back, cfg.dwell_warp_back),
            };

            // Dynamic rate: tension_fast modulates speed, but never below min_rate
            // For Guided phase: use constant rate (ignore tension for progression)
            // This GUARANTEES we cross 0.60 for unfold24
            let final_rate = if self.ring_dive.current_phase == 2 {
                // Guided: constant min_rate, tension only affects visuals not progression
                min_rate
            } else {
                // Other phases: tension can speed things up
                let dynamic_rate = min_rate * (1.0 + self.tension_fast * 0.5);
                dynamic_rate.max(min_rate)
            };
            self.ring_dive.last_prog_rate = final_rate;

            // Only advance if past dwell time (creates "beats" in the sequence)
            let past_dwell = self.ring_dive.phase_enter_time > dwell_time;

            // Check phase boundary - don't advance past phase end until dwell is done
            // For Guided phase: enforce MIN 2 SECONDS inside before allowing exit
            let at_phase_boundary = match self.ring_dive.current_phase {
                0 => self.ring_dive.t >= 0.17,  // About to leave LockOn
                1 => self.ring_dive.t >= 0.34,  // About to leave WarpOut
                2 => self.ring_dive.t >= 0.79 && self.ring_dive.guided_time < 2.0,  // Min 2s inside
                3 => self.ring_dive.t >= 0.99,  // About to complete
                _ => false,
            };

            if !at_phase_boundary || past_dwell {
                self.ring_dive.t += final_rate * dt * 0.2; // Scale to ~5s total (faster 24-fold arrival)
            }

            // === ABORT: ONLY ALLOWED IN LOCK_ON (t < 0.18) ===
            // Once committed, we finish the sequence no matter what
            self.ring_dive.abort_reason = None;

            if self.ring_dive.t < 0.18 {
                // Still in LockOn - check abort conditions
                let should_abort = self.tension_slow < cfg.tension_exit_thresh
                    && self.section_energy < cfg.energy_exit_thresh;

                if should_abort {
                    self.ring_dive.abort_reason = Some("low_energy_tension");
                    log::trace!("[RingDive] Aborted in LockOn (energy={:.2}, tension={:.2})",
                        self.section_energy, self.tension_slow);
                    self.ring_dive.active = false;
                    self.ring_dive.t = 0.0;
                    self.ring_dive.cooldown = 3.0; // Short cooldown for aborts
                    return;
                }
            }
            // Once t >= 0.18, NO ABORTS - sequence runs to completion

            // Track inside duration (during warp_out and guided_flight)
            if self.ring_dive.t > 0.18 && self.ring_dive.t < 0.80 {
                if self.distance < 1.8 {
                    self.ring_dive.inside_duration += dt;
                }
            }

            // Complete
            if self.ring_dive.t >= 1.0 {
                log::trace!("[RingDive] Complete! inside_duration={:.2}s, final_t={:.2}",
                    self.ring_dive.inside_duration, self.ring_dive.t);
                self.ring_dive.active = false;
                self.ring_dive.t = 0.0;
                self.ring_dive.cooldown = cfg.cooldown_duration;
                self.ring_dive.energy_above_thresh_time = 0.0;
                self.ring_dive.tension_above_thresh_time = 0.0;
            }

            // Update camera rail when active
            self.update_ring_dive_camera(dt);
        }
    }

    // ========================================================================
    // FOLD24 - Secondary Inside Transition (12-fold → 24-fold)
    // TIMELINE-DRIVEN: Follows RingDive t, not tension/energy detection
    // ========================================================================
    //
    // RingDive timeline chapters:
    //   lock_on:       t 0.00 → 0.18  (outside, preparing)
    //   warp_out:      t 0.18 → 0.35  (accelerate through portal)
    //   guided_flight: t 0.35 → 0.60  (inside, 12-fold)
    //   unfold24:      t 0.60 → 0.82  (inside, 12→24 transition)
    //   warp_back:     t 0.82 → 1.00  (return, 24-fold visible on rings)
    //
    // The 24-fold unfold is a SCRIPTED BEAT, not a detector.
    // Even if music is "ambiguous," the journey still lands.
    // ========================================================================

    /// Update Fold24 state - DUAL-PATH ENGAGEMENT MODEL
    ///
    /// Path 1: RingDive (scripted beat)
    ///   - unfold window: ring_dive.t 0.60 → 0.82
    ///   - fold24.t_raw = smoothstep(0.60, 0.82, ring_dive.t)
    ///
    /// Path 2: TopologyRequest (router-driven)
    ///   - Router sets fold24_request when stability conditions met
    ///   - Engagement after REQUEST_DWELL_BARS of sustained request
    ///
    /// Engagement is latched: MIN_ENGAGED_BARS before disengage is possible.
    /// On engagement, we ALWAYS force Scaffold24 first (safe doorway).
    fn update_fold24_timeline(&mut self, dt: f32) {
        // === CONSTANTS ===
        const UNFOLD_START: f32 = 0.60;
        const UNFOLD_END: f32 = 0.82;
        const REQUEST_DWELL_BARS: u32 = 2;   // Bars of sustained request before engage
        const MIN_ENGAGED_BARS: u32 = 4;     // Min bars in 24-fold before disengage
        const ENGAGE_RAMP_S: f32 = 1.2;      // Ramp time for engage_t

        let was_engaged = self.fold24.engaged;
        let was_t_raw = self.fold24.t_raw;
        let current_bar = self.topology_router.bar_index;

        // === UPDATE REQUEST COOLDOWN ===
        if self.fold24.request_cooldown_s > 0.0 {
            self.fold24.request_cooldown_s = (self.fold24.request_cooldown_s - dt).max(0.0);
        }

        // === PATH 1: RINGDIVE PROGRESS ===
        // RingDive can directly set fold24.t_raw based on timeline
        let ringdive_progress = if self.ring_dive.active {
            let rd_t = self.ring_dive.t;
            if rd_t <= UNFOLD_START {
                0.0
            } else if rd_t >= UNFOLD_END {
                1.0
            } else {
                let t = (rd_t - UNFOLD_START) / (UNFOLD_END - UNFOLD_START);
                t * t * (3.0 - 2.0 * t) // smoothstep
            }
        } else {
            0.0
        };

        // RingDive engagement: engaged when progress >= 0.5
        let ringdive_wants_engage = ringdive_progress >= 0.5;

        // === PATH 2: TOPOLOGY REQUEST ===
        // Router has set fold24_request based on stability conditions
        // We accumulate request_latch_bars at bar boundaries
        let request_wants_engage = self.topology_router.fold24_request
            && self.fold24.request_latch_bars >= REQUEST_DWELL_BARS
            && self.fold24.request_cooldown_s <= 0.0;

        // === ENGAGEMENT DECISION (bar-edge conceptually, but we update engage_t continuously) ===
        let should_engage = self.fold24.permitted && (ringdive_wants_engage || request_wants_engage);

        // Check min hold before allowing disengage
        let bars_engaged = if self.fold24.engaged {
            current_bar.saturating_sub(self.fold24.engaged_bar_index)
        } else {
            0
        };
        let can_disengage = !self.fold24.engaged || bars_engaged >= MIN_ENGAGED_BARS as u64;

        // Engagement state transition
        if should_engage && !self.fold24.engaged {
            // === ENGAGE ===
            const ENTRY_FUNNEL_BARS: u32 = 2;  // Bars before topology switching allowed

            self.fold24.engaged = true;
            self.fold24.engaged_bar_index = current_bar;
            self.fold24.entry_funnel_bars_left = ENTRY_FUNNEL_BARS;
            self.fold24.entry_reason = if ringdive_wants_engage {
                Fold24EntryReason::RingDive
            } else {
                Fold24EntryReason::TopologyRequest
            };

            // Track which topology triggered the request (for preference after funnel)
            // Vortex is the default requester since TorusKnot has its own emergence path
            self.fold24.entry_requested_topology = if !ringdive_wants_engage {
                Some(TopologyId::Vortex)
            } else {
                None
            };

            log::info!("[Fold24] *** ENGAGED *** reason={:?}, bar={}, funnel={} bars",
                self.fold24.entry_reason, current_bar, ENTRY_FUNNEL_BARS);

            // === FORCE SCAFFOLD24 ON ENGAGEMENT (safe doorway) ===
            if self.topology_router.topology_id != TopologyId::Scaffold24 {
                log::info!("[Fold24] Forcing Scaffold24 as entry point");
                self.topology_router.topology_prev_id = self.topology_router.topology_id;
                self.topology_router.topology_id = TopologyId::Scaffold24;
                self.topology_router.topology_hold_s = 0.0;
                self.topology_router.topology_phase = TopologyPhase::Idle;
                self.topology_router.topology_xfade_t = 0.0;
            }
        } else if !should_engage && self.fold24.engaged && can_disengage {
            // === DISENGAGE ===
            self.fold24.engaged = false;
            self.fold24.entry_reason = Fold24EntryReason::None;
            self.fold24.entry_funnel_bars_left = 0;
            self.fold24.entry_requested_topology = None;
            self.fold24.request_latch_bars = 0;  // Clear latch on disengage
            self.fold24.request_cooldown_s = 2.0; // Prevent spam
            log::info!("[Fold24] *** DISENGAGED *** after {} bars", bars_engaged);
        }

        // === UPDATE ENGAGE_T RAMP ===
        let engage_target = if self.fold24.engaged { 1.0 } else { 0.0 };
        let ramp_rate = 1.0 / ENGAGE_RAMP_S;
        if self.fold24.engage_t < engage_target {
            self.fold24.engage_t = (self.fold24.engage_t + ramp_rate * dt).min(1.0);
        } else if self.fold24.engage_t > engage_target {
            self.fold24.engage_t = (self.fold24.engage_t - ramp_rate * dt).max(0.0);
        }

        // === UPDATE T_RAW AND T ===
        // When engaged via request (not RingDive), use engage_t directly
        // When engaged via RingDive, use ringdive_progress
        if self.ring_dive.active {
            self.fold24.t_raw = ringdive_progress;
        } else if self.fold24.engaged {
            self.fold24.t_raw = self.fold24.engage_t;
        } else if self.torus_knot.active || self.torus_knot.t > 0.01 {
            // TorusKnot keeps fold24 maxed
            self.fold24.t_raw = 1.0;
        } else {
            self.fold24.t_raw = self.fold24.engage_t;
        }

        // Smooth via blend spring
        self.blend.fold24_t.set_target(self.fold24.t_raw);
        self.fold24.t = self.blend.fold24_t.value;

        // === LEGACY: SYNC active TO engaged ===
        // Keep `active` in sync with `engaged` for backwards compatibility.
        // Hysteresis band (0.45–0.55) prevents oscillation when t hovers near threshold.
        self.fold24.active = self.fold24.engaged || if self.fold24.active {
            self.fold24.t > 0.45  // Already active: stay until t drops below 0.45
        } else {
            self.fold24.t > 0.55  // Inactive: enter when t rises above 0.55
        };

        // === WITNESS BEAT (on engagement) ===
        if self.fold24.engaged && !was_engaged {
            self.fold24.witness_timer = 0.25;
            self.fold24.aim_offset_angle = 0.0;
            self.fold24.trigger_reason = Some(match self.fold24.entry_reason {
                Fold24EntryReason::RingDive => "ring_dive_engage",
                Fold24EntryReason::TopologyRequest => "topology_request",
                _ => "engaged",
            });
        }

        // === CAMERA WITNESS BEAT ===
        if self.fold24.witness_timer > 0.0 {
            self.fold24.witness_timer -= dt;
            let witness_progress = 1.0 - (self.fold24.witness_timer / 0.25);
            self.fold24.aim_offset_angle = smoothstep(0.0, 1.0, witness_progress) * (std::f32::consts::PI / 12.0);
        } else {
            self.fold24.aim_offset_angle *= (-dt * 3.0).exp();
        }

        // Decay aim offset when not engaged
        if !self.fold24.engaged {
            self.fold24.aim_offset_angle *= (-dt * 3.0).exp();
            if self.fold24.aim_offset_angle.abs() < 0.001 {
                self.fold24.aim_offset_angle = 0.0;
            }
        }

        // === HYPERSPACE JUMP DRIVE ===
        // Sync hyperspace effect with fold24 transition
        // Now that fold24.t is timeline-driven, hyperspace will animate properly
        self.hyperspace.sync_to_fold24(self.fold24.t, self.fold24.active);
        // Update manual trigger animation (for torus exit effect)
        self.hyperspace.update(dt);
    }

    // ========================================================================
    // TOROIDAL KNOT CYMATICS - Tertiary Inside Transition
    // ========================================================================
    //
    // Triggers when 24-fold is stable and sustained. The 24 spokes curl into
    // a torus ring, then thread into a (p,q) torus knot. Nodes march along
    // the knot strand based on detected frequency.
    //
    // Subphases:
    //   CurlToTorus (0.0-0.35): 24 spokes bend into donut
    //   ThreadKnot  (0.35-0.75): Ring morphs into torus knot
    //   Stabilize   (0.75-1.0): Damped settle, nodes march
    //   Active:     Full knot rendering with marching nodes
    // ========================================================================

    /// Update torus knot cymatics state
    /// Uses TWO-STAGE LATCH system:
    ///   Stage A: 24-fold latch with hysteresis (prevents oscillation)
    ///   Stage B: Knot charge (polarity flips as bonus, not requirement)
    fn update_torus_knot(&mut self, dt: f32, dominant_pc: usize, chroma: &[f32; 12]) {
        let cfg = self.torus_knot.config.clone();

        // === COOLDOWN & REENTRY LOCKOUT ===
        if self.torus_knot.cooldown > 0.0 {
            self.torus_knot.cooldown -= dt;
            // Reset charge state during cooldown (but keep latch state)
            self.torus_knot.fold24_hold_secs = 0.0;
            self.torus_knot.charge = 0.0;
            self.torus_knot.flip_score = 0.0;
            self.torus_knot.flip_debounce_secs = 0.0;
            return;
        }

        // Reentry lockout (prevents ping-pong re-triggering)
        if self.torus_knot.reentry_lockout > 0.0 {
            self.torus_knot.reentry_lockout -= dt;
        }

        // === FIND SECONDARY PITCH CLASS (2nd highest chroma bin) ===
        let secondary_pc = {
            let mut bins: Vec<(usize, f32)> = chroma.iter().enumerate()
                .map(|(i, &v)| (i, v))
                .collect();
            bins.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            // Skip dominant, take second
            if bins.len() > 1 && bins[0].0 == dominant_pc {
                bins[1].0
            } else if !bins.is_empty() {
                bins[0].0
            } else {
                (dominant_pc + 7) % 12 // Default to fifth if no chroma
            }
        };

        // === STABILITY (derived from tension_slow) ===
        let stability = 1.0 - self.tension_slow.clamp(0.0, 1.0); // High tension = low stability
        self.torus_knot.stability = stability;

        // === UPDATE RATIO FROM INTERVAL ===
        self.torus_knot.update_ratio(dominant_pc, secondary_pc, stability);

        // === UPDATE LIVE Hz (approximate from dominant pitch class) ===
        // Map pitch class to frequency (A4 = 440Hz, A = pitch class 9)
        let semitones_from_a4 = (dominant_pc as i32 - 9).rem_euclid(12) as f32;
        self.torus_knot.live_hz = 440.0 * (2.0_f32).powf(semitones_from_a4 / 12.0);

        // ====================================================================
        // CANVAS/PAINT SYSTEM UPDATE
        // ====================================================================
        // Canvas = knot topology (rarely changes)
        // Paint = node pattern (can change more often, but quantized)
        // ====================================================================

        // === CANVAS UPDATE (topology, changes rarely) ===
        // Canvas changes based on quality tier and harmonic confidence
        // Only allow change if hold timer exceeded and not transitioning
        const CANVAS_MIN_HOLD: f32 = 3.0; // Minimum 3s before canvas can change
        const CANVAS_SWITCH_CONF: f32 = 0.7; // Need high confidence to switch

        self.torus_knot.canvas_hold_timer += dt;

        if !self.torus_knot.canvas_transitioning
            && self.torus_knot.canvas_hold_timer > CANVAS_MIN_HOLD
            && stability > CANVAS_SWITCH_CONF
        {
            // Select canvas using REG context (quality, confidence, interval, QC invariants)
            let quality_tier = self.torus_knot.quality.as_i32();
            let interval = TorusKnotState::interval_from_pitches(dominant_pc, secondary_pc);

            // REG invariants from QuaternionCharges
            let lattice_ok = self.qc.invariants.lattice_ok_24;
            let invariant_count = [
                self.qc.invariants.complements_ok,
                self.qc.invariants.total_ok,
                self.qc.invariants.palindrome_ok,
                self.qc.invariants.lattice_ok_24,
            ].iter().filter(|&&x| x).count();

            let new_canvas = TorusCanvas::for_reg_context(
                quality_tier,
                stability,
                interval,
                lattice_ok,
                invariant_count,
            );

            if new_canvas != self.torus_knot.canvas {
                self.torus_knot.canvas = new_canvas;
                self.torus_knot.canvas_hold_timer = 0.0;

                // NOTE: Canvas is now label-only! Do NOT set target_p/target_q from canvas.
                // update_ratio() is the sole driver of p,q from audio-derived IntervalRatio.
                // Compute link_count from actual p,q (gcd)
                self.torus_knot.link_count = gcd(self.torus_knot.p, self.torus_knot.q);
                // NOTE: density_level_target is now updated via snapshot mechanism in hysteresis section
            }
        }

        // === PAINT UPDATE (node pattern, more responsive but still discrete) ===
        // Paint changes based on detected interval, with cooldown
        const PAINT_MIN_HOLD: f32 = 0.5; // Minimum 500ms before paint can change
        const PAINT_COOLDOWN: f32 = 1.0; // 1s cooldown after paint change

        self.torus_knot.paint_hold_timer += dt;
        if self.torus_knot.paint_cooldown > 0.0 {
            self.torus_knot.paint_cooldown -= dt;
        }

        // Only change paint if cooldown elapsed and hold timer exceeded
        if self.torus_knot.paint_cooldown <= 0.0
            && self.torus_knot.paint_hold_timer > PAINT_MIN_HOLD
            && stability > 0.4 // Need some stability to change paint
        {
            // Compute interval from dominant to secondary
            let interval = TorusKnotState::interval_from_pitches(dominant_pc, secondary_pc);
            let new_paint = TorusPaint::from_interval(interval);

            if new_paint != self.torus_knot.paint {
                self.torus_knot.paint = new_paint;
                self.torus_knot.paint_hold_timer = 0.0;
                self.torus_knot.paint_cooldown = PAINT_COOLDOWN;
                // Bead count recalculated below via density level system
            }
        }

        // === PAINT PHASE STEPPING (12-TET wobble, quantized to 24 anchors) ===
        // Only step phase on strong beats when paint is 12-TET type
        // Step by ±1 anchor (15° = TAU/24) for controlled wobble
        if self.torus_knot.paint.is_tet() && self.torus_knot.active {
            // On kick with decent energy, step the phase
            let kick_threshold = 0.5;
            let energy_threshold = 0.3;

            // Use flip detection to trigger phase steps (polarity change = step)
            if self.torus_knot.flip_score > 0.3 && self.section_energy > energy_threshold {
                // Step direction based on interval movement
                let step_dir = if (dominant_pc as i32 - self.torus_knot.prev_dom_pc).rem_euclid(12) > 6 {
                    -1
                } else {
                    1
                };
                self.torus_knot.paint_phase_step =
                    (self.torus_knot.paint_phase_step + step_dir).rem_euclid(24);
            }
        }

        // === SPICE GATING UPDATE (cinematic reveals for 7/11-limit intervals) ===
        // Two-stage system:
        // 1. Eligibility: Only 7-limit or 11-limit ratios can have spice > 0
        // 2. Gate: Harmonic coherence + confidence must hold to activate
        // 3. Level: Steps toward target based on gate state and eligibility
        {
            // --- A) Compute chroma coherence metrics ---
            let (p_max, p2, entropy) = chroma_coherence(chroma);
            self.torus_knot.spice_last_entropy = entropy;
            self.torus_knot.spice_last_p_max = p_max;
            self.torus_knot.spice_last_p2 = p2;

            // --- A2) Confidence detection with hysteresis (K latch) ---
            // Uses MUCH lower thresholds than topology-commit (~0.65)
            // This lets stable_time charge on "pretty good" harmony
            let hc = self.stability_smooth;
            let is_confident_raw = if self.torus_knot.spice_confident_latched {
                hc >= SPICE_HC_CLOSE  // Stay latched until drops below close threshold
            } else {
                hc >= SPICE_HC_OPEN   // Open when exceeds open threshold
            };

            // Latch confidence with time hysteresis (prevents flicker)
            if is_confident_raw != self.torus_knot.spice_confident_latched {
                self.torus_knot.spice_confident_latch_timer += dt;
                if self.torus_knot.spice_confident_latch_timer >= SPICE_CONFIDENT_LATCH_TIME {
                    self.torus_knot.spice_confident_latched = is_confident_raw;
                    self.torus_knot.spice_confident_latch_timer = 0.0;
                }
            } else {
                self.torus_knot.spice_confident_latch_timer = 0.0;
            }

            let is_confident = self.torus_knot.spice_confident_latched;

            // --- B) Coherence detection with hysteresis ---
            // OPEN: ANY metric passing (OR logic)
            let can_open = p_max >= SPICE_P_MAX_OPEN || p2 >= SPICE_P2_OPEN || entropy <= SPICE_ENTROPY_OPEN;
            // CLOSE: ALL metrics must fail (AND logic) - much stricter
            let must_close = p_max < SPICE_P_MAX_CLOSE && p2 < SPICE_P2_CLOSE && entropy > SPICE_ENTROPY_CLOSE;

            // Determine raw coherence state
            let is_coherent_raw = if self.torus_knot.spice_coherence_latched {
                !must_close  // Stay latched unless ALL metrics fail
            } else {
                can_open     // Open if ANY metric passes
            };

            // Latch coherence with time hysteresis (prevents flicker)
            if is_coherent_raw != self.torus_knot.spice_coherence_latched {
                self.torus_knot.spice_coherence_latch_timer += dt;
                if self.torus_knot.spice_coherence_latch_timer >= SPICE_COHERENCE_LATCH_TIME {
                    self.torus_knot.spice_coherence_latched = is_coherent_raw;
                    self.torus_knot.spice_coherence_latch_timer = 0.0;
                }
            } else {
                self.torus_knot.spice_coherence_latch_timer = 0.0;
            }

            let is_coherent = self.torus_knot.spice_coherence_latched;

            // --- C) Compute eligibility (snapshot-based, not every frame) ---
            let (pr, qr) = reduced_pq(self.torus_knot.p, self.torus_knot.q);
            let canonical = (pr, qr);

            // Only recompute when canonical topology changes
            if canonical != self.torus_knot.spice_eligibility_snapshot {
                let limit = ratio_limit(pr, qr);
                self.torus_knot.spice_prime_limit = limit;
                self.torus_knot.spice_eligible = limit >= 7;
                self.torus_knot.spice_eligibility_snapshot = canonical;

                // If now ineligible, force target to 0 and close gate
                if !self.torus_knot.spice_eligible {
                    self.torus_knot.spice_level_target = 0;
                    self.torus_knot.spice_gate_active = false;
                }
            }

            let limit = self.torus_knot.spice_prime_limit;
            let eligible = self.torus_knot.spice_eligible;

            // --- D) Stable time accumulation/decay ---
            // Only accumulate when eligible AND confident AND coherent
            if eligible && is_confident && is_coherent {
                self.torus_knot.spice_stable_time += dt;
                self.torus_knot.spice_close_timer = 0.0;  // Reset close timer
            } else {
                // Decay faster when not coherent (2x speed)
                self.torus_knot.spice_stable_time = (self.torus_knot.spice_stable_time - dt * 2.0).max(0.0);
                // Accumulate close timer
                self.torus_knot.spice_close_timer += dt;
            }

            // --- E) Gate state machine ---
            // Open gate when stable_time reaches threshold
            if !self.torus_knot.spice_gate_active {
                if self.torus_knot.spice_stable_time >= SPICE_GATE_ON_TIME {
                    self.torus_knot.spice_gate_active = true;
                    self.torus_knot.spice_hold_timer = SPICE_LEVEL_HOLD;
                }
            }
            // Close gate when close_timer exceeds threshold (sustained instability)
            if self.torus_knot.spice_gate_active {
                if self.torus_knot.spice_close_timer >= SPICE_GATE_OFF_TIME {
                    self.torus_knot.spice_gate_active = false;
                    self.torus_knot.spice_stable_time = 0.0;  // Reset for next activation
                }
            }

            // --- F) Compute spice level target ---
            // Eligibility already computed in section C (snapshot-based)
            // Target level rules:
            // - Not eligible (5-limit or below): always target 0
            // - Eligible + gate closed: target 0
            // - Eligible + gate open + 7-limit: target 1 (maybe 2 on strong phrase)
            // - Eligible + gate open + 11-limit: target 2
            self.torus_knot.spice_level_target = if !eligible {
                0
            } else if !self.torus_knot.spice_gate_active {
                0
            } else if limit >= 11 {
                2  // 11-limit can reach full spice
            } else if limit >= 7 {
                // 7-limit gets level 1, or 2 if phrase is high + extra stability
                if self.torus_knot.phrase_env > 0.7 && is_confident && self.torus_knot.spice_stable_time > 3.0 {
                    2
                } else {
                    1
                }
            } else {
                0  // Should not reach here due to eligible check
            };

            // --- G) Step spice level toward target with hold timer ---
            self.torus_knot.spice_hold_timer = (self.torus_knot.spice_hold_timer - dt).max(0.0);
            if self.torus_knot.spice_hold_timer <= 0.0 {
                if self.torus_knot.spice_level < self.torus_knot.spice_level_target {
                    // Step UP toward target
                    self.torus_knot.spice_level += 1;
                    self.torus_knot.spice_hold_timer = SPICE_LEVEL_HOLD;
                } else if self.torus_knot.spice_level > self.torus_knot.spice_level_target {
                    // Step DOWN toward target (slightly slower)
                    self.torus_knot.spice_level -= 1;
                    self.torus_knot.spice_hold_timer = SPICE_LEVEL_HOLD * 1.2;
                }
            }

            // Debug logging disabled - spice system working
            // Uncomment to debug:
            // static mut DEBUG_TIMER: f32 = 0.0;
            // unsafe {
            //     DEBUG_TIMER += dt;
            //     if DEBUG_TIMER > 1.0 && self.torus_knot.active {
            //         DEBUG_TIMER = 0.0;
            //         let flags = format!("{}{}{}{}",
            //             if eligible { 'E' } else { 'e' },
            //             if is_coherent { 'C' } else { 'c' },
            //             if is_confident { 'K' } else { 'k' },
            //             if self.torus_knot.spice_gate_active { 'G' } else { 'g' }
            //         );
            //         println!("[Spice] L:{}→{} | {} | Lim:{} | stab:{:.2} pMax:{:.2} | t:{:.1}s",
            //             self.torus_knot.spice_level, self.torus_knot.spice_level_target,
            //             flags, limit, self.torus_knot.stability, p_max, self.torus_knot.spice_stable_time);
            //     }
            // }
        }

        // === PHRASE ENVELOPE UPDATE (density breathing with musical phrasing) ===
        // Beat-driven: detects onset events via spectral flux, modulates phrase_bias.
        // Same topology can "breathe" across sections without changing topology.
        {
            // Detect onset (beat-like event) via spectral flux threshold (very low for testing)
            let onset_detected = self.spectral_flux > 0.05;
            let is_strong_onset = self.spectral_flux > 0.1 && self.section_energy > 0.1;
            let is_weak_section = self.section_energy < 0.3 && self.stability_smooth < 0.5;

            // Accumulate "beats" over time (pseudo beat counting via onset frequency)
            if onset_detected {
                self.torus_knot.phrase_hold_beats += dt * 2.0;  // Faster accumulation on onsets
            } else {
                self.torus_knot.phrase_hold_beats += dt * 0.5;  // Slow accumulation otherwise
            }

            // Clamp beat counter to reasonable range
            self.torus_knot.phrase_hold_beats = self.torus_knot.phrase_hold_beats.min(10.0);

            // Update phrase envelope based on section characteristics
            if is_strong_onset && self.torus_knot.phrase_hold_beats >= PHRASE_MIN_HOLD_BEATS {
                // Strong onset: step envelope UP
                self.torus_knot.phrase_env = (self.torus_knot.phrase_env + PHRASE_STEP_UP).min(1.0);
                self.torus_knot.phrase_hold_beats = 0.0;  // Reset hold counter
            } else if is_weak_section && self.torus_knot.phrase_hold_beats >= PHRASE_MIN_HOLD_BEATS {
                // Weak section: step envelope DOWN
                self.torus_knot.phrase_env = (self.torus_knot.phrase_env - PHRASE_STEP_DOWN).max(0.0);
                self.torus_knot.phrase_hold_beats = 0.0;
            } else if self.torus_knot.phrase_hold_beats >= PHRASE_MIN_HOLD_BEATS * 2.0 {
                // Neutral: slow decay toward 0.5
                self.torus_knot.phrase_env += (0.5 - self.torus_knot.phrase_env) * PHRASE_DECAY_RATE;
            }

            // Derive discrete bias (only changes after hold period)
            let new_bias: i8 = if self.torus_knot.phrase_env < 0.33 {
                -1
            } else if self.torus_knot.phrase_env > 0.66 {
                1
            } else {
                0
            };

            if new_bias != self.torus_knot.phrase_bias && self.torus_knot.phrase_hold_beats >= PHRASE_MIN_HOLD_BEATS {
                self.torus_knot.phrase_bias = new_bias;
                self.torus_knot.phrase_hold_beats = 0.0;  // Reset after bias change
            }
        }

        // === ORNAMENT LAYER UPDATE (premium accents locked to symmetry) ===
        // Event-driven selection on bar-like boundaries, mutually exclusive types.
        {
            // Ornament hold timer tick
            self.torus_knot.ornament_hold_timer = (self.torus_knot.ornament_hold_timer - dt).max(0.0);

            // Detect "bar boundary" via strong onset with energy (very low for testing)
            let bar_like_event = self.spectral_flux > 0.1 && self.section_energy > 0.15;

            if bar_like_event && self.torus_knot.ornament_hold_timer <= 0.0 {
                // Compute selection scores (very low for testing)
                let high_band_energy = (chroma[8] + chroma[9] + chroma[10] + chroma[11]) /
                    (chroma.iter().sum::<f32>().max(0.001));
                let scaffold_stable = self.blend.fold24_t.value > 0.3 && self.stability_smooth > 0.1;
                let beat_strong = self.kick_impulse > 0.1 || self.spectral_flux > 0.15;

                // Priority selection (mutually exclusive) - very permissive for testing
                let new_ornament = if beat_strong && self.section_energy > 0.15 {
                    3  // Knot Halo (dramatic)
                } else if scaffold_stable {
                    2  // Harmonic Notches (structural)
                } else if high_band_energy > 0.1 {
                    1  // Radial Filigree (shimmer)
                } else {
                    0  // None
                };

                if new_ornament != self.torus_knot.ornament_id {
                    self.torus_knot.ornament_id = new_ornament;
                    self.torus_knot.ornament_hold_timer = ORNAMENT_MIN_HOLD_SECS;
                    self.torus_knot.ornament_pattern = (self.torus_knot.ornament_pattern + 1) % 3;

                    // Update spoke mask for filigree (every k-th spoke)
                    let k = 1 + self.torus_knot.ornament_pattern as u32;
                    let mut mask = 0u32;
                    for i in 0..24 {
                        if i % k == 0 {
                            mask |= 1 << i;
                        }
                    }
                    self.torus_knot.ornament_spoke_mask = mask;
                }
            }

            // Intensity quantization with hysteresis
            let raw_intensity = self.section_energy * (0.5 + self.stability_smooth * 0.5);
            let target_intensity = if raw_intensity < 0.25 {
                0.0
            } else if raw_intensity < 0.55 {
                0.5
            } else {
                1.0
            };

            if (target_intensity - self.torus_knot.ornament_intensity).abs() > 0.1 {
                self.torus_knot.ornament_intensity_timer += dt;
                if self.torus_knot.ornament_intensity_timer >= ORNAMENT_INTENSITY_HOLD {
                    self.torus_knot.ornament_intensity = target_intensity;
                    self.torus_knot.ornament_intensity_timer = 0.0;
                }
            } else {
                self.torus_knot.ornament_intensity_timer = 0.0;
            }
        }

        // === WI SECTOR ACCENT UPDATE (winding interval as sector highlights) ===
        // Brief accent flash on onset events, highlights sectors based on WI.
        {
            // Compute effective WI (wrapped to 0-360)
            let wi_raw = (self.torus_knot.p as f32 * 360.0) / self.torus_knot.q.max(1) as f32;
            let wi_eff = wi_raw.rem_euclid(360.0);

            // Quantize to active fold (use 24-fold for torus knot)
            let n = 24u8;
            let wi_eff_step = ((wi_eff / (360.0 / n as f32)).round() as u8) % n;

            // Trigger on onset (not every frame) - lowered threshold
            if self.spectral_flux > 0.25 && self.torus_knot.wi_accent_timer <= -WI_ACCENT_COOLDOWN {
                self.torus_knot.wi_accent_step = wi_eff_step;
                self.torus_knot.wi_accent_timer = WI_ACCENT_DURATION;
                self.torus_knot.wi_accent_active = true;
            }

            // Timer tick
            if self.torus_knot.wi_accent_timer > 0.0 {
                self.torus_knot.wi_accent_timer -= dt;
                if self.torus_knot.wi_accent_timer <= 0.0 {
                    self.torus_knot.wi_accent_active = false;
                }
            } else {
                self.torus_knot.wi_accent_timer -= dt;  // Goes negative for cooldown tracking
            }
        }

        // === PREMIUM 3D QUANTIZED STATE UPDATES ===
        // Beat-driven updates for camera orbit, tube radius, vortex, and lighting.
        // All params step on beat-like events, not continuous drift.
        // NOTE: This block only affects visual parameters AFTER torus is active,
        // it does NOT affect the entry/exit logic.
        {
            let cfg = &self.torus_knot.config;

            // --- STABILITY GATE (only for premium visual updates, not entry logic) ---
            // DISABLED: Was too aggressive and causing confusion.
            // The entry logic has its own stability checks via can_charge conditions.
            self.torus_knot.state_frozen = false;  // Never freeze for now

            // Tick hold timers even when frozen (time still passes)
            self.torus_knot.camera_orbit_hold_timer = (self.torus_knot.camera_orbit_hold_timer - dt).max(0.0);
            self.torus_knot.tube_radius_hold_timer = (self.torus_knot.tube_radius_hold_timer - dt).max(0.0);
            self.torus_knot.vortex_hold_timer = (self.torus_knot.vortex_hold_timer - dt).max(0.0);

            // Only update state when not frozen
            if !self.torus_knot.state_frozen && self.torus_knot.active {
                // Convert beats to seconds (assume ~120 BPM = 0.5s per beat)
                let beat_to_secs = |beats: f32| beats * 0.5;

                // Detect beat-like events
                let onset_strong = self.spectral_flux > 0.15;
                let kick_detected = self.kick_impulse > 0.15;

                // --- CAMERA ORBIT (steps on strong onsets, locked to 24-fold) ---
                if onset_strong && self.torus_knot.camera_orbit_hold_timer <= 0.0 {
                    // Step direction based on energy change
                    let step_dir = if self.section_energy > 0.5 { 1 } else { 0 };
                    self.torus_knot.camera_orbit_step = (self.torus_knot.camera_orbit_step + step_dir).rem_euclid(24);
                    self.torus_knot.camera_orbit_hold_timer = beat_to_secs(cfg.camera_step_min_beats);
                }

                // --- TUBE RADIUS (steps on kick) ---
                if kick_detected && self.torus_knot.tube_radius_hold_timer <= 0.0 {
                    // Map energy to bucket [0..3]
                    let target = if self.section_energy < 0.25 {
                        0  // thin
                    } else if self.section_energy < 0.50 {
                        1  // normal
                    } else if self.section_energy < 0.75 {
                        2  // thick
                    } else {
                        3  // bold
                    };
                    self.torus_knot.tube_radius_step = target;
                    self.torus_knot.tube_radius_hold_timer = beat_to_secs(cfg.tube_step_min_beats);
                }

                // --- VORTEX STRENGTH (steps on onset) ---
                if onset_strong && self.torus_knot.vortex_hold_timer <= 0.0 {
                    // Lock vortex fold to p value
                    self.torus_knot.vortex_fold_n = self.torus_knot.p;
                    // Quantize to thirds
                    self.torus_knot.vortex_strength_latched = if self.section_energy < 0.33 {
                        0.0
                    } else if self.section_energy < 0.66 {
                        0.33
                    } else {
                        0.66
                    };
                    self.torus_knot.vortex_hold_timer = beat_to_secs(cfg.vortex_step_min_beats);
                }

                // --- LIGHTING LATCHES (step on any onset) ---
                if onset_strong {
                    // High band energy for rim
                    let high_band = (chroma[8] + chroma[9] + chroma[10] + chroma[11]) /
                        (chroma.iter().sum::<f32>().max(0.001));
                    self.torus_knot.rim_gain_latched = if high_band < 0.25 {
                        0.0
                    } else if high_band < 0.5 {
                        0.5
                    } else {
                        1.0
                    };

                    // Depth separation inversely related to tension
                    let depth_factor = 1.0 - self.tension_slow;
                    self.torus_knot.depth_sep_latched = if depth_factor < 0.33 {
                        0.0
                    } else if depth_factor < 0.66 {
                        0.5
                    } else {
                        1.0
                    };

                    // Spec boost on kick
                    self.torus_knot.spec_boost_latched = if kick_detected { 1.0 } else { 0.0 };
                }
            }
        }

        // === DENSITY LEVEL HYSTERESIS ===
        // Smoothly step density level toward target, with hold timer for stability.
        // NOW INTEGRATES: spice_level (for bucket) and phrase_bias (for target modulation).
        // IMPORTANT: Only update density_level_target when topology commits (p,q changes)
        // to prevent jitter from flickering candidates.
        {
            let quality_tier = self.torus_knot.quality.as_i32();
            let max_level = max_density_level(quality_tier);

            // Compare against REDUCED (pR,qR) to avoid spurious updates on equivalent ratios
            // e.g., 6:4 and 3:2 both reduce to (3,2) - should not trigger density change
            let (pr, qr) = reduced_pq(self.torus_knot.p, self.torus_knot.q);
            let current_reduced = (pr, qr);
            if current_reduced != self.torus_knot.density_pq_snapshot {
                // Topology has committed to genuinely different reduced values

                // === SPICE ELIGIBILITY: Only 7-limit or 11-limit ratios can be "spicy" ===
                let limit = ratio_limit(pr, qr);
                self.torus_knot.spice_prime_limit = limit;
                self.torus_knot.spice_eligible = limit >= 7;

                // If topology changed to ineligible, start stepping spice down immediately
                if !self.torus_knot.spice_eligible {
                    self.torus_knot.spice_level_target = 0;
                    // Close gate since we're not eligible anymore
                    self.torus_knot.spice_gate_active = false;
                }

                // Use spice_level for gated boost (cinematic reveals)
                let bucket = complexity_bucket(pr, qr, self.torus_knot.spice_level);
                // Apply phrase_bias for density breathing
                let biased = (bucket as i8 + self.torus_knot.phrase_bias).clamp(0, max_level as i8) as u8;
                self.torus_knot.density_level_target = biased;
                self.torus_knot.density_pq_snapshot = current_reduced;  // Store reduced form
            } else {
                // Topology unchanged, but phrase_bias may have changed - update target
                let bucket = complexity_bucket(pr, qr, self.torus_knot.spice_level);
                let biased = (bucket as i8 + self.torus_knot.phrase_bias).clamp(0, max_level as i8) as u8;
                self.torus_knot.density_level_target = biased;
            }

            // TIER CHANGE DETECTION: reset hold timer on any tier change
            // Prevents "instant step" from timer that was near threshold under previous tier
            let tier_changed = quality_tier != self.torus_knot.density_prev_tier;
            if tier_changed {
                self.torus_knot.density_hold_timer = 0.0;
                self.torus_knot.density_prev_tier = quality_tier;
            }

            // TIER CLAMP: if tier dropped, clamp level immediately (safe, not stepwise)
            // This prevents density_level from exceeding new max_level after tier downgrade
            // Note: tier upgrades let hysteresis step upward naturally (keeps stability)
            if self.torus_knot.density_level > max_level {
                self.torus_knot.density_level = max_level;
                // Also update target to match new tier cap
                self.torus_knot.density_level_target = self.torus_knot.density_level_target.min(max_level);
            }

            // Hysteresis: only step level when target differs and hold timer exceeded
            let delta = (self.torus_knot.density_level_target as i8 - self.torus_knot.density_level as i8).abs();
            if delta >= 1 {
                self.torus_knot.density_hold_timer += dt;
                if self.torus_knot.density_hold_timer >= DENSITY_HOLD_TIME {
                    // Step by ±1 level toward target
                    if self.torus_knot.density_level_target > self.torus_knot.density_level {
                        self.torus_knot.density_level += 1;
                    } else {
                        self.torus_knot.density_level = self.torus_knot.density_level.saturating_sub(1);
                    }
                    self.torus_knot.density_hold_timer = 0.0;
                }
            } else {
                // Target matches current, reset hold timer
                self.torus_knot.density_hold_timer = 0.0;
            }

            // Compute bead count: base from level, then apply paint multiplier, then tier clamp
            let base_beads = beads_for_level(self.torus_knot.density_level, quality_tier);
            let multiplier = self.torus_knot.paint.bead_multiplier();
            let unclamped = base_beads * multiplier;
            // EXPLICIT TIER CLAMP: paint multiplier must not exceed tier cap
            let tier_max_beads = beads_for_level(max_level, quality_tier);
            self.torus_knot.bead_count = unclamped.min(tier_max_beads);

            // Debug overlay (when enabled)
            if self.torus_knot.debug_overlay_enabled {
                let max_pq = pr.max(qr);
                let boost = spice_boost(max_pq, self.torus_knot.spice_level);
                log::info!(
                    "[Density] T({}:{}) | {}→{} | {}×{}→{} | spice({},{}×) | phrase({:.0}%,{:+}) | orn({},{:.1}) | wi={}",
                    self.torus_knot.p, self.torus_knot.q,
                    complexity_bucket(pr, qr, self.torus_knot.spice_level), self.torus_knot.density_level,
                    base_beads, multiplier, self.torus_knot.bead_count,
                    self.torus_knot.spice_level, boost,
                    self.torus_knot.phrase_env * 100.0, self.torus_knot.phrase_bias,
                    self.torus_knot.ornament_id, self.torus_knot.ornament_intensity,
                    self.torus_knot.wi_accent_step
                );
            }
        }

        // === UPDATE P/Q SMOOTH (for rendering) ===
        // Smoothly interpolate p,q for visual continuity
        let pq_smooth_rate = 2.0; // Smooth over ~0.5s
        let pq_alpha = 1.0 - (-dt * pq_smooth_rate).exp();
        self.torus_knot.p_smooth += (self.torus_knot.target_p as f32 - self.torus_knot.p_smooth) * pq_alpha;
        self.torus_knot.q_smooth += (self.torus_knot.target_q as f32 - self.torus_knot.q_smooth) * pq_alpha;

        // Latch p,q when active (no continuous morphing)
        if self.torus_knot.active && self.torus_knot.subphase == TorusKnotSubphase::Active {
            self.torus_knot.p = self.torus_knot.target_p;
            self.torus_knot.q = self.torus_knot.target_q;
        }

        // === FLIP DEBOUNCE TICK ===
        if self.torus_knot.flip_debounce_secs > 0.0 {
            self.torus_knot.flip_debounce_secs -= dt;
        }

        // === SILENCE/INSTABILITY DETECTION ===
        let is_silent = self.section_energy < cfg.silence_threshold;
        // DISABLED: Stability check commented out for testing
        // let is_unstable = stability < cfg.stability_required;
        let is_unstable = false;

        if !self.torus_knot.active {
            // =================================================================
            // TWO-STAGE LATCH: Stage A + Stage B
            // =================================================================

            // === STAGE A: 24-fold latch with hysteresis ===
            // This prevents oscillation when fold24.t hovers near a threshold
            let above_enter = self.fold24.active && self.fold24.t >= cfg.fold24_enter_t;
            let below_exit = !self.fold24.active || self.fold24.t <= cfg.fold24_exit_t;

            if !self.torus_knot.fold24_latched {
                // Not latched - try to latch ON
                if above_enter {
                    self.torus_knot.fold24_enter_hold_s += dt;
                    if self.torus_knot.fold24_enter_hold_s >= cfg.fold24_enter_hold {
                        // LATCH ON
                        self.torus_knot.fold24_latched = true;
                        self.torus_knot.fold24_enter_hold_s = 0.0;
                        self.torus_knot.fold24_exit_hold_s = 0.0;
                        log::info!("[TorusKnot] 24-fold LATCHED (enter_hold satisfied, fold24.t={:.2})", self.fold24.t);
                    }
                } else {
                    // Reset enter hold if we drop below threshold
                    self.torus_knot.fold24_enter_hold_s = 0.0;
                }
            } else {
                // Latched - try to unlatch (exit)
                if below_exit {
                    self.torus_knot.fold24_exit_hold_s += dt;
                    if self.torus_knot.fold24_exit_hold_s >= cfg.fold24_exit_hold {
                        // UNLATCH (exit)
                        self.torus_knot.fold24_latched = false;
                        self.torus_knot.fold24_exit_hold_s = 0.0;
                        self.torus_knot.fold24_hold_secs = 0.0; // Reset hold timer
                        log::info!("[TorusKnot] 24-fold UNLATCHED (exit_hold satisfied, fold24.t={:.2})", self.fold24.t);
                    }
                } else {
                    // Reset exit hold if we rise above threshold
                    self.torus_knot.fold24_exit_hold_s = 0.0;
                }
            }

            // === Accumulate latched hold time ===
            if self.torus_knot.fold24_latched {
                self.torus_knot.fold24_hold_secs += dt;
            }
            // Note: fold24_hold_secs is NOT reset when unlatched - soft persistence

            // === STAGE B: Knot charge accumulation ===
            // Charge when: latched AND harmonic_conf >= threshold
            // Use stability_smooth as proxy for harmonic confidence (high stability = clear tonal content)
            let harmonic_conf = self.stability_smooth;
            // Balance and tension qualifiers for torus mode entry:
            // - Balance > 40%: need reasonably balanced harmonic content
            // - Tension < 65%: need relatively stable harmonic field
            let balance_ok = self.balance > 0.40;
            let tension_ok = self.tension_slow < 0.65;
            let can_charge = self.torus_knot.fold24_latched
                && harmonic_conf >= cfg.harmonic_conf_required
                && self.section_energy >= cfg.energy_required
                && balance_ok
                && tension_ok;

            // === POLARITY FLIP DETECTION (bonus, not requirement) ===
            let dom_pc_i32 = dominant_pc as i32;
            if self.torus_knot.prev_dom_pc >= 0 && self.torus_knot.flip_debounce_secs <= 0.0 {
                let dist_raw = (dom_pc_i32 - self.torus_knot.prev_dom_pc).abs();
                let cyclic_dist = dist_raw.min(12 - dist_raw);

                if cyclic_dist >= cfg.flip_distance && !is_unstable {
                    // Boost flip_score (capped at 1.0)
                    self.torus_knot.flip_score = (self.torus_knot.flip_score + 0.5).min(1.0);
                    self.torus_knot.flip_debounce_secs = 0.12;
                    log::trace!("[TorusKnot] Polarity flip detected: pc {} → {} (dist={}), flip_score={:.2}",
                        self.torus_knot.prev_dom_pc, dom_pc_i32, cyclic_dist, self.torus_knot.flip_score);
                }
            }
            self.torus_knot.prev_dom_pc = dom_pc_i32;

            // Decay flip_score over time (exponential decay)
            let flip_decay_alpha = 1.0 - (-dt / cfg.flip_decay_tau).exp();
            self.torus_knot.flip_score *= 1.0 - flip_decay_alpha;

            // === CHARGE INTEGRATION ===
            if can_charge {
                // Base charge rate + flip bonus
                let flip_bonus = self.torus_knot.flip_score * cfg.flip_bonus_rate;
                self.torus_knot.charge += dt * (cfg.charge_rate + flip_bonus);
            } else if is_silent && cfg.freeze_on_silence {
                // FREEZE charge during silence (don't decay)
            } else if is_unstable {
                // Soft decay on instability (NOT hard reset to 0)
                self.torus_knot.charge -= dt * cfg.decay_rate * 0.5; // Half-rate decay
            } else {
                // Normal decay when conditions not met
                self.torus_knot.charge -= dt * cfg.decay_rate;
            }
            self.torus_knot.charge = self.torus_knot.charge.clamp(0.0, 1.0);

            // === EMERGENCE STATE MACHINE ===
            // Phases: Idle → Candidate → Prewarm → Active
            // Decision is O(1), no allocations, no dual-render
            let charge_full = self.torus_knot.charge >= 1.0;
            let hold_satisfied = self.torus_knot.fold24_hold_secs >= cfg.charge_min_hold;
            let lockout_clear = self.torus_knot.reentry_lockout <= 0.0;
            let conditions_met = charge_full && hold_satisfied && self.torus_knot.fold24_latched && lockout_clear;

            // Update perf pressure slow EMA (for admission gate)
            let pressure_target = if self.torus_knot.admission_frame_ms > 16.0 {
                ((self.torus_knot.admission_frame_ms - 16.0) / 8.0).min(1.0)
            } else {
                0.0
            };
            let pressure_alpha = 1.0 - (-dt / 0.5).exp();
            self.torus_knot.perf_pressure_slow = lerp(self.torus_knot.perf_pressure_slow, pressure_target, pressure_alpha);

            // Update emergence state time
            self.torus_knot.emerge_state_time += dt;

            match self.torus_knot.emerge_state {
                EmergenceState::Idle => {
                    // === IDLE → CANDIDATE (hysteresis + hold) ===
                    // Only latch intent on sustained conditions
                    if conditions_met {
                        self.torus_knot.emerge_state_time += dt; // Count hold time
                        let candidate_hold = 0.35; // Require 350ms of conditions before candidate
                        if self.torus_knot.emerge_state_time >= candidate_hold {
                            log::debug!("[TorusKnot] Idle → Candidate (conditions held {:.2}s)",
                                self.torus_knot.emerge_state_time);
                            self.torus_knot.emerge_state = EmergenceState::Candidate;
                            self.torus_knot.emerge_state_time = 0.0;
                        }
                    } else {
                        self.torus_knot.emerge_state_time = 0.0; // Reset hold timer
                    }
                }

                EmergenceState::Candidate => {
                    // === CANDIDATE: Cheap preview cue, waiting for admission ===
                    // NO active=true here - just visual hints in shader via u_emerge_candidate

                    // Exit if conditions collapse
                    if !conditions_met && self.torus_knot.emerge_state_time > 0.5 {
                        log::debug!("[TorusKnot] Candidate → Idle (conditions lost)");
                        self.torus_knot.emerge_state = EmergenceState::Idle;
                        self.torus_knot.emerge_state_time = 0.0;
                    } else {
                        // === ADMISSION CONTROL: Candidate → Prewarm ===
                        // Only advance if GPU has budget
                        let has_budget = self.torus_knot.perf_pressure_slow < 0.4;
                        let pressure_stable = self.torus_knot.emerge_state_time > 0.3;

                        if has_budget && pressure_stable {
                            log::info!("[TorusKnot] Candidate → Prewarm (pressure={:.2}, frame_ms={:.1})",
                                self.torus_knot.perf_pressure_slow, self.torus_knot.admission_frame_ms);
                            self.torus_knot.emerge_state = EmergenceState::Prewarm;
                            self.torus_knot.emerge_state_time = 0.0;
                            self.torus_knot.prewarm_progress = 0.0;
                            self.torus_knot.prewarm_frames = 0;

                            // Prepare state (but DON'T set active=true yet!)
                            self.torus_knot.seed_phase = self.fold24.aim_offset_angle;
                            self.torus_knot.ref_hz = self.torus_knot.live_hz;
                            self.torus_knot.p = self.torus_knot.target_p;
                            self.torus_knot.q = self.torus_knot.target_q;
                            self.torus_knot.p_smooth = self.torus_knot.target_p as f32;
                            self.torus_knot.q_smooth = self.torus_knot.target_q as f32;
                        }
                    }
                }

                EmergenceState::Prewarm => {
                    // === PREWARM: Invisible prep, budgeted multi-frame ===
                    // active=false, no torus shader work yet
                    self.torus_knot.prewarm_frames += 1;

                    // Budgeted progress (simulate prep work over ~3-4 frames)
                    let budget_per_frame = 0.35; // ~3 frames to complete
                    self.torus_knot.prewarm_progress = (self.torus_knot.prewarm_progress + budget_per_frame).min(1.0);

                    // Abort if conditions collapse during prewarm
                    if !conditions_met && self.torus_knot.emerge_state_time > 0.4 {
                        log::debug!("[TorusKnot] Prewarm → Idle (conditions collapsed)");
                        self.torus_knot.emerge_state = EmergenceState::Idle;
                        self.torus_knot.emerge_state_time = 0.0;
                    } else {
                        // === BEAT-COMMIT: Prewarm → Active ===
                        // Only commit when prewarm complete AND on beat
                        let prewarm_ready = self.torus_knot.prewarm_progress >= 1.0
                            && self.torus_knot.prewarm_frames >= 3;
                        let beat_now = self.kick_impulse > 0.15;
                        let timeout = self.torus_knot.prewarm_frames > 30;

                        if prewarm_ready && (beat_now || timeout) {
                            log::info!("[TorusKnot] *** COMMIT *** Prewarm → Active (frames={}, beat={}, p:q={}:{})",
                                self.torus_knot.prewarm_frames, beat_now,
                                self.torus_knot.target_p, self.torus_knot.target_q);

                            // === SINGLE FLIP - no allocations, no dual render ===
                            self.torus_knot.emerge_state = EmergenceState::Active;
                            self.torus_knot.emerge_state_time = 0.0;
                            self.torus_knot.active = true;
                            self.torus_knot.t = 0.001; // Start animation
                            self.torus_knot.subphase = TorusKnotSubphase::CurlToTorus;
                            self.torus_knot.active_time = 0.0;
                            self.torus_knot.support = 1.0;
                            self.torus_knot.min_on_timer = cfg.torus_min_on_secs;

                            // === SYNC TOPOLOGY ROUTER ===
                            // Update router to track TorusKnot activation from emergence system
                            if self.topology_router.topology_id != TopologyId::TorusKnotCymatics {
                                self.topology_router.topology_prev_id = self.topology_router.topology_id;
                                self.topology_router.topology_id = TopologyId::TorusKnotCymatics;
                                self.topology_router.topology_hold_s = 0.0;
                                log::debug!("[TopologyRouter] Synced to TorusKnotCymatics from emergence");
                            }
                            self.torus_knot.exit_grace_timer = 0.0;
                            self.torus_knot.hard_drop_timer = 0.0;
                        }
                    }
                }

                EmergenceState::Active => {
                    // Handled in the else branch below (active sustain)
                }
            }
        } else {
            // =================================================================
            // TORUS ACTIVE: Perceptual Sustain Model
            // =================================================================

            // Track active time for logging
            self.torus_knot.active_time += dt;

            // Decrement min-on timer
            self.torus_knot.min_on_timer = (self.torus_knot.min_on_timer - dt).max(0.0);

            // === SUPPORT SCORE COMPUTATION (human hearing model) ===
            // Support is driven by slow envelopes, NOT raw bands or 24-fold purity
            let harmonic_conf = self.stability_smooth; // Stability = harmonic confidence proxy
            let support_target = (
                cfg.support_w_energy * self.section_energy +
                cfg.support_w_stability * stability +
                cfg.support_w_harmonic * harmonic_conf -
                cfg.support_w_tension * self.tension_slow
            ).clamp(0.0, 1.0);

            // Asymmetric update: attack faster than release (sticky feel)
            if support_target > self.torus_knot.support {
                self.torus_knot.support += dt * cfg.support_attack;
            } else {
                self.torus_knot.support -= dt * cfg.support_release;
            }
            self.torus_knot.support = self.torus_knot.support.clamp(0.0, 1.0);

            // === POLARITY FLIP DETECTION (boosts support, extends stay) ===
            let dom_pc_i32 = dominant_pc as i32;
            if self.torus_knot.prev_dom_pc >= 0 && self.torus_knot.flip_debounce_secs <= 0.0 {
                let dist_raw = (dom_pc_i32 - self.torus_knot.prev_dom_pc).abs();
                let cyclic_dist = dist_raw.min(12 - dist_raw);

                if cyclic_dist >= cfg.flip_distance {
                    // Polarity flip - boost support and reset min-on timer
                    log::trace!("[TorusKnot] Active polarity flip: pc {} → {} (dist={}), boosting support",
                        self.torus_knot.prev_dom_pc, dom_pc_i32, cyclic_dist);
                    self.torus_knot.support = (self.torus_knot.support + 0.3).min(1.0);
                    self.torus_knot.min_on_timer = (self.torus_knot.min_on_timer).max(cfg.torus_min_on_secs * 0.5);
                    self.torus_knot.flip_debounce_secs = 0.12;
                }
            }
            self.torus_knot.prev_dom_pc = dom_pc_i32;

            // === TRANSITION ANIMATION ===
            let total_duration = cfg.curl_duration + cfg.thread_duration + cfg.stabilize_duration;
            let curl_end = cfg.curl_duration / total_duration;
            let thread_end = (cfg.curl_duration + cfg.thread_duration) / total_duration;

            // Determine subphase from t
            let new_subphase = if self.torus_knot.t < curl_end {
                TorusKnotSubphase::CurlToTorus
            } else if self.torus_knot.t < thread_end {
                TorusKnotSubphase::ThreadKnot
            } else if self.torus_knot.t < 1.0 {
                TorusKnotSubphase::Stabilize
            } else {
                TorusKnotSubphase::Active
            };

            if new_subphase != self.torus_knot.subphase {
                log::info!("[TorusKnot] Phase: {} → {} (t={:.2})",
                    self.torus_knot.subphase.name(), new_subphase.name(), self.torus_knot.t);
                self.torus_knot.subphase = new_subphase;
            }

            // Advance t toward 1.0 (then stay in Active)
            if self.torus_knot.t < 1.0 {
                let rate = 1.0 / total_duration;
                self.torus_knot.t = (self.torus_knot.t + rate * dt).min(1.0);
            }

            // === PHASE ADVANCE (marching nodes, scaled by support) ===
            let freq_ratio = self.torus_knot.live_hz / self.torus_knot.ref_hz.max(1.0);
            // Cap motion speed based on support to avoid frenzy when support is low
            let motion_scale = 0.7 + 0.3 * self.torus_knot.support;
            self.torus_knot.phase_advance += dt * cfg.march_speed * freq_ratio * motion_scale;
            self.torus_knot.phase_advance = self.torus_knot.phase_advance.rem_euclid(1.0);

            // === SMOOTH P,Q INTERPOLATION ===
            let pq_tau = 0.8;
            let pq_alpha = 1.0 - (-dt / pq_tau).exp();
            self.torus_knot.p_smooth += (self.torus_knot.target_p as f32 - self.torus_knot.p_smooth) * pq_alpha;
            self.torus_knot.q_smooth += (self.torus_knot.target_q as f32 - self.torus_knot.q_smooth) * pq_alpha;

            if (self.torus_knot.p_smooth - self.torus_knot.target_p as f32).abs() < 0.1 {
                self.torus_knot.p = self.torus_knot.target_p;
            }
            if (self.torus_knot.q_smooth - self.torus_knot.target_q as f32).abs() < 0.1 {
                self.torus_knot.q = self.torus_knot.target_q;
            }

            // === VIDEO FADE (modulated by support) ===
            let base_fade = match self.torus_knot.subphase {
                TorusKnotSubphase::CurlToTorus => {
                    let sub_t = self.torus_knot.t / curl_end;
                    1.0 - (1.0 - cfg.video_fade_min) * sub_t
                }
                TorusKnotSubphase::ThreadKnot => cfg.video_fade_min,
                TorusKnotSubphase::Stabilize => {
                    let sub_t = (self.torus_knot.t - thread_end) / (1.0 - thread_end);
                    cfg.video_fade_min + (0.5 - cfg.video_fade_min) * sub_t
                }
                TorusKnotSubphase::Active => 0.5,
                TorusKnotSubphase::Idle => 1.0,
            };
            // Fade more when support is low (prepares for exit)
            self.torus_knot.video_fade = base_fade + (1.0 - base_fade) * (1.0 - self.torus_knot.support) * 0.3;

            // =================================================================
            // EXIT CONDITIONS (Perceptual Sustain Model)
            // =================================================================
            // Exit is SLOW and DECISIVE - requires sustained low support OR hard drop
            // EXCEPTION: Very high tension (100%) can abort during ANY subphase

            // === VERY HIGH TENSION ABORT (works in ANY subphase) ===
            // Use SMOOTHED chroma for consistent tension calculation
            // Tension = sum of positive chroma deviations from mean
            let chroma_sum: f32 = self.smoothed_chroma.iter().sum();
            let chroma_mean = if chroma_sum > 0.01 { chroma_sum / 12.0 } else { 0.0 };
            let chroma_tension: f32 = self.smoothed_chroma.iter()
                .map(|&c| (c - chroma_mean).max(0.0))
                .sum();
            // Normalize to 0-1 (max ~1.0 when single note dominant)
            let ui_tension = (chroma_tension / 1.0).min(1.0);

            // Exit thresholds
            const TENSION_EXIT_THRESHOLD: f32 = 0.95;
            const EXTREME_TENSION_THRESHOLD: f32 = 0.98;  // Exit regardless of energy
            const ENERGY_EXIT_THRESHOLD: f32 = 0.75;
            const LOW_ENERGY_THRESHOLD: f32 = 0.10;  // Exit if energy <= 10%

            // Check for exit conditions:
            // 1. Energy <= 10% (silence/very low) OR
            // 2. Extreme tension (>=98%) always exits OR
            // 3. High tension (>=95%) AND low energy (<75%)
            let low_energy = self.section_energy <= LOW_ENERGY_THRESHOLD;
            let extreme_tension = ui_tension >= EXTREME_TENSION_THRESHOLD;
            let tension_energy_exit = ui_tension >= TENSION_EXIT_THRESHOLD
                && self.section_energy < ENERGY_EXIT_THRESHOLD;
            let should_exit = low_energy || extreme_tension || tension_energy_exit;

            // Debug: log energy when low
            if self.section_energy < 0.20 {
                log::info!("[TorusKnot] Low energy detected: section_energy={:.1}%, threshold={:.1}%, will_exit={}",
                    self.section_energy * 100.0, LOW_ENERGY_THRESHOLD * 100.0, low_energy);
            }

            if should_exit {
                // Accumulate abort timer
                self.torus_knot.exit_grace_timer += dt;
                if low_energy {
                    log::debug!("[TorusKnot] Low energy! energy={:.1}%, timer={:.3}",
                        self.section_energy * 100.0, self.torus_knot.exit_grace_timer);
                } else if extreme_tension {
                    log::debug!("[TorusKnot] Extreme tension! tension={:.0}%, timer={:.3}",
                        ui_tension * 100.0, self.torus_knot.exit_grace_timer);
                } else {
                    log::debug!("[TorusKnot] Exit conditions met! tension={:.0}%, energy={:.0}%, timer={:.3}",
                        ui_tension * 100.0, self.section_energy * 100.0, self.torus_knot.exit_grace_timer);
                }
                if self.torus_knot.exit_grace_timer >= 0.25 {
                    let reason = if low_energy {
                        format!("zero energy ({:.1}%)", self.section_energy * 100.0)
                    } else if extreme_tension {
                        format!("extreme tension ({:.0}%)", ui_tension * 100.0)
                    } else {
                        format!("high tension ({:.0}%) + low energy ({:.0}%)", ui_tension * 100.0, self.section_energy * 100.0)
                    };
                    log::info!("[TorusKnot] ABORT due to {} during {:?}", reason, self.torus_knot.subphase);

                    // Trigger jumpdrive effect for cool exit
                    self.hyperspace.trigger();

                    self.torus_knot.active = false;
                    self.torus_knot.t = 0.0;
                    self.torus_knot.subphase = TorusKnotSubphase::Idle;
                    self.torus_knot.video_fade = 1.0;
                    self.torus_knot.reentry_lockout = cfg.reentry_lockout_secs;
                    self.torus_knot.charge = 0.0;
                    self.torus_knot.fold24_hold_secs = 0.0;
                    self.torus_knot.emerge_state = EmergenceState::Idle;
                    self.torus_knot.emerge_state_time = 0.0;
                    self.torus_knot.prewarm_progress = 0.0;
                    self.torus_knot.prewarm_frames = 0;
                    self.torus_knot.flip_score = 0.0;
                    self.torus_knot.support = 0.0;
                    self.torus_knot.exit_grace_timer = 0.0;
                    self.torus_knot.hard_drop_timer = 0.0;
                    self.torus_knot.min_on_timer = 0.0;
                    self.torus_knot.active_time = 0.0;

                    // === SYNC TOPOLOGY ROUTER ===
                    // TorusKnot deactivated - return to Scaffold24
                    if self.topology_router.topology_id == TopologyId::TorusKnotCymatics {
                        self.topology_router.topology_prev_id = TopologyId::TorusKnotCymatics;
                        self.topology_router.topology_id = TopologyId::Scaffold24;
                        self.topology_router.topology_hold_s = 0.0;
                        log::debug!("[TopologyRouter] Synced to Scaffold24 from TorusKnot exit");
                    }
                }
            } else {
                // Reset abort timer when conditions are good
                // (only reset if not in Active subphase, which has its own timer logic)
                if self.torus_knot.subphase != TorusKnotSubphase::Active {
                    self.torus_knot.exit_grace_timer = 0.0;
                }
            }

            // Skip remaining exit logic if we just aborted
            if !self.torus_knot.active {
                return;
            }

            if self.torus_knot.subphase == TorusKnotSubphase::Active {
                let stay_ok = self.torus_knot.support >= cfg.stay_threshold;
                let hard_drop = self.fold24.t < cfg.fold24_hard_drop_t;

                // === HARD DROP CHECK (24-fold completely collapses) ===
                if hard_drop {
                    self.torus_knot.hard_drop_timer += dt;
                } else {
                    self.torus_knot.hard_drop_timer = 0.0;
                }

                // === EXIT DECISION ===
                // ONLY high tension (>95%) can exit torus (handled by abort path above)
                // All other exit conditions DISABLED
            }
        }
    }

    /// Update torus knot quality based on frame time (auto-scale)
    pub fn update_torus_knot_quality(&mut self, frame_time_ms: f32, dt: f32) {
        if !self.torus_knot.active {
            self.torus_knot.quality_downgrade_time = 0.0;
            self.torus_knot.quality_upgrade_time = 0.0;
            return;
        }

        // Downgrade if frame time > 20ms sustained
        if frame_time_ms > 20.0 {
            self.torus_knot.quality_downgrade_time += dt;
            self.torus_knot.quality_upgrade_time = 0.0;

            if self.torus_knot.quality_downgrade_time > 1.5 {
                let old_quality = self.torus_knot.quality;
                self.torus_knot.quality = match self.torus_knot.quality {
                    TorusKnotQuality::High => TorusKnotQuality::Med,
                    TorusKnotQuality::Med => TorusKnotQuality::Low,
                    TorusKnotQuality::Low => TorusKnotQuality::Low,
                };
                if self.torus_knot.quality != old_quality {
                    log::info!("[TorusKnot] Quality downgrade: {:?} → {:?} ({}ms)",
                        old_quality, self.torus_knot.quality, frame_time_ms);
                }
                self.torus_knot.quality_downgrade_time = 0.0;
            }
        }
        // Upgrade if frame time < 14ms sustained
        else if frame_time_ms < 14.0 {
            self.torus_knot.quality_upgrade_time += dt;
            self.torus_knot.quality_downgrade_time = 0.0;

            if self.torus_knot.quality_upgrade_time > 3.0 {
                let old_quality = self.torus_knot.quality;
                self.torus_knot.quality = match self.torus_knot.quality {
                    TorusKnotQuality::Low => TorusKnotQuality::Med,
                    TorusKnotQuality::Med => TorusKnotQuality::High,
                    TorusKnotQuality::High => TorusKnotQuality::High,
                };
                if self.torus_knot.quality != old_quality {
                    log::info!("[TorusKnot] Quality upgrade: {:?} → {:?} ({}ms)",
                        old_quality, self.torus_knot.quality, frame_time_ms);
                }
                self.torus_knot.quality_upgrade_time = 0.0;
            }
        } else {
            // In acceptable range, slowly decay both timers
            self.torus_knot.quality_downgrade_time =
                (self.torus_knot.quality_downgrade_time - dt * 0.5).max(0.0);
            self.torus_knot.quality_upgrade_time =
                (self.torus_knot.quality_upgrade_time - dt * 0.5).max(0.0);
        }
    }

    /// Get debug string for torus knot state
    pub fn torus_knot_debug_string(&self) -> String {
        self.torus_knot.debug_string(self.fold24.t)
    }

    // ========================================================================
    // QUADRIPOLAR TRAVEL - REG Icositetragon Journey
    // ========================================================================

    /// Update quadripolar travel state
    /// Triggers during RingDive guided phase when ready, based on tension
    fn update_quadripolar(&mut self, dt: f32, dominant_pc: usize) {
        // Update cooldown and state machine
        self.quadripolar.update(dt);

        // === TRIGGER CONDITIONS ===
        // Only trigger during RingDive guided_flight phase when:
        // 1. Quadripolar is ready (not active, cooldown expired)
        // 2. RingDive is in guided phase (t > 0.6)
        // 3. High tension (musical intensity)
        // 4. Sufficient time has passed since RingDive started

        let in_guided = self.ring_dive.active && self.ring_dive.t > 0.55 && self.ring_dive.t < 0.85;
        let high_tension = self.tension_slow > 0.6;
        let rd_time_ok = self.ring_dive.guided_time > 1.5; // Give RingDive guided phase time to settle

        if self.quadripolar.is_ready() && in_guided && high_tension && rd_time_ok {
            // Start journey from dominant pitch class
            self.quadripolar.start_from_pitch(dominant_pc);
            log::info!("[QuadripolarTravel] Triggered during RingDive guided phase (tension={:.2})",
                self.tension_slow);
        }

        // === APPLY QUADRIPOLAR EFFECTS TO CAMERA/VISUALS ===
        if self.quadripolar.active {
            // Override fold24 seam_basis with quadripolar's seam_basis
            // This creates the 12→24 fold transition
            let quad_basis = self.quadripolar.seam_basis;

            // Blend between normal fold24 and quadripolar control
            // During quadripolar travel, it takes over the seam basis
            let phase_control = match self.quadripolar.phase {
                QuadripolarPhase::Guided12 => 0.2,   // Hint of takeover
                QuadripolarPhase::Refract => 0.5,   // Half control
                QuadripolarPhase::Unfold24 => 1.0,  // Full control
                QuadripolarPhase::Resolve => 0.8,   // Handing back
                QuadripolarPhase::Inactive => 0.0,
            };

            // Set fold24 target based on quadripolar's seam basis
            // This makes fold24 track the quadripolar 12→24 transition
            if phase_control > 0.1 {
                let blended_target = lerp(
                    self.blend.fold24_t.target,
                    quad_basis,
                    phase_control
                );
                self.blend.fold24_t.set_target(blended_target);
            }

            // Apply video density reduction during unfold
            // This is handled by the shader via uniform

            // NOTE: Camera aim angle is now controlled by CylinderMotion system
            // which provides anchor-based snapping instead of direct audio mapping.
            // Quadripolar camera influence disabled to prevent conflicts.
            // The cylinder_motion handles all aim_angle transitions with proper
            // anticipate → snap → swing → settle state machine.

            // Energy burst creates visual emphasis
            if self.quadripolar.energy_burst > 0.1 {
                // Boost rim during energy burst
                self.rim_boost = self.rim_boost.max(self.quadripolar.energy_burst * 0.5);
            }
        }
    }

    // ========================================================================
    // TOPOLOGY ROUTER - Bar-Edge Deterministic Routing
    // ========================================================================

    /// Update topology router state based on bar-edge evaluated metrics
    /// Routes are evaluated only at bar boundaries for musical coherence
    fn update_topology_router(
        &mut self,
        dt: f32,
        beat_phase: f32,
        stability: f32,
        mid_dominant: bool,
        high_dominant: bool,
    ) {
        let router = &mut self.topology_router;
        let cfg = router.config.clone();

        // === UPDATE COOLDOWN ===
        if router.topology_cooldown_s > 0.0 {
            router.topology_cooldown_s = (router.topology_cooldown_s - dt).max(0.0);
        }

        // === CROSSFADE PROGRESS ===
        if router.topology_phase == TopologyPhase::Crossfade {
            let rate = 1.0 / cfg.xfade_duration;
            router.topology_xfade_t = (router.topology_xfade_t + rate * dt).min(1.0);
            if router.topology_xfade_t >= 1.0 {
                router.complete_transition();
            }
        }

        // === HOLD TIMER ===
        router.topology_hold_s += dt;

        // === BEAT/BAR TRACKING ===
        // Detect beats via kick impulse (beat_phase is approximated from kick)
        // A beat is when kick fires fresh (transition from low to high)
        let kick_now = beat_phase < 0.25;  // beat_phase is 0.0 when kick active, 0.5 when not
        let kick_prev = router.prev_beat_phase < 0.25;
        let beat_crossed = kick_now && !kick_prev;  // Rising edge of kick
        router.prev_beat_phase = beat_phase;

        // Time-based fallback: if no beats detected, use ~120 BPM (0.5s per beat, 2s per bar)
        // This ensures bars always progress even if beat detection fails
        const FALLBACK_BAR_DURATION: f32 = 2.0; // 2 seconds per bar at 120 BPM
        let time_per_beat = FALLBACK_BAR_DURATION / cfg.beats_per_bar as f32;

        if beat_crossed {
            router.beat_accumulator += 1.0;
        } else {
            // Fallback: accumulate fractional beats based on time
            router.beat_accumulator += dt / time_per_beat;
        }

        if router.beat_accumulator >= cfg.beats_per_bar as f32 {
            // Bar boundary reached
            router.bar_index += 1;
            router.beat_accumulator -= cfg.beats_per_bar as f32; // Subtract instead of reset for smoother timing

            // Compute bar-averaged metrics
            let frame_count = router.bar_frame_count.max(1) as f32;
            router.onset_rate_bar = router.onset_accumulator / frame_count;
            router.energy_bar = router.energy_accumulator / frame_count;

            // Compute tension slope
            router.tension_slope = self.tension_slow - router.tension_bar_start;
            router.tension_bar_start = self.tension_slow;

            // Reset accumulators
            router.onset_accumulator = 0.0;
            router.energy_accumulator = 0.0;
            router.bar_frame_count = 0;

            // === FOLD24 REQUEST (bar-edge only) ===
            // Compute whether to request 24-fold engagement
            // MIDI mapping: 24-fold at notches 6-8 (~46-62%)
            const HC_ENTER: f32 = 0.3;  // Harmonic confidence threshold
            const E_ENTER: f32 = 0.40;  // Energy threshold - notch 6 (~46%)
            let harmonic_ok = stability >= HC_ENTER;
            let energy_ok = router.energy_bar >= E_ENTER;
            let not_crossfading = router.topology_phase != TopologyPhase::Crossfade;

            let should_request = harmonic_ok && energy_ok && not_crossfading;
            router.fold24_request = should_request;
            router.fold24_stability = stability;

            // Update fold24 request latch bars
            if should_request {
                self.fold24.request_latch_bars += 1;
                log::debug!("[TopologyRouter] fold24_request: latch_bars={}, stability={:.2}, energy={:.2}",
                    self.fold24.request_latch_bars, stability, router.energy_bar);
            } else {
                self.fold24.request_latch_bars = 0;
            }
        }

        // Accumulate per-frame metrics
        router.onset_accumulator += if self.kick_impulse > 0.1 { 1.0 } else { 0.0 };
        router.energy_accumulator += self.section_energy;
        router.bar_frame_count += 1;

        // === ROUTE EVALUATION (BAR-EDGE ONLY) ===
        if router.bar_index == router.last_routed_bar_index {
            return; // Only evaluate at bar boundaries
        }
        router.last_routed_bar_index = router.bar_index;

        // === ENTRY FUNNEL (bar-edge) ===
        // Decrement funnel counter; while > 0, accumulate dwells but don't switch
        if self.fold24.entry_funnel_bars_left > 0 {
            self.fold24.entry_funnel_bars_left -= 1;
            log::debug!("[TopologyRouter] Entry funnel: {} bars remaining", self.fold24.entry_funnel_bars_left);
            // Continue to accumulate dwells below, but don't actually switch
        }

        // Skip if in cooldown or transition
        if router.topology_cooldown_s > 0.0 || router.topology_phase == TopologyPhase::Crossfade {
            // Reset all dwell timers when blocked
            for dwell in router.route_dwell_s.iter_mut() {
                *dwell = 0.0;
            }
            return;
        }

        // === ROUTE PREDICATES ===
        // MIDI mapping: notches 8-13 (~62%+) = geometric (TorusKnot, Vortex)

        // Route 0: Scaffold24 → TorusKnotCymatics (high energy + harmony)
        // Requires fold24.engaged and high energy (~62%+)
        let route0_cond = router.topology_id == TopologyId::Scaffold24
            && self.fold24.engaged
            && router.energy_bar > 0.55   // MIDI notch 8+ (~62%)
            && stability > 0.45
            && !mid_dominant;

        // Route 1: TorusKnotCymatics → Scaffold24 (retreat on low energy)
        let route1_cond = router.topology_id == TopologyId::TorusKnotCymatics
            && router.energy_bar < 0.25   // Drop below notch 4
            && router.topology_hold_s > 4.0;

        // Route 2: Scaffold24 → Vortex (moderate energy + good stability)
        // RELAXED: No longer requires mid_dominant - stability is the key
        let route2_cond = router.topology_id == TopologyId::Scaffold24
            && router.energy_bar > 0.35   // Lowered from 0.45
            && router.energy_bar < 0.85   // Raised from 0.80
            && stability > 0.55;          // Stability gate instead of mid_dominant

        // Route 3: TorusKnotCymatics → Vortex (tension falling, stable)
        // RELAXED: No longer requires mid_dominant - tension dynamics drive it
        let route3_cond = router.topology_id == TopologyId::TorusKnotCymatics
            && router.tension_slope < -0.05  // Relaxed from -0.1
            && stability > 0.50
            && router.topology_hold_s > 3.0;

        // Route 4: Vortex → RadialInterferenceShell (high_dominant + busy onsets)
        let route4_cond = router.topology_id == TopologyId::Vortex
            && high_dominant
            && router.onset_rate_bar > 0.5;

        // Route 5: Vortex → Scaffold24 (retreat on low energy)
        let route5_cond = router.topology_id == TopologyId::Vortex
            && router.energy_bar < 0.30   // Drop below notch 5
            && router.topology_hold_s > 4.0;

        // Route 6: Vortex → BraidedStandingWave (very stable harmony)
        let route6_cond = router.topology_id == TopologyId::Vortex
            && stability > 0.75
            && router.tension_slope.abs() < 0.05;

        // Route 7: RadialInterferenceShell → Scaffold24
        let route7_cond = router.topology_id == TopologyId::RadialInterferenceShell
            && (router.energy_bar < 0.2 || router.topology_hold_s > 8.0);

        // Route 8: Vortex → TorusKnotCymatics (re-escalation)
        let route8_cond = router.topology_id == TopologyId::Vortex
            && router.tension_slope > 0.2
            && router.energy_bar > 0.4;

        // Route 9: BraidedStandingWave → Scaffold24
        let route9_cond = router.topology_id == TopologyId::BraidedStandingWave
            && (router.energy_bar < 0.2 || stability < 0.3 || router.topology_hold_s > 10.0);

        // Route 10: Vortex OR TorusKnot → DodecahedralResonance (harmonic arrival - crystalline state)
        // Triggered by: low chroma entropy (coherent pitch), high stability, sustained conditions
        // This is the "we've landed" state - groove locked in, harmony resolved
        // EXPANDED: Can now enter from TorusKnot directly, not just Vortex
        let entropy = self.torus_knot.spice_last_entropy;
        let in_vortex = router.topology_id == TopologyId::Vortex;
        let in_torus = router.topology_id == TopologyId::TorusKnotCymatics;
        let entropy_ok = entropy < dodeca_const::ENTRY_ENTROPY_MAX;
        let stability_ok = stability > dodeca_const::ENTRY_STABILITY_MIN;
        let energy_ok = router.energy_bar > dodeca_const::ENTRY_MID_ENERGY_MIN;
        let hold_ok = router.topology_hold_s > 3.0;
        let cooldown_ok = self.dodecahedral.cooldown <= 0.0;

        let route10_cond = (in_vortex || in_torus) && entropy_ok && stability_ok && energy_ok && hold_ok && cooldown_ok;

        // Route 11: DodecahedralResonance → Scaffold24 (exit when harmony destabilizes)
        // Hysteresis: exit thresholds are looser than entry to prevent flapping
        let route11_cond = router.topology_id == TopologyId::DodecahedralResonance
            && (entropy > dodeca_const::EXIT_ENTROPY_MAX  // 0.40 - harmony spread
                || stability < dodeca_const::EXIT_STABILITY_MIN  // 0.55 - destabilized
                || router.energy_bar < 0.15  // Energy dropped significantly
                || router.topology_hold_s > 15.0);  // Max hold time

        let conditions = [
            route0_cond, route1_cond, route2_cond, route3_cond, route4_cond,
            route5_cond, route6_cond, route7_cond, route8_cond, route9_cond,
            route10_cond, route11_cond,
        ];

        // Update dwell timers
        for (i, &cond) in conditions.iter().enumerate() {
            if cond {
                router.route_dwell_s[i] += (router.beats_per_bar as f32) * 0.5; // ~bar duration
            } else {
                router.route_dwell_s[i] = (router.route_dwell_s[i] - dt).max(0.0);
            }
        }

        // === ENTRY FUNNEL CHECK ===
        // If still in entry funnel, accumulate dwells but don't switch yet
        if self.fold24.entry_funnel_bars_left > 0 {
            return; // Dwells accumulated above, but no switching during funnel
        }

        // Check routes in priority order and trigger first match
        // If entry_requested_topology is set, prefer that topology
        let preferred_topology = self.fold24.entry_requested_topology;

        let route_targets = [
            (0, TopologyId::TorusKnotCymatics),
            (3, TopologyId::Vortex),         // TorusKnot → Vortex
            (2, TopologyId::Vortex),         // Scaffold → Vortex
            (10, TopologyId::DodecahedralResonance), // Vortex → Dodecahedral (harmonic arrival)
            (8, TopologyId::TorusKnotCymatics), // Vortex → TorusKnot
            (4, TopologyId::RadialInterferenceShell),
            (6, TopologyId::BraidedStandingWave),
            (1, TopologyId::Scaffold24),
            (5, TopologyId::Scaffold24),
            (7, TopologyId::Scaffold24),
            (9, TopologyId::Scaffold24),
            (11, TopologyId::Scaffold24),    // Dodecahedral → Scaffold (exit)
        ];

        // First pass: check if preferred topology is available
        if let Some(preferred) = preferred_topology {
            for &(route_idx, target) in &route_targets {
                if target == preferred && router.check_route(route_idx, conditions[route_idx]) {
                    log::info!("[TopologyRouter] Switching to preferred topology: {:?}", preferred);
                    router.begin_transition(target);
                    self.fold24.entry_requested_topology = None; // Clear preference
                    for dwell in router.route_dwell_s.iter_mut() {
                        *dwell = 0.0;
                    }
                    return;
                }
            }
        }

        // Second pass: normal priority order
        for &(route_idx, target) in &route_targets {
            if router.check_route(route_idx, conditions[route_idx]) {
                router.begin_transition(target);
                self.fold24.entry_requested_topology = None; // Clear preference after any switch
                // Reset all dwell timers after transition
                for dwell in router.route_dwell_s.iter_mut() {
                    *dwell = 0.0;
                }
                break;
            }
        }
    }

    // ========================================================================
    // VORTEX STATE - Spiral Flow Topology
    // ========================================================================

    /// Update vortex state based on topology router and audio signals
    fn update_vortex(&mut self, dt: f32, dominant_pc: usize, beat_phase: f32) {
        let vortex = &mut self.vortex;
        let router = &self.topology_router;

        // === COOLDOWN ===
        if vortex.cooldown > 0.0 {
            vortex.cooldown = (vortex.cooldown - dt).max(0.0);
        }

        // === DIRECTION COOLDOWN ===
        if vortex.direction_cooldown > 0.0 {
            vortex.direction_cooldown = (vortex.direction_cooldown - dt).max(0.0);
        }

        // Determine if vortex should be active based on topology router
        let should_be_active = router.topology_id == TopologyId::Vortex
            || (router.topology_phase == TopologyPhase::Crossfade
                && (router.topology_id == TopologyId::Vortex
                    || router.topology_prev_id == TopologyId::Vortex));

        // === ACTIVATION ===
        if should_be_active && !vortex.active && vortex.cooldown <= 0.0 {
            vortex.active = true;
            vortex.t = 0.0;
            vortex.seed_phase = self.fold24.t * std::f32::consts::TAU;

            // Inherit gate from current dominant pitch class (24-fold quantized)
            // Dominant PC (0-11) maps to gate (0-23) via 2x
            vortex.inherited_gate = ((dominant_pc * 2) % 24) as u8;
            vortex.gate_center = vortex.inherited_gate;
            vortex.circulation_step = 0;

            vortex.min_on_timer = vortex.config.min_on_time;
            vortex.active_time = 0.0;
            vortex.pressure_env = 0.0;
            vortex.pressure_target = 0.5;

            // === GRANT TRIANGLE INITIALIZATION (initial selection, no hysteresis) ===
            // Use grant_initial_select for first activation (no current ID to compare)
            let triangle_idx = grant_initial_select(self.section_energy, self.stability_smooth);
            vortex.grant_triangle_id = triangle_idx;
            vortex.triangle_last_request_id = triangle_idx;
            vortex.triangle_hold_timer = 0.0;
            vortex.triangle_confidence_lp = self.stability_smooth; // Initialize to current
            let triangle = GRANT_TRIANGLES[triangle_idx as usize];
            vortex.grant_shell_radii = triangle.compute_shell_radii();
            vortex.grant_shell_count = 9;
            // Quantize characteristic angle to 24-fold using round() for REG-true nearest-anchor
            // For right triangles θ ∈ [0, π/2], so quantized range is 0..6 (since π/2 = TAU/4 = 6 steps)
            let theta = triangle.characteristic_angle();
            let q_raw = ((theta / std::f32::consts::TAU) * 24.0).round() as u8;
            vortex.grant_characteristic_angle_q = q_raw.min(6); // Clamp to right-triangle range

            log::info!(
                "[Vortex] Activated, gate={}, triangle[{}]=({:.0},{:.0},{:.0})",
                vortex.inherited_gate,
                triangle_idx,
                triangle.a, triangle.b, triangle.c
            );
        }

        // === DEACTIVATION ===
        if !should_be_active && vortex.active && vortex.min_on_timer <= 0.0 {
            vortex.active = false;
            vortex.cooldown = vortex.config.cooldown_duration;
            log::info!("[Vortex] Deactivated after {:.1}s", vortex.active_time);
        }

        // === TRANSITION PROGRESS ===
        if vortex.active && vortex.t < 1.0 {
            let rate = 1.0 / vortex.config.enter_duration;
            vortex.t = (vortex.t + rate * dt).min(1.0);
        } else if !vortex.active && vortex.t > 0.0 {
            let rate = 1.0 / vortex.config.exit_duration;
            vortex.t = (vortex.t - rate * dt).max(0.0);
        }

        // === UPDATE TIMERS ===
        if vortex.active {
            vortex.min_on_timer = (vortex.min_on_timer - dt).max(0.0);
            vortex.active_time += dt;
        }

        // Skip rest if not active/transitioning
        if vortex.t < 0.01 {
            return;
        }

        // === GATE CONFIDENCE CHECK ===
        // Use gate_ok_target (hysteresis boolean latch) for CPU stepping decisions.
        // This ensures stepping only resumes when gate is truly confident (target=1),
        // preventing "half enabled" behavior during the visual ramp.
        // GPU uses the lerped gate_ok for smooth visual transitions.
        let gate_confident = self.gate_ok_target > 0.5;  // true when target=1, false when target=0

        // === BEAT TRACKING FOR CIRCULATION ===
        let beat_crossed = beat_phase < vortex.prev_beat_phase && vortex.prev_beat_phase > 0.5;
        vortex.prev_beat_phase = beat_phase;

        // Only allow circulation stepping when gate is confident
        if beat_crossed && vortex.active && gate_confident {
            // Determine circulation step based on audio characteristics
            // Use tension slope as indicator: rising tension = CW, falling = CCW
            let tension_slope = self.topology_router.tension_slope;
            if tension_slope > 0.1 {
                vortex.circulation_step = 1;
            } else if tension_slope < -0.1 {
                vortex.circulation_step = -1;
            } else {
                vortex.circulation_step = 0;
            }

            // Apply circulation (max ±1 per beat)
            let new_gate = (vortex.gate_center as i16 + vortex.circulation_step as i16).rem_euclid(24);
            vortex.gate_center = new_gate as u8;
        } else if !gate_confident {
            // Freeze circulation when ambiguous
            vortex.circulation_step = 0;
        }

        // === PITCH QUANTIZATION ===
        // Dominant PC (0-11) maps to pitch_q (0-23) via 2x
        vortex.pitch_q = ((dominant_pc * 2) % 24) as u8;

        // === PRESSURE ENVELOPE ===
        // When gate is ambiguous, only allow decay (no new pulses)
        if gate_confident {
            // Target pressure from energy
            vortex.pressure_target = self.section_energy.clamp(0.2, 0.9);

            // Envelope attack/decay
            if vortex.pressure_env < vortex.pressure_target {
                vortex.pressure_env += vortex.config.pressure_attack * dt;
                vortex.pressure_env = vortex.pressure_env.min(vortex.pressure_target);
            } else {
                vortex.pressure_env -= vortex.config.pressure_decay * dt;
                vortex.pressure_env = vortex.pressure_env.max(vortex.pressure_target);
            }
        } else {
            // Ambiguous: decay only, no new pulses
            vortex.pressure_target = 0.2;  // Minimum
            vortex.pressure_env -= vortex.config.pressure_decay * 0.5 * dt;
            vortex.pressure_env = vortex.pressure_env.max(0.2);
        }

        // === DIRECTION FLIPPING ===
        // Rare direction flip based on significant energy change
        if vortex.direction_cooldown <= 0.0 && vortex.active {
            let energy_spike = self.section_energy > 0.7 && self.kick_impulse > 0.3;
            if energy_spike {
                vortex.direction = vortex.direction.flip();
                vortex.direction_cooldown = vortex.config.direction_cooldown;
                log::debug!("[Vortex] Direction flip to {:?}", vortex.direction);
            }
        }

        // === SHELL SET SELECTION ===
        // Based on stability: low=0, mid=1, high=2
        vortex.shell_set_id = if self.stability_smooth < 0.3 {
            0
        } else if self.stability_smooth < 0.6 {
            1
        } else {
            2
        };

        // === GRANT TRIANGLE UPDATE (Hardened: Latched, Quantized, Confidence-Locked) ===
        // Audio selects which harmonic universe is active, not deformation.
        // Changes are rare, quantized to beat edges, and require confidence + hold time.

        // Rule 0: Update timers & smoothed confidence EVERY FRAME
        vortex.triangle_hold_timer += dt;
        let conf_alpha = (GRANT_CONF_SMOOTH_RATE * dt).min(1.0);
        vortex.triangle_confidence_lp = vortex.triangle_confidence_lp * (1.0 - conf_alpha)
            + self.stability_smooth * conf_alpha;

        // Rule 1: SILENCE FREEZE (hard stop) - silence NEVER reselects
        let is_silence = self.section_energy < GRANT_SILENCE_ENERGY;
        if !is_silence && vortex.active {
            // Rule 2: ELIGIBILITY GATE - all must be true for change to be considered
            let conf_arm = GRANT_CONF_MIN + GRANT_CONF_HYST;
            let conf_disarm = GRANT_CONF_MIN - GRANT_CONF_HYST;

            let eligible = beat_crossed
                && vortex.triangle_hold_timer >= GRANT_MIN_HOLD_SEC
                && vortex.triangle_confidence_lp >= conf_arm
                && self.stability_smooth >= GRANT_STAB_MIN
                && gate_confident;

            if eligible {
                // Rule 3: HYSTERESIS-BASED TARGET SELECTION
                let complexity = grant_complexity_score(self.section_energy, self.stability_smooth);
                let req_id = grant_hysteresis_select(vortex.grant_triangle_id, complexity);
                vortex.triangle_last_request_id = req_id;

                // Rule 4: ±1 STEP LIMITER (mandatory - no jumps > 1)
                let current_id = vortex.grant_triangle_id;
                let clamped_id = if req_id > current_id + 1 {
                    current_id + 1
                } else if req_id + 1 < current_id {
                    current_id - 1
                } else {
                    req_id
                };

                // Rule 5: CONFIDENCE DISARM CHECK
                let can_commit = vortex.triangle_confidence_lp >= conf_disarm
                    && clamped_id != current_id;

                // Rule 6: COMMIT (atomic swap)
                if can_commit {
                    let old_id = vortex.grant_triangle_id;
                    vortex.grant_triangle_id = clamped_id;
                    let triangle = GRANT_TRIANGLES[clamped_id as usize];
                    vortex.grant_shell_radii = triangle.compute_shell_radii();
                    vortex.grant_shell_count = 9;
                    // REG-true quantization: round() to nearest anchor, clamp to [0..6] for right triangles
                    let q_raw = ((triangle.characteristic_angle() / std::f32::consts::TAU) * 24.0).round() as u8;
                    vortex.grant_characteristic_angle_q = q_raw.min(6);
                    vortex.triangle_hold_timer = 0.0; // Reset hold timer ONLY on commit

                    log::info!(
                        "[Vortex] Triangle commit: {} -> {} (e={:.2}, s={:.2}, conf={:.2}, score={:.2})",
                        old_id,
                        clamped_id,
                        self.section_energy,
                        self.stability_smooth,
                        vortex.triangle_confidence_lp,
                        complexity
                    );
                }
            }
        }
        // Note: If silence or not eligible, hold timer continues but no commit happens

        // === 3D VORTEX / DRAIN UPDATE ===
        // Preset sink radii (normalized): index 0=tight, 4=wide
        const SINK_RADIUS_PRESETS: [f32; 5] = [0.15, 0.25, 0.35, 0.45, 0.55];
        const SINK_PULSE_PEAK: f32 = 0.8;
        const SINK_ENV_DECAY_TAU: f32 = 0.4; // seconds
        const MIN_SINK_HOLD_BARS: u32 = 4;
        const SINK_STRENGTH_SMOOTH: f32 = 2.0; // interpolation speed

        // A) Stability gate: freeze deformation parameters when ambiguous
        let can_update_sink = gate_confident && vortex.active;

        // B) Sink activation (bar-edge only)
        // Check if we're at a new bar boundary
        let current_bar = self.topology_router.bar_index;
        let bars_since_change = current_bar.saturating_sub(vortex.sink_last_change_bar);

        if can_update_sink && bars_since_change >= MIN_SINK_HOLD_BARS as u64 {
            // Compute band energies from smoothed chroma
            let mid_sum: f32 = self.smoothed_chroma[2..8].iter().sum();
            let bass_sum: f32 = self.smoothed_chroma[0..3].iter().sum();
            let high_sum: f32 = self.smoothed_chroma[8..12].iter().sum();
            let mid_dominant = mid_sum > bass_sum && mid_sum > high_sum;
            let high_calm = high_sum < 0.3;

            // Set sink strength target based on mid energy
            let new_sink_target = if mid_dominant && high_calm {
                (mid_sum / 6.0 * 1.5).clamp(0.3, 0.9)  // Normalize mid_sum
            } else {
                (self.section_energy * 0.7).clamp(0.1, 0.6)
            };
            vortex.vortex_sink_strength_target = new_sink_target;

            // Choose sink radius preset based on stability bands
            let new_radius_q = if self.stability_smooth < 0.25 {
                0 // Tight when unstable
            } else if self.stability_smooth < 0.5 {
                1
            } else if self.stability_smooth < 0.7 {
                2
            } else if self.stability_smooth < 0.85 {
                3
            } else {
                4 // Wide when very stable
            };

            // Only change if different and hold expired
            if new_radius_q != vortex.vortex_sink_radius_q {
                vortex.vortex_sink_radius_q = new_radius_q;
                vortex.sink_last_change_bar = current_bar;

                // Set bulge phase based on gate alignment
                vortex.vortex_bulge_phase_q = vortex.gate_center;
            }
        }

        // Cap sink strength when ambiguous
        if !gate_confident {
            vortex.vortex_sink_strength_target = vortex.vortex_sink_strength_target.min(0.2);
        }

        // Smooth interpolation of sink strength
        let sink_diff = vortex.vortex_sink_strength_target - vortex.vortex_sink_strength;
        vortex.vortex_sink_strength += sink_diff * (SINK_STRENGTH_SMOOTH * dt).min(1.0);

        // C) Sink "pump" (beat-edge only)
        if beat_crossed && vortex.active {
            // Strong onset triggers gulp
            if self.kick_impulse > 0.2 && gate_confident {
                vortex.sink_env = vortex.sink_env.max(SINK_PULSE_PEAK * self.kick_impulse.min(1.0));
            }
        }
        // Always decay sink envelope
        vortex.sink_env *= (-dt / SINK_ENV_DECAY_TAU).exp();

        // D) 3D depth coupling
        // Depth follows engagement and sink envelope
        let target_depth = if vortex.active {
            0.4 + 0.3 * vortex.vortex_sink_strength + 0.2 * vortex.sink_env
        } else {
            0.0
        };
        let depth_diff = target_depth - vortex.vortex_depth;
        vortex.vortex_depth += depth_diff * (1.5 * dt).min(1.0);
        vortex.vortex_depth = vortex.vortex_depth.clamp(0.0, 1.0);

        // Occlusion follows depth
        vortex.vortex_occlusion = 0.3 + 0.4 * vortex.vortex_depth;

        // Expose sink radius for shaders (convert from preset index)
        let _ = SINK_RADIUS_PRESETS[vortex.vortex_sink_radius_q as usize];

        // =====================================================================
        // E) QS WHEEL DRAIN SPRING ANIMATION
        // =====================================================================
        // Behavior:
        //   - Ease IN: When vortex active, QS wheel slowly gets pulled into drain
        //   - Snap OUT: When vortex deactivates, QS wheel quickly snaps back
        //   - Ripple: After snapping out, oscillate and settle into place
        //
        // Uses damped spring physics with different parameters for pull vs release

        // Spring constants
        const PULL_SPRING_K: f32 = 2.0;      // Stiffness when pulling in (slow ease)
        const PULL_DAMPING: f32 = 3.0;       // High damping = smooth ease, no overshoot
        const RELEASE_SPRING_K: f32 = 25.0;  // Stiffness when releasing (fast snap)
        const RELEASE_DAMPING: f32 = 4.0;    // Lower damping = allows ripple/overshoot

        // Set target based on vortex state
        // Target = 1.0 when active (pulled in), 0.0 when inactive (released)
        let was_pulling = vortex.qs_drain_target > 0.5;
        vortex.qs_drain_target = if vortex.active {
            // Pull strength modulated by sink_strength and pressure
            (vortex.vortex_sink_strength * 0.7 + vortex.pressure_env * 0.3).clamp(0.3, 1.0)
        } else {
            0.0
        };
        let now_releasing = was_pulling && vortex.qs_drain_target < 0.5;

        // Choose spring parameters based on direction
        let (spring_k, damping) = if vortex.qs_drain_target > vortex.qs_drain_pull {
            // Pulling IN: slow, smooth ease
            (PULL_SPRING_K, PULL_DAMPING)
        } else {
            // Releasing OUT: fast snap with ripple
            (RELEASE_SPRING_K, RELEASE_DAMPING)
        };

        // If just started releasing, give initial velocity for snappier feel
        if now_releasing && vortex.qs_drain_vel > -2.0 {
            vortex.qs_drain_vel = -3.0;  // Initial outward velocity
        }

        // Damped spring physics: F = -k*x - c*v
        // x = displacement from target, v = velocity
        let displacement = vortex.qs_drain_pull - vortex.qs_drain_target;
        let spring_force = -spring_k * displacement;
        let damping_force = -damping * vortex.qs_drain_vel;
        let acceleration = spring_force + damping_force;

        // Integrate velocity and position
        vortex.qs_drain_vel += acceleration * dt;
        vortex.qs_drain_pull += vortex.qs_drain_vel * dt;

        // Clamp to valid range (allow slight overshoot for ripple, but not too much)
        vortex.qs_drain_pull = vortex.qs_drain_pull.clamp(-0.15, 1.1);

        // Dampen velocity when very close to rest (prevents micro-jitter)
        if displacement.abs() < 0.01 && vortex.qs_drain_vel.abs() < 0.1 {
            vortex.qs_drain_vel *= 0.8;
        }
    }

    // ========================================================================
    // DODECAHEDRAL RESONANCE - Crystalline Harmonic Arrival
    // ========================================================================
    //
    // The dodecahedron represents "harmonic arrival" - when music settles into
    // coherent, stable harmony. Emerges from Vortex when:
    //   - Chroma entropy is low (coherent pitch)
    //   - Stability is high (harmonic relationships locked)
    //   - Mid-range energy is sustained (groove in the pocket)
    //
    // Geometry (Grant Projection Theorem alignment):
    //   - 12 pentagonal faces × 2 orientations = 24-fold alignment
    //   - Face depths driven by Grant shell radii (9 means)
    //   - 20 vertices breathe with pitch class
    //   - 30 edges pulse on beat transients
    //   - Golden ratio (Phi) embedded in pentagonal geometry
    //
    // ========================================================================

    /// Update dodecahedral resonance state
    fn update_dodecahedral(&mut self, dt: f32, dominant_pc: usize, beat_phase: f32, chroma: &[f32; 12]) {
        use dodeca_const::*;

        let dodeca = &mut self.dodecahedral;
        let router = &self.topology_router;

        // === COOLDOWN TIMER ===
        if dodeca.cooldown > 0.0 {
            dodeca.cooldown = (dodeca.cooldown - dt).max(0.0);
        }

        // === CHECK IF ACTIVE TOPOLOGY ===
        let is_active_topology = router.topology_id == TopologyId::DodecahedralResonance;
        let is_crossfading_in = router.topology_phase == TopologyPhase::Crossfade
            && router.topology_id == TopologyId::DodecahedralResonance;
        let is_crossfading_out = router.topology_phase == TopologyPhase::Crossfade
            && router.topology_prev_id == TopologyId::DodecahedralResonance;

        let should_be_active = is_active_topology || is_crossfading_in;
        let was_active = dodeca.active;

        // === ACTIVATION HANDLING ===
        if should_be_active && !was_active {
            // Just activated - initialize state
            dodeca.active = true;
            dodeca.min_on_timer = MIN_ON_TIME;
            dodeca.active_time = 0.0;

            // Inherit state from vortex for continuity
            dodeca.inherited_gate = self.vortex.gate_center;
            dodeca.inherited_grant_id = self.vortex.grant_triangle_id;
            dodeca.grant_shell_radii = self.vortex.grant_shell_radii;

            // Latch entry conditions
            dodeca.entry_entropy = self.torus_knot.spice_last_entropy;
            dodeca.entry_stability = self.stability_smooth;

            // Initialize face activation from chroma
            for i in 0..12 {
                dodeca.face_activation[i] = chroma[i];
            }

            // Map Grant shell radii to face depths (cycle 9 means across 12 faces)
            for i in 0..12 {
                let shell_idx = i % 9;
                dodeca.face_depths[i] = dodeca.grant_shell_radii[shell_idx];
            }

            log::info!(
                "[Dodecahedral] ACTIVATED: gate={} grant_id={} entropy={:.2} stability={:.2}",
                dodeca.inherited_gate, dodeca.inherited_grant_id,
                dodeca.entry_entropy, dodeca.entry_stability
            );
        }

        // === DEACTIVATION HANDLING ===
        if !should_be_active && was_active && !is_crossfading_out {
            // Deactivating - set cooldown
            dodeca.active = false;
            dodeca.cooldown = COOLDOWN_TIME;
            log::info!(
                "[Dodecahedral] DEACTIVATED: active_time={:.1}s, setting cooldown={:.1}s",
                dodeca.active_time, COOLDOWN_TIME
            );
        }

        // === TRANSITION PROGRESS ===
        // t: 0→1 during activation, 1→0 during deactivation
        let t_target = if should_be_active { 1.0 } else { 0.0 };
        let t_rate = 1.0 / TRANSITION_TIME;
        if dodeca.t < t_target {
            dodeca.t = (dodeca.t + t_rate * dt).min(1.0);
        } else if dodeca.t > t_target {
            dodeca.t = (dodeca.t - t_rate * dt).max(0.0);
        }

        // === EARLY EXIT IF NOT ACTIVE ===
        if dodeca.t < 0.01 && !dodeca.active {
            return;
        }

        // === UPDATE TIMERS ===
        if dodeca.active {
            dodeca.active_time += dt;
            if dodeca.min_on_timer > 0.0 {
                dodeca.min_on_timer = (dodeca.min_on_timer - dt).max(0.0);
            }
        }

        // === BEAT TRACKING ===
        let beat_crossed = beat_phase < 0.25 && dodeca.prev_beat_phase >= 0.75;
        dodeca.prev_beat_phase = beat_phase;

        // === FACE ACTIVATION (chroma-driven) ===
        // Each of 12 faces corresponds to one pitch class
        // Smooth interpolation with different attack/decay
        for i in 0..12 {
            let target = chroma[i].clamp(0.0, 1.0);
            let current = dodeca.face_activation[i];
            let rate = if target > current { 8.0 } else { 3.0 }; // Fast attack, slower decay
            dodeca.face_activation[i] += (target - current) * (rate * dt).min(1.0);
        }

        // === DOMINANT FACE (strongest chroma bin) ===
        let mut max_val = 0.0f32;
        let mut max_idx = 0usize;
        for (i, &val) in dodeca.face_activation.iter().enumerate() {
            if val > max_val {
                max_val = val;
                max_idx = i;
            }
        }
        dodeca.dominant_face = max_idx as u8;

        // === VERTEX BREATHING ===
        // Vertices breathe in sync with dominant pitch class rhythm
        let breath_freq = VERTEX_BREATH_FREQ * std::f32::consts::TAU;
        dodeca.vertex_breath_phase += breath_freq * dt;
        if dodeca.vertex_breath_phase > std::f32::consts::TAU {
            dodeca.vertex_breath_phase -= std::f32::consts::TAU;
        }

        // Amplitude follows stability - more stable = more pronounced breathing
        let breath_target = self.stability_smooth * 0.8;
        dodeca.vertex_breath_amp += (breath_target - dodeca.vertex_breath_amp) * (2.0 * dt).min(1.0);

        // === EDGE PULSES (beat transient driven) ===
        if beat_crossed {
            // Trigger edge pulse on beat
            let pulse_strength = self.kick_impulse.clamp(0.0, 1.0);
            dodeca.edge_pulse_env = (dodeca.edge_pulse_env + pulse_strength * EDGE_PULSE_ATTACK * dt).min(1.0);
        }
        // Decay edge pulse
        dodeca.edge_pulse_env *= (-dt * EDGE_PULSE_DECAY).exp();

        // Edge glow follows overall energy
        let glow_target = self.section_energy.clamp(0.0, 1.0) * 0.6;
        dodeca.edge_glow += (glow_target - dodeca.edge_glow) * (3.0 * dt).min(1.0);

        // === ROTATION (geological drift) ===
        dodeca.rotation_angle += dodeca.rotation_velocity * dt;
        if dodeca.rotation_angle > std::f32::consts::TAU {
            dodeca.rotation_angle -= std::f32::consts::TAU;
        }

        // === FACE LOCK STRENGTH ===
        // How strongly faces align to 24-fold gates (driven by stability)
        let lock_target = if self.stability_smooth > 0.6 { 1.0 } else { self.stability_smooth / 0.6 };
        dodeca.face_lock_strength += (lock_target - dodeca.face_lock_strength) * (FACE_LOCK_RATE * dt).min(1.0);

        // === PHI RESONANCE ===
        // Golden ratio emphasis - stronger when harmony is especially coherent
        // Phi resonance triggers when face depths align with golden ratio relationships
        let coherence = 1.0 - self.torus_knot.spice_last_entropy;  // High coherence = low entropy
        let phi_target_val = if coherence > 0.7 && self.stability_smooth > 0.75 {
            coherence * self.stability_smooth
        } else {
            0.0
        };
        dodeca.phi_target = phi_target_val;
        dodeca.phi_resonance += (dodeca.phi_target - dodeca.phi_resonance) * (PHI_LERP_RATE * dt).min(1.0);

        // === UPDATE FACE DEPTHS (Grant shell continuity) ===
        // Face depths slowly track Grant shell radii (inherited from vortex)
        // This provides geometric continuity during transition
        if beat_crossed && dodeca.active {
            // Update Grant shell radii from vortex (they may have changed)
            // But only if vortex has valid radii
            let vortex_has_radii = self.vortex.grant_shell_radii.iter().any(|&r| r > 0.01);
            if vortex_has_radii {
                dodeca.grant_shell_radii = self.vortex.grant_shell_radii;
            }

            // Remap to face depths
            for i in 0..12 {
                let shell_idx = i % 9;
                let target_depth = dodeca.grant_shell_radii[shell_idx];
                // Smooth transition
                dodeca.face_depths[i] += (target_depth - dodeca.face_depths[i]) * 0.3;
            }
        }
    }

    // ========================================================================
    // OUTER BACKDROP - Premium Cathedral Background
    // ========================================================================
    //
    // Three jobs for the outside:
    //   1. Frame the torus (vignette + aperture)
    //   2. Echo 24-fold seams (subtle ticks + arcs)
    //   3. Carry low-cost motion (slow breathing + beat-quantized pulses)
    //
    // CRITICAL: outside brightness ≤ 0.35 * scaffold brightness (hard cap)
    // ========================================================================

    /// Update outer backdrop state - aperture, pulse, style selection
    /// Premium polish: two-stage pulse, topology-driven routing, gate confidence fade
    fn update_outer_backdrop(&mut self, dt: f32, beat_phase: f32, kick: f32) {
        // Constants
        const PULSE_FAST_DECAY_TAU: f32 = 0.15;  // Fast envelope decay (beat response)
        const PULSE_SLOW_DECAY_TAU: f32 = 1.5;   // Slow envelope decay (bar-length bloom)
        const MIN_HOLD_BARS: u64 = 4;            // Min bars before aperture change
        const STYLE_B_HOLD_BARS: u64 = 8;        // Style B is a "reward" - needs 8 bars
        const GATE_FADE_SMOOTH: f32 = 3.0;       // Gate fade interpolation speed

        // === TOPOLOGY FLAGS ===
        // Detect current topology for context-aware rendering
        let topo = self.topology_router.topology_id;
        let crossfading_to_vortex = self.topology_router.topology_phase == TopologyPhase::Crossfade
            && topo == TopologyId::Vortex;
        let crossfading_from_vortex = self.topology_router.topology_phase == TopologyPhase::Crossfade
            && self.topology_router.topology_prev_id == TopologyId::Vortex;

        self.outer_is_vortex = topo == TopologyId::Vortex || crossfading_to_vortex;
        self.outer_is_torusknot = topo == TopologyId::TorusKnotCymatics && !crossfading_to_vortex;

        // Freeze discrete updates when gate is ambiguous (use hysteresis boolean latch)
        self.outer_can_update = self.gate_ok_target > 0.5;

        // === GATE EMPHASIS FADE ===
        // Use gate_ok directly for visual fade (already has hysteresis + ramp)
        let target_gate_fade = self.gate_ok;
        let gate_diff = target_gate_fade - self.outer_gate_fade;
        self.outer_gate_fade += gate_diff * (GATE_FADE_SMOOTH * dt).min(1.0);

        // === TWO-STAGE PULSE ENVELOPE ===
        // Fast attack on beat, fast + slow decay for lens bloom feel
        static mut PREV_BEAT_PHASE: f32 = 0.0;
        let beat_crossed = unsafe {
            let crossed = beat_phase < PREV_BEAT_PHASE && PREV_BEAT_PHASE > 0.5;
            PREV_BEAT_PHASE = beat_phase;
            crossed
        };

        // Pulse on beat (stronger on kick)
        if beat_crossed {
            let pulse_strength = if kick > 0.2 {
                0.7 + kick * 0.3  // Kick-boosted
            } else {
                0.5  // Normal beat
            };
            // Fast envelope jumps immediately
            self.outer_pulse_env = self.outer_pulse_env.max(pulse_strength);
            // Slow envelope also gets a boost (but smaller)
            self.outer_pulse_slow = self.outer_pulse_slow.max(pulse_strength * 0.6);
        }

        // Decay envelopes at different rates
        self.outer_pulse_env *= (-dt / PULSE_FAST_DECAY_TAU).exp();
        self.outer_pulse_slow *= (-dt / PULSE_SLOW_DECAY_TAU).exp();

        // === TOPOLOGY-DRIVEN STYLE ROUTING (deterministic) ===
        let current_bar = self.topology_router.bar_index;
        let bars_since_change = current_bar.saturating_sub(self.outer_last_change_bar);

        // A) VORTEX: Force Style A, tighter aperture
        if self.outer_is_vortex {
            // Always Style A for clean drain read
            if self.outer_style_id != 0 {
                self.outer_style_id = 0;
                self.outer_last_change_bar = current_bar;
            }
            // Tighten aperture by 1 step for "black hole" feel
            let base_aperture: u8 = if self.section_energy < 0.3 { 0 }
                else if self.section_energy < 0.5 { 1 }
                else if self.section_energy < 0.7 { 2 }
                else { 3 };
            let vortex_aperture = base_aperture.saturating_sub(1);  // Tighter
            if vortex_aperture != self.outer_aperture_q && bars_since_change >= MIN_HOLD_BARS {
                self.outer_aperture_q = vortex_aperture;
                self.outer_last_change_bar = current_bar;
            }
        }
        // B) TORUS KNOT: Style A default, Style B as rare reward
        else if self.outer_is_torusknot {
            // Select aperture based on energy
            let new_aperture: u8 = if self.section_energy < 0.3 { 0 }
                else if self.section_energy < 0.5 { 1 }
                else if self.section_energy < 0.7 { 2 }
                else { 3 };

            if self.outer_can_update && bars_since_change >= MIN_HOLD_BARS {
                if new_aperture != self.outer_aperture_q {
                    self.outer_aperture_q = new_aperture;
                    self.outer_last_change_bar = current_bar;
                }

                // Style B (constellation) only as reward: high stability + 8 bars hold
                if self.stability_smooth > 0.85 && bars_since_change >= STYLE_B_HOLD_BARS {
                    if self.outer_style_id != 1 {
                        self.outer_style_id = 1;
                        self.outer_last_change_bar = current_bar;
                    }
                } else if self.stability_smooth < 0.7 && self.outer_style_id == 1 {
                    // Drop back to Style A if stability falls
                    self.outer_style_id = 0;
                    self.outer_last_change_bar = current_bar;
                }
            }
        }
        // C) SCAFFOLD / AFTERGLOW: Style A, widest aperture (calm, open frame)
        else {
            if self.outer_style_id != 0 {
                self.outer_style_id = 0;
                self.outer_last_change_bar = current_bar;
            }
            // Widest aperture for calm, open frame
            if self.outer_aperture_q != 3 && bars_since_change >= MIN_HOLD_BARS {
                self.outer_aperture_q = 3;
                self.outer_last_change_bar = current_bar;
            }
        }
    }

    // ========================================================================
    // REG-LOCKED CAMERA - Quantized Camera Motion
    // ========================================================================
    //
    // Camera rides the same angular grid as the geometry:
    //   - Only targets discrete, symmetry-valid poses
    //   - Audio events select targets; camera eases toward them
    //   - Portal Dive locks to a single QC-derived wedge
    //
    // Key invariants:
    //   - ±1 anchor step max per commit (no teleporting)
    //   - Commits only on beat crossings that pass confidence gates
    //   - Silence freezes all targets (energy < CAM_SILENCE)
    //
    // ========================================================================

    /// Update REG-locked camera state
    /// Called every frame, but discrete state only changes at beat boundaries
    pub fn update_camera_reg(
        &mut self,
        dt: f32,
        beat_phase: f32,
        beat_confidence: f32,
        harmonic_confidence: f32,
        gate_stability: f32,
        dominant_gate_k: u8,
        energy: f32,
    ) {
        // === MODE SWITCH: Remap anchor_k when fold24.active transitions ===
        // Must happen BEFORE any gate validation or use-site math.
        // Skip during portal dive — portal manages anchor_k independently via portal_anchor_k_locked.
        self.camera_mode_switch_cooldown = (self.camera_mode_switch_cooldown - dt).max(0.0);
        let fold24_now = self.fold24.active;
        let in_portal = self.camera_reg.rig == CameraRig::PortalDive;
        if !in_portal && fold24_now != self.prev_camera_fold24_active && self.camera_mode_switch_cooldown <= 0.0 {
            let old_k = self.camera_reg.anchor_k;
            let new_k = if fold24_now {
                // 12 → 24: expand (even indices are the 12-fold subset)
                (old_k.rem_euclid(12)) * 2
            } else {
                // 24 → 12: collapse (snap to nearest even anchor, then halve)
                let k_even = if old_k % 2 == 0 { old_k } else { old_k.saturating_sub(1) };
                (k_even / 2).rem_euclid(12)
            };
            let old_mode = if self.prev_camera_fold24_active { 24 } else { 12 };
            let new_mode = if fold24_now { 24 } else { 12 };
            log::info!(
                "[CameraREG] Mode switch {}->{}: anchor_k {} → {} (domain remap)",
                old_mode, new_mode, old_k, new_k
            );
            self.camera_reg.anchor_k = new_k;
            self.reg_validation.prev_anchor_k = new_k;
            // If frozen, update frozen_anchor_k so INV-5 doesn't false-positive on domain remap
            if self.reg_validation.was_frozen {
                self.reg_validation.frozen_anchor_k = new_k;
            }
            self.camera_mode_switch_cooldown = 1.0; // 1s cooldown prevents rapid flip-flop
            self.prev_camera_fold24_active = fold24_now;
        }

        // Symmetry mode is determined by the ACTUAL camera domain (prev_camera_fold24_active),
        // not fold24.active which can oscillate during cooldown.
        let symmetry_mode = if self.prev_camera_fold24_active { 24u8 } else { 12u8 };
        let n = symmetry_mode;
        let az_step = std::f32::consts::TAU / symmetry_mode as f32;

        // === VALIDATION: Track freeze state transitions ===
        let is_frozen = energy < CAM_SILENCE;
        let was_frozen = self.reg_validation.was_frozen;

        if is_frozen && !was_frozen {
            // FREEZE_ENTER
            self.reg_validation.freeze_enters += 1;
            self.reg_validation.add_event(RegEventType::FreezeEnter, self.camera_reg.anchor_k as i32);
            self.reg_validation.was_frozen = true;
            self.reg_validation.frozen_anchor_k = self.camera_reg.anchor_k;
            log::info!(
                "[REG_EVENT] FREEZE_ENTER: energy={:.3} < {:.3}, anchor_k={} LATCHED",
                energy, CAM_SILENCE, self.camera_reg.anchor_k
            );
        } else if !is_frozen && was_frozen {
            // FREEZE_EXIT
            self.reg_validation.freeze_exits += 1;
            self.reg_validation.add_event(RegEventType::FreezeExit, self.camera_reg.anchor_k as i32);
            self.reg_validation.was_frozen = false;
            log::info!(
                "[REG_EVENT] FREEZE_EXIT: energy={:.3}, anchor_k={} (was frozen at {})",
                energy, self.camera_reg.anchor_k, self.reg_validation.frozen_anchor_k
            );

            // INV-5: Freeze rule - anchor must not have changed during freeze
            if self.camera_reg.anchor_k != self.reg_validation.frozen_anchor_k {
                self.reg_validation.inv5_freeze_rule += 1;
                log::warn!(
                    "[REG_VIOLATION] INV-5 FREEZE_RULE: anchor changed during freeze! {} → {}",
                    self.reg_validation.frozen_anchor_k, self.camera_reg.anchor_k
                );
            }
        }

        // === 1. GLOBAL FREEZE (silence = no commits) ===
        if is_frozen {
            // Freeze all targets, let smooth systems coast
            self.camera_reg.cooldown_timer = (self.camera_reg.cooldown_timer - dt).max(0.0);
            self.camera_reg.hold_timer += dt;
            // Still update targets for rendering (they're just frozen)
            self.update_camera_targets_internal(az_step);
            return;
        }

        // === 2. BEAT CROSSING DETECTION ===
        let beat_crossed = beat_phase < self.camera_reg.prev_beat_phase && self.camera_reg.prev_beat_phase > 0.8;
        self.camera_reg.prev_beat_phase = beat_phase;
        self.camera_reg.beat_cross_detected = beat_crossed;

        // === VALIDATION: Increment beat_id on each beat crossing ===
        if beat_crossed {
            self.reg_validation.beat_id += 1;
        }

        // Phrase tracking (for elevation changes)
        if beat_crossed {
            self.camera_reg.beat_count += 1;
            self.camera_reg.phrase_edge = self.camera_reg.beat_count % CAM_BEATS_PER_PHRASE == 0;
        } else {
            self.camera_reg.phrase_edge = false;
        }

        // === 3. UPDATE TIMERS ===
        self.camera_reg.hold_timer += dt;
        self.camera_reg.cooldown_timer = (self.camera_reg.cooldown_timer - dt).max(0.0);

        // === 4. PORTAL DIVE SPECIAL HANDLING ===
        if self.camera_reg.rig == CameraRig::PortalDive {
            self.update_portal_dive_internal(dt);

            // Update Portal FPS mode
            let onset_detected = self.spectral_flux > 0.05;
            self.update_portal_fps(dt, onset_detected, beat_crossed);

            return;  // Portal has its own schedule
        }

        // === 5. PORTAL ELIGIBILITY CHECK (24-fold only) ===
        if symmetry_mode == 24 {
            self.update_portal_eligibility_internal(
                dt, harmonic_confidence, beat_confidence, gate_stability, beat_crossed
            );
        }

        // === 6. COMMIT GATING ===
        let can_commit = beat_crossed
            && beat_confidence > CAM_BEAT_OK
            && self.camera_reg.hold_timer > CAM_T_HOLD
            && self.camera_reg.cooldown_timer <= 0.0;

        if !can_commit {
            self.update_camera_targets_internal(az_step);
            return;
        }

        // === 7. COMMIT: UPDATE DISCRETE STATE ===

        // 7a. Azimuth: ±1 anchor toward desired
        let desired_anchor_k = self.compute_desired_anchor_k_internal(dominant_gate_k, n);
        let current_anchor = self.camera_reg.anchor_k;
        let delta = shortest_wrapped_delta(desired_anchor_k as i16, current_anchor as i16, n as i16);
        let clamped_delta = delta.clamp(-1, 1);
        let new_anchor = ((current_anchor as i16 + clamped_delta).rem_euclid(n as i16)) as u8;

        // Only log if actually changing
        if new_anchor != current_anchor {
            log::trace!(
                "[CameraREG] Anchor commit: {} → {} (desired={}, delta={})",
                current_anchor, new_anchor, desired_anchor_k, clamped_delta
            );
        }
        self.camera_reg.anchor_k = new_anchor;

        // 7b. Radius: based on energy slope
        if self.energy_slope > 0.1 && harmonic_confidence > CAM_HARM_OK {
            // Energy rising + good harmony → move inward
            if self.camera_reg.radius_step > 0 {
                self.camera_reg.radius_step -= 1;
                log::trace!("[CameraREG] Radius in: R{}", self.camera_reg.radius_step);
            }
        } else if self.energy_slope < -0.1 || harmonic_confidence < 0.4 {
            // Energy falling or poor harmony → back out
            if self.camera_reg.radius_step < CAM_RADIUS_STEPS.len() as u8 - 1 {
                self.camera_reg.radius_step += 1;
                log::trace!("[CameraREG] Radius out: R{}", self.camera_reg.radius_step);
            }
        }

        // 7c. Elevation: only on phrase boundary
        let phrase_edge = self.camera_reg.phrase_edge;
        if phrase_edge {
            let mid_dominant = self.blend.tilt_angle.value > 0.1;  // Use existing tilt as proxy
            if mid_dominant && self.camera_reg.elev_level < 2 {
                self.camera_reg.elev_level += 1;
                log::trace!("[CameraREG] Elev up: E{:+}", self.camera_reg.elev_level);
            } else if !mid_dominant && self.camera_reg.elev_level > -2 {
                self.camera_reg.elev_level -= 1;
                log::trace!("[CameraREG] Elev down: E{:+}", self.camera_reg.elev_level);
            }
        }

        // 7d. Reset timers
        self.camera_reg.hold_timer = 0.0;
        self.camera_reg.cooldown_timer = CAM_T_COOLDOWN;

        // === VALIDATION: CAM_COMMIT event logging and invariant checks ===
        let prev_anchor = self.reg_validation.prev_anchor_k;
        let prev_radius = self.reg_validation.prev_radius_step;
        let prev_elev = self.reg_validation.prev_elev_level;

        // CAM_COMMIT event
        self.reg_validation.cam_commits += 1;
        self.reg_validation.add_event(RegEventType::CamCommit, self.camera_reg.anchor_k as i32);
        log::info!(
            "[REG_EVENT] CAM_COMMIT #{}: anchor={} (was {}), radius=R{} (was R{}), elev=E{:+} (was E{:+}), beat_id={}",
            self.reg_validation.cam_commits,
            self.camera_reg.anchor_k, prev_anchor,
            self.camera_reg.radius_step, prev_radius,
            self.camera_reg.elev_level, prev_elev,
            self.reg_validation.beat_id
        );

        // INV-1: Discrete range check
        let anchor_in_range = self.camera_reg.anchor_k < n;
        let radius_in_range = (self.camera_reg.radius_step as usize) < CAM_RADIUS_STEPS.len();
        let elev_in_range = self.camera_reg.elev_level >= -2 && self.camera_reg.elev_level <= 2;
        let fov_in_range = (self.camera_reg.fov_step as usize) < CAM_FOV_STEPS.len();

        if !anchor_in_range || !radius_in_range || !elev_in_range || !fov_in_range {
            self.reg_validation.inv1_discrete_range += 1;
            log::warn!(
                "[REG_VIOLATION] INV-1 DISCRETE_RANGE: anchor={}/{}, radius={}/{}, elev={}/[-2,2], fov={}/{}",
                self.camera_reg.anchor_k, n,
                self.camera_reg.radius_step, CAM_RADIUS_STEPS.len(),
                self.camera_reg.elev_level,
                self.camera_reg.fov_step, CAM_FOV_STEPS.len()
            );
        }

        // INV-2: Step limit (±1 max per commit)
        let anchor_delta = shortest_wrapped_delta(
            self.camera_reg.anchor_k as i16,
            prev_anchor as i16,
            n as i16
        ).abs();
        let radius_delta = (self.camera_reg.radius_step as i16 - prev_radius as i16).abs();
        let elev_delta = (self.camera_reg.elev_level as i16 - prev_elev as i16).abs();

        if anchor_delta > 1 || radius_delta > 1 || elev_delta > 1 {
            self.reg_validation.inv2_step_limit += 1;
            log::warn!(
                "[REG_VIOLATION] INV-2 STEP_LIMIT: anchor_delta={}, radius_delta={}, elev_delta={} (max allowed: 1)",
                anchor_delta, radius_delta, elev_delta
            );
        }

        // INV-3: Beat gating (no double-commits on same beat)
        let current_beat_id = self.reg_validation.beat_id;
        if current_beat_id == self.reg_validation.last_cam_commit_beat_id {
            self.reg_validation.inv3_beat_gating += 1;
            log::warn!(
                "[REG_VIOLATION] INV-3 BEAT_GATING: double CAM_COMMIT on beat_id={}",
                current_beat_id
            );
        }
        self.reg_validation.last_cam_commit_beat_id = current_beat_id;

        // Update previous state for next commit
        self.reg_validation.prev_anchor_k = self.camera_reg.anchor_k;
        self.reg_validation.prev_radius_step = self.camera_reg.radius_step;
        self.reg_validation.prev_elev_level = self.camera_reg.elev_level;

        // === 8. UPDATE DERIVED TARGETS ===
        self.update_camera_targets_internal(az_step);
    }

    /// Compute target pose from discrete state
    fn update_camera_targets_internal(&mut self, az_step: f32) {
        let reg = &mut self.camera_reg;

        reg.target_azimuth = reg.anchor_k as f32 * az_step;
        reg.target_elevation = CAM_ELEV_LEVELS[(reg.elev_level + 2) as usize];
        reg.target_radius = CAM_RADIUS_STEPS[reg.radius_step as usize];
        reg.target_fov = CAM_FOV_STEPS[reg.fov_step as usize];

        // SAFETY: Render rotation must ONLY derive from anchor_k (quantized).
        // reg_degree_a is INFO telemetry and must NEVER feed into camera azimuth.
        // Uses rem_euclid for unambiguous [0, step) remainder regardless of sign.
        debug_assert!(
            az_step > 0.0 && {
                let remainder = reg.target_azimuth.rem_euclid(az_step);
                remainder < 1e-4 || (az_step - remainder) < 1e-4
            },
            "INVARIANT VIOLATION: target_azimuth ({}) is off-grid (step={}, anchor_k={}). \
             Only anchor_k may drive camera rotation, never reg_degree_a.",
            reg.target_azimuth, az_step, reg.anchor_k
        );
    }

    /// Compute desired anchor from QC port or dominant gate
    fn compute_desired_anchor_k_internal(&self, dominant_gate_k: u8, n: u8) -> u8 {
        // If QC is eligible and confident, use QC port
        if self.qc.invariants.lattice_ok_24 && self.qc_port.port_conf_lp > CAM_QC_OK {
            // Scale QC port (always 0..23) to current symmetry mode
            if n == 24 {
                self.qc_port.port_k_latched
            } else {
                // 12-fold: halve the 24-fold anchor
                (self.qc_port.port_k_latched / 2) % n
            }
        } else {
            // Fallback: use dominant gate
            if n == 24 {
                // Scale 12-fold gate to 24-fold
                (dominant_gate_k * 2) % n
            } else {
                dominant_gate_k % n
            }
        }
    }

    /// Update portal eligibility check (24-fold only)
    /// Two-key mechanism with hysteresis:
    ///   Key A (structural): lattice_ok_24 + 24-fold + gate stable + not frozen/portal
    ///   Key B (musical): harmonic confidence sustained (LP filtered with hysteresis)
    fn update_portal_eligibility_internal(
        &mut self,
        dt: f32,
        harmonic_confidence: f32,
        beat_confidence: f32,
        gate_stability: f32,
        beat_crossed: bool,
    ) {
        let elapsed = self.reg_validation.elapsed_s();

        // === 1. UPDATE LP FILTERS ===
        let alpha_harm = 1.0 - (-dt / PORTAL_LP_TAU_HARM).exp();
        let alpha_gate = 1.0 - (-dt / PORTAL_LP_TAU_GATE).exp();
        self.camera_reg.portal_ready_lp += (harmonic_confidence - self.camera_reg.portal_ready_lp) * alpha_harm;
        self.camera_reg.gate_stability_lp += (gate_stability - self.camera_reg.gate_stability_lp) * alpha_gate;

        // === 2. DECREMENT BLOCK TIMER ===
        self.camera_reg.portal_block_timer = (self.camera_reg.portal_block_timer - dt).max(0.0);

        // === 3. CHECK STRUCTURAL ATOMS (Key A) ===
        // Portal now available from ANY topology - REG math preserved via gate_stability
        // The anchor lock ensures camera always snaps to valid 24-fold grid position
        let structural_ok = self.camera_reg.gate_stability_lp > PORTAL_GATE_OK
            && self.camera_reg.rig != CameraRig::PortalDive
            && self.camera_reg.cooldown_timer <= 0.0
            && self.camera_reg.portal_block_timer <= 0.0
            && (elapsed - self.camera_reg.last_portal_time) > PORTAL_T_MIN_INTERVAL;

        // === 4. CHECK MUSICAL ATOM (Key B) with hysteresis ===
        let harm_lp = self.camera_reg.portal_ready_lp;
        let was_ready = self.camera_reg.portal_ready_timer > 0.0 || self.camera_reg.portal_enter_armed;

        // Hysteresis: use higher threshold to enter, lower to exit
        let harm_ok = if was_ready {
            harm_lp > PORTAL_HARM_EXIT  // Stay ready until drops below exit threshold
        } else {
            harm_lp > PORTAL_HARM_ENTER  // Need to exceed enter threshold to become ready
        };

        // === 5. COMBINED READY STATE ===
        let ready = structural_ok && harm_ok;

        // === DIAGNOSTIC: Log portal blocking reasons every 5s ===
        static LAST_DIAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now_ms = (elapsed * 1000.0) as u64;
        let last = LAST_DIAG.load(std::sync::atomic::Ordering::Relaxed);
        if now_ms > last + 5000 {
            LAST_DIAG.store(now_ms, std::sync::atomic::Ordering::Relaxed);
            let topo = self.topology_router.topology_id;
            // Detailed breakdown of struct conditions
            let cond_gate = self.camera_reg.gate_stability_lp > PORTAL_GATE_OK;
            let cond_notdive = self.camera_reg.rig != CameraRig::PortalDive;
            let cond_cd = self.camera_reg.cooldown_timer <= 0.0;
            let cond_block = self.camera_reg.portal_block_timer <= 0.0;
            let cond_interval = (elapsed - self.camera_reg.last_portal_time) > PORTAL_T_MIN_INTERVAL;
            log::info!(
                "[Portal DIAG] topo={:?} harm_lp={:.2} | struct={} [gate={} notdive={} cd={} block={} interval={}] armed={} timer={:.1}s",
                topo,
                harm_lp,
                structural_ok,
                cond_gate, cond_notdive, cond_cd, cond_block, cond_interval,
                self.camera_reg.portal_enter_armed,
                self.camera_reg.portal_ready_timer
            );
        }

        // === 6. ACCUMULATE OR DECAY READY TIMER ===
        if ready {
            self.camera_reg.portal_ready_timer += dt;
            self.camera_reg.portal_ready_exit_timer = 0.0;  // Reset exit timer
        } else {
            // Decay timer (not instant reset) to prevent "almost but never"
            self.camera_reg.portal_ready_timer = (self.camera_reg.portal_ready_timer - 2.0 * dt).max(0.0);

            // Track time below threshold for hysteresis disarm
            if self.camera_reg.portal_enter_armed && !harm_ok {
                self.camera_reg.portal_ready_exit_timer += dt;
            }
        }

        // === 7. ARM PORTAL WHEN TIMER MATURES ===
        if self.camera_reg.portal_ready_timer >= PORTAL_T_ARM && !self.camera_reg.portal_enter_armed {
            self.camera_reg.portal_enter_armed = true;
            log::info!(
                "[Portal] ARMED: harm_lp={:.2}, gate_lp={:.2}, ready_timer={:.2}s",
                harm_lp, self.camera_reg.gate_stability_lp, self.camera_reg.portal_ready_timer
            );
        }

        // === 8. DISARM IF CONDITIONS DROP FOR TOO LONG ===
        if self.camera_reg.portal_enter_armed && self.camera_reg.portal_ready_exit_timer >= PORTAL_T_EXIT_HOLD {
            self.camera_reg.portal_enter_armed = false;
            self.camera_reg.portal_ready_timer = 0.0;
            self.camera_reg.portal_ready_exit_timer = 0.0;
            log::info!("[Portal] DISARMED: conditions dropped for {:.2}s", PORTAL_T_EXIT_HOLD);
        }

        // === 9. ENTER PORTAL ON DOWNBEAT WHEN ARMED ===
        if self.camera_reg.portal_enter_armed && beat_crossed && beat_confidence > CAM_BEAT_OK {
            // Capture previous rig for event logging
            let prev_rig = self.camera_reg.rig;

            // Enter PORTAL_DIVE
            self.camera_reg.rig = CameraRig::PortalDive;
            self.camera_reg.portal_anchor_k_locked = self.qc_port.port_k_latched;
            self.camera_reg.portal_phase = 0.0;
            self.camera_reg.portal_radius_seq_idx = 0;
            self.camera_reg.fov_step = 2;  // Wide FOV for immersion
            self.camera_reg.hold_timer = 0.0;
            self.camera_reg.cooldown_timer = CAM_T_EXIT_HOLD;

            // Consume armed state and set block timer
            self.camera_reg.portal_enter_armed = false;
            self.camera_reg.portal_ready_timer = 0.0;
            self.camera_reg.portal_block_timer = PORTAL_T_BLOCK;
            self.camera_reg.last_portal_time = elapsed;

            // === INITIALIZE PORTAL FPS ===
            self.init_portal_fps(self.camera_reg.portal_anchor_k_locked);

            // === VALIDATION: PORTAL_ENTER and RIG_CHANGE events ===
            self.reg_validation.portal_enters += 1;
            self.reg_validation.add_event(RegEventType::PortalEnter, self.camera_reg.portal_anchor_k_locked as i32);
            self.reg_validation.rig_changes += 1;
            self.reg_validation.was_in_portal = true;
            self.reg_validation.portal_entry_anchor_k = self.camera_reg.portal_anchor_k_locked;

            // Sync prev state: portal will use locked anchor
            self.reg_validation.prev_anchor_k = self.camera_reg.portal_anchor_k_locked;

            log::info!(
                "[REG_EVENT] PORTAL_ENTER #{}: locked_anchor_k={}, prev_rig={:?}, harm_lp={:.2}, gate_lp={:.2}",
                self.reg_validation.portal_enters,
                self.camera_reg.portal_anchor_k_locked,
                prev_rig,
                harm_lp,
                self.camera_reg.gate_stability_lp
            );
            log::info!(
                "[REG_EVENT] RIG_CHANGE #{}: {:?} → {:?}",
                self.reg_validation.rig_changes,
                prev_rig,
                CameraRig::PortalDive
            );
        }
    }

    /// Update portal dive progress (time-based, not audio-driven)
    fn update_portal_dive_internal(&mut self, dt: f32) {
        // Capture previous state for PORTAL_STEP detection
        let prev_radius_seq_idx = self.camera_reg.portal_radius_seq_idx;

        // === DIAGNOSTIC: Log portal phase progression ===
        let phase_before = self.camera_reg.portal_phase;

        // Advance portal phase
        self.camera_reg.portal_phase += dt / CAM_T_DIVE;

        // Log every ~0.5s of phase progress
        if (phase_before * 10.0).floor() != (self.camera_reg.portal_phase * 10.0).floor() {
            log::info!(
                "[Portal PHASE] dt={:.4} T_DIVE={:.1} phase={:.3}→{:.3}",
                dt, CAM_T_DIVE, phase_before, self.camera_reg.portal_phase
            );
        }

        // Update radius based on phase thresholds
        for (i, &threshold) in PORTAL_PHASE_THRESHOLDS.iter().enumerate() {
            if self.camera_reg.portal_phase >= threshold {
                self.camera_reg.portal_radius_seq_idx = i as u8;
            }
        }
        self.camera_reg.radius_step = PORTAL_RADIUS_SEQ[self.camera_reg.portal_radius_seq_idx as usize];

        // === VALIDATION: PORTAL_STEP event on radius sequence change ===
        if self.camera_reg.portal_radius_seq_idx != prev_radius_seq_idx {
            self.reg_validation.portal_steps += 1;
            log::info!(
                "[REG_EVENT] PORTAL_STEP #{}: seq_idx {} → {}, radius_step=R{}, phase={:.2}",
                self.reg_validation.portal_steps,
                prev_radius_seq_idx,
                self.camera_reg.portal_radius_seq_idx,
                self.camera_reg.radius_step,
                self.camera_reg.portal_phase
            );
        }

        // Azimuth stays locked
        self.camera_reg.anchor_k = self.camera_reg.portal_anchor_k_locked;

        // === VALIDATION: INV-4 Portal lock check ===
        // During portal dive, anchor_k must equal portal_anchor_k_locked
        if self.camera_reg.anchor_k != self.camera_reg.portal_anchor_k_locked {
            self.reg_validation.inv4_portal_lock += 1;
            log::warn!(
                "[REG_VIOLATION] INV-4 PORTAL_LOCK: anchor_k={} but locked={}",
                self.camera_reg.anchor_k, self.camera_reg.portal_anchor_k_locked
            );
        }

        // Exit condition
        if self.camera_reg.portal_phase >= 1.0 {
            // === VALIDATION: PORTAL_EXIT and RIG_CHANGE events ===
            self.reg_validation.portal_exits += 1;
            self.reg_validation.add_event(RegEventType::PortalExit, self.camera_reg.anchor_k as i32);
            self.reg_validation.rig_changes += 1;
            self.reg_validation.was_in_portal = false;

            log::info!(
                "[REG_EVENT] PORTAL_EXIT #{}: returning to Orbit, anchor_k={} (entry was {})",
                self.reg_validation.portal_exits,
                self.camera_reg.anchor_k,
                self.reg_validation.portal_entry_anchor_k
            );
            log::info!(
                "[REG_EVENT] RIG_CHANGE #{}: PortalDive → Orbit",
                self.reg_validation.rig_changes
            );

            // === SYNC VALIDATION PREV STATE ===
            // Portal dive may have changed anchor_k; sync prev state to avoid false INV-2 violations
            self.reg_validation.prev_anchor_k = self.camera_reg.anchor_k;
            self.reg_validation.prev_radius_step = self.camera_reg.radius_step;

            self.camera_reg.rig = CameraRig::Orbit;
            self.camera_reg.fov_step = 1;  // Return to normal FOV
            self.camera_reg.cooldown_timer = CAM_T_COOLDOWN;
            self.camera_reg.portal_phase = 0.0;

            // Deactivate Portal FPS
            self.portal_fps.active = false;
        }

        // Update targets with 24-fold azimuth step
        let az_step = std::f32::consts::TAU / 24.0;
        self.update_camera_targets_internal(az_step);
    }

    // ========================================================================
    // PORTAL FPS MODE - Rail Shooter During Portal Dive
    // ========================================================================
    //
    // When entering portal dive, transform into first-person rail shooter:
    //   - Forward motion through 8 segments with varying speed/density
    //   - Discrete lane switching (±1 step per onset, 5 lanes: -2 to +2)
    //   - Micro sway for liveliness (bounded, no full orbit)
    //   - Video rails bound to 24-fold seams
    //   - Near-miss bar events (bounded buffer)
    //   - VR-safe: no roll, no free yaw, discrete targets only
    //
    // ========================================================================

    /// Initialize portal FPS state on portal entry
    fn init_portal_fps(&mut self, locked_anchor_k: u8) {
        let fps = &mut self.portal_fps;

        // Activate FPS mode
        fps.active = true;

        // Choose path deterministically from current state
        // Use latched chroma % 4 for variety
        fps.path_id = match self.presentation.dominant_chroma_idx_latched.unsigned_abs() % 4 {
            0 => PortalPath::TunnelGrid,
            1 => PortalPath::Corridor,
            2 => PortalPath::RibLanes,
            _ => PortalPath::KnotPass,
        };

        // Reset lane state
        fps.lane_idx = 0;
        fps.lane_target_idx = 0;
        fps.lane_hold_timer = 0.0;
        fps.lane_cooldown_timer = 0.0;
        fps.lane_transition_t = 1.0;  // Fully at current position

        // Reset segment state
        fps.segment_id = 0;
        fps.forward_phase = 0.0;

        // Load initial segment parameters
        let seg = &PORTAL_SEGMENTS[0];
        fps.speed_tier = seg.speed_tier;
        fps.density_tier = seg.density_tier;
        fps.near_miss_budget_remaining = seg.near_miss_budget;

        // Initialize sway
        fps.sway_tier = 1;
        fps.sway_phase = 0.0;

        // Set seed for rail variation
        fps.line_set_seed = (locked_anchor_k as u32).wrapping_mul(31337)
            .wrapping_add(self.camera_reg.beat_count);
        fps.line_scroll_phase = 0.0;

        // Clear near-miss buffer
        fps.near_miss_active_count = 0;
        for bar in fps.near_miss_bars.iter_mut() {
            bar.active = false;
        }

        // Compute locked frame from anchor
        // F = (cos θ, 0, sin θ), U = (0, 1, 0), R = cross(U, F) = (-sin θ, 0, cos θ)
        let theta = locked_anchor_k as f32 * (std::f32::consts::TAU / 24.0);
        fps.frame_forward_x = theta.cos();
        fps.frame_forward_z = theta.sin();
        fps.frame_right_x = -theta.sin();
        fps.frame_right_z = theta.cos();

        // Reset tracking
        fps.prev_dominant_chroma = self.presentation.dominant_chroma_idx_latched as u8;
        fps.prev_beat_phase = self.beat_phase_accum;

        // Initialize "make it hit" accent scalars
        fps.lane_impulse = 0.0;
        fps.lane_impulse_dir = 0;
        fps.vista_pulse = 0.0;
        fps.hit_emphasis = 0.0;
        fps.corridor_tier = seg.corridor_tier;
        fps.scaffold_emphasis = 0.0;
        fps.rail_group_pulse = [0.0; 3];

        // InsideView camera state: aperture starts closed, z_offset at origin
        fps.entry_aperture = 1.0;  // Iris fully closed on entry, opens over ~200ms
        fps.z_offset = 0.0;        // Start at portal entry point

        log::info!(
            "[PortalFPS] INIT: path={:?}, anchor_k={}, corridor={}, frame=({:.2},{:.2})",
            fps.path_id,
            locked_anchor_k,
            fps.corridor_tier,
            fps.frame_forward_x,
            fps.frame_forward_z
        );
    }

    /// Update portal FPS state during portal dive
    /// "Make it hit" version with parallax, symmetric near-miss, and vista support
    fn update_portal_fps(&mut self, dt: f32, onset_detected: bool, beat_crossed: bool) {
        if !self.portal_fps.active {
            return;
        }

        // Get current beat phase for offbeat detection
        let beat_phase = self.beat_phase_accum;
        let is_offbeat = beat_phase > 0.4 && beat_phase < 0.6;

        // Cache anchor for near-miss calculations
        let anchor_k = self.camera_reg.portal_anchor_k_locked;

        let fps = &mut self.portal_fps;

        // === 0. DECAY ALL ACCENT SCALARS ===
        fps.lane_impulse *= (-dt * PORTAL_FPS_IMPULSE_DECAY).exp();
        fps.hit_emphasis *= (-dt * PORTAL_FPS_HIT_DECAY).exp();
        fps.scaffold_emphasis *= (-dt * 4.0).exp();  // Slower decay for vista
        for pulse in fps.rail_group_pulse.iter_mut() {
            *pulse *= (-dt * PORTAL_FPS_RAIL_PULSE_DECAY).exp();
        }
        if fps.lane_impulse < 0.01 { fps.lane_impulse = 0.0; }
        if fps.hit_emphasis < 0.01 { fps.hit_emphasis = 0.0; }
        if fps.scaffold_emphasis < 0.01 { fps.scaffold_emphasis = 0.0; }

        // Entry aperture decay (~200ms = 5/s decay rate)
        fps.entry_aperture *= (-dt * 5.0).exp();
        if fps.entry_aperture < 0.01 { fps.entry_aperture = 0.0; }

        // === 1. FORWARD MOTION (segment-based) ===
        let speed_mult = PORTAL_SPEED_MULT[fps.speed_tier as usize].min(1.5);
        let forward_delta = dt * speed_mult * PORTAL_FPS_FORWARD_RATE;
        fps.forward_phase += forward_delta;

        // Accumulate Z offset for camera (corridor depth perception)
        // Z offset grows continuously to give forward motion feel
        const PORTAL_Z_RATE: f32 = 0.5;  // Units per second at speed 1.0
        fps.z_offset += forward_delta * PORTAL_Z_RATE;

        // Check for segment transition
        if fps.forward_phase >= 1.0 {
            fps.forward_phase = 0.0;
            let prev_segment = fps.segment_id;

            // === THREAT DEBT: Track missed spawns from previous segment ===
            if prev_segment < PORTAL_SEGMENTS.len() as u8 {
                let prev_seg_type = PORTAL_SEGMENTS[prev_segment as usize].seg_type;
                let was_threat = prev_seg_type == SegmentType::Threat || prev_seg_type == SegmentType::Needle;
                if was_threat && !fps.nm_spawned_in_segment {
                    // Threat segment ended with 0 spawns -> accumulate debt
                    fps.threat_debt = fps.threat_debt.saturating_add(1).min(3);  // Cap at 3
                    log::info!("[PortalFPS] THREAT_DEBT: no spawns in seg {}, debt={}", prev_segment, fps.threat_debt);
                }
            }

            fps.segment_id = (fps.segment_id + 1).min(PORTAL_SEGMENTS.len() as u8 - 1);

            // Track segment advance event
            if fps.segment_id != prev_segment {
                self.reg_validation.fps_segment_advances += 1;
                self.reg_validation.add_event(RegEventType::SegmentAdvance, fps.segment_id as i32);
            }

            // Load new segment parameters
            let seg = &PORTAL_SEGMENTS[fps.segment_id as usize];
            fps.speed_tier = seg.speed_tier;
            fps.density_tier = seg.density_tier;
            fps.near_miss_budget_remaining = seg.near_miss_budget;
            fps.corridor_tier = seg.corridor_tier;

            // Reset segment tracking for new segment
            fps.nm_spawned_in_segment = false;
            fps.beats_since_spawn_attempt = 0;

            // === VISTA SEGMENT HANDLING ===
            // On entering Vista, trigger scaffold emphasis
            if seg.seg_type == SegmentType::Vista {
                fps.vista_pulse = 1.0;
                fps.scaffold_emphasis = PORTAL_FPS_VISTA_PEAK;
                log::info!("[PortalFPS] VISTA: corridor widens, showing shell silhouette");
            }

            log::info!(
                "[PortalFPS] SEGMENT {} ({:?}): spd={}, dns={}, corridor={}, nm={}, debt={}",
                fps.segment_id,
                seg.seg_type,
                fps.speed_tier,
                fps.density_tier,
                fps.corridor_tier,
                fps.near_miss_budget_remaining,
                fps.threat_debt
            );
        }

        // Update vista pulse (time-based decay during Vista segment)
        let current_seg = &PORTAL_SEGMENTS[fps.segment_id as usize];
        if current_seg.seg_type == SegmentType::Vista {
            fps.vista_pulse = (fps.vista_pulse - dt / PORTAL_FPS_VISTA_DURATION).max(0.0);
        } else {
            fps.vista_pulse = 0.0;
        }

        // === 2. ROBUST LANE SWITCHING (chroma-peak intent with hysteresis) ===
        fps.lane_hold_timer += dt;
        fps.lane_cooldown_timer = (fps.lane_cooldown_timer - dt).max(0.0);

        // Get current beat_id for intent tracking
        let beat_id = self.reg_validation.beat_id;

        // On downbeat: compute lane intent from chroma peak (once per beat)
        if beat_crossed && fps.lane_intent_beat_id != beat_id {
            fps.lane_intent_beat_id = beat_id;

            // Find chroma peak = argmax(smoothed_chroma[12])
            let mut peak_bin: u8 = 0;
            let mut peak_val: f32 = self.smoothed_chroma[0];
            for i in 1..12 {
                if self.smoothed_chroma[i] > peak_val {
                    peak_val = self.smoothed_chroma[i];
                    peak_bin = i as u8;
                }
            }

            // Map peak bins to intent:
            //   bins 0-3  => +1 (strafe right)
            //   bins 4-7  => 0  (hold)
            //   bins 8-11 => -1 (strafe left)
            let new_intent: i8 = if peak_bin <= 3 {
                1
            } else if peak_bin <= 7 {
                0
            } else {
                -1
            };

            // Hysteresis RELAXED inside portal: confirm immediately (1 beat)
            // Outside portal we'd require 2 consecutive beats, but inside
            // the shorter response feels more responsive and game-like
            fps.lane_intent_confirmed = true;
            fps.lane_intent = new_intent;
            fps.lane_intent_prev = new_intent;
        }

        // Permission check for lane commit
        let energy = self.section_energy;
        let can_commit_lane = fps.lane_cooldown_timer <= 0.0
            && fps.lane_hold_timer > PORTAL_FPS_LANE_HOLD_MIN
            && fps.lane_intent_confirmed
            && fps.lane_intent != 0
            && energy > 0.1;  // Not in silence

        // Commit lane change on onset (preferred) or downbeat
        let commit_trigger = onset_detected || beat_crossed;

        if can_commit_lane && commit_trigger {
            // Apply confirmed intent with ±1 step limit
            let new_target = (fps.lane_target_idx + fps.lane_intent).clamp(-2, 2);

            if new_target != fps.lane_target_idx {
                let old_target = fps.lane_target_idx;
                fps.lane_target_idx = new_target;
                fps.lane_transition_t = 0.0;
                fps.lane_cooldown_timer = PORTAL_FPS_LANE_COOLDOWN;
                fps.lane_hold_timer = 0.0;

                // Clear intent after applying (requires 2 more beats to re-confirm)
                fps.lane_intent_confirmed = false;

                // === PARALLAX IMPULSE ===
                fps.lane_impulse = PORTAL_FPS_IMPULSE_MAG[fps.speed_tier as usize];
                fps.lane_impulse_dir = fps.lane_intent;

                // Track lane switch event
                self.reg_validation.fps_lane_switches += 1;
                self.reg_validation.add_event(RegEventType::LaneSwitch, new_target as i32);

                log::info!(
                    "[PortalFPS] LANE_SWITCH: {} → {} (intent={:+}, trigger={})",
                    old_target,
                    new_target,
                    fps.lane_intent,
                    if onset_detected { "onset" } else { "beat" }
                );
            }
        }

        // Commit lane position on beat crossing (latch target to actual)
        if beat_crossed && fps.lane_idx != fps.lane_target_idx {
            let prev_lane = fps.lane_idx;
            // Step limit: only ±1 per beat
            let step = (fps.lane_target_idx - fps.lane_idx).clamp(-1, 1);
            fps.lane_idx += step;
            fps.lane_transition_t = 1.0;
            log::info!("[PortalFPS] LANE_COMMIT: {} → {} (step={})", prev_lane, fps.lane_idx, step);
        }

        // Smooth lane transition
        if fps.lane_transition_t < 1.0 {
            fps.lane_transition_t = (fps.lane_transition_t + dt * PORTAL_FPS_LANE_TRANSITION_RATE).min(1.0);
        }

        // === 3. VIDEO RAIL GROUP PULSING ===
        // Group 0 = downbeat, Group 1 = offbeat, Group 2 = onset
        if beat_crossed {
            fps.rail_group_pulse[0] = 1.0;  // Downbeat pulse
        }
        if is_offbeat && fps.prev_beat_phase < 0.4 {
            fps.rail_group_pulse[1] = 1.0;  // Offbeat pulse
        }
        if onset_detected {
            fps.rail_group_pulse[2] = 0.8;  // Onset pulse (slightly softer)
        }
        fps.prev_beat_phase = beat_phase;

        // === 4. MICRO SWAY (bounded harmonic wobble) ===
        fps.sway_phase = (fps.sway_phase + dt * 0.5) % 1.0;

        // === 5. VIDEO RAIL SCROLL ===
        fps.line_scroll_phase = (fps.line_scroll_phase + dt * speed_mult * 0.4) % 1.0;

        // === 6. ROBUST NEAR-MISS SPAWNING (with threat debt fallback) ===
        let budget = fps.near_miss_budget_remaining;
        let seg_type = current_seg.seg_type;
        let is_threat_segment = seg_type == SegmentType::Threat || seg_type == SegmentType::Needle;

        // Track beats since last spawn attempt for fallback timing
        if beat_crossed {
            fps.beats_since_spawn_attempt += 1;
        }

        // Spawn trigger: strong onset OR downbeat fallback (if no onset in 2+ beats)
        let strong_onset = onset_detected && self.section_energy > 0.15;
        let downbeat_fallback = beat_crossed && fps.beats_since_spawn_attempt >= 2;
        let spawn_trigger = strong_onset || downbeat_fallback;

        // Check if we should spawn (either from budget or debt)
        let should_spawn_from_budget = is_threat_segment && budget > 0 && spawn_trigger;
        let should_spawn_from_debt = is_threat_segment && fps.threat_debt > 0 && beat_crossed;

        if should_spawn_from_budget || should_spawn_from_debt {
            let beat_id = self.reg_validation.beat_id;

            // Determine spawn count and wedge selection
            let effective_budget = if should_spawn_from_budget {
                budget
            } else {
                1  // Debt spawns exactly 1
            };

            if effective_budget >= 2 {
                // === SYMMETRIC PAIR: anchor+1 and anchor-1 ===
                Self::spawn_near_miss_bar(fps, anchor_k, 1, fps.lane_idx);
                Self::spawn_near_miss_bar(fps, anchor_k, -1, fps.lane_idx);
                fps.near_miss_budget_remaining = fps.near_miss_budget_remaining.saturating_sub(2);
                fps.hit_emphasis = 1.0;
                fps.nm_spawned_in_segment = true;
                fps.beats_since_spawn_attempt = 0;
                // Track spawns
                self.reg_validation.fps_near_miss_spawns += 2;
                self.reg_validation.add_event(RegEventType::NearMissSpawn, 2);
                log::info!("[PortalFPS] NEAR_MISS PAIR: k±1 from anchor {} (budget)", anchor_k);
            } else {
                // === SINGLE BAR: deterministic wedge from beat_id % 3 ===
                let offset: i8 = match beat_id % 3 {
                    0 => 0,   // Center
                    1 => 1,   // Right
                    _ => -1,  // Left
                };
                Self::spawn_near_miss_bar(fps, anchor_k, offset, fps.lane_idx);
                fps.hit_emphasis = 0.8;
                fps.nm_spawned_in_segment = true;
                fps.beats_since_spawn_attempt = 0;

                // Consume budget or debt
                if should_spawn_from_budget {
                    fps.near_miss_budget_remaining = fps.near_miss_budget_remaining.saturating_sub(1);
                    log::info!("[PortalFPS] NEAR_MISS SINGLE: k{:+} from anchor {} (budget)", offset, anchor_k);
                } else {
                    fps.threat_debt = fps.threat_debt.saturating_sub(1);
                    log::info!("[PortalFPS] NEAR_MISS SINGLE: k{:+} from anchor {} (DEBT repay, remaining={})",
                        offset, anchor_k, fps.threat_debt);
                }

                // Track spawn
                self.reg_validation.fps_near_miss_spawns += 1;
                self.reg_validation.add_event(RegEventType::NearMissSpawn, 1);
            }

            // Track max active bars
            if fps.near_miss_active_count > self.reg_validation.fps_max_active_bars {
                self.reg_validation.fps_max_active_bars = fps.near_miss_active_count;
            }
        }

        // Update active near-miss bars
        for bar in fps.near_miss_bars.iter_mut() {
            if bar.active {
                bar.lifetime -= dt * speed_mult * 1.2;

                // Progress radius tier as bar approaches (creates depth)
                if bar.lifetime < 0.8 && bar.radius_tier == 2 {
                    bar.radius_tier = 1;
                } else if bar.lifetime < 0.3 && bar.radius_tier == 1 {
                    bar.radius_tier = 0;
                }

                // Despawn when passed
                if bar.lifetime <= 0.0 {
                    bar.active = false;
                    fps.near_miss_active_count = fps.near_miss_active_count.saturating_sub(1);
                }
            }
        }
    }

    /// Helper to spawn a near-miss bar with lane-aware radius
    fn spawn_near_miss_bar(fps: &mut PortalFpsState, anchor_k: u8, k_offset: i8, lane_idx: i8) {
        // Find inactive slot
        for bar in fps.near_miss_bars.iter_mut() {
            if !bar.active {
                bar.active = true;
                bar.lifetime = 1.2;  // Time to pass player

                // Wedge position: anchor + offset, wrapped to 24-fold
                bar.wedge_k = ((anchor_k as i16 + k_offset as i16 + 24) % 24) as u8;

                // Radius tier based on lane: center lane = closer bars
                // Outer lanes get slightly farther tiers for comfort
                bar.radius_tier = match lane_idx.abs() {
                    0 => 1,     // Center: start at mid tier (feels close)
                    1 => 1,     // Adjacent: also mid tier
                    _ => 2,     // Outer: start farther (more comfortable)
                };

                fps.near_miss_active_count += 1;
                break;
            }
        }
    }

    // ========================================================================
    // QC PORT SELECTION - Quaternion Charge to Camera Wedge Mapping
    // ========================================================================
    //
    // Deterministic mapping from QC state to a 24-fold wedge anchor:
    //   1. Compute 4 port angles from w: [w, π-w, π+w, τ-w]
    //   2. Quantize each to nearest 24-fold anchor (round, not floor)
    //   3. Select port based on quadrant (hemisphere pair) + closeness
    //   4. Latch with hysteresis and ±1 step limiting
    //
    // ========================================================================

    /// Update QC port selection (deterministic mapping from QC state)
    pub fn update_qc_port(&mut self, dt: f32, beat_crossed: bool) {
        let qc = &self.qc;
        let port = &mut self.qc_port;

        // === 1. ELIGIBILITY CHECK ===
        let eligible = qc.invariants.lattice_ok_24
            && self.stability_smooth > CAM_GATE_OK;

        if !eligible {
            port.cooldown_timer = (port.cooldown_timer - dt).max(0.0);
            return;
        }

        // === 2. COMPUTE 4 PORT WEDGE INDICES ===
        let w = qc.w;
        let ports_rad = [
            w,                                          // A0 = w
            std::f32::consts::PI - w,                   // A1 = π - w
            std::f32::consts::PI + w,                   // A2 = π + w
            std::f32::consts::TAU - w,                  // A3 = τ - w
        ];

        let az_step_24 = std::f32::consts::TAU / 24.0;
        let mut k_ports = [0u8; 4];
        let mut closeness = [0.0f32; 4];

        for i in 0..4 {
            let angle = ports_rad[i].rem_euclid(std::f32::consts::TAU);
            // Use round() for nearest-anchor quantization (REG-true)
            let k_raw = (angle / az_step_24).round() as u8 % 24;
            k_ports[i] = k_raw;

            // Closeness: how near the port is to its quantized anchor
            let err = (angle - k_raw as f32 * az_step_24).abs();
            closeness[i] = 1.0 - (err / (az_step_24 / 2.0)).min(1.0);
        }
        port.closeness = closeness;

        // === 3. PORT SELECTION POLICY ===
        // Quadrant chooses hemisphere pair
        let (pair_a, pair_b) = match qc.current_quadrant {
            0 | 1 => (0, 1),  // Q1/Q2 (upper): prefer A0, A1
            _ => (2, 3),      // Q3/Q4 (lower): prefer A2, A3
        };

        // Choose from pair based on closeness + proximity to latched
        let dist_a = shortest_wrapped_delta(k_ports[pair_a] as i16, port.port_k_latched as i16, 24).abs();
        let dist_b = shortest_wrapped_delta(k_ports[pair_b] as i16, port.port_k_latched as i16, 24).abs();

        // Prefer higher closeness, then smaller distance to latched
        let score_a = closeness[pair_a] - dist_a as f32 * 0.05;
        let score_b = closeness[pair_b] - dist_b as f32 * 0.05;

        let selected = if score_a >= score_b { pair_a } else { pair_b };
        port.port_id_active = selected as u8;
        port.port_k_target = k_ports[selected];

        // === 4. LP FILTER CONFIDENCE ===
        let tau = 0.4;
        let alpha = 1.0 - (-dt / tau).exp();
        let qc_conf = if qc.invariants.lattice_ok_24 { 1.0 } else { 0.0 };
        port.port_conf_lp = port.port_conf_lp + (qc_conf - port.port_conf_lp) * alpha;

        // === 5. LATCH UPDATE (only on commit events) ===
        port.hold_timer += dt;
        port.cooldown_timer = (port.cooldown_timer - dt).max(0.0);

        let can_commit = beat_crossed
            && port.hold_timer > CAM_T_PORT_HOLD
            && port.cooldown_timer <= 0.0
            && self.camera_reg.rig != CameraRig::PortalDive;  // No changes during dive

        if can_commit {
            // Capture prev state for validation
            let prev_k_latched = port.port_k_latched;

            // Step-limit toward target
            let delta = shortest_wrapped_delta(
                port.port_k_target as i16,
                port.port_k_latched as i16,
                24
            );
            let clamped = delta.clamp(-1, 1);
            let new_latched = ((port.port_k_latched as i16 + clamped).rem_euclid(24)) as u8;

            port.port_k_latched = new_latched;
            port.hold_timer = 0.0;
            port.cooldown_timer = CAM_T_PORT_COOLDOWN;

            // === VALIDATION: QC_COMMIT event logging ===
            self.reg_validation.qc_commits += 1;
            self.reg_validation.add_event(RegEventType::QcCommit, new_latched as i32);
            log::info!(
                "[REG_EVENT] QC_COMMIT #{}: port_k_latched={} (was {}), target={}, delta={}, beat_id={}",
                self.reg_validation.qc_commits,
                new_latched, prev_k_latched,
                self.qc_port.port_k_target, clamped,
                self.reg_validation.beat_id
            );

            // INV-2: Step limit (±1 max per commit)
            let actual_delta = shortest_wrapped_delta(
                new_latched as i16,
                self.reg_validation.prev_qc_k_latched as i16,
                24
            ).abs();

            if actual_delta > 1 {
                self.reg_validation.inv2_step_limit += 1;
                log::warn!(
                    "[REG_VIOLATION] INV-2 STEP_LIMIT (QC): qc_k_delta={} (max allowed: 1)",
                    actual_delta
                );
            }

            // INV-3: Beat gating (no double-commits on same beat)
            let current_beat_id = self.reg_validation.beat_id;
            if current_beat_id == self.reg_validation.last_qc_commit_beat_id {
                self.reg_validation.inv3_beat_gating += 1;
                log::warn!(
                    "[REG_VIOLATION] INV-3 BEAT_GATING: double QC_COMMIT on beat_id={}",
                    current_beat_id
                );
            }
            self.reg_validation.last_qc_commit_beat_id = current_beat_id;

            // Update previous state
            self.reg_validation.prev_qc_k_latched = new_latched;
        }
    }

    // ========================================================================
    // QUATERNION CHARGES - REG Charge Visualization
    // ========================================================================

    /// Update QuaternionCharges state
    /// Active during fold24 transition window (0.10 < t < 0.90) when inside
    fn update_quaternion_charges(&mut self, dt: f32, dominant_pc: usize, energy: f32) {
        use quaternion_charges::*;

        // Check activation window
        let inside = self.interior_state.is_inside();
        let fold24_active = self.fold24.active;
        let in_window = self.fold24.t > 0.10 && self.fold24.t < 0.90;

        self.qc.active = inside && fold24_active && in_window;

        // Always reset polarity_changed at start of each frame
        self.qc.polarity_changed = false;

        if !self.qc.active {
            // Reset strength when inactive for clean fade
            self.qc.strength = 0.0;
            // Reset quadrant hold timer when inactive
            self.qc.quadrant_hold_timer = 0.0;
            return;
        }

        // Derive harmonic angle from dominant pitch class
        // Each pitch class is 30° (π/6 radians), which is on the 15° lattice (π/12).
        // REG CONTRACT: Input must be pre-quantized to 15° grid for lattice_ok_24 to lock.
        // Since 30° is a multiple of 15°, this is satisfied by using integer pitch_class.
        // If continuous/unquantized angles were fed here, QC invariants would appear unstable.
        let a = (dominant_pc as f32) * std::f32::consts::FRAC_PI_6;

        // Compute quaternion state
        self.qc.w = reduce_to_w(a);

        // === VALIDATION: INV-6 Unit contract ===
        // Check that input angle `a` is on the 15° (π/12) lattice
        let lattice_step = std::f32::consts::PI / 12.0;  // 15°
        let lattice_k = (a / lattice_step).round();
        let quantization_error = (a - lattice_k * lattice_step).abs();
        let on_lattice = quantization_error < 0.001;  // Small tolerance for float precision

        // Check that w is in expected range [0, π/2) per reduce_to_w()
        let w_in_range = self.qc.w >= 0.0 && self.qc.w < std::f32::consts::FRAC_PI_2 + 0.001;

        if !on_lattice || !w_in_range {
            self.reg_validation.inv6_unit_contract += 1;
            log::warn!(
                "[REG_VIOLATION] INV-6 UNIT_CONTRACT: a={:.4} (err={:.6}), w={:.4} (range [0, π/2))",
                a, quantization_error, self.qc.w
            );
        }
        self.qc.ports = quaternion_ports(self.qc.w);
        self.qc.charges = [
            charge_for_angle(self.qc.ports[0]),
            charge_for_angle(self.qc.ports[1]),
            charge_for_angle(self.qc.ports[2]),
            charge_for_angle(self.qc.ports[3]),
        ];
        self.qc.invariants = compute_invariants(&self.qc.ports);

        // Active port = nearest to camera aim angle with hysteresis
        self.qc.active_port = compute_active_port_nearest(
            self.aim_angle,
            &self.qc.ports,
            self.qc.active_port,
        );

        // Overlay strength from fold24_t (smoothstep ramp up/down)
        self.qc.strength = compute_qc_strength(self.fold24.t);

        // === POLARITY / QUADRANT DETECTION (REG-True) ===
        // Compute degree A from pitch class AND energy (not just pitch class)
        // This uses the canonical REG formula: A = ring * 24 + spoke_offset
        let degree_a = compute_degree_a(dominant_pc, energy);
        self.qc.current_degree = degree_a;

        // Compute quadrant from degree A (which charge axis it's nearest to)
        let new_quadrant = quadrant_from_degree(degree_a);

        // Hysteresis: 50ms hold before committing quadrant change (reduced for responsiveness)
        const QUADRANT_HYSTERESIS: f32 = 0.05;

        if new_quadrant != self.qc.current_quadrant {
            self.qc.quadrant_hold_timer += dt;
            if self.qc.quadrant_hold_timer >= QUADRANT_HYSTERESIS {
                // Commit the quadrant change
                self.qc.prev_quadrant = self.qc.current_quadrant;
                self.qc.polarity_direction =
                    quadrant_direction(self.qc.prev_quadrant, new_quadrant);
                self.qc.current_quadrant = new_quadrant;
                self.qc.polarity_changed = true;
                self.qc.quadrant_hold_timer = 0.0;

                // Toggle QS wheel visibility on polarity change when in torus mode
                if self.torus_knot.active {
                    self.qs_wheel_visible = !self.qs_wheel_visible;
                    log::trace!("Torus mode: QS wheel toggled to {} on polarity change", self.qs_wheel_visible);
                }

                // Log polarity change for debugging
                let quadrant_names = ["E(+1)", "N(+i)", "W(-1)", "S(-i)"];
                let dir_str = if self.qc.polarity_direction > 0 {
                    "CW"
                } else {
                    "CCW"
                };
                log::info!(
                    "Polarity change: {}→{} (deg={}, dir={})",
                    quadrant_names[self.qc.prev_quadrant as usize],
                    quadrant_names[self.qc.current_quadrant as usize],
                    degree_a,
                    dir_str
                );

                // === TRIGGER POLARITY EFFECT ===
                // Start effect timer at 3.0 (longer duration for label visibility)
                self.qc.effect_timer = 3.0;
                // Effect angle: center of new quadrant (0=E, 90=N, 180=W, 270=S)
                self.qc.effect_angle = (new_quadrant as f32) * std::f32::consts::FRAC_PI_2;
                self.qc.effect_intensity = 1.0;
            }
        } else {
            // Same quadrant, reset hold timer
            self.qc.quadrant_hold_timer = 0.0;
        }

        // === UPDATE POLARITY EFFECT TIMER ===
        if self.qc.effect_timer > 0.0 {
            self.qc.effect_timer = (self.qc.effect_timer - dt).max(0.0);
        }
    }

    /// Apply camera effects when polarity changes
    /// - Yaw nudge: ±7.5° (π/24) based on polarity direction
    /// - Orbit direction: flip sign on each polarity change
    fn apply_polarity_camera_effects(&mut self, dt: f32) {
        // Only apply effects during fold state (when 24-fold is active)
        let in_fold_state = self.fold24.active || self.fold24.t > 0.1;

        if self.qc.polarity_changed && in_fold_state {
            // Yaw nudge: ±7.5° (π/24 radians) based on polarity direction
            self.polarity_yaw_nudge =
                (std::f32::consts::PI / 24.0) * self.qc.polarity_direction as f32;

            // Flip orbit direction
            self.polarity_orbit_sign *= -1.0;

            log::debug!(
                "Polarity camera: yaw_nudge={:.2}°, orbit_sign={:.0}",
                self.polarity_yaw_nudge.to_degrees(),
                self.polarity_orbit_sign
            );
        }

        // Decay yaw nudge exponentially (tau ~0.3s)
        // exp(-dt * 3.0) gives ~0.3s time constant
        self.polarity_yaw_nudge *= (-dt * 3.3).exp();

        // Clamp to avoid floating point accumulation
        if self.polarity_yaw_nudge.abs() < 0.001 {
            self.polarity_yaw_nudge = 0.0;
        }
    }

    /// Select which frequency band to target for RingDive portal
    fn select_ring_dive_target_band(&self) -> usize {
        // Choose band with highest energy during trigger
        // For simplicity, use dominance_smooth to pick bass(0), mid(1), or high(2)
        if self.dominance_smooth > 0.6 {
            0 // Bass (strong beat dominance)
        } else if self.dominance_smooth > 0.3 {
            1 // Mid (melodic content)
        } else {
            2 // High (detailed/sparse content)
        }
    }

    /// Generate Catmull-Rom spline path for RingDive camera rail
    fn generate_ring_dive_path(&mut self, dominant_pc: usize) {
        let dom_pc = dominant_pc as f32;
        let angle = dom_pc * std::f32::consts::FRAC_PI_6;
        let lateral_x = angle.cos() * 0.8;
        let lateral_z = angle.sin() * 0.8;

        // Current camera position approximation
        let p0 = [0.0, 0.0, self.distance];
        let p1 = [lateral_x * 0.3, -0.2, self.distance - 1.0];  // Forward + slight down
        let p2 = [lateral_x * 0.5, -0.15, 0.5];                 // Deep inside
        let p3 = [lateral_x * 0.2, 0.1, 1.0];                   // Rising out
        let p4 = [0.0, 0.0, self.distance * 0.9];               // Return near start

        self.ring_dive.path_points = [p0, p1, p2, p3, p4];

        // Initialize look target
        self.ring_dive.look_target = [0.0, 0.0, 0.0];
        self.ring_dive.look_target_vel = [0.0, 0.0, 0.0];
    }

    /// Evaluate Catmull-Rom spline at t
    fn catmull_rom_eval(&self, t: f32) -> [f32; 3] {
        let pts = &self.ring_dive.path_points;
        let n = pts.len() - 1;
        let segment = ((t * n as f32) as usize).min(n - 1);
        let local_t = (t * n as f32) - segment as f32;

        // Get 4 control points (clamp at ends)
        let p0 = pts[segment.saturating_sub(1)];
        let p1 = pts[segment];
        let p2 = pts[(segment + 1).min(n)];
        let p3 = pts[(segment + 2).min(n)];

        // Catmull-Rom with tension 0.5
        Self::catmull_rom_point(p0, p1, p2, p3, local_t, 0.5)
    }

    /// Compute single Catmull-Rom point
    fn catmull_rom_point(
        p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3],
        t: f32, tension: f32
    ) -> [f32; 3] {
        let t2 = t * t;
        let t3 = t2 * t;

        let s = (1.0 - tension) / 2.0;

        let mut result = [0.0f32; 3];
        for i in 0..3 {
            let m0 = s * (p2[i] - p0[i]);
            let m1 = s * (p3[i] - p1[i]);

            result[i] = (2.0 * t3 - 3.0 * t2 + 1.0) * p1[i]
                + (t3 - 2.0 * t2 + t) * m0
                + (-2.0 * t3 + 3.0 * t2) * p2[i]
                + (t3 - t2) * m1;
        }
        result
    }

    /// Update camera position and look target during RingDive
    fn update_ring_dive_camera(&mut self, dt: f32) {
        if !self.ring_dive.active { return; }

        let t = self.ring_dive.t;
        let sub_t = self.ring_dive.subrange_t();
        let subrange = self.ring_dive.subrange();

        // Position along spline (mapped to subranges)
        let spline_t = match subrange {
            "lock_on" => smoothstep(0.0, 1.0, sub_t) * 0.2,       // Ease in slowly
            "warp_out" => 0.2 + sub_t * 0.4,                      // Accelerate through
            "guided_flight" => 0.6 + sub_t * 0.25,                // Cruise inside
            "warp_back" => 0.85 + sub_t * 0.15,                   // Return
            _ => 0.0,
        };

        let _path_pos = self.catmull_rom_eval(spline_t);

        // Rail blend factor (how much camera follows the rail vs normal behavior)
        // WarpBack has STRONG blend to force camera to ring-viewing position
        let rail_blend = match subrange {
            "lock_on" => smoothstep(0.0, 1.0, sub_t) * 0.5,
            "warp_out" => 0.5 + sub_t * 0.5,
            "guided_flight" => 1.0,
            "warp_back" => 1.0,  // FULL blend during warp back - force camera position
            _ => 0.0,
        };

        // Override distance during RingDive
        // WarpBack: force camera to portal-viewing distance (where you can see rings)
        // Last 10% (sub_t >= 0.5): LOCK camera distance for clean ring reveal
        let portal_viewing_dist = 2.8;  // Optimal distance to see harmonic rings
        let target_dist = match subrange {
            "lock_on" => lerp(self.distance, 2.2, sub_t * 0.5),
            "warp_out" => lerp(2.2, 1.2, sub_t),
            "guided_flight" => lerp(1.2, 1.5, sub_t * 0.3),
            // WarpBack: reach target by sub_t=0.5, then LOCK
            "warp_back" => {
                let reach_t = smoothstep(0.0, 0.5, sub_t);  // Reaches 1.0 at sub_t=0.5
                lerp(1.5, portal_viewing_dist, reach_t)
            },
            _ => self.distance,
        };
        // WarpBack: STRONGER lerp in first half, then locked in last 10%
        let dist_lerp_strength = match subrange {
            "warp_back" => if sub_t > 0.5 { 0.5 } else { 0.35 },  // Very strong to reach target fast
            _ => 0.15,
        };
        self.distance = lerp(self.distance, target_dist, rail_blend * dist_lerp_strength);

        // Override inside_dip during dive
        // WarpBack: interior fades to 0 by t=0.90 (sub_t=0.5)
        // Then rings are visible for clean reveal
        let target_dip = match subrange {
            "lock_on" => 0.1 * sub_t,
            "warp_out" => 0.1 + sub_t * 0.6,
            "guided_flight" => 0.7 + sub_t * 0.1,
            // WarpBack: FAST interior exit - done by sub_t=0.5 (t=0.90)
            "warp_back" => lerp(0.8, 0.0, smoothstep(0.0, 0.5, sub_t)),
            _ => 0.0,
        };
        // WarpBack uses STRONG lerp to force interior exit fast
        let dip_lerp_strength = if subrange == "warp_back" { 0.4 } else { 0.2 };
        self.inside_dip = lerp(self.inside_dip, target_dip, rail_blend * dip_lerp_strength);

        // Warp intensity (visual streaking)
        self.warp_intensity = match subrange {
            "lock_on" => 0.0,
            "warp_out" => smoothstep(0.0, 0.5, sub_t) * (1.0 - smoothstep(0.7, 1.0, sub_t)),
            "guided_flight" => 0.0,
            // WarpBack: warp fades out by t=0.6 so rings are clearly visible
            "warp_back" => smoothstep(0.0, 0.3, sub_t) * (1.0 - smoothstep(0.4, 0.7, sub_t)),
            _ => 0.0,
        };

        // === ORBIT SUPPRESSION during guided entry ===
        // Smoothly suppress orbit for 0.5s on entering guided to avoid "sliding along surface"
        // Uses spring targets for fluid deceleration instead of hard stop
        if subrange == "guided_flight" && self.ring_dive.orbit_suppress_timer > 0.0 {
            self.blend.orbit_impulse.set_target(0.0);
            self.blend.orbit_speed.set_target(0.005);  // Very slow drift, not dead stop
        }

        // === LOOK TARGET (smoothed separately) ===
        // Guided: FORCE look INWARD (toward geometry center, not tangent)
        // This prevents the "slide along surface" feeling
        let look_target = match subrange {
            "lock_on" => [0.0, 0.0, -0.5],     // Look at portal
            "warp_out" => [0.0, -0.1, -1.0],   // Look forward into warp
            "guided_flight" => {
                // Look INWARD toward center, not sideways
                // Slight downward tilt to see interior structure
                [0.0, -0.15, -0.3]
            },
            "warp_back" => [0.0, 0.0, 0.0],    // Return to center (ring position)
            _ => [0.0, 0.0, 0.0],
        };

        // Guided uses strong look-target to force inward view
        // WarpBack uses stronger look-target smoothing (faster snap to center)
        let tau = match subrange {
            "guided_flight" => 0.2,  // Strong pull inward
            "warp_back" => 0.15,     // Fast snap to center
            _ => 0.3,
        };
        for i in 0..3 {
            let diff = look_target[i] - self.ring_dive.look_target[i];
            self.ring_dive.look_target_vel[i] += diff * dt / tau;
            self.ring_dive.look_target_vel[i] *= (-dt / tau).exp();
            self.ring_dive.look_target[i] += self.ring_dive.look_target_vel[i] * dt;
        }

        // FOV modulation during RingDive
        // Guided: WIDE FOV to see the room
        // WarpBack: return to normal FOV so rings look proper
        let target_fov = match subrange {
            "lock_on" => lerp(45.0, 50.0, sub_t),
            "warp_out" => lerp(50.0, 90.0, sub_t),    // Wide during warp
            "guided_flight" => lerp(90.0, 75.0, sub_t), // Stay wide to see room
            "warp_back" => lerp(75.0, 45.0, sub_t),   // Return to normal
            _ => 45.0,
        };
        let fov_lerp_strength = if subrange == "warp_back" { 0.15 } else { 0.1 };
        self.fov = lerp(self.fov, target_fov, rail_blend * fov_lerp_strength);
    }

    /// Update RingDive quality tier based on frame time
    /// Call this from renderer with actual frame time in milliseconds
    pub fn update_ring_dive_quality(&mut self, frame_time_ms: f32, dt: f32) {
        // Only scale quality when RingDive is active (effects are running)
        if !self.ring_dive.active {
            // Reset timers when inactive
            self.ring_dive.quality_downgrade_time = 0.0;
            self.ring_dive.quality_upgrade_time = 0.0;
            return;
        }

        // Downgrade if > 20ms for 1.5s
        if frame_time_ms > 20.0 {
            self.ring_dive.quality_downgrade_time += dt;
            self.ring_dive.quality_upgrade_time = 0.0;

            if self.ring_dive.quality_downgrade_time > 1.5 {
                let old_quality = self.ring_dive.quality;
                self.ring_dive.quality = match self.ring_dive.quality {
                    RingDiveQuality::High => RingDiveQuality::Med,
                    RingDiveQuality::Med => RingDiveQuality::Low,
                    RingDiveQuality::Low => RingDiveQuality::Low,
                };
                if self.ring_dive.quality != old_quality {
                    log::trace!("[RingDive] Quality downgrade: {:?} → {:?} (frame_time={:.1}ms)",
                        old_quality, self.ring_dive.quality, frame_time_ms);
                }
                self.ring_dive.quality_downgrade_time = 0.0;
            }
        }
        // Upgrade if < 14ms for 3s
        else if frame_time_ms < 14.0 {
            self.ring_dive.quality_upgrade_time += dt;
            self.ring_dive.quality_downgrade_time = 0.0;

            if self.ring_dive.quality_upgrade_time > 3.0 {
                let old_quality = self.ring_dive.quality;
                self.ring_dive.quality = match self.ring_dive.quality {
                    RingDiveQuality::Low => RingDiveQuality::Med,
                    RingDiveQuality::Med => RingDiveQuality::High,
                    RingDiveQuality::High => RingDiveQuality::High,
                };
                if self.ring_dive.quality != old_quality {
                    log::trace!("[RingDive] Quality upgrade: {:?} → {:?} (frame_time={:.1}ms)",
                        old_quality, self.ring_dive.quality, frame_time_ms);
                }
                self.ring_dive.quality_upgrade_time = 0.0;
            }
        } else {
            // Frame time is acceptable, slowly decay timers
            self.ring_dive.quality_downgrade_time = (self.ring_dive.quality_downgrade_time - dt * 0.5).max(0.0);
            self.ring_dive.quality_upgrade_time = (self.ring_dive.quality_upgrade_time - dt * 0.5).max(0.0);
        }
    }

    /// Get debug string for RingDive overlay
    pub fn ring_dive_debug_string(&self) -> String {
        self.ring_dive.debug_string(self.section_energy, self.tension_slow)
    }

    /// Get debug string for interior state + fold24
    pub fn interior_debug_string(&self) -> String {
        let forced_marker = if self.interior_forced_by_ring_dive { " (FORCED)" } else { "" };
        format!(
            "Interior: {}{} | {}",
            self.interior_state.name(),
            forced_marker,
            self.fold24.debug_string()
        )
    }

    /// Check if we should transition to a new phase (4-ACT HERO PRESET)
    fn check_phase_transition(&mut self, kick: f32) -> HeroAct {
        match self.phase {
            // ACT 1: BIRTH → LOCK
            // Field is forming, waiting for coherence
            HeroAct::Birth => {
                if self.phase_time < Self::BIRTH_MIN_TIME {
                    return self.phase;
                }
                // → Lock: audio present with any energy
                if self.section_energy > Self::BIRTH_TO_LOCK_ENERGY {
                    log::trace!("[Phase] BIRTH → LOCK! energy={:.2} > {:.2}", self.section_energy, Self::BIRTH_TO_LOCK_ENERGY);
                    return HeroAct::Lock;
                }
                // → Lock: max time reached (forced progression)
                if self.phase_time > Self::BIRTH_MAX_TIME {
                    return HeroAct::Lock;
                }
            }

            // ACT 2: LOCK → DIVE or → BIRTH
            // Standing wave formed, appreciating the geometry
            // NOTE: NO KICK HERE - kick causes flapping. Use section_energy + stability + dominance.
            HeroAct::Lock => {
                // DEBUG: Log Lock conditions
                static mut LOCK_DEBUG_TIMER: f32 = 0.0;
                unsafe {
                    LOCK_DEBUG_TIMER += 0.016;
                    if LOCK_DEBUG_TIMER > 1.0 {
                        log::trace!("[Phase LOCK] time={:.1}s, energy={:.2} (>{:.2}? {}), stability={:.2} (>{:.2}? {}), dominance={:.2} (>{:.2}? {})",
                            self.phase_time,
                            self.section_energy, Self::LOCK_TO_DIVE_ENERGY_MIN, self.section_energy > Self::LOCK_TO_DIVE_ENERGY_MIN,
                            self.stability_smooth, Self::LOCK_TO_DIVE_STABILITY, self.stability_smooth > Self::LOCK_TO_DIVE_STABILITY,
                            self.dominance_smooth, Self::LOCK_TO_DIVE_DOMINANCE, self.dominance_smooth > Self::LOCK_TO_DIVE_DOMINANCE);
                        LOCK_DEBUG_TIMER = 0.0;
                    }
                }

                if self.phase_time < Self::LOCK_MIN_TIME {
                    return self.phase;
                }
                // → Dive: energy in sweet spot + good stability + clear dominance
                // This creates a smooth, earned transition - not a kick-triggered jump
                if self.section_energy > Self::LOCK_TO_DIVE_ENERGY_MIN
                    && self.section_energy < Self::LOCK_TO_DIVE_ENERGY_MAX
                    && self.stability_smooth > Self::LOCK_TO_DIVE_STABILITY
                    && self.dominance_smooth > Self::LOCK_TO_DIVE_DOMINANCE
                {
                    log::trace!("[Phase] LOCK → DIVE! All conditions met.");
                    return HeroAct::Dive;
                }
                // → Dive: max time reached (forced progression to keep journey moving)
                if self.phase_time > Self::LOCK_MAX_TIME {
                    return HeroAct::Dive;
                }
                // → Birth: if it gets too quiet, reset
                if self.section_energy < Self::AFTERGLOW_TO_BIRTH_ENERGY {
                    return HeroAct::Birth;
                }
            }

            // ACT 3: DIVE → AFTERGLOW
            // Inside the geometry, peak experience
            //
            // HERO'S JOURNEY: The Ordeal happens HERE. PEAK is the climax.
            // Low stability + high energy = INTENSITY, not exit signal.
            // The 24-fold (treasure) reveals during peak tension.
            // Only exit when energy TRULY drops (the wind-down).
            //
            HeroAct::Dive => {
                // Log entry once (check if phase_time is near 0)
                if self.phase_time < 0.05 {
                    log::trace!("[Phase] *** ENTERED DIVE *** energy={:.3}, tension={:.2}, interior={:?}",
                        self.section_energy, self.tension_slow, self.interior_state);
                    self.dive_low_energy_time = 0.0; // Reset on entry
                }

                // DEBUG: Log Dive state every second
                let log_interval = (self.phase_time * 1.0) as i32;
                let prev_interval = ((self.phase_time - 0.016) * 1.0) as i32;
                if log_interval != prev_interval {
                    log::trace!("[Phase DIVE] time={:.1}s, energy={:.3}, tension_slow={:.2}, fold24_t={:.2}, interior={:?}",
                        self.phase_time,
                        self.section_energy,
                        self.tension_slow,
                        self.fold24.t,
                        self.interior_state);
                }

                // === STRICT GUARDS: Stay in Dive until conditions are right ===

                // Guard 1: MUST wait for interior to reach Inside
                if self.interior_state == InteriorState::Entering {
                    return self.phase;
                }

                // Guard 2: MUST be inside for the full experience
                if self.interior_state != InteriorState::Inside {
                    // If we're not Inside and not Entering, something's wrong - stay anyway
                    if self.phase_time < Self::DIVE_MIN_TIME {
                        return self.phase;
                    }
                }

                // Guard 3: Minimum time in Dive (10 seconds)
                if self.phase_time < Self::DIVE_MIN_TIME {
                    return self.phase;
                }

                // === EXIT CONDITIONS (only checked after all guards pass) ===

                // Exit: Energy has been low for 3+ seconds (extended hysteresis)
                if self.section_energy < Self::DIVE_TO_AFTERGLOW_ENERGY {
                    self.dive_low_energy_time += 0.016;
                    if self.dive_low_energy_time > 3.0 {  // 3 seconds of low energy required
                        log::trace!("[Phase] *** EXITING DIVE *** (low energy for 3s) - fold24_t was {:.2}",
                            self.fold24.t);
                        self.dive_low_energy_time = 0.0;
                        return HeroAct::Afterglow;
                    }
                } else {
                    self.dive_low_energy_time = 0.0; // Reset when energy is healthy
                }

                // Exit: Max time reached (30 seconds)
                if self.phase_time > Self::DIVE_MAX_TIME {
                    log::trace!("[Phase] *** EXITING DIVE *** (max time {}) - fold24_t was {:.2}",
                        Self::DIVE_MAX_TIME, self.fold24.t);
                    return HeroAct::Afterglow;
                }
            }

            // ACT 4: AFTERGLOW → BIRTH or → LOCK
            // Pulling back, residual coherence
            HeroAct::Afterglow => {
                if self.phase_time < Self::AFTERGLOW_MIN_TIME {
                    return self.phase;
                }
                // → Birth: very quiet, full reset
                if self.section_energy < Self::AFTERGLOW_TO_BIRTH_ENERGY {
                    return HeroAct::Birth;
                }
                // → Lock: energy stable, ready for another dive
                if self.section_energy > Self::LOCK_TO_DIVE_ENERGY_MIN
                    && self.stability_smooth > Self::LOCK_TO_DIVE_STABILITY
                {
                    return HeroAct::Lock;
                }
                // → Lock: max time reached (keeps journey cycling)
                if self.phase_time > Self::AFTERGLOW_MAX_TIME {
                    return HeroAct::Lock;
                }
            }
        }

        self.phase
    }

    // ========================================================================
    // INTERIOR LATCH THRESHOLDS
    // ========================================================================

    /// Entry conditions - must ALL be true to enter
    const INTERIOR_ENTER_STABILITY: f32 = 0.40;   // Harmonic stability required
    const INTERIOR_ENTER_DOMINANCE: f32 = 0.20;   // Clear dominant pitch
    const INTERIOR_ENTER_DISTANCE: f32 = 2.0;     // Camera must be close
    const INTERIOR_ENTER_ENERGY: f32 = 0.25;      // Minimum energy

    /// Time thresholds
    const INTERIOR_ENTER_DURATION: f32 = 0.5;     // Time in Entering before Inside
    const INTERIOR_EXIT_DURATION: f32 = 0.8;      // Time in Exiting before Outside
    const INTERIOR_MIN_TIME: f32 = 3.0;           // Minimum time inside before can exit

    /// Exit conditions - ANY of these triggers exit
    const INTERIOR_EXIT_STABILITY: f32 = 0.15;    // Stability below this → exit
    const INTERIOR_EXIT_ENERGY: f32 = 0.12;       // Energy below this → exit
    const INTERIOR_MAX_TIME: f32 = 15.0;          // Max time inside

    /// Check interior latch transitions
    /// PRIORITY: RingDive overrides HeroAct::Dive
    ///
    /// When RingDive.active and t >= 0.18 (past lock_on), we are INSIDE.
    /// This ensures the HUD shows "Inside" during RingDive and fold24
    /// can properly trigger from the RingDive timeline.
    fn check_interior_transition(&self, _kick: f32) -> InteriorState {
        // === TORUS KNOT FORCES INSIDE ===
        // When torus knot is active or transitioning, we MUST stay inside
        if self.torus_knot.active || self.torus_knot.t > 0.01 {
            return InteriorState::Inside;
        }

        // === RINGDIVE TAKES PRIORITY ===
        // RingDive is the authoritative "inside" signal for:
        // - lock_on (0.00-0.18): still outside, preparing
        // - warp_out (0.18-0.35): entering
        // - guided_flight (0.35-0.80): inside
        // - warp_back (0.80-1.00): exiting but still "inside" for visuals

        let ring_dive_inside = self.ring_dive.active && self.ring_dive.t >= 0.18;
        let ring_dive_entering = self.ring_dive.active && self.ring_dive.t >= 0.15 && self.ring_dive.t < 0.35;
        let ring_dive_exiting = self.ring_dive.active && self.ring_dive.t >= 0.80;

        // Fallback: HeroAct::Dive if no RingDive active
        let in_dive = self.phase == HeroAct::Dive;

        // Combined "should be inside" signal
        let should_be_inside = ring_dive_inside || in_dive;

        match self.interior_state {
            InteriorState::Outside => {
                // Enter when RingDive hits warp_out OR when HeroAct is Dive
                if ring_dive_entering || (in_dive && !self.ring_dive.active) {
                    return InteriorState::Entering;
                }
                // Fast-forward to Inside if already deep in RingDive
                if ring_dive_inside && self.ring_dive.t >= 0.35 {
                    return InteriorState::Inside;
                }
            }

            InteriorState::Entering => {
                // Quick transition to Inside
                if self.interior_state_time > 0.2 || (ring_dive_inside && self.ring_dive.t >= 0.35) {
                    return InteriorState::Inside;
                }
                // Abort if we left both RingDive and Dive phase
                if !should_be_inside {
                    return InteriorState::Exiting;
                }
            }

            InteriorState::Inside => {
                // Start exiting during warp_back
                if ring_dive_exiting {
                    return InteriorState::Exiting;
                }
                // Also exit if no longer in any inside-triggering state
                if !should_be_inside {
                    return InteriorState::Exiting;
                }
            }

            InteriorState::Exiting => {
                // Quick transition back to Outside (but not during active RingDive)
                if self.interior_state_time > 0.4 && !self.ring_dive.active {
                    return InteriorState::Outside;
                }
                // Complete RingDive before going fully outside
                if self.ring_dive.active && self.ring_dive.t >= 1.0 {
                    return InteriorState::Outside;
                }
                // Re-enter if we're back in a dive state
                if should_be_inside && !ring_dive_exiting {
                    return InteriorState::Entering;
                }
            }
        }

        self.interior_state
    }

    // ========================================================================
    // RENDER BLEND WEIGHT COMPUTATION (Deterministic, derived per-frame)
    // ========================================================================
    //
    // These are NOT drifting states - they're pure functions of (state, state_time)
    // with smoothstep easing. No accumulation, no dt-based drift.
    // ========================================================================

    /// Compute inside_mix: how "inside" we are visually [0..1]
    ///
    /// When RingDive is active: derive directly from ring_dive.t (the continuous cinematic timeline)
    ///   - Enter blend window: t ∈ [0.18 .. 0.35]
    ///   - Exit blend window: t ∈ [0.80 .. 0.92]
    ///   - interior_mix = smoothstep(0.18, 0.35, t) * (1 - smoothstep(0.80, 0.92, t))
    ///
    /// When RingDive is NOT active: use interior_state_time with longer premium ramps
    fn compute_inside_mix(&self) -> f32 {
        if self.ring_dive.active {
            // RingDive is the continuous timeline - derive interior_mix directly from it
            let t = self.ring_dive.t;
            let enter = smoothstep(0.18, 0.35, t);
            let exit = 1.0 - smoothstep(0.80, 0.92, t);
            enter * exit
        } else {
            // Outside RingDive: use interior_state_time with longer ramps
            const ENTER_FADE: f32 = 0.50;  // 500ms enter ramp
            const EXIT_FADE: f32 = 0.80;   // 800ms exit ramp

            match self.interior_state {
                InteriorState::Outside => 0.0,
                InteriorState::Entering => {
                    let t = (self.interior_state_time / ENTER_FADE).min(1.0);
                    t * t * (3.0 - 2.0 * t)  // smoothstep
                }
                InteriorState::Inside => 1.0,
                InteriorState::Exiting => {
                    let t = (self.interior_state_time / EXIT_FADE).min(1.0);
                    1.0 - t * t * (3.0 - 2.0 * t)  // 1 - smoothstep
                }
            }
        }
    }

    /// Compute fold24_mix: how "24-fold" we are visually [0..1]
    /// Deterministic function of (ring_dive.t, latched_fold_mode, fold_mode_change_time)
    fn compute_fold24_mix(&self) -> f32 {
        const FOLD_FADE: f32 = 0.30;

        if self.ring_dive.active {
            // Timeline-driven: overlapped ramps within RingDive
            let t = self.ring_dive.t;
            let up_ramp = smoothstep(0.56, 0.64, t);        // 12→24 ramp
            let down_ramp = 1.0 - smoothstep(0.80, 0.88, t); // 24→12 ramp
            up_ramp * down_ramp
        } else {
            // Outside RingDive: time-based crossfade to latched mode
            let t = (self.fold_mode_change_time / FOLD_FADE).min(1.0);
            let eased = t * t * (3.0 - 2.0 * t);  // smoothstep

            if self.latched_fold_mode == 24 {
                eased
            } else {
                1.0 - eased
            }
        }
    }

    /// Get effective orbit speed (base + impulse)
    pub fn effective_orbit_speed(&self) -> f32 {
        self.orbit_speed + self.orbit_impulse
    }

    /// Legacy compatibility methods
    pub fn compute_targets(&self, _energy: f32, _stability: f32, _kick: f32) -> DynamicCameraState {
        self.clone()
    }

    pub fn smooth_toward(&mut self, _target: &DynamicCameraState, _dt: f32) {
        // No-op, update() handles everything now
    }

    /// Toggle HUD visibility
    pub fn toggle_hud(&mut self) {
        self.hud_visible = !self.hud_visible;
    }

    /// Toggle QS Wheel visibility
    pub fn toggle_qs_wheel(&mut self) {
        self.qs_wheel_visible = !self.qs_wheel_visible;
    }

    /// Set QS Wheel visibility
    pub fn set_qs_wheel_visible(&mut self, visible: bool) {
        self.qs_wheel_visible = visible;
    }

    /// Set whether to hide QS Wheel numbers
    pub fn set_hide_qs_numbers(&mut self, hide: bool) {
        self.hide_qs_numbers = hide;
    }

    /// Get QS Wheel visibility
    pub fn is_qs_wheel_visible(&self) -> bool {
        self.qs_wheel_visible
    }

    /// Set QS Wheel dominant-only mode
    pub fn set_qs_dominant_only(&mut self, dominant_only: bool) {
        self.qs_dominant_only = dominant_only;
    }

    /// Get QS Wheel dominant-only mode
    pub fn is_qs_dominant_only(&self) -> bool {
        self.qs_dominant_only
    }

    /// Get debug HUD strings for REG camera/QC/portal/FPS state
    pub fn get_debug_hud_strings(&self) -> Vec<String> {
        let mut lines = Vec::with_capacity(24);

        // === AUDIO & TIMING ===
        let sym_mode = if self.fold24.active { 24 } else { 12 };
        lines.push(format!("=== AUDIO & STATE ==="));
        lines.push(format!("sym:{} fold24:{} harm_lp:{:.0}% gate_lp:{:.0}%",
            sym_mode,
            if self.fold24.active { "ON" } else { "off" },
            self.camera_reg.portal_ready_lp * 100.0,
            self.camera_reg.gate_stability_lp * 100.0
        ));
        lines.push(format!("gate_stab:{:.0}% energy:{:.2} tension:{:.2}",
            self.stability_smooth * 100.0,
            self.section_energy,
            self.tension_fast
        ));

        // === REG CAMERA ===
        lines.push(format!("=== REG CAMERA ==="));
        if cfg!(debug_assertions) {
            lines.push("AZ SOURCE: anchor_k only".to_string());
        }
        lines.push(format!("{}", self.camera_reg.debug_string(sym_mode)));
        lines.push(format!("elev:{:+} r_step:{} fov_step:{}",
            self.camera_reg.elev_level,
            self.camera_reg.radius_step,
            self.camera_reg.fov_step
        ));

        // === QC PORT ===
        lines.push(format!("=== QC PORT ==="));
        lines.push(format!("{}", self.qc_port.debug_string()));
        lines.push(format!("lattice_ok:{} closeness:[{:.0},{:.0},{:.0},{:.0}]",
            if self.qc.invariants.lattice_ok_24 { "Y" } else { "N" },
            self.qc_port.closeness[0] * 100.0,
            self.qc_port.closeness[1] * 100.0,
            self.qc_port.closeness[2] * 100.0,
            self.qc_port.closeness[3] * 100.0
        ));

        // === PORTAL ===
        lines.push(format!("=== PORTAL ==="));
        let in_portal = self.camera_reg.rig == CameraRig::PortalDive;
        if in_portal {
            lines.push(format!("IN_PORTAL: anchor_k={} phase={:.0}%",
                self.camera_reg.portal_anchor_k_locked,
                self.camera_reg.portal_phase * 100.0
            ));
        } else {
            lines.push(format!("ready:{:.0}% armed:{} block:{:.1}s",
                self.camera_reg.portal_ready_lp * 100.0,
                if self.camera_reg.portal_enter_armed { "YES" } else { "no" },
                self.camera_reg.portal_block_timer
            ));
        }

        // === PORTAL FPS ===
        lines.push(format!("=== PORTAL FPS ==="));
        lines.push(format!("{}", self.portal_fps.debug_string()));
        if self.portal_fps.active {
            lines.push(format!("fwd:{:.0}% lane_t:{:.0}% scroll:{:.0}%",
                self.portal_fps.forward_phase * 100.0,
                self.portal_fps.lane_transition_t * 100.0,
                self.portal_fps.line_scroll_phase * 100.0
            ));
            lines.push(format!("vista:{:.0}% hit:{:.0}% scaffold:{:.0}%",
                self.portal_fps.vista_pulse * 100.0,
                self.portal_fps.hit_emphasis * 100.0,
                self.portal_fps.scaffold_emphasis * 100.0
            ));
            // InsideView camera info
            let fps = &self.portal_fps;
            let corridor_r = match fps.corridor_tier {
                0 => 0.33, 1 => 0.42, _ => 0.55
            };
            lines.push(format!("INSIDE: z={:.2} corr_r={:.2} aper={:.0}%",
                fps.z_offset,
                corridor_r,
                fps.entry_aperture * 100.0
            ));
            lines.push(format!("intent:{:+} frame:({:.2},{:.2})",
                fps.lane_intent,
                fps.frame_forward_x,
                fps.frame_forward_z
            ));
        }

        // === VALIDATION ===
        lines.push(format!("=== VALIDATION ==="));
        let v = &self.reg_validation;
        lines.push(format!("cam:{} qc:{} portal:{}/{} freeze:{}/{}",
            v.cam_commits, v.qc_commits,
            v.portal_enters, v.portal_exits,
            v.freeze_enters, v.freeze_exits
        ));
        lines.push(format!("fps: lane:{} seg:{} nm:{}",
            v.fps_lane_switches, v.fps_segment_advances, v.fps_near_miss_spawns
        ));
        let violations = v.total_violations();
        if violations > 0 {
            lines.push(format!("VIOLATIONS: {} (1:{} 2:{} 3:{} 4:{} 5:{} 6:{})",
                violations,
                v.inv1_discrete_range, v.inv2_step_limit, v.inv3_beat_gating,
                v.inv4_portal_lock, v.inv5_freeze_rule, v.inv6_unit_contract
            ));
        } else {
            lines.push(format!("INVARIANTS: OK (no violations)"));
        }

        // === RECENT EVENTS ===
        lines.push(format!("=== RECENT EVENTS ==="));
        for i in 0..4 {
            let idx = (v.recent_event_idx + v.recent_events.len() - 1 - i) % v.recent_events.len();
            let evt = &v.recent_events[idx];
            if evt.event_type != RegEventType::None {
                lines.push(format!("{:.1}s {:?} ({})",
                    evt.timestamp_s, evt.event_type, evt.detail
                ));
            }
        }

        lines
    }
}

/// Phase-specific target parameters
pub(crate) struct PhaseTargets {
    distance: f32,
    fov: f32,
    orbit_speed: f32,
    tilt: f32,
    noise_rot: f32,
    noise_pos: f32,
    inside_dip: f32,
    aim_radius: f32,
    field_density: f32,
    surface_opacity: f32,
    interior_enable: f32,
    skin_intensity: f32,
    video_visibility: f32,
    // Compel narrative state
    compel_exterior_bind: f32,
    compel_interior_reveal: f32,
    compel_emissive_persist: f32,
    compel_flicker_gate: f32,
}

/// Internal: per-mode camera parameters
struct CameraModeParams {
    base_dist: f32,
    fov: f32,
    orbit_speed: f32,
    tilt_angle: f32,
    noise_rot: f32,
    noise_pos: f32,
    inside_dip_max: f32,
    aim_radius: f32,  // how far aim offset can move from center
}

/// Smoothstep function
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear interpolation
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ============================================================================
// JOURNEY BLEND - Smooth Parameter System (Critically Damped Springs)
// ============================================================================
//
// Every visual parameter transitions smoothly using critically damped springs.
// This eliminates visual pops when state changes occur.
//
// Critically damped spring (no overshoot, fastest settling):
//   x'' + 2*w*x' + w^2*(x - target) = 0
//
// Discrete update per frame:
//   vel += (target - x) * (w*w) * dt
//   vel *= exp(-2*w*dt)
//   x += vel * dt
//
// Different omega (w) values for fast vs slow parameters:
//   - Camera distance/dip: w = 2.0 (~1.5s settle)
//   - Most visual params: w = 4.0 (~0.5s settle)
//   - Fast response: w = 8.0 (~0.25s settle)
// ============================================================================

/// A single parameter with critically damped spring dynamics
#[derive(Clone, Debug)]
pub struct SmoothParam {
    /// Current value
    pub value: f32,
    /// Target value
    pub target: f32,
    /// Current velocity
    velocity: f32,
    /// Spring frequency (omega) - higher = faster response
    omega: f32,
}

impl SmoothParam {
    /// Create a new smooth param with initial value and spring frequency
    pub fn new(initial: f32, omega: f32) -> Self {
        Self {
            value: initial,
            target: initial,
            velocity: 0.0,
            omega,
        }
    }

    /// Update the spring toward target (critically damped)
    pub fn update(&mut self, dt: f32) {
        let w = self.omega;
        let w2 = w * w;

        // Critically damped spring update
        let diff = self.target - self.value;
        self.velocity += diff * w2 * dt;
        self.velocity *= (-2.0 * w * dt).exp();
        self.value += self.velocity * dt;

        // Snap to target if very close (prevent endless tiny oscillations)
        if diff.abs() < 0.0001 && self.velocity.abs() < 0.001 {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    /// Set target value
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Immediately snap to value (no animation)
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// Check if settled (at target with no velocity)
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.001 && self.velocity.abs() < 0.01
    }
}

impl Default for SmoothParam {
    fn default() -> Self {
        Self::new(0.0, 4.0)
    }
}

// ============================================================================
// INERTIAL PARAM - True mass-based physics for heavy, fluid motion
// ============================================================================
//
// Unlike critically-damped springs which converge directly to target,
// InertialParam simulates a heavy mass floating in fluid:
//
//   force = (target - value) * stiffness
//   acceleration = force / mass
//   velocity += acceleration * dt
//   velocity *= exp(-drag * dt)   // Fluid drag
//   value += velocity * dt
//
// This gives TRUE MOMENTUM - the object continues moving after forces stop,
// and heavy objects take time to accelerate/decelerate.
//
// Presets:
//   - HEAVY: mass=30, drag=1.5, stiffness=2.0 (~3-4s to settle, very sluggish)
//   - MEDIUM: mass=15, drag=2.0, stiffness=3.0 (~2s to settle)
//   - LIGHT: mass=5, drag=3.0, stiffness=5.0 (~1s to settle)
// ============================================================================

/// Preset values for InertialParam (defined before module so it can be used there)
#[derive(Clone, Copy, Debug)]
pub struct InertiaPreset {
    pub mass: f32,
    pub drag: f32,
    pub stiffness: f32,
    pub target_smooth: f32,  // Rate at which smooth_target approaches target (0.5-3.0)
    pub max_velocity: f32,   // Max speed to prevent sudden movements
}

/// Inertial parameter presets
pub mod inertia {
    use super::InertiaPreset;
    /// Heavy mass, slow response (~3-4s settle) - main camera motion
    /// Very smooth target tracking, strict velocity limit
    pub const HEAVY: InertiaPreset = InertiaPreset {
        mass: 30.0, drag: 1.5, stiffness: 2.0,
        target_smooth: 0.8,   // Slow target easing
        max_velocity: 0.3,    // Strict speed limit
    };
    /// Medium mass (~2s settle) - secondary camera params
    pub const MEDIUM: InertiaPreset = InertiaPreset {
        mass: 15.0, drag: 2.0, stiffness: 3.0,
        target_smooth: 1.2,
        max_velocity: 0.5,
    };
    /// Light mass (~1s settle) - responsive params
    pub const LIGHT: InertiaPreset = InertiaPreset {
        mass: 5.0, drag: 3.0, stiffness: 5.0,
        target_smooth: 2.0,
        max_velocity: 1.0,
    };
    /// Very heavy, glacial motion (~5-6s settle) - aim drift
    /// Extremely smooth, barely perceptible motion
    pub const GLACIAL: InertiaPreset = InertiaPreset {
        mass: 50.0, drag: 1.2, stiffness: 1.5,
        target_smooth: 0.5,   // Very slow target easing
        max_velocity: 0.15,   // Very strict speed limit
    };
}

/// A parameter with true mass-based physics (momentum, inertia)
///
/// Enhanced with:
/// - Target smoothing: The target itself eases, preventing jerky force changes
/// - Velocity clamping: Max speed limit prevents sudden movements
#[derive(Clone, Debug)]
pub struct InertialParam {
    /// Current value
    pub value: f32,
    /// Requested target (what the caller wants)
    pub target: f32,
    /// Smoothed target (what physics actually pulls toward - eases toward target)
    smooth_target: f32,
    /// Current velocity (persists - this is the key difference from springs!)
    velocity: f32,
    /// Mass - higher = more sluggish, harder to accelerate/decelerate
    mass: f32,
    /// Drag coefficient - higher = more friction, faster to slow down
    drag: f32,
    /// Stiffness - higher = stronger pull toward target
    stiffness: f32,
    /// Target smoothing rate (higher = faster target tracking, 0.5-3.0 typical)
    target_smooth: f32,
    /// Maximum velocity (prevents sudden movements)
    max_velocity: f32,
}

impl InertialParam {
    /// Create with preset
    pub fn new(initial: f32, preset: InertiaPreset) -> Self {
        Self {
            value: initial,
            target: initial,
            smooth_target: initial,
            velocity: 0.0,
            mass: preset.mass,
            drag: preset.drag,
            stiffness: preset.stiffness,
            target_smooth: preset.target_smooth,
            max_velocity: preset.max_velocity,
        }
    }

    /// Create with custom physics parameters
    pub fn with_params(initial: f32, mass: f32, drag: f32, stiffness: f32) -> Self {
        Self {
            value: initial,
            target: initial,
            smooth_target: initial,
            velocity: 0.0,
            mass,
            drag,
            stiffness,
            target_smooth: 1.0,
            max_velocity: 0.5,
        }
    }

    /// Physics update - true mass-based motion with extra smoothing
    pub fn update(&mut self, dt: f32) {
        // === STAGE 1: Smooth the target itself ===
        // This prevents sudden target changes from creating jerky forces
        let target_diff = self.target - self.smooth_target;
        self.smooth_target += target_diff * self.target_smooth * dt;

        // === STAGE 2: Physics toward smooth_target ===
        let diff = self.smooth_target - self.value;
        let force = diff * self.stiffness;

        // F = ma, so a = F/m
        let acceleration = force / self.mass;

        // Update velocity (this persists - momentum!)
        self.velocity += acceleration * dt;

        // Apply drag (fluid resistance)
        self.velocity *= (-self.drag * dt).exp();

        // === STAGE 3: Clamp velocity ===
        // Prevents sudden movements regardless of forces
        self.velocity = self.velocity.clamp(-self.max_velocity, self.max_velocity);

        // Update position
        self.value += self.velocity * dt;

        // Snap if very close AND very slow (prevent endless micro-drift)
        if diff.abs() < 0.0005 && self.velocity.abs() < 0.0005 {
            self.value = self.smooth_target;
            self.velocity = 0.0;
        }
    }

    /// Set target value
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Add impulse (direct velocity change - like being bumped)
    pub fn add_impulse(&mut self, impulse: f32) {
        self.velocity += impulse / self.mass;
    }

    /// Immediately snap to value (no animation)
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.smooth_target = value;
        self.velocity = 0.0;
    }

    /// Check if settled
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.001 && self.velocity.abs() < 0.001
    }

    /// Get current velocity (for debugging/display)
    pub fn velocity(&self) -> f32 {
        self.velocity
    }
}

impl Default for InertialParam {
    fn default() -> Self {
        Self::new(0.0, inertia::MEDIUM)
    }
}

/// Inertial angle parameter with wrap-around handling
/// Enhanced with target smoothing and velocity clamping
#[derive(Clone, Debug)]
pub struct InertialAngle {
    /// Current angle (radians)
    pub value: f32,
    /// Requested target angle (radians)
    pub target: f32,
    /// Smoothed target (what physics pulls toward)
    smooth_target: f32,
    /// Angular velocity
    velocity: f32,
    /// Mass
    mass: f32,
    /// Drag
    drag: f32,
    /// Stiffness
    stiffness: f32,
    /// Target smoothing rate
    target_smooth: f32,
    /// Maximum angular velocity
    max_velocity: f32,
}

impl InertialAngle {
    pub fn new(initial: f32, preset: InertiaPreset) -> Self {
        Self {
            value: Self::wrap(initial),
            target: Self::wrap(initial),
            smooth_target: Self::wrap(initial),
            velocity: 0.0,
            mass: preset.mass,
            drag: preset.drag,
            stiffness: preset.stiffness,
            target_smooth: preset.target_smooth,
            max_velocity: preset.max_velocity,
        }
    }

    /// Wrap angle to [-PI, PI]
    fn wrap(angle: f32) -> f32 {
        let pi = std::f32::consts::PI;
        let mut a = angle % (2.0 * pi);
        if a > pi {
            a -= 2.0 * pi;
        } else if a < -pi {
            a += 2.0 * pi;
        }
        a
    }

    /// Shortest angular difference between two angles
    fn angular_diff_between(from: f32, to: f32) -> f32 {
        Self::wrap(to - from)
    }

    /// Shortest angular difference from value to smooth_target
    fn angular_diff(&self) -> f32 {
        Self::wrap(self.smooth_target - self.value)
    }

    /// Physics update with wrap-aware motion and extra smoothing
    pub fn update(&mut self, dt: f32) {
        // === STAGE 1: Smooth the target itself (wrap-aware) ===
        let target_diff = Self::angular_diff_between(self.smooth_target, self.target);
        self.smooth_target += target_diff * self.target_smooth * dt;
        self.smooth_target = Self::wrap(self.smooth_target);

        // === STAGE 2: Physics toward smooth_target ===
        let diff = self.angular_diff();
        let force = diff * self.stiffness;
        let acceleration = force / self.mass;

        self.velocity += acceleration * dt;

        // Apply drag
        self.velocity *= (-self.drag * dt).exp();

        // === STAGE 3: Clamp velocity ===
        self.velocity = self.velocity.clamp(-self.max_velocity, self.max_velocity);

        // Update position
        self.value += self.velocity * dt;
        self.value = Self::wrap(self.value);

        // Snap if very close
        if diff.abs() < 0.0005 && self.velocity.abs() < 0.0005 {
            self.value = Self::wrap(self.smooth_target);
            self.velocity = 0.0;
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = Self::wrap(target);
    }

    pub fn add_impulse(&mut self, impulse: f32) {
        self.velocity += impulse / self.mass;
    }

    pub fn snap(&mut self, value: f32) {
        self.value = Self::wrap(value);
        self.target = self.value;
        self.smooth_target = self.value;
        self.velocity = 0.0;
    }

    /// Drift toward nearest n-fold anchor from current position
    pub fn drift_to_nearest_anchor(&mut self, n_fold: u32) {
        let pi = std::f32::consts::PI;
        let step = 2.0 * pi / n_fold as f32;
        let anchor = (self.value / step).round() * step;
        self.target = Self::wrap(anchor);
    }
}

impl Default for InertialAngle {
    fn default() -> Self {
        Self::new(0.0, inertia::MEDIUM)
    }
}

/// Smooth angle parameter with wrap-around handling
#[derive(Clone, Debug)]
pub struct SmoothAngle {
    /// Current angle (radians)
    pub value: f32,
    /// Target angle (radians)
    pub target: f32,
    /// Current angular velocity
    velocity: f32,
    /// Spring frequency
    omega: f32,
}

impl SmoothAngle {
    pub fn new(initial: f32, omega: f32) -> Self {
        Self {
            value: initial,
            target: initial,
            velocity: 0.0,
            omega,
        }
    }

    /// Wrap angle to [-PI, PI]
    fn wrap(angle: f32) -> f32 {
        let pi = std::f32::consts::PI;
        let mut a = angle % (2.0 * pi);
        if a > pi {
            a -= 2.0 * pi;
        } else if a < -pi {
            a += 2.0 * pi;
        }
        a
    }

    /// Shortest angular difference from current to target
    fn angular_diff(&self) -> f32 {
        Self::wrap(self.target - self.value)
    }

    /// Update with wrap-aware critically damped spring
    pub fn update(&mut self, dt: f32) {
        let w = self.omega;
        let w2 = w * w;

        // Use shortest path difference
        let diff = self.angular_diff();
        self.velocity += diff * w2 * dt;
        self.velocity *= (-2.0 * w * dt).exp();
        self.value += self.velocity * dt;
        self.value = Self::wrap(self.value);

        // Snap if very close
        if diff.abs() < 0.0001 && self.velocity.abs() < 0.001 {
            self.value = Self::wrap(self.target);
            self.velocity = 0.0;
        }
    }

    /// Set target, optionally snapping to nearest n-fold symmetry anchor
    pub fn set_target_with_symmetry(&mut self, target: f32, n_fold: u32) {
        let pi = std::f32::consts::PI;
        let step = 2.0 * pi / n_fold as f32;

        // Find nearest symmetry anchor to target
        let anchor = (target / step).round() * step;
        self.target = Self::wrap(anchor);
    }

    /// Set target angle directly (shortest path)
    pub fn set_target(&mut self, target: f32) {
        self.target = Self::wrap(target);
    }

    /// Drift toward nearest n-fold anchor from current position
    pub fn drift_to_nearest_anchor(&mut self, n_fold: u32) {
        let pi = std::f32::consts::PI;
        let step = 2.0 * pi / n_fold as f32;
        let anchor = (self.value / step).round() * step;
        self.target = Self::wrap(anchor);
    }

    pub fn snap(&mut self, value: f32) {
        self.value = Self::wrap(value);
        self.target = self.value;
        self.velocity = 0.0;
    }
}

impl Default for SmoothAngle {
    fn default() -> Self {
        Self::new(0.0, 3.0)
    }
}

// ============================================================================
// CYLINDER SNAP CONTROLLER
// Quantized, inertia-based motion with physical dance behavior
// ============================================================================
//
// VISUAL DANCE SEQUENCE:
// 1. Idle     - Frozen exactly on anchor, no motion
// 2. Acquire  - Locks target, brief anticipation (no visible motion yet)
// 3. Moving   - Min-jerk snap toward target with controlled overshoot
// 4. Settling - Damped oscillator releases energy, decays to rest
// 5. Holding  - Locked exactly on anchor, mandatory dwell before next move
//
// HARD CONSTRAINTS:
// - Quantized anchors ONLY (12-fold = 30°, 24-fold = 15°)
// - Max ±1 anchor step per move (no large jumps)
// - Target LOCKED during motion (no retargeting)
// - Exact snap to anchor at end (no floating point drift)
// - Hysteresis triggering with dwell times
// - 2-layer EMA filtering (slow for decisions, fast for intensity)
// ============================================================================

/// Motion phase for the cylinder dance
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapPhase {
    /// Frozen exactly on anchor - ZERO motion
    Idle,
    /// Target acquired, brief anticipation before movement
    Acquire,
    /// Min-jerk motion toward target with overshoot
    Moving,
    /// Damped oscillator settling around target
    Settling,
    /// Locked on anchor, mandatory dwell before allowing next move
    Holding,
}

impl Default for SnapPhase {
    fn default() -> Self {
        Self::Idle
    }
}

impl SnapPhase {
    /// Debug name for HUD display
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Acquire => "ACQUIRE",
            Self::Moving => "MOVING",
            Self::Settling => "SETTLE",
            Self::Holding => "HOLD",
        }
    }
}

/// Cylinder snap controller with physical inertia and symmetry constraints
#[derive(Clone, Debug)]
pub struct CylinderMotion {
    // === OUTPUT (THE ONLY THING THAT MATTERS) ===
    /// Current angle (radians) - drives all rendering
    pub angle: f32,

    // === SYMMETRY ===
    /// Fold count: 12 (outside) or 24 (inside)
    pub fold: i32,

    // === ANCHOR STATE ===
    /// Current anchor index (0..fold-1) - authoritative position
    pub cur_k: i32,
    /// Target anchor index (LOCKED during motion)
    pub target_k: i32,

    // === PHASE STATE ===
    /// Current motion phase
    pub phase: SnapPhase,
    /// Progress through current phase [0..1]
    pub phase_t: f32,

    // === TRANSITION GEOMETRY (locked at acquire) ===
    /// Starting angle for current transition
    start_angle: f32,
    /// Target angle for current transition
    locked_target_angle: f32,
    /// Direction of movement (-1 or +1)
    step_dir: i32,

    // === TIMING (milliseconds) ===
    /// Remaining hold time before allowing idle
    pub hold_ms: f32,
    /// Cooldown before next move allowed
    pub cooldown_ms: f32,
    /// Time above high threshold (for arming)
    pub armed_ms: f32,
    /// Time below low threshold (for reset)
    pub reset_ms: f32,
    /// Time pitch has been stable
    pub pitch_stable_ms: f32,

    // === HYSTERESIS STATE ===
    /// Armed to trigger (above high threshold long enough)
    pub armed: bool,
    /// Reset complete (below low threshold long enough)
    reset: bool,

    // === 2-LAYER EMA FILTERING ===
    /// Slow EMA for decision making (tau ~1.0s)
    pub decision_ema: f32,
    /// Fast EMA for intensity/overshoot only (tau ~0.18s)
    pub intensity_ema: f32,
    /// Pitch class EMA for stability detection
    pitch_ema: f32,
    /// Last stable pitch class
    last_stable_pc: i32,

    // === SETTLE OSCILLATOR STATE ===
    /// Settle amplitude (computed from intensity at transition start)
    settle_amp: f32,
    /// Settle phase offset
    settle_phase: f32,
    /// Damping coefficient (computed for ~400ms settle)
    settle_damping: f32,
    /// Oscillation frequency (rad/s)
    settle_omega: f32,

    // === MODE ===
    /// Inside mode (24-fold) vs Outside mode (12-fold)
    pub is_inside: bool,
}

// === TUNING CONSTANTS ===
impl CylinderMotion {
    // --- Thresholds ---
    const SILENCE_THRESHOLD: f32 = 0.05;   // Below this = no motion ever
    const TRIGGER_HIGH: f32 = 0.20;        // Must exceed to ARM
    const TRIGGER_LOW: f32 = 0.07;         // Must drop below to RESET

    // --- Dwell times (milliseconds) ---
    const ARM_DWELL_MS: f32 = 100.0;       // Time above high to arm
    const RESET_DWELL_MS: f32 = 70.0;      // Time below low to reset
    const PITCH_STABLE_MS: f32 = 120.0;    // Pitch must be stable this long
    const HOLD_DURATION_MS: f32 = 350.0;   // Mandatory freeze after settling
    const COOLDOWN_MS: f32 = 150.0;        // Min time between moves

    // --- Phase durations (seconds) ---
    const ACQUIRE_DURATION: f32 = 0.04;    // Brief anticipation
    const MOVE_DURATION: f32 = 0.22;       // Min-jerk snap
    const SETTLE_DURATION: f32 = 0.38;     // Damped oscillator

    // --- Motion parameters ---
    const OVERSHOOT_MIN: f32 = 0.02;       // Minimum overshoot (radians)
    const OVERSHOOT_MAX: f32 = 0.18;       // Maximum overshoot (radians)
    const SETTLE_ZETA: f32 = 0.35;         // Damping ratio (< 1 = underdamped)
    const SETTLE_FREQ_HZ: f32 = 4.5;       // Natural frequency

    // --- EMA time constants (seconds) ---
    const DECISION_TAU: f32 = 1.0;         // Slow EMA for decisions
    const INTENSITY_TAU: f32 = 0.18;       // Fast EMA for intensity
    const PITCH_TAU: f32 = 0.35;           // Medium EMA for pitch stability
}

impl Default for CylinderMotion {
    fn default() -> Self {
        Self {
            angle: 0.0,
            fold: 12,
            cur_k: 0,
            target_k: 0,
            phase: SnapPhase::Idle,
            phase_t: 0.0,
            start_angle: 0.0,
            locked_target_angle: 0.0,
            step_dir: 1,
            hold_ms: 0.0,
            cooldown_ms: 0.0,
            armed_ms: 0.0,
            reset_ms: Self::RESET_DWELL_MS, // Start reset
            pitch_stable_ms: 0.0,
            armed: false,
            reset: true,
            decision_ema: 0.0,
            intensity_ema: 0.0,
            pitch_ema: 0.0,
            last_stable_pc: 0,
            settle_amp: 0.0,
            settle_phase: 0.0,
            settle_damping: Self::SETTLE_ZETA * 2.0 * std::f32::consts::PI * Self::SETTLE_FREQ_HZ,
            settle_omega: 2.0 * std::f32::consts::PI * Self::SETTLE_FREQ_HZ,
            is_inside: false,
        }
    }
}

impl CylinderMotion {
    pub fn new() -> Self {
        Self::default()
    }

    // ========================================================================
    // GEOMETRY HELPERS
    // ========================================================================

    /// Anchor index → angle (radians)
    fn k_to_angle(&self, k: i32) -> f32 {
        let step = std::f32::consts::TAU / self.fold as f32;
        ((k % self.fold + self.fold) % self.fold) as f32 * step
    }

    /// Pitch class (0..11) → anchor index
    fn pc_to_k(&self, pc: usize) -> i32 {
        if self.fold == 12 {
            (pc % 12) as i32
        } else {
            // 24-fold: each pitch class maps to primary anchor
            ((pc % 12) * 2) as i32
        }
    }

    /// Shortest angular distance from a to b (signed, handles wrap)
    fn angle_diff(a: f32, b: f32) -> f32 {
        let mut d = b - a;
        while d > std::f32::consts::PI { d -= std::f32::consts::TAU; }
        while d < -std::f32::consts::PI { d += std::f32::consts::TAU; }
        d
    }

    /// Wrap angle to [0, TAU)
    fn wrap_angle(a: f32) -> f32 {
        let mut x = a % std::f32::consts::TAU;
        if x < 0.0 { x += std::f32::consts::TAU; }
        x
    }

    /// Step-limited target: max ±1 anchor from current
    fn step_limited_k(&self, desired_k: i32) -> i32 {
        if desired_k == self.cur_k {
            return self.cur_k;
        }

        // Shortest direction
        let fwd = (desired_k - self.cur_k + self.fold) % self.fold;
        let bwd = (self.cur_k - desired_k + self.fold) % self.fold;

        if fwd <= bwd {
            (self.cur_k + 1) % self.fold
        } else {
            (self.cur_k - 1 + self.fold) % self.fold
        }
    }

    // ========================================================================
    // MODE CONTROL
    // ========================================================================

    /// Set inside/outside mode (changes fold count)
    pub fn set_mode(&mut self, inside: bool) {
        if self.is_inside == inside {
            return;
        }

        // Don't change mode during motion
        if self.phase == SnapPhase::Moving || self.phase == SnapPhase::Settling {
            return;
        }

        self.is_inside = inside;
        self.fold = if inside { 24 } else { 12 };

        // Re-anchor to nearest valid position in new fold
        let step = std::f32::consts::TAU / self.fold as f32;
        let positive = Self::wrap_angle(self.angle);
        self.cur_k = ((positive / step).round() as i32) % self.fold;
        self.target_k = self.cur_k;
        self.angle = self.k_to_angle(self.cur_k);
        self.phase = SnapPhase::Idle;
    }

    // ========================================================================
    // MAIN UPDATE
    // ========================================================================

    /// Update the controller - call every frame
    pub fn update(
        &mut self,
        dt: f32,
        raw_energy: f32,
        raw_tension: f32,
        dominant_pc: usize,
    ) {
        let dt_ms = dt * 1000.0;

        // === UPDATE EMAs ===
        let alpha_d = 1.0 - (-dt / Self::DECISION_TAU).exp();
        let alpha_i = 1.0 - (-dt / Self::INTENSITY_TAU).exp();
        let alpha_p = 1.0 - (-dt / Self::PITCH_TAU).exp();

        self.decision_ema = lerp(self.decision_ema, raw_tension.max(raw_energy), alpha_d);
        self.intensity_ema = lerp(self.intensity_ema, raw_tension, alpha_i);
        self.pitch_ema = lerp(self.pitch_ema, dominant_pc as f32, alpha_p);

        // === UPDATE TIMERS ===
        self.cooldown_ms = (self.cooldown_ms - dt_ms).max(0.0);

        // === PITCH STABILITY ===
        let pc_i32 = dominant_pc as i32;
        if (self.pitch_ema - dominant_pc as f32).abs() < 0.6 && pc_i32 == self.last_stable_pc {
            self.pitch_stable_ms += dt_ms;
        } else {
            self.pitch_stable_ms = 0.0;
            self.last_stable_pc = pc_i32;
        }

        // === HYSTERESIS TRACKING ===
        if self.decision_ema > Self::TRIGGER_HIGH {
            self.armed_ms += dt_ms;
            self.reset_ms = 0.0;
        } else if self.decision_ema < Self::TRIGGER_LOW {
            self.reset_ms += dt_ms;
            self.armed_ms = 0.0;
        } else {
            self.armed_ms = (self.armed_ms - dt_ms * 0.3).max(0.0);
            self.reset_ms = (self.reset_ms - dt_ms * 0.3).max(0.0);
        }

        if self.armed_ms >= Self::ARM_DWELL_MS {
            self.armed = true;
        }
        if self.reset_ms >= Self::RESET_DWELL_MS {
            self.reset = true;
            self.armed = false;
            self.armed_ms = 0.0;
        }

        // === PHASE STATE MACHINE ===
        match self.phase {
            SnapPhase::Idle => {
                // FROZEN exactly on anchor
                self.angle = self.k_to_angle(self.cur_k);

                // Check trigger conditions
                if self.should_trigger(dominant_pc) {
                    self.begin_acquire(dominant_pc);
                }
            }

            SnapPhase::Acquire => {
                // Brief anticipation - no visible motion yet
                self.phase_t += dt / Self::ACQUIRE_DURATION;

                // Stay at current anchor during acquire
                self.angle = self.k_to_angle(self.cur_k);

                if self.phase_t >= 1.0 {
                    self.begin_moving();
                }
            }

            SnapPhase::Moving => {
                // Min-jerk snap with overshoot
                self.phase_t += dt / Self::MOVE_DURATION;
                let t = self.phase_t.min(1.0);

                // Smoothstep5 (min-jerk): t³(t(6t-15)+10)
                let u = t * t * t * (t * (6.0 * t - 15.0) + 10.0);

                // Base interpolation
                let travel = Self::angle_diff(self.start_angle, self.locked_target_angle);
                let base_angle = self.start_angle + travel * u;

                // Overshoot bump: peaks at t≈0.7, proportional to intensity
                let os_curve = (t * std::f32::consts::PI).sin();
                let os_envelope = (1.0 - (t - 0.7).abs() * 2.0).max(0.0);
                let overshoot_offset = self.settle_amp * os_curve * os_envelope * 0.4;

                self.angle = base_angle + overshoot_offset * self.step_dir as f32;

                if self.phase_t >= 1.0 {
                    self.begin_settling();
                }
            }

            SnapPhase::Settling => {
                // Damped oscillator around target
                self.phase_t += dt / Self::SETTLE_DURATION;
                let t = self.phase_t.min(1.0);
                let t_sec = t * Self::SETTLE_DURATION;

                // Underdamped oscillator: A * exp(-ζωt) * cos(ωd*t + φ)
                let decay = (-self.settle_damping * t_sec).exp();
                let omega_d = self.settle_omega * (1.0 - Self::SETTLE_ZETA * Self::SETTLE_ZETA).sqrt();
                let osc = (omega_d * t_sec + self.settle_phase).cos();

                let offset = self.settle_amp * decay * osc * self.step_dir as f32;
                self.angle = self.locked_target_angle + offset;

                if self.phase_t >= 1.0 {
                    self.begin_holding();
                }
            }

            SnapPhase::Holding => {
                // FROZEN exactly on anchor
                self.angle = self.k_to_angle(self.cur_k);

                self.hold_ms -= dt_ms;
                if self.hold_ms <= 0.0 {
                    self.phase = SnapPhase::Idle;
                    self.hold_ms = 0.0;
                }
            }
        }

        // Final wrap
        self.angle = Self::wrap_angle(self.angle);
    }

    // ========================================================================
    // TRIGGER LOGIC
    // ========================================================================

    fn should_trigger(&self, dominant_pc: usize) -> bool {
        // 1. Silence check
        if self.decision_ema < Self::SILENCE_THRESHOLD {
            return false;
        }

        // 2. Cooldown check
        if self.cooldown_ms > 0.0 {
            return false;
        }

        // 3. Hysteresis check
        if !self.armed || !self.reset {
            return false;
        }

        // 4. Pitch stability check
        if self.pitch_stable_ms < Self::PITCH_STABLE_MS {
            return false;
        }

        // 5. Different anchor check (after step limiting)
        let desired_k = self.pc_to_k(dominant_pc);
        let target_k = self.step_limited_k(desired_k);
        if target_k == self.cur_k {
            return false;
        }

        true
    }

    // ========================================================================
    // PHASE TRANSITIONS
    // ========================================================================

    fn begin_acquire(&mut self, dominant_pc: usize) {
        let desired_k = self.pc_to_k(dominant_pc);
        self.target_k = self.step_limited_k(desired_k);

        // Compute direction
        let fwd = (self.target_k - self.cur_k + self.fold) % self.fold;
        let bwd = (self.cur_k - self.target_k + self.fold) % self.fold;
        self.step_dir = if fwd <= bwd { 1 } else { -1 };

        // Lock transition geometry
        self.start_angle = self.angle;
        self.locked_target_angle = self.k_to_angle(self.target_k);

        // Handle wrap for shortest path
        let diff = self.locked_target_angle - self.start_angle;
        if diff.abs() > std::f32::consts::PI {
            if diff > 0.0 {
                self.locked_target_angle -= std::f32::consts::TAU;
            } else {
                self.locked_target_angle += std::f32::consts::TAU;
            }
        }

        // Compute settle amplitude from intensity
        let intensity_factor = self.intensity_ema.clamp(0.15, 1.0);
        self.settle_amp = lerp(Self::OVERSHOOT_MIN, Self::OVERSHOOT_MAX, intensity_factor);

        self.phase = SnapPhase::Acquire;
        self.phase_t = 0.0;

        // Consume trigger and set cooldown
        self.reset = false;
        self.cooldown_ms = Self::COOLDOWN_MS;
    }

    fn begin_moving(&mut self) {
        self.phase = SnapPhase::Moving;
        self.phase_t = 0.0;
    }

    fn begin_settling(&mut self) {
        // Snap to target angle at start of settle
        self.angle = self.locked_target_angle;

        // Initialize oscillator with small phase offset for smooth entry
        self.settle_phase = 0.0;

        self.phase = SnapPhase::Settling;
        self.phase_t = 0.0;
    }

    fn begin_holding(&mut self) {
        // EXACT snap to anchor - no floating point drift
        self.cur_k = self.target_k;
        self.angle = self.k_to_angle(self.cur_k);

        self.phase = SnapPhase::Holding;
        self.phase_t = 0.0;
        self.hold_ms = Self::HOLD_DURATION_MS;
    }

    // ========================================================================
    // PUBLIC API
    // ========================================================================

    /// Force settle to nearest anchor (fail-safe)
    pub fn settle_to_nearest(&mut self) {
        if self.phase == SnapPhase::Moving || self.phase == SnapPhase::Settling {
            return; // Let motion complete
        }
        self.phase = SnapPhase::Idle;
        self.angle = self.k_to_angle(self.cur_k);
    }

    /// Is the cylinder currently in motion?
    pub fn is_moving(&self) -> bool {
        matches!(self.phase, SnapPhase::Moving | SnapPhase::Settling | SnapPhase::Acquire)
    }

    /// Get current phase
    pub fn current_phase(&self) -> SnapPhase {
        self.phase
    }

    // ========================================================================
    // DEBUG HUD DATA
    // ========================================================================

    /// Get debug info for HUD display
    pub fn debug_info(&self) -> CylinderDebugInfo {
        CylinderDebugInfo {
            phase: self.phase.name(),
            fold: self.fold,
            cur_k: self.cur_k,
            target_k: self.target_k,
            angle_deg: self.angle.to_degrees(),
            decision_ema: self.decision_ema,
            intensity_ema: self.intensity_ema,
            armed: self.armed,
            armed_ms: self.armed_ms,
            reset_ms: self.reset_ms,
            cooldown_ms: self.cooldown_ms,
            hold_ms: self.hold_ms,
        }
    }
}

/// Debug info struct for HUD display
#[derive(Clone, Debug)]
pub struct CylinderDebugInfo {
    pub phase: &'static str,
    pub fold: i32,
    pub cur_k: i32,
    pub target_k: i32,
    pub angle_deg: f32,
    pub decision_ema: f32,
    pub intensity_ema: f32,
    pub armed: bool,
    pub armed_ms: f32,
    pub reset_ms: f32,
    pub cooldown_ms: f32,
    pub hold_ms: f32,
}

/// Spring frequency presets
pub mod spring {
    /// Geological (~4-5s settle) - major camera motion, inertial feel
    pub const VERY_SLOW: f32 = 0.8;
    /// Very slow (~2s settle) - distance, major camera moves
    pub const SLOW: f32 = 1.5;
    /// Slow (~1s settle) - inside_dip, FOV
    pub const MEDIUM_SLOW: f32 = 2.5;
    /// Medium (~0.5s settle) - most visual params
    pub const MEDIUM: f32 = 4.0;
    /// Fast (~0.25s settle) - opacity, intensity
    pub const FAST: f32 = 6.0;
    /// Very fast (~0.15s settle) - flicker, rapid response
    pub const VERY_FAST: f32 = 10.0;
}

/// All render-driving parameters as smoothed values
/// This is THE source of truth for what gets rendered.
/// State machines set TARGETS, physics/springs smooth toward them.
///
/// Key camera params use InertialParam for true mass-based motion (momentum).
/// Visual state params use SmoothParam (critically-damped springs) for responsiveness.
#[derive(Clone, Debug)]
pub struct JourneyBlend {
    // === CAMERA (InertialParam for heavy, fluid motion) ===
    pub distance: InertialParam,      // HEAVY - main camera distance
    pub fov: SmoothParam,             // Springs ok for FOV (instant feel is fine)
    pub orbit_speed: InertialParam,   // HEAVY - orbit momentum
    pub orbit_impulse: InertialParam, // MEDIUM - kick impulse with momentum
    pub tilt_angle: InertialParam,    // HEAVY - smooth tilts
    pub noise_rot: SmoothParam,       // Springs ok (small values)
    pub noise_pos: SmoothParam,       // Springs ok (small values)
    pub inside_dip: InertialParam,    // HEAVY - smooth depth changes
    pub aim_angle: InertialAngle,     // GLACIAL - very slow aim drift
    pub aim_radius: SmoothParam,      // Springs ok

    // === WARP/TRANSITION ===
    pub warp_intensity: SmoothParam,

    // === VISUAL STATE ===
    pub field_density: SmoothParam,
    pub surface_opacity: SmoothParam,
    pub interior_enable: SmoothParam,
    pub skin_intensity: SmoothParam,
    pub video_visibility: SmoothParam,

    // === SYMMETRY ===
    pub fold24_t: SmoothParam,

    // === RING/PORTAL ===
    pub ring_alpha: SmoothParam,
    pub portal_depth: SmoothParam,

    // === COMPEL NARRATIVE ===
    pub compel_exterior_bind: SmoothParam,
    pub compel_interior_reveal: SmoothParam,
    pub compel_emissive_persist: SmoothParam,
    pub compel_flicker_gate: SmoothParam,

    // === INTERIOR PARAMS ===
    pub interior_video_boost: SmoothParam,
    pub interior_cell_size: SmoothParam,
    pub interior_lattice_freq: SmoothParam,

    // === RESTING STATE ===
    /// True when in resting phase after journey
    pub is_resting: bool,
    /// Time spent resting (1.0-1.5s before marking journey complete)
    pub resting_time: f32,

    // === REST RELEASE EFFECTS ===
    /// Seam pulse intensity (0-1, decays during release)
    pub seam_pulse: SmoothParam,
    /// Radial breathing factor (centered at 1.0)
    pub radial_breath: SmoothParam,
    /// Wobble angle offset (radians, oscillates then settles)
    pub wobble_offset: SmoothParam,
    /// Orbit speed boost during release
    pub orbit_boost: SmoothParam,
}

impl Default for JourneyBlend {
    fn default() -> Self {
        Self {
            // Camera - TRUE MASS-BASED PHYSICS for heavy, fluid motion
            // InertialParam gives real momentum - objects continue moving!
            distance: InertialParam::new(2.8, inertia::HEAVY),      // Heavy mass, 3-4s settle
            fov: SmoothParam::new(45.0, spring::SLOW),              // Springs ok for FOV
            orbit_speed: InertialParam::new(0.03, inertia::HEAVY),  // Heavy orbit momentum
            orbit_impulse: InertialParam::new(0.0, inertia::MEDIUM), // Medium mass for impulse
            tilt_angle: InertialParam::new(0.18, inertia::HEAVY),   // Heavy tilt
            noise_rot: SmoothParam::new(0.003, spring::MEDIUM),     // Small values, springs ok
            noise_pos: SmoothParam::new(0.004, spring::MEDIUM),
            inside_dip: InertialParam::new(0.0, inertia::HEAVY),    // Heavy dip motion
            aim_angle: InertialAngle::new(0.0, inertia::GLACIAL),   // VERY slow aim drift
            aim_radius: SmoothParam::new(0.05, spring::SLOW),       // Springs ok

            // Warp - medium for dramatic effect
            warp_intensity: SmoothParam::new(0.0, spring::MEDIUM),

            // Visual state - fast for responsiveness
            field_density: SmoothParam::new(0.3, spring::FAST),
            surface_opacity: SmoothParam::new(0.5, spring::FAST),
            interior_enable: SmoothParam::new(0.0, spring::FAST),
            skin_intensity: SmoothParam::new(0.4, spring::FAST),
            video_visibility: SmoothParam::new(0.8, spring::FAST),

            // Symmetry - slow for dramatic reveal
            fold24_t: SmoothParam::new(0.0, spring::MEDIUM_SLOW),

            // Ring/portal - medium
            ring_alpha: SmoothParam::new(1.0, spring::MEDIUM),
            portal_depth: SmoothParam::new(0.0, spring::MEDIUM),

            // Compel narrative - fast
            compel_exterior_bind: SmoothParam::new(0.0, spring::FAST),
            compel_interior_reveal: SmoothParam::new(0.0, spring::FAST),
            compel_emissive_persist: SmoothParam::new(0.0, spring::FAST),
            compel_flicker_gate: SmoothParam::new(0.2, spring::FAST),

            // Interior - medium
            interior_video_boost: SmoothParam::new(1.9, spring::MEDIUM),
            interior_cell_size: SmoothParam::new(0.4, spring::MEDIUM),
            interior_lattice_freq: SmoothParam::new(5.0, spring::MEDIUM),

            // Resting state
            is_resting: false,
            resting_time: 0.0,

            // Rest release effects - fast decay
            seam_pulse: SmoothParam::new(0.0, spring::FAST),
            radial_breath: SmoothParam::new(1.0, spring::MEDIUM),
            wobble_offset: SmoothParam::new(0.0, spring::MEDIUM),
            orbit_boost: SmoothParam::new(0.0, spring::MEDIUM),
        }
    }
}

impl JourneyBlend {
    /// Update all springs toward their targets
    pub fn update(&mut self, dt: f32) {
        self.distance.update(dt);
        self.fov.update(dt);
        self.orbit_speed.update(dt);
        self.orbit_impulse.update(dt);
        self.tilt_angle.update(dt);
        self.noise_rot.update(dt);
        self.noise_pos.update(dt);
        self.inside_dip.update(dt);
        self.aim_angle.update(dt);
        self.aim_radius.update(dt);

        self.warp_intensity.update(dt);

        self.field_density.update(dt);
        self.surface_opacity.update(dt);
        self.interior_enable.update(dt);
        self.skin_intensity.update(dt);
        self.video_visibility.update(dt);

        self.fold24_t.update(dt);

        self.ring_alpha.update(dt);
        self.portal_depth.update(dt);

        self.compel_exterior_bind.update(dt);
        self.compel_interior_reveal.update(dt);
        self.compel_emissive_persist.update(dt);
        self.compel_flicker_gate.update(dt);

        self.interior_video_boost.update(dt);
        self.interior_cell_size.update(dt);
        self.interior_lattice_freq.update(dt);

        // Rest release effects
        self.seam_pulse.update(dt);
        self.radial_breath.update(dt);
        self.wobble_offset.update(dt);
        self.orbit_boost.update(dt);

        // Update resting timer
        if self.is_resting {
            self.resting_time += dt;
        }
    }

    /// Set targets from phase
    pub fn set_phase_targets(&mut self, targets: &PhaseTargets) {
        self.distance.set_target(targets.distance);
        self.fov.set_target(targets.fov);
        self.orbit_speed.set_target(targets.orbit_speed);
        self.tilt_angle.set_target(targets.tilt);
        self.noise_rot.set_target(targets.noise_rot);
        self.noise_pos.set_target(targets.noise_pos);
        self.inside_dip.set_target(targets.inside_dip);
        self.aim_radius.set_target(targets.aim_radius);

        self.field_density.set_target(targets.field_density);
        self.surface_opacity.set_target(targets.surface_opacity);
        self.interior_enable.set_target(targets.interior_enable);
        self.skin_intensity.set_target(targets.skin_intensity);
        self.video_visibility.set_target(targets.video_visibility);

        self.compel_exterior_bind.set_target(targets.compel_exterior_bind);
        self.compel_interior_reveal.set_target(targets.compel_interior_reveal);
        self.compel_emissive_persist.set_target(targets.compel_emissive_persist);
        self.compel_flicker_gate.set_target(targets.compel_flicker_gate);
    }

    /// Set targets for inside state (the big room)
    pub fn set_inside_targets(&mut self) {
        self.surface_opacity.set_target(0.0);
        self.interior_enable.set_target(1.0);
        self.video_visibility.set_target(1.0);
        self.inside_dip.set_target(0.85);
        self.interior_video_boost.set_target(2.0);
        self.compel_interior_reveal.set_target(1.0);
        self.warp_intensity.set_target(0.0);
        self.fov.set_target(58.0);  // Wide FOV for big room
        self.distance.set_target(2.2);  // Settle inside
    }

    /// Set targets for entering transition (hyperspace)
    pub fn set_entering_targets(&mut self, t: f32) {
        // Warp peaks at t=0.4
        let warp_peak = 0.4;
        let warp = if t < warp_peak {
            (t / warp_peak).powi(2)
        } else {
            1.0 - ((t - warp_peak) / (1.0 - warp_peak)).powi(2)
        };
        self.warp_intensity.set_target(warp);

        // FOV: 45 → 120 at peak → 58
        let peak_fov = 120.0;
        let settle_fov = 58.0;
        let fov = if t < warp_peak {
            lerp(45.0, peak_fov, warp)
        } else {
            lerp(peak_fov, settle_fov, (t - warp_peak) / (1.0 - warp_peak))
        };
        self.fov.set_target(fov);

        // Distance: pull back then slam forward
        let peak_dist = 4.0;
        let settle_dist = 1.8;
        let dist = if t < warp_peak * 0.5 {
            lerp(2.5, peak_dist, t / (warp_peak * 0.5))
        } else {
            lerp(peak_dist, settle_dist, (t - warp_peak * 0.5) / (1.0 - warp_peak * 0.5))
        };
        self.distance.set_target(dist);

        // Surface/interior blend
        self.surface_opacity.set_target(lerp(0.5, 0.0, t));
        self.interior_enable.set_target(lerp(0.0, 1.0, t));
        self.video_visibility.set_target(lerp(0.8, 1.0, t));
        self.inside_dip.set_target(lerp(0.0, 0.85, t * t));
    }

    /// Set targets for exiting transition
    pub fn set_exiting_targets(&mut self, t: f32, phase_targets: &PhaseTargets) {
        // Reverse warp - peak at start, fade out
        let warp_peak = 0.3;
        let warp = if t < warp_peak {
            1.0 - (t / warp_peak)
        } else {
            0.0
        };
        self.warp_intensity.set_target(warp);

        // Blend back to phase targets
        self.surface_opacity.set_target(lerp(0.0, phase_targets.surface_opacity, t));
        self.interior_enable.set_target(lerp(1.0, phase_targets.interior_enable, t));
        self.video_visibility.set_target(lerp(1.0, phase_targets.video_visibility, t));
        self.inside_dip.set_target(lerp(0.85, phase_targets.inside_dip, t));
        self.fov.set_target(lerp(58.0, phase_targets.fov, t));
        self.distance.set_target(lerp(2.2, phase_targets.distance, t));
    }

    /// Start resting phase (smooth return to equilibrium)
    pub fn enter_resting(&mut self, n_fold: u32) {
        self.is_resting = true;
        self.resting_time = 0.0;

        // Drift aim to nearest symmetry anchor
        self.aim_angle.drift_to_nearest_anchor(n_fold);

        // Return to normal values
        self.ring_alpha.set_target(1.0);
        self.warp_intensity.set_target(0.0);
    }

    /// Check if resting is complete
    pub fn resting_complete(&self) -> bool {
        self.is_resting && self.resting_time >= 1.2
    }

    /// Exit resting phase
    pub fn exit_resting(&mut self) {
        self.is_resting = false;
        self.resting_time = 0.0;
    }

    /// Check if all parameters have settled
    pub fn all_settled(&self) -> bool {
        self.distance.is_settled()
            && self.fov.is_settled()
            && self.inside_dip.is_settled()
            && self.warp_intensity.is_settled()
            && self.surface_opacity.is_settled()
            && self.interior_enable.is_settled()
            && self.fold24_t.is_settled()
    }

    /// Apply rest release OFFSETS to current blend state (v2: offset-based)
    ///
    /// This function adds the oscillator offsets to whatever targets were already set.
    /// The offsets decay naturally through the damped oscillators, creating smooth
    /// motion that settles to the underlying target.
    ///
    /// Call this AFTER setting normal targets from journey/interior state.
    pub fn apply_rest_release_offsets(&mut self, rest: &RestReleaseState, current_theta: f32) {
        // === ALWAYS-ON EFFECTS (even when inactive, for smooth decay) ===
        // The oscillators continue to ring down even after release ends

        // Rotation offset adds to aim
        let rot_off = rest.rot_offset();
        if rot_off.abs() > 0.0001 {
            self.aim_angle.target += rot_off;
        }

        // Scale offset modifies radial breathing
        let scale_off = rest.scale_offset();
        if scale_off.abs() > 0.0001 {
            self.radial_breath.target += scale_off;
        }

        // Tilt offset adds to tilt
        let tilt_off = rest.tilt_offset();
        if tilt_off.abs() > 0.0001 {
            self.tilt_angle.target += tilt_off;
        }

        if !rest.active {
            // Idle state - decay seam effects
            self.seam_pulse.set_target(self.seam_pulse.target * 0.95);
            return;
        }

        // === ACTIVE RELEASE EFFECTS ===

        // Seam-locked ripple (replaces camera shake)
        // This creates the "REG feel" - patterns locked to symmetry
        let ripple = rest.seam_ripple(current_theta);
        self.seam_pulse.set_target(rest.seam_intensity + ripple * 0.3);

        // Ring bloom
        self.ring_alpha.set_target(1.0 + rest.ring_bloom * 0.4);

        // Orbit boost
        self.orbit_boost.set_target(rest.orbit_boost);

        // FOV micro-pulse from seam ripple (subtle)
        let fov_base = self.fov.target.min(50.0);
        self.fov.set_target(fov_base + ripple * 2.0 * rest.released_energy);

        // NOTE: aim_angle lock phase disabled - CylinderMotion system now
        // handles all anchor alignment via state machine (anticipate → snap → swing → settle)
    }

    /// Legacy function - redirects to offset-based approach
    pub fn set_rest_release_targets(&mut self, rest: &RestReleaseState) {
        // Call the offset-based version with a default theta
        // The actual theta should come from the caller
        self.apply_rest_release_offsets(rest, 0.0);
    }
}
