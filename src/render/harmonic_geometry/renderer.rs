//! Geometric Field Renderer
//!
//! REG-Aligned renderer implementing ThemeRenderer trait.
//! Geometry emerges directly from the 12-bin chroma vector.
//!
//! ## Philosophy
//!
//! - No named solids (torus, hyperboloid, gyroid)
//! - The 12-bin chroma IS the geometry
//! - Symmetry emerges from resonance
//! - Disorder appears only when the music is noisy

use anyhow::{anyhow, Result};
use eframe::glow::{self, HasContext};
use image::GenericImageView;
use std::sync::Arc;

use super::{
    harmonic_field::HarmonicField,
    key_detector::{HarmonicLabel, KeyDetector},
    presets::HeroConfig,
    shaders,
    skin::{FieldSkin, SkinMode},
    CameraRig, DynamicCameraState, HeroAct, InteriorState,
};
use crate::render::theme::{AudioAnalysis, RendererType, ThemePreset, ThemeRenderer};

/// Shader initialization steps for frame-distributed loading
/// Each step does a small amount of work to keep the UI responsive
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderInitStep {
    /// Not started yet
    NotStarted,
    /// Compiling main harmonic field shader (GPU-intensive)
    CompilingMainShader,
    /// Creating VAO/VBO buffers (fast)
    CreatingBuffers,
    /// Loading QS numbers atlas texture
    LoadingQsAtlas,
    /// Loading chroma notes atlas texture
    LoadingChromaAtlas,
    /// Loading GLB model for silence state
    LoadingGlbModel,
    /// Compiling TAA resolve shader (if enabled)
    CompilingTaaShader,
    /// Warming up shader (forces GPU pipeline compilation)
    WarmingUp,
    /// All initialization complete
    Complete,
}

impl ShaderInitStep {
    /// Get a human-readable description of the current step
    pub fn description(&self) -> &'static str {
        match self {
            ShaderInitStep::NotStarted => "Preparing...",
            ShaderInitStep::CompilingMainShader => "Compiling shaders...",
            ShaderInitStep::CreatingBuffers => "Creating buffers...",
            ShaderInitStep::LoadingQsAtlas => "Loading textures...",
            ShaderInitStep::LoadingChromaAtlas => "Loading textures...",
            ShaderInitStep::LoadingGlbModel => "Loading 3D model...",
            ShaderInitStep::CompilingTaaShader => "Compiling TAA shader...",
            ShaderInitStep::WarmingUp => "Warming up GPU...",
            ShaderInitStep::Complete => "Ready",
        }
    }

    /// Get progress (0.0 - 1.0) for this step
    pub fn progress(&self) -> f32 {
        match self {
            ShaderInitStep::NotStarted => 0.0,
            ShaderInitStep::CompilingMainShader => 0.1,
            ShaderInitStep::CreatingBuffers => 0.3,
            ShaderInitStep::LoadingQsAtlas => 0.4,
            ShaderInitStep::LoadingChromaAtlas => 0.5,
            ShaderInitStep::LoadingGlbModel => 0.6,
            ShaderInitStep::CompilingTaaShader => 0.7,
            ShaderInitStep::WarmingUp => 0.85,
            ShaderInitStep::Complete => 1.0,
        }
    }
}

/// Note names for display (C through B)
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Smoothstep interpolation (Hermite)
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Display state for UI/debugging
#[derive(Clone, Debug)]
pub struct HarmonicFieldState {
    /// Smoothed chroma values (display-ready, mean-subtracted, normalized)
    pub chroma: [f32; 12],
    /// Raw averaged chroma (for variance/balance calculation)
    pub raw_chroma: [f32; 12],
    /// Dominant pitch class index (0-11) - HELD with hysteresis, not raw argmax
    pub dominant_pitch: usize,
    /// Dominant pitch name (or "—" if not tonal)
    pub dominant_name: &'static str,
    /// Raw dominant pitch class (instant argmax, for debugging)
    pub raw_dominant_pitch: usize,
    /// Whether chroma passes tonal validity gates
    pub is_tonal: bool,
    /// Tonal confidence (0-1)
    pub tonal_confidence: f32,
    /// Tonal energy (chroma bin sum, for tonal content detection)
    pub tonal_energy: f32,
    /// Perceptual audio energy (remapped for UI, 0-1 visible range)
    pub energy_vis: f32,
    /// Temporal stability (0-1) - how stable chroma is frame-to-frame
    pub stability: f32,
    /// Harmonic balance (0-1): uniform chroma distribution = 1.0, single-note = 0.0
    /// Used for lighting palette selection (low balance = cooler colors)
    pub balance: f32,
    /// Gate confidence [0..1] - authoritative kill switch for gate-dependent effects
    pub gate_ok: f32,
    /// Current rotation phase (0.0-1.0, wraps)
    pub phase: f32,
    /// Current preset name
    pub preset_name: &'static str,
    /// Render mode description
    pub render_mode: &'static str,
    /// Symmetry mode N (12 or 24) — the active quantization grid
    pub symmetry: u8,
    /// Camera anchor index (0..N-1) — the one true discrete angle source
    pub anchor_k: u8,
    /// Quantized anchor angle in degrees: anchor_k * (360/N) — ALWAYS on-grid
    pub anchor_theta_deg: f32,
    /// Harmonic label (Stable, Rotating, Ambiguous, Atonal)
    pub harmonic_label: HarmonicLabel,
    /// Journey phase (Birth/Lock/Dive/Afterglow) - emergent temporal act
    pub journey_phase: HeroAct,
    /// Time spent in current journey phase (seconds)
    pub journey_phase_time: f32,
    /// Interior latch state (Outside/Entering/Inside/Exiting)
    pub interior_state: InteriorState,
    /// RingDive active state
    pub ring_dive_active: bool,
    /// RingDive timeline progress (0-1)
    pub ring_dive_t: f32,
    /// RingDive current subrange name
    pub ring_dive_subrange: &'static str,
    /// Fold24 active state (24-fold transition) - DEPRECATED, use engaged
    pub fold24_active: bool,
    /// Fold24 transition progress (0-1)
    pub fold24_t: f32,
    /// Fold24 engaged state (new model)
    pub fold24_engaged: bool,
    /// Fold24 request latch bars (how long request held)
    pub fold24_request_latch_bars: u32,
    /// Tension (slow EMA) for UI display
    pub tension_slow: f32,
    /// QuaternionCharges active state
    pub qc_active: bool,
    /// QuaternionCharges reduced angle w (degrees)
    pub qc_w_deg: f32,
    /// QuaternionCharges port angles (degrees)
    pub qc_ports_deg: [f32; 4],
    /// QuaternionCharges active port index
    pub qc_active_port: usize,
    /// QuaternionCharges overlay strength (0-1)
    pub qc_strength: f32,
    /// QuaternionCharges invariant ok flags [complements, total, palindrome, lattice]
    pub qc_ok: [bool; 4],
    /// TorusKnot active state (tertiary inside transition)
    pub torus_knot_active: bool,
    /// TorusKnot transition progress (0-1)
    pub torus_knot_t: f32,
    /// TorusKnot subphase name
    pub torus_knot_phase: &'static str,
    /// TorusKnot (p,q) ratio
    pub torus_knot_p: i32,
    pub torus_knot_q: i32,
    /// TorusKnot canvas name (e.g., "Golden 55:89")
    pub torus_canvas_name: &'static str,
    /// TorusKnot paint name (e.g., "P5")
    pub torus_paint_name: &'static str,
    /// TorusKnot winding interval (WI = p*360/q degrees, raw)
    pub torus_winding_interval: f32,
    /// TorusKnot effective winding interval (wrapped to 0-360°)
    pub torus_winding_interval_eff: f32,
    /// TorusKnot bead count (sampling density)
    pub torus_bead_count: i32,
    /// TorusKnot link count (1 = knot, >1 = link)
    pub torus_link_count: i32,
    /// TorusKnot is high complexity (performance warning)
    pub torus_is_high_complexity: bool,

    // === ICONIC TUNING STATE ===
    /// Spice gate active (7/11-limit boosts enabled)
    pub torus_spice_active: bool,
    /// Spice level (0, 1, 2)
    pub torus_spice_level: u8,
    /// Spice level target (what level is stepping toward)
    pub torus_spice_level_target: u8,
    /// Spice eligible (true if 7-limit or 11-limit topology)
    pub torus_spice_eligible: bool,
    /// Prime limit of current topology
    pub torus_spice_prime_limit: i32,
    /// Coherence latched (true if harmonic coherence conditions met)
    pub torus_spice_coherent: bool,
    /// Spice debug: harmonic confidence (stability_smooth)
    pub torus_spice_hc: f32,
    /// Spice debug: chroma entropy (0-1)
    pub torus_spice_entropy: f32,
    /// Spice debug: max bin probability (0-1)
    pub torus_spice_p_max: f32,
    /// Spice debug: sum of squared probs (0-1)
    pub torus_spice_p2: f32,
    /// Spice debug: accumulated stable time (seconds)
    pub torus_spice_stable_time: f32,
    /// Phrase envelope (0.0-1.0)
    pub torus_phrase_env: f32,
    /// Phrase bias (-1, 0, +1)
    pub torus_phrase_bias: i8,
    /// Ornament ID (0=none, 1=filigree, 2=notches, 3=halo)
    pub torus_ornament_id: u8,
    /// Ornament intensity (0.0, 0.5, 1.0)
    pub torus_ornament_intensity: f32,
    /// Ornament pattern (0, 1, 2)
    pub torus_ornament_pattern: u8,
    /// Ornament spoke mask (24-bit)
    pub torus_ornament_spoke_mask: u32,
    /// WI accent step (sector index)
    pub torus_wi_accent_step: u8,
    /// WI accent active
    pub torus_wi_accent_active: bool,
    /// WI accent timer (for fade)
    pub torus_wi_accent_timer: f32,

    // === POLARITY STATE ===
    /// Current polarity quadrant (0=E/+1, 1=N/+i, 2=W/-1, 3=S/-i)
    pub polarity_quadrant: u8,
    /// Previous polarity quadrant
    pub polarity_prev_quadrant: u8,
    /// REG harmonic integer A (1-360) — informational, NOT a symmetry angle.
    /// Derived from compute_degree_a(pitch_class, energy). Not quantized to 30°/15° grid.
    pub reg_degree_a: i32,

    // === TOPOLOGY ROUTER STATE ===
    /// Current topology name
    pub topology_name: &'static str,
    /// Topology crossfade progress (0-1)
    pub topology_xfade_t: f32,
    /// Current bar index
    pub topology_bar_index: u64,
    /// Router bar-averaged energy (debug)
    pub router_energy_bar: f32,
    /// Router mid-dominant flag (debug)
    pub router_mid_dominant: bool,
    /// Router tension slope (debug)
    pub router_tension_slope: f32,
    /// Router hold time (seconds)
    pub router_hold_s: f32,

    // === VORTEX STATE ===
    /// Vortex active state
    pub vortex_active: bool,
    /// Vortex transition progress (0-1)
    pub vortex_t: f32,
    /// Vortex gate center (0-23)
    pub vortex_gate: u8,
    /// Vortex direction name
    pub vortex_direction: &'static str,

    // === DODECAHEDRAL STATE ===
    /// Dodecahedral active state
    pub dodeca_active: bool,
    /// Dodecahedral transition progress (0-1)
    pub dodeca_t: f32,
    /// Dodecahedral cooldown remaining (seconds)
    pub dodeca_cooldown: f32,
    /// Phi resonance (0-1)
    pub dodeca_phi: f32,
    /// Current chroma entropy (for debug)
    pub dodeca_entropy: f32,
    /// Entry conditions met (for debug)
    pub dodeca_entry_ok: bool,
}

impl Default for HarmonicFieldState {
    fn default() -> Self {
        Self {
            chroma: [0.0; 12],
            raw_chroma: [0.0; 12],
            dominant_pitch: 0,
            dominant_name: "—",
            raw_dominant_pitch: 0,
            is_tonal: false,
            tonal_confidence: 0.0,
            tonal_energy: 0.0,
            energy_vis: 0.0,
            stability: 0.0,
            balance: 0.0,
            gate_ok: 0.0,
            phase: 0.0,
            preset_name: "Hero's Journey",
            render_mode: "Extrusion",
            symmetry: 12,
            anchor_k: 0,
            anchor_theta_deg: 0.0,
            harmonic_label: HarmonicLabel::Atonal,
            journey_phase: HeroAct::Birth,
            journey_phase_time: 0.0,
            interior_state: InteriorState::Outside,
            ring_dive_active: false,
            ring_dive_t: 0.0,
            ring_dive_subrange: "none",
            fold24_active: false,
            fold24_t: 0.0,
            fold24_engaged: false,
            fold24_request_latch_bars: 0,
            tension_slow: 0.0,
            qc_active: false,
            qc_w_deg: 0.0,
            qc_ports_deg: [0.0; 4],
            qc_active_port: 0,
            qc_strength: 0.0,
            qc_ok: [false; 4],
            torus_knot_active: false,
            torus_knot_t: 0.0,
            torus_knot_phase: "Idle",
            torus_knot_p: 3,
            torus_knot_q: 2,
            torus_canvas_name: "Golden 8:13",
            torus_paint_name: "Unison",
            torus_winding_interval: 221.5,  // 8*360/13
            torus_winding_interval_eff: 221.5,  // No wrap needed for this value
            torus_bead_count: 8,
            torus_link_count: 1,
            torus_is_high_complexity: false,

            // Iconic tuning defaults
            torus_spice_active: false,
            torus_spice_level: 0,
            torus_spice_level_target: 0,
            torus_spice_eligible: false,
            torus_spice_prime_limit: 3,  // Default T(3:2) is 3-limit
            torus_spice_coherent: false,
            torus_spice_hc: 0.0,
            torus_spice_entropy: 1.0,
            torus_spice_p_max: 0.0,
            torus_spice_p2: 0.0,
            torus_spice_stable_time: 0.0,
            torus_phrase_env: 0.5,
            torus_phrase_bias: 0,
            torus_ornament_id: 0,
            torus_ornament_intensity: 0.0,
            torus_ornament_pattern: 0,
            torus_ornament_spoke_mask: 0xFFFFFF,
            torus_wi_accent_step: 0,
            torus_wi_accent_active: false,
            torus_wi_accent_timer: 0.0,

            // Polarity defaults
            polarity_quadrant: 0,
            polarity_prev_quadrant: 0,
            reg_degree_a: 0,

            // Topology router defaults
            topology_name: "Scaffold24",
            topology_xfade_t: 0.0,
            topology_bar_index: 0,
            router_energy_bar: 0.0,
            router_mid_dominant: false,
            router_tension_slope: 0.0,
            router_hold_s: 0.0,

            // Vortex defaults
            vortex_active: false,
            vortex_t: 0.0,
            vortex_gate: 0,
            vortex_direction: "Inward",

            // Dodecahedral
            dodeca_active: false,
            dodeca_t: 0.0,
            dodeca_cooldown: 0.0,
            dodeca_phi: 0.0,
            dodeca_entropy: 1.0,
            dodeca_entry_ok: false,
        }
    }
}

/// Geometric Field Renderer
///
/// Renders the harmonic field - geometry that emerges from the 12-bin chroma.
/// No named shapes, no beat-synced animation. Slow, geological motion.
///
/// GPU OPTIMIZATION: When inside, quality is dynamically reduced via u_interior_quality
/// uniform. This reduces raymarch steps and layer count to prevent GPU burn.
pub struct HarmonicGeometryRenderer {
    /// OpenGL context
    gl: Option<Arc<glow::Context>>,

    /// The harmonic field - core data structure
    harmonic_field: HarmonicField,

    /// Hero configuration (single canonical voice)
    hero_config: HeroConfig,

    // OpenGL resources - main shader
    program: Option<glow::Program>,
    vao: Option<glow::VertexArray>,
    vbo: Option<glow::Buffer>,

    /// Whether initialized
    initialized: bool,

    /// Current shader initialization step (for frame-distributed loading)
    init_step: ShaderInitStep,

    /// Render dimensions
    width: u32,
    height: u32,

    /// Time accumulator (for slow rotation)
    time: f32,

    /// Base rotation angle (slow, continuous)
    rotation: f32,

    /// Independent field rotation (audio-reactive spiral drift)
    /// Decoupled from base rotation so field spins while QS wheel stays fixed
    field_rotation: f32,

    /// Cached tonal confidence from last update
    tonal_confidence: f32,

    /// Cached tonal energy from last update
    tonal_energy: f32,

    /// Perceptual audio energy (remapped for UI, 0-1 visible range)
    energy_vis: f32,

    /// Temporal stability (0-1) - how stable chroma is frame-to-frame
    stability: f32,

    /// Whether audio currently passes tonal validity gates
    is_tonal: bool,

    /// Kick detected this frame (for specular flash)
    kick: f32,

    /// Continuous low-band onset strength from the analyzer (kick-envelope flux),
    /// carried through to LightingMetrics for the tempo autocorrelator.
    onset_strength: f32,

    /// Frequency band energies (for spectral rings)
    bass: f32,
    mid: f32,
    high: f32,

    /// Raw energy sum (pre-normalization) for true silence detection.
    /// Updated each frame from AudioAnalysis.raw_bass + raw_mid + raw_high.
    raw_energy: f32,
    raw_bass: f32,
    raw_high: f32,

    /// Slow EMA band energies for AAA rings (τ≈300ms, smooth musical motion)
    bass_ema: f32,
    mid_ema: f32,
    high_ema: f32,

    /// Peak frequencies per band (Hz) - smoothed for display
    bass_peak_hz: f32,
    mid_peak_hz: f32,
    high_peak_hz: f32,

    /// AAA Ring style settings
    ring_style_aaa: bool,        // false = classic, true = AAA
    ring_flow_lines: bool,       // animated energy flow on rings
    ring_sparkle: bool,          // specular sparkle on rings
    ring_fog: bool,              // depth fog for 3D feel
    ring_bloom_mask: bool,       // emit bloom from ring cores

    /// Live band centroid frequencies (Hz) for spectral ring labels
    /// [Sub, Kick, Low-mid, Mid, High-mid, Air]
    band_centroids: [f32; 6],

    /// Dominant pitch class (0-11) - from KeyDetector (held with hysteresis)
    dominant_pc: usize,

    /// Raw dominant pitch class (instant argmax, for debugging)
    raw_dominant_pc: usize,

    /// Previous dominant pitch class (for detecting changes)
    prev_dominant_pc: usize,

    /// Fast-smoothed chroma for responsive UI display (shader/HUD)
    /// Separate from harmonic_field.chroma_smooth() which uses slow smoothing
    display_chroma: [f32; 12],

    /// Key detector with dominant pitch latching
    key_detector: KeyDetector,

    /// Active skin configuration
    skin: FieldSkin,

    /// Active skin preset name (or None for custom)
    skin_preset_name: Option<String>,

    /// External video texture for skin (from pipeline's Compel video)
    /// This is borrowed from the pipeline, not owned by the renderer
    skin_texture: Option<glow::Texture>,

    /// Whether the skin texture is valid and should be used
    skin_texture_valid: bool,

    /// Video aspect ratio (width / height) - used for portrait detection
    video_aspect: f32,

    /// Camera texture for background media layer (composited with skin in wheel)
    camera_texture: Option<glow::Texture>,

    /// Camera texture dimensions (width, height)
    camera_texture_dims: (u32, u32),

    /// Camera alpha (0-1) for blending with skin texture
    camera_alpha: f32,

    /// QS Wheel number atlas texture (1-360 pre-rendered digits)
    qs_numbers_texture: Option<glow::Texture>,

    /// Chroma wheel note atlas texture (C, C#, D, ... B pre-rendered labels)
    chroma_notes_texture: Option<glow::Texture>,

    /// Dynamic camera state (blended between Observer/Immersive/Performer modes)
    camera_state: DynamicCameraState,

    // === GLB 3D Model (REG logo for silence state) ===
    /// GLB mesh shader program
    glb_program: Option<glow::Program>,
    /// GLB mesh VAO
    glb_vao: Option<glow::VertexArray>,
    /// GLB mesh VBO (vertices: position + normal + uv)
    glb_vbo: Option<glow::Buffer>,
    /// GLB mesh EBO (indices)
    glb_ebo: Option<glow::Buffer>,
    /// GLB texture (baked material colors)
    glb_texture: Option<glow::Texture>,
    /// Number of indices to draw
    glb_index_count: u32,
    /// Whether GLB is loaded
    glb_loaded: bool,
    /// Smoothed GLB visibility (0-1, with heavy easing)
    glb_visibility: f32,

    // === PERFORMANCE + QUALITY SYSTEM ===
    /// Quality tier (0=Low, 1=Med, 2=High)
    pub quality_tier: u8,
    /// Dynamic render scale (0.5-1.0) for SDF pass
    render_scale: f32,
    /// License-based render scale cap (entitlements.render_scale).
    /// Clamps the maximum render_scale regardless of quality tier.
    license_render_cap: f32,
    /// Watermark uniform value: 1.0 = show watermark, 0.0 = hide
    watermark_value: f32,
    /// Target SDF time budget in ms (for dynamic scaling)
    sdf_time_budget_ms: f32,
    /// Last measured SDF render time in ms
    last_sdf_time_ms: f32,
    /// EMA of SDF render time for smooth adaptation
    sdf_time_ema_ms: f32,
    /// Frame time EMA for display
    frame_time_ema_ms: f32,
    /// Last frame timestamp
    last_frame_time: std::time::Instant,

    // === OFFSCREEN FBO FOR DYNAMIC RESOLUTION ===
    /// Offscreen FBO for low-res SDF rendering
    sdf_fbo: Option<glow::Framebuffer>,
    /// Offscreen color texture
    sdf_texture: Option<glow::Texture>,
    /// Current offscreen resolution
    sdf_width: u32,
    sdf_height: u32,
    /// Upscale shader program
    upscale_program: Option<glow::Program>,

    // === TORUS KNOT POLYLINE (Precomputed curve points) ===
    /// Precomputed torus knot curve points (uploaded as texture)
    torus_curve_texture: Option<glow::Texture>,
    /// Number of curve points
    torus_curve_count: u32,
    /// Last computed (p,q) for curve
    torus_curve_pq: (i32, i32),

    // === PERF HUD DATA ===
    /// Average ray steps (from shader feedback - simulated)
    perf_avg_steps: f32,
    /// Max ray steps this frame
    perf_max_steps: u32,
    /// Hit rate (% pixels hitting geometry)
    perf_hit_rate: f32,
    /// Show perf HUD
    pub show_perf_hud: bool,

    // === BACKGROUND QUALITY BUDGET SYSTEM ===
    /// BG quality tier (0=Low, 1=Med, 2=High)
    bg_quality: u8,
    /// Auto-scale BG quality based on frame time
    bg_quality_auto: bool,
    /// Consecutive frames above 18ms (for auto-downgrade)
    bg_frame_time_hot_streak: u32,
    /// Consecutive frames below 14ms (for auto-upgrade)
    bg_frame_time_cool_streak: u32,

    // === TEMPORAL ANTI-ALIASING (TAA) ===
    /// TAA enabled flag
    taa_enabled: bool,
    /// TAA history texture (previous resolved frame)
    taa_history_texture: Option<glow::Texture>,
    /// TAA history FBO
    taa_history_fbo: Option<glow::Framebuffer>,
    /// TAA current frame texture (render target)
    taa_current_texture: Option<glow::Texture>,
    /// TAA current FBO
    taa_current_fbo: Option<glow::Framebuffer>,
    /// TAA resolve shader program
    taa_resolve_program: Option<glow::Program>,
    /// Frame index for jitter sequence (wraps at 8)
    taa_frame_index: u32,
    /// Previous rotation for motion vector calculation
    taa_prev_rotation: f32,
    /// TAA texture dimensions (for resize detection)
    taa_width: u32,
    taa_height: u32,
}

impl Default for HarmonicGeometryRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl HarmonicGeometryRenderer {
    pub fn new() -> Self {
        let hero_config = HeroConfig::hero();

        let mut harmonic_field = HarmonicField::new(
            hero_config.base_radius,
            hero_config.gain,
        );
        harmonic_field.set_smoothing_tau(hero_config.smoothing_tau);

        Self {
            gl: None,
            harmonic_field,
            hero_config,
            program: None,
            vao: None,
            vbo: None,
            initialized: false,
            init_step: ShaderInitStep::NotStarted,
            width: 1920,
            height: 1080,
            time: 0.0,
            rotation: 0.0,
            field_rotation: 0.0,
            tonal_confidence: 0.0,
            tonal_energy: 0.0,
            energy_vis: 0.0,
            stability: 0.0,
            is_tonal: false,
            kick: 0.0,
            onset_strength: 0.0,
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            raw_energy: 0.0,
            raw_bass: 0.0,
            raw_high: 0.0,
            // Slow EMA for AAA rings (smooth musical motion)
            bass_ema: 0.0,
            mid_ema: 0.0,
            high_ema: 0.0,
            // Peak frequencies per band
            bass_peak_hz: 80.0,
            mid_peak_hz: 1000.0,
            high_peak_hz: 8000.0,
            // AAA Ring settings (defaults: AAA on, all features enabled)
            ring_style_aaa: true,
            ring_flow_lines: true,
            ring_sparkle: true,
            ring_fog: true,
            ring_bloom_mask: true,
            band_centroids: [35.0, 112.0, 355.0, 1122.0, 3550.0, 11225.0],
            dominant_pc: 0,
            raw_dominant_pc: 0,
            prev_dominant_pc: 0,
            display_chroma: [0.0; 12],
            key_detector: KeyDetector::new(),
            skin: FieldSkin::default(),
            skin_preset_name: None,
            skin_texture: None,
            skin_texture_valid: false,
            video_aspect: 1.0,
            camera_texture: None,
            camera_texture_dims: (0, 0),
            camera_alpha: 0.0,
            qs_numbers_texture: None,
            chroma_notes_texture: None,
            camera_state: DynamicCameraState::default(),
            // GLB mesh
            glb_program: None,
            glb_vao: None,
            glb_vbo: None,
            glb_ebo: None,
            glb_texture: None,
            glb_index_count: 0,
            glb_loaded: false,
            glb_visibility: 1.0,  // Start visible (silence state)

            // Performance + Quality system
            quality_tier: 1,  // Default to Medium
            render_scale: 1.0,
            license_render_cap: 1.0,  // No cap until entitlements loaded
            watermark_value: 1.0,     // Watermark on by default until entitlements loaded
            sdf_time_budget_ms: 12.0,  // Target 12ms for SDF pass
            last_sdf_time_ms: 0.0,
            sdf_time_ema_ms: 8.0,
            frame_time_ema_ms: 16.0,
            last_frame_time: std::time::Instant::now(),

            // Offscreen FBO (created in init)
            sdf_fbo: None,
            sdf_texture: None,
            sdf_width: 0,
            sdf_height: 0,
            upscale_program: None,

            // Torus knot polyline
            torus_curve_texture: None,
            torus_curve_count: 0,
            torus_curve_pq: (0, 0),

            // Perf HUD
            perf_avg_steps: 0.0,
            perf_max_steps: 0,
            perf_hit_rate: 0.0,
            show_perf_hud: false,

            // BG Quality Budget System
            bg_quality: 1,              // Default to Medium
            bg_quality_auto: true,       // Auto-scale enabled by default
            bg_frame_time_hot_streak: 0,
            bg_frame_time_cool_streak: 0,

            // TAA (Temporal Anti-Aliasing)
            taa_enabled: true,           // TAA enabled by default
            taa_history_texture: None,
            taa_history_fbo: None,
            taa_current_texture: None,
            taa_current_fbo: None,
            taa_resolve_program: None,
            taa_frame_index: 0,
            taa_prev_rotation: 0.0,
            taa_width: 0,
            taa_height: 0,
        }
    }

    // ========== TAA (TEMPORAL ANTI-ALIASING) SYSTEM ==========

    /// Enable or disable TAA
    pub fn set_taa_enabled(&mut self, enabled: bool) {
        self.taa_enabled = enabled;
        log::info!("[HarmonicGeometry] TAA {}", if enabled { "enabled" } else { "disabled" });
    }

    /// Check if TAA is enabled
    pub fn is_taa_enabled(&self) -> bool {
        self.taa_enabled
    }

    /// Toggle TAA on/off
    pub fn toggle_taa(&mut self) {
        self.taa_enabled = !self.taa_enabled;
        // Reset frame index when toggling to ensure clean history
        self.taa_frame_index = 0;
        log::info!("[HarmonicGeometry] TAA toggled: {}", if self.taa_enabled { "ON" } else { "OFF" });
    }

    // ========== SKIN SYSTEM ==========

    /// Set the active skin configuration
    pub fn set_skin(&mut self, skin: FieldSkin) {
        self.skin = skin;
        self.skin_preset_name = None;
        log::trace!("[HarmonicGeometry] Skin updated: mode={:?}, strength={:.2}",
            self.skin.mode, self.skin.strength);
    }

    /// Enable Hero skin mode (act-driven compel visibility)
    pub fn enable_hero_skin(&mut self) {
        self.skin = FieldSkin::hero();
        self.skin_preset_name = Some("Hero's Journey".to_string());
        log::trace!("[HarmonicGeometry] Hero skin enabled - act system controls visibility");
    }

    /// Disable skin (pure harmonic field)
    pub fn disable_skin(&mut self) {
        self.skin = FieldSkin::none();
        self.skin_preset_name = None;
        log::trace!("[HarmonicGeometry] Skin disabled");
    }

    /// Get current skin configuration
    pub fn skin(&self) -> &FieldSkin {
        &self.skin
    }

    /// Get mutable reference to skin for UI editing
    pub fn skin_mut(&mut self) -> &mut FieldSkin {
        &mut self.skin
    }

    /// Check if Hero skin is active
    pub fn is_hero_skin_active(&self) -> bool {
        self.skin.is_active()
    }

    /// Set the video texture to use as skin (called each frame from pipeline)
    /// The texture is owned by the pipeline, not by this renderer
    ///
    /// HERO PRESET: When video is loaded, automatically configure skin for
    /// OBVIOUS visibility (SurfaceEmissive mode, high strength)
    ///
    /// aspect_ratio: video width / height (used for portrait detection in QS wheel)
    pub fn set_skin_texture(&mut self, texture: Option<glow::Texture>, aspect_ratio: f32) {
        let was_valid = self.skin_texture_valid;
        self.skin_texture = texture;
        self.skin_texture_valid = texture.is_some();
        self.video_aspect = aspect_ratio;

        // Auto-configure skin for video visibility when texture becomes available
        if self.skin_texture_valid && !was_valid {
            // If skin was in None mode, switch to SurfaceEmissive for obvious video
            if self.skin.mode == SkinMode::None {
                self.skin.mode = SkinMode::SurfaceEmissive;
                self.skin.strength = 0.9;  // High strength for visibility
                self.skin.emissive_strength = 0.4;  // Glow from video
                log::trace!("[HarmonicGeometry] Auto-configured skin for video: SurfaceEmissive, strength=0.9");
            }
        }
    }

    /// Configure skin for Hero preset (obvious video visibility)
    pub fn configure_hero_skin(&mut self) {
        self.skin.mode = SkinMode::SurfaceEmissive;
        self.skin.strength = 0.9;
        self.skin.emissive_strength = 0.5;
        self.skin.rim_strength = 0.25;
        self.skin.bloom_amount = 0.2;
        log::trace!("[HarmonicGeometry] Configured Hero skin preset");
    }

    /// Check if a skin texture is set
    pub fn has_skin_texture(&self) -> bool {
        self.skin_texture_valid && self.skin_texture.is_some()
    }

    /// Set camera texture for background media layer (composited with skin in QS wheel)
    /// texture: Camera texture (RGB)
    /// width, height: Texture dimensions
    /// alpha: Blend factor (0 = no camera, 1 = camera only)
    pub fn set_camera_texture(&mut self, texture: Option<glow::Texture>, width: u32, height: u32, alpha: f32) {
        self.camera_texture = texture;
        self.camera_texture_dims = (width, height);
        self.camera_alpha = alpha.clamp(0.0, 1.0);
    }

    /// Update camera alpha without changing texture
    pub fn set_camera_alpha(&mut self, alpha: f32) {
        self.camera_alpha = alpha.clamp(0.0, 1.0);
    }

    /// Check if camera texture is set
    pub fn has_camera_texture(&self) -> bool {
        self.camera_texture.is_some() && self.camera_alpha > 0.001
    }

    /// Toggle symmetry HUD visibility
    pub fn toggle_hud(&mut self) {
        self.camera_state.toggle_hud();
        log::trace!("[HarmonicGeometry] HUD toggled: {}", self.camera_state.hud_visible);
    }

    /// Check if HUD is visible
    pub fn is_hud_visible(&self) -> bool {
        self.camera_state.hud_visible
    }

    /// Set HUD visibility
    pub fn set_hud_visible(&mut self, visible: bool) {
        self.camera_state.hud_visible = visible;
    }

    /// Toggle QS Wheel visibility
    pub fn toggle_qs_wheel(&mut self) {
        self.camera_state.toggle_qs_wheel();
        log::trace!("[HarmonicGeometry] QS Wheel toggled: {}", self.camera_state.qs_wheel_visible);
    }

    /// Check if QS Wheel is visible
    pub fn is_qs_wheel_visible(&self) -> bool {
        self.camera_state.qs_wheel_visible
    }

    /// Set QS Wheel visibility
    pub fn set_qs_wheel_visible(&mut self, visible: bool) {
        self.camera_state.set_qs_wheel_visible(visible);
    }

    /// Set whether to hide QS Wheel numbers
    pub fn set_hide_qs_numbers(&mut self, hide: bool) {
        self.camera_state.set_hide_qs_numbers(hide);
    }

    /// Set QS Wheel dominant-only mode (true = dominant only, false = all notes)
    pub fn set_qs_dominant_only(&mut self, dominant_only: bool) {
        self.camera_state.set_qs_dominant_only(dominant_only);
    }

    /// Get QS Wheel dominant-only mode
    pub fn is_qs_dominant_only(&self) -> bool {
        self.camera_state.is_qs_dominant_only()
    }

    /// Get debug HUD strings for display
    pub fn get_debug_hud_strings(&self) -> Vec<String> {
        self.camera_state.get_debug_hud_strings()
    }

    /// Set state from frozen frame (for freeze + scrub playback)
    /// Updates internal harmonic field to display frozen chroma
    pub fn set_frozen_state(&mut self, state: &HarmonicFieldState) {
        // Update internal state from frozen frame
        self.harmonic_field.set_chroma_direct(&state.chroma);
        self.is_tonal = state.is_tonal;
        self.tonal_confidence = state.tonal_confidence;
        self.tonal_energy = state.tonal_energy;
        self.energy_vis = state.energy_vis;
        self.stability = state.stability;
        // Keep rotation frozen at the frame's phase
        self.rotation = state.phase * std::f32::consts::TAU;
    }

    // ========== QUALITY + PERFORMANCE SYSTEM ==========

    /// Set quality tier (0=Low, 1=Med, 2=High)
    pub fn set_quality_tier(&mut self, tier: u8) {
        self.quality_tier = tier.min(3);
        // Set render scale based on tier, capped by license entitlement
        self.render_scale = match self.quality_tier {
            0 => 0.6f32,  // Low: fixed 60%
            1 => 0.8,     // Med: start at 80%, adapt dynamically
            2 => 1.0,     // High: full resolution
            _ => 1.5,     // Ultra: 1.5x supersampling for big screens
        }.min(self.license_render_cap);
        log::info!("[HarmonicGeometry] Quality tier set to {} (render_scale={}, cap={})", tier, self.render_scale, self.license_render_cap);
    }

    /// Get current quality tier
    pub fn get_quality_tier(&self) -> u8 {
        self.quality_tier
    }

    /// Toggle perf HUD visibility
    pub fn toggle_perf_hud(&mut self) {
        self.show_perf_hud = !self.show_perf_hud;
    }

    /// Apply license entitlements to rendering parameters
    pub fn apply_entitlements(&mut self, entitlements: &crate::auth::Entitlements) {
        self.license_render_cap = entitlements.render_scale;
        self.watermark_value = if entitlements.watermark { 1.0 } else { 0.0 };
        // Re-apply quality tier with the new cap
        self.render_scale = self.render_scale.min(self.license_render_cap);
    }

    // ========== AAA RING SETTINGS ==========

    /// Set AAA ring style (true = 3D torus, false = classic flat)
    pub fn set_ring_style_aaa(&mut self, enabled: bool) {
        self.ring_style_aaa = enabled;
    }

    /// Set ring flow lines enabled
    pub fn set_ring_flow_lines(&mut self, enabled: bool) {
        self.ring_flow_lines = enabled;
    }

    /// Set ring sparkle enabled
    pub fn set_ring_sparkle(&mut self, enabled: bool) {
        self.ring_sparkle = enabled;
    }

    /// Set ring fog enabled
    pub fn set_ring_fog(&mut self, enabled: bool) {
        self.ring_fog = enabled;
    }

    /// Set ring bloom mask enabled
    pub fn set_ring_bloom_mask(&mut self, enabled: bool) {
        self.ring_bloom_mask = enabled;
    }

    // ========== BG QUALITY BUDGET SYSTEM ==========

    /// Set BG quality tier (0=Low, 1=Med, 2=High)
    pub fn set_bg_quality(&mut self, quality: u8) {
        self.bg_quality = quality.min(2);
    }

    /// Get current BG quality tier
    pub fn get_bg_quality(&self) -> u8 {
        self.bg_quality
    }

    /// Set BG quality auto-scaling enabled
    pub fn set_bg_quality_auto(&mut self, enabled: bool) {
        self.bg_quality_auto = enabled;
        // Reset streaks when toggling
        self.bg_frame_time_hot_streak = 0;
        self.bg_frame_time_cool_streak = 0;
    }

    /// Get BG quality auto-scaling state
    pub fn get_bg_quality_auto(&self) -> bool {
        self.bg_quality_auto
    }

    /// Get performance stats for HUD display
    pub fn perf_stats(&self) -> (f32, f32, f32, f32, f32) {
        (
            self.frame_time_ema_ms,
            self.sdf_time_ema_ms,
            self.perf_avg_steps,
            self.perf_hit_rate,
            self.render_scale,
        )
    }

    /// Check if video upload is needed (PERF OPTIMIZATION)
    /// Returns false when video/skin texture isn't contributing to the render,
    /// allowing callers to skip expensive GPU texture uploads
    pub fn needs_video_upload(&self) -> bool {
        // PRELOAD GRACE PERIOD: Always upload for first 3 seconds to create GPU resources
        // This prevents freezing when video visibility is first increased from 0
        if self.time < 3.0 {
            return true;
        }

        let cam = &self.camera_state;

        // Video is needed if:
        // 1. Video visibility is meaningful (for surface skin or interior)
        // 2. Interior video boost is active (for chopped video inside)
        // 3. Skin intensity is visible (surface texture contribution)
        let video_visible = cam.video_visibility > 0.05;
        let interior_video = cam.interior_state.is_inside() && cam.interior_video_boost > 0.1;
        let skin_visible = cam.skin_intensity > 0.05;

        video_visible || interior_video || skin_visible
    }

    /// Update dynamic render scale based on GPU time
    fn update_render_scale(&mut self) {
        if self.quality_tier != 1 {
            return; // Only Medium tier does dynamic scaling
        }

        // Smooth adaptation using EMA, capped by license entitlement
        let cap = self.license_render_cap;
        let target_scale = if self.sdf_time_ema_ms > self.sdf_time_budget_ms * 1.2 {
            // Running hot - reduce scale
            (self.render_scale - 0.02).max(0.5)
        } else if self.sdf_time_ema_ms < self.sdf_time_budget_ms * 0.8 {
            // Running cool - increase scale
            (self.render_scale + 0.01).min(cap)
        } else {
            self.render_scale
        };

        // Smooth transition
        self.render_scale = (self.render_scale * 0.95 + target_scale * 0.05).min(cap);
    }

    /// Compute torus knot curve points for polyline SDF
    /// Returns Vec of (x, y, z) normalized positions
    fn compute_torus_curve(&self, p: i32, q: i32, num_points: usize) -> Vec<f32> {
        let mut points = Vec::with_capacity(num_points * 3);
        let major_r = self.camera_state.torus_knot.config.major_radius;
        let minor_r = self.camera_state.torus_knot.config.minor_radius;

        for i in 0..num_points {
            let t = i as f32 / num_points as f32;
            let phi = (p as f32) * t * std::f32::consts::TAU;
            let theta = (q as f32) * t * std::f32::consts::TAU;

            let rr = minor_r * theta.cos() + major_r;
            let x = rr * phi.cos();
            let y = rr * phi.sin();
            let z = minor_r * theta.sin();

            points.push(x);
            points.push(y);
            points.push(z);
        }
        points
    }

    /// Upload torus curve to GPU texture
    fn upload_torus_curve(&mut self, gl: &glow::Context, p: i32, q: i32) {
        // Skip if already computed for this (p,q)
        if self.torus_curve_pq == (p, q) && self.torus_curve_texture.is_some() {
            return;
        }

        let num_points = 512; // 512 points for smooth curve
        let points = self.compute_torus_curve(p, q, num_points);

        unsafe {
            // Create or reuse texture
            let texture = self.torus_curve_texture.unwrap_or_else(|| gl.create_texture().unwrap());

            gl.bind_texture(glow::TEXTURE_2D, Some(texture));

            // Upload as RGB32F texture (512x1)
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGB32F as i32,
                num_points as i32,
                1,
                0,
                glow::RGB,
                glow::FLOAT,
                Some(bytemuck::cast_slice(&points)),
            );

            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);

            self.torus_curve_texture = Some(texture);
            self.torus_curve_count = num_points as u32;
            self.torus_curve_pq = (p, q);
        }
    }

    /// Get harmonic field state for UI display
    pub fn field_state(&self) -> HarmonicFieldState {
        // Use FAST display_chroma for responsive UI (bars react quickly)
        // This is the fast-smoothed chroma from the audio analyzer
        let chroma = self.display_chroma;
        // Raw chroma for variance/balance calculation (not mean-subtracted)
        let raw_chroma = *self.key_detector.raw_chroma();

        // Use HELD dominant from key_detector (latched with hysteresis)
        let held_dominant = self.dominant_pc;
        let raw_dominant = self.raw_dominant_pc;

        let render_mode = match self.hero_config.render_mode {
            super::harmonic_field::RenderMode::Extrusion => "Extrusion",
            super::harmonic_field::RenderMode::Lathe => "Lathe",
            super::harmonic_field::RenderMode::Both => "Both",
        };

        // Show "—" when not tonal (noise/silence)
        let dominant_name = if self.is_tonal {
            NOTE_NAMES[held_dominant]
        } else {
            "—"
        };

        // Get harmonic label from key_detector
        let harmonic_label = self.key_detector.harmonic_label();

        // Compute phase (0-1) from rotation angle
        let phase = (self.rotation / std::f32::consts::TAU).fract();

        HarmonicFieldState {
            chroma,
            raw_chroma,
            dominant_pitch: held_dominant,
            dominant_name,
            raw_dominant_pitch: raw_dominant,
            is_tonal: self.is_tonal,
            tonal_confidence: self.tonal_confidence,
            tonal_energy: self.tonal_energy,
            energy_vis: self.energy_vis,
            stability: self.stability,
            balance: self.camera_state.balance,
            gate_ok: self.camera_state.gate_ok,
            phase,
            preset_name: "Hero's Journey",
            render_mode,
            harmonic_label,
            // Symmetry mode N and anchor state (the sacred quantized values)
            symmetry: if self.camera_state.fold24.t > 0.1 { 24 } else { 12 },
            anchor_k: self.camera_state.camera_reg.anchor_k,
            anchor_theta_deg: {
                let n = if self.camera_state.fold24.t > 0.1 { 24u8 } else { 12u8 };
                self.camera_state.camera_reg.anchor_k as f32 * (360.0 / n as f32)
            },
            journey_phase: self.camera_state.phase,
            journey_phase_time: self.camera_state.phase_time,
            interior_state: self.camera_state.interior_state,
            // RingDive debug info
            ring_dive_active: self.camera_state.ring_dive.active,
            ring_dive_t: self.camera_state.ring_dive.t,
            ring_dive_subrange: self.camera_state.ring_dive.subrange(),
            // Fold24 debug info
            fold24_active: self.camera_state.fold24.active,
            fold24_t: self.camera_state.fold24.t,
            fold24_engaged: self.camera_state.fold24.engaged,
            fold24_request_latch_bars: self.camera_state.fold24.request_latch_bars,
            // Tension for UI
            tension_slow: self.camera_state.tension_slow,
            // QuaternionCharges debug info
            qc_active: self.camera_state.qc.active,
            qc_w_deg: self.camera_state.qc.w.to_degrees(),
            qc_ports_deg: [
                self.camera_state.qc.ports[0].to_degrees(),
                self.camera_state.qc.ports[1].to_degrees(),
                self.camera_state.qc.ports[2].to_degrees(),
                self.camera_state.qc.ports[3].to_degrees(),
            ],
            qc_active_port: self.camera_state.qc.active_port,
            qc_strength: self.camera_state.qc.strength,
            qc_ok: [
                self.camera_state.qc.invariants.complements_ok,
                self.camera_state.qc.invariants.total_ok,
                self.camera_state.qc.invariants.palindrome_ok,
                self.camera_state.qc.invariants.lattice_ok_24,
            ],
            // TorusKnot debug info
            torus_knot_active: self.camera_state.torus_knot.active,
            torus_knot_t: self.camera_state.torus_knot.t,
            torus_knot_phase: self.camera_state.torus_knot.subphase.name(),
            torus_knot_p: self.camera_state.torus_knot.p,
            torus_knot_q: self.camera_state.torus_knot.q,
            // Canvas name: recognize family from actual p,q (not from canvas preset)
            torus_canvas_name: crate::render::harmonic_geometry::recognize_family(
                self.camera_state.torus_knot.p,
                self.camera_state.torus_knot.q
            ),
            torus_paint_name: self.camera_state.torus_knot.paint.name(),
            // WI computed from REDUCED p,q (consistent with all derived stats)
            torus_winding_interval: {
                let (pr, qr) = super::reduced_pq(
                    self.camera_state.torus_knot.p,
                    self.camera_state.torus_knot.q
                );
                (pr as f32 * 360.0) / qr as f32
            },
            torus_winding_interval_eff: {
                let (pr, qr) = super::reduced_pq(
                    self.camera_state.torus_knot.p,
                    self.camera_state.torus_knot.q
                );
                let wi_raw = (pr as f32 * 360.0) / qr as f32;
                wi_raw % 360.0
            },
            torus_bead_count: self.camera_state.torus_knot.bead_count,
            torus_link_count: self.camera_state.torus_knot.link_count,
            // High complexity based on actual p*q product
            torus_is_high_complexity: self.camera_state.torus_knot.p * self.camera_state.torus_knot.q > 1000,

            // Iconic tuning state
            torus_spice_active: self.camera_state.torus_knot.spice_gate_active,
            torus_spice_level: self.camera_state.torus_knot.spice_level,
            torus_spice_level_target: self.camera_state.torus_knot.spice_level_target,
            torus_spice_eligible: self.camera_state.torus_knot.spice_eligible,
            torus_spice_prime_limit: self.camera_state.torus_knot.spice_prime_limit,
            torus_spice_coherent: self.camera_state.torus_knot.spice_coherence_latched,
            torus_spice_hc: self.camera_state.stability_smooth,
            torus_spice_entropy: self.camera_state.torus_knot.spice_last_entropy,
            torus_spice_p_max: self.camera_state.torus_knot.spice_last_p_max,
            torus_spice_p2: self.camera_state.torus_knot.spice_last_p2,
            torus_spice_stable_time: self.camera_state.torus_knot.spice_stable_time,
            torus_phrase_env: self.camera_state.torus_knot.phrase_env,
            torus_phrase_bias: self.camera_state.torus_knot.phrase_bias,
            torus_ornament_id: self.camera_state.torus_knot.ornament_id,
            torus_ornament_intensity: self.camera_state.torus_knot.ornament_intensity,
            torus_ornament_pattern: self.camera_state.torus_knot.ornament_pattern,
            torus_ornament_spoke_mask: self.camera_state.torus_knot.ornament_spoke_mask,
            torus_wi_accent_step: self.camera_state.torus_knot.wi_accent_step,
            torus_wi_accent_active: self.camera_state.torus_knot.wi_accent_active,
            torus_wi_accent_timer: self.camera_state.torus_knot.wi_accent_timer,

            // Polarity state
            polarity_quadrant: self.camera_state.qc.current_quadrant,
            polarity_prev_quadrant: self.camera_state.qc.prev_quadrant,
            reg_degree_a: self.camera_state.qc.current_degree,

            // Topology router state
            topology_name: self.camera_state.topology_router.topology_id.name(),
            topology_xfade_t: self.camera_state.topology_router.topology_xfade_t,
            topology_bar_index: self.camera_state.topology_router.bar_index,
            router_energy_bar: self.camera_state.topology_router.energy_bar,
            router_mid_dominant: {
                let mid_sum: f32 = self.camera_state.smoothed_chroma[2..8].iter().sum();
                let bass_sum: f32 = self.camera_state.smoothed_chroma[0..3].iter().sum();
                let high_sum: f32 = self.camera_state.smoothed_chroma[8..12].iter().sum();
                mid_sum > bass_sum && mid_sum > high_sum
            },
            router_tension_slope: self.camera_state.topology_router.tension_slope,
            router_hold_s: self.camera_state.topology_router.topology_hold_s,

            // Vortex state
            vortex_active: self.camera_state.vortex.active,
            vortex_t: self.camera_state.vortex.t,
            vortex_gate: self.camera_state.vortex.gate_center,
            vortex_direction: match self.camera_state.vortex.direction {
                crate::render::harmonic_geometry::VortexDirection::Inward => "Inward",
                crate::render::harmonic_geometry::VortexDirection::Outward => "Outward",
            },

            // Dodecahedral state
            dodeca_active: self.camera_state.dodecahedral.active,
            dodeca_t: self.camera_state.dodecahedral.t,
            dodeca_cooldown: self.camera_state.dodecahedral.cooldown,
            dodeca_phi: self.camera_state.dodecahedral.phi_resonance,
            dodeca_entropy: self.camera_state.torus_knot.spice_last_entropy,
            dodeca_entry_ok: {
                // Check if entry conditions are currently met (for debug)
                let entropy = self.camera_state.torus_knot.spice_last_entropy;
                let stability = self.camera_state.stability_smooth;
                let energy = self.camera_state.topology_router.energy_bar;
                let in_vortex = self.camera_state.topology_router.topology_id == crate::render::harmonic_geometry::TopologyId::Vortex;
                in_vortex
                    && entropy < crate::render::harmonic_geometry::dodeca_const::ENTRY_ENTROPY_MAX
                    && stability > crate::render::harmonic_geometry::dodeca_const::ENTRY_STABILITY_MIN
                    && energy > crate::render::harmonic_geometry::dodeca_const::ENTRY_MID_ENERGY_MIN
            },
        }
    }

    /// Get reference to the key detector for external use (e.g., OSC output)
    pub fn key_detector(&self) -> &KeyDetector {
        &self.key_detector
    }

    /// Get mutable reference to the key detector (e.g., tuning changes from UI)
    pub fn key_detector_mut(&mut self) -> &mut KeyDetector {
        &mut self.key_detector
    }

    /// Get lighting metrics for OSC cue engine
    /// These are the 4 core audio-reactive signals for lighting control
    pub fn lighting_metrics(&self) -> crate::render::LightingMetrics {
        crate::render::LightingMetrics {
            // Use energy_vis directly (AudioAnalyzer's smoothed normalized energy)
            // NOT section_energy, which applies an additional 0.8s tau smoother
            // designed for the 3D camera macro journey — too slow for lighting zones.
            energy: self.energy_vis,
            stability: self.stability,
            balance: self.camera_state.balance,
            tension: self.camera_state.tension_fast,
            // Use gate_stability_lp as beat confidence proxy (both measure confidence)
            beat_confidence: self.camera_state.camera_reg.gate_stability_lp,
            // Onset detected when spectral flux spikes above threshold
            onset: self.camera_state.spectral_flux > 0.05,
            kick: self.kick > 0.5,
            onset_strength: self.onset_strength,
            // Track if dominant pitch class changed from last frame
            dominant_changed: self.dominant_pc != self.prev_dominant_pc,
            dominant_pc: self.dominant_pc as u8,
            // Polarity state from quadrant controller
            polarity_quadrant: self.camera_state.qc.current_quadrant,
            polarity_prev_quadrant: self.camera_state.qc.prev_quadrant,
            polarity_changed: self.camera_state.qc.current_quadrant != self.camera_state.qc.prev_quadrant,
            // Tonality from key detector
            major_likelihood: self.key_detector.major_likelihood(),
            // Frequency bands for macro state machine (fast-smoothed ~15Hz)
            bass: self.bass,
            mid: self.mid,
            high: self.high,
            // Raw energy for true silence detection (pre-normalization sum)
            raw_energy: self.raw_energy,
            raw_bass: self.raw_bass,
            raw_high: self.raw_high,
            // Collatz fields default to 0 (populated by pipeline from Mythos if active)
            collatz_heat: 0.0,
            collatz_phase: 0.0,
            collatz_intensity: 0.0,
            collatz_breath: 0.0,
            collatz_act: 0,
            // Harmonic field state (emerge field → autonomous lighting): the field
            // rotation phase + normalized quaternion ports, both already computed
            // upstream this frame. Radians (0..TAU) → 0-1; ports 0 & 2 differ by 0.5.
            phase: (self.rotation / std::f32::consts::TAU).fract(),
            qc_ports: [
                (self.camera_state.qc.ports[0] / std::f32::consts::TAU).rem_euclid(1.0),
                (self.camera_state.qc.ports[1] / std::f32::consts::TAU).rem_euclid(1.0),
                (self.camera_state.qc.ports[2] / std::f32::consts::TAU).rem_euclid(1.0),
                (self.camera_state.qc.ports[3] / std::f32::consts::TAU).rem_euclid(1.0),
            ],
            // Journey phase (HeroAct) + time-in-phase — the emerge temporal act,
            // the same source field_state() uses. Drives the B1 lighting macro.
            journey_phase: self.camera_state.phase,
            journey_phase_time: self.camera_state.phase_time,
            // Harmonic coherence (B2): the same trust the visuals use (u_trust).
            trust: self.harmonic_field.trust(),
            // Quaternion overlay (B3): same source as the u_qc_* visual uniforms.
            qc_strength: self.camera_state.qc.strength,
            qc_active_port: self.camera_state.qc.active_port as u8,
            // Symmetry order (B4): same fold24 derivation field_state() uses.
            symmetry: if self.camera_state.fold24.t > 0.1 { 24 } else { 12 },
            // Chroma (B5): the slew-limited variant — its stability-adaptive slew
            // already prevents colour flopping, so no extra smoothing pass needed.
            chroma: *self.harmonic_field.chroma_smooth(),
        }
    }

    /// Compile a shader program
    fn compile_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> Result<glow::Program> {
        unsafe {
            let vs = gl
                .create_shader(glow::VERTEX_SHADER)
                .map_err(|e| anyhow!("Failed to create vertex shader: {}", e))?;
            gl.shader_source(vs, vs_src);
            gl.compile_shader(vs);
            if !gl.get_shader_compile_status(vs) {
                let log = gl.get_shader_info_log(vs);
                gl.delete_shader(vs);
                return Err(anyhow!("Vertex shader compile error: {}", log));
            }

            let fs = gl
                .create_shader(glow::FRAGMENT_SHADER)
                .map_err(|e| anyhow!("Failed to create fragment shader: {}", e))?;
            gl.shader_source(fs, fs_src);
            gl.compile_shader(fs);
            if !gl.get_shader_compile_status(fs) {
                let log = gl.get_shader_info_log(fs);
                gl.delete_shader(vs);
                gl.delete_shader(fs);
                return Err(anyhow!("Fragment shader compile error: {}", log));
            }

            let program = gl
                .create_program()
                .map_err(|e| anyhow!("Failed to create program: {}", e))?;
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                gl.delete_shader(vs);
                gl.delete_shader(fs);
                return Err(anyhow!("Program link error: {}", log));
            }

            gl.delete_shader(vs);
            gl.delete_shader(fs);

            Ok(program)
        }
    }

    /// Load a PNG image as an OpenGL texture
    fn load_png_texture(gl: &glow::Context, path: &str) -> Result<glow::Texture> {
        // Load image from file
        let img = image::open(path)
            .map_err(|e| anyhow!("Failed to load image {}: {}", path, e))?;

        let (width, height) = img.dimensions();
        let rgba = img.to_rgba8();
        let pixels = rgba.as_raw();

        unsafe {
            let texture = gl
                .create_texture()
                .map_err(|e| anyhow!("Failed to create texture: {}", e))?;

            gl.bind_texture(glow::TEXTURE_2D, Some(texture));

            // Upload pixel data
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(pixels),
            );

            // Set texture parameters - use NEAREST to prevent interpolation artifacts
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

            gl.bind_texture(glow::TEXTURE_2D, None);

            log::trace!("[HarmonicGeometry] Loaded texture: {} ({}x{})", path, width, height);
            Ok(texture)
        }
    }

    /// Execute the next initialization step (frame-distributed loading)
    /// Returns (is_complete, current_step) - call each frame until complete
    /// This spreads shader compilation across multiple frames to keep UI responsive
    pub fn init_next_step(&mut self, gl: &Arc<glow::Context>) -> Result<(bool, ShaderInitStep)> {
        use ShaderInitStep::*;

        match self.init_step {
            NotStarted => {
                log::info!("[HarmonicGeometry] Starting frame-distributed initialization...");
                self.init_step = CompilingMainShader;
                Ok((false, self.init_step))
            }

            CompilingMainShader => {
                log::info!("[HarmonicGeometry] Step: Compiling main shader...");
                let program = Self::compile_program(
                    gl,
                    shaders::vertex_shader(),
                    shaders::harmonic_field_fragment_shader(),
                )?;
                self.program = Some(program);
                log::info!("[HarmonicGeometry] Main shader compiled successfully");
                self.init_step = CreatingBuffers;
                Ok((false, self.init_step))
            }

            CreatingBuffers => {
                log::info!("[HarmonicGeometry] Step: Creating VAO/VBO...");
                let program = self.program.ok_or_else(|| anyhow!("Program not compiled"))?;
                unsafe {
                    let vao = gl
                        .create_vertex_array()
                        .map_err(|e| anyhow!("Failed to create VAO: {}", e))?;
                    gl.bind_vertex_array(Some(vao));

                    let vbo = gl
                        .create_buffer()
                        .map_err(|e| anyhow!("Failed to create VBO: {}", e))?;
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

                    let vertices: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];
                    gl.buffer_data_u8_slice(
                        glow::ARRAY_BUFFER,
                        bytemuck::cast_slice(&vertices),
                        glow::STATIC_DRAW,
                    );

                    let pos_loc = gl.get_attrib_location(program, "a_pos").unwrap_or(0);
                    gl.enable_vertex_attrib_array(pos_loc);
                    gl.vertex_attrib_pointer_f32(pos_loc, 2, glow::FLOAT, false, 8, 0);

                    gl.bind_vertex_array(None);

                    self.vao = Some(vao);
                    self.vbo = Some(vbo);
                }
                self.init_step = LoadingQsAtlas;
                Ok((false, self.init_step))
            }

            LoadingQsAtlas => {
                log::info!("[HarmonicGeometry] Step: Loading QS numbers atlas...");
                let atlas_paths = crate::resources::candidates("assets/qs_numbers_atlas.png");
                for path in atlas_paths.iter().filter_map(|p| p.to_str()) {
                    if let Ok(tex) = Self::load_png_texture(gl, path) {
                        self.qs_numbers_texture = Some(tex);
                        log::info!("[HarmonicGeometry] Loaded QS numbers atlas from: {}", path);
                        break;
                    }
                }
                if self.qs_numbers_texture.is_none() {
                    log::warn!("[HarmonicGeometry] QS numbers atlas not found - digits will not render");
                }
                self.init_step = LoadingChromaAtlas;
                Ok((false, self.init_step))
            }

            LoadingChromaAtlas => {
                log::info!("[HarmonicGeometry] Step: Loading chroma notes atlas...");
                let chroma_atlas_paths = crate::resources::candidates("assets/chroma_notes_atlas.png");
                for path in chroma_atlas_paths.iter().filter_map(|p| p.to_str()) {
                    if let Ok(tex) = Self::load_png_texture(gl, path) {
                        self.chroma_notes_texture = Some(tex);
                        log::info!("[HarmonicGeometry] Loaded chroma notes atlas from: {}", path);
                        break;
                    }
                }
                if self.chroma_notes_texture.is_none() {
                    log::warn!("[HarmonicGeometry] Chroma notes atlas not found - note labels will use fallback");
                }
                self.init_step = LoadingGlbModel;
                Ok((false, self.init_step))
            }

            LoadingGlbModel => {
                log::info!("[HarmonicGeometry] Step: Loading GLB model...");
                self.load_glb_model(gl);
                self.init_step = CompilingTaaShader;
                Ok((false, self.init_step))
            }

            CompilingTaaShader => {
                if self.taa_enabled {
                    log::info!("[HarmonicGeometry] Step: Compiling TAA shader...");
                    match Self::compile_program(gl, shaders::vertex_shader(), shaders::taa_resolve_fragment_shader()) {
                        Ok(p) => {
                            self.taa_resolve_program = Some(p);
                            log::info!("[HarmonicGeometry] TAA resolve shader compiled successfully");
                        }
                        Err(e) => {
                            log::warn!("[HarmonicGeometry] TAA resolve shader failed: {}. TAA disabled.", e);
                            self.taa_enabled = false;
                        }
                    }
                } else {
                    log::info!("[HarmonicGeometry] Step: Skipping TAA shader (disabled)");
                }
                self.init_step = WarmingUp;
                Ok((false, self.init_step))
            }

            WarmingUp => {
                log::info!("[HarmonicGeometry] Step: Warming up shader...");
                self.warmup_shader(gl);
                log::info!("[HarmonicGeometry] Shader warmup complete");
                self.init_step = Complete;
                self.gl = Some(gl.clone());
                self.initialized = true;
                log::info!("[HarmonicGeometry] Frame-distributed initialization complete!");
                Ok((true, self.init_step))
            }

            Complete => {
                // Already complete
                Ok((true, self.init_step))
            }
        }
    }

    /// Get current initialization step (for progress display)
    pub fn get_init_step(&self) -> ShaderInitStep {
        self.init_step
    }

    /// Initialize OpenGL resources (legacy synchronous method)
    fn init_gl(&mut self, gl: &Arc<glow::Context>) -> Result<()> {
        // Compile harmonic field shader
        log::info!("[HarmonicGeometry] Compiling shader program...");
        let program = match Self::compile_program(
            gl,
            shaders::vertex_shader(),
            shaders::harmonic_field_fragment_shader(),
        ) {
            Ok(p) => {
                log::info!("[HarmonicGeometry] Shader compiled successfully");
                p
            }
            Err(e) => {
                log::error!("[HarmonicGeometry] SHADER COMPILATION FAILED: {}", e);
                return Err(e);
            }
        };
        self.program = Some(program);

        unsafe {
            // Create VAO
            let vao = gl
                .create_vertex_array()
                .map_err(|e| anyhow!("Failed to create VAO: {}", e))?;
            gl.bind_vertex_array(Some(vao));

            // Create VBO for fullscreen triangle
            let vbo = gl
                .create_buffer()
                .map_err(|e| anyhow!("Failed to create VBO: {}", e))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

            // Fullscreen triangle (more efficient than quad)
            let vertices: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&vertices),
                glow::STATIC_DRAW,
            );

            // Setup vertex attribute
            let pos_loc = gl.get_attrib_location(program, "a_pos").unwrap_or(0);
            gl.enable_vertex_attrib_array(pos_loc);
            gl.vertex_attrib_pointer_f32(pos_loc, 2, glow::FLOAT, false, 8, 0);

            gl.bind_vertex_array(None);

            self.vao = Some(vao);
            self.vbo = Some(vbo);
        }

        // Load QS numbers atlas texture
        // Try multiple paths for development vs production
        let atlas_paths = crate::resources::candidates("assets/qs_numbers_atlas.png");

        for path in atlas_paths.iter().filter_map(|p| p.to_str()) {
            match Self::load_png_texture(gl, path) {
                Ok(tex) => {
                    self.qs_numbers_texture = Some(tex);
                    log::info!("[HarmonicGeometry] Loaded QS numbers atlas from: {}", path);
                    break;
                }
                Err(e) => {
                    log::trace!("[HarmonicGeometry] Atlas not at {}: {}", path, e);
                }
            }
        }

        if self.qs_numbers_texture.is_none() {
            log::warn!("[HarmonicGeometry] QS numbers atlas not found - digits will not render");
        }

        // Load chroma note atlas texture (C, C#, D, ... B)
        let chroma_atlas_paths = crate::resources::candidates("assets/chroma_notes_atlas.png");

        for path in chroma_atlas_paths.iter().filter_map(|p| p.to_str()) {
            match Self::load_png_texture(gl, path) {
                Ok(tex) => {
                    self.chroma_notes_texture = Some(tex);
                    log::info!("[HarmonicGeometry] Loaded chroma notes atlas from: {}", path);
                    break;
                }
                Err(e) => {
                    log::trace!("[HarmonicGeometry] Chroma atlas not at {}: {}", path, e);
                }
            }
        }

        if self.chroma_notes_texture.is_none() {
            log::warn!("[HarmonicGeometry] Chroma notes atlas not found - note labels will use fallback");
        }

        // Load REG GLB model for silence state
        self.load_glb_model(gl);

        // Initialize TAA resolve shader
        if self.taa_enabled {
            match Self::compile_program(gl, shaders::vertex_shader(), shaders::taa_resolve_fragment_shader()) {
                Ok(p) => {
                    self.taa_resolve_program = Some(p);
                    log::info!("[HarmonicGeometry] TAA resolve shader compiled successfully");
                }
                Err(e) => {
                    log::warn!("[HarmonicGeometry] TAA resolve shader failed: {}. TAA disabled.", e);
                    self.taa_enabled = false;
                }
            }
        }

        log::trace!("[HarmonicGeometry] OpenGL resources initialized");
        Ok(())
    }

    /// Initialize or resize TAA framebuffers
    fn init_taa_fbos(&mut self, gl: &glow::Context, width: u32, height: u32) {
        if !self.taa_enabled || (self.taa_width == width && self.taa_height == height) {
            return;
        }

        // Cleanup existing TAA resources
        unsafe {
            if let Some(tex) = self.taa_history_texture.take() {
                gl.delete_texture(tex);
            }
            if let Some(fbo) = self.taa_history_fbo.take() {
                gl.delete_framebuffer(fbo);
            }
            if let Some(tex) = self.taa_current_texture.take() {
                gl.delete_texture(tex);
            }
            if let Some(fbo) = self.taa_current_fbo.take() {
                gl.delete_framebuffer(fbo);
            }
        }

        // Create new TAA textures and FBOs
        unsafe {
            // History texture
            let history_tex = gl.create_texture().expect("TAA: create history texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(history_tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA16F as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::HALF_FLOAT,
                None,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

            let history_fbo = gl.create_framebuffer().expect("TAA: create history FBO");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(history_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(history_tex),
                0,
            );

            // Current frame texture
            let current_tex = gl.create_texture().expect("TAA: create current texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(current_tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA16F as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::HALF_FLOAT,
                None,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

            let current_fbo = gl.create_framebuffer().expect("TAA: create current FBO");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(current_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(current_tex),
                0,
            );

            // Unbind
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);

            self.taa_history_texture = Some(history_tex);
            self.taa_history_fbo = Some(history_fbo);
            self.taa_current_texture = Some(current_tex);
            self.taa_current_fbo = Some(current_fbo);
            self.taa_width = width;
            self.taa_height = height;

            // Clear history to black on first frame
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(history_fbo));
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            log::info!("[HarmonicGeometry] TAA FBOs initialized: {}x{}", width, height);
        }
    }

    /// Get sub-pixel jitter for current frame using Halton(2,3) sequence
    fn get_taa_jitter(&self) -> (f32, f32) {
        // Halton(2,3) sequence for 8-frame cycle - provides good temporal distribution
        const JITTER_SEQUENCE: [(f32, f32); 8] = [
            (0.500, 0.333),  // Frame 0
            (0.250, 0.666),  // Frame 1
            (0.750, 0.111),  // Frame 2
            (0.125, 0.444),  // Frame 3
            (0.625, 0.777),  // Frame 4
            (0.375, 0.222),  // Frame 5
            (0.875, 0.555),  // Frame 6
            (0.0625, 0.888), // Frame 7
        ];

        let idx = (self.taa_frame_index % 8) as usize;
        let (jx, jy) = JITTER_SEQUENCE[idx];

        // Scale to sub-pixel offset: jitter in range [-0.5, 0.5] pixels
        let w = self.width as f32;
        let h = self.height as f32;
        ((jx - 0.5) / w, (jy - 0.5) / h)
    }

    /// Load GLB 3D model for silence state display
    fn load_glb_model(&mut self, gl: &Arc<glow::Context>) {
        // Try reg.glb first, fallback to compeller.glb if that fails
        let glb_files = vec!["reg.glb", "compeller.glb"];

        for glb_file in &glb_files {
            if self.try_load_glb_file(gl, glb_file) {
                return;
            }
        }

        log::warn!("[HarmonicGeometry] No GLB models could be loaded");
    }

    fn try_load_glb_file(&mut self, gl: &Arc<glow::Context>, filename: &str) -> bool {
        // Build paths list dynamically to include app bundle Resources for production
        // Executable dir, .app Resources, repo root (cargo run), then cwd-relative
        let mut glb_paths: Vec<std::path::PathBuf> =
            crate::resources::candidates(&format!("public/3d/{}", filename));

        // Add app bundle Resources path for production (macOS .app bundle)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(bundle_path) = exe_path.parent().and_then(|p| p.parent()) {
                glb_paths.push(bundle_path.join(format!("Resources/3d/{}", filename)));
                glb_paths.push(bundle_path.join(format!("Resources/public/3d/{}", filename)));
            }
        }


        log::info!("[HarmonicGeometry] Looking for GLB model (cwd: {:?})...", std::env::current_dir());

        let mut glb_data = None;
        for path in &glb_paths {
            log::info!("[HarmonicGeometry] Trying GLB path: {:?}", path);
            if let Ok(data) = std::fs::read(path) {
                log::info!("[HarmonicGeometry] Found GLB at: {:?} ({} bytes)", path, data.len());
                glb_data = Some(data);
                break;
            }
        }

        let data = match glb_data {
            Some(d) => d,
            None => {
                log::warn!("[HarmonicGeometry] GLB model {} not found", filename);
                return false;
            }
        };

        // Parse GLB - try import_slice first, fallback to manual parsing for WebP textures
        let (document, buffers, images): (gltf::Document, Vec<gltf::buffer::Data>, Vec<gltf::image::Data>) = match gltf::import_slice(&data) {
            Ok((doc, bufs, imgs)) => (doc, bufs, imgs),
            Err(e) => {
                log::warn!("[HarmonicGeometry] gltf::import_slice failed: {}. Trying manual parse...", e);
                // Fallback: parse GLB header manually to skip texture validation
                match gltf::Glb::from_slice(&data) {
                    Ok(glb) => {
                        // Parse JSON part with relaxed validation
                        match gltf::Gltf::from_slice(&glb.json) {
                            Ok(gltf_doc) => {
                                // Create buffer data from GLB binary chunk
                                let buffer_data: Vec<gltf::buffer::Data> = if let Some(bin) = glb.bin {
                                    vec![gltf::buffer::Data(bin.into_owned())]
                                } else {
                                    log::error!("[HarmonicGeometry] GLB has no binary chunk");
                                    return false;
                                };
                                // No images in fallback mode
                                (gltf_doc.document, buffer_data, Vec::new())
                            }
                            Err(e2) => {
                                log::error!("[HarmonicGeometry] Failed to parse GLB JSON: {}", e2);
                                return false;
                            }
                        }
                    }
                    Err(e2) => {
                        log::error!("[HarmonicGeometry] Failed to parse GLB: {} / {}", e, e2);
                        return false;
                    }
                }
            }
        };

        log::info!("[HarmonicGeometry] GLB has {} meshes, {} nodes, {} scenes",
            document.meshes().count(), document.nodes().count(), document.scenes().count());

        // Collect ALL vertices and indices from ALL meshes and primitives
        let mut all_positions: Vec<[f32; 3]> = Vec::new();
        let mut all_normals: Vec<[f32; 3]> = Vec::new();
        let mut all_uvs: Vec<[f32; 2]> = Vec::new();
        let mut all_indices: Vec<u32> = Vec::new();

        // Matrix multiplication for 4x4 matrices (column-major as glTF uses)
        fn mat4_multiply(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
            let mut result = [[0.0f32; 4]; 4];
            for i in 0..4 {
                for j in 0..4 {
                    result[i][j] = a[0][j] * b[i][0]
                               + a[1][j] * b[i][1]
                               + a[2][j] * b[i][2]
                               + a[3][j] * b[i][3];
                }
            }
            result
        }

        // Helper to apply 4x4 transform matrix to a point
        fn transform_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
            [
                m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
                m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
                m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
            ]
        }

        // Helper to apply 3x3 rotation (from 4x4 matrix) to normal
        fn transform_normal(m: &[[f32; 4]; 4], n: [f32; 3]) -> [f32; 3] {
            let result = [
                m[0][0] * n[0] + m[1][0] * n[1] + m[2][0] * n[2],
                m[0][1] * n[0] + m[1][1] * n[1] + m[2][1] * n[2],
                m[0][2] * n[0] + m[1][2] * n[1] + m[2][2] * n[2],
            ];
            // Normalize
            let len = (result[0]*result[0] + result[1]*result[1] + result[2]*result[2]).sqrt();
            if len > 0.0001 {
                [result[0]/len, result[1]/len, result[2]/len]
            } else {
                [0.0, 1.0, 0.0]
            }
        }

        // Recursive function to process nodes with accumulated transforms
        fn process_node(
            node: gltf::Node,
            parent_transform: &[[f32; 4]; 4],
            buffers: &[gltf::buffer::Data],
            all_positions: &mut Vec<[f32; 3]>,
            all_normals: &mut Vec<[f32; 3]>,
            all_uvs: &mut Vec<[f32; 2]>,
            all_indices: &mut Vec<u32>,
        ) {
            // Accumulate this node's transform with parent
            let local_transform = node.transform().matrix();
            let world_transform = mat4_multiply(parent_transform, &local_transform);

            // If this node has a mesh, process it
            if let Some(mesh) = node.mesh() {
                for primitive in mesh.primitives() {
                    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

                    let positions: Vec<[f32; 3]> = match reader.read_positions() {
                        Some(iter) => iter.collect(),
                        None => continue,
                    };

                    let normals: Vec<[f32; 3]> = match reader.read_normals() {
                        Some(iter) => iter.collect(),
                        None => positions.iter().map(|_| [0.0, 1.0, 0.0]).collect(),
                    };

                    // Read UV coordinates (TEXCOORD_0)
                    let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                        Some(iter) => iter.into_f32().collect(),
                        None => positions.iter().map(|_| [0.0, 0.0]).collect(),
                    };

                    let indices: Vec<u32> = match reader.read_indices() {
                        Some(iter) => iter.into_u32().collect(),
                        None => (0..positions.len() as u32).collect(),
                    };

                    // Track base index for this primitive
                    let base_idx = all_positions.len() as u32;

                    // Transform and add positions with world transform
                    for pos in &positions {
                        all_positions.push(transform_point(&world_transform, *pos));
                    }

                    // Transform and add normals
                    for norm in &normals {
                        all_normals.push(transform_normal(&world_transform, *norm));
                    }

                    // Add UVs (no transform needed)
                    for uv in &uvs {
                        all_uvs.push(*uv);
                    }

                    // Add indices with offset
                    for idx in &indices {
                        all_indices.push(base_idx + idx);
                    }

                    log::info!("[HarmonicGeometry] Loaded primitive: {} verts, {} indices, has UVs={} from node '{}'",
                        positions.len(), indices.len(), !uvs.is_empty() && uvs[0] != [0.0, 0.0],
                        node.name().unwrap_or("unnamed"));
                }
            }

            // Recursively process children
            for child in node.children() {
                process_node(child, &world_transform, buffers, all_positions, all_normals, all_uvs, all_indices);
            }
        }

        // Identity matrix as root transform
        let identity: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];

        // Process scene hierarchy starting from root nodes
        if let Some(scene) = document.default_scene().or_else(|| document.scenes().next()) {
            for node in scene.nodes() {
                process_node(node, &identity, &buffers, &mut all_positions, &mut all_normals, &mut all_uvs, &mut all_indices);
            }
        } else {
            // Fallback: process all nodes if no scene is defined
            for node in document.nodes() {
                // Only process root nodes (those without parents would be processed multiple times otherwise)
                // Since we can't easily check for parents, just process all and let transforms accumulate
                process_node(node, &identity, &buffers, &mut all_positions, &mut all_normals, &mut all_uvs, &mut all_indices);
            }
        }

        if all_positions.is_empty() {
            log::error!("[HarmonicGeometry] No geometry found in GLB");
            return false;
        }

        // Calculate bounding box for auto-centering and scaling
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for pos in &all_positions {
            for i in 0..3 {
                min[i] = min[i].min(pos[i]);
                max[i] = max[i].max(pos[i]);
            }
        }
        let center = [
            (min[0] + max[0]) / 2.0,
            (min[1] + max[1]) / 2.0,
            (min[2] + max[2]) / 2.0,
        ];
        let size = [
            max[0] - min[0],
            max[1] - min[1],
            max[2] - min[2],
        ];
        let max_dim = size[0].max(size[1]).max(size[2]);

        log::info!("[HarmonicGeometry] GLB total bounds: min={:?}, max={:?}, size={:?}",
            min, max, size);

        // Normalize positions to fit in unit cube centered at origin
        let normalized_positions: Vec<[f32; 3]> = all_positions.iter().map(|pos| {
            [
                (pos[0] - center[0]) / max_dim,
                (pos[1] - center[1]) / max_dim,
                (pos[2] - center[2]) / max_dim,
            ]
        }).collect();

        log::info!("[HarmonicGeometry] GLB loaded: {} total vertices, {} total indices",
            normalized_positions.len(), all_indices.len());

        // Use all_normals, all_uvs, and all_indices for the rest
        let normals = all_normals;
        let uvs = all_uvs;
        let indices = all_indices;

        // Interleave position + normal + uv into single buffer (using normalized positions)
        // Layout: pos(3) + normal(3) + uv(2) = 8 floats per vertex
        let mut vertex_data: Vec<f32> = Vec::with_capacity(normalized_positions.len() * 8);
        for ((pos, norm), uv) in normalized_positions.iter().zip(normals.iter()).zip(uvs.iter()) {
            vertex_data.extend_from_slice(pos);
            vertex_data.extend_from_slice(norm);
            vertex_data.extend_from_slice(uv);
        }

        // Compile 3D mesh shader with texture sampling
        let mesh_vs = r#"#version 330 core
            layout(location = 0) in vec3 a_position;
            layout(location = 1) in vec3 a_normal;
            layout(location = 2) in vec2 a_uv;

            uniform mat4 u_mvp;
            uniform mat4 u_model;
            uniform float u_time;

            out vec3 v_normal;
            out vec3 v_world_pos;
            out vec2 v_uv;

            void main() {
                v_normal = mat3(u_model) * a_normal;
                v_world_pos = (u_model * vec4(a_position, 1.0)).xyz;
                v_uv = a_uv;
                gl_Position = u_mvp * vec4(a_position, 1.0);
            }
        "#;

        let mesh_fs = r#"#version 330 core
            in vec3 v_normal;
            in vec3 v_world_pos;
            in vec2 v_uv;

            uniform float u_time;
            uniform float u_alpha;
            uniform sampler2D u_texture;
            uniform int u_has_texture;

            out vec4 frag_color;

            void main() {
                vec3 n = normalize(v_normal);
                vec3 view_dir = normalize(-v_world_pos);

                // Sample texture for base color
                vec3 base_color;
                if (u_has_texture == 1) {
                    base_color = texture(u_texture, v_uv).rgb;
                } else {
                    // Fallback: gold color if no texture
                    base_color = vec3(0.85, 0.65, 0.35);
                }

                // Multi-directional lighting
                vec3 light1 = normalize(vec3(0.5, 1.0, 0.8));   // Main key light
                vec3 light2 = normalize(vec3(-0.8, 0.3, 0.5));  // Fill light
                vec3 light3 = normalize(vec3(0.0, -0.3, 1.0));  // Rim light

                float diff1 = max(dot(n, light1), 0.0);
                float diff2 = max(dot(n, light2), 0.0);
                float diff3 = max(dot(n, light3), 0.0);

                // Lighting variations
                vec3 shadow_color = base_color * 0.5;
                vec3 highlight = base_color * 1.3 + vec3(0.1);

                // Blend colors based on lighting angle
                float facing = dot(n, view_dir);
                vec3 lit_color = mix(shadow_color, base_color, diff1 * 0.6 + 0.4);
                lit_color = mix(lit_color, highlight, pow(max(diff1, 0.0), 2.0) * 0.3);

                // Specular highlights
                vec3 half_vec = normalize(light1 + view_dir);
                float spec = pow(max(dot(n, half_vec), 0.0), 48.0);
                vec3 spec_color = vec3(1.0) * spec * 0.4;

                // Combine lighting
                float lighting = diff1 * 0.5 + diff2 * 0.2 + diff3 * 0.1 + 0.3;
                vec3 color = lit_color * lighting + spec_color;

                // Fresnel rim
                float fresnel = 1.0 - max(facing, 0.0);
                fresnel = pow(fresnel, 3.0);
                color += base_color * fresnel * 0.2;

                frag_color = vec4(color, u_alpha);
            }
        "#;

        let program = match Self::compile_program(gl, mesh_vs, mesh_fs) {
            Ok(p) => p,
            Err(e) => {
                log::error!("[HarmonicGeometry] Failed to compile mesh shader: {}", e);
                return false;
            }
        };

        unsafe {
            // Create VAO for mesh
            let vao = match gl.create_vertex_array() {
                Ok(v) => v,
                Err(e) => {
                    log::error!("[HarmonicGeometry] Failed to create mesh VAO: {}", e);
                    return false;
                }
            };
            gl.bind_vertex_array(Some(vao));

            // Create VBO
            let vbo = match gl.create_buffer() {
                Ok(b) => b,
                Err(e) => {
                    log::error!("[HarmonicGeometry] Failed to create mesh VBO: {}", e);
                    return false;
                }
            };
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&vertex_data),
                glow::STATIC_DRAW,
            );

            // Create EBO
            let ebo = match gl.create_buffer() {
                Ok(b) => b,
                Err(e) => {
                    log::error!("[HarmonicGeometry] Failed to create mesh EBO: {}", e);
                    return false;
                }
            };
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(&indices),
                glow::STATIC_DRAW,
            );

            // Setup vertex attributes (position + normal + uv, stride = 8 floats)
            let stride = 8 * std::mem::size_of::<f32>() as i32;

            // Position (location 0)
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);

            // Normal (location 1)
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 3 * std::mem::size_of::<f32>() as i32);

            // UV (location 2) - 2 floats
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, 6 * std::mem::size_of::<f32>() as i32);

            gl.bind_vertex_array(None);

            self.glb_program = Some(program);
            self.glb_vao = Some(vao);
            self.glb_vbo = Some(vbo);
            self.glb_ebo = Some(ebo);
            self.glb_index_count = indices.len() as u32;

            // Load embedded texture from GLB if available
            if !images.is_empty() {
                let img_data = &images[0];
                log::info!("[HarmonicGeometry] Loading embedded texture: {}x{} {:?}",
                    img_data.width, img_data.height, img_data.format);

                // Create OpenGL texture
                match gl.create_texture() {
                    Ok(texture) => {
                        gl.bind_texture(glow::TEXTURE_2D, Some(texture));

                        // Determine format based on image format
                        let (internal_format, format, pixel_data) = match img_data.format {
                            gltf::image::Format::R8G8B8 => {
                                (glow::RGB8 as i32, glow::RGB, &img_data.pixels[..])
                            }
                            gltf::image::Format::R8G8B8A8 => {
                                (glow::RGBA8 as i32, glow::RGBA, &img_data.pixels[..])
                            }
                            _ => {
                                log::warn!("[HarmonicGeometry] Unsupported texture format: {:?}", img_data.format);
                                (glow::RGBA8 as i32, glow::RGBA, &img_data.pixels[..])
                            }
                        };

                        gl.tex_image_2d(
                            glow::TEXTURE_2D,
                            0,
                            internal_format,
                            img_data.width as i32,
                            img_data.height as i32,
                            0,
                            format,
                            glow::UNSIGNED_BYTE,
                            Some(pixel_data),
                        );

                        // Set texture parameters
                        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
                        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
                        gl.generate_mipmap(glow::TEXTURE_2D);

                        gl.bind_texture(glow::TEXTURE_2D, None);

                        self.glb_texture = Some(texture);
                        log::info!("[HarmonicGeometry] Embedded texture loaded successfully");
                    }
                    Err(e) => {
                        log::error!("[HarmonicGeometry] Failed to create texture: {}", e);
                    }
                }
            } else {
                log::info!("[HarmonicGeometry] No embedded texture in GLB");
            }

            self.glb_loaded = true;
            log::info!("[HarmonicGeometry] GLB mesh loaded and ready for rendering");
        }
        true
    }

    /// Set shader uniforms
    fn set_uniforms(&self, gl: &glow::Context, program: glow::Program) {
        unsafe {
            gl.use_program(Some(program));

            // Helper closures for setting uniforms
            let set_f32 = |name: &str, value: f32| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_1_f32(Some(&loc), value);
                }
            };

            let set_i32 = |name: &str, value: i32| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_1_i32(Some(&loc), value);
                }
            };

            let set_vec2 = |name: &str, x: f32, y: f32| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_2_f32(Some(&loc), x, y);
                }
            };

            let set_vec4 = |name: &str, x: f32, y: f32, z: f32, w: f32| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_4_f32(Some(&loc), x, y, z, w);
                }
            };

            // Time and resolution
            set_f32("u_time", self.time);
            set_vec2("u_resolution", self.width as f32, self.height as f32);

            // Chroma packed into 3 vec4s for GPU efficiency
            // Uses FAST-smoothed display_chroma for responsive UI/HUD
            // (harmonic_field uses SLOW chroma for stable geometry)
            let chroma = &self.display_chroma;
            set_vec4("u_chroma_0_3", chroma[0], chroma[1], chroma[2], chroma[3]);
            set_vec4("u_chroma_4_7", chroma[4], chroma[5], chroma[6], chroma[7]);
            set_vec4("u_chroma_8_11", chroma[8], chroma[9], chroma[10], chroma[11]);

            // Precomputed chroma mean (saves per-pixel computation)
            let chroma_mean: f32 = chroma.iter().sum::<f32>() / 12.0;
            set_f32("u_chroma_mean", chroma_mean);

            // Harmonic field parameters
            set_f32("u_base_radius", self.hero_config.base_radius);
            set_f32("u_gain", self.hero_config.gain);

            // Thickness from tonal energy
            let thickness = (0.3 + self.tonal_energy * self.hero_config.thickness_multiplier)
                .clamp(0.1, 3.0);
            set_f32("u_thickness", thickness);

            set_f32("u_tonal_confidence", self.tonal_confidence);

            // Render and symmetry modes
            set_i32("u_render_mode", self.hero_config.render_mode.as_i32());
            set_i32("u_symmetry_mode", self.hero_config.symmetry_mode.as_i32());

            // Quality tier system (0=Low, 1=Med, 2=High)
            set_i32("u_quality_tier", self.quality_tier as i32);
            set_f32("u_render_scale", self.render_scale);

            // License watermark (driven as shader uniform, not a boolean check)
            set_f32("u_watermark", self.watermark_value);

            // BG quality budget system
            set_i32("u_bg_quality", self.bg_quality as i32);
            set_f32("u_bg_budget", 1.0);  // Future: could be slider
            set_f32("u_qs_enabled", if self.camera_state.qs_wheel_visible { 1.0 } else { 0.0 });

            // TAA jitter for temporal anti-aliasing
            let (jitter_x, jitter_y) = if self.taa_enabled {
                self.get_taa_jitter()
            } else {
                (0.0, 0.0)
            };
            set_vec2("u_taa_jitter", jitter_x, jitter_y);

            // Rotation (slow, continuous) + Fold24 witness beat aim offset + polarity yaw nudge
            let fold24_aim_offset = self.camera_state.fold24.aim_offset_angle;
            let polarity_yaw_nudge = self.camera_state.polarity_yaw_nudge;
            set_f32("u_rotation", self.rotation + fold24_aim_offset + polarity_yaw_nudge);

            // Independent field rotation (audio-reactive spiral drift)
            // Decoupled from base rotation - field spins while QS wheel stays fixed
            set_f32("u_field_rotation", self.field_rotation);

            // Camera - base values from hero config
            set_f32("u_cam_dist", self.hero_config.visual.camera_distance);
            set_f32("u_cam_height", self.hero_config.visual.camera_height);
            set_f32("u_cam_angle", self.hero_config.visual.camera_angle);

            // Dynamic camera parameters (blended Observer/Immersive/Performer)
            // Convert FOV from degrees to shader factor (1.0 ≈ 45°)
            let fov_factor = self.camera_state.fov / 45.0;
            set_f32("u_cam_fov", fov_factor);
            // Use effective orbit speed (base + beat-quantized impulse)
            // GATED BY ENERGY: Stop orbit when silent
            // Polarity orbit sign: flips orbit direction on quadrant changes
            let orbit_gate = (self.energy_vis - 0.02).max(0.0).min(0.13) / 0.13;
            let polarity_orbit_sign = self.camera_state.polarity_orbit_sign;
            set_f32("u_cam_orbit_speed", self.camera_state.effective_orbit_speed() * orbit_gate * polarity_orbit_sign);
            set_f32("u_cam_tilt", self.camera_state.tilt_angle);
            set_f32("u_cam_noise_rot", self.camera_state.noise_rot);
            set_f32("u_cam_noise_pos", self.camera_state.noise_pos);
            set_f32("u_cam_inside_dip", self.camera_state.inside_dip);
            set_vec2("u_cam_aim_offset", self.camera_state.aim_offset[0], self.camera_state.aim_offset[1]);
            set_f32("u_cam_dyn_dist", self.camera_state.distance);  // Dynamic distance override
            // Dynamic rim boost when camera is close (prevents flat blob)
            set_f32("u_cam_rim_boost", self.camera_state.rim_boost);

            // === JOURNEY PHASE UNIFORMS ===
            set_f32("u_journey_phase", self.camera_state.phase.as_f32());
            set_f32("u_journey_phase_time", self.camera_state.phase_time);
            set_f32("u_journey_phase_blend", self.camera_state.phase_blend);

            // Journey-driven visual state
            set_f32("u_field_density", self.camera_state.field_density);
            set_f32("u_surface_opacity", self.camera_state.surface_opacity);
            set_f32("u_interior_enable", self.camera_state.interior_enable);
            set_f32("u_skin_intensity_journey", self.camera_state.skin_intensity);
            set_f32("u_video_visibility", self.camera_state.video_visibility);

            // INSIDE_MIX: Smooth visual blend weight [0..1] for surface/interior crossfade
            // Derived from ring_dive.t when RingDive active, or interior_state_time otherwise
            // This is the VISUAL driver - no pops!
            set_f32("u_inside_mix", self.camera_state.inside_mix);

            // INTERIOR_LATCH: PERMISSION bit for expensive inside-only passes
            // 1 when interior_state.is_inside() (Entering or Inside)
            // This is for PERFORMANCE gating, NOT visual blending
            let interior_perm = self.camera_state.interior_state.is_inside();
            set_f32("u_interior_latch", if interior_perm { 1.0 } else { 0.0 });

            // INTERIOR_HARD: Stricter permission - only 1 when truly Inside (not Entering)
            // Use for the most expensive features that should only run when fully inside
            let interior_hard = matches!(self.camera_state.interior_state, InteriorState::Inside);
            set_i32("u_interior_hard", if interior_hard { 1 } else { 0 });

            // Compel narrative state
            set_f32("u_compel_exterior_bind", self.camera_state.compel_exterior_bind);
            set_f32("u_compel_interior_reveal", self.camera_state.compel_interior_reveal);
            set_f32("u_compel_emissive_persist", self.camera_state.compel_emissive_persist);
            set_f32("u_compel_flicker_gate", self.camera_state.compel_flicker_gate);

            // Visual settings
            set_f32("u_glow", self.hero_config.visual.glow);
            set_f32("u_fog_density", self.hero_config.visual.fog_density);
            set_f32("u_exposure", self.hero_config.visual.exposure);

            // Audio-reactive uniforms (for post-processing effects, NOT geometry)
            set_f32("u_energy", self.energy_vis);
            set_f32("u_kick", self.kick);
            set_f32("u_stability", self.stability);
            set_i32("u_dom_pc", self.dominant_pc as i32);
            // Section energy (slow RMS ~3s) for macro story / mode selection
            set_f32("u_section_energy", self.camera_state.section_energy);

            // Spectral band uniforms (for spectral rings)
            set_f32("u_bass", self.bass);
            set_f32("u_mid", self.mid);
            set_f32("u_high", self.high);
            set_f32("u_spectral_rings", 1.0);  // Always enabled for now

            // AAA Ring uniforms - slow EMA for smooth musical motion
            set_f32("u_bass_ema", self.bass_ema);
            set_f32("u_mid_ema", self.mid_ema);
            set_f32("u_high_ema", self.high_ema);
            // Peak frequencies per band (Hz)
            set_f32("u_bass_peak_hz", self.bass_peak_hz);
            set_f32("u_mid_peak_hz", self.mid_peak_hz);
            set_f32("u_high_peak_hz", self.high_peak_hz);
            // AAA Ring style flags (packed as floats for shader)
            set_f32("u_ring_style_aaa", if self.ring_style_aaa { 1.0 } else { 0.0 });
            set_f32("u_ring_flow_lines", if self.ring_flow_lines { 1.0 } else { 0.0 });
            set_f32("u_ring_sparkle", if self.ring_sparkle { 1.0 } else { 0.0 });
            set_f32("u_ring_fog", if self.ring_fog { 1.0 } else { 0.0 });
            set_f32("u_ring_bloom_mask", if self.ring_bloom_mask { 1.0 } else { 0.0 });

            // Live band centroid frequencies (Hz) for frequency labels
            // 6 bands: Sub, Kick, Low-mid, Mid, High-mid, Air
            // These are LIVE spectral centroids from FFT analysis, not static values!
            for i in 0..6 {
                let name = format!("u_band_freq[{}]", i);
                if let Some(loc) = gl.get_uniform_location(program, &name) {
                    gl.uniform_1_f32(Some(&loc), self.band_centroids[i]);
                }
            }
            set_f32("u_freq_labels", 1.0);  // Always enabled for now

            // Hyperspace warp effect (for entry/exit transitions)
            set_f32("u_warp_intensity", self.camera_state.warp_intensity);

            // === RING DIVE UNIFORMS ===
            // Cinematic inside transition sequence
            set_i32("u_ring_dive_active", if self.camera_state.ring_dive.active { 1 } else { 0 });
            set_f32("u_ring_dive_t", self.camera_state.ring_dive.t);
            set_i32("u_ring_dive_quality", self.camera_state.ring_dive.quality.as_int());
            set_i32("u_ring_dive_target_band", self.camera_state.ring_dive.target_band as i32);
            set_f32("u_ring_dive_portal_depth", self.camera_state.ring_dive.config.portal_depth);
            set_f32("u_ring_dive_warp_strength", self.camera_state.ring_dive.config.warp_strength);
            set_f32("u_ring_dive_alpha_budget", self.camera_state.ring_dive.config.interior_alpha_budget);
            // Pass tension for shader use
            // Use fast tension EMA for visual effects
            set_f32("u_tension", self.camera_state.tension_fast);

            // === PORTAL FPS UNIFORMS ===
            // Rail shooter mode during portal dive
            let fps = &self.camera_state.portal_fps;
            set_i32("u_portal_fps_active", if fps.active { 1 } else { 0 });
            set_i32("u_portal_fps_path_id", fps.path_id as i32);
            set_i32("u_portal_fps_lane_idx", fps.lane_idx as i32);
            set_i32("u_portal_fps_segment_id", fps.segment_id as i32);
            set_f32("u_portal_fps_forward_phase", fps.forward_phase);
            set_f32("u_portal_fps_lane_t", fps.lane_transition_t);
            set_i32("u_portal_fps_speed_tier", fps.speed_tier as i32);
            set_i32("u_portal_fps_density_tier", fps.density_tier as i32);
            set_f32("u_portal_fps_sway_phase", fps.sway_phase);
            set_f32("u_portal_fps_scroll", fps.line_scroll_phase);
            set_i32("u_portal_fps_nm_count", fps.near_miss_active_count as i32);
            // Portal FPS frame vectors
            set_f32("u_portal_fps_fwd_x", fps.frame_forward_x);
            set_f32("u_portal_fps_fwd_z", fps.frame_forward_z);
            set_f32("u_portal_fps_right_x", fps.frame_right_x);
            set_f32("u_portal_fps_right_z", fps.frame_right_z);
            // Portal FPS "make it hit" accents
            set_f32("u_portal_fps_lane_impulse", fps.lane_impulse);
            set_i32("u_portal_fps_lane_impulse_dir", fps.lane_impulse_dir as i32);
            set_f32("u_portal_fps_vista_pulse", fps.vista_pulse);
            set_f32("u_portal_fps_hit_emphasis", fps.hit_emphasis);
            set_i32("u_portal_fps_corridor_tier", fps.corridor_tier as i32);
            set_f32("u_portal_fps_scaffold_emph", fps.scaffold_emphasis);
            set_f32("u_portal_fps_rail_pulse_0", fps.rail_group_pulse[0]);
            set_f32("u_portal_fps_rail_pulse_1", fps.rail_group_pulse[1]);
            set_f32("u_portal_fps_rail_pulse_2", fps.rail_group_pulse[2]);
            set_f32("u_portal_entry_aperture", fps.entry_aperture);
            set_f32("u_portal_z_offset", fps.z_offset);

            // === FOLD24 UNIFORMS ===
            // Secondary inside transition (12-fold → 24-fold)
            set_i32("u_inside_mode", self.camera_state.fold24.inside_mode());
            set_f32("u_fold24_t", self.camera_state.fold24.t);

            // FOLD24_MIX: Smooth blend weight for 12/24-fold scaffold crossfade
            // Deterministic from RingDive timeline or latched mode outside RingDive
            set_f32("u_fold24_mix", self.camera_state.fold24_mix);
            set_f32("u_fold12_mix", 1.0 - self.camera_state.fold24_mix);

            // === TORUS KNOT CYMATICS UNIFORMS ===
            // Tertiary inside transition (24-fold → torus knot)
            let tk = &self.camera_state.torus_knot;
            set_i32("u_torus_active", if tk.active || tk.t > 0.01 { 1 } else { 0 });
            set_f32("u_torus_t", tk.t);
            set_i32("u_torus_subphase", tk.subphase.as_i32());
            set_f32("u_torus_p", tk.p_smooth);
            set_f32("u_torus_q", tk.q_smooth);
            set_f32("u_torus_phase", tk.phase_advance + tk.seed_phase);
            set_i32("u_torus_quality", tk.quality.as_i32());
            set_f32("u_torus_ref_hz", tk.ref_hz);
            set_f32("u_torus_live_hz", tk.live_hz);
            set_f32("u_torus_stability", tk.stability);
            set_f32("u_torus_golden_blend", tk.golden_blend);
            set_f32("u_torus_video_fade", tk.video_fade);
            set_f32("u_torus_support", tk.support);
            set_f32("u_torus_seed_phase", tk.seed_phase);
            set_f32("u_torus_major_r", tk.config.major_radius);
            set_f32("u_torus_minor_r", tk.config.minor_radius);
            set_f32("u_torus_line_thick", tk.config.line_thickness);
            set_f32("u_torus_node_radius", tk.config.node_radius);
            // Enhanced visual uniforms
            set_f32("u_torus_tube_radius", tk.config.tube_radius);
            set_f32("u_torus_rim_intensity", tk.config.rim_intensity);
            set_f32("u_torus_spec_power", tk.config.spec_power);
            set_f32("u_torus_spec_intensity", tk.config.spec_intensity);
            set_f32("u_torus_depth_haze", tk.config.depth_haze);
            set_f32("u_torus_bloom_thresh", tk.config.bloom_thresh);
            set_f32("u_torus_particle_rate", tk.config.particle_rate);
            set_f32("u_torus_turbulence", self.camera_state.tension_slow); // Tension drives turbulence
            set_f32("u_torus_cam_orbit", tk.config.cam_orbit);
            set_f32("u_torus_cam_dist", tk.config.cam_dist);
            set_i32("u_torus_debug_flags", tk.config.debug_flags as i32);
            // Pro-level vortex visual uniforms
            set_f32("u_torus_chromatic", tk.config.chromatic_intensity);
            set_f32("u_torus_sss_intensity", tk.config.sss_intensity);

            // === PREMIUM 3D QUANTIZED STATE UNIFORMS ===
            set_i32("u_torus_orbit_step", tk.camera_orbit_step);
            set_f32("u_torus_tube_r_latched", tk.config.tube_radius_levels[tk.tube_radius_step as usize]);
            set_i32("u_torus_vortex_n", tk.vortex_fold_n);
            set_f32("u_torus_vortex_strength", tk.vortex_strength_latched);
            set_f32("u_torus_rim_gain", tk.rim_gain_latched);
            set_f32("u_torus_depth_sep", tk.depth_sep_latched);
            set_f32("u_torus_spec_boost", tk.spec_boost_latched);

            // === CANVAS/PAINT SYSTEM UNIFORMS ===
            // Quantized cymatic topology system
            set_i32("u_torus_canvas_id", tk.canvas.as_i32());
            set_i32("u_torus_paint_id", tk.paint.as_i32());
            set_i32("u_torus_link_count", tk.link_count);
            set_i32("u_torus_node_freq", tk.bead_count);  // Renamed: now bead_count (density via bucketed system)
            set_i32("u_torus_paint_phase", tk.paint_phase_step);
            set_i32("u_torus_paint_is_tet", if tk.paint.is_tet() { 1 } else { 0 });
            // WI computed from actual p,q (not from canvas preset)
            set_f32("u_torus_winding_interval",
                (tk.p as f32 * 360.0) / tk.q.max(1) as f32);

            // === VORTEX TOPOLOGY UNIFORMS ===
            // Spiral flow topology - sibling to TorusKnot
            let vortex = &self.camera_state.vortex;
            set_i32("u_vortex_active", if vortex.active || vortex.t > 0.01 { 1 } else { 0 });
            set_i32("u_vortex_gate_center", vortex.gate_center as i32);
            set_i32("u_vortex_circulation_step", vortex.circulation_step as i32);
            set_i32("u_vortex_direction", vortex.direction.as_i32());
            set_i32("u_vortex_shell_set_id", vortex.shell_set_id as i32);
            set_i32("u_vortex_pitch_q", vortex.pitch_q as i32);
            set_f32("u_vortex_pressure_env", vortex.pressure_env);
            set_f32("u_vortex_t", vortex.t);

            // === 3D VORTEX / DRAIN UNIFORMS ===
            // Sink radius presets (must match mod.rs SINK_RADIUS_PRESETS)
            const SINK_RADIUS_PRESETS: [f32; 5] = [0.15, 0.25, 0.35, 0.45, 0.55];
            let sink_radius = SINK_RADIUS_PRESETS[vortex.vortex_sink_radius_q.min(4) as usize];

            set_f32("u_vortex_depth", vortex.vortex_depth);
            set_f32("u_vortex_sink_strength", vortex.vortex_sink_strength);
            set_f32("u_vortex_sink_radius", sink_radius);
            set_i32("u_vortex_bulge_phase_q", vortex.vortex_bulge_phase_q as i32);
            set_f32("u_vortex_occlusion", vortex.vortex_occlusion);
            set_f32("u_vortex_sink_env", vortex.sink_env);
            set_f32("u_vortex_qs_drain_pull", vortex.qs_drain_pull);

            // === GRANT TRIANGLE SHELL RADII ===
            // Nine Generative Means from Pythagorean triple, normalized [0..1]
            set_i32("u_grant_triangle_id", vortex.grant_triangle_id as i32);
            set_i32("u_grant_shell_count", vortex.grant_shell_count as i32);
            // Set shell radii array
            for (i, &radius) in vortex.grant_shell_radii.iter().enumerate() {
                let name = format!("u_grant_shell_radii[{}]", i);
                set_f32(&name, radius);
            }

            // === DODECAHEDRAL RESONANCE UNIFORMS ===
            // Crystalline harmonic arrival - 12 pentagonal faces (24-fold alignment)
            let dodeca = &self.camera_state.dodecahedral;
            set_i32("u_dodeca_active", if dodeca.active || dodeca.t > 0.01 { 1 } else { 0 });
            set_f32("u_dodeca_t", dodeca.t);
            // Face activation array
            for (i, &activation) in dodeca.face_activation.iter().enumerate() {
                let name = format!("u_dodeca_face_activation[{}]", i);
                set_f32(&name, activation);
            }
            // Face depth array
            for (i, &depth) in dodeca.face_depths.iter().enumerate() {
                let name = format!("u_dodeca_face_depths[{}]", i);
                set_f32(&name, depth);
            }
            set_i32("u_dodeca_dominant_face", dodeca.dominant_face as i32);
            set_f32("u_dodeca_vertex_breath", dodeca.vertex_breath_phase);
            set_f32("u_dodeca_vertex_amp", dodeca.vertex_breath_amp);
            set_f32("u_dodeca_edge_pulse", dodeca.edge_pulse_env);
            set_f32("u_dodeca_edge_glow", dodeca.edge_glow);
            set_f32("u_dodeca_rotation", dodeca.rotation_angle);
            set_f32("u_dodeca_face_lock", dodeca.face_lock_strength);
            set_f32("u_dodeca_phi", dodeca.phi_resonance);
            set_i32("u_dodeca_gate", dodeca.inherited_gate as i32);

            // === GATE CONFIDENCE UNIFORM (master kill switch) ===
            // When u_gate_ok == 0, all gate-dependent features show uniform seam weighting
            set_f32("u_gate_ok", self.camera_state.gate_ok);

            // === OUTER BACKDROP UNIFORMS ===
            // Premium cathedral background: aperture + rim halo
            set_i32("u_outer_style_id", self.camera_state.outer_style_id as i32);
            set_i32("u_outer_aperture_q", self.camera_state.outer_aperture_q as i32);
            set_f32("u_outer_pulse_env", self.camera_state.outer_pulse_env);
            set_f32("u_outer_pulse_slow", self.camera_state.outer_pulse_slow);
            set_f32("u_outer_gate_fade", self.camera_state.outer_gate_fade);
            set_i32("u_outer_is_torusknot", if self.camera_state.outer_is_torusknot { 1 } else { 0 });
            set_i32("u_outer_is_vortex", if self.camera_state.outer_is_vortex { 1 } else { 0 });

            // === TOPOLOGY CROSSFADE UNIFORMS ===
            // Bar-edge deterministic routing between 24-fold topologies
            let router = &self.camera_state.topology_router;
            set_i32("u_topology_id", router.topology_id.as_i32());
            set_i32("u_topology_prev_id", router.topology_prev_id.as_i32());
            set_f32("u_topology_xfade_t", router.topology_xfade_t);

            // === REG-LOCKED CAMERA UNIFORMS ===
            // Quantized camera motion that rides the symmetry grid
            let cam_reg = &self.camera_state.camera_reg;

            // SAFETY: target_azimuth must equal anchor_k × (TAU/N) exactly.
            // If this fires, someone routed reg_degree_a into camera rotation.
            // N is keyed off prev_camera_fold24_active (the camera's authoritative domain),
            // NOT fold24.engaged which can change before the camera domain remaps.
            {
                let n = if self.camera_state.prev_camera_fold24_active { 24u8 } else { 12u8 };
                let az_step = std::f32::consts::TAU / n as f32;
                let expected = cam_reg.anchor_k as f32 * az_step;
                debug_assert!(
                    (cam_reg.target_azimuth - expected).abs() < 1e-3,
                    "INVARIANT: Render azimuth must use anchor_theta only, never reg_degree_a. \
                    azimuth={}, expected=anchor_k({})×(TAU/{}), az_step={}",
                    cam_reg.target_azimuth, cam_reg.anchor_k, n, az_step
                );
            }

            set_i32("u_cam_rig", cam_reg.rig.as_i32());
            set_f32("u_cam_reg_azimuth", cam_reg.target_azimuth);
            set_f32("u_cam_reg_elevation", cam_reg.target_elevation);
            set_f32("u_cam_reg_radius", cam_reg.target_radius);
            set_f32("u_cam_reg_fov", cam_reg.target_fov);
            set_i32("u_cam_in_portal", if cam_reg.rig == CameraRig::PortalDive { 1 } else { 0 });
            set_f32("u_cam_portal_phase", cam_reg.portal_phase);
            set_i32("u_cam_portal_anchor_k", cam_reg.portal_anchor_k_locked as i32);

            // === EMERGENCE STATE UNIFORMS ===
            // For candidate preview cue (cheap visual hint before full activation)
            set_i32("u_emerge_state", tk.emerge_state.as_i32());
            set_f32("u_emerge_candidate", if matches!(tk.emerge_state, crate::render::harmonic_geometry::EmergenceState::Candidate) { 1.0 } else { 0.0 });
            set_f32("u_emerge_prewarm", tk.prewarm_progress);

            // === WARP JUMP QS UNIFORMS ===
            // QS wheel IS the portal. Tracers outside, portal inside.
            let warp = &self.camera_state.hyperspace;  // Using alias (HyperspaceState = WarpJump)
            set_i32("u_warp_active", if warp.active { 1 } else { 0 });
            set_i32("u_warp_phase", warp.phase.as_i32());
            set_f32("u_warp_t", warp.t);
            set_f32("u_warp_phase_t", warp.phase_t);
            set_vec2("u_warp_portal_center", warp.portal_center[0], warp.portal_center[1]);
            set_f32("u_warp_portal_radius", warp.portal_radius);
            set_f32("u_warp_portal_spin", warp.portal_spin);
            set_f32("u_warp_zoom", warp.zoom);
            set_f32("u_warp_flash", warp.flash);
            set_f32("u_warp_ripple", warp.ripple);
            set_f32("u_warp_tracer_intensity", warp.tracer_intensity);
            set_f32("u_warp_camera_z", warp.camera_z_offset);
            set_f32("u_warp_strength", warp.warp_strength);
            set_f32("u_warp_qs_contrast", warp.qs_contrast);
            set_f32("u_warp_qs_thickness", warp.qs_thickness);
            set_f32("u_warp_qs_seam_glow", warp.qs_seam_glow);
            set_f32("u_warp_qs_scale", warp.qs_scale);
            set_f32("u_warp_chromatic", warp.chromatic_aberration);
            set_f32("u_warp_vortex", warp.vortex_strength);

            // === GLOBAL SILENCE UNIFORMS ===
            // When active, all layers fade to black except GLB
            let silence = &self.camera_state.silence;
            set_i32("u_silence_active", if silence.active { 1 } else { 0 });
            set_f32("u_silence_blackout", silence.blackout_alpha);
            set_f32("u_silence_glb_alpha", silence.glb_alpha);
            set_f32("u_silence_grace", silence.resume_grace_timer);

            // === PRESENTATION MODE UNIFORMS (Museum-Mode Polish) ===
            // Smooth crossfade between Idle (screensaver) and Live (performance) rendering
            let presentation = &self.camera_state.presentation;
            set_i32("u_presentation_mode", presentation.mode as i32);
            set_f32("u_idle_mix", presentation.idle_mix);
            set_i32("u_dom_chroma_latched", presentation.dominant_chroma_idx_latched);
            set_f32("u_activity_level", presentation.activity_level);

            // Unfold24 intensity with overlap easing at boundaries
            // unfold24_intensity = smoothstep(0.56,0.64,t) * (1-smoothstep(0.80,0.88,t))
            let ring_t = self.camera_state.ring_dive.t;
            let unfold_in = smoothstep(0.56, 0.64, ring_t);
            let unfold_out = 1.0 - smoothstep(0.80, 0.88, ring_t);
            let unfold24_intensity = if self.camera_state.ring_dive.active {
                unfold_in * unfold_out
            } else {
                0.0
            };
            set_f32("u_unfold24_intensity", unfold24_intensity);

            // === QUADRIPOLAR TRAVEL UNIFORMS (REG icositetragon journey) ===
            let quad = &self.camera_state.quadripolar;
            set_f32("u_quad_active", if quad.active { 1.0 } else { 0.0 });
            set_f32("u_quad_phase", quad.phase.as_f32());
            set_f32("u_quad_phase_t", quad.phase_t);
            set_f32("u_quad_total_t", quad.total_t);
            set_f32("u_quad_seam_basis", quad.seam_basis);        // 0=12-fold, 1=24-fold
            set_f32("u_quad_video_density", quad.video_density);  // Reduced during unfold
            set_f32("u_quad_spoke_flash", quad.spoke_flash);      // Brief spoke visibility
            set_f32("u_quad_wedge_intensity", quad.wedge_intensity); // 24-wedge segmentation
            set_f32("u_quad_energy_burst", quad.energy_burst);    // Visual emphasis
            set_f32("u_quad_camera_angle", quad.camera_angle_rad()); // Current camera angle

            // Pack quaternion ports into vec4 (radians)
            if let Some(loc) = gl.get_uniform_location(program, "u_quad_ports") {
                gl.uniform_4_f32(Some(&loc), quad.ports[0], quad.ports[1], quad.ports[2], quad.ports[3]);
            }

            // === QUATERNION CHARGES UNIFORMS (REG charge visualization) ===
            let qc = &self.camera_state.qc;
            set_i32("u_qc_active", if qc.active { 1 } else { 0 });
            set_f32("u_qc_w", qc.w);
            set_i32("u_qc_active_port", qc.active_port as i32);
            set_f32("u_qc_strength", qc.strength);

            // Pack QC ports into vec4 (radians)
            if let Some(loc) = gl.get_uniform_location(program, "u_qc_ports") {
                gl.uniform_4_f32(Some(&loc), qc.ports[0], qc.ports[1], qc.ports[2], qc.ports[3]);
            }

            // Pack QC charges into ivec4 (0=+1, 1=+i, 2=-1, 3=-i)
            if let Some(loc) = gl.get_uniform_location(program, "u_qc_charges") {
                gl.uniform_4_i32(
                    Some(&loc),
                    qc.charges[0].as_i32(),
                    qc.charges[1].as_i32(),
                    qc.charges[2].as_i32(),
                    qc.charges[3].as_i32(),
                );
            }

            // Pack QC invariant ok flags into vec4
            if let Some(loc) = gl.get_uniform_location(program, "u_qc_ok") {
                gl.uniform_4_f32(
                    Some(&loc),
                    if qc.invariants.complements_ok { 1.0 } else { 0.0 },
                    if qc.invariants.total_ok { 1.0 } else { 0.0 },
                    if qc.invariants.palindrome_ok { 1.0 } else { 0.0 },
                    if qc.invariants.lattice_ok_24 { 1.0 } else { 0.0 },
                );
            }

            // === POLARITY EFFECT UNIFORMS (flash + smoke on quadrant change) ===
            set_f32("u_polarity_effect_timer", qc.effect_timer);
            set_f32("u_polarity_effect_angle", qc.effect_angle);
            set_i32("u_polarity_quadrant", qc.current_quadrant as i32);
            set_i32("u_polarity_prev_quadrant", qc.prev_quadrant as i32);
            set_i32("u_polarity_direction", qc.polarity_direction as i32);
            set_i32("u_polarity_degree", qc.current_degree);

            // === QUATERNION SYMMETRY WHEEL AUDIO UNIFORMS ===
            // REG Figure 8: Compute active integer A from audio
            // Spoke from dominant pitch class (2 spokes per pitch class)
            // Ring from overall energy (low energy = inner, high = outer)
            // Cell integer formula: N = (24 - spoke) + ring * 24 (matches shader)
            {
                let chroma = self.harmonic_field.chroma_smooth();
                let dom_pc = self.dominant_pc as i32;

                // Spoke: dominant pitch class maps to 2 adjacent spokes
                // s = (dom_pc * 2) gives spoke 0,2,4...22
                // Use chroma energy to choose between the two spokes
                let s0 = (dom_pc * 2) % 24;
                let s1 = (dom_pc * 2 + 1) % 24;

                // Pick spoke based on which has more energy from neighboring bins
                let pc_energy = chroma[dom_pc as usize];
                let next_pc = ((dom_pc + 1) % 12) as usize;
                let neighbor_energy = chroma[next_pc];
                let spoke = if pc_energy >= neighbor_energy { s0 } else { s1 };

                // Ring: map energy to ring index (0-14)
                // Low energy = inner rings, high energy = outer rings
                let num_rings = 15;
                let ring = ((self.energy_vis * (num_rings as f32 - 1.0)).round() as i32)
                    .clamp(0, num_rings - 1);

                // Active integer: N = residue + ring * 24
                // REG Figure 8 layout: NORTH has residue 24, proceeding CW
                // residue = ((spoke + 23) % 24) + 1 gives: spoke 0 -> 24, spoke 1 -> 1, spoke 2 -> 2, etc.
                let residue = ((spoke + 23) % 24) + 1;
                let active_a = residue + ring * 24;

                // Compute quaternion set Q(A) = {A, 360-A, 180-A, 180+A}
                // Normalize to [1..360]: if result is 0, use 360; if > 360, wrap
                let norm360 = |x: i32| -> i32 {
                    let r = ((x % 360) + 360) % 360;
                    if r == 0 { 360 } else { r }
                };

                let q0 = active_a;                    // A
                let q1 = norm360(360 - active_a);     // 360 - A
                let q2 = norm360(180 - active_a);     // 180 - A
                let q3 = norm360(180 + active_a);     // 180 + A

                // Intensity from audio - higher base so highlights are always visible
                // Base 0.5, plus energy and chroma contributions
                let intensity = (0.5 + 0.3 * self.energy_vis + 0.2 * pc_energy).clamp(0.0, 1.0);

                // Set uniforms
                set_i32("u_qs_active_A", active_a);
                set_f32("u_qs_intensity", intensity);
                set_i32("u_qs_debug", 0);        // Debug overlay off by default
                set_i32("u_qs_debug_color", 0);  // Color debug off (audio modulation enabled)

                // Pack quartet into ivec4
                if let Some(loc) = gl.get_uniform_location(program, "u_qs_quartet") {
                    gl.uniform_4_i32(Some(&loc), q0, q1, q2, q3);
                }
            }

            // Symmetry HUD (radial chroma visualization) - toggle with 'H' key
            set_f32("u_waterfall_enable", if self.camera_state.hud_visible { 1.0 } else { 0.0 });
            // QS Wheel (Quaternion Symmetry Wheel) - separate toggle, hidden by default
            set_f32("u_qs_wheel_enable", if self.camera_state.qs_wheel_visible { 1.0 } else { 0.0 });
            set_f32("u_hud_fold", self.camera_state.fold24.t);  // 0 = 12-fold, 1 = 24-fold

            // === GEOMETRY STABILITY UNIFORMS ===
            // Trust factor gates geometry deformation (0 = circle, 1 = full lobes)
            // When low (ambiguous/unstable), geometry stays neutral to prevent flopping
            set_f32("u_trust", self.harmonic_field.trust());

            // === SKIN SYSTEM UNIFORMS ===
            set_i32("u_skin_mode", self.skin.mode.as_i32());
            set_f32("u_skin_strength", self.skin.strength);

            // Bool uniforms (GLSL 140 doesn't have native bool uniforms, use int)
            let set_bool = |name: &str, value: bool| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_1_i32(Some(&loc), if value { 1 } else { 0 });
                }
            };

            set_bool("u_skin_lock_dominant", self.skin.lock_to_dominant);
            set_bool("u_skin_hue_follow", self.skin.hue_follow_pitch);
            set_f32("u_skin_contrast", self.skin.surf_contrast);
            set_f32("u_skin_saturation", self.skin.surf_saturation);
            set_f32("u_skin_emissive", self.skin.emissive_strength);
            set_f32("u_skin_rim", self.skin.rim_strength);
            set_f32("u_skin_bloom", self.skin.bloom_amount);
            set_bool("u_skin_starburst", self.skin.starburst_enabled);
            set_f32("u_skin_edge", self.skin.edge_outline);
            set_f32("u_skin_sparkle", self.skin.specular_sparkle);

            // NEW: Albedo-first rendering uniforms
            set_f32("u_skin_surface_mix", self.skin.surface_mix);
            set_f32("u_skin_shadow_lift", self.skin.shadow_lift);
            set_f32("u_skin_ao_clamp", self.skin.ao_clamp);
            set_i32("u_skin_uv_mode", self.skin.uv_mode);

            // Skin texture binding - bind for any mode that uses texture (Surface, Emissive, or SurfaceEmissive)
            let has_texture = self.skin_texture_valid
                && self.skin_texture.is_some()
                && self.skin.mode != SkinMode::None;

            set_bool("u_skin_has_texture", has_texture);
            set_f32("u_video_aspect", self.video_aspect);

            if has_texture {
                if let Some(texture) = self.skin_texture {
                    // Bind skin texture to texture unit 0
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));

                    // Set sampler uniform
                    if let Some(loc) = gl.get_uniform_location(program, "u_skin_tex") {
                        gl.uniform_1_i32(Some(&loc), 0); // Texture unit 0
                    }
                }
            }

            // === CAMERA TEXTURE (composited on top of skin in QS wheel) ===
            let has_camera = self.camera_texture.is_some() && self.camera_alpha > 0.001;
            set_bool("u_camera_has_texture", has_camera);
            set_f32("u_camera_alpha", self.camera_alpha);

            if has_camera {
                if let Some(texture) = self.camera_texture {
                    // Bind camera texture to texture unit 3 (0=skin, 1=qs_numbers, 2=chroma_notes)
                    gl.active_texture(glow::TEXTURE3);
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));

                    if let Some(loc) = gl.get_uniform_location(program, "u_camera_tex") {
                        gl.uniform_1_i32(Some(&loc), 3); // Texture unit 3
                    }

                    // Camera aspect ratio for proper scaling
                    let cam_aspect = self.camera_texture_dims.0 as f32 / self.camera_texture_dims.1.max(1) as f32;
                    set_f32("u_camera_aspect", cam_aspect);
                }
            }

            // === QS NUMBERS ATLAS TEXTURE ===
            // Bind pre-rendered number atlas for QS wheel digit display
            let has_qs_numbers = self.qs_numbers_texture.is_some();
            set_bool("u_qs_numbers_has_texture", has_qs_numbers);
            set_i32("u_qs_hide_numbers", if self.camera_state.hide_qs_numbers { 1 } else { 0 });
            set_i32("u_qs_dominant_only", if self.camera_state.qs_dominant_only { 1 } else { 0 });

            if let Some(texture) = self.qs_numbers_texture {
                // Bind to texture unit 1 (unit 0 is skin texture)
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));

                if let Some(loc) = gl.get_uniform_location(program, "u_qs_numbers_tex") {
                    gl.uniform_1_i32(Some(&loc), 1); // Texture unit 1
                }
            }

            // === CHROMA NOTES ATLAS TEXTURE ===
            // Bind pre-rendered note names atlas for chroma wheel labels
            let has_chroma_notes = self.chroma_notes_texture.is_some();
            set_bool("u_chroma_notes_has_texture", has_chroma_notes);

            if let Some(texture) = self.chroma_notes_texture {
                // Bind to texture unit 2 (unit 0 = skin, unit 1 = qs numbers)
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));

                if let Some(loc) = gl.get_uniform_location(program, "u_chroma_notes_tex") {
                    gl.uniform_1_i32(Some(&loc), 2); // Texture unit 2
                }
            }

            // === INTERIOR RENDERING UNIFORMS ===
            // These control the volumetric interior when camera dips inside
            set_f32("u_interior_cell_size", self.camera_state.interior_cell_size);
            set_f32("u_interior_lattice_freq", self.camera_state.interior_lattice_freq);
            set_f32("u_interior_layers", self.camera_state.interior_layers);
            set_f32("u_interior_layer_step", self.camera_state.interior_layer_step);
            set_f32("u_interior_video_boost", self.camera_state.interior_video_boost);
            set_f32("u_interior_seam_strength", self.camera_state.interior_seam_strength);
            set_f32("u_interior_fold_iters", self.camera_state.interior_fold_iters);
            set_f32("u_interior_density_scale", self.camera_state.interior_density_scale);

            // === INSIDE 24-FOLD QUALITY SYSTEM ===
            // Dynamic quality tier with perf guardrails
            let iq = &self.camera_state.inside_quality;
            set_i32("u_inside_quality_tier", iq.quality.as_i32());
            set_i32("u_inside_shell_count", iq.quality.shell_count());
            set_i32("u_inside_accent_budget", iq.quality.accent_budget());
            set_i32("u_inside_label_density", iq.quality.label_density());
            set_f32("u_inside_fog_enabled", if iq.quality.fog_enabled() { 1.0 } else { 0.0 });
            set_f32("u_inside_sparkle_enabled", if iq.quality.sparkle_enabled() { 1.0 } else { 0.0 });
            set_f32("u_inside_perf_pressure", iq.perf_pressure);

            // Legacy interior_quality (for backwards compat with existing shader code)
            let interior_quality = match iq.quality {
                crate::render::harmonic_geometry::InsideQuality::Low => 0.35,
                crate::render::harmonic_geometry::InsideQuality::Med => 0.6,
                crate::render::harmonic_geometry::InsideQuality::High => 1.0,
            };
            set_f32("u_interior_quality", interior_quality);
        }
    }

    /// Matrix multiplication helper for 4x4 matrices (used for GLB rendering)
    fn mat4_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
        let mut result = [0.0_f32; 16];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    result[i * 4 + j] += a[i * 4 + k] * b[k * 4 + j];
                }
            }
        }
        result
    }

    /// Warmup shader by doing a single offscreen draw (PERF OPTIMIZATION)
    /// This forces driver shader compilation and warms the GPU pipeline,
    /// preventing the "freeze for a couple seconds" on first real render
    fn warmup_shader(&mut self, gl: &Arc<glow::Context>) {
        unsafe {
            if let (Some(program), Some(vao)) = (self.program, self.vao) {
                // Save current viewport
                let mut viewport = [0i32; 4];
                gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);

                // Set small viewport (1x1 is enough to trigger compilation)
                gl.viewport(0, 0, 1, 1);

                // Bind program and VAO
                gl.use_program(Some(program));
                gl.bind_vertex_array(Some(vao));

                // Set minimal uniforms needed for a valid draw
                // These prevent shader errors during warmup
                if let Some(loc) = gl.get_uniform_location(program, "u_resolution") {
                    gl.uniform_2_f32(Some(&loc), 1.0, 1.0);
                }
                if let Some(loc) = gl.get_uniform_location(program, "u_time") {
                    gl.uniform_1_f32(Some(&loc), 0.0);
                }
                if let Some(loc) = gl.get_uniform_location(program, "u_quality_tier") {
                    gl.uniform_1_i32(Some(&loc), 0); // Low quality for fast warmup
                }
                if let Some(loc) = gl.get_uniform_location(program, "u_base_radius") {
                    gl.uniform_1_f32(Some(&loc), 1.0);
                }
                // Set energy to 0 so it renders quickly (no complex path)
                if let Some(loc) = gl.get_uniform_location(program, "u_energy") {
                    gl.uniform_1_f32(Some(&loc), 0.0);
                }

                // Draw single triangle (triggers full shader compilation)
                gl.draw_arrays(glow::TRIANGLES, 0, 3);

                // Flush to ensure compilation completes
                gl.flush();

                // Restore viewport
                gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);

                // Unbind
                gl.bind_vertex_array(None);
                gl.use_program(None);

                log::info!("[HarmonicGeometry] Shader warmup complete");
            }
        }
    }

    /// Render only the silence-state GLB model (the REG logo) over whatever is
    /// already in the bound framebuffer — no scene render, no colour clear.
    ///
    /// Lets the pipeline give projectM the same GLB the Geometric Field shows
    /// when it falls silent. `silence_show` (0..1) drives the shrink/fade
    /// exactly like the in-renderer silence path: ≤0.08 hides it, 1.0 is full
    /// size. This mirrors the GLB draw block in `render()` so the look matches.
    pub fn render_silence_glb(&self, width: u32, height: u32, silence_show: f32) {
        if !self.glb_loaded || silence_show <= 0.08 {
            return;
        }
        let gl = match &self.gl {
            Some(gl) => gl.clone(),
            None => return,
        };
        let (program, vao) = match (self.glb_program, self.glb_vao) {
            (Some(p), Some(v)) => (p, v),
            _ => return,
        };

        unsafe {
            gl.use_program(Some(program));

            // Blend for transparency, depth for the interlocking frame structure
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::CULL_FACE);

            // Turntable spin + forward tilt to show depth
            let aspect = width as f32 / height as f32;
            let rotation_y = self.time * 0.15_f32;
            let tilt_x: f32 = 0.4;

            // Scale lerps with silence_show — shrinks as audio fades in (easeOutQuad),
            // min 5% to avoid popping
            let scale_t = silence_show * silence_show;
            let scale: f32 = 0.85 * scale_t.max(0.05);

            let cos_y = rotation_y.cos();
            let sin_y = rotation_y.sin();
            let cos_x = tilt_x.cos();
            let sin_x = tilt_x.sin();

            // Rotation matrix Rx * Ry with uniform scale, column-major
            let rot: [f32; 16] = [
                scale * cos_y,   scale * sin_x * sin_y,  scale * cos_x * sin_y,  0.0,
                0.0,             scale * cos_x,          scale * -sin_x,         0.0,
                scale * -sin_y,  scale * sin_x * cos_y,  scale * cos_x * cos_y,  0.0,
                0.0,             0.0,                    0.0,                    1.0,
            ];

            // Aspect correction: divide X column by aspect to keep proportions
            let ax = 1.0 / aspect;
            let model: [f32; 16] = [
                rot[0] * ax,  rot[1],  rot[2],  rot[3],
                rot[4] * ax,  rot[5],  rot[6],  rot[7],
                rot[8] * ax,  rot[9],  rot[10], rot[11],
                rot[12],      rot[13], rot[14], rot[15],
            ];
            let mvp = model; // Orthographic — model is the MVP

            if let Some(loc) = gl.get_uniform_location(program, "u_mvp") {
                gl.uniform_matrix_4_f32_slice(Some(&loc), false, &mvp);
            }
            if let Some(loc) = gl.get_uniform_location(program, "u_model") {
                gl.uniform_matrix_4_f32_slice(Some(&loc), false, &model);
            }
            if let Some(loc) = gl.get_uniform_location(program, "u_time") {
                gl.uniform_1_f32(Some(&loc), self.time);
            }
            if let Some(loc) = gl.get_uniform_location(program, "u_alpha") {
                // Fade synced with scale shrink, slightly faster so it's gone before min size
                let alpha = (silence_show * 1.2).min(1.0).powf(0.7) * 0.9;
                gl.uniform_1_f32(Some(&loc), alpha);
            }

            if let Some(texture) = self.glb_texture {
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                if let Some(loc) = gl.get_uniform_location(program, "u_texture") {
                    gl.uniform_1_i32(Some(&loc), 0);
                }
                if let Some(loc) = gl.get_uniform_location(program, "u_has_texture") {
                    gl.uniform_1_i32(Some(&loc), 1);
                }
            } else if let Some(loc) = gl.get_uniform_location(program, "u_has_texture") {
                gl.uniform_1_i32(Some(&loc), 0);
            }

            gl.bind_vertex_array(Some(vao));
            gl.draw_elements(glow::TRIANGLES, self.glb_index_count as i32, glow::UNSIGNED_INT, 0);

            // Restore state for the pipeline's subsequent passes
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }
}

// ============ ThemeRenderer Implementation ============

impl ThemeRenderer for HarmonicGeometryRenderer {
    fn renderer_type(&self) -> RendererType {
        RendererType::HarmonicGeometry
    }

    fn init(&mut self, gl: Arc<glow::Context>) -> Result<()> {
        log::info!("[HarmonicGeometry] init() called, already_initialized={}", self.initialized);
        if self.initialized {
            return Ok(());
        }

        log::info!("[HarmonicGeometry] Calling init_gl...");
        self.init_gl(&gl)?;
        self.gl = Some(gl.clone());
        self.initialized = true;

        // === SHADER WARMUP (PERF OPTIMIZATION) ===
        // Force shader compilation and GPU pipeline warmup with a dummy draw
        // This prevents the "freeze for a couple seconds" on first real render
        log::info!("[HarmonicGeometry] Running shader warmup...");
        self.warmup_shader(&gl);

        log::info!("[HarmonicGeometry] Renderer initialized successfully");
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn update(&mut self, audio: &AudioAnalysis, dt: f32) {
        self.time += dt;

        // Slow continuous rotation - GATED BY ENERGY
        // When silent (energy < 0.02), rotation stops completely
        // Smooth gate prevents abrupt start/stop
        let rotation_gate = (audio.energy - 0.02).max(0.0).min(0.13) / 0.13;
        self.rotation += dt * self.hero_config.rotation_speed * rotation_gate;

        // === AUDIO-REACTIVE FIELD ROTATION ===
        // Independent spiral drift for field effects while QS wheel stays fixed
        // Components: base_drift + energy_boost + kick_pulse
        // Direction: clockwise when stable, counter-clockwise when tense
        {
            // Base drift: slow constant rotation (~0.05 rad/s) - field is always alive
            let base_drift = 0.05;

            // Energy boost: faster spin at high energy (scaled 0-0.3 rad/s)
            let energy_boost = audio.energy * 0.3;

            // Kick pulse: brief acceleration on kicks (adds impulse)
            // kick is already smoothly decaying from 1.0 to 0.0
            let kick_pulse = self.kick * 0.5;

            // Direction: positive = clockwise (stable), negative = counter-clockwise (tense)
            // tension_slow is 0 = stable, 1 = tense
            // At tension=0: direction = +1, at tension=1: direction = -1
            let tension = self.camera_state.tension_slow;
            let direction = 1.0 - tension * 2.0;  // Maps 0->1, 1->-1

            // Combined field rotation speed
            let field_speed = (base_drift + energy_boost + kick_pulse) * direction;

            // Apply with energy gate (stop when silent)
            self.field_rotation += dt * field_speed * rotation_gate;
        }

        // Use tonal validity from analyzer (has proper gates)
        self.is_tonal = audio.is_tonal;
        self.tonal_confidence = audio.tonal_separation;
        self.tonal_energy = audio.tonal_energy;
        self.energy_vis = audio.energy_vis;
        self.stability = audio.stability;

        // Store fast-smoothed chroma for responsive UI (shader/HUD)
        self.display_chroma = audio.display_chroma;

        // Update key detector with chroma and delta time for dominant pitch latching
        // The key detector applies hysteresis to prevent jittery dominant switches
        // Note: beat_phase is not available in theme::AudioAnalysis, using 0.0
        // (dominant latching is time-based, not beat-based, so this is fine)
        self.key_detector.update_with_dt(
            &audio.chroma,
            0.0, // beat_phase not used for dominant latching
            Some(audio.energy),
            dt,
        );

        // Use HELD dominant from key_detector (latched, stable for UI/shaders)
        // Track previous for change detection (used by lighting OSC)
        self.prev_dominant_pc = self.dominant_pc;
        self.dominant_pc = self.key_detector.held_dom_pc() as usize;
        self.raw_dominant_pc = self.key_detector.raw_dom_pc() as usize;

        // Kick detection with decay for smooth visual effect
        // On kick: set to 1.0, then decay over ~100ms
        if audio.kick {
            self.kick = 1.0;
        } else {
            // Fast decay (about 10 frames at 60fps)
            self.kick = (self.kick - dt * 10.0).max(0.0);
        }
        // Carry the analyzer's continuous onset flux straight through (no decay) —
        // the tempo autocorrelator wants the raw per-frame onset, not the visual
        // kick pulse.
        self.onset_strength = audio.onset_strength;

        // Frequency bands for spectral rings (smooth with fast response)
        let band_alpha = 1.0 - (-dt * 15.0).exp();  // ~15 Hz response
        self.bass = self.bass + (audio.bass - self.bass) * band_alpha;
        self.mid = self.mid + (audio.mid - self.mid) * band_alpha;
        self.high = self.high + (audio.high - self.high) * band_alpha;

        // Raw energy for silence detection (no smoothing - instant response)
        // This is pre-normalization energy used by video SilenceGate and CueEngine
        self.raw_energy = audio.raw_bass + audio.raw_mid + audio.raw_high;
        self.raw_bass = audio.raw_bass;
        self.raw_high = audio.raw_high;

        // Slow EMA for AAA rings (τ≈300ms = ~3.3 Hz, smooth musical motion)
        // This gives smooth, non-jittery animation that feels "musical"
        let ema_alpha = 1.0 - (-dt * 3.3).exp();  // ~300ms time constant
        self.bass_ema = self.bass_ema + (audio.bass - self.bass_ema) * ema_alpha;
        self.mid_ema = self.mid_ema + (audio.mid - self.mid_ema) * ema_alpha;
        self.high_ema = self.high_ema + (audio.high - self.high_ema) * ema_alpha;

        // Peak frequency tracking per band (from band centroids, smoothed)
        // Sub+Kick → bass peak, Low-mid+Mid → mid peak, High-mid+Air → high peak
        let peak_alpha = 1.0 - (-dt * 5.0).exp();  // ~200ms smoothing
        let bass_peak_raw = (audio.band_centroids[0] + audio.band_centroids[1]) * 0.5;
        let mid_peak_raw = (audio.band_centroids[2] + audio.band_centroids[3]) * 0.5;
        let high_peak_raw = (audio.band_centroids[4] + audio.band_centroids[5]) * 0.5;
        self.bass_peak_hz = self.bass_peak_hz + (bass_peak_raw - self.bass_peak_hz) * peak_alpha;
        self.mid_peak_hz = self.mid_peak_hz + (mid_peak_raw - self.mid_peak_hz) * peak_alpha;
        self.high_peak_hz = self.high_peak_hz + (high_peak_raw - self.high_peak_hz) * peak_alpha;

        // Live band centroid frequencies (already smoothed in analyzer)
        self.band_centroids = audio.band_centroids;

        // === DYNAMIC CAMERA UPDATE ===
        // Use new unified update() with all parameters for:
        // - Inside-dip safety rails (needs dominance)
        // - Phase-locked aim offset (needs dominant_pc)
        // - Beat-quantized orbit (needs kick)
        // - Section energy for macro story
        // - Chroma for spectral flux tension
        self.camera_state.update(
            self.energy_vis,
            self.stability,
            self.kick,
            self.tonal_confidence,  // dominance
            self.dominant_pc,
            &audio.chroma,
            dt,
        );

        // Update harmonic field with new chroma data AND stability
        // This enables stability-adaptive smoothing and slew limiting
        self.harmonic_field.update_with_stability(
            &audio.chroma,
            self.tonal_confidence,
            self.tonal_energy,
            self.stability,  // Pass stability for adaptive smoothing
            dt,
        );

        // === GLB VISIBILITY SMOOTHING ===
        // Heavy easing so the GLB fades in/out gradually, not suddenly
        // Target: 1.0 when silent, 0.0 when audio is active
        let silence_threshold = 0.12;
        let target_visibility = (1.0 - (self.energy_vis / silence_threshold).min(1.0)).max(0.0);

        // Different time constants for fade in vs fade out
        // Fade out (audio starts): ~2 seconds to shrink away
        // Fade in (silence): ~3 seconds to grow back
        let tau = if target_visibility < self.glb_visibility {
            2.0  // Fading out - 2 second time constant
        } else {
            3.0  // Fading in - 3 second time constant
        };

        // Exponential smoothing toward target
        let alpha = 1.0 - (-dt / tau).exp();
        self.glb_visibility += (target_visibility - self.glb_visibility) * alpha;

        // Clamp to valid range
        self.glb_visibility = self.glb_visibility.clamp(0.0, 1.0);

        // === FRAME TIME TRACKING ===
        let now = std::time::Instant::now();
        let frame_ms = now.duration_since(self.last_frame_time).as_secs_f32() * 1000.0;
        self.last_frame_time = now;

        // EMA smoothing for frame time (avoid spikes)
        let ema_alpha = 0.1;  // 10% new, 90% old
        self.frame_time_ema_ms = self.frame_time_ema_ms * (1.0 - ema_alpha) + frame_ms * ema_alpha;

        // === BG QUALITY AUTO-SCALER ===
        // Adjusts bg_quality based on frame time EMA to maintain smooth performance
        if self.bg_quality_auto {
            if self.frame_time_ema_ms > 18.0 {
                // Running hot - track consecutive hot frames
                self.bg_frame_time_hot_streak += 1;
                self.bg_frame_time_cool_streak = 0;

                // After ~1 second of hot frames, drop BG quality
                if self.bg_frame_time_hot_streak > 60 {
                    if self.bg_quality > 0 {
                        self.bg_quality -= 1;
                        log::debug!("[HarmonicGeometry] BG quality auto-downgraded to {} (frame_ms={:.1})",
                            self.bg_quality, self.frame_time_ema_ms);
                    }
                    self.bg_frame_time_hot_streak = 0;
                }
            } else if self.frame_time_ema_ms < 14.0 {
                // Running cool - track consecutive cool frames
                self.bg_frame_time_cool_streak += 1;
                self.bg_frame_time_hot_streak = 0;

                // After ~3 seconds of cool frames, upgrade BG quality
                if self.bg_frame_time_cool_streak > 180 {
                    if self.bg_quality < 2 {
                        self.bg_quality += 1;
                        log::debug!("[HarmonicGeometry] BG quality auto-upgraded to {} (frame_ms={:.1})",
                            self.bg_quality, self.frame_time_ema_ms);
                    }
                    self.bg_frame_time_cool_streak = 0;
                }
            } else {
                // In target range - reset streaks
                self.bg_frame_time_hot_streak = 0;
                self.bg_frame_time_cool_streak = 0;
            }
        }

        // === INSIDE 24-FOLD QUALITY AUTO-SCALER ===
        // Only update when inside (interior_state is Inside or we're in 24-fold)
        let inside_active = self.camera_state.interior_state.is_inside()
            || self.camera_state.fold24.active;
        if inside_active {
            self.camera_state.inside_quality.update(self.frame_time_ema_ms, dt);
        }

        // === TORUS KNOT ADMISSION CONTROL ===
        // Pass frame time for admission budget check
        self.camera_state.torus_knot.admission_frame_ms = self.frame_time_ema_ms;
    }

    fn render(&mut self, width: u32, height: u32) {
        if !self.initialized {
            return;
        }

        let gl = match &self.gl {
            Some(gl) => gl.clone(),
            None => return,
        };

        self.width = width;
        self.height = height;

        // Initialize/resize TAA FBOs if needed
        if self.taa_enabled {
            self.init_taa_fbos(&gl, width, height);
        }

        // Determine if we're on first frame (no valid history)
        let is_first_frame = self.taa_frame_index == 0;

        unsafe {
            // === GLB VISIBILITY: Use global silence state for proper hysteresis ===
            let silence_show = self.camera_state.silence.glb_alpha.max(self.glb_visibility);

            // ==========================================================
            // PASS 1: Render main shader (with TAA jitter) to FBO or screen
            // ==========================================================
            if self.taa_enabled && self.taa_current_fbo.is_some() {
                // Render to TAA current FBO
                gl.bind_framebuffer(glow::FRAMEBUFFER, self.taa_current_fbo);
            }

            gl.viewport(0, 0, width as i32, height as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            // Render main SDF shader with jitter applied
            if let (Some(program), Some(vao)) = (self.program, self.vao) {
                gl.use_program(Some(program));
                self.set_uniforms(&gl, program);
                gl.bind_vertex_array(Some(vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }

            // ==========================================================
            // PASS 2: TAA Resolve (blend current with history)
            // ==========================================================
            if self.taa_enabled {
                if let (Some(resolve_program), Some(vao), Some(current_tex), Some(history_tex)) = (
                    self.taa_resolve_program,
                    self.vao,
                    self.taa_current_texture,
                    self.taa_history_texture,
                ) {
                    // Render to screen (default framebuffer)
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    gl.viewport(0, 0, width as i32, height as i32);
                    gl.clear_color(0.0, 0.0, 0.0, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);

                    gl.use_program(Some(resolve_program));

                    // Bind current frame texture
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(current_tex));
                    if let Some(loc) = gl.get_uniform_location(resolve_program, "u_current") {
                        gl.uniform_1_i32(Some(&loc), 0);
                    }

                    // Bind history texture
                    gl.active_texture(glow::TEXTURE1);
                    gl.bind_texture(glow::TEXTURE_2D, Some(history_tex));
                    if let Some(loc) = gl.get_uniform_location(resolve_program, "u_history") {
                        gl.uniform_1_i32(Some(&loc), 1);
                    }

                    // Set resolution
                    if let Some(loc) = gl.get_uniform_location(resolve_program, "u_resolution") {
                        gl.uniform_2_f32(Some(&loc), width as f32, height as f32);
                    }

                    // Compute motion vector from rotation delta
                    let rotation_delta = self.rotation - self.taa_prev_rotation;
                    // Convert rotation to approximate screen-space motion
                    // Simplified: rotation causes horizontal pan
                    let motion_x = rotation_delta * 0.1; // Scale factor tuned empirically
                    let motion_y = 0.0;

                    if let Some(loc) = gl.get_uniform_location(resolve_program, "u_motion_x") {
                        gl.uniform_1_f32(Some(&loc), motion_x);
                    }
                    if let Some(loc) = gl.get_uniform_location(resolve_program, "u_motion_y") {
                        gl.uniform_1_f32(Some(&loc), motion_y);
                    }

                    // Blend factor: 10% current, 90% history for smooth accumulation
                    if let Some(loc) = gl.get_uniform_location(resolve_program, "u_blend_factor") {
                        gl.uniform_1_f32(Some(&loc), 0.1);
                    }

                    // First frame flag
                    if let Some(loc) = gl.get_uniform_location(resolve_program, "u_first_frame") {
                        gl.uniform_1_f32(Some(&loc), if is_first_frame { 1.0 } else { 0.0 });
                    }

                    // Draw fullscreen triangle for TAA resolve
                    gl.bind_vertex_array(Some(vao));
                    gl.draw_arrays(glow::TRIANGLES, 0, 3);

                    // ==========================================================
                    // PASS 3: Copy resolved result to history for next frame
                    // ==========================================================
                    // We need to copy the screen back to history texture
                    // Use glCopyTexSubImage2D for efficiency
                    if let Some(history_fbo) = self.taa_history_fbo {
                        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None); // Read from screen
                        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(history_fbo)); // Write to history
                        gl.blit_framebuffer(
                            0, 0, width as i32, height as i32,
                            0, 0, width as i32, height as i32,
                            glow::COLOR_BUFFER_BIT,
                            glow::NEAREST,
                        );
                        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    }

                    // Update TAA state for next frame
                    self.taa_prev_rotation = self.rotation;
                    self.taa_frame_index = self.taa_frame_index.wrapping_add(1);
                }
            }

            // === Render GLB model during silence ===
            // Shows when energy is low (sustained silence), shrinks and fades out when audio starts

            if self.glb_loaded && silence_show > 0.08 {  // Hide once shrunk below ~8%
                if let (Some(program), Some(vao)) = (self.glb_program, self.glb_vao) {
                    gl.use_program(Some(program));

                    // Enable blending for transparency
                    gl.enable(glow::BLEND);
                    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

                    // Enable depth testing
                    gl.enable(glow::DEPTH_TEST);
                    gl.depth_func(glow::LEQUAL);
                    gl.clear(glow::DEPTH_BUFFER_BIT);

                    // Disable face culling to see all faces
                    gl.disable(glow::CULL_FACE);

                    // 3D rotation to show the interlocking frame structure
                    let aspect = width as f32 / height as f32;
                    let rotation_y = self.time * 0.15_f32; // Turntable spin
                    let tilt_x: f32 = 0.4; // Tilt forward to show depth

                    // Scale lerps with silence_show - shrinks as audio fades in
                    // Use easeOutQuad curve for natural shrinking motion
                    let scale_t = silence_show * silence_show; // Ease out - shrinks faster at end
                    let scale: f32 = 0.85 * scale_t.max(0.05);  // Min 5% to avoid popping

                    let cos_y = rotation_y.cos();
                    let sin_y = rotation_y.sin();
                    let cos_x = tilt_x.cos();
                    let sin_x = tilt_x.sin();

                    // Rotation matrix: Rx * Ry (tilt then spin) with uniform scale
                    // Then apply aspect ratio correction as a separate step
                    // Column-major order for OpenGL
                    let rot: [f32; 16] = [
                        scale * cos_y,              scale * sin_x * sin_y,     scale * cos_x * sin_y,    0.0,
                        0.0,                        scale * cos_x,             scale * -sin_x,           0.0,
                        scale * -sin_y,             scale * sin_x * cos_y,     scale * cos_x * cos_y,    0.0,
                        0.0,                        0.0,                       0.0,                      1.0,
                    ];

                    // Apply aspect correction: divide X by aspect to maintain proportions
                    let ax = 1.0 / aspect;
                    let model: [f32; 16] = [
                        rot[0] * ax,  rot[1],  rot[2],  rot[3],
                        rot[4] * ax,  rot[5],  rot[6],  rot[7],
                        rot[8] * ax,  rot[9],  rot[10], rot[11],
                        rot[12],      rot[13], rot[14], rot[15],
                    ];

                    // Use model directly as MVP (orthographic, no perspective needed)
                    let mvp = model;

                    // Set uniforms
                    if let Some(loc) = gl.get_uniform_location(program, "u_mvp") {
                        gl.uniform_matrix_4_f32_slice(Some(&loc), false, &mvp);
                    }
                    if let Some(loc) = gl.get_uniform_location(program, "u_model") {
                        gl.uniform_matrix_4_f32_slice(Some(&loc), false, &model);
                    }
                    if let Some(loc) = gl.get_uniform_location(program, "u_time") {
                        gl.uniform_1_f32(Some(&loc), self.time);
                    }
                    if let Some(loc) = gl.get_uniform_location(program, "u_alpha") {
                        // Fade in/out based on silence level - synced with scale shrink
                        // Fades slightly faster than scale so it's invisible before minimum size
                        let alpha = (silence_show * 1.2).min(1.0).powf(0.7) * 0.9;
                        gl.uniform_1_f32(Some(&loc), alpha);
                    }

                    // Bind texture if available
                    if let Some(texture) = self.glb_texture {
                        gl.active_texture(glow::TEXTURE0);
                        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                        if let Some(loc) = gl.get_uniform_location(program, "u_texture") {
                            gl.uniform_1_i32(Some(&loc), 0);
                        }
                        if let Some(loc) = gl.get_uniform_location(program, "u_has_texture") {
                            gl.uniform_1_i32(Some(&loc), 1);
                        }
                    } else {
                        if let Some(loc) = gl.get_uniform_location(program, "u_has_texture") {
                            gl.uniform_1_i32(Some(&loc), 0);
                        }
                    }

                    // Draw mesh
                    gl.bind_vertex_array(Some(vao));
                    gl.draw_elements(glow::TRIANGLES, self.glb_index_count as i32, glow::UNSIGNED_INT, 0);

                    gl.disable(glow::DEPTH_TEST);
                    gl.disable(glow::BLEND);
                }
            }

            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    fn cleanup(&mut self) {
        if let Some(gl) = &self.gl {
            unsafe {
                if let Some(program) = self.program {
                    gl.delete_program(program);
                }
                if let Some(vao) = self.vao {
                    gl.delete_vertex_array(vao);
                }
                if let Some(vbo) = self.vbo {
                    gl.delete_buffer(vbo);
                }
                // TAA cleanup
                if let Some(tex) = self.taa_history_texture {
                    gl.delete_texture(tex);
                }
                if let Some(fbo) = self.taa_history_fbo {
                    gl.delete_framebuffer(fbo);
                }
                if let Some(tex) = self.taa_current_texture {
                    gl.delete_texture(tex);
                }
                if let Some(fbo) = self.taa_current_fbo {
                    gl.delete_framebuffer(fbo);
                }
                if let Some(program) = self.taa_resolve_program {
                    gl.delete_program(program);
                }
            }
        }

        self.program = None;
        self.vao = None;
        self.vbo = None;
        self.taa_history_texture = None;
        self.taa_history_fbo = None;
        self.taa_current_texture = None;
        self.taa_current_fbo = None;
        self.taa_resolve_program = None;
        self.gl = None;
        self.initialized = false;

        log::trace!("[HarmonicGeometry] Renderer cleaned up");
    }

    // === Hero Mode (single canonical voice, no presets) ===

    fn scan_presets(&mut self) -> Vec<ThemePreset> {
        // Single Hero entry - the phase system IS the experience
        vec![ThemePreset {
            id: "harmonic_geometry:hero".to_string(),
            name: "Hero's Journey".to_string(),
            category: "Geometric Field".to_string(),
            subcategory: None,
            renderer_type: Some(RendererType::HarmonicGeometry),
        }]
    }

    fn get_categories(&self) -> Vec<String> {
        vec!["Geometric Field".to_string()]
    }

    fn get_presets_in_category(&self, category: &str) -> Vec<ThemePreset> {
        if category == "Geometric Field" {
            vec![ThemePreset {
                id: "harmonic_geometry:hero".to_string(),
                name: "Hero's Journey".to_string(),
                category: "Geometric Field".to_string(),
                subcategory: None,
                renderer_type: Some(RendererType::HarmonicGeometry),
            }]
        } else {
            Vec::new()
        }
    }

    fn load_preset(&mut self, _preset_id: &str) -> Result<()> {
        // Hero mode - always the same configuration
        // Phase system handles all variation
        log::trace!("[HarmonicGeometry] Hero mode - phase system controls experience");
        Ok(())
    }

    fn load_preset_by_name(&mut self, _category: &str, _name: &str) -> Result<()> {
        // Hero mode - always the same configuration
        log::trace!("[HarmonicGeometry] Hero mode - phase system controls experience");
        Ok(())
    }

    fn current_preset(&self) -> Option<ThemePreset> {
        Some(ThemePreset {
            id: "harmonic_geometry:hero".to_string(),
            name: "Hero's Journey".to_string(),
            category: "Geometric Field".to_string(),
            subcategory: None,
            renderer_type: Some(RendererType::HarmonicGeometry),
        })
    }

    fn set_random_preset(&mut self) -> Option<ThemePreset> {
        // Hero mode - no randomization, phase system handles variation
        ThemeRenderer::current_preset(self)
    }

    fn has_presets(&self) -> bool {
        // Return false to indicate no preset selection needed
        false
    }
}

impl Drop for HarmonicGeometryRenderer {
    fn drop(&mut self) {
        self.cleanup();
    }
}
