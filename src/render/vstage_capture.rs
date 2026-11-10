//! Virtual Stage screen capture — puts REACT's live rendered frame onto the
//! virtual stage LED back-wall.
//!
//! REACT renders its visual straight to the screen. To show that frame on the
//! back-wall (an arbitrary perspective quad inside the egui overlay) we:
//!   1. blit the rendered screen into a fixed-size capture texture — one cheap
//!      GPU->GPU copy, only while the virtual stage is on;
//!   2. draw that texture as a 4-corner quad at the projected back-wall corners.
//!
//! Both steps run inside egui paint callbacks. Nothing here runs when the
//! virtual stage is toggled off.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use glow::HasContext;

/// Capture resolution. A back-wall preview does not need full render res.
const CAPTURE_W: i32 = 1280;
const CAPTURE_H: i32 = 720;

const VS_SRC: &str = r#"#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;
void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

const FS_SRC: &str = r#"#version 330 core
in vec2 v_uv;
out vec4 frag;
uniform sampler2D u_tex;
void main() {
    // Force opaque — the captured frame fills the LED wall.
    frag = vec4(texture(u_tex, v_uv).rgb, 1.0);
}
"#;

/// Owns the capture FBO/texture and the textured-quad shader used to paint the
/// live REACT frame onto the virtual stage back-wall.
pub struct VstageCapture {
    gl: Arc<glow::Context>,
    capture_fbo: glow::Framebuffer,
    capture_tex: glow::Texture,
    program: glow::Program,
    u_tex: Option<glow::UniformLocation>,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    /// True once at least one screen capture has landed in the texture.
    has_frame: bool,
}

impl VstageCapture {
    pub fn new(gl: Arc<glow::Context>) -> Result<Self> {
        unsafe {
            let capture_tex = gl
                .create_texture()
                .map_err(|e| anyhow!("vstage capture texture: {}", e))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(capture_tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                CAPTURE_W,
                CAPTURE_H,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

            let capture_fbo = gl
                .create_framebuffer()
                .map_err(|e| anyhow!("vstage capture fbo: {}", e))?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(capture_fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(capture_tex),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(capture_fbo);
                gl.delete_texture(capture_tex);
                return Err(anyhow!("vstage capture FBO incomplete: status {}", status));
            }

            let program = compile_program(&gl, VS_SRC, FS_SRC)?;
            let u_tex = gl.get_uniform_location(program, "u_tex");

            let vao = gl
                .create_vertex_array()
                .map_err(|e| anyhow!("vstage vao: {}", e))?;
            let vbo = gl
                .create_buffer()
                .map_err(|e| anyhow!("vstage vbo: {}", e))?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            // 4 vertices * 4 floats (pos.xy, uv.xy) * 4 bytes — refilled per draw.
            gl.buffer_data_size(glow::ARRAY_BUFFER, 4 * 4 * 4, glow::DYNAMIC_DRAW);
            let stride = 4 * 4;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            Ok(Self {
                gl,
                capture_fbo,
                capture_tex,
                program,
                u_tex,
                vao,
                vbo,
                has_frame: false,
            })
        }
    }

    /// Blit the live REACT render (default framebuffer) into the capture
    /// texture. Viewport rect is in physical pixels (GL bottom-left origin).
    pub fn capture(&mut self, vp_x: i32, vp_y: i32, vp_w: i32, vp_h: i32) {
        if vp_w <= 0 || vp_h <= 0 {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.capture_fbo));
            gl.blit_framebuffer(
                vp_x,
                vp_y,
                vp_x + vp_w,
                vp_y + vp_h,
                0,
                0,
                CAPTURE_W,
                CAPTURE_H,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.has_frame = true;
    }

    /// Whether a frame has been captured at least once (controls back-wall
    /// fallback to the placeholder).
    pub fn has_frame(&self) -> bool {
        self.has_frame
    }

    /// Draw the captured frame as a quad. `corners` are clip-space [-1, 1] in
    /// top-left, top-right, bottom-right, bottom-left order, relative to
    /// `viewport` (`[x, y, w, h]`, physical pixels, GL bottom-left origin).
    pub fn draw(&self, corners: [[f32; 2]; 4], viewport: [i32; 4]) {
        if !self.has_frame {
            return;
        }
        // pos.xy, uv.xy — GL textures are bottom-up, so the top corners take
        // v = 1 and the bottom corners v = 0.
        let verts: [f32; 16] = [
            corners[0][0], corners[0][1], 0.0, 1.0,
            corners[1][0], corners[1][1], 1.0, 1.0,
            corners[2][0], corners[2][1], 1.0, 0.0,
            corners[3][0], corners[3][1], 0.0, 0.0,
        ];
        let gl = &self.gl;
        unsafe {
            let bytes =
                std::slice::from_raw_parts(verts.as_ptr() as *const u8, verts.len() * 4);

            // The clip-space corners are relative to this viewport.
            gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);

            gl.use_program(Some(self.program));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.capture_tex));
            if let Some(loc) = &self.u_tex {
                gl.uniform_1_i32(Some(loc), 0);
            }

            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLE_FAN, 0, 4);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            gl.enable(glow::SCISSOR_TEST);
        }
    }
}

impl Drop for VstageCapture {
    fn drop(&mut self) {
        let gl = &self.gl;
        unsafe {
            gl.delete_framebuffer(self.capture_fbo);
            gl.delete_texture(self.capture_tex);
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
        }
    }
}

fn compile_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> Result<glow::Program> {
    unsafe {
        let vs = gl
            .create_shader(glow::VERTEX_SHADER)
            .map_err(|e| anyhow!("vstage vertex shader: {}", e))?;
        gl.shader_source(vs, vs_src);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            let log = gl.get_shader_info_log(vs);
            gl.delete_shader(vs);
            return Err(anyhow!("vstage vertex shader compile: {}", log));
        }

        let fs = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .map_err(|e| anyhow!("vstage fragment shader: {}", e))?;
        gl.shader_source(fs, fs_src);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            let log = gl.get_shader_info_log(fs);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            return Err(anyhow!("vstage fragment shader compile: {}", log));
        }

        let program = gl
            .create_program()
            .map_err(|e| anyhow!("vstage program: {}", e))?;
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.link_program(program);
        let linked = gl.get_program_link_status(program);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        if !linked {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(anyhow!("vstage program link: {}", log));
        }
        Ok(program)
    }
}
