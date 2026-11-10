//! LED Wall Renderer - Realistic festival LED panel simulation
//!
//! Features:
//! - True per-pixel control with individual state
//! - Multiple pattern modes (spectrum, bars, pulse, wave, etc.)
//! - Realistic LED pixel appearance with glow and gaps
//! - Per-pixel smoothing with attack/release
//! - Kick-triggered effects (flash, ripple)
//! - INSTANCED RENDERING: 1 draw call for all 576 pixels!

use anyhow::Result;
use eframe::glow::{self, HasContext};
use std::sync::Arc;

use super::theme::{ThemeRenderer, ThemePreset, AudioAnalysis, RendererType};

/// LED Wall visualization patterns
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LedPattern {
    /// Classic spectrum analyzer - columns = frequency bands
    #[default]
    Spectrum,
    /// Horizontal bars reacting to bass/mid/high
    Bars,
    /// Center pulse radiating outward on kicks
    Pulse,
    /// Horizontal wave modulated by bass
    Wave,
    /// Dual VU meter style (bass left, high right)
    VuMeter,
    /// Fire effect growing with energy
    Fire,
    /// Pixels raining down, speed based on energy
    Rain,
}

impl LedPattern {
    /// Get all available patterns
    pub fn all() -> &'static [LedPattern] {
        &[
            LedPattern::Spectrum,
            LedPattern::Bars,
            LedPattern::Pulse,
            LedPattern::Wave,
            LedPattern::VuMeter,
            LedPattern::Fire,
            LedPattern::Rain,
        ]
    }

    /// Get display name
    pub fn name(&self) -> &'static str {
        match self {
            LedPattern::Spectrum => "Spectrum Analyzer",
            LedPattern::Bars => "Horizontal Bars",
            LedPattern::Pulse => "Center Pulse",
            LedPattern::Wave => "Bass Wave",
            LedPattern::VuMeter => "VU Meter",
            LedPattern::Fire => "Fire",
            LedPattern::Rain => "Rain",
        }
    }

    /// Get pattern ID for serialization
    pub fn id(&self) -> &'static str {
        match self {
            LedPattern::Spectrum => "spectrum",
            LedPattern::Bars => "bars",
            LedPattern::Pulse => "pulse",
            LedPattern::Wave => "wave",
            LedPattern::VuMeter => "vu_meter",
            LedPattern::Fire => "fire",
            LedPattern::Rain => "rain",
        }
    }

    /// Parse from ID
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "spectrum" => Some(LedPattern::Spectrum),
            "bars" => Some(LedPattern::Bars),
            "pulse" => Some(LedPattern::Pulse),
            "wave" => Some(LedPattern::Wave),
            "vu_meter" => Some(LedPattern::VuMeter),
            "fire" => Some(LedPattern::Fire),
            "rain" => Some(LedPattern::Rain),
            _ => None,
        }
    }

    /// Convert to ThemePreset
    pub fn to_preset(&self) -> ThemePreset {
        ThemePreset {
            id: format!("led_wall:{}", self.id()),
            name: self.name().to_string(),
            category: "LED Wall".to_string(),
            subcategory: None,
            renderer_type: Some(RendererType::LedWall),
        }
    }
}

/// Grid dimensions - 32x18 for widescreen LED wall (576 pixels)
const GRID_COLS: usize = 32;
const GRID_ROWS: usize = 18;
const TOTAL_PIXELS: usize = GRID_COLS * GRID_ROWS;

/// Number of frequency bands for spectrum
const NUM_BANDS: usize = 32;

/// Per-pixel state
#[derive(Clone, Copy)]
struct Pixel {
    /// Current brightness (smoothed, 0-1)
    brightness: f32,
    /// Target brightness (0-1)
    target: f32,
    /// Base hue (0-1)
    hue: f32,
    /// Saturation (0-1)
    saturation: f32,
    /// Phase offset for wave effects
    phase: f32,
    /// Individual attack rate
    attack: f32,
    /// Individual release rate
    release: f32,
}

impl Default for Pixel {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            target: 0.0,
            hue: 0.0,
            saturation: 1.0,
            phase: 0.0,
            attack: 0.5,
            release: 0.08,
        }
    }
}

/// GPU-side per-instance data (packed for efficient upload)
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceData {
    /// Position (x, y) in 0-1 range
    pos: [f32; 2],
    /// Color (r, g, b)
    color: [f32; 3],
    /// Brightness (0-1)
    brightness: f32,
    /// Glow intensity
    glow: f32,
    /// Padding for alignment
    _padding: f32,
}

/// LED Wall renderer with instanced rendering (1 draw call!)
pub struct LedWallRenderer {
    gl: Arc<glow::Context>,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    instance_vbo: glow::Buffer,

    /// Per-pixel state array
    pixels: Vec<Pixel>,

    /// Instance data for GPU upload
    instance_data: Vec<InstanceData>,

    /// Synthesized spectrum bands (interpolated from bass/mid/high)
    spectrum: [f32; NUM_BANDS],

    /// Previous spectrum for smoothing
    prev_spectrum: [f32; NUM_BANDS],

    /// Time accumulator
    time: f32,

    /// Kick flash intensity (decays over time)
    kick_flash: f32,

    /// Kick ripple center and age
    ripple_age: f32,

    /// Random seed for noise
    noise_seed: u32,

    /// Current energy level (smoothed)
    energy: f32,

    /// Strobe intensity (0 = normal pixels, 1 = full strobe)
    strobe: f32,

    /// Strobe color hue
    strobe_hue: f32,

    /// Strobe flicker phase (for rapid on/off)
    strobe_phase: f32,

    /// Master fade (0 = black, 1 = full brightness) for silence fade
    master_fade: f32,

    /// Current visualization pattern
    current_pattern: LedPattern,

    /// Smoothed bass level for patterns
    bass_smooth: f32,
    /// Smoothed mid level for patterns
    mid_smooth: f32,
    /// Smoothed high level for patterns
    high_smooth: f32,

    /// Rain pattern: per-column drop positions
    rain_drops: [f32; GRID_COLS],
    /// Rain pattern: per-column drop speeds
    rain_speeds: [f32; GRID_COLS],

    /// Fire pattern: heat map
    fire_heat: Vec<f32>,

    initialized: bool,
}

impl LedWallRenderer {
    pub fn new(gl: Arc<glow::Context>) -> Self {
        let (program, vao, vbo, instance_vbo) = unsafe { Self::init_gl(&gl) };

        // Initialize pixels with varied properties
        let mut pixels = vec![Pixel::default(); TOTAL_PIXELS];
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;
                let col_norm = col as f32 / (GRID_COLS - 1) as f32;

                pixels[idx].hue = col_norm * 0.7;
                pixels[idx].saturation = 0.9;
                pixels[idx].phase = ((col * 7 + row * 13) % 100) as f32 / 100.0;

                let variation = ((col + row * 3) % 10) as f32 / 100.0;
                pixels[idx].attack = 0.4 + variation;
                pixels[idx].release = 0.06 + variation * 0.5;
            }
        }

        // Pre-allocate instance data
        let instance_data = vec![InstanceData::default(); TOTAL_PIXELS];

        // Initialize rain drops with random positions
        let mut rain_drops = [0.0f32; GRID_COLS];
        let mut rain_speeds = [0.0f32; GRID_COLS];
        for i in 0..GRID_COLS {
            rain_drops[i] = (i as f32 * 3.7) % (GRID_ROWS as f32);
            rain_speeds[i] = 0.3 + (i as f32 * 0.13) % 0.5;
        }

        Self {
            gl,
            program,
            vao,
            vbo,
            instance_vbo,
            pixels,
            instance_data,
            spectrum: [0.0; NUM_BANDS],
            prev_spectrum: [0.0; NUM_BANDS],
            time: 0.0,
            kick_flash: 0.0,
            ripple_age: 1.0,
            noise_seed: 12345,
            energy: 0.0,
            strobe: 0.0,
            strobe_hue: 0.0,
            strobe_phase: 0.0,
            master_fade: 1.0,
            current_pattern: LedPattern::default(),
            bass_smooth: 0.0,
            mid_smooth: 0.0,
            high_smooth: 0.0,
            rain_drops,
            rain_speeds,
            fire_heat: vec![0.0; TOTAL_PIXELS],
            initialized: true,
        }
    }

    /// Set the current pattern
    pub fn set_pattern(&mut self, pattern: LedPattern) {
        if self.current_pattern != pattern {
            log::info!("[LedWall] Switching to pattern: {:?}", pattern);
            self.current_pattern = pattern;
        }
    }

    /// Get the current pattern
    pub fn current_pattern(&self) -> LedPattern {
        self.current_pattern
    }

    unsafe fn init_gl(gl: &glow::Context) -> (glow::Program, glow::VertexArray, glow::Buffer, glow::Buffer) {
        // Vertex shader with instancing - uses gl_InstanceID
        let vs_src = r#"
            #version 140

            in vec2 a_pos;
            in vec2 a_inst_pos;
            in vec3 a_inst_color;
            in float a_inst_brightness;
            in float a_inst_glow;

            uniform vec2 u_pixel_size;
            uniform float u_strobe;

            out vec3 v_color;
            out vec2 v_uv;
            out float v_brightness;
            out float v_glow;
            out float v_strobe;

            void main() {
                // Position this instance's quad
                vec2 pos = a_pos * u_pixel_size + a_inst_pos;
                gl_Position = vec4(pos * 2.0 - 1.0, 0.0, 1.0);

                v_color = a_inst_color;
                v_uv = a_pos;
                v_brightness = a_inst_brightness;
                v_glow = a_inst_glow;
                v_strobe = u_strobe;
            }
        "#;

        // Fragment shader - realistic LED pixel with hot spot, glow, and strobe
        let fs_src = r#"
            #version 140

            in vec3 v_color;
            in vec2 v_uv;
            in float v_brightness;
            in float v_glow;
            in float v_strobe;

            out vec4 frag_color;

            void main() {
                vec2 center = vec2(0.5);
                vec2 d = v_uv - center;
                float dist = length(d);

                // Pixel size expands during strobe
                float pixel_size = mix(0.48, 0.52, v_strobe);
                float corner = mix(0.04, 0.01, v_strobe);

                // Rounded rectangle mask
                vec2 size = vec2(pixel_size);
                vec2 q = abs(d) - size;
                float rect_dist = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - corner;

                float edge_soft = mix(0.02, 0.01, v_strobe);
                float pixel_mask = 1.0 - smoothstep(-edge_soft, edge_soft, rect_dist);

                // Hot spot in center
                float hotspot = exp(-dist * dist * 8.0);

                // Strobe boost
                float strobe_boost = 1.0 + v_strobe * 2.0;

                // Outer glow
                float glow_strength = v_glow + v_strobe * 1.5;
                float glow = exp(-dist * dist * 2.0) * glow_strength;

                // During strobe, push towards white
                vec3 strobe_color = mix(v_color, vec3(1.0), v_strobe * 0.5);

                // Combine
                float intensity = v_brightness * (0.7 + hotspot * 0.5) * strobe_boost;
                vec3 color = strobe_color * intensity * pixel_mask;
                color += strobe_color * glow * v_brightness;

                // Minimum dim state
                float dim = 0.02;
                color = max(color, v_color * dim * pixel_mask);
                color = min(color, vec3(3.0));

                frag_color = vec4(color, 1.0);
            }
        "#;

        // Compile shaders
        let vs = gl.create_shader(glow::VERTEX_SHADER).unwrap();
        gl.shader_source(vs, vs_src);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            log::error!("[LedWall] VS compile error: {}", gl.get_shader_info_log(vs));
        }

        let fs = gl.create_shader(glow::FRAGMENT_SHADER).unwrap();
        gl.shader_source(fs, fs_src);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            log::error!("[LedWall] FS compile error: {}", gl.get_shader_info_log(fs));
        }

        let program = gl.create_program().unwrap();
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            log::error!("[LedWall] Program link error: {}", gl.get_program_info_log(program));
        }

        gl.delete_shader(vs);
        gl.delete_shader(fs);

        // Create VAO
        let vao = gl.create_vertex_array().unwrap();
        gl.bind_vertex_array(Some(vao));

        // Quad vertices VBO
        let vbo = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

        let vertices: [f32; 12] = [
            0.0, 0.0,
            1.0, 0.0,
            1.0, 1.0,
            0.0, 0.0,
            1.0, 1.0,
            0.0, 1.0,
        ];

        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(&vertices),
            glow::STATIC_DRAW,
        );

        // Per-vertex position attribute
        let pos_loc = gl.get_attrib_location(program, "a_pos").unwrap();
        gl.enable_vertex_attrib_array(pos_loc);
        gl.vertex_attrib_pointer_f32(pos_loc, 2, glow::FLOAT, false, 8, 0);

        // Instance data VBO
        let instance_vbo = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(instance_vbo));

        // Allocate instance buffer (will be updated each frame)
        let instance_size = TOTAL_PIXELS * std::mem::size_of::<InstanceData>();
        gl.buffer_data_size(glow::ARRAY_BUFFER, instance_size as i32, glow::DYNAMIC_DRAW);

        // Instance attributes (stride = 32 bytes = sizeof(InstanceData))
        let stride = std::mem::size_of::<InstanceData>() as i32;

        // a_inst_pos (offset 0, 2 floats)
        let inst_pos_loc = gl.get_attrib_location(program, "a_inst_pos").unwrap();
        gl.enable_vertex_attrib_array(inst_pos_loc);
        gl.vertex_attrib_pointer_f32(inst_pos_loc, 2, glow::FLOAT, false, stride, 0);
        gl.vertex_attrib_divisor(inst_pos_loc, 1); // Per instance

        // a_inst_color (offset 8, 3 floats)
        let inst_color_loc = gl.get_attrib_location(program, "a_inst_color").unwrap();
        gl.enable_vertex_attrib_array(inst_color_loc);
        gl.vertex_attrib_pointer_f32(inst_color_loc, 3, glow::FLOAT, false, stride, 8);
        gl.vertex_attrib_divisor(inst_color_loc, 1);

        // a_inst_brightness (offset 20, 1 float)
        let inst_bright_loc = gl.get_attrib_location(program, "a_inst_brightness").unwrap();
        gl.enable_vertex_attrib_array(inst_bright_loc);
        gl.vertex_attrib_pointer_f32(inst_bright_loc, 1, glow::FLOAT, false, stride, 20);
        gl.vertex_attrib_divisor(inst_bright_loc, 1);

        // a_inst_glow (offset 24, 1 float)
        let inst_glow_loc = gl.get_attrib_location(program, "a_inst_glow").unwrap();
        gl.enable_vertex_attrib_array(inst_glow_loc);
        gl.vertex_attrib_pointer_f32(inst_glow_loc, 1, glow::FLOAT, false, stride, 24);
        gl.vertex_attrib_divisor(inst_glow_loc, 1);

        gl.bind_vertex_array(None);

        log::info!("[LedWall] Initialized {}x{} LED wall ({} pixels, INSTANCED - 1 draw call!)",
            GRID_COLS, GRID_ROWS, TOTAL_PIXELS);

        (program, vao, vbo, instance_vbo)
    }

    fn next_random(&mut self) -> f32 {
        self.noise_seed = self.noise_seed.wrapping_mul(1103515245).wrapping_add(12345);
        (self.noise_seed >> 16) as f32 / 65536.0
    }

    /// Synthesize a 32-band spectrum from bass/mid/high values
    fn synthesize_spectrum(&mut self, bass: f32, mid: f32, high: f32) {
        for i in 0..NUM_BANDS {
            let pos = i as f32 / (NUM_BANDS - 1) as f32;

            let value = if pos < 0.3 {
                let t = pos / 0.3;
                bass * (1.0 - t * 0.5)
            } else if pos < 0.7 {
                let t = (pos - 0.3) / 0.4;
                bass * 0.5 * (1.0 - t) + mid * t
            } else {
                let t = (pos - 0.7) / 0.3;
                mid * (1.0 - t) + high * t
            };

            // Smooth with previous
            self.spectrum[i] = self.prev_spectrum[i] * 0.3 + value * 0.7;
        }

        self.prev_spectrum = self.spectrum;
    }

    /// Update state from audio analysis
    pub fn update(&mut self, bass: f32, mid: f32, high: f32, kick: bool, dt: f32) {
        self.time += dt;

        // Smooth bass/mid/high for patterns
        self.bass_smooth += (bass - self.bass_smooth) * 0.3;
        self.mid_smooth += (mid - self.mid_smooth) * 0.3;
        self.high_smooth += (high - self.high_smooth) * 0.3;

        // Synthesize spectrum
        self.synthesize_spectrum(bass, mid, high);

        // Calculate energy
        let raw_energy = bass * 0.5 + mid * 0.3 + high * 0.2;
        if raw_energy > self.energy {
            self.energy += (raw_energy - self.energy) * 0.6;
        } else {
            self.energy += (raw_energy - self.energy) * 0.15;
        }

        // Strobe activation
        let strobe_threshold = 0.65;
        let target_strobe = if self.energy > strobe_threshold {
            ((self.energy - strobe_threshold) / (1.0 - strobe_threshold)).powf(0.5)
        } else {
            0.0
        };

        if target_strobe > self.strobe {
            self.strobe += (target_strobe - self.strobe) * 0.7;
        } else {
            self.strobe *= 0.75;
        }

        // Strobe flicker
        let flicker_speed = 15.0 + self.energy * 20.0;
        self.strobe_phase += dt * flicker_speed * std::f32::consts::TAU;
        if self.strobe_phase > std::f32::consts::TAU {
            self.strobe_phase -= std::f32::consts::TAU;
        }

        // Handle kick
        if kick && self.energy > 0.4 {
            self.strobe = 1.0;
            self.strobe_hue = self.time.fract();
        }

        if kick {
            self.kick_flash = 1.0;
            self.ripple_age = 0.0;
        }

        self.kick_flash *= 0.85;
        self.ripple_age += dt * 2.0;

        // Silence detection
        const SILENCE_THRESHOLD: f32 = 0.02;
        let has_audio = raw_energy > SILENCE_THRESHOLD;

        if has_audio {
            self.master_fade = (self.master_fade + dt * 3.0).min(1.0);
        } else {
            self.master_fade = (self.master_fade - dt * 0.5).max(0.0);
        }

        // Update pixels based on current pattern
        match self.current_pattern {
            LedPattern::Spectrum => self.update_pattern_spectrum(),
            LedPattern::Bars => self.update_pattern_bars(),
            LedPattern::Pulse => self.update_pattern_pulse(),
            LedPattern::Wave => self.update_pattern_wave(),
            LedPattern::VuMeter => self.update_pattern_vu_meter(),
            LedPattern::Fire => self.update_pattern_fire(dt),
            LedPattern::Rain => self.update_pattern_rain(dt),
        }
    }

    /// Spectrum analyzer pattern (classic)
    fn update_pattern_spectrum(&mut self) {
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;
                let pixel = &mut self.pixels[idx];

                let band_idx = col.min(NUM_BANDS - 1);
                let band_energy = self.spectrum[band_idx];

                let row_threshold = band_energy * GRID_ROWS as f32;
                let row_f = row as f32;

                let mut target = if row_f < row_threshold {
                    1.0
                } else if row_f < row_threshold + 1.0 {
                    row_threshold - row_f + 1.0
                } else {
                    0.0
                };

                // Add ripple on kick
                if self.ripple_age < 1.0 {
                    let center_x = GRID_COLS as f32 / 2.0;
                    let center_y = GRID_ROWS as f32 / 2.0;
                    let dx = col as f32 - center_x;
                    let dy = row as f32 - center_y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let ripple_radius = self.ripple_age * 20.0;
                    let ripple_width = 3.0;
                    let ripple = (1.0 - (dist - ripple_radius).abs().min(ripple_width) / ripple_width)
                                 * (1.0 - self.ripple_age);
                    target = (target + ripple * 0.5).min(1.0);
                }

                target = (target + self.kick_flash * 0.3).min(1.0);
                self.update_pixel(idx, target, col as f32 / GRID_COLS as f32 * 0.7, band_energy);
            }
        }
    }

    /// Horizontal bars pattern
    fn update_pattern_bars(&mut self) {
        let bass_rows = (self.bass_smooth * 6.0) as usize;
        let mid_rows = (self.mid_smooth * 6.0) as usize;
        let high_rows = (self.high_smooth * 6.0) as usize;

        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;

                // Bottom = bass (red), Middle = mid (green), Top = high (blue)
                let (target, hue) = if row < 6 {
                    // Bass zone (bottom)
                    let in_bar = row < bass_rows;
                    (if in_bar { 1.0 } else { 0.1 }, 0.0) // Red
                } else if row < 12 {
                    // Mid zone
                    let in_bar = (row - 6) < mid_rows;
                    (if in_bar { 1.0 } else { 0.1 }, 0.33) // Green
                } else {
                    // High zone (top)
                    let in_bar = (row - 12) < high_rows;
                    (if in_bar { 1.0 } else { 0.1 }, 0.66) // Blue
                };

                let target = (target + self.kick_flash * 0.3).min(1.0);
                self.update_pixel(idx, target, hue, self.energy);
            }
        }
    }

    /// Center pulse pattern
    fn update_pattern_pulse(&mut self) {
        let center_x = GRID_COLS as f32 / 2.0;
        let center_y = GRID_ROWS as f32 / 2.0;
        let max_dist = ((center_x * center_x) + (center_y * center_y)).sqrt();

        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;

                let dx = col as f32 - center_x;
                let dy = row as f32 - center_y;
                let dist = (dx * dx + dy * dy).sqrt();
                let norm_dist = dist / max_dist;

                // Expanding ring on kick
                let mut target = 0.0;
                if self.ripple_age < 1.5 {
                    let ring_pos = self.ripple_age;
                    let ring_width = 0.2;
                    let ring_dist = (norm_dist - ring_pos).abs();
                    if ring_dist < ring_width {
                        target = (1.0 - ring_dist / ring_width) * (1.0 - self.ripple_age / 1.5);
                    }
                }

                // Add energy-based glow from center
                let center_glow = (1.0 - norm_dist) * self.energy * 0.5;
                target = (target + center_glow + self.kick_flash * 0.2).min(1.0);

                let hue = (norm_dist * 0.3 + self.time * 0.1).fract();
                self.update_pixel(idx, target, hue, self.energy);
            }
        }
    }

    /// Wave pattern
    fn update_pattern_wave(&mut self) {
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;

                // Horizontal wave modulated by bass
                let wave_freq = 0.3;
                let wave_amp = self.bass_smooth * (GRID_ROWS as f32 / 2.0);
                let wave_center = GRID_ROWS as f32 / 2.0;
                let wave_y = wave_center + (self.time * 3.0 + col as f32 * wave_freq).sin() * wave_amp;

                let dist_from_wave = (row as f32 - wave_y).abs();
                let wave_width = 2.0 + self.mid_smooth * 3.0;

                let target = if dist_from_wave < wave_width {
                    1.0 - dist_from_wave / wave_width
                } else {
                    0.0
                };

                let target = (target + self.kick_flash * 0.2).min(1.0);
                let hue = (col as f32 / GRID_COLS as f32 * 0.5 + self.time * 0.05).fract();
                self.update_pixel(idx, target, hue, self.energy);
            }
        }
    }

    /// VU Meter pattern
    fn update_pattern_vu_meter(&mut self) {
        let half_cols = GRID_COLS / 2;

        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;

                // Left side = bass, Right side = high
                let (level, hue) = if col < half_cols {
                    (self.bass_smooth, 0.0) // Red for bass
                } else {
                    (self.high_smooth, 0.55) // Cyan for highs
                };

                let meter_height = (level * GRID_ROWS as f32) as usize;
                let target = if row < meter_height {
                    1.0
                } else if row == meter_height {
                    0.5
                } else {
                    0.05
                };

                let target = (target + self.kick_flash * 0.2).min(1.0);
                self.update_pixel(idx, target, hue, self.energy);
            }
        }
    }

    /// Fire pattern
    fn update_pattern_fire(&mut self, dt: f32) {
        // Add heat at bottom based on audio
        for col in 0..GRID_COLS {
            let idx = col; // Bottom row
            let heat_add = self.bass_smooth * 0.8 + self.energy * 0.5;
            self.fire_heat[idx] = (self.fire_heat[idx] + heat_add).min(1.0);
        }

        // Propagate heat upward with cooling
        let cooling = 0.15 * dt * 60.0;
        for row in (1..GRID_ROWS).rev() {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;
                let below_idx = (row - 1) * GRID_COLS + col;

                // Average heat from below with slight spread
                let left = if col > 0 { self.fire_heat[below_idx - 1] } else { 0.0 };
                let center = self.fire_heat[below_idx];
                let right = if col < GRID_COLS - 1 { self.fire_heat[below_idx + 1] } else { 0.0 };

                self.fire_heat[idx] = ((left * 0.2 + center * 0.6 + right * 0.2) - cooling).max(0.0);
            }
        }

        // Update pixels from heat map
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;
                let heat = self.fire_heat[idx];

                // Fire color: black -> red -> orange -> yellow -> white
                let hue = 0.0 + heat * 0.1; // Red to orange
                let sat = 1.0 - heat * 0.3;
                let target = heat;

                let pixel = &mut self.pixels[idx];
                pixel.target = (target + self.kick_flash * 0.2).min(1.0);
                let rate = if pixel.target > pixel.brightness { 0.5 } else { 0.1 };
                pixel.brightness += (pixel.target - pixel.brightness) * rate;
                pixel.hue = hue;
                pixel.saturation = sat;
            }
        }
    }

    /// Rain pattern
    fn update_pattern_rain(&mut self, dt: f32) {
        // Update drop positions
        let speed_mult = 1.0 + self.energy * 2.0;
        for col in 0..GRID_COLS {
            self.rain_drops[col] += self.rain_speeds[col] * speed_mult * dt * 30.0;
            if self.rain_drops[col] >= GRID_ROWS as f32 {
                self.rain_drops[col] = 0.0;
                // Randomize speed for next drop
                self.rain_speeds[col] = 0.3 + (self.next_random() * 0.5);
            }
        }

        // Reset some drops on kick
        if self.kick_flash > 0.9 {
            for col in 0..GRID_COLS {
                if self.next_random() > 0.7 {
                    self.rain_drops[col] = 0.0;
                }
            }
        }

        // Render drops
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;

                let drop_y = self.rain_drops[col];
                let dist = (row as f32 - drop_y).abs();
                let trail_len = 3.0 + self.energy * 2.0;

                let target = if dist < trail_len && row as f32 <= drop_y {
                    1.0 - dist / trail_len
                } else {
                    0.02
                };

                let target = (target + self.kick_flash * 0.1).min(1.0);
                let hue = 0.55 + col as f32 / GRID_COLS as f32 * 0.15; // Cyan to blue
                self.update_pixel(idx, target, hue, self.energy);
            }
        }
    }

    /// Helper to update a single pixel with smoothing
    fn update_pixel(&mut self, idx: usize, target: f32, hue: f32, energy: f32) {
        let pixel = &mut self.pixels[idx];
        pixel.target = target;

        let rate = if target > pixel.brightness { pixel.attack } else { pixel.release };
        pixel.brightness += (target - pixel.brightness) * rate;

        let energy_shift = energy * 0.1;
        let time_shift = (self.time * 0.05 + pixel.phase).sin() * 0.05;
        pixel.hue = (hue + energy_shift + time_shift).fract();
        pixel.saturation = 0.8 + energy * 0.2;
    }

    /// Render the LED wall with a single instanced draw call
    pub fn render(&mut self, _width: u32, _height: u32) {
        if !self.initialized || self.master_fade < 0.01 {
            return;
        }

        let gl = &self.gl;

        // Calculate strobe flicker
        let flicker = if self.strobe > 0.3 {
            let wave = self.strobe_phase.sin();
            let flicker_amount = (self.strobe - 0.3) / 0.7;
            if wave > 0.0 { 1.0 } else { 1.0 - flicker_amount * 0.9 }
        } else {
            1.0
        };
        let effective_strobe = self.strobe * flicker;

        // Calculate pixel sizes
        let gap = 0.003;
        let pixel_w = (1.0 - gap * (GRID_COLS + 1) as f32) / GRID_COLS as f32;
        let pixel_h = (1.0 - gap * (GRID_ROWS + 1) as f32) / GRID_ROWS as f32;

        // Update instance data on CPU
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let idx = row * GRID_COLS + col;
                let pixel = &self.pixels[idx];

                // Position
                let x = gap + col as f32 * (pixel_w + gap);
                let y = gap + row as f32 * (pixel_h + gap);

                // Color - use strobe hue during strobe
                let hue = if self.strobe > 0.5 {
                    self.strobe_hue * 0.7
                } else {
                    pixel.hue
                };
                let sat = pixel.saturation * (1.0 - self.strobe * 0.6);
                let rgb = hsv_to_rgb(hue, sat, 1.0);

                // Brightness with master fade
                let base_brightness = pixel.brightness + self.strobe * (1.0 - pixel.brightness);
                let brightness = base_brightness * self.master_fade;
                let glow = (base_brightness * base_brightness * 0.8 + self.strobe * 0.5) * self.master_fade;

                self.instance_data[idx] = InstanceData {
                    pos: [x, y],
                    color: rgb,
                    brightness,
                    glow,
                    _padding: 0.0,
                };
            }
        }

        unsafe {
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));

            // Additive blending for glow
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE);

            // Set uniforms
            let u_pixel_size = gl.get_uniform_location(self.program, "u_pixel_size");
            let u_strobe = gl.get_uniform_location(self.program, "u_strobe");
            gl.uniform_2_f32(u_pixel_size.as_ref(), pixel_w, pixel_h);
            gl.uniform_1_f32(u_strobe.as_ref(), effective_strobe);

            // Upload instance data
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instance_vbo));
            gl.buffer_sub_data_u8_slice(
                glow::ARRAY_BUFFER,
                0,
                bytemuck::cast_slice(&self.instance_data),
            );

            // SINGLE INSTANCED DRAW CALL for all 576 pixels!
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, TOTAL_PIXELS as i32);

            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.disable(glow::BLEND);
        }
    }

    pub fn cleanup(&mut self) {
        if self.initialized {
            unsafe {
                self.gl.delete_program(self.program);
                self.gl.delete_vertex_array(self.vao);
                self.gl.delete_buffer(self.vbo);
                self.gl.delete_buffer(self.instance_vbo);
            }
            self.initialized = false;
        }
    }
}

/// Convert HSV to RGB
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.fract() + 1.0).fract();
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);

    match i % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

impl Drop for LedWallRenderer {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// === ThemeRenderer trait implementation ===

impl ThemeRenderer for LedWallRenderer {
    fn renderer_type(&self) -> RendererType {
        RendererType::LedWall
    }

    fn init(&mut self, _gl: Arc<glow::Context>) -> Result<()> {
        // Already initialized in new()
        Ok(())
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn update(&mut self, audio: &AudioAnalysis, dt: f32) {
        // Call existing update with analyzed audio bands
        LedWallRenderer::update(self, audio.bass, audio.mid, audio.high, audio.kick, dt);
    }

    fn render(&mut self, width: u32, height: u32) {
        LedWallRenderer::render(self, width, height);
    }

    fn set_size(&mut self, _width: u32, _height: u32) {
        // LED Wall doesn't need size - it renders to the full FBO
        // The aspect ratio is handled in the shader
    }

    fn cleanup(&mut self) {
        LedWallRenderer::cleanup(self);
    }

    // === Preset/Pattern support ===

    fn scan_presets(&mut self) -> Vec<ThemePreset> {
        // Return all available patterns as presets
        LedPattern::all().iter().map(|p| p.to_preset()).collect()
    }

    fn get_categories(&self) -> Vec<String> {
        vec!["LED Wall".to_string()]
    }

    fn get_presets_in_category(&self, category: &str) -> Vec<ThemePreset> {
        if category == "LED Wall" {
            LedPattern::all().iter().map(|p| p.to_preset()).collect()
        } else {
            Vec::new()
        }
    }

    fn load_preset(&mut self, preset_id: &str) -> Result<()> {
        // Parse pattern ID from preset_id (format: "led_wall:pattern_id")
        let pattern_id = preset_id.strip_prefix("led_wall:").unwrap_or(preset_id);

        if let Some(pattern) = LedPattern::from_id(pattern_id) {
            self.set_pattern(pattern);
            Ok(())
        } else {
            anyhow::bail!("Unknown LED Wall pattern: {}", pattern_id)
        }
    }

    fn load_preset_by_name(&mut self, _category: &str, name: &str) -> Result<()> {
        // Find pattern by display name
        for pattern in LedPattern::all() {
            if pattern.name() == name {
                self.set_pattern(*pattern);
                return Ok(());
            }
        }
        anyhow::bail!("Unknown LED Wall pattern: {}", name)
    }

    fn current_preset(&self) -> Option<ThemePreset> {
        Some(self.current_pattern.to_preset())
    }

    fn set_random_preset(&mut self) -> Option<ThemePreset> {
        use rand::Rng;
        let patterns = LedPattern::all();
        let idx = rand::thread_rng().gen_range(0..patterns.len());
        let pattern = patterns[idx];
        self.set_pattern(pattern);
        Some(pattern.to_preset())
    }

    fn has_presets(&self) -> bool {
        true
    }
}
