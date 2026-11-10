//! Main render pipeline - manages FBO, projectM, video, and compositing
//!
//! Uses PBO (Pixel Buffer Object) for async texture uploads to avoid GPU stalls.
//! On macOS, uses CVOpenGLTextureCache for zero-copy GPU texture creation from
//! IOSurface-backed CVPixelBuffers (for 4K video performance).
//! Metal LED Wall uses IOSurface sharing via CGLTexImageIOSurface2D for zero-copy.

use anyhow::{anyhow, Result};
use eframe::glow::{self, HasContext};
use std::sync::Arc;

use super::{Compositor, ProjectMRenderer, RenderParams, LedWallRenderer, OverlayPosition, AudioAnalysis, ThemeRenderer, RendererType, HarmonicGeometryRenderer, MythosRenderer};
use super::harmonic_geometry::ShaderInitStep;

#[cfg(target_os = "macos")]
use super::MetalLedWall;

#[cfg(target_os = "macos")]
use super::led_wall_metal::IOSurfaceRef;

/// Wrapper for IOSurfaceRef that's safe to send across threads
/// IOSurface is a reference-counted CoreFoundation object that's thread-safe
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct SendableIOSurface(IOSurfaceRef);

#[cfg(target_os = "macos")]
unsafe impl Send for SendableIOSurface {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SendableIOSurface {}

#[cfg(target_os = "macos")]
impl SendableIOSurface {
    fn null() -> Self {
        Self(std::ptr::null_mut())
    }

    fn get(&self) -> IOSurfaceRef {
        self.0
    }

    fn set(&mut self, surface: IOSurfaceRef) {
        self.0 = surface;
    }
}

/// Render mode - determines which visualizer to use
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// projectM Milkdrop visualizer
    ProjectM,
    /// Custom LED Wall grid
    LedWall,
    /// REG Geometric Field - mathematical visualization
    HarmonicGeometry,
    /// Mythos - continuous field with Collatz modulation
    Mythos,
}

#[cfg(target_os = "macos")]
use crate::video::{TextureCache, RetainedPixelBuffer};

// CGLTexImageIOSurface2D for zero-copy IOSurface → OpenGL texture binding
#[cfg(target_os = "macos")]
type CGLContextObj = *mut std::ffi::c_void;
#[cfg(target_os = "macos")]
type CGLPixelFormatObj = *mut std::ffi::c_void;

#[cfg(target_os = "macos")]
const GL_TEXTURE_RECTANGLE: u32 = 0x84F5;
#[cfg(target_os = "macos")]
const GL_RGBA: u32 = 0x1908;
#[cfg(target_os = "macos")]
const GL_BGRA: u32 = 0x80E1;
#[cfg(target_os = "macos")]
const GL_UNSIGNED_INT_8_8_8_8_REV: u32 = 0x8367;

#[cfg(target_os = "macos")]
#[link(name = "OpenGL", kind = "framework")]
extern "C" {
    fn CGLGetCurrentContext() -> CGLContextObj;
    fn CGLTexImageIOSurface2D(
        ctx: CGLContextObj,
        target: u32,
        internal_format: u32,
        width: i32,
        height: i32,
        format: u32,
        typ: u32,
        iosurface: IOSurfaceRef,
        plane: u32,
    ) -> i32;
}

/// Number of PBOs for async texture upload (triple buffering for smoother 4K)
const NUM_PBOS: usize = 3;

/// Per-camera media entry combining texture, dimensions, and alpha for cache locality
struct CameraMediaEntry {
    texture: glow::Texture,
    width: u32,
    height: u32,
    alpha: f32,
}

/// Silence-driven fade-to-black for the projectM theme.
///
/// projectM (Milkdrop) has no audio gate of its own, so it keeps animating
/// during silence while the compel video and the lights both go dark. This is
/// a pure fade animator: it ramps `blackout_alpha` toward black whenever the
/// app's `SilenceGate` reports confirmed silence (`RenderParams::theme_silent`)
/// and back when audio returns. Keying off the same gate that fades the compels
/// keeps projectM, the compels, and the lights all darkening together — and it
/// respects the user-configured silence threshold without a second detector.
///
/// (Detection deliberately lives in the app's gate, not here: it runs on the
/// RAW pre-normalization band energy. The normalized `energy`/`energy_vis`
/// values are useless for silence — the adaptive normalizer lifts the noise
/// floor toward ~0.5 when the audio actually stops.)
#[derive(Default)]
struct ProjectMSilence {
    /// Fade weight: 0 = projectM fully visible, 1 = faded to black.
    blackout_alpha: f32,
}

impl ProjectMSilence {
    /// Ramp the fade toward black while `silent`, back toward visible otherwise.
    fn update(&mut self, silent: bool, dt: f32) {
        let target = if silent { 1.0 } else { 0.0 };
        // Slow fade in, faster fade out — premium feel, akin to the Geometric Field.
        let tau = if silent { 0.4 } else { 0.25 };
        let rate = 1.0 - (-dt / tau).exp();
        self.blackout_alpha += (target - self.blackout_alpha) * rate;
    }
}

/// Main render pipeline that composites projectM and video
pub struct RenderPipeline {
    /// OpenGL context
    gl: Arc<glow::Context>,

    /// Framebuffer object for offscreen rendering (currently unused, kept for future post-processing)
    fbo: glow::Framebuffer,

    /// Color texture attached to FBO
    fbo_texture: glow::Texture,

    /// Current FBO dimensions
    fbo_width: u32,
    fbo_height: u32,

    /// Virtual stage screen capture — live REACT frame for the LED back-wall.
    /// Lazily created the first time the virtual stage requests a capture.
    vstage_capture: Option<super::VstageCapture>,

    /// Virtual stage bloom post pass — additive glow for the laser fans.
    /// Lazily created the first time the stage requests a bloom pass.
    vstage_bloom: Option<super::BloomPass>,

    /// projectM renderer (initialized lazily on first render)
    projectm: Option<ProjectMRenderer>,

    /// Whether projectM initialization has been attempted
    projectm_init_attempted: bool,

    /// Silence fade-to-black state for projectM (projectM has no audio gate)
    projectm_silence: ProjectMSilence,

    /// Wall clock for the projectM silence fade — gives a real dt even if
    /// render_frame runs more than once per frame.
    silence_fade_clock: Option<std::time::Instant>,

    /// LED Wall renderer (OpenGL fallback)
    led_wall: Option<LedWallRenderer>,

    /// Geometric Field renderer (REG mathematical visualization)
    harmonic_geometry: Option<HarmonicGeometryRenderer>,

    /// Whether harmonic geometry initialization has been attempted
    harmonic_geometry_init_attempted: bool,

    /// Mythos renderer (continuous field with Collatz modulation)
    mythos: Option<MythosRenderer>,

    /// Whether Mythos initialization has been attempted
    mythos_init_attempted: bool,

    /// Metal LED Wall renderer (high performance, macOS only)
    #[cfg(target_os = "macos")]
    metal_led_wall: Option<MetalLedWall>,

    /// Texture for Metal LED Wall output (bound to IOSurface)
    #[cfg(target_os = "macos")]
    metal_led_texture: Option<glow::Texture>,

    /// Current IOSurface bound to metal_led_texture (to detect changes)
    #[cfg(target_os = "macos")]
    metal_led_iosurface: SendableIOSurface,

    /// Last dimensions of the Metal LED texture
    #[cfg(target_os = "macos")]
    metal_led_width: u32,
    #[cfg(target_os = "macos")]
    metal_led_height: u32,

    /// Current render mode
    render_mode: RenderMode,

    /// Video texture for decoded frames
    video_texture: Option<glow::Texture>,

    /// Video texture dimensions
    video_width: u32,
    video_height: u32,

    /// PBOs for async texture upload
    video_pbos: Vec<glow::Buffer>,

    /// Current PBO index for round-robin
    pbo_index: usize,

    /// Size of allocated PBOs (to detect resize)
    pbo_size: usize,

    /// Compositor for blending layers
    compositor: Compositor,

    /// Whether pipeline is initialized
    initialized: bool,

    /// Frame counter for debugging
    frame_count: u64,

    /// CVOpenGLTextureCache for zero-copy GPU texture creation (macOS only)
    #[cfg(target_os = "macos")]
    texture_cache: Option<TextureCache>,

    /// Current zero-copy video texture ID (from CVOpenGLTexture)
    #[cfg(target_os = "macos")]
    cv_video_texture: Option<u32>,

    /// Texture target for CV texture (GL_TEXTURE_RECTANGLE or GL_TEXTURE_2D)
    #[cfg(target_os = "macos")]
    cv_video_target: u32,

    /// CV texture dimensions
    #[cfg(target_os = "macos")]
    cv_video_width: u32,
    #[cfg(target_os = "macos")]
    cv_video_height: u32,

    /// Skin texture (TEXTURE_2D copy of video for harmonic geometry skin)
    /// On macOS, this is a copy of the CV TEXTURE_RECTANGLE converted to TEXTURE_2D
    skin_video_texture: Option<glow::Texture>,
    skin_video_fbo: Option<glow::Framebuffer>,
    skin_video_width: u32,
    skin_video_height: u32,

    /// Logo overlay texture
    logo_texture: Option<glow::Texture>,
    /// Logo texture dimensions
    logo_width: u32,
    logo_height: u32,
    /// Current logo path (to detect changes)
    current_logo_path: Option<String>,

    /// Track announcement texture
    track_texture: Option<glow::Texture>,
    /// Track texture dimensions
    track_tex_width: u32,
    track_tex_height: u32,
    /// Last rendered track info (to detect changes)
    last_track_title: String,
    last_track_artist: String,
    /// Font for text rendering
    font_data: Option<Vec<u8>>,

    /// Preset browser texture
    preset_browser_texture: Option<glow::Texture>,
    /// Preset browser texture dimensions
    preset_browser_tex_width: u32,
    preset_browser_tex_height: u32,
    /// Last rendered preset browser info (to detect changes)
    last_preset_browser_name: String,
    last_preset_browser_index: usize,

    /// Cached harmonic field state (for UI display in any render mode)
    /// Updated from audio analysis regardless of render mode
    cached_harmonic_state: super::HarmonicFieldState,

    /// Time accumulator for phase calculation (in all modes)
    phase_time: f32,

    // ===== Camera Media Background Layers =====
    /// Per-camera media entries (texture, dimensions, alpha combined for cache locality)
    camera_media: std::collections::HashMap<u32, CameraMediaEntry>,

    /// Stable ordering for camera compositing (back to front)
    camera_composite_order: Vec<u32>,

    /// Camera compositor configuration (group settings)
    camera_compositor_config: crate::camera::CameraCompositorConfig,

    /// Frame counter for scheduler epoch advancement
    camera_frame_counter: u32,

    /// Active camera for HarmonicGeometry (camera_index, texture, width, height, alpha)
    active_camera_for_hg: Option<(u32, glow::Texture, u32, u32, f32)>,
}

impl RenderPipeline {
    /// Create a new render pipeline
    pub fn new(gl: Arc<glow::Context>) -> Result<Self> {
        log::info!("Creating render pipeline");

        // Create a 1x1 dummy texture on TEXTURE_2D to prevent macOS driver warnings.
        // The driver checks all texture units when a shader runs, and will log
        // "GLD_TEXTURE_INDEX_2D is unloadable" if no valid texture is bound.
        // We create this texture once at startup and leave it bound when no other
        // texture is being used.
        unsafe {
            let dummy = gl.create_texture()
                .map_err(|e| anyhow!("Failed to create dummy texture: {}", e))?;
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(dummy));
            let black: [u8; 4] = [0, 0, 0, 255];
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1, 1, 0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&black),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            // Leave this texture bound as a fallback
            log::debug!("[RenderPipeline] Created dummy texture to prevent driver warnings");
        }

        // Create FBO for offscreen rendering
        let (fbo, fbo_texture) = unsafe {
            let fbo = gl.create_framebuffer()
                .map_err(|e| anyhow!("Failed to create FBO: {}", e))?;

            let texture = gl.create_texture()
                .map_err(|e| anyhow!("Failed to create FBO texture: {}", e))?;

            // Initialize texture with default size (will be resized)
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1920,
                1080,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

            // Attach texture to FBO
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );

            // Check FBO completeness
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(anyhow!("FBO incomplete: status {}", status));
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);

            (fbo, texture)
        };

        // Create compositor
        let compositor = Compositor::new(gl.clone())?;

        // NOTE: projectM is initialized lazily on first render to ensure GL context is ready
        log::info!("Render pipeline created (projectM will be initialized on first render)");

        Ok(Self {
            gl,
            fbo,
            fbo_texture,
            fbo_width: 1920,
            fbo_height: 1080,
            vstage_capture: None,
            vstage_bloom: None,
            projectm: None,
            projectm_init_attempted: false,
            projectm_silence: ProjectMSilence::default(),
            silence_fade_clock: None,
            led_wall: None,  // Initialized lazily like projectM
            harmonic_geometry: None,  // Initialized lazily
            harmonic_geometry_init_attempted: false,
            mythos: None,  // Initialized lazily
            mythos_init_attempted: false,
            #[cfg(target_os = "macos")]
            metal_led_wall: None,  // Metal-based LED Wall (high performance)
            #[cfg(target_os = "macos")]
            metal_led_texture: None,
            #[cfg(target_os = "macos")]
            metal_led_iosurface: SendableIOSurface::null(),
            #[cfg(target_os = "macos")]
            metal_led_width: 0,
            #[cfg(target_os = "macos")]
            metal_led_height: 0,
            render_mode: RenderMode::ProjectM,  // Default to projectM
            video_texture: None,
            video_width: 0,
            video_height: 0,
            video_pbos: Vec::new(),  // Created on demand when video is loaded
            pbo_index: 0,
            pbo_size: 0,
            compositor,
            initialized: true,
            frame_count: 0,
            #[cfg(target_os = "macos")]
            texture_cache: None,  // Initialized lazily on first use
            #[cfg(target_os = "macos")]
            cv_video_texture: None,
            #[cfg(target_os = "macos")]
            cv_video_target: glow::TEXTURE_RECTANGLE,
            #[cfg(target_os = "macos")]
            cv_video_width: 0,
            #[cfg(target_os = "macos")]
            cv_video_height: 0,
            skin_video_texture: None,
            skin_video_fbo: None,
            skin_video_width: 0,
            skin_video_height: 0,
            logo_texture: None,  // Loaded lazily
            logo_width: 0,
            logo_height: 0,
            current_logo_path: None,
            track_texture: None,
            track_tex_width: 0,
            track_tex_height: 0,
            last_track_title: String::new(),
            last_track_artist: String::new(),
            font_data: None,
            preset_browser_texture: None,
            preset_browser_tex_width: 0,
            preset_browser_tex_height: 0,
            last_preset_browser_name: String::new(),
            last_preset_browser_index: 0,
            cached_harmonic_state: super::HarmonicFieldState::default(),
            phase_time: 0.0,
            camera_media: std::collections::HashMap::new(),
            camera_composite_order: Vec::new(),
            camera_compositor_config: crate::camera::CameraCompositorConfig::default(),
            camera_frame_counter: 0,
            active_camera_for_hg: None,
        })
    }

    /// Ensure PBOs are allocated for the given frame size
    fn ensure_pbos(&mut self, frame_size: usize) {
        if self.pbo_size == frame_size && self.video_pbos.len() == NUM_PBOS {
            return;  // Already correct size
        }

        // Delete old PBOs if they exist
        for pbo in self.video_pbos.drain(..) {
            unsafe {
                self.gl.delete_buffer(pbo);
            }
        }

        // Create new PBOs
        unsafe {
            for i in 0..NUM_PBOS {
                match self.gl.create_buffer() {
                    Ok(pbo) => {
                        self.gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(pbo));
                        // Allocate buffer with STREAM_DRAW hint (data updated frequently, used for drawing)
                        self.gl.buffer_data_size(
                            glow::PIXEL_UNPACK_BUFFER,
                            frame_size as i32,
                            glow::STREAM_DRAW,
                        );
                        self.video_pbos.push(pbo);
                        log::debug!("[Pipeline] Created PBO {} with size {} bytes", i, frame_size);
                    }
                    Err(e) => {
                        log::error!("[Pipeline] Failed to create PBO {}: {}", i, e);
                    }
                }
            }
            self.gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        }

        self.pbo_size = frame_size;
        self.pbo_index = 0;
        log::info!("[Pipeline] Allocated {} PBOs of {} bytes each for async texture upload",
            self.video_pbos.len(), frame_size);
    }

    /// Resize FBO if dimensions changed
    fn ensure_fbo_size(&mut self, width: u32, height: u32) {
        if width == self.fbo_width && height == self.fbo_height {
            return;
        }

        if width == 0 || height == 0 {
            return;
        }

        log::debug!("Resizing FBO from {}x{} to {}x{}", self.fbo_width, self.fbo_height, width, height);

        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.fbo_texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }

        self.fbo_width = width;
        self.fbo_height = height;

        // Notify projectM of size change
        if let Some(ref mut projectm) = self.projectm {
            projectm.set_size(width, height);
        }
    }

    /// Initialize projectM lazily (called from render thread with valid GL context)
    /// Made public to allow category/preset access before first render
    pub fn ensure_projectm_initialized(&mut self) {
        if self.projectm_init_attempted {
            return;
        }

        self.projectm_init_attempted = true;

        log::info!("[Pipeline] Initializing projectM (lazy init on first render)");

        match ProjectMRenderer::new(self.gl.clone()) {
            Ok(pm) => {
                log::info!("[Pipeline] projectM initialized successfully");
                self.projectm = Some(pm);
            }
            Err(e) => {
                log::warn!("[Pipeline] projectM initialization failed: {} - continuing without projectM", e);
            }
        }
    }

    /// Initialize LED wall lazily (called from render thread with valid GL context)
    fn ensure_led_wall_initialized(&mut self) {
        // On macOS, prefer Metal LED Wall for maximum GPU performance
        #[cfg(target_os = "macos")]
        {
            if self.metal_led_wall.is_some() {
                return;
            }

            log::info!("[Pipeline] Initializing Metal LED Wall renderer (GPU compute + instanced + IOSurface zero-copy)");
            match MetalLedWall::new(1920, 1080) {
                Ok(metal_wall) => {
                    // Create OpenGL texture and bind to IOSurface for zero-copy
                    unsafe {
                        let texture = self.gl.create_texture().unwrap();
                        let (w, h) = metal_wall.dimensions();
                        let iosurface = metal_wall.iosurface();

                        // Bind IOSurface to texture using GL_TEXTURE_RECTANGLE
                        self.gl.bind_texture(GL_TEXTURE_RECTANGLE, Some(texture));
                        self.gl.tex_parameter_i32(GL_TEXTURE_RECTANGLE as u32, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                        self.gl.tex_parameter_i32(GL_TEXTURE_RECTANGLE as u32, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);

                        // Use CGLTexImageIOSurface2D for zero-copy binding
                        let cgl_ctx = CGLGetCurrentContext();
                        let result = CGLTexImageIOSurface2D(
                            cgl_ctx,
                            GL_TEXTURE_RECTANGLE,
                            GL_RGBA,
                            w as i32,
                            h as i32,
                            GL_BGRA,
                            GL_UNSIGNED_INT_8_8_8_8_REV,
                            iosurface,
                            0,
                        );

                        if result != 0 {
                            log::warn!("[Pipeline] CGLTexImageIOSurface2D failed: {}", result);
                            self.gl.delete_texture(texture);
                            // Fall through to OpenGL fallback
                        } else {
                            self.metal_led_texture = Some(texture);
                            self.metal_led_iosurface.set(iosurface);
                            self.metal_led_width = w;
                            self.metal_led_height = h;
                            self.metal_led_wall = Some(metal_wall);
                            log::info!("[Pipeline] Metal LED Wall initialized with IOSurface zero-copy (1 draw call, no GPU readback!)");
                            return;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[Pipeline] Metal LED Wall failed, falling back to OpenGL: {}", e);
                }
            }
        }

        // Fallback to OpenGL LED Wall
        if self.led_wall.is_some() {
            return;
        }

        log::info!("[Pipeline] Initializing OpenGL LED wall renderer");
        self.led_wall = Some(LedWallRenderer::new(self.gl.clone()));
        log::info!("[Pipeline] LED wall renderer initialized");
    }

    /// Initialize Geometric Field renderer lazily on first render
    fn ensure_harmonic_geometry_initialized(&mut self) {
        if self.harmonic_geometry.is_some() {
            return;
        }

        // Log only first attempt to avoid spam
        let is_first_attempt = !self.harmonic_geometry_init_attempted;
        self.harmonic_geometry_init_attempted = true;

        log::info!("[Pipeline] ensure_harmonic_geometry_initialized: is_first={}, has_renderer={}",
                   is_first_attempt, self.harmonic_geometry.is_some());

        let mut renderer = HarmonicGeometryRenderer::new();
        // Enable Hero skin (act-driven compel visibility)
        renderer.enable_hero_skin();
        log::info!("[Pipeline] Calling renderer.init()...");
        match renderer.init(self.gl.clone()) {
            Ok(_) => {
                self.harmonic_geometry = Some(renderer);
                log::info!("[Pipeline] Geometric Field renderer initialized with Hero skin");
            }
            Err(e) => {
                // Reset flag to allow retry on next request
                self.harmonic_geometry_init_attempted = false;
                log::error!("[Pipeline] Failed to initialize Geometric Field: {} (will retry)", e);
            }
        }
    }

    fn ensure_mythos_initialized(&mut self) {
        if self.mythos.is_some() {
            return;
        }

        self.mythos_init_attempted = true;

        let mut renderer = MythosRenderer::new();
        match renderer.init(self.gl.clone()) {
            Ok(_) => {
                self.mythos = Some(renderer);
                log::info!("[Pipeline] Mythos renderer initialized");
            }
            Err(e) => {
                self.mythos_init_attempted = false;
                log::error!("[Pipeline] Failed to initialize Mythos: {} (will retry)", e);
            }
        }
    }

    /// Pre-initialize HarmonicGeometry renderer (public, call during loading phase)
    /// This compiles shaders while loading screen is visible, preventing freeze on first render
    pub fn pre_init_harmonic_geometry(&mut self) {
        log::info!("[Pipeline] pre_init_harmonic_geometry: Starting shader compilation...");
        self.ensure_harmonic_geometry_initialized();
        log::info!("[Pipeline] pre_init_harmonic_geometry: Complete");
    }

    /// Initialize HarmonicGeometry one step at a time (frame-distributed loading)
    /// Call this each frame during loading to spread compilation across frames
    /// Returns (is_complete, current_step_description, progress 0-1)
    pub fn init_harmonic_geometry_step(&mut self) -> (bool, &'static str, f32) {
        // Create renderer on first call if needed
        if self.harmonic_geometry.is_none() && !self.harmonic_geometry_init_attempted {
            self.harmonic_geometry_init_attempted = true;
            let mut renderer = HarmonicGeometryRenderer::new();
            renderer.enable_hero_skin();
            self.harmonic_geometry = Some(renderer);
            log::info!("[Pipeline] Created HarmonicGeometry renderer for step-based init");
        }

        // Execute next step
        if let Some(ref mut renderer) = self.harmonic_geometry {
            match renderer.init_next_step(&self.gl) {
                Ok((is_complete, step)) => {
                    (is_complete, step.description(), step.progress())
                }
                Err(e) => {
                    log::error!("[Pipeline] Shader init step failed: {}", e);
                    // Return as if complete to avoid infinite loop
                    (true, "Error", 1.0)
                }
            }
        } else {
            // No renderer - shouldn't happen but handle gracefully
            (true, "Ready", 1.0)
        }
    }

    /// Get current shader init step for progress display
    pub fn get_shader_init_step(&self) -> Option<ShaderInitStep> {
        self.harmonic_geometry.as_ref().map(|r| r.get_init_step())
    }

    /// Load logo texture lazily (called from render thread with valid GL context)
    /// If custom_path is provided and different from current, reloads the texture
    fn ensure_logo_loaded(&mut self, custom_path: Option<&str>) {
        // Check if we need to reload due to path change
        let path_changed = custom_path != self.current_logo_path.as_deref();

        if self.logo_texture.is_some() && !path_changed {
            return;
        }

        // If path changed, delete old texture
        if path_changed {
            if let Some(tex) = self.logo_texture.take() {
                unsafe { self.gl.delete_texture(tex); }
            }
            self.current_logo_path = custom_path.map(|s| s.to_string());
            log::info!("[Pipeline] Logo path changed to: {:?}", self.current_logo_path);
        }

        // Build search paths - custom path first if provided
        let mut search_paths: Vec<std::path::PathBuf> = Vec::new();

        // Custom path takes priority
        if let Some(custom) = custom_path {
            search_paths.push(std::path::PathBuf::from(custom));
        }

        // Default logo paths
        let default_paths = [
            "public/images/compeller_logo.png",
            "../public/images/compeller_logo.png",
            "../../public/images/compeller_logo.png",
        ];
        search_paths.extend(default_paths.iter().map(|p| std::path::PathBuf::from(p)));

        // Also check relative to executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                // Check in Resources for macOS .app bundle
                search_paths.push(exe_dir.join("../Resources/public/images/compeller_logo.png"));
                // Check alongside executable
                search_paths.push(exe_dir.join("public/images/compeller_logo.png"));
            }
        }

        for path in &search_paths {
            if path.exists() {
                log::info!("[Pipeline] Loading logo from: {:?}", path);
                match image::open(path) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        let (w, h) = (rgba.width(), rgba.height());
                        let pixels = rgba.into_raw();

                        unsafe {
                            if let Ok(texture) = self.gl.create_texture() {
                                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                                self.gl.tex_image_2d(
                                    glow::TEXTURE_2D,
                                    0,
                                    glow::RGBA8 as i32,
                                    w as i32,
                                    h as i32,
                                    0,
                                    glow::RGBA,
                                    glow::UNSIGNED_BYTE,
                                    Some(&pixels),
                                );
                                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                                self.gl.bind_texture(glow::TEXTURE_2D, None);

                                self.logo_texture = Some(texture);
                                self.logo_width = w;
                                self.logo_height = h;
                                log::info!("[Pipeline] Logo texture loaded: {}x{}", w, h);
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("[Pipeline] Failed to load logo {:?}: {}", path, e);
                    }
                }
            }
        }

        log::warn!("[Pipeline] Logo file not found in any search path");
    }

    /// Update track announcement texture if track info changed
    fn update_track_texture(&mut self, title: &str, artist: &str, opacity: f32, progress: f32) {
        // Skip if track info hasn't changed and texture exists
        if self.track_texture.is_some()
            && self.last_track_title == title
            && self.last_track_artist == artist {
            return;
        }

        // Skip if no track info
        if title.is_empty() && artist.is_empty() {
            return;
        }

        // Load font if needed (prefer bold fonts)
        if self.font_data.is_none() {
            // Try to find a bold system font
            let font_paths = [
                "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
                "/Library/Fonts/Arial Bold.ttf",
                "/System/Library/Fonts/Helvetica.ttc",  // Contains bold variant
                "/System/Library/Fonts/SFNSText-Bold.otf",
                "C:\\Windows\\Fonts\\arialbd.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            ];

            for path in &font_paths {
                if let Ok(data) = std::fs::read(path) {
                    log::info!("[Pipeline] Loaded font from {}", path);
                    self.font_data = Some(data);
                    break;
                }
            }

            if self.font_data.is_none() {
                log::warn!("[Pipeline] No system font found for track text");
                return;
            }
        }

        let font_data = self.font_data.as_ref().unwrap();

        // Parse font
        use ab_glyph::{FontRef, PxScale};
        let font = match FontRef::try_from_slice(font_data) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("[Pipeline] Failed to parse font: {}", e);
                return;
            }
        };

        // Render text to image
        let title_scale = PxScale::from(48.0);
        let artist_scale = PxScale::from(32.0);

        // Calculate text dimensions
        let padding = 24;  // Slightly more padding for the plate
        let line_spacing = 8;

        // Simple width estimation (proper measurement would use font metrics)
        let title_width = (title.len() as f32 * 28.0) as u32;
        let artist_width = (artist.len() as f32 * 18.0) as u32;
        let img_width = title_width.max(artist_width) + padding * 2;
        let img_height = 48 + 32 + line_spacing as u32 + padding * 2;

        // Create RGBA image - start with semi-transparent dark plate background
        let mut img = image::RgbaImage::new(img_width, img_height);

        // Draw dark semi-transparent background plate with rounded corners effect
        // Using gradient from center (darker) to edges (more transparent) for subtle blur-like effect
        let plate_color_center = image::Rgba([0u8, 0u8, 0u8, 180u8]);  // 70% opacity black
        let plate_color_edge = image::Rgba([0u8, 0u8, 0u8, 120u8]);    // 47% opacity black
        let corner_radius = 12.0_f32;

        for y in 0..img_height {
            for x in 0..img_width {
                // Calculate distance from nearest corner for rounded effect
                let dx = if x < corner_radius as u32 {
                    corner_radius - x as f32
                } else if x > img_width - corner_radius as u32 {
                    x as f32 - (img_width as f32 - corner_radius)
                } else {
                    0.0
                };
                let dy = if y < corner_radius as u32 {
                    corner_radius - y as f32
                } else if y > img_height - corner_radius as u32 {
                    y as f32 - (img_height as f32 - corner_radius)
                } else {
                    0.0
                };

                // Check if in corner region
                if dx > 0.0 && dy > 0.0 {
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist > corner_radius {
                        // Outside rounded corner - transparent
                        continue;
                    }
                    // Soft edge on corners
                    let edge_softness = (corner_radius - dist) / corner_radius;
                    let alpha = (plate_color_edge.0[3] as f32 * edge_softness) as u8;
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, alpha]));
                } else {
                    // Inside rectangle - use gradient based on distance from edge for softer look
                    let edge_dist = (x.min(img_width - x) as f32).min(y.min(img_height - y) as f32);
                    let blend = (edge_dist / 20.0).min(1.0);  // Blend over 20 pixels
                    let alpha = plate_color_edge.0[3] as f32 + (plate_color_center.0[3] as f32 - plate_color_edge.0[3] as f32) * blend;
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, alpha as u8]));
                }
            }
        }

        // Draw text using imageproc
        let white = image::Rgba([255u8, 255u8, 255u8, 255u8]);
        let gray = image::Rgba([200u8, 200u8, 200u8, 255u8]);

        // Draw title with faux-bold (draw multiple times with offsets)
        let title_x = padding as i32;
        let title_y = padding as i32;
        for dx in 0..=1 {
            for dy in 0..=1 {
                imageproc::drawing::draw_text_mut(
                    &mut img,
                    white,
                    title_x + dx,
                    title_y + dy,
                    title_scale,
                    &font,
                    title,
                );
            }
        }

        // Draw artist below title with faux-bold
        let artist_x = padding as i32;
        let artist_y = (padding + 48 + line_spacing as u32) as i32;
        for dx in 0..=1 {
            imageproc::drawing::draw_text_mut(
                &mut img,
                gray,
                artist_x + dx,
                artist_y,
                artist_scale,
                &font,
                artist,
            );
        }

        // Upload to GPU texture
        unsafe {
            // Delete old texture if exists
            if let Some(old_tex) = self.track_texture.take() {
                self.gl.delete_texture(old_tex);
            }

            if let Ok(texture) = self.gl.create_texture() {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    img_width as i32,
                    img_height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    Some(&img.into_raw()),
                );
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                self.gl.bind_texture(glow::TEXTURE_2D, None);

                self.track_texture = Some(texture);
                self.track_tex_width = img_width;
                self.track_tex_height = img_height;
                self.last_track_title = title.to_string();
                self.last_track_artist = artist.to_string();

                log::debug!("[Pipeline] Updated track texture: {}x{}", img_width, img_height);
            }
        }
    }

    /// Update preset browser overlay texture
    fn update_preset_browser_texture(&mut self, display_name: &str, file_name: &str, category: &str, current_idx: usize, total: usize) {
        // Skip if info hasn't changed and texture exists
        if self.preset_browser_texture.is_some()
            && self.last_preset_browser_name == display_name
            && self.last_preset_browser_index == current_idx {
            return;
        }

        // Skip if no preset name
        if display_name.is_empty() {
            return;
        }

        // Only show file name if different from display name
        let show_file_name = !file_name.is_empty() && file_name != display_name;

        // Ensure font is loaded
        if self.font_data.is_none() {
            let font_paths = [
                "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
                "/Library/Fonts/Arial Bold.ttf",
                "/System/Library/Fonts/Helvetica.ttc",
                "/System/Library/Fonts/SFNSText-Bold.otf",
                "C:\\Windows\\Fonts\\arialbd.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            ];

            for path in &font_paths {
                if let Ok(data) = std::fs::read(path) {
                    self.font_data = Some(data);
                    break;
                }
            }

            if self.font_data.is_none() {
                return;
            }
        }

        let font_data = self.font_data.as_ref().unwrap();

        use ab_glyph::{FontRef, PxScale};
        let font = match FontRef::try_from_slice(font_data) {
            Ok(f) => f,
            Err(_) => return,
        };

        // Layout: display name (large), file name (smaller if different), category, index, and "push to select" instruction
        let display_scale = PxScale::from(56.0);  // Large for center screen
        let file_scale = PxScale::from(22.0);     // Smaller for file name
        let index_scale = PxScale::from(24.0);    // Smaller for index
        let cat_scale = PxScale::from(18.0);      // Category name
        let hint_scale = PxScale::from(18.0);     // Push to select hint

        let padding = 32;
        let line_spacing = 10;

        // Truncate long display names (max ~30 chars)
        let display_text = if display_name.chars().count() > 35 {
            let truncated: String = display_name.chars().take(32).collect();
            format!("{}...", truncated)
        } else {
            display_name.to_string()
        };

        // Truncate file name
        let file_text = if file_name.chars().count() > 45 {
            let truncated: String = file_name.chars().take(42).collect();
            format!("{}...", truncated)
        } else {
            file_name.to_string()
        };

        // Calculate actual text widths using font metrics
        use ab_glyph::Font;
        let calc_width = |text: &str, scale: PxScale| -> u32 {
            let mut width = 0.0f32;
            for c in text.chars() {
                let glyph_id = font.glyph_id(c);
                width += font.h_advance_unscaled(glyph_id) * scale.x / font.height_unscaled();
            }
            width.ceil() as u32
        };

        let index_text = format!("< {} of {} >", current_idx, total);
        let hint_text = "PUSH TO SELECT";

        let display_width = calc_width(&display_text, display_scale);
        let file_width = if show_file_name { calc_width(&file_text, file_scale) } else { 0 };
        let index_width = calc_width(&index_text, index_scale);
        let cat_width = calc_width(category, cat_scale);
        let hint_width = calc_width(hint_text, hint_scale);

        let file_line_height = if show_file_name { 22 + line_spacing as u32 } else { 0 };
        let img_width = display_width.max(file_width).max(index_width).max(cat_width).max(hint_width).max(400) + padding * 2;
        let img_height = 56 + file_line_height + 18 + 24 + 18 + line_spacing as u32 * 3 + padding * 2;

        // Create RGBA image with dark semi-transparent background
        let mut img = image::RgbaImage::new(img_width, img_height);

        let plate_color_center = image::Rgba([0u8, 0u8, 0u8, 200u8]);
        let plate_color_edge = image::Rgba([0u8, 0u8, 0u8, 140u8]);
        let corner_radius = 16.0_f32;

        for y in 0..img_height {
            for x in 0..img_width {
                let dx = if x < corner_radius as u32 {
                    corner_radius - x as f32
                } else if x > img_width - corner_radius as u32 {
                    x as f32 - (img_width as f32 - corner_radius)
                } else {
                    0.0
                };
                let dy = if y < corner_radius as u32 {
                    corner_radius - y as f32
                } else if y > img_height - corner_radius as u32 {
                    y as f32 - (img_height as f32 - corner_radius)
                } else {
                    0.0
                };

                if dx > 0.0 && dy > 0.0 {
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist > corner_radius {
                        continue;
                    }
                    let edge_softness = (corner_radius - dist) / corner_radius;
                    let alpha = (plate_color_edge.0[3] as f32 * edge_softness) as u8;
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, alpha]));
                } else {
                    let edge_dist = (x.min(img_width - x) as f32).min(y.min(img_height - y) as f32);
                    let blend = (edge_dist / 20.0).min(1.0);
                    let alpha = plate_color_edge.0[3] as f32 + (plate_color_center.0[3] as f32 - plate_color_edge.0[3] as f32) * blend;
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, alpha as u8]));
                }
            }
        }

        // Draw text (centered horizontally)
        let white = image::Rgba([255u8, 255u8, 255u8, 255u8]);
        let gold = image::Rgba([255u8, 215u8, 100u8, 255u8]);  // Golden color for index
        let gray = image::Rgba([140u8, 140u8, 140u8, 255u8]);  // Gray for file name
        let dim_gray = image::Rgba([100u8, 100u8, 100u8, 255u8]); // Dimmer gray for category
        let green = image::Rgba([100u8, 255u8, 150u8, 255u8]); // Green for push hint

        let mut y_offset = padding as i32;

        // Index indicator at top (★ 3 of 12 ★)
        let index_x = ((img_width - index_width) / 2) as i32;
        imageproc::drawing::draw_text_mut(
            &mut img,
            gold,
            index_x,
            y_offset,
            index_scale,
            &font,
            &index_text,
        );
        y_offset += 24 + line_spacing as i32;

        // Display name (large, bold) - custom name or file name
        let display_x = ((img_width - display_width) / 2) as i32;
        for dx in 0..=1 {
            for dy in 0..=1 {
                imageproc::drawing::draw_text_mut(
                    &mut img,
                    white,
                    display_x + dx,
                    y_offset + dy,
                    display_scale,
                    &font,
                    &display_text,
                );
            }
        }
        y_offset += 56 + line_spacing as i32;

        // File name (smaller, gray) - only if different from display name
        if show_file_name {
            let file_x = ((img_width - file_width) / 2) as i32;
            imageproc::drawing::draw_text_mut(
                &mut img,
                gray,
                file_x,
                y_offset,
                file_scale,
                &font,
                &file_text,
            );
            y_offset += 22 + line_spacing as i32;
        }

        // Category name
        let cat_x = ((img_width - cat_width) / 2) as i32;
        imageproc::drawing::draw_text_mut(
            &mut img,
            dim_gray,
            cat_x,
            y_offset,
            cat_scale,
            &font,
            category,
        );
        y_offset += 18 + line_spacing as i32;

        // "PUSH TO SELECT" hint at bottom
        let hint_x = ((img_width - hint_width) / 2) as i32;
        imageproc::drawing::draw_text_mut(
            &mut img,
            green,
            hint_x,
            y_offset,
            hint_scale,
            &font,
            hint_text,
        );

        // Upload to GPU texture
        unsafe {
            if let Some(old_tex) = self.preset_browser_texture.take() {
                self.gl.delete_texture(old_tex);
            }

            if let Ok(texture) = self.gl.create_texture() {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    img_width as i32,
                    img_height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    Some(&img.into_raw()),
                );
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                self.gl.bind_texture(glow::TEXTURE_2D, None);

                self.preset_browser_texture = Some(texture);
                self.preset_browser_tex_width = img_width;
                self.preset_browser_tex_height = img_height;
                self.last_preset_browser_name = display_name.to_string();
                self.last_preset_browser_index = current_idx;

                log::debug!("[Pipeline] Updated preset browser texture: {}x{}", img_width, img_height);
            }
        }
    }

    /// Set the render mode
    pub fn set_render_mode(&mut self, mode: RenderMode) {
        if self.render_mode != mode {
            log::info!("[Pipeline] Switching render mode to {:?}", mode);
            self.render_mode = mode;
        }
    }

    /// Get current render mode
    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    /// Update LED wall with audio analysis data (all GPU - no CPU copy)
    /// bass, mid, high: 0.0-1.0 frequency band energy
    /// kick: true if kick drum detected this frame
    /// dt: delta time in seconds
    pub fn update_led_wall(&mut self, bass: f32, mid: f32, high: f32, kick: bool, dt: f32) {
        // Prefer Metal LED Wall on macOS
        #[cfg(target_os = "macos")]
        if let Some(ref mut metal_wall) = self.metal_led_wall {
            metal_wall.update_state(bass, mid, high, kick, dt);
            return;
        }

        // Fallback to OpenGL
        if let Some(ref mut led_wall) = self.led_wall {
            led_wall.update(bass, mid, high, kick, dt);
        }
    }

    /// Update the active theme renderer with audio analysis data
    /// Uses the ThemeRenderer trait for unified audio routing
    pub fn update_theme(&mut self, audio: &AudioAnalysis, dt: f32) {
        // ALWAYS update HarmonicGeometry for lighting metrics (OSC/Hue/WLED)
        // regardless of render mode. This ensures raw_energy, key detection,
        // and other lighting signals are computed even when displaying ProjectM.
        if let Some(ref mut hg) = self.harmonic_geometry {
            ThemeRenderer::update(hg, audio, dt);
        }

        // Update the active renderer based on mode
        match self.render_mode {
            RenderMode::ProjectM => {
                if let Some(ref mut projectm) = self.projectm {
                    // Use ThemeRenderer trait method
                    ThemeRenderer::update(projectm, audio, dt);
                }
            }
            RenderMode::LedWall => {
                // Metal LED Wall on macOS doesn't implement ThemeRenderer
                // (it's a specialized high-performance path)
                #[cfg(target_os = "macos")]
                if let Some(ref mut metal_wall) = self.metal_led_wall {
                    metal_wall.update_state(audio.bass, audio.mid, audio.high, audio.kick, dt);
                }
                #[cfg(not(target_os = "macos"))]
                {
                    // OpenGL LED Wall uses ThemeRenderer
                    if let Some(ref mut led_wall) = self.led_wall {
                        ThemeRenderer::update(led_wall, audio, dt);
                    }
                }
            }
            RenderMode::HarmonicGeometry => {
                // Already updated above
            }
            RenderMode::Mythos => {
                if let Some(ref mut mythos) = self.mythos {
                    ThemeRenderer::update(mythos, audio, dt);
                }
            }
        }

        // Always update cached harmonic state for UI display (works in any render mode)
        // Find dominant pitch from chroma
        let mut dominant_idx = 0;
        let mut max_val = 0.0f32;
        for (i, &v) in audio.chroma.iter().enumerate() {
            if v > max_val {
                max_val = v;
                dominant_idx = i;
            }
        }

        const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        let dominant_name = if audio.is_tonal {
            NOTE_NAMES[dominant_idx]
        } else {
            "—"
        };

        let mode_name = match self.render_mode {
            RenderMode::ProjectM => "projectM",
            RenderMode::LedWall => "LED Wall",
            RenderMode::HarmonicGeometry => "Geometric Field",
            RenderMode::Mythos => "Mythos Field",
        };

        // Update phase time (slow rotation for visual continuity)
        self.phase_time += dt * 0.1; // ~0.1 rad/sec
        let phase = (self.phase_time / std::f32::consts::TAU).fract();

        // Determine harmonic label based on confidence thresholds
        let harmonic_label = if !audio.is_tonal || audio.tonal_energy < 0.1 {
            super::HarmonicLabel::Atonal
        } else if audio.tonal_separation >= 0.25 {
            super::HarmonicLabel::Stable
        } else if audio.tonal_separation >= 0.12 {
            super::HarmonicLabel::Rotating
        } else {
            super::HarmonicLabel::Ambiguous
        };

        self.cached_harmonic_state = super::HarmonicFieldState {
            chroma: audio.chroma,
            raw_chroma: audio.chroma, // In pipeline, use raw audio chroma
            dominant_pitch: dominant_idx,
            dominant_name,
            raw_dominant_pitch: dominant_idx, // In pipeline, raw == held (no latching here)
            is_tonal: audio.is_tonal,
            tonal_confidence: audio.tonal_separation, // tonal_separation is confidence
            tonal_energy: audio.tonal_energy,
            energy_vis: audio.energy_vis,
            stability: audio.stability,
            balance: 0.5, // Default balanced for non-HG modes
            gate_ok: 0.0, // Not tracked for non-HG modes
            phase,
            preset_name: mode_name,
            render_mode: "—", // Not applicable for non-HG modes
            symmetry: 12,
            anchor_k: 0,
            anchor_theta_deg: 0.0,
            harmonic_label,
            journey_phase: super::HeroAct::Birth, // Default for non-HG modes
            journey_phase_time: 0.0,
            interior_state: super::InteriorState::Outside,
            // RingDive/Fold24 not applicable for non-HG modes
            ring_dive_active: false,
            ring_dive_t: 0.0,
            ring_dive_subrange: "none",
            fold24_active: false,
            fold24_t: 0.0,
            fold24_engaged: false,
            fold24_request_latch_bars: 0,
            tension_slow: 0.0, // Not tracked for non-HG modes
            // QuaternionCharges not applicable for non-HG modes
            qc_active: false,
            qc_w_deg: 0.0,
            qc_ports_deg: [0.0; 4],
            qc_active_port: 0,
            qc_strength: 0.0,
            qc_ok: [false; 4],
            // TorusKnot not applicable for non-HG modes
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

            // Iconic tuning fallback
            torus_spice_active: false,
            torus_spice_level: 0,
            torus_spice_level_target: 0,
            torus_spice_eligible: false,
            torus_spice_prime_limit: 3,
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

            // Polarity not applicable for non-HG modes
            polarity_quadrant: 0,
            polarity_prev_quadrant: 0,
            reg_degree_a: 0,

            // Topology router not applicable for non-HG modes
            topology_name: "Scaffold24",
            topology_xfade_t: 0.0,
            topology_bar_index: 0,
            router_energy_bar: 0.0,
            router_mid_dominant: false,
            router_tension_slope: 0.0,
            router_hold_s: 0.0,

            // Vortex not applicable for non-HG modes
            vortex_active: false,
            vortex_t: 0.0,
            vortex_gate: 0,
            vortex_direction: "Inward",

            // Dodecahedral not applicable for non-HG modes
            dodeca_active: false,
            dodeca_t: 0.0,
            dodeca_cooldown: 0.0,
            dodeca_phi: 0.0,
            dodeca_entropy: 1.0,
            dodeca_entry_ok: false,
        };
    }

    /// Get the current renderer type
    pub fn current_renderer_type(&self) -> RendererType {
        match self.render_mode {
            RenderMode::ProjectM => RendererType::ProjectM,
            RenderMode::LedWall => RendererType::LedWall,
            RenderMode::HarmonicGeometry => RendererType::HarmonicGeometry,
            RenderMode::Mythos => RendererType::Mythos,
        }
    }

    /// Check if the current renderer has presets available
    pub fn current_renderer_has_presets(&self) -> bool {
        match self.render_mode {
            RenderMode::ProjectM => {
                self.projectm.as_ref().map(|p| p.has_presets()).unwrap_or(false)
            }
            RenderMode::LedWall => {
                // LED Wall has pattern presets
                true
            }
            RenderMode::HarmonicGeometry => {
                // Geometric Field has presets
                true
            }
            RenderMode::Mythos => {
                true
            }
        }
    }

    /// Get harmonic field state for UI display
    /// Returns HarmonicGeometry renderer's state when in HG mode, otherwise returns cached state
    /// This allows the harmonic field UI to work in any render mode
    pub fn get_harmonic_field_state(&self) -> Option<super::HarmonicFieldState> {
        // Prefer HarmonicGeometry renderer's state when in HG mode (has more details)
        if self.render_mode == RenderMode::HarmonicGeometry {
            if let Some(ref hg) = self.harmonic_geometry {
                return Some(hg.field_state());
            }
        }
        // Otherwise return cached state (populated from audio analysis in any mode)
        Some(self.cached_harmonic_state.clone())
    }

    /// Get reference to the KeyDetector for OSC output
    /// Returns None if HarmonicGeometry renderer is not initialized
    pub fn get_key_detector(&self) -> Option<&super::harmonic_geometry::KeyDetector> {
        self.harmonic_geometry.as_ref().map(|hg| hg.key_detector())
    }

    /// Get lighting metrics for OSC output (balance/tension/stability/energy)
    /// Returns None if HarmonicGeometry renderer is not initialized
    pub fn get_lighting_metrics(&self) -> Option<super::LightingMetrics> {
        let mut metrics = self.harmonic_geometry.as_ref().map(|hg| hg.lighting_metrics());
        // Overlay Collatz state from Mythos (available to all lighting outputs)
        if let Some(ref mythos) = self.mythos {
            if let Some(ref mut m) = metrics {
                let cz = mythos.collatz_output();
                m.collatz_heat = cz.heat;
                m.collatz_phase = cz.phase;
                m.collatz_intensity = cz.intensity;
                m.collatz_breath = cz.breath;
                m.collatz_act = cz.act;
            }
        }
        metrics
    }

    /// Set the tuning reference (432Hz or 440Hz) on the key detector
    pub fn set_tuning_reference(&mut self, tuning: super::harmonic_geometry::TuningReference) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.key_detector_mut().set_tuning(tuning);
        }
    }

    /// Set harmonic field state externally (used for frozen playback)
    /// This overrides the cached state so frozen frames display correctly
    /// Also updates HarmonicGeometry renderer if in HG mode
    pub fn set_frozen_harmonic_state(&mut self, state: super::HarmonicFieldState) {
        self.cached_harmonic_state = state.clone();

        // Also update HarmonicGeometry renderer if in HG mode
        if self.render_mode == RenderMode::HarmonicGeometry {
            if let Some(ref mut hg) = self.harmonic_geometry {
                hg.set_frozen_state(&state);
            }
        }
    }

    // ========== HERO SKIN SYSTEM ==========

    /// Enable Hero skin (act-driven compel visibility)
    pub fn enable_hero_skin(&mut self) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.enable_hero_skin();
        }
    }

    /// Disable skin (pure harmonic field)
    pub fn disable_hero_skin(&mut self) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.disable_skin();
        }
    }

    /// Get mutable reference to harmonic skin for UI editing
    pub fn get_harmonic_skin_mut(&mut self) -> Option<&mut super::harmonic_geometry::FieldSkin> {
        self.harmonic_geometry.as_mut().map(|hg| hg.skin_mut())
    }

    /// Toggle symmetry HUD visibility (shows 12/24-fold chroma visualization)
    pub fn toggle_symmetry_hud(&mut self) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.toggle_hud();
        }
    }

    /// Check if symmetry HUD is visible
    pub fn is_symmetry_hud_visible(&self) -> bool {
        self.harmonic_geometry.as_ref().map(|hg| hg.is_hud_visible()).unwrap_or(false)
    }

    /// Set symmetry HUD visibility (Chroma Wheel)
    pub fn set_symmetry_hud_visible(&mut self, visible: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_hud_visible(visible);
        }
    }

    /// Set QS Wheel (Quaternion Symmetry Wheel) visibility
    pub fn set_qs_wheel_visible(&mut self, visible: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_qs_wheel_visible(visible);
        }
    }

    /// Set whether to hide QS Wheel numbers
    pub fn set_hide_qs_numbers(&mut self, hide: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_hide_qs_numbers(hide);
        }
    }

    /// Set QS Wheel dominant-only mode
    pub fn set_qs_dominant_only(&mut self, dominant_only: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_qs_dominant_only(dominant_only);
        }
    }

    /// Check if QS Wheel is visible
    pub fn is_qs_wheel_visible(&self) -> bool {
        self.harmonic_geometry.as_ref().map(|hg| hg.is_qs_wheel_visible()).unwrap_or(false)
    }

    /// Apply license entitlements to rendering parameters
    pub fn apply_entitlements(&mut self, entitlements: &crate::auth::Entitlements) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.apply_entitlements(entitlements);
        }
    }

    /// Set quality tier (0=Low, 1=Med, 2=High)
    pub fn set_quality_tier(&mut self, tier: u8) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_quality_tier(tier);
        }
        if let Some(ref mut mythos) = self.mythos {
            mythos.set_quality_tier(tier);
        }
    }

    /// Get current quality tier
    pub fn get_quality_tier(&self) -> u8 {
        self.harmonic_geometry.as_ref().map(|hg| hg.get_quality_tier()).unwrap_or(1)
    }

    /// Toggle perf HUD visibility
    pub fn toggle_perf_hud(&mut self) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.toggle_perf_hud();
        }
    }

    /// Get debug HUD strings for display
    pub fn get_debug_hud_strings(&self) -> Vec<String> {
        self.harmonic_geometry.as_ref().map(|hg| hg.get_debug_hud_strings()).unwrap_or_default()
    }

    // ========== AAA RING SETTINGS ==========

    /// Set AAA ring style (true = 3D torus, false = classic flat)
    pub fn set_ring_style_aaa(&mut self, enabled: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_ring_style_aaa(enabled);
        }
    }

    /// Set ring flow lines enabled
    pub fn set_ring_flow_lines(&mut self, enabled: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_ring_flow_lines(enabled);
        }
    }

    /// Set ring sparkle enabled
    pub fn set_ring_sparkle(&mut self, enabled: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_ring_sparkle(enabled);
        }
    }

    /// Set ring fog enabled
    pub fn set_ring_fog(&mut self, enabled: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_ring_fog(enabled);
        }
    }

    /// Set ring bloom mask enabled
    pub fn set_ring_bloom_mask(&mut self, enabled: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_ring_bloom_mask(enabled);
        }
    }

    // ========== BG QUALITY BUDGET SYSTEM ==========

    /// Set BG quality tier (0=Low, 1=Med, 2=High)
    pub fn set_bg_quality(&mut self, quality: u8) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_bg_quality(quality);
        }
    }

    /// Get current BG quality tier
    pub fn get_bg_quality(&self) -> u8 {
        self.harmonic_geometry.as_ref().map(|hg| hg.get_bg_quality()).unwrap_or(1)
    }

    /// Set BG quality auto-scaling enabled
    pub fn set_bg_quality_auto(&mut self, enabled: bool) {
        if let Some(ref mut hg) = self.harmonic_geometry {
            hg.set_bg_quality_auto(enabled);
        }
    }

    /// Get BG quality auto-scaling state
    pub fn get_bg_quality_auto(&self) -> bool {
        self.harmonic_geometry.as_ref().map(|hg| hg.get_bg_quality_auto()).unwrap_or(true)
    }

    /// Get performance stats (frame_ms, sdf_ms, avg_steps, hit_rate, render_scale)
    pub fn get_perf_stats(&self) -> (f32, f32, f32, f32, f32) {
        self.harmonic_geometry.as_ref().map(|hg| hg.perf_stats()).unwrap_or((16.0, 8.0, 24.0, 0.5, 1.0))
    }

    /// Check if harmonic geometry needs video texture upload (PERF OPTIMIZATION)
    /// Returns false when video/skin isn't contributing, allowing callers to skip uploads
    pub fn harmonic_needs_video(&self) -> bool {
        // Mythos always needs video for field-reactive skin
        if self.render_mode == RenderMode::Mythos {
            return self.mythos.as_ref().map(|m| m.skin_texture_valid).unwrap_or(false)
                || self.video_texture.is_some();
        }
        // HarmonicGeometry checks its own skin state
        if self.render_mode != RenderMode::HarmonicGeometry {
            return false;
        }
        self.harmonic_geometry.as_ref().map(|hg| hg.needs_video_upload()).unwrap_or(false)
    }

    /// Render a frame - renders projectM to FBO then composites to screen
    /// Capture the live rendered screen into the virtual stage back-wall
    /// texture. The capture resource is created lazily on first call. Viewport
    /// rect is in physical pixels (GL bottom-left origin).
    pub fn capture_vstage_screen(&mut self, vp_x: i32, vp_y: i32, vp_w: i32, vp_h: i32) {
        if self.vstage_capture.is_none() {
            match super::VstageCapture::new(self.gl.clone()) {
                Ok(c) => self.vstage_capture = Some(c),
                Err(e) => {
                    log::warn!("[VStage] back-wall capture init failed: {}", e);
                    return;
                }
            }
        }
        if let Some(c) = self.vstage_capture.as_mut() {
            c.capture(vp_x, vp_y, vp_w, vp_h);
        }
    }

    /// Draw the captured live frame as the virtual stage back-wall quad.
    /// `corners` are clip-space [-1, 1] in top-left, top-right, bottom-right,
    /// bottom-left order.
    pub fn draw_vstage_backwall(&self, corners: [[f32; 2]; 4], viewport: [i32; 4]) {
        if let Some(c) = self.vstage_capture.as_ref() {
            c.draw(corners, viewport);
        }
    }

    /// Whether a live frame has been captured and is ready to show.
    pub fn vstage_backwall_ready(&self) -> bool {
        self.vstage_capture.as_ref().map_or(false, |c| c.has_frame())
    }

    /// Run the additive bloom post pass over the drawn virtual stage. The
    /// bloom resources are created lazily on first call. `viewport` is
    /// `[x, y, w, h]` in physical pixels, GL bottom-left origin.
    pub fn draw_vstage_bloom(&mut self, viewport: [i32; 4]) {
        if self.vstage_bloom.is_none() {
            match super::BloomPass::new(self.gl.clone()) {
                Ok(b) => self.vstage_bloom = Some(b),
                Err(e) => {
                    log::warn!("[VStage] bloom init failed: {}", e);
                    return;
                }
            }
        }
        if let Some(b) = self.vstage_bloom.as_ref() {
            b.apply(viewport);
        }
    }

    pub fn render_frame(&mut self, width: u32, height: u32, params: &RenderParams) -> Result<()> {
        if !self.initialized {
            return Err(anyhow!("Pipeline not initialized"));
        }

        if width == 0 || height == 0 {
            return Ok(());
        }

        self.frame_count += 1;

        // Advance the projectM silence fade. Real dt from a wall clock so the
        // ramp stays correct even if render_frame runs more than once per frame;
        // clamped so a stall (e.g. after a pause) can't snap it to black.
        let now = std::time::Instant::now();
        let silence_dt = self
            .silence_fade_clock
            .replace(now)
            .map(|prev| (now - prev).as_secs_f32())
            .unwrap_or(1.0 / 60.0)
            .min(0.1);
        self.projectm_silence.update(params.theme_silent, silence_dt);

        // Ensure FBO is correct size
        self.ensure_fbo_size(width, height);

        // Update skin texture from CV video (macOS) - do this BEFORE main render
        // Needed for HarmonicGeometry and Mythos (both use video as skin texture)
        #[cfg(target_os = "macos")]
        if (self.render_mode == RenderMode::HarmonicGeometry || self.render_mode == RenderMode::Mythos)
            && self.cv_video_texture.is_some()
        {
            self.update_skin_texture_from_cv();
        }

        // Notify projectM of size change (important after window resize or landing transition)
        if let Some(ref mut projectm) = self.projectm {
            projectm.set_size(width, height);
        }

        unsafe {
            // Render based on current mode
            match self.render_mode {
                RenderMode::ProjectM => {
                    // Lazily initialize projectM on first render (ensures GL context is valid)
                    self.ensure_projectm_initialized();

                    // Update transition state (may load pending preset after transition completes)
                    if let Some(ref mut projectm) = self.projectm {
                        projectm.update_transition();
                    }

                    // Render projectM
                    if params.projectm_visibility > 0.0 {
                        if let Some(ref mut projectm) = self.projectm {
                            // Set OpenGL state for projectM
                            self.gl.disable(glow::DEPTH_TEST);
                            self.gl.disable(glow::CULL_FACE);
                            self.gl.enable(glow::BLEND);
                            self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

                            // Render projectM to the currently bound framebuffer
                            projectm.render_frame();

                            #[cfg(debug_assertions)]
                            if self.frame_count % 60 == 0 {
                                log::debug!("[Pipeline] Rendered projectM (frame {})", self.frame_count);
                            }
                        }
                    } else {
                        // Clear when projectM is off
                        self.gl.clear_color(0.02, 0.02, 0.05, 1.0);
                        self.gl.clear(glow::COLOR_BUFFER_BIT);
                    }
                    // Camera backgrounds are composited AFTER video overlay (below)
                }

                RenderMode::LedWall => {
                    // Lazily initialize LED wall on first render
                    self.ensure_led_wall_initialized();

                    // Clear background
                    self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                    self.gl.clear(glow::COLOR_BUFFER_BIT);
                    // Camera backgrounds are composited AFTER video overlay (below)

                    // Render LED wall grid if visibility > 0 (same check as projectM)
                    if params.projectm_visibility > 0.0 {
                        // Prefer Metal LED Wall on macOS (1 draw call vs 576!)
                        #[cfg(target_os = "macos")]
                        if let Some(ref mut metal_wall) = self.metal_led_wall {
                            // Resize Metal LED Wall to match viewport if needed
                            metal_wall.resize(width, height);

                            // Render using Metal compute + instanced draw
                            metal_wall.render();

                            // Get dimensions
                            let (w, h) = metal_wall.dimensions();

                            // Check if IOSurface changed (resize) and rebind if needed
                            let current_iosurface = metal_wall.iosurface();
                            if current_iosurface != self.metal_led_iosurface.get()
                                || w != self.metal_led_width
                                || h != self.metal_led_height
                            {
                                if let Some(texture) = self.metal_led_texture {
                                    // Rebind IOSurface to texture
                                    self.gl.bind_texture(GL_TEXTURE_RECTANGLE, Some(texture));
                                    let cgl_ctx = CGLGetCurrentContext();
                                    let result = CGLTexImageIOSurface2D(
                                        cgl_ctx,
                                        GL_TEXTURE_RECTANGLE,
                                        GL_RGBA,
                                        w as i32,
                                        h as i32,
                                        GL_BGRA,
                                        GL_UNSIGNED_INT_8_8_8_8_REV,
                                        current_iosurface,
                                        0,
                                    );
                                    if result == 0 {
                                        self.metal_led_iosurface.set(current_iosurface);
                                        self.metal_led_width = w;
                                        self.metal_led_height = h;
                                        log::info!("[Pipeline] Rebound IOSurface for Metal LED Wall ({}x{})", w, h);
                                    }
                                }
                            }

                            // Draw using the IOSurface-bound texture (zero-copy - no GPU readback!)
                            if let Some(texture) = self.metal_led_texture {
                                // Draw fullscreen quad with GL_TEXTURE_RECTANGLE
                                // Texture is now same size as viewport, so use w,h for both
                                self.compositor.draw_texture_rect(
                                    texture,
                                    GL_TEXTURE_RECTANGLE,
                                    width,
                                    height,
                                    w,      // Texture width (now matches viewport)
                                    h,      // Texture height (now matches viewport)
                                    1.0,    // full visibility (alpha)
                                    1.0,    // scale
                                    false,  // no Y flip needed for Metal LED Wall
                                    0.0,    // no luma key for LED wall
                                );
                            }

                            #[cfg(debug_assertions)]
                            if self.frame_count % 60 == 0 {
                                log::debug!("[Pipeline] Rendered Metal LED Wall (frame {}, IOSurface zero-copy!)", self.frame_count);
                            }
                        } else if let Some(ref mut led_wall) = self.led_wall {
                            // Fallback to OpenGL LED Wall (instanced rendering)
                            led_wall.render(width, height);
                        }

                        // Non-macOS: always use OpenGL LED Wall
                        #[cfg(not(target_os = "macos"))]
                        if let Some(ref mut led_wall) = self.led_wall {
                            led_wall.render(width, height);

                            #[cfg(debug_assertions)]
                            if self.frame_count % 60 == 0 {
                                log::debug!("[Pipeline] Rendered LED Wall (frame {})", self.frame_count);
                            }
                        }
                    }
                }

                RenderMode::HarmonicGeometry => {
                    // Lazily initialize Geometric Field on first render
                    self.ensure_harmonic_geometry_initialized();

                    // === SKIN TEXTURE CAPTURE ===
                    // On macOS: Draw video first, copy to skin texture, then render HG
                    #[cfg(target_os = "macos")]
                    if self.cv_video_texture.is_some() && params.video_visibility > 0.0 {
                        // Clear to black for clean video capture
                        self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                        self.gl.clear(glow::COLOR_BUFFER_BIT);

                        // Draw video to framebuffer
                        let cv_tex_id = self.cv_video_texture.unwrap();
                        let texture = glow::NativeTexture(std::num::NonZeroU32::new(cv_tex_id).unwrap());
                        self.gl.disable(glow::BLEND);
                        self.compositor.draw_texture_rect(
                            texture,
                            self.cv_video_target,
                            width,
                            height,
                            self.cv_video_width,
                            self.cv_video_height,
                            1.0,   // Full opacity for capture
                            1.0,   // No scaling
                            true,  // Flip Y
                            0.0,   // No luma key for skin capture
                        );

                        // Copy framebuffer to skin texture
                        self.copy_framebuffer_to_skin_texture(width, height);
                    }

                    // Clear to dark background for HG render
                    self.gl.clear_color(0.02, 0.02, 0.05, 1.0);
                    self.gl.clear(glow::COLOR_BUFFER_BIT);

                    // Select active camera for HG BEFORE rendering
                    self.select_active_camera_for_hg();

                    // Render HG with skin texture
                    if params.projectm_visibility > 0.0 {
                        let skin_tex = self.get_skin_texture();
                        let video_aspect = self.get_video_aspect();

                        if let Some(ref mut hg) = self.harmonic_geometry {
                            hg.set_skin_texture(skin_tex, video_aspect);

                            // Pass camera texture to HG for blending in QS wheel
                            if let Some((_, cam_tex, cam_w, cam_h, cam_alpha)) = self.active_camera_for_hg {
                                hg.set_camera_texture(Some(cam_tex), cam_w, cam_h, cam_alpha);
                            } else {
                                hg.set_camera_texture(None, 0, 0, 0.0);
                            }

                            hg.render(width, height);
                        }
                    }
                    // Video overlay is drawn below (same as projectM/LedWall)
                }

                RenderMode::Mythos => {
                    // Lazily initialize Mythos on first render
                    self.ensure_mythos_initialized();

                    // === SKIN TEXTURE CAPTURE (same as HG) ===
                    // On macOS: Draw video first, copy to skin texture, then render Mythos
                    #[cfg(target_os = "macos")]
                    if self.cv_video_texture.is_some() {
                        // Clear to black for clean video capture
                        self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                        self.gl.clear(glow::COLOR_BUFFER_BIT);

                        // Draw video to framebuffer
                        let cv_tex_id = self.cv_video_texture.unwrap();
                        let texture = glow::NativeTexture(std::num::NonZeroU32::new(cv_tex_id).unwrap());
                        self.gl.disable(glow::BLEND);
                        self.compositor.draw_texture_rect(
                            texture,
                            self.cv_video_target,
                            width,
                            height,
                            self.cv_video_width,
                            self.cv_video_height,
                            1.0,   // Full opacity for capture
                            1.0,   // No scaling
                            true,  // Flip Y
                            0.0,   // No luma key for skin capture
                        );

                        // Copy framebuffer to skin texture
                        self.copy_framebuffer_to_skin_texture(width, height);
                    }

                    // Clear to dark background for Mythos render
                    self.gl.clear_color(0.015, 0.008, 0.045, 1.0);
                    self.gl.clear(glow::COLOR_BUFFER_BIT);

                    if params.projectm_visibility > 0.0 {
                        let skin_tex = self.get_skin_texture();
                        let video_aspect = self.get_video_aspect();
                        if let Some(ref mut mythos) = self.mythos {
                            mythos.set_skin_texture(skin_tex, video_aspect);
                            mythos.set_video_params(
                                params.video_visibility,
                                params.video_scale,
                                params.video_luma_key,
                            );
                            mythos.render(width, height);
                        }
                    }
                }
            }

            // Continuous theme visibility — fade the theme layer to black based on
            // params.projectm_visibility (slider in show_projectm_section). The renderer arms
            // above only gate rendering on >0.0; this pass scales the result smoothly. Applied
            // after theme rendering and before camera/video overlays so it only affects the theme.
            let mut theme_fade = 1.0 - params.projectm_visibility.clamp(0.0, 1.0);
            // projectM has no audio gate of its own — fade it to black on silence so
            // it goes dark with the lights and compels. The LED Wall and Geometric
            // Field already self-gate, so only projectM needs this. Darker fade wins.
            if self.render_mode == RenderMode::ProjectM {
                theme_fade = theme_fade.max(self.projectm_silence.blackout_alpha);
            }
            self.compositor.draw_fade_to_black(theme_fade);

            // projectM has no GLB of its own — borrow the Geometric Field's
            // silence GLB (the REG logo) so projectM's blacked-out silence state
            // shows the same model the Geometric Field does. Drawn over the
            // faded-black theme, before the camera/video overlays, matching where
            // the GLB sits in HarmonicGeometry's own render. Fades/shrinks in with
            // projectm_silence.blackout_alpha; the call no-ops below the threshold.
            if self.render_mode == RenderMode::ProjectM {
                let glb_show = self.projectm_silence.blackout_alpha;
                if let Some(ref hg) = self.harmonic_geometry {
                    hg.render_silence_glb(width, height, glb_show);
                }
            }

            // Draw fullscreen camera layer between theme and video overlay
            self.draw_fullscreen_camera_layer(width, height, params.camera_luma_key);

            // Draw video on top of visuals with smooth blending
            // For HarmonicGeometry: use luma key to make black areas transparent so HG shows through
            // For Mythos: skip — the compel IS the Mythos surface (skin_mode=4 dominant), so it
            //   rides the field with audio-reactive push/pull via Instability/Disturbance/Field-Strength.
            //   A flat overlay on top would just hide the field-driven distortion.
            if params.video_visibility > 0.0 && self.render_mode != RenderMode::Mythos {
                // Try zero-copy CV texture first (macOS)
                #[cfg(target_os = "macos")]
                if let Some(cv_tex_id) = self.cv_video_texture {
                    // Set blending for video overlay
                    self.gl.enable(glow::BLEND);
                    self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

                    let texture = glow::NativeTexture(std::num::NonZeroU32::new(cv_tex_id).unwrap());
                    self.compositor.draw_texture_rect(
                        texture,
                        self.cv_video_target,
                        width,
                        height,
                        self.cv_video_width,
                        self.cv_video_height,
                        params.video_visibility,
                        params.video_scale,
                        true,  // flip Y (video is top-down)
                        params.video_luma_key,
                    );
                } else if let Some(video_tex) = self.video_texture {
                    self.gl.enable(glow::BLEND);
                    self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

                    self.compositor.draw_texture_scaled_ex(
                        video_tex,
                        width,
                        height,
                        self.video_width,
                        self.video_height,
                        params.video_visibility,
                        params.video_scale,
                        false,
                        true,
                        params.video_luma_key,
                    );
                }

                #[cfg(not(target_os = "macos"))]
                if let Some(video_tex) = self.video_texture {
                    self.gl.enable(glow::BLEND);
                    self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

                    self.compositor.draw_texture_scaled_ex(
                        video_tex,
                        width,
                        height,
                        self.video_width,
                        self.video_height,
                        params.video_visibility,
                        params.video_scale,
                        false,
                        true,
                        params.video_luma_key,
                    );
                }
            }

            // Composite camera backgrounds ON TOP of video
            // This allows camera visibility to control whether video shows through
            self.composite_camera_backgrounds(width, height);

            // Render logo overlay if enabled
            if params.logo_visible && params.logo_opacity > 0.0 {
                // Ensure logo is loaded (use custom path if provided)
                self.ensure_logo_loaded(params.custom_logo_path.as_deref());

                if let Some(logo_tex) = self.logo_texture {
                    self.gl.enable(glow::BLEND);
                    self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

                    // Calculate logo size maintaining aspect ratio
                    // Base size is percentage of screen, scaled by logo_scale
                    let logo_aspect = self.logo_width as f32 / self.logo_height.max(1) as f32;
                    let screen_aspect = width as f32 / height.max(1) as f32;
                    // Size in NDC (range is -1 to 1, so 0.3 = 15% of screen)
                    let base_size = 0.25 * params.logo_scale;  // Larger base size
                    let size_x = base_size;
                    let size_y = base_size / logo_aspect * screen_aspect;

                    // Calculate position based on preset
                    // Safe-area margins account for output window aspect ratio crop
                    // Output window uses fill-with-crop mode, which can clip edges
                    // when aspect ratios differ (e.g., 16:10 source on 16:9 output)
                    let margin_x = 0.12;  // ~6% safe area for horizontal crop
                    let margin_y = 0.15;  // ~7.5% safe area for vertical crop
                    let half_x = size_x / 2.0;
                    let half_y = size_y / 2.0;
                    let (pos_x, pos_y) = match params.logo_position {
                        OverlayPosition::TopLeft => (-1.0 + margin_x + half_x, 1.0 - margin_y - half_y),
                        OverlayPosition::TopRight => (1.0 - margin_x - half_x, 1.0 - margin_y - half_y),
                        OverlayPosition::BottomLeft => (-1.0 + margin_x + half_x, -1.0 + margin_y + half_y),
                        OverlayPosition::BottomRight => (1.0 - margin_x - half_x, -1.0 + margin_y + half_y),
                        OverlayPosition::Center => (0.0, 0.0),
                    };

                    // Draw logo overlay with subtle glow
                    self.compositor.draw_overlay(
                        logo_tex,
                        pos_x,
                        pos_y,
                        size_x,
                        size_y,
                        params.logo_opacity,
                        0.2,  // Subtle glow
                    );
                }
            }

            // Render track announcement overlay if visible
            if params.track_overlay.visible && params.track_overlay.opacity > 0.0 {
                // Update track texture if needed
                self.update_track_texture(
                    &params.track_overlay.title,
                    &params.track_overlay.artist,
                    params.track_overlay.opacity,
                    params.track_overlay.animation_progress,
                );

                if let Some(track_tex) = self.track_texture {
                    self.gl.enable(glow::BLEND);
                    self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

                    // Calculate track overlay size - wider for text
                    let tex_aspect = self.track_tex_width as f32 / self.track_tex_height.max(1) as f32;
                    let screen_aspect = width as f32 / height.max(1) as f32;

                    // Size relative to screen - text should be readable
                    let base_height = 0.15;  // 7.5% of screen height
                    let size_y = base_height;
                    let size_x = base_height * tex_aspect / screen_aspect;

                    // Position in bottom-left with safe-area margins
                    let margin_x = 0.12;  // Match logo safe area
                    let margin_y = 0.15;
                    let pos_x = -1.0 + margin_x + size_x / 2.0;
                    let pos_y = -1.0 + margin_y + size_y / 2.0;

                    // Draw track overlay
                    self.compositor.draw_overlay(
                        track_tex,
                        pos_x,
                        pos_y,
                        size_x,
                        size_y,
                        params.track_overlay.opacity * params.track_overlay.animation_progress,
                        0.0,  // No glow for text
                    );
                }
            }

            // NOTE: Preset browser overlay is now rendered via egui in app.rs
            // This keeps it visible only on the main app (DJ view), not on output window/NDI (spectator view)

            // Reset GL state
            self.gl.use_program(None);
            self.gl.bind_vertex_array(None);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }

        // CRITICAL: Flush texture cache to release CVPixelBuffer references
        // Without this, the cache keeps ALL pixel buffers alive, causing massive memory leaks!
        #[cfg(target_os = "macos")]
        self.flush_texture_cache();

        Ok(())
    }

    /// Load a projectM preset
    pub fn load_preset(&mut self, path: &str) -> Result<()> {
        // Ensure projectM is initialized before loading
        self.ensure_projectm_initialized();
        if let Some(ref mut projectm) = self.projectm {
            projectm.load_preset(path)
        } else {
            Err(anyhow!("projectM not available"))
        }
    }

    /// Get available preset categories from ALL renderers
    /// Returns unified list: projectM categories + "LED Wall" + future renderers
    pub fn get_preset_categories(&mut self) -> Vec<String> {
        // Ensure projectM is initialized so we get all categories
        self.ensure_projectm_initialized();

        let mut categories = Vec::new();

        // Add Geometric Field first (primary category)
        categories.push("Geometric Field".to_string());

        // Add Mythos Field
        categories.push("Mythos Field".to_string());

        // Add LED Wall
        categories.push("LED Wall".to_string());

        // Add projectM categories (sorted)
        if let Some(ref projectm) = self.projectm {
            let mut pm_cats = projectm.get_categories();
            pm_cats.sort();
            for cat in pm_cats {
                if cat != "Geometric Field" && cat != "Mythos Field" && cat != "LED Wall" {
                    categories.push(cat);
                }
            }
        }

        categories
    }

    /// Check if projectM presets have finished loading in the background
    /// Returns true if presets are ready, false if still scanning
    pub fn presets_ready(&self) -> bool {
        if let Some(ref projectm) = self.projectm {
            projectm.presets_ready()
        } else {
            false // Not initialized yet
        }
    }

    /// Get presets in a category - works for ANY renderer type
    /// Returns preset names for the category (from whichever renderer owns it)
    pub fn get_presets_in_category(&self, category: &str) -> Vec<String> {
        // LED Wall patterns (always available, no initialization needed)
        if category == "LED Wall" {
            return crate::render::LedPattern::all()
                .iter()
                .map(|p| p.name().to_string())
                .collect();
        }

        // Geometric Field - REG Hero's Journey mode only (no presets)
        if category == "Geometric Field" {
            return vec!["Hero's Journey".to_string()];
        }

        // Mythos Field presets
        if category == "Mythos Field" {
            return vec![
                "Mythos".to_string(),
                "Deep Aether".to_string(),
                "Hailstone Storm".to_string(),
                "Harmonic Drift".to_string(),
                "Solar Furnace".to_string(),
            ];
        }

        // projectM presets (only if initialized)
        if let Some(ref projectm) = self.projectm {
            return projectm.get_presets_in_category(category);
        }

        Vec::new()
    }

    /// Set random preset
    /// Returns (category, preset_name) if successful
    pub fn set_random_preset(&mut self) -> Option<(String, String)> {
        // Ensure projectM is initialized before setting random preset
        self.ensure_projectm_initialized();
        if let Some(ref mut projectm) = self.projectm {
            if let Some((category, name)) = projectm.set_random_preset() {
                // Switch to ProjectM mode so the preset is actually displayed
                self.set_render_mode(RenderMode::ProjectM);
                Some((category, name))
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Load a preset by category and name
    /// Load preset by category and name - works for ANY renderer
    /// Automatically switches render mode based on the preset's renderer type
    pub fn load_preset_by_name(&mut self, category: &str, name: &str) -> Result<()> {
        log::info!("[Pipeline] load_preset_by_name called: category='{}', name='{}'", category, name);

        // LED Wall patterns
        if category == "LED Wall" {
            self.ensure_led_wall_initialized();
            // Find pattern by name
            for pattern in crate::render::LedPattern::all() {
                if pattern.name() == name {
                    self.set_led_wall_pattern(*pattern);
                    self.set_render_mode(RenderMode::LedWall);
                    log::info!("[Pipeline] Loaded LED Wall pattern: {}", name);
                    return Ok(());
                }
            }
            return Err(anyhow!("Unknown LED Wall pattern: {}", name));
        }

        // Mythos Field
        if category == "Mythos Field" {
            self.ensure_mythos_initialized();
            if let Some(ref mut mythos) = self.mythos {
                use super::ThemeRenderer;
                let result = mythos.load_preset_by_name(category, name);
                if result.is_ok() {
                    self.set_render_mode(RenderMode::Mythos);
                    log::info!("[Pipeline] Loaded Mythos preset: {}", name);
                }
                return result;
            }
            return Err(anyhow!("Mythos not available"));
        }

        // Geometric Field presets
        if category == "Geometric Field" {
            log::info!("[Pipeline] Handling Geometric Field category, init_attempted={}, has_renderer={}",
                       self.harmonic_geometry_init_attempted, self.harmonic_geometry.is_some());
            self.ensure_harmonic_geometry_initialized();
            log::info!("[Pipeline] After init: has_renderer={}", self.harmonic_geometry.is_some());
            if let Some(ref mut hg) = self.harmonic_geometry {
                use super::ThemeRenderer;
                let result = hg.load_preset_by_name(category, name);
                if result.is_ok() {
                    self.set_render_mode(RenderMode::HarmonicGeometry);
                    log::info!("[Pipeline] Loaded Geometric Field preset: {}", name);
                }
                return result;
            }
            return Err(anyhow!("Geometric Field not available"));
        }

        // projectM presets
        self.ensure_projectm_initialized();
        if let Some(ref mut projectm) = self.projectm {
            let result = projectm.load_preset_by_name(category, name);
            if result.is_ok() {
                self.set_render_mode(RenderMode::ProjectM);
            }
            return result;
        }

        Err(anyhow!("No renderer available for category: {}", category))
    }

    /// Get preset file path by category and name
    pub fn get_preset_path(&self, category: &str, name: &str) -> Option<String> {
        if let Some(ref projectm) = self.projectm {
            projectm.get_preset_path(category, name)
        } else {
            None
        }
    }

    /// Get preset subcategory (folder hierarchy within category)
    pub fn get_preset_subcategory(&self, category: &str, name: &str) -> Option<String> {
        if let Some(ref projectm) = self.projectm {
            projectm.get_preset_subcategory(category, name)
        } else {
            None
        }
    }

    /// Populate a preset registry with presets from all renderers
    /// Ensures renderers are initialized first
    pub fn populate_registry(&mut self, registry: &mut super::PresetRegistry) {
        use super::ThemeRenderer;

        // Clear existing presets (keeps favorites/pinned state)
        registry.clear();

        // Get projectM presets
        self.ensure_projectm_initialized();
        if let Some(ref mut projectm) = self.projectm {
            let presets = ThemeRenderer::scan_presets(projectm);
            log::info!("[Pipeline] Registering {} projectM presets", presets.len());
            registry.register_all(presets);
        }

        // Get LED Wall patterns
        self.ensure_led_wall_initialized();
        if let Some(ref mut led_wall) = self.led_wall {
            let presets = ThemeRenderer::scan_presets(led_wall);
            log::info!("[Pipeline] Registering {} LED Wall patterns", presets.len());
            registry.register_all(presets);
        }

        // Get Geometric Field presets
        self.ensure_harmonic_geometry_initialized();
        if let Some(ref mut hg) = self.harmonic_geometry {
            let presets = ThemeRenderer::scan_presets(hg);
            log::info!("[Pipeline] Registering {} Geometric Field presets", presets.len());
            registry.register_all(presets);
        }

        // Get Mythos presets
        self.ensure_mythos_initialized();
        if let Some(ref mut mythos) = self.mythos {
            let presets = ThemeRenderer::scan_presets(mythos);
            log::info!("[Pipeline] Registering {} Mythos presets", presets.len());
            registry.register_all(presets);
        }

        log::info!("[Pipeline] Registry populated with {} total presets", registry.count());
    }

    /// Load a preset from registry by ID
    pub fn load_preset_from_registry(&mut self, preset_id: &str) -> Result<()> {
        use super::ThemeRenderer;

        // Determine renderer type from preset ID
        if preset_id.starts_with("led_wall:") {
            self.ensure_led_wall_initialized();
            if let Some(ref mut led_wall) = self.led_wall {
                let result = ThemeRenderer::load_preset(led_wall, preset_id);
                if result.is_ok() {
                    self.set_render_mode(RenderMode::LedWall);
                }
                return result;
            }
            return Err(anyhow!("LED Wall not available"));
        }

        if preset_id.starts_with("mythos:") {
            self.ensure_mythos_initialized();
            if let Some(ref mut mythos) = self.mythos {
                let result = ThemeRenderer::load_preset(mythos, preset_id);
                if result.is_ok() {
                    self.set_render_mode(RenderMode::Mythos);
                }
                return result;
            }
            return Err(anyhow!("Mythos not available"));
        }

        if preset_id.starts_with("harmonic_geometry:") {
            self.ensure_harmonic_geometry_initialized();
            if let Some(ref mut hg) = self.harmonic_geometry {
                let result = ThemeRenderer::load_preset(hg, preset_id);
                if result.is_ok() {
                    self.set_render_mode(RenderMode::HarmonicGeometry);
                }
                return result;
            }
            return Err(anyhow!("Geometric Field not available"));
        }

        // Default to projectM (file path presets)
        self.ensure_projectm_initialized();
        if let Some(ref mut projectm) = self.projectm {
            let result = ThemeRenderer::load_preset(projectm, preset_id);
            if result.is_ok() {
                self.set_render_mode(RenderMode::ProjectM);
            }
            return result;
        }
        Err(anyhow!("projectM not available"))
    }

    /// Get current LED Wall pattern
    pub fn get_led_wall_pattern(&self) -> Option<super::LedPattern> {
        // On macOS, prefer Metal LED Wall
        #[cfg(target_os = "macos")]
        {
            if let Some(ref metal_wall) = self.metal_led_wall {
                return Some(metal_wall.current_pattern());
            }
        }
        // Fallback to OpenGL LED Wall
        self.led_wall.as_ref().map(|lw| lw.current_pattern())
    }

    /// Set LED Wall pattern
    pub fn set_led_wall_pattern(&mut self, pattern: super::LedPattern) {
        self.ensure_led_wall_initialized();
        // On macOS, prefer Metal LED Wall
        #[cfg(target_os = "macos")]
        {
            if let Some(ref mut metal_wall) = self.metal_led_wall {
                metal_wall.set_pattern(pattern);
                return;
            }
        }
        // Fallback to OpenGL LED Wall
        if let Some(ref mut led_wall) = self.led_wall {
            led_wall.set_pattern(pattern);
        }
    }

    /// Get Collatz state from Mythos for OSC/lighting export
    pub fn get_collatz_output(&self) -> Option<super::CollatzOutput> {
        self.mythos.as_ref().map(|m| m.collatz_output())
    }

    /// Set Mythos engine parameters from UI
    pub fn set_mythos_params(
        &mut self,
        instability: f32,
        field_strength: f32,
        disturbance_size: f32,
        harmonic_influence: f32,
        decay: f32,
    ) {
        if let Some(ref mut mythos) = self.mythos {
            mythos.params.instability = instability;
            mythos.params.field_strength = field_strength;
            mythos.params.disturbance_size = disturbance_size;
            mythos.params.harmonic_influence = harmonic_influence;
            mythos.params.decay = decay;
        }
    }

    /// Add audio samples for projectM
    /// PERFORMANCE: Takes &self to allow audio thread to add samples without blocking render
    pub fn add_audio_samples(&self, samples: &[f32]) {
        if let Some(ref projectm) = self.projectm {
            projectm.add_audio_samples(samples);
        }
    }

    /// Set video texture from decoded frame (RGBA data)
    /// Uses PBO for async upload to avoid GPU pipeline stalls
    pub fn set_video_frame(&mut self, rgba_data: &[u8], width: u32, height: u32) {
        if width == 0 || height == 0 || rgba_data.is_empty() {
            return;
        }

        let expected_size = (width * height * 4) as usize;
        if rgba_data.len() != expected_size {
            log::warn!("[Pipeline] Video frame size mismatch: got {}, expected {}",
                rgba_data.len(), expected_size);
            return;
        }

        // Ensure PBOs are allocated for this frame size
        self.ensure_pbos(expected_size);

        unsafe {
            // Create or resize texture if needed
            if self.video_texture.is_none() || self.video_width != width || self.video_height != height {
                // Delete old texture if exists
                if let Some(tex) = self.video_texture.take() {
                    self.gl.delete_texture(tex);
                }

                // Create new texture (allocate storage only, no data yet)
                // PERFORMANCE: Use BGRA format which is native on macOS GPU
                // This avoids any format conversion during upload
                let texture = self.gl.create_texture()
                    .expect("Failed to create video texture");

                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,  // Internal format (how GPU stores it)
                    width as i32,
                    height as i32,
                    0,
                    glow::BGRA,          // Upload format matches AVFoundation output
                    glow::UNSIGNED_BYTE,
                    None,  // No data - just allocate storage
                );
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                self.gl.bind_texture(glow::TEXTURE_2D, None);

                self.video_texture = Some(texture);
                self.video_width = width;
                self.video_height = height;

                log::info!("[Pipeline] Created video texture: {}x{} (PBO upload)", width, height);
            }

            // Use PBO for async texture upload
            if !self.video_pbos.is_empty() {
                let pbo = self.video_pbos[self.pbo_index];

                // Bind PBO for pixel unpack operations
                self.gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(pbo));

                // Orphan the old buffer data (allows GPU to continue using old data while we write new)
                // This is the key to async: GPU can still read old buffer while we write to new allocation
                self.gl.buffer_data_size(
                    glow::PIXEL_UNPACK_BUFFER,
                    expected_size as i32,
                    glow::STREAM_DRAW,
                );

                // Map buffer for writing (this should be fast - just returns pointer)
                // Note: glow doesn't have map_buffer, so we use buffer_sub_data which is still faster
                // than direct texture upload because it goes through DMA
                self.gl.buffer_sub_data_u8_slice(
                    glow::PIXEL_UNPACK_BUFFER,
                    0,
                    rgba_data,
                );

                // Now update texture from PBO (async - GPU does DMA transfer)
                // PERFORMANCE: Use BGRA format to match source data (no conversion needed)
                if let Some(texture) = self.video_texture {
                    self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    // Offset of 0 means "read from currently bound PBO at offset 0"
                    self.gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0, 0,
                        width as i32,
                        height as i32,
                        glow::BGRA,  // Native format - no conversion during upload
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::BufferOffset(0),
                    );
                    self.gl.bind_texture(glow::TEXTURE_2D, None);
                }

                // Unbind PBO
                self.gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);

                // Rotate to next PBO for next frame
                self.pbo_index = (self.pbo_index + 1) % self.video_pbos.len();
            } else {
                // Fallback: direct upload (no PBOs available)
                if let Some(texture) = self.video_texture {
                    self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    self.gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0, 0,
                        width as i32,
                        height as i32,
                        glow::BGRA,  // Native format
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(rgba_data),
                    );
                    self.gl.bind_texture(glow::TEXTURE_2D, None);
                }
            }
        }
    }

    /// Clear video texture
    pub fn clear_video(&mut self) {
        if let Some(tex) = self.video_texture.take() {
            unsafe {
                self.gl.delete_texture(tex);
            }
        }
        self.video_width = 0;
        self.video_height = 0;

        // Also clear CV texture state
        #[cfg(target_os = "macos")]
        {
            self.cv_video_texture = None;
            self.cv_video_width = 0;
            self.cv_video_height = 0;
        }

        log::info!("[Pipeline] Cleared video texture");
    }

    /// Check if video is loaded
    pub fn has_video(&self) -> bool {
        #[cfg(target_os = "macos")]
        if self.cv_video_texture.is_some() {
            return true;
        }
        self.video_texture.is_some()
    }

    /// Ensure texture cache is initialized (macOS only)
    #[cfg(target_os = "macos")]
    fn ensure_texture_cache(&mut self) -> bool {
        if self.texture_cache.is_some() {
            return true;
        }

        match TextureCache::new() {
            Ok(cache) => {
                log::info!("[Pipeline] CVOpenGLTextureCache initialized for zero-copy video");
                self.texture_cache = Some(cache);
                true
            }
            Err(e) => {
                log::warn!("[Pipeline] Failed to create TextureCache: {} - falling back to PBO path", e);
                false
            }
        }
    }

    /// Set video texture from CVPixelBuffer (zero-copy IOSurface path, macOS only)
    /// This creates a GPU texture directly from the pixel buffer without any CPU memcpy.
    #[cfg(target_os = "macos")]
    pub fn set_video_pixel_buffer(&mut self, pixel_buffer: &RetainedPixelBuffer) -> bool {
        // Ensure texture cache is initialized
        if !self.ensure_texture_cache() {
            return false;
        }

        // Check if this pixel buffer is IOSurface-backed
        if !pixel_buffer.is_iosurface_backed() {
            log::warn!("[Pipeline] Pixel buffer is not IOSurface-backed, falling back to PBO path");
            return false;
        }

        // Create texture from pixel buffer
        if let Some(ref mut cache) = self.texture_cache {
            match cache.create_texture_from_pixel_buffer(pixel_buffer.as_ptr()) {
                Ok((texture_id, texture_target, width, height)) => {
                    self.cv_video_texture = Some(texture_id);
                    self.cv_video_target = texture_target;
                    self.cv_video_width = width;
                    self.cv_video_height = height;

                    #[cfg(debug_assertions)]
                    if self.frame_count % 300 == 0 {
                        log::debug!("[Pipeline] Zero-copy texture created: {}x{} (target=0x{:X})",
                            width, height, texture_target);
                    }

                    true
                }
                Err(e) => {
                    log::warn!("[Pipeline] Failed to create texture from pixel buffer: {}", e);
                    false
                }
            }
        } else {
            false
        }
    }

    /// Flush the texture cache (call after rendering to ensure GPU is done)
    #[cfg(target_os = "macos")]
    pub fn flush_texture_cache(&self) {
        if let Some(ref cache) = self.texture_cache {
            cache.flush();
        }
    }

    /// Ensure skin texture exists with correct size (macOS)
    /// Called before render to prepare the texture
    #[cfg(target_os = "macos")]
    fn update_skin_texture_from_cv(&mut self) {
        if self.cv_video_texture.is_none() {
            return;
        }

        let width = self.cv_video_width;
        let height = self.cv_video_height;

        if width == 0 || height == 0 {
            return;
        }

        unsafe {
            // Create or resize skin texture if needed
            if self.skin_video_texture.is_none() || self.skin_video_width != width || self.skin_video_height != height {
                // Delete old texture
                if let Some(tex) = self.skin_video_texture.take() {
                    self.gl.delete_texture(tex);
                }

                // Create new TEXTURE_2D for skin
                let texture = self.gl.create_texture().expect("Failed to create skin texture");
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    width as i32,
                    height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    None,
                );
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                self.gl.bind_texture(glow::TEXTURE_2D, None);

                self.skin_video_texture = Some(texture);
                self.skin_video_width = width;
                self.skin_video_height = height;

                log::info!("[Pipeline] Created skin texture: {}x{}", width, height);
            }
        }
    }

    /// Copy current framebuffer content to skin texture (called after video is drawn)
    /// FIX: Resize texture to match framebuffer dimensions so UV coordinates map correctly
    #[cfg(target_os = "macos")]
    fn copy_framebuffer_to_skin_texture(&mut self, fb_width: u32, fb_height: u32) {
        if fb_width == 0 || fb_height == 0 {
            return;
        }

        unsafe {
            // Resize skin texture to match framebuffer if needed
            // This ensures UV (0.5, 0.5) always maps to framebuffer center
            if self.skin_video_texture.is_none() || self.skin_video_width != fb_width || self.skin_video_height != fb_height {
                // Delete old texture
                if let Some(tex) = self.skin_video_texture.take() {
                    self.gl.delete_texture(tex);
                }

                // Create new TEXTURE_2D at framebuffer dimensions
                let texture = self.gl.create_texture().expect("Failed to create skin texture");
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    fb_width as i32,
                    fb_height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    None,
                );
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

                self.skin_video_texture = Some(texture);
                self.skin_video_width = fb_width;
                self.skin_video_height = fb_height;

                log::info!("[Pipeline] Resized skin texture to framebuffer: {}x{}", fb_width, fb_height);
            }

            if let Some(skin_tex) = self.skin_video_texture {
                // Copy entire framebuffer to texture
                // This ensures UV coordinates map directly to framebuffer positions
                self.gl.bind_texture(glow::TEXTURE_2D, Some(skin_tex));

                self.gl.copy_tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,           // level
                    0,           // xoffset in texture
                    0,           // yoffset in texture
                    0,           // x in framebuffer (copy from origin)
                    0,           // y in framebuffer (copy from origin)
                    fb_width as i32,
                    fb_height as i32,
                );

                self.gl.bind_texture(glow::TEXTURE_2D, None);
            }
        }
    }

    /// Get the skin-compatible video texture (TEXTURE_2D)
    /// On macOS with CV textures, this returns the converted copy
    /// On other platforms, returns the regular video texture
    pub fn get_skin_texture(&self) -> Option<glow::Texture> {
        #[cfg(target_os = "macos")]
        {
            // Prefer skin texture (converted from CV) if available
            if self.skin_video_texture.is_some() {
                return self.skin_video_texture;
            }
        }
        // Fall back to regular video texture
        self.video_texture
    }

    /// Get the video aspect ratio (width / height)
    /// Returns 1.0 if no video is loaded
    pub fn get_video_aspect(&self) -> f32 {
        #[cfg(target_os = "macos")]
        {
            if self.cv_video_width > 0 && self.cv_video_height > 0 {
                return self.cv_video_width as f32 / self.cv_video_height as f32;
            }
        }
        // Fall back to regular video dimensions
        if self.video_width > 0 && self.video_height > 0 {
            self.video_width as f32 / self.video_height as f32
        } else {
            1.0 // Default to square
        }
    }

    // ===== Camera Media Background Methods =====

    /// Ensure texture exists for camera, creating if necessary
    /// Returns the texture handle
    pub fn ensure_camera_texture(&mut self, camera_index: u32, width: u32, height: u32) -> Option<glow::Texture> {
        // Check if texture exists and dimensions match
        if let Some(entry) = self.camera_media.get(&camera_index) {
            if entry.width == width && entry.height == height {
                return Some(entry.texture);
            }
            // Dimensions changed, delete old texture
            unsafe {
                self.gl.delete_texture(entry.texture);
            }
        }

        // Create new texture
        unsafe {
            match self.gl.create_texture() {
                Ok(texture) => {
                    self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    self.gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGB8 as i32,
                        width as i32,
                        height as i32,
                        0,
                        glow::RGB,
                        glow::UNSIGNED_BYTE,
                        None,
                    );
                    self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                    self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                    self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                    self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                    self.gl.bind_texture(glow::TEXTURE_2D, None);

                    self.camera_media.insert(camera_index, CameraMediaEntry {
                        texture,
                        width,
                        height,
                        alpha: 0.0,
                    });

                    // Add to composite order if not already there
                    if !self.camera_composite_order.contains(&camera_index) {
                        self.camera_composite_order.push(camera_index);
                        self.camera_composite_order.sort();
                    }

                    log::info!("[Pipeline] Created camera {} texture: {}x{}", camera_index, width, height);
                    Some(texture)
                }
                Err(e) => {
                    log::error!("[Pipeline] Failed to create camera {} texture: {}", camera_index, e);
                    None
                }
            }
        }
    }

    /// Upload RGB frame data to camera texture
    pub fn upload_camera_frame(&mut self, camera_index: u32, data: &[u8], width: u32, height: u32) {
        // Ensure texture exists
        let texture = match self.ensure_camera_texture(camera_index, width, height) {
            Some(t) => t,
            None => return,
        };

        let expected_size = (width * height * 3) as usize;
        if data.len() != expected_size {
            log::warn!("[Pipeline] Camera {} frame size mismatch: {} vs expected {}", camera_index, data.len(), expected_size);
            return;
        }

        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                width as i32,
                height as i32,
                glow::RGB,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(data),
            );
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    /// Update camera media alpha value (from smoothed visibility)
    pub fn update_camera_media_alpha(&mut self, camera_index: u32, alpha: f32) {
        if let Some(entry) = self.camera_media.get_mut(&camera_index) {
            entry.alpha = alpha;
        }
    }

    /// Select the active camera for HarmonicGeometry rendering
    /// Picks the camera with highest alpha that has a valid texture
    fn select_active_camera_for_hg(&mut self) {
        self.active_camera_for_hg = None;

        if !self.camera_compositor_config.cameras_enabled {
            return;
        }

        let luma_cap = self.camera_compositor_config.luma_cap;
        let mut best_alpha = 0.0f32;
        let mut best_camera: Option<(u32, glow::Texture, u32, u32, f32)> = None;

        // Log diagnostic info (throttled to every 120 frames = ~2 sec)
        let should_log = self.camera_frame_counter % 120 == 0;
        if should_log && !self.camera_composite_order.is_empty() {
            let alphas: Vec<_> = self.camera_composite_order.iter()
                .filter_map(|idx| self.camera_media.get(idx).map(|e| (*idx, e.alpha)))
                .collect();
            log::info!("[CamHG] composite_order={:?}, alphas={:?}, entries={}",
                self.camera_composite_order,
                alphas,
                self.camera_media.len()
            );
        }

        // Find camera with highest alpha
        for camera_index in &self.camera_composite_order {
            if let Some(entry) = self.camera_media.get(camera_index) {
                if entry.alpha > 0.001 && entry.alpha > best_alpha {
                    best_alpha = entry.alpha;
                    best_camera = Some((*camera_index, entry.texture, entry.width, entry.height, entry.alpha.min(luma_cap)));
                }
            }
        }

        if should_log && best_camera.is_some() {
            log::info!("[CamHG] Selected camera for HG: {:?}", best_camera.map(|(idx, _, w, h, a)| (idx, w, h, a)));
        }

        self.active_camera_for_hg = best_camera;
    }

    /// Draw fullscreen camera layer between theme and video overlay.
    /// Selects the camera with the highest alpha (vis_smoothed) and draws it fullscreen.
    /// The camera_luma_key parameter makes dark camera pixels transparent, revealing the theme.
    fn draw_fullscreen_camera_layer(&mut self, width: u32, height: u32, camera_luma_key: f32) {
        if !self.camera_compositor_config.cameras_enabled {
            return;
        }

        // Find the camera with the highest alpha
        let mut best_alpha = 0.0f32;
        let mut best_camera: Option<(glow::Texture, u32, u32, f32)> = None;

        for camera_index in &self.camera_composite_order {
            if let Some(entry) = self.camera_media.get(camera_index) {
                if entry.alpha > 0.001 && entry.alpha > best_alpha {
                    best_alpha = entry.alpha;
                    best_camera = Some((entry.texture, entry.width, entry.height, entry.alpha));
                }
            }
        }

        if let Some((texture, tex_w, tex_h, alpha)) = best_camera {
            unsafe {
                self.gl.enable(glow::BLEND);
                self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            }

            self.compositor.draw_texture_scaled_ex(
                texture,
                width,
                height,
                tex_w,
                tex_h,
                alpha,
                1.0,   // scale = fullscreen
                false,  // swizzle_bgra = false (RGB data)
                true,   // flip_y = true (camera is top-down)
                camera_luma_key,
            );
        }
    }

    /// Composite all active camera backgrounds
    /// Camera is now rendered inside HarmonicGeometry shader as a texture
    /// This method just advances the scheduler for frame counting
    pub fn composite_camera_backgrounds(&mut self, _width: u32, _height: u32) {
        // Camera compositing is now handled by HarmonicGeometry shader
        // This method just advances the scheduler

        if !self.camera_compositor_config.cameras_enabled {
            return;
        }

        // Advance scheduler every 4 frames
        self.camera_frame_counter += 1;
        if self.camera_frame_counter % 4 == 0 {
            self.camera_compositor_config.advance_scheduler();
        }
    }

    /// Get mutable reference to camera compositor config
    pub fn camera_compositor_config_mut(&mut self) -> &mut crate::camera::CameraCompositorConfig {
        &mut self.camera_compositor_config
    }

    /// Get reference to camera compositor config
    pub fn camera_compositor_config(&self) -> &crate::camera::CameraCompositorConfig {
        &self.camera_compositor_config
    }

    /// Remove camera texture (when camera is removed)
    pub fn remove_camera_texture(&mut self, camera_index: u32) {
        if let Some(entry) = self.camera_media.remove(&camera_index) {
            unsafe {
                self.gl.delete_texture(entry.texture);
            }
        }
        self.camera_composite_order.retain(|&idx| idx != camera_index);
    }

    /// Clean up OpenGL resources
    pub fn cleanup(&mut self, gl: &glow::Context) {
        log::info!("Cleaning up render pipeline");

        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.fbo_texture);

            if let Some(tex) = self.video_texture {
                gl.delete_texture(tex);
            }

            // Clean up PBOs
            for pbo in self.video_pbos.drain(..) {
                gl.delete_buffer(pbo);
            }

            // Clean up skin texture resources
            if let Some(tex) = self.skin_video_texture.take() {
                gl.delete_texture(tex);
            }
            if let Some(fbo) = self.skin_video_fbo.take() {
                gl.delete_framebuffer(fbo);
            }

            // Clean up camera media textures
            for (_, entry) in self.camera_media.drain() {
                gl.delete_texture(entry.texture);
            }
            self.camera_composite_order.clear();
        }

        if let Some(ref mut projectm) = self.projectm {
            projectm.cleanup();
        }

        // LED wall cleanup happens via Drop trait
        self.led_wall = None;

        self.compositor.cleanup(gl);
        self.initialized = false;
    }
}
