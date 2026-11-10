//! Native output window for projectors, LED walls, and secondary monitors
//!
//! Zero-copy GPU texture sharing via IOSurface.
//! Main app renders to IOSurface, output window reads from same IOSurface.

#![cfg(target_os = "macos")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use parking_lot::Mutex;

use cocoa::appkit::{
    NSWindow, NSWindowStyleMask, NSBackingStoreType, NSScreen,
    NSView, NSOpenGLView, NSOpenGLPixelFormat,
};
use cocoa::base::{id, nil, NO};
use cocoa::foundation::{NSRect, NSPoint, NSSize, NSString, NSAutoreleasePool};
use objc::{msg_send, sel, sel_impl, class};

use super::{SharedIOSurface, FrameBuffer};

// CGL functions for IOSurface texture binding
#[link(name = "OpenGL", kind = "framework")]
extern "C" {
    fn CGLGetCurrentContext() -> *mut std::ffi::c_void;
    fn CGLTexImageIOSurface2D(
        ctx: *mut std::ffi::c_void,
        target: u32,
        internal_format: u32,
        width: i32,
        height: i32,
        format: u32,
        type_: u32,
        iosurface: *mut std::ffi::c_void,
        plane: u32,
    ) -> i32;
}

// IOSurface lookup function
#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceLookup(surface_id: u32) -> *mut std::ffi::c_void;
}

// CoreFoundation release function
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *mut std::ffi::c_void);
}

const GL_TEXTURE_RECTANGLE: u32 = 0x84F5;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_RGBA: u32 = 0x1908;
const GL_BGRA: u32 = 0x80E1;
const GL_UNSIGNED_INT_8_8_8_8_REV: u32 = 0x8367;
const GL_UNSIGNED_BYTE: u32 = 0x1401;

/// Wrapper to make Cocoa id pointer Send+Sync
struct SendableId(id);
unsafe impl Send for SendableId {}
unsafe impl Sync for SendableId {}

/// Native output window using Cocoa NSWindow with IOSurface zero-copy
pub struct NativeOutputWindow {
    running: Arc<AtomicBool>,
    fullscreen: Arc<AtomicBool>,
    needs_context_update: Arc<AtomicBool>,
    window: Arc<Mutex<Option<SendableId>>>,
    gl_context: Arc<Mutex<Option<SendableId>>>,
    render_thread: Option<JoinHandle<()>>,
}

impl NativeOutputWindow {
    pub fn new(
        shared_surface: Arc<Mutex<Option<SharedIOSurface>>>,
        frame_number: Arc<AtomicU32>,
        _frame_buffer: Arc<Mutex<FrameBuffer>>,  // Unused in zero-copy mode
        _frame_ready: Arc<AtomicBool>,            // Unused in zero-copy mode
    ) -> anyhow::Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let fullscreen = Arc::new(AtomicBool::new(false));
        let needs_context_update = Arc::new(AtomicBool::new(false));
        let window = Arc::new(Mutex::new(None));
        let gl_context_arc = Arc::new(Mutex::new(None));

        let window_clone = window.clone();
        let gl_context_clone = gl_context_arc.clone();

        unsafe {
            let _pool = NSAutoreleasePool::new(nil);

            let screen: id = NSScreen::mainScreen(nil);
            let screen_frame: NSRect = msg_send![screen, frame];

            let width = 1280.0;
            let height = 720.0;
            let x = (screen_frame.size.width - width) / 2.0;
            let y = (screen_frame.size.height - height) / 2.0;

            let frame = NSRect::new(
                NSPoint::new(x, y),
                NSSize::new(width, height),
            );

            let style = NSWindowStyleMask::NSTitledWindowMask
                | NSWindowStyleMask::NSClosableWindowMask
                | NSWindowStyleMask::NSResizableWindowMask
                | NSWindowStyleMask::NSMiniaturizableWindowMask;

            let ns_window: id = NSWindow::alloc(nil).initWithContentRect_styleMask_backing_defer_(
                frame,
                style,
                NSBackingStoreType::NSBackingStoreBuffered,
                NO,
            );

            let title = NSString::alloc(nil).init_str("REACT Output");
            ns_window.setTitle_(title);
            ns_window.setReleasedWhenClosed_(NO);
            ns_window.makeKeyAndOrderFront_(nil);
            ns_window.setLevel_(1);

            // Enable native macOS fullscreen support (green button + Cmd+Ctrl+F)
            const NS_WINDOW_COLLECTION_BEHAVIOR_FULL_SCREEN_PRIMARY: u64 = 1 << 7;
            let _: () = msg_send![ns_window, setCollectionBehavior: NS_WINDOW_COLLECTION_BEHAVIOR_FULL_SCREEN_PRIMARY];

            // Core Profile for modern GL
            let attrs: &[u32] = &[
                cocoa::appkit::NSOpenGLPFAOpenGLProfile as u32, 0x3200,
                cocoa::appkit::NSOpenGLPFAColorSize as u32, 24,
                cocoa::appkit::NSOpenGLPFAAlphaSize as u32, 8,
                cocoa::appkit::NSOpenGLPFADoubleBuffer as u32,
                cocoa::appkit::NSOpenGLPFAAccelerated as u32,
                0,
            ];
            let pixel_format: id = NSOpenGLPixelFormat::alloc(nil)
                .initWithAttributes_(attrs);

            if pixel_format == nil {
                return Err(anyhow::anyhow!("Failed to create OpenGL pixel format"));
            }

            let content_rect: NSRect = msg_send![ns_window, contentRectForFrameRect:frame];
            let gl_view: id = NSOpenGLView::alloc(nil)
                .initWithFrame_pixelFormat_(content_rect, pixel_format);

            if gl_view == nil {
                return Err(anyhow::anyhow!("Failed to create OpenGL view"));
            }

            // Enable best resolution (HiDPI/Retina) - this makes convertRectToBacking work correctly
            let _: () = msg_send![gl_view, setWantsBestResolutionOpenGLSurface: cocoa::base::YES];

            ns_window.setContentView_(gl_view);
            let gl_context: id = msg_send![gl_view, openGLContext];

            *window_clone.lock() = Some(SendableId(ns_window));
            *gl_context_clone.lock() = Some(SendableId(gl_context));

            log::info!("[OutputWindow] Created with zero-copy IOSurface support");
        }

        let running_clone = running.clone();
        let fullscreen_render = fullscreen.clone();
        let needs_update_render = needs_context_update.clone();
        let window_render = window.clone();
        let gl_context_render = gl_context_arc.clone();

        let render_thread = thread::Builder::new()
            .name("output-render".into())
            .spawn(move || {
                render_loop_iosurface(
                    shared_surface,
                    frame_number,
                    running_clone,
                    fullscreen_render,
                    needs_update_render,
                    window_render,
                    gl_context_render,
                );
            })?;

        Ok(Self {
            running,
            fullscreen,
            needs_context_update,
            window,
            gl_context: gl_context_arc,
            render_thread: Some(render_thread),
        })
    }

    pub fn close(mut self) {
        self.running.store(false, Ordering::SeqCst);

        unsafe {
            if let Some(SendableId(ns_window)) = self.window.lock().take() {
                let _pool = NSAutoreleasePool::new(nil);
                let _: () = msg_send![ns_window, close];
            }
        }

        if let Some(handle) = self.render_thread.take() {
            let _ = handle.join();
        }
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        let current = self.fullscreen.load(Ordering::SeqCst);
        if current == fullscreen {
            log::info!("[OutputWindow] Fullscreen already {}", fullscreen);
            return;
        }

        log::info!("[OutputWindow] Setting fullscreen to {}", fullscreen);
        self.fullscreen.store(fullscreen, Ordering::SeqCst);
        self.needs_context_update.store(true, Ordering::SeqCst);
        // The render thread will pick up the fullscreen change via needs_context_update
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen.load(Ordering::SeqCst)
    }
}

impl Drop for NativeOutputWindow {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        unsafe {
            if let Some(SendableId(ns_window)) = self.window.lock().take() {
                let _pool = NSAutoreleasePool::new(nil);
                let _: () = msg_send![ns_window, close];
            }
        }

        if let Some(handle) = self.render_thread.take() {
            let _ = handle.join();
        }
    }
}

fn load_gl_functions() {
    gl::load_with(|symbol| {
        let symbol_name = std::ffi::CString::new(symbol).unwrap();
        unsafe {
            let lib = libc::dlopen(
                b"/System/Library/Frameworks/OpenGL.framework/OpenGL\0".as_ptr() as *const _,
                libc::RTLD_LAZY,
            );
            if lib.is_null() {
                std::ptr::null()
            } else {
                let ptr = libc::dlsym(lib, symbol_name.as_ptr());
                libc::dlclose(lib);
                ptr
            }
        }
    });
}

unsafe fn compile_shader(shader_type: u32, source: &str) -> Result<u32, String> {
    let shader = gl::CreateShader(shader_type);
    let c_str = std::ffi::CString::new(source).unwrap();
    gl::ShaderSource(shader, 1, &c_str.as_ptr(), std::ptr::null());
    gl::CompileShader(shader);

    let mut success = 0;
    gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut success);
    if success == 0 {
        let mut len = 0;
        gl::GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut len);
        let mut buffer = vec![0u8; len as usize];
        gl::GetShaderInfoLog(shader, len, std::ptr::null_mut(), buffer.as_mut_ptr() as *mut _);
        let error = String::from_utf8_lossy(&buffer);
        gl::DeleteShader(shader);
        return Err(format!("Shader compile error: {}", error));
    }
    Ok(shader)
}

/// Create shader for GL_TEXTURE_RECTANGLE (IOSurface textures use non-normalized coords)
/// Uses bicubic interpolation for high-quality upscaling on large screens
unsafe fn create_rect_blit_program() -> Result<u32, String> {
    // GLSL 150 with sampler2DRect for IOSurface texture
    // Uses texture coordinate scaling/offset for fill-with-crop mode:
    // - uTexScale: which portion of texture to sample (1.0 = full, <1 = crop)
    // - uTexOffset: offset to center the cropped region
    // NOTE: Using explicit layout locations to match VAO attribute indices
    let vs_src = r#"#version 150
        #extension GL_ARB_explicit_attrib_location : enable
        layout(location = 0) in vec2 aPos;
        layout(location = 1) in vec2 aTexCoord;
        out vec2 vTexCoord;
        out vec2 vNormCoord;  // Normalized 0-1 coordinates for upscale detection
        uniform vec2 uTexSize;
        uniform vec2 uTexScale;   // Scale factor (1.0 = full texture, <1.0 = sample subset)
        uniform vec2 uTexOffset;  // Offset to center the sampled region
        void main() {
            gl_Position = vec4(aPos, 0.0, 1.0);
            // Scale and offset the texture coordinates, then convert to pixels
            vec2 scaledCoord = aTexCoord * uTexScale + uTexOffset;
            vTexCoord = scaledCoord * uTexSize;
            vNormCoord = scaledCoord;
        }
    "#;

    // Bicubic interpolation + CAS sharpening for crisp output on big screens
    // Uses Catmull-Rom spline for upscaling, AMD CAS for sharpening
    let fs_src = r#"#version 150
        in vec2 vTexCoord;
        in vec2 vNormCoord;
        out vec4 FragColor;
        uniform sampler2DRect uTexture;
        uniform vec2 uTexSize;
        uniform vec2 uViewportSize;  // Output viewport size in pixels

        // CAS sharpening strength (0.0 = off, 1.0 = maximum)
        const float CAS_SHARPNESS = 0.5;

        // Catmull-Rom spline weight function
        vec4 cubic(float v) {
            vec4 n = vec4(1.0, 2.0, 3.0, 4.0) - v;
            vec4 s = n * n * n;
            float x = s.x;
            float y = s.y - 4.0 * s.x;
            float z = s.z - 4.0 * s.y + 6.0 * s.x;
            float w = 6.0 - x - y - z;
            return vec4(x, y, z, w) * (1.0 / 6.0);
        }

        // Bicubic texture sample using 4x4 neighborhood
        vec4 textureBicubic(sampler2DRect sampler, vec2 texCoords) {
            vec2 texSize = uTexSize;
            vec2 invTexSize = 1.0 / texSize;

            // Get fractional part of texel position
            vec2 fxy = fract(texCoords);
            texCoords = floor(texCoords);

            // Calculate bicubic weights
            vec4 xcubic = cubic(fxy.x);
            vec4 ycubic = cubic(fxy.y);

            // Combine weights into sample offsets
            vec4 c = vec4(texCoords.x - 0.5, texCoords.x + 1.5,
                          texCoords.y - 0.5, texCoords.y + 1.5);

            vec4 s = vec4(xcubic.x + xcubic.y, xcubic.z + xcubic.w,
                          ycubic.x + ycubic.y, ycubic.z + ycubic.w);

            vec4 offset = c + vec4(xcubic.y, xcubic.w, ycubic.y, ycubic.w) / s;

            // Sample using hardware bilinear for efficiency (4 samples instead of 16)
            vec4 sample0 = texture(sampler, vec2(offset.x, offset.z));
            vec4 sample1 = texture(sampler, vec2(offset.y, offset.z));
            vec4 sample2 = texture(sampler, vec2(offset.x, offset.w));
            vec4 sample3 = texture(sampler, vec2(offset.y, offset.w));

            float sx = s.x / (s.x + s.y);
            float sy = s.z / (s.z + s.w);

            return mix(
                mix(sample3, sample2, sx),
                mix(sample1, sample0, sx),
                sy
            );
        }

        // AMD Contrast Adaptive Sharpening (CAS) - sharpens without halos
        vec3 applyCAS(vec3 center, vec2 texCoord) {
            // Sample 4 neighbors (cross pattern)
            vec3 n = texture(uTexture, texCoord + vec2(0.0, -1.0)).rgb;
            vec3 s = texture(uTexture, texCoord + vec2(0.0, 1.0)).rgb;
            vec3 e = texture(uTexture, texCoord + vec2(1.0, 0.0)).rgb;
            vec3 w = texture(uTexture, texCoord + vec2(-1.0, 0.0)).rgb;

            // Find min and max in neighborhood
            vec3 minRGB = min(center, min(min(n, s), min(e, w)));
            vec3 maxRGB = max(center, max(max(n, s), max(e, w)));

            // Calculate sharpening amount based on local contrast
            // Low contrast = more sharpening, high contrast = less (avoids halos)
            vec3 rcpM = 1.0 / (maxRGB + 0.001);
            vec3 amp = clamp(min(minRGB, 2.0 - maxRGB) * rcpM, 0.0, 1.0);
            amp = sqrt(amp);

            // Sharpening weight (negative = sharpen)
            float peak = -1.0 / mix(8.0, 5.0, CAS_SHARPNESS);
            vec3 wN = amp * peak;
            vec3 wS = amp * peak;
            vec3 wE = amp * peak;
            vec3 wW = amp * peak;

            // Apply sharpening: center + weighted neighbors
            vec3 result = center + n * wN + s * wS + e * wE + w * wW;
            result /= (1.0 + wN + wS + wE + wW);

            return clamp(result, 0.0, 1.0);
        }

        void main() {
            // Determine upscale factor
            float upscaleFactor = max(uViewportSize.x / uTexSize.x, uViewportSize.y / uTexSize.y);

            vec4 color;
            // Use bicubic for upscaling (factor > 1.1), bilinear otherwise
            if (upscaleFactor > 1.1) {
                color = textureBicubic(uTexture, vTexCoord);
                // Apply CAS sharpening when upscaling to restore crispness
                color.rgb = applyCAS(color.rgb, vTexCoord);
            } else {
                // Direct sample for 1:1 or downscale (hardware filtering is fine)
                color = texture(uTexture, vTexCoord);
            }
            FragColor = color;
        }
    "#;

    let vs = compile_shader(gl::VERTEX_SHADER, vs_src)?;
    let fs = compile_shader(gl::FRAGMENT_SHADER, fs_src)?;

    let program = gl::CreateProgram();
    gl::AttachShader(program, vs);
    gl::AttachShader(program, fs);
    gl::LinkProgram(program);

    let mut success = 0;
    gl::GetProgramiv(program, gl::LINK_STATUS, &mut success);
    if success == 0 {
        let mut len = 0;
        gl::GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut len);
        let mut buffer = vec![0u8; len as usize];
        gl::GetProgramInfoLog(program, len, std::ptr::null_mut(), buffer.as_mut_ptr() as *mut _);
        let error = String::from_utf8_lossy(&buffer);
        gl::DeleteProgram(program);
        gl::DeleteShader(vs);
        gl::DeleteShader(fs);
        return Err(format!("Program link error: {}", error));
    }

    gl::DeleteShader(vs);
    gl::DeleteShader(fs);

    Ok(program)
}

unsafe fn create_quad_vao() -> (u32, u32) {
    // Vertex format: x, y, u, v
    // Flip V coordinate (1.0 - v) to correct upside-down image
    let vertices: [f32; 16] = [
        -1.0, -1.0,   0.0, 0.0,  // bottom-left -> top-left of texture
         1.0, -1.0,   1.0, 0.0,  // bottom-right -> top-right of texture
        -1.0,  1.0,   0.0, 1.0,  // top-left -> bottom-left of texture
         1.0,  1.0,   1.0, 1.0,  // top-right -> bottom-right of texture
    ];

    let mut vao = 0u32;
    let mut vbo = 0u32;

    gl::GenVertexArrays(1, &mut vao);
    gl::GenBuffers(1, &mut vbo);

    gl::BindVertexArray(vao);
    gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
    gl::BufferData(
        gl::ARRAY_BUFFER,
        (vertices.len() * std::mem::size_of::<f32>()) as isize,
        vertices.as_ptr() as *const _,
        gl::STATIC_DRAW,
    );

    gl::VertexAttribPointer(0, 2, gl::FLOAT, gl::FALSE, 4 * std::mem::size_of::<f32>() as i32, std::ptr::null());
    gl::EnableVertexAttribArray(0);

    gl::VertexAttribPointer(1, 2, gl::FLOAT, gl::FALSE, 4 * std::mem::size_of::<f32>() as i32, (2 * std::mem::size_of::<f32>()) as *const _);
    gl::EnableVertexAttribArray(1);

    gl::BindVertexArray(0);

    (vao, vbo)
}

/// Apply fullscreen state change to the output window
/// Uses native macOS fullscreen via performSelectorOnMainThread for thread safety
unsafe fn apply_fullscreen_change(ns_window: id, _fullscreen: bool) {
    let _pool = NSAutoreleasePool::new(nil);

    log::info!("[OutputWindow] Toggling native fullscreen");

    // Use performSelectorOnMainThread to safely call toggleFullScreen on main thread
    let toggle_sel = sel!(toggleFullScreen:);
    let _: () = msg_send![ns_window, performSelectorOnMainThread:toggle_sel withObject:nil waitUntilDone:NO];
}

/// Zero-copy render loop using IOSurface
fn render_loop_iosurface(
    shared_surface: Arc<Mutex<Option<SharedIOSurface>>>,
    frame_number: Arc<AtomicU32>,
    running: Arc<AtomicBool>,
    fullscreen: Arc<AtomicBool>,
    _needs_update: Arc<AtomicBool>,
    window: Arc<Mutex<Option<SendableId>>>,
    gl_context: Arc<Mutex<Option<SendableId>>>,
) {
    log::info!("[OutputWindow] Zero-copy render thread started");

    let mut gl_initialized = false;
    let mut program: Option<u32> = None;
    let mut vao: Option<u32> = None;
    let mut tex_size_loc: i32 = -1;
    let mut tex_scale_loc: i32 = -1;
    let mut tex_offset_loc: i32 = -1;
    let mut viewport_size_loc: i32 = -1;
    let mut iosurface_texture: Option<u32> = None;
    let mut bound_surface_id: u32 = 0;
    let mut bound_width: u32 = 0;
    let mut bound_height: u32 = 0;
    let mut last_frame: u32 = 0;
    let mut current_fullscreen = false;
    let mut last_vp_width: i32 = 0;
    let mut last_vp_height: i32 = 0;

    while running.load(Ordering::SeqCst) {
        let ctx_guard = gl_context.lock();
        let window_guard = window.lock();

        let (ctx, ns_window) = match (&*ctx_guard, &*window_guard) {
            (Some(SendableId(c)), Some(SendableId(w))) if *c != nil && *w != nil => (*c, *w),
            _ => {
                drop(ctx_guard);
                drop(window_guard);
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
        };

        unsafe {
            let _: () = msg_send![ctx, makeCurrentContext];

            if !gl_initialized {
                load_gl_functions();

                match create_rect_blit_program() {
                    Ok(prog) => {
                        program = Some(prog);
                        let tex_size_name = std::ffi::CString::new("uTexSize").unwrap();
                        let tex_scale_name = std::ffi::CString::new("uTexScale").unwrap();
                        let tex_offset_name = std::ffi::CString::new("uTexOffset").unwrap();
                        let viewport_size_name = std::ffi::CString::new("uViewportSize").unwrap();
                        tex_size_loc = gl::GetUniformLocation(prog, tex_size_name.as_ptr());
                        tex_scale_loc = gl::GetUniformLocation(prog, tex_scale_name.as_ptr());
                        tex_offset_loc = gl::GetUniformLocation(prog, tex_offset_name.as_ptr());
                        viewport_size_loc = gl::GetUniformLocation(prog, viewport_size_name.as_ptr());
                        log::info!("[OutputWindow] Shader uniforms: uTexSize={}, uTexScale={}, uTexOffset={}, uViewportSize={}",
                            tex_size_loc, tex_scale_loc, tex_offset_loc, viewport_size_loc);
                    }
                    Err(e) => {
                        log::error!("[OutputWindow] Failed to create shader: {}", e);
                    }
                }

                let (va, _vb) = create_quad_vao();
                vao = Some(va);

                // Create texture for IOSurface binding with linear filtering for upscaling
                let mut tex = 0u32;
                gl::GenTextures(1, &mut tex);
                gl::BindTexture(GL_TEXTURE_RECTANGLE, tex);
                // Enable linear filtering for smooth upscaling (bicubic shader uses this)
                gl::TexParameteri(GL_TEXTURE_RECTANGLE, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
                gl::TexParameteri(GL_TEXTURE_RECTANGLE, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
                gl::TexParameteri(GL_TEXTURE_RECTANGLE, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE as i32);
                gl::TexParameteri(GL_TEXTURE_RECTANGLE, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE as i32);
                gl::BindTexture(GL_TEXTURE_RECTANGLE, 0);
                iosurface_texture = Some(tex);

                // Create a dummy 1x1 GL_TEXTURE_2D to satisfy macOS driver checks.
                // The driver validates both TEXTURE_2D and TEXTURE_RECTANGLE on the same
                // unit even when using sampler2DRect, causing "GLD_TEXTURE_INDEX_2D unloadable"
                // warnings if no valid 2D texture is bound.
                let mut dummy_tex = 0u32;
                gl::GenTextures(1, &mut dummy_tex);
                gl::BindTexture(GL_TEXTURE_2D, dummy_tex);
                let black_pixel: [u8; 4] = [0, 0, 0, 255];
                gl::TexImage2D(
                    GL_TEXTURE_2D,
                    0,
                    GL_RGBA as i32,
                    1, 1, 0,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    black_pixel.as_ptr() as *const _,
                );
                gl::TexParameteri(GL_TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
                gl::TexParameteri(GL_TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);
                // Keep the dummy texture bound on unit 0 to prevent driver warnings
                log::debug!("[OutputWindow] Created dummy TEXTURE_2D {} to prevent driver warnings", dummy_tex);

                gl_initialized = true;
                log::info!("[OutputWindow] GL initialized (zero-copy mode)");
            }

            // Handle fullscreen toggle - apply changes when state differs
            let want_fullscreen = fullscreen.load(Ordering::SeqCst);
            if want_fullscreen != current_fullscreen {
                log::info!("[OutputWindow] Fullscreen change detected: {} -> {}", current_fullscreen, want_fullscreen);
                apply_fullscreen_change(ns_window, want_fullscreen);
                current_fullscreen = want_fullscreen;
            }

            // Get IOSurface info
            let surface_guard = shared_surface.lock();
            let (surface_id, tex_width, tex_height) = if let Some(ref surface) = *surface_guard {
                (surface.surface_id(), surface.width, surface.height)
            } else {
                drop(surface_guard);
                drop(ctx_guard);
                drop(window_guard);
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            };
            drop(surface_guard);

            // Always rebind IOSurface to texture each frame to handle surface recreation
            // This prevents crashes when the main app resizes/recreates the surface
            if let Some(tex) = iosurface_texture {
                // Lookup IOSurface by ID - get fresh reference each frame
                let iosurface_ref = IOSurfaceLookup(surface_id);
                if iosurface_ref.is_null() {
                    // Surface doesn't exist yet or was destroyed - skip this frame
                    let _: () = msg_send![class!(NSOpenGLContext), clearCurrentContext];
                    drop(ctx_guard);
                    drop(window_guard);
                    thread::sleep(std::time::Duration::from_millis(8));
                    continue;
                }

                // Rebind texture to IOSurface
                gl::BindTexture(GL_TEXTURE_RECTANGLE, tex);

                let cgl_ctx = CGLGetCurrentContext();
                let result = CGLTexImageIOSurface2D(
                    cgl_ctx,
                    GL_TEXTURE_RECTANGLE,
                    GL_RGBA,
                    tex_width as i32,
                    tex_height as i32,
                    GL_BGRA,
                    GL_UNSIGNED_INT_8_8_8_8_REV,
                    iosurface_ref,
                    0,
                );

                // Release our reference to the IOSurface (CGLTexImageIOSurface2D retains it)
                CFRelease(iosurface_ref);

                if result != 0 {
                    log::error!("[OutputWindow] CGLTexImageIOSurface2D failed: {}", result);
                    gl::BindTexture(GL_TEXTURE_RECTANGLE, 0);
                    let _: () = msg_send![class!(NSOpenGLContext), clearCurrentContext];
                    drop(ctx_guard);
                    drop(window_guard);
                    thread::sleep(std::time::Duration::from_millis(8));
                    continue;
                }

                // Log only when dimensions change
                if bound_surface_id != surface_id || bound_width != tex_width || bound_height != tex_height {
                    log::info!("[OutputWindow] Bound IOSurface {} ({}x{}) - zero-copy!", surface_id, tex_width, tex_height);
                    bound_surface_id = surface_id;
                    bound_width = tex_width;
                    bound_height = tex_height;
                }

                gl::BindTexture(GL_TEXTURE_RECTANGLE, 0);
            }

            // Check for new frame
            let current_frame = frame_number.load(Ordering::Acquire);
            if current_frame == last_frame && last_frame > 0 {
                // No new frame, skip render
                let _: () = msg_send![class!(NSOpenGLContext), clearCurrentContext];
                drop(ctx_guard);
                drop(window_guard);
                thread::sleep(std::time::Duration::from_millis(2));
                continue;
            }
            last_frame = current_frame;

            // Get viewport size in PIXELS (not points) for HiDPI/Retina support
            let content_view: id = msg_send![ns_window, contentView];
            if content_view == nil {
                let _: () = msg_send![class!(NSOpenGLContext), clearCurrentContext];
                drop(ctx_guard);
                drop(window_guard);
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }

            // Use convertRectToBacking: to get the actual pixel dimensions
            // This correctly handles different display scale factors when window moves
            let bounds: NSRect = msg_send![content_view, bounds];
            let backing_rect: NSRect = msg_send![content_view, convertRectToBacking:bounds];
            let vp_width = backing_rect.size.width as i32;
            let vp_height = backing_rect.size.height as i32;

            if vp_width <= 0 || vp_height <= 0 {
                let _: () = msg_send![class!(NSOpenGLContext), clearCurrentContext];
                drop(ctx_guard);
                drop(window_guard);
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }

            // Detect viewport size change and update GL context
            // This syncs the drawable buffer with the window size when moving between displays
            if vp_width != last_vp_width || vp_height != last_vp_height {
                log::info!("[OutputWindow] Viewport changed: {}x{} -> {}x{}",
                    last_vp_width, last_vp_height, vp_width, vp_height);

                // Use performSelectorOnMainThread to safely update context
                let update_sel = sel!(update);
                let _: () = msg_send![ctx, performSelectorOnMainThread:update_sel withObject:nil waitUntilDone:NO];

                last_vp_width = vp_width;
                last_vp_height = vp_height;

                // Skip this frame to let the update take effect
                let _: () = msg_send![class!(NSOpenGLContext), clearCurrentContext];
                drop(ctx_guard);
                drop(window_guard);
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }

            // Render at full pixel resolution
            gl::Viewport(0, 0, vp_width, vp_height);
            gl::ClearColor(0.0, 0.0, 0.0, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT);

            if let (Some(prog), Some(va), Some(tex)) = (program, vao, iosurface_texture) {
                if bound_width > 0 && bound_height > 0 {
                    gl::UseProgram(prog);
                    gl::BindVertexArray(va);
                    gl::ActiveTexture(gl::TEXTURE0);
                    gl::BindTexture(GL_TEXTURE_RECTANGLE, tex);

                    // Fill screen while preserving aspect ratio (crop mode)
                    // Adjust which portion of the texture we sample to fill viewport
                    let src_aspect = bound_width as f32 / bound_height as f32;  // 16:9 = 1.78
                    let dst_aspect = vp_width as f32 / vp_height as f32;

                    // Calculate texture scale/offset for fill-with-crop:
                    // - scale < 1.0 means sample only a portion of texture (crop)
                    // - offset centers the sampled region
                    let (tex_scale_x, tex_scale_y, tex_offset_x, tex_offset_y) = if dst_aspect > src_aspect {
                        // Viewport is wider than source - sample full width, crop top/bottom
                        let scale_y = src_aspect / dst_aspect;  // <1.0, sample less height
                        let offset_y = (1.0 - scale_y) / 2.0;   // Center vertically
                        (1.0_f32, scale_y, 0.0_f32, offset_y)
                    } else {
                        // Viewport is taller than source - sample full height, crop left/right
                        let scale_x = dst_aspect / src_aspect;  // <1.0, sample less width
                        let offset_x = (1.0 - scale_x) / 2.0;   // Center horizontally
                        (scale_x, 1.0_f32, offset_x, 0.0_f32)
                    };

                    // Debug: log once when scale changes significantly
                    static mut LAST_SCALE: (f32, f32) = (0.0, 0.0);
                    if (tex_scale_x - LAST_SCALE.0).abs() > 0.01 || (tex_scale_y - LAST_SCALE.1).abs() > 0.01 {
                        log::info!("[OutputWindow] Aspect: src={:.3} dst={:.3}, TexScale=({:.3}, {:.3}), TexOffset=({:.3}, {:.3})",
                            src_aspect, dst_aspect, tex_scale_x, tex_scale_y, tex_offset_x, tex_offset_y);
                        LAST_SCALE = (tex_scale_x, tex_scale_y);
                    }

                    gl::Uniform2f(tex_size_loc, bound_width as f32, bound_height as f32);
                    gl::Uniform2f(tex_scale_loc, tex_scale_x, tex_scale_y);
                    gl::Uniform2f(tex_offset_loc, tex_offset_x, tex_offset_y);
                    gl::Uniform2f(viewport_size_loc, vp_width as f32, vp_height as f32);
                    gl::DrawArrays(gl::TRIANGLE_STRIP, 0, 4);

                    gl::BindTexture(GL_TEXTURE_RECTANGLE, 0);
                    gl::BindVertexArray(0);
                    gl::UseProgram(0);
                }
            }

            let _: () = msg_send![ctx, flushBuffer];
            let _: () = msg_send![class!(NSOpenGLContext), clearCurrentContext];
        }

        drop(ctx_guard);
        drop(window_guard);
        thread::sleep(std::time::Duration::from_millis(8));
    }

    log::info!("[OutputWindow] Zero-copy render thread stopped");
}
