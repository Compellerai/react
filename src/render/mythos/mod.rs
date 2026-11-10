//! Mythos Visual Engine
//!
//! A continuous field excited by music through a Collatz-sequence modulation layer.
//!
//! ## Features
//! - Multi-layer fields with independent Collatz state machines
//! - 5 color palettes across 5 presets
//! - Act system (Dormant → Rising → Peak → Falling)
//! - Video skin integration (Surface / Emissive / Both)
//! - Quality-tier adaptive rendering (FBM octave scaling)
//! - Collatz state export for OSC/DMX lighting control

use anyhow::{anyhow, Result};
use eframe::glow::{self, HasContext};
use std::sync::Arc;

use crate::render::theme::{AudioAnalysis, RendererType, ThemePreset, ThemeRenderer};

// ============================================================
// GLSL Shaders
// ============================================================

const VERTEX_SHADER: &str = r#"#version 330 core
layout(location = 0) in vec2 aPos;
out vec2 vUV;
void main() {
    vUV = aPos * 0.5 + 0.5;
    gl_Position = vec4(aPos, 0.0, 1.0);
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 330 core

in vec2 vUV;
out vec4 fragColor;

uniform float u_time;
uniform vec2  u_resolution;

// Audio
uniform float u_bass;
uniform float u_mid;
uniform float u_high;
uniform float u_energy;
uniform float u_kick;
uniform float u_chroma[12];

// Multi-layer Collatz state (3 layers)
uniform float u_c0_phase, u_c0_heat, u_c0_intensity;
uniform float u_c1_phase, u_c1_heat, u_c1_intensity;
uniform float u_c2_phase, u_c2_heat, u_c2_intensity;
uniform vec3  u_layer_weights;

// Parameters
uniform float u_instability;
uniform float u_field_strength;
uniform float u_disturbance_size;
uniform float u_harmonic_influence;
uniform float u_decay;

// Quality & palette
uniform int   u_quality_tier;   // 0=Low, 1=Med, 2=High, 3=Ultra
uniform int   u_palette;        // 0-4
uniform int   u_act;            // 0=Dormant, 1=Rising, 2=Peak, 3=Falling
uniform float u_act_t;          // Act transition progress 0-1

// Video skin
uniform sampler2D u_skin_tex;
uniform int   u_skin_has_texture;
uniform int   u_skin_mode;      // 0=none, 1=surface, 2=emissive, 3=both, 4=dominant (compel rides the field)
uniform float u_skin_strength;
uniform float u_skin_surface_mix;
uniform float u_skin_emissive;
uniform float u_video_visibility;  // 0-1 blend amount
uniform float u_video_scale;       // UV scale (1.0 = fill, <1 = zoom in)
uniform float u_video_luma_key;    // 0 = off, 1 = full (dark pixels transparent)

// --- Gradient noise ---

vec3 hash33(vec3 p) {
    p = vec3(dot(p, vec3(127.1, 311.7, 74.7)),
             dot(p, vec3(269.5, 183.3, 246.1)),
             dot(p, vec3(113.5, 271.9, 124.6)));
    return -1.0 + 2.0 * fract(sin(p) * 43758.5453123);
}

float gnoise(vec3 p) {
    vec3 i = floor(p);
    vec3 f = fract(p);
    vec3 u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(mix(dot(hash33(i),               f),
                dot(hash33(i + vec3(1,0,0)),  f - vec3(1,0,0)), u.x),
            mix(dot(hash33(i + vec3(0,1,0)),  f - vec3(0,1,0)),
                dot(hash33(i + vec3(1,1,0)),  f - vec3(1,1,0)), u.x), u.y),
        mix(mix(dot(hash33(i + vec3(0,0,1)),  f - vec3(0,0,1)),
                dot(hash33(i + vec3(1,0,1)),  f - vec3(1,0,1)), u.x),
            mix(dot(hash33(i + vec3(0,1,1)),  f - vec3(0,1,1)),
                dot(hash33(i + vec3(1,1,1)),  f - vec3(1,1,1)), u.x), u.y),
        u.z);
}

// Parameterised FBM (max 5 octaves, early-exit for quality gating)
float fbm(vec3 p, int octaves) {
    float v = 0.0, a = 0.5;
    for (int i = 0; i < 5; i++) {
        if (i >= octaves) break;
        v += a * gnoise(p);
        p *= 2.0;
        a *= 0.5;
    }
    return v;
}

// --- Field layer (one Collatz-modulated noise field) ---

float computeLayer(vec2 world, float time, float c_phase, float c_heat,
                   float c_intensity, float seed_off, int octaves) {
    float slow_t = time * 0.08;
    float mid_t  = time * 0.25;
    float fscale = 1.8 + u_field_strength * 2.2;

    vec3 p1 = vec3(world * fscale + seed_off, slow_t);
    float primary = fbm(p1, octaves);

    vec3 p2 = vec3(world * fscale * 2.3 + primary * 0.35 + seed_off * 0.5, mid_t);
    float secondary = fbm(p2, max(octaves - 1, 2));

    float field = primary * 0.6 + secondary * 0.4;

    // Collatz modulation — gated by audio energy (still when silent)
    float energy_gate = smoothstep(0.02, 0.12, u_energy);
    float expansion = c_phase;
    // Tamed instability — caps the visual ceiling so the upper slider range has usable headroom
    // instead of going seizure-y. Slider 0..1 now drives effective instability 0..0.3.
    float inst_eff = u_instability * 0.3;
    float instab = inst_eff * c_heat * energy_gate;
    float warp_amt = 0.08 * energy_gate + instab * 0.35 + expansion * 0.18 * c_intensity * energy_gate;
    float warp = gnoise(vec3(world * 3.2 + field * warp_amt + seed_off * 0.3, slow_t * 0.6));
    field += warp * warp_amt * inst_eff * energy_gate;

    float breath = 1.0 + (expansion - 0.5) * 0.25 * c_intensity * inst_eff * energy_gate;
    field *= breath;

    return field;
}

// --- Color palette ramps ---

vec3 paletteColor(float f, int pal) {
    vec3 c0, c1, c2, c3, c4;
    if (pal == 1) { // Warm: deep red → gold
        c0 = vec3(0.04, 0.01, 0.01);  c1 = vec3(0.15, 0.04, 0.02);
        c2 = vec3(0.35, 0.12, 0.04);  c3 = vec3(0.65, 0.35, 0.10);
        c4 = vec3(0.92, 0.75, 0.45);
    } else if (pal == 2) { // Cold: navy → ice
        c0 = vec3(0.01, 0.015, 0.04); c1 = vec3(0.02, 0.05, 0.12);
        c2 = vec3(0.05, 0.15, 0.30);  c3 = vec3(0.15, 0.40, 0.60);
        c4 = vec3(0.50, 0.80, 0.95);
    } else if (pal == 3) { // Electric: purple → pink
        c0 = vec3(0.03, 0.01, 0.05);  c1 = vec3(0.10, 0.02, 0.15);
        c2 = vec3(0.25, 0.05, 0.35);  c3 = vec3(0.55, 0.15, 0.60);
        c4 = vec3(0.85, 0.50, 0.90);
    } else if (pal == 4) { // Fire: black → yellow
        c0 = vec3(0.02, 0.005, 0.0);  c1 = vec3(0.12, 0.02, 0.0);
        c2 = vec3(0.40, 0.08, 0.0);   c3 = vec3(0.70, 0.30, 0.02);
        c4 = vec3(0.95, 0.80, 0.20);
    } else { // Default (0): violet → cyan
        c0 = vec3(0.015, 0.008, 0.045); c1 = vec3(0.04, 0.06, 0.15);
        c2 = vec3(0.10, 0.18, 0.38);    c3 = vec3(0.28, 0.50, 0.68);
        c4 = vec3(0.65, 0.82, 0.92);
    }
    return (f < 0.25) ? mix(c0, c1, f * 4.0) :
           (f < 0.50) ? mix(c1, c2, (f - 0.25) * 4.0) :
           (f < 0.75) ? mix(c2, c3, (f - 0.50) * 4.0) :
                        mix(c3, c4, (f - 0.75) * 4.0);
}

void main() {
    float aspect = u_resolution.x / u_resolution.y;
    vec2 world = (vUV - 0.5) * vec2(aspect, 1.0);
    float dist = length(world);

    // Quality-dependent octave counts
    int oct0 = (u_quality_tier == 0) ? 2 : (u_quality_tier == 1) ? 3 : 4;
    int oct1 = (u_quality_tier == 0) ? 2 : (u_quality_tier >= 2)  ? 3 : 2;

    // === MULTI-LAYER FIELD ===
    float field = computeLayer(world, u_time, u_c0_phase, u_c0_heat,
                               u_c0_intensity, 0.0, oct0) * u_layer_weights.x;

    // Layer 1 (Med+ quality)
    if (u_quality_tier >= 1) {
        field += computeLayer(world, u_time * 1.15, u_c1_phase, u_c1_heat,
                              u_c1_intensity, 17.3, oct1) * u_layer_weights.y;
    }

    // Layer 2 (High+ quality)
    if (u_quality_tier >= 2) {
        field += computeLayer(world, u_time * 0.85, u_c2_phase, u_c2_heat,
                              u_c2_intensity, 31.7, 2) * u_layer_weights.z;
    }

    // === AUDIO DISTURBANCES ===
    float bass_wave = sin(dist * 5.0 - u_time * 1.8) * u_bass * u_field_strength;
    field += bass_wave * u_disturbance_size * 0.7;

    float mid_perturb = gnoise(vec3(world * 4.5, u_time * 0.7)) * u_mid * 0.45;
    field += mid_perturb * u_field_strength;

    float shimmer = gnoise(vec3(world * 10.0, u_time * 1.8)) * u_high * 0.25;
    field += shimmer * u_field_strength;

    float ring_phase = dist * 8.0 - u_time * 6.0;
    float kick_ring = u_kick * sin(ring_phase) * 0.6;
    kick_ring *= exp(-dist * 1.5 / max(u_disturbance_size + 0.2, 0.05));
    field += kick_ring;

    // === HARMONIC INFLUENCE ===
    float angle = atan(world.y, world.x);
    float harmonic = 0.0;
    for (int i = 0; i < 12; i++) {
        float a = float(i) * 6.2831853 / 12.0;
        harmonic += u_chroma[i] * cos(angle - a + u_time * 0.032);
    }
    harmonic = harmonic / 12.0 * u_harmonic_influence;

    // === ENERGY ENVELOPE ===
    float brightness = 0.25 + u_energy * 0.75;
    field *= brightness;

    // === COLOR ===
    float f = clamp(field * 0.5 + 0.5, 0.0, 1.0);
    vec3 color = paletteColor(f, u_palette);

    // Collatz phase tinting (layer 0 primary)
    float expansion   = u_c0_phase;
    float contraction  = 1.0 - expansion;
    // Tamed instability gain — same cap as in computeLayer, applied here so color shifts and
    // the skin-pass breath_disp (below) respond consistently to the slider.
    float inst_eff = u_instability * 0.3;
    color += vec3(0.07, -0.02, 0.05) * expansion  * u_c0_intensity * inst_eff;
    color += vec3(-0.03, 0.02, 0.07) * contraction * u_c0_intensity * inst_eff;

    // Harmonic tinting
    color += vec3(harmonic * 0.04, harmonic * 0.015, -harmonic * 0.025);

    // Act modulation
    if (u_act == 1) {        // Rising: warm shift
        color *= mix(vec3(1.0), vec3(1.06, 0.98, 0.94), u_act_t);
    } else if (u_act == 2) { // Peak: brighten
        color *= 1.0 + u_act_t * 0.18;
    } else if (u_act == 3) { // Falling: cool shift
        color *= mix(vec3(1.0), vec3(0.94, 0.98, 1.06), u_act_t);
    }
    // Dormant (0): no modulation

    // Vignette
    float vignette = 1.0 - smoothstep(0.5, 1.4, dist);
    color *= 0.75 + vignette * 0.25;
    color *= brightness;

    // === VIDEO SKIN (field-displaced) ===
    // The compel video bends WITH the field — it's embedded in the aether,
    // not layered on top. Field perturbations warp the video UV.
    if (u_skin_has_texture != 0 && u_skin_mode > 0 && u_video_visibility > 0.001) {
        // Scale: zoom toward center (scale < 1 = zoom in, > 1 = zoom out)
        vec2 scaled_uv = (vUV - 0.5) / max(u_video_scale, 0.05) + 0.5;

        // Build displacement from field perturbations (reuse existing signals).
        // Multipliers tuned so the field visibly pushes/pulls the sampled compel — was
        // ~3x smaller (0.06/0.025/0.02/0.015) and the compel looked frozen at small patches.
        vec2 radial = (dist > 0.001) ? world / dist : vec2(0.0);
        float disp_scale = u_skin_strength * 0.18;

        // Field flow: use noise at two nearby points for a flow vector
        vec2 flow = vec2(
            gnoise(vec3(world * 2.8 + 0.5, u_time * 0.12)),
            gnoise(vec3(world * 2.8 + 97.0, u_time * 0.12))
        );
        // Audio-gate the baseline flow displacement so silent audio leaves the compel intact
        // (the kick/bass/breath terms below are already audio-gated by their source signals).
        // Without this, time-driven noise alone produced ~12% UV displacement at silent audio,
        // scrambling the compel into a static-looking chaotic pattern that read as "frozen".
        float skin_energy_gate = smoothstep(0.02, 0.12, u_energy);
        vec2 displacement = flow * field * disp_scale * skin_energy_gate;

        // Kick ring: radial ripple distortion
        displacement += radial * kick_ring * 0.10;

        // Bass: slow radial undulation
        displacement += radial * bass_wave * u_disturbance_size * 0.08;

        // Collatz breathing: expansion stretches outward, contraction pulls in
        float breath_disp = (u_c0_phase - 0.5) * u_c0_intensity * inst_eff * 0.05;
        displacement += radial * breath_disp;

        vec2 displaced_uv = clamp(scaled_uv + displacement, 0.0, 1.0);
        vec3 skin = texture(u_skin_tex, displaced_uv).rgb;

        // Luma key: dark pixels become transparent (field shows through)
        float luma = dot(skin, vec3(0.299, 0.587, 0.114));
        float luma_alpha = (u_video_luma_key > 0.001)
            ? smoothstep(u_video_luma_key * 0.5, u_video_luma_key, luma)
            : 1.0;

        // Combined alpha: visibility * luma key
        float skin_alpha = u_video_visibility * luma_alpha;

        if (u_skin_mode == 1 || u_skin_mode == 3) {
            vec3 overlay = 2.0 * color * skin;
            color = mix(color, overlay, skin_alpha * u_skin_surface_mix);
        }
        if (u_skin_mode == 2 || u_skin_mode == 3) {
            color += skin * skin_alpha * u_skin_emissive;
        }
        // Mode 4: the compel IS the surface — field-displaced compel replaces the field color
        // at full skin_alpha, with the field providing displacement + a partial backdrop where
        // the slider/luma key dial it back. Instability/Disturbance/Field-Strength knobs drive
        // the displacement equations above so tweaking them visibly bends the compel image.
        if (u_skin_mode == 4) {
            color = mix(color, skin, skin_alpha);
        }
    }

    fragColor = vec4(max(color, vec3(0.0)), 1.0);
}
"#;

// ============================================================
// Collatz (3x+1) State Machine
// ============================================================

struct CollatzState {
    value: u64,
    step: u32,
    sequence_max: u64,
    is_odd_step: bool,
    heat: f32,
    intensity: f32,
    seed: u64,
    steps_since_reset: u32,
}

impl CollatzState {
    fn new(seed: u64) -> Self {
        let s = seed.max(2);
        Self {
            value: s,
            step: 0,
            sequence_max: s,
            is_odd_step: s % 2 != 0,
            heat: 0.0,
            intensity: 0.0,
            seed: s,
            steps_since_reset: 0,
        }
    }

    fn step(&mut self) {
        if self.value <= 1 {
            self.seed = self.seed.wrapping_mul(7).wrapping_add(13) % 997 + 7;
            self.value = self.seed;
            self.step = 0;
            self.sequence_max = self.seed;
            self.steps_since_reset = 0;
        }
        let was_odd = self.value % 2 != 0;
        self.is_odd_step = was_odd;
        if was_odd {
            self.value = self.value.saturating_mul(3).saturating_add(1);
            self.heat = (self.heat + 0.08).min(1.0);
        } else {
            self.value /= 2;
            self.heat = (self.heat - 0.04).max(0.0);
        }
        self.sequence_max = self.sequence_max.max(self.value);
        self.step += 1;
        self.steps_since_reset += 1;
        self.intensity = if self.sequence_max > 0 {
            (self.value as f32 / self.sequence_max as f32).min(1.0)
        } else {
            0.0
        };
    }

    fn reseed(&mut self, seed: u64) {
        self.seed = seed.max(2);
        self.value = self.seed;
        self.step = 0;
        self.sequence_max = self.seed;
        self.steps_since_reset = 0;
    }

    fn normalized_value(&self) -> f32 {
        if self.sequence_max > 0 {
            (self.value as f32 / self.sequence_max as f32).min(1.0)
        } else {
            0.0
        }
    }

    fn phase(&self) -> f32 {
        if self.is_odd_step { 1.0 } else { 0.0 }
    }
}

// ============================================================
// Public types for OSC/lighting export
// ============================================================

/// Collatz state output for OSC / DMX lighting.
/// All floats 0-1 and ready to send as continuous control values.
#[derive(Clone, Copy, Debug, Default)]
pub struct CollatzOutput {
    /// Accumulated instability (rises on odd steps, falls on even)
    pub heat: f32,
    /// Current step phase (1.0 = odd/expansion, 0.0 = even/contraction)
    pub phase: f32,
    /// Normalized magnitude of current value in sequence
    pub intensity: f32,
    /// Normalized raw value (current / max seen)
    pub value: f32,
    /// Expansion-contraction breath cycle (smoothed phase for lighting)
    pub breath: f32,
    /// Current act (0=Dormant, 1=Rising, 2=Peak, 3=Falling)
    pub act: u8,
    /// Act transition progress (0-1)
    pub act_t: f32,
}

/// Mythos act phases (emergent from Collatz heat dynamics)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MythosAct {
    Dormant = 0,
    Rising  = 1,
    Peak    = 2,
    Falling = 3,
}

// ============================================================
// Preset definitions
// ============================================================

struct PresetDef {
    id: &'static str,
    name: &'static str,
    seeds: [u64; 3],
    layer_weights: [f32; 3],
    instability: f32,
    field_strength: f32,
    disturbance_size: f32,
    harmonic_influence: f32,
    decay: f32,
    palette: u32,
}

const PRESETS: &[PresetDef] = &[
    PresetDef {
        id: "mythos:default", name: "Mythos",
        seeds: [27, 97, 157], layer_weights: [0.55, 0.30, 0.15],
        instability: 0.50, field_strength: 0.60, disturbance_size: 0.40,
        harmonic_influence: 0.30, decay: 0.50, palette: 0,
    },
    PresetDef {
        id: "mythos:deep_aether", name: "Deep Aether",
        seeds: [127, 251, 509], layer_weights: [0.45, 0.35, 0.20],
        instability: 0.25, field_strength: 0.75, disturbance_size: 0.30,
        harmonic_influence: 0.20, decay: 0.70, palette: 2,
    },
    PresetDef {
        id: "mythos:hailstone_storm", name: "Hailstone Storm",
        seeds: [73, 41, 19], layer_weights: [0.60, 0.25, 0.15],
        instability: 0.85, field_strength: 0.70, disturbance_size: 0.55,
        harmonic_influence: 0.15, decay: 0.30, palette: 3,
    },
    PresetDef {
        id: "mythos:harmonic_drift", name: "Harmonic Drift",
        seeds: [199, 337, 523], layer_weights: [0.40, 0.35, 0.25],
        instability: 0.40, field_strength: 0.55, disturbance_size: 0.35,
        harmonic_influence: 0.70, decay: 0.60, palette: 0,
    },
    PresetDef {
        id: "mythos:solar_furnace", name: "Solar Furnace",
        seeds: [31, 63, 127], layer_weights: [0.55, 0.30, 0.15],
        instability: 0.70, field_strength: 0.80, disturbance_size: 0.60,
        harmonic_influence: 0.25, decay: 0.35, palette: 4,
    },
];

// ============================================================
// Parameters
// ============================================================

pub struct MythosParams {
    pub instability: f32,
    pub field_strength: f32,
    pub disturbance_size: f32,
    pub harmonic_influence: f32,
    pub decay: f32,
}

impl Default for MythosParams {
    fn default() -> Self {
        Self {
            instability: 0.50,
            field_strength: 0.60,
            disturbance_size: 0.40,
            harmonic_influence: 0.30,
            decay: 0.50,
        }
    }
}

// ============================================================
// Mythos Renderer
// ============================================================

pub struct MythosRenderer {
    gl: Option<Arc<glow::Context>>,
    initialized: bool,

    // GPU resources
    program: Option<glow::Program>,
    vao: Option<glow::VertexArray>,
    vbo: Option<glow::Buffer>,

    // Timing
    time: f32,

    // Multi-layer Collatz state machines
    collatz: [CollatzState; 3],
    step_timers: [f32; 3],
    step_intervals: [f32; 3],
    layer_weights: [f32; 3],

    // Smoothed audio
    bass: f32,
    mid: f32,
    high: f32,
    energy: f32,
    kick: f32,
    chroma: [f32; 12],

    // Act system
    act: MythosAct,
    act_t: f32,
    act_timer: f32,
    prev_heat: f32,
    heat_rising_duration: f32,

    // Smoothed breath (for OSC export)
    breath: f32,

    // Parameters
    pub params: MythosParams,

    // Palette index (0-4)
    palette: u32,

    // Quality tier
    quality_tier: u8,

    // Video skin
    skin_texture: Option<glow::Texture>,
    pub skin_texture_valid: bool,
    skin_mode: u32,       // 0=none, 1=surface, 2=emissive, 3=both, 4=dominant (compel rides the field)
    skin_strength: f32,
    skin_surface_mix: f32,
    skin_emissive: f32,

    // Video controls (from UI: visibility, scale, luma key)
    video_visibility: f32,
    video_scale: f32,
    video_luma_key: f32,

    // Active preset index
    active_preset: usize,

    // Viewport
    width: u32,
    height: u32,
}

impl MythosRenderer {
    pub fn new() -> Self {
        let p = &PRESETS[0];
        Self {
            gl: None,
            initialized: false,
            program: None,
            vao: None,
            vbo: None,
            time: 0.0,
            collatz: [
                CollatzState::new(p.seeds[0]),
                CollatzState::new(p.seeds[1]),
                CollatzState::new(p.seeds[2]),
            ],
            step_timers: [0.0; 3],
            step_intervals: [0.2, 0.25, 0.3],
            layer_weights: p.layer_weights,
            bass: 0.0,
            mid: 0.0,
            high: 0.0,
            energy: 0.0,
            kick: 0.0,
            chroma: [0.0; 12],
            act: MythosAct::Dormant,
            act_t: 0.0,
            act_timer: 0.0,
            prev_heat: 0.0,
            heat_rising_duration: 0.0,
            breath: 0.0,
            params: MythosParams::default(),
            palette: 0,
            quality_tier: 1,
            skin_texture: None,
            skin_texture_valid: false,
            skin_mode: 0,
            skin_strength: 0.7,
            skin_surface_mix: 0.6,
            skin_emissive: 0.4,
            video_visibility: 1.0,
            video_scale: 1.0,
            video_luma_key: 0.0,
            active_preset: 0,
            width: 1920,
            height: 1080,
        }
    }

    /// Get current Collatz state for OSC / lighting export.
    /// Uses primary layer (index 0) as the canonical signal.
    pub fn collatz_output(&self) -> CollatzOutput {
        let c = &self.collatz[0];
        CollatzOutput {
            heat: c.heat,
            phase: c.phase(),
            intensity: c.intensity,
            value: c.normalized_value(),
            breath: self.breath,
            act: self.act as u8,
            act_t: self.act_t,
        }
    }

    pub fn set_quality_tier(&mut self, tier: u8) {
        self.quality_tier = tier.min(3);
    }

    pub fn set_skin_texture(&mut self, texture: Option<glow::Texture>, _aspect: f32) {
        self.skin_texture = texture;
        self.skin_texture_valid = texture.is_some();
        if self.skin_texture_valid && self.skin_mode == 0 {
            // Auto-enable Dominant: the compel rides the field as its surface, with the field
            // driving audio-reactive push/pull. Replaces the legacy mode-3 SurfaceEmissive default,
            // which dimmed the compel multiplicatively and was hidden under the flat overlay.
            self.skin_mode = 4;
        }
    }

    /// Set video controls (visibility, scale, luma key) from UI RenderParams
    pub fn set_video_params(&mut self, visibility: f32, scale: f32, luma_key: f32) {
        self.video_visibility = visibility;
        self.video_scale = scale;
        self.video_luma_key = luma_key;
    }

    fn apply_preset(&mut self, idx: usize) {
        if idx >= PRESETS.len() { return; }
        let p = &PRESETS[idx];
        self.active_preset = idx;
        self.params.instability = p.instability;
        self.params.field_strength = p.field_strength;
        self.params.disturbance_size = p.disturbance_size;
        self.params.harmonic_influence = p.harmonic_influence;
        self.params.decay = p.decay;
        self.palette = p.palette;
        self.layer_weights = p.layer_weights;
        for (i, s) in p.seeds.iter().enumerate() {
            self.collatz[i].reseed(*s);
        }
    }

    fn update_act_system(&mut self, dt: f32) {
        let heat = self.collatz[0].heat;
        let delta = heat - self.prev_heat;
        self.prev_heat = heat;

        // Track rising duration
        if delta > 0.001 {
            self.heat_rising_duration += dt;
        } else {
            self.heat_rising_duration = (self.heat_rising_duration - dt * 0.5).max(0.0);
        }

        self.act_timer += dt;

        let new_act = match self.act {
            MythosAct::Dormant => {
                if heat > 0.20 && self.heat_rising_duration > 0.5 {
                    MythosAct::Rising
                } else {
                    MythosAct::Dormant
                }
            }
            MythosAct::Rising => {
                if heat > 0.60 && self.collatz[0].intensity > 0.45 {
                    MythosAct::Peak
                } else if heat < 0.10 {
                    MythosAct::Dormant
                } else {
                    MythosAct::Rising
                }
            }
            MythosAct::Peak => {
                if heat < 0.40 || (delta < -0.005 && self.act_timer > 1.0) {
                    MythosAct::Falling
                } else {
                    MythosAct::Peak
                }
            }
            MythosAct::Falling => {
                if heat < 0.12 {
                    MythosAct::Dormant
                } else if heat > 0.55 {
                    MythosAct::Rising
                } else {
                    MythosAct::Falling
                }
            }
        };

        if new_act != self.act {
            self.act = new_act;
            self.act_t = 0.0;
            self.act_timer = 0.0;
        } else {
            self.act_t = (self.act_t + dt * 2.0).min(1.0);
        }

        // Smooth breath signal for OSC
        let target_breath = self.collatz[0].phase();
        let breath_speed = 4.0;
        self.breath += (target_breath - self.breath) * (1.0 - (-dt * breath_speed).exp());
    }

    fn compile_shader(gl: &glow::Context, shader_type: u32, source: &str) -> Result<glow::Shader> {
        unsafe {
            let shader = gl.create_shader(shader_type)
                .map_err(|e| anyhow!("Failed to create shader: {}", e))?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let info = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                return Err(anyhow!("Shader compilation failed: {}", info));
            }
            Ok(shader)
        }
    }

    fn set_f32(&self, gl: &glow::Context, prog: glow::Program, name: &str, v: f32) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_1_f32(Some(&loc), v);
            }
        }
    }

    fn set_i32(&self, gl: &glow::Context, prog: glow::Program, name: &str, v: i32) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_1_i32(Some(&loc), v);
            }
        }
    }

    fn set_2f32(&self, gl: &glow::Context, prog: glow::Program, name: &str, x: f32, y: f32) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_2_f32(Some(&loc), x, y);
            }
        }
    }

    fn set_3f32(&self, gl: &glow::Context, prog: glow::Program, name: &str, x: f32, y: f32, z: f32) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_3_f32(Some(&loc), x, y, z);
            }
        }
    }
}

impl ThemeRenderer for MythosRenderer {
    fn renderer_type(&self) -> RendererType {
        RendererType::Mythos
    }

    fn init(&mut self, gl: Arc<glow::Context>) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        log::info!("[Mythos] Initializing renderer...");

        unsafe {
            let vs = Self::compile_shader(&gl, glow::VERTEX_SHADER, VERTEX_SHADER)?;
            let fs = Self::compile_shader(&gl, glow::FRAGMENT_SHADER, FRAGMENT_SHADER)?;

            let program = gl.create_program()
                .map_err(|e| anyhow!("Failed to create program: {}", e))?;
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                let info = gl.get_program_info_log(program);
                gl.delete_program(program);
                gl.delete_shader(vs);
                gl.delete_shader(fs);
                return Err(anyhow!("Program link failed: {}", info));
            }
            gl.delete_shader(vs);
            gl.delete_shader(fs);

            let vertices: [f32; 12] = [
                -1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0,
            ];
            let vao = gl.create_vertex_array()
                .map_err(|e| anyhow!("Failed to create VAO: {}", e))?;
            let vbo = gl.create_buffer()
                .map_err(|e| anyhow!("Failed to create VBO: {}", e))?;

            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let bytes: &[u8] = std::slice::from_raw_parts(
                vertices.as_ptr() as *const u8,
                vertices.len() * std::mem::size_of::<f32>(),
            );
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            self.program = Some(program);
            self.vao = Some(vao);
            self.vbo = Some(vbo);
        }

        self.gl = Some(gl);
        self.initialized = true;
        log::info!("[Mythos] Renderer initialized successfully");
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn update(&mut self, audio: &AudioAnalysis, dt: f32) {
        self.time += dt;

        // Smooth audio
        let tau = 0.04 + self.params.decay * 0.12;
        let alpha = 1.0 - (-dt / tau).exp();
        self.bass += (audio.bass - self.bass) * alpha;
        self.mid += (audio.mid - self.mid) * alpha;
        self.high += (audio.high - self.high) * alpha;
        self.energy += (audio.energy - self.energy) * alpha;

        if audio.kick {
            self.kick = 1.0;
        } else {
            self.kick = (self.kick - dt * (3.0 + self.params.decay * 5.0)).max(0.0);
        }

        for i in 0..12 {
            self.chroma[i] += (audio.chroma[i] - self.chroma[i]) * alpha;
        }

        // === MULTI-LAYER COLLATZ (only steps when audio is present) ===
        let has_audio = self.energy > 0.02;

        if has_audio {
            let base_interval = 0.30 - self.energy * 0.22;
            self.step_intervals[0] = base_interval.max(0.06);
            self.step_intervals[1] = (base_interval * 1.2).max(0.08);
            self.step_intervals[2] = (base_interval * 0.8).max(0.05);

            for layer in 0..3 {
                self.step_timers[layer] += dt;

                // Beat triggers primary layer immediately, secondary/tertiary with delay
                if audio.kick && (layer == 0 || self.energy > 0.4 + layer as f32 * 0.15) {
                    self.collatz[layer].step();
                    self.step_timers[layer] = 0.0;
                }

                while self.step_timers[layer] >= self.step_intervals[layer] {
                    self.step_timers[layer] -= self.step_intervals[layer];
                    self.collatz[layer].step();
                }
            }

            // Reseed primary on strong beats after long sequences
            if audio.kick && self.collatz[0].steps_since_reset > 40 && self.energy > 0.65 {
                let seed = (audio.dominant_pc as u64 + 7) * 11
                    + (self.time * 97.0) as u64 % 200 + 7;
                self.collatz[0].reseed(seed);
            }
        }

        // Passive heat decay when quiet
        if self.energy < 0.08 {
            for c in &mut self.collatz {
                c.heat *= 1.0 - dt * 0.4;
            }
        }

        // Act system
        self.update_act_system(dt);
    }

    fn render(&mut self, width: u32, height: u32) {
        let gl = match &self.gl { Some(g) => g.clone(), None => return };
        let prog = match self.program { Some(p) => p, None => return };
        let vao = match self.vao { Some(v) => v, None => return };

        self.width = width;
        self.height = height;

        unsafe {
            gl.use_program(Some(prog));

            self.set_f32(&gl, prog, "u_time", self.time);
            self.set_2f32(&gl, prog, "u_resolution", width as f32, height as f32);

            // Audio
            self.set_f32(&gl, prog, "u_bass", self.bass);
            self.set_f32(&gl, prog, "u_mid", self.mid);
            self.set_f32(&gl, prog, "u_high", self.high);
            self.set_f32(&gl, prog, "u_energy", self.energy);
            self.set_f32(&gl, prog, "u_kick", self.kick);
            for i in 0..12 {
                let name = format!("u_chroma[{}]", i);
                if let Some(loc) = gl.get_uniform_location(prog, &name) {
                    gl.uniform_1_f32(Some(&loc), self.chroma[i]);
                }
            }

            // Multi-layer Collatz
            self.set_f32(&gl, prog, "u_c0_phase", self.collatz[0].phase());
            self.set_f32(&gl, prog, "u_c0_heat", self.collatz[0].heat);
            self.set_f32(&gl, prog, "u_c0_intensity", self.collatz[0].intensity);
            self.set_f32(&gl, prog, "u_c1_phase", self.collatz[1].phase());
            self.set_f32(&gl, prog, "u_c1_heat", self.collatz[1].heat);
            self.set_f32(&gl, prog, "u_c1_intensity", self.collatz[1].intensity);
            self.set_f32(&gl, prog, "u_c2_phase", self.collatz[2].phase());
            self.set_f32(&gl, prog, "u_c2_heat", self.collatz[2].heat);
            self.set_f32(&gl, prog, "u_c2_intensity", self.collatz[2].intensity);
            self.set_3f32(&gl, prog, "u_layer_weights",
                self.layer_weights[0], self.layer_weights[1], self.layer_weights[2]);

            // Parameters
            self.set_f32(&gl, prog, "u_instability", self.params.instability);
            self.set_f32(&gl, prog, "u_field_strength", self.params.field_strength);
            self.set_f32(&gl, prog, "u_disturbance_size", self.params.disturbance_size);
            self.set_f32(&gl, prog, "u_harmonic_influence", self.params.harmonic_influence);
            self.set_f32(&gl, prog, "u_decay", self.params.decay);

            // Quality, palette, act
            self.set_i32(&gl, prog, "u_quality_tier", self.quality_tier as i32);
            self.set_i32(&gl, prog, "u_palette", self.palette as i32);
            self.set_i32(&gl, prog, "u_act", self.act as i32);
            self.set_f32(&gl, prog, "u_act_t", self.act_t);

            // Video skin
            let has_skin = self.skin_texture_valid && self.skin_mode > 0;
            self.set_i32(&gl, prog, "u_skin_has_texture", has_skin as i32);
            self.set_i32(&gl, prog, "u_skin_mode", self.skin_mode as i32);
            self.set_f32(&gl, prog, "u_skin_strength", self.skin_strength);
            self.set_f32(&gl, prog, "u_skin_surface_mix", self.skin_surface_mix);
            self.set_f32(&gl, prog, "u_skin_emissive", self.skin_emissive);
            self.set_f32(&gl, prog, "u_video_visibility", self.video_visibility);
            self.set_f32(&gl, prog, "u_video_scale", self.video_scale);
            self.set_f32(&gl, prog, "u_video_luma_key", self.video_luma_key);

            if has_skin {
                if let Some(tex) = self.skin_texture {
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                    self.set_i32(&gl, prog, "u_skin_tex", 0);
                }
            }

            // Draw
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(Some(vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    fn cleanup(&mut self) {
        if let Some(ref gl) = self.gl {
            unsafe {
                if let Some(p) = self.program { gl.delete_program(p); }
                if let Some(v) = self.vao { gl.delete_vertex_array(v); }
                if let Some(b) = self.vbo { gl.delete_buffer(b); }
            }
        }
        self.program = None;
        self.vao = None;
        self.vbo = None;
        self.gl = None;
        self.initialized = false;
    }

    fn scan_presets(&mut self) -> Vec<ThemePreset> {
        PRESETS.iter().map(|p| {
            ThemePreset::new(p.id, p.name, "Mythos Field")
                .with_renderer(RendererType::Mythos)
        }).collect()
    }

    fn get_categories(&self) -> Vec<String> {
        vec!["Mythos Field".to_string()]
    }

    fn get_presets_in_category(&self, category: &str) -> Vec<ThemePreset> {
        if category == "Mythos Field" {
            PRESETS.iter().map(|p| {
                ThemePreset::new(p.id, p.name, "Mythos Field")
                    .with_renderer(RendererType::Mythos)
            }).collect()
        } else {
            Vec::new()
        }
    }

    fn load_preset(&mut self, preset_id: &str) -> Result<()> {
        if let Some(idx) = PRESETS.iter().position(|p| p.id == preset_id) {
            self.apply_preset(idx);
            Ok(())
        } else {
            Err(anyhow!("Unknown Mythos preset: {}", preset_id))
        }
    }

    fn load_preset_by_name(&mut self, category: &str, name: &str) -> Result<()> {
        if category != "Mythos Field" {
            return Err(anyhow!("Unknown category: {}", category));
        }
        if let Some(idx) = PRESETS.iter().position(|p| p.name == name) {
            self.apply_preset(idx);
            Ok(())
        } else {
            Err(anyhow!("Unknown Mythos preset: {}", name))
        }
    }

    fn current_preset(&self) -> Option<ThemePreset> {
        let p = &PRESETS[self.active_preset];
        Some(ThemePreset::new(p.id, p.name, "Mythos Field")
            .with_renderer(RendererType::Mythos))
    }

    fn has_presets(&self) -> bool {
        true
    }
}
