//! Virtual Stage bloom — an additive HDR-style glow post pass for the
//! laser-heavy stage preview.
//!
//! egui composites alpha-over: crossing laser beams layer instead of summing,
//! so the preview can't fake the bright haze a real laser fan throws into fog.
//! This module adds the missing step — a GL post-process run inside an egui
//! paint callback, after the stage is drawn:
//!   1. blit the drawn stage region into a capture texture;
//!   2. bright-pass it — keep only what is brighter than a threshold;
//!   3. blur that with a separable multi-pass Gaussian (ping-pong FBOs);
//!   4. additively composite the blurred bloom back over the stage.
//!
//! Operator-only surface: it runs only while the virtual stage is on and does
//! not touch the streamed / NDI / recorded output.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use glow::HasContext;

/// Capture resolution for the drawn stage. The bloom is blurry by nature, so a
/// fixed mid-res buffer is plenty and avoids window-resize bookkeeping.
const SCENE_W: i32 = 1280;
const SCENE_H: i32 = 720;
/// Blur runs at half res — cheaper, and a wider/softer blur per tap.
const BLUR_W: i32 = 640;
const BLUR_H: i32 = 360;
/// Separable H+V passes. Each iteration widens its step, so the glow falls off
/// smoothly over a wide radius without a huge per-pass tap count.
const BLUR_ITERATIONS: usize = 3;
/// Luma above this blooms; below is left dark. Tuned to catch laser beams and
/// fixture hotspots while leaving the dark stage and the video wall alone.
const BRIGHT_THRESHOLD: f32 = 0.62;
/// Additive gain on the composited bloom.
const BLOOM_GAIN: f32 = 1.2;

const VS_SRC: &str = r#"#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;
void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

// Bright-pass: keep colour scaled by how far its luma sits above the
// threshold, so the bloom seed has a soft knee instead of a hard cut.
const FS_BRIGHT: &str = r#"#version 330 core
in vec2 v_uv;
out vec4 frag;
uniform sampler2D u_tex;
uniform float u_threshold;
void main() {
    vec3 c = texture(u_tex, v_uv).rgb;
    float luma = max(c.r, max(c.g, c.b));
    float k = clamp((luma - u_threshold) / max(1.0 - u_threshold, 1e-3), 0.0, 1.0);
    frag = vec4(c * k, 1.0);
}
"#;

// One axis of a separable 9-tap Gaussian; u_dir is the texel step.
const FS_BLUR: &str = r#"#version 330 core
in vec2 v_uv;
out vec4 frag;
uniform sampler2D u_tex;
uniform vec2 u_dir;
void main() {
    float w0 = 0.227027;
    float w1 = 0.194595;
    float w2 = 0.121622;
    float w3 = 0.054054;
    float w4 = 0.016216;
    vec3 c = texture(u_tex, v_uv).rgb * w0;
    c += texture(u_tex, v_uv + u_dir * 1.0).rgb * w1;
    c += texture(u_tex, v_uv - u_dir * 1.0).rgb * w1;
    c += texture(u_tex, v_uv + u_dir * 2.0).rgb * w2;
    c += texture(u_tex, v_uv - u_dir * 2.0).rgb * w2;
    c += texture(u_tex, v_uv + u_dir * 3.0).rgb * w3;
    c += texture(u_tex, v_uv - u_dir * 3.0).rgb * w3;
    c += texture(u_tex, v_uv + u_dir * 4.0).rgb * w4;
    c += texture(u_tex, v_uv - u_dir * 4.0).rgb * w4;
    frag = vec4(c, 1.0);
}
"#;

// Composite: the blurred bloom, gained, drawn with additive blend.
const FS_COPY: &str = r#"#version 330 core
in vec2 v_uv;
out vec4 frag;
uniform sampler2D u_tex;
uniform float u_gain;
void main() {
    frag = vec4(texture(u_tex, v_uv).rgb * u_gain, 1.0);
}
"#;

/// Owns the bloom FBOs/textures and the three post-process shaders.
pub struct BloomPass {
    gl: Arc<glow::Context>,
    /// Drawn stage, blitted from the default framebuffer.
    scene_fbo: glow::Framebuffer,
    scene_tex: glow::Texture,
    /// Bright-pass output and the even side of the blur ping-pong.
    bright_fbo: glow::Framebuffer,
    bright_tex: glow::Texture,
    /// The odd side of the blur ping-pong.
    pong_fbo: glow::Framebuffer,
    pong_tex: glow::Texture,
    prog_bright: glow::Program,
    prog_blur: glow::Program,
    prog_copy: glow::Program,
    u_bright_tex: Option<glow::UniformLocation>,
    u_bright_threshold: Option<glow::UniformLocation>,
    u_blur_tex: Option<glow::UniformLocation>,
    u_blur_dir: Option<glow::UniformLocation>,
    u_copy_tex: Option<glow::UniformLocation>,
    u_copy_gain: Option<glow::UniformLocation>,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

impl BloomPass {
    pub fn new(gl: Arc<glow::Context>) -> Result<Self> {
        unsafe {
            let (scene_fbo, scene_tex) = make_target(&gl, SCENE_W, SCENE_H)?;
            let (bright_fbo, bright_tex) = make_target(&gl, BLUR_W, BLUR_H)?;
            let (pong_fbo, pong_tex) = make_target(&gl, BLUR_W, BLUR_H)?;

            let prog_bright = compile_program(&gl, VS_SRC, FS_BRIGHT)?;
            let prog_blur = compile_program(&gl, VS_SRC, FS_BLUR)?;
            let prog_copy = compile_program(&gl, VS_SRC, FS_COPY)?;

            let u_bright_tex = gl.get_uniform_location(prog_bright, "u_tex");
            let u_bright_threshold = gl.get_uniform_location(prog_bright, "u_threshold");
            let u_blur_tex = gl.get_uniform_location(prog_blur, "u_tex");
            let u_blur_dir = gl.get_uniform_location(prog_blur, "u_dir");
            let u_copy_tex = gl.get_uniform_location(prog_copy, "u_tex");
            let u_copy_gain = gl.get_uniform_location(prog_copy, "u_gain");

            // Static fullscreen quad: every pass renders the whole target.
            let vao = gl
                .create_vertex_array()
                .map_err(|e| anyhow!("bloom vao: {}", e))?;
            let vbo = gl
                .create_buffer()
                .map_err(|e| anyhow!("bloom vbo: {}", e))?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let verts: [f32; 16] = [
                -1.0, -1.0, 0.0, 0.0, // pos.xy, uv.xy
                1.0, -1.0, 1.0, 0.0,
                1.0, 1.0, 1.0, 1.0,
                -1.0, 1.0, 0.0, 1.0,
            ];
            let bytes =
                std::slice::from_raw_parts(verts.as_ptr() as *const u8, verts.len() * 4);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            let stride = 4 * 4;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            Ok(Self {
                gl,
                scene_fbo,
                scene_tex,
                bright_fbo,
                bright_tex,
                pong_fbo,
                pong_tex,
                prog_bright,
                prog_blur,
                prog_copy,
                u_bright_tex,
                u_bright_threshold,
                u_blur_tex,
                u_blur_dir,
                u_copy_tex,
                u_copy_gain,
                vao,
                vbo,
            })
        }
    }

    /// Run the bloom over the drawn stage and composite it back additively.
    /// `viewport` is `[x, y, w, h]` of the stage region in physical pixels,
    /// GL bottom-left origin. Must be called inside an egui paint callback,
    /// after the stage shapes have been drawn.
    pub fn apply(&self, viewport: [i32; 4]) {
        let [vx, vy, vw, vh] = viewport;
        if vw <= 0 || vh <= 0 {
            return;
        }
        let gl = &self.gl;
        unsafe {
            // 1. capture the drawn stage region from the default framebuffer.
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.scene_fbo));
            gl.blit_framebuffer(
                vx,
                vy,
                vx + vw,
                vy + vh,
                0,
                0,
                SCENE_W,
                SCENE_H,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );

            // Offscreen passes overwrite, never blend.
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);

            // 2. bright-pass: scene -> bright (downscaled to blur res).
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.bright_fbo));
            gl.viewport(0, 0, BLUR_W, BLUR_H);
            gl.use_program(Some(self.prog_bright));
            gl.bind_texture(glow::TEXTURE_2D, Some(self.scene_tex));
            if let Some(l) = &self.u_bright_tex {
                gl.uniform_1_i32(Some(l), 0);
            }
            if let Some(l) = &self.u_bright_threshold {
                gl.uniform_1_f32(Some(l), BRIGHT_THRESHOLD);
            }
            gl.draw_arrays(glow::TRIANGLE_FAN, 0, 4);

            // 3. separable Gaussian, ping-ponging bright <-> pong. Each
            //    iteration widens its step for a smooth wide falloff.
            gl.use_program(Some(self.prog_blur));
            if let Some(l) = &self.u_blur_tex {
                gl.uniform_1_i32(Some(l), 0);
            }
            for i in 0..BLUR_ITERATIONS {
                let step = 1.0 + i as f32;
                // horizontal: bright_tex -> pong_fbo
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.pong_fbo));
                gl.bind_texture(glow::TEXTURE_2D, Some(self.bright_tex));
                if let Some(l) = &self.u_blur_dir {
                    gl.uniform_2_f32(Some(l), step / BLUR_W as f32, 0.0);
                }
                gl.draw_arrays(glow::TRIANGLE_FAN, 0, 4);
                // vertical: pong_tex -> bright_fbo
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.bright_fbo));
                gl.bind_texture(glow::TEXTURE_2D, Some(self.pong_tex));
                if let Some(l) = &self.u_blur_dir {
                    gl.uniform_2_f32(Some(l), 0.0, step / BLUR_H as f32);
                }
                gl.draw_arrays(glow::TRIANGLE_FAN, 0, 4);
            }
            // bright_tex now holds the finished bloom.

            // 4. additively composite the bloom back over the stage.
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(vx, vy, vw, vh);
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE);
            gl.use_program(Some(self.prog_copy));
            gl.bind_texture(glow::TEXTURE_2D, Some(self.bright_tex));
            if let Some(l) = &self.u_copy_tex {
                gl.uniform_1_i32(Some(l), 0);
            }
            if let Some(l) = &self.u_copy_gain {
                gl.uniform_1_f32(Some(l), BLOOM_GAIN);
            }
            gl.draw_arrays(glow::TRIANGLE_FAN, 0, 4);

            // 5. restore the GL state egui-glow expects for the rest of paint.
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE_MINUS_DST_ALPHA,
                glow::ONE,
            );
            gl.enable(glow::SCISSOR_TEST);
        }
    }
}

impl Drop for BloomPass {
    fn drop(&mut self) {
        let gl = &self.gl;
        unsafe {
            gl.delete_framebuffer(self.scene_fbo);
            gl.delete_texture(self.scene_tex);
            gl.delete_framebuffer(self.bright_fbo);
            gl.delete_texture(self.bright_tex);
            gl.delete_framebuffer(self.pong_fbo);
            gl.delete_texture(self.pong_tex);
            gl.delete_program(self.prog_bright);
            gl.delete_program(self.prog_blur);
            gl.delete_program(self.prog_copy);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
        }
    }
}

/// Create an RGBA8 texture + an FBO with that texture as colour attachment 0.
unsafe fn make_target(
    gl: &glow::Context,
    w: i32,
    h: i32,
) -> Result<(glow::Framebuffer, glow::Texture)> {
    let tex = gl
        .create_texture()
        .map_err(|e| anyhow!("bloom texture: {}", e))?;
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA8 as i32,
        w,
        h,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        None,
    );
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    // CLAMP_TO_EDGE so the Gaussian does not wrap-sample at the texture edge.
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

    let fbo = gl
        .create_framebuffer()
        .map_err(|e| anyhow!("bloom fbo: {}", e))?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(tex),
        0,
    );
    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    gl.bind_texture(glow::TEXTURE_2D, None);
    if status != glow::FRAMEBUFFER_COMPLETE {
        gl.delete_framebuffer(fbo);
        gl.delete_texture(tex);
        return Err(anyhow!("bloom FBO incomplete: status {}", status));
    }
    Ok((fbo, tex))
}

fn compile_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> Result<glow::Program> {
    unsafe {
        let vs = gl
            .create_shader(glow::VERTEX_SHADER)
            .map_err(|e| anyhow!("bloom vertex shader: {}", e))?;
        gl.shader_source(vs, vs_src);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            let log = gl.get_shader_info_log(vs);
            gl.delete_shader(vs);
            return Err(anyhow!("bloom vertex shader compile: {}", log));
        }

        let fs = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .map_err(|e| anyhow!("bloom fragment shader: {}", e))?;
        gl.shader_source(fs, fs_src);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            let log = gl.get_shader_info_log(fs);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            return Err(anyhow!("bloom fragment shader compile: {}", log));
        }

        let program = gl
            .create_program()
            .map_err(|e| anyhow!("bloom program: {}", e))?;
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.link_program(program);
        let linked = gl.get_program_link_status(program);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        if !linked {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(anyhow!("bloom program link: {}", log));
        }
        Ok(program)
    }
}
