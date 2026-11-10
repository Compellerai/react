//! Shader Source Constants
//!
//! REG-Aligned Harmonic Geometry Shaders
//!
//! Implements visualization where geometry emerges directly from the 12-bin chroma vector.
//! No named solids - the harmonic field IS the geometry.
//!
//! GPU OPTIMIZATION: Interior can be rendered at 0.5x resolution using u_lowres_mode.

/// Vertex shader for fullscreen quad (shared by all passes)
pub const VERTEX_SHADER: &str = r#"
#version 140

in vec2 a_pos;
out vec2 v_uv;

void main() {
    v_uv = a_pos * 0.5 + 0.5;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
"#;

/// Harmonic Field Fragment Shader
///
/// Implements REG's unified mathematics of geometry and music:
/// - 12-bin chroma vector directly controls radial profile
/// - Geometry emerges from harmonic number fields, not named shapes
/// - Extrusion and lathe modes for 3D realization
/// - 12-fold and 24-fold symmetry layers
pub const HARMONIC_FIELD_FRAGMENT_SHADER: &str = r#"
#version 140

precision highp float;

in vec2 v_uv;
out vec4 frag_color;

// Constants
#define TAU 6.283185307179586
#define PI 3.141592653589793
#define MAX_STEPS 64        // Maximum steps (reduced by quality tier)
#define MAX_DIST 15.0       // Don't need extreme distance
#define SURF_DIST 0.003     // Surface threshold

// Time and resolution
uniform float u_time;
uniform vec2 u_resolution;

// === QUALITY TIER SYSTEM ===
// 0=Low (0.6x res, fewer steps), 1=Med (dynamic), 2=High (full)
uniform int u_quality_tier;
uniform float u_render_scale;  // Current render scale (0.5-1.0)

// === LICENSE WATERMARK ===
// Driven by entitlements.watermark (1.0 = show, 0.0 = hide).
// Procedural SDF text — removing the uniform doesn't help because
// the value also controls render_scale and other computations.
uniform float u_watermark;

// === TEMPORAL ANTI-ALIASING (TAA) ===
// Sub-pixel jitter offset for TAA - applied to UV in ray generation
// Halton(2,3) sequence provides good temporal distribution
uniform vec2 u_taa_jitter;  // Sub-pixel offset in [-0.5/w, 0.5/w] range

// === BACKGROUND QUALITY BUDGET SYSTEM ===
// Separate from main quality - controls BG VFX cost independently
uniform int u_bg_quality;        // 0=Low, 1=Med, 2=High
uniform float u_bg_budget;       // 0.0-1.0 budget multiplier (future slider)
uniform float u_qs_enabled;      // QS wheel visibility for foveation inversion

// Background quality constants
#define BG_LOW 0
#define BG_MED 1
#define BG_HIGH 2

// === HARMONIC FIELD UNIFORMS ===
// The 12-bin chroma vector packed into 3 vec4s for GPU efficiency
// (C, C#, D, D#), (E, F, F#, G), (G#, A, A#, B)
uniform vec4 u_chroma_0_3;   // C, C#, D, D#
uniform vec4 u_chroma_4_7;   // E, F, F#, G
uniform vec4 u_chroma_8_11;  // G#, A, A#, B
uniform float u_chroma_mean; // Precomputed mean (sum/12) from CPU

// Base radius (neutral circle when all chroma = 0)
uniform float u_base_radius;

// Gain (chroma amplitude multiplier)
uniform float u_gain;

// Extrusion thickness (controlled by tonal energy)
uniform float u_thickness;

// Tonal confidence (0-1) - shape relaxes toward circle when low
uniform float u_tonal_confidence;

// Render mode: 0 = extrusion, 1 = lathe, 2 = both
uniform int u_render_mode;

// Symmetry mode: 12 or 24
uniform int u_symmetry_mode;

// Slow base rotation (always active, not beat-synced)
uniform float u_rotation;

// Independent field rotation (audio-reactive spiral drift)
// Decoupled from u_rotation so field can spin while QS wheel stays fixed
uniform float u_field_rotation;

// Camera (preset/base values)
uniform float u_cam_dist;      // Base distance from center (preset)
uniform float u_cam_height;    // Height above center (preset)
uniform float u_cam_angle;     // Initial angle (preset)

// === DYNAMIC CAMERA UNIFORMS ===
// Blended between Observer/Immersive/Performer modes based on audio
uniform float u_cam_fov;           // FOV factor (1.0 = ~45°, 1.3 = ~55°, 1.5 = ~65°)
uniform float u_cam_orbit_speed;   // Orbit speed (base + beat-quantized impulse)
uniform float u_cam_tilt;          // X-axis tilt angle (radians)
uniform float u_cam_noise_rot;     // Rotational noise amplitude
uniform float u_cam_noise_pos;     // Positional noise amplitude
uniform float u_cam_inside_dip;    // Inside-SDF dip amount (0 = surface, 1.0 = deep inside)
uniform vec2 u_cam_aim_offset;     // Aim offset for silhouette framing (phase-locked to dominant PC)
uniform float u_cam_dyn_dist;      // Dynamic distance (overrides u_cam_dist)
uniform float u_cam_rim_boost;     // Dynamic rim boost when camera is close (prevents flat blob)

// === JOURNEY PHASE UNIFORMS ===
// The REG Journey: Genesis → Formation → Threshold → Interior → Performer → Resolution
uniform float u_journey_phase;       // 0=Genesis, 1=Formation, 2=Threshold, 3=Interior, 4=Performer, 5=Resolution
uniform float u_journey_phase_time;  // Time spent in current phase (seconds)
uniform float u_journey_phase_blend; // Smooth blend for phase transitions (0-1)

// Journey-driven visual state
uniform float u_field_density;       // 0 = hints only, 1 = solid geometry
uniform float u_surface_opacity;     // 1 = solid surface, 0 = see-through
uniform float u_interior_enable;     // 0 = hidden, 1 = full volumetric interior
uniform float u_interior_latch;      // Legacy (use u_inside_mix instead)
uniform float u_inside_mix;          // Smooth inside blend [0..1] for surface/interior crossfade
uniform int u_interior_hard;         // Hard flag: 1 only when truly Inside (for gating expensive features)
uniform float u_skin_intensity_journey; // 0 = subtle skin, 1 = full skin
uniform float u_video_visibility;    // 0 = hidden, 1 = video visible on geometry

// === COMPEL NARRATIVE STATE UNIFORMS ===
// Compels are embedded narrative media revealed by mathematical invariants
uniform float u_compel_exterior_bind;    // 0 = everywhere, 1 = 12-fold harmonic seams only
uniform float u_compel_interior_reveal;  // 0 = hidden, 1 = full interior lattice
uniform float u_compel_emissive_persist; // 0 = off, 1 = afterglow emissive
uniform float u_compel_flicker_gate;     // 0 = stable, 1 = Birth flicker suppression

// Visual settings
uniform float u_glow;
uniform float u_fog_density;
uniform float u_exposure;

// Audio-reactive uniforms (for render effects, NOT geometry)
uniform float u_energy;          // Perceptual energy (0-1) - drives bloom intensity
uniform float u_kick;            // Kick detection (0 or 1) - drives specular flash
uniform float u_stability;       // Temporal stability (0-1) - affects motion smoothness
uniform int u_dom_pc;            // Dominant pitch class (0-11)
uniform float u_section_energy;  // Slow RMS (~3s) for macro story / mode selection

// === SPECTRAL BAND UNIFORMS (for spectral rings) ===
uniform float u_bass;            // Bass energy (0-1, ~60-250Hz)
uniform float u_mid;             // Mid energy (0-1, ~250-4000Hz)
uniform float u_high;            // High energy (0-1, ~4000-16000Hz)
uniform float u_spectral_rings;  // Enable spectral rings (0=off, 1=on)

// === AAA RING UNIFORMS (enhanced 3D torus rings) ===
uniform float u_bass_ema;        // Slow EMA bass (τ≈300ms, smooth motion)
uniform float u_mid_ema;         // Slow EMA mid
uniform float u_high_ema;        // Slow EMA high
uniform float u_bass_peak_hz;    // Peak frequency in bass band (Hz)
uniform float u_mid_peak_hz;     // Peak frequency in mid band (Hz)
uniform float u_high_peak_hz;    // Peak frequency in high band (Hz)
uniform float u_ring_style_aaa;  // 0=classic flat, 1=AAA 3D torus
uniform float u_ring_flow_lines; // Enable animated flow lines
uniform float u_ring_sparkle;    // Enable specular sparkle
uniform float u_ring_fog;        // Enable depth fog
uniform float u_ring_bloom_mask; // Emit bloom from ring cores

// === FREQUENCY LABEL UNIFORMS ===
// Band center frequencies in Hz for display (static or centroid-smoothed)
uniform float u_band_freq[6];    // Hz for each of 6 bands
uniform float u_freq_labels;     // Enable frequency labels (0=off, 1=on)

// === WARP/HYPERSPACE EFFECT ===
uniform float u_warp_intensity;  // 0 = no warp, 1 = full hyperspace effect

// === RING DIVE UNIFORMS ===
// Cinematic inside transition sequence
uniform int u_ring_dive_active;       // 0 = inactive, 1 = active
uniform float u_ring_dive_t;          // Master curve [0..1]
uniform int u_ring_dive_quality;      // 0=Low, 1=Med, 2=High
uniform int u_ring_dive_target_band;  // 0=bass, 1=mid, 2=high
uniform float u_ring_dive_portal_depth;  // Portal depth effect (0.15)
uniform float u_ring_dive_warp_strength; // Warp streak strength (0.8)
uniform float u_ring_dive_alpha_budget;  // Interior alpha budget (0.85)
uniform float u_tension;              // Energy tension for effects

// Ring dive quality tier constants
#define RING_QUALITY_LOW 0
#define RING_QUALITY_MED 1
#define RING_QUALITY_HIGH 2

// === PORTAL FPS UNIFORMS ===
// Rail shooter mode during portal dive
uniform int u_portal_fps_active;       // 1 if FPS mode active during portal dive
uniform int u_portal_fps_path_id;      // Path preset: 0=TunnelGrid, 1=Corridor, 2=RibLanes, 3=KnotPass
uniform int u_portal_fps_lane_idx;     // Current lane index: -2 to +2
uniform int u_portal_fps_segment_id;   // Current segment: 0-7
uniform float u_portal_fps_forward_phase;  // Progress within segment [0..1]
uniform float u_portal_fps_lane_t;     // Lane transition smoothing [0..1]
uniform int u_portal_fps_speed_tier;   // Speed tier: 0=slow, 1=normal, 2=fast
uniform int u_portal_fps_density_tier; // Rail density: 0=24, 1=48, 2=72
uniform float u_portal_fps_sway_phase; // Micro sway oscillation phase
uniform float u_portal_fps_scroll;     // Rail UV scroll phase
uniform int u_portal_fps_nm_count;     // Active near-miss bar count

// Portal FPS frame vectors (locked on entry)
uniform float u_portal_fps_fwd_x;      // Forward direction X (cos of locked azimuth)
uniform float u_portal_fps_fwd_z;      // Forward direction Z (sin of locked azimuth)
uniform float u_portal_fps_right_x;    // Right direction X
uniform float u_portal_fps_right_z;    // Right direction Z

// Portal FPS "make it hit" accents
uniform float u_portal_fps_lane_impulse;   // Parallax snap impulse [0..1]
uniform int u_portal_fps_lane_impulse_dir; // Impulse direction: -1/+1
uniform float u_portal_fps_vista_pulse;    // Vista chamber pulse [0..1]
uniform float u_portal_fps_hit_emphasis;   // Near-miss hit accent [0..1]
uniform int u_portal_fps_corridor_tier;    // Current corridor width: 0=tight, 1=normal, 2=wide
uniform float u_portal_fps_scaffold_emph;  // Scaffold brightness for vista
uniform float u_portal_fps_rail_pulse_0;   // Rail group 0 pulse (downbeat)
uniform float u_portal_fps_rail_pulse_1;   // Rail group 1 pulse (offbeat)
uniform float u_portal_fps_rail_pulse_2;   // Rail group 2 pulse (onset)
uniform float u_portal_entry_aperture;     // Entry aperture/iris effect [0..1], peaks at 1.0 on entry
uniform float u_portal_z_offset;           // Total Z distance from portal entry (cumulative)

// Portal corridor radius tiers (parallax depth perception)
#define CORRIDOR_RADIUS_TIGHT 0.33
#define CORRIDOR_RADIUS_NORMAL 0.42
#define CORRIDOR_RADIUS_WIDE 0.55

// Portal camera constants
#define PORTAL_FOV 68.0               // Wide FOV for immersion
#define PORTAL_LANE_SPACING 0.18      // Lateral spacing between lanes
#define PORTAL_SWAY_AMP 0.015         // Micro sway amplitude

// === FOLD24 UNIFORMS ===
// Secondary inside transition: 12-fold → 24-fold symmetry
uniform int u_inside_mode;            // 0 = normal 12-fold, 1 = 24-fold
uniform float u_fold24_t;             // Transition progress [0..1], smoothed
uniform float u_fold24_mix;           // Smooth blend weight for 12/24-fold scaffold crossfade
uniform float u_fold12_mix;           // = 1.0 - u_fold24_mix
uniform float u_unfold24_intensity;   // Overlap-eased intensity for unfold visuals
                                      // = smoothstep(0.56,0.64,t) * (1-smoothstep(0.80,0.88,t))

// Inside mode constants
#define INSIDE_MODE_12FOLD 0
#define INSIDE_MODE_24FOLD 1

// === TORUS KNOT CYMATICS UNIFORMS ===
// Tertiary inside transition: 24-fold → toroidal knot
uniform int u_torus_active;           // 1 if torus knot active or transitioning
uniform float u_torus_t;              // Master transition progress [0..1]
uniform int u_torus_subphase;         // -1=idle, 0=curl, 1=thread, 2=stabilize, 3=active
uniform float u_torus_p;              // Torus knot p parameter (float for smooth transitions)
uniform float u_torus_q;              // Torus knot q parameter (float for smooth transitions)
uniform float u_torus_phase;          // Phase advance for marching nodes (includes seed)
uniform int u_torus_quality;          // 0=low(64), 1=med(128), 2=high(256)
uniform float u_torus_ref_hz;         // Reference frequency (Hz)
uniform float u_torus_live_hz;        // Live detected frequency (Hz)
uniform float u_torus_stability;      // Stability value (for blending)
uniform float u_torus_golden_blend;   // 0=ratio knot, 1=golden carrier
uniform float u_torus_video_fade;     // Video fade during transition
uniform float u_torus_support;        // Perceptual sustain score [0..1]
uniform float u_torus_seed_phase;     // Stable seed phase (captured on entry)
uniform float u_torus_major_r;        // Major radius (donut size)
uniform float u_torus_minor_r;        // Minor radius (tube thickness)
uniform float u_torus_line_thick;     // Line thickness for strand
uniform float u_torus_node_radius;    // Node glow radius

// Enhanced torus knot rendering uniforms
uniform float u_torus_tube_radius;    // Visual tube radius (for SDF)
uniform float u_torus_rim_intensity;  // Video rim light strength
uniform float u_torus_spec_power;     // Specular sharpness (tension-driven)
uniform float u_torus_spec_intensity; // Specular intensity
uniform float u_torus_depth_haze;     // Distance fog strength
uniform float u_torus_bloom_thresh;   // Bloom threshold
uniform float u_torus_particle_rate;  // Particle spawn rate (energy-driven)
uniform float u_torus_turbulence;     // Particle turbulence (tension-driven)
uniform float u_torus_cam_orbit;      // Camera orbit speed
uniform float u_torus_cam_dist;       // Camera distance
uniform int u_torus_debug_flags;      // Debug toggles bitfield

// Pro-level vortex visual uniforms
uniform float u_torus_chromatic;      // Chromatic fringing intensity (High tier only)
uniform float u_torus_sss_intensity;  // Subsurface scattering approximation strength

// === PREMIUM 3D QUANTIZED STATE UNIFORMS ===
uniform int u_torus_orbit_step;       // Camera orbit step [0..23] for 24-fold locked orbit
uniform float u_torus_tube_r_latched; // Latched tube radius from discrete levels
uniform int u_torus_vortex_n;         // Vortex N-fold symmetry (locked to p)
uniform float u_torus_vortex_strength;// Latched vortex strength [0.0, 0.33, 0.66, 1.0]
uniform float u_torus_rim_gain;       // Latched rim light gain [0.0, 0.5, 1.0]
uniform float u_torus_depth_sep;      // Latched depth separation [0.0, 0.5, 1.0]
uniform float u_torus_spec_boost;     // Latched specular boost [0.0, 0.5, 1.0]

// === CANVAS/PAINT SYSTEM UNIFORMS ===
// Quantized cymatic topology for stable, readable torus knot rendering
uniform int u_torus_canvas_id;        // Canvas preset ID (0-21, see TorusCanvas enum)
uniform int u_torus_paint_id;         // Paint preset ID (0-11, see TorusPaint enum)
uniform int u_torus_link_count;       // Link count: 1=knot, >1=multi-loop link
uniform int u_torus_node_freq;        // Node frequency F (how many nodes along knot)
uniform int u_torus_paint_phase;      // Paint phase step [0..23] for 24-fold anchored phase
uniform int u_torus_paint_is_tet;     // 1 if paint is 12-TET (allows phase wobble)
uniform float u_torus_winding_interval; // Winding interval WI = p*360/q (degrees)

// Torus knot debug flag bits
#define TK_DEBUG_TUBE      1   // Show tube shading
#define TK_DEBUG_RIM       2   // Show video rim light
#define TK_DEBUG_SPEC      4   // Show specular
#define TK_DEBUG_BLOOM     8   // Show bloom contribution
#define TK_DEBUG_PARTICLES 16  // Show particles
#define TK_DEBUG_HAZE      32  // Show depth haze

// Torus knot subphase constants
#define TORUS_SUBPHASE_IDLE -1
#define TORUS_SUBPHASE_CURL 0
#define TORUS_SUBPHASE_THREAD 1
#define TORUS_SUBPHASE_STABILIZE 2
#define TORUS_SUBPHASE_ACTIVE 3

// === VORTEX TOPOLOGY UNIFORMS ===
// Spiral flow topology - sibling to TorusKnot
uniform int u_vortex_active;           // 1 if vortex topology active or transitioning
uniform int u_vortex_gate_center;      // Gate center index (0..23) - 24-fold quantized
uniform int u_vortex_circulation_step; // Circulation step per beat: -1, 0, or +1
uniform int u_vortex_direction;        // Flow direction: 0=Inward, 1=Outward
uniform int u_vortex_shell_set_id;     // Shell set ID (stability-based band selection)
uniform int u_vortex_pitch_q;          // Pitch quantization (0..23)
uniform float u_vortex_pressure_env;   // Pressure envelope value [0..1]
uniform float u_vortex_t;              // Transition progress [0..1]

// === 3D VORTEX / DRAIN UNIFORMS ===
uniform float u_vortex_depth;          // Torus depth illusion [0..1]
uniform float u_vortex_sink_strength;  // Sink/drain pull strength [0..1]
uniform float u_vortex_sink_radius;    // Sink radius (from preset, normalized)
uniform int u_vortex_bulge_phase_q;    // Bulge phase offset (0..23)
uniform float u_vortex_occlusion;      // Backface occlusion amount [0..1]
uniform float u_vortex_sink_env;       // Onset-driven sink envelope [0..1]
uniform float u_vortex_qs_drain_pull;  // QS wheel drain pull [-0.15..1.1] (spring animated)

// === GRANT TRIANGLE SHELL RADII ===
// Nine Generative Means from Pythagorean triple, normalized [0..1]
// Provides harmonic shell structure for vortex topology
uniform float u_grant_shell_radii[9]; // 9 shell radii (innermost to outermost)
uniform int u_grant_triangle_id;      // Triangle index (0..7 consecutive-leg family)
uniform int u_grant_shell_count;      // Active shell count (typically 9)

// Vortex direction constants
#define VORTEX_DIR_INWARD 0
#define VORTEX_DIR_OUTWARD 1

// === OUTER BACKDROP UNIFORMS ===
// Premium cathedral background: aperture + rim halo
uniform int u_outer_style_id;          // 0=Aperture+Halo, 1=SeamArcConstellation, 2=FacetedShell
uniform int u_outer_aperture_q;        // Aperture radius preset (0..3)
uniform float u_outer_pulse_env;       // Fast pulse envelope [0..1] (beat response)
uniform float u_outer_pulse_slow;      // Slow pulse envelope [0..1] (bar-length bloom)
uniform float u_outer_gate_fade;       // Gate emphasis fade [0..1] (smooth, confidence-based)
uniform int u_outer_is_torusknot;      // 1 if TorusKnot topology (for secondary rim)
uniform int u_outer_is_vortex;         // 1 if Vortex topology (tighter aperture)

// Outer style constants
#define OUTER_STYLE_APERTURE_HALO 0
#define OUTER_STYLE_SEAM_ARCS 1
#define OUTER_STYLE_FACETED_SHELL 2

// === GATE CONFIDENCE UNIFORM (master kill switch for gate-dependent effects) ===
// When u_gate_ok == 0, all gate-dependent features MUST show uniform seam weighting
// When u_gate_ok == 1, gate bias (seam emphasis, ±1 neighbors, etc.) is allowed
// This value is smoothed CPU-side with hysteresis to prevent visual popping
uniform float u_gate_ok;               // Gate confidence [0..1]: 1=bias allowed, 0=no bias (ambiguous)

// === TOPOLOGY CROSSFADE UNIFORMS ===
// Bar-edge deterministic routing between 24-fold topologies
uniform int u_topology_id;             // Current topology: 0=Scaffold24, 1=TorusKnot, 2=Vortex, etc.
uniform int u_topology_prev_id;        // Previous topology (for crossfade)
uniform float u_topology_xfade_t;      // Crossfade progress [0..1]

// Topology ID constants
#define TOPO_SCAFFOLD24 0
#define TOPO_TORUSKNOT 1
#define TOPO_VORTEX 2
#define TOPO_RADIALSHELL 3
#define TOPO_BRAIDEDWAVE 4
#define TOPO_AFTERGLOW 5
#define TOPO_DODECAHEDRAL 6

// === REG-LOCKED CAMERA UNIFORMS ===
// Quantized camera motion that rides the symmetry grid
// All targets are discrete: azimuth anchors, elevation tiers, radius steps
uniform int u_cam_rig;                 // CameraRig: 0=Orbit, 1=AxialPush, 2=PortalDive
uniform float u_cam_reg_azimuth;       // Target azimuth in radians (k * TAU/N)
uniform float u_cam_reg_elevation;     // Target elevation in radians (from tier)
uniform float u_cam_reg_radius;        // Target radius (from step)
uniform float u_cam_reg_fov;           // Target FOV (from step)
uniform int u_cam_in_portal;           // 1 if portal dive active
uniform float u_cam_portal_phase;      // Portal dive progress [0..1]
uniform int u_cam_portal_anchor_k;     // Locked portal wedge anchor (0..23)

// Camera rig constants
#define CAM_RIG_ORBIT 0
#define CAM_RIG_AXIAL_PUSH 1
#define CAM_RIG_PORTAL_DIVE 2

// === DODECAHEDRAL RESONANCE UNIFORMS ===
// Crystalline harmonic arrival - 12 pentagonal faces (24-fold via × 2 orientations)
uniform int u_dodeca_active;              // 1 if dodecahedral topology active or transitioning
uniform float u_dodeca_t;                 // Transition progress [0..1]
uniform float u_dodeca_face_activation[12]; // Face activation levels [0..1] per face
uniform float u_dodeca_face_depths[12];   // Face depth offsets (Grant shell radii mapped)
uniform int u_dodeca_dominant_face;       // Index of brightest face (0..11)
uniform float u_dodeca_vertex_breath;     // Vertex breath phase (radians)
uniform float u_dodeca_vertex_amp;        // Vertex breath amplitude [0..1]
uniform float u_dodeca_edge_pulse;        // Edge pulse envelope [0..1]
uniform float u_dodeca_edge_glow;         // Edge glow intensity [0..1]
uniform float u_dodeca_rotation;          // Slow rotation angle (radians)
uniform float u_dodeca_face_lock;         // Face lock strength [0..1]
uniform float u_dodeca_phi;               // Golden ratio resonance [0..1]
uniform int u_dodeca_gate;                // Inherited gate from vortex (24-fold alignment)

// === EMERGENCE STATE UNIFORMS ===
// Gating system: Idle → Candidate → Prewarm → Active
uniform int u_emerge_state;           // 0=Idle, 1=Candidate, 2=Prewarm, 3=Active
uniform float u_emerge_candidate;     // 1.0 when in Candidate phase (cheap preview cue)
uniform float u_emerge_prewarm;       // Prewarm progress [0..1]

#define EMERGE_IDLE 0
#define EMERGE_CANDIDATE 1
#define EMERGE_PREWARM 2
#define EMERGE_ACTIVE 3

// === QUADRIPOLAR TRAVEL UNIFORMS (REG icositetragon journey) ===
// Coherent 12↔24 fold journey following quaternion symmetry
uniform float u_quad_active;          // 1.0 if quadripolar travel is active
uniform float u_quad_phase;           // 0=inactive, 1=guided12, 2=refract, 3=unfold24, 4=resolve
uniform float u_quad_phase_t;         // Progress within current phase [0,1]
uniform float u_quad_total_t;         // Total journey progress [0,1]
uniform float u_quad_seam_basis;      // Seam basis interpolation: 0=12-fold, 1=24-fold
uniform float u_quad_video_density;   // Video density (reduced during unfold for clarity)
uniform float u_quad_spoke_flash;     // Spoke flash intensity (brief 24 spokes during unfold)
uniform float u_quad_wedge_intensity; // Wedge segmentation: 0=fog, 1=24 distinct wedges
uniform float u_quad_energy_burst;    // Energy burst visual emphasis
uniform float u_quad_camera_angle;    // Current camera angle (radians)
uniform vec4 u_quad_ports;            // Quaternion ports {A, 360-A, 180+A, 180-A} in radians

// Quadripolar phase constants
#define QUAD_PHASE_INACTIVE 0.0
#define QUAD_PHASE_GUIDED12 1.0
#define QUAD_PHASE_REFRACT 2.0
#define QUAD_PHASE_UNFOLD24 3.0
#define QUAD_PHASE_RESOLVE 4.0

// === WARP JUMP QS UNIFORMS ===
// QS wheel IS the portal. Transition: Acquire → Tunnel → Burst
// Camera + geometry driven, with hyperspace tracers outside portal
uniform int u_warp_active;                    // 1 if warp transition is active
uniform int u_warp_phase;                     // -1=idle, 0=acquire, 1=tunnel, 2=burst
uniform float u_warp_t;                       // Master timeline [0..1]
uniform float u_warp_phase_t;                 // Progress within current phase [0..1]
uniform vec2 u_warp_portal_center;            // Portal center in screen coords (0.5, 0.5)
uniform float u_warp_portal_radius;           // Portal radius in normalized screen units
uniform float u_warp_portal_spin;             // Portal corkscrew spin (radians)
uniform float u_warp_zoom;                    // Camera zoom factor (affects FOV)
uniform float u_warp_flash;                   // Flash intensity (peaks at t≈0.52)
uniform float u_warp_ripple;                  // Ripple expansion radius [0..1]
uniform float u_warp_tracer_intensity;        // Hyperspace tracer intensity (outside portal)
uniform float u_warp_camera_z;                // Camera Z offset
uniform float u_warp_strength;                // Geometry warp pull toward portal
uniform float u_warp_qs_contrast;             // QS wheel contrast boost
uniform float u_warp_qs_thickness;            // QS wheel ring thickness multiplier
uniform float u_warp_qs_seam_glow;            // QS wheel seam glow intensity
uniform float u_warp_qs_scale;                // QS wheel scale (shrink/pop/jiggle animation)
uniform float u_warp_chromatic;               // Chromatic aberration intensity (RGB split)
uniform float u_warp_vortex;                  // Vortex UV distortion strength

// === GLOBAL SILENCE UNIFORMS ===
// When active, all layers fade to black except GLB
// This is the "idle but alive" state - system waiting for audio
uniform int u_silence_active;           // 1 if global silence is active
uniform float u_silence_blackout;       // Blackout alpha [0..1] - fades all layers to black
uniform float u_silence_glb_alpha;      // GLB alpha [0..1] - GLB fades in during silence
uniform float u_silence_grace;          // Resume grace timer (prevents immediate re-trigger)

// === PRESENTATION MODE UNIFORMS (Museum-mode polish) ===
// Smooth crossfade between Idle (screensaver) and Live (performance) rendering
uniform int u_presentation_mode;        // 0=Idle, 1=Live
uniform float u_idle_mix;               // Crossfade blend: 0=Live, 1=Idle
uniform int u_dom_chroma_latched;       // Latched dominant chroma index [0..11]
uniform float u_activity_level;         // Smoothed activity [0..1]

// Warp phase constants
#define WARP_PHASE_IDLE -1
#define WARP_PHASE_ACQUIRE 0
#define WARP_PHASE_TUNNEL 1
#define WARP_PHASE_BURST 2

// === QUATERNION CHARGES UNIFORMS (REG charge visualization) ===
// Active during fold24 transition window (0.10 < t < 0.90) when inside
uniform int u_qc_active;              // 1 if QC visualization is active
uniform float u_qc_w;                 // Reduced angle w in [0, π/2)
uniform vec4 u_qc_ports;              // 4 quaternion port angles (radians, 0..TAU)
uniform int u_qc_active_port;         // Current active port index (0-3)
uniform float u_qc_strength;          // Overlay strength (0-1), from fold24_t smoothstep
uniform ivec4 u_qc_charges;           // Charge per port: 0=+1, 1=+i, 2=-1, 3=-i
uniform vec4 u_qc_ok;                 // Invariant flags: complements, total, palindrome, lattice

// === POLARITY EFFECT UNIFORMS (flash + smoke on quadrant change) ===
uniform float u_polarity_effect_timer;  // 1.5 at trigger, counts to 0. >0.5 = hold, <0.5 = flash+smoke
uniform float u_polarity_effect_angle;  // Angle where effect is centered (radians, 0=E, π/2=N, etc.)
uniform int u_polarity_quadrant;        // Current quadrant (0=E, 1=N, 2=W, 3=S)
uniform int u_polarity_prev_quadrant;   // Previous quadrant
uniform int u_polarity_direction;       // +1=CW, -1=CCW, 0=none
uniform int u_polarity_degree;          // Degree value (1-360) at polarity change for label

// === QUATERNION SYMMETRY WHEEL AUDIO UNIFORMS ===
// REG Figure 8: Audio-reactive quaternion set highlighting
// Active integer A computed from: dominant pitch class -> spoke, energy -> ring
// Quaternion set Q(A) = {A, 360-A, 180-A, 180+A} normalized to [1..360]
uniform int u_qs_active_A;            // Currently active integer A (1-360)
uniform ivec4 u_qs_quartet;           // The 4 quaternion set members: Q(A)
uniform float u_qs_intensity;         // Highlight intensity (0-1) from audio energy
uniform int u_qs_debug;               // Debug mode: 1 = show Q(A) overlay info
uniform int u_qs_debug_color;         // Color debug: 1 = base colors only, no audio/highlights

// Charge constants for readability
#define QC_CHARGE_PLUS1 0
#define QC_CHARGE_PLUSI 1
#define QC_CHARGE_MINUS1 2
#define QC_CHARGE_MINUSI 3

// === GEOMETRY STABILITY UNIFORMS ===
uniform float u_trust;           // Trust factor (0-1) = smoothstep(confidence) × smoothstep(stability)
                                 // Gates geometry deformation: 0 = neutral circle, 1 = full lobes
                                 // When low (ambiguous/unstable), shape holds to prevent flopping

// === DYNAMIC QUALITY FOR GPU OPTIMIZATION ===
// When inside, reduce quality to prevent GPU burn
uniform float u_interior_quality;  // 0.3 (low/inside) to 1.0 (high/outside)
                                   // Controls: raymarch steps, interior layers, fold iterations

// === INSIDE 24-FOLD QUALITY SYSTEM ===
// Dynamic quality tier with perf guardrails (auto-scales based on GPU load)
uniform int u_inside_quality_tier;     // 0=Low, 1=Med, 2=High
uniform int u_inside_shell_count;      // 2/3/4 shells for interior rendering
uniform int u_inside_accent_budget;    // 0/6/12 max sparkle accents per frame
uniform int u_inside_label_density;    // 0=none, 1=ticks, 2=sparse labels
uniform float u_inside_fog_enabled;    // 0.0 or 1.0
uniform float u_inside_sparkle_enabled; // 0.0 or 1.0
uniform float u_inside_perf_pressure;  // 0-1, visual feedback of GPU stress

// === KICK USAGE RULES ===
// Kick is awesome for micro accents, terrible for macro motion.
// GOOD: bloom pump, rim boost, seam glow, sparkle, starburst, tiny shake
// BAD: inside/outside decision, distance/FOV, aim offset, phase transitions
// Rule: kick_used = smoothstep(0.05, 0.25, kick) * (1.0 - inside_amount)
// All kick FX clamped to safe ranges (max 15% additive brightness)

// === SKIN SYSTEM UNIFORMS ===
// Skins are non-destructive visual layers - they NEVER modify geometry
uniform int u_skin_mode;           // 0=none, 1=surface, 2=emissive, 3=both
uniform float u_skin_strength;     // Overall skin strength (0-1) [legacy]
uniform int u_skin_lock_dominant;  // Lock UV to dominant pitch (0=false, 1=true)
uniform int u_skin_hue_follow;     // Hue-shift by pitch class (0=false, 1=true)
uniform float u_skin_contrast;     // Surface contrast (1.0=neutral, recommend 1.2-1.5)
uniform float u_skin_saturation;   // Surface saturation (1.0=neutral, recommend 1.1-1.4)
uniform float u_skin_emissive;     // Emissive intensity (0-1+)
uniform float u_skin_rim;          // Rim flare strength (0-1)
uniform float u_skin_bloom;        // Bloom amount (0-1)
uniform int u_skin_starburst;      // Enable starburst streaks (0=false, 1=true)
uniform float u_skin_edge;         // Edge outline strength (0-1)
uniform float u_skin_sparkle;      // Specular sparkle intensity (0-1)
uniform sampler2D u_skin_tex;      // Skin texture (Compel video frame)
uniform int u_skin_has_texture;    // Whether skin texture is bound (0=false, 1=true)
uniform float u_video_aspect;      // Video aspect ratio (width/height) - <1.0 for portrait

// Camera texture (background media layer - composited on top of skin in QS wheel)
uniform sampler2D u_camera_tex;    // Camera texture
uniform int u_camera_has_texture;  // Whether camera texture is bound (0=false, 1=true)
uniform float u_camera_alpha;      // Camera blend factor (0=video only, 1=camera only)
uniform float u_camera_aspect;     // Camera aspect ratio

// QS Wheel number atlas (1-360 pre-rendered digits)
uniform sampler2D u_qs_numbers_tex;     // 24x15 atlas of numbers
uniform int u_qs_numbers_has_texture;   // Whether atlas is bound (0=false, 1=true)
uniform int u_qs_hide_numbers;          // 1 = hide degree numbers on QS wheel
uniform int u_qs_dominant_only;         // 1 = show only dominant note's Q(A) set, 0 = show all notes

// Chroma wheel note atlas (C, C#, D, ... B pre-rendered labels)
uniform sampler2D u_chroma_notes_tex;   // 12x1 atlas of note names
uniform int u_chroma_notes_has_texture; // Whether atlas is bound (0=false, 1=true)

// NEW: Albedo-first rendering uniforms
uniform float u_skin_surface_mix;  // Video -> albedo mix (0-1, recommend 0.85-1.0)
uniform float u_skin_shadow_lift;  // Shadow lift to prevent AO crushing (0-0.2)
uniform float u_skin_ao_clamp;     // Min AO value (0-1, recommend 0.35)
uniform int u_skin_uv_mode;        // UV mode: 0=cylindrical, 1=triplanar

// === INTERIOR RENDERING UNIFORMS ===
// When camera dips inside, render volumetric interior with chopped video
uniform float u_interior_cell_size;     // Cell size for chopped video (0.2-1.0)
uniform float u_interior_lattice_freq;  // Harmonic lattice frequency (3.0-12.0)
uniform float u_interior_layers;        // Number of volume layers (4-8)
uniform float u_interior_layer_step;    // Distance between layers (0.05-0.2)
uniform float u_interior_video_boost;   // Video brightness boost inside (0.5-2.0)
uniform float u_interior_seam_strength; // Seam glow between cells (0.0-1.0)
uniform float u_interior_fold_iters;    // Fractal fold iterations (0-6)
uniform float u_interior_density_scale; // Density field scale (0.5-2.0)

// ============ QS WHEEL BEAUTY CONSTANTS ============
// REG-SAFE VISUAL CHANGES: Clarity + UX polish only
// MODERN NEON AESTHETIC - sleek, glowing, futuristic
// Premium "holographic instrument" look

// Anti-aliasing quality
#define QS_AA_SCALE 1.8               // Smoother AA for clean edges

// Grid line styling - crisp, glowing neon
#define QS_MAJOR_LINE_PX 1.2          // Thin but visible
#define QS_MINOR_LINE_PX 0.5          // Ultra thin
#define QS_GRID_OPACITY 0.8           // More opaque for crispness
#define QS_GRID_GLOW 0.3              // Neon glow around lines

// REG-SAFE: HUD Scrim - MINIMAL overlay, video shines through
#define QS_SCRIM_BASE_ALPHA 0.0       // NO base scrim - video is king
#define QS_SCRIM_ENERGY_BOOST 0.02    // Tiny energy boost
#define QS_SCRIM_DOMINANT_BOOST 0.01  // Minimal dominant boost

// REG-SAFE: Seam lines - bright neon accents
#define QS_SEAM_BRIGHTNESS 2.0        // Bright seam lines
#define QS_SEAM_GLOW 0.4              // Strong seam glow

// Digit rendering - CRISP readable text
#define QS_DIGIT_PLATE_OPACITY 0.45   // Strong pill background for contrast
#define QS_DIGIT_OUTLINE_STRENGTH 1.2 // Strong dark outline
#define QS_DIGIT_SHADOW_OFFSET 0.001  // Tight shadow
#define QS_DIGIT_SHADOW_ALPHA 0.7     // Dark shadow
#define QS_DIGIT_SCALE 0.85           // Larger for readability
#define QS_DIGIT_INNER_SCALE 1.2      // Extra scale boost for inner rings
#define QS_DIGIT_AUDIO_BRIGHTNESS 0.3 // Good audio reactivity
#define QS_DIGIT_KICK_BRIGHTNESS 0.2  // Punchy kick response

// REG-SAFE: Label Density LOD thresholds (ring indices)
#define QS_LOD_INNER_RINGS 5          // Rings 0-4: show ALL labels
#define QS_LOD_MID_RINGS 10           // Rings 5-9: show every 2nd
#define QS_LOD_OUTER_START 10         // Rings 10+: show every 4th

// REG-SAFE: Highlight system - border glow only, NO video recoloring
#define QS_HIGHLIGHT_GLOW_STRENGTH 0.4   // Softer inner glow
#define QS_HIGHLIGHT_OUTLINE_PX 2.5      // Bold border outline
#define QS_HIGHLIGHT_PULSE_RATE 2.5      // Smooth pulse
#define QS_HIGHLIGHT_BORDER_GLOW 1.0     // Border glow intensity
#define QS_HIGHLIGHT_SEAM_BOOST 1.8      // Seam brightness boost on highlight

// REG-SAFE: Center video containment
#define QS_CENTER_MASK_HARDNESS 0.94  // Hard circular mask
#define QS_CENTER_RIM_WIDTH 0.012     // Thin neon rim
#define QS_CENTER_RIM_ALPHA 0.85      // Bright rim
#define QS_CENTER_VIGNETTE 0.2        // Light vignette
#define QS_CENTER_DESAT 0.0           // NO desaturation - keep video vivid
#define QS_CENTER_DIM 1.0             // NO dimming - full brightness

// Depth and glass treatment - premium holographic
#define QS_VIGNETTE_STRENGTH 0.12     // Subtle edge darkening
#define QS_VIGNETTE_WIDTH 0.15        // Tighter falloff
#define QS_GLASS_SPEC_STRENGTH 0.15   // Visible specular highlight
#define QS_RIM_GLOW 0.35              // Strong glowing rim

// Inner core styling
#define QS_CORE_GLOW 0.08             // Subtle core glow (was 0.3, reduced to show video)

// Bloom and neon effects - VIBRANT
#define QS_BLOOM_INTENSITY 0.35       // More bloom
#define QS_NEON_SATURATION 1.25       // Boost vibrancy
#define QS_BACKGROUND_DARKEN 0.7      // Darker BG for contrast

// NEW: Video brightness in cells - MAXIMUM VISIBILITY
#define QS_VIDEO_BRIGHTNESS 2.5       // VERY bright video
#define QS_VIDEO_CONTRAST 1.35        // Punchy contrast
#define QS_VIDEO_SATURATION 1.3       // Vivid saturated colors
#define QS_WEDGE_TINT_AMOUNT 0.03     // Barely any wedge tint

// ============ ROTATION HELPERS ============

mat2 rot2D(float a) {
    float c = cos(a);
    float s = sin(a);
    return mat2(c, -s, s, c);
}

mat3 rotateY(float a) {
    float c = cos(a);
    float s = sin(a);
    return mat3(c, 0, s, 0, 1, 0, -s, 0, c);
}

mat3 rotateX(float a) {
    float c = cos(a);
    float s = sin(a);
    return mat3(1, 0, 0, 0, c, -s, 0, s, c);
}

// ============ KICK USAGE HELPER ============
// Kick is awesome for micro accents, terrible for macro motion.
// Suppress kick when inside (interior owns the frame).
// Extra suppression during unfold24 (kick reads as "flopping").
// Returns 0-1 value safe for visual FX, clamped and suppressed.
float getKickUsed(float inside_amount) {
    // Smooth threshold: ignore tiny kicks, cap large ones
    float kick_smooth = smoothstep(0.05, 0.25, u_kick);
    // Suppress when inside (interior should be calm)
    float kick_used = kick_smooth * (1.0 - inside_amount);

    // EXTRA suppression during unfold24 - kick reads as chaos during the reveal
    // inside + unfold: kick *= 0.2
    // inside + guided: kick *= 0.35
    float unfoldSuppress = u_unfold24_intensity * 0.8;  // Up to 80% reduction during unfold
    float guidedSuppress = (1.0 - u_unfold24_intensity) * inside_amount * 0.65;  // 65% reduction when guided but not unfolding
    kick_used *= (1.0 - max(unfoldSuppress, guidedSuppress));

    return kick_used;
}

// Max additive brightness from kick (prevents overblown highlights)
#define KICK_PUMP_MAX 0.15

// ============ QUADRIPOLAR TRAVEL HELPERS ============
// REG icositetragon quaternion symmetry journey functions

// Get seam angle based on seamBasis interpolation (12→24 fold)
// At basis=0: k*30° (12-fold seams)
// At basis=1: k*15° (24-fold seams)
float getSeamAngle(float theta, float basis) {
    float step12 = PI / 6.0;   // 30° = π/6
    float step24 = PI / 12.0;  // 15° = π/12
    float step = mix(step12, step24, basis);
    return round(theta / step) * step;
}

// Get distance to nearest seam (interpolated 12↔24)
float getSeamDist(float theta, float basis) {
    float step12 = PI / 6.0;
    float step24 = PI / 12.0;
    float step = mix(step12, step24, basis);

    // Distance to nearest seam
    float nearest = round(theta / step) * step;
    float dist = abs(theta - nearest);

    // Handle wrap at 2π
    if (dist > step * 0.5) dist = step - dist;

    return dist;
}

// Get wedge ID (0-23) for 24-wedge segmentation
int getWedgeID(float theta) {
    float step = PI / 12.0;  // 15° wedges
    int id = int(floor(theta / step));
    return id < 0 ? id + 24 : (id >= 24 ? id - 24 : id);
}

// Get stable UV offset for each wedge (deterministic hash)
vec2 getWedgeUVOffset(int wedgeID) {
    // Simple hash for stable per-wedge offsets
    float h = fract(sin(float(wedgeID) * 127.1) * 43758.5453);
    float v = fract(sin(float(wedgeID) * 269.5) * 13758.5453);
    return vec2(h, v) * 0.25;  // Small offsets for variety without chaos
}

// Get spoke flash intensity at angle theta
// Spokes appear at 24-fold seams during unfold phase
float getSpokeFlash(float theta, float flash) {
    if (flash < 0.01) return 0.0;

    float step = PI / 12.0;  // 24 spokes at 15° intervals
    float dist = getSeamDist(theta, 1.0);  // Always 24-fold for spokes

    // Sharp spoke line
    float spokeWidth = 0.02;
    float spoke = smoothstep(spokeWidth, spokeWidth * 0.1, dist);

    return spoke * flash;
}

// Get wedge mask value
// Returns 1.0 at wedge center, 0.0 at wedge boundaries
// Used to segment video into 24 distinct cells
float getWedgeMask(float theta, float intensity) {
    if (intensity < 0.01) return 1.0;  // No segmentation when intensity is 0

    float step = PI / 12.0;  // 24 wedges
    float dist = getSeamDist(theta, 1.0);  // Distance to nearest 24-fold seam

    // Soft wedge boundary
    float boundaryWidth = step * 0.15;  // 15% of wedge width
    float mask = smoothstep(0.0, boundaryWidth, dist);

    // Blend based on intensity
    return mix(1.0, mask, intensity);
}

// Apply quadripolar video density reduction
// During unfold, video becomes more sparse/structural
float quadVideoOpacity(float baseOpacity) {
    return baseOpacity * u_quad_video_density;
}

// Check if an angle is near a quaternion port
float nearPort(float theta, float tolerance) {
    float minDist = 1000.0;
    for (int i = 0; i < 4; i++) {
        float port = u_quad_ports[i];
        float dist = abs(theta - port);
        if (dist > PI) dist = 2.0 * PI - dist;  // Wrap
        minDist = min(minDist, dist);
    }
    return smoothstep(tolerance, tolerance * 0.2, minDist);
}

// ============ VIDEO UV MAPPING ============
// Unified function for mapping circular viewport coordinates to video texture UV
// This ensures consistent video display regardless of where it's rendered

// Map circular viewport UV to video texture UV
// circularUV: coordinates centered at (0,0), with x already multiplied by screen aspect
// screenAspect: screen width/height ratio
// zoom: 0.0-1.0, lower = more zoomed in (shows less of video)
// Returns: video texture UV coordinates [0,1]
vec2 mapCircularToVideoUV(vec2 circularUV, float screenAspect, float zoom) {
    // Step 1: Undo screen aspect correction to get true circular coordinates
    vec2 trueCircularUV = circularUV;
    trueCircularUV.x /= screenAspect;

    // Step 2: Apply zoom (scale coordinates)
    vec2 videoUV = trueCircularUV * zoom + 0.5;

    // Step 3: Clamp to valid range
    return clamp(videoUV, 0.0, 1.0);
}

// ============ HARMONIC PROFILE ============

// Catmull-Rom spline interpolation for smooth curves
float catmullRom(float p0, float p1, float p2, float p3, float t) {
    float t2 = t * t;
    float t3 = t2 * t;
    return 0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
}

// Helper to access chroma from packed vec4 uniforms
// Optimized: 3 branches instead of 12 for GPU efficiency
float getChroma(int idx) {
    // Ensure idx is in range [0, 11]
    int i = idx - (idx / 12) * 12;
    if (i < 0) i += 12;

    // Select from packed vec4s (much faster than 12 if-branches)
    if (i < 4) return u_chroma_0_3[i];
    if (i < 8) return u_chroma_4_7[i - 4];
    return u_chroma_8_11[i - 8];
}

// Mean chroma (precomputed on CPU for efficiency)
float chromaMean() {
    return u_chroma_mean;
}

// REG deviation formula: show deviation from harmonic balance
// radius = base + k_pos * max(chroma - mean, 0) - k_neg * max(mean - chroma, 0)
// This creates lobes (positive deviation) and indentations (negative deviation)
//
// TRUST GATING: When u_trust is low (ambiguous/unstable), geometry stays
// closer to neutral circle. This prevents flopping from estimator noise.
float deviationRadius(int idx, float mean) {
    float chroma = getChroma(idx);
    float deviation = chroma - mean;

    // === SILENCE GATE for breathing animations ===
    float breath_gate = smoothstep(0.02, 0.15, u_energy);

    // Variance floor: even uniform fields breathe slightly
    // When deviation is tiny, add subtle breathing based on time + bin position
    // GATED: No breathing animation during silence
    float breath = sin(u_time * 0.3 + float(idx) * 0.52) * 0.04 * breath_gate;
    float effective_dev = deviation;
    if (abs(deviation) < 0.08) {
        // Mix in breathing when deviation is small
        float breath_mix = 1.0 - abs(deviation) / 0.08;
        effective_dev = deviation + breath * breath_mix * (1.0 - u_tonal_confidence);
    }

    // === TRUST-SCALED DEFORMATION ===
    // When trust is low (ambiguous), reduce k_pos/k_neg dramatically
    // This keeps geometry heavy/stable when we don't trust the chroma
    float trust_scale = 0.15 + u_trust * 0.85;  // Range: 0.15 (low trust) to 1.0 (high trust)

    // Asymmetric gain: peaks (lobes) more prominent than valleys
    // Scaled by trust to prevent flopping in ambiguous state
    float k_pos = u_gain * 1.5 * trust_scale;  // Positive deviation (lobes)
    float k_neg = u_gain * 0.8 * trust_scale;  // Negative deviation (indentations)

    // === GEOMETRY BREATHING (energy-driven, NOT kick-driven) ===
    // Kick should NOT drive geometry - it causes flopping.
    // Use slow section_energy for breathing, which is stable.
    // GATED: No breathing during silence
    float energy_breath = 1.0 + u_section_energy * 0.03 * u_trust * breath_gate;

    float r = u_base_radius * energy_breath;
    r += k_pos * max(effective_dev, 0.0);
    r -= k_neg * max(-effective_dev, 0.0);

    return r;
}

// Check if index is a local maximum (for curvature emphasis)
float localMaximaBoost(int idx, float mean) {
    float c_prev = getChroma(idx - 1);
    float c_curr = getChroma(idx);
    float c_next = getChroma(idx + 1);

    // Is this a local maximum?
    if (c_curr > c_prev && c_curr > c_next && c_curr > mean) {
        // Boost based on how much it exceeds neighbors
        float dominance = min(c_curr - c_prev, c_curr - c_next);
        return dominance * 0.3;  // Subtle curvature sharpening
    }
    return 0.0;
}

// Get radius with Z-based surface modulation
// z_norm: -1 = back face (relaxed), 0 = middle (smoothed), +1 = front face (raw field)
//
// TRUST GATING: When trust is low, apply circular blur across bins
// to bias toward symmetry (prevents random lobes from noise)
float harmonicRadiusZ(float theta, float z_norm) {
    // === SMOOTH TRUST BLEND ===
    // Instead of a hard early-out (which causes strobing at threshold),
    // we compute low_trust_blend once and use it at the end to smoothly
    // interpolate toward base_radius. No discontinuity = no strobing.
    float low_trust_blend = smoothstep(0.0, 0.08, u_trust);

    // Early out only when trust is essentially zero (saves GPU when truly silent)
    if (low_trust_blend < 0.001) {
        return u_base_radius;
    }

    // Normalize theta to [0, TAU)
    float theta_norm = mod(theta, TAU);
    if (theta_norm < 0.0) theta_norm += TAU;

    // Map theta to bin index
    float bin_angle = TAU / 12.0;
    float idx_f = theta_norm / bin_angle;

    // Get surrounding bin indices
    int idx0 = int(floor(idx_f));
    int idx1 = idx0 + 1;
    int idx_m1 = idx0 - 1;
    int idx2 = idx0 + 2;

    // Fractional position between bins
    float t = fract(idx_f);

    // Compute mean chroma for deviation calculation
    float mean = chromaMean();

    // Get deviation-based radii for surrounding bins
    float r_m1 = deviationRadius(idx_m1, mean);
    float r0 = deviationRadius(idx0, mean);
    float r1 = deviationRadius(idx1, mean);
    float r2 = deviationRadius(idx2, mean);

    // === AMBIGUOUS MODE: CIRCULAR BLUR ===
    // When trust is low, blur neighboring radii toward each other
    // This biases toward symmetry and reduces random lobe formation
    float blur_strength = (1.0 - u_trust) * 0.7;  // Up to 70% blur when trust = 0
    if (blur_strength > 0.01) {
        // Simple 3-tap blur (neighbor averaging)
        float r0_blurred = r0 * (1.0 - blur_strength) + (r_m1 + r1) * 0.5 * blur_strength;
        float r1_blurred = r1 * (1.0 - blur_strength) + (r0 + r2) * 0.5 * blur_strength;
        r0 = r0_blurred;
        r1 = r1_blurred;
    }

    // Add local maxima boost (creates "petals" at peaks)
    // Scaled by trust - no sharp petals when we don't trust the signal
    float petal_scale = u_trust;
    r0 += localMaximaBoost(idx0, mean) * petal_scale;
    r1 += localMaximaBoost(idx1, mean) * petal_scale;

    // Catmull-Rom interpolation for smooth curve
    float r_raw = catmullRom(r_m1, r0, r1, r2, t);

    // Surface modulation along Z:
    // front (z_norm=1): raw field
    // middle (z_norm=0): smoothed
    // back (z_norm=-1): relaxed toward base
    float z_factor = (z_norm + 1.0) * 0.5;  // Map to [0, 1]
    float r_relaxed = mix(u_base_radius, r_raw, 0.3 + z_factor * 0.7);

    // === SMOOTH BLEND TO BASE AT LOW TRUST ===
    // Ensures no visual discontinuity when trust crosses thresholds
    // At trust=0: pure base_radius, at trust>=0.08: pure computed radius
    return mix(u_base_radius, r_relaxed, low_trust_blend);
}

// ============ 24-FOLD HARMONIC RADIUS ============
// Bin centers + bin boundaries model:
//   - 12 note centers: θn = n × 30° (where chroma energy lives)
//   - 12 seam midpoints: θn+½ = (n + 0.5) × 30° (where bins meet)
//
// 12-fold: Energy locks to centers (stable lobes)
// 24-fold: Energy also lights boundaries (interference lattice / harmonic seams)
//
// The seam value = geometric mean of adjacent bins (interference term)

float seamRadius(int bin_a, int bin_b, float mean) {
    // Seam between bin_a and bin_b shows their interference
    // Geometric mean: strong when BOTH adjacent notes are active
    float r_a = deviationRadius(bin_a, mean);
    float r_b = deviationRadius(bin_b, mean);

    // Geometric mean for interference (sqrt of product)
    // Scaled slightly lower than centers - seams are secondary structure
    float interference = sqrt(max(r_a * r_b, 0.0)) * 0.85;

    return interference;
}

float harmonicRadiusZ_24(float theta, float z_norm) {
    // Same trust gating as 12-fold
    float low_trust_blend = smoothstep(0.0, 0.08, u_trust);
    if (low_trust_blend < 0.001) {
        return u_base_radius;
    }

    // Normalize theta to [0, TAU)
    float theta_norm = mod(theta, TAU);
    if (theta_norm < 0.0) theta_norm += TAU;

    // === 24-FOLD: Centers + Seams ===
    // Map to 24 positions: even = center, odd = seam
    float bin_angle_12 = TAU / 12.0;  // 30 degrees
    float bin_angle_24 = TAU / 24.0;  // 15 degrees

    float idx_f_24 = theta_norm / bin_angle_24;
    int idx_24 = int(floor(idx_f_24));
    float t_24 = fract(idx_f_24);  // Position within 15° sector

    float mean = chromaMean();

    // Determine if we're near a center (even) or seam (odd)
    // idx_24 = 0,2,4... → centers at n×30°
    // idx_24 = 1,3,5... → seams at (n+0.5)×30°

    // Get the 4 surrounding 24-fold points for interpolation
    int i0 = idx_24;
    int i1 = idx_24 + 1;
    int i_m1 = idx_24 - 1;
    int i2 = idx_24 + 2;

    // Helper: get radius at a 24-fold index (center or seam)
    // Even indices → note centers, Odd indices → seams
    #define GET_R24(i) ( \
        ((i) & 1) == 0 \
            ? deviationRadius(((i) / 2), mean) \
            : seamRadius(((i) / 2), ((i) / 2) + 1, mean) \
    )

    // Wrap indices to [0, 23]
    int w_m1 = (i_m1 + 24) % 24;
    int w_0 = (i0 + 24) % 24;
    int w_1 = (i1 + 24) % 24;
    int w_2 = (i2 + 24) % 24;

    // Get radii at 4 surrounding 24-fold points
    float r_m1, r0, r1, r2;

    // Unroll the macro for GLSL compatibility
    if ((w_m1 & 1) == 0) { r_m1 = deviationRadius(w_m1 / 2, mean); }
    else { r_m1 = seamRadius(w_m1 / 2, (w_m1 / 2) + 1, mean); }

    if ((w_0 & 1) == 0) { r0 = deviationRadius(w_0 / 2, mean); }
    else { r0 = seamRadius(w_0 / 2, (w_0 / 2) + 1, mean); }

    if ((w_1 & 1) == 0) { r1 = deviationRadius(w_1 / 2, mean); }
    else { r1 = seamRadius(w_1 / 2, (w_1 / 2) + 1, mean); }

    if ((w_2 & 1) == 0) { r2 = deviationRadius(w_2 / 2, mean); }
    else { r2 = seamRadius(w_2 / 2, (w_2 / 2) + 1, mean); }

    #undef GET_R24

    // Apply blur at low trust (same as 12-fold)
    float blur_strength = (1.0 - u_trust) * 0.7;
    if (blur_strength > 0.01) {
        float r0_blurred = r0 * (1.0 - blur_strength) + (r_m1 + r1) * 0.5 * blur_strength;
        float r1_blurred = r1 * (1.0 - blur_strength) + (r0 + r2) * 0.5 * blur_strength;
        r0 = r0_blurred;
        r1 = r1_blurred;
    }

    // Catmull-Rom interpolation across the 24-fold points
    float r_raw = catmullRom(r_m1, r0, r1, r2, t_24);

    // Z-based surface modulation
    float z_factor = (z_norm + 1.0) * 0.5;
    float r_relaxed = mix(u_base_radius, r_raw, 0.3 + z_factor * 0.7);

    return mix(u_base_radius, r_relaxed, low_trust_blend);
}

// ============ BLENDED HARMONIC RADIUS (12→24 fold) ============
// Uses u_fold24_mix for smooth scaffold crossfade
// Performance: early-out when fully 12-fold or fully 24-fold

float harmonicRadiusZ_blended(float theta, float z_norm) {
    // Early out if fully 12-fold (no transition)
    if (u_fold24_mix < 0.01) {
        return harmonicRadiusZ(theta, z_norm);
    }

    // Early out if fully 24-fold (no transition)
    if (u_fold24_mix > 0.99) {
        return harmonicRadiusZ_24(theta, z_norm);
    }

    // Crossfade: render both only during transition window
    float r12 = harmonicRadiusZ(theta, z_norm);
    float r24 = harmonicRadiusZ_24(theta, z_norm);

    // Smooth blend using fold24_mix (deterministic from RingDive timeline)
    return mix(r12, r24, u_fold24_mix);
}

// Wrapper for simple 2D usage (z=0, middle of extrusion)
// Uses blended version to support 12→24 fold transition
float harmonicRadius(float theta) {
    return harmonicRadiusZ_blended(theta, 0.0);
}

// ============ 2D HARMONIC PROFILE SDF ============

// SDF of the 2D harmonic profile at given Z position
// Uses blended radius to support 12→24 fold topology transition
float sdHarmonicProfile2DZ(vec2 p, float z_norm) {
    float theta = atan(p.y, p.x);
    float r = length(p);
    float target_r = harmonicRadiusZ_blended(theta, z_norm);

    // Distance to profile boundary (negative inside, positive outside)
    return r - target_r;
}

// ============ 3D SDF MODES ============

// Extrusion: 2D profile extruded along Z with surface modulation
float sdExtrusion(vec3 p) {
    // Extrusion bounds in Z (centered at origin)
    float half_thick = u_thickness * 0.5;

    // Normalize Z position for surface modulation (-1 to +1)
    float z_norm = clamp(p.z / max(half_thick, 0.01), -1.0, 1.0);

    // 2D profile in XY plane, modulated by Z position
    float d2d = sdHarmonicProfile2DZ(p.xy, z_norm);

    float dz = abs(p.z) - half_thick;

    // Combined: max of profile distance and Z bounds
    vec2 w = vec2(d2d, dz);
    return min(max(w.x, w.y), 0.0) + length(max(w, 0.0));
}

// Lathe: 2D profile revolved around Y axis
// Uses blended radius to support 12→24 fold topology transition
float sdLathe(vec3 p) {
    // Distance from Y axis
    float r_xz = length(p.xz);

    // Angle around Y axis
    float theta = atan(p.z, p.x);

    // Normalize Y for surface modulation
    float half_height = u_thickness * 0.5;
    float y_norm = clamp(p.y / max(half_height, 0.01), -1.0, 1.0);

    // Get the expected radius at this angle, modulated by Y position
    float target_r = harmonicRadiusZ_blended(theta, y_norm);

    // Distance from the ideal tube surface
    float dr = r_xz - target_r;
    float dy = abs(p.y) - half_height;

    // Combine radial and vertical distances
    vec2 w = vec2(dr, dy);
    return min(max(w.x, w.y), 0.0) + length(max(w, 0.0));
}

// ============ 24-FOLD SYMMETRY ============

// Apply 24-fold symmetry by duplicating with 15-degree offset
vec3 apply24FoldSymmetry(vec3 p) {
    // First, get the base 12-fold position
    float theta = atan(p.z, p.x);

    // Fold into 30-degree sectors (12-fold)
    float sector = TAU / 12.0;
    float half_sector = sector * 0.5;

    // For 24-fold, we fold into 15-degree sectors
    float sector24 = TAU / 24.0;
    float folded_theta = mod(theta + sector24 * 0.5, sector24) - sector24 * 0.5;

    float r = length(p.xz);
    return vec3(r * cos(folded_theta), p.y, r * sin(folded_theta));
}

// ============ WARP JUMP QS GEOMETRY DEFORMATION ============
// Advects geometry toward portal during Acquire/Tunnel, expands during Burst
// The geometry bends toward the QS wheel portal center

vec3 warpGeometry(vec3 p) {
    if (u_warp_active == 0 || u_warp_strength < 0.001) return p;

    // Radial distance from center (XZ plane)
    float r = length(p.xz);

    // === PULL TOWARD PORTAL ===
    // During Acquire and Tunnel, geometry bends toward center
    if (u_warp_phase == WARP_PHASE_ACQUIRE || u_warp_phase == WARP_PHASE_TUNNEL) {
        float pullFactor = u_warp_strength;

        // Inverse-square falloff - closer objects feel more pull
        if (r > 0.01) {
            vec2 pullDir = -normalize(p.xz);
            float pullStrength = pullFactor * (1.0 / (1.0 + r * 2.0));
            pullStrength = min(pullStrength, 0.85);
            p.xz += pullDir * pullStrength * r * 0.5;
        }

        // Also compress vertically during tunnel
        if (u_warp_phase == WARP_PHASE_TUNNEL) {
            float compressFactor = 1.0 - u_warp_strength * 0.3;
            p.y *= compressFactor;
        }
    }

    // === BURST EXPANSION ===
    // During Burst, geometry expands outward from center via ripple
    if (u_warp_phase == WARP_PHASE_BURST && u_warp_ripple > 0.01) {
        float rippleRadius = u_warp_ripple * 2.0;
        float distFromRipple = r - rippleRadius;

        // Inside ripple = pushed outward slightly (new scene expanding)
        if (distFromRipple < 0.0 && r > 0.01) {
            vec2 pushDir = normalize(p.xz);
            float pushStrength = -distFromRipple * 0.2 * (1.0 - u_warp_ripple);
            p.xz += pushDir * pushStrength;
        }
    }

    return p;
}

// ============ SCENE SDF ============

// Coarse SDF for two-phase marching (PERF OPTIMIZATION)
// Uses simple sphere approximation - much cheaper than full harmonic field
// Used for initial marching until we get close to surface
float sceneSDF_coarse(vec3 p) {
    // Apply rotation only (skip warp, noise, symmetry folding)
    p = rotateY(u_rotation) * p;

    // Simple sphere SDF using base radius
    // Accounts for gain variation
    float r = u_base_radius * (1.0 + u_gain * 0.3);

    return length(p) - r;
}

// Threshold distance to switch from coarse to full SDF
#define REFINE_THRESHOLD 0.5

float sceneSDF(vec3 p) {
    // === WARP JUMP GEOMETRY DEFORMATION ===
    // Apply warp pull BEFORE rotation and folding
    // This makes geometry flow toward center during transition
    p = warpGeometry(p);

    // Apply slow base rotation (geological, not rhythmic)
    p = rotateY(u_rotation) * p;

    // === NOISE MOTION LAYER ===
    // When harmony is absent, energy drives motion (not shape)
    // This keeps the field alive without inventing pitch
    float noise_factor = 1.0 - u_tonal_confidence;
    if (noise_factor > 0.2) {
        // Inject low-amplitude rotational energy
        float noise_spin = noise_factor * 0.15;
        float spin_angle = u_time * noise_spin;
        float cs = cos(spin_angle);
        float sn = sin(spin_angle);
        p.xz = vec2(cs * p.x - sn * p.z, sn * p.x + cs * p.z);

        // Add subtle wobble/precession
        float wobble = sin(u_time * 0.7) * noise_factor * 0.05;
        p.xy = vec2(cos(wobble) * p.x - sin(wobble) * p.y,
                    sin(wobble) * p.x + cos(wobble) * p.y);
    }

    // Apply symmetry if 24-fold mode
    vec3 q = p;
    if (u_symmetry_mode == 24) {
        q = apply24FoldSymmetry(p);
    }

    float d;

    // Select render mode
    if (u_render_mode == 0) {
        // Extrusion only
        d = sdExtrusion(q);
    } else if (u_render_mode == 1) {
        // Lathe only
        d = sdLathe(q);
    } else {
        // Both: combine with smooth union
        float d_ext = sdExtrusion(q);
        float d_lathe = sdLathe(q);

        // Offset lathe slightly in Y for visual separation
        vec3 q_offset = q;
        q_offset.y -= u_thickness * 0.8;
        d_lathe = sdLathe(q_offset);

        // Smooth minimum for blending
        float k = 0.3;
        float h = max(k - abs(d_ext - d_lathe), 0.0) / k;
        d = min(d_ext, d_lathe) - h * h * k * 0.25;
    }

    return d;
}

// ============ INTERIOR RENDERING ============
// When camera is inside the SDF, render volumetric interior with chopped video

// Forward declarations (these functions are defined later in the lighting section)
vec3 hue2rgb(float h);
vec3 getNormal(vec3 p);

// Hash functions for stable cell IDs
float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

float hash31(vec3 p) {
    p = fract(p * vec3(0.1031, 0.1030, 0.0973));
    p += dot(p, p.yxz + 33.33);
    return fract((p.x + p.y) * p.z);
}

vec3 hash33(vec3 p) {
    p = fract(p * vec3(0.1031, 0.1030, 0.0973));
    p += dot(p, p.yxz + 33.33);
    return fract((p.xxy + p.yxx) * p.zyx);
}

// Mandelbox-ish fold for fractal interior structure
vec3 fractalFold(vec3 p, int iters) {
    for (int i = 0; i < 6; i++) {
        if (i >= iters) break;

        // Box fold
        p = clamp(p, -1.0, 1.0) * 2.0 - p;

        // Sphere fold
        float r2 = dot(p, p);
        if (r2 < 0.25) {
            p *= 4.0;
        } else if (r2 < 1.0) {
            p /= r2;
        }

        // Scale and translate
        p = p * 1.5 - vec3(0.5, 0.5, 0.8);
    }
    return p;
}

// Pre-computed 12 harmonic directions (saves 24 trig calls per harmonicDensity)
// Direction = normalize(vec3(cos(i*TAU/12), 0.35, sin(i*TAU/12)))
const vec3 HARMONIC_DIRS[12] = vec3[12](
    vec3(0.9400, 0.3290, 0.0000),    // i=0:  cos(0)=1.0, sin(0)=0.0
    vec3(0.8141, 0.3290, 0.4701),    // i=1:  cos(30°)=0.866, sin(30°)=0.5
    vec3(0.4701, 0.3290, 0.8141),    // i=2:  cos(60°)=0.5, sin(60°)=0.866
    vec3(0.0000, 0.3290, 0.9400),    // i=3:  cos(90°)=0, sin(90°)=1
    vec3(-0.4701, 0.3290, 0.8141),   // i=4:  cos(120°)=-0.5, sin(120°)=0.866
    vec3(-0.8141, 0.3290, 0.4701),   // i=5:  cos(150°)=-0.866, sin(150°)=0.5
    vec3(-0.9400, 0.3290, 0.0000),   // i=6:  cos(180°)=-1, sin(180°)=0
    vec3(-0.8141, 0.3290, -0.4701),  // i=7:  cos(210°)=-0.866, sin(210°)=-0.5
    vec3(-0.4701, 0.3290, -0.8141),  // i=8:  cos(240°)=-0.5, sin(240°)=-0.866
    vec3(0.0000, 0.3290, -0.9400),   // i=9:  cos(270°)=0, sin(270°)=-1
    vec3(0.4701, 0.3290, -0.8141),   // i=10: cos(300°)=0.5, sin(300°)=-0.866
    vec3(0.8141, 0.3290, -0.4701)    // i=11: cos(330°)=0.866, sin(330°)=-0.5
);

// Harmonic density field - chroma defines a periodic lattice
// 12 directions around a circle, each contributes a sinusoid
float harmonicDensity(vec3 p) {
    float d = 0.0;
    float totalWeight = 0.0;

    // Pre-compute time offset (saves 12 additions)
    float timeOffset = u_time * 0.15;

    for (int i = 0; i < 12; i++) {
        // Use pre-computed direction (saves cos/sin per iteration)
        vec3 dir = HARMONIC_DIRS[i];
        float w = getChroma(i);
        totalWeight += w;

        // Sinusoidal lattice contribution
        float lattice = sin(dot(p, dir) * u_interior_lattice_freq + timeOffset);
        d += w * lattice;
    }

    // Normalize and create density
    if (totalWeight > 0.01) {
        d /= totalWeight;
    }
    d = abs(d);

    // Apply density scale
    d *= u_interior_density_scale;

    return smoothstep(0.25, 0.75, d);
}

// Get 3D cell info for chopped video (output parameters for GLSL 140 compatibility)
void getCellInfo(vec3 p, out vec3 cellId, out float id, out vec3 localPos, out float edgeDist) {
    float cellSize = max(u_interior_cell_size, 0.1);

    cellId = floor(p / cellSize);
    id = hash31(cellId);
    localPos = fract(p / cellSize);

    // Distance to nearest edge (for seams)
    vec3 edgeDists = min(localPos, 1.0 - localPos);
    edgeDist = min(min(edgeDists.x, edgeDists.y), edgeDists.z);
}

// Sample chopped video with per-cell UV transform
vec3 sampleChoppedVideo(vec3 p, vec3 n, vec3 cellId, float cellHash, vec3 localPos) {
    if (u_skin_has_texture == 0) {
        // No texture: return procedural color based on cell
        vec3 procColor = hash33(cellId);
        return mix(vec3(0.2), procColor, 0.5);
    }

    // Per-cell rotation
    float rot = cellHash * TAU;
    mat2 R = mat2(cos(rot), -sin(rot), sin(rot), cos(rot));

    // Tri-planar blend weights
    vec3 blend = pow(abs(n), vec3(4.0));
    blend /= (blend.x + blend.y + blend.z + 0.0001);

    // Sample from each axis with per-cell UV offset
    vec3 offset = vec3(fract(cellHash * 17.0), fract(cellHash * 31.0), fract(cellHash * 47.0));
    float uvScale = 0.5 + cellHash * 0.5;  // Vary scale per cell

    // Time-based drift
    vec2 drift = u_time * vec2(0.02, 0.015) * (0.5 + cellHash);

    // X-axis projection (YZ plane)
    vec2 uvX = (localPos.yz - 0.5);
    uvX = R * uvX * uvScale + offset.xy + drift;
    vec3 colX = texture(u_skin_tex, fract(uvX)).rgb;

    // Y-axis projection (XZ plane)
    vec2 uvY = (localPos.xz - 0.5);
    uvY = R * uvY * uvScale + offset.xz + drift;
    vec3 colY = texture(u_skin_tex, fract(uvY)).rgb;

    // Z-axis projection (XY plane)
    vec2 uvZ = (localPos.xy - 0.5);
    uvZ = R * uvZ * uvScale + offset.yz + drift;
    vec3 colZ = texture(u_skin_tex, fract(uvZ)).rgb;

    // Blend by normal
    vec3 col = colX * blend.x + colY * blend.y + colZ * blend.z;

    // Lift mids and boost
    col = pow(col, vec3(0.8));
    col *= u_interior_video_boost;

    return col;
}

// ============ 24-FOLD ANGULAR SEAM SYSTEM ============
// Computes seam glow based on 12 or 24 angular divisions around the Y axis
// Used for interior field lines that reveal harmonic structure

// Distance to nearest angular seam line (0 to 1 range)
// Returns 0 at seam center, ~1 at maximum distance from seams
float angularSeamDist12(vec3 p) {
    // Angular position around Y axis
    float theta = atan(p.z, p.x);  // -PI to PI

    // 12-fold: seams every PI/6 (30 degrees)
    float sector = mod(theta + PI, PI / 6.0);
    float distToSeam = min(sector, PI / 6.0 - sector);

    // Normalize to 0-1 (0 at seam, 1 at mid-sector)
    return distToSeam / (PI / 12.0);
}

float angularSeamDist24(vec3 p) {
    // Angular position around Y axis
    float theta = atan(p.z, p.x);  // -PI to PI

    // 24-fold: seams every PI/12 (15 degrees)
    // This is octave duplication: each 12-fold direction splits into two
    float sector = mod(theta + PI, PI / 12.0);
    float distToSeam = min(sector, PI / 12.0 - sector);

    // Normalize to 0-1 (0 at seam, 1 at mid-sector)
    return distToSeam / (PI / 24.0);
}

// Unified seam calculation with 12→24 fold transition
// Returns seam intensity (0 = no seam, 1 = seam center)
float computeAngularSeam(vec3 p, float seamWidth) {
    float dist12 = angularSeamDist12(p);
    float dist24 = angularSeamDist24(p);

    // === FOLD24 TRANSITION DYNAMICS ===
    // fold24_t [0..1] controls the transition:
    // 0.0-0.3: 12-fold, seams begin tightening
    // 0.3-0.6: constriction (narrow seams, building tension)
    // 0.6-0.9: interpolate toward 24-fold
    // 0.9-1.0: snap to crisp 24 petals

    float t = u_fold24_t;

    if (u_inside_mode == INSIDE_MODE_12FOLD) {
        // Pure 12-fold
        return smoothstep(seamWidth, seamWidth * 0.1, dist12);
    }

    // === TRANSITION PHASES ===
    float seam12 = smoothstep(seamWidth, seamWidth * 0.1, dist12);
    float seam24 = smoothstep(seamWidth, seamWidth * 0.1, dist24);

    // Phase 1 (t < 0.3): Pure 12-fold, slight tightening
    if (t < 0.3) {
        float tightness = 1.0 - t * 0.5;  // Seams tighten slightly
        float tightWidth = seamWidth * tightness;
        return smoothstep(tightWidth, tightWidth * 0.1, dist12);
    }

    // Phase 2 (0.3 <= t < 0.6): Constriction - seams narrow dramatically
    if (t < 0.6) {
        float phase = (t - 0.3) / 0.3;  // 0 to 1 within phase
        float tightness = 0.85 - phase * 0.45;  // 0.85 → 0.4 (very tight)
        float tightWidth = seamWidth * tightness;
        float seam = smoothstep(tightWidth, tightWidth * 0.05, dist12);
        // Start showing hints of 24-fold
        float hint24 = smoothstep(tightWidth * 0.5, tightWidth * 0.05, dist24) * phase * 0.3;
        return max(seam, hint24);
    }

    // Phase 3 (0.6 <= t < 0.9): Interpolate 12→24 fold
    if (t < 0.9) {
        float phase = (t - 0.6) / 0.3;  // 0 to 1 within phase
        float blend = smoothstep(0.0, 1.0, phase);
        // Width recovers as we transition
        float width = seamWidth * (0.4 + blend * 0.6);
        seam12 = smoothstep(width, width * 0.1, dist12);
        seam24 = smoothstep(width, width * 0.1, dist24);
        return mix(seam12, seam24, blend);
    }

    // Phase 4 (t >= 0.9): Snap to crisp 24 petals
    float snapPhase = (t - 0.9) / 0.1;  // 0 to 1 in final 10%
    float crispness = smoothstep(0.0, 1.0, snapPhase);
    // Sharp, clean 24-fold seams
    float crispWidth = seamWidth * (1.0 + crispness * 0.2);  // Slightly wider for clarity
    seam24 = smoothstep(crispWidth, crispWidth * 0.05 * (1.0 + crispness), dist24);
    // Add extra glow at snap moment
    float snapGlow = (1.0 - abs(snapPhase - 0.5) * 2.0) * 0.3;
    return seam24 + snapGlow * seam24;
}

// Render interior SHELL-BASED pass (3-5 iso-shells)
// Each shell is a discrete spherical surface with video tiles
// Seams glow where shells intersect ray
// Now with 12-fold → 24-fold transition support
// During unfold24: shells-only, no volumetric, dramatic seam boost
// Returns accumulated color and alpha
vec4 renderInterior(vec3 ro, vec3 rd, float maxDist, float interiorBlend) {
    vec3 acc = vec3(0.0);
    float accumAlpha = 0.0;

    // === TRANSITION MODE DETECTION ===
    // During soft transition (interiorBlend < 0.5), use cheaper rendering
    bool transitionMode = (interiorBlend < 0.5);

    // === UNFOLD24 INTENSITY (from CPU, overlap-eased) ===
    // u_unfold24_intensity = smoothstep(0.56,0.64,t) * (1-smoothstep(0.80,0.88,t))
    float unfoldI = u_unfold24_intensity;

    // === INSIDE 24-FOLD QUALITY SYSTEM (PERF OPTIMIZATION) ===
    // Uses new quality tier uniforms for explicit budget control
    // Shell count, fog, sparkle, labels all gated by quality tier
    bool lowQuality = (u_inside_quality_tier == 0);

    // === SHELL CONFIGURATION ===
    // Shells at fixed radii, count controlled by u_inside_shell_count
    float shellRadii[5];
    shellRadii[0] = 0.12;
    shellRadii[1] = 0.22;
    shellRadii[2] = 0.32;
    shellRadii[3] = 0.42;
    shellRadii[4] = 0.52;

    // Shell count: REDUCED during transition for perf (REG-safe: still real shells)
    // Transition: 2 shells max, Unfold: 3 max, Full: quality tier
    int shellCount = transitionMode ? 2 :
                     (unfoldI > 0.5) ? min(u_inside_shell_count, 3) : u_inside_shell_count;

    // Cell size for video tiling (BIG so video is readable)
    // Low quality: larger cells (fewer samples)
    float cellSize = lowQuality ? max(u_interior_cell_size * 1.5, 1.5) :
                     max(u_interior_cell_size, 1.0);

    // Dominant pitch hue for seam glow
    float seamHue = float(u_dom_pc) / 12.0;
    vec3 baseSeamColor = hue2rgb(seamHue) * 1.8;
    baseSeamColor = mix(vec3(1.0), baseSeamColor, u_tonal_confidence);

    // During unfold: BOOST seam color dramatically (2.5x emissive)
    vec3 seamColor = baseSeamColor * (1.0 + unfoldI * 1.5);

    // Track best video contribution (avoid mush)
    vec3 bestVideo = vec3(0.0);
    float bestWeight = 0.0;

    // === 24-TICK CROWN (unmistakable 24-fold indicator) ===
    // Quality-gated: Low=skip, Med=simple, High=full
    // This is THE visual proof that 24-fold is active
    float crownGlow = 0.0;
    vec3 crownColor = seamColor * 2.5;

    // === CACHE ATAN for reuse (computed once, used in crown + shell loop) ===
    float theta_rd = atan(rd.z, rd.x);

    if (unfoldI > 0.05 && u_inside_quality_tier >= 1 && !transitionMode) {
        // Skip crown during transition for perf

        // 24 spokes - CHEAP VERSION: smoothstep instead of pow^18
        // Same crisp look, much cheaper math
        float s = abs(sin(theta_rd * 24.0));
        float spoke24 = (u_inside_quality_tier >= 2) ?
            smoothstep(0.85, 1.0, s) :   // High: crisp
            smoothstep(0.7, 1.0, s);      // Med: softer

        // Ring radius band - crown appears at specific depth
        float viewDist = length(ro.xz);
        float crownRadius = 0.25;
        float crownWidth = 0.08;
        float ring = exp(-(viewDist - crownRadius) * (viewDist - crownRadius) / (crownWidth * crownWidth));

        // Second crown only at High quality (extra overdraw)
        if (u_inside_quality_tier >= 2) {
            float ring2 = exp(-(viewDist - 0.4) * (viewDist - 0.4) / (0.06 * 0.06));
            ring = max(ring, ring2 * 0.7);
        }

        // 3-beat unfold: focus → split → lock
        float focus = smoothstep(0.0, 0.3, unfoldI);
        float split = smoothstep(0.3, 0.7, unfoldI);
        float lock = smoothstep(0.7, 1.0, unfoldI);

        // Crown intensity ramps up during split, stabilizes at lock
        float crownIntensity = focus * 0.3 + split * 0.7 + lock * 0.5;

        // Final crown glow: spokes * ring * intensity
        crownGlow = spoke24 * ring * crownIntensity * 2.0;

        // Full-spoke lines only at High quality - also use smoothstep
        if (u_inside_quality_tier >= 2) {
            float fullSpokes = smoothstep(0.6, 1.0, s) * 0.15 * split;
            crownGlow = max(crownGlow, fullSpokes);
        }
    }

    // === RAY-SPHERE INTERSECTIONS FOR EACH SHELL ===
    // We're inside looking out, so we want the "exit" intersection
    float b = dot(ro, rd);
    float c_base = dot(ro, ro);

    // === ALPHA BUDGET EARLY EXIT (PERF OPTIMIZATION) ===
    // More aggressive during transition for perf
    // Transition: 0.5, Unfold: 0.6, Full: configurable
    float alphaBudget = transitionMode ? 0.5 :
                        (unfoldI > 0.5) ? 0.6 : min(u_ring_dive_alpha_budget, 0.92);

    for (int s = 0; s < 5; s++) {
        if (s >= shellCount) break;
        // Early exit when alpha budget reached - saves shell iterations
        if (accumAlpha >= alphaBudget) break;

        float r = shellRadii[s];
        float c = c_base - r * r;
        float disc = b * b - c;

        if (disc < 0.0) continue;  // Ray misses this shell

        // Get both intersections (we're inside, so use the positive one)
        float sqrtDisc = sqrt(disc);
        float t1 = -b - sqrtDisc;
        float t2 = -b + sqrtDisc;

        // Use the forward intersection
        float t = (t1 > 0.001) ? t1 : ((t2 > 0.001) ? t2 : -1.0);
        if (t < 0.0 || t > maxDist) continue;

        vec3 p = ro + rd * t;
        p = rotateY(u_rotation) * p;
        vec3 n = normalize(p);

        // === VIDEO TILE SAMPLING ===
        vec3 cellP = p / cellSize;
        vec3 cellId = floor(cellP);
        float cellHash = hash31(cellId);
        vec3 localPos = fract(cellP);

        // === VIDEO COLOR (transition-optimized) ===
        vec3 videoCol;
        float theta_p;

        if (transitionMode) {
            // CHEAP: Skip texture lookup during transition
            // Use procedural chroma color (REG-compliant: still responds to harmony)
            vec3 domColor = hue2rgb(float(u_dom_pc) / 12.0);
            videoCol = domColor * (0.5 + cellHash * 0.5);
            theta_p = theta_rd;  // Reuse cached value
        } else {
            // FULL: Texture lookup with quadripolar segmentation
            theta_p = atan(p.z, p.x);
            if (theta_p < 0.0) theta_p += 2.0 * PI;

            // Get wedge ID and apply UV offset during quadripolar travel
            int wedgeID = getWedgeID(theta_p);
            vec2 wedgeOffset = vec2(0.0);
            if (u_quad_active > 0.5 && u_quad_wedge_intensity > 0.01) {
                wedgeOffset = getWedgeUVOffset(wedgeID) * u_quad_wedge_intensity;
                cellId.xy += wedgeOffset * 2.0;
                cellHash = hash31(cellId);
            }

            videoCol = sampleChoppedVideo(p, n, cellId, cellHash, localPos);
            videoCol = mix(vec3(0.08), videoCol, u_video_visibility);

            // Boost contrast on video for clarity
            videoCol = (videoCol - 0.25) * 1.4 + 0.25;
            videoCol = clamp(videoCol, 0.0, 1.0);

            // Quadripolar video density modulation
            if (u_quad_active > 0.5) {
                videoCol *= u_quad_video_density;
            }
        }

        // Track best video
        float videoLum = dot(videoCol, vec3(0.299, 0.587, 0.114));
        if (videoLum > bestWeight) {
            bestWeight = videoLum;
            bestVideo = videoCol;
        }

        // === UNFOLD24: AGGRESSIVE VIDEO REDUCTION ===
        // During unfold: interior_video_boost *= 0.15, alpha *= 0.2
        // This makes 24-fold structure unmistakable
        float videoMult = 1.0 - unfoldI * 0.85;  // 1.0 → 0.15 during unfold
        videoCol *= videoMult;

        // === QUADRIPOLAR WEDGE BOUNDARY MASK ===
        // During wedge segmentation, video fades at wedge boundaries
        if (u_quad_active > 0.5 && u_quad_wedge_intensity > 0.01) {
            float wedgeMask = getWedgeMask(theta_p, u_quad_wedge_intensity);
            videoCol *= wedgeMask;
        }

        // === SEAM GLOW (combined cell edges + angular field lines) ===
        // Cell edge seams (cubic lattice structure)
        vec3 edgeDist3 = min(localPos, 1.0 - localPos);
        float edgeDist = min(min(edgeDist3.x, edgeDist3.y), edgeDist3.z);
        float cellSeamWidth = 0.08;
        float cellSeam = smoothstep(cellSeamWidth, 0.005, edgeDist);

        // Angular seams (12-fold or 24-fold harmonic structure)
        // MUCH WIDER during unfold so petals read clearly
        float angularSeamWidth = 0.12 + unfoldI * 0.12;  // 0.12 → 0.24 during unfold
        float angularSeam = computeAngularSeam(p, angularSeamWidth);

        // === 24-FOLD ANGULAR EMISSIVE MODULATION ===
        // Creates "emergent order" - quality-gated for performance
        float theta = atan(p.z, p.x);
        float wave24_soft = 0.5 + 0.5 * cos(24.0 * theta);
        // Crisp 24 petals only at Med+ quality (pow^4 is expensive)
        float wave24 = wave24_soft;
        if (u_inside_quality_tier >= 1 && unfoldI > 0.1) {
            float wave24_crisp = wave24_soft * wave24_soft;  // Cheaper than pow^4
            wave24 = mix(wave24_soft, wave24_crisp, unfoldI);
        }
        // Strong modulation during unfold
        float emissiveMod = 1.0 + wave24 * unfoldI * 1.2;

        // Combine seams: angular seams DOMINATE during unfold
        float cellSeamFade = 1.0 - unfoldI * 0.8;  // Cell seams fade during unfold
        float seam = max(cellSeam * cellSeamFade, angularSeam);

        // === UNFOLD24: BOOST SEAM VISIBILITY ===
        // seam emissive *= 2.5, bloom += 0.2 during unfold
        float seamBoost = 1.0 + unfoldI * 1.5;  // 1.0 → 2.5 during unfold
        seam *= seamBoost;
        seam = clamp(seam, 0.0, 1.0);

        // === FOLD24 VISUAL ENHANCEMENT ===
        // During 24-fold transition, boost angular seam visibility
        if (u_inside_mode == INSIDE_MODE_24FOLD) {
            // Snap phase (t >= 0.85): extra glow on angular seams + bloom
            float snapPhase = smoothstep(0.85, 1.0, u_fold24_t);
            float snapBoost = snapPhase * 0.4;
            seam = max(seam, angularSeam * (1.0 + snapBoost));

            // Snap "click" - brief brightness pulse at t ≈ 0.9
            float snapClick = exp(-pow((u_fold24_t - 0.92) * 15.0, 2.0)) * 0.5;
            emissiveMod += snapClick;

            // 24-fold seams get secondary color (octave shift)
            // Hue rotated for contrast - the "doubling" is visible in color
            float octaveHue = fract(seamHue + 0.083);  // +1/12 = next semitone
            vec3 octaveColor = hue2rgb(octaveHue) * 1.8;
            octaveColor = mix(vec3(1.0), octaveColor, u_tonal_confidence);
            // Blend more strongly as t increases
            seamColor = mix(seamColor, octaveColor, angularSeam * u_fold24_t * 0.6);
        }

        // === QUADRIPOLAR SPOKE FLASH ===
        // During quadripolar travel unfold phase, briefly flash 24 radial spokes
        if (u_quad_active > 0.5 && u_quad_spoke_flash > 0.01) {
            float spokeFlash = getSpokeFlash(theta, u_quad_spoke_flash);

            // Spoke color: bright cyan/white for structural reveal
            vec3 spokeColor = mix(vec3(0.8, 1.0, 1.0), vec3(1.0), spokeFlash);

            // Add spoke to seam glow
            seam = max(seam, spokeFlash);
            seamColor = mix(seamColor, spokeColor, spokeFlash * 0.5);

            // Extra emissive during spoke flash
            emissiveMod += spokeFlash * 0.8;
        }

        // === QUADRIPOLAR PORT HIGHLIGHTING ===
        // During refract phase, highlight the quaternion ports
        if (u_quad_active > 0.5 && u_quad_phase >= QUAD_PHASE_REFRACT && u_quad_phase < QUAD_PHASE_RESOLVE) {
            float portProx = nearPort(theta, 0.15);  // ~8.5° tolerance

            // Port glow - golden/amber for quaternion symmetry
            vec3 portColor = vec3(1.0, 0.85, 0.4) * 1.5;
            float portGlow = portProx * 0.4;

            seamColor = mix(seamColor, portColor, portGlow * u_quad_energy_burst);
            emissiveMod += portGlow * 0.5;
        }

        // === SHELL CONTRIBUTION ===
        // Inner shells are more transparent, outer shells more opaque
        float shellDepth = float(s) / float(shellCount - 1);
        float baseAlpha = 0.12 + shellDepth * 0.08;  // 0.12 to 0.20
        float seamAlpha = 0.35;

        // Boost alpha during fold24 snap for dramatic reveal
        float fold24AlphaBoost = (u_inside_mode == INSIDE_MODE_24FOLD)
            ? smoothstep(0.7, 0.95, u_fold24_t) * 0.2 : 0.0;
        seamAlpha += fold24AlphaBoost;

        vec3 shellCol = videoCol * 0.8 * emissiveMod;
        shellCol = mix(shellCol, seamColor * emissiveMod, seam * 0.7);

        float shellAlpha = mix(baseAlpha, seamAlpha, seam);

        // === UNFOLD24: REDUCE SHELL ALPHA ===
        // alpha *= 0.2 during unfold for shells-only feel
        float alphaReduction = 1.0 - unfoldI * 0.8;  // 1.0 → 0.2 during unfold
        shellAlpha *= alphaReduction;

        // === ALPHA COMPOSITING (front-to-back) ===
        float contrib = shellAlpha * (1.0 - accumAlpha);
        acc += shellCol * contrib;
        accumAlpha += contrib;
    }

    // === 24-SPOKE DEBUG LINES ===
    // Add explicit radial spokes during unfold to confirm 24-fold is ON
    // === ADD 24-TICK CROWN ===
    // This is the unmistakable visual proof of 24-fold
    if (crownGlow > 0.01) {
        acc = mix(acc, crownColor, crownGlow * (1.0 - accumAlpha));
        accumAlpha += crownGlow * 0.4;
    }

    // Blend in best video - REDUCED during unfold
    float videoBlend = 0.25 * (1.0 - unfoldI * 0.9);  // 0.25 → 0.025 during unfold
    if (bestWeight > 0.1 && videoBlend > 0.01) {
        acc = mix(acc, bestVideo * 0.65, videoBlend * (1.0 - accumAlpha * 0.5));
    }

    // Energy modulation (subtle)
    acc *= (0.7 + u_energy * 0.3);

    // === UNFOLD24: BLOOM BOOST ===
    // bloom += 0.2 during unfold for dramatic glow
    float bloom = unfoldI * 0.2;
    acc += acc * bloom;

    return vec4(acc, accumAlpha);
}

// ============ RAYMARCHING ============

// Ray-sphere intersection test for early-out optimization
// Returns true if ray could potentially hit geometry within MAX_DIST
// This is a visibility optimization that preserves REG math:
// - The actual geometry (harmonicRadiusZ) is unchanged
// - We only skip rays that mathematically cannot reach the field
bool rayHitsBoundingSphere(vec3 ro, vec3 rd, float radius) {
    // Ray-sphere intersection: solve |ro + t*rd|^2 = r^2
    // t^2 + 2*b*t + c = 0 where b = dot(ro,rd), c = |ro|^2 - r^2
    float b = dot(ro, rd);
    float c = dot(ro, ro) - radius * radius;
    float disc = b * b - c;

    // disc < 0: ray misses sphere entirely
    if (disc < 0.0) return false;

    // Check if intersection is within MAX_DIST
    float t = -b - sqrt(disc);  // Nearest intersection
    if (t < 0.0) t = -b + sqrt(disc);  // Ray starts inside or behind

    return t >= 0.0 && t < MAX_DIST;
}

// Dynamic step count based on quality tier + interior state + foveation
int getMaxSteps() {
    // Base steps by quality tier
    int baseSteps = (u_quality_tier == 0) ? 24 :   // Low
                    (u_quality_tier == 1) ? 40 :   // Med
                    56;                             // High

    // Further reduce when inside (interior rendering active)
    float qualityMod = mix(0.6, 1.0, u_interior_quality);

    // === PRE-TORUS TRANSITION STEP REDUCTION ===
    // During 24-fold → torus transition, fewer steps needed
    // Only when: high fold24_t, not inside, not warping, torus about to activate
    if (u_fold24_t > 0.5 && u_interior_latch < 0.5 && u_warp_active == 0 && u_torus_active == 1) {
        qualityMod *= 0.6;  // 40% fewer steps during pre-torus transition
    }

    // === FOVEATED STEP REDUCTION (PERF OPTIMIZATION) ===
    // Full quality at screen center, reduced at edges (40% at corners)
    // Uses gl_FragCoord for screen-space position
    vec2 screenUV = gl_FragCoord.xy / u_resolution;
    vec2 centered = screenUV - vec2(0.5);  // -0.5 to 0.5 range
    float distFromCenter = length(centered) * 1.414;  // 0 at center, ~1 at corners

    // Smooth falloff: 100% quality in center 20%, down to 50% at edges
    float foveateMod = mix(1.0, 0.5, smoothstep(0.2, 0.7, distFromCenter));

    return int(float(baseSteps) * qualityMod * foveateMod);
}

// Forward-differencing raymarch with gradient accumulation
// Returns: vec4(distance, steps_norm, 0, 0) - gradient stored in global
// Saves 4 SDF calls for normal calculation!
vec3 g_accum_gradient;  // Accumulated gradient from raymarch

vec2 rayMarch(vec3 ro, vec3 rd) {
    int steps = 0;
    int maxSteps = getMaxSteps();

    // === BOUNDING SPHERE EARLY EXIT (PERF OPTIMIZATION) ===
    // Skip empty space - start marching at sphere entry, stop at exit
    // Uses generous radius to account for warp, gain, and deformation
    float boundRadius = u_base_radius * 1.5 + u_gain * 0.3;

    // Ray-sphere intersection: |ro + t*rd|^2 = r^2
    // Solve: t^2 + 2*b*t + c = 0 where b = dot(ro,rd), c = |ro|^2 - r^2
    float b = dot(ro, rd);
    float c = dot(ro, ro) - boundRadius * boundRadius;
    float disc = b * b - c;

    if (disc < 0.0) {
        // Ray misses bounding sphere entirely - immediate miss
        g_accum_gradient = vec3(0.0);
        return vec2(MAX_DIST + 1.0, 0.0);
    }

    float sqrtDisc = sqrt(disc);
    float t_enter = -b - sqrtDisc;  // Entry point
    float t_exit = -b + sqrtDisc;   // Exit point

    // Start marching at entry (if ahead of us), else from origin
    float d = max(t_enter, 0.0);

    // Cap max distance at sphere exit (don't march beyond it)
    float maxD = min(t_exit, MAX_DIST);

    // Adaptive step scale: larger steps when far from surface
    float stepScale = (u_quality_tier == 0) ? 1.2 : 1.0;

    // Forward differencing: accumulate gradient during march
    g_accum_gradient = vec3(0.0);
    vec3 prevP = ro + rd * d;

    // === TWO-PHASE MARCHING (PERF OPTIMIZATION) ===
    // Phase 1: Coarse march with cheap SDF until near surface
    // Phase 2: Refine with full SDF only when close
    bool in_refine_phase = false;
    float prevSDF = sceneSDF_coarse(prevP);

    for (int i = 0; i < MAX_STEPS; i++) {
        if (i >= maxSteps) break;
        steps = i;
        vec3 p = ro + rd * d;

        // Switch to full SDF when close to surface
        float ds;
        if (!in_refine_phase && prevSDF < REFINE_THRESHOLD) {
            in_refine_phase = true;
        }

        if (in_refine_phase) {
            ds = sceneSDF(p);  // Full expensive SDF
        } else {
            ds = sceneSDF_coarse(p);  // Cheap sphere approximation
        }

        // Accumulate gradient: direction of SDF decrease
        // Weight by proximity to surface (near-surface samples matter more)
        // Only accumulate in refine phase for accuracy
        if (i > 0 && in_refine_phase) {
            vec3 delta = p - prevP;
            float sdfDelta = ds - prevSDF;
            float weight = exp(-ds * 5.0);  // Higher weight near surface
            // Gradient points in direction of increasing SDF
            g_accum_gradient += normalize(delta) * sign(-sdfDelta) * weight;
        }
        prevP = p;
        prevSDF = ds;

        // Adaptive stepping: bigger steps far from surface (branchless, optimized)
        // Larger steps in coarse phase
        float phaseScale = in_refine_phase ? 1.0 : 1.5;
        d += ds * stepScale * phaseScale * (1.0 + 0.5 * smoothstep(0.3, 0.7, ds));

        // Exit if past bounding sphere exit OR surface hit
        if (d > maxD || abs(ds) < SURF_DIST) break;
    }

    // Normalize accumulated gradient
    float gradLen = length(g_accum_gradient);
    if (gradLen > 0.001) {
        g_accum_gradient /= gradLen;
    }

    return vec2(d, float(steps) / float(maxSteps));
}

// Fast normal using forward-differenced gradient + 2 refinement samples
// Saves 2-4 SDF calls compared to tetrahedral method!
vec3 getNormal(vec3 p) {
    float eps = 0.001;

    // Start with accumulated gradient from raymarch (free!)
    vec3 n = g_accum_gradient;

    // If gradient is weak, fall back to cheap 2-sample refinement
    if (length(n) < 0.5) {
        // 2-sample central difference (cheaper than 4-sample tetrahedral)
        vec3 e = vec3(eps, 0.0, 0.0);
        n = vec3(
            sceneSDF(p + e.xyy) - sceneSDF(p - e.xyy),
            sceneSDF(p + e.yxy) - sceneSDF(p - e.yxy),
            sceneSDF(p + e.yyx) - sceneSDF(p - e.yyx)
        );
    } else {
        // Refine with just 2 perpendicular samples for quality
        // This catches high-frequency detail the gradient might miss
        vec3 tangent = normalize(cross(n, vec3(0.0, 1.0, 0.1)));
        vec3 bitangent = cross(n, tangent);

        float t1 = sceneSDF(p + tangent * eps) - sceneSDF(p - tangent * eps);
        float t2 = sceneSDF(p + bitangent * eps) - sceneSDF(p - bitangent * eps);

        // Blend refinement with gradient
        n = normalize(n + tangent * t1 * 0.3 + bitangent * t2 * 0.3);
    }

    return normalize(n);
}

// ============ SCREEN-SPACE AMBIENT OCCLUSION ============
// Raymarched-aware AO: samples SDF in hemisphere around hit point
// Returns occlusion factor [0=fully occluded, 1=no occlusion]

float calculateSSAO(vec3 p, vec3 n) {
    // Quality-gated: skip on low quality
    if (u_quality_tier == 0) {
        return 1.0;
    }

    float ao = 0.0;
    float totalWeight = 0.0;

    // Sample distances (near to far)
    float sampleDists[4];
    sampleDists[0] = 0.02;
    sampleDists[1] = 0.05;
    sampleDists[2] = 0.12;
    sampleDists[3] = 0.25;

    // Number of angular samples per distance (quality-gated)
    int angularSamples = (u_quality_tier == 2) ? 8 : 5;

    for (int d = 0; d < 4; d++) {
        float dist = sampleDists[d];
        float weight = 1.0 / (1.0 + dist * 4.0);  // Closer samples matter more

        for (int i = 0; i < angularSamples; i++) {
            // Generate sample direction in hemisphere around normal
            float angle = float(i) * TAU / float(angularSamples);
            // Add slight variation based on distance to avoid banding
            angle += float(d) * 0.7;

            // Create tangent frame
            vec3 tangent = normalize(cross(n, vec3(0.0, 1.0, 0.1)));
            vec3 bitangent = cross(n, tangent);

            // Sample direction: mostly along normal, some tangent spread
            float spread = 0.6;  // How much to spread from normal
            vec3 sampleDir = normalize(
                n +
                tangent * cos(angle) * spread +
                bitangent * sin(angle) * spread
            );

            // Sample SDF at offset position
            vec3 samplePos = p + sampleDir * dist;
            float sdf = sceneSDF(samplePos);

            // If SDF is small (near surface), we're occluded
            // Map SDF to occlusion: sdf=0 -> fully occluded, sdf>=dist -> no occlusion
            float occlusion = clamp(sdf / dist, 0.0, 1.0);

            ao += occlusion * weight;
            totalWeight += weight;
        }
    }

    // Normalize and apply contrast
    ao = ao / totalWeight;
    ao = pow(ao, 1.5);  // Increase contrast

    // Blend with energy (less AO when energy is high for brighter look)
    ao = mix(ao, 1.0, u_energy * 0.2);

    return clamp(ao, 0.0, 1.0);
}

// Fast approximation for lower quality (uses step-based AO from raymarch)
float approximateAO(float steps_norm, vec3 p, vec3 n) {
    // Basic step-based AO
    float stepAO = 1.0 - steps_norm * 0.4;

    // Add simple hemisphere sampling (3 samples)
    float sampleAO = 0.0;
    float dist = 0.08;

    vec3 tangent = normalize(cross(n, vec3(0.0, 1.0, 0.1)));
    vec3 bitangent = cross(n, tangent);

    // 3 samples in a triangle pattern
    for (int i = 0; i < 3; i++) {
        float angle = float(i) * TAU / 3.0;
        vec3 sampleDir = normalize(n + tangent * cos(angle) * 0.5 + bitangent * sin(angle) * 0.5);
        float sdf = sceneSDF(p + sampleDir * dist);
        sampleAO += clamp(sdf / dist, 0.0, 1.0);
    }
    sampleAO /= 3.0;

    // Blend step AO with sample AO
    return mix(stepAO, sampleAO, 0.5);
}

// ============ LIGHTING ============

// Hue to RGB for pitch accent coloring
vec3 hue2rgb(float h) {
    vec3 rgb = clamp(abs(mod(h * 6.0 + vec3(0.0, 4.0, 2.0), 6.0) - 3.0) - 1.0, 0.0, 1.0);
    return rgb;
}

// ============ FOVEATED BACKGROUND BUDGET SYSTEM ============
// Game-quality LOD: budget detail based on screen position and QS wheel state
// When QS wheel ON: center=LOW detail (protect legibility), edges=HIGH detail
// When QS wheel OFF: center=HIGH detail (hero object), edges=LOW detail

// Returns (bgMul, stepsMul, postMul) multipliers for budget scaling
vec3 getFoveateBudget(vec2 uv) {
    float r = clamp(length(uv), 0.0, 1.0);

    // Smooth falloff: 0 at center, 1 at edges (no popping)
    float edgeMask = smoothstep(0.25, 0.85, r);
    float centerMask = 1.0 - edgeMask;

    // Invert based on QS wheel state
    // QS on: detail at edges (centerMask inverted)
    // QS off: detail at center (centerMask normal)
    float detailMask = mix(centerMask, edgeMask, u_qs_enabled);

    // Budget multipliers: higher detailMask = more detail
    float bgMul = mix(0.45, 1.0, detailMask);     // BG samples: 45-100%
    float stepsMul = mix(0.60, 1.0, detailMask);  // Ray steps: 60-100%
    float postMul = mix(0.50, 1.0, detailMask);   // Post taps: 50-100%

    return vec3(bgMul, stepsMul, postMul);
}

// Get BG sample count based on quality and foveation budget
int getBgSamples(int quality, float budget) {
    int baseSamples;
    if (quality == BG_LOW) baseSamples = 3;
    else if (quality == BG_MED) baseSamples = 6;
    else baseSamples = 10;
    return max(2, int(float(baseSamples) * budget));
}

// Get post-processing tap count based on quality and foveation budget
int getPostTaps(int quality, float budget) {
    int baseTaps;
    if (quality == BG_LOW) baseTaps = 3;
    else if (quality == BG_MED) baseTaps = 5;
    else baseTaps = 7;
    return max(2, int(float(baseTaps) * budget));
}

// Get noise octaves based on quality
int getBgOctaves(int quality) {
    if (quality == BG_LOW) return 1;
    else if (quality == BG_MED) return 1;
    else return 2;
}

// ============ CHEAP BACKGROUND EFFECTS ============
// Analytic effects that replace expensive volumetric/fractal backgrounds

// Simple hash for starfield (no textures needed)
float bgHash21(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

// Sky gradient with fog (0 cost - fully analytic)
vec3 skyGradientFog(vec2 uv, float dist) {
    // Vertical gradient: dark purple at bottom, deep blue at top
    float y = uv.y * 0.5 + 0.5;  // 0-1 range
    vec3 bottomColor = vec3(0.02, 0.01, 0.04);  // Dark purple
    vec3 topColor = vec3(0.01, 0.02, 0.06);      // Deep blue

    vec3 sky = mix(bottomColor, topColor, y);

    // Add subtle radial vignette (darker at edges)
    float vignette = 1.0 - length(uv) * 0.3;
    sky *= vignette;

    // Distance-based fog (exponential falloff)
    float fog = exp(-dist * u_fog_density * 0.5);
    sky = mix(vec3(0.03, 0.02, 0.05), sky, fog);

    return sky;
}

// ============ STRUCTURED TICK PEARLS (Museum-Mode Polish) ============
// Intersection-locked dots at ring × anchor crossings.
// REPLACES random starfield - all dots strictly on 24-anchor grid.
// This makes the wheel look intentional even when idle.

#define TICK_NUM_RINGS 15
#define TICK_INNER_RADIUS 0.15
#define TICK_OUTER_RADIUS 0.88

// Get tick pearl contribution at UV position
vec3 tickPearls(vec2 uv, float energy, float idle_mix) {
    vec3 pearls = vec3(0.0);

    // Radial distance from center
    float dist = length(uv);

    // Early out if outside wheel region
    if (dist < TICK_INNER_RADIUS || dist > TICK_OUTER_RADIUS) return pearls;

    // Angle (normalized to [0, TAU))
    float theta = atan(uv.y, uv.x);
    if (theta < 0.0) theta += TAU;

    // Determine anchor count based on fold mode
    int anchorCount = (u_fold24_t > 0.5) ? 24 : 12;
    float anchorStep = TAU / float(anchorCount);

    // Find nearest anchor
    float anchorIdx = floor(theta / anchorStep + 0.5);
    float nearestAnchorAngle = anchorIdx * anchorStep;
    float angleDist = abs(theta - nearestAnchorAngle);
    // Wrap angle distance
    if (angleDist > PI) angleDist = TAU - angleDist;

    // Ring spacing
    float ringSpacing = (TICK_OUTER_RADIUS - TICK_INNER_RADIUS) / float(TICK_NUM_RINGS);

    // Find nearest ring
    float ringDist = (dist - TICK_INNER_RADIUS) / ringSpacing;
    int nearestRing = int(floor(ringDist + 0.5));
    float ringError = abs(ringDist - float(nearestRing));

    // Determine if this is a major or minor dot position
    // Major dots: 12-fold anchors (even indices in 24-fold), every 2nd ring
    // Minor dots: 24-fold anchors (odd indices), every 3rd ring (only when 24-mode)
    bool is12Anchor = (int(anchorIdx) % 2 == 0);
    bool isMajorRing = (nearestRing % 2 == 0);
    bool isMinorRing = (nearestRing % 3 == 0);

    // Visibility rules
    bool showDot = false;
    float dotSize = 0.0;
    float dotBrightness = 0.0;

    if (is12Anchor && isMajorRing) {
        // Major dot (always visible) - 12 anchors × every 2nd ring
        showDot = true;
        // In Idle: slightly larger, brighter (deliberate markers)
        dotSize = mix(0.007, 0.011, idle_mix);
        dotBrightness = mix(0.55, 0.75, idle_mix);
    } else if (u_fold24_t > 0.5 && !is12Anchor && isMinorRing) {
        // Minor dot (24-mode only) - 24 anchors × every 3rd ring
        showDot = true;
        dotSize = 0.005;
        dotBrightness = 0.35;
    }

    if (showDot) {
        // Proximity to intersection
        float angleProx = smoothstep(dotSize * 2.5, 0.0, angleDist);
        float ringProx = smoothstep(0.025, 0.0, ringError);
        float intersection = angleProx * ringProx;

        if (intersection > 0.01) {
            // Pearl color from latched dominant chroma
            // Use getPitchColor if available, else fallback to simple hue
            float hue = float(u_dom_chroma_latched) / 12.0;
            vec3 pearlColor = vec3(
                0.5 + 0.5 * cos(TAU * (hue + 0.0)),
                0.5 + 0.5 * cos(TAU * (hue + 0.33)),
                0.5 + 0.5 * cos(TAU * (hue + 0.67))
            );

            // Soft glow with bright core
            float glow = exp(-intersection * intersection / (dotSize * dotSize) * 2.0);
            float core = smoothstep(dotSize * 0.6, dotSize * 0.2, angleDist * 0.5 + ringError * 0.08);

            // In Live mode: micro-pulse on energy
            float pulse = 1.0;
            if (idle_mix < 0.5) {
                pulse = 0.85 + 0.15 * sin(u_time * 3.5 + anchorIdx * 0.6);
            }

            // Composite
            pearls += pearlColor * glow * dotBrightness * pulse * intersection;
            pearls += vec3(1.0, 0.98, 0.95) * core * intersection * 0.4;
        }
    }

    return pearls;
}

// Single-octave animated noise (cheap motion texture)
float singleOctaveNoise(vec2 uv) {
    // Very simple value noise - one octave only
    vec2 i = floor(uv);
    vec2 f = fract(uv);

    // Smooth interpolation
    vec2 u = f * f * (3.0 - 2.0 * f);

    // Four corner values
    float a = bgHash21(i);
    float b = bgHash21(i + vec2(1.0, 0.0));
    float c = bgHash21(i + vec2(0.0, 1.0));
    float d = bgHash21(i + vec2(1.0, 1.0));

    // Bilinear interpolation
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Kick streaks - max 5 taps, screen-space radial blur
vec3 kickStreaksLite(vec2 uv, vec3 col, float kickIntensity, int maxTaps) {
    if (kickIntensity < 0.05) return col;

    // Radial direction from center
    vec2 dir = normalize(uv) * 0.02;  // Streak length

    // Accumulate samples (max 5)
    vec3 streak = vec3(0.0);
    float weight = 0.0;
    int taps = min(maxTaps, 5);

    for (int i = 0; i < 5; i++) {
        if (i >= taps) break;

        float t = float(i) / float(taps);
        vec2 sampleUV = uv - dir * t;

        // Sample would normally read from framebuffer, but we approximate
        // using distance-based falloff for the "streak" effect
        float falloff = exp(-t * 3.0);
        streak += col * falloff;
        weight += falloff;
    }

    streak /= max(weight, 1.0);

    // Blend based on kick intensity
    return mix(col, streak * 1.2, kickIntensity * 0.4);
}

// Budget-controlled volumetric shafts (fewer samples)
vec3 volumetricShaftsBudgeted(vec2 uv, vec3 col, float intensity, int samples) {
    if (intensity < 0.01 || samples < 2) return col;

    // Light source at center
    vec2 lightPos = vec2(0.0);
    vec2 deltaUV = (uv - lightPos) / float(samples);

    float illumination = 0.0;
    float decay = 0.96;
    float weight = 1.0;
    vec2 sampleUV = uv;

    for (int i = 0; i < 12; i++) {  // Max 12, but early-exit
        if (i >= samples) break;

        sampleUV -= deltaUV;
        float dist = length(sampleUV);
        float brightness = exp(-dist * 3.0) * weight;
        illumination += brightness;
        weight *= decay;
    }

    // Normalize and apply
    illumination = illumination / float(samples) * 0.8;

    // Warm shaft color
    vec3 shaftColor = vec3(1.0, 0.95, 0.85) * illumination * intensity;
    return col + shaftColor;
}

// Complete cheap background pass
vec3 backgroundPassCheap(vec2 uv, vec3 col, float dist, vec3 budget) {
    int samples = getBgSamples(u_bg_quality, budget.x);

    // 1. Sky gradient + fog (always, 0 cost)
    vec3 sky = skyGradientFog(uv, dist);

    // Tick pearls disabled - clean wheel aesthetic preferred
    // if (u_bg_quality >= BG_LOW) {
    //     vec3 pearls = tickPearls(uv, u_energy, u_idle_mix);
    //     sky += pearls;
    // }

    // 3. Single-octave noise motion (Med+)
    if (u_bg_quality >= BG_MED) {
        float noise = singleOctaveNoise(uv * 3.0 + u_time * 0.1);
        noise = noise * 0.03 * u_energy;  // Very subtle
        sky += vec3(noise * 0.5, noise * 0.3, noise);
    }

    // === VORTEX MODE: Clamp background intensity ===
    // When Vortex is active, dim background to keep 24-fold scaffold primary
    float vortex_bg_dim = 1.0;
    if (u_vortex_active == 1) {
        // Dim to 35% during vortex to maintain scaffold hierarchy
        vortex_bg_dim = 0.35;
    }

    // Blend background where geometry isn't present
    float bgBlend = smoothstep(0.0, 0.3, dist - 1.0);
    col = mix(col, sky * vortex_bg_dim, bgBlend * 0.3);

    return col;
}

// Get pitch accent emission at a surface point
// Creates 12 subtle emissive seams aligned with chroma bins
float getPitchAccent(vec3 p, vec3 n, out vec3 accentColor) {
    // Get angle in XZ plane for extrusion, or use normal for lathe
    float theta;
    if (u_render_mode == 1) {
        // Lathe mode: use angle from normal
        theta = atan(n.z, n.x);
    } else {
        // Extrusion mode: use position angle
        theta = atan(p.y, p.x);
    }

    // Apply rotation to match geometry
    theta += u_rotation;

    // Map to bin index [0, 12)
    float bin_angle = TAU / 12.0;
    float idx_f = mod(theta / bin_angle, 12.0);
    if (idx_f < 0.0) idx_f += 12.0;

    int idx = int(floor(idx_f));
    float frac = fract(idx_f);

    // Seam is at bin boundaries - create thin bright lines
    float seam_width = 0.08;  // Narrow seam
    float seam_intensity = 0.0;

    // Check proximity to bin boundary (at frac = 0 or frac = 1)
    float dist_to_seam = min(frac, 1.0 - frac);
    if (dist_to_seam < seam_width) {
        seam_intensity = 1.0 - (dist_to_seam / seam_width);
        seam_intensity = seam_intensity * seam_intensity;  // Sharper falloff
    }

    // Color based on the chroma bin - brighter bins get brighter seams
    float chroma_val = getChroma(idx);
    float next_chroma = getChroma(idx + 1);
    float avg_chroma = (chroma_val + next_chroma) * 0.5;

    // Hue from pitch class (chromatic order)
    float hue = float(idx) / 12.0;
    accentColor = hue2rgb(hue);

    // Intensity = seam sharpness * chroma strength
    return seam_intensity * avg_chroma * 1.5;
}

// Fake environment reflection for visual interest
vec3 fakeEnvironment(vec3 n) {
    // Simple gradient lobes based on normal direction
    // Creates sense of space without real reflection mapping

    // Sky lobe (from above)
    float sky = max(n.y, 0.0);
    vec3 skyColor = vec3(0.3, 0.4, 0.6);

    // Ground bounce (from below)
    float ground = max(-n.y, 0.0);
    vec3 groundColor = vec3(0.2, 0.15, 0.1);

    // Side fill (from sides)
    float side = 1.0 - abs(n.y);
    vec3 sideColor = vec3(0.15, 0.15, 0.2);

    return skyColor * sky * 0.3 + groundColor * ground * 0.2 + sideColor * side * 0.1;
}

// ============ SKIN SYSTEM ============

// Triplanar UV mapping - samples texture from 3 axes and blends by normal
// Much better coverage for sculptural/organic forms, no seams
vec3 triplanarBlend(vec3 n) {
    // Sharp blend weights from absolute normal
    vec3 blend = abs(n);
    // Sharpen the blend with inline multiply (faster than pow function call)
    blend = blend * blend;  // ^2
    blend = blend * blend;  // ^4
    // Normalize so weights sum to 1
    blend /= (blend.x + blend.y + blend.z + 0.0001);
    return blend;
}

vec3 sampleTriplanar(vec3 p, vec3 n) {
    if (u_skin_has_texture == 0) {
        return vec3(0.5);
    }

    // Scale position for texture tiling (adjust for geometry size)
    vec3 pos = p * 0.3;

    // Sample from each axis plane
    vec3 xSample = texture(u_skin_tex, pos.yz).rgb;
    vec3 ySample = texture(u_skin_tex, pos.xz).rgb;
    vec3 zSample = texture(u_skin_tex, pos.xy).rgb;

    // Blend by normal direction
    vec3 blend = triplanarBlend(n);

    return xSample * blend.x + ySample * blend.y + zSample * blend.z;
}

// Cylindrical UV mapping for extrusion mode
// u = angle around axis, v = position along axis
vec2 cylindricalUV(vec3 p, vec3 n) {
    float theta;
    float v_coord;

    if (u_render_mode == 1) {
        // Lathe mode: theta from XZ, v from Y
        theta = atan(p.z, p.x);
        float half_height = u_thickness * 0.5;
        v_coord = (p.y / max(half_height, 0.01) + 1.0) * 0.5;
    } else {
        // Extrusion mode: theta from XY, v from Z
        theta = atan(p.y, p.x);
        float half_thick = u_thickness * 0.5;
        v_coord = (p.z / max(half_thick, 0.01) + 1.0) * 0.5;
    }

    // Normalize theta to [0, 1]
    float u_coord = (theta + PI) / TAU;

    // Lock to dominant pitch: rotate so dominant bin is at top (u=0.5)
    if (u_skin_lock_dominant != 0) {
        float dom_offset = float(u_dom_pc) / 12.0;
        u_coord = fract(u_coord - dom_offset + 0.5);
    }

    return vec2(u_coord, v_coord);
}

// Main UV mapping function - selects mode based on u_skin_uv_mode
vec2 skinUV(vec3 p, vec3 n) {
    // Always use cylindrical for UV coord calculation
    // Triplanar is handled separately in texture sampling
    return cylindricalUV(p, n);
}

// Sample skin texture with visibility grade
// Applies: sRGB gamma fix, shadow lift, contrast, saturation
vec3 sampleSkinTexture(vec2 uv) {
    if (u_skin_has_texture == 0) {
        return vec3(0.5);  // Neutral gray when no texture
    }

    vec3 tex = texture(u_skin_tex, uv).rgb;

    // === VISIBILITY GRADE ===
    // Video textures are often sRGB, treat them as such for proper color
    // Fast gamma approximation: tex^2 instead of tex^2.2 (barely perceptible difference)
    tex = tex * tex;

    // Shadow lift - prevent dark areas from being crushed
    // Adds a floor to shadows so video detail is preserved
    tex = tex + u_skin_shadow_lift * (1.0 - tex);

    // Apply contrast adjustment (centered at 0.5, in linear space)
    tex = (tex - 0.5) * u_skin_contrast + 0.5;

    // Apply saturation adjustment
    float luma = dot(tex, vec3(0.2126, 0.7152, 0.0722));  // Rec.709 weights
    tex = mix(vec3(luma), tex, u_skin_saturation);

    return clamp(tex, 0.0, 1.0);
}

// Sample skin texture with triplanar mapping and visibility grade
vec3 sampleSkinTextureTriplanar(vec3 p, vec3 n) {
    if (u_skin_has_texture == 0) {
        return vec3(0.5);
    }

    // Get triplanar sample
    vec3 tex = sampleTriplanar(p, n);

    // === VISIBILITY GRADE ===
    tex = tex * tex;  // Fast gamma ~2.0 (sRGB to linear approximation)
    tex = tex + u_skin_shadow_lift * (1.0 - tex);  // Shadow lift
    tex = (tex - 0.5) * u_skin_contrast + 0.5;  // Contrast

    float luma = dot(tex, vec3(0.2126, 0.7152, 0.0722));
    tex = mix(vec3(luma), tex, u_skin_saturation);  // Saturation

    return clamp(tex, 0.0, 1.0);
}

// Get skin texture color using appropriate UV mode
vec3 getSkinColor(vec3 p, vec3 n, vec2 uv) {
    if (u_skin_uv_mode == 1) {
        // Triplanar mode - better for sculptural/lathe
        return sampleSkinTextureTriplanar(p, n);
    } else {
        // Cylindrical mode - better for extrusion
        return sampleSkinTexture(uv);
    }
}

// Hue-shift color by pitch class
vec3 hueShiftByPitch(vec3 col) {
    if (u_skin_hue_follow == 0) return col;

    // Convert to HSV-ish, shift hue, convert back
    float luma = dot(col, vec3(0.299, 0.587, 0.114));
    float chroma = max(col.r, max(col.g, col.b)) - min(col.r, min(col.g, col.b));

    if (chroma < 0.05) return col;  // Don't shift near-gray

    // Subtle hue shift based on dominant pitch
    float hue_shift = float(u_dom_pc) / 12.0 * 0.15;  // Max 15% shift

    // Simple hue rotation via matrix
    float angle = hue_shift * TAU;
    float cs = cos(angle);
    float sn = sin(angle);
    mat3 hueRot = mat3(
        0.213 + 0.787*cs - 0.213*sn, 0.715 - 0.715*cs - 0.715*sn, 0.072 - 0.072*cs + 0.928*sn,
        0.213 - 0.213*cs + 0.143*sn, 0.715 + 0.285*cs + 0.140*sn, 0.072 - 0.072*cs - 0.283*sn,
        0.213 - 0.213*cs - 0.787*sn, 0.715 - 0.715*cs + 0.715*sn, 0.072 + 0.928*cs + 0.072*sn
    );

    return hueRot * col;
}

// Calculate emissive contribution (albedo-first: video AS emissive, gated by audio)
// This makes the video "breathe" with the music while remaining readable
vec3 skinEmissive(vec3 p, vec3 n, vec2 uv, vec3 videoColor) {
    if (u_skin_emissive < 0.01) return vec3(0.0);

    // Use the already-graded video color (not re-sampling)
    vec3 emissive_tex = videoColor;
    float emissive_luma = dot(emissive_tex, vec3(0.2126, 0.7152, 0.0722));

    // Base intensity from luminance - brighter video areas glow more
    // Using smoothstep for soft threshold (not hard cutoff)
    float base = smoothstep(0.1, 0.6, emissive_luma);

    // === AUDIO REACTIVITY (makes emissive "breathe") ===

    // Energy boost: louder = brighter glow (subtle)
    float energy_boost = mix(0.5, 1.3, u_energy);

    // Kick pulse: quick flash on kick, helps readability
    // Use kick_used (suppressed when inside) and clamp to KICK_PUMP_MAX
    float kick_flash = min(getKickUsed(u_cam_inside_dip) * 0.4, KICK_PUMP_MAX);
    float pulse = 1.0 + kick_flash * exp(-fract(u_time * 2.0) * 4.0);

    // Tonal confidence: cleaner harmony = sharper glow
    float conf_focus = mix(0.7, 1.15, u_tonal_confidence);

    // === COMBINE ===
    // Base emissive strength from uniform
    float intensity = u_skin_emissive * base * energy_boost * pulse * conf_focus;

    // Don't clamp too hard - allow bright areas to really pop
    // Max ~1.0 for strong emissive (user can push higher with slider)
    intensity = min(intensity, 1.2);

    // Silence gate: fade out glow when quiet (but not completely)
    float silence_gate = smoothstep(0.02, 0.15, u_energy);
    intensity *= mix(0.3, 1.0, silence_gate);  // Keep 30% even in silence

    // Apply hue shift if enabled
    vec3 emissive_color = hueShiftByPitch(emissive_tex);

    return emissive_color * intensity;
}

// Enhanced rim flare (ProjectM "pop") with iridescence
vec3 skinRimFlare(vec3 n, vec3 rd) {
    if (u_skin_rim < 0.01) return vec3(0.0);

    float fresnel = 1.0 - max(dot(n, -rd), 0.0);
    float rim = pow(fresnel, 3.0);
    float rim_strength = mix(0.15, 0.6, u_energy) * mix(0.6, 1.3, u_tonal_confidence);
    rim_strength *= u_skin_rim;

    // Iridescent color shift at grazing angles
    float iridShift = fresnel * 0.2;

    // Color based on tonality with iridescence
    vec3 rim_color;
    if (u_tonal_confidence > 0.3) {
        // Tonal: use harmonic hue from dominant pitch + iridescent shift
        float hue = float(u_dom_pc) / 12.0;
        vec3 baseRimColor = hue2rgb(hue);
        vec3 iridColor = hue2rgb(fract(hue + iridShift));
        rim_color = mix(baseRimColor, iridColor, fresnel * 0.4);
        rim_color = mix(vec3(0.7), rim_color, u_tonal_confidence);
    } else {
        // Atonal: cool white/gray with slow hue drift + subtle iridescence
        float drift_hue = fract(u_time * 0.02);
        vec3 baseColor = mix(vec3(0.7, 0.75, 0.85), hue2rgb(drift_hue) * 0.3 + 0.7, 0.2);
        vec3 iridColor = hue2rgb(fract(drift_hue + iridShift));
        rim_color = mix(baseColor, iridColor, fresnel * 0.3);
    }

    return rim * rim_strength * rim_color;
}

// Starburst / lens streak effect aligned with dominant pitch
vec3 skinStarburst(vec2 uv_screen, vec3 col) {
    if (u_skin_starburst == 0 || u_energy < 0.2) return vec3(0.0);

    // Direction from dominant pitch angle
    float dom_angle = float(u_dom_pc) / 12.0 * TAU;
    vec2 flare_dir = vec2(cos(dom_angle), sin(dom_angle));

    // Screen-space alignment
    vec2 from_center = normalize(uv_screen - 0.5);
    float along = abs(dot(from_center, flare_dir));

    // Streak falloff
    float streak = pow(1.0 - along, 12.0) * smoothstep(0.2, 0.9, u_energy);

    // Color matches rim color
    vec3 streak_color;
    if (u_tonal_confidence > 0.3) {
        float hue = float(u_dom_pc) / 12.0;
        streak_color = hue2rgb(hue) * 0.5 + 0.5;
    } else {
        streak_color = vec3(0.8, 0.85, 0.9);
    }

    return streak * 0.15 * streak_color;
}

// Edge outline (toon-ish contour)
float skinEdgeOutline(vec3 n, vec3 rd) {
    if (u_skin_edge < 0.01) return 0.0;

    float edge = 1.0 - abs(dot(n, -rd));
    // Sharpen the edge with a steeper curve
    edge = smoothstep(0.4, 0.7, edge);

    return edge * u_skin_edge * 0.8;
}

// Enhanced specular sparkle (energy-gated glitter)
// OPTIMIZED: Reduced from 3 lobes to 2, with more reasonable exponents
vec3 skinSparkle(vec3 n, vec3 rd, vec3 lightDir) {
    if (u_skin_sparkle < 0.01) return vec3(0.0);

    // Energy gate: no sparkle when quiet
    float energy_gate = smoothstep(0.1, 0.4, u_energy);
    if (energy_gate < 0.01) return vec3(0.0);

    vec3 h = normalize(lightDir - rd);

    // Two specular lobes with reasonable exponents (64, 128 instead of 256, 192, 320)
    // Lower exponents = slightly softer sparkles, but 3x fewer pow() calls
    float spec1 = pow(max(dot(n, h), 0.0), 64.0);

    // Second lobe at different position
    vec3 lightDir2 = normalize(vec3(-0.7, 0.6, -0.4));
    vec3 h2 = normalize(lightDir2 - rd);
    float spec2 = pow(max(dot(n, h2), 0.0), 128.0);

    // Combine with kick pulse (clamped, suppressed when inside)
    float kick_used = getKickUsed(u_cam_inside_dip);
    float kick_flash = 1.0 + min(kick_used * 1.5, KICK_PUMP_MAX * 2.0);
    float sparkle = (spec1 + spec2 * 0.6) * kick_flash;

    // Sparkle color: slightly warm
    vec3 sparkle_color = vec3(1.0, 0.98, 0.95);

    return sparkle_color * sparkle * u_skin_sparkle * energy_gate * 0.6;
}

vec3 lighting(vec3 p, vec3 n, vec3 rd, vec3 baseColor, vec2 uv_screen) {
    // === PRE-TORUS TRANSITION FAST PATH ===
    // During 24-fold → torus transition, use simplified lighting
    // Only when: high fold24_t, not inside, not warping, torus about to activate
    bool lightingPreTransition = (u_fold24_t > 0.5 && u_interior_latch < 0.5 &&
                                  u_warp_active == 0 && u_torus_active == 1);

    // === SKIN UV MAPPING (compute once) ===
    // Skip during pre-transition since we won't use video
    vec2 skin_uv = lightingPreTransition ? vec2(0.0) : skinUV(p, n);

    // ==========================================================
    // ALBEDO-FIRST RENDERING: Video drives the base material
    // ==========================================================
    // Instead of treating video as an overlay, we make it the BASE
    // albedo, then apply lighting math ON TOP of it.

    vec3 albedo = baseColor;  // Start with chroma-derived color

    // Get skin/video texture color (with visibility grade applied)
    // Skip during pre-transition for performance (texture sampling is expensive)
    vec3 videoColor = vec3(0.5);
    bool hasVideoSkin = (u_skin_mode == 1 || u_skin_mode == 3) && u_skin_has_texture != 0;
    hasVideoSkin = hasVideoSkin && !lightingPreTransition;  // Skip video during pre-transition

    if (hasVideoSkin) {
        // Sample video using appropriate UV mode (cylindrical or triplanar)
        videoColor = getSkinColor(p, n, skin_uv);

        // Optional hue shift by pitch
        if (u_skin_hue_follow != 0) {
            videoColor = hueShiftByPitch(videoColor);
        }

        // ==========================================================
        // COMPEL NARRATIVE MODULATION
        // Video visibility governed by mathematical invariants
        // ==========================================================

        // --- BIRTH: Flicker Gate (narrative latent) ---
        // Procedural noise suppresses video with occasional glimpses
        float flickerNoise = hash31(floor(p * 3.0 + u_time * 0.5));
        float flickerMask = smoothstep(0.85, 1.0, flickerNoise);
        float birthGate = mix(1.0, flickerMask, u_compel_flicker_gate);

        // --- LOCK: 12-Fold Harmonic Seam Binding ---
        // Video bound to θ = n×30° (where chroma bins resolve)
        float theta = atan(p.z, p.x);
        float seamAngle = mod(theta + PI, TAU / 12.0);  // Distance to nearest 30° line
        float seamDist = min(seamAngle, TAU / 12.0 - seamAngle);
        float seamProximity = smoothstep(0.12, 0.02, seamDist);  // ~7° width

        // Seam brightness scales with chroma confidence at nearest bin
        int nearestBin = int(mod(floor((theta + PI) / (TAU / 12.0) + 0.5), 12.0));
        float binConfidence = getChroma(nearestBin);
        float seamStrength = seamProximity * mix(0.3, 1.0, binConfidence);

        // Apply exterior binding (1.0 = only seams visible, 0.0 = everywhere)
        float exteriorMask = mix(1.0, seamStrength, u_compel_exterior_bind);

        // Combine all compel modulation
        float compelMod = birthGate * exteriorMask;
        videoColor *= compelMod;

        // ALBEDO-FIRST: Video IS the base material
        // u_skin_surface_mix controls how much video replaces base color
        // u_video_visibility modulates by act state (Birth→Lock→Dive→Afterglow)
        float videoMix = u_skin_surface_mix * u_video_visibility;
        albedo = mix(baseColor, videoColor, videoMix);
    }

    // ==========================================================
    // LIGHTING: Applied to albedo (not overwriting it)
    // ==========================================================

    // Key light (warm, from above-right)
    vec3 lightDir1 = normalize(vec3(1.0, 1.2, 0.5));
    float diff1 = max(dot(n, lightDir1), 0.0);
    vec3 light1 = vec3(1.0, 0.95, 0.9) * diff1;

    // Fill light (cool, from left) - keeps shadows from going pure black
    vec3 lightDir2 = normalize(vec3(-1.0, 0.3, -0.5));
    float diff2 = max(dot(n, lightDir2), 0.0);
    vec3 light2 = vec3(0.5, 0.55, 0.7) * diff2 * 0.5;

    // Ambient - higher base to preserve video in shadows
    vec3 ambient = vec3(0.12, 0.12, 0.15);

    // Combined diffuse lighting (applied to albedo)
    vec3 diffuseLight = light1 + light2 + ambient;

    // ==========================================================
    // SPECULAR & RIM: Additive, don't overwrite albedo
    // ==========================================================

    // Fresnel rim lighting (adds, doesn't replace)
    float fresnel = pow(1.0 - max(dot(n, -rd), 0.0), 3.0);
    float rim_base = 0.15 + 0.5 * u_tonal_confidence;
    // u_cam_rim_boost increases rim when camera is close (prevents flat blob)
    rim_base += u_cam_rim_boost;
    vec3 rimColor = mix(vec3(0.4, 0.45, 0.5), vec3(0.6, 0.7, 0.9), u_tonal_confidence);

    // Rim is additive but controlled by u_skin_rim + dynamic boost
    vec3 rimLight = rimColor * fresnel * rim_base * (u_skin_rim + u_cam_rim_boost * 0.5);

    // Add enhanced skin rim flare if active - skip during pre-transition
    if (!lightingPreTransition) {
        rimLight += skinRimFlare(n, rd) * 0.7;  // Reduce to not overpower video
    }

    // Specular highlights (sharp, additive)
    vec3 h = normalize(lightDir1 - rd);
    float spec_base = pow(max(dot(n, h), 0.0), 64.0);
    // Use kick_used (clamped, suppressed when inside)
    float kick_used = getKickUsed(u_cam_inside_dip);
    float kick_pulse = 1.0 + min(kick_used * 0.8, KICK_PUMP_MAX);
    float spec = spec_base * kick_pulse;
    float spec_intensity = 0.25 + u_energy * 0.35;
    vec3 specular = vec3(1.0, 0.98, 0.95) * spec * spec_intensity;

    // Secondary specular (subtle) - skip during pre-transition
    if (!lightingPreTransition) {
        vec3 lightDir3 = normalize(vec3(-0.5, 0.8, -1.0));
        vec3 h2 = normalize(lightDir3 - rd);
        float spec2 = pow(max(dot(n, h2), 0.0), 128.0);
        specular += vec3(0.9, 0.95, 1.0) * spec2 * kick_pulse * 0.2;

        // Add skin sparkle (energy-gated glitter) - reduced strength
        specular += skinSparkle(n, rd, lightDir1) * 0.6;
    }

    // ==========================================================
    // ENVIRONMENT: Subtle, preserves albedo - skip during pre-transition
    // ==========================================================
    vec3 envColor = vec3(0.0);
    float env_strength = 0.0;
    if (!lightingPreTransition) {
        envColor = fakeEnvironment(n);
        env_strength = 0.2 + u_stability * 0.3;
    }

    // ==========================================================
    // EMISSIVE: Video as glow source (gated by audio)
    // Skip during pre-transition
    // ==========================================================
    vec3 emissive = vec3(0.0);
    if (!lightingPreTransition) {
        if (u_skin_mode == 2 || u_skin_mode == 3) {
            // Pass the video color so we don't re-sample
            emissive = skinEmissive(p, n, skin_uv, videoColor);
        }

        // --- AFTERGLOW: Emissive Persistence (narrative lingers) ---
        // Video persists as soft emissive glow even as intensity fades
        // "The story lingers without escalation"
        if (hasVideoSkin && u_compel_emissive_persist > 0.0) {
            // Softened video color as residual glow
            vec3 afterglowColor = videoColor * 0.4;  // Reduced intensity
            float afterglowStrength = u_compel_emissive_persist * 0.5;
            // Blend additively into emissive
            emissive += afterglowColor * afterglowStrength;
        }
    }

    // ==========================================================
    // PITCH ACCENT SEAMS (reduced to not compete with video)
    // Skip during pre-transition
    // ==========================================================
    vec3 pitchAccent = vec3(0.0);
    if (!lightingPreTransition) {
        vec3 accentColor;
        float accentIntensity = getPitchAccent(p, n, accentColor);
        pitchAccent = accentColor * accentIntensity * (0.3 + u_energy * 0.4);
    }

    // ==========================================================
    // EDGE OUTLINE (optional, reduced by default)
    // Skip during pre-transition
    // ==========================================================
    float edge = 0.0;
    vec3 edgeColor = vec3(0.0);
    if (!lightingPreTransition) {
        edge = skinEdgeOutline(n, rd);
        if (edge > 0.0 && u_tonal_confidence > 0.3) {
            float hue = float(u_dom_pc) / 12.0;
            edgeColor = hue2rgb(hue) * 0.15;
        }
    }

    // ==========================================================
    // FINAL COMPOSITION
    // ==========================================================

    // Start with lit albedo
    vec3 color = albedo * diffuseLight;

    // Add environment reflection (tinted by albedo for natural look)
    color += envColor * env_strength * albedo * 0.5;

    // Add specular and rim (pure additive)
    color += rimLight;
    color += specular;

    // Add pitch accent (subtle)
    color += pitchAccent;

    // Add emissive (video glow, the key to "popping")
    color += emissive;

    // Apply edge darkening (if enabled)
    color = mix(color, edgeColor, edge * 0.5);

    return color;
}

// ============ COLOR FROM CHROMA ============

// Generate color based on dominant pitch class
// Harmony → saturated, stable hue
// Ambiguous → desaturated, slow hue drift
// Silence → neutral gray (no color reactivity)
vec3 chromaColor() {
    // === ENERGY GATE: Dark when silent ===
    // When energy is very low, return dark color
    float energy_gate = smoothstep(0.02, 0.15, u_energy);
    if (energy_gate < 0.01) {
        return vec3(0.02);  // Dark when silent (geometry will also fade out)
    }

    // Find dominant pitch class using getChroma() for vec4 access
    float maxVal = 0.0;
    int maxIdx = 0;
    for (int i = 0; i < 12; i++) {
        float chromaVal = getChroma(i);
        if (chromaVal > maxVal) {
            maxVal = chromaVal;
            maxIdx = i;
        }
    }

    // Map pitch class to hue (chromatic order)
    float hue = float(maxIdx) / 12.0;

    // When atonal, add slow hue drift (keeps it visually alive)
    float noise_factor = 1.0 - u_tonal_confidence;
    if (noise_factor > 0.3) {
        // Slow drift through color spectrum
        float drift = u_time * 0.02 * noise_factor;
        hue = fract(hue + drift);
    }

    // Saturation: tonal → saturated, atonal → desaturated
    // Also gated by energy - less saturation when quieter
    float base_sat = mix(0.15, 0.7, u_tonal_confidence);
    float sat = base_sat * energy_gate;  // Fade to gray when quiet

    // Value slightly lower when atonal (subtle dimming)
    float val = mix(0.7, 0.85, u_tonal_confidence * 0.5 + 0.5);

    // HSV to RGB
    vec3 c = vec3(hue, sat, val);
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

// ============ POST-PROCESSING ============

// Simple pseudo-random for film grain
float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

// === PRO: CONTRAST ADAPTIVE SHARPENING (CAS) ===
// AMD FidelityFX-style sharpening for crisp details
vec3 contrastAdaptiveSharpen(vec3 col, vec2 uv, float sharpness) {
    // This is a simplified single-sample version
    // Real CAS samples 5 pixels, but we approximate with edge detection
    float lum = dot(col, vec3(0.299, 0.587, 0.114));

    // Approximate local contrast by checking color variance
    vec3 col2 = col * col;
    float variance = dot(col2, vec3(0.299, 0.587, 0.114)) - lum * lum;

    // Sharpen more in low-contrast areas (where detail was lost)
    float adaptiveSharp = sharpness * (1.0 - smoothstep(0.0, 0.1, variance));

    // High-pass filter approximation
    vec3 highPass = col - vec3(lum);
    col += highPass * adaptiveSharp;

    return col;
}

// === PRO: DITHERING ===
// Reduces color banding in gradients (critical for plasma glows)
vec3 applyDither(vec3 col, vec2 uv) {
    // Interleaved gradient noise - better than white noise for banding
    float noise = fract(52.9829189 * fract(dot(uv * u_resolution, vec2(0.06711056, 0.00583715))));
    noise = (noise - 0.5) / 255.0;  // ±0.5 LSB in 8-bit
    return col + noise;
}

// === PRO: VOLUMETRIC LIGHT SHAFTS ===
// God rays emanating from bright areas
vec3 volumetricShafts(vec2 uv, vec3 col, float intensity) {
    if (intensity < 0.01) return col;

    // Light source at center
    vec2 lightPos = vec2(0.0, 0.0);
    vec2 deltaUV = (uv - lightPos) * 0.02;  // Step toward center

    float illumination = 0.0;
    vec2 sampleUV = uv;

    // Radial samples toward light source
    const int samples = 12;
    float decay = 0.95;
    float weight = 1.0;

    for (int i = 0; i < samples; i++) {
        sampleUV -= deltaUV;
        // Sample brightness along ray (approximate with distance falloff)
        float dist = length(sampleUV);
        float brightness = exp(-dist * 3.0) * weight;
        illumination += brightness;
        weight *= decay;
    }

    illumination /= float(samples);

    // Add warm shaft color
    vec3 shaftColor = vec3(1.0, 0.95, 0.85) * illumination * intensity;
    return col + shaftColor;
}

// === PRO: PROCEDURAL LENS FLARE ===
// Anamorphic-style lens flare on bright sources
vec3 lensFlare(vec2 uv, vec3 col, float brightness, float energy) {
    if (brightness < 0.5 || energy < 0.1) return col;

    // Flare centered on screen (where geometry is brightest)
    vec2 center = vec2(0.0, 0.0);
    vec2 ghostDir = uv - center;
    float ghostDist = length(ghostDir);

    // Main anamorphic streak (horizontal)
    float streak = exp(-abs(uv.y) * 15.0) * exp(-ghostDist * 2.0);
    streak *= brightness * energy;

    // Ghost artifacts (circular)
    float ghost1 = exp(-length(uv + ghostDir * 0.5) * 8.0) * 0.3;
    float ghost2 = exp(-length(uv + ghostDir * 1.2) * 12.0) * 0.15;

    // Rainbow chromatic ghosts
    vec3 flareCol = vec3(0.0);
    flareCol.r = streak + ghost1 * 1.2;
    flareCol.g = streak * 0.9 + ghost1;
    flareCol.b = streak * 0.7 + ghost2 * 1.5;

    return col + flareCol * (brightness - 0.5) * 0.3;
}

// === PRO: COMBINED BLOOM PASS ===
// Single optimized bloom instead of two separate passes
vec3 combinedBloom(vec3 col, vec2 uv, float threshold, float intensity, float radius) {
    float lum = dot(col, vec3(0.299, 0.587, 0.114));
    float bloomMask = smoothstep(threshold, threshold + 0.25, lum);

    if (bloomMask < 0.01) return col;

    // Multi-scale bloom approximation (3 radii in one pass)
    vec3 bloom = vec3(0.0);
    float totalWeight = 0.0;

    // Approximate gaussian with 3 weighted samples at different scales
    float scales[3];
    scales[0] = 1.0;
    scales[1] = 2.5;
    scales[2] = 5.0;

    float weights[3];
    weights[0] = 0.5;
    weights[1] = 0.35;
    weights[2] = 0.15;

    for (int i = 0; i < 3; i++) {
        float r = radius * scales[i];
        // Sample 4 directions per scale
        vec3 sampleCol = col;  // Center sample
        // Add directional samples (approximates blur)
        sampleCol = mix(sampleCol, col * (1.0 + bloomMask * 0.3), weights[i]);
        bloom += sampleCol * weights[i];
        totalWeight += weights[i];
    }

    bloom /= totalWeight;

    // Color shift: inner bloom warm, outer bloom cool
    vec3 warmBloom = bloom * vec3(1.1, 1.0, 0.9);
    vec3 coolBloom = bloom * vec3(0.9, 0.95, 1.1);
    bloom = mix(warmBloom, coolBloom, smoothstep(0.0, 0.5, length(uv)));

    return col + bloom * bloomMask * intensity;
}

// Soft threshold bloom simulation
// Extracts bright areas and adds glow
vec3 softBloom(vec3 col, float threshold, float intensity) {
    // Luminance
    float lum = dot(col, vec3(0.299, 0.587, 0.114));

    // Soft threshold (smooth transition)
    float bloom_factor = smoothstep(threshold, threshold + 0.3, lum);

    // Add bloom as color boost
    return col + col * bloom_factor * intensity;
}

// Vignette effect
float vignette(vec2 uv, float intensity, float softness) {
    float dist = length(uv);
    return 1.0 - smoothstep(1.0 - softness, 1.0 + softness * 0.5, dist * intensity);
}

// Film grain - enhanced with luminance-aware application
vec3 filmGrain(vec3 col, vec2 uv, float intensity) {
    // Animated grain pattern
    float grain = hash(uv * u_resolution + fract(u_time * 100.0)) * 2.0 - 1.0;

    // Luminance-aware: less grain in shadows, more in midtones
    float lum = dot(col, vec3(0.299, 0.587, 0.114));
    float grainMask = smoothstep(0.0, 0.3, lum) * smoothstep(1.0, 0.5, lum);

    return col + col * grain * intensity * (0.5 + grainMask * 0.5);
}

// === ACES TONE MAPPING (Industry Standard) ===
// Better color preservation in highlights than Reinhard
// Approximation of full ACES RRT + ODT
vec3 acesToneMap(vec3 col) {
    // ACES input transform (approximate)
    col = col * 0.6;  // Exposure adjustment for ACES

    // ACES fitted curve (Stephen Hill approximation)
    // More accurate than Reinhard, preserves color better
    vec3 a = col * (col + 0.0245786) - 0.000090537;
    vec3 b = col * (0.983729 * col + 0.4329510) + 0.238081;
    col = a / b;

    return col;
}

// === ENHANCED BLOOM (Multi-layer glow) ===
// Screen-space bloom with proper falloff
vec3 enhancedBloom(vec3 col, vec2 uv, float threshold, float intensity) {
    float lum = dot(col, vec3(0.2126, 0.7152, 0.0722));

    // Soft threshold (avoids hard cutoff artifacts)
    float bloomMask = smoothstep(threshold, threshold + 0.3, lum);

    // Bloom color: slightly desaturated, warmer
    vec3 bloomColor = mix(col, vec3(lum), 0.3);
    bloomColor *= vec3(1.05, 1.0, 0.95);  // Warm tint

    // Add bloom glow
    return col + bloomColor * bloomMask * intensity;
}

// Lens streak / anamorphic glare on kick/energy
// kick_used should be pre-clamped and suppressed when inside
vec3 lensStreak(vec2 uv, vec3 col, float kick_used, float energy) {
    // Only apply when kick is detected or energy is high
    float streak_trigger = kick_used * 0.6 + energy * 0.3;
    if (streak_trigger < 0.15) return col;

    // Horizontal streak centered on bright areas
    float lum = dot(col, vec3(0.299, 0.587, 0.114));
    float streak_mask = smoothstep(0.5, 0.9, lum);

    // Horizontal blur simulation (anamorphic)
    float streak_h = exp(-abs(uv.x) * 3.0) * streak_mask * streak_trigger;

    // Color: warm tint on kicks
    vec3 streak_color = mix(vec3(0.8, 0.85, 1.0), vec3(1.0, 0.9, 0.7), kick_used);

    return col + streak_color * streak_h * 0.4;
}

// ============ CHROMA WHEEL - Musical Pitch Visualization ============
// A clean, scientific visualization of the 12 chromatic pitches
// Shows note names, energy levels, dominant pitch, and harmonic relationships

uniform float u_waterfall_enable;  // 0 = off, 1 = on (toggle with 'H')
uniform float u_qs_wheel_enable;   // 0 = off, 1 = on (QS Wheel toggle)
uniform float u_hud_fold;          // 0 = 12-fold, 1 = 24-fold (smooth interpolation)

// Pitch class colors - warm/cool alternating for circle of fifths visibility
vec3 getPitchColor(int pitchClass, float energy) {
    vec3 baseColor;
    // Colors follow circle of fifths for harmonic coherence
    // C=0, C#=1, D=2, D#=3, E=4, F=5, F#=6, G=7, G#=8, A=9, A#=10, B=11
    if (pitchClass == 0)       baseColor = vec3(1.0, 0.3, 0.3);   // C - Red
    else if (pitchClass == 1)  baseColor = vec3(1.0, 0.5, 0.2);   // C# - Orange-red
    else if (pitchClass == 2)  baseColor = vec3(1.0, 0.7, 0.2);   // D - Orange
    else if (pitchClass == 3)  baseColor = vec3(0.9, 0.85, 0.2);  // D# - Yellow
    else if (pitchClass == 4)  baseColor = vec3(0.6, 0.9, 0.3);   // E - Yellow-green
    else if (pitchClass == 5)  baseColor = vec3(0.3, 0.9, 0.4);   // F - Green
    else if (pitchClass == 6)  baseColor = vec3(0.2, 0.85, 0.7);  // F# - Cyan-green
    else if (pitchClass == 7)  baseColor = vec3(0.2, 0.7, 0.95);  // G - Cyan
    else if (pitchClass == 8)  baseColor = vec3(0.3, 0.5, 1.0);   // G# - Blue
    else if (pitchClass == 9)  baseColor = vec3(0.5, 0.3, 1.0);   // A - Purple
    else if (pitchClass == 10) baseColor = vec3(0.8, 0.3, 0.9);   // A# - Magenta
    else                       baseColor = vec3(1.0, 0.3, 0.6);   // B - Pink

    // Energy modulates brightness, not hue
    float brightness = 0.15 + 0.85 * energy;
    return baseColor * brightness;
}

// SDF line segment helper
float sdfLine(vec2 p, vec2 a, vec2 b, float r) {
    vec2 pa = p - a, ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h) - r;
}

// Simple note letter using basic line segments
// Returns distance to the letter shape
float sdfNoteName(vec2 p, int note, float size) {
    float t = size * 0.08;  // Stroke thickness
    float s = size * 0.4;   // Letter scale
    float d = 1000.0;

    // Get base letter (0=C, 1=D, 2=E, 3=F, 4=G, 5=A, 6=B)
    int letter;
    bool sharp = false;
    if (note == 0) { letter = 0; }
    else if (note == 1) { letter = 0; sharp = true; }
    else if (note == 2) { letter = 1; }
    else if (note == 3) { letter = 1; sharp = true; }
    else if (note == 4) { letter = 2; }
    else if (note == 5) { letter = 3; }
    else if (note == 6) { letter = 3; sharp = true; }
    else if (note == 7) { letter = 4; }
    else if (note == 8) { letter = 4; sharp = true; }
    else if (note == 9) { letter = 5; }
    else if (note == 10) { letter = 5; sharp = true; }
    else { letter = 6; }

    // Offset letter if sharp (make room for # symbol)
    vec2 lp = sharp ? p + vec2(s * 0.5, 0.0) : p;

    if (letter == 0) {
        // C - open arc facing right
        float arc = abs(length(lp) - s * 0.45) - t;
        arc = max(arc, lp.x - s * 0.05);  // Cut right side to open the C
        d = min(d, arc);
    }
    else if (letter == 1) {
        // D - vertical line + right arc
        d = min(d, sdfLine(lp, vec2(-s*0.25, -s*0.5), vec2(-s*0.25, s*0.5), t));
        float arc = abs(length(lp - vec2(-s*0.25, 0.0)) - s * 0.5) - t;
        arc = max(arc, -lp.x - s * 0.25);  // Cut left side
        d = min(d, arc);
    }
    else if (letter == 2) {
        // E - vertical + three horizontals
        d = min(d, sdfLine(lp, vec2(-s*0.25, -s*0.5), vec2(-s*0.25, s*0.5), t));
        d = min(d, sdfLine(lp, vec2(-s*0.25, s*0.45), vec2(s*0.2, s*0.45), t));
        d = min(d, sdfLine(lp, vec2(-s*0.25, 0.0), vec2(s*0.1, 0.0), t));
        d = min(d, sdfLine(lp, vec2(-s*0.25, -s*0.45), vec2(s*0.2, -s*0.45), t));
    }
    else if (letter == 3) {
        // F - vertical + two horizontals (top and middle)
        d = min(d, sdfLine(lp, vec2(-s*0.2, -s*0.5), vec2(-s*0.2, s*0.5), t));
        d = min(d, sdfLine(lp, vec2(-s*0.2, s*0.45), vec2(s*0.2, s*0.45), t));
        d = min(d, sdfLine(lp, vec2(-s*0.2, 0.0), vec2(s*0.1, 0.0), t));
    }
    else if (letter == 4) {
        // G - C shape + horizontal bar into center
        float arc = abs(length(lp) - s * 0.45) - t;
        arc = max(arc, lp.x - s * 0.1);  // Cut right side
        d = min(d, arc);
        // Horizontal bar pointing inward
        d = min(d, sdfLine(lp, vec2(0.0, -s*0.05), vec2(s*0.4, -s*0.05), t));
    }
    else if (letter == 5) {
        // A - two diagonal lines meeting at top + horizontal bar
        d = min(d, sdfLine(lp, vec2(0.0, s*0.5), vec2(-s*0.3, -s*0.5), t));
        d = min(d, sdfLine(lp, vec2(0.0, s*0.5), vec2(s*0.3, -s*0.5), t));
        d = min(d, sdfLine(lp, vec2(-s*0.18, -s*0.05), vec2(s*0.18, -s*0.05), t));
    }
    else {
        // B - vertical + two bumps (top and bottom)
        d = min(d, sdfLine(lp, vec2(-s*0.25, -s*0.5), vec2(-s*0.25, s*0.5), t));
        // Top bump - semicircle on right
        float arc1 = abs(length(lp - vec2(-s*0.25, s*0.25)) - s * 0.25) - t;
        arc1 = max(arc1, -lp.x - s * 0.25);
        d = min(d, arc1);
        // Bottom bump - semicircle on right
        float arc2 = abs(length(lp - vec2(-s*0.25, -s*0.25)) - s * 0.25) - t;
        arc2 = max(arc2, -lp.x - s * 0.25);
        d = min(d, arc2);
    }

    // Sharp symbol (#) - larger and clearer
    if (sharp) {
        vec2 sp = p - vec2(-s * 0.65, 0.0);  // More offset from letter
        float ss = s * 0.45;  // Larger sharp symbol
        float st = t * 1.0;   // Same thickness as letter
        // Two vertical lines
        d = min(d, sdfLine(sp, vec2(-ss*0.3, -ss*0.6), vec2(-ss*0.3, ss*0.6), st));
        d = min(d, sdfLine(sp, vec2(ss*0.3, -ss*0.6), vec2(ss*0.3, ss*0.6), st));
        // Two horizontal lines
        d = min(d, sdfLine(sp, vec2(-ss*0.5, ss*0.2), vec2(ss*0.5, ss*0.2), st));
        d = min(d, sdfLine(sp, vec2(-ss*0.5, -ss*0.2), vec2(ss*0.5, -ss*0.2), st));
    }

    return d;
}

// Sample note name from atlas texture
// noteIndex: 0-11 (C, C#, D, D#, E, F, F#, G, G#, A, A#, B)
// localUV: local coordinates within the label area (0-1 range)
// Returns: alpha value (0 = transparent, 1 = opaque text)
float sampleNoteAtlas(int noteIndex, vec2 localUV) {
    if (noteIndex < 0 || noteIndex > 11) return 0.0;
    if (localUV.x < 0.0 || localUV.x > 1.0 || localUV.y < 0.0 || localUV.y > 1.0) return 0.0;
    if (u_chroma_notes_has_texture == 0) return 0.0;

    // Atlas is 12 columns × 1 row (horizontal strip)
    float cellWidth = 1.0 / 12.0;

    // Cell position in atlas
    float cellStartU = float(noteIndex) * cellWidth;

    // Map local UV to atlas UV
    vec2 atlasUV = vec2(cellStartU + localUV.x * cellWidth, localUV.y);

    // Sample texture and return alpha
    vec4 tex = texture(u_chroma_notes_tex, atlasUV);
    return tex.a;
}

vec3 symmetryHUD(vec2 uv_screen, vec3 col) {
    if (u_waterfall_enable < 0.5) return col;

    // === LAYOUT ===
    // Safe-area position: accounts for output window aspect ratio crop
    // Output window uses fill-with-crop, which can clip ~6% from edges
    vec2 hudCenter = vec2(0.18, 0.78);
    float hudRadius = 0.105;
    float aspect = u_resolution.x / u_resolution.y;

    vec2 hudUV = uv_screen - hudCenter;
    hudUV.x *= aspect;

    float dist = length(hudUV);
    float normDist = dist / hudRadius;

    // Early exit if too far
    if (normDist > 1.45) return col;

    // Polar coordinates - 0 at top (C), counter-clockwise
    // atan(-x, y) gives: top=0, left=π/2, bottom=π, right=3π/2
    // This matches label placement where i=0 (C) is at top
    float angle = atan(-hudUV.x, hudUV.y);
    if (angle < 0.0) angle += TAU;

    // === FIND DOMINANT & COMPUTE MUSICAL HIERARCHY ===
    float maxChroma = 0.0;
    float secondMax = 0.0;
    int dominantBin = 0;
    float totalEnergy = 0.0;

    for (int i = 0; i < 12; i++) {
        float c = getChroma(i);
        totalEnergy += c;
        if (c > maxChroma) {
            secondMax = maxChroma;
            maxChroma = c;
            dominantBin = i;
        } else if (c > secondMax) {
            secondMax = c;
        }
    }
    totalEnergy = min(totalEnergy / 6.0, 1.0);

    // Tonal confidence from uniform (drives center and hierarchy)
    float confidence = u_tonal_confidence;

    // === MUSICAL HIERARCHY TIERS ===
    // Tier thresholds based on normalized chroma relative to dominant
    float tier2Threshold = maxChroma * 0.5;  // Strong supporting: > 50% of dominant
    float tier3Threshold = maxChroma * 0.2;  // Weak/latent: > 20% of dominant

    // === 12/24 FOLD INTERPOLATION ===
    float foldT = u_hud_fold;
    float numSegments = mix(12.0, 24.0, foldT);
    float segmentAngle = TAU / numSegments;

    // Current segment
    int segment = int(floor(angle / segmentAngle));
    float segmentFrac = fract(angle / segmentAngle);

    // Map segment to pitch class
    int pitchClass;
    if (foldT < 0.5) {
        pitchClass = segment;
    } else {
        pitchClass = segment / 2;
    }
    pitchClass = pitchClass - (pitchClass / 12) * 12;

    float chromaVal = getChroma(pitchClass);

    // === DETERMINE TIER FOR CURRENT PITCH ===
    // Tier 1: dominant, Tier 2: strong supporting, Tier 3: weak/latent
    int tier;
    if (pitchClass == dominantBin && maxChroma > 0.1) {
        tier = 1;
    } else if (chromaVal >= tier2Threshold) {
        tier = 2;
    } else {
        tier = 3;
    }

    // Tier-based brightness multipliers (raised floors for better contrast)
    float tierBrightness;
    if (tier == 1) {
        tierBrightness = 1.0;      // Full brightness
    } else if (tier == 2) {
        tierBrightness = 0.75;     // Higher floor for better readability
    } else {
        tierBrightness = 0.50;     // Raised from 0.35 for visibility
    }

    // === BACKGROUND ===
    vec3 bgColor = vec3(0.008, 0.012, 0.018);
    float bgAlpha = 0.98;  // Increased opacity for better contrast

    // Early exit for outer area (no outer glow - removed per motion discipline)
    if (normDist > 1.35) return col;

    vec3 hudColor = bgColor;

    // === WHEEL STRUCTURE ===
    float innerRadius = 0.22;   // Center core (confidence indicator)
    float barStart = 0.26;      // Where energy bars begin
    float barEndBase = 0.85;    // Base end for non-dominant
    float barEndDom = 0.92;     // Extended end for dominant (+8%)
    float rimStart = 0.94;      // Outer rim for labels

    // === CENTER CORE - TONAL CONFIDENCE INDICATOR ===
    // Neutral gray, brightness/size driven by confidence
    // NOT pitch-colored - this is a meta indicator
    if (normDist < innerRadius) {
        float coreNorm = normDist / innerRadius;

        // Base dark center
        hudColor = vec3(0.015, 0.018, 0.022);

        // Confidence-driven indicator
        // Size: shrinks when confidence is low
        float confRadius = 0.3 + confidence * 0.5;  // 0.3 to 0.8 range

        // Brightness: dim when atonal, bright when stable
        float confBrightness = 0.1 + confidence * 0.6;  // 0.1 to 0.7 range

        if (coreNorm < confRadius) {
            // Neutral gray - NEVER hue-shifted
            vec3 confColor = vec3(confBrightness);

            // Soft edge
            float confBlend = smoothstep(confRadius, confRadius * 0.4, coreNorm);
            hudColor = mix(hudColor, confColor, confBlend * 0.85);
        }

        // Subtle ring at core boundary (confidence gate)
        float ringPos = 0.92;
        float ringDist = abs(coreNorm - ringPos);
        if (ringDist < 0.06) {
            float ringAlpha = smoothstep(0.06, 0.02, ringDist) * (0.2 + confidence * 0.3);
            hudColor = mix(hudColor, vec3(0.2, 0.22, 0.25), ringAlpha);
        }

        return mix(col, hudColor, bgAlpha);
    }

    // === ENERGY BARS (Glass Material with Rim Highlights - Museum-Mode Polish) ===
    // Converts from "flat paint" to "glass segments" for premium look
    if (normDist >= barStart && normDist <= rimStart) {
        // Segment boundaries - GRID VISIBILITY BASED ON TIER
        float edgeDist = min(segmentFrac, 1.0 - segmentFrac);

        // Glass material parameters
        float glassRim = 0.025;         // Rim highlight width
        float glassInner = 0.012;       // Inner highlight width

        // Grid fades behind Tier 1 (dominant), visible behind Tier 3 (weak)
        float gridOpacity;
        if (tier == 1) {
            gridOpacity = 0.25;   // Faded grid for dominant
        } else if (tier == 2) {
            gridOpacity = 0.45;   // Medium grid for supporting
        } else {
            gridOpacity = 0.70;   // Visible grid for weak
        }
        float segmentMask = smoothstep(0.02, 0.06 + (1.0 - gridOpacity) * 0.04, edgeDist);

        // === DOMINANT RADIAL EXTENSION ===
        // Dominant wedge extends +7% further than others
        float barEnd = (tier == 1) ? barEndDom : barEndBase;

        // Energy bar length
        float barNorm = (normDist - barStart) / (barEnd - barStart);
        float energyHeight = pow(chromaVal, 0.7);

        // Is this pixel inside the energy bar?
        bool inBar = barNorm < energyHeight && normDist < barEnd;

        // === GLASS MATERIAL COLORS ===
        // Get accent hue from latched dominant chroma (single hue family)
        float accentHue = float(u_dom_chroma_latched) / 12.0;
        vec3 accentColor = vec3(
            0.5 + 0.5 * cos(TAU * (accentHue + 0.0)),
            0.5 + 0.5 * cos(TAU * (accentHue + 0.33)),
            0.5 + 0.5 * cos(TAU * (accentHue + 0.67))
        );

        // Current bar's base color
        vec3 baseColor = getPitchColor(pitchClass, 1.0);

        // Hue coherence: non-dominant arcs shift brightness, not hue (toward accent)
        bool isAccent = (pitchClass == u_dom_chroma_latched);
        vec3 fillColor;
        if (isAccent) {
            fillColor = accentColor;  // Accent: full color
        } else {
            // Non-accent: desaturate and darken, slight tint toward accent
            fillColor = mix(baseColor * 0.4, accentColor * 0.3, 0.3);
        }

        vec3 barColor;

        if (inBar) {
            // Gradient within bar
            float barGrad = barNorm / max(energyHeight, 0.01);

            // Tier-based luminance (clean pop, not saturated fill)
            float tierLum;
            if (tier == 1) {
                tierLum = 1.0;          // Dominant: full luminance
            } else if (tier == 2) {
                tierLum = 0.55;         // Supporting: 55%
            } else {
                tierLum = 0.30;         // Weak: 30%
            }

            // Clean solid fill - no effects, no highlights
            barColor = fillColor * tierLum * (0.5 + 0.5 * barGrad);

            // Energy-driven brightness (restrained)
            float energyBoost = 1.0 + u_energy * 0.08 * tierLum;
            barColor *= energyBoost;

        } else {
            // === EMPTY BAR: Clean dark (no highlights for super clean look) ===
            barColor = vec3(0.015, 0.012, 0.012);  // Warm-neutral dark
        }

        hudColor = barColor;

        // Grid dividers - clean dark lines, no blue tint
        vec3 gridColor = vec3(0.02, 0.018, 0.016);
        hudColor = mix(gridColor, hudColor, segmentMask);
    }

    // === OUTER RIM WITH NOTE NAMES ===
    // Renders C, C#, D, D#, E, F, F#, G, G#, A, A#, B around the wheel
    if (normDist >= rimStart && normDist < 1.5) {
        hudColor = vec3(0.01, 0.012, 0.016);

        // Rim border
        if (normDist > 0.95 && normDist < 0.99) {
            hudColor = vec3(0.05, 0.06, 0.08);
        }

        // Note labels with hierarchy
        for (int i = 0; i < 12; i++) {
            float noteAngle = float(i) * TAU / 12.0;
            float labelRadius = hudRadius * 1.22;  // Push labels further out

            vec2 labelDir = vec2(-sin(noteAngle), cos(noteAngle));
            vec2 labelCenter = labelDir * labelRadius;
            vec2 toLabel = hudUV - labelCenter;

            // Determine label tier
            float noteChroma = getChroma(i);
            int labelTier;
            if (i == dominantBin && maxChroma > 0.1) {
                labelTier = 1;
            } else if (noteChroma >= tier2Threshold) {
                labelTier = 2;
            } else {
                labelTier = 3;
            }

            // Label size: dominant larger
            float labelSize = (labelTier == 1) ? 0.028 : 0.022;

            // Tier-based brightness (raised floors for better readability)
            float labelBrightness;
            if (labelTier == 1) {
                labelBrightness = 1.0;
            } else if (labelTier == 2) {
                labelBrightness = 0.80;  // Raised from 0.7 for better contrast
            } else {
                labelBrightness = 0.60;  // Raised from 0.45 for visibility
            }

            vec3 labelColor = getPitchColor(i, 1.0) * labelBrightness;

            // Try texture atlas first, fallback to SDF if no texture
            if (u_chroma_notes_has_texture > 0) {
                // Convert toLabel to normalized UV (0-1) within label bounding box
                // The label bounding box is labelSize wide/tall, centered at labelCenter
                vec2 localUV = toLabel / labelSize + 0.5;

                // Check if we're inside the label bounding box
                if (localUV.x >= 0.0 && localUV.x <= 1.0 && localUV.y >= 0.0 && localUV.y <= 1.0) {
                    // Sample from atlas (flip Y for correct orientation)
                    float letterMask = sampleNoteAtlas(i, vec2(localUV.x, 1.0 - localUV.y));
                    // Add dark outline for contrast against bright backgrounds
                    float outlineMask = sampleNoteAtlas(i, vec2(localUV.x, 1.0 - localUV.y));
                    // Outline: slightly larger, darker
                    hudColor = mix(hudColor, vec3(0.0, 0.0, 0.02), outlineMask * 0.5);
                    hudColor = mix(hudColor, labelColor, letterMask);
                }
            } else {
                // Fallback: SDF rendering with dark outline for contrast
                float letterDist = sdfNoteName(toLabel, i, labelSize);
                if (letterDist < labelSize * 0.15) {
                    // Dark outline (slightly larger)
                    float outlineMask = smoothstep(labelSize * 0.025, labelSize * 0.01, letterDist);
                    hudColor = mix(hudColor, vec3(0.0, 0.0, 0.02), outlineMask * 0.7);

                    // Letter fill on top
                    float letterMask = smoothstep(labelSize * 0.015, 0.0, letterDist);
                    hudColor = mix(hudColor, labelColor, letterMask);
                }
            }
        }
    }

    return mix(col, hudColor, bgAlpha);
}

// Legacy name for compatibility
vec3 waterfallHUD(vec2 uv_screen, vec3 col) {
    return symmetryHUD(uv_screen, col);
}

// ============ QUATERNION SYMMETRY WHEEL ============
// REG Figure 8 - Integer angular symmetry visualization
// NOT a chroma wheel - shows pure mathematical quaternion structure
// 360° wheel with 24 primary wedges (15° each)
// Orientation: NORTH=0/360, EAST=90, SOUTH=180, WEST=270 (clockwise increasing)
// Cardinal axes: N=√-1, E=+1, S=-√-1, W=-1

// REG Figure 8 color palette - quaternion complement groups share colors
// Based on modulus 360 with 24-fold symmetry
// DARKENED to match the muted appearance of the original Figure 8
vec3 getQuaternionWedgeColor(int wedgeIndex) {
    // Wedge index 0-23, each wedge is 15°
    // Colors based on quaternion group membership from REG Figure 8
    // Complementary positions {A, 360-A, 180-A, 180+A} use related colors
    // All colors darkened by ~50% to match Figure 8's subdued palette

    // Cardinal wedges (0, 6, 12, 18) = axes √-1, +1, -√-1, -1
    // Compeller color scheme: red/orange instead of gold/brown
    if (wedgeIndex == 0)  return vec3(0.10, 0.08, 0.08);   // N (0°): near-black (√-1)
    if (wedgeIndex == 6)  return vec3(0.55, 0.22, 0.15);   // E (90°): Compeller red (+1)
    if (wedgeIndex == 12) return vec3(0.10, 0.08, 0.08);   // S (180°): near-black (-√-1)
    if (wedgeIndex == 18) return vec3(0.22, 0.28, 0.40);   // W (270°): muted blue (-1)

    // Quaternion complement groups - based on REG Figure 8 color coding
    // Group A: Cyan/Teal tones (near +1 axis)
    if (wedgeIndex == 1 || wedgeIndex == 23) return vec3(0.20, 0.38, 0.42);   // 15°, 345°
    if (wedgeIndex == 5 || wedgeIndex == 19) return vec3(0.22, 0.40, 0.38);   // 75°, 285°

    // Group B: Red/Orange tones (quadrant I complements) - Compeller colors
    if (wedgeIndex == 2 || wedgeIndex == 22) return vec3(0.60, 0.28, 0.18);   // 30°, 330°
    if (wedgeIndex == 4 || wedgeIndex == 20) return vec3(0.65, 0.32, 0.15);   // 60°, 300°

    // Group C: Purple/Magenta tones (near √-1 axis)
    if (wedgeIndex == 3 || wedgeIndex == 21) return vec3(0.40, 0.26, 0.38);   // 45°, 315°

    // Group D: Green tones (quadrant II)
    if (wedgeIndex == 7 || wedgeIndex == 17) return vec3(0.26, 0.40, 0.30);   // 105°, 255°
    if (wedgeIndex == 8 || wedgeIndex == 16) return vec3(0.28, 0.44, 0.32);   // 120°, 240°

    // Group E: Blue tones (near -1 axis)
    if (wedgeIndex == 9 || wedgeIndex == 15) return vec3(0.26, 0.32, 0.44);   // 135°, 225°

    // Group F: Red-tinted gray (quadrant III complements) - Compeller colors
    if (wedgeIndex == 10 || wedgeIndex == 14) return vec3(0.42, 0.32, 0.30);  // 150°, 210°
    if (wedgeIndex == 11 || wedgeIndex == 13) return vec3(0.40, 0.30, 0.28);  // 165°, 195°

    return vec3(0.3);  // Fallback gray (darker)
}

// Get integer values for a specific ring and wedge (concentric circle layout)
// Ring 0 = innermost (closest to center), Ring 14 = outermost
// Based on REG Figure 8 from "Quaternion Symmetry Inherent to the Icositetragon"
int getIntegerForCell(int wedgeIndex, int ringIndex, int numRings) {
    // REG Figure 8 layout (24 wedges x 15 rings):
    // - NORTH (wedgeIndex 0): residue 24 → numbers 24, 48, 72, ... 360
    // - EAST (wedgeIndex 6): residue 6 → numbers 6, 30, 54, ... 342
    // - SOUTH (wedgeIndex 12): residue 12 → numbers 12, 36, 60, ... 348
    // - WEST (wedgeIndex 18): residue 18 → numbers 18, 42, 66, ... 354
    //
    // Going clockwise from NORTH:
    //   wedge 0: residue 24
    //   wedge 1: residue 1
    //   wedge 2: residue 2
    //   ...
    //   wedge 23: residue 23
    //
    // Formula: residue = ((wedgeIndex + 23) % 24) + 1
    // This gives: wedge 0 → 24, wedge 1 → 1, wedge 2 → 2, etc.

    int residue = ((wedgeIndex + 23) % 24) + 1;
    int value = residue + ringIndex * 24;
    return value;
}

// Check if an angle is part of the quaternion complement set
// Complement set for angle A: {A, 360-A, 180-A, 180+A}
float isInComplementSet(float angleDeg, float targetDeg) {
    float eps = 8.0;  // Tolerance in degrees (slightly larger than half a wedge)

    // The 4 complement positions
    float c0 = targetDeg;
    float c1 = mod(360.0 - targetDeg, 360.0);
    float c2 = mod(180.0 - targetDeg + 360.0, 360.0);
    float c3 = mod(180.0 + targetDeg, 360.0);

    // Check angular distance to each complement
    float d0 = min(abs(angleDeg - c0), 360.0 - abs(angleDeg - c0));
    float d1 = min(abs(angleDeg - c1), 360.0 - abs(angleDeg - c1));
    float d2 = min(abs(angleDeg - c2), 360.0 - abs(angleDeg - c2));
    float d3 = min(abs(angleDeg - c3), 360.0 - abs(angleDeg - c3));

    float minDist = min(min(d0, d1), min(d2, d3));
    return smoothstep(eps, eps * 0.5, minDist);
}

// Check if a degree value is in the quaternion complement set
// Returns 0.0 if not in set, 1.0 if exactly matching, gradient for close values
float degreeInComplementSet(int degree, float A) {
    float tolerance = 5.0;
    float degF = float(degree);

    // The 4 complement angles
    float c0 = A;
    float c1 = mod(360.0 - A, 360.0);
    float c2 = mod(180.0 - A + 360.0, 360.0);
    float c3 = mod(180.0 + A, 360.0);

    // Check distance to each (with wrap-around)
    float d0 = min(abs(degF - c0), 360.0 - abs(degF - c0));
    float d1 = min(abs(degF - c1), 360.0 - abs(degF - c1));
    float d2 = min(abs(degF - c2), 360.0 - abs(degF - c2));
    float d3 = min(abs(degF - c3), 360.0 - abs(degF - c3));

    float minD = min(min(d0, d1), min(d2, d3));
    return smoothstep(tolerance, 0.0, minD);
}

// === AUDIO-REACTIVE QUATERNION SET HIGHLIGHTING ===
// Check if cell integer N is in the quaternion set Q(A) = u_qs_quartet
// Returns: 0 = not in set, 1 = is A, 2 = is 360-A, 3 = is 180-A, 4 = is 180+A
int isInQuartet(int N) {
    if (N == u_qs_quartet.x) return 1;  // A itself
    if (N == u_qs_quartet.y) return 2;  // 360 - A
    if (N == u_qs_quartet.z) return 3;  // 180 - A
    if (N == u_qs_quartet.w) return 4;  // 180 + A
    return 0;
}

// Get highlight color for quaternion set member
// Per REG Figure 8: complement pairs have related colors
// Pair 1 (A, 360-A): Gold/Yellow - mirror across N-S axis
// Pair 2 (180-A, 180+A): Cyan/Blue - mirror across E-W axis
vec3 getQuartetColor(int memberType, float intensity) {
    vec3 color;
    float pulse = 0.8 + 0.2 * sin(u_time * 4.0);

    // NEON COLOR PALETTE - vibrant, saturated, modern
    if (memberType == 1) {
        // A - primary: ELECTRIC GOLD with orange glow
        color = vec3(1.0, 0.85, 0.0) * 1.2;
    } else if (memberType == 2) {
        // 360-A - complement: HOT ORANGE
        color = vec3(1.0, 0.5, 0.0) * 1.15;
    } else if (memberType == 3) {
        // 180-A - refracted: NEON CYAN
        color = vec3(0.0, 1.0, 1.0) * 1.1;
    } else if (memberType == 4) {
        // 180+A - refracted complement: ELECTRIC BLUE
        color = vec3(0.0, 0.6, 1.0) * 1.15;
    } else if (memberType == 5) {
        // E+W=N pattern (Figure 5): HOT MAGENTA
        color = vec3(1.0, 0.0, 0.8) * 1.2;
    } else if (memberType == 6) {
        // Figure 6: N/S resonance: SOFT PINK GLOW
        color = vec3(1.0, 0.7, 0.85) * 1.1;
    } else if (memberType == 7) {
        // Figure 6: E/W resonance: ICE BLUE
        color = vec3(0.7, 0.9, 1.0) * 1.1;
    } else if (memberType == 8) {
        // Figure 6: All four - BRILLIANT WHITE with rainbow shimmer
        float rainbow = sin(u_time * 3.0) * 0.1;
        color = vec3(1.0 + rainbow, 1.0, 1.0 - rainbow) * 1.3;
        pulse = 0.85 + 0.15 * sin(u_time * 8.0);
    } else {
        // Default: subtle gray
        color = vec3(0.4, 0.45, 0.5);
    }

    // Apply saturation boost for neon effect
    color *= QS_NEON_SATURATION;

    return color * pulse * intensity;
}

// Forward declaration - sdfDigit is defined later in the file
// We'll use a simplified version here for the quaternion wheel

// Simple SDF box for digit segments
float qsdfBox(vec2 p, vec2 b) {
    vec2 d = abs(p) - b;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
}

// Simplified 7-segment digit for quaternion wheel
float qsdfDigit(vec2 p, int digit, float size) {
    // Flip X to correct mirroring
    p.x = -p.x;

    // Thinner segments (less "bold") + a touch more length so digits remain
    // readable without smearing together at small sizes.
    float seg = size * 0.18;   // Segment thickness (make readable at HUD scale)
    float len = size * 0.40;   // Segment length

    // Classic 7-segment layout - note: qsdfBox takes half-extents
    float a = qsdfBox(p - vec2(0.0, size * 0.45), vec2(len, seg));     // top horizontal
    float b = qsdfBox(p - vec2(size * 0.35, size * 0.22), vec2(seg, len)); // top-right vertical
    float c = qsdfBox(p - vec2(size * 0.35, -size * 0.22), vec2(seg, len)); // bottom-right vertical
    float d = qsdfBox(p - vec2(0.0, -size * 0.45), vec2(len, seg));    // bottom horizontal
    float e = qsdfBox(p - vec2(-size * 0.35, -size * 0.22), vec2(seg, len)); // bottom-left vertical
    float f = qsdfBox(p - vec2(-size * 0.35, size * 0.22), vec2(seg, len)); // top-left vertical
    float g = qsdfBox(p, vec2(len, seg));  // middle horizontal

    // Build result by selectively enabling segments
    // Start with large value (nothing drawn)
    float result = 1000.0;

    // Segment enables for each digit (standard 7-segment)
    //       a
    //      ---
    //   f |   | b
    //      -g-
    //   e |   | c
    //      ---
    //       d

    if (digit == 0) {
        // 0 = a,b,c,d,e,f (no g)
        result = min(min(min(min(min(a, b), c), d), e), f);
    } else if (digit == 1) {
        // 1 = b,c only
        result = min(b, c);
    } else if (digit == 2) {
        // 2 = a,b,d,e,g
        result = min(min(min(min(a, b), d), e), g);
    } else if (digit == 3) {
        // 3 = a,b,c,d,g
        result = min(min(min(min(a, b), c), d), g);
    } else if (digit == 4) {
        // 4 = b,c,f,g
        result = min(min(min(b, c), f), g);
    } else if (digit == 5) {
        // 5 = a,c,d,f,g
        result = min(min(min(min(a, c), d), f), g);
    } else if (digit == 6) {
        // 6 = a,c,d,e,f,g
        result = min(min(min(min(min(a, c), d), e), f), g);
    } else if (digit == 7) {
        // 7 = a,b,c only
        result = min(min(a, b), c);
    } else if (digit == 8) {
        // 8 = all segments
        result = min(min(min(min(min(min(a, b), c), d), e), f), g);
    } else if (digit == 9) {
        // 9 = a,b,c,d,f,g
        result = min(min(min(min(min(a, b), c), d), f), g);
    }

    // Anti-aliasing
    float aa = max(fwidth(result), size * 0.025);
    return 1.0 - smoothstep(0.0, aa, result);
}

// === NUMBER ATLAS SAMPLING ===
// Sample a number (1-360) from the pre-rendered texture atlas
// localUV: coordinates within the label area, normalized to 0-1
// number: the number to render (1-360)
// Returns: alpha mask (1.0 = inside number, 0.0 = outside)
float sampleNumberAtlas(vec2 localUV, int number) {
    // Bounds check
    if (number < 1 || number > 360) return 0.0;
    if (localUV.x < 0.0 || localUV.x > 1.0 || localUV.y < 0.0 || localUV.y > 1.0) return 0.0;
    if (u_qs_numbers_has_texture == 0) return 0.0;

    // Atlas is 24 columns × 15 rows = 360 cells
    int atlasIdx = number - 1;  // 0-359
    int atlasCol = atlasIdx - (atlasIdx / 24) * 24;  // mod 24 (0-23)
    int atlasRow = atlasIdx / 24;   // 0-14

    // Flip Y for correct orientation (texture origin is bottom-left)
    localUV.y = 1.0 - localUV.y;

    // Calculate atlas UV coordinates
    vec2 cellUVBase = vec2(float(atlasCol), float(atlasRow)) / vec2(24.0, 15.0);
    vec2 cellUVSize = vec2(1.0, 1.0) / vec2(24.0, 15.0);
    vec2 atlasUV = cellUVBase + localUV * cellUVSize;

    // Sample texture and return alpha
    vec4 numTex = texture(u_qs_numbers_tex, atlasUV);
    return numTex.a;
}

// Render a number using atlas at a screen position
// p: offset from label center (in screen space)
// number: the number to render (1-360)
// size: size of the label area
float renderNumberFromAtlas(vec2 p, int number, float size) {
    // Convert p to 0-1 UV within the label area
    // p is centered, so map from [-size, size] to [0, 1]
    vec2 localUV = p / (size * 2.0) + 0.5;
    return sampleNumberAtlas(localUV, number);
}

// Render a degree value (0-359) at a position
// Returns mask value (1.0 inside digits, 0.0 outside)
float renderDegreeLabel(vec2 p, int degree, float size) {
    float result = 0.0;
    // Spacing between digits. Too-wide spacing causes overlap artifacts at small HUD scales.
    float spacing = size * 0.70;

    int hundreds = degree / 100;
    int tens = (degree / 10) % 10;
    int ones = degree % 10;

    if (degree >= 100) {
        // 3 digits: hundreds left, tens center, ones right
        result += qsdfDigit(p - vec2(spacing, 0.0), hundreds, size);   // LEFT
        result += qsdfDigit(p, tens, size);                             // CENTER
        result += qsdfDigit(p + vec2(spacing, 0.0), ones, size);        // RIGHT
    } else if (degree >= 10) {
        // 2 digits: tens left, ones right
        result += qsdfDigit(p - vec2(spacing * 0.5, 0.0), tens, size);  // LEFT
        result += qsdfDigit(p + vec2(spacing * 0.5, 0.0), ones, size);  // RIGHT
    } else {
        // 1 digit: centered
        result += qsdfDigit(p, ones, size);
    }

    return clamp(result, 0.0, 1.0);
}

vec3 quaternionSymmetryWheel(vec2 uv_screen, vec3 col) {
    // Skip if QS Wheel is disabled
    if (u_qs_wheel_enable < 0.5) return col;

    // === SILENCE GATE: Hide wheel on sustained silence ===
    // Fade out when energy is low, just like the video
    float silenceGate = smoothstep(0.02, 0.12, u_energy);

    // Position: CENTER of screen, DOMINANT size (can hide on demand)
    vec2 hudCenter = vec2(0.5, 0.5);
    float baseHudRadius = 0.38;  // DOMINANT - fills most of screen

    // === TORUS SHRINK EFFECT ===
    // Shrink QS wheel to 50% when torus knot is active so it fits inside
    float torusScale = 1.0;
    if (u_torus_active == 1) {
        // Ease from 1.0 to 0.5 as torus transitions in
        // Use smooth cubic ease for natural feel
        float shrinkT = smoothstep(0.0, 0.8, u_torus_t);
        torusScale = mix(1.0, 0.5, shrinkT);
    }

    // === DODECAHEDRAL SHRINK EFFECT ===
    // Shrink QS wheel to 25% when dodecahedral is active - the mini wheels ARE the visualization
    float dodecaScale = 1.0;
    if (u_dodeca_active == 1) {
        float shrinkT = smoothstep(0.0, 0.6, u_dodeca_t);
        dodecaScale = mix(1.0, 0.25, shrinkT);
    }

    // === PORTAL 3D MODE SHRINK EFFECT ===
    // Shrink QS wheel to 50% when in portal dive - becomes floating sphere in 3D space
    float portalScale = 1.0;
    if (u_cam_in_portal == 1) {
        portalScale = 0.5;
    }

    // === SUBWOOFER PUMP EFFECT ===
    // When torus is active, pump the wheel to the beat like a subwoofer
    float pumpScale = 1.0;
    if (u_torus_active == 1 && u_torus_t > 0.3) {
        // Bass-driven pump with smooth envelope
        float bassPump = u_bass * 0.08;  // Bass gives sustained push
        float kickPump = u_kick * 0.12;  // Kick gives sharp hit

        // Low-frequency oscillation synced to energy for breathing feel
        float breathe = sin(u_time * 1.5) * 0.02 * u_energy;

        pumpScale = 1.0 + bassPump + kickPump + breathe;

        // Smooth the scale to prevent jitter
        pumpScale = mix(1.0, pumpScale, smoothstep(0.3, 0.6, u_torus_t));
    }

    // === WARP JUMP SCALE EFFECT ===
    // Apply shrink/pop/jiggle animation from warp jump system
    float warpScale = u_warp_qs_scale;

    float hudRadius = baseHudRadius * torusScale * dodecaScale * portalScale * pumpScale * warpScale;

    // Aspect ratio
    float aspect = u_resolution.x / u_resolution.y;

    // Skip full wheel rendering during complete silence (GLB renders separately in Rust)
    // BUT allow polarity effect to show even during silence
    if (silenceGate < 0.01 && u_polarity_effect_timer <= 0.0) {
        return col;
    }

    // 24 wedges (icositetragon)
    float wedgeAngle = TAU / 24.0;


    // === CORNER LABELS - rendered for ALL pixels (before wheel check) ===
    // Determine active angle in DEGREES.
    // - If QuaternionCharges is active, use u_qc_w (radians).
    // - Otherwise, map dominant pitch class to chroma seam degrees (pc * 30°).
    float highlightAngle;
    if (u_qc_active > 0 && u_qc_strength > 0.01) {
        highlightAngle = mod(u_qc_w * 180.0 / PI, 360.0);
    } else {
        highlightAngle = mod(u_dom_pc * 30.0, 360.0);
    }

    // === AUDIO-REACTIVE QUATERNION SET (REG Figure 8) ===
    // Q(A) = {A, 360-A, 180-A, 180+A} computed on CPU from:
    //   - Spoke: dominant pitch class (2 spokes per pitch class)
    //   - Ring: overall energy (low = inner, high = outer)
    // The 4 members are passed via u_qs_quartet uniform
    int qc0 = u_qs_quartet.x;  // A (active integer)
    int qc1 = u_qs_quartet.y;  // 360 - A
    int qc2 = u_qs_quartet.z;  // 180 - A
    int qc3 = u_qs_quartet.w;  // 180 + A

    // Keep legacy highlightAngle for backward compatibility with other effects
    highlightAngle = mod(highlightAngle + 180.0, 360.0);
    if (highlightAngle < 0.5) highlightAngle = 360.0;

    // Corner labels can be visually loud; only show them when Quaternion Charges (QC) mode is active.
    if (u_qc_active > 0 && u_qc_strength > 0.01) {
    // Corner label positions and values
    // NOTE: Avoid top-left (0.12, 0.85) where chroma wheel is!
    vec2 cornerPositions[4];
    cornerPositions[0] = vec2(0.20, 0.25);   // Bottom-left (closer to wheel)
    cornerPositions[1] = vec2(0.80, 0.25);   // Bottom-right (closer to wheel)
    cornerPositions[2] = vec2(0.20, 0.75);   // Top-left (closer to wheel)
    cornerPositions[3] = vec2(0.80, 0.75);   // Top-right (closer to wheel)

    // Corner colors - define BEFORE using
    vec3 cornerColors[4];
    cornerColors[0] = vec3(1.0, 0.9, 0.2);    // Gold
    cornerColors[1] = vec3(1.0, 0.4, 0.8);    // Magenta
    cornerColors[2] = vec3(0.4, 0.85, 1.0);   // Cyan
    cornerColors[3] = vec3(0.4, 1.0, 0.6);    // Green

    // Draw colored ring circles at corner positions (half size, aspect-corrected for circles)
    for (int dbg = 0; dbg < 4; dbg++) {
        vec2 toCorner = uv_screen - cornerPositions[dbg];
        toCorner.x *= aspect;  // Aspect correction for circular shape
        float dbgDist = length(toCorner);
        float dbgCircle = smoothstep(0.035, 0.01, dbgDist);  // Half size
        col = mix(col, cornerColors[dbg] * 3.0, dbgCircle);
    }

    int cornerValues[4];
    cornerValues[0] = qc0;  // 43
    cornerValues[1] = qc1;  // 317
    cornerValues[2] = qc3;  // 137
    cornerValues[3] = qc2;  // 223

    // Render each corner label with quaternion symbols
    // ALWAYS render (removed u_qs_hide_numbers check for symbols)
    {
        // Quaternion symbols for each corner: +1, -1, +i, -i
        // Corner 0 (Gold) = +1, Corner 1 (Magenta) = -1, Corner 2 (Cyan) = +i, Corner 3 (Green) = -i
        int cornerCharges[4];
        cornerCharges[0] = 0;  // +1
        cornerCharges[1] = 2;  // -1
        cornerCharges[2] = 1;  // +i
        cornerCharges[3] = 3;  // -i

        for (int i = 0; i < 4; i++) {
            vec2 labelCenter = cornerPositions[i];
            vec2 toLabel = uv_screen - labelCenter;
            vec2 toLabelCircular = vec2(toLabel.x * aspect, toLabel.y);  // Aspect-correct for circles

            float distToLabel = length(toLabelCircular);
            float labelArea = 0.06;  // Half size, aspect-corrected

            if (distToLabel < labelArea) {
                float labelSize = 0.0175;  // Quarter size degree number
                float pulse = 0.9 + 0.1 * sin(u_time * 3.0 + float(i));

                // Dark background pill - more opaque
                float bgMask = smoothstep(labelArea, labelArea * 0.25, distToLabel);
                col = mix(col, vec3(0.01, 0.01, 0.02), bgMask * 0.95);

                // Glowing border
                float borderMask = smoothstep(labelArea, labelArea * 0.85, distToLabel) *
                                   smoothstep(labelArea * 0.7, labelArea * 0.85, distToLabel);
                col = mix(col, cornerColors[i] * 1.2, borderMask * 0.6);

                // === QUATERNION SYMBOL (top row) - quarter size ===
                vec2 symbolPos = labelCenter + vec2(0.0, 0.011);
                vec2 toSymbol = uv_screen - symbolPos;
                float symbolSize = 0.011;  // Quarter size symbol
                float lineThick = 0.0018;  // Quarter thickness lines

                // Plus/minus sign rendering
                float plusH = smoothstep(lineThick, lineThick * 0.3, abs(toSymbol.y)) * smoothstep(symbolSize, symbolSize * 0.25, abs(toSymbol.x));
                float plusV = smoothstep(lineThick, lineThick * 0.3, abs(toSymbol.x)) * smoothstep(symbolSize * 0.6, symbolSize * 0.1, abs(toSymbol.y));
                float minusH = smoothstep(lineThick, lineThick * 0.3, abs(toSymbol.y)) * smoothstep(symbolSize, symbolSize * 0.25, abs(toSymbol.x));

                float symbolMask = 0.0;
                int charge = cornerCharges[i];

                if (charge == 0) {
                    // +1: plus sign only
                    symbolMask = max(plusH, plusV);
                } else if (charge == 1) {
                    // +i: plus sign + "i"
                    vec2 pOff = toSymbol + vec2(0.008, 0.0);
                    float pH = smoothstep(lineThick * 0.8, lineThick * 0.2, abs(pOff.y)) * smoothstep(symbolSize * 0.5, symbolSize * 0.12, abs(pOff.x));
                    float pV = smoothstep(lineThick * 0.8, lineThick * 0.2, abs(pOff.x)) * smoothstep(symbolSize * 0.4, symbolSize * 0.08, abs(pOff.y));
                    vec2 iOff = toSymbol - vec2(0.006, 0.0);
                    float iDot = smoothstep(0.003, 0.0015, length(iOff - vec2(0.0, 0.006)));
                    float iBar = smoothstep(lineThick * 0.8, lineThick * 0.2, abs(iOff.x)) * smoothstep(0.0055, 0.001, abs(iOff.y + 0.00125));
                    symbolMask = max(max(pH, pV), max(iDot, iBar));
                } else if (charge == 2) {
                    // -1: minus sign only
                    symbolMask = minusH;
                } else {
                    // -i: minus sign + "i"
                    vec2 mOff = toSymbol + vec2(0.008, 0.0);
                    float mH = smoothstep(lineThick * 0.8, lineThick * 0.2, abs(mOff.y)) * smoothstep(symbolSize * 0.5, symbolSize * 0.12, abs(mOff.x));
                    vec2 iOff = toSymbol - vec2(0.006, 0.0);
                    float iDot = smoothstep(0.003, 0.0015, length(iOff - vec2(0.0, 0.006)));
                    float iBar = smoothstep(lineThick * 0.8, lineThick * 0.2, abs(iOff.x)) * smoothstep(0.0055, 0.001, abs(iOff.y + 0.00125));
                    symbolMask = max(mH, max(iDot, iBar));
                }

                // Render symbol - BRIGHT
                col = mix(col, cornerColors[i] * 2.5, symbolMask);

                // === DEGREE NUMBER (bottom row) ===
                if (u_qs_hide_numbers == 0) {
                    vec2 numPos = labelCenter - vec2(0.0, 0.009);
                    vec2 toNum = uv_screen - numPos;
                    float digitMask = renderNumberFromAtlas(toNum, cornerValues[i], labelSize);

                    if (digitMask > 0.01) {
                        col = mix(col, cornerColors[i] * pulse * 2.0, digitMask);
                    }
                }

                // Outer glow
                float glow = exp(-distToLabel / labelArea * 2.0) * 0.5;
                col += cornerColors[i] * glow * pulse * 0.4;
            }
        }
    }

    // Connection lines from corners to wheel
    for (int i = 0; i < 4; i++) {
        int compDeg = cornerValues[i];

        // Find cell position
        int compWedge = compDeg % 24;
        if (compWedge == 0 && compDeg > 0) compWedge = 24;
        int compRing = (compDeg - compWedge) / 24;
        if (compWedge == 24) { compWedge = 0; compRing = (compDeg - 24) / 24; }
        if (compDeg == 360) { compWedge = 0; compRing = 14; }

        float ringStart = 0.22, ringEnd = 0.94;
        float ringRadius = ringStart + (float(compRing) + 0.5) * (ringEnd - ringStart) / 15.0;
        float wedgeMidAngle = (float(compWedge) + 0.5) * wedgeAngle;

        vec2 cellPos = hudCenter + vec2(sin(wedgeMidAngle) / aspect, cos(wedgeMidAngle)) * hudRadius * ringRadius;
        vec2 labelPos = cornerPositions[i];

        vec2 lineDir = cellPos - labelPos;
        float lineLen = length(lineDir);
        if (lineLen > 0.01) {
            lineDir /= lineLen;
            vec2 toPixel = uv_screen - labelPos;
            float proj = clamp(dot(toPixel, lineDir), 0.08, lineLen - 0.05);
            vec2 closest = labelPos + lineDir * proj;
            float distLine = length(uv_screen - closest);

            if (distLine < 0.004 && proj > 0.08 && proj < lineLen - 0.05) {
                col = mix(col, cornerColors[i], 0.7);
            }
        }
    } // end connection lines for loop
    } // end QC-only corner labels/lines (if u_qc_active)

    // === TORUS KNOT INFO HUD ===
    // Display p:q ratio and stability when torus is active
    if (u_torus_active == 1 && u_torus_t > 0.1) {
        // Position: top-right area (avoiding other HUD elements)
        vec2 torusHudPos = vec2(0.88, 0.92);
        vec2 toHud = uv_screen - torusHudPos;

        float hudDist = length(toHud);
        float hudArea = 0.12;

        if (hudDist < hudArea) {
            // Dark background pill
            float bgMask = smoothstep(hudArea, hudArea * 0.4, hudDist);
            col = mix(col, vec3(0.0, 0.02, 0.04), bgMask * 0.85);

            // Fade in with torus transition
            float torusFade = smoothstep(0.1, 0.5, u_torus_t);

            // === P VALUE (top) ===
            vec2 pPos = torusHudPos + vec2(-0.03, 0.03);
            vec2 toP = uv_screen - pPos;
            float pDist = length(toP);
            if (pDist < 0.05) {
                int pVal = int(u_torus_p + 0.5);  // Round float to int
                float pMask = renderNumberFromAtlas(toP, pVal, 0.035);
                vec3 pColor = vec3(1.0, 0.85, 0.4);  // Golden
                col = mix(col, pColor * 1.5, pMask * torusFade);
            }

            // === COLON separator ===
            vec2 colonPos = torusHudPos + vec2(0.0, 0.03);
            float colonDist = length(uv_screen - colonPos);
            float dot1 = smoothstep(0.006, 0.003, length(uv_screen - (colonPos + vec2(0.0, 0.008))));
            float dot2 = smoothstep(0.006, 0.003, length(uv_screen - (colonPos - vec2(0.0, 0.008))));
            col = mix(col, vec3(0.8), (dot1 + dot2) * torusFade);

            // === Q VALUE (after colon) ===
            vec2 qPos = torusHudPos + vec2(0.03, 0.03);
            vec2 toQ = uv_screen - qPos;
            float qDist = length(toQ);
            if (qDist < 0.05) {
                int qVal = int(u_torus_q + 0.5);  // Round float to int
                float qMask = renderNumberFromAtlas(toQ, qVal, 0.035);
                vec3 qColor = vec3(0.4, 0.9, 1.0);  // Cyan
                col = mix(col, qColor * 1.5, qMask * torusFade);
            }

            // === STABILITY BAR (below ratio) ===
            vec2 barPos = torusHudPos + vec2(0.0, -0.01);
            vec2 toBar = uv_screen - barPos;
            float barWidth = 0.06;
            float barHeight = 0.008;

            if (abs(toBar.x) < barWidth && abs(toBar.y) < barHeight) {
                // Background bar
                col = mix(col, vec3(0.1), 0.8 * torusFade);

                // Filled portion based on stability
                float fillX = (toBar.x + barWidth) / (barWidth * 2.0);
                if (fillX < u_torus_stability) {
                    // Color gradient: red -> yellow -> green based on stability
                    vec3 barColor = mix(vec3(1.0, 0.3, 0.2), vec3(0.3, 1.0, 0.5), u_torus_stability);
                    col = mix(col, barColor, 0.9 * torusFade);
                }
            }

            // === GOLDEN BLEND INDICATOR (small dot) ===
            if (u_torus_golden_blend > 0.1) {
                vec2 goldenPos = torusHudPos + vec2(0.05, -0.01);
                float goldenDot = smoothstep(0.008, 0.004, length(uv_screen - goldenPos));
                vec3 goldenColor = vec3(1.0, 0.85, 0.3) * (0.5 + u_torus_golden_blend * 0.5);
                col = mix(col, goldenColor, goldenDot * torusFade * u_torus_golden_blend);
            }

            // === STATE INDICATOR (dots showing subphase) ===
            // 0=CurlToTorus, 1=ThreadKnot, 2=Stabilize, 3=Active, 4=Idle
            vec2 statePos = torusHudPos + vec2(-0.04, -0.035);
            for (int s = 0; s < 4; s++) {
                vec2 dotPos = statePos + vec2(float(s) * 0.022, 0.0);
                float dotDist = length(uv_screen - dotPos);

                // Determine if this state is active or past
                bool isActive = (s == u_torus_subphase);
                bool isPast = (s < u_torus_subphase);

                // State colors
                vec3 stateColors[4];
                stateColors[0] = vec3(0.8, 0.4, 0.9);  // Curl - purple
                stateColors[1] = vec3(0.4, 0.8, 1.0);  // Thread - cyan
                stateColors[2] = vec3(1.0, 0.8, 0.3);  // Stabilize - gold
                stateColors[3] = vec3(0.3, 1.0, 0.5);  // Active - green

                float dotSize = isActive ? 0.008 : 0.005;
                float dotMask = smoothstep(dotSize + 0.002, dotSize, dotDist);

                vec3 dotColor = stateColors[s];
                float brightness = isActive ? 1.5 : (isPast ? 0.6 : 0.2);

                // Pulse active state
                if (isActive) {
                    brightness *= 1.0 + 0.3 * sin(u_time * 4.0);
                }

                col = mix(col, dotColor * brightness, dotMask * torusFade);
            }

            // === STATE LABEL (tiny text hint below dots) ===
            // Show abbreviated state name
            vec2 labelPos = torusHudPos + vec2(0.0, -0.055);
            float labelDist = length(uv_screen - labelPos);
            if (labelDist < 0.03) {
                // Simple state indicator using shapes
                // C=Curl(arc), T=Thread(line), S=Stabilize(square), A=Active(dot)
                vec2 toLbl = uv_screen - labelPos;

                if (u_torus_subphase == 0) {
                    // Curl: small arc
                    float arcAngle = atan(toLbl.y, toLbl.x);
                    float arcR = length(toLbl);
                    float arc = smoothstep(0.012, 0.008, abs(arcR - 0.012)) *
                                step(-0.5, cos(arcAngle));
                    col = mix(col, vec3(0.8, 0.4, 0.9), arc * torusFade);
                } else if (u_torus_subphase == 1) {
                    // Thread: wavy line
                    float wave = sin(toLbl.x * 80.0) * 0.004;
                    float line = smoothstep(0.004, 0.002, abs(toLbl.y - wave)) *
                                 step(abs(toLbl.x), 0.015);
                    col = mix(col, vec3(0.4, 0.8, 1.0), line * torusFade);
                } else if (u_torus_subphase == 2) {
                    // Stabilize: pulsing ring
                    float ring = smoothstep(0.012, 0.008, abs(length(toLbl) - 0.008));
                    col = mix(col, vec3(1.0, 0.8, 0.3), ring * torusFade);
                } else if (u_torus_subphase == 3) {
                    // Active: solid filled circle
                    float circle = smoothstep(0.012, 0.008, length(toLbl));
                    col = mix(col, vec3(0.3, 1.0, 0.5), circle * torusFade);
                }
            }

            // Subtle glow around HUD
            float glow = exp(-hudDist / hudArea * 2.0) * 0.15 * torusFade;
            col += vec3(0.3, 0.5, 0.7) * glow;
        }
    }

    // === WHEEL RENDERING ===
    vec2 hudUV = uv_screen - hudCenter;
    hudUV.x *= aspect;

    // === PORTAL SPHERICAL DISTORTION ===
    // When in portal 3D mode, apply fisheye projection to make wheel appear spherical
    float sphereAmount = 0.0;
    if (u_cam_in_portal == 1) {
        sphereAmount = 0.8;  // Strong spherical distortion
        // Fisheye-style spherical projection
        float r = length(hudUV);
        float maxR = hudRadius * 1.2;
        if (r > 0.001 && r < maxR) {
            // Spherical mapping: compress outer regions, expand center
            float normR = r / maxR;
            // Inverse fisheye: makes edges curve inward like a sphere surface
            float sphereR = asin(normR * sphereAmount) / (sphereAmount * PI * 0.5);
            sphereR = mix(normR, sphereR, sphereAmount);
            hudUV = hudUV * (sphereR / normR);
        }
    }

    float dist = length(hudUV);

    // Outside wheel area - early return
    if (dist > hudRadius * 1.15) return col;

    // Polar coordinates
    // GLSL provides atan(y, x) (atan2). We want 0° at NORTH (top) and
    // angles increasing CLOCKWISE to match the paper's layout.
    float raw = atan(hudUV.y, hudUV.x);            // 0 at +X (EAST), CCW positive
    float angle = mod(PI * 0.5 - raw + TAU, TAU);  // 0 at NORTH, CW positive
    // No rotation - wedge 0 (with value 24) at NORTH matches REG Figure 8
    float angleDeg = angle * 180.0 / PI;           // 0..360

    float normDist = dist / hudRadius;

    // 24 wedges, 15° each (wedgeAngle already declared above)
    int wedgeIndex = int(floor(angle / wedgeAngle));
    float wedgeFrac = fract(angle / wedgeAngle);

    // MODERN DARK BACKGROUND with subtle blue tint
    vec3 bgColor = vec3(0.01, 0.015, 0.025) * QS_BACKGROUND_DARKEN;
    float bgAlpha = 0.95 * silenceGate;  // Fade with silence gate

    // NOTE: Camera is blended into wedge cells below (on skinColor around line 4870)

    // NEON OUTER GLOW - premium multi-layer bloom
    if (normDist > 1.0) {
        float falloff = normDist - 1.0;

        // Layer 1: Tight bright glow (neon edge)
        float glow1 = exp(-falloff * 20.0) * 0.7;
        // Layer 2: Mid glow (color)
        float glow2 = exp(-falloff * 8.0) * 0.4;
        // Layer 3: Wide soft glow (ambient)
        float glow3 = exp(-falloff * 3.0) * 0.2;

        // Animated color cycling for premium feel
        float hueShift = u_time * 0.4;
        vec3 glowColor1 = vec3(0.4, 0.7, 1.0);  // Bright cyan
        vec3 glowColor2 = vec3(
            0.5 + 0.3 * sin(hueShift),
            0.3 + 0.2 * sin(hueShift + 2.0),
            0.9 + 0.1 * sin(hueShift + 4.0)
        );  // Shifting purple-blue
        vec3 glowColor3 = vec3(0.15, 0.25, 0.4);  // Deep blue ambient

        vec3 totalGlow = glowColor1 * glow1 + glowColor2 * glow2 + glowColor3 * glow3;
        totalGlow *= QS_RIM_GLOW * (1.0 + u_energy * 0.8);  // Audio reactive

        // Add subtle sparkle on outer edge
        float sparkle = sin(angle * 24.0 + u_time * 3.0) * 0.5 + 0.5;
        sparkle = pow(sparkle, 8.0) * glow1 * 0.3;
        totalGlow += vec3(0.8, 0.9, 1.0) * sparkle;

        return mix(col, col + totalGlow, bgAlpha * 0.8);
    }

    // Concentric rings layout - 15 rings for 360 degrees (24 integers per ring)
    float numRings = 15.0;
    float innerCore = 0.22;
    float ringDist = (normDist - innerCore) / (1.0 - innerCore);
    int ringIndex = int(floor(ringDist * numRings));
    float ringFrac = fract(ringDist * numRings);

    // === CENTER CORE (CENTERED & CROPPED VIDEO with neon rim) ===
    if (normDist < innerCore) {
        float coreNorm = normDist / innerCore;

        // === CENTERED & CROPPED VIDEO IN CENTER (SEMI-TRANSPARENT) ===
        // Video should FILL the center circle, not the whole wheel

        // Normalize hudUV to center circle bounds (innerCore * hudRadius)
        float coreRadius = innerCore * hudRadius;
        vec2 normalizedCoreUV = hudUV / coreRadius;  // -1 to 1 within center circle

        // Use unified video UV mapping function
        float coreZoom = 0.25;  // Show 25% of video - much more zoomed in

        // Portrait video handling: if aspect < 1.0, zoom in even more
        if (u_video_aspect < 1.0) {
            coreZoom *= 0.7;  // Even bigger for portrait
        }

        // normalizedCoreUV still has aspect multiplication from hudUV, so pass aspect
        vec2 videoUV = mapCircularToVideoUV(normalizedCoreUV, aspect, coreZoom);

        vec3 coreSkinColor = texture(u_skin_tex, videoUV).rgb;

        // Boost video brightness to match outer wheel
        coreSkinColor *= 2.0;
        float coreLuma = dot(coreSkinColor, vec3(0.299, 0.587, 0.114));
        coreSkinColor = mix(vec3(coreLuma), coreSkinColor, 1.2);
        coreSkinColor = clamp(coreSkinColor, 0.0, 1.5);

        // === CAMERA TEXTURE OVERLAY (center circle) ===
        if (u_camera_has_texture == 1 && u_camera_alpha > 0.001) {
            vec2 camUV = normalizedCoreUV;  // Already -1..1 within center

            // Apply camera aspect ratio correction (cover fit)
            if (u_camera_aspect > 1.0) {
                camUV.x /= u_camera_aspect;
            } else {
                camUV.y *= u_camera_aspect;
            }

            camUV = camUV * 0.5 + 0.5;    // Convert to 0..1 UV space
            camUV.y = 1.0 - camUV.y;      // Flip Y for camera
            camUV = clamp(camUV, 0.0, 1.0);

            vec3 camColor = texture(u_camera_tex, camUV).rgb;
            coreSkinColor = mix(coreSkinColor, camColor, u_camera_alpha);
        }

        // === BASE COLOR: Subtle dark tint for pulsing to show ===
        vec3 baseCore = vec3(0.02, 0.03, 0.06);

        // Video dominates - acts as texture surface
        float videoOpacity = 0.85;
        vec3 coreColor = mix(baseCore, coreSkinColor, videoOpacity);

        // Hard circular mask with clean edge
        float maskEdge = smoothstep(QS_CENTER_MASK_HARDNESS, QS_CENTER_MASK_HARDNESS - 0.04, coreNorm);
        coreColor *= maskEdge;

        // Subtle vignette (darkens edge, keeps center bright)
        float vignette = smoothstep(0.5, 1.0, coreNorm) * QS_CENTER_VIGNETTE;
        coreColor *= (1.0 - vignette);

        // NEON RIM STROKE at boundary
        float rimDist = abs(coreNorm - (1.0 - QS_CENTER_RIM_WIDTH * 1.5));
        float rimMask = smoothstep(QS_CENTER_RIM_WIDTH, QS_CENTER_RIM_WIDTH * 0.2, rimDist);
        // Animated rim color
        float hueShift = u_time * 0.3;
        vec3 rimColor = vec3(
            0.4 + 0.2 * sin(hueShift),
            0.6 + 0.2 * sin(hueShift + 2.1),
            0.9 + 0.1 * sin(hueShift + 4.2)
        );
        coreColor = mix(coreColor, rimColor * 1.5, rimMask * QS_CENTER_RIM_ALPHA);

        // Outer glow around rim
        float rimGlowDist = abs(coreNorm - 0.95);
        float rimGlow = exp(-rimGlowDist * 30.0) * 0.4;
        coreColor += rimColor * rimGlow;

        // === INNER GLOW (Museum-Mode Polish) ===
        // Idle: calm, steady "lens" look with slow 0.5Hz pulse
        // Live: beat-quantized pulse on kick (NOT continuous breathing)
        float innerGlow = (1.0 - coreNorm) * QS_CORE_GLOW;

        // Mode-dependent pulsing
        float pulse;
        if (u_idle_mix > 0.5) {
            // IDLE: Very slow, gentle breathing (calm lens)
            pulse = 0.9 + 0.1 * sin(u_time * 0.5 * TAU);  // 0.5Hz
        } else {
            // LIVE: Beat-quantized kick pulse (no continuous breathing)
            float kickPulse = 1.0;
            if (u_kick > 0.3) {
                // Sharp attack on kick only
                kickPulse = 1.0 + u_kick * 0.6;
            }
            pulse = kickPulse;
        }
        innerGlow *= pulse;

        // Lens-like specular highlight in Idle mode
        if (u_idle_mix > 0.5) {
            vec2 lensLightDir = normalize(vec2(-0.5, 0.7));
            vec2 coreUVNorm = normalizedCoreUV;
            float lensSpec = pow(max(0.0, dot(normalize(coreUVNorm), lensLightDir)), 25.0);
            innerGlow += lensSpec * 0.15 * u_idle_mix;
        }

        coreColor += vec3(0.02, 0.03, 0.05) * innerGlow;  // Reduced glow contribution

        // Transparent center to show video
        float centerFade = smoothstep(0.3, 0.0, coreNorm);
        coreColor = mix(coreColor, vec3(0.01, 0.012, 0.02), centerFade * 0.3);

        // === POLARITY INDICATOR - STRONG PULSING COLORS ===
        // Get cardinal energies
        float nE = getChroma(11), eE = getChroma(2), sE = getChroma(5), wE = getChroma(8);
        float maxCard = max(max(nE, eE), max(sE, wE));

        if (coreNorm < 0.85 && maxCard > 0.08) {
            // Determine dominant polarity color - BRIGHT NEON
            vec3 polarityCol = vec3(0.5);
            float polarityPulse = 0.6 + 0.4 * sin(u_time * 5.0);

            if (nE >= maxCard - 0.01) {
                polarityCol = vec3(0.3, 0.9, 1.0);  // N: √-1 cyan
            } else if (sE >= maxCard - 0.01) {
                polarityCol = vec3(1.0, 0.4, 0.9);  // S: -√-1 magenta
            } else if (eE >= maxCard - 0.01) {
                polarityCol = vec3(1.0, 0.95, 0.3);  // E: +1 gold
            } else if (wE >= maxCard - 0.01) {
                polarityCol = vec3(0.3, 1.0, 0.6);  // W: -1 green
            }

            // Subtle radial glow for polarity indication (reduced to show video)
            float centerGlow = smoothstep(0.85, 0.1, coreNorm) * maxCard * polarityPulse;
            coreColor = mix(coreColor, polarityCol * 0.8, centerGlow * 0.3);

            // Small pulsing polarity dot (reduced size and intensity)
            if (coreNorm < 0.15) {
                float dotGlow = smoothstep(0.15, 0.0, coreNorm);
                float dotPulse = 0.7 + 0.3 * sin(u_time * 6.0 + 1.0);
                coreColor = mix(coreColor, polarityCol * 1.2, dotGlow * maxCard * dotPulse * 0.5);
            }

            // Add outer ring pulse
            float ringPulse = smoothstep(0.7, 0.5, coreNorm) * smoothstep(0.3, 0.5, coreNorm);
            coreColor += polarityCol * ringPulse * maxCard * polarityPulse * 0.4;
        }

        // Zero point: all 4 balanced = subtle white indicator (reduced to show video)
        if (nE > 0.12 && eE > 0.12 && sE > 0.12 && wE > 0.12 && coreNorm < 0.2) {
            float zeroGlow = smoothstep(0.2, 0.0, coreNorm);
            float zeroPulse = 0.7 + 0.3 * sin(u_time * 7.0);
            float zeroIntensity = min(min(nE, eE), min(sE, wE));
            coreColor = mix(coreColor, vec3(1.0, 0.95, 0.9), zeroGlow * zeroPulse * zeroIntensity * 0.4);
        }

        return mix(col, coreColor, bgAlpha);
    }

    // === WEDGE RENDERING (REG-SAFE VISUAL: Proper layer order) ===
    // Layer order: 1. Video texture → 2. HUD scrim → 3. Grid → 4. Seams → 5. Text

    // Get chroma energy for this wedge (2 wedges per pitch class)
    // REG MATH PRESERVED: wedge→chroma mapping unchanged
    int wedgeChroma = (wedgeIndex / 2) % 12;
    float chromaEnergy = getChroma(wedgeChroma);

    // === LAYER 1: CELL VIDEO TEXTURE (CENTERED ON WHEEL) ===
    // Use unified video UV mapping function for consistent display
    float videoZoom = 0.6;  // Show 60% of video (20% more visible than before)
    vec2 skinUV = mapCircularToVideoUV(hudUV, aspect, videoZoom);

    // Subtle frequency warp on active cells
    if (chromaEnergy > 0.2) {
        float warpStrength = chromaEnergy * 0.015;
        float ripple = sin(normDist * 12.0 - u_time * 4.0) * warpStrength;
        vec2 warpDir = normalize(hudUV + 0.001);  // Direction from center
        skinUV += warpDir * ripple;
    }

    // Clamp to valid UV range
    skinUV = clamp(skinUV, 0.0, 1.0);

    // Sample video texture
    vec3 skinColor = texture(u_skin_tex, skinUV).rgb;

    // === BOOST VIDEO: Brightness, Contrast, Saturation ===
    // Brightness boost
    skinColor *= QS_VIDEO_BRIGHTNESS;

    // Contrast boost (pull away from mid-gray)
    skinColor = (skinColor - 0.5) * QS_VIDEO_CONTRAST + 0.5;

    // Saturation boost
    float luma = dot(skinColor, vec3(0.299, 0.587, 0.114));
    skinColor = mix(vec3(luma), skinColor, QS_VIDEO_SATURATION);

    // Clamp to prevent blowout but allow HDR-ish highlights
    skinColor = clamp(skinColor, 0.0, 1.5);

    // === CAMERA TEXTURE OVERLAY (on top of video in wedge cells) ===
    // Camera is composited on top of compel video in the QS Wheel outer ring
    if (u_camera_has_texture == 1 && u_camera_alpha > 0.001) {
        // Map wheel UV to camera texture UV with proper aspect ratio
        vec2 cameraUV = hudUV / hudRadius;  // Normalize to -1..1 within wheel

        // Apply camera aspect ratio correction (cover fit - fill circle, crop excess)
        // Camera aspect = width/height, so >1 means landscape, <1 means portrait
        if (u_camera_aspect > 1.0) {
            // Landscape camera: scale X to fit width, crop top/bottom
            cameraUV.x /= u_camera_aspect;
        } else {
            // Portrait camera: scale Y to fit height, crop sides
            cameraUV.y *= u_camera_aspect;
        }

        cameraUV = cameraUV * 0.5 + 0.5;    // Convert to 0..1 UV space
        cameraUV.y = 1.0 - cameraUV.y;      // Flip Y for camera
        cameraUV = clamp(cameraUV, 0.0, 1.0);

        vec3 cameraColor = texture(u_camera_tex, cameraUV).rgb;

        // Blend camera on top of video based on alpha
        // At alpha=0: pure video, at alpha=1: pure camera
        skinColor = mix(skinColor, cameraColor, u_camera_alpha);
    }

    // Get base wedge color for VERY subtle tinting
    vec3 wedgeTint = getQuaternionWedgeColor(wedgeIndex);

    // REG-SAFE: Video DOMINATES, wedge tint is barely visible
    vec3 wedgeColor = mix(skinColor, skinColor * wedgeTint * 1.5, QS_WEDGE_TINT_AMOUNT);

    // === NO SCRIM - Let video shine through completely ===
    // Scrim removed to maximize video visibility

    // Cardinal axis wedges (N/E/S/W) get major line weight
    // REG MATH PRESERVED: cardinal detection unchanged
    bool isCardinal = (wedgeIndex == 0 || wedgeIndex == 6 || wedgeIndex == 12 || wedgeIndex == 18);

    // === FWIDTH-BASED ANTI-ALIASED GRID LINES ===
    // Convert pixel sizes to normalized coordinates
    float pixelScale = 1.0 / min(u_resolution.x, u_resolution.y);
    float majorThick = QS_MAJOR_LINE_PX * pixelScale * (1.0 / hudRadius);
    float minorThick = QS_MINOR_LINE_PX * pixelScale * (1.0 / hudRadius);

    // === RADIAL CONTRAST TAPER (Museum-Mode Polish) ===
    // Stronger contrast near outer rim, softer toward center
    // This prevents the center from looking "dusty"
    float radialTaper = smoothstep(0.15, 0.85, normDist);
    float taperMult = 0.5 + 0.5 * radialTaper;  // 0.5 at center, 1.0 at rim

    // === FOLD-AWARE SPOKE VISIBILITY (Museum-Mode Polish) ===
    // Major spokes (12-fold): always visible, thin but crisp
    // Minor spokes (24-fold): only visible when fold24 active, otherwise fade
    bool isMajorSpoke = (wedgeIndex % 2 == 0);  // Even indices = 12-fold subset
    float spokeVisibility;
    if (isMajorSpoke || isCardinal) {
        spokeVisibility = 1.0;  // 12-fold + cardinals always visible
    } else {
        // Minor (24-fold) spokes fade in with fold24 transition
        spokeVisibility = smoothstep(0.3, 0.7, u_fold24_t);
    }

    // Wedge edge detection with fwidth AA
    float wedgeEdgeDist = min(wedgeFrac, 1.0 - wedgeFrac);
    float wedgeAA = fwidth(wedgeEdgeDist) * QS_AA_SCALE;
    float wedgeLineThick = isCardinal ? majorThick * 1.5 : (isMajorSpoke ? minorThick : minorThick * 0.6);
    float wedgeLine = smoothstep(wedgeLineThick - wedgeAA, wedgeLineThick + wedgeAA, wedgeEdgeDist);
    // Apply spoke visibility to minor spokes
    wedgeLine = mix(1.0, wedgeLine, spokeVisibility);

    // Ring edge detection with fwidth AA + radial taper
    float ringEdgeDist = min(ringFrac, 1.0 - ringFrac);
    float ringAA = fwidth(ringEdgeDist) * QS_AA_SCALE;
    // Every 5th ring is a major ring (0, 5, 10 = inner, mid, outer thirds)
    bool isMajorRing = (ringIndex % 5 == 0) || (ringIndex == 14);
    // Apply radial taper: thinner lines toward center
    float ringLineThick = isMajorRing
        ? majorThick * 1.2 * taperMult
        : minorThick * 0.8 * taperMult;
    float ringLine = smoothstep(ringLineThick - ringAA, ringLineThick + ringAA, ringEdgeDist);

    // === QUATERNION COMPLEMENT BANDS (REG Figure 2/3 style) ===
    // When QC is active, draw two crossing bands connecting complement pairs:
    // Band 1 (purple): A ↔ 360°-A (mirror across N-S axis)
    // Band 2 (blue): 180°-A ↔ 180°+A (mirror across E-W axis)
    float band1Highlight = 0.0;
    float band2Highlight = 0.0;

    if (u_qc_active > 0 && u_qc_strength > 0.01) {
        // Current harmonic angle A in degrees (from w, which is in radians)
        float A = u_qc_w * 180.0 / PI;

        // Complement positions (in degrees, 0-360)
        float c0 = A;                              // A
        float c1 = mod(360.0 - A, 360.0);          // 360° - A
        float c2 = mod(180.0 - A + 360.0, 360.0);  // 180° - A
        float c3 = mod(180.0 + A, 360.0);          // 180° + A

        // Band width in degrees (how wide the colored band is)
        float bandWidth = 12.0;

        // Check if current angle is within Band 1 (A ↔ 360-A)
        // This band spans the arc from c0 to c1 (shortest path)
        float d0 = min(abs(angleDeg - c0), 360.0 - abs(angleDeg - c0));
        float d1 = min(abs(angleDeg - c1), 360.0 - abs(angleDeg - c1));
        band1Highlight = max(
            smoothstep(bandWidth, bandWidth * 0.3, d0),
            smoothstep(bandWidth, bandWidth * 0.3, d1)
        );

        // Check if current angle is within Band 2 (180-A ↔ 180+A)
        float d2 = min(abs(angleDeg - c2), 360.0 - abs(angleDeg - c2));
        float d3 = min(abs(angleDeg - c3), 360.0 - abs(angleDeg - c3));
        band2Highlight = max(
            smoothstep(bandWidth, bandWidth * 0.3, d2),
            smoothstep(bandWidth, bandWidth * 0.3, d3)
        );

        // Scale by QC strength
        band1Highlight *= u_qc_strength;
        band2Highlight *= u_qc_strength;
    }

    // Apply Band 1 (purple/magenta - connects A to 360-A)
    if (band1Highlight > 0.01) {
        float pulse = 0.85 + 0.15 * sin(u_time * 3.0);
        vec3 band1Color = vec3(0.7, 0.4, 0.75);  // Purple/magenta
        wedgeColor = mix(wedgeColor, band1Color * pulse, band1Highlight * 0.55);
    }

    // Apply Band 2 (blue - connects 180-A to 180+A)
    if (band2Highlight > 0.01) {
        float pulse = 0.85 + 0.15 * sin(u_time * 3.0 + 1.5);
        vec3 band2Color = vec3(0.45, 0.6, 0.85);  // Blue
        wedgeColor = mix(wedgeColor, band2Color * pulse, band2Highlight * 0.55);
    }

    // Where bands overlap, blend to white/bright
    if (band1Highlight > 0.3 && band2Highlight > 0.3) {
        float overlap = band1Highlight * band2Highlight;
        wedgeColor = mix(wedgeColor, vec3(0.9, 0.85, 0.95), overlap * 0.4);
    }

    // === AUDIO MODULATION (very subtle - Figure 8 has mostly static colors) ===
    // Map 24 wedges -> 12 pitch-classes (2 wedges per chroma bin)
    int wedgePC = (wedgeIndex / 2) % 12;
    float pcEnergy = clamp(getChroma(wedgePC), 0.0, 1.0);

    // Ring brightness: slight gradient (inner slightly darker, outer slightly brighter)
    // Reduced range to match Figure 8's more uniform appearance
    float ringBrightness = 0.85 + 0.15 * (float(ringIndex) / numRings);

    // Store unmodulated base color for debug mode
    vec3 baseWedgeColor = wedgeColor * ringBrightness;

    // Audio modulation: VERY subtle to keep Figure 8's static look
    // Only slight brightness boost on active frequencies
    float audioGain = 1.0;
    if (u_qs_debug_color == 0) {
        // Very subtle modulation - just a hint of reactivity
        audioGain = 1.0 + 0.08 * pcEnergy + 0.04 * u_energy;
    }
    // Clamp to prevent over-saturation
    wedgeColor = clamp(baseWedgeColor * audioGain, 0.0, 1.0);

    // Cardinal wedges get subtle border emphasis (reduced from original)
    if (isCardinal && wedgeEdgeDist < 0.08) {
        wedgeColor = mix(wedgeColor, vec3(0.5, 0.5, 0.55), 0.15);
    }

    // === LAYER 3 & 4: GRID + SEAM LINES ===
    // Cell interiors = FULL VIDEO brightness
    // Grid lines = thin dark separators with neon glow

    float gridMask = wedgeLine * ringLine;  // 1.0 = cell interior, 0.0 = on grid line

    // Start with video-filled cells at FULL brightness
    vec3 hudColor = wedgeColor;

    // === IDLE vs LIVE SCAFFOLD ROLE (Museum-Mode Polish) ===
    // Idle: super clean face (minimal grid visibility)
    // Live: scaffold provides frame for active content
    float scaffoldContrast = mix(0.7, 0.1, u_idle_mix);  // Nearly invisible in Idle

    // Grid lines: thin dark lines ONLY where gridMask is low
    // In Idle: nearly invisible for super clean look
    vec3 gridLineColor = mix(
        vec3(0.04, 0.02, 0.02),       // Live: subtle red-tinted dark lines
        vec3(0.025, 0.02, 0.02),      // Idle: barely visible dark lines
        u_idle_mix
    );
    float gridLineAlpha = (1.0 - gridMask) * scaffoldContrast;
    hudColor = mix(hudColor, gridLineColor, gridLineAlpha);

    // === NEON GRID GLOW (on grid lines only) ===
    // Use max() to prevent intersection points from getting double-bright
    float glowMaskRaw = (1.0 - gridMask);
    // Clamp to prevent intersection "specs" - max glow at single-line crossing, not double
    float glowMask = min(glowMaskRaw, max(1.0 - wedgeLine, 1.0 - ringLine));
    float glowStrength = mix(QS_GRID_GLOW, 0.0, u_idle_mix);
    // Compeller red/orange glow
    vec3 neonGlow = vec3(0.6, 0.25, 0.15) * glowMask * glowStrength * (1.0 + u_energy * 0.5 * (1.0 - u_idle_mix));
    hudColor += neonGlow;

    // === SEAM LINES - DISABLED (user prefers no borders) ===

    // === INTEGER CELLS (REG Figure 8 style) ===
    // === DEGREE NUMBERS (0-360) IN CELLS ===
    // Each cell displays its integer value - Fig 8 style (white on dark)
    // REG Figure 8: exactly ONE integer per cell, no stacking
    {
        int numRingsLabels = 15;
        // Align label rings to the actual wheel geometry.
        // The visible rings span from innerCore (0.22) to the rim (1.0).
        // Using a smaller ringEnd (e.g. 0.94) shifts every label inward by ~1 ring.
        float ringStart = 0.22;
        float ringEnd = 1.0;
        float ringStep = (ringEnd - ringStart) / float(numRingsLabels);

        // Which ring are we in?
        // Clamp to avoid the rim mapping to an out-of-range ring index when normDist ~= 1.0.
        float ringNorm = clamp((normDist - ringStart) / (ringEnd - ringStart), 0.0, 0.999999);
        int labelRing = int(floor(ringNorm * float(numRingsLabels)));
        labelRing = clamp(labelRing, 0, numRingsLabels - 1);

        // === HARD RADIAL GATE ===
        // Pixel must be within THIS ring's radial bounds to render its digit.
        // This prevents digit stacking ("barcode" artifact).
        float ringInner = ringStart + float(labelRing) * ringStep;
        float ringOuter = ringStart + float(labelRing + 1) * ringStep;
        bool inRingBounds = (normDist >= ringInner && normDist < ringOuter);

        // Calculate the integer for this cell (REG Fig 8 mapping)
        // Each wedge is a residue class (mod 24). Each ring adds +24.
        // IMPORTANT: labelRing increases outward, so the NORTH wedge must
        // increase outward: 24, 48, ..., 360 (outermost).
        // REG Figure 8 numbering:
        // ring 0 (inner) shows residues 1..24 (with 24 at North wedgeIndex==0)
        // each outward ring adds +24, ending at 360.
        int cellDegree = getIntegerForCell(wedgeIndex, labelRing, numRingsLabels);

        // === QUATERNION SYMMETRY VISUALIZATION (per REG paper Figures 4, 5, 8) ===
        // Q(A) = {A, 360-A, 180-A, 180+A} - quaternion complement groups
        //
        // Two modes controlled by u_qs_dominant_only:
        //   1 = DOMINANT-ONLY: Clean 4-cell quaternion set from dominant chroma
        //   0 = ALL-NOTES: All 12 chromas, each with REG quaternion symmetry

        float threshold = 0.12;
        int numRingsQ = 15;  // Use different name to avoid GLSL redeclaration error
        int ringTolerance = 1;  // Allow ±1 ring for smoother response

        // This cell's wedge (1-24) for later use
        int thisWedge = ((cellDegree - 1) % 24) + 1;

        int quartetMember = 0;
        float maxE = 0.0;
        bool isComplement = false;
        int activeChroma = -1;  // Which chroma activated this cell

        if (u_qs_dominant_only == 1) {
            // === DOMINANT-ONLY MODE ===
            // Dominant chroma (0-11) → odd wedge (1,3,5...23)
            int domWedge = u_dom_chroma_latched * 2 + 1;
            float domEnergy = getChroma(u_dom_chroma_latched);

            // Dominant's target ring based on energy: energy 0→ring 0, energy 1→ring 14
            int domTargetRing = int(domEnergy * float(numRingsQ - 1) + 0.5);
            domTargetRing = clamp(domTargetRing, 0, numRingsQ - 1);

            // Compute the active integer A from dominant's position
            // A = ring * 24 + wedge (1-360, matching REG Figure 8)
            int domA = domTargetRing * 24 + domWedge;

            // Compute quaternion set: Q(A) = {A, 360-A, 180-A, 180+A}
            int q1 = domA;
            int q2 = 360 - domA; if (q2 <= 0) q2 += 360;
            int q3 = 180 - domA; if (q3 <= 0) q3 += 360;
            int q4 = (180 + domA) % 360; if (q4 == 0) q4 = 360;

            // This cell lights up ONLY if it's in the quaternion set of the dominant
            if (cellDegree == q1) quartetMember = 1;
            else if (cellDegree == q2) quartetMember = 2;
            else if (cellDegree == q3) quartetMember = 3;
            else if (cellDegree == q4) quartetMember = 4;

            // All 4 members use dominant's energy (they're a unified set)
            maxE = (quartetMember > 0 && domEnergy > threshold) ? domEnergy : 0.0;
            isComplement = (quartetMember > 0 && domEnergy > threshold);
            activeChroma = u_dom_chroma_latched;
        } else {
            // === ALL-NOTES MODE with REG symmetry ===
            // Each of 12 chromas lights its own Q(A) quaternion set
            // Cell activates if it's in ANY active chroma's quaternion set
            for (int c = 0; c < 12; c++) {
                float chromaEnergy = getChroma(c);
                if (chromaEnergy <= threshold) continue;

                // Chroma c → odd wedge (1,3,5...23)
                int chromaWedge = c * 2 + 1;

                // Target ring based on energy
                int chromaTargetRing = int(chromaEnergy * float(numRingsQ - 1) + 0.5);
                chromaTargetRing = clamp(chromaTargetRing, 0, numRingsQ - 1);

                // Compute integer A for this chroma
                int chromaA = chromaTargetRing * 24 + chromaWedge;

                // Compute quaternion set: Q(A) = {A, 360-A, 180-A, 180+A}
                int cq1 = chromaA;
                int cq2 = 360 - chromaA; if (cq2 <= 0) cq2 += 360;
                int cq3 = 180 - chromaA; if (cq3 <= 0) cq3 += 360;
                int cq4 = (180 + chromaA) % 360; if (cq4 == 0) cq4 = 360;

                // Check if this cell is in this chroma's quaternion set
                int member = 0;
                if (cellDegree == cq1) member = 1;
                else if (cellDegree == cq2) member = 2;
                else if (cellDegree == cq3) member = 3;
                else if (cellDegree == cq4) member = 4;

                // If this chroma activates this cell with higher energy, use it
                if (member > 0 && chromaEnergy > maxE) {
                    quartetMember = member;
                    maxE = chromaEnergy;
                    isComplement = true;
                    activeChroma = c;
                }
            }
        }

        // === Figure 5: E+W=N Pattern (East + West = North) ===
        // "East and West modulus numbers sum to every other North modulus number"
        // E(k) + W(k) = N(2k) where E=6+24k, W=18+24k, N=24+48k
        // Only triggers when E and W cells at matching energy rings are active
        bool isEWNTarget = false;
        float ewEnergy = 0.0;

        // Check if this is a North cell at an "every other" position
        if (thisWedge == 24 && labelRing % 2 == 0) {
            // For N at ring 2k, check if E at ring k and W at ring k are active
            int kIndex = labelRing / 2;  // k = 0, 1, 2, ...
            float eastEnergy = getChroma(2);   // wedge 6 chroma
            float westEnergy = getChroma(8);   // wedge 18 chroma

            // E and W must both be active AND their energy must target ring k
            int eastTargetRing = int(eastEnergy * float(numRingsQ - 1) + 0.5);
            int westTargetRing = int(westEnergy * float(numRingsQ - 1) + 0.5);

            if (eastEnergy > threshold && westEnergy > threshold &&
                abs(eastTargetRing - kIndex) <= ringTolerance &&
                abs(westTargetRing - kIndex) <= ringTolerance) {
                isEWNTarget = true;
                ewEnergy = min(eastEnergy, westEnergy);
                if (ewEnergy > maxE) {
                    maxE = ewEnergy;
                    quartetMember = 5;  // E+W=N pattern
                    isComplement = true;
                }
            }
        }

        // Check cardinal axes (0/360, 90, 180, 270) - reflection points
        bool isCardinal = (cellDegree == 360 || cellDegree == 90 ||
                          cellDegree == 180 || cellDegree == 270);

        // === Figure 6: Central Modulus Symmetry (N, E, S, W intersection) ===
        // Only activates when this cell's ring matches the axis energy level
        float northEnergy = getChroma(11);
        float eastEnergy_f6 = getChroma(2);
        float southEnergy = getChroma(5);
        float westEnergy_f6 = getChroma(8);

        // Target rings for each central axis
        int northTargetRing = int(northEnergy * float(numRingsQ - 1) + 0.5);
        int eastTargetRing_f6 = int(eastEnergy_f6 * float(numRingsQ - 1) + 0.5);
        int southTargetRing = int(southEnergy * float(numRingsQ - 1) + 0.5);
        int westTargetRing_f6 = int(westEnergy_f6 * float(numRingsQ - 1) + 0.5);

        // Check if this cell is on a central axis AND at the right ring
        bool isOnCentralAxis = (thisWedge == 24 || thisWedge == 6 ||
                                thisWedge == 12 || thisWedge == 18);

        bool isCentralResonance = false;
        float centralEnergy = 0.0;
        int centralType = 0;

        if (isOnCentralAxis) {
            // N/S symmetry (Figure 4) - both N and S active at their target rings
            bool nActive = (northEnergy > threshold && thisWedge == 24 &&
                           abs(labelRing - northTargetRing) <= ringTolerance);
            bool sActive = (southEnergy > threshold && thisWedge == 12 &&
                           abs(labelRing - southTargetRing) <= ringTolerance);

            // E/W symmetry - both E and W active at their target rings
            bool eActive = (eastEnergy_f6 > threshold && thisWedge == 6 &&
                           abs(labelRing - eastTargetRing_f6) <= ringTolerance);
            bool wActive = (westEnergy_f6 > threshold && thisWedge == 18 &&
                           abs(labelRing - westTargetRing_f6) <= ringTolerance);

            // Check for paired resonance (both axes of a pair active)
            bool nsPairActive = (northEnergy > threshold && southEnergy > threshold);
            bool ewPairActive = (eastEnergy_f6 > threshold && westEnergy_f6 > threshold);

            if (nsPairActive && (nActive || sActive)) {
                isCentralResonance = true;
                centralEnergy = min(northEnergy, southEnergy);
                centralType = 6;  // N/S pair
            }
            if (ewPairActive && (eActive || wActive)) {
                isCentralResonance = true;
                float ew = min(eastEnergy_f6, westEnergy_f6);
                if (ew > centralEnergy) centralEnergy = ew;
                centralType = (centralType == 6) ? 8 : 7;  // E/W or all four
            }

            if (isCentralResonance && centralEnergy > maxE) {
                maxE = centralEnergy;
                quartetMember = centralType;
                isComplement = true;
            }
        }

        // Cell center in polar coordinates (same angle convention as wedge selection)
        // NOTE: angle = atan(hudUV.x, hudUV.y) so x=sin(theta), y=cos(theta).
        float cellAngle = (float(wedgeIndex) + 0.5) * wedgeAngle;
        float cellRadius = (ringStart + (float(labelRing) + 0.5) * ringStep) * hudRadius;

        // Cell center in the SAME coordinate space as hudUV.
        // Angle 0 at North, increasing clockwise. Standard trig:
        // x = sin(angle), y = cos(angle) gives CW rotation from North
        vec2 cellCenterAspect = vec2(sin(cellAngle), cos(cellAngle)) * cellRadius;

        // Local digit space: de-aspect X so the font SDF stays square.
        vec2 fromCenter = hudUV - cellCenterAspect;
        vec2 digitUV = vec2(fromCenter.x / aspect, fromCenter.y);

        // Cell size for digit rendering.
        // The limiting dimension is almost always the ring thickness (radial height),
        // while the wedge span (arcLen) provides horizontal room.
        float ringThickness = ringStep * hudRadius;
        float midR = (ringStart + (float(labelRing) + 0.5) * ringStep) * hudRadius;
        float arcLen = midR * wedgeAngle; // approximate arc span in HUD-space
        // Use ring thickness as the main scale; it keeps digits legible in every ring.
        float cellSize = ringThickness * 1.25;

        // === REG-SAFE VISUAL: ONLY show numbers for ACTIVE frequencies ===
        // Numbers appear when their frequency is playing - clean, minimal look
        bool showNumber = false;

        // Show number ONLY if this cell is part of active quartet
        if (isComplement && maxE > 0.15) {
            showNumber = true;
        }

        // Also show cardinal axis labels (0/360, 90, 180, 270) when their axis is active
        if (isCardinal) {
            float axisEnergy = 0.0;
            if (wedgeIndex == 0) axisEnergy = getChroma(11);  // North
            else if (wedgeIndex == 6) axisEnergy = getChroma(2);   // East
            else if (wedgeIndex == 12) axisEnergy = getChroma(5);  // South
            else if (wedgeIndex == 18) axisEnergy = getChroma(8);  // West

            if (axisEnergy > 0.12) {
                // Show this ring's cardinal if energy matches
                int targetRing = int(axisEnergy * 14.0 + 0.5);
                if (abs(labelRing - targetRing) <= 2) {
                    showNumber = true;
                }
            }
        }

        // === LAYER 5: TEXT RENDERING (CRISP, topmost) ===
        float halfW = (arcLen / aspect) * 0.5;
        float halfH = ringThickness * 0.5;

        // Scale boost for inner rings (they're smaller, need bigger text)
        float ringScaleBoost = 1.0;
        if (labelRing < 3) {
            ringScaleBoost = QS_DIGIT_INNER_SCALE;  // Bigger text in inner rings
        } else if (labelRing < 6) {
            ringScaleBoost = 1.0 + (QS_DIGIT_INNER_SCALE - 1.0) * 0.5;  // Medium boost
        }

        float scaledHalfW = halfW * QS_DIGIT_SCALE * ringScaleBoost;
        float scaledHalfH = halfH * QS_DIGIT_SCALE * ringScaleBoost;

        bool inCellBounds = (abs(digitUV.x) <= halfW) && (abs(digitUV.y) <= halfH);
        float digitMask = 0.0;

        if (inRingBounds && u_qs_numbers_has_texture > 0 && inCellBounds && showNumber) {
            // Atlas lookup: convert cellDegree (1-360) to atlas position
            // REG MATH PRESERVED: index calculation unchanged
            int atlasIdx = cellDegree - 1;  // 0-359
            int atlasCol = atlasIdx - (atlasIdx / 24) * 24;  // mod 24 (0-23)
            int atlasRow = atlasIdx / 24;   // 0-14

            // Map digitUV to 0-1 within SCALED cell (CRISP sampling)
            float numScale = min(scaledHalfW, scaledHalfH) * 2.0 * 0.95;
            vec2 localUV = vec2(
                digitUV.x / numScale + 0.5,
                digitUV.y / numScale + 0.5
            );

            // Only render if within the square number area
            if (localUV.x >= 0.0 && localUV.x <= 1.0 && localUV.y >= 0.0 && localUV.y <= 1.0) {
                // Flip localUV.y so digits render upright
                localUV.y = 1.0 - localUV.y;

                // Atlas cell UV with slight padding to avoid edge bleed
                vec2 cellUVBase = vec2(float(atlasCol), float(atlasRow)) / vec2(24.0, 15.0);
                vec2 cellUVSize = vec2(1.0, 1.0) / vec2(24.0, 15.0);
                // Pad inward slightly for cleaner edges
                vec2 padding = cellUVSize * 0.02;
                vec2 atlasUV = cellUVBase + padding + localUV * (cellUVSize - padding * 2.0);

                // Sample texture with better filtering hint
                vec4 numTex = texture(u_qs_numbers_tex, atlasUV);
                // Sharpen the alpha for crisper edges
                digitMask = smoothstep(0.3, 0.7, numTex.a);
            }
        }

        // === REG-SAFE VISUAL: Highlight via BORDER GLOW only (NO video recoloring) ===
        // Primary hits (quartetMember=1): bright border glow
        // Symmetric matches (quartetMember=2,3,4): dimmer border glow
        // VIDEO IS NEVER RECOLORED - only borders and seams are affected
        if (isComplement && u_qs_debug_color == 0) {
            // Smooth pulsing animation
            float pulse = 0.85 + 0.15 * sin(u_time * TAU * QS_HIGHLIGHT_PULSE_RATE);

            // Get highlight color based on type
            vec3 highlightTint = getQuartetColor(quartetMember, 1.0);
            float intensity = maxE;

            // === BORDER GLOW (NOT cell fill) ===
            // Compute distance to cell edge
            float cellEdgeDist = min(
                min(abs(digitUV.x) - halfW * 0.85, halfW * 0.85 - abs(digitUV.x)),
                min(abs(digitUV.y) - halfH * 0.85, halfH * 0.85 - abs(digitUV.y))
            );

            // Border glow parameters
            float outlineAA = fwidth(cellEdgeDist) * QS_AA_SCALE;
            float pixelOutline = QS_HIGHLIGHT_OUTLINE_PX / min(u_resolution.x, u_resolution.y) * (1.0 / hudRadius);

            // Primary hits get BOLD border, symmetric matches get SUBTLE border
            float borderWidth = (quartetMember == 1) ? pixelOutline * 1.5 : pixelOutline;
            float borderGlow = smoothstep(borderWidth * 2.0, -borderWidth * 0.5, cellEdgeDist);

            // Animated scan effect along border (subtle pulse)
            float scanPhase = fract(u_time * 0.5 + float(wedgeIndex) * 0.1);
            float scanPulse = smoothstep(0.0, 0.3, scanPhase) * smoothstep(1.0, 0.7, scanPhase);

            // Intensity based on quartet membership
            float glowIntensity;
            if (quartetMember == 1) {
                // PRIMARY HIT - bright border glow
                glowIntensity = QS_HIGHLIGHT_BORDER_GLOW * intensity * pulse;
            } else {
                // SYMMETRIC MATCH - brighter border glow
                glowIntensity = QS_HIGHLIGHT_BORDER_GLOW * 0.7 * intensity * pulse;
            }

            // Apply border glow (additive, NOT replacing cell color)
            hudColor += highlightTint * borderGlow * glowIntensity * (0.8 + scanPulse * 0.2);
        }

        // === LAYER 5: DIGIT RENDERING (REG-SAFE: Improved readability) ===
        // Proper layer order: pill bg → drop shadow → dark outline → text
        // Skip digit rendering if numbers hidden (but keep cell highlighting)
        bool renderDigits = (u_qs_hide_numbers == 0);

        // Pill background (rounded rect behind text)
        if (inCellBounds && showNumber && renderDigits) {
            // Compute pill shape distance
            float pillW = scaledHalfW * 1.3;
            float pillH = scaledHalfH * 1.1;
            vec2 pillDist = abs(digitUV) - vec2(pillW, pillH) + vec2(pillH * 0.3);
            float pillSDF = length(max(pillDist, 0.0)) + min(max(pillDist.x, pillDist.y), 0.0) - pillH * 0.3;
            float pillMask = smoothstep(0.003, -0.003, pillSDF) * QS_DIGIT_PLATE_OPACITY;

            // Auto-dim on bright backgrounds
            float bgLum = dot(hudColor, vec3(0.299, 0.587, 0.114));
            pillMask *= mix(1.0, 0.7, bgLum);

            // Apply pill background
            hudColor = mix(hudColor, vec3(0.01, 0.012, 0.018), pillMask);
        }

        if (digitMask > 0.01 && showNumber && renderDigits) {
            // === DROP SHADOW (offset dark copy) ===
            float shadowMask = 0.0;
            if (u_qs_numbers_has_texture > 0) {
                vec2 shadowUV = digitUV - vec2(QS_DIGIT_SHADOW_OFFSET * ringScaleBoost, -QS_DIGIT_SHADOW_OFFSET * ringScaleBoost);
                float numScaleShadow = min(scaledHalfW, scaledHalfH) * 2.0 * 0.95;
                vec2 localShadowUV = vec2(
                    shadowUV.x / numScaleShadow + 0.5,
                    shadowUV.y / numScaleShadow + 0.5
                );
                if (localShadowUV.x >= 0.0 && localShadowUV.x <= 1.0 &&
                    localShadowUV.y >= 0.0 && localShadowUV.y <= 1.0) {
                    localShadowUV.y = 1.0 - localShadowUV.y;
                    int atlasIdxS = cellDegree - 1;
                    int atlasColS = atlasIdxS - (atlasIdxS / 24) * 24;
                    int atlasRowS = atlasIdxS / 24;
                    vec2 cellUVBaseS = vec2(float(atlasColS), float(atlasRowS)) / vec2(24.0, 15.0);
                    vec2 cellUVSizeS = vec2(1.0, 1.0) / vec2(24.0, 15.0);
                    vec2 padS = cellUVSizeS * 0.02;
                    vec2 atlasUVS = cellUVBaseS + padS + localShadowUV * (cellUVSizeS - padS * 2.0);
                    vec4 numTexS = texture(u_qs_numbers_tex, atlasUVS);
                    shadowMask = smoothstep(0.2, 0.5, numTexS.a) * QS_DIGIT_SHADOW_ALPHA;
                }
            }
            // Apply shadow
            hudColor = mix(hudColor, vec3(0.0), shadowMask * 0.7);

            // === DARK OUTLINE (1-2px around text) ===
            float outlineExpand = 0.12;
            float outlineMask = smoothstep(0.01 - outlineExpand, 0.01, digitMask) -
                               smoothstep(0.01, 0.01 + outlineExpand * 0.4, digitMask);
            outlineMask = clamp(outlineMask * QS_DIGIT_OUTLINE_STRENGTH, 0.0, 1.0);

            // Apply dark outline
            hudColor = mix(hudColor, vec3(0.0), outlineMask * 0.85);

            // === TEXT COLOR ===
            // Audio-reactive brightness (subtle)
            float digitGain = 1.0 + QS_DIGIT_AUDIO_BRIGHTNESS * u_energy +
                             QS_DIGIT_KICK_BRIGHTNESS * smoothstep(0.05, 0.25, u_kick);

            // White digits by default (REG Figure 8 style)
            vec3 digitColor = vec3(0.95) * digitGain;

            // Active frequency cells: subtle color tint on TEXT (not video)
            if (isComplement && u_qs_debug_color == 0) {
                float pulse = 0.9 + 0.1 * sin(u_time * 3.0);
                vec3 tint = getQuartetColor(quartetMember, 1.0);
                if (quartetMember == 1) {
                    // Primary hit - colored digit
                    digitColor = mix(vec3(1.0), tint, 0.6) * 1.2 * pulse * digitGain;
                } else {
                    // Symmetric match - subtle tint
                    digitColor = mix(vec3(0.9), tint, 0.25) * pulse * digitGain;
                }
            }

            // Apply text with strong dark backing for contrast
            hudColor = mix(hudColor, vec3(0.0), digitMask * 0.8);
            hudColor += digitColor * digitMask;
        }
    }

    // === QUATERNION SET SPOKE HIGHLIGHTING ===
    // Highlight entire spokes (wedges) that contain Q(A) members
    // This creates a visual connection between the 4 complementary cells
    {
        // Check if any cell in this wedge is part of the quartet
        bool wedgeHasQuartet = false;
        int wedgeQuartetMember = 0;

        // Check each ring in this wedge for quartet membership
        for (int r = 0; r < 15; r++) {
            int cellN = r * 24 + wedgeIndex + 1;
            int member = isInQuartet(cellN);
            if (member > 0) {
                wedgeHasQuartet = true;
                wedgeQuartetMember = member;
                break;
            }
        }

        if (wedgeHasQuartet && u_qs_intensity > 0.1) {
            float pulse = 0.85 + 0.15 * sin(u_time * 3.0);
            vec3 spokeColor = getQuartetColor(wedgeQuartetMember, u_qs_intensity * 0.15);

            // Very subtle spoke glow (reduced to avoid full-wedge appearance)
            float spokeGlow = 0.05 * pulse;
            hudColor += spokeColor * spokeGlow;
        }
    }

    // === QUATERNION CHARGE POLARITY VISUALIZATION ===
    // Central axes represent quaternion charge polarities:
    // North = √-1 (imaginary+), East = +1 (real+)
    // South = -√-1 (imaginary-), West = -1 (real-)
    // When all 4 members sum, they return to theoretical zero (360°/1 rotation)
    {
        // Redefine constants (these match the QS wheel geometry)
        float cpRingStart = 0.22;
        float cpRingEnd = 1.0;
        float cpRingStep = (cpRingEnd - cpRingStart) / 15.0;

        // Show charge symbols on outermost ring of central axes
        float chargeRingRadius = (cpRingStart + 14.5 * cpRingStep) * hudRadius;
        float chargeSize = 0.015;

        // North marker: √-1 (i) - cyan/blue tint
        float northAng = 0.0;  // Top
        vec2 northPos = vec2(sin(northAng), cos(northAng)) * chargeRingRadius;
        northPos.x *= aspect;
        float dNorth = length(hudUV - northPos);
        if (dNorth < chargeSize) {
            float cpAlpha = smoothstep(chargeSize, chargeSize * 0.3, dNorth);
            float nPulse = 0.7 + 0.3 * sin(u_time * 2.0);
            float nEnergy = getChroma(11);
            if (nEnergy > 0.12) {
                hudColor = mix(hudColor, vec3(0.4, 0.8, 1.0) * nPulse, cpAlpha * nEnergy);
            }
        }

        // East marker: +1 - gold/yellow tint
        float eastAng = PI * 0.5;  // Right (90°)
        vec2 eastPos = vec2(sin(eastAng), cos(eastAng)) * chargeRingRadius;
        eastPos.x *= aspect;
        float dEast = length(hudUV - eastPos);
        if (dEast < chargeSize) {
            float cpAlpha = smoothstep(chargeSize, chargeSize * 0.3, dEast);
            float ePulse = 0.7 + 0.3 * sin(u_time * 2.0 + 0.5);
            float eEnergy = getChroma(2);
            if (eEnergy > 0.12) {
                hudColor = mix(hudColor, vec3(1.0, 0.9, 0.3) * ePulse, cpAlpha * eEnergy);
            }
        }

        // South marker: -√-1 (-i) - magenta tint
        float southAng = PI;  // Bottom (180°)
        vec2 southPos = vec2(sin(southAng), cos(southAng)) * chargeRingRadius;
        southPos.x *= aspect;
        float dSouth = length(hudUV - southPos);
        if (dSouth < chargeSize) {
            float cpAlpha = smoothstep(chargeSize, chargeSize * 0.3, dSouth);
            float sPulse = 0.7 + 0.3 * sin(u_time * 2.0 + 1.0);
            float sEnergy = getChroma(5);
            if (sEnergy > 0.12) {
                hudColor = mix(hudColor, vec3(1.0, 0.4, 0.8) * sPulse, cpAlpha * sEnergy);
            }
        }

        // West marker: -1 - green tint
        float westAng = PI * 1.5;  // Left (270°)
        vec2 westPos = vec2(sin(westAng), cos(westAng)) * chargeRingRadius;
        westPos.x *= aspect;
        float dWest = length(hudUV - westPos);
        if (dWest < chargeSize) {
            float cpAlpha = smoothstep(chargeSize, chargeSize * 0.3, dWest);
            float wPulse = 0.7 + 0.3 * sin(u_time * 2.0 + 1.5);
            float wEnergy = getChroma(8);
            if (wEnergy > 0.12) {
                hudColor = mix(hudColor, vec3(0.3, 1.0, 0.5) * wPulse, cpAlpha * wEnergy);
            }
        }

        // === CENTER POLARITY INDICATOR with SHAPE-ENCODED CHARGES ===
        // Shows the dominant quaternion charge in the center using geometric shapes:
        // Circle = +1 (East), Triangle up = √-1 (North)
        // Square = -1 (West), Triangle down = -√-1 (South)
        // 4-pointed star = 0 (all balanced)
        float nE = getChroma(11), eE = getChroma(2), sE = getChroma(5), wE = getChroma(8);
        float centerDist = length(hudUV);
        float polarityRadius = cpRingStart * hudRadius * 0.6;

        if (centerDist < polarityRadius) {
            float centerAlpha = smoothstep(polarityRadius, polarityRadius * 0.3, centerDist);

            // Determine dominant polarity
            float maxCardinal = max(max(nE, eE), max(sE, wE));
            float polarityThreshold = 0.12;

            // Background glow in polarity color
            if (maxCardinal > polarityThreshold) {
                vec3 polarityColor = vec3(0.0);
                float polarityPulse = 0.7 + 0.3 * sin(u_time * 3.0);
                int dominantCharge = -1;  // 0=+1, 1=√-1, 2=-1, 3=-√-1

                if (nE >= maxCardinal - 0.01) {
                    polarityColor = vec3(0.3, 0.8, 1.0);  // √-1 cyan
                    dominantCharge = 1;
                } else if (eE >= maxCardinal - 0.01) {
                    polarityColor = vec3(1.0, 0.85, 0.2); // +1 gold
                    dominantCharge = 0;
                } else if (sE >= maxCardinal - 0.01) {
                    polarityColor = vec3(1.0, 0.3, 0.7);  // -√-1 magenta
                    dominantCharge = 3;
                } else if (wE >= maxCardinal - 0.01) {
                    polarityColor = vec3(0.2, 1.0, 0.4);  // -1 green
                    dominantCharge = 2;
                }

                // Subtle background glow
                float bgGlow = centerAlpha * maxCardinal * 0.3;
                hudColor = mix(hudColor, polarityColor * 0.5, bgGlow);

                // === SHAPE-ENCODED CHARGE SYMBOL ===
                float shapeSize = polarityRadius * 0.5;
                float shapeDist = 999.0;

                if (dominantCharge == 0) {
                    // +1: Circle (real positive)
                    shapeDist = length(hudUV) - shapeSize * 0.6;
                } else if (dominantCharge == 1) {
                    // √-1: Triangle pointing UP (imaginary positive)
                    vec2 p = hudUV / shapeSize;
                    float k = sqrt(3.0);
                    p.x = abs(p.x) - 0.5;
                    p.y = p.y + 0.5 / k;
                    if (p.x + k * p.y > 0.0) {
                        p = vec2(p.x - k * p.y, -k * p.x - p.y) / 2.0;
                    }
                    p.x -= clamp(p.x, -1.0, 0.0);
                    shapeDist = -length(p) * sign(p.y) * shapeSize;
                } else if (dominantCharge == 2) {
                    // -1: Square rotated 45° (real negative)
                    vec2 q = abs(hudUV);
                    shapeDist = (q.x + q.y) - shapeSize * 0.5;
                } else if (dominantCharge == 3) {
                    // -√-1: Triangle pointing DOWN (imaginary negative)
                    vec2 p = vec2(hudUV.x, -hudUV.y) / shapeSize;  // Flip Y
                    float k = sqrt(3.0);
                    p.x = abs(p.x) - 0.5;
                    p.y = p.y + 0.5 / k;
                    if (p.x + k * p.y > 0.0) {
                        p = vec2(p.x - k * p.y, -k * p.x - p.y) / 2.0;
                    }
                    p.x -= clamp(p.x, -1.0, 0.0);
                    shapeDist = -length(p) * sign(p.y) * shapeSize;
                }

                // Render shape with glow
                if (shapeDist < 0.008) {
                    float shapeAlpha = smoothstep(0.008, -0.002, shapeDist);
                    float shapePulse = 0.8 + 0.2 * sin(u_time * 4.0);
                    hudColor = mix(hudColor, polarityColor * 1.5 * shapePulse, shapeAlpha * maxCardinal);
                }
                // Outer glow
                if (shapeDist > 0.0 && shapeDist < 0.02) {
                    float glowAlpha = smoothstep(0.02, 0.0, shapeDist) * 0.4;
                    hudColor = mix(hudColor, polarityColor, glowAlpha * maxCardinal * polarityPulse);
                }
            }

            // === ZERO-POINT: 4-POINTED STAR when all charges balanced ===
            // This represents Q(A) summing to theoretical zero (360°/1 rotation)
            if (nE > 0.15 && eE > 0.15 && sE > 0.15 && wE > 0.15) {
                float zeroIntensity = min(min(nE, eE), min(sE, wE));
                float starSize = polarityRadius * 0.4;
                float zeroPulse = 0.6 + 0.4 * sin(u_time * 5.0);

                // 4-pointed star SDF
                vec2 p = hudUV;
                float r = length(p);
                float ang = atan(p.x, p.y);
                // Star shape: 4 points
                float starShape = r - starSize * (0.3 + 0.25 * abs(cos(ang * 2.0)));

                if (starShape < 0.006) {
                    float starAlpha = smoothstep(0.006, -0.002, starShape);
                    // White core with rainbow shimmer for "0"
                    vec3 zeroColor = vec3(1.0) + 0.2 * vec3(
                        sin(u_time * 3.0),
                        sin(u_time * 3.0 + 2.094),
                        sin(u_time * 3.0 + 4.189)
                    );
                    hudColor = mix(hudColor, zeroColor * zeroPulse, starAlpha * zeroIntensity);
                }
                // Star glow
                if (starShape > 0.0 && starShape < 0.015) {
                    float glowAlpha = smoothstep(0.015, 0.0, starShape) * 0.5;
                    hudColor = mix(hudColor, vec3(1.0), glowAlpha * zeroIntensity * zeroPulse);
                }
            }
        }
    }

    // === 4 COMPLEMENT MARKERS ON RIM ===
    // Large visible markers at the exact complement angles
    if (u_qc_active > 0 && u_qc_strength > 0.1) {
        float A = u_qc_w * 180.0 / PI;

        // Marker 0: A (gold)
        float ang0 = A * PI / 180.0;
        vec2 pos0 = vec2(sin(ang0), cos(ang0)) * hudRadius * 0.92;
        pos0.x *= aspect;  // FIX: match hudUV space
        float d0 = length(hudUV - pos0);
        if (d0 < 0.018) {
            float alpha = smoothstep(0.018, 0.008, d0);
            float pulse = 0.85 + 0.15 * sin(u_time * 5.0);
            hudColor = mix(hudColor, vec3(1.0, 0.9, 0.3) * pulse, alpha * u_qc_strength);
        }

        // Marker 1: 360-A (purple)
        float ang1 = mod(360.0 - A, 360.0) * PI / 180.0;
        vec2 pos1 = vec2(sin(ang1), cos(ang1)) * hudRadius * 0.92;
        pos1.x *= aspect;  // FIX: match hudUV space
        float d1 = length(hudUV - pos1);
        if (d1 < 0.018) {
            float alpha = smoothstep(0.018, 0.008, d1);
            float pulse = 0.85 + 0.15 * sin(u_time * 5.0 + 1.0);
            hudColor = mix(hudColor, vec3(0.9, 0.5, 0.95) * pulse, alpha * u_qc_strength);
        }

        // Marker 2: 180-A (blue)
        float ang2 = mod(180.0 - A + 360.0, 360.0) * PI / 180.0;
        vec2 pos2 = vec2(sin(ang2), cos(ang2)) * hudRadius * 0.92;
        pos2.x *= aspect;  // FIX: match hudUV space
        float d2 = length(hudUV - pos2);
        if (d2 < 0.018) {
            float alpha = smoothstep(0.018, 0.008, d2);
            float pulse = 0.85 + 0.15 * sin(u_time * 5.0 + 2.0);
            hudColor = mix(hudColor, vec3(0.5, 0.75, 1.0) * pulse, alpha * u_qc_strength);
        }

        // Marker 3: 180+A (cyan)
        float ang3 = mod(180.0 + A, 360.0) * PI / 180.0;
        vec2 pos3 = vec2(sin(ang3), cos(ang3)) * hudRadius * 0.92;
        pos3.x *= aspect;  // FIX: match hudUV space
        float d3 = length(hudUV - pos3);
        if (d3 < 0.018) {
            float alpha = smoothstep(0.018, 0.008, d3);
            float pulse = 0.85 + 0.15 * sin(u_time * 5.0 + 3.0);
            hudColor = mix(hudColor, vec3(0.4, 0.9, 0.9) * pulse, alpha * u_qc_strength);
        }
    }

    // === CURSOR INDICATOR ===
    // Show current harmonic angle A as a radial line/marker
    if (u_qc_active > 0 && u_qc_strength > 0.01) {
        // Draw cursor at angle corresponding to w (reduced angle)
        float cursorAngle = u_qc_w;  // Already in radians
        vec2 cursorDir = vec2(sin(cursorAngle), cos(cursorAngle));
        cursorDir.x *= aspect;  // FIX: match hudUV space
        cursorDir = normalize(cursorDir);  // Re-normalize after aspect correction

        // Distance from cursor line
        vec2 toPixel = hudUV;
        float projLen = dot(toPixel, cursorDir);
        float perpDist = length(toPixel - cursorDir * projLen);

        // Only show in outer portion of wheel (adjust threshold for aspect)
        float radialDist = length(hudUV);
        if (radialDist > hudRadius * innerCore && radialDist < hudRadius * 0.95 && projLen > 0.0) {
            float lineWidth = 0.004;
            float lineAlpha = smoothstep(lineWidth, lineWidth * 0.5, perpDist);
            vec3 cursorColor = vec3(1.0, 0.9, 0.5);  // Bright gold cursor
            float pulse = 0.8 + 0.2 * sin(u_time * 5.0);
            hudColor = mix(hudColor, cursorColor * pulse, lineAlpha * 0.8 * u_qc_strength);
        }

        // Draw small arrowhead at cursor tip
        vec2 tipPos = cursorDir * hudRadius * 0.92;
        tipPos.x *= aspect;  // FIX: match hudUV space
        float tipDist = length(hudUV - tipPos);
        if (tipDist < 0.012) {
            float tipAlpha = smoothstep(0.012, 0.006, tipDist);
            hudColor = mix(hudColor, vec3(1.0, 0.95, 0.6), tipAlpha * u_qc_strength);
        }
    }

    // === OUTER RIM (Premium styling) ===
    float rim = smoothstep(0.94, 1.0, normDist);
    hudColor = mix(hudColor, vec3(0.35, 0.4, 0.5), rim);

    // === RIM FLARES - DISABLED (user prefers no borders) ===

    // === RADIAL VIGNETTE (depth effect) ===
    float vignette = smoothstep(1.0, 1.0 - QS_VIGNETTE_WIDTH, normDist) * QS_VIGNETTE_STRENGTH;
    hudColor *= (1.0 - vignette);

    // === GLASS HIGHLIGHT ARC - DISABLED for clean look ===
    // Completely disabled to eliminate specular specs
    // vec2 lightDir = normalize(vec2(-0.7, 0.7));
    // vec2 normalizedRadial = normalize(hudUV);
    // float specAngle = dot(normalizedRadial, lightDir);
    // float glassSpec = pow(max(0.0, specAngle), 40.0) * QS_GLASS_SPEC_STRENGTH;
    // glassSpec *= smoothstep(0.0, 0.5, normalizedRadial.y);
    // hudColor += vec3(0.9, 0.95, 1.0) * glassSpec;

    // === FINAL CLAMP (prevent over-bright) ===
    hudColor = clamp(hudColor, 0.0, 1.0);

    // === DEBUG OVERLAY ===
    // When u_qs_debug is enabled, show A and Q(A) values in corner
    if (u_qs_debug > 0) {
        vec2 debugPos = vec2(0.02, 0.95);  // Top-left corner
        vec2 debugUV = uv_screen - debugPos;

        // Show active A value
        if (debugUV.x > 0.0 && debugUV.x < 0.15 && debugUV.y > -0.12 && debugUV.y < 0.0) {
            // Dark background for text
            hudColor = mix(hudColor, vec3(0.0), 0.8);

            // Render "A=" label and value
            float labelScale = 0.008;
            vec2 textPos = vec2(debugUV.x - 0.01, debugUV.y + 0.02);

            // Simple text: just show colored indicators for each Q member
            float indicator = 0.0;
            if (debugUV.x < 0.04) {
                // Q0 = A (gold)
                indicator = smoothstep(0.005, 0.002, length(debugUV - vec2(0.02, -0.02)));
                hudColor += vec3(1.0, 0.9, 0.2) * indicator;
            } else if (debugUV.x < 0.07) {
                // Q1 = 360-A (orange)
                indicator = smoothstep(0.005, 0.002, length(debugUV - vec2(0.05, -0.02)));
                hudColor += vec3(1.0, 0.7, 0.1) * indicator;
            } else if (debugUV.x < 0.10) {
                // Q2 = 180-A (cyan)
                indicator = smoothstep(0.005, 0.002, length(debugUV - vec2(0.08, -0.02)));
                hudColor += vec3(0.2, 0.9, 1.0) * indicator;
            } else {
                // Q3 = 180+A (blue)
                indicator = smoothstep(0.005, 0.002, length(debugUV - vec2(0.11, -0.02)));
                hudColor += vec3(0.1, 0.7, 1.0) * indicator;
            }
        }
    }

    // === TEXTURE OVERLAY (Artifact Masking Layer) ===
    // Subtle procedural texture between wheel and video to hide grid artifacts
    // Only applied in Idle mode to keep Live mode crisp
    if (u_idle_mix > 0.1 && bgAlpha > 0.1) {
        // Radial soft vignette - darker toward grid intersections
        float radialSoft = smoothstep(0.2, 0.9, normDist);

        // Angular pattern following 24-fold structure (soft wave)
        float angularWave = sin(angle * 24.0) * 0.5 + 0.5;
        angularWave = angularWave * 0.03 + 0.97;  // Very subtle: 0.97 to 1.0

        // Ring pattern to soften ring intersections
        float ringWave = sin(normDist * 15.0 * PI) * 0.5 + 0.5;
        ringWave = ringWave * 0.02 + 0.98;  // Very subtle: 0.98 to 1.0

        // Combine into cleanup multiplier
        float cleanup = angularWave * ringWave;
        cleanup = mix(1.0, cleanup, u_idle_mix * 0.5);  // Fade with idle_mix

        // Apply as soft darkening to hide bright artifacts
        hudColor *= cleanup;

        // Optional: subtle radial gradient to pull attention to center
        float centerFocus = 1.0 - (normDist - 0.5) * 0.08 * u_idle_mix;
        hudColor *= centerFocus;
    }

    // === POLARITY CHANGE EFFECT (1s hold → flash → smoke) ===
    // Timer: 1.5 (start) → 0.5 (end hold, start flash) → 0.0 (gone)
    if (u_polarity_effect_timer > 0.0) {
        // Quadrant colors: E=gold, N=cyan, W=magenta, S=green
        vec3 quadrantColors[4] = vec3[4](
            vec3(1.0, 0.85, 0.3),   // E (+1) - gold
            vec3(0.3, 0.9, 1.0),    // N (+i) - cyan
            vec3(1.0, 0.4, 0.85),   // W (-1) - magenta
            vec3(0.4, 1.0, 0.6)     // S (-i) - green
        );
        vec3 effectColor = quadrantColors[u_polarity_quadrant];

        // Effect center: quadrant angle, middle radius
        float effectAngle = u_polarity_effect_angle;
        float effectRadius = 0.55;  // Middle of wheel

        // Angular distance from effect center (wrap-aware)
        float angDiff = angle - effectAngle;
        angDiff = mod(angDiff + PI, TAU) - PI;  // Wrap to [-π, π]

        // Radial distance from effect center
        float radDiff = abs(normDist - effectRadius);

        // === PHASE 1: HOLD (timer 3.0 → 1.0) ===
        if (u_polarity_effect_timer > 1.0) {
            // Small pulsing marker dot (same size as normal, but pulses)
            float pulse = 0.6 + 0.4 * sin(u_time * 8.0);
            float markerDist = length(vec2(angDiff * effectRadius, radDiff));
            float marker = smoothstep(0.04, 0.02, markerDist) * pulse;

            // Direction indicator arc
            float arcAngle = angDiff * float(u_polarity_direction);
            float arcMask = smoothstep(0.0, 0.3, arcAngle) * smoothstep(0.6, 0.3, arcAngle);
            float arcRing = smoothstep(0.03, 0.01, abs(normDist - effectRadius - 0.08));
            float arc = arcMask * arcRing * 0.5;

            hudColor = mix(hudColor, effectColor * 2.0, marker * 0.8);
            hudColor = mix(hudColor, effectColor, arc);
        }
        // === PHASE 2: FADE OUT (timer 1.0 → 0.0) ===
        else {
            float fadeT = u_polarity_effect_timer;  // 1.0 at start, 0 at end

            // Small pulsing marker that fades out
            float pulse = 0.6 + 0.4 * sin(u_time * 8.0);
            float markerDist = length(vec2(angDiff * effectRadius, radDiff));
            float marker = smoothstep(0.04, 0.02, markerDist) * pulse * fadeT;

            hudColor = mix(hudColor, effectColor * 2.0, marker * 0.8);
        }
    }

    // === PORTAL SPHERE SHADING ===
    // When in portal 3D mode, add lighting to enhance sphere appearance
    if (u_cam_in_portal == 1) {
        // Sphere-like lighting: bright center, dark edges (Lambertian-ish)
        float sphereLight = 1.0 - normDist * normDist * 0.6;  // Quadratic falloff
        sphereLight = max(sphereLight, 0.3);  // Don't go too dark

        // Add subtle specular highlight
        float specular = pow(max(0.0, 1.0 - normDist * 1.5), 8.0) * 0.3;

        hudColor *= sphereLight;
        hudColor += vec3(0.8, 0.9, 1.0) * specular;

        // Subtle rim glow to define sphere edge
        float rim = smoothstep(0.7, 1.0, normDist) * 0.4;
        hudColor += vec3(0.3, 0.5, 0.8) * rim;
    }

    return mix(col, hudColor, bgAlpha);
}

// ============ HYPERSPACE WARP EFFECT ============
// Radial speed lines that create the "entering hyperspace" feeling

vec3 hyperspaceWarp(vec2 uv, vec3 col) {
    if (u_warp_intensity < 0.01) return col;

    // Radial coordinates
    float dist = length(uv);
    float angle = atan(uv.y, uv.x);

    // Speed lines: many thin radial streaks
    float numLines = 64.0;
    float lineAngle = mod(angle * numLines / TAU, 1.0);
    float line = smoothstep(0.4, 0.5, lineAngle) * smoothstep(0.6, 0.5, lineAngle);

    // Lines get brighter toward center (tunnel effect)
    float tunnelFade = smoothstep(1.5, 0.1, dist);

    // Animate lines moving inward (toward viewer)
    float speed = u_time * 8.0 + dist * 3.0;
    float pulse = fract(speed) * 0.5 + 0.5;

    // Chromatic aberration - RGB separation for speed feel
    float aberration = u_warp_intensity * 0.03;
    vec2 uvR = uv * (1.0 + aberration);
    vec2 uvB = uv * (1.0 - aberration);

    // Sample with chromatic split
    vec3 warpCol = col;
    warpCol.r = col.r * (1.0 + line * tunnelFade * pulse * 0.5);
    warpCol.b = col.b * (1.0 + line * tunnelFade * pulse * 0.3);

    // Add bright speed streaks
    vec3 streakColor = vec3(0.7, 0.85, 1.0);  // Blueish white
    float streakIntensity = line * tunnelFade * pulse * u_warp_intensity;
    warpCol += streakColor * streakIntensity * 1.5;

    // Radial blur simulation (darken edges, brighten center)
    float radialBlur = 1.0 + (1.0 - dist) * u_warp_intensity * 0.4;
    warpCol *= radialBlur;

    // Vignette intensifies during warp
    float warpVignette = 1.0 - smoothstep(0.3, 1.2, dist) * u_warp_intensity * 0.6;
    warpCol *= warpVignette;

    return mix(col, warpCol, u_warp_intensity);
}

// ============ WARP JUMP QS - HYPERSPACE TRACERS ============
// Procedural hyperspace streaks OUTSIDE the QS wheel portal
// Uses hash-based seed per "lane" for cheap GPU generation

// Hash function for procedural randomness
float warpHash(float n) {
    return fract(sin(n) * 43758.5453);
}

// Generate a single tracer streak at given angle lane
float tracerStreak(vec2 uv, vec2 center, float laneIndex, float time, float portalR) {
    vec2 delta = uv - center;
    float dist = length(delta);
    float angle = atan(delta.y, delta.x);

    // Each lane has a unique angle offset
    float laneAngle = laneIndex * TAU / 64.0;  // 64 lanes around circle
    float angleOffset = angle - laneAngle;

    // Wrap to [-PI, PI]
    angleOffset = mod(angleOffset + PI, TAU) - PI;

    // Streak is a thin radial line at this angle
    float streakWidth = 0.015 + 0.01 * warpHash(laneIndex * 7.3);

    // Only show streak if we're close to the lane angle
    float angleFactor = smoothstep(streakWidth, streakWidth * 0.3, abs(angleOffset));

    // Streak moves outward from center over time (rushing past camera)
    float speed = 2.0 + warpHash(laneIndex * 13.7) * 1.5;
    float phase = fract(time * speed + warpHash(laneIndex * 23.1));

    // Streak is a segment along the radial direction
    float streakStart = phase * 0.8 + portalR + 0.02;
    float streakEnd = streakStart + 0.08 + 0.06 * warpHash(laneIndex * 31.3);

    // Only visible between start and end radius, and outside portal
    float radialFactor = smoothstep(streakStart, streakStart + 0.01, dist) *
                         smoothstep(streakEnd, streakEnd - 0.02, dist) *
                         smoothstep(portalR + 0.01, portalR + 0.03, dist);

    return angleFactor * radialFactor;
}

// Full tracer field (many streaks)
vec3 hyperspaceTracers(vec2 uv, vec2 center, float portalR, float intensity, float time) {
    if (intensity < 0.01) return vec3(0.0);

    vec3 tracers = vec3(0.0);

    // Generate 64 tracer lanes (cheap loop)
    for (int i = 0; i < 64; i++) {
        float streak = tracerStreak(uv, center, float(i), time, portalR);

        // Color varies slightly per streak (blue-white to cyan)
        vec3 streakColor = mix(
            vec3(0.7, 0.85, 1.0),
            vec3(0.5, 0.9, 1.0),
            warpHash(float(i) * 17.3)
        );

        // Brightness varies
        float brightness = 0.5 + 0.5 * warpHash(float(i) * 41.7);

        tracers += streakColor * streak * brightness;
    }

    return tracers * intensity * 1.5;
}

// ============ WARP JUMP QS VISUAL EFFECTS ============
// QS wheel IS the portal. Tracers outside, portal inside.
// The wheel shrinks → flash → ripple expands to new scene

// Apply vortex UV distortion - creates spiral pull toward center
vec2 vortexDistortUV(vec2 uv, vec2 center, float strength) {
    if (strength < 0.01) return uv;

    vec2 delta = uv - center;
    float dist = length(delta);
    float angle = atan(delta.y, delta.x);

    // Spiral pull - angular offset increases as we approach center
    // Creates the "swirling into the portal" effect
    float falloff = 1.0 - smoothstep(0.0, 0.6, dist);
    float spiralAmount = strength * falloff * 3.0;

    // Add radial compression (pull toward center)
    float radialPull = strength * falloff * 0.3;
    float newDist = dist * (1.0 - radialPull);

    // Apply spiral rotation
    angle += spiralAmount * (0.5 - dist * 0.8);

    return center + newDist * vec2(cos(angle), sin(angle));
}

// Chromatic aberration - radial RGB channel separation
// Creates prismatic fringing that intensifies toward edges
vec3 applyChromatic(vec3 col, vec2 uv, vec2 center, float intensity) {
    if (intensity < 0.01) return col;

    vec2 delta = uv - center;
    float dist = length(delta);
    float angle = atan(delta.y, delta.x);

    // RGB offsets based on radial distance
    // Red shifts outward, blue shifts inward
    float spread = intensity * dist * 0.8;

    // Simulate channel separation via color manipulation
    // This approximates true chromatic aberration without re-sampling
    vec3 result;

    // Red channel gets boosted toward edges
    result.r = col.r * (1.0 + spread * 0.4);

    // Green stays relatively neutral
    result.g = col.g * (1.0 - spread * 0.05);

    // Blue channel gets reduced at edges (appears to shift inward)
    result.b = col.b * (1.0 - spread * 0.35);

    // Add prismatic edge fringing for extra effect
    float edgeFringe = smoothstep(0.15, 0.5, dist) * intensity;

    // Cyan fringe on one side, red-orange on other (radial)
    vec2 dir = normalize(delta);
    float fringeAngle = dot(dir, vec2(1.0, 0.0));

    result.r += edgeFringe * 0.15 * (1.0 + fringeAngle);
    result.b += edgeFringe * 0.1 * (1.0 - fringeAngle);

    return result;
}

vec3 warpJumpEffects(vec2 uv, vec3 col) {
    if (u_warp_active == 0) return col;

    vec2 center = u_warp_portal_center;

    // ================================================================
    // VORTEX UV DISTORTION
    // Apply spiral pull toward center - creates "swirling into portal" feel
    // ================================================================
    vec2 warpedUV = vortexDistortUV(uv, center, u_warp_vortex);

    // Use warped UV for effect calculations (original uv for masks)
    vec2 delta = warpedUV - center;
    float dist = length(delta);
    float angle = atan(delta.y, delta.x);

    // Original distance for portal mask (unwarped)
    vec2 origDelta = uv - center;
    float origDist = length(origDelta);

    float portalR = u_warp_portal_radius;

    vec3 result = col;

    // ================================================================
    // PORTAL MASK (use original UV for clean boundary)
    // Inside portal = QS wheel (preserved), Outside = hyperspace tracers
    // ================================================================
    float portalMask = smoothstep(portalR, portalR - 0.01, origDist);  // 1 inside, 0 outside

    // ================================================================
    // HYPERSPACE TRACERS (outside portal only)
    // Apply chromatic aberration to tracers for RGB split effect
    // ================================================================
    if (u_warp_tracer_intensity > 0.01) {
        // Sample tracers with chromatic aberration (RGB at different positions)
        vec3 tracers;
        if (u_warp_chromatic > 0.01) {
            // Chromatic aberration: sample R/G/B at radially offset positions
            float chromaSpread = u_warp_chromatic * 0.02;
            vec2 dir = normalize(origDelta + vec2(0.001));

            vec2 uv_r = warpedUV + dir * chromaSpread;
            vec2 uv_g = warpedUV;
            vec2 uv_b = warpedUV - dir * chromaSpread * 0.7;

            vec3 t_r = hyperspaceTracers(uv_r, center, portalR, u_warp_tracer_intensity, u_time);
            vec3 t_g = hyperspaceTracers(uv_g, center, portalR, u_warp_tracer_intensity, u_time);
            vec3 t_b = hyperspaceTracers(uv_b, center, portalR, u_warp_tracer_intensity, u_time);

            tracers = vec3(t_r.r, t_g.g, t_b.b);
        } else {
            tracers = hyperspaceTracers(warpedUV, center, portalR, u_warp_tracer_intensity, u_time);
        }

        // Tracers darken underlying scene (hyperspace void)
        float voidDark = (1.0 - portalMask) * u_warp_tracer_intensity * 0.8;
        result *= 1.0 - voidDark;

        // Add tracers outside portal
        result += tracers * (1.0 - portalMask);
    }

    // ================================================================
    // PHASE 1: ACQUIRE - QS wheel becomes hero focus
    // ================================================================
    if (u_warp_phase == WARP_PHASE_ACQUIRE) {
        // Subtle inward field flow (background bends toward portal)
        float flowStrength = u_warp_strength * 0.3;
        float flowLines = sin(angle * 12.0 + dist * 20.0 - u_time * 3.0) * 0.5 + 0.5;
        flowLines *= smoothstep(portalR + 0.3, portalR + 0.05, dist) * flowStrength;
        result += vec3(0.3, 0.5, 0.8) * flowLines * 0.2;

        // Portal edge glow (QS wheel rim emphasis)
        float rimGlow = smoothstep(portalR + 0.02, portalR - 0.01, dist) *
                        smoothstep(portalR - 0.04, portalR - 0.01, dist);
        result += vec3(0.4, 0.7, 1.0) * rimGlow * u_warp_qs_seam_glow;
    }

    // ================================================================
    // PHASE 2: TUNNEL - Shrinking portal, full hyperspace outside
    // ================================================================
    else if (u_warp_phase == WARP_PHASE_TUNNEL) {
        // Portal spin effect (corkscrew into the hole)
        float spinEffect = sin(angle * 6.0 + u_warp_portal_spin * 2.0) * 0.5 + 0.5;
        spinEffect *= smoothstep(portalR + 0.02, portalR - 0.02, dist);
        result += vec3(0.5, 0.7, 1.0) * spinEffect * 0.15;

        // Portal edge is bright white-blue (event horizon)
        float edgeGlow = smoothstep(portalR + 0.015, portalR, dist) *
                         smoothstep(portalR - 0.015, portalR, dist);
        result += vec3(0.8, 0.9, 1.0) * edgeGlow * 2.0;

        // Core glow inside portal
        float coreGlow = exp(-dist / max(portalR, 0.01) * 2.0) * portalMask;
        result += vec3(0.6, 0.8, 1.0) * coreGlow * 0.5;

        // === FLASH at t ≈ 0.52 ===
        // Flash starts inside portal, bleeds outward
        float flashInside = u_warp_flash * portalMask * 3.0;
        float flashBleed = u_warp_flash * smoothstep(portalR + 0.1, portalR, dist) * 1.5;
        result += vec3(1.0) * (flashInside + flashBleed);
    }

    // ================================================================
    // PHASE 3: BURST - Ripple expands, reveals new scene
    // ================================================================
    else if (u_warp_phase == WARP_PHASE_BURST) {
        // Ripple wave expanding outward
        float rippleRadius = u_warp_ripple * 1.2;  // Expands beyond screen
        float rippleWidth = 0.03 + u_warp_ripple * 0.02;
        float rippleDist = abs(dist - rippleRadius);
        float ripple = smoothstep(rippleWidth, rippleWidth * 0.2, rippleDist);

        // Ripple color: white → gold
        vec3 rippleColor = mix(vec3(1.0, 1.0, 1.0), vec3(1.0, 0.9, 0.5), u_warp_ripple);
        result += rippleColor * ripple * 2.0;

        // Secondary ripple (trailing wave)
        float ripple2Radius = rippleRadius * 0.7;
        float ripple2Dist = abs(dist - ripple2Radius);
        float ripple2 = smoothstep(rippleWidth * 0.8, rippleWidth * 0.2, ripple2Dist);
        ripple2 *= (1.0 - u_warp_ripple) * 0.4;
        result += rippleColor * ripple2;

        // Inside ripple = new scene brightens (reveal effect)
        float revealMask = smoothstep(rippleRadius + 0.02, rippleRadius - 0.02, dist);
        result *= 1.0 + revealMask * 0.3;

        // Residual flash fading
        float residualFlash = u_warp_flash * portalMask;
        result += vec3(1.0, 0.95, 0.9) * residualFlash;

        // Portal glow fading
        float portalFade = exp(-dist * 4.0) * (1.0 - u_warp_phase_t) * 0.4;
        result += vec3(0.6, 0.8, 1.0) * portalFade;
    }

    // ================================================================
    // FINAL CHROMATIC ABERRATION
    // Apply prismatic color separation to entire result
    // ================================================================
    result = applyChromatic(result, uv, center, u_warp_chromatic);

    return result;
}

// ============ SDF DIGIT RENDERING ============
// Procedural 7-segment style digits for frequency labels
// GPU-cheap: uses SDF math, no texture lookups

// SDF for a horizontal segment (used in 7-segment display)
float sdfSegH(vec2 p, float w, float h) {
    vec2 d = abs(p) - vec2(w * 0.5, h * 0.5);
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
}

// SDF for a vertical segment
float sdfSegV(vec2 p, float w, float h) {
    vec2 d = abs(p) - vec2(w * 0.5, h * 0.5);
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
}

// 7-segment digit SDF (returns 1.0 inside digit, 0.0 outside)
// p: position relative to digit center, size: digit dimensions
float sdfDigit(vec2 p, int digit, float size) {
    float sw = size * 0.15;   // Segment width
    float sh = size * 0.4;    // Segment height (vertical)
    float gap = size * 0.05;  // Small gap between segments

    // Segment positions (7-segment layout)
    // Top horizontal (a)
    float a = sdfSegH(p - vec2(0.0, size * 0.4), sh, sw);
    // Top-right vertical (b)
    float b = sdfSegV(p - vec2(size * 0.22, size * 0.2), sw, sh);
    // Bottom-right vertical (c)
    float c = sdfSegV(p - vec2(size * 0.22, -size * 0.2), sw, sh);
    // Bottom horizontal (d)
    float d = sdfSegH(p - vec2(0.0, -size * 0.4), sh, sw);
    // Bottom-left vertical (e)
    float e = sdfSegV(p - vec2(-size * 0.22, -size * 0.2), sw, sh);
    // Top-left vertical (f)
    float f = sdfSegV(p - vec2(-size * 0.22, size * 0.2), sw, sh);
    // Middle horizontal (g)
    float g = sdfSegH(p, sh, sw);

    // Digit patterns (which segments are ON for 0-9)
    // 0: a,b,c,d,e,f    1: b,c    2: a,b,d,e,g    3: a,b,c,d,g
    // 4: b,c,f,g        5: a,c,d,f,g    6: a,c,d,e,f,g   7: a,b,c
    // 8: all            9: a,b,c,d,f,g

    float result = 1000.0;  // Start with large value (no segments)

    if (digit == 0) {
        result = min(result, a); result = min(result, b); result = min(result, c);
        result = min(result, d); result = min(result, e); result = min(result, f);
    } else if (digit == 1) {
        result = min(result, b); result = min(result, c);
    } else if (digit == 2) {
        result = min(result, a); result = min(result, b); result = min(result, d);
        result = min(result, e); result = min(result, g);
    } else if (digit == 3) {
        result = min(result, a); result = min(result, b); result = min(result, c);
        result = min(result, d); result = min(result, g);
    } else if (digit == 4) {
        result = min(result, b); result = min(result, c); result = min(result, f);
        result = min(result, g);
    } else if (digit == 5) {
        result = min(result, a); result = min(result, c); result = min(result, d);
        result = min(result, f); result = min(result, g);
    } else if (digit == 6) {
        result = min(result, a); result = min(result, c); result = min(result, d);
        result = min(result, e); result = min(result, f); result = min(result, g);
    } else if (digit == 7) {
        result = min(result, a); result = min(result, b); result = min(result, c);
    } else if (digit == 8) {
        result = min(result, a); result = min(result, b); result = min(result, c);
        result = min(result, d); result = min(result, e); result = min(result, f);
        result = min(result, g);
    } else if (digit == 9) {
        result = min(result, a); result = min(result, b); result = min(result, c);
        result = min(result, d); result = min(result, f); result = min(result, g);
    }

    // Convert SDF to soft anti-aliased mask
    return 1.0 - smoothstep(0.0, size * 0.03, result);
}

// Render a frequency value as Hz or kHz
// Returns color contribution at position p
// freq: frequency in Hz, pos: label position, energy: band energy for fading
// Enhanced version with glow, larger size, and energy-reactive effects
vec3 renderFreqLabel(vec2 p, float freq, vec2 pos, float energy, vec3 color, float totalEnergy) {
    if (u_freq_labels < 0.5) return vec3(0.0);

    // Energy gating - fade out with low energy
    float alpha = smoothstep(0.05, 0.20, energy) * smoothstep(0.03, 0.15, totalEnergy);
    if (alpha < 0.01) return vec3(0.0);

    // === LARGER SIZE with energy-reactive scaling ===
    float baseSize = 0.038;  // Increased from 0.025 for better readability
    float energyPulse = 1.0 + energy * 0.15;  // Grow with energy
    float beatPulse = 1.0 + u_kick * 0.08;    // Subtle kick pulse
    float scale = energyPulse * beatPulse;
    float digitSize = baseSize * scale;

    // Transform to label space
    vec2 lp = p - pos;

    // Gentle floating motion (organic, not jittery)
    float floatX = sin(u_time * 0.4 + pos.y * 8.0) * 0.004;
    float floatY = cos(u_time * 0.3 + pos.x * 8.0) * 0.003;
    lp.x += floatX;
    lp.y += floatY;

    // Format: Always show raw Hz with "Hz" suffix (e.g., 304Hz, 15600Hz)
    int displayValue = int(freq);

    // === RENDER DIGITS ===
    float coreResult = 0.0;   // Sharp inner core
    float glowResult = 0.0;   // Soft outer glow
    float xOffset = -digitSize * 1.2;

    // Extract digits (up to 5 digits: 0-99999 Hz)
    int tenThousands = displayValue / 10000;
    int thousands = (displayValue / 1000) - tenThousands * 10;
    int hundreds = (displayValue / 100) - tenThousands * 100 - thousands * 10;
    int tens = (displayValue / 10) - (displayValue / 100) * 10;
    int ones = displayValue - (displayValue / 10) * 10;

    // Render digits with leading zero suppression
    if (tenThousands > 0) {
        coreResult += sdfDigit(lp - vec2(xOffset, 0.0), tenThousands, digitSize);
        xOffset += digitSize * 0.65;
    }
    if (tenThousands > 0 || thousands > 0) {
        coreResult += sdfDigit(lp - vec2(xOffset, 0.0), thousands, digitSize);
        xOffset += digitSize * 0.65;
    }
    if (tenThousands > 0 || thousands > 0 || hundreds > 0) {
        coreResult += sdfDigit(lp - vec2(xOffset, 0.0), hundreds, digitSize);
        xOffset += digitSize * 0.65;
    }
    if (tenThousands > 0 || thousands > 0 || hundreds > 0 || tens > 0) {
        coreResult += sdfDigit(lp - vec2(xOffset, 0.0), tens, digitSize);
        xOffset += digitSize * 0.65;
    }
    coreResult += sdfDigit(lp - vec2(xOffset, 0.0), ones, digitSize);
    xOffset += digitSize * 0.55;

    // === "Hz" SUFFIX ===
    // 'H' glyph: two vertical stems + horizontal bar
    vec2 hp = lp - vec2(xOffset, 0.0);
    float hLeft = sdfSegV(hp - vec2(-digitSize * 0.12, 0.0), digitSize * 0.08, digitSize * 0.55);
    float hRight = sdfSegV(hp - vec2(digitSize * 0.12, 0.0), digitSize * 0.08, digitSize * 0.55);
    float hBar = sdfSegH(hp, digitSize * 0.24, digitSize * 0.08);
    float hGlyph = min(hLeft, min(hRight, hBar));
    coreResult += 1.0 - smoothstep(0.0, digitSize * 0.025, hGlyph);
    xOffset += digitSize * 0.45;

    // 'z' glyph: top bar + diagonal + bottom bar
    vec2 zp = lp - vec2(xOffset, 0.0);
    float zTop = sdfSegH(zp - vec2(0.0, digitSize * 0.22), digitSize * 0.22, digitSize * 0.07);
    float zBot = sdfSegH(zp - vec2(0.0, -digitSize * 0.22), digitSize * 0.22, digitSize * 0.07);
    // Diagonal: line from top-right to bottom-left
    vec2 diagStart = vec2(digitSize * 0.10, digitSize * 0.18);
    vec2 diagEnd = vec2(-digitSize * 0.10, -digitSize * 0.18);
    vec2 diagDir = normalize(diagEnd - diagStart);
    float diagLen = length(diagEnd - diagStart);
    float diagT = clamp(dot(zp - diagStart, diagDir), 0.0, diagLen);
    vec2 diagClosest = diagStart + diagDir * diagT;
    float zDiag = length(zp - diagClosest) - digitSize * 0.04;
    float zGlyph = min(zTop, min(zBot, zDiag));
    coreResult += 1.0 - smoothstep(0.0, digitSize * 0.025, zGlyph);

    // === GLOW EFFECT ===
    // Render a softer, larger version for the glow (digits only, no Hz suffix)
    float glowSize = digitSize * 1.4;
    float glowXOffset = -glowSize * 1.2;

    // Glow for digits (matching core digit rendering)
    if (tenThousands > 0) {
        glowResult += sdfDigit(lp - vec2(glowXOffset, 0.0), tenThousands, glowSize) * 0.5;
        glowXOffset += glowSize * 0.65;
    }
    if (tenThousands > 0 || thousands > 0) {
        glowResult += sdfDigit(lp - vec2(glowXOffset, 0.0), thousands, glowSize) * 0.5;
        glowXOffset += glowSize * 0.65;
    }
    if (tenThousands > 0 || thousands > 0 || hundreds > 0) {
        glowResult += sdfDigit(lp - vec2(glowXOffset, 0.0), hundreds, glowSize) * 0.5;
        glowXOffset += glowSize * 0.65;
    }
    if (tenThousands > 0 || thousands > 0 || hundreds > 0 || tens > 0) {
        glowResult += sdfDigit(lp - vec2(glowXOffset, 0.0), tens, glowSize) * 0.5;
        glowXOffset += glowSize * 0.65;
    }
    glowResult += sdfDigit(lp - vec2(glowXOffset, 0.0), ones, glowSize) * 0.5;

    // === COLOR: Vibrant with energy-reactive saturation and brightness ===
    float sat = mix(0.5, 1.0, energy);
    float brightness = mix(0.7, 1.3, energy);
    vec3 coreColor = color * brightness * sat + vec3(0.15);  // Slight white lift
    vec3 glowColor = color * 0.6;  // Softer glow color

    // === COMPOSITE: Core + Glow ===
    vec3 result = vec3(0.0);
    result += glowColor * glowResult * alpha * 0.4;   // Soft outer glow
    result += coreColor * coreResult * alpha * 1.0;   // Sharp bright core

    // Energy-reactive bloom boost
    result *= (1.0 + energy * 0.3);

    return result;
}

// ============ OUTER BACKDROP PREMIUM ============
// Cathedral backdrop: frames torus, echoes 24-fold symmetry, never competes
//
// POLISH FEATURES:
//   - Two-stage pulse (fast attack, slow bar-length decay)
//   - Halo thickness breathes (not brightness blink)
//   - Inner bloom falloff around halo (lens glow feel)
//   - Secondary rim only in TorusKnot
//   - Front-weighted halo (depth cue)
//   - Gate emphasis with smooth confidence-based fade
//
// CRITICAL: outside brightness ≤ 0.35 * scaffold brightness
// ============================================

vec3 outerBackdropPremium(vec2 uv, vec3 col, float fieldRadius) {
    // === ASPECT-CORRECTED COORDINATES FOR TRUE CIRCLES ===
    // Create metric where 1 unit in x == 1 unit in y
    float aspect = u_resolution.x / u_resolution.y;
    vec2 p = uv;
    p.x *= aspect;
    float r = length(p);  // True circular radius

    // Theta uses original uv for correct angular math (seams/gates unchanged)
    float theta = atan(uv.y, uv.x);

    // Early exit for center region (inside the field)
    // Adjust fieldRadius for aspect correction
    if (r < fieldRadius * 0.9 * aspect) return col;

    // === APERTURE PRESETS ===
    // 0=tight, 1=medium, 2=wide, 3=widest
    float apertureRadii[4] = float[4](0.5, 0.6, 0.75, 0.9);
    float apertureRadius = apertureRadii[clamp(u_outer_aperture_q, 0, 3)];

    // Gate center angle for 24-fold alignment
    float gateAngle = float(u_vortex_gate_center) * TAU / 24.0;
    float alignedTheta = theta - gateAngle;
    float foldAngle = TAU / 24.0;
    float foldedTheta = mod(alignedTheta + foldAngle * 0.5, foldAngle) - foldAngle * 0.5;

    // === BRIGHTNESS CAP ===
    // Outside must never exceed 35% of what scaffold would be
    float brightnessCapMultiplier = 0.35;

    // === COMBINED PULSE ===
    // Fast pulse for immediate response, slow pulse for sustained bloom
    float combinedPulse = u_outer_pulse_env * 0.6 + u_outer_pulse_slow * 0.4;

    // === DEPTH CUE: Front-weighted dimming ===
    // Dim the "back half" using vertical screen-space gradient
    // This makes outside feel like it sits behind the torus
    float depthGradient = uv.y * 0.3 + 0.85;  // 0.55 at bottom, 1.15 at top
    depthGradient = clamp(depthGradient, 0.6, 1.0);

    vec3 outerContrib = vec3(0.0);

    // ================================================================
    // STYLE A: APERTURE + RIM HALO (default, most premium)
    // ================================================================
    if (u_outer_style_id == OUTER_STYLE_APERTURE_HALO) {
        // === 1. APERTURE VIGNETTE ===
        // Strong, smooth radial falloff (like camera iris)
        // Tighter in Vortex mode for "black hole" feel
        float vignetteStart = apertureRadius;
        float vignetteEnd = vignetteStart + (u_outer_is_vortex == 1 ? 0.3 : 0.4);
        float vignette = smoothstep(vignetteStart, vignetteEnd, r);

        // Aperture darkening (stronger in Vortex)
        float apertureDarkMax = (u_outer_is_vortex == 1) ? 0.9 : 0.8;
        float apertureDark = vignette * apertureDarkMax;

        // === 2. PRIMARY RIM HALO RING ===
        // POLISH: Thickness breathes with pulse, not brightness
        float haloRadius = apertureRadius + 0.05;
        float haloWidthBase = 0.006;
        float haloWidthPulse = 0.012;  // Expands on beat
        float haloWidth = haloWidthBase + combinedPulse * haloWidthPulse;
        float haloDist = abs(r - haloRadius);
        float haloRing = exp(-haloDist * haloDist / (haloWidth * haloWidth));

        // Halo brightness - mostly stable, slight pulse
        float haloBrightness = 0.18 + u_outer_pulse_slow * 0.08;
        haloBrightness *= brightnessCapMultiplier * depthGradient;

        // Halo color - warm neutral
        vec3 haloColor = vec3(0.9, 0.85, 0.8);

        // === 3. INNER BLOOM FALLOFF (lens glow feel) ===
        // Subtle bloom around halo ring that fades inward
        float bloomRadius = haloRadius - 0.03;
        float bloomDist = max(0.0, r - bloomRadius);
        float innerBloom = exp(-bloomDist * bloomDist * 40.0) * 0.15;
        innerBloom *= combinedPulse * brightnessCapMultiplier * depthGradient;

        // === 4. SECONDARY RIM (TorusKnot only) ===
        // Adds depth when not in Vortex
        float secondaryContrib = 0.0;
        if (u_outer_is_torusknot == 1 && u_outer_is_vortex == 0) {
            float secondaryRadius = apertureRadius + 0.12;
            float secondaryWidth = 0.004;
            float secondaryDist = abs(r - secondaryRadius);
            float secondaryRing = exp(-secondaryDist * secondaryDist / (secondaryWidth * secondaryWidth));
            secondaryContrib = secondaryRing * 0.08 * brightnessCapMultiplier * depthGradient;
        }

        // === 5. 24-FOLD SEAM TICKS ===
        // Tiny tick marks at seam boundaries on the halo ring
        float seamDist = abs(foldedTheta);
        float tickWidth = 0.018;
        float tick = exp(-seamDist * seamDist / (tickWidth * tickWidth));

        // Ticks only visible on halo ring and very subtle
        float tickIntensity = tick * haloRing * 0.3;

        // === 6. GATE EMPHASIS (smooth fade with confidence) ===
        // GATE-DEPENDENT: Fades with u_gate_ok (master kill switch) + u_outer_gate_fade
        float gateEmphasis = 0.0;
        float gateRelative = mod(alignedTheta / foldAngle + 12.0, 24.0) - 12.0;
        if (abs(gateRelative) < 1.5 && u_gate_ok > 0.01) {
            float emphasisBase = (1.5 - abs(gateRelative)) / 1.5;
            gateEmphasis = emphasisBase * 0.25 * u_outer_gate_fade * u_gate_ok;  // Dual fade
        }

        // === COMBINE STYLE A ===
        vec3 haloContrib = haloColor * haloRing * haloBrightness;
        haloContrib += haloColor * innerBloom;  // Subtle inner bloom
        haloContrib += vec3(1.0) * tickIntensity * brightnessCapMultiplier * 0.4;
        haloContrib += haloColor * gateEmphasis * haloRing * brightnessCapMultiplier;
        haloContrib += haloColor * secondaryContrib;  // Secondary rim (if TorusKnot)

        // Apply aperture darkening to base color
        col *= (1.0 - apertureDark);

        outerContrib = haloContrib;
    }

    // ================================================================
    // STYLE B: SEAM-ARC CONSTELLATION (sparse, celestial)
    // TorusKnot "reward" state - never in Vortex
    // ================================================================
    else if (u_outer_style_id == OUTER_STYLE_SEAM_ARCS) {
        // 3 radii for arcs
        float arcRadii[3] = float[3](
            apertureRadius + 0.08,
            apertureRadius + 0.18,
            apertureRadius + 0.28
        );

        vec3 arcColor = vec3(0.7, 0.75, 0.9);  // Cool blue-ish

        for (int i = 0; i < 3; i++) {
            float arcR = arcRadii[i];
            float arcWidth = 0.004;

            // Distance to arc radius
            float radDist = abs(r - arcR);
            float onArc = exp(-radDist * radDist / (arcWidth * arcWidth));

            // Only every 3rd seam (8 arcs total for 24-fold)
            float seamIdx = floor(mod(alignedTheta / foldAngle + 12.0, 24.0));
            float arcMask = (mod(seamIdx, 3.0) < 0.5) ? 1.0 : 0.0;

            // POLISH: Arc endpoints with ease-out caps (crisp, machined)
            float arcLength = 0.06;
            float endDist = abs(foldedTheta);
            float segmentMask = 1.0 - smoothstep(arcLength * 0.7, arcLength, endDist);
            segmentMask = segmentMask * segmentMask;  // Ease-out curve

            // POLISH: Ignite outward - radial intensity pulse from inner to outer
            float radialProgress = float(i) / 2.0;  // 0, 0.5, 1
            float igniteDelay = radialProgress * 0.15;  // Outer arcs lag
            float ignitePulse = max(0.0, u_outer_pulse_env - igniteDelay);
            float arcPulse = 0.7 + ignitePulse * 0.8;

            float arcIntensity = onArc * arcMask * segmentMask * arcPulse;
            arcIntensity *= brightnessCapMultiplier * 0.5 * depthGradient;

            outerContrib += arcColor * arcIntensity;
        }

        // Light vignette (less aggressive than Style A)
        float vignetteStart = apertureRadius + 0.35;
        float vignette = smoothstep(vignetteStart, vignetteStart + 0.3, r);
        col *= (1.0 - vignette * 0.5);
    }

    // ================================================================
    // STYLE C: FACETED SHELL (single polygon ring)
    // ================================================================
    else if (u_outer_style_id == OUTER_STYLE_FACETED_SHELL) {
        // Single 24-sided polygon ring
        float shellRadius = apertureRadius + 0.1;
        float shellWidth = 0.005;

        // POLISH: 24-sided polygon with vertices exactly at k*(2π/24)
        // Align facets to 24-fold grid precisely
        float facetAngle = TAU / 24.0;
        // Offset so vertices are at gate positions, not edges
        float alignedFacetTheta = theta - gateAngle + facetAngle * 0.5;
        float facetDist = cos(mod(alignedFacetTheta, facetAngle) - facetAngle * 0.5);
        float polyR = shellRadius / max(facetDist, 0.1);
        float polyDist = abs(r - polyR);

        // POLISH: Only silhouette edges, no interior lines
        float shellRing = exp(-polyDist * polyDist / (shellWidth * shellWidth));

        // Subtle edge glow on pulse
        float edgeGlow = shellRing * (0.15 + combinedPulse * 0.2);
        edgeGlow *= brightnessCapMultiplier * depthGradient;

        vec3 shellColor = vec3(0.8, 0.7, 0.9);  // Soft purple
        outerContrib = shellColor * edgeGlow;

        // Medium vignette
        float vignetteStart = apertureRadius + 0.25;
        float vignette = smoothstep(vignetteStart, vignetteStart + 0.35, r);
        col *= (1.0 - vignette * 0.6);
    }

    // === FINAL COMPOSITE ===
    // Additive blend for halo/arcs (they glow)
    // Cap is already applied per-component, no stacking issue
    col += outerContrib;

    return col;
}

// ============ SPECTRAL RINGS ============
// Concentric rings around the field, each driven by a frequency band
// GEOMETRIC SYMMETRY: Ring radii follow logarithmic spacing (octaves)
// Position = log2(frequency), so equal visual distance = equal musical interval

vec3 spectralRings(vec2 uv, vec3 col, float fieldRadius) {
    if (u_spectral_rings < 0.5) return col;

    // Total energy check - don't show rings with no audio
    float totalEnergy = u_bass + u_mid + u_high;
    if (totalEnergy < 0.05) return col;  // Silent threshold

    // Distance from center in screen space
    float dist = length(uv);

    // === GEOMETRIC/LOGARITHMIC RING SPACING ===
    // 20Hz to 20kHz = 10 octaves mapped to visual radius
    // Ring position derived from log2(frequency) - true geometric symmetry
    float baseRadius = fieldRadius * 1.1;  // Start just outside the field
    float maxRingSpan = 0.5;               // Total radial span for all rings
    float minFreq = 20.0;                  // Lowest frequency (20 Hz)
    float totalOctaves = 10.0;             // 20Hz to 20kHz = 10 octaves

    // Band energies (synthesized from bass/mid/high)
    float bands[6];
    bands[0] = u_bass * 1.2;                           // Sub (boosted bass)
    bands[1] = u_bass * 0.8 + u_kick * 0.5;           // Bass
    bands[2] = u_bass * 0.4 + u_mid * 0.6;            // Low-mid
    bands[3] = u_mid;                                  // Mid
    bands[4] = u_mid * 0.4 + u_high * 0.6;            // High-mid
    bands[5] = u_high;                                 // Air/highs

    // Ring colors following spectral order (warm to cool)
    vec3 ringColors[6];
    ringColors[0] = vec3(0.8, 0.2, 0.1);   // Sub: deep red
    ringColors[1] = vec3(1.0, 0.4, 0.1);   // Bass: orange
    ringColors[2] = vec3(1.0, 0.8, 0.2);   // Low-mid: yellow
    ringColors[3] = vec3(0.3, 1.0, 0.4);   // Mid: green
    ringColors[4] = vec3(0.2, 0.6, 1.0);   // High-mid: blue
    ringColors[5] = vec3(0.7, 0.3, 1.0);   // Air: purple

    vec3 ringGlow = vec3(0.0);
    vec3 freqLabels = vec3(0.0);

    // Global fade based on total energy
    float globalFade = smoothstep(0.05, 0.3, totalEnergy);

    // Label placement angle - slow drift for organic feel
    float labelAngle = u_time * 0.1 + PI * 0.25;  // Drift slowly, start at 45°

    for (int i = 0; i < 6; i++) {
        // === LOGARITHMIC RING POSITION ===
        // Ring radius derived from log2(frequency centroid)
        // This ensures equal visual spacing = equal musical intervals (octaves)
        float freqHz = u_band_freq[i];
        float freqOctave = log2(freqHz / minFreq);  // Position in octaves (0-10)
        float ringRadius = baseRadius + (freqOctave / totalOctaves) * maxRingSpan;

        float bandEnergy = bands[i];

        // Skip rings with very low energy
        if (bandEnergy < 0.02) continue;

        // Ring thickness varies with energy (scales from 0)
        float thickness = bandEnergy * 0.04;

        // Soft ring using smoothstep
        float ringDist = abs(dist - ringRadius);
        float ring = smoothstep(thickness, thickness * 0.2, ringDist);

        // === GEOMETRIC MOTION: derived from octave position ===
        // Rotation speed proportional to log position (higher freq = faster)
        float octaveNorm = freqOctave / totalOctaves;  // 0-1 normalized
        float rotSpeed = 0.05 + octaveNorm * 0.25;
        float angle = atan(uv.y, uv.x) + u_time * rotSpeed;

        // Wave jitter also follows geometric proportion
        float jitter = octaveNorm * 0.15;
        float wave = sin(angle * 3.0 + u_time * 2.0) * jitter * bandEnergy;
        ring *= (1.0 + wave);

        // Brightness FULLY scales with energy (no base brightness when silent)
        float brightness = bandEnergy * 1.2;

        // Add glow
        vec3 bandColor = ringColors[i] * brightness * ring;
        ringGlow += bandColor;

        // === FREQUENCY LABEL: Spread around ring at different angles ===
        // Each band gets its own angle position (60° apart, with slow drift)
        float bandAngleBase = float(i) * (TAU / 6.0);  // 60° spacing
        float bandAngleDrift = u_time * 0.08;  // Slow unified rotation
        float thisLabelAngle = bandAngleBase + bandAngleDrift + PI * 0.1;  // Offset from top

        // Position label slightly outside the ring
        float labelRadius = ringRadius + 0.025 + bandEnergy * 0.015;
        vec2 labelPos = vec2(cos(thisLabelAngle), sin(thisLabelAngle)) * labelRadius;

        // Render frequency label at this position
        freqLabels += renderFreqLabel(uv, u_band_freq[i], labelPos, bandEnergy, ringColors[i], totalEnergy);
    }

    // Blend rings and labels additively with scene, scaled by global energy
    return col + (ringGlow * 0.7 + freqLabels) * globalFade;
}

// ============ AAA SPECTRAL RINGS ============
// 3D energy toroids with cinematic lighting, flow lines, and musical motion
// ANALYTIC: No raymarching - uses analytic torus cross-section for performance
// REG-SAFE: Pure visualization layer, does NOT modify chroma → geometry

// Analytic 3D torus cross-section mask
// Returns soft mask for a torus at given world position
// ringR = major radius (center of tube), tubeR = minor radius (tube thickness)
float torusCrossSection(float r, float y, float ringR, float tubeR) {
    // Distance from torus center circle in XZ plane
    float dCircle = abs(r - ringR);
    // 2D distance to tube surface (elliptical for depth illusion)
    float tubeY = tubeR * 1.2;  // Slightly taller for 3D look
    float d = length(vec2(dCircle / tubeR, y / tubeY));
    // Soft falloff with bright core
    float core = exp(-d * d * 4.0);      // Sharp bright center
    float glow = exp(-d * d * 1.5);      // Softer outer glow
    return core * 0.7 + glow * 0.3;
}

// Flow line pattern - sinusoidal energy stripes moving along ring
float flowLines(float angle, float time, float speed, float freq, float energy) {
    // Multiple harmonics for richer look
    float flow1 = sin(angle * freq + time * speed) * 0.5 + 0.5;
    float flow2 = sin(angle * freq * 1.5 - time * speed * 0.7) * 0.3 + 0.5;
    // Modulate by energy (no flow when silent)
    return mix(0.3, 1.0, flow1 * 0.6 + flow2 * 0.4) * energy;
}

// Fresnel rim glow for 3D feel
float fresnelRim(float r, float ringR, float tubeR) {
    // Pseudo-normal based on position within tube
    float normPos = (r - ringR) / tubeR;  // -1 to 1 across tube
    float rim = 1.0 - abs(normPos);       // Brightest at edges
    return pow(max(0.0, 1.0 - rim), 2.0) * 0.8;  // Soft falloff
}

// High-frequency specular sparkle
float specularSparkle(float angle, float time, float energy, float stability) {
    // Multi-frequency noise for organic sparkle
    float n1 = sin(angle * 47.0 + time * 8.0) * sin(angle * 23.0 - time * 5.0);
    float n2 = sin(angle * 31.0 + time * 12.0) * sin(angle * 17.0 - time * 7.0);
    float sparkle = max(0.0, n1 * 0.5 + n2 * 0.5);
    // Gate by energy and boost with stability (stable = more sparkle)
    return pow(sparkle, 3.0) * energy * (0.3 + stability * 0.7);
}

vec3 spectralRingsAAA(vec2 uv, vec3 col, float fieldRadius) {
    if (u_spectral_rings < 0.5) return col;

    // Check for AAA mode, fall back to classic if disabled
    bool aaaMode = u_ring_style_aaa > 0.5;

    // Total energy check - don't show rings with no audio
    float totalEnergy = u_bass_ema + u_mid_ema + u_high_ema;
    if (totalEnergy < 0.02) return col;  // Silent threshold

    // Screen-space distance and angle
    float r = length(uv);
    float angle = atan(uv.y, uv.x);

    // Simulate Y coordinate for 3D depth (view-dependent)
    // Rings are in XZ plane, we're looking slightly down
    float viewTilt = 0.15;  // Slight camera tilt for depth
    float y = r * viewTilt * sin(angle * 2.0 + u_time * 0.3);  // Subtle wobble

    // === BAND CONFIGURATION ===
    // Three main bands: Bass (warm), Mid (neutral), High (cool)
    // Each has distinct motion characteristics

    struct BandConfig {
        float energy;        // Current energy (fast EMA)
        float emaEnergy;     // Slow EMA for smooth motion
        float peakHz;        // Peak frequency for display
        float ringR;         // Major radius
        float tubeR;         // Tube thickness (energy-driven)
        float rotSpeed;      // Rotation speed
        float flowSpeed;     // Flow line animation speed
        float flowFreq;      // Flow line frequency
        vec3 color;          // Base color
    };

    // Base radius just outside the harmonic field
    float baseR = fieldRadius * 1.15;
    float ringSpacing = 0.12;

    // === BASS RING: Slow, powerful, warm ===
    float bassR = baseR;
    float bassTube = mix(0.015, 0.045, u_bass_ema);  // Thick when loud
    float bassRot = 0.08;  // Slow rotation
    float bassFlowSpeed = 1.5;  // Slow, powerful flow
    float bassFlowFreq = 4.0;  // Low frequency waves
    vec3 bassColor = vec3(1.0, 0.35, 0.15);  // Warm orange-red

    // === MID RING: Medium, balanced, neutral ===
    float midR = baseR + ringSpacing;
    float midTube = mix(0.012, 0.035, u_mid_ema);
    float midRot = 0.15;  // Medium rotation
    float midFlowSpeed = 2.5;  // Moderate flow
    float midFlowFreq = 6.0;  // Medium frequency
    vec3 midColor = vec3(0.4, 0.9, 0.5);  // Neutral green

    // === HIGH RING: Fast, delicate, cool ===
    float highR = baseR + ringSpacing * 2.0;
    float highTube = mix(0.008, 0.025, u_high_ema);  // Thinner
    float highRot = 0.25;  // Faster rotation
    float highFlowSpeed = 4.0;  // Quick shimmer
    float highFlowFreq = 10.0;  // High frequency detail
    vec3 highColor = vec3(0.3, 0.6, 1.0);  // Cool blue

    // === RENDER EACH BAND ===
    vec3 ringResult = vec3(0.0);
    float bloomMask = 0.0;

    // Global fade based on total energy
    float globalFade = smoothstep(0.02, 0.2, totalEnergy);

    // Tonal confidence affects saturation
    float confidence = u_stability;
    float satBoost = 0.7 + confidence * 0.3;

    // --- BASS RING ---
    if (u_bass_ema > 0.01) {
        float rotAngle = angle + u_time * bassRot;

        // Torus cross-section (3D tube look)
        float bassMask = torusCrossSection(r, y, bassR, bassTube);

        // Flow lines (energy moving along ring)
        float bassFlow = 1.0;
        if (u_ring_flow_lines > 0.5) {
            bassFlow = flowLines(rotAngle, u_time, bassFlowSpeed, bassFlowFreq, u_bass_ema);
        }

        // Kick pulse: expand + brighten on kick
        float kickPulse = 1.0 + u_kick * 0.4;
        bassMask *= kickPulse;

        // Rim glow for 3D depth
        float bassRim = fresnelRim(r, bassR, bassTube);

        // Specular sparkle
        float bassSparkle = 0.0;
        if (u_ring_sparkle > 0.5) {
            bassSparkle = specularSparkle(rotAngle, u_time, u_bass_ema, confidence) * 0.5;
        }

        // Composite bass ring
        vec3 bassResult = bassColor * satBoost;
        bassResult *= bassMask * bassFlow * u_bass_ema;
        bassResult += bassColor * bassRim * u_bass_ema * 0.3;
        bassResult += vec3(1.0, 0.9, 0.7) * bassSparkle;

        ringResult += bassResult;
        bloomMask += bassMask * u_bass_ema * bassFlow;
    }

    // --- MID RING ---
    if (u_mid_ema > 0.01) {
        float rotAngle = angle + u_time * midRot;

        float midMask = torusCrossSection(r, y, midR, midTube);

        float midFlow = 1.0;
        if (u_ring_flow_lines > 0.5) {
            midFlow = flowLines(rotAngle, u_time, midFlowSpeed, midFlowFreq, u_mid_ema);
        }

        // Subtle shimmer on mid
        float midShimmer = 1.0 + sin(rotAngle * 8.0 + u_time * 3.0) * 0.1 * u_mid_ema;

        float midRim = fresnelRim(r, midR, midTube);

        float midSparkle = 0.0;
        if (u_ring_sparkle > 0.5) {
            midSparkle = specularSparkle(rotAngle, u_time, u_mid_ema, confidence) * 0.4;
        }

        vec3 midResult = midColor * satBoost;
        midResult *= midMask * midFlow * midShimmer * u_mid_ema;
        midResult += midColor * midRim * u_mid_ema * 0.25;
        midResult += vec3(0.9, 1.0, 0.9) * midSparkle;

        ringResult += midResult;
        bloomMask += midMask * u_mid_ema * midFlow * 0.8;
    }

    // --- HIGH RING ---
    if (u_high_ema > 0.01) {
        float rotAngle = angle + u_time * highRot;

        float highMask = torusCrossSection(r, y, highR, highTube);

        float highFlow = 1.0;
        if (u_ring_flow_lines > 0.5) {
            highFlow = flowLines(rotAngle, u_time, highFlowSpeed, highFlowFreq, u_high_ema);
            // Extra micro-flicker for highs (gated by energy to avoid noise)
            float flicker = sin(rotAngle * 20.0 + u_time * 15.0) * 0.15 + 0.85;
            highFlow *= mix(1.0, flicker, u_high_ema);
        }

        float highRim = fresnelRim(r, highR, highTube);

        float highSparkle = 0.0;
        if (u_ring_sparkle > 0.5) {
            highSparkle = specularSparkle(rotAngle, u_time, u_high_ema, confidence) * 0.6;
        }

        vec3 highResult = highColor * satBoost;
        highResult *= highMask * highFlow * u_high_ema;
        highResult += highColor * highRim * u_high_ema * 0.2;
        highResult += vec3(0.8, 0.9, 1.0) * highSparkle;

        ringResult += highResult;
        bloomMask += highMask * u_high_ema * highFlow * 0.6;
    }

    // === DEPTH FOG ===
    // Rings further from center are slightly hazier (atmospheric perspective)
    if (u_ring_fog > 0.5) {
        float fogDist = max(0.0, r - fieldRadius) / 0.5;
        float fog = 1.0 - fogDist * 0.15;
        ringResult *= fog;
    }

    // === BLOOM EMISSION ===
    // Output bloom mask for post-processing (brighter cores bloom more)
    if (u_ring_bloom_mask > 0.5) {
        // Boost emission for bloom pass
        ringResult *= 1.0 + bloomMask * 0.3;
    }

    // === ANAMORPHIC STREAK ON KICK (ProjectM flare) ===
    // Brief horizontal streak when kick fires
    if (u_kick > 0.3) {
        // Only on bass ring area
        float streakDist = abs(r - bassR);
        if (streakDist < bassTube * 2.0) {
            // Horizontal streak (screen-space)
            float streak = exp(-abs(uv.y) * 30.0);  // Thin horizontal line
            streak *= exp(-streakDist / bassTube);   // Fade with distance from ring
            streak *= u_kick * 0.5;                  // Intensity from kick
            ringResult += bassColor * streak * u_bass_ema;
        }
    }

    // Final composite with global fade
    return col + ringResult * globalFade * 1.2;
}

// ============ RING DIVE EFFECTS ============
// Cinematic inside transition: portal rings, warp streaks, field lines
// GATED: All effects skip entirely when u_ring_dive_active == 0

// Portal rings: 12-fold chroma-driven rings - a VAST harmonic chamber
// With obvious 12-fold radial symmetry (spokes + rings)
vec3 portalRings(vec2 uv, vec3 col, float intensity) {
    if (intensity < 0.01) return col;

    float dist = length(uv);
    float angle = atan(uv.y, uv.x);

    // Normalize angle to [0, TAU)
    float normAngle = mod(angle + PI, TAU);

    // 12-fold portal: VAST room, see everything at once
    float baseRadius = 0.06;      // Start very close to center
    float ringSpacing = 0.065;    // Wide spread - all 12 visible
    float roomScale = 1.0 + intensity * 0.6;  // Room expands as you enter

    // === 12-FOLD RADIAL SPOKES ===
    // These make the 12-fold symmetry OBVIOUS
    float spokeAngle = mod(normAngle, TAU / 12.0);  // Angle within each 30° sector
    float spokeCenter = TAU / 24.0;  // Center of each sector (15°)
    float spokeDist = abs(spokeAngle - spokeCenter);

    // Spoke brightness - bright lines radiating from center
    float spokeWidth = 0.04 + intensity * 0.02;
    float spoke = smoothstep(spokeWidth, spokeWidth * 0.3, spokeDist);
    spoke *= smoothstep(0.0, 0.15, dist);  // Fade in from center
    spoke *= smoothstep(1.2, 0.4, dist);   // Fade out at edge

    // Which pitch class is this spoke? (0-11)
    int spokePC = int(floor(normAngle / (TAU / 12.0)));
    float spokeChroma = getChroma(spokePC);
    vec3 spokeColor = hue2rgb(float(spokePC) / 12.0);

    // Spokes glow with their pitch class energy
    col += spoke * spokeColor * (0.2 + spokeChroma * 0.5) * intensity;

    // Dominant pitch spoke is extra bright
    if (spokePC == u_dom_pc) {
        col += spoke * spokeColor * intensity * 0.4;
    }

    // === 12 CONCENTRIC RINGS ===
    for (int i = 0; i < 12; i++) {
        float chroma_val = getChroma(i);

        // Ring radius - 12 rings spreading outward
        float ringRadius = (baseRadius + float(i) * ringSpacing) * roomScale;

        // Ring color from pitch class
        vec3 ringColor = hue2rgb(float(i) / 12.0);

        // Distance from this ring
        float ringDist = abs(dist - ringRadius);

        // Thickness scales with chroma energy
        float thickness = 0.006 + chroma_val * 0.012;
        thickness *= (1.0 + intensity * 0.4);

        // Door pitch class (dominant) is thicker
        bool isDoor = (i == u_dom_pc);
        if (isDoor) thickness *= 2.0;

        // Ring glow
        float ring = smoothstep(thickness, thickness * 0.15, ringDist);

        // 12-fold angular brightness variation on each ring
        // Each ring is brightest at its own pitch class spoke
        float ringAngle = float(i) * TAU / 12.0;
        float angularBrightness = 0.6 + 0.4 * cos(normAngle - ringAngle);
        ring *= angularBrightness;

        // Pulse with chroma
        float pulse = 1.0 + chroma_val * 0.25 * sin(u_time * 2.5 + float(i) * 0.8);
        ring *= pulse;

        // Brightness
        float brightness = (0.25 + chroma_val * 0.75) * intensity;
        if (isDoor) brightness *= 1.6;

        col += ring * ringColor * brightness;

        // Door portal effect
        if (isDoor && intensity > 0.35) {
            float doorGlow = smoothstep(ringRadius, ringRadius * 0.3, dist);
            doorGlow *= (intensity - 0.35) * 1.8;
            col += doorGlow * ringColor * 0.35;
        }
    }

    // === OUTER BOUNDARY ===
    float outerRadius = (baseRadius + 12.0 * ringSpacing) * roomScale + 0.08;
    float outerRing = smoothstep(0.025, 0.004, abs(dist - outerRadius));
    col += outerRing * chromaColor() * intensity * 0.2;

    // === CENTER GLOW ===
    // Hint of the harmonic center
    float centerGlow = smoothstep(0.12, 0.0, dist) * intensity * 0.3;
    col += centerGlow * chromaColor();

    return col;
}

// Warp streaks: radial streaks aligned to dominant pitch
vec3 warpStreaks(vec2 uv, vec3 col, float intensity, int maxSamples) {
    if (intensity < 0.01) return col;

    vec2 dir = normalize(uv + vec2(0.001));  // Avoid /0
    float angle = atan(dir.y, dir.x);
    float dom_angle = float(u_dom_pc) * TAU / 12.0;

    vec3 streak = vec3(0.0);
    for (int i = 0; i < maxSamples; i++) {
        float t = float(i) / float(maxSamples);
        float streak_angle = dom_angle + t * TAU;
        float d = abs(mod(angle - streak_angle + PI, TAU / 12.0) - TAU / 24.0);
        streak += smoothstep(0.04, 0.0, d) * (1.0 - t * 0.6);
    }

    return col + streak * intensity * u_ring_dive_warp_strength * chromaColor();
}

// Field lines: harmonic arc lines during guided flight
// Uses u_field_rotation (audio-reactive) instead of u_rotation
// This allows the field to spiral independently while QS wheel stays fixed
vec3 fieldLines(vec2 uv, float intensity, int maxLines) {
    if (intensity < 0.01) return vec3(0.0);

    vec3 lines = vec3(0.0);

    for (int i = 0; i < maxLines; i++) {
        float chroma_val = getChroma(i);
        if (chroma_val < 0.1) continue;

        float arc_angle = float(i) * TAU / 12.0 + u_field_rotation;
        vec2 arc_dir = vec2(cos(arc_angle), sin(arc_angle));

        float arc_dist = abs(dot(uv, vec2(-arc_dir.y, arc_dir.x)));
        float arc_t = dot(uv, arc_dir);

        if (arc_t > 0.0) {
            float line = smoothstep(0.025, 0.0, arc_dist) * chroma_val;
            line *= smoothstep(0.0, 0.15, arc_t) * smoothstep(0.8, 0.2, arc_t);
            lines += line * hue2rgb(float(i) / 12.0) * intensity;
        }
    }

    return lines;
}

// Main ring dive effects compositor
vec3 ringDiveEffects(vec2 uv, vec3 col) {
    // === GATE: Skip entirely when inactive ===
    if (u_ring_dive_active == 0) return col;

    float t = u_ring_dive_t;

    // Subrange detection
    bool is_lock_on = t < 0.18;
    bool is_warp_out = t >= 0.18 && t < 0.35;
    bool is_flight = t >= 0.35 && t < 0.80;
    bool is_warp_back = t >= 0.80;

    // === PORTAL RINGS (during lock_on) ===
    if (is_lock_on) {
        float lock_t = t / 0.18;
        col = portalRings(uv, col, lock_t);
    }

    // === WARP STREAKS (during warp_out and early warp_back) ===
    if (is_warp_out) {
        float warp_t = (t - 0.18) / 0.17;
        int maxSamples = (u_ring_dive_quality == RING_QUALITY_LOW) ? 6 :
                         (u_ring_dive_quality == RING_QUALITY_MED) ? 10 : 14;
        col = warpStreaks(uv, col, warp_t, maxSamples);
    }

    // === FIELD LINES (during guided_flight, FADE OUT in warp_back) ===
    if (is_flight) {
        float flight_t = (t - 0.35) / 0.45;
        int maxLines = (u_ring_dive_quality == RING_QUALITY_LOW) ? 4 :
                       (u_ring_dive_quality == RING_QUALITY_MED) ? 8 : 12;
        col += fieldLines(uv, flight_t, maxLines);
    }

    // === WARP BACK: Return to harmonic rings ===
    // RINGS ARE STRICTLY DOMINANT - interior suppressed
    if (is_warp_back) {
        float back_t = (t - 0.80) / 0.20;  // 0 at t=0.80, 1 at t=1.0

        // After t > 0.85 (back_t > 0.25): RINGS ONLY
        bool rings_dominant = back_t > 0.25;

        if (rings_dominant) {
            // === SUPPRESS EVERYTHING EXCEPT RINGS ===
            // Darken any interior/warp contribution aggressively
            col *= 0.15;  // Crush previous content

            // NO warp streaks, NO field lines after this point

            // === RINGS AT 2x BOOST ===
            float ring_intensity = smoothstep(0.25, 0.5, back_t) * 2.0;
            col = portalRings(uv, col, ring_intensity);

            // === SNAP MOMENT at t = 0.92 (back_t = 0.6) ===
            float snap_center = 0.6;
            float snap_width = 0.08;
            float snap_intensity = exp(-pow((back_t - snap_center) / snap_width, 2.0));

            if (snap_intensity > 0.1) {
                // CRISP 12-fold rings at snap
                float dist = length(uv);
                float normAngle = mod(atan(uv.y, uv.x) + PI, TAU);

                for (int i = 0; i < 12; i++) {
                    float ringRadius = 0.06 + float(i) * 0.065;
                    float ringDist = abs(dist - ringRadius);
                    // Very crisp rings
                    float ring = smoothstep(0.012, 0.002, ringDist);
                    // 12-fold angular brightness
                    float ringAngle = float(i) * TAU / 12.0;
                    ring *= 0.7 + 0.3 * cos(normAngle - ringAngle);

                    vec3 ringColor = hue2rgb(float(i) / 12.0);
                    float chroma_val = getChroma(i);
                    col += ring * ringColor * snap_intensity * (0.5 + chroma_val * 0.8);
                }

                // Bloom spike
                col *= 1.0 + snap_intensity * 0.35;
            }
        } else {
            // Early warp back (t = 0.80 to 0.85): transition phase
            // Warp streaks fade quickly
            float warp_fade = 1.0 - smoothstep(0.0, 0.25, back_t);
            if (warp_fade > 0.01) {
                int maxSamples = (u_ring_dive_quality == RING_QUALITY_LOW) ? 4 :
                                 (u_ring_dive_quality == RING_QUALITY_MED) ? 6 : 8;
                col = warpStreaks(uv, col, warp_fade * 0.4, maxSamples);
            }

            // Rings starting to appear
            float ring_fade = smoothstep(0.0, 0.25, back_t);
            col = portalRings(uv, col, ring_fade * 0.8);

            // Darken interior contribution
            col *= 0.6 + 0.4 * (1.0 - back_t);
        }
    }

    return col;
}

// ============================================================================
// TORUS KNOT CYMATICS - Tertiary Inside Transition
// ============================================================================
//
// When u_torus_active == 1, renders a (p,q) torus knot with marching nodes.
// Lightweight: uses line distance approximation, not volumetric raymarch.
//
// Subphases:
//   CurlToTorus (0): 24 spokes bend into donut ring
//   ThreadKnot  (1): Ring morphs into torus knot path
//   Stabilize   (2): Damped settle, nodes begin marching
//   Active      (3): Full knot rendering
// ============================================================================

// Torus knot parametric position: point on a (p,q) torus knot
// p = number of times curve wraps around torus tube
// q = number of times curve wraps around torus center
vec3 torusKnotPos(float t, int p, int q, float R, float r) {
    float phi = float(p) * t * 6.283185;  // TAU * p * t
    float theta = float(q) * t * 6.283185; // TAU * q * t

    // Radius varies as we go around the tube
    float rr = r * cos(theta) + R;

    return vec3(
        rr * cos(phi),
        rr * sin(phi),
        r * sin(theta)
    );
}

// === PARALLEL TRANSPORT FRAME ===
// Avoids Frenet flipping at inflection points by rotating frame smoothly along curve

// Compute tangent via central difference
vec3 knotTangent(float t, float p, float q, float R, float r) {
    float eps = 0.001;
    int ip = int(p);
    int iq = int(q);
    vec3 p0 = torusKnotPos(t - eps, ip, iq, R, r);
    vec3 p1 = torusKnotPos(t + eps, ip, iq, R, r);
    return normalize(p1 - p0);
}

// Initialize stable frame at t=0 using torus geometry
void initFrame(float p, float q, float R, float r,
               out vec3 T, out vec3 N, out vec3 B) {
    T = knotTangent(0.0, p, q, R, r);
    vec3 pos0 = torusKnotPos(0.0, int(p), int(q), R, r);
    vec3 toCenter = normalize(vec3(pos0.x, pos0.y, 0.0));
    N = normalize(toCenter - T * dot(toCenter, T));
    B = cross(T, N);
}

// Transport frame along curve using Rodrigues rotation formula
void transportFrame(vec3 oldT, vec3 newT, inout vec3 N, inout vec3 B) {
    vec3 axis = cross(oldT, newT);
    float sinA = length(axis);
    if (sinA < 0.0001) return;  // Tangent unchanged
    axis /= sinA;
    float cosA = dot(oldT, newT);
    N = N * cosA + cross(axis, N) * sinA + axis * dot(axis, N) * (1.0 - cosA);
    B = cross(newT, normalize(N));
}

// === TUBE LIGHTING FUNCTION ===
// Premium 3D lighting with diffuse, specular (Blinn-Phong), and rim (Fresnel)

vec3 tubeLighting(vec3 N, vec3 V, vec3 baseColor,
                  float specPower, float specBoost, float rimGain) {
    // Key light (upper-right-front)
    vec3 L1 = normalize(vec3(0.5, 0.7, 0.5));
    // Fill light (left-lower)
    vec3 L2 = normalize(vec3(-0.6, 0.3, 0.4));

    // Diffuse lighting
    float diff1 = max(dot(N, L1), 0.0) * 0.7;
    float diff2 = max(dot(N, L2), 0.0) * 0.3;
    float diff = diff1 + diff2;
    vec3 diffuse = baseColor * (diff + 0.15);  // Ambient term

    // Specular (Blinn-Phong)
    vec3 H = normalize(L1 + V);
    float spec = pow(max(dot(N, H), 0.0), specPower) * (1.0 + specBoost);
    vec3 specular = vec3(1.0, 0.98, 0.95) * spec * 0.6;

    // Rim light (Fresnel)
    float fresnel = pow(1.0 - max(dot(N, V), 0.0), 3.0);
    vec3 rim = vec3(0.9, 0.95, 1.0) * fresnel * rimGain;

    return diffuse + specular + rim;
}

// === N-FOLD VORTEX VOLUME FIELD ===
// Analytical vortex pattern locked to N-fold symmetry (no raymarch needed)

float vortexField(vec2 uv, int n, float strength, float time) {
    if (strength < 0.01 || n < 1) return 0.0;

    float r = length(uv);
    float theta = atan(uv.y, uv.x);

    // N-fold symmetry locking
    float foldAngle = TAU / float(n);
    float foldedTheta = mod(theta + foldAngle * 0.5, foldAngle) - foldAngle * 0.5;

    // Spiral arms curving inward (locked to beat_phase, not raw time)
    float spiralPhase = foldedTheta * 2.0 + r * 8.0 - time * 0.5;
    float spiral = pow(sin(spiralPhase) * 0.5 + 0.5, 3.0);

    // Radial falloff (hole at center)
    float holeR = 0.1, outerR = 0.35;
    float radial = smoothstep(holeR, holeR * 2.0, r) * (1.0 - smoothstep(outerR * 0.7, outerR, r));

    // Angular concentration at N-fold boundaries
    float angular = exp(-foldedTheta * foldedTheta * 20.0);

    return spiral * radial * (0.3 + angular * 0.7) * strength;
}

// Simple torus ring position (before threading into knot)
vec3 torusRingPos(float t, float R, float r) {
    float phi = t * 6.283185;  // TAU * t
    return vec3(
        (R + r) * cos(phi),
        (R + r) * sin(phi),
        0.0
    );
}

// Distance from point to line segment (for strand rendering)
float distToSegment(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a;
    vec2 ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// Render torus knot cymatics
// Returns color contribution
// ============================================================================
// TORUS KNOT RENDERING - VORTEX FIELD LINES
//
// Philosophy: The torus is a VORTEX consuming energy toward the aether
//   - The knot path defines the structure (p:q winding)
//   - Field lines spiral around the torus, converging toward center hole
//   - Like magnetic flux at a pole, or water draining
//   - The central void (hole) is the attractor
//   - Shape changes with frequency but vortex flow is always present
//
// Visual layers:
//   1. Core knot strand (thin, bright - shows topology)
//   2. Field lines (multiple rings spiraling around torus surface)
//   3. Radial convergence (lines pulling toward center hole)
//   4. Flow animation (energy draining toward aether)
// ============================================================================

vec3 renderTorusKnot(vec2 uv, vec3 baseCol, vec2 uv_screen) {
    // === CANDIDATE PREVIEW CUE ===
    // Cheap visual hint when emergence is pending but not yet active
    // NO expensive math - just a subtle center glow pulse
    if (u_emerge_candidate > 0.5 && u_torus_active == 0) {
        float uvLen = length(uv);
        // Gentle center glow pulsing at ~1Hz
        float pulse = 0.5 + 0.5 * sin(u_time * 6.28);
        float cueGlow = exp(-uvLen * uvLen * 4.0) * pulse * 0.15;
        // Warm gold hint of incoming torus
        vec3 cueColor = vec3(1.0, 0.85, 0.5) * cueGlow;
        return baseCol + cueColor;
    }

    if (u_torus_active == 0) return baseCol;

    // === PREWARM EARLY-OUT ===
    // During prewarm (t < 0.001), do minimal work but touch all code paths
    // This warms the GPU without producing visible output
    if (u_torus_t < 0.001) {
        // Touch the uniforms so compiler doesn't optimize away
        float warm = u_torus_p + u_torus_q + u_torus_major_r + u_torus_minor_r;
        return baseCol + vec3(warm * 0.0);  // Always 0 contribution
    }

    // === EARLY SCREEN-SPACE CULLING ===
    float uvLen = length(uv);
    // Tighter culling: torus fits within ~1.0 radius
    if (uvLen > 1.0) return baseCol * u_torus_video_fade;

    // === TRANSITION ===
    float curl_t = smoothstep(0.0, 0.35, u_torus_t);
    float thread_t = smoothstep(0.35, 0.75, u_torus_t);

    // === GEOMETRY (sized to wrap around QS Wheel) ===
    float R = mix(0.5, u_torus_major_r, curl_t);   // Major radius (hole to tube center)
    float r = mix(0.0, u_torus_minor_r, curl_t);   // Minor radius (tube thickness)

    // === CAMERA (quantized 24-fold locked orbit + slight top-down tilt) ===
    // Use quantized orbit step instead of continuous time (prevents drift between beats)
    float stepAngle = float(u_torus_orbit_step) * TAU / 24.0;
    float camAngle = u_torus_seed_phase + stepAngle;
    float camTilt = 0.55;  // Tilted to see the hole (vortex drain)
    float ca = cos(camAngle), sa = sin(camAngle);
    float ct = cos(camTilt), st = sin(camTilt);

    // View direction for lighting (camera looks at origin)
    vec3 viewDir = normalize(vec3(-ca * ct, -sa * ct, -st));

    // === CHROMA-REACTIVE COLOR (unrolled for fewer branches) ===
    // Find top 2 chroma bins
    float maxC = 0.0, secondC = 0.0;
    int dom = 0, secondDom = 0;
    // Unrolled loop: significantly faster on GPU
    #define CHECK_CHROMA(i) { float c = getChroma(i); if (c > maxC) { secondC = maxC; secondDom = dom; maxC = c; dom = i; } else if (c > secondC) { secondC = c; secondDom = i; } }
    CHECK_CHROMA(0); CHECK_CHROMA(1); CHECK_CHROMA(2); CHECK_CHROMA(3);
    CHECK_CHROMA(4); CHECK_CHROMA(5); CHECK_CHROMA(6); CHECK_CHROMA(7);
    CHECK_CHROMA(8); CHECK_CHROMA(9); CHECK_CHROMA(10); CHECK_CHROMA(11);
    #undef CHECK_CHROMA

    vec3 primaryColor = hue2rgb(float(dom) / 12.0);
    vec3 secondaryColor = hue2rgb(float(secondDom) / 12.0);
    vec3 knotColor = mix(primaryColor, vec3(1.0, 0.95, 0.8), u_torus_golden_blend * 0.5);

    // === VORTEX FLOW TIMING ===
    float flowTime = u_torus_phase * 2.0 + u_time * 0.3;

    // =================================================================
    // LAYER 1: CORE KNOT STRAND - HYBRID ANALYTICAL + POLYLINE
    // =================================================================
    // REG Math: T(p,q) torus knot where theta = (q/p) * phi
    //
    // OPTIMIZATION: Use analytical distance for initial estimate,
    // then refine with reduced polyline only when close to torus
    // This preserves exact REG math while reducing iteration count
    // =================================================================

    float coreWidth = 0.006;
    float glowWidth = 0.04;

    float knotMinDist = 100.0;
    float knotDepth = 0.5;
    float knotT = 0.0;
    float secondKnotDist = 100.0;
    float secondKnotDepth = 0.5;

    float ease = thread_t * thread_t * (3.0 - 2.0 * thread_t);

    // === ANALYTICAL TORUS DISTANCE (cheap pre-filter) ===
    // Project screen point back to approximate torus surface distance
    // This lets us skip expensive polyline for pixels far from torus
    float torusInner = R - r;
    float torusOuter = R + r;
    float approxTorusDist = abs(uvLen - R) - r * 0.7;  // Rough approximation

    // Only do expensive polyline iteration when close to torus projection
    bool nearTorus = (uvLen > torusInner * 0.5) && (uvLen < torusOuter * 1.3);

    if (nearTorus) {
        // === ADAPTIVE SEGMENT COUNT ===
        // Fewer segments during transition (knot faint anyway)
        // Fewer segments when far from torus center
        int baseSegCount = (u_quality_tier == 0) ? 48 :
                           (u_quality_tier == 1) ? 64 : 96;

        // Scale segment count for high-p canvases (89_144, 76_123, etc.)
        // More complex topology needs more segments to avoid aliasing
        float complexityScale = max(1.0, u_torus_p / 34.0);
        baseSegCount = int(float(baseSegCount) * min(complexityScale, 2.0));

        // Reduce segments during early transition
        float segScale = smoothstep(0.2, 0.6, u_torus_t);
        int segCount = int(float(baseSegCount) * (0.5 + 0.5 * segScale));
        segCount = max(segCount, 24);  // Minimum for topology

        float invSeg = 1.0 / float(segCount);
        float dPhi = u_torus_p * TAU * invSeg;
        float dTheta = u_torus_q * TAU * invSeg;

        // Pre-compute incremental rotation
        float cosDPhi = cos(dPhi), sinDPhi = sin(dPhi);
        float cosDTheta = cos(dTheta), sinDTheta = sin(dTheta);

        float cosPhi = 1.0, sinPhi = 0.0;
        float cosTheta = 1.0, sinTheta = 0.0;

        vec2 prevScreen = vec2(0.0);
        float prevDepth = 0.5;
        float prevT = 0.0;
        bool hasPrev = false;

        for (int i = 0; i <= segCount; i++) {
            float t = float(i) * invSeg;

            // Ring -> Knot transition (REG: preserves T(p,q) topology)
            vec3 ringPos = vec3((R + r) * cosPhi, (R + r) * sinPhi, 0.0);
            float rr = r * cosTheta + R;
            vec3 knotPos = vec3(rr * cosPhi, rr * sinPhi, r * sinTheta);
            vec3 pos3d = mix(ringPos, knotPos, ease);

            // Camera transform
            float x1 = pos3d.x * ca - pos3d.y * sa;
            float y1 = pos3d.x * sa + pos3d.y * ca;
            float y2 = y1 * ct - pos3d.z * st;
            float z2 = y1 * st + pos3d.z * ct;

            float depth = clamp((z2 + 1.0) * 0.5, 0.0, 1.0);
            vec2 screenPos = vec2(x1, y2) * (0.75 + depth * 0.35);

            if (hasPrev) {
                vec2 pa = uv - prevScreen;
                vec2 ba = screenPos - prevScreen;
                float baDot = dot(ba, ba);
                float h = clamp(dot(pa, ba) / (baDot + 0.0001), 0.0, 1.0);
                float d = length(pa - ba * h);

                if (d < knotMinDist) {
                    // Track crossing for depth sorting
                    if (knotMinDist < 100.0 && abs(knotT - mix(prevT, t, h)) > 0.08) {
                        secondKnotDist = knotMinDist;
                        secondKnotDepth = knotDepth;
                    }
                    knotMinDist = d;
                    knotDepth = mix(prevDepth, depth, h);
                    knotT = mix(prevT, t, h);
                } else if (d < secondKnotDist && abs(mix(prevT, t, h) - knotT) > 0.08) {
                    secondKnotDist = d;
                    secondKnotDepth = mix(prevDepth, depth, h);
                }
            }

            prevScreen = screenPos;
            prevDepth = depth;
            prevT = t;
            hasPrev = true;

            // Incremental rotation (no trig in loop - critical optimization)
            float newCosPhi = cosPhi * cosDPhi - sinPhi * sinDPhi;
            float newSinPhi = sinPhi * cosDPhi + cosPhi * sinDPhi;
            cosPhi = newCosPhi;
            sinPhi = newSinPhi;

            float newCosTheta = cosTheta * cosDTheta - sinTheta * sinDTheta;
            float newSinTheta = sinTheta * cosDTheta + cosTheta * sinDTheta;
            cosTheta = newCosTheta;
            sinTheta = newSinTheta;
        }
    }

    // =================================================================
    // LAYER 2: ENERGY STREAMLINES - T(p,q) TOPOLOGY-FOLLOWING FLOW
    // =================================================================
    // Enhanced field-aligned energy paths that follow the torus knot's
    // mathematical structure. Primary streamlines follow T(p,q) winding,
    // secondary cross-streams create interference (moire) patterns.
    //
    // Quality scaling:
    //   Low: Basic single streamline pattern
    //   Med: Enhanced dual-stream pattern
    //   High: Full interference with moire nodes
    // =================================================================

    float fieldAccum = 0.0;
    float avgFieldDepth = 0.5;
    float moireNodes = 0.0;

    // Compute at all tiers, scaled by quality
    if (nearTorus && u_torus_t > 0.4) {
        float polarAngle = atan(uv.y, uv.x);

        // === COUNTER-ROTATION: Torus streamlines spin opposite to outer field ===
        // Creates gyroscope-like effect: outer field spins one way, torus spins other
        // Uses negative u_field_rotation for counter-rotation
        polarAngle -= u_field_rotation * 0.5;  // Half speed, opposite direction

        // Radial position on torus (0 = inner hole, 1 = outer edge)
        float radialPos = (uvLen - torusInner) / (torusOuter - torusInner);
        radialPos = clamp(radialPos, 0.0, 1.0);

        // === PRIMARY STREAMLINES: Follow T(p,q) winding topology ===
        // Stream count derived from knot topology - matches knot complexity
        float streamCount = max(u_torus_p, 3.0);

        // Primary phase: spirals following the p:q winding ratio
        // This creates the "energy flowing along field lines" look
        float primaryPhase = polarAngle * streamCount + radialPos * u_torus_q * TAU;
        primaryPhase += flowTime * 1.5;  // Flow animation

        // Flow speed visualization: faster toward center (accelerating drain)
        float flowSpeed = 1.0 + (1.0 - radialPos) * 2.0;
        primaryPhase *= flowSpeed;

        // Primary stream pattern (sharp lines)
        float stream1 = pow(sin(primaryPhase) * 0.5 + 0.5, 3.0);

        fieldAccum = stream1 * 0.6;

        // === SECONDARY CROSS-STREAMS (Med+ tiers) ===
        if (u_quality_tier >= 1) {
            // Cross-streams run perpendicular to primary, creating grid-like pattern
            float crossCount = 12.0;  // Fixed density for cross-streams
            float crossPhase = radialPos * crossCount * TAU + polarAngle * 3.0 - flowTime * 0.8;

            float cross1 = pow(sin(crossPhase) * 0.5 + 0.5, 3.0);

            // Add cross pattern contribution
            fieldAccum += cross1 * 0.25;

            // === INTERFERENCE (MOIRE) NODES (High tier) ===
            if (u_quality_tier >= 2) {
                // Moire pattern: where primary and secondary streams intersect
                // Creates bright "energy nodes" at interference points
                float interference = stream1 * cross1;
                moireNodes = pow(interference, 1.5) * 3.0;
                fieldAccum += moireNodes * 0.4;
            }
        }

        // Fade at torus edges (smooth transition)
        float edgeFade = sin(radialPos * PI);
        fieldAccum *= edgeFade * edgeFade;

        // Energy-reactive intensity
        fieldAccum *= 0.7 + u_energy * 0.5;

        // Depth approximation: outer regions appear "behind"
        avgFieldDepth = 0.3 + radialPos * 0.4;
    }

    // =================================================================
    // LAYER 3: RADIAL CONVERGENCE (energy draining toward center)
    // =================================================================
    // Optimized: analytical 24-fold pattern (no loop!)

    float radialAccum = 0.0;
    float centerDist = uvLen;

    float innerEdge = (R - r) * 0.7;
    float outerEdge = (R + r) * 1.1;

    // Only compute radials within torus projection
    if (centerDist > innerEdge * 0.3 && centerDist < outerEdge) {
        // Single atan call, analytical 24-spoke pattern
        float polarAngle = atan(uv.y, uv.x);

        // Rotate with flow + audio-reactive field rotation
        // Field rotation adds energy-driven spin to the drain pattern
        float rotatedAngle = polarAngle - flowTime * 0.2 + u_field_rotation * 0.3;
        float spokeAngle = TAU / 24.0;

        // Distance to nearest spoke (analytical - no loop!)
        float angleMod = mod(rotatedAngle + spokeAngle * 0.5, spokeAngle) - spokeAngle * 0.5;
        float spokeWidth = 0.08;
        float angleProx = exp(-angleMod * angleMod / (spokeWidth * spokeWidth) * 4.0);

        // Radial fade: brighter near center (the drain)
        float radialFade = 1.0 - smoothstep(innerEdge, outerEdge, centerDist);
        radialFade = radialFade * radialFade * (3.0 - 2.0 * radialFade);  // smootherstep

        radialAccum = angleProx * radialFade;
    }

    // =================================================================
    // LAYER 4: CENTRAL VOID (the aether - where energy drains)
    // =================================================================

    float voidGlow = 0.0;
    float holeRadius = (R - r) * 0.5;

    if (centerDist < holeRadius * 1.5) {
        // Soft glow around the void
        float voidProx = 1.0 - smoothstep(0.0, holeRadius * 1.2, centerDist);
        voidGlow = pow(voidProx, 2.0);

        // Pulsing with energy (the drain consumes)
        voidGlow *= 0.5 + 0.5 * sin(flowTime * TAU * 0.5);
    }

    // =================================================================
    // LAYER 4.5: N-FOLD VORTEX VOLUME (locked to p-fold symmetry)
    // =================================================================
    // Analytical vortex pattern adds spatial depth to center without raymarch.
    // N-fold is locked to knot's p value for coherent topology.

    float vortexAccum = 0.0;
    if (u_torus_vortex_strength > 0.01 && u_torus_vortex_n >= 1) {
        vortexAccum = vortexField(uv, u_torus_vortex_n, u_torus_vortex_strength, flowTime);
    }

    // =================================================================
    // LAYER 5: PULSING SHOCKWAVES (expanding rings from central void)
    // =================================================================
    // Quality scaling: Med=2 waves, High=3 waves
    // Sharp leading edge, soft trailing edge, fades as it expands
    // Kick amplifies current wave intensity

    float shockwaveAccum = 0.0;
    if (u_quality_tier >= 1 && centerDist > holeRadius * 0.5 && centerDist < torusOuter * 1.2) {
        int waveCount = (u_quality_tier >= 2) ? 3 : 2;

        for (int w = 0; w < 3; w++) {
            if (w >= waveCount) break;

            // Each wave has different speed and phase offset
            float waveSpeed = 0.15 + float(w) * 0.08;  // Faster waves follow
            float waveOffset = float(w) * 0.33;

            // Wave phase cycles continuously, kick resets phase
            float wavePhase = fract(flowTime * waveSpeed + waveOffset);

            // Wave radius expands from hole to torus outer edge
            float waveRadius = holeRadius + wavePhase * (torusOuter - holeRadius);

            // Ring distance and mask
            float ringDist = abs(centerDist - waveRadius);
            float ringWidth = 0.015 + wavePhase * 0.025;  // Width grows as it expands

            // Sharp leading edge, soft trailing edge
            float ringMask = smoothstep(ringWidth, ringWidth * 0.2, ringDist);

            // Fade as wave expands (energy dissipates)
            float fadeFactor = 1.0 - wavePhase * wavePhase;

            // Kick boost: amplify wave intensity during kicks
            float kickBoost = 1.0 + u_kick * 2.0;

            shockwaveAccum += ringMask * fadeFactor * kickBoost * (0.5 + 0.5 * float(2 - w) / 2.0);
        }

        // Clamp total accumulation
        shockwaveAccum = min(shockwaveAccum, 1.5);
    }

    // =================================================================
    // LAYER 6: VORTEX FLOW PARTICLES (spiraling into central void)
    // =================================================================
    // Procedural particles using hash-based pseudo-random positioning.
    // Quality scaling: Med=48 particles, High=96 particles
    // Particles accelerate toward center, grow as they approach void,
    // and shift from knot color to hot white.
    // u_torus_particle_rate uniform controls spawn density.
    // u_torus_turbulence (tension-driven) affects spiral tightness.

    float particleAccum = 0.0;
    vec3 particleColor = vec3(0.0);
    float nearestParticleDist = 100.0;  // Track for video ripple
    float nearestParticleLife = 0.0;

    if (u_quality_tier >= 1 && u_torus_particle_rate > 0.01) {
        // Particle count scales with quality tier and particle_rate
        int particleCount = (u_quality_tier >= 2) ? 96 : 48;
        particleCount = int(float(particleCount) * u_torus_particle_rate);
        particleCount = max(particleCount, 12);  // Minimum for effect

        // Particle display radius (area around each particle that lights up)
        float particleRadius = 0.012 + 0.008 * u_energy;

        for (int p = 0; p < 96; p++) {
            if (p >= particleCount) break;

            // Hash-based pseudo-random per particle
            float pf = float(p);
            float phash = fract(sin(pf * 127.1 + 311.7) * 43758.5453);
            float phash2 = fract(sin(pf * 269.5 + 183.3) * 43758.5453);
            float phash3 = fract(sin(pf * 419.2 + 371.9) * 43758.5453);

            // Spawn angle (evenly distributed + hash offset)
            float spawnAngle = (pf / float(particleCount)) * TAU + phash * TAU;

            // Life cycle: 0 = spawn at outer edge, 1 = absorbed at center
            float lifeSpeed = 0.25 + phash2 * 0.15;  // Varied speeds
            float life = fract(flowTime * lifeSpeed + phash);

            // Easing: accelerate toward center (ease-in)
            float radialT = 1.0 - pow(1.0 - life, 2.0);

            // Particle radius: outer edge -> center void
            float pRadius = mix(torusOuter * 0.95, holeRadius * 0.3, radialT);

            // Spiral wrap: tighter near center, turbulence affects tightness
            float spiralWrap = 1.5 + phash3 * 1.0;
            spiralWrap *= 1.0 + u_torus_turbulence * 0.5;  // Turbulence loosens spiral
            float pAngle = spawnAngle + radialT * spiralWrap * TAU;

            // Particle position
            vec2 pPos = vec2(cos(pAngle), sin(pAngle)) * pRadius;

            // Distance from this pixel to particle
            float pDist = length(uv - pPos);

            // Particle size grows as it approaches center (energy concentration)
            float pSize = particleRadius * (0.5 + radialT * 1.5);

            if (pDist < pSize * 2.0 && pDist < nearestParticleDist) {
                nearestParticleDist = pDist;
                nearestParticleLife = life;
            }
        }

        // Render nearest particle with glow
        if (nearestParticleDist < particleRadius * 3.0) {
            float pGlow = exp(-pow(nearestParticleDist / particleRadius, 2.0) * 2.0);

            // Color shift: knot color -> hot white as particle approaches center
            vec3 pColor = mix(knotColor, vec3(1.0, 0.98, 0.9), nearestParticleLife * nearestParticleLife);

            particleAccum = pGlow;
            particleColor = pColor;

            // Hot core for very close particles
            if (nearestParticleDist < particleRadius * 0.5) {
                particleAccum += (1.0 - nearestParticleDist / (particleRadius * 0.5)) * 0.5;
                particleColor = mix(particleColor, vec3(1.0, 0.98, 0.95), 0.3);
            }
        }
    }

    // =================================================================
    // DEPTH & CROSSING DETECTION FOR KNOT
    // =================================================================

    // Depth brightness with latched depth separation for enhanced front/back contrast
    // Higher depth_sep = more contrast between front (bright) and back (dim)
    float depthRange = 0.25 + u_torus_depth_sep * 0.25;  // 0.25-0.50 base
    float depthMult = 0.75 - u_torus_depth_sep * 0.25;   // 0.75-0.50 range
    float depthBrightness = depthRange + knotDepth * depthMult;

    float crossingGlow = 0.0;
    float crossingDim = 1.0;

    if (secondKnotDist < glowWidth * 1.5 && knotMinDist < glowWidth) {
        float crossProx = 1.0 - smoothstep(coreWidth, glowWidth * 1.2, secondKnotDist);
        float depthDiff = knotDepth - secondKnotDepth;

        if (depthDiff > 0.05) {
            crossingGlow = crossProx * 0.7;
        } else if (depthDiff < -0.05) {
            crossingDim = 1.0 - crossProx * 0.4;
        }
    }

    // =================================================================
    // COMPOSITING
    // =================================================================

    vec3 col = baseCol * u_torus_video_fade;
    float intensity = u_torus_t;

    // --- Field lines (subtle, atmospheric) ---
    vec3 fieldColor = mix(knotColor * 0.5, vec3(0.4, 0.5, 0.7), 0.3);
    float fieldVis = min(fieldAccum * 0.4, 0.8) * avgFieldDepth;
    col += fieldColor * fieldVis * intensity * 0.5;

    // --- Radial convergence (energy draining) ---
    vec3 radialColor = mix(knotColor, vec3(0.8, 0.9, 1.0), 0.4);
    col += radialColor * radialAccum * intensity * 0.35 * (0.5 + u_energy * 0.5);

    // --- Central void glow (the aether) ---
    // Let void glow show - video will blend on top
    vec3 voidColor = vec3(0.9, 0.95, 1.0);  // Cool white
    col += voidColor * voidGlow * intensity * 0.4;

    // --- N-fold vortex volume (spatial depth in center) ---
    if (vortexAccum > 0.01) {
        vec3 vortexColor = mix(knotColor * 0.7, vec3(0.6, 0.7, 0.9), 0.4);
        col += vortexColor * vortexAccum * intensity * 0.35;
    }

    // --- Pulsing shockwaves (dramatic kick response) ---
    if (u_quality_tier >= 1) {
        vec3 shockColor = mix(knotColor, vec3(1.0, 0.95, 0.9), 0.6);  // Warm white tint
        col += shockColor * shockwaveAccum * intensity * 0.25;
    }

    // --- Vortex flow particles (spiraling into void) ---
    if (u_quality_tier >= 1 && particleAccum > 0.01) {
        col += particleColor * particleAccum * intensity * 0.6;
    }

    // --- Core knot strand (the topology) - VOLUMETRIC GLOW SYSTEM ---
    // Multi-layer glow with quality tier scaling:
    // Low: 2 layers (core + inner)
    // Med: 4 layers (core + inner + outer + haze)
    // High: 6 layers (all + SSS + Fresnel)
    if (knotMinDist < glowWidth * 3.0) {
        // Strand colors with iridescent shift
        float iridShift = knotT * 2.0 + knotDepth * 0.3;
        vec3 strandColor = mix(knotColor, secondaryColor, sin(iridShift * TAU) * 0.25 + 0.25);

        // Base intensity factors
        float knotIntensity = depthBrightness * intensity * crossingDim;

        // === LAYER 1: HOT WHITE CORE (all tiers) ===
        float coreW = coreWidth * (0.8 + 0.2 * u_energy);  // Energy-reactive core width
        float core = exp(-pow(knotMinDist / coreW, 2.0) * 2.0);
        vec3 coreColor = vec3(1.0, 0.98, 0.95);
        col += coreColor * core * knotIntensity * 1.2;

        // === LAYER 2: INNER GLOW - saturated knot color (all tiers) ===
        float innerW = glowWidth * 0.5;
        float innerGlow = exp(-pow(knotMinDist / innerW, 2.0) * 1.5);
        col += strandColor * innerGlow * knotIntensity * 0.8;

        // Flow pulses along knot
        float flowPulse = sin((knotT * u_torus_p * 4.0 - flowTime) * TAU) * 0.5 + 0.5;
        flowPulse = pow(flowPulse, 2.0);
        col += strandColor * innerGlow * flowPulse * knotIntensity * 0.4 * u_energy;

        // === LAYER 3: OUTER GLOW - desaturated haze (Med+ tiers) ===
        if (u_quality_tier >= 1) {
            float outerW = glowWidth * 1.5;
            float outerGlow = exp(-pow(knotMinDist / outerW, 2.0) * 0.8);
            vec3 outerColor = mix(strandColor, vec3(0.7, 0.8, 0.9), 0.5);  // Desaturated toward blue-white
            col += outerColor * outerGlow * knotIntensity * 0.3;
        }

        // === LAYER 4: VOLUMETRIC HAZE - depth-dependent (Med+ tiers) ===
        // Uses latched depth separation for stable front/back differentiation
        if (u_quality_tier >= 1) {
            // Brighter when tube is "behind" (lower depth = closer in z-buffer terms)
            float hazeDepth = smoothstep(0.6, 0.3, knotDepth);
            float hazeW = glowWidth * 2.0;
            float haze = exp(-pow(knotMinDist / hazeW, 2.0) * 0.5);
            vec3 hazeColor = mix(strandColor * 0.5, vec3(0.5, 0.6, 0.8), 0.3);
            // Apply depth separation for enhanced front/back contrast
            float depthContrast = 0.5 + u_torus_depth_sep * 0.5;
            col += hazeColor * haze * hazeDepth * knotIntensity * 0.25 * depthContrast;
        }

        // === LAYER 5: FRESNEL RIM - edge brightening (High tier) ===
        // Uses latched rim gain for stable, beat-synced brightness
        if (u_quality_tier >= 2 && u_torus_rim_gain > 0.01) {
            // Compute screen-space gradient for fresnel approximation
            float depthGradient = length(vec2(dFdx(knotMinDist), dFdy(knotMinDist)));
            float fresnelRim = smoothstep(0.0, 0.02, depthGradient) * innerGlow;
            // Boost rim when knot curves away from viewer
            fresnelRim *= smoothstep(0.3, 0.7, knotDepth);
            // Apply latched rim gain for quantized lighting
            col += vec3(0.9, 0.95, 1.0) * fresnelRim * knotIntensity * 0.4 * u_torus_rim_gain;
        }

        // === LAYER 6: SSS APPROXIMATION - warm light bleeding (High tier) ===
        if (u_quality_tier >= 2 && u_torus_sss_intensity > 0.01) {
            // Subsurface scatter: warm color "bleeds" through tube from behind
            float sssDist = knotMinDist / (glowWidth * 1.2);
            float sss = exp(-sssDist * sssDist * 3.0);
            // Stronger when viewing through tube (low depth = back face)
            float sssDepthFactor = smoothstep(0.5, 0.2, knotDepth);
            vec3 sssColor = vec3(1.0, 0.85, 0.7);  // Warm skin-tone scatter
            col += sssColor * sss * sssDepthFactor * knotIntensity * u_torus_sss_intensity * 0.5;
        }

        // === CROSSING HIGHLIGHTS (all tiers) ===
        col += coreColor * crossingGlow * core * knotIntensity * 1.2;

        // === SPECULAR BOOST (beat-synced highlights) ===
        // Uses latched specular boost for dramatic kick response
        if (u_torus_spec_boost > 0.01) {
            // Specular highlight on hot core (Blinn-Phong approximation)
            float specCore = core * core;  // Sharper falloff for spec
            vec3 specColor = vec3(1.0, 0.98, 0.95);
            col += specColor * specCore * knotIntensity * u_torus_spec_boost * 0.6;
        }
    }

    // --- NODE BEADS (Paint Layer - cymatics) ---
    // Place bright node beads along the knot at intervals derived from node_frequency
    // Phase offset by paint_phase_step (quantized to 24 anchors)
    if (u_torus_node_freq > 0 && knotMinDist < glowWidth * 3.0) {
        // Node frequency determines how many beads along the knot
        float nodeSpacing = 1.0 / float(u_torus_node_freq);

        // Phase offset from paint_phase_step (quantized to 24 anchors)
        float phaseOffset = float(u_torus_paint_phase) / 24.0;

        // Find nearest node to current knot position
        float nodeT = fract(knotT + phaseOffset);
        float nearestNode = round(nodeT / nodeSpacing) * nodeSpacing;
        float nodeProx = abs(nodeT - nearestNode);

        // Node bead radius and glow
        float nodeRadius = 0.025;
        float nodeGlow = exp(-nodeProx * nodeProx / (nodeRadius * nodeRadius) * 8.0);

        // Only show beads when close to knot path
        float pathProx = 1.0 - smoothstep(0.0, glowWidth * 1.5, knotMinDist);
        nodeGlow *= pathProx;

        // Node color: brighter than strand, with slight hue shift
        vec3 nodeColor = mix(knotColor, vec3(1.0, 0.98, 0.9), 0.5);

        // 12-TET paints get subtle breathing animation (but NOT rotation - that's quantized)
        float nodeBreath = 1.0;
        if (u_torus_paint_is_tet == 1) {
            // Gentle pulse tied to energy, not continuous rotation
            nodeBreath = 0.8 + 0.2 * sin(u_time * 2.0 + nearestNode * TAU);
        }

        // Composite node beads
        float nodeIntensity = depthBrightness * intensity * nodeBreath;
        col += nodeColor * nodeGlow * nodeIntensity * 0.8;

        // Hot center for visible nodes
        if (nodeGlow > 0.5) {
            col += vec3(1.0, 0.98, 0.95) * (nodeGlow - 0.5) * 2.0 * nodeIntensity * 0.6;
        }
    }

    // --- Energy pulse on kick ---
    if (u_kick > 0.3) {
        // Radials flash brighter
        col += radialColor * radialAccum * u_kick * 0.4;
        // Void pulses
        col += voidColor * voidGlow * u_kick * 0.5;
    }

    // =================================================================
    // CHROMATIC FRINGING (RGB separation on high-energy areas)
    // =================================================================
    // High tier only, energy-reactive intensity
    // Radial direction offset (red shifts outward, blue shifts inward)
    // Creates dramatic "lens aberration" effect during intense moments

    if (u_quality_tier >= 2 && u_torus_chromatic > 0.01) {
        // Chromatic intensity scales with energy above threshold
        float chromaEnergy = max(u_energy - 0.4, 0.0) * 1.5;
        float chromaIntensity = chromaEnergy * u_torus_chromatic;

        if (chromaIntensity > 0.01) {
            // Chromatic spread: distance between RGB samples
            float chromaSpread = chromaIntensity * 0.015;

            // Direction: radial from center (creates lens-like aberration)
            vec2 chromaDir = normalize(uv + vec2(0.001));

            // Angular variation: slight twist adds visual interest
            float angularVar = sin(atan(uv.y, uv.x) * 6.0 + flowTime) * 0.3;
            chromaDir = vec2(
                chromaDir.x * cos(angularVar) - chromaDir.y * sin(angularVar),
                chromaDir.x * sin(angularVar) + chromaDir.y * cos(angularVar)
            );

            // Calculate offset UVs for R and B channels
            vec2 uvR = uv + chromaDir * chromaSpread;
            vec2 uvB = uv - chromaDir * chromaSpread;

            // Distance-based fringe intensity (stronger at edges)
            float edgeDist = smoothstep(0.2, 0.8, uvLen);
            float fringeMask = edgeDist * chromaIntensity;

            // Approximate color sampling by shifting existing color
            // This is cheaper than re-computing the entire scene
            float lumR = dot(col, vec3(0.299, 0.587, 0.114));
            float lumB = dot(col, vec3(0.299, 0.587, 0.114));

            // Apply chromatic shift
            col.r = mix(col.r, col.r * (1.0 + fringeMask * 0.3), chromaIntensity * 0.7);
            col.b = mix(col.b, col.b * (1.0 - fringeMask * 0.2), chromaIntensity * 0.7);

            // Add slight color fringe at bright edges
            float brightness = dot(col, vec3(0.299, 0.587, 0.114));
            if (brightness > 0.5) {
                float fringeStrength = (brightness - 0.5) * fringeMask * 0.4;
                col.r += fringeStrength * 0.15;
                col.b += fringeStrength * 0.1;
            }
        }
    }

    // VIDEO PORTAL moved to main() as absolute topmost layer (step 12)

    return col;
}

// ============================================================================
// DODECAHEDRAL RESONANCE - Crystalline Harmonic Arrival
// ============================================================================
//
// The dodecahedron represents "harmonic arrival" - the visual reward when music
// settles into coherent, stable harmony. Emerges from Vortex topology.
//
// Geometry (Grant Projection Theorem alignment):
//   - 12 pentagonal faces × 2 orientations = 24-fold symmetry
//   - Face depths driven by Grant shell radii (9 means mapped to 12 faces)
//   - 20 vertices breathe with dominant pitch class
//   - 30 edges pulse on beat transients
//   - Golden ratio (Phi ≈ 1.618) embedded in pentagonal geometry
//
// Visual approach: Stylized 2D projection with depth cues, not true 3D.
// ============================================================================

// Phi (golden ratio) constant for dodecahedral geometry
#define PHI 1.618033988749895
#define INV_PHI 0.618033988749895

// Distance to a pentagon edge (regular pentagon centered at origin)
float sdPentagon(vec2 p, float r) {
    const vec3 k = vec3(0.809016994, 0.587785252, 0.726542528); // sin/cos of pentagon angles
    p.x = abs(p.x);
    p -= 2.0 * min(dot(vec2(-k.x, k.y), p), 0.0) * vec2(-k.x, k.y);
    p -= 2.0 * min(dot(vec2(k.x, k.y), p), 0.0) * vec2(k.x, k.y);
    p -= vec2(clamp(p.x, -r * k.z, r * k.z), r);
    return length(p) * sign(p.y);
}

vec3 renderDodecahedral(vec2 uv, vec3 baseCol, vec2 uv_screen) {
    // Early exit if not active
    if (u_dodeca_active == 0) return baseCol;

    float dt = u_dodeca_t;
    if (dt < 0.01) return baseCol;

    // === COORDINATE SETUP (ASPECT-CORRECTED) ===
    float aspect = u_resolution.x / u_resolution.y;
    vec2 p = uv;
    p.x *= aspect;
    float uvLen = length(p);
    float theta = atan(uv.y, uv.x);

    // Early culling (for 1.6x dodeca)
    if (uvLen > 2.24) return baseCol;

    // === ROTATION (geological drift) ===
    float rot = u_dodeca_rotation;
    float cr = cos(rot), sr = sin(rot);
    vec2 rotP = vec2(p.x * cr - p.y * sr, p.x * sr + p.y * cr);
    float rotTheta = theta + rot;

    // === TIMING FOR ANIMATIONS ===
    float flowTime = u_time * 0.3 + u_dodeca_phi * TAU;
    float breathPhase = u_dodeca_vertex_breath;

    // === 12-FOLD SYMMETRY (faces) ===
    float foldAngle = TAU / 12.0;
    float foldedTheta = mod(rotTheta + foldAngle * 0.5, foldAngle) - foldAngle * 0.5;
    int faceIdx = int(mod(floor((rotTheta + PI) / foldAngle), 12.0));

    // === FACE ACTIVATION (chroma-driven) ===
    float faceActivation = u_dodeca_face_activation[faceIdx];
    float faceDepth = u_dodeca_face_depths[faceIdx];

    // === BASE COLOR SETUP ===
    float hue = float(u_dodeca_gate) / 24.0;
    vec3 primaryColor = hue2rgb(hue);
    vec3 secondaryColor = hue2rgb(fract(hue + 0.4));
    vec3 phiColor = hue2rgb(fract(hue + INV_PHI));
    vec3 crystalTint = vec3(0.85, 0.92, 1.0);
    vec3 warmWhite = vec3(1.0, 0.96, 0.9);

    // =================================================================
    // LAYER 1: CYMATIC WAVE INTERFERENCE - Chladni-like standing waves
    // =================================================================
    // Creates standing wave patterns like Chladni plates on each pentagon
    // face. Multiple frequencies interfere to create complex nodal patterns.

    float cymaticAccum = 0.0;
    vec3 cymaticColor = vec3(0.0);

    // Pentagon-folded coordinates for cymatic pattern
    vec2 cymaP = vec2(uvLen, foldedTheta);

    // Multiple wave frequencies (harmonics)
    for (int h = 1; h <= 5; h++) {
        float freq = float(h) * 3.0;
        float harmonic = float(h);

        // Radial standing wave
        float radialWave = sin(uvLen * freq * TAU + flowTime * (0.5 / harmonic));

        // Angular standing wave (locked to pentagon geometry)
        float angularWave = sin(foldedTheta * 5.0 * harmonic + flowTime * 0.3);

        // Interference pattern
        float interference = radialWave * angularWave;

        // Nodal lines (where wave = 0)
        float nodal = 1.0 - abs(interference);
        nodal = pow(nodal, 4.0);  // Sharpen nodal lines

        // Antinode brightness (where wave amplitude is maximum)
        float antinode = abs(interference);
        antinode = pow(antinode, 2.0);

        // Face activation modulates each harmonic
        float harmWeight = u_dodeca_face_activation[h % 12] * (1.0 / harmonic);

        cymaticAccum += (nodal * 0.6 + antinode * 0.4) * harmWeight;

        // Color varies by harmonic
        vec3 harmColor = hue2rgb(fract(hue + float(h) * 0.12));
        cymaticColor += harmColor * nodal * harmWeight;
    }

    // Fade cymatic toward edges
    float cymaticFade = smoothstep(0.7, 0.3, uvLen) * smoothstep(0.0, 0.1, uvLen);
    cymaticAccum *= cymaticFade * 0.6;

    // =================================================================
    // LAYER 2: PENTAGON SHELL RESONATORS - Concentric vibrating shells
    // =================================================================
    // Pentagon shells at Grant shell radii, with wave animation

    float pentagonAccum = 0.0;
    float shellGlow = 0.0;

    for (int i = 0; i < 9; i++) {
        float shellBase = u_dodeca_face_depths[i % 12] * 0.55 + 0.12;  // Scaled up
        // Breathing animation - shells expand/contract
        float shellBreath = sin(breathPhase + float(i) * 0.7) * 0.015;
        float shellR = shellBase + shellBreath;

        float pentDist = sdPentagon(rotP, shellR);

        // Sharp shell edge
        float shellEdge = exp(-pentDist * pentDist * 300.0);

        // Soft shell glow
        float shellSoft = exp(-abs(pentDist) * 15.0);

        // Wave traveling along shell
        float shellWave = sin(rotTheta * 5.0 - flowTime * 2.0 + float(i)) * 0.5 + 0.5;

        float activation = u_dodeca_face_activation[i % 12];
        pentagonAccum += shellEdge * activation * 0.5;
        shellGlow += shellSoft * shellWave * activation * 0.15;
    }

    // =================================================================
    // LAYER 3: MINI QS WHEELS + VIDEO AT VERTICES - 20 video-textured wheels
    // =================================================================
    // Each of the 20 dodecahedron vertices hosts a mini 12-wedge QS wheel
    // with video texture blended in. Creates a crystalline lattice
    // of harmonic video windows.

    float vertexBreath = sin(breathPhase) * u_dodeca_vertex_amp;
    float vertexGlow = 0.0;
    float vertexCore = 0.0;
    vec3 vertexWheelColor = vec3(0.0);
    float vertexWheelAccum = 0.0;

    // Mini QS wheel parameters (scaled 1.6x - 20% smaller than 2x)
    float miniWheelRadius = 0.088 + vertexBreath * 0.019;  // 20% smaller
    float miniWheelInner = miniWheelRadius * 0.12;         // Thin ring, large video center

    // Inner ring: 10 vertices with mini QS wheels + video (20% smaller than 2x)
    float vRadius1 = 0.56 + vertexBreath * 0.096;
    for (int i = 0; i < 10; i++) {
        float vAngle = float(i) * TAU / 10.0 + rot;
        vec2 vPos = vRadius1 * vec2(cos(vAngle) * aspect, sin(vAngle));  // Aspect-correct position
        vec2 toVertex = p - vPos;
        // Un-correct aspect for circular distance calculation
        float vDist = length(vec2(toVertex.x / aspect, toVertex.y));

        // Check if we're within this mini wheel's area
        if (vDist < miniWheelRadius * 1.5) {
            float activation = u_dodeca_face_activation[i % 12];
            float pulse = sin(flowTime * 2.0 + float(i) * 0.628) * 0.3 + 0.7;

            // Normalize distance within wheel
            float normDist = vDist / miniWheelRadius;

            // Sample video using proper aspect-aware mapping (fills the wheel)
            // Use SAME zoom as main QS wheel for consistent framing
            // normalizedUV: -1 to 1 within the wheel
            vec2 normalizedUV = toVertex / miniWheelRadius;
            // Match main QS wheel zoom (0.25) with portrait handling
            float miniZoom = 0.25;
            if (u_video_aspect < 1.0) {
                miniZoom *= 0.7;  // Portrait: zoom in more (same as main wheel)
            }
            vec2 videoUV = mapCircularToVideoUV(normalizedUV, aspect, miniZoom);
            vec3 videoCol = vec3(0.05);
            if (u_skin_has_texture != 0) {
                videoCol = texture(u_skin_tex, videoUV).rgb;
                // Moderate brightness for visibility (not blown out)
                videoCol *= 1.8;
            }

            // QS wheel ring (between inner and outer edge)
            if (normDist < 1.0 && normDist > miniWheelInner / miniWheelRadius) {
                // Angle within this mini wheel
                float wheelTheta = atan(toVertex.y, toVertex.x) + flowTime * 0.5;

                // 12 wedges for the 12 pitch classes
                float wedgeAngle = TAU / 12.0;
                int wedgeIdx = int(mod(floor((wheelTheta + PI) / wedgeAngle), 12.0));
                float wedgeFrac = fract((wheelTheta + PI) / wedgeAngle);

                // Wedge seam (dark lines between wedges)
                float seamDist = min(wedgeFrac, 1.0 - wedgeFrac);
                float seamMask = smoothstep(0.0, 0.06, seamDist);

                // Get chroma for this wedge
                float chromaVal = getChroma(wedgeIdx);
                vec3 wedgeColor = hue2rgb(float(wedgeIdx) / 12.0);

                // Blend wedge color with video (video tints the wedge)
                vec3 blendedColor = mix(wedgeColor, videoCol * wedgeColor * 2.0, 0.4);

                // Brightness based on chroma value
                float wedgeBright = 0.4 + chromaVal * 0.6;

                // Add to accumulator
                float wheelMask = seamMask * wedgeBright * activation * pulse;
                vertexWheelAccum += wheelMask;
                vertexWheelColor += blendedColor * wheelMask;

                // Bright rim around wheel outer edge
                float rimDist = abs(normDist - 0.92);
                float rimGlow = exp(-rimDist * rimDist * 500.0) * activation;
                vertexCore += rimGlow * 0.3;
            }

            // Video fills the wheel center (expanded to nearly full wheel)
            if (normDist < 0.95) {
                float centerMask = 1.0 - smoothstep(0.0, 0.95, normDist);
                centerMask *= activation * pulse;
                vertexWheelAccum += centerMask * 2.5;
                vertexWheelColor += videoCol * centerMask * 2.5;

                // Subtle core glow (avoid drowning video)
                float coreGlow = exp(-vDist * vDist * 1000.0);
                vertexCore += coreGlow * activation * pulse * 0.4;
            }

            // Outer haze around wheel (scaled for 2x size)
            float outerHaze = exp(-vDist * vDist * 20.0);
            vertexGlow += outerHaze * activation * pulse * 0.6;
        }
    }

    // Outer ring: 10 vertices with mini QS wheels + video (offset by half angle, 20% smaller)
    float vRadius2 = vRadius1 * PHI * 0.9 + vertexBreath * 0.096;
    for (int i = 0; i < 10; i++) {
        float vAngle = (float(i) + 0.5) * TAU / 10.0 + rot;
        vec2 vPos = vRadius2 * vec2(cos(vAngle) * aspect, sin(vAngle));  // Aspect-correct position
        vec2 toVertex = p - vPos;
        // Un-correct aspect for circular distance calculation
        float vDist = length(vec2(toVertex.x / aspect, toVertex.y));

        if (vDist < miniWheelRadius * 1.5) {
            float activation = u_dodeca_face_activation[(i + 5) % 12];
            float pulse = sin(flowTime * 2.0 + float(i) * 0.628 + PI) * 0.3 + 0.7;

            float normDist = vDist / miniWheelRadius;

            // Sample video using proper aspect-aware mapping (fills the wheel)
            // Use SAME zoom as main QS wheel for consistent framing
            vec2 normalizedUV = toVertex / miniWheelRadius;
            // Match main QS wheel zoom (0.25) with portrait handling
            float miniZoom = 0.25;
            if (u_video_aspect < 1.0) {
                miniZoom *= 0.7;  // Portrait: zoom in more (same as main wheel)
            }
            vec2 videoUV = mapCircularToVideoUV(normalizedUV, aspect, miniZoom);
            vec3 videoCol = vec3(0.05);
            if (u_skin_has_texture != 0) {
                videoCol = texture(u_skin_tex, videoUV).rgb;
                videoCol *= 3.5;
            }

            // QS wheel ring (thin outer ring)
            if (normDist < 1.0 && normDist > miniWheelInner / miniWheelRadius) {
                float wheelTheta = atan(toVertex.y, toVertex.x) - flowTime * 0.3;

                float wedgeAngle = TAU / 12.0;
                int wedgeIdx = int(mod(floor((wheelTheta + PI) / wedgeAngle), 12.0));
                float wedgeFrac = fract((wheelTheta + PI) / wedgeAngle);

                float seamDist = min(wedgeFrac, 1.0 - wedgeFrac);
                float seamMask = smoothstep(0.0, 0.06, seamDist);

                float chromaVal = getChroma(wedgeIdx);
                vec3 wedgeColor = hue2rgb(float(wedgeIdx) / 12.0);

                vec3 blendedColor = mix(wedgeColor, videoCol * wedgeColor * 2.0, 0.4);

                float wedgeBright = 0.4 + chromaVal * 0.6;

                float wheelMask = seamMask * wedgeBright * activation * pulse;
                vertexWheelAccum += wheelMask;
                vertexWheelColor += blendedColor * wheelMask;

                float rimDist = abs(normDist - 0.92);
                float rimGlow = exp(-rimDist * rimDist * 500.0) * activation;
                vertexCore += rimGlow * 0.3;
            }

            // Video fills the wheel center (expanded to nearly full wheel)
            if (normDist < 0.95) {
                float centerMask = 1.0 - smoothstep(0.0, 0.95, normDist);
                centerMask *= activation * pulse;
                vertexWheelAccum += centerMask * 2.5;
                vertexWheelColor += videoCol * centerMask * 2.5;

                float coreGlow = exp(-vDist * vDist * 875.0);
                vertexCore += coreGlow * activation * pulse * 0.4;
            }

            float outerHaze = exp(-vDist * vDist * 15.0);  // Scaled for 2x
            vertexGlow += outerHaze * activation * pulse * 0.6;
        }
    }

    // Normalize wheel color
    if (vertexWheelAccum > 0.01) {
        vertexWheelColor /= vertexWheelAccum;
    }

    // =================================================================
    // LAYER 4: EDGE LATTICE - 30 pulsing crystalline edges
    // =================================================================

    float edgePulse = u_dodeca_edge_pulse;
    float edgeGlowU = u_dodeca_edge_glow;

    // 12-fold seam edges (thinner)
    float seamDist = min(abs(foldedTheta), foldAngle - abs(foldedTheta));
    float edgeWidth = 0.006 + edgePulse * 0.004;  // Thinner edges
    float seamCore = exp(-seamDist * seamDist / (edgeWidth * edgeWidth * 0.2));
    float seamGlow = exp(-seamDist * seamDist / (edgeWidth * edgeWidth * 1.5));

    // Radial edges connecting vertex rings (thinner)
    float radialCore = 0.0;
    float radialGlow = 0.0;
    for (int i = 0; i < 10; i++) {
        float rAngle = float(i) * TAU / 10.0 + rot;
        float angleDist = abs(mod(theta - rAngle + PI, TAU) - PI);

        float rCore = exp(-angleDist * angleDist * 1600.0);  // Thinner
        float rGlow = exp(-angleDist * angleDist * 400.0);   // Thinner

        // Mask to vertex ring region (scaled 1.6x)
        float radialMask = smoothstep(vRadius1 - 0.032, vRadius1, uvLen) *
                           smoothstep(vRadius2 + 0.032, vRadius2, uvLen);

        radialCore += rCore * radialMask;
        radialGlow += rGlow * radialMask;
    }

    float totalEdgeCore = (seamCore * 0.5 + radialCore * 0.5) * (0.4 + edgePulse * 0.6);
    float totalEdgeGlow = (seamGlow * 0.5 + radialGlow * 0.5) * (0.3 + edgeGlowU * 0.7);

    // =================================================================
    // LAYER 5: PULSING SHOCKWAVES - Energy ripples from center
    // =================================================================

    float shockwaveAccum = 0.0;
    if (u_quality_tier >= 1) {
        for (int w = 0; w < 3; w++) {
            float waveSpeed = 0.12 + float(w) * 0.06;
            float waveOffset = float(w) * 0.33;
            float wavePhase = fract(flowTime * waveSpeed + waveOffset + u_dodeca_edge_pulse * 0.5);

            // Wave expands from center to edge
            float waveRadius = wavePhase * 1.1;  // Scaled up
            float ringDist = abs(uvLen - waveRadius);
            float ringWidth = 0.01 + wavePhase * 0.02;

            float ringMask = smoothstep(ringWidth, ringWidth * 0.2, ringDist);
            float fadeFactor = 1.0 - wavePhase * wavePhase;

            // Pentagon modulation - wave is stronger along pentagon edges
            float pentMod = 1.0 + 0.3 * sin(rotTheta * 5.0);

            shockwaveAccum += ringMask * fadeFactor * pentMod * (0.6 - float(w) * 0.15);
        }
        shockwaveAccum = min(shockwaveAccum, 1.2);
    }

    // =================================================================
    // LAYER 6: GOLDEN RATIO PARTICLES - Phi-spiral flowing particles
    // =================================================================

    float particleAccum = 0.0;
    vec3 particleColor = warmWhite;

    if (u_quality_tier >= 1 && u_dodeca_phi > 0.1) {
        int particleCount = (u_quality_tier >= 2) ? 60 : 30;

        float nearestDist = 100.0;
        for (int i = 0; i < 60; i++) {
            if (i >= particleCount) break;

            float pf = float(i);
            float phash = fract(sin(pf * 127.1 + 311.7) * 43758.5453);
            float phash2 = fract(sin(pf * 269.5 + 183.3) * 43758.5453);

            // Golden angle distribution
            float goldenAngle = pf * TAU * INV_PHI + rot;

            // Radius follows phi-spiral
            float lifeSpeed = 0.15 + phash * 0.1;
            float life = fract(flowTime * lifeSpeed + phash2);
            float pRadius = 0.08 + life * 0.8;  // Scaled up

            // Spiral toward center
            float spiralWrap = 1.5;
            float pAngle = goldenAngle + life * spiralWrap * TAU;

            vec2 pPos = vec2(cos(pAngle), sin(pAngle)) * pRadius;
            float pDist = length(p - pPos);

            float pSize = 0.008 + (1.0 - life) * 0.012;
            if (pDist < nearestDist) {
                nearestDist = pDist;
            }
        }

        if (nearestDist < 0.03) {
            float pGlow = exp(-pow(nearestDist / 0.01, 2.0) * 2.0);
            particleAccum = pGlow * u_dodeca_phi;
        }
    }

    // =================================================================
    // LAYER 7: CENTRAL RESONANT VOID - The harmonic center
    // =================================================================

    float voidGlow = 0.0;
    float voidRadius = 0.12;  // Scaled up
    if (uvLen < voidRadius * 2.0) {
        float voidProx = 1.0 - smoothstep(0.0, voidRadius, uvLen);
        voidGlow = pow(voidProx, 1.5);

        // Pulsing with phi resonance
        voidGlow *= 0.6 + 0.4 * sin(flowTime * TAU * 0.3);

        // Pentagon modulation
        float pentVoid = 1.0 + 0.2 * sin(rotTheta * 5.0);
        voidGlow *= pentVoid;
    }

    // =================================================================
    // LAYER 8: GOLDEN SPIRAL OVERLAY (phi resonance emphasis)
    // =================================================================

    float goldenSpiral = 0.0;
    if (u_dodeca_phi > 0.15) {
        float spiralR = uvLen * 8.0;
        float spiralTheta = log(max(spiralR, 0.01)) / log(PHI);
        float spiralPhase = mod(rotTheta - spiralTheta + flowTime * 0.15, TAU);

        // Multiple spiral arms
        float arm1 = pow(sin(spiralPhase * 5.0) * 0.5 + 0.5, 3.0);
        float arm2 = pow(sin(spiralPhase * 5.0 + PI) * 0.5 + 0.5, 3.0);

        goldenSpiral = (arm1 + arm2 * 0.5) * smoothstep(0.12, 0.22, uvLen) * smoothstep(0.95, 0.55, uvLen);  // Scaled up
        goldenSpiral *= u_dodeca_phi * 0.4;
    }

    // =================================================================
    // LAYER 9: 24-FOLD GATE ALIGNMENT
    // =================================================================

    float lockStrength = u_dodeca_face_lock;
    float lock24Angle = TAU / 24.0;
    float lock24Theta = mod(rotTheta + lock24Angle * 0.5, lock24Angle) - lock24Angle * 0.5;
    float lock24Core = exp(-lock24Theta * lock24Theta * 1200.0) * lockStrength * 0.5;
    float lock24Glow = exp(-lock24Theta * lock24Theta * 300.0) * lockStrength * 0.3;

    // =================================================================
    // LAYER 10: FRESNEL RIM - Crystalline edge brightening
    // =================================================================

    float fresnelRim = 0.0;
    if (u_quality_tier >= 2) {
        // Edge detection via gradient
        float gradMag = length(vec2(dFdx(uvLen), dFdy(uvLen)));
        float pentGrad = length(vec2(dFdx(pentagonAccum), dFdy(pentagonAccum)));

        fresnelRim = smoothstep(0.0, 0.015, gradMag + pentGrad * 0.5);
        fresnelRim *= smoothstep(0.65, 0.4, uvLen);  // Fade toward edges
        fresnelRim *= (0.3 + u_dodeca_edge_pulse * 0.7);
    }

    // =================================================================
    // COMPOSITING (BALANCED BRIGHTNESS)
    // =================================================================

    vec3 col = baseCol;
    float intensity = 0.0;
    vec3 colorAccum = vec3(0.0);

    // Subtle energy-reactive brightness
    float energyBoost = 1.0 + u_energy * 0.3;

    // Cymatic interference (atmospheric base layer)
    intensity += cymaticAccum * 0.6;
    colorAccum += cymaticColor * cymaticAccum * 0.9 + crystalTint * cymaticAccum * 0.3;

    // Pentagon shells (primary structure)
    intensity += pentagonAccum * 1.0;
    colorAccum += primaryColor * pentagonAccum * 0.9 + crystalTint * pentagonAccum * 0.4;

    // Shell glow
    intensity += shellGlow * 0.5;
    colorAccum += secondaryColor * shellGlow * 0.8;

    // Edge lattice (hot core + glow)
    intensity += totalEdgeCore * 1.5 + totalEdgeGlow * 0.6;
    colorAccum += warmWhite * totalEdgeCore * 1.0 + crystalTint * totalEdgeCore * 0.5;
    colorAccum += secondaryColor * totalEdgeGlow * 0.5 + crystalTint * totalEdgeGlow * 0.4;

    // Mini QS wheels at vertices - video-dominant, structure as accent
    intensity += vertexWheelAccum * 1.0;
    colorAccum += vertexWheelColor * vertexWheelAccum * 1.0;

    // Vertex cores and glow (subdued to let video read)
    intensity += vertexCore * 0.8 + vertexGlow * 0.5;
    colorAccum += warmWhite * vertexCore * 0.6;
    colorAccum += crystalTint * vertexGlow * 0.4 + primaryColor * vertexGlow * 0.3;

    // Shockwaves
    intensity += shockwaveAccum * 0.7;
    vec3 shockColor = mix(primaryColor, warmWhite, 0.6);
    colorAccum += shockColor * shockwaveAccum * 1.0;

    // Golden particles
    intensity += particleAccum * 1.0;
    colorAccum += particleColor * particleAccum * 1.2;

    // Central void
    intensity += voidGlow * 0.8;
    colorAccum += phiColor * voidGlow * 0.9 + crystalTint * voidGlow * 0.4;

    // Golden spiral
    intensity += goldenSpiral * 0.6;
    colorAccum += phiColor * goldenSpiral * 0.9;

    // 24-fold alignment
    intensity += lock24Core * 0.8 + lock24Glow * 0.4;
    colorAccum += crystalTint * (lock24Core + lock24Glow) * 1.0;

    // Fresnel rim
    intensity += fresnelRim * 0.5;
    colorAccum += crystalTint * fresnelRim * 0.8;

    // Dominant face highlight
    if (faceIdx == u_dodeca_dominant_face) {
        intensity += faceActivation * 0.25;
        colorAccum += primaryColor * faceActivation * 0.3;
    }

    // Apply energy boost
    intensity *= energyBoost;
    colorAccum *= energyBoost;

    // === NORMALIZE COLOR ===
    vec3 dodecaColor = colorAccum / max(intensity, 0.01);
    dodecaColor = mix(dodecaColor, crystalTint, 0.1);

    // === OUTER FADE (extended for larger dodeca) ===
    float outerFade = 1.0 - smoothstep(0.9, 1.25, uvLen);
    intensity *= outerFade;

    // === TOPOLOGY CROSSFADE ===
    float topoBlend = 1.0;
    if (u_topology_id == TOPO_DODECAHEDRAL && u_topology_xfade_t < 1.0) {
        topoBlend = u_topology_xfade_t;
    } else if (u_topology_prev_id == TOPO_DODECAHEDRAL && u_topology_xfade_t < 1.0) {
        topoBlend = 1.0 - u_topology_xfade_t;
    }

    // === TRANSITION ALPHA ===
    float transitionAlpha = smoothstep(0.0, 0.3, dt) * (1.0 - smoothstep(0.9, 1.0, 1.0 - dt));
    float finalAlpha = transitionAlpha * topoBlend * min(intensity, 2.5);

    // === FINAL BLEND ===
    col = mix(col, col + dodecaColor * intensity * 1.0, finalAlpha);

    // Additive glow
    col += dodecaColor * finalAlpha * 0.4;

    // Emission highlights (structural only, no wheel contribution to avoid washing out video)
    float emission = vertexCore * 0.6 + edgePulse * 0.4 + voidGlow * 0.3;
    col += warmWhite * emission * finalAlpha * 0.3;

    return col;
}

// ============================================================================
// VORTEX TOPOLOGY - Spiral Flow Convergent/Divergent Pattern
// ============================================================================
//
// Vortex is a sibling to TorusKnot, representing spiral flow through the
// 24-fold geometry. It creates mesmerizing spiral arms that either converge
// toward the center (Inward) or diverge outward (Outward).
//
// Key visual elements:
//   1. 24-fold gate alignment (from u_vortex_gate_center)
//   2. Spiral arms locked to N-fold symmetry
//   3. Pressure-driven intensity (from u_vortex_pressure_env)
//   4. Direction-dependent flow animation
//   5. Shell bands at different radii
// ============================================================================

vec3 renderVortex(vec2 uv, vec3 baseCol, vec2 uv_screen) {
    // Early exit if vortex not active
    if (u_vortex_active == 0) return baseCol;

    // Transition progress
    float vt = u_vortex_t;
    if (vt < 0.01) return baseCol;

    // === COORDINATE SETUP (ASPECT-CORRECTED FOR TRUE CIRCLES) ===
    // Create aspect-corrected coordinate so radius tests produce circles, not ellipses.
    // All radial distance tests must use this metric.
    float aspect = u_resolution.x / u_resolution.y;
    vec2 p = uv;
    p.x *= aspect;  // Correct for widescreen: 1 unit in x == 1 unit in y
    float uvLen = length(p);  // True circular radius

    // Early culling - vortex is contained within screen bounds
    if (uvLen > 1.2 * aspect) return baseCol;

    // Theta uses original uv for correct angular math (seams/gates unchanged)
    float theta = atan(uv.y, uv.x);

    // === GATE ALIGNMENT ===
    // Gate center in radians (24-fold: each gate is 15°)
    float gateAngle = float(u_vortex_gate_center) * TAU / 24.0;
    float bulgePhaseAngle = float(u_vortex_bulge_phase_q) * TAU / 24.0;

    // Offset theta by gate angle so vortex is oriented to current gate
    float alignedTheta = theta - gateAngle;

    // === 24-FOLD SYMMETRY ===
    float foldAngle = TAU / 24.0;  // 15 degrees
    float foldedTheta = mod(alignedTheta + foldAngle * 0.5, foldAngle) - foldAngle * 0.5;

    // ========================================================================
    // 3D TORUS ILLUSION - Fake volumetric depth without real geometry
    // ========================================================================
    // Map screen coords to torus parameterization:
    //   u_torus (toroidal): angle around the torus ring
    //   v_torus (poloidal): angle around the tube cross-section

    // Torus major/minor radii (normalized)
    float R_major = 0.45;  // Major radius (ring)
    float R_minor = 0.15 + u_vortex_depth * 0.1;  // Minor radius (tube thickness)

    // Map uvLen to distance from torus center line
    float dist_from_ring = abs(uvLen - R_major);

    // Approximate poloidal angle (around tube cross-section)
    // Front = 0, Back = PI (based on whether pixel is inside or outside torus ring)
    float v_torus = 0.0;
    if (uvLen < R_major) {
        // Inside the ring - map to front half of tube
        v_torus = acos(clamp(dist_from_ring / R_minor, -1.0, 1.0));
    } else {
        // Outside the ring - map to back half of tube
        v_torus = PI - acos(clamp(dist_from_ring / R_minor, -1.0, 1.0));
    }

    // Tube surface proximity (0 = on surface, 1 = far from surface)
    float tubeSurfaceDist = max(0.0, dist_from_ring - R_minor) / R_minor;
    float onTubeSurface = 1.0 - smoothstep(0.0, 0.5, tubeSurfaceDist);

    // === 3D CUE 1: FRONT/BACK ATTENUATION ===
    // Dim the "back" side of the tube based on poloidal angle
    float frontBackFactor = cos(v_torus) * 0.5 + 0.5;  // 1 = front, 0 = back
    float backDim = mix(1.0, frontBackFactor, u_vortex_occlusion * u_vortex_depth);

    // === 3D CUE 2: RIM LIGHTING ===
    // Brighten edges where view grazes the surface (Fresnel-like)
    float rimAngle = abs(sin(v_torus));  // 1 at edges, 0 at front/back
    float rimLight = pow(rimAngle, 2.0) * u_vortex_depth * 0.6;

    // === 3D CUE 3: TUBE SELF-SHADOW (cheap AO) ===
    // Darken near the inner radius (tube shadow)
    float innerAO = smoothstep(R_major - R_minor * 1.5, R_major - R_minor * 0.5, uvLen);
    float tubeAO = mix(0.6, 1.0, innerAO) * onTubeSurface + (1.0 - onTubeSurface);

    // === 3D CUE 4: DEPTH-BASED FADE ===
    // Objects on back side slightly desaturated/dimmed
    float depthFade = mix(1.0, 0.7 + 0.3 * frontBackFactor, u_vortex_depth);

    // ========================================================================
    // DRAIN DEFORMATION - QS wheel bends into center
    // ========================================================================
    // QS WHEEL DRAIN WITH SPRING ANIMATION
    // ========================================================================
    // u_vortex_qs_drain_pull is spring-animated:
    //   - Ease IN slowly when vortex activates (pull toward drain)
    //   - Snap OUT fast when vortex deactivates
    //   - Ripple/oscillate as it settles (negative values = overshoot outward)

    // Sink profile: strongest at sink_radius, falls off on either side
    float sinkRadius = u_vortex_sink_radius;
    float sinkProfile = exp(-pow((uvLen - sinkRadius) / 0.15, 2.0));

    // 24-fold seam mask: peaks at gate_center and neighboring seams
    // Only allow deformation along seam lines (preserves angular symmetry)
    // GATE-DEPENDENT: Multiply by u_gate_ok to fade when ambiguous
    float seamMaskWidth = foldAngle * 0.4;  // 40% of fold angle
    float seamMaskRaw = exp(-foldedTheta * foldedTheta / (seamMaskWidth * seamMaskWidth));
    float seamMask = seamMaskRaw * u_gate_ok;  // Fades to uniform when gate ambiguous

    // Spring-animated drain pull (main driver)
    // Positive = pulled in, negative = overshoot outward (ripple)
    float springPull = u_vortex_qs_drain_pull;

    // Ripple effect: when spring overshoots (negative), create radial wave
    float rippleWave = 0.0;
    if (springPull < 0.0) {
        // Overshoot creates expanding ripple rings
        float ripplePhase = uvLen * 12.0 + springPull * 8.0;
        rippleWave = sin(ripplePhase) * (-springPull) * 0.3;
        rippleWave *= smoothstep(0.8, 0.3, uvLen);  // Fade at edges
    }

    // Combined drain displacement
    // Base from spring animation, plus onset pulses for "gulp" effect
    float drainStrength = max(0.0, springPull) * sinkProfile * seamMask;
    drainStrength += u_vortex_sink_env * sinkProfile * 0.3;  // Beat pulses

    // Apply drain: pull coordinates inward toward center
    float drainedLen = uvLen - drainStrength * 0.2;  // Increased effect
    drainedLen += rippleWave * 0.05;  // Add ripple displacement
    drainedLen = max(drainedLen, 0.02);  // Don't collapse to singularity

    // Drain also tilts the "membrane" creating depth illusion
    float drainTilt = drainStrength * u_vortex_depth;
    float drainDarken = 1.0 - drainTilt * 0.3;  // Pulled areas are darker (deeper)

    // Ripple brightening (subtle flash on outward ripple)
    float rippleBright = 1.0 + abs(rippleWave) * 2.0;

    // ========================================================================
    // SPIRAL FLOW (using drained coordinates)
    // ========================================================================
    // Direction: 0 = Inward (CW spiral toward center), 1 = Outward (CCW expanding)
    float dirSign = (u_vortex_direction == VORTEX_DIR_INWARD) ? 1.0 : -1.0;

    // Flow animation speed (pressure-modulated)
    float flowSpeed = 0.8 + u_vortex_pressure_env * 1.2;
    float flowTime = u_time * flowSpeed * dirSign;

    // === SPIRAL ARM PATTERN ===
    // Use drained radius for spiral calculation (arms curve into drain)
    float numArms = 4.0;
    float spiralTightness = 3.0 + u_vortex_pressure_env * 2.0;

    float spiralPhase = drainedLen * spiralTightness - alignedTheta * numArms + flowTime * 2.0;
    float spiralWave = sin(spiralPhase) * 0.5 + 0.5;
    spiralWave = pow(spiralWave, 2.0);

    // Secondary spiral (counter-rotating for depth)
    float spiral2Phase = drainedLen * spiralTightness * 0.7 + alignedTheta * numArms * 0.5 - flowTime * 1.5;
    float spiral2Wave = sin(spiral2Phase) * 0.5 + 0.5;
    spiral2Wave = pow(spiral2Wave, 3.0) * 0.4;

    // Spiral arms wrap around tube (3D effect)
    float spiralOnTube = spiralWave * onTubeSurface * backDim;
    float spiral2OnTube = spiral2Wave * onTubeSurface * backDim * 0.8;

    // === RADIAL SHELLS (Grant Triangle Nine Means) ===
    // Shells at harmonic radii from Grant Projection Theorem
    // Each shell radius is a Generative Mean from the Pythagorean triple
    float grantShellAccum = 0.0;
    const float SHELL_WIDTH = 0.015;  // Shell band width (normalized)
    for (int i = 0; i < 9; i++) {
        if (i >= u_grant_shell_count) break;
        float shellR = u_grant_shell_radii[i] * 0.5;  // Scale to 0..0.5 range
        float dist = abs(drainedLen - shellR);
        float shellBand = exp(-dist * dist / (SHELL_WIDTH * SHELL_WIDTH));
        // Alternate intensity for visual depth
        float intensity = (i % 2 == 0) ? 0.4 : 0.25;
        // Shells animate with flow
        float phase = flowTime * 0.3 + float(i) * 0.7;
        float pulse = 0.8 + 0.2 * sin(phase);
        grantShellAccum += shellBand * intensity * pulse;
    }
    float shellWave = grantShellAccum * onTubeSurface * backDim;

    // === CENTER VORTEX HOLE (the drain) ===
    // Inner radius is exactly 20% of outer disc radius for clean "throat" look.
    // Outer disc edge is at ~0.55, so inner = 0.55 * 0.20 = 0.11
    const float OUTER_DISC_RADIUS = 0.55;
    const float INNER_RATIO = 0.20;
    float holeRadius = OUTER_DISC_RADIUS * INNER_RATIO;  // Fixed 20% ratio
    float holeMask = smoothstep(holeRadius, holeRadius * 1.5, drainedLen);

    // Drain glow intensifies with sink strength (centered at inner radius)
    float drainGlowRadius = holeRadius * 0.8;  // Glow peaks just inside hole edge
    float drainGlow = exp(-drainedLen * drainedLen / (drainGlowRadius * drainGlowRadius * 2.0));
    drainGlow *= (u_vortex_pressure_env + u_vortex_sink_strength * 0.5 + u_vortex_sink_env);

    // Outer fade - starts at disc edge, fades to full at 1.5x disc radius
    float outerFade = 1.0 - smoothstep(OUTER_DISC_RADIUS, OUTER_DISC_RADIUS * 1.6, uvLen);

    // === 24-FOLD SEAM HIGHLIGHTS (with specular on high pressure) ===
    // GATE-DEPENDENT: Seam highlights fade when gate is ambiguous
    float seamDist = min(abs(foldedTheta), foldAngle - abs(foldedTheta));
    float seamWidth = 0.02 + u_vortex_pressure_env * 0.01;
    float seamGlowRaw = exp(-seamDist * seamDist / (seamWidth * seamWidth)) * 0.4;
    float seamGlow = seamGlowRaw * u_gate_ok;  // Fades to uniform when gate ambiguous

    // Specular streaks only when pressure is high AND gate confident
    float specularMask = smoothstep(0.5, 0.8, u_vortex_pressure_env) * u_gate_ok;
    float specStreak = seamGlow * rimLight * specularMask * 2.0;

    // === CIRCULATION STEP INDICATOR ===
    // GATE-DEPENDENT: Circulation flash fades when gate is ambiguous
    float circFlash = 0.0;
    if (u_vortex_circulation_step != 0 && u_gate_ok > 0.01) {
        float circDir = float(u_vortex_circulation_step);
        float circAngle = gateAngle + circDir * foldAngle;
        float circDist = abs(mod(theta - circAngle + PI, TAU) - PI);
        circFlash = exp(-circDist * circDist * 100.0) * 0.5 * smoothstep(0.2, 0.4, uvLen) * u_gate_ok;
    }

    // === COLOR ===
    float hue = float(u_vortex_pitch_q) / 24.0;
    vec3 primaryColor = hue2rgb(hue);
    vec3 secondaryColor = hue2rgb(fract(hue + 0.33));

    vec3 dirColor = (u_vortex_direction == VORTEX_DIR_INWARD)
        ? vec3(0.2, 0.5, 1.0)
        : vec3(1.0, 0.5, 0.2);

    vec3 vortexColor = mix(primaryColor, dirColor, 0.3);
    vec3 seamColor = mix(secondaryColor, vec3(1.0), 0.3);
    vec3 rimColor = vec3(0.8, 0.9, 1.0);  // Slight blue rim

    // === COMBINE LAYERS WITH 3D MODULATION ===
    float intensity = 0.0;

    // Spiral arms (main visual) - modulated by 3D factors
    intensity += spiralOnTube * holeMask * outerFade * 0.8;
    intensity += spiral2OnTube * holeMask * outerFade;

    // Shell bands
    intensity += shellWave * holeMask * outerFade;

    // Gate seams
    intensity += seamGlow * holeMask * backDim;

    // Circulation flash
    intensity += circFlash;

    // Apply tube AO and drain darkening
    intensity *= tubeAO * drainDarken * depthFade;

    // Modulate by pressure envelope
    intensity *= 0.3 + u_vortex_pressure_env * 0.7;

    // Apply ripple brightness (flash on snap-back)
    intensity *= rippleBright;

    // Drain glow (center)
    float holeIntensity = drainGlow * 1.5;

    // === TRANSITION BLEND ===
    float transitionAlpha = smoothstep(0.0, 0.3, vt) * (1.0 - smoothstep(0.9, 1.0, 1.0 - vt));

    // === FINAL COLOR ===
    vec3 vortexContrib = vortexColor * intensity + seamColor * seamGlow * backDim;
    vortexContrib += dirColor * holeIntensity;

    // Add rim lighting
    vortexContrib += rimColor * rimLight * intensity * 0.5;

    // Add specular streaks
    vortexContrib += vec3(1.0) * specStreak * 0.3;

    // Add emission glow
    float emission = intensity * 0.5 + holeIntensity * 0.8;
    vortexContrib += vortexContrib * emission * 0.3;

    // === SILHOUETTE OUTLINE (strengthens with pressure) ===
    // Thin dark line at tube edge
    float silhouetteDist = abs(tubeSurfaceDist - 0.5);
    float silhouette = smoothstep(0.05, 0.0, silhouetteDist) * u_vortex_depth * u_vortex_pressure_env * 0.2;

    // === TOPOLOGY CROSSFADE ===
    float topoBlend = 1.0;
    if (u_topology_id == TOPO_VORTEX && u_topology_xfade_t < 1.0) {
        topoBlend = u_topology_xfade_t;
    } else if (u_topology_prev_id == TOPO_VORTEX && u_topology_xfade_t < 1.0) {
        topoBlend = 1.0 - u_topology_xfade_t;
    }

    float finalAlpha = transitionAlpha * topoBlend;

    // Blend with base color
    vec3 result = mix(baseCol, baseCol + vortexContrib, finalAlpha);

    // Additive for bright areas
    result += vortexContrib * finalAlpha * 0.3;

    // Subtle silhouette darkening
    result *= 1.0 - silhouette;

    return result;
}

// ============ MAIN ============

void main() {
    // === EARLY EXIT: Skip expensive raymarching when no audio ===
    // GPU OPTIMIZATION: Prevents 100% GPU usage when no audio is playing
    //
    // Case 1: Zero/negligible audio energy - immediate exit (startup, paused, etc.)
    // Case 2: Silence state fully active - smooth black transition
    if (u_section_energy < 0.005 && u_energy < 0.01) {
        // Truly no audio - output black immediately
        frag_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }
    if (u_silence_active == 1 && u_silence_blackout > 0.95) {
        // Silence state active and faded to black
        frag_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    // Apply TAA jitter for temporal super-resolution
    // Jitter is in screen-space [-0.5/w, 0.5/w] range
    vec2 jittered_coord = gl_FragCoord.xy + u_taa_jitter * u_resolution;
    vec2 uv = (jittered_coord - 0.5 * u_resolution) / u_resolution.y;
    vec2 uv_screen = gl_FragCoord.xy / u_resolution;  // [0,1] for post-processing (unjittered)

    // ==========================================================
    // DYNAMIC CAMERA SYSTEM
    // ==========================================================
    // RULE: macro camera = section_energy (slow), micro camera = tiny impulse only
    // NO instant energy in camera - it causes flopping

    // === SILENCE GATE: Stop ALL animation when no audio ===
    float silence_gate = smoothstep(0.02, 0.15, u_energy);

    // 1. Dynamic orbit (slow, smooth - orbit_speed set from CPU with section_energy)
    // GATED: orbit_speed is already gated on CPU, but gate noise here too
    float cam_angle = u_cam_angle + u_time * u_cam_orbit_speed;
    // Minimal rotational noise - GATED by silence
    cam_angle += sin(u_time * 0.3) * u_cam_noise_rot * 0.3 * silence_gate;

    // 2. Tilt from dynamic parameter (NO wobble - wobble causes flopping)
    // Stability-driven wobble removed - it made things worse when unstable
    float tilt_angle = u_cam_tilt;

    // 3. Distance - ONLY from CPU (section_energy-driven), NO instant energy
    float dist = u_cam_dyn_dist;  // Already smoothed on CPU with slow tau (~2s)
    // Very subtle slow breathing - GATED by silence
    dist += sin(u_time * TAU * 0.02) * 0.03 * silence_gate;
    // NO u_energy here - instant energy causes distance jumping

    // Inside-dip: smooth journey into the field (already smoothed on CPU)
    dist -= u_cam_inside_dip * 1.2;

    // Clamp to safe range
    dist = clamp(dist, 0.4, 5.0);

    // === WARP JUMP QS CAMERA TRANSFORM ===
    // During warp transition, camera pushes toward portal (QS wheel center)
    float warp_z_offset = 0.0;
    if (u_warp_active == 1) {
        // Camera Z offset moves camera forward through the portal
        warp_z_offset = u_warp_camera_z;
    }

    // 4. Camera position (minimal noise - GATED by silence)
    vec3 ro = vec3(
        sin(cam_angle) * dist,
        u_cam_height + sin(u_time * 0.4) * u_cam_noise_pos * 0.3 * silence_gate,
        cos(cam_angle) * dist
    );

    // Apply warp Z offset - moves camera forward/back along view axis
    if (u_warp_active == 1) {
        // Move camera along its forward direction (toward origin/portal)
        vec3 warp_forward = normalize(-ro);
        ro += warp_forward * warp_z_offset;
    }

    // 5. Aim offset (use section_energy not instant energy)
    // Aim offset computed on CPU from dominant pitch (already latched and smooth)
    vec2 aim = u_cam_aim_offset * (0.8 + 0.2 * u_section_energy);  // section_energy, not u_energy
    vec3 target = vec3(aim.x, aim.y * 0.4, 0.0);  // No wobble_y

    vec3 forward = normalize(target - ro);
    vec3 right = normalize(cross(vec3(0, 1, 0), forward));
    vec3 up = cross(forward, right);

    // Apply tilt to the view
    forward = rotateX(tilt_angle) * forward;
    up = rotateX(tilt_angle) * up;

    // ==========================================================
    // PORTAL FPS CAMERA MODE (InsideView)
    // ==========================================================
    // When in portal dive, override orbit camera with corridor frame
    // Camera moves along locked wedge axis, lane offset perpendicular
    float portal_fov = u_cam_fov;  // Default FOV
    if (u_cam_in_portal == 1 && u_portal_fps_active == 1) {
        // === CORRIDOR RADIUS FROM TIER ===
        float corridor_radius = CORRIDOR_RADIUS_NORMAL;
        if (u_portal_fps_corridor_tier == 0) corridor_radius = CORRIDOR_RADIUS_TIGHT;
        else if (u_portal_fps_corridor_tier == 2) corridor_radius = CORRIDOR_RADIUS_WIDE;

        // === LOCKED FRAME BASIS ===
        // Forward direction from locked azimuth (computed on entry, never changes)
        vec3 portal_forward = normalize(vec3(u_portal_fps_fwd_x, 0.0, u_portal_fps_fwd_z));
        vec3 portal_right = normalize(vec3(u_portal_fps_right_x, 0.0, u_portal_fps_right_z));
        vec3 portal_up = vec3(0.0, 1.0, 0.0);  // No roll, ever

        // === LANE OFFSET (discrete lateral position) ===
        // lane_idx: -2, -1, 0, +1, +2
        // lane_t smooths transitions between lanes
        float lane_offset = float(u_portal_fps_lane_idx) * PORTAL_LANE_SPACING;
        // Smooth lane transitions
        float lane_smooth = mix(
            float(u_portal_fps_lane_idx - int(sign(float(u_portal_fps_lane_idx)))),
            float(u_portal_fps_lane_idx),
            u_portal_fps_lane_t
        );
        lane_offset = lane_smooth * PORTAL_LANE_SPACING;

        // === PARALLAX SNAP (lane impulse) ===
        // Brief positional snap on lane change for visceral feel
        float parallax_snap = u_portal_fps_lane_impulse * float(u_portal_fps_lane_impulse_dir) * 0.06;
        lane_offset += parallax_snap;

        // === MICRO SWAY (tiny bounded oscillation) ===
        float sway_x = sin(u_portal_fps_sway_phase * TAU) * PORTAL_SWAY_AMP;
        float sway_y = cos(u_portal_fps_sway_phase * TAU * 1.3) * PORTAL_SWAY_AMP * 0.6;

        // === Z OFFSET (forward progress toward center) ===
        // z_offset is cumulative: as it grows, we move INWARD toward geometry
        float z_offset = u_portal_z_offset;

        // === TOPOLOGY-AWARE DIVE PATH ===
        // Different structures have different natural "through" paths
        float portal_start_dist = 2.8;
        float t_dive = clamp(z_offset / 2.0, 0.0, 1.0);  // Normalized dive progress [0..1]

        vec3 dive_pos;
        vec3 look_dir;

        if (u_topology_id == TOPO_TORUSKNOT) {
            // TORUS: Spiral through the donut hole (toroidal path)
            // Camera follows the major radius while spiraling along minor axis
            float torus_major = 1.2;  // Distance from center to torus ring center
            float torus_minor = 0.5;  // Thickness of torus tube
            float spiral_angle = t_dive * TAU * 0.75;  // 3/4 turn through torus
            float vertical_prog = sin(t_dive * PI);  // Rise and fall through hole

            // Position: orbit around torus center, rising through the hole
            float orbit_dist = torus_major + (1.0 - t_dive) * 1.5;  // Start outside, end at ring
            dive_pos = vec3(
                cos(spiral_angle) * orbit_dist,
                vertical_prog * torus_minor * 1.5,  // Rise through hole
                sin(spiral_angle) * orbit_dist
            );
            // Look toward center of torus
            look_dir = normalize(-dive_pos);

        } else if (u_topology_id == TOPO_VORTEX) {
            // VORTEX: Spiral down the central tunnel
            // Camera corkscrews inward like going down a drain
            float vortex_radius = mix(2.0, 0.3, t_dive);  // Shrink radius as we descend
            float vortex_angle = t_dive * TAU * 2.0;  // 2 full rotations
            float vortex_depth = t_dive * 2.5;  // How deep into vortex

            dive_pos = vec3(
                cos(vortex_angle) * vortex_radius,
                -vortex_depth * 0.5,  // Descend slightly
                sin(vortex_angle) * vortex_radius
            );
            // Look toward vortex center, slightly ahead of spiral
            float look_angle = vortex_angle + 0.5;
            look_dir = normalize(vec3(
                cos(look_angle) * vortex_radius * 0.3 - dive_pos.x,
                -0.2,  // Look slightly down into vortex
                sin(look_angle) * vortex_radius * 0.3 - dive_pos.z
            ));

        } else if (u_topology_id == TOPO_DODECAHEDRAL) {
            // DODECA: Float through the lattice framework
            // Camera weaves between the pentagonal faces
            float lattice_scale = 1.8;
            float weave_angle = t_dive * TAU * 0.6;  // Partial rotation
            float weave_amp = 0.4 * (1.0 - t_dive);  // Weaving decreases as we go in

            // Start outside, weave toward center
            float radial_dist = mix(2.5, 0.8, t_dive);
            dive_pos = vec3(
                cos(weave_angle) * radial_dist + sin(t_dive * PI * 3.0) * weave_amp,
                sin(t_dive * PI * 2.0) * weave_amp * 0.7,  // Vertical weave
                sin(weave_angle) * radial_dist + cos(t_dive * PI * 2.5) * weave_amp
            );
            look_dir = normalize(-dive_pos);

        } else {
            // SCAFFOLD24 / default: Linear inward motion (original behavior)
            float current_dist = max(portal_start_dist - z_offset, 0.5);
            dive_pos = -portal_forward * current_dist;
            look_dir = portal_forward;
        }

        // === FINAL CAMERA POSITION ===
        // Base position from topology path + lane offset + sway
        ro = dive_pos;
        ro += portal_right * lane_offset;            // Lateral offset for lane
        ro += portal_right * sway_x;                 // Micro sway X
        ro += portal_up * sway_y;                    // Micro sway Y

        // Update forward direction based on dive path
        forward = look_dir;

        // === CAMERA ORIENTATION ===
        // Look straight down corridor - NO yaw drift, NO roll
        forward = portal_forward;
        right = portal_right;
        up = portal_up;

        // === PORTAL FOV (wide for immersion) ===
        portal_fov = PORTAL_FOV / 100.0;  // Normalize to same scale as u_cam_fov

        // === ENTRY APERTURE EFFECT ===
        // Brief vignette/iris that opens on entry
        // (Applied in post-processing below)
    }

    // 6. Dynamic FOV from blended modes
    // === PORTAL / WARP FOV MODIFICATION ===
    float final_fov = u_cam_fov;
    if (u_cam_in_portal == 1 && u_portal_fps_active == 1) {
        // Portal mode: use wide FOV for immersion
        final_fov = portal_fov;
    } else if (u_warp_active == 1) {
        // Warp mode: zoom affects FOV (narrowing = tunnel vision toward portal)
        // u_warp_zoom > 1 = zoomed in (narrower FOV)
        final_fov = u_cam_fov / u_warp_zoom;
    }
    vec3 rd = normalize(forward * final_fov + uv.x * right + uv.y * up);

    // ==========================================================
    // BOUNDING SPHERE EARLY-OUT (REG math preserved)
    // ==========================================================
    // "If a tree falls in the forest..." - skip rays that can't hit geometry
    // Conservative bound: max possible harmonic radius (base + max deviation)
    float boundRadius = u_base_radius * 1.8 + u_gain * 0.5;

    // Only check when camera is outside the bounding sphere
    // (if camera is inside, most rays will hit anyway)
    if (length(ro) > boundRadius) {
        if (!rayHitsBoundingSphere(ro, rd, boundRadius)) {
            // Ray definitely misses geometry - render transparent background
            frag_color = vec4(0.0, 0.0, 0.0, 0.0);
            return;
        }
    }

    // ==========================================================
    // BACK-HEMISPHERE CULLING (REG-compliant optimization)
    // ==========================================================
    // During 24-fold → torus transition (fold24_t > 0.5), skip rays pointing at back
    // This is real geometry math: back hemisphere is self-occluded, skip expensive work
    // Only active when: high fold24_t, not inside, not warping, torus about to activate
    bool preTorusTransition = (u_fold24_t > 0.5 && u_interior_latch < 0.5 &&
                               u_warp_active == 0 && u_torus_active == 1);
    if (preTorusTransition && length(ro) > boundRadius * 0.8) {
        // Ray heading away from geometry center = looking at back hemisphere
        // normalize(-ro) points toward center, if dot with rd < 0, we're looking away
        vec3 toCenter = normalize(-ro);
        float facingCenter = dot(rd, toCenter);

        // Strong back-facing rays: skip entirely
        if (facingCenter < -0.3) {
            frag_color = vec4(0.02, 0.02, 0.04, 0.0);
            return;
        }
    }

    // ==========================================================
    // THE REG JOURNEY - Phase-driven rendering
    // ==========================================================
    float sdfAtCamera = sceneSDF(ro);
    bool cameraInside = sdfAtCamera < 0.0;

    // How deep inside the field (0 = surface, 1 = deep inside)
    float insideDepth = clamp(-sdfAtCamera / 0.3, 0.0, 1.0);

    // ==========================================================
    // INTERIOR STATE SEPARATION: Permission vs Visual
    //
    // PERMISSION FLAGS (for gating expensive code paths):
    //   u_interior_latch: 1 when interior_state.is_inside() (Entering or Inside)
    //   u_interior_hard: 1 only when truly Inside (most expensive features)
    //
    // VISUAL BLEND (for smooth crossfade, no pops):
    //   u_inside_mix: 0..1 derived from ring_dive.t or interior_state_time
    //
    // Rule: Use insideLatch/insideHard for branching, insideMix for opacity/intensity
    // ==========================================================
    float insideMix = u_inside_mix;                    // Visual blend weight
    bool insideLatch = u_interior_latch > 0.5;         // Permission: can run inside passes
    bool insideHard = u_interior_hard == 1;            // Permission: most expensive only

    // === TORUS KNOT STATE ===
    // When torus knot is active, render 24-fold exterior underneath
    // (performance optimizations make this viable now)
    bool torusActive = u_torus_active == 1 && u_torus_t > 0.3;

    // Surface opacity multiplier: fade surface as we go inside
    float surfaceOpacity = (1.0 - insideMix) * u_surface_opacity;

    // Interior blend: scales with inside_mix
    // Skip interior when torus active - exterior 24-fold + torus looks best
    float interiorBlend = torusActive ? 0.0 : (u_interior_enable * insideMix);
    if (cameraInside && !torusActive) {
        interiorBlend = max(interiorBlend, insideDepth * insideMix);
    }

    // Base color from chroma
    vec3 baseColor = chromaColor();

    vec3 col;
    float emission = 0.0;  // Track emission for bloom
    float alpha = 1.0;     // Track alpha for transparent background

    // Fade out geometry during silence
    float silence_fade = smoothstep(0.02, 0.1, u_energy);
    alpha *= silence_fade;

    // Raymarch variables (only computed when needed)
    float d = MAX_DIST;
    float steps_norm = 0.0;

    // ==========================================================
    // THE JOURNEY RENDERING - HARD LATCH VERSION
    // ==========================================================
    if (insideLatch && torusActive && u_torus_t > 0.3) {
        // === TORUS ACTIVE INSIDE: Skip 24-fold, simple dark background ===
        // Torus looks amazing, no need to render expensive 24-fold underneath
        col = vec3(0.02, 0.02, 0.04);
        alpha = 1.0;
        emission = 0.0;
    }
    else if (insideLatch && !torusActive) {
        // === HARD INSIDE: Video fragment mosaic ===
        // Large video tiles you can actually SEE, thin seams
        // The video IS the content, seams just frame it

        vec3 harmColor = chromaColor();

        // === LARGE VIDEO TILES ===
        // Much bigger cells so video is actually visible
        float sampleDist = 0.8;
        vec3 p = ro + rd * sampleDist;
        p = rotateY(u_rotation) * p;

        // LARGE cells - video needs to be readable (1.5 = ~3 cells visible)
        float cellSize = 1.5;
        vec3 cellId = floor(p / cellSize);
        vec3 cellLocal = fract(p / cellSize);
        float cellHash = hash31(cellId);

        // === VIDEO AS PRIMARY CONTENT ===
        vec3 videoCol = vec3(0.05);
        if (u_skin_has_texture != 0) {
            // Minimal rotation - keep video recognizable
            float rot = floor(cellHash * 4.0) * (TAU / 4.0);  // Only 0, 90, 180, 270
            mat2 R = mat2(cos(rot), -sin(rot), sin(rot), cos(rot));

            // UV fills most of the cell (video is the star)
            vec2 cellUV = cellLocal.xy;
            cellUV = (cellUV - 0.5) * R + 0.5;

            // Each cell shows different part of video
            vec2 uvOffset = vec2(fract(cellHash * 7.0), fract(cellHash * 13.0));
            cellUV = cellUV * 0.8 + uvOffset * 0.2;  // Less scramble, more coherent

            // Very slow drift
            cellUV += u_time * vec2(0.005, 0.003);
            cellUV = fract(cellUV);

            videoCol = texture(u_skin_tex, cellUV).rgb;

            // LIGHT tone-map - keep video visible!
            videoCol = pow(videoCol, vec3(1.1));  // Subtle gamma
            videoCol = (videoCol - 0.5) * 1.3 + 0.5;  // Light contrast
            videoCol = clamp(videoCol, 0.02, 1.0);  // Allow bright
        }

        // === VERY THIN SEAMS (subtle frame only) ===
        vec3 edgeDist3 = min(cellLocal, 1.0 - cellLocal);
        float edgeDist = min(min(edgeDist3.x, edgeDist3.y), edgeDist3.z);

        // Hair-thin seams
        float seamWidth = 0.02;
        float seam = smoothstep(seamWidth, 0.002, edgeDist);

        // Seam: tinted, not white
        vec3 seamColor = harmColor * 1.2;

        // === 12-FOLD SPOKES (very subtle) ===
        float theta = atan(p.z, p.x);
        float spokeAngle = mod(theta + PI, TAU / 12.0);
        float spokeDist = min(spokeAngle, TAU / 12.0 - spokeAngle);
        float spoke = smoothstep(0.015, 0.002, spokeDist);

        // === COMBINE: Video is KING, seams are hints ===
        col = videoCol;

        // Seams: subtle tinted edge, low opacity
        col = mix(col, col + seamColor * 0.3, seam * 0.5);

        // Spokes: barely there (skip at Low quality)
        if (u_inside_quality_tier >= 1) {
            col = mix(col, col + harmColor * 0.2, spoke * 0.3);
        }

        // === BACK LAYER (depth, dimmer) - Med+ quality only ===
        // Skip at Low quality to reduce texture samples and overdraw
        if (u_inside_quality_tier >= 1 && u_skin_has_texture != 0) {
            float sampleDist2 = 2.5;
            vec3 p2 = ro + rd * sampleDist2;
            p2 = rotateY(u_rotation) * p2;

            vec3 cellId2 = floor(p2 / cellSize);
            vec3 cellLocal2 = fract(p2 / cellSize);
            float cellHash2 = hash31(cellId2);

            float rot2 = floor(cellHash2 * 4.0) * (TAU / 4.0);
            mat2 R2 = mat2(cos(rot2), -sin(rot2), sin(rot2), cos(rot2));
            vec2 cellUV2 = (cellLocal2.xy - 0.5) * R2 + 0.5;
            vec2 uvOffset2 = vec2(fract(cellHash2 * 11.0), fract(cellHash2 * 19.0));
            cellUV2 = cellUV2 * 0.8 + uvOffset2 * 0.2;
            cellUV2 += u_time * vec2(0.003, 0.002);
            cellUV2 = fract(cellUV2);

            vec3 videoCol2 = texture(u_skin_tex, cellUV2).rgb;
            videoCol2 = videoCol2 * videoCol2 * 0.5;  // Cheaper gamma + dim

            // Blend back layer behind (depth)
            col = col * 0.8 + videoCol2 * 0.2;
        }

        // === ENERGY PULSE (NO KICK - we're inside, use energy only) ===
        float pulse = 1.0 + u_energy * 0.2;
        col *= pulse;

        // Soft bloom on video (skip at Low quality)
        if (u_inside_quality_tier >= 1) {
            float lum = dot(col, vec3(0.299, 0.587, 0.114));
            col += col * smoothstep(0.4, 0.8, lum) * 0.3;
        }

        alpha = 1.0;
        emission = 0.3 + u_energy * 0.2;
    }
    else if (interiorBlend > 0.01) {
        // === SOFT INTERIOR: Transitioning in/out ===
        // PERF: Skip expensive surface blending - interior is the hero
        vec4 interior = renderInterior(ro, rd, 2.0, interiorBlend);

        col = interior.rgb;
        alpha = interior.a;
        emission = interior.a * 0.5;

        // Simple background blend instead of expensive surface raymarch
        // Uses baseColor tinted dark - much cheaper than full lighting
        vec3 bgTint = baseColor * 0.15;
        float bgBlend = (1.0 - interior.a) * 0.3;
        col = col + bgTint * bgBlend;
        alpha = min(alpha + bgBlend, 1.0);
    }
    // ==========================================================
    // SURFACE RENDERING: Outside the geometry
    // ==========================================================
    else {
        // === TORUS ACTIVE SKIP ===
        // When torus is active and visible, skip expensive 24-fold surface rendering
        // Torus will be composited on top anyway - just use simple background
        if (torusActive && u_torus_t > 0.3) {
            // Simple dark background - torus renders on top
            col = vec3(0.02, 0.02, 0.04);
            d = MAX_DIST;  // No surface hit
        } else {
            // Only raymarch when actually rendering surface (BIG PERF WIN)
            vec2 rm = rayMarch(ro, rd);
            d = rm.x;
            steps_norm = rm.y;
        }
    }

    if (!insideLatch && interiorBlend <= 0.01 && !(torusActive && u_torus_t > 0.3)) {
        // Surface or background rendering
        if (d < MAX_DIST) {
            vec3 p = ro + rd * d;
            vec3 n = getNormal(p);

            // === PRE-TORUS TRANSITION MODE ===
            // During 24-fold → torus transition, simplify rendering
            // Only when: high fold24_t, not warping, torus about to activate
            bool preTransition = (u_fold24_t > 0.5 && u_warp_active == 0 && u_torus_active == 1);

            col = lighting(p, n, rd, baseColor, uv_screen);

            // SSAO: skip expensive SSAO during pre-transition
            float ao;
            if (preTransition) {
                ao = approximateAO(steps_norm, p, n);  // Always use cheap AO
            } else {
                ao = (u_quality_tier >= 1) ? calculateSSAO(p, n) : approximateAO(steps_norm, p, n);
            }
            ao = clamp(ao, u_skin_ao_clamp, 1.0);
            col *= ao;

            // === ENHANCED OUTSIDE VISUALS ===
            // Skip during pre-transition for performance

            float veins = 0.0;
            float sss_intensity = 0.0;
            float rim = 0.0;
            float rimIntensity = 0.0;
            vec3 veinColor = vec3(0.0);
            vec3 subsurface = vec3(0.0);
            vec3 rimGlow = vec3(0.0);

            if (!preTransition) {
                // Rotate p for consistent pattern orientation
                vec3 p_rot = rotateY(u_rotation) * p;

                // 1. ENERGY VEINS - flowing lines that pulse with audio
                // 12-fold harmonic veins following the geometry
                float theta = atan(p_rot.z, p_rot.x);
                float phi = asin(clamp(p_rot.y / (length(p_rot) + 0.001), -1.0, 1.0));

                // Primary veins - 12-fold radial pattern
                float veinAngle = mod(theta + PI, TAU / 12.0);
                float veinDist = min(veinAngle, TAU / 12.0 - veinAngle);
                float primaryVein = smoothstep(0.08, 0.02, veinDist);

                // Secondary veins - latitude lines
                float latVein = smoothstep(0.06, 0.02, abs(mod(phi + PI * 0.5, 0.4) - 0.2));

                // Flowing energy along veins
                float veinFlow = sin(theta * 6.0 - u_time * 3.0 + u_energy * 5.0) * 0.5 + 0.5;
                veinFlow *= sin(phi * 8.0 + u_time * 2.0) * 0.5 + 0.5;

                // Vein pulse (kick clamped, energy-driven)
                float kick_used = getKickUsed(u_cam_inside_dip);
                float veinPulse = 1.0 + min(kick_used * 0.8, KICK_PUMP_MAX) + u_energy * 0.4;

                // Combine veins
                veins = max(primaryVein, latVein * 0.7) * veinFlow * veinPulse;
                veinColor = baseColor * 2.0 + vec3(0.3, 0.1, 0.4);

                // 2. SUBSURFACE GLOW - light penetrating from within
                // Fake SSS: glow from edges + depth-based interior light
                float sss_rim = pow(1.0 - max(dot(n, -rd), 0.0), 1.5);
                float sss_depth = exp(-d * 0.8);  // Closer = more glow

                // Interior glow color - warmer, shifts with chroma
                vec3 sss_color = baseColor * 1.5 + vec3(0.4, 0.2, 0.1);
                sss_color = mix(sss_color, chromaColor() * 2.0, 0.3);

                // Pulsing interior light (energy-driven, kick clamped)
                float sss_pulse = 0.6 + sin(u_time * 1.5 + u_energy * 3.0) * 0.3;
                sss_pulse += min(kick_used * 0.3, KICK_PUMP_MAX);

                // SSS intensity - stronger at edges and with energy
                sss_intensity = sss_rim * sss_depth * sss_pulse * (0.5 + u_energy * 0.5);
                subsurface = sss_color * sss_intensity;

                // 3. Rim glow - cinematic with distance-aware sharpness + iridescence
                // Distance-aware: sharper rim close up, softer far away (DOF perception)
                float rimSharpness = mix(3.5, 1.8, clamp(d / MAX_DIST, 0.0, 1.0));
                float fresnel = 1.0 - max(dot(n, -rd), 0.0);
                rim = pow(fresnel, rimSharpness);

                // Iridescent color shift at grazing angles (rainbow edge effect)
                float hue = float(u_dom_pc) / 12.0;
                float iridShift = fresnel * 0.25;  // Shift up to 25% of color wheel at edges
                vec3 iridColor = hue2rgb(fract(hue + iridShift));

                rimIntensity = 0.6 + u_energy * 0.4 + min(kick_used * 0.2, KICK_PUMP_MAX);
                vec3 rimColor = mix(baseColor * 1.5, iridColor, fresnel * 0.5);
                rimColor += vec3(0.2, 0.1, 0.3) * u_energy;
                rimGlow = rimColor * rim * rimIntensity;

                // 4. Audio-reactive breathing (energy-driven, kick clamped)
                float breath = 1.0 + sin(u_time * 2.0 + u_energy * 4.0) * 0.08 * u_energy;
                breath += min(kick_used * 0.08, KICK_PUMP_MAX * 0.5);
                col *= breath;

                // === BOOST VIDEO ON SURFACE ===
                // Make the video/skin texture more visible
                if (u_skin_has_texture != 0) {
                    // Sample video directly for boost (triplanar)
                    vec3 videoCol = sampleSkinTextureTriplanar(p, n);

                    // Increase video contrast
                    videoCol = (videoCol - 0.5) * 1.4 + 0.5;
                    videoCol = clamp(videoCol, 0.0, 1.0);

                    // Blend video more strongly onto surface
                    float videoMix = u_skin_surface_mix * 1.3;  // Boost mix
                    videoMix = min(videoMix, 0.85);  // Cap it
                    col = mix(col, videoCol * col * 2.0, videoMix);
                }
            } else {
                // Pre-transition: simple rim glow only (cheap)
                float fresnel = 1.0 - max(dot(n, -rd), 0.0);
                rim = fresnel * fresnel;  // Cheaper than pow
                rimIntensity = 0.5 + u_energy * 0.3;
                rimGlow = baseColor * rim * rimIntensity;
            }

            // === COMBINE ALL EFFECTS ===

            // Add energy veins (bright lines on surface)
            col = mix(col, veinColor, veins * 0.6);

            // Add subsurface glow (interior light bleeding through)
            col += subsurface * 0.7;

            // Add rim glow
            col += rimGlow;

            // Emission from veins and SSS
            emission = veins * 0.8 + sss_intensity * 0.6 + rim * rimIntensity * 0.4;

            // === SKIN STARBURST (screen-space) ===
            // Skip during pre-transition
            if (!preTransition) {
                col += skinStarburst(uv_screen, col);
            }

            // === JOURNEY: Surface opacity ===
            alpha = u_surface_opacity;

            // Also scale intensity by field_density for Genesis phase hints
            col *= u_field_density;
        } else {
            // Background - transparent so video shows through
            col = vec3(0.0);
            alpha = 0.0;
        }
    }

    // Fog
    if (u_fog_density > 0.0 && d < MAX_DIST) {
        float fog = 1.0 - exp(-d * u_fog_density * 0.05);
        vec3 fogColor = vec3(0.08, 0.08, 0.12);
        col = mix(col, fogColor, fog);
    }

    // === POST-PROCESSING (AAA Pipeline) ===

    // === PRE-TORUS TRANSITION POST-PROCESSING GATE ===
    // Skip expensive post-processing during 24-fold → torus transition
    // Only when: high fold24_t, not inside, not warping, torus about to activate
    bool postPreTransition = (u_fold24_t > 0.5 && u_interior_latch < 0.5 &&
                              u_warp_active == 0 && u_torus_active == 1);

    // === FOVEATED BUDGET SYSTEM ===
    // Compute detail budget based on screen position and QS wheel state
    // QS ON: center LOW detail (legibility), edges HIGH detail
    // QS OFF: center HIGH detail (hero), edges LOW detail
    vec3 bgBudget = getFoveateBudget(uv);
    int bgSamples = getBgSamples(u_bg_quality, bgBudget.x);
    int postTaps = getPostTaps(u_bg_quality, bgBudget.z);

    // -1. CHEAP BACKGROUND PASS - budget-controlled sky, stars, noise
    // Only applies when not fully on geometry (background visible)
    if (!insideLatch && d > MAX_DIST * 0.9) {
        col = backgroundPassCheap(uv, col, d, bgBudget);
    }

    // 0. HYPERSPACE WARP - radial speed lines during entry/exit
    col = hyperspaceWarp(uv, col);

    // 1. COMBINED BLOOM - skip entirely during pre-torus transition
    if (!postPreTransition) {
        float bloom_threshold = 0.55 - u_energy * 0.2;
        float bloom_intensity = 0.35 + u_energy * 0.5 + emission * 0.35;
        bloom_intensity += u_skin_bloom * 0.4;
        col = combinedBloom(col, uv, bloom_threshold, bloom_intensity, 0.02);
    }

    // 1.5 OUTER BACKDROP - Premium cathedral background
    // Replaces busy spectral rings with clean aperture + rim halo
    // Skip during pre-transition for performance
    if (!insideLatch && !postPreTransition) {
        // Estimate field radius in screen space (approximate based on camera distance)
        float fieldScreenRadius = u_base_radius / max(dist, 0.5) * 0.4;

        // Use premium outer backdrop (Style A: Aperture+Halo is default)
        // This is the "cathedral backdrop" that frames without competing
        col = outerBackdropPremium(uv, col, fieldScreenRadius);

        // Optional: Add subtle spectral ring hints at low opacity when NOT in vortex
        // This preserves some frequency visualization without being busy
        if (u_vortex_active == 0 && u_spectral_rings > 0.5 && u_ring_style_aaa > 0.5) {
            // Mix in AAA rings at reduced intensity for frequency hints
            vec3 ringsCol = col;
            ringsCol = spectralRingsAAA(uv, ringsCol, fieldScreenRadius);
            col = mix(col, ringsCol, 0.25);  // Only 25% contribution
        }
    }

    // 1.6 Ring Dive effects - portal rings, warp streaks, field lines
    // Self-gated: skips entirely when u_ring_dive_active == 0
    // Also skip during pre-torus transition for performance
    if (!postPreTransition) {
        col = ringDiveEffects(uv, col);
    }

    // 1.7 Torus Knot Cymatics - tertiary inside transition
    // Self-gated: skips entirely when u_torus_active == 0
    col = renderTorusKnot(uv, col, uv_screen);

    // 1.8 Vortex Topology - spiral flow pattern
    // Self-gated: skips entirely when u_vortex_active == 0
    col = renderVortex(uv, col, uv_screen);

    // 1.9 Dodecahedral Resonance - crystalline harmonic arrival
    // Self-gated: skips entirely when u_dodeca_active == 0
    col = renderDodecahedral(uv, col, uv_screen);

    // 2. Lens streak on kick (use kick_used, suppressed when inside)
    // Skip during pre-transition
    float final_kick_used = getKickUsed(u_cam_inside_dip);
    if (!postPreTransition) {
        col = lensStreak(uv, col, final_kick_used, u_energy);
    }

    // 3. Beat-synced exposure micro-pump (ProjectM vibe, clamped)
    float kick_pump = 1.0 + min(final_kick_used * 0.06, KICK_PUMP_MAX * 0.4);
    col *= kick_pump;

    // 3.5 PRO: VOLUMETRIC LIGHT SHAFTS - god rays from center
    // Skip during pre-transition for performance
    if (u_bg_quality >= BG_MED && !postPreTransition) {
        float shaftIntensity = u_energy * 0.15 * (1.0 - u_cam_inside_dip);
        col = volumetricShaftsBudgeted(uv, col, shaftIntensity, bgSamples);
    }

    // 3.55 KICK STREAKS LITE - max 5 taps, gated by kick
    // Skip during pre-transition
    if (u_kick > 0.1 && u_bg_quality >= BG_LOW && !postPreTransition) {
        float kickStreakIntensity = u_kick * u_bass_ema;
        col = kickStreaksLite(uv, col, kickStreakIntensity, min(postTaps, 5));
    }

    // 3.6 PRO: LENS FLARE - anamorphic streak on bright areas
    // Skip during pre-transition
    float sceneBrightness = dot(col, vec3(0.299, 0.587, 0.114));
    if (!postPreTransition) {
        col = lensFlare(uv, col, sceneBrightness, u_energy);
    }

    // 4. Enhanced bloom - SKIP (combined into step 1)
    // col = enhancedBloom(col, uv_screen, 0.7, 0.15);

    // 5. Exposure and ACES tone mapping (industry standard)
    col *= u_exposure;
    col = acesToneMap(col);

    // 5.5 Cinematic color grading (digital cinema look)
    {
        float lum = dot(col, vec3(0.299, 0.587, 0.114));

        // Shadows: slightly cool, desaturated (noir feel)
        vec3 shadowGrade = mix(vec3(lum), col, 0.85);  // Desaturate 15%
        shadowGrade *= vec3(0.95, 0.97, 1.03);  // Cool blue shift

        // Midtones: preserve, slight warmth
        vec3 midGrade = col * vec3(1.02, 1.0, 0.98);

        // Highlights: warm, saturated (golden hour feel)
        vec3 highGrade = col + vec3(0.06, 0.03, 0.0) * smoothstep(0.6, 1.0, lum);

        // Blend based on luminance
        float shadowMask = 1.0 - smoothstep(0.0, 0.3, lum);
        float highMask = smoothstep(0.5, 0.9, lum);
        float midMask = 1.0 - shadowMask - highMask;

        col = shadowGrade * shadowMask + midGrade * midMask + highGrade * highMask;
    }

    // 6. Vignette - subtle darkening at edges
    float vig = vignette(uv_screen - 0.5, 1.4, 0.4);
    col *= mix(0.7, 1.0, vig);

    // 6.5 PORTAL ENTRY APERTURE - iris/vignette effect on portal entry
    // u_portal_entry_aperture: 1.0 on entry, decays to 0 over ~200ms
    if (u_cam_in_portal == 1 && u_portal_entry_aperture > 0.01) {
        // Iris effect: dark ring that shrinks from edge to center
        vec2 centerUV = uv_screen - 0.5;
        float aspect = u_resolution.x / u_resolution.y;
        centerUV.x *= aspect;
        float dist = length(centerUV);

        // Aperture radius expands from 0.1 to 0.8 as effect fades
        float apertureRadius = mix(0.15, 0.9, 1.0 - u_portal_entry_aperture);
        float softness = 0.15;

        // Dark iris ring
        float iris = smoothstep(apertureRadius - softness, apertureRadius, dist);
        iris *= u_portal_entry_aperture;  // Fade with effect

        // Apply as darkening
        col *= (1.0 - iris * 0.85);
    }

    // 7. Film grain - luminance-aware, subtle texture
    float grain_intensity = 0.015 + (1.0 - u_stability) * 0.015;
    col = filmGrain(col, uv_screen, grain_intensity);

    // 8. Waterfall HUD - tiny spectrogram in corner (before gamma)
    col = waterfallHUD(uv_screen, col);

    // 9. Gamma correction (ACES output is linear, needs sRGB conversion)
    col = pow(col, vec3(1.0 / 2.2));

    // 9.5 PRO: CONTRAST ADAPTIVE SHARPENING - recover detail
    // Skip during pre-transition for performance
    if (!postPreTransition) {
        float sharpAmount = (u_quality_tier == 2) ? 0.4 : 0.2;
        col = contrastAdaptiveSharpen(col, uv_screen, sharpAmount);
    }

    // 9.6 PRO: DITHERING - eliminate color banding in gradients
    // Skip during pre-transition
    if (!postPreTransition) {
        col = applyDither(col, uv_screen);
    }

    // Clamp to valid range
    col = clamp(col, 0.0, 1.0);

    // 10. Quaternion Symmetry Wheel - REG Figure 8 angular symmetry
    // Rendered LAST, after all post-processing, so nothing affects its visibility
    col = quaternionSymmetryWheel(uv_screen, col);

    // 11. Warp Jump Drive - PERMANENTLY DISABLED (was causing bright oval artifact)
    // The warpJumpEffects function uses non-aspect-corrected length() calls
    // col = warpJumpEffects(uv_screen, col);

    // 12. VIDEO PORTAL - ABSOLUTE TOPMOST LAYER (when QS wheel off)
    // Renders AFTER all post-processing for clean video presentation
    // Particles "landing" cause ripple distortions on the video surface
    if (u_qs_wheel_enable < 0.5 && u_torus_active == 1) {
        // Screen-space UV for video portal
        vec2 centerUV = uv_screen - 0.5;  // Center at screen middle
        float aspect = u_resolution.x / u_resolution.y;
        centerUV.x *= aspect;  // Correct for aspect ratio

        float portalRadius = 0.25;  // Radius in screen space
        float centerDist = length(centerUV);

        if (centerDist < portalRadius * 1.5) {
            // === PARTICLE LANDING RIPPLE DISTORTION ===
            // Procedural ripples that simulate particles hitting the video surface
            // Each ripple spawns, expands outward, and fades
            vec2 rippleOffset = vec2(0.0);
            float rippleTime = u_time * 0.8;  // Ripple animation speed

            // Multiple concurrent ripples (8 ripple sources)
            for (int r = 0; r < 8; r++) {
                float rf = float(r);
                // Hash-based spawn position within portal
                float rHash1 = fract(sin(rf * 127.1 + 311.7) * 43758.5453);
                float rHash2 = fract(sin(rf * 269.5 + 183.3) * 43758.5453);
                float rHash3 = fract(sin(rf * 419.2 + 371.9) * 43758.5453);

                // Ripple spawn position (random point within portal)
                float spawnAngle = rHash1 * 6.283185;
                float spawnRadius = rHash2 * portalRadius * 0.6;  // Stay within portal
                vec2 rippleCenter = vec2(cos(spawnAngle), sin(spawnAngle)) * spawnRadius;

                // Ripple life cycle (staggered timing)
                float rippleSpeed = 0.4 + rHash3 * 0.3;
                float rippleLife = fract(rippleTime * rippleSpeed + rHash1);

                // Ripple ring expands outward as life increases
                float rippleRingRadius = rippleLife * portalRadius * 1.2;
                float rippleWidth = 0.02 + rippleLife * 0.03;  // Ring gets thicker as it expands

                // Distance from this pixel to ripple ring
                float distToRippleCenter = length(centerUV - rippleCenter);
                float ringDist = abs(distToRippleCenter - rippleRingRadius);

                // Ripple intensity (strongest at ring edge, fades with age)
                float rippleIntensity = smoothstep(rippleWidth, 0.0, ringDist);
                rippleIntensity *= 1.0 - rippleLife;  // Fade out as ripple ages
                rippleIntensity *= 0.6 + u_energy * 0.4;  // Energy-reactive

                // Push pixels outward from ripple center
                vec2 pushDir = normalize(centerUV - rippleCenter + 0.001);
                rippleOffset += pushDir * rippleIntensity * 0.025;
            }

            // Apply ripple distortion to UV
            vec2 distortedUV = centerUV + rippleOffset;

            // Normalize to -1 to 1 range within portal (distortedUV still has aspect multiplied)
            vec2 normalizedPortalUV = distortedUV / portalRadius;

            // Use unified video UV mapping function
            float portalZoom = 0.35;  // Show 35% of video (30% bigger/more zoomed in)
            vec2 videoUV = mapCircularToVideoUV(normalizedPortalUV, aspect, portalZoom);
            vec3 videoCol = texture(u_skin_tex, videoUV).rgb;

            // Soft edge blend
            float videoMask = smoothstep(portalRadius * 1.2, portalRadius * 0.4, centerDist);

            // Blend video on top at 95% visibility
            col = mix(col, videoCol, videoMask * 0.95);
        }
    }

    // === GLOBAL SILENCE BLACKOUT ===
    // When silence is active, fade all layers to black except GLB (rendered separately in Rust)
    // This is the "idle but alive" state - system waiting for audio to return
    if (u_silence_active == 1 || u_silence_blackout > 0.01) {
        // Fade to black based on blackout alpha
        // Use smooth cubic curve for premium feel
        float blackout = u_silence_blackout * u_silence_blackout * (3.0 - 2.0 * u_silence_blackout);
        col = mix(col, vec3(0.0), blackout);

        // Also fade alpha (geometry disappears cleanly)
        alpha *= (1.0 - blackout);
    }

    // === LICENSE WATERMARK ===
    // Procedural SDF text "compeller.ai" in bottom-right corner.
    // Driven by u_watermark uniform — removing the check doesn't help
    // because the same license blob controls render_scale, stream limits, etc.
    if (u_watermark > 0.5) {
        vec2 wm_uv = v_uv;
        // Position: bottom-right corner with padding
        vec2 wm_origin = vec2(0.72, 0.06);
        vec2 wm_pos = (wm_uv - wm_origin) * vec2(u_resolution.x / u_resolution.y, 1.0);
        wm_pos *= 12.0;  // Scale

        // Simple horizontal bar segments spelling "compeller.ai"
        // Use a compact bitmap approach: each character is a 3x5 grid
        float wm_d = 1.0;

        // Draw a subtle dotted line pattern as brand mark
        float stripe_x = fract(wm_pos.x * 2.0);
        float stripe_mask = step(0.0, wm_pos.x) * step(wm_pos.x, 3.6)
                          * step(0.0, wm_pos.y) * step(wm_pos.y, 0.35);
        float dots = step(0.4, stripe_x) * step(stripe_x, 0.6);
        wm_d = min(wm_d, mix(1.0, dots, stripe_mask));

        // "C" shape
        float c_x = wm_pos.x;
        float c_y = wm_pos.y - 0.55;
        float c_d = length(vec2(c_x, c_y)) - 0.28;
        c_d = max(c_d, -(length(vec2(c_x - 0.08, c_y)) - 0.18));
        c_d = max(c_d, -step(0.0, c_x) * step(c_y, 0.0) * step(-0.15, c_y));
        wm_d = min(wm_d, smoothstep(0.02, -0.02, c_d));

        float wm_alpha = wm_d * 0.25 * u_watermark;  // Subtle 25% opacity
        col = mix(col, vec3(1.0), wm_alpha);
    }

    frag_color = vec4(col, alpha);
}
"#;

// Keep the old shaders as constants for reference during migration
// These will be removed once the new system is fully validated

/// Legacy SDF Fragment Shader (to be removed)
#[allow(dead_code)]
pub const LEGACY_SDF_FRAGMENT_SHADER: &str = r#"
// Preserved for reference - see git history for full implementation
"#;

/// Legacy Polygon Overlay Shader (no longer needed - geometry IS the symbol)
#[allow(dead_code)]
pub const LEGACY_POLYGON_FRAGMENT_SHADER: &str = r#"
// Preserved for reference - see git history for full implementation
"#;

// ============================================================================
// UPSCALE SHADER - Composites low-res interior with high-res exterior
// ============================================================================

/// Simple upscale shader for compositing low-res interior rendering
/// Uses bilinear filtering (built into texture sampling)
pub const UPSCALE_FRAGMENT_SHADER: &str = r#"
#version 140

precision highp float;

in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_lowres_tex;     // Low-res interior render
uniform sampler2D u_highres_tex;    // High-res exterior render (if separate)
uniform float u_interior_blend;      // How much interior to show (0-1)
uniform vec2 u_resolution;

void main() {
    // Sample the low-res interior texture (bilinear filtering handles upscale)
    vec4 interior = texture(u_lowres_tex, v_uv);

    // For now, just output the upscaled interior
    // When interior_blend < 1, we'd blend with exterior, but that requires
    // multi-pass rendering. For now, the main shader handles blending internally.
    frag_color = interior;
}
"#;

/// Temporal Anti-Aliasing (TAA) Resolve Shader
///
/// Blends current jittered frame with accumulated history using:
/// - Neighborhood clamping to reject invalid/ghosted history
/// - Motion-based reprojection (simplified: uses camera rotation delta)
/// - Exponential blend favoring history (~90%) for stability
///
/// TAA works especially well for SDF raymarching because:
/// - Mathematically precise geometry means jitter samples TRUE sub-pixel detail
/// - Smooth continuous motion (audio-reactive) is ideal for temporal accumulation
/// - Reconstructs actual detail, not just sharpening existing pixels
pub const TAA_RESOLVE_FRAGMENT_SHADER: &str = r#"
#version 140

precision highp float;

in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_current;      // Current jittered frame
uniform sampler2D u_history;      // Previous resolved frame
uniform vec2 u_resolution;        // Screen resolution
uniform float u_motion_x;         // Camera rotation delta (simplified motion vector)
uniform float u_motion_y;         // Vertical motion component (usually 0)
uniform float u_blend_factor;     // 0.1 = 10% current, 90% history (default)
uniform float u_first_frame;      // 1.0 on first frame (no history yet)

// RGB to luminance
float luminance(vec3 c) {
    return dot(c, vec3(0.299, 0.587, 0.114));
}

// Catmull-Rom (bicubic-ish) for sharper history sampling
vec3 sampleHistoryBicubic(vec2 uv) {
    vec2 texSize = u_resolution;
    vec2 texelSize = 1.0 / texSize;

    // For performance, use bilinear with slight sharpening
    vec3 center = texture(u_history, uv).rgb;

    // Sample 4 neighbors for sharpening kernel
    vec3 left = texture(u_history, uv - vec2(texelSize.x, 0.0)).rgb;
    vec3 right = texture(u_history, uv + vec2(texelSize.x, 0.0)).rgb;
    vec3 top = texture(u_history, uv - vec2(0.0, texelSize.y)).rgb;
    vec3 bottom = texture(u_history, uv + vec2(0.0, texelSize.y)).rgb;

    // Slight sharpening: center * 1.5 - neighbors * 0.125 each
    vec3 sharpened = center * 1.2 - (left + right + top + bottom) * 0.05;

    return mix(center, sharpened, 0.3);
}

void main() {
    vec2 uv = v_uv;
    vec2 texelSize = 1.0 / u_resolution;

    // Sample current frame
    vec3 current = texture(u_current, uv).rgb;

    // On first frame, just output current (no history yet)
    if (u_first_frame > 0.5) {
        frag_color = vec4(current, 1.0);
        return;
    }

    // Reproject: where was this pixel last frame?
    // Simplified motion model based on camera rotation
    vec2 motion = vec2(u_motion_x, u_motion_y);
    vec2 history_uv = uv - motion;

    // Sample history with slight sharpening to counteract blur accumulation
    vec3 history;
    if (history_uv.x >= 0.0 && history_uv.x <= 1.0 &&
        history_uv.y >= 0.0 && history_uv.y <= 1.0) {
        history = sampleHistoryBicubic(history_uv);
    } else {
        // History UV out of bounds - use current frame
        frag_color = vec4(current, 1.0);
        return;
    }

    // =========================================================
    // NEIGHBORHOOD CLAMPING (reject invalid/ghosted history)
    // =========================================================
    // Sample 3x3 neighborhood of current frame to find valid color range
    vec3 near_min = vec3(1.0);
    vec3 near_max = vec3(0.0);

    // Cross pattern (5 samples) for performance
    vec3 s_center = current;
    vec3 s_left = texture(u_current, uv + vec2(-texelSize.x, 0.0)).rgb;
    vec3 s_right = texture(u_current, uv + vec2(texelSize.x, 0.0)).rgb;
    vec3 s_top = texture(u_current, uv + vec2(0.0, -texelSize.y)).rgb;
    vec3 s_bottom = texture(u_current, uv + vec2(0.0, texelSize.y)).rgb;

    near_min = min(near_min, s_center);
    near_min = min(near_min, s_left);
    near_min = min(near_min, s_right);
    near_min = min(near_min, s_top);
    near_min = min(near_min, s_bottom);

    near_max = max(near_max, s_center);
    near_max = max(near_max, s_left);
    near_max = max(near_max, s_right);
    near_max = max(near_max, s_top);
    near_max = max(near_max, s_bottom);

    // Slight expansion of bounds to allow some temporal super-resolution
    vec3 bound_expansion = (near_max - near_min) * 0.1;
    near_min -= bound_expansion;
    near_max += bound_expansion;

    // Clamp history to valid neighborhood range
    vec3 clamped_history = clamp(history, near_min, near_max);

    // =========================================================
    // BLEND FACTOR ADJUSTMENT
    // =========================================================
    // Base blend: 10% current, 90% history (smooth accumulation)
    float blend = u_blend_factor;

    // Increase blend toward current when history was clamped significantly
    // (indicates disocclusion or major change)
    float clamp_amount = length(history - clamped_history);
    blend = mix(blend, 0.5, smoothstep(0.0, 0.3, clamp_amount));

    // Increase blend toward current in high-contrast regions
    // (edges need more responsiveness)
    float local_contrast = length(near_max - near_min);
    blend = mix(blend, blend * 2.0, smoothstep(0.2, 0.5, local_contrast));
    blend = clamp(blend, 0.05, 0.5);

    // =========================================================
    // FINAL BLEND
    // =========================================================
    vec3 resolved = mix(clamped_history, current, blend);

    // Subtle tone mapping to prevent fireflies from accumulating
    float lum = luminance(resolved);
    if (lum > 1.5) {
        resolved *= 1.5 / lum;
    }

    frag_color = vec4(resolved, 1.0);
}
"#;