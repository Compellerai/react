//! Main application state and update loop

use eframe::{egui, glow};
use eframe::glow::HasContext;
use std::sync::Arc;
use std::sync::mpsc;
use std::path::PathBuf;
use parking_lot::Mutex;

use crate::audio::{AudioCapture, AudioSource, AudioAnalyzer, AudioAnalysis, AudioMeters, MeterValues};
use crate::render::{RenderPipeline, RenderParams};
use crate::ui::ControlPanel;
use crate::video::VideoDecoder;
use crate::db::Database;
use crate::track::{TrackAnnouncement, AbletonLinkListener, AppleMusicSource, ProDjLinkSource, TrackSource};
use crate::output::OutputManager;
use crate::compel_matcher;
use crate::compel_picker;
use std::sync::atomic::{AtomicU32, Ordering};

/// Find a resource file by searching paths relative to executable and working directory
fn find_resource(relative_path: &str) -> Option<PathBuf> {
    crate::resources::find(relative_path)
}

/// Serialize AudioSource to string for database storage
/// Format: "system" | "app:bundle_id:name" | "input:device_name" | "mic"
fn serialize_audio_source(source: &AudioSource) -> String {
    match source {
        AudioSource::SystemAudio => "system".to_string(),
        AudioSource::Application { bundle_id, name } => format!("app:{}:{}", bundle_id, name),
        AudioSource::InputDevice { name, device_name } => format!("input:{}", device_name),
        AudioSource::Microphone => "mic".to_string(),
    }
}

/// Parse AudioSource from database string
fn parse_audio_source(s: &str) -> Option<AudioSource> {
    if s == "system" {
        return Some(AudioSource::SystemAudio);
    }
    if s == "mic" {
        return Some(AudioSource::Microphone);
    }
    if let Some(rest) = s.strip_prefix("app:") {
        // Format: "app:bundle_id:name"
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            return Some(AudioSource::Application {
                bundle_id: parts[0].to_string(),
                name: parts[1].to_string(),
            });
        }
    }
    if let Some(device_name) = s.strip_prefix("input:") {
        return Some(AudioSource::InputDevice {
            name: device_name.to_string(),
            device_name: device_name.to_string(),
        });
    }
    None
}

// macOS IOSurface zero-copy support
#[cfg(target_os = "macos")]
mod iosurface_support {
    use std::ffi::c_void;

    // CGL and IOSurface FFI declarations
    #[link(name = "OpenGL", kind = "framework")]
    extern "C" {
        pub fn CGLGetCurrentContext() -> *mut c_void;
        pub fn CGLTexImageIOSurface2D(
            ctx: *mut c_void,
            target: u32,
            internal_format: u32,
            width: i32,
            height: i32,
            format: u32,
            type_: u32,
            io_surface: *mut c_void,
            plane: u32,
        ) -> i32;
        // GL functions from OpenGL framework
        pub fn glGenTextures(n: i32, textures: *mut u32);
        pub fn glBindTexture(target: u32, texture: u32);
        pub fn glGenFramebuffers(n: i32, framebuffers: *mut u32);
        pub fn glBindFramebuffer(target: u32, framebuffer: u32);
        pub fn glFramebufferTexture2D(target: u32, attachment: u32, textarget: u32, texture: u32, level: i32);
        pub fn glBlitFramebuffer(
            srcX0: i32, srcY0: i32, srcX1: i32, srcY1: i32,
            dstX0: i32, dstY0: i32, dstX1: i32, dstY1: i32,
            mask: u32, filter: u32
        );
        pub fn glFlush();
        pub fn glFinish();  // Blocks until GPU completes all commands
        pub fn glDisable(cap: u32);
        pub fn glReadPixels(
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            format: u32,
            type_: u32,
            pixels: *mut std::ffi::c_void,
        );
        // PBO functions for async readback
        pub fn glGenBuffers(n: i32, buffers: *mut u32);
        pub fn glDeleteBuffers(n: i32, buffers: *const u32);
        pub fn glBindBuffer(target: u32, buffer: u32);
        pub fn glBufferData(target: u32, size: isize, data: *const std::ffi::c_void, usage: u32);
        pub fn glMapBuffer(target: u32, access: u32) -> *mut std::ffi::c_void;
        pub fn glUnmapBuffer(target: u32) -> u8;
        // Fence sync for proper GPU synchronization
        pub fn glFenceSync(condition: u32, flags: u32) -> *mut std::ffi::c_void;
        pub fn glClientWaitSync(sync: *mut std::ffi::c_void, flags: u32, timeout: u64) -> u32;
        pub fn glDeleteSync(sync: *mut std::ffi::c_void);

        // Shader functions for bicubic upscaling
        pub fn glCreateShader(shader_type: u32) -> u32;
        pub fn glDeleteShader(shader: u32);
        pub fn glShaderSource(shader: u32, count: i32, string: *const *const i8, length: *const i32);
        pub fn glCompileShader(shader: u32);
        pub fn glGetShaderiv(shader: u32, pname: u32, params: *mut i32);
        pub fn glGetShaderInfoLog(shader: u32, bufSize: i32, length: *mut i32, infoLog: *mut i8);
        pub fn glCreateProgram() -> u32;
        pub fn glDeleteProgram(program: u32);
        pub fn glAttachShader(program: u32, shader: u32);
        pub fn glLinkProgram(program: u32);
        pub fn glGetProgramiv(program: u32, pname: u32, params: *mut i32);
        pub fn glGetProgramInfoLog(program: u32, bufSize: i32, length: *mut i32, infoLog: *mut i8);
        pub fn glUseProgram(program: u32);
        pub fn glGetUniformLocation(program: u32, name: *const i8) -> i32;
        pub fn glUniform1i(location: i32, v0: i32);
        pub fn glUniform2f(location: i32, v0: f32, v1: f32);

        // VAO/VBO functions
        pub fn glGenVertexArrays(n: i32, arrays: *mut u32);
        pub fn glDeleteVertexArrays(n: i32, arrays: *const u32);
        pub fn glBindVertexArray(array: u32);
        pub fn glVertexAttribPointer(index: u32, size: i32, type_: u32, normalized: u8, stride: i32, pointer: *const c_void);
        pub fn glEnableVertexAttribArray(index: u32);
        pub fn glDrawArrays(mode: u32, first: i32, count: i32);

        // Texture functions
        pub fn glActiveTexture(texture: u32);
        pub fn glTexParameteri(target: u32, pname: u32, param: i32);
        pub fn glTexImage2D(target: u32, level: i32, internalformat: i32, width: i32, height: i32, border: i32, format: u32, type_: u32, pixels: *const c_void);
        pub fn glDeleteTextures(n: i32, textures: *const u32);
        pub fn glDeleteFramebuffers(n: i32, framebuffers: *const u32);
        pub fn glViewport(x: i32, y: i32, width: i32, height: i32);
    }

    // PBO constants
    pub const GL_PIXEL_PACK_BUFFER: u32 = 0x88EB;
    pub const GL_STREAM_READ: u32 = 0x88E1;
    pub const GL_READ_ONLY: u32 = 0x88B8;
    // Fence sync constants
    pub const GL_SYNC_GPU_COMMANDS_COMPLETE: u32 = 0x9117;
    pub const GL_SYNC_FLUSH_COMMANDS_BIT: u32 = 0x00000001;
    pub const GL_ALREADY_SIGNALED: u32 = 0x911A;
    pub const GL_CONDITION_SATISFIED: u32 = 0x911C;

    #[link(name = "IOSurface", kind = "framework")]
    extern "C" {
        pub fn IOSurfaceLookup(surface_id: u32) -> *mut c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(cf: *mut c_void);
    }

    // GL constants
    pub const GL_TEXTURE_RECTANGLE: u32 = 0x84F5;
    pub const GL_TEXTURE_2D: u32 = 0x0DE1;
    pub const GL_RGBA: u32 = 0x1908;
    pub const GL_SCISSOR_TEST: u32 = 0x0C11;
    pub const GL_BGRA: u32 = 0x80E1;
    pub const GL_UNSIGNED_INT_8_8_8_8_REV: u32 = 0x8367;
    pub const GL_FRAMEBUFFER: u32 = 0x8D40;
    pub const GL_READ_FRAMEBUFFER: u32 = 0x8CA8;
    pub const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
    pub const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
    pub const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
    pub const GL_NEAREST: u32 = 0x2600;
    pub const GL_LINEAR: u32 = 0x2601;
    pub const GL_UNSIGNED_BYTE: u32 = 0x1401;
    pub const GL_FLOAT: u32 = 0x1406;
    pub const GL_FALSE: u8 = 0;
    pub const GL_ARRAY_BUFFER: u32 = 0x8892;
    pub const GL_STATIC_DRAW: u32 = 0x88E4;
    pub const GL_TRIANGLE_STRIP: u32 = 0x0005;
    pub const GL_TEXTURE0: u32 = 0x84C0;
    pub const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
    pub const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
    pub const GL_TEXTURE_WRAP_S: u32 = 0x2802;
    pub const GL_TEXTURE_WRAP_T: u32 = 0x2803;
    pub const GL_CLAMP_TO_EDGE: u32 = 0x812F;
    pub const GL_VERTEX_SHADER: u32 = 0x8B31;
    pub const GL_FRAGMENT_SHADER: u32 = 0x8B30;
    pub const GL_COMPILE_STATUS: u32 = 0x8B81;
    pub const GL_LINK_STATUS: u32 = 0x8B82;
    pub const GL_INFO_LOG_LENGTH: u32 = 0x8B84;
}

/// Helper to convert f32 gain to u32 bits for atomic storage
fn gain_to_bits(gain: f32) -> u32 {
    gain.to_bits()
}

/// Helper to convert u32 bits back to f32 gain
fn bits_to_gain(bits: u32) -> f32 {
    f32::from_bits(bits)
}

// ============================================================================
// Background Loading Types
// ============================================================================

/// Initialization phases for tracking loading progress
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InitPhase {
    Starting,
    Database,
    RenderPipeline,
    Audio,
    Resources,
    /// Heavy shader compilation (HarmonicGeometry) - happens last with visible progress
    Shaders,
    /// GPU warmup - render actual frames to force all shader path compilation
    /// Loading screen stays visible during this phase
    Rendering,
    Ready,
}

impl InitPhase {
    fn label(&self) -> &'static str {
        match self {
            InitPhase::Starting => "Starting up...",
            InitPhase::Database => "Opening database...",
            InitPhase::RenderPipeline => "Setting up renderer...",
            InitPhase::Audio => "Initializing audio...",
            InitPhase::Resources => "Loading resources...",
            InitPhase::Shaders => "Compiling shaders...",
            InitPhase::Rendering => "Warming up GPU...",
            InitPhase::Ready => "Ready!",
        }
    }
}

/// Loading screen state for tracking initialization progress
pub struct LoadingState {
    pub phase: InitPhase,
    pub progress: f32,  // 0.0 - 1.0
    pub messages: Vec<String>,
    pub start_time: std::time::Instant,
    /// Number of warmup render frames completed (for GPU path compilation)
    pub warmup_frames: u32,
    /// Recent render times in ms for stability detection
    pub render_times: [f32; 8],
    /// Index for circular buffer
    pub render_time_idx: usize,
    /// Number of consecutive stable frames (render time < threshold)
    pub stable_frames: u32,
    /// Whether GPU is confirmed ready (stable render times achieved)
    pub gpu_ready: bool,
}

impl LoadingState {
    fn new() -> Self {
        Self {
            phase: InitPhase::Starting,
            progress: 0.0,
            messages: Vec::new(),
            start_time: std::time::Instant::now(),
            warmup_frames: 0,
            render_times: [100.0; 8],  // Start high to indicate not ready
            render_time_idx: 0,
            stable_frames: 0,
            gpu_ready: false,
        }
    }

    /// Record a render time and check if GPU is stable
    fn record_render_time(&mut self, time_ms: f32) {
        self.render_times[self.render_time_idx] = time_ms;
        self.render_time_idx = (self.render_time_idx + 1) % 8;

        // Check if this frame was fast (< 50ms = 20+ FPS)
        const STABLE_THRESHOLD_MS: f32 = 50.0;
        if time_ms < STABLE_THRESHOLD_MS {
            self.stable_frames += 1;
        } else {
            self.stable_frames = 0;  // Reset on slow frame
        }

        // GPU is ready when we have 10+ consecutive fast frames after minimum warmup
        const REQUIRED_STABLE_FRAMES: u32 = 10;
        const MIN_WARMUP_FRAMES: u32 = 60;  // ~1 second minimum
        if self.stable_frames >= REQUIRED_STABLE_FRAMES && self.warmup_frames >= MIN_WARMUP_FRAMES {
            if !self.gpu_ready {
                log::info!("[Startup] GPU confirmed ready: {} consecutive stable frames (avg {:.1}ms)",
                    self.stable_frames, self.avg_render_time());
            }
            self.gpu_ready = true;
        }
    }

    /// Get average render time
    fn avg_render_time(&self) -> f32 {
        self.render_times.iter().sum::<f32>() / 8.0
    }

    fn add_message(&mut self, msg: impl Into<String>) {
        self.messages.push(msg.into());
        // Keep only the last 4 messages
        if self.messages.len() > 4 {
            self.messages.remove(0);
        }
    }

    fn update_progress(&mut self, phase: InitPhase, progress: f32, msg: Option<String>) {
        self.phase = phase;
        self.progress = progress;
        if let Some(m) = msg {
            self.add_message(m);
        }
    }
}

/// Updates sent from background initialization threads
pub enum InitUpdate {
    /// Progress update with phase, progress (0.0-1.0), and optional message
    Progress { phase: InitPhase, progress: f32, message: Option<String> },
    /// Database ready (None if failed to open)
    DatabaseReady(Option<Database>),
    /// Resources loaded (MIDI mappings, etc.)
    ResourcesReady {
        midi_mappings: Vec<crate::midi::MidiMapping>,
        saved_audio_source: Option<AudioSource>,
        osc_config: crate::osc::OscConfig,
        osc_enabled: bool,
        hue_config: crate::hue::HueConfig,
        wled_config: crate::wled::WledConfig,
    },
    /// Directories loaded
    DirectoriesReady {
        video_directory: Option<String>,
        recording_directory: Option<String>,
        show_setup_wizard: bool,
        show_sync_prompt: bool,
    },
    /// All background initialization complete (signal to start audio on main thread)
    BackgroundComplete,
    /// Error during initialization
    Error(String),
}

/// Silence gate state for hysteresis-based video visibility fade
#[derive(Debug, Clone, Copy, PartialEq)]
enum SilenceGateState {
    AudioActive,       // Normal — opacity easing to target
    EnteringSilence,   // Energy < enter threshold, accumulating hold timer
    Silent,            // Confirmed silence — opacity → 0
    ReturningAudio,    // Energy > return threshold, accumulating confirm timer
}

const SILENCE_ENTER_THRESHOLD: f32 = 0.0003;  // Below this → entering silence (raw FFT band sum)
const AUDIO_RETURN_THRESHOLD: f32 = 0.0005;   // Above this → returning audio (raw FFT band sum)
const AUDIO_CONFIRM_DURATION: f32 = 0.2;     // 200 ms confirm window
const GATE_COOLDOWN: f32 = 0.5;              // 500 ms anti-bounce
const FADE_OUT_RATE: f32 = 0.92;             // Per-frame multiplier
const FADE_IN_RATE: f32 = 0.12;              // Per-frame ease factor

/// Main application state
pub struct CompellerApp {
    // ========================================================================
    // Loading State (for background initialization)
    // ========================================================================

    /// Loading screen state (Some while initializing, None when ready)
    loading_state: Option<LoadingState>,

    /// Channel to receive updates from background initialization threads
    init_rx: Option<std::sync::mpsc::Receiver<InitUpdate>>,

    // ========================================================================
    // Core Subsystems
    // ========================================================================

    /// OpenGL render pipeline (projectM + video compositor) - shared for paint callback
    render_pipeline: Arc<Mutex<Option<RenderPipeline>>>,

    /// Audio capture from system/app/mic
    audio_capture: Arc<Mutex<AudioCapture>>,

    /// Audio analyzer for FFT/beat detection
    audio_analyzer: Arc<Mutex<AudioAnalyzer>>,

    /// Latest audio analysis results (shared with UI)
    audio_analysis: Arc<Mutex<AudioAnalysis>>,

    /// fundsp-based audio meters (band splitting, peak hold, RMS)
    audio_meters: Option<AudioMeters>,

    /// Latest meter values for UI display
    meter_values: MeterValues,

    /// Audio output for scrub playback
    audio_output: crate::audio::AudioOutput,

    /// Shared audio gain value (stored as bits for atomic access)
    audio_gain: Arc<AtomicU32>,

    /// UI control panel state
    control_panel: ControlPanel,

    /// Frame counter for performance monitoring
    frame_count: u64,

    /// Whether audio capture is enabled
    audio_enabled: bool,

    /// Whether the menu panel is visible (toggle with 'M' key)
    menu_visible: bool,

    /// Whether to show the landing page
    show_landing: bool,

    /// TV turn-on transition progress (0.0 = off/landing, 1.0 = fully on)
    tv_transition_progress: f32,

    /// Whether TV transition is currently active
    tv_transition_active: bool,

    /// Frames since main app became visible (for layout settling)
    frames_since_main_visible: u32,

    /// Saved menu visibility state (restored after landing transition)
    saved_menu_visible: bool,

    /// Startup TV turn-on animation progress (0.0 = black, 1.0 = landing visible)
    startup_progress: f32,

    /// Whether startup animation is active
    startup_active: bool,

    /// Video decoder for landing page background
    landing_video: VideoDecoder,

    /// Whether landing video has been initialized
    landing_video_loaded: bool,

    /// Whether the last saved preset has been loaded
    initial_preset_loaded: bool,

    /// Show first-run setup wizard for directory selection
    show_setup_wizard: bool,

    /// Show sync prompt when compels directory is empty
    show_sync_prompt: bool,

    /// Logo texture handle
    logo_texture: Option<egui::TextureHandle>,

    /// Tagline image texture
    tagline_texture: Option<egui::TextureHandle>,

    /// Landing page timer for auto-enter
    landing_start: Option<std::time::Instant>,

    /// Silence gate state machine for hysteresis-based video pause/resume
    silence_gate: SilenceGateState,

    /// TRIG-DIAG probe: last 1Hz emit time. Logs the full trigger-consumer
    /// gate chain while a manual trigger is held — measuring the runtime fact
    /// static analysis couldn't settle (triggers-without-audio).
    trig_diag_at: Option<std::time::Instant>,
    /// TRIG-DIAG probe: frames where the lighting block (send_input +
    /// dmx/wled update) actually executed. A held trigger with this counter
    /// frozen = the block is gated; advancing = the block runs and the gate
    /// is further downstream.
    trig_diag_block_frames: u64,

    /// Accumulator for silence enter hold time
    silence_enter_accumulator: f32,

    /// Accumulator for audio return confirm time
    audio_return_accumulator: f32,

    /// Cooldown timer to prevent gate bounce
    gate_cooldown_remaining: f32,

    /// Video visibility before silence fade (to restore)
    pre_silence_visibility: f32,

    /// Current effective video visibility (eased)
    effective_video_visibility: f32,

    /// Database for persistent state (Clone via internal Arc)
    db: Option<Database>,

    /// Track sources (CDJ takes priority over Apple Music when master is playing)
    track_sources: Vec<Box<dyn TrackSource>>,
    /// Ableton Link tempo listener — passive multicast clock source.
    ableton_link: AbletonLinkListener,

    /// Track announcement state for UI display
    track_announcement: TrackAnnouncement,

    /// 3-stage compel picker with embedded LLM fallback
    compel_picker: compel_picker::CompelPicker,

    /// Last known CDJ state for change detection
    last_cdj_state: Vec<(u8, String, bool, bool, bool)>, // (device_id, track_title+artist, is_master, is_playing, is_looping)

    /// Album artwork texture for track announcement
    artwork_texture: Option<egui::TextureHandle>,

    /// Last artwork data hash (to detect changes)
    last_artwork_hash: u64,

    /// PERFORMANCE: Pre-allocated buffers for lock-free audio processing in render loop
    audio_raw_buffer: Vec<f32>,
    audio_amplified_buffer: Vec<f32>,
    audio_projectm_buffer: Vec<f32>,

    /// Output manager for native window and NDI outputs
    output_manager: OutputManager,

    /// Previous frame's recording-processing state (for detecting stitch completion)
    was_recording_processing: bool,

    /// Cached IOSurface GL resources for zero-copy output (texture, FBO, last surface ID, dimensions)
    #[cfg(target_os = "macos")]
    iosurface_cache: Arc<Mutex<IOSurfaceCache>>,

    /// MIDI controller manager
    midi_controller: crate::midi::MidiController,

    /// Learning preset browser encoder: waiting for second mapping (LoadPreset)
    learning_preset_browser_step2: bool,

    /// Learning compel browser encoder: waiting for second mapping (LoadCompel)
    learning_compel_browser_step2: bool,

    /// Ring buffer for harmonic state history (freeze + scrub)
    harmonic_state_buffer: crate::render::HarmonicStateBuffer,

    /// OSC output thread (CueEngine + OSC sending on dedicated thread)
    osc_thread: crate::osc::OscThread,

    /// Hue Entertainment API thread (RGB mapping + DTLS streaming on dedicated thread)
    hue_thread: crate::hue::thread::HueThread,

    /// WLED LED strip manager for UDP streaming
    wled_manager: crate::wled::WledManager,

    /// DMX / Art-Net demo output adapter (env-enabled, disabled by default)
    dmx_manager: crate::dmx::DmxManager,

    /// Operator-only 3D virtual stage preview (Lighting-menu toggle).
    virtual_stage_view: crate::ui::VirtualStageView,

    /// Ephemeral per-fixture live mix (intensity trim / solo / mute), edited from
    /// the virtual-stage fixture panel and applied to both the preview and the
    /// real DMX. In-memory only — reset to default every launch (like Blackout).
    fixture_overrides: crate::emerge::FixtureOverrides,
    /// Last-serialized per-fixture Segment FX state — change-detector + payload for
    /// the debounced persist of the (persistent) Segment FX choices.
    segfx_last_json: String,
    /// Debounce countdown (frames) before the Segment FX choice is flushed to db.
    segfx_save_in: u32,

    /// Authentication manager for Compeller.ai login
    auth_manager: crate::auth::AuthManager,

    /// Last license blob refresh timestamp (for 24h periodic refresh)
    last_license_refresh: std::time::Instant,

    /// YouTube authentication (connection status via compeller.ai)
    youtube_auth: crate::youtube::auth::YouTubeAuth,

    /// YouTube eligibility cache (can user stream?)
    youtube_eligibility: crate::youtube::eligibility::EligibilityCache,

    /// YouTube control plane API client
    youtube_control_plane: crate::youtube::control_plane::YouTubeControlPlane,

    /// YouTube live session state machine
    youtube_session: Option<crate::youtube::session::LiveSession>,

    /// YouTube RTMP streamer (data plane)
    youtube_rtmp_streamer: Option<crate::output::GStreamerStreamer>,

    /// Twitch RTMP streamer (GStreamer for proper A/V sync)
    twitch_ffmpeg_streamer: Option<crate::output::GStreamerStreamer>,

    /// Kick RTMP streamer (GStreamer; same CBR profile as Twitch)
    kick_ffmpeg_streamer: Option<crate::output::GStreamerStreamer>,

    /// Per-output audio gain Arcs (persistent, shared with writer threads)
    youtube_audio_gain: Arc<AtomicU32>,
    twitch_audio_gain: Arc<AtomicU32>,
    kick_audio_gain: Arc<AtomicU32>,

    /// Camera tracking manager (multi-camera detection + tracking)
    camera_manager: crate::camera::CameraManager,

    /// Camera media manager (background capture for visual layers)
    camera_media_manager: crate::camera::CameraMediaManager,
}

/// Cache for IOSurface GL resources to avoid recreating every frame
#[cfg(target_os = "macos")]
struct IOSurfaceCache {
    /// GL texture ID bound to IOSurface
    texture: u32,
    /// GL FBO for blitting to IOSurface
    fbo: u32,
    /// Last bound surface ID (to detect changes)
    surface_id: u32,
    /// Cached dimensions
    width: u32,
    height: u32,
    /// Async PBO readback for NDI (zero-alloc after init)
    async_readback: AsyncReadback,

    // Bicubic upscale resources
    /// Intermediate texture to copy viewport content (source for bicubic)
    src_texture: u32,
    /// FBO for intermediate texture
    src_fbo: u32,
    /// Cached source dimensions (viewport size)
    src_width: u32,
    src_height: u32,
    /// Bicubic upscale shader program
    bicubic_program: u32,
    /// VAO for fullscreen quad
    bicubic_vao: u32,
    /// VBO for fullscreen quad vertices
    bicubic_vbo: u32,
    /// Uniform locations
    u_texture: i32,
    u_src_size: i32,
    u_dst_size: i32,
}

#[cfg(target_os = "macos")]
impl Default for IOSurfaceCache {
    fn default() -> Self {
        Self {
            texture: 0,
            fbo: 0,
            surface_id: 0,
            width: 0,
            height: 0,
            async_readback: AsyncReadback::default(),
            // Bicubic resources (0 = not initialized)
            src_texture: 0,
            src_fbo: 0,
            src_width: 0,
            src_height: 0,
            bicubic_program: 0,
            bicubic_vao: 0,
            bicubic_vbo: 0,
            u_texture: -1,
            u_src_size: -1,
            u_dst_size: -1,
        }
    }
}

#[cfg(target_os = "macos")]
impl IOSurfaceCache {
    /// Initialize bicubic upscale shader and fullscreen quad
    /// Call once when resources are needed
    unsafe fn init_bicubic(&mut self) {
        use iosurface_support::*;

        if self.bicubic_program != 0 {
            return; // Already initialized
        }

        // Bicubic vertex shader
        let vs_src = r#"#version 330 core
layout(location = 0) in vec2 aPos;
layout(location = 1) in vec2 aTexCoord;
out vec2 vTexCoord;
void main() {
    gl_Position = vec4(aPos, 0.0, 1.0);
    vTexCoord = aTexCoord;
}
"#;

        // Bicubic fragment shader with Catmull-Rom interpolation
        let fs_src = r#"#version 330 core
in vec2 vTexCoord;
out vec4 FragColor;
uniform sampler2D uTexture;
uniform vec2 uSrcSize;      // Source texture size
uniform vec2 uDstSize;      // Destination (NDI) size

// Catmull-Rom cubic weight function
vec4 cubic(float v) {
    vec4 n = vec4(1.0, 2.0, 3.0, 4.0) - v;
    vec4 s = n * n * n;
    float x = s.x;
    float y = s.y - 4.0 * s.x;
    float z = s.z - 4.0 * s.y + 6.0 * s.x;
    float w = 6.0 - x - y - z;
    return vec4(x, y, z, w) * (1.0 / 6.0);
}

vec4 textureBicubic(sampler2D sampler, vec2 texCoords, vec2 texSize) {
    vec2 invTexSize = 1.0 / texSize;
    texCoords = texCoords * texSize - 0.5;
    vec2 fxy = fract(texCoords);
    texCoords -= fxy;

    vec4 xcubic = cubic(fxy.x);
    vec4 ycubic = cubic(fxy.y);

    vec4 c = texCoords.xxyy + vec2(-0.5, 1.5).xyxy;
    vec4 s = vec4(xcubic.xz + xcubic.yw, ycubic.xz + ycubic.yw);
    vec4 offset = c + vec4(xcubic.yw, ycubic.yw) / s;
    offset *= invTexSize.xxyy;

    vec4 sample0 = texture(sampler, offset.xz);
    vec4 sample1 = texture(sampler, offset.yz);
    vec4 sample2 = texture(sampler, offset.xw);
    vec4 sample3 = texture(sampler, offset.yw);

    float sx = s.x / (s.x + s.y);
    float sy = s.z / (s.z + s.w);

    return mix(mix(sample3, sample2, sx), mix(sample1, sample0, sx), sy);
}

void main() {
    float upscale = max(uDstSize.x / uSrcSize.x, uDstSize.y / uSrcSize.y);
    if (upscale > 1.1) {
        FragColor = textureBicubic(uTexture, vTexCoord, uSrcSize);
    } else {
        FragColor = texture(uTexture, vTexCoord);
    }
}
"#;

        // Compile vertex shader
        let vs = glCreateShader(GL_VERTEX_SHADER);
        let vs_src_ptr = vs_src.as_ptr() as *const i8;
        let vs_len = vs_src.len() as i32;
        glShaderSource(vs, 1, &vs_src_ptr, &vs_len);
        glCompileShader(vs);

        let mut success: i32 = 0;
        glGetShaderiv(vs, GL_COMPILE_STATUS, &mut success);
        if success == 0 {
            let mut log_buf = [0i8; 512];
            glGetShaderInfoLog(vs, 512, std::ptr::null_mut(), log_buf.as_mut_ptr());
            log::error!("[Bicubic] Vertex shader compile error: {:?}", std::ffi::CStr::from_ptr(log_buf.as_ptr()));
            glDeleteShader(vs);
            return;
        }

        // Compile fragment shader
        let fs = glCreateShader(GL_FRAGMENT_SHADER);
        let fs_src_ptr = fs_src.as_ptr() as *const i8;
        let fs_len = fs_src.len() as i32;
        glShaderSource(fs, 1, &fs_src_ptr, &fs_len);
        glCompileShader(fs);

        glGetShaderiv(fs, GL_COMPILE_STATUS, &mut success);
        if success == 0 {
            let mut log_buf = [0i8; 512];
            glGetShaderInfoLog(fs, 512, std::ptr::null_mut(), log_buf.as_mut_ptr());
            log::error!("[Bicubic] Fragment shader compile error: {:?}", std::ffi::CStr::from_ptr(log_buf.as_ptr()));
            glDeleteShader(vs);
            glDeleteShader(fs);
            return;
        }

        // Link program
        let program = glCreateProgram();
        glAttachShader(program, vs);
        glAttachShader(program, fs);
        glLinkProgram(program);

        glGetProgramiv(program, GL_LINK_STATUS, &mut success);
        if success == 0 {
            let mut log_buf = [0i8; 512];
            glGetProgramInfoLog(program, 512, std::ptr::null_mut(), log_buf.as_mut_ptr());
            log::error!("[Bicubic] Program link error: {:?}", std::ffi::CStr::from_ptr(log_buf.as_ptr()));
            glDeleteProgram(program);
            glDeleteShader(vs);
            glDeleteShader(fs);
            return;
        }

        glDeleteShader(vs);
        glDeleteShader(fs);

        self.bicubic_program = program;

        // Get uniform locations
        self.u_texture = glGetUniformLocation(program, "uTexture\0".as_ptr() as *const i8);
        self.u_src_size = glGetUniformLocation(program, "uSrcSize\0".as_ptr() as *const i8);
        self.u_dst_size = glGetUniformLocation(program, "uDstSize\0".as_ptr() as *const i8);

        // Create fullscreen quad VAO/VBO
        // Vertex format: x, y, u, v (flip V for OpenGL)
        let vertices: [f32; 16] = [
            -1.0, -1.0,   0.0, 0.0,  // bottom-left
             1.0, -1.0,   1.0, 0.0,  // bottom-right
            -1.0,  1.0,   0.0, 1.0,  // top-left
             1.0,  1.0,   1.0, 1.0,  // top-right
        ];

        let mut vao: u32 = 0;
        let mut vbo: u32 = 0;
        glGenVertexArrays(1, &mut vao);
        glGenBuffers(1, &mut vbo);

        glBindVertexArray(vao);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        glBufferData(
            GL_ARRAY_BUFFER,
            (vertices.len() * std::mem::size_of::<f32>()) as isize,
            vertices.as_ptr() as *const std::ffi::c_void,
            GL_STATIC_DRAW,
        );

        // Position attribute (location 0)
        glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 4 * std::mem::size_of::<f32>() as i32, std::ptr::null());
        glEnableVertexAttribArray(0);

        // TexCoord attribute (location 1)
        glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, 4 * std::mem::size_of::<f32>() as i32, (2 * std::mem::size_of::<f32>()) as *const std::ffi::c_void);
        glEnableVertexAttribArray(1);

        glBindVertexArray(0);
        glBindBuffer(GL_ARRAY_BUFFER, 0);

        self.bicubic_vao = vao;
        self.bicubic_vbo = vbo;

        log::info!("[Bicubic] Initialized shader program {} VAO {} VBO {}", program, vao, vbo);
    }

    /// Ensure intermediate source texture is the right size for the viewport
    unsafe fn ensure_src_texture(&mut self, width: u32, height: u32) {
        use iosurface_support::*;

        if self.src_texture != 0 && self.src_width == width && self.src_height == height {
            return; // Already correct size
        }

        // Delete old resources if resizing
        if self.src_texture != 0 {
            glDeleteTextures(1, &self.src_texture);
            glDeleteFramebuffers(1, &self.src_fbo);
        }

        // Create texture for intermediate copy
        let mut tex: u32 = 0;
        glGenTextures(1, &mut tex);
        glBindTexture(GL_TEXTURE_2D, tex);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR as i32);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR as i32);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA as i32, width as i32, height as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());

        // Create FBO for this texture
        let mut fbo: u32 = 0;
        glGenFramebuffers(1, &mut fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);

        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        glBindTexture(GL_TEXTURE_2D, 0);

        self.src_texture = tex;
        self.src_fbo = fbo;
        self.src_width = width;
        self.src_height = height;

        log::info!("[Bicubic] Created source texture {} FBO {} ({}x{})", tex, fbo, width, height);
    }
}

/// Async PBO readback for NDI capture (triple-buffered, no per-frame allocs)
#[cfg(target_os = "macos")]
struct AsyncReadback {
    /// Triple-buffered PBOs for async readback
    pbos: [u32; 3],
    /// Fence syncs for each PBO (wrapped for Send/Sync)
    fences: [SendableSync; 3],
    /// Current write index
    write_idx: usize,
    /// Pre-allocated flip buffer (reused every frame)
    flip_buffer: Vec<u8>,
    /// Buffer size (width * height * 4)
    buffer_size: usize,
    /// Initialized flag
    initialized: bool,
}

/// Wrapper for GL sync object that's safe to send across threads
/// GL sync objects are only accessed from the GL thread via the mutex
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct SendableSync(*mut std::ffi::c_void);

#[cfg(target_os = "macos")]
unsafe impl Send for SendableSync {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SendableSync {}

#[cfg(target_os = "macos")]
impl Default for SendableSync {
    fn default() -> Self {
        Self(std::ptr::null_mut())
    }
}

#[cfg(target_os = "macos")]
impl SendableSync {
    fn get(&self) -> *mut std::ffi::c_void {
        self.0
    }
    fn set(&mut self, ptr: *mut std::ffi::c_void) {
        self.0 = ptr;
    }
    fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

#[cfg(target_os = "macos")]
impl Default for AsyncReadback {
    fn default() -> Self {
        Self {
            pbos: [0; 3],
            fences: [SendableSync::default(); 3],
            write_idx: 0,
            flip_buffer: Vec::new(),
            buffer_size: 0,
            initialized: false,
        }
    }
}

#[cfg(target_os = "macos")]
impl AsyncReadback {
    /// Initialize PBOs for the given dimensions
    fn init(&mut self, width: u32, height: u32) {
        use iosurface_support::*;

        let size = (width * height * 4) as usize;
        if self.initialized && self.buffer_size == size {
            return;
        }

        // Clean up old PBOs if resizing
        if self.initialized {
            self.cleanup();
        }

        unsafe {
            // Generate 3 PBOs
            glGenBuffers(3, self.pbos.as_mut_ptr());

            // Allocate each PBO
            for &pbo in &self.pbos {
                glBindBuffer(GL_PIXEL_PACK_BUFFER, pbo);
                glBufferData(GL_PIXEL_PACK_BUFFER, size as isize, std::ptr::null(), GL_STREAM_READ);
            }
            glBindBuffer(GL_PIXEL_PACK_BUFFER, 0);
        }

        // Pre-allocate flip buffer
        self.flip_buffer = vec![0u8; size];
        self.buffer_size = size;
        self.initialized = true;
        self.write_idx = 0;

        log::info!("[AsyncReadback] Initialized 3 PBOs of {} bytes each for {}x{}", size, width, height);
    }

    /// Start async readback to current PBO (non-blocking)
    fn start_readback(&mut self, width: u32, height: u32) {
        use iosurface_support::*;

        // Initialize or reinitialize if resolution changed
        let needed_size = (width * height * 4) as usize;
        if !self.initialized || self.buffer_size != needed_size {
            self.init(width, height);
        }

        let pbo = self.pbos[self.write_idx];

        unsafe {
            // Delete old fence if exists
            if !self.fences[self.write_idx].is_null() {
                glDeleteSync(self.fences[self.write_idx].get());
            }

            // Bind PBO and start async read
            glBindBuffer(GL_PIXEL_PACK_BUFFER, pbo);
            glReadPixels(
                0, 0,
                width as i32, height as i32,
                GL_RGBA, GL_UNSIGNED_BYTE,
                std::ptr::null_mut(), // With PBO bound, this is an offset into the PBO
            );

            // Insert fence to track completion
            self.fences[self.write_idx].set(glFenceSync(GL_SYNC_GPU_COMMANDS_COMPLETE, 0));

            glBindBuffer(GL_PIXEL_PACK_BUFFER, 0);
        }

        // Advance to next PBO
        self.write_idx = (self.write_idx + 1) % 3;
    }

    /// Try to read from oldest PBO (returns None if not ready yet)
    fn try_read(&mut self, width: u32, height: u32) -> Option<&[u8]> {
        use iosurface_support::*;

        let expected_size = (width * height * 4) as usize;
        if !self.initialized || self.buffer_size != expected_size {
            return None; // Not initialized or resolution changed
        }

        // Read from PBO that's 2 frames behind (the oldest, about to be overwritten)
        // After start_readback advances write_idx, write_idx points to the next PBO
        // we'll write to, which is the oldest data (2 frames old)
        let read_idx = self.write_idx;

        if self.fences[read_idx].is_null() {
            return None;
        }

        unsafe {
            // Check if fence is signaled (with small timeout to avoid missing frames)
            // 1ms timeout = 1_000_000 ns - enough for GPU to finish if it's close
            let result = glClientWaitSync(self.fences[read_idx].get(), GL_SYNC_FLUSH_COMMANDS_BIT, 1_000_000);

            if result != GL_ALREADY_SIGNALED && result != GL_CONDITION_SATISFIED {
                return None; // Not ready yet
            }

            // Map PBO to read pixels
            let pbo = self.pbos[read_idx];
            glBindBuffer(GL_PIXEL_PACK_BUFFER, pbo);
            let ptr = glMapBuffer(GL_PIXEL_PACK_BUFFER, GL_READ_ONLY);

            if !ptr.is_null() {
                // Fast sequential copy (no flip - NDI thread will flip during RGBA->BGRA)
                let src = std::slice::from_raw_parts(ptr as *const u8, self.buffer_size);
                self.flip_buffer.copy_from_slice(src);
                glUnmapBuffer(GL_PIXEL_PACK_BUFFER);
            }
            glBindBuffer(GL_PIXEL_PACK_BUFFER, 0);
        }

        Some(&self.flip_buffer)
    }

    /// Cleanup PBOs and fences
    fn cleanup(&mut self) {
        use iosurface_support::*;

        if !self.initialized {
            return;
        }

        unsafe {
            for fence in &self.fences {
                if !fence.is_null() {
                    glDeleteSync(fence.get());
                }
            }
            glDeleteBuffers(3, self.pbos.as_ptr());
        }

        self.pbos = [0; 3];
        self.fences = [SendableSync::default(); 3];
        self.initialized = false;
        log::info!("[AsyncReadback] Cleaned up PBOs");
    }
}

impl CompellerApp {
    fn auto_select_track_theme(
        &mut self,
        title: &str,
        artist: Option<&str>,
        video_path: Option<&str>,
        allow_random: bool,
    ) {
        // Priority: 1) theme associated to the matched compel, 2) fuzzy track/theme match, 3) random theme.
        let mut theme_paths: Vec<String> = Vec::new();
        if let Some(ref db) = self.db {
            if let Some(path) = video_path {
                theme_paths = db.get_video_presets(path);
                if !theme_paths.is_empty() {
                    log::info!(
                        "[Track] Found {} theme(s) associated with compel",
                        theme_paths.len()
                    );
                }
            }

            if theme_paths.is_empty() {
                theme_paths = db.find_presets_for_track_fuzzy(title, artist);
                if !theme_paths.is_empty() {
                    log::info!("[Track] Fuzzy theme match found for '{}'", title);
                }
            }
        }

        let mut theme_loaded = false;
        if !theme_paths.is_empty() {
            if let Some(mut pipeline_guard) = self.render_pipeline.try_lock() {
                if let Some(ref mut pipeline) = *pipeline_guard {
                    for preset_path in &theme_paths {
                        if self.control_panel.load_preset_path(pipeline, preset_path) {
                            log::info!(
                                "[Track] Auto-loaded matched theme: {}",
                                preset_path.split('/').last().unwrap_or(preset_path.as_str())
                            );
                            theme_loaded = true;
                            break;
                        }
                    }
                }
            } else {
                log::warn!("[Track] Could not lock render pipeline for matched theme load");
            }
        }

        if !theme_loaded && allow_random {
            if let Some(mut pipeline_guard) = self.render_pipeline.try_lock() {
                if let Some(ref mut pipeline) = *pipeline_guard {
                    if self.control_panel.set_random_preset(pipeline) {
                        log::info!("[Track] No matched theme found; loaded random theme");
                    } else {
                        log::warn!("[Track] No matched theme found and random theme load failed");
                    }
                }
            } else {
                log::warn!("[Track] Could not lock render pipeline for random theme fallback");
            }
        } else if !theme_loaded {
            log::info!("[Track] No matched theme found; waiting for async compel result before random fallback");
        }
    }

    /// Create a new application instance
    ///
    /// This function is designed to return as quickly as possible, showing a loading
    /// screen immediately while heavy initialization happens in background threads.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        log::info!("[Startup] CompellerApp::new() - beginning minimal initialization");

        // ====================================================================
        // PHASE 1: Immediate (blocking) - minimal setup for loading screen
        // ====================================================================

        // Load custom fonts - keep this synchronous as it's needed for loading screen text
        let mut fonts = egui::FontDefinitions::default();
        if let Some(font_path) = find_resource("public/fonts/Roboto-Black.ttf") {
            if let Ok(font_data) = std::fs::read(&font_path) {
                fonts.font_data.insert(
                    "Roboto-Black".to_owned(),
                    egui::FontData::from_owned(font_data).into(),
                );
                fonts.families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .insert(0, "Roboto-Black".to_owned());
                fonts.families.insert(
                    egui::FontFamily::Name("Bold".into()),
                    vec!["Roboto-Black".to_owned()],
                );
            }
        }
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);

        // Get the OpenGL context (needed for RenderPipeline creation on main thread)
        let gl = cc.gl.clone();

        // Create a 1x1 dummy texture immediately to prevent macOS driver warnings.
        // The driver logs "GLD_TEXTURE_INDEX_2D is unloadable" if a shader runs
        // before any valid texture is bound to unit 0.
        if let Some(ref gl) = gl {
            unsafe {
                use eframe::glow::HasContext;
                if let Ok(dummy) = gl.create_texture() {
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
                    log::debug!("[Startup] Created dummy texture to prevent driver warnings");
                }
            }
        }

        // Create channel for background thread communication
        let (init_tx, init_rx) = mpsc::channel();

        // ====================================================================
        // PHASE 2: Create core subsystems (lightweight, non-blocking)
        // ====================================================================

        // Create audio capture and analyzer - lightweight, no blocking
        let audio_capture = Arc::new(Mutex::new(AudioCapture::new()));

        // Start async audio preparation immediately (queries SCShareableContent in background)
        // This prevents blocking the main thread during audio initialization
        {
            let audio = audio_capture.lock();
            audio.prepare_async();
        }

        let audio_analyzer = Arc::new(Mutex::new(AudioAnalyzer::new()));
        let audio_analysis = Arc::new(Mutex::new(AudioAnalysis::default()));
        let audio_gain = Arc::new(AtomicU32::new(gain_to_bits(3.5)));

        // Create render pipeline - this compiles shaders but projectM is deferred
        let render_pipeline = if let Some(ref gl) = gl {
            let tx = init_tx.clone();
            let _ = tx.send(InitUpdate::Progress {
                phase: InitPhase::RenderPipeline,
                progress: 0.1,
                message: Some("Compiling shaders...".into()),
            });
            match RenderPipeline::new(gl.clone()) {
                Ok(pipeline) => {
                    log::info!("[Startup] Render pipeline created (projectM deferred)");
                    let _ = tx.send(InitUpdate::Progress {
                        phase: InitPhase::RenderPipeline,
                        progress: 0.3,
                        message: Some("Shaders ready".into()),
                    });
                    Some(pipeline)
                }
                Err(e) => {
                    log::error!("Failed to initialize render pipeline: {}", e);
                    let _ = tx.send(InitUpdate::Error(format!("Render pipeline failed: {}", e)));
                    None
                }
            }
        } else {
            log::error!("No OpenGL context available");
            None
        };
        let render_pipeline = Arc::new(Mutex::new(render_pipeline));

        // Initialize track sources
        let mut track_sources: Vec<Box<dyn TrackSource>> = Vec::new();
        #[cfg(target_os = "macos")]
        track_sources.push(Box::new(AppleMusicSource::new()));
        track_sources.push(Box::new(ProDjLinkSource::new()));

        // ====================================================================
        // PHASE 3: Spawn background threads for heavy initialization
        // ====================================================================

        // Clone what we need for the background thread
        let audio_gain_clone = audio_gain.clone();
        let init_tx_clone = init_tx.clone();

        std::thread::spawn(move || {
            log::info!("[Startup] Background init thread started");

            // --- Database Initialization ---
            let _ = init_tx_clone.send(InitUpdate::Progress {
                phase: InitPhase::Database,
                progress: 0.2,
                message: Some("Opening database...".into()),
            });

            let db = match Database::open() {
                Ok(db) => {
                    log::info!("[Startup] Database opened successfully");
                    Some(db)
                }
                Err(e) => {
                    log::error!("[DB] Failed to open database: {}", e);
                    let _ = init_tx_clone.send(InitUpdate::Error(format!("Database error: {}", e)));
                    None
                }
            };

            let _ = init_tx_clone.send(InitUpdate::DatabaseReady(db.clone()));

            // --- Load saved state and directories ---
            if let Some(ref db) = db {
                let _ = init_tx_clone.send(InitUpdate::Progress {
                    phase: InitPhase::Resources,
                    progress: 0.4,
                    message: Some("Loading saved state...".into()),
                });

                let state = db.load_state();

                // Update audio gain from saved state
                audio_gain_clone.store(gain_to_bits(state.audio_gain), Ordering::Relaxed);

                // Check directories and determine if setup wizard is needed
                let show_setup_wizard = state.video_directory.is_none() && state.recording_directory.is_none();
                let show_sync_prompt = if let Some(ref dir) = state.video_directory {
                    let video_count = if std::path::Path::new(dir).exists() {
                        std::fs::read_dir(dir)
                            .map(|entries| entries
                                .filter_map(|e| e.ok())
                                .filter(|e| {
                                    let path = e.path();
                                    path.is_file() && path.extension()
                                        .map(|ext| {
                                            let ext = ext.to_string_lossy().to_lowercase();
                                            ext == "mp4" || ext == "mov" || ext == "webm"
                                        })
                                        .unwrap_or(false)
                                })
                                .count())
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    video_count == 0
                } else {
                    false
                };

                let _ = init_tx_clone.send(InitUpdate::DirectoriesReady {
                    video_directory: state.video_directory.clone(),
                    recording_directory: state.recording_directory.clone(),
                    show_setup_wizard,
                    show_sync_prompt,
                });

                // --- Load MIDI mappings and OSC config ---
                let _ = init_tx_clone.send(InitUpdate::Progress {
                    phase: InitPhase::Resources,
                    progress: 0.6,
                    message: Some("Loading MIDI mappings...".into()),
                });

                let mapping_rows = db.get_midi_mappings();
                let midi_mappings: Vec<crate::midi::MidiMapping> = mapping_rows.iter().filter_map(|row| {
                    let msg_type = match row.msg_type.as_str() {
                        "CC" => crate::midi::MidiMsgType::ControlChange,
                        "NoteOn" => crate::midi::MidiMsgType::NoteOn,
                        "NoteOff" => crate::midi::MidiMsgType::NoteOff,
                        "PitchBend" => crate::midi::MidiMsgType::PitchBend,
                        _ => return None,
                    };
                    let target = match row.target.as_str() {
                        "AudioGain" => crate::midi::ControlTarget::AudioGain,
                        "KickSensitivity" => crate::midi::ControlTarget::KickSensitivity,
                        "ProjectMVisibility" => crate::midi::ControlTarget::ProjectMVisibility,
                        "VideoVisibility" => crate::midi::ControlTarget::VideoVisibility,
                        "VideoScale" => crate::midi::ControlTarget::VideoScale,
                        "VideoLumaKey" => crate::midi::ControlTarget::VideoLumaKey,
                        "LogoOpacity" => crate::midi::ControlTarget::LogoOpacity,
                        "LogoScale" => crate::midi::ControlTarget::LogoScale,
                        "LogoVisible" => crate::midi::ControlTarget::LogoVisible,
                        "ToggleNdi" => crate::midi::ControlTarget::ToggleNdi,
                        "ToggleNdiAudio" => crate::midi::ControlTarget::ToggleNdiAudio,
                        "ToggleOutputWindow" => crate::midi::ControlTarget::ToggleOutputWindow,
                        "ToggleOutputFullscreen" => crate::midi::ControlTarget::ToggleOutputFullscreen,
                        "ToggleRecording" => crate::midi::ControlTarget::ToggleRecording,
                        "RandomPreset" => crate::midi::ControlTarget::RandomPreset,
                        "ToggleProjectM" => crate::midi::ControlTarget::ToggleProjectM,
                        "ToggleVideo" => crate::midi::ControlTarget::ToggleVideo,
                        "NextPreset" => crate::midi::ControlTarget::NextPreset,
                        "PrevPreset" => crate::midi::ControlTarget::PrevPreset,
                        "BrowseVideoNext" => crate::midi::ControlTarget::BrowseVideoNext,
                        "BrowseVideoPrev" => crate::midi::ControlTarget::BrowseVideoPrev,
                        "LoadVideo" => crate::midi::ControlTarget::LoadVideo,
                        "BrowsePresetNext" => crate::midi::ControlTarget::BrowsePresetNext,
                        "BrowsePresetPrev" => crate::midi::ControlTarget::BrowsePresetPrev,
                        "LoadPreset" => crate::midi::ControlTarget::LoadPreset,
                        "BrowsePresetEncoder" => crate::midi::ControlTarget::BrowsePresetEncoder,
                        "BrowseVideoEncoder" => crate::midi::ControlTarget::BrowseVideoEncoder,
                        "BrowseCompelEncoder" => crate::midi::ControlTarget::BrowseCompelEncoder,
                        "LoadCompel" => crate::midi::ControlTarget::LoadCompel,
                        other if other.starts_with("CameraVisibility(") => {
                            // Parse "CameraVisibility(N)" format
                            let idx_str = other.trim_start_matches("CameraVisibility(").trim_end_matches(')');
                            if let Ok(idx) = idx_str.parse::<u32>() {
                                crate::midi::ControlTarget::CameraVisibility(idx)
                            } else {
                                return None;
                            }
                        }
                        other if other.starts_with("CameraEnable(") => {
                            // Parse "CameraEnable(N)" format
                            let idx_str = other.trim_start_matches("CameraEnable(").trim_end_matches(')');
                            if let Ok(idx) = idx_str.parse::<u32>() {
                                crate::midi::ControlTarget::CameraEnable(idx)
                            } else {
                                return None;
                            }
                        }
                        other if other.starts_with("CameraMediaVisibility(") => {
                            // Parse "CameraMediaVisibility(N)" format
                            let idx_str = other.trim_start_matches("CameraMediaVisibility(").trim_end_matches(')');
                            if let Ok(idx) = idx_str.parse::<u32>() {
                                crate::midi::ControlTarget::CameraMediaVisibility(idx)
                            } else {
                                return None;
                            }
                        }
                        other if other.starts_with("CameraMediaEnable(") => {
                            // Parse "CameraMediaEnable(N)" format
                            let idx_str = other.trim_start_matches("CameraMediaEnable(").trim_end_matches(')');
                            if let Ok(idx) = idx_str.parse::<u32>() {
                                crate::midi::ControlTarget::CameraMediaEnable(idx)
                            } else {
                                return None;
                            }
                        }
                        other if other.starts_with("CameraSolo(") => {
                            // Parse "CameraSolo(N)" format
                            let idx_str = other.trim_start_matches("CameraSolo(").trim_end_matches(')');
                            if let Ok(idx) = idx_str.parse::<u32>() {
                                crate::midi::ControlTarget::CameraSolo(idx)
                            } else {
                                return None;
                            }
                        }
                        "ToggleChromaKey" => crate::midi::ControlTarget::ToggleChromaKey,
                        "ToggleMathOverlay" => crate::midi::ControlTarget::ToggleMathOverlay,
                        "TogglePerfHud" => crate::midi::ControlTarget::TogglePerfHud,
                        "ToggleDebugHud" => crate::midi::ControlTarget::ToggleDebugHud,
                        "ToggleQsWheel" => crate::midi::ControlTarget::ToggleQsWheel,
                        "TriggerStrobe" => crate::midi::ControlTarget::TriggerStrobe,
                        "TriggerBlackout" => crate::midi::ControlTarget::TriggerBlackout,
                        "TriggerBlinder" => crate::midi::ControlTarget::TriggerBlinder,
                        "TriggerUv" => crate::midi::ControlTarget::TriggerUv,
                        "TriggerFog" => crate::midi::ControlTarget::TriggerFog,
                        "ForceBuildup" => crate::midi::ControlTarget::ForceBuildup,
                        "CameraLumaKey" => crate::midi::ControlTarget::CameraLumaKey,
                        "GlobalBrightness" => crate::midi::ControlTarget::GlobalBrightness,
                        "UvLevel" => crate::midi::ControlTarget::UvLevel,
                        "GenreTempoGuidance" => crate::midi::ControlTarget::GenreTempoGuidance,
                        "StyleAuto" => crate::midi::ControlTarget::StyleAuto,
                        "StyleChill" => crate::midi::ControlTarget::StyleChill,
                        "StyleEnergetic" => crate::midi::ControlTarget::StyleEnergetic,
                        "StyleBanger" => crate::midi::ControlTarget::StyleBanger,
                        "StyleNext" => crate::midi::ControlTarget::StyleNext,
                        "MythosInstability" => crate::midi::ControlTarget::MythosInstability,
                        "MythosFieldStrength" => crate::midi::ControlTarget::MythosFieldStrength,
                        "MythosDisturbanceSize" => crate::midi::ControlTarget::MythosDisturbanceSize,
                        "MythosHarmonicInfluence" => crate::midi::ControlTarget::MythosHarmonicInfluence,
                        "MythosDecay" => crate::midi::ControlTarget::MythosDecay,
                        _ => return None,
                    };
                    Some(crate::midi::MidiMapping {
                        key: crate::midi::MidiKey {
                            device_name: row.device_name.clone(),
                            channel: row.channel,
                            msg_type,
                            index: row.msg_index,
                        },
                        target,
                        min_value: row.min_value,
                        max_value: row.max_value,
                        inverted: row.inverted,
                    })
                }).collect();

                let saved_audio_source = db.get_audio_source().and_then(|s| parse_audio_source(&s));
                let osc_config = db.load_osc_config();
                let osc_enabled = db.is_osc_enabled();
                let hue_config = db.load_hue_config();
                let wled_config = db.load_wled_config();

                let _ = init_tx_clone.send(InitUpdate::ResourcesReady {
                    midi_mappings,
                    saved_audio_source,
                    osc_config,
                    osc_enabled,
                    hue_config,
                    wled_config,
                });
            }

            // --- Signal background work complete (audio will start on main thread) ---
            let _ = init_tx_clone.send(InitUpdate::BackgroundComplete);

            // --- Ping Twitch ingest servers (async, doesn't block startup) ---
            std::thread::spawn(|| {
                log::info!("[Startup] Pinging Twitch ingest servers...");
                crate::output::find_best_twitch_server();
                log::info!("[Startup] Twitch server ping complete");
            });

            log::info!("[Startup] Background init thread finished");
        });

        // ====================================================================
        // PHASE 4: Create app with loading state
        // ====================================================================

        log::info!("[Startup] Creating app with loading state");

        // Create OSC thread first, then Hue thread (shares CueSnapshot Arc)
        let osc_thread = crate::osc::OscThread::new();
        let hue_thread = crate::hue::thread::HueThread::new(osc_thread.snapshot_arc());

        Self {
            // Loading state - will show loading screen until ready
            loading_state: Some(LoadingState::new()),
            init_rx: Some(init_rx),

            // Core subsystems
            render_pipeline,
            audio_capture,
            audio_analyzer,
            audio_analysis,
            audio_meters: None, // Created when audio becomes ready
            meter_values: MeterValues::default(),
            audio_output: crate::audio::AudioOutput::new(),
            audio_gain,
            control_panel: ControlPanel::default(),
            frame_count: 0,
            audio_enabled: false,
            menu_visible: true,
            show_landing: false,
            tv_transition_progress: 0.0,
            tv_transition_active: false,
            frames_since_main_visible: 0,
            saved_menu_visible: true, // Will be updated from DB
            startup_progress: 0.0,
            startup_active: false, // Disabled during loading - will activate after init
            landing_video: VideoDecoder::new(),
            landing_video_loaded: false,
            initial_preset_loaded: false,
            show_setup_wizard: false,
            show_sync_prompt: false,
            logo_texture: None,
            tagline_texture: None,
            landing_start: None,
            silence_gate: SilenceGateState::AudioActive, // Start visible — gate will fade out if no audio after boot cooldown
            trig_diag_at: None,
            trig_diag_block_frames: 0,
            silence_enter_accumulator: 0.0,
            audio_return_accumulator: 0.0,
            gate_cooldown_remaining: 2.0, // Boot cooldown — ignore startup noise for 2 seconds
            pre_silence_visibility: 1.0, // Default - will be updated from DB
            effective_video_visibility: 1.0, // Start visible — matches old behavior
            db: None, // Will be set when DatabaseReady received
            track_sources,
            ableton_link: AbletonLinkListener::new(),
            track_announcement: TrackAnnouncement::new(),
            compel_picker: compel_picker::CompelPicker::with_defaults(),
            last_cdj_state: Vec::new(),
            artwork_texture: None,
            last_artwork_hash: 0,
            audio_raw_buffer: Vec::with_capacity(8192),
            audio_amplified_buffer: Vec::with_capacity(8192),
            audio_projectm_buffer: Vec::with_capacity(8192),
            output_manager: OutputManager::new(),
            was_recording_processing: false,
            #[cfg(target_os = "macos")]
            iosurface_cache: Arc::new(Mutex::new(IOSurfaceCache::default())),
            midi_controller: crate::midi::MidiController::new(),
            learning_preset_browser_step2: false,
            learning_compel_browser_step2: false,
            harmonic_state_buffer: crate::render::HarmonicStateBuffer::new(300),
            osc_thread,
            hue_thread,
            wled_manager: crate::wled::WledManager::new(),
            dmx_manager: crate::dmx::DmxManager::from_env(),
            virtual_stage_view: crate::ui::VirtualStageView::default(),
            fixture_overrides: crate::emerge::FixtureOverrides::default(),
            segfx_last_json: String::new(),
            segfx_save_in: 0,
            auth_manager: crate::auth::AuthManager::new(),
            last_license_refresh: std::time::Instant::now(),
            youtube_auth: crate::youtube::auth::YouTubeAuth::new(),
            youtube_eligibility: crate::youtube::eligibility::EligibilityCache::new(),
            youtube_control_plane: crate::youtube::control_plane::YouTubeControlPlane::new(),
            youtube_session: None,
            youtube_rtmp_streamer: None,
            twitch_ffmpeg_streamer: None,
            kick_ffmpeg_streamer: None,
            youtube_audio_gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            twitch_audio_gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            kick_audio_gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            camera_manager: crate::camera::CameraManager::new(crate::camera::TrackingConfig::default()),
            camera_media_manager: crate::camera::CameraMediaManager::new(),
        }
    }

    /// Apply an initialization update from a background thread
    fn apply_init_update(&mut self, update: InitUpdate) {
        match update {
            InitUpdate::Progress { phase, progress, message } => {
                if let Some(ref mut state) = self.loading_state {
                    state.update_progress(phase, progress, message);
                }
            }

            InitUpdate::DatabaseReady(db) => {
                log::info!("[Startup] Database ready");
                if let Some(ref db_ref) = db {
                    // Wire the DB into the DMX manager so stages cache locally
                    // (offline-first; SaaS stays authority when online).
                    self.dmx_manager.set_db(db_ref.clone());
                    // Apply any DMX backend chosen in a previous session (DMX panel),
                    // overriding the env default. No-op if nothing was persisted.
                    self.dmx_manager.restore_output_backend();
                    // Load saved state values
                    let state = db_ref.load_state();
                    self.saved_menu_visible = state.menu_visible;
                    self.pre_silence_visibility = state.video_visibility;
                    self.effective_video_visibility = state.video_visibility;

                    // Restore recording gain
                    self.output_manager.set_rec_user_gain(state.rec_audio_gain);

                    // Connect database to control panel
                    self.control_panel.set_database(db_ref.clone());
                    // Restore persisted per-fixture Segment FX choices (local, keyed
                    // by fixture id; orphans for removed fixtures are harmless).
                    if let Some(json) = db_ref.get_setting("fixture_segment_fx") {
                        self.fixture_overrides.load_segfx_json(&json);
                    }
                    self.segfx_last_json = self.fixture_overrides.segfx_json();
                    // COM-499 Phase B4: push the persisted LFO bank to the OSC runtime.
                    self.osc_thread.set_lfo_bank(self.control_panel.lfo_bank());

                    // Configure compel picker LLM model path
                    // Search bundled llm/instruct/ first, then fallback to config dir
                    let model_files = [
                        "qwen2.5-0.5b-instruct-q4_k_m.gguf",
                        "Llama-3.2-1B-Instruct-Q4_K_M.gguf",
                    ];
                    let mut model_found = false;
                    // Priority 1: Bundled llm/instruct/ (works in dev and .app bundle via find_resource)
                    for model_file in &model_files {
                        let relative = format!("llm/instruct/{}", model_file);
                        if let Some(model_path) = find_resource(&relative) {
                            log::info!("[CompelPicker] Found bundled LLM model: {:?}", model_path);
                            self.compel_picker.set_model_path(model_path);
                            self.compel_picker.load_model_async();
                            model_found = true;
                            break;
                        }
                    }
                    // Priority 2: User config directory ~/.compeller-react/models/
                    if !model_found {
                        if let Some(config_dir) = dirs::config_dir() {
                            let model_dir = config_dir.join("compeller-react").join("models");
                            for model_file in &model_files {
                                let model_path = model_dir.join(model_file);
                                if model_path.exists() {
                                    log::info!("[CompelPicker] Found LLM model in config: {:?}", model_path);
                                    self.compel_picker.set_model_path(model_path);
                                    self.compel_picker.load_model_async();
                                    model_found = true;
                                    break;
                                }
                            }
                        }
                    }
                    if !model_found {
                        log::info!("[CompelPicker] No LLM model found in bundled or config paths, using heuristic fallback");
                    }

                    // Connect database to output manager for segmented recording
                    self.output_manager.set_database(db_ref.clone());

                    // Clean up stale 'active' sessions from previous crashes
                    if let Err(e) = db_ref.cleanup_stale_active_sessions() {
                        log::warn!("[Startup] Failed to cleanup stale sessions: {}", e);
                    }

                    // Check for crashed recording sessions that can be recovered
                    let recoveries = self.output_manager.check_recording_recovery();
                    if !recoveries.is_empty() {
                        log::info!(
                            "[Startup] Found {} crashed recording session(s) with recoverable segments",
                            recoveries.len()
                        );
                        for recovery in &recoveries {
                            log::info!(
                                "  - Session {}: {} segments, {}ms recovered (max loss: {}ms)",
                                recovery.session_id,
                                recovery.committed_segments.len(),
                                recovery.recovered_duration_ms,
                                recovery.max_loss_ms
                            );
                        }
                        // TODO: Show recovery prompt in UI
                    }

                    // Restore auth state from database
                    self.auth_manager.restore_from_db(db_ref);
                    // Mirror admin status so a restored admin session can edit shared
                    // stages from the first frame (before any stage-list refresh).
                    let admin = self.auth_manager.is_admin();
                    self.dmx_manager.set_admin(admin);

                    // Validate stored token with server (async - won't block startup)
                    // If token is invalid/expired, auth state will be cleared
                    self.auth_manager.validate_token_async(db_ref.clone());

                    // Sync YouTube components with Compeller session token
                    let token = self.auth_manager.get_token();
                    self.youtube_auth.set_compeller_token(token.clone());
                    self.youtube_eligibility.set_compeller_token(token.clone());
                    self.youtube_control_plane.set_compeller_token(token);

                    // Check YouTube connection status if logged in
                    if self.auth_manager.is_authenticated() {
                        self.youtube_auth.check_status();
                    }

                    // Create upload manager for syncing recordings to Compeller.ai
                    let upload_manager = crate::set_sync::SetUploadManager::new(
                        std::sync::Arc::new(db_ref.clone()),
                        self.auth_manager.clone(),
                    );

                    // Validate local sync status against SaaS first (if authenticated and online)
                    // This will reset "completed" entries that don't exist in SaaS, and delete
                    // entries for files that no longer exist
                    match upload_manager.validate_sync_status() {
                        Ok(count) if count > 0 => log::info!("[Startup] Fixed {} uploads with invalid sync status", count),
                        Err(e) => log::debug!("[Startup] Sync validation skipped: {}", e),
                        _ => {}
                    }

                    // Clean up any remaining orphaned upload entries (files that no longer exist on disk)
                    match db_ref.cleanup_orphaned_uploads() {
                        Ok(count) if count > 0 => log::info!("[Startup] Cleaned up {} orphaned upload entries", count),
                        Err(e) => log::warn!("[Startup] Failed to cleanup orphaned uploads: {}", e),
                        _ => {}
                    }

                    self.control_panel.set_upload_manager(upload_manager);
                    log::info!("[Startup] Set upload manager initialized");

                    // Initialize compel video sync (authenticated user's compels → compels/ directory)
                    if self.auth_manager.is_authenticated() {
                        if let Some(ref video_dir) = self.control_panel.get_video_directory() {
                            let compel_sync = crate::compel_sync::CompelSync::new(
                                std::path::PathBuf::from(video_dir),
                            );

                            // Reset interrupted downloads from previous session
                            let _ = db_ref.reset_interrupted_compel_downloads();

                            // Refresh list and start sync in background
                            if let Some(ref token) = self.auth_manager.get_token() {
                                compel_sync.refresh_list(db_ref.clone(), token.clone());
                            }

                            self.control_panel.set_compel_sync(compel_sync);
                            log::info!("[Startup] Compel video sync initialized");
                        }
                    }

                    // Load saved Twitch stream key
                    if let Some(key) = db_ref.get_twitch_stream_key() {
                        self.control_panel.set_twitch_stream_key(key);
                    }

                    // Load saved Twitch stream info
                    if let Some(channel) = db_ref.get_twitch_channel() {
                        self.control_panel.set_twitch_channel(channel);
                    }
                    if let Some(title) = db_ref.get_twitch_stream_title() {
                        self.control_panel.set_twitch_stream_title(title);
                    }
                    if let Some(category) = db_ref.get_twitch_stream_category() {
                        self.control_panel.set_twitch_stream_category(category);
                    }
                    if let Some(tags) = db_ref.get_twitch_stream_tags() {
                        self.control_panel.set_twitch_stream_tags(tags);
                    }

                    // Load saved Kick stream settings
                    if let Some(url) = db_ref.get_kick_stream_url() {
                        self.control_panel.set_kick_stream_url(url);
                    }
                    if let Some(key) = db_ref.get_kick_stream_key() {
                        self.control_panel.set_kick_stream_key(key);
                    }
                    if let Some(title) = db_ref.get_kick_stream_title() {
                        self.control_panel.set_kick_stream_title(title);
                    }
                }
                self.db = db;
            }

            InitUpdate::DirectoriesReady { video_directory, recording_directory, show_setup_wizard, show_sync_prompt } => {
                log::info!("[Startup] Directories ready - wizard: {}, sync: {}", show_setup_wizard, show_sync_prompt);
                self.show_setup_wizard = show_setup_wizard;
                self.show_sync_prompt = show_sync_prompt;

                if let Some(ref dir) = video_directory {
                    self.control_panel.set_video_directory(dir.clone());
                }
                if let Some(ref dir) = recording_directory {
                    self.control_panel.set_recording_directory(dir.clone());
                }

                // Video is not auto-loaded on startup — user selects one or
                // a track announcement triggers it.
            }

            InitUpdate::ResourcesReady { midi_mappings, saved_audio_source, osc_config, osc_enabled, hue_config, wled_config } => {
                log::info!("[Startup] Resources ready - {} MIDI mappings", midi_mappings.len());

                // Apply saved audio source
                if let Some(source) = saved_audio_source {
                    log::info!("[Startup] Restoring audio source: {:?}", source);
                    // Set calibration and assist cap based on source type.
                    let (cal, cap, vol_boost) = match &source {
                        AudioSource::InputDevice { device_name, .. } => {
                            let profile = crate::audio::get_device_profile(device_name);
                            (profile.recording_boost, 4.0, 1.0)
                        }
                        AudioSource::Microphone => (1.5, 4.0, 1.0),
                        _ => (3.0, 8.0, 1.0), // Writer thread handles gain via cal×assist×user + limiter
                    };
                    self.output_manager.set_ndi_calibration(cal, cap);
                    self.output_manager.set_rec_calibration(cal, cap);
                    self.output_manager.set_rec_audio_volume_boost(vol_boost);
                    let mut audio = self.audio_capture.lock();
                    audio.set_source(source);
                }

                // Load MIDI mappings
                self.midi_controller.load_mappings(midi_mappings);
                self.midi_controller.connect_all();

                // Lighting requires a Compeller sign-in (enforced at the
                // configure surface + here at auto-restore — never the
                // per-frame feed). Signed out at boot: configs still load so
                // nothing is lost, but outputs stay OFF until sign-in.
                // restore_from_db ran above in this handler, so a previously
                // signed-in session counts even before token validation
                // completes (a slow auth endpoint must not dark a rig).
                let lighting_signed_in = self.auth_manager.is_authenticated();
                if !lighting_signed_in && (osc_enabled || hue_config.enabled || wled_config.enabled) {
                    log::info!("[Startup] Lighting outputs NOT restored — sign-in required");
                }

                // Apply OSC config (thread owns the manager; we keep a main-thread config copy)
                self.osc_thread.set_config(osc_config);
                if osc_enabled && lighting_signed_in {
                    self.osc_thread.set_enabled(true);
                }

                // Apply Hue config via thread command (thread handles archetype fetch internally)
                let hue_bridge_ip = hue_config.bridge_ip.clone();
                let mut hue_config = hue_config;
                if !lighting_signed_in {
                    hue_config.enabled = false;
                }
                let hue_enabled = hue_config.enabled;
                self.hue_thread.send_command(crate::hue::thread::HueCommand::LoadConfig(hue_config));
                log::info!("[Startup] Hue config loaded (enabled={}, bridge={})",
                    hue_enabled, hue_bridge_ip);

                // Apply WLED config (restore strips, modes from previous session)
                let wled_enabled = wled_config.enabled && lighting_signed_in;
                let wled_strip_count = wled_config.strips.len();
                self.wled_manager.set_config(wled_config);
                if wled_enabled {
                    self.wled_manager.set_enabled(true);
                }
                log::info!("[Startup] WLED config loaded (enabled={}, strips={})",
                    wled_enabled, wled_strip_count);
            }

            InitUpdate::BackgroundComplete => {
                log::info!("[Startup] Background work complete - checking audio preparation");

                // Check if audio async preparation is complete
                let audio_prepared = self.audio_capture.lock().is_prepared();

                if audio_prepared {
                    // Audio content is cached - start immediately (non-blocking)
                    log::info!("[Startup] Audio prepared - starting capture");
                    if let Some(ref mut state) = self.loading_state {
                        state.update_progress(InitPhase::Audio, 0.9, Some("Starting audio capture...".into()));
                    }

                    self.start_audio();

                    if let Some(ref mut state) = self.loading_state {
                        state.update_progress(InitPhase::Ready, 1.0, Some("Ready!".into()));
                    }
                } else {
                    // Still preparing - show message and let check_init_complete handle it
                    log::info!("[Startup] Audio still preparing - waiting...");
                    if let Some(ref mut state) = self.loading_state {
                        state.update_progress(InitPhase::Audio, 0.8, Some("Initializing audio...".into()));
                    }
                }
            }

            InitUpdate::Error(msg) => {
                log::error!("[Startup] Initialization error: {}", msg);
                if let Some(ref mut state) = self.loading_state {
                    state.add_message(format!("Error: {}", msg));
                }
            }
        }
    }

    /// Check if initialization is complete and transition to main app
    fn check_init_complete(&mut self) {
        // Check audio preparation status outside of the loading_state borrow
        let audio_prepared = self.audio_capture.lock().is_prepared();

        // Determine if we should start audio (prepared but not started yet)
        let should_start_audio = audio_prepared && !self.audio_enabled && self.loading_state.is_some();

        // Start audio before entering the loading_state borrow scope
        if should_start_audio {
            log::info!("[Startup] Audio prepared - starting capture in check_init_complete");
            self.start_audio();
        }

        if let Some(ref mut state) = self.loading_state {
            // Update progress after starting audio (but don't jump to Ready yet)
            if should_start_audio && state.phase != InitPhase::Shaders {
                state.update_progress(InitPhase::Resources, 0.8, Some("Audio ready".into()));
            }

            // === SHADER COMPILATION PHASE (Frame-Distributed) ===
            // Each frame, execute ONE step of shader initialization
            // This keeps the loading animation smooth instead of freezing
            if state.phase == InitPhase::Shaders {
                // Execute one init step
                let (is_complete, step_desc, step_progress) = {
                    if let Some(ref mut pipeline) = *self.render_pipeline.lock() {
                        pipeline.init_harmonic_geometry_step()
                    } else {
                        (true, "Ready", 1.0)
                    }
                };

                // Update progress display with current step
                let overall_progress = 0.85 + step_progress * 0.10; // Shaders are 85-95%
                state.progress = overall_progress;
                state.add_message(step_desc.to_string());

                if is_complete {
                    // Shader compilation complete - transition to GPU warmup phase
                    log::info!("[Startup] Shader compilation complete - entering GPU warmup phase");

                    if let Some(ref mut pipeline) = *self.render_pipeline.lock() {
                        pipeline.set_render_mode(crate::render::RenderMode::HarmonicGeometry);
                    }
                    self.control_panel.set_render_mode(crate::render::RenderMode::HarmonicGeometry);
                    self.control_panel.set_current_preset("Geometric Field".to_string(), "Hero's Journey".to_string());

                    // Enter rendering warmup phase (don't exit loading yet!)
                    state.phase = InitPhase::Rendering;
                    state.warmup_frames = 0;
                }
                return;
            }

            // === GPU WARMUP PHASE ===
            // Render actual frames while loading screen is visible to force GPU compilation
            // Wait for GPU to be truly ready (stable render times) before showing app
            if state.phase == InitPhase::Rendering {
                const MAX_WARMUP_FRAMES: u32 = 300; // Max ~5 seconds at 60fps (safety limit)

                state.warmup_frames += 1;

                // Progress based on stable frames or frame count
                let stability_progress = state.stable_frames as f32 / 10.0;  // Need 10 stable frames
                let frame_progress = (state.warmup_frames as f32 / 60.0).min(1.0);  // Min 60 frames
                state.progress = 0.95 + (stability_progress.max(frame_progress) * 0.05).min(0.05);

                // Initialize projectM early (first warmup frame) to prevent lag when menu opens
                if state.warmup_frames == 1 {
                    if let Some(ref mut pipeline) = *self.render_pipeline.lock() {
                        state.add_message("Scanning presets...");
                        pipeline.ensure_projectm_initialized();
                    }
                }

                // Update message during warmup
                if state.warmup_frames == 20 {
                    state.add_message("Warming up GPU...");
                }

                // Do an actual render pass and measure time
                let render_start = std::time::Instant::now();
                if let Some(ref mut pipeline) = *self.render_pipeline.lock() {
                    // Create render params for warmup - use defaults but ensure all shader paths are exercised
                    let mut warmup_params = RenderParams::default();
                    warmup_params.video_visibility = 0.5; // Force video shader path compilation
                    warmup_params.logo_visible = false; // Don't need logo during warmup
                    warmup_params.bloom_intensity = 0.5; // Force bloom shader compilation
                    warmup_params.vignette_intensity = 0.3; // Force vignette shader
                    warmup_params.video_luma_key = 0.3; // Force luma key shader path

                    // Cycle through render modes to warm up ALL shader paths
                    // Frame 1-40: HarmonicGeometry (primary mode), Frame 41-60: ProjectM, then stay on user's mode
                    if state.warmup_frames == 1 {
                        state.add_message("Compiling geometry shaders...");
                        pipeline.set_render_mode(crate::render::RenderMode::HarmonicGeometry);
                    } else if state.warmup_frames == 41 {
                        state.add_message("Compiling preset shaders...");
                        pipeline.set_render_mode(crate::render::RenderMode::ProjectM);
                    } else if state.warmup_frames == 61 {
                        // Switch back to HarmonicGeometry for stability check
                        pipeline.set_render_mode(crate::render::RenderMode::HarmonicGeometry);
                    }

                    // Render to force GPU compilation (result not displayed - loading screen covers it)
                    let _ = pipeline.render_frame(1920, 1080, &warmup_params);
                }
                let render_time_ms = render_start.elapsed().as_secs_f32() * 1000.0;

                // Record render time and check for stability
                state.record_render_time(render_time_ms);

                // Log slow frames during warmup
                if render_time_ms > 100.0 {
                    log::info!("[Warmup] Frame {} took {:.1}ms (shader compilation)", state.warmup_frames, render_time_ms);
                }

                // Exit warmup when GPU is confirmed ready OR max frames reached
                let ready = state.gpu_ready || state.warmup_frames >= MAX_WARMUP_FRAMES;
                if ready {
                    let reason = if state.gpu_ready { "GPU stable" } else { "max frames" };
                    log::info!("[Startup] GPU warmup complete: {} ({} frames, avg {:.1}ms) - transitioning to main app",
                        reason, state.warmup_frames, state.avg_render_time());

                    // Clear loading state and start TV turn-on animation
                    self.loading_state = None;
                    self.init_rx = None;
                    self.startup_active = true;
                    self.startup_progress = 0.0;
                }
                return;
            }

            // === PRE-SHADER CHECKS (wait for all resources) ===
            // Check all readiness conditions before entering shader phase

            // 1. Audio must be enabled
            if !self.audio_enabled {
                if !audio_prepared {
                    state.add_message("Initializing audio...");
                } else {
                    state.add_message("Waiting for audio...");
                }
                return;
            }

            // 2. If not in setup wizard, video should be loaded (or timed out)
            let min_load_time = std::time::Duration::from_millis(500);
            let elapsed = state.start_time.elapsed();

            if !self.show_setup_wizard && !self.control_panel.has_video() {
                // Give video some time to load, but don't wait forever
                if elapsed < std::time::Duration::from_secs(3) {
                    if elapsed > min_load_time {
                        state.add_message("Loading video...");
                    }
                    return;
                }
                // Timeout - proceed without video
                log::warn!("[Startup] Video load timeout - proceeding without video");
            }

            // 3. Ensure minimum loading time for smooth UX
            if elapsed < min_load_time {
                return;
            }

            // === ALL RESOURCES READY - Enter shader compilation phase ===
            // This shows "Compiling shaders..." and returns. Next frame will do the actual compilation.
            log::info!("[Startup] All resources ready (elapsed: {:?}) - entering shader phase", elapsed);
            state.update_progress(InitPhase::Shaders, 0.95, Some("Compiling shaders...".into()));
        }
    }

    /// Render the loading screen
    fn render_loading_screen(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();

        // Load logo texture on first call
        if self.logo_texture.is_none() {
            if let Some(logo_path) = find_resource("public/images/compeller_logo.png") {
                if let Ok(image_data) = std::fs::read(&logo_path) {
                    if let Ok(image) = image::load_from_memory(&image_data) {
                        let rgba = image.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let pixels = rgba.into_raw();
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                        self.logo_texture = Some(ctx.load_texture(
                            "compeller_logo",
                            color_image,
                            egui::TextureOptions::LINEAR,
                        ));
                        log::info!("[Loading] Loaded logo from {:?}", logo_path);
                    }
                }
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(10, 10, 15)))
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());

                ui.vertical_centered(|ui| {
                    // Center content vertically
                    ui.add_space(screen.height() / 3.0);

                    // Display logo image (or fallback text if not loaded)
                    if let Some(ref logo) = self.logo_texture {
                        let logo_height = 160.0;  // Doubled from 80.0
                        let aspect = logo.aspect_ratio();
                        let logo_width = logo_height * aspect;
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(logo_width, logo_height)));
                    } else {
                        // Fallback text if logo not loaded
                        ui.label(egui::RichText::new("REACT")
                            .size(48.0)
                            .color(egui::Color32::WHITE)
                            .family(egui::FontFamily::Name("Bold".into())));
                        ui.label(egui::RichText::new("by Compeller.ai")
                            .size(16.0)
                            .color(egui::Color32::from_rgb(150, 150, 160)));
                    }

                    ui.add_space(40.0);

                    // Progress bar
                    let progress = self.loading_state.as_ref().map(|s| s.progress).unwrap_or(0.0);
                    let bar_width = 300.0;
                    let bar_height = 4.0;

                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(bar_width, bar_height),
                        egui::Sense::hover(),
                    );

                    // Background
                    ui.painter().rect_filled(
                        rect,
                        2.0,
                        egui::Color32::from_rgb(40, 40, 50),
                    );

                    // Progress fill with gradient
                    let fill_rect = egui::Rect::from_min_size(
                        rect.min,
                        egui::vec2(rect.width() * progress, rect.height()),
                    );
                    ui.painter().rect_filled(
                        fill_rect,
                        2.0,
                        egui::Color32::from_rgb(100, 140, 255),
                    );

                    ui.add_space(20.0);

                    // Current phase label
                    if let Some(ref state) = self.loading_state {
                        ui.label(egui::RichText::new(state.phase.label())
                            .size(14.0)
                            .color(egui::Color32::from_rgb(120, 120, 140)));

                        // Recent messages
                        ui.add_space(10.0);
                        for msg in &state.messages {
                            ui.label(egui::RichText::new(msg)
                                .size(12.0)
                                .color(egui::Color32::from_rgb(80, 80, 100)));
                        }
                    }

                    ui.add_space(20.0);

                    // Animated spinner
                    ui.add(egui::Spinner::new().size(24.0));
                });
            });

        // Keep animating
        ctx.request_repaint();
    }

    /// Start audio capture
    pub fn start_audio(&mut self) {
        if !self.audio_enabled {
            log::info!("Starting audio capture...");
            let mut audio = self.audio_capture.lock();

            match audio.start() {
                Ok(()) => {
                    self.audio_enabled = true;
                    log::info!("Audio capture started");

                    // Set the source sample rate for NDI resampling
                    let sample_rate = audio.sample_rate;
                    self.output_manager.set_ndi_source_sample_rate(sample_rate);

                    // Create audio meters with the meters consumer
                    if let Some(meters_consumer) = audio.take_meters_consumer() {
                        log::info!("[AudioMeters] Got meters consumer, creating AudioMeters...");
                        self.audio_meters = Some(AudioMeters::new(meters_consumer));
                        log::info!("[AudioMeters] Started fundsp-based metering");
                    } else {
                        log::warn!("[AudioMeters] No meters consumer available!");
                    }

                    // Take output consumers and start independent drain threads
                    let (ndi_consumer, rec_consumer, rtmp_consumer) = audio.take_output_consumers();
                    if let (Some(ndi_c), Some(rec_c), Some(rtmp_c)) = (ndi_consumer, rec_consumer, rtmp_consumer) {
                        self.output_manager.start_audio_drains(ndi_c, rec_c, rtmp_c);
                    } else {
                        log::warn!("[Audio] Missing output consumers — drain threads not started");
                    }

                    // Test mode: auto-start NDI if AUTO_NDI=1 env var is set
                    if std::env::var("AUTO_NDI").map_or(false, |v| v == "1") {
                        log::info!("[Test] AUTO_NDI=1 detected, starting NDI output");
                        match self.output_manager.toggle_ndi_output() {
                            Ok(true) => log::info!("[Test] NDI output auto-started"),
                            Ok(false) => log::warn!("[Test] NDI was already running?"),
                            Err(e) => log::error!("[Test] Failed to auto-start NDI: {}", e),
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to start audio capture: {}", e);
                }
            }
        }
    }

    /// Stop audio capture
    pub fn stop_audio(&mut self) {
        if self.audio_enabled {
            log::info!("Stopping audio capture...");

            // Stop drain threads first (they hold consumers from the AudioBus)
            self.output_manager.stop_audio_drains();

            let mut audio = self.audio_capture.lock();
            audio.stop();
            self.audio_enabled = false;

            // Stop audio meters
            if let Some(ref mut meters) = self.audio_meters {
                meters.stop();
            }
            self.audio_meters = None;
        }
    }

    /// Apply a MIDI control value to the corresponding target
    /// Set the lighting style from a MIDI binding: same path as the
    /// trigger-row dropdown's UpdateConfig (set_config + persist). The engine
    /// picks the new posture up on the next per-frame style push; the UI
    /// dropdown reads the config field, so it follows automatically.
    fn apply_lighting_style(&mut self, style: crate::emerge::LightingStyle) {
        let mut config = self.osc_thread.config().clone();
        if config.lighting_style == style {
            return;
        }
        config.lighting_style = style;
        self.osc_thread.set_config(config.clone());
        if let Some(ref db) = self.db {
            let _ = db.save_osc_config(&config);
        }
        log::info!("[STYLE] MIDI set lighting style -> {}", style.as_str());
    }

    fn apply_midi_value(&mut self, target: crate::midi::ControlTarget, value: f32) {
        use crate::midi::ControlTarget;

        match target {
            ControlTarget::AudioGain => {
                // Logarithmic scale 0-1 to 0.1-128.0 range (matches slider)
                // gain = min * (max/min)^value = 0.1 * 1280^value
                let gain = 0.1 * (1280.0_f32).powf(value);
                self.audio_gain.store(gain_to_bits(gain), Ordering::Relaxed);
                self.control_panel.set_audio_gain(gain);
            }
            ControlTarget::KickSensitivity => {
                // Scale 0-1 to 0.5-5.0 range
                let sensitivity = 0.5 + value * 4.5;
                self.control_panel.set_kick_sensitivity(sensitivity);
            }
            ControlTarget::ProjectMVisibility => {
                self.control_panel.set_projectm_visibility(value);
            }
            ControlTarget::VideoVisibility => {
                log::info!("[MIDI] VideoVisibility value={:.3}", value);
                self.control_panel.set_video_visibility(value);
            }
            ControlTarget::VideoScale => {
                // Scale 0-1 to 0.05-4.0 range
                let scale = 0.05 + value * 3.95;
                self.control_panel.set_video_scale(scale);
            }
            ControlTarget::VideoLumaKey => {
                self.control_panel.set_video_luma_key(value);
            }
            ControlTarget::LogoOpacity => {
                self.control_panel.set_logo_opacity(value);
            }
            ControlTarget::LogoScale => {
                // Scale 0-1 to 0.1-2.0 range
                let scale = 0.1 + value * 1.9;
                self.control_panel.set_logo_scale(scale);
            }
            ControlTarget::LogoVisible => {
                if value > 0.5 {
                    self.control_panel.toggle_logo_visible();
                }
            }
            ControlTarget::ToggleNdi => {
                if value > 0.5 {
                    let _ = self.output_manager.toggle_ndi_output();
                }
            }
            ControlTarget::ToggleNdiAudio => {
                if value > 0.5 {
                    let enabled = !self.output_manager.is_ndi_audio_enabled();
                    self.output_manager.set_ndi_audio_enabled(enabled);
                }
            }
            ControlTarget::ToggleOutputWindow => {
                if value > 0.5 {
                    let _ = self.output_manager.toggle_native_window();
                }
            }
            ControlTarget::ToggleOutputFullscreen => {
                if value > 0.5 {
                    let fs = !self.output_manager.is_native_fullscreen();
                    self.output_manager.set_native_fullscreen(fs);
                }
            }
            ControlTarget::ToggleRecording => {
                if value > 0.5 {
                    if self.output_manager.is_recording() {
                        let _ = self.output_manager.stop_recording();
                    } else {
                        // Start recording with auto-generated path
                        if let Some(dir) = self.control_panel.get_recording_directory() {
                            let name = format!("react_{}.mp4", chrono::Local::now().format("%Y%m%d_%H%M%S"));
                            let path = std::path::PathBuf::from(dir).join(&name);
                            let _ = self.output_manager.start_recording(path);
                        }
                    }
                }
            }
            ControlTarget::RandomPreset => {
                // Trigger on value > 0.5 (button press)
                if value > 0.5 {
                    if let Some(mut pipeline) = self.render_pipeline.try_lock() {
                        if let Some(ref mut p) = *pipeline {
                            self.control_panel.set_random_preset(p);
                        }
                    }
                }
            }
            ControlTarget::ToggleProjectM => {
                if value > 0.5 {
                    let current = self.control_panel.params().projectm_visibility;
                    let new_val = if current > 0.5 { 0.0 } else { 1.0 };
                    self.control_panel.set_projectm_visibility(new_val);
                }
            }
            ControlTarget::ToggleVideo => {
                if value > 0.5 {
                    let current = self.control_panel.params().video_visibility;
                    let new_val = if current > 0.5 { 0.0 } else { 1.0 };
                    self.control_panel.set_video_visibility(new_val);
                }
            }
            ControlTarget::NextPreset => {
                if value > 0.5 {
                    if let Some(mut pipeline) = self.render_pipeline.try_lock() {
                        if let Some(ref mut p) = *pipeline {
                            self.control_panel.navigate_next_preset(p);
                        }
                    }
                }
            }
            ControlTarget::PrevPreset => {
                if value > 0.5 {
                    if let Some(mut pipeline) = self.render_pipeline.try_lock() {
                        if let Some(ref mut p) = *pipeline {
                            self.control_panel.navigate_prev_preset(p);
                        }
                    }
                }
            }
            ControlTarget::BrowseVideoNext => {
                if value > 0.5 {
                    self.control_panel.browse_video_next();
                }
            }
            ControlTarget::BrowseVideoPrev => {
                if value > 0.5 {
                    self.control_panel.browse_video_prev();
                }
            }
            ControlTarget::LoadVideo => {
                if value > 0.5 {
                    self.control_panel.load_browsed_video();
                }
            }
            ControlTarget::BrowsePresetNext => {
                // Trigger on any positive value (encoders may send 1-127)
                log::info!("[MIDI] BrowsePresetNext triggered, value={}", value);
                if value > 0.0 {
                    if let Some(mut pipeline) = self.render_pipeline.try_lock() {
                        if let Some(ref mut p) = *pipeline {
                            self.control_panel.browse_preset_next(p);
                        }
                    }
                }
            }
            ControlTarget::BrowsePresetPrev => {
                log::info!("[MIDI] BrowsePresetPrev triggered, value={}", value);
                if value > 0.0 {
                    if let Some(mut pipeline) = self.render_pipeline.try_lock() {
                        if let Some(ref mut p) = *pipeline {
                            self.control_panel.browse_preset_prev(p);
                        }
                    }
                }
            }
            ControlTarget::LoadPreset => {
                log::info!("[MIDI] LoadPreset triggered, value={}", value);
                if value > 0.5 {
                    if let Some(mut pipeline) = self.render_pipeline.try_lock() {
                        if let Some(ref mut p) = *pipeline {
                            self.control_panel.load_browsed_preset(p);
                        }
                    }
                }
            }
            ControlTarget::BrowsePresetEncoder => {
                // Encoder sends relative values: 1-63 = right/next, 65-127 = left/prev
                // Normalized: 0.0-0.49 = left, 0.51-1.0 = right
                log::info!("[MIDI] BrowsePresetEncoder, value={}", value);
                if let Some(mut pipeline) = self.render_pipeline.try_lock() {
                    if let Some(ref mut p) = *pipeline {
                        if value > 0.5 {
                            self.control_panel.browse_preset_next(p);
                        } else if value > 0.0 && value < 0.5 {
                            self.control_panel.browse_preset_prev(p);
                        }
                    }
                }
            }
            ControlTarget::BrowseVideoEncoder => {
                log::info!("[MIDI] BrowseVideoEncoder, value={}", value);
                if value > 0.5 {
                    self.control_panel.browse_video_next();
                } else if value > 0.0 && value < 0.5 {
                    self.control_panel.browse_video_prev();
                }
            }
            ControlTarget::BrowseCompelEncoder => {
                // Encoder sends relative values: 1-63 = right/next, 65-127 = left/prev
                log::info!("[MIDI] BrowseCompelEncoder, value={}", value);
                if value > 0.5 {
                    self.control_panel.browse_compel_next();
                } else if value > 0.0 && value < 0.5 {
                    self.control_panel.browse_compel_prev();
                }
            }
            ControlTarget::LoadCompel => {
                log::info!("[MIDI] LoadCompel triggered, value={}", value);
                if value > 0.5 {
                    self.control_panel.load_browsed_compel();
                }
            }
            ControlTarget::CameraVisibility(camera_index) => {
                // value is 0.0-1.0, convert to CC (0-127) for the MIDI pickup logic
                let cc_value = (value * 127.0) as u8;
                self.control_panel.set_camera_visibility_midi(camera_index, cc_value);
            }
            ControlTarget::CameraEnable(camera_index) => {
                // Trigger toggle on button press (value > 0.5)
                if value > 0.5 {
                    self.control_panel.toggle_camera_enable(camera_index);
                }
            }
            ControlTarget::CameraMediaVisibility(camera_index) => {
                // value is 0.0-1.0, convert to CC (0-127) for the MIDI pickup logic
                let cc_value = (value * 127.0) as u8;
                self.control_panel.set_camera_media_visibility_midi(camera_index, cc_value);
            }
            ControlTarget::CameraMediaEnable(camera_index) => {
                // Trigger toggle on button press (value > 0.5)
                if value > 0.5 {
                    self.control_panel.toggle_camera_media_enable(camera_index);
                }
            }
            ControlTarget::CameraSolo(camera_index) => {
                // Momentary solo: hold = solo this camera, release = restore
                if value > 0.5 {
                    self.control_panel.solo_camera(camera_index);
                } else {
                    self.control_panel.unsolo_camera();
                }
            }
            ControlTarget::ToggleChromaKey => {
                if value > 0.5 {
                    self.control_panel.toggle_chroma_key();
                }
            }
            ControlTarget::GenreTempoGuidance => {
                if value > 0.5 {
                    self.control_panel.toggle_genre_tempo_guidance();
                }
            }
            ControlTarget::StyleAuto => {
                if value > 0.5 {
                    self.apply_lighting_style(crate::emerge::LightingStyle::Auto);
                }
            }
            ControlTarget::StyleChill => {
                if value > 0.5 {
                    self.apply_lighting_style(crate::emerge::LightingStyle::Chill);
                }
            }
            ControlTarget::StyleEnergetic => {
                if value > 0.5 {
                    self.apply_lighting_style(crate::emerge::LightingStyle::Energetic);
                }
            }
            ControlTarget::StyleBanger => {
                if value > 0.5 {
                    self.apply_lighting_style(crate::emerge::LightingStyle::Banger);
                }
            }
            ControlTarget::StyleNext => {
                if value > 0.5 {
                    let next = self.osc_thread.config().lighting_style.next();
                    self.apply_lighting_style(next);
                }
            }
            ControlTarget::ToggleMathOverlay => {
                if value > 0.5 {
                    self.control_panel.toggle_math_overlay();
                }
            }
            ControlTarget::TogglePerfHud => {
                if value > 0.5 {
                    self.control_panel.toggle_perf_hud();
                }
            }
            ControlTarget::ToggleDebugHud => {
                if value > 0.5 {
                    self.control_panel.toggle_debug_hud();
                }
            }
            ControlTarget::ToggleQsWheel => {
                if value > 0.5 {
                    self.control_panel.toggle_qs_wheel();
                }
            }
            // Manual lighting triggers (hold-to-activate)
            // value > 0.5 = press, value <= 0.5 = release
            ControlTarget::TriggerStrobe => {
                if value > 0.5 {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerPress(crate::emerge::TriggerButton::Strobe));
                } else {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerRelease(crate::emerge::TriggerButton::Strobe));
                }
            }
            ControlTarget::TriggerBlackout => {
                if value > 0.5 {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerPress(crate::emerge::TriggerButton::Blackout));
                } else {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerRelease(crate::emerge::TriggerButton::Blackout));
                }
            }
            ControlTarget::TriggerBlinder => {
                if value > 0.5 {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerPress(crate::emerge::TriggerButton::Blinder));
                } else {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerRelease(crate::emerge::TriggerButton::Blinder));
                }
            }
            ControlTarget::TriggerUv => {
                if value > 0.5 {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerPress(crate::emerge::TriggerButton::Uv));
                } else {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerRelease(crate::emerge::TriggerButton::Uv));
                }
            }
            ControlTarget::TriggerFog => {
                if value > 0.5 {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerPress(crate::emerge::TriggerButton::Fog));
                } else {
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerRelease(crate::emerge::TriggerButton::Fog));
                }
            }
            ControlTarget::ForceBuildup => {
                if value > 0.5 {
                    log::info!("[MIDI] ForceBuildup triggered");
                    self.osc_thread.send_command(crate::osc::OscThreadCommand::ForceBuildup);
                }
            }
            ControlTarget::CameraLumaKey => {
                self.control_panel.params_mut().camera_luma_key = value;
            }
            ControlTarget::GlobalBrightness => {
                // Update global brightness in OSC config (0.0-1.0), keeps main-thread copy in sync
                self.osc_thread.set_global_brightness(value);
            }
            ControlTarget::UvLevel => {
                // Update operator UV accent level (0.0-1.0), keeps main-thread copy in sync
                self.osc_thread.set_uv_level(value);
            }
            ControlTarget::MythosInstability => {
                self.control_panel.set_mythos_instability(value);
            }
            ControlTarget::MythosFieldStrength => {
                self.control_panel.set_mythos_field_strength(value);
            }
            ControlTarget::MythosDisturbanceSize => {
                self.control_panel.set_mythos_disturbance_size(value);
            }
            ControlTarget::MythosHarmonicInfluence => {
                self.control_panel.set_mythos_harmonic_influence(value);
            }
            ControlTarget::MythosDecay => {
                self.control_panel.set_mythos_decay(value);
            }
        }
    }

    /// Load the last saved preset from database
    fn load_initial_preset(&mut self) {
        // Get the saved preset from control panel (loaded in set_database)
        if let Some((category, preset)) = self.control_panel.get_current_preset() {
            log::info!("[Startup] Loading last preset: {} / {}", category, preset);

            if let Some(ref mut pipeline) = *self.render_pipeline.lock() {
                // Set render mode based on category
                if category == "LED Wall" {
                    self.initial_preset_loaded = true;
                    pipeline.set_render_mode(crate::render::RenderMode::LedWall);
                    self.control_panel.set_render_mode(crate::render::RenderMode::LedWall);
                    log::info!("[Startup] Activated LED Wall mode: {}", preset);
                } else if category == "Geometric Field" {
                    self.initial_preset_loaded = true;
                    pipeline.set_render_mode(crate::render::RenderMode::HarmonicGeometry);
                    self.control_panel.set_render_mode(crate::render::RenderMode::HarmonicGeometry);
                    log::info!("[Startup] Activated Geometric Field mode: {}", preset);
                } else {
                    // ProjectM mode - check if presets are ready
                    if !pipeline.presets_ready() {
                        // Presets still loading in background - defer and retry next frame
                        log::info!("[Startup] Presets still loading, deferring initial preset load");
                        return; // Don't set initial_preset_loaded, will retry
                    }
                    self.initial_preset_loaded = true;
                    pipeline.set_render_mode(crate::render::RenderMode::ProjectM);
                    self.control_panel.set_render_mode(crate::render::RenderMode::ProjectM);
                    if let Err(e) = pipeline.load_preset_by_name(&category, &preset) {
                        log::error!("[Startup] Failed to load preset: {}", e);
                    } else {
                        log::info!("[Startup] Loaded preset successfully");
                    }
                }
            }
        } else {
            self.initial_preset_loaded = true;
            log::info!("[Startup] No saved preset found");
        }
    }

    /// Render the first-run setup wizard
    fn render_setup_wizard(&mut self, ctx: &egui::Context) {
        if !self.show_setup_wizard {
            return;
        }

        // Dimmed background
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("setup_bg"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
                );
            });

        // Setup wizard window
        egui::Window::new("Welcome to REACT")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(450.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("🎬").size(48.0));
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Choose Output Folder").size(20.0).strong());
                    ui.add_space(8.0);
                    ui.label("Select where REACT should store videos and recordings.");
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Two subfolders will be created:")
                        .color(egui::Color32::GRAY));
                    ui.label(egui::RichText::new("• compels/ - synced video content")
                        .color(egui::Color32::GRAY));
                    ui.label(egui::RichText::new("• recordings/ - your recorded output")
                        .color(egui::Color32::GRAY));
                });

                ui.add_space(16.0);

                // Default path suggestion
                #[cfg(target_os = "macos")]
                let default_path = dirs::video_dir()
                    .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
                    .join("Compeller");
                #[cfg(not(target_os = "macos"))]
                let default_path = dirs::video_dir()
                    .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
                    .join("Compeller");

                ui.horizontal(|ui| {
                    ui.label("Suggested:");
                    ui.label(egui::RichText::new(default_path.display().to_string())
                        .color(egui::Color32::from_rgb(100, 200, 255))
                        .code());
                });

                ui.add_space(16.0);

                ui.horizontal(|ui| {
                    // Use default button
                    if ui.button("📁 Use Suggested Folder").clicked() {
                        self.setup_output_directory(default_path.clone());
                    }

                    ui.add_space(8.0);

                    // Choose custom button
                    if ui.button("📂 Choose Different...").clicked() {
                        let start_dir = default_path.parent().unwrap_or(&default_path);
                        if let Some(path) = rfd::FileDialog::new()
                            .set_directory(start_dir)
                            .pick_folder()
                        {
                            self.setup_output_directory(path);
                        }
                    }
                });

                ui.add_space(8.0);

                // Skip button (smaller, less prominent)
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() - 80.0);
                    if ui.small_button("Skip for now").clicked() {
                        self.show_setup_wizard = false;
                    }
                });
            });
    }

    /// Setup output directory with compels and recordings subdirectories
    fn setup_output_directory(&mut self, base_path: std::path::PathBuf) {
        let compels_path = base_path.join("compels");
        let recordings_path = base_path.join("recordings");

        // Create directories
        if let Err(e) = std::fs::create_dir_all(&compels_path) {
            log::error!("[Setup] Failed to create compels directory: {}", e);
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&recordings_path) {
            log::error!("[Setup] Failed to create recordings directory: {}", e);
            return;
        }

        log::info!("[Setup] Created output directories at {:?}", base_path);

        // Save to database
        if let Some(ref db) = self.db {
            let compels_str = compels_path.display().to_string();
            let recordings_str = recordings_path.display().to_string();

            if let Err(e) = db.set_setting("video_directory", &compels_str) {
                log::error!("[Setup] Failed to save video_directory: {}", e);
            }
            if let Err(e) = db.set_setting("recording_directory", &recordings_str) {
                log::error!("[Setup] Failed to save recording_directory: {}", e);
            }

            // Update control panel
            self.control_panel.set_video_directory(compels_str.clone());
            self.control_panel.set_recording_directory(recordings_str.clone());

            log::info!("[Setup] Saved directories - compels: {}, recordings: {}", compels_str, recordings_str);
        }

        // Close wizard
        self.show_setup_wizard = false;

        // Auto-start compel sync if authenticated
        if self.auth_manager.is_authenticated() {
            if let Some(ref db) = self.db {
                let compels_str = compels_path.display().to_string();
                let compel_sync = crate::compel_sync::CompelSync::new(
                    std::path::PathBuf::from(&compels_str),
                );

                let _ = db.reset_interrupted_compel_downloads();

                if let Some(ref token) = self.auth_manager.get_token() {
                    compel_sync.refresh_list(db.clone(), token.clone());
                }

                self.control_panel.set_compel_sync(compel_sync);
                self.show_sync_prompt = false;
                log::info!("[Setup] Auto-started compel sync after folder setup");
            }
        }
    }

    /// Render the sync prompt when compels directory is empty
    fn render_sync_prompt(&mut self, ctx: &egui::Context) -> bool {
        if !self.show_sync_prompt {
            return false;
        }

        let mut start_sync = false;

        // Dimmed background
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("sync_prompt_bg"))
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let rect = screen;
                ui.painter().rect_filled(rect, 0.0, egui::Color32::from_black_alpha(180));
            });

        // Centered dialog
        egui::Window::new("🎬 Welcome to REACT")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(400.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("☁️").size(48.0));
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Sync Your Compels").size(20.0).strong());
                    ui.add_space(8.0);
                    ui.label("Your compels folder is empty.");
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Sync now to download video content from Compeller.ai")
                        .color(egui::Color32::GRAY));
                });

                ui.add_space(16.0);

                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 180.0) / 2.0);

                    if ui.button("☁️ Sync Now").clicked() {
                        start_sync = true;
                        self.show_sync_prompt = false;
                    }

                    ui.add_space(8.0);

                    if ui.small_button("Later").clicked() {
                        self.show_sync_prompt = false;
                    }
                });

                ui.add_space(8.0);
            });

        start_sync
    }

    /// Process audio samples from lock-free ring buffer
    ///
    /// PERFORMANCE: Called from render loop - drains samples without blocking audio thread.
    /// This replaces the old callback-based approach that had mutex contention.
    fn process_audio_samples(&mut self) {
        if !self.audio_enabled {
            return;
        }

        // Clear reusable buffers (doesn't deallocate!)
        self.audio_raw_buffer.clear();
        self.audio_amplified_buffer.clear();
        self.audio_projectm_buffer.clear();

        // Drain samples from lock-free ring buffer
        let sample_count = {
            let mut audio = self.audio_capture.lock();
            audio.drain_samples(&mut self.audio_raw_buffer)
        };

        if sample_count == 0 {
            return;
        }

        // Get current gain - this directly controls projectM audio sensitivity
        let gain = bits_to_gain(self.audio_gain.load(Ordering::Relaxed));

        // Check if frozen for scrub audio playback
        let is_frozen = self.control_panel.is_frozen();

        if is_frozen {
            // Start audio output if not already running
            if !self.audio_output.is_active() {
                if let Err(e) = self.audio_output.start() {
                    log::warn!("[AudioOutput] Failed to start scrub playback: {}", e);
                }
            }

            // FROZEN: Play scrub audio instead of live audio
            // Resize buffer to match incoming sample count
            self.audio_amplified_buffer.resize(self.audio_raw_buffer.len(), 0.0);
            self.harmonic_state_buffer.get_scrub_audio(&mut self.audio_amplified_buffer);

            // Apply gain and fill projectM buffer
            for &s in &self.audio_amplified_buffer {
                let amp = (s * gain).clamp(-1.0, 1.0);
                self.audio_projectm_buffer.push(amp);
            }

            // Push scrub audio to local speakers
            self.audio_output.push_samples(&self.audio_amplified_buffer);

            // Push video frames to streamers (audio now handled by drain threads)
            if let Some(ref streamer) = self.youtube_rtmp_streamer {
                streamer.push_video();
                streamer.drain_direct_audio();
            }
            if let Some(ref streamer) = self.twitch_ffmpeg_streamer {
                streamer.push_video();
                streamer.drain_direct_audio();
            }
            if let Some(ref streamer) = self.kick_ffmpeg_streamer {
                streamer.push_video();
                streamer.drain_direct_audio();
            }
        } else {
            // Stop audio output when not frozen
            if self.audio_output.is_active() {
                self.audio_output.stop();
            }
            // LIVE: Normal audio processing

            // Single pass: compute both amplified and projectM samples
            // ProjectM gets gain applied directly (no extra hardcoded boost)
            for &s in &self.audio_raw_buffer {
                let amp = (s * gain).clamp(-1.0, 1.0);
                self.audio_amplified_buffer.push(amp);
                self.audio_projectm_buffer.push(amp);
            }

            // Push audio to freeze buffer for scrub playback
            self.harmonic_state_buffer.push_audio(&self.audio_raw_buffer);

            // Audio to NDI/recorder/RTMP now handled by independent drain threads (20ms cadence)

            // Push video frames to streamers
            if let Some(ref streamer) = self.youtube_rtmp_streamer {
                streamer.push_video();
                streamer.drain_direct_audio();
            }
            if let Some(ref streamer) = self.twitch_ffmpeg_streamer {
                streamer.push_video();
                streamer.drain_direct_audio();
            }
            if let Some(ref streamer) = self.kick_ffmpeg_streamer {
                streamer.push_video();
                streamer.drain_direct_audio();
            }
        }

        // Feed boosted audio to projectM for visualization
        if let Some(ref pipe) = *self.render_pipeline.lock() {
            pipe.add_audio_samples(&self.audio_projectm_buffer);
        }

        // Analyze for UI meters
        let analysis = self.audio_analyzer.lock().analyze(&self.audio_amplified_buffer);
        *self.audio_analysis.lock() = analysis;
    }

    /// Set audio source - stops and restarts capture to apply new source
    pub fn set_audio_source(&mut self, source: AudioSource) {
        let was_enabled = self.audio_enabled;

        // Stop capture if running
        if was_enabled {
            self.stop_audio();
        }

        // Set calibration and assist cap based on source type.
        // Level Assist will automatically compute the assist gain within the cap.
        let (cal, cap, vol_boost) = match &source {
            AudioSource::InputDevice { device_name, .. } => {
                let profile = crate::audio::get_device_profile(device_name);
                (profile.recording_boost, 4.0, 1.0)
            }
            AudioSource::Microphone => (1.5, 4.0, 1.0),
            _ => (3.0, 8.0, 1.0), // Writer thread handles gain via cal×assist×user + limiter
        };
        self.output_manager.set_ndi_calibration(cal, cap);
        self.output_manager.set_rec_calibration(cal, cap);
        self.output_manager.set_rec_audio_volume_boost(vol_boost);

        // Save to database for persistence
        if let Some(ref db) = self.db {
            let source_str = serialize_audio_source(&source);
            if let Err(e) = db.set_audio_source(&source_str) {
                log::warn!("[Audio] Failed to save audio source: {}", e);
            } else {
                log::info!("[Audio] Saved audio source: {}", source_str);
            }
        }

        // Set the new source
        {
            let mut audio = self.audio_capture.lock();
            audio.set_source(source);
        }

        // Restart capture if it was running
        if was_enabled {
            self.start_audio();
        }
    }

    /// Render the OpenGL visualization
    fn render_visualization(&mut self, ui: &mut egui::Ui, width: f32, height: f32) {
        let size = egui::vec2(width, height);
        let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());

        // Note: request_repaint is called at end of update() - no need to call here

        if rect.width() < 10.0 || rect.height() < 10.0 {
            return;
        }

        // Get render params and apply silence-based visibility
        let mut params = self.control_panel.get_params();
        params.video_visibility = self.effective_video_visibility;
        // Same SilenceGate that fades the compels also fades the projectM theme,
        // so the visuals and the video darken together when the audio stops.
        params.theme_silent = self.silence_gate == SilenceGateState::Silent;

        // Pass track announcement data to render params for GPU overlay
        if self.track_announcement.is_visible() {
            if let Some(track) = self.track_announcement.current_track() {
                params.track_overlay.visible = true;
                params.track_overlay.title = track.title.clone();
                params.track_overlay.artist = track.artist.clone();
                params.track_overlay.opacity = self.track_announcement.opacity();
                params.track_overlay.animation_progress = self.track_announcement.animation_progress();
            }
        }

        // Pass preset browser overlay data to render params
        {
            let mut pipeline_guard = self.render_pipeline.lock();
            if let Some(ref mut pipeline) = *pipeline_guard {
                if let Some((display_name, file_name, category, current_idx, total, opacity)) = self.control_panel.get_browse_overlay_state(pipeline) {
                    params.preset_browser_overlay.visible = true;
                    params.preset_browser_overlay.preset_name = display_name;
                    params.preset_browser_overlay.file_name = file_name;
                    params.preset_browser_overlay.category = category;
                    params.preset_browser_overlay.current_index = current_idx;
                    params.preset_browser_overlay.total_count = total;
                    params.preset_browser_overlay.opacity = opacity;
                }
                // Sync Chroma Wheel (symmetry HUD) visibility from control panel
                pipeline.set_symmetry_hud_visible(self.control_panel.is_chroma_hud_visible());
                // Sync QS Wheel visibility from control panel
                pipeline.set_qs_wheel_visible(self.control_panel.show_qs_wheel());
                pipeline.set_hide_qs_numbers(self.control_panel.hide_qs_numbers());
                pipeline.set_qs_dominant_only(self.control_panel.qs_dominant_only());
                // Apply license entitlements (render_scale cap, watermark)
                pipeline.apply_entitlements(&self.auth_manager.get_entitlements());
                // Sync quality tier from control panel
                pipeline.set_quality_tier(self.control_panel.quality_tier);
                // Sync AAA ring settings from control panel
                pipeline.set_ring_style_aaa(self.control_panel.ring_style_aaa);
                pipeline.set_ring_flow_lines(self.control_panel.ring_flow_lines);
                pipeline.set_ring_sparkle(self.control_panel.ring_sparkle);
                pipeline.set_ring_fog(self.control_panel.ring_fog);
                pipeline.set_ring_bloom_mask(self.control_panel.ring_bloom_mask);
                // Sync BG quality settings from control panel
                pipeline.set_bg_quality(self.control_panel.bg_quality);
                pipeline.set_bg_quality_auto(self.control_panel.bg_quality_auto);
            }
        }

        // Update camera media smoothing and captures
        let dt = 1.0 / 60.0; // Approximate frame time
        self.control_panel.update_camera_media_smoothing(dt);
        let camera_media_states = self.control_panel.get_camera_media_states();
        // Ensure camera media manager knows stable device IDs
        self.camera_media_manager.set_available_cameras(self.camera_manager.available_cameras());
        self.camera_media_manager.update_captures(camera_media_states);

        // Get camera frames and alpha values for the render callback
        let camera_frames = self.camera_media_manager.get_all_frames();
        let camera_alphas: Vec<(u32, f32)> = camera_media_states
            .iter()
            .filter_map(|(idx, state)| {
                if state.should_decode() && state.vis_smoothed > 0.001 {
                    Some((*idx, state.vis_smoothed))
                } else {
                    None
                }
            })
            .collect();

        // Collect all known camera indices so we can zero stale alphas in the pipeline
        let all_camera_indices: Vec<u32> = camera_media_states.keys().cloned().collect();

        // Throttled diagnostic logging (every 60 frames = ~1 second)
        static DIAG_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let diag_count = DIAG_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if diag_count % 60 == 0 && !camera_media_states.is_empty() {
            let states_info: Vec<_> = camera_media_states
                .iter()
                .map(|(idx, s)| format!("{}:en={},vis={},sm={:.3},st={:?}",
                    idx, s.enabled, s.vis_q, s.vis_smoothed, s.decode_state))
                .collect();
            let capturing: Vec<_> = self.camera_media_manager.capturing_cameras();
            log::info!("[CamApp] states={:?}, capturing={:?}, frames={}, alphas={:?}",
                states_info, capturing, camera_frames.len(), camera_alphas);
        }

        let pipeline = self.render_pipeline.clone();

        // Check if output window needs frame capture
        let capture_frame = self.output_manager.needs_frame_capture()
            || self.youtube_rtmp_streamer.is_some()
            || self.twitch_ffmpeg_streamer.is_some()
            || self.kick_ffmpeg_streamer.is_some();
        let ndi_active = self.output_manager.has_ndi_output();
        // Twitch and Kick both cap at 1080p; YouTube/NDI may want 4K.
        let rtmp_1080_only = (self.twitch_ffmpeg_streamer.is_some() || self.kick_ffmpeg_streamer.is_some())
            && !ndi_active
            && self.youtube_rtmp_streamer.is_none();

        // Get output resolution - use 1080p for Twitch/Kick-only streaming (no point rendering at 4K
        // just to downscale back to 1080p), otherwise use NDI resolution setting
        let (output_width, output_height) = if rtmp_1080_only {
            (1920u32, 1080u32)  // Twitch/Kick are always 1080p, skip unnecessary upscale
        } else {
            self.control_panel.ndi_resolution().dimensions()
        };

        // Get frame ring for NDI/recording pixel capture (lock-free)
        // Note: capture_frame already includes NDI, recording, and output window
        let frame_ring = if capture_frame {
            Some(self.output_manager.get_frame_ring())
        } else {
            None
        };

        // Get IOSurface ID for zero-copy output (macOS only)
        #[cfg(target_os = "macos")]
        let (surface_id, frame_number_arc) = if capture_frame {
            // Use selected resolution for output surface
            let sid = self.output_manager.ensure_surface_size(output_width, output_height);
            let fna = self.output_manager.get_frame_number_arc();
            (sid, Some(fna))
        } else {
            (None, None)
        };

        #[cfg(target_os = "macos")]
        let iosurface_cache = self.iosurface_cache.clone();

        // Whether to capture the live frame for the virtual stage back-wall.
        let vstage_on = self.control_panel.virtual_stage_enabled();

        // Check if pipeline exists
        let has_pipeline = pipeline.lock().is_some();

        if has_pipeline {
            // Use a paint callback to render with OpenGL
            let callback = egui::PaintCallback {
                rect,
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                    let gl = painter.gl();

                    // Lock the pipeline and render
                    if let Some(ref mut pipe) = *pipeline.lock() {
                        unsafe {
                            // Get viewport info from the callback - this is in physical pixels
                            let viewport = info.viewport_in_pixels();
                            let vp_width = viewport.width_px as u32;
                            let vp_height = viewport.height_px as u32;

                            if vp_width > 0 && vp_height > 0 {
                                gl.viewport(
                                    viewport.left_px as i32,
                                    viewport.from_bottom_px as i32,
                                    vp_width as i32,
                                    vp_height as i32,
                                );

                                // Upload camera frames for background compositing
                                for (cam_idx, frame) in &camera_frames {
                                    pipe.upload_camera_frame(*cam_idx, &frame.data, frame.width, frame.height);
                                }

                                // Update camera alpha values for compositing
                                // Zero all first to clear stale values, then set active ones
                                for cam_idx in &all_camera_indices {
                                    pipe.update_camera_media_alpha(*cam_idx, 0.0);
                                }
                                for (cam_idx, alpha) in &camera_alphas {
                                    pipe.update_camera_media_alpha(*cam_idx, *alpha);
                                }

                                // Render at output resolution when capture is active for crisp big-screen output
                                // Otherwise render at viewport size for efficiency
                                let (render_w, render_h) = if capture_frame {
                                    // Use output resolution (HD or 4K) for maximum quality
                                    (output_width.max(vp_width), output_height.max(vp_height))
                                } else {
                                    (vp_width, vp_height)
                                };

                                if let Err(e) = pipe.render_frame(render_w, render_h, &params) {
                                    log::error!("Render error: {}", e);
                                }

                                // Capture the live frame for the virtual stage back-wall.
                                // Only while the stage is on — zero cost otherwise.
                                if vstage_on {
                                    pipe.capture_vstage_screen(
                                        viewport.left_px as i32,
                                        viewport.from_bottom_px as i32,
                                        viewport.width_px as i32,
                                        viewport.height_px as i32,
                                    );
                                }

                                // Zero-copy IOSurface blit for output window (macOS)
                                #[cfg(target_os = "macos")]
                                if let Some(sid) = surface_id {
                                    use crate::app::iosurface_support::*;

                                    let mut cache = iosurface_cache.lock();

                                    // Check if we need to (re)bind the IOSurface (new surface or resolution changed)
                                    if cache.surface_id != sid || cache.width != output_width || cache.height != output_height {
                                        // Get IOSurface by ID
                                        let io_surface = IOSurfaceLookup(sid);
                                        if !io_surface.is_null() {
                                            // Create texture if needed
                                            if cache.texture == 0 {
                                                let mut tex: u32 = 0;
                                                glGenTextures(1, &mut tex);
                                                cache.texture = tex;
                                                log::info!("[IOSurface] Created output texture {}", tex);
                                            }

                                            // Bind IOSurface to texture (dimensions MUST match IOSurface)
                                            glBindTexture(GL_TEXTURE_RECTANGLE, cache.texture);
                                            let cgl_ctx = CGLGetCurrentContext();
                                            let result = CGLTexImageIOSurface2D(
                                                cgl_ctx,
                                                GL_TEXTURE_RECTANGLE,
                                                GL_RGBA,
                                                output_width as i32,
                                                output_height as i32,
                                                GL_BGRA,
                                                GL_UNSIGNED_INT_8_8_8_8_REV,
                                                io_surface,
                                                0,
                                            );

                                            if result != 0 {
                                                log::error!("[IOSurface] CGLTexImageIOSurface2D failed: {}", result);
                                            }

                                            // Create FBO if needed
                                            if cache.fbo == 0 {
                                                let mut fbo: u32 = 0;
                                                glGenFramebuffers(1, &mut fbo);
                                                cache.fbo = fbo;
                                                log::info!("[IOSurface] Created output FBO {}", fbo);
                                            }

                                            // Attach texture to FBO
                                            glBindFramebuffer(GL_FRAMEBUFFER, cache.fbo);
                                            glFramebufferTexture2D(
                                                GL_FRAMEBUFFER,
                                                GL_COLOR_ATTACHMENT0,
                                                GL_TEXTURE_RECTANGLE,
                                                cache.texture,
                                                0,
                                            );

                                            cache.surface_id = sid;
                                            cache.width = output_width;
                                            cache.height = output_height;

                                            CFRelease(io_surface);
                                            log::info!("[IOSurface] Bound surface {} to texture {} ({}x{})", sid, cache.texture, output_width, output_height);
                                        }
                                    }

                                    // Bicubic upscale from default framebuffer to IOSurface FBO
                                    // IMPORTANT: Preserve aspect ratio by cropping source, not stretching
                                    if cache.fbo != 0 {
                                        // Disable scissor test (egui might have set it)
                                        glDisable(GL_SCISSOR_TEST);

                                        // Use the smaller of render and viewport dimensions as source —
                                        // the default framebuffer is bounded by the window, so reading
                                        // beyond viewport size produces black/garbage (half-screen bug).
                                        // The bicubic shader handles upscaling from any source size.
                                        let src_width = render_w.min(vp_width);
                                        let src_height = render_h.min(vp_height);

                                        // Calculate source rectangle to preserve aspect ratio (fill with crop)
                                        let src_aspect = src_width as f32 / src_height as f32;
                                        let dst_aspect = output_width as f32 / output_height as f32;

                                        let (src_x, src_y, src_w, src_h) = if src_aspect > dst_aspect {
                                            // Source is wider - crop left/right
                                            let crop_width = (src_height as f32 * dst_aspect) as i32;
                                            let crop_x = ((src_width as i32 - crop_width) / 2) as i32;
                                            (crop_x, 0, crop_width, src_height as i32)
                                        } else {
                                            // Source is taller - crop top/bottom
                                            let crop_height = (src_width as f32 / dst_aspect) as i32;
                                            let crop_y = ((src_height as i32 - crop_height) / 2) as i32;
                                            (0, crop_y, src_width as i32, crop_height)
                                        };

                                        // Initialize bicubic resources if needed
                                        cache.init_bicubic();

                                        // Ensure intermediate source texture matches cropped dimensions
                                        cache.ensure_src_texture(src_w as u32, src_h as u32);

                                        // Step 1: Blit aspect-corrected region to intermediate texture (1:1 copy)
                                        glBindFramebuffer(GL_READ_FRAMEBUFFER, 0);
                                        glBindFramebuffer(GL_DRAW_FRAMEBUFFER, cache.src_fbo);
                                        glBlitFramebuffer(
                                            viewport.left_px as i32 + src_x,
                                            viewport.from_bottom_px as i32 + src_y,
                                            viewport.left_px as i32 + src_x + src_w,
                                            viewport.from_bottom_px as i32 + src_y + src_h,
                                            0, 0,
                                            src_w, src_h,
                                            GL_COLOR_BUFFER_BIT,
                                            GL_NEAREST,  // 1:1 copy, no filtering needed
                                        );

                                        // Step 2: Bicubic upscale from intermediate texture to IOSurface FBO
                                        glBindFramebuffer(GL_FRAMEBUFFER, cache.fbo);
                                        glViewport(0, 0, output_width as i32, output_height as i32);

                                        glUseProgram(cache.bicubic_program);

                                        // Bind source texture
                                        glActiveTexture(GL_TEXTURE0);
                                        glBindTexture(GL_TEXTURE_2D, cache.src_texture);
                                        glUniform1i(cache.u_texture, 0);

                                        // Set uniforms
                                        glUniform2f(cache.u_src_size, src_w as f32, src_h as f32);
                                        glUniform2f(cache.u_dst_size, output_width as f32, output_height as f32);

                                        // Draw fullscreen quad
                                        glBindVertexArray(cache.bicubic_vao);
                                        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
                                        glBindVertexArray(0);

                                        glUseProgram(0);
                                        glBindTexture(GL_TEXTURE_2D, 0);

                                        // Async PBO readback for NDI (non-blocking, lock-free)
                                        if let Some(ref ring) = frame_ring {
                                            // Bind FBO as READ framebuffer
                                            glBindFramebuffer(GL_READ_FRAMEBUFFER, cache.fbo);

                                            // Start async readback to next PBO (non-blocking)
                                            cache.async_readback.start_readback(output_width, output_height);

                                            // Try to read from oldest PBO (2 frames behind, should be ready)
                                            if let Some(pixels) = cache.async_readback.try_read(output_width, output_height) {
                                                // Write to lock-free frame ring
                                                ring.write(pixels, output_width, output_height);
                                            }
                                        }

                                        // Restore default framebuffer
                                        glBindFramebuffer(GL_FRAMEBUFFER, 0);

                                        // Flush GPU commands to IOSurface
                                        // Note: IOSurface handles cross-context sync; glFlush ensures
                                        // commands are submitted. The atomic frame_number provides
                                        // memory ordering for the reader.
                                        glFlush();

                                        // Signal frame ready after blit completes
                                        if let Some(ref fna) = frame_number_arc {
                                            fna.fetch_add(1, std::sync::atomic::Ordering::Release);
                                        }
                                    }
                                }
                            }
                        }
                    }
                })),
            };
            ui.painter().add(callback);
        } else {
            // No render pipeline - show placeholder
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 30));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Initializing render pipeline...",
                egui::FontId::proportional(24.0),
                egui::Color32::from_rgb(100, 100, 120),
            );
        }
    }

}

impl eframe::App for CompellerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame_count += 1;

        // ====================================================================
        // Check for initialization updates from background thread
        // ====================================================================
        // Collect updates first to avoid borrow issues
        let updates: Vec<_> = if let Some(ref rx) = self.init_rx {
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        } else {
            Vec::new()
        };
        for update in updates {
            self.apply_init_update(update);
        }

        // Check if initialization is complete
        self.check_init_complete();

        // Show loading screen while initializing
        if self.loading_state.is_some() {
            self.render_loading_screen(ctx);
            return;
        }

        // ====================================================================
        // Normal app update (after initialization complete)
        // ====================================================================

        // Sync the virtual stage preview to the Lighting-menu toggle. Toggle off
        // => no fixture-state evaluation and no stage rendering. DMX output runs
        // independently of this (it follows the DMX output configuration).
        // When the stage first loads, open the menu so the controls are at hand.
        if self
            .dmx_manager
            .set_virtual_stage_enabled(self.control_panel.virtual_stage_enabled())
        {
            self.menu_visible = true;
        }

        // Periodic license refresh (every 24 hours)
        if self.auth_manager.is_authenticated()
            && self.last_license_refresh.elapsed() > std::time::Duration::from_secs(24 * 3600)
        {
            if let Some(ref db) = self.db {
                self.auth_manager.refresh_license_async(db.clone());
                self.last_license_refresh = std::time::Instant::now();
            }
        }

        // Browser sign-in handoff poll (Sign in with Google) — unconditional
        // (NOT inside the menu closure), so closing the menu while the
        // browser is open can't strand the flow.
        if self.auth_manager.should_poll_desktop_signin() {
            if let Some(ref db) = self.db {
                self.auth_manager.poll_desktop_signin(db.clone());
            }
        }

        // Auto-randomize theme on a fixed interval when the toggle is on.
        if self.control_panel.poll_auto_randomize_theme() {
            if let Some(mut pipeline_guard) = self.render_pipeline.try_lock() {
                if let Some(ref mut p) = *pipeline_guard {
                    self.control_panel.set_random_preset(p);
                }
            }
        }

        // Update Twitch server list in UI (from background ping results)
        // Only update when empty (servers are fetched once at startup)
        if self.control_panel.get_twitch_servers().is_empty() {
            let servers = crate::output::get_twitch_servers();
            if !servers.is_empty() {
                let server_infos: Vec<crate::ui::TwitchServerInfo> = servers.iter().map(|s| {
                    crate::ui::TwitchServerInfo {
                        name: s.name.clone(),
                        host: s.host.clone(),
                        latency_ms: s.latency_ms,
                        selected: s.selected,
                    }
                }).collect();
                self.control_panel.set_twitch_servers(server_infos);
            }
        }

        // PERFORMANCE: Process audio from lock-free ring buffer
        // Moved from callback to render loop to eliminate mutex contention
        self.process_audio_samples();

        // Detect recording stitch completion (processing true→false)
        {
            let is_processing = self.output_manager.is_processing();
            if self.was_recording_processing && !is_processing {
                log::info!("[Recording] Stitch complete — refreshing recordings list");
                self.control_panel.refresh_recordings();
            }
            self.was_recording_processing = is_processing;
        }

        // Poll audio meters for latest values (fundsp band splitting)
        if let Some(ref meters) = self.audio_meters {
            if let Some(values) = meters.get_meters() {
                self.meter_values = values;
            }
        }

        // Poll MIDI controller and apply values (non-blocking)
        let (midi_values, new_mapping) = self.midi_controller.poll();
        for (target, value) in midi_values {
            self.apply_midi_value(target, value);
        }

        // Save newly learned MIDI mapping to database
        if let Some(ref mapping) = new_mapping {
            if let Some(ref db) = self.db {
                let msg_type = match mapping.key.msg_type {
                    crate::midi::MidiMsgType::ControlChange => "CC",
                    crate::midi::MidiMsgType::NoteOn => "NoteOn",
                    crate::midi::MidiMsgType::NoteOff => "NoteOff",
                    crate::midi::MidiMsgType::PitchBend => "PitchBend",
                };
                let target = format!("{:?}", mapping.target);
                let row = crate::db::MidiMappingRow {
                    device_name: mapping.key.device_name.clone(),
                    channel: mapping.key.channel,
                    msg_type: msg_type.to_string(),
                    msg_index: mapping.key.index,
                    target,
                    min_value: mapping.min_value,
                    max_value: mapping.max_value,
                    inverted: mapping.inverted,
                };
                if let Err(e) = db.save_midi_mapping(&row) {
                    log::error!("[MIDI] Failed to save mapping: {}", e);
                }
            }

            // Check if we need to continue preset browser learning (step 2)
            if self.learning_preset_browser_step2 && mapping.target == crate::midi::ControlTarget::BrowsePresetEncoder {
                // Step 1 complete, now learn the select button
                self.midi_controller.start_learn();
                self.midi_controller.set_learn_target(crate::midi::ControlTarget::LoadPreset);
                self.learning_preset_browser_step2 = false;  // Will complete after this mapping
                log::info!("[MIDI] Preset browser learning - step 2: push button");
            } else if mapping.target == crate::midi::ControlTarget::LoadPreset {
                // Step 2 complete (or standalone learn)
                log::info!("[MIDI] Preset browser learning complete!");
            }

            // Check if we need to continue compel browser learning (step 2)
            if self.learning_compel_browser_step2 && mapping.target == crate::midi::ControlTarget::BrowseCompelEncoder {
                // Step 1 complete, now learn the select button
                self.midi_controller.start_learn();
                self.midi_controller.set_learn_target(crate::midi::ControlTarget::LoadCompel);
                self.learning_compel_browser_step2 = false;  // Will complete after this mapping
                log::info!("[MIDI] Compel browser learning - step 2: push button");
            } else if mapping.target == crate::midi::ControlTarget::LoadCompel {
                // Step 2 complete (or standalone learn)
                log::info!("[MIDI] Compel browser learning complete!");
            }
        }

        // Startup TV turn-on animation
        if self.startup_active {
            // Animate startup (slower than exit transition for dramatic effect)
            self.startup_progress += 0.025; // ~0.67 seconds at 60fps
            if self.startup_progress >= 1.0 {
                self.startup_progress = 1.0;
                self.startup_active = false;
            }
            ctx.request_repaint();

            // Render startup TV turn-on effect
            let screen = ctx.screen_rect();
            let p = self.startup_progress;

            // Black background during startup
            egui::Area::new(egui::Id::new("startup_bg"))
                .fixed_pos(screen.min)
                .order(egui::Order::Background)
                .show(ctx, |ui| {
                    ui.painter().rect_filled(screen, 0.0, egui::Color32::BLACK);
                });

            egui::Area::new(egui::Id::new("startup_tv"))
                .fixed_pos(screen.min)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let painter = ui.painter();
                    let center = screen.center();

                    if p < 0.4 {
                        // Phase 1: Horizontal white line appears and expands
                        let phase = p / 0.4;
                        let ease = phase * phase; // Ease in
                        let line_width = screen.width() * ease;
                        let line_height = 2.0 + ease * 6.0;

                        // Glow layers
                        for i in 0..6 {
                            let glow_alpha = ((6 - i) as f32 / 6.0) * 0.4 * phase;
                            let glow_expand = i as f32 * 10.0;
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    center,
                                    egui::vec2(line_width + glow_expand * 2.0, line_height + glow_expand * 2.0),
                                ),
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(180, 200, 255, (glow_alpha * 255.0) as u8),
                            );
                        }

                        // Core bright line
                        painter.rect_filled(
                            egui::Rect::from_center_size(center, egui::vec2(line_width, line_height)),
                            0.0,
                            egui::Color32::WHITE,
                        );
                    } else if p < 0.85 {
                        // Phase 2: Vertical expansion with glow
                        let phase = (p - 0.4) / 0.45;
                        let ease = 1.0 - (1.0 - phase).powi(2); // Ease out

                        let rect_width = screen.width();
                        let rect_height = 8.0 + (screen.height() - 8.0) * ease;

                        // Glow during expansion
                        for i in 0..4 {
                            let glow_alpha = ((4 - i) as f32 / 4.0) * 0.25 * (1.0 - phase * 0.5);
                            let glow_expand = i as f32 * 12.0;
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    center,
                                    egui::vec2(rect_width + glow_expand * 2.0, rect_height + glow_expand * 2.0),
                                ),
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(150, 170, 255, (glow_alpha * 255.0) as u8),
                            );
                        }

                        // Main rectangle - starts bright, fades as it expands
                        let brightness = 1.0 - phase * 0.4;
                        let val = (brightness * 255.0) as u8;
                        painter.rect_filled(
                            egui::Rect::from_center_size(center, egui::vec2(rect_width, rect_height)),
                            0.0,
                            egui::Color32::from_rgb(val, val, val),
                        );
                    } else {
                        // Phase 3: White fades out to reveal landing page
                        let phase = (p - 0.85) / 0.15;
                        let alpha = ((1.0 - phase) * 255.0) as u8;

                        painter.rect_filled(
                            screen,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
                        );
                    }
                });

            return; // Don't render anything else during startup
        }

        // Handle keyboard input (but not when typing in a text field)
        let has_text_focus = ctx.memory(|m| m.focused().is_some());
        let mut toggle_output = false;
        let mut toggle_output_fullscreen = false;
        let mut toggle_ndi = false;
        let mut toggle_hud = false;
        ctx.input(|i| {
            // 'M' to toggle menu (only if not typing in a text field)
            if i.key_pressed(egui::Key::M) && !has_text_focus {
                self.menu_visible = !self.menu_visible;
            }
            // 'O' to toggle output window
            if i.key_pressed(egui::Key::O) && !has_text_focus {
                toggle_output = true;
            }
            // 'N' to toggle NDI output
            if i.key_pressed(egui::Key::N) && !has_text_focus {
                toggle_ndi = true;
            }
            // 'F' to toggle output window fullscreen (when output is open)
            if i.key_pressed(egui::Key::F) && !has_text_focus && self.output_manager.has_native_window() {
                toggle_output_fullscreen = true;
            }
            // 'H' to toggle symmetry HUD (12/24-fold chroma visualization)
            if i.key_pressed(egui::Key::H) && !has_text_focus {
                toggle_hud = true;
            }
            // 'Space' to toggle freeze (harmonic state scrubbing)
            if i.key_pressed(egui::Key::Space) && !has_text_focus && !self.show_landing {
                self.control_panel.toggle_freeze();
            }
            // Any key or click starts TV turn-on transition
            if self.show_landing && !self.tv_transition_active {
                if i.pointer.any_click() || i.keys_down.len() > 0 {
                    self.tv_transition_active = true;
                    self.tv_transition_progress = 0.0;
                }
            }
        });

        // Handle output window toggle (outside input closure to satisfy borrow checker)
        if toggle_output {
            match self.output_manager.toggle_native_window() {
                Ok(true) => log::info!("[Output] Native window opened"),
                Ok(false) => log::info!("[Output] Native window closed"),
                Err(e) => log::error!("[Output] Failed to toggle window: {}", e),
            }
        }
        if toggle_output_fullscreen {
            let fs = !self.output_manager.is_native_fullscreen();
            self.output_manager.set_native_fullscreen(fs);
            log::info!("[Output] Fullscreen: {}", fs);
        }
        // Handle NDI toggle
        if toggle_ndi {
            match self.output_manager.toggle_ndi_output() {
                Ok(true) => log::info!("[NDI] Output started"),
                Ok(false) => log::info!("[NDI] Output stopped"),
                Err(e) => log::error!("[NDI] Failed to toggle: {}", e),
            }
        }

        // Handle HUD toggle - toggles both shader HUD and UI chroma wheel
        if toggle_hud {
            // Toggle UI chroma wheel
            self.control_panel.toggle_chroma_hud();
            let visible = self.control_panel.is_chroma_hud_visible();

            // Sync shader HUD to same state
            if let Some(ref mut pipeline) = *self.render_pipeline.lock() {
                if pipeline.is_symmetry_hud_visible() != visible {
                    pipeline.toggle_symmetry_hud();
                }
            }
            log::info!("[HUD] Chroma display: {}", if visible { "ON" } else { "OFF" });
        }

        // Update TV turn-on transition
        if self.tv_transition_active {
            // Animate progress (fast turn-on effect ~0.4 seconds at 60fps)
            self.tv_transition_progress += 0.04;
            if self.tv_transition_progress >= 1.0 {
                self.tv_transition_progress = 1.0;
                self.tv_transition_active = false;
                self.show_landing = false;

                // Restore saved menu visibility state after a few frames for layout to settle
                // (done below in frames_since_main_visible check)
            }
            ctx.request_repaint(); // Keep animating
        }

        // Load initial preset after TV transition (may retry if presets still loading)
        if !self.show_landing && !self.initial_preset_loaded {
            self.load_initial_preset();
        }

        // Request repaint for first several frames after main app becomes visible
        // to ensure proper layout settling
        if !self.show_landing {
            self.frames_since_main_visible += 1;

            // On first frame after landing, reset egui's area memory (in case of stale cache)
            if self.frames_since_main_visible == 1 {
                ctx.memory_mut(|mem| mem.reset_areas());
            }

            if self.frames_since_main_visible < 30 {
                ctx.request_repaint();
            }
            // Restore saved menu visibility after layout has settled (frame 10)
            if self.frames_since_main_visible == 10 {
                self.menu_visible = self.saved_menu_visible;
            }
        }

        // Always request repaint when not on landing page to keep animations smooth
        // The control panel spring animation needs continuous updates
        if !self.show_landing {
            ctx.request_repaint();
        }

        // Landing page overlay
        if self.show_landing {
            let screen = ctx.screen_rect();

            // No video auto-loaded on landing — user selects one or
            // a track announcement triggers it.
            if !self.landing_video_loaded {
                self.landing_video_loaded = true;
            }

            // Start landing timer on first frame
            if self.landing_start.is_none() {
                self.landing_start = Some(std::time::Instant::now());
            }

            // Auto-enter after 3 seconds
            if let Some(start) = self.landing_start {
                if start.elapsed().as_secs_f32() >= 3.0 && !self.tv_transition_active {
                    self.tv_transition_active = true;
                    self.tv_transition_progress = 0.0;
                }
            }

            // Load logo texture on first frame
            if self.logo_texture.is_none() {
                if let Some(logo_path) = find_resource("public/images/compeller.png") {
                    if let Ok(image_data) = std::fs::read(&logo_path) {
                        if let Ok(image) = image::load_from_memory(&image_data) {
                            let rgba = image.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let pixels = rgba.into_raw();
                            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                            self.logo_texture = Some(ctx.load_texture(
                                "compeller_logo",
                                color_image,
                                egui::TextureOptions::LINEAR,
                            ));
                            log::info!("[Landing] Loaded logo from {:?}", logo_path);
                        }
                    }
                }
            }

            // Load tagline texture on first frame
            if self.tagline_texture.is_none() {
                if let Some(tagline_path) = find_resource("public/images/sounddriven.png") {
                    if let Ok(image_data) = std::fs::read(&tagline_path) {
                        if let Ok(image) = image::load_from_memory(&image_data) {
                            let rgba = image.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let pixels = rgba.into_raw();
                            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                            self.tagline_texture = Some(ctx.load_texture(
                                "sounddriven_tagline",
                                color_image,
                                egui::TextureOptions::LINEAR,
                            ));
                            log::info!("[Landing] Loaded tagline from {:?}", tagline_path);
                        }
                    }
                }
            }

            // Render video background using OpenGL paint callback
            if self.landing_video.is_playing() {
                let pipeline = self.render_pipeline.clone();

                // Get video frame and upload to pipeline for rendering
                let mut frame_uploaded = false;
                #[cfg(target_os = "macos")]
                {
                    if self.landing_video.is_zero_copy() {
                        if let Some(pixel_buffer) = self.landing_video.get_frame_zero_copy() {
                            if let Some(ref mut p) = *pipeline.lock() {
                                frame_uploaded = p.set_video_pixel_buffer(&pixel_buffer);
                            }
                        }
                    }
                }
                // Fallback only if zero-copy didn't work
                if !frame_uploaded {
                    if let Some((frame_data, width, height)) = self.landing_video.get_frame() {
                        if let Some(ref mut p) = *pipeline.lock() {
                            p.set_video_frame(frame_data, width, height);
                        }
                    }
                }

                // Render video as fullscreen background
                egui::Area::new(egui::Id::new("landing_video"))
                    .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                    .order(egui::Order::Background)
                    .show(ctx, |ui| {
                        let pipeline_for_render = self.render_pipeline.clone();

                        let callback = egui::PaintCallback {
                            rect: screen,
                            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                                let gl = painter.gl();
                                if let Some(ref mut pipe) = *pipeline_for_render.lock() {
                                    unsafe {
                                        let viewport = info.viewport_in_pixels();
                                        gl.viewport(
                                            viewport.left_px as i32,
                                            viewport.from_bottom_px as i32,
                                            viewport.width_px as i32,
                                            viewport.height_px as i32,
                                        );
                                        // Render just the video (no projectM) - full visibility
                                        let mut params = RenderParams::default();
                                        params.projectm_visibility = 0.0;
                                        params.video_visibility = 1.0;
                                        params.video_scale = 1.0;
                                        let _ = pipe.render_frame(
                                            viewport.width_px as u32,
                                            viewport.height_px as u32,
                                            &params,
                                        );
                                    }
                                }
                            })),
                        };
                        ui.painter().add(callback);
                    });

                // Subtle dark overlay for text readability
                egui::Area::new(egui::Id::new("landing_overlay"))
                    .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                    .order(egui::Order::Middle)
                    .show(ctx, |ui| {
                        ui.painter().rect_filled(
                            screen,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                        );
                    });
            } else {
                // Fallback: dark background if no video
                egui::Area::new(egui::Id::new("landing_bg"))
                    .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
                    .order(egui::Order::Background)
                    .show(ctx, |ui| {
                        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_rgb(5, 5, 8));
                    });
            }

            // Minimalist landing: Logo + Tagline image (auto-enters after 3s)
            egui::Area::new(egui::Id::new("landing"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let center = screen.center();

                    // Logo - centered (2.6x bigger)
                    if let Some(ref texture) = self.logo_texture {
                        let desired_height = 650.0;
                        let aspect = texture.size()[0] as f32 / texture.size()[1] as f32;
                        let logo_size = egui::vec2(desired_height * aspect, desired_height);
                        let logo_pos = egui::pos2(
                            center.x - logo_size.x / 2.0,
                            center.y - logo_size.y / 2.0 - 60.0,
                        );
                        ui.painter().image(
                            texture.id(),
                            egui::Rect::from_min_size(logo_pos, logo_size),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }

                    // Tagline image - centered below logo
                    if let Some(ref texture) = self.tagline_texture {
                        let desired_height = 120.0;
                        let aspect = texture.size()[0] as f32 / texture.size()[1] as f32;
                        let tagline_size = egui::vec2(desired_height * aspect, desired_height);
                        let tagline_pos = egui::pos2(
                            center.x - tagline_size.x / 2.0,
                            center.y + 50.0,
                        );
                        ui.painter().image(
                            texture.id(),
                            egui::Rect::from_min_size(tagline_pos, tagline_size),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                });
        }

        // TV turn-on transition effect overlay
        if self.tv_transition_active {
            let screen = ctx.screen_rect();
            let p = self.tv_transition_progress;

            egui::Area::new(egui::Id::new("tv_transition"))
                .fixed_pos(screen.min)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let painter = ui.painter();

                    // Phase 1 (0.0-0.3): Horizontal line expands from center
                    // Phase 2 (0.3-0.8): Vertical expansion with glow
                    // Phase 3 (0.8-1.0): Brightness fades to reveal content

                    let center = screen.center();

                    if p < 0.3 {
                        // Phase 1: Horizontal white line appears and expands
                        let phase = p / 0.3; // 0-1 within this phase
                        let line_width = screen.width() * phase;
                        let line_height = 2.0 + phase * 4.0; // Grows from 2 to 6 pixels

                        // Glow effect (larger, more transparent)
                        for i in 0..5 {
                            let glow_alpha = ((5 - i) as f32 / 5.0) * 0.3 * phase;
                            let glow_expand = i as f32 * 8.0;
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    center,
                                    egui::vec2(line_width + glow_expand * 2.0, line_height + glow_expand * 2.0),
                                ),
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(200, 220, 255, (glow_alpha * 255.0) as u8),
                            );
                        }

                        // Core bright line
                        painter.rect_filled(
                            egui::Rect::from_center_size(center, egui::vec2(line_width, line_height)),
                            0.0,
                            egui::Color32::WHITE,
                        );
                    } else if p < 0.8 {
                        // Phase 2: Vertical expansion - rectangle grows to fill screen
                        let phase = (p - 0.3) / 0.5; // 0-1 within this phase
                        let ease_phase = 1.0 - (1.0 - phase).powi(3); // Ease out cubic

                        let rect_width = screen.width();
                        let rect_height = 6.0 + (screen.height() - 6.0) * ease_phase;

                        // Outer glow during expansion
                        for i in 0..3 {
                            let glow_alpha = ((3 - i) as f32 / 3.0) * 0.2 * (1.0 - phase);
                            let glow_expand = i as f32 * 15.0;
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    center,
                                    egui::vec2(rect_width + glow_expand * 2.0, rect_height + glow_expand * 2.0),
                                ),
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(150, 180, 255, (glow_alpha * 255.0) as u8),
                            );
                        }

                        // Main expanding rectangle - bright white fading to reveal content
                        let brightness = 1.0 - phase * 0.3; // Dim slightly as it expands
                        let color_val = (brightness * 255.0) as u8;
                        painter.rect_filled(
                            egui::Rect::from_center_size(center, egui::vec2(rect_width, rect_height)),
                            0.0,
                            egui::Color32::from_rgb(color_val, color_val, color_val),
                        );
                    } else {
                        // Phase 3: Brightness fades out to reveal the app
                        let phase = (p - 0.8) / 0.2; // 0-1 within this phase
                        let alpha = ((1.0 - phase) * 255.0) as u8;

                        painter.rect_filled(
                            screen,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
                        );
                    }
                });
        }

        // Audio watchdog: check every ~5 seconds (300 frames at 60fps)
        // If audio capture has gone stale (no samples for 10+ seconds), auto-reconnect
        if self.frame_count % 300 == 0 && self.audio_enabled {
            let is_stale = self.audio_capture.lock().is_stale(10);
            if is_stale {
                log::warn!("[AudioWatchdog] Audio capture stale - reconnecting...");
                let mut audio = self.audio_capture.lock();
                match audio.reconnect() {
                    Ok(()) => {
                        log::info!("[AudioWatchdog] Audio reconnected successfully");
                        // Update sample rate for NDI
                        let sample_rate = audio.sample_rate;
                        self.output_manager.set_ndi_source_sample_rate(sample_rate);
                    }
                    Err(e) => {
                        log::error!("[AudioWatchdog] Failed to reconnect audio: {}", e);
                        // Set audio_enabled to false so user sees "Stopped" state
                        drop(audio); // Release lock before modifying self
                        self.audio_enabled = false;
                    }
                }
            }
        }

        // Silence gate: 4-state hysteresis machine that pauses decoders on sustained silence
        {
            let analysis = self.audio_analysis.lock().clone();
            let dt = 1.0 / 60.0; // Approximate frame time

            // Use RAW (pre-normalization) band energies for the gate.
            // Normalized bass/mid/high are useless here — the adaptive normalizer
            // maps noise to ~0.5 during silence, defeating silence detection.
            let raw_sum = analysis.raw_bass + analysis.raw_mid + analysis.raw_high;
            let energy = raw_sum;

            // Tick cooldown
            if self.gate_cooldown_remaining > 0.0 {
                self.gate_cooldown_remaining = (self.gate_cooldown_remaining - dt).max(0.0);
            }

            let prev_state = self.silence_gate;

            match self.silence_gate {
                SilenceGateState::AudioActive => {
                    // Sync pre_silence_visibility from control panel only in AudioActive
                    let user_visibility = self.control_panel.get_params().video_visibility;
                    if (user_visibility - self.pre_silence_visibility).abs() > 0.01 {
                        self.pre_silence_visibility = user_visibility;
                    }

                    // Ease toward target visibility
                    let target = self.pre_silence_visibility;
                    let diff = target - self.effective_video_visibility;
                    if diff.abs() > 0.01 {
                        self.effective_video_visibility += diff * FADE_IN_RATE;
                    } else {
                        self.effective_video_visibility = target;
                    }

                    // Check for silence entry (only after cooldown)
                    if energy < SILENCE_ENTER_THRESHOLD && self.gate_cooldown_remaining <= 0.0 {
                        self.silence_gate = SilenceGateState::EnteringSilence;
                        self.silence_enter_accumulator = 0.0;
                    }
                }

                SilenceGateState::EnteringSilence => {
                    // Energy recovered — back to active with cooldown to prevent oscillation
                    if energy >= SILENCE_ENTER_THRESHOLD {
                        self.silence_gate = SilenceGateState::AudioActive;
                        self.silence_enter_accumulator = 0.0;
                        self.gate_cooldown_remaining = GATE_COOLDOWN;
                    } else {
                        // Accumulate hold time
                        self.silence_enter_accumulator += dt;
                        if self.silence_enter_accumulator >= self.control_panel.silence_threshold() {
                            // Confirmed silence — fade out video
                            self.silence_gate = SilenceGateState::Silent;
                            self.gate_cooldown_remaining = GATE_COOLDOWN;
                        }
                    }
                }

                SilenceGateState::Silent => {
                    // Fade opacity to 0
                    self.effective_video_visibility *= FADE_OUT_RATE;
                    if self.effective_video_visibility < 0.01 {
                        self.effective_video_visibility = 0.0;
                    }

                    // Check for audio return (only after cooldown)
                    if energy > AUDIO_RETURN_THRESHOLD && self.gate_cooldown_remaining <= 0.0 {
                        self.silence_gate = SilenceGateState::ReturningAudio;
                        self.audio_return_accumulator = 0.0;
                    }
                }

                SilenceGateState::ReturningAudio => {
                    // Energy dropped back — return to silent
                    if energy <= AUDIO_RETURN_THRESHOLD {
                        self.silence_gate = SilenceGateState::Silent;
                        self.audio_return_accumulator = 0.0;
                    } else {
                        // Accumulate confirm time
                        self.audio_return_accumulator += dt;

                        // Half-speed anticipation fade-in during confirm
                        let target = self.pre_silence_visibility;
                        let diff = target - self.effective_video_visibility;
                        if diff.abs() > 0.01 {
                            self.effective_video_visibility += diff * (FADE_IN_RATE * 0.5);
                        }

                        if self.audio_return_accumulator >= AUDIO_CONFIRM_DURATION {
                            // Confirmed audio return — fade in video
                            self.silence_gate = SilenceGateState::AudioActive;
                            self.gate_cooldown_remaining = GATE_COOLDOWN;
                        }
                    }
                }
            }

            // Log state transitions at info level for diagnostics
            if self.silence_gate != prev_state {
                log::info!("[SilenceGate] {:?} -> {:?} (energy={:.4}, vis={:.3}, cooldown={:.2})",
                    prev_state, self.silence_gate, energy, self.effective_video_visibility,
                    self.gate_cooldown_remaining);
            }

            // Periodic diagnostic: log gate state + energy every ~1 second (calibration)
            if self.frame_count % 60 == 0 {
                log::info!("[SilenceGate] state={:?} raw_sum={:.6} vis={:.3} cooldown={:.2}",
                    self.silence_gate, raw_sum, self.effective_video_visibility,
                    self.gate_cooldown_remaining);
            }
        }

        // First-run setup wizard (shown after landing page is dismissed)
        if !self.show_landing && self.show_setup_wizard {
            self.render_setup_wizard(ctx);
            return; // Don't show main UI while setup wizard is open
        }

        // Sync prompt (shown when compels directory is empty)
        if !self.show_landing && self.show_sync_prompt {
            if self.render_sync_prompt(ctx) {
                // User clicked Sync - open menu and start sync
                self.menu_visible = true;
                self.control_panel.start_compels_sync();

                // Also start compel sync if authenticated
                if self.auth_manager.is_authenticated() {
                    self.control_panel.start_compel_sync(self.auth_manager.get_token());
                }
            }
            return; // Don't show main UI while sync prompt is open
        }

        // Drain any sync-worker events (StagePulled) and run the debounced
        // auto-flush diff each frame — independent of whether the virtual
        // stage preview is visible, so operator drag edits to fixtures behind
        // a closed menu still persist.
        let sync_token = self.auth_manager.get_token();
        // Forced stage selection (Koob): REACT no longer auto-opens the last stage on
        // startup — it comes up with NO stage so the operator must explicitly select one
        // from the Lighting menu. To restore the old offline-first auto-open, call
        // `self.dmx_manager.restore_last_stage(tok)` here (the helper is retained).
        self.dmx_manager.poll(sync_token.as_deref());

        // Virtual stage — operator 3D preview of the lighting rig. Gated entirely
        // by the Lighting-menu toggle: when off, nothing here runs, so there is
        // no stage-render overhead. DMX output is independent of this toggle.
        if !self.show_landing && self.control_panel.virtual_stage_enabled() {
            let view = self.control_panel.virtual_stage_view();
            let zoom = self.control_panel.virtual_stage_zoom();
            let yaw = self.control_panel.virtual_stage_yaw();
            let pitch = self.control_panel.virtual_stage_pitch();
            let show_labels = self.control_panel.virtual_stage_labels();
            // Venue is editable from the in-stage selector. Seed a local copy,
            // let show() mutate it, then push back only on a real change so the
            // setter doesn't dirty settings every frame.
            let mut venue = self.control_panel.virtual_stage_venue();
            // A selected SaaS *wedding* stage forces the Wedding room: it renders
            // from `self.patch`, and without this the per-frame swap below would
            // snap a wedding-stage pick straight back to the operator's Concert
            // toggle. A concert / un-migrated / no-stage selection leaves the
            // operator's Venue toggle in charge, so the local "Wedding Room" demo
            // stays reachable with a concert stage loaded.
            if self.dmx_manager.stage_venue() == Some(crate::emerge::VenueKind::WeddingRoom)
                && venue != crate::emerge::VenueKind::WeddingRoom
            {
                venue = crate::emerge::VenueKind::WeddingRoom;
                self.control_panel.set_virtual_stage_venue(venue);
            }
            // Swap the manager's active scene to match the venue BEFORE reading
            // intents/patch (Concert = the SaaS stage; Wedding = a SaaS wedding
            // stage or the REACT-local demo). No-op when the venue is unchanged.
            self.dmx_manager.set_active_venue(venue);
            // Fog trigger drives the virtual-stage smoke build-up. Latch
            // composes with hold so a tap-locked fog still renders.
            let fog_ui = self.osc_thread.ui_state();
            let fog = fog_ui.trigger_fog_held || fog_ui.trigger_fog_latched;
            // Clone intents + profiles first (immutable borrows), then take
            // the patch mutably — the stage view edits fixture position/aim
            // in place. Profile clone is cheap (typically <20 fixtures) and
            // lets the panel + render path consult the ATLABASE profile data
            // without re-borrowing the manager.
            let states = self.dmx_manager.fixture_intents().to_vec();
            // Preview draw-list: glyphs (a GigBAR expands into 8 sections) + the
            // editable-fixture index each glyph belongs to. Cloned before the
            // mutable patch borrow below.
            let glyph_fixtures = self.dmx_manager.glyph_fixtures().to_vec();
            let glyph_parents = self.dmx_manager.glyph_parents().to_vec();
            let profiles = self.dmx_manager.fixture_profiles().clone();
            let has_stage = self.dmx_manager.selected_stage_id().is_some();
            let library_profiles: Vec<crate::emerge::FixtureProfile> = self
                .dmx_manager
                .profile_library()
                .all_profiles();
            // Snapshot the last DMX frame so pixel bars draw their true
            // per-segment color (cloned before the mutable patch borrow).
            let dmx_frame = self.dmx_manager.last_dmx_frame().cloned();
            // Read-only when the active patch is a shared admin template the login
            // can't edit (the local Wedding demo always edits freely). Read before
            // the mutable patch borrow. Disables drag / aim / +Fixture in the view.
            let editable = self.dmx_manager.active_patch_editable();
            let patch = self.dmx_manager.fixture_patch_mut();
            if !self.virtual_stage_view.show(
                ctx, view, zoom, yaw, pitch, &self.render_pipeline, patch,
                &mut self.fixture_overrides, &glyph_fixtures, &states, &glyph_parents, &profiles,
                &library_profiles, has_stage, dmx_frame.as_ref(), fog, show_labels,
                &mut venue, editable,
            ) {
                self.control_panel.set_virtual_stage_enabled(false);
            }
            // The overlay Venue selector edits `venue`; persist the operator's
            // choice (next frame `set_active_venue` swaps the scene to match).
            if venue != self.control_panel.virtual_stage_venue() {
                self.control_panel.set_virtual_stage_venue(venue);
            }
        } else {
            // Stage closed — stop any screen-video decoder so it doesn't keep
            // decoding on its background thread (24/7-demo CPU). The file binding
            // is retained and resumes when the stage reopens.
            self.virtual_stage_view.release_video();
        }

        // Persist the per-fixture Segment FX choice, debounced: the serialized state
        // doubles as the change-detector, a settled edit flushes ~0.5s later, and the
        // latest is always written on exit.
        let segfx_now = self.fixture_overrides.segfx_json();
        if segfx_now != self.segfx_last_json {
            self.segfx_last_json = segfx_now;
            self.segfx_save_in = 30;
        } else if self.segfx_save_in > 0 {
            self.segfx_save_in -= 1;
            if self.segfx_save_in == 0 {
                if let Some(ref db) = self.db {
                    let _ = db.set_setting("fixture_segment_fx", &self.segfx_last_json);
                }
            }
        }

        // Menu toggle button (always visible when not on landing page)
        if !self.show_landing {
            let screen = ctx.screen_rect();
            let panel_width = 420.0;  // Must match overlay panel width

            // Position the toggle button at the left edge of the overlay panel
            let button_x = if self.menu_visible {
                screen.max.x - panel_width - 10.0 - 32.0  // Left edge of panel (panel_margin + button_width)
            } else {
                screen.max.x - 32.0  // Screen edge when menu hidden
            };

            egui::Area::new(egui::Id::new("menu_toggle"))
                .fixed_pos(egui::pos2(button_x, screen.center().y - 30.0))
                .order(egui::Order::Tooltip)  // Above everything including menu
                .show(ctx, |ui| {
                    let arrow = if self.menu_visible { "▶" } else { "◀" };
                    let tooltip = if self.menu_visible { "Hide menu (M)" } else { "Show menu (M)" };

                    let button = egui::Button::new(
                        egui::RichText::new(arrow)
                            .size(20.0)
                            .color(egui::Color32::WHITE)
                    )
                    .fill(egui::Color32::from_rgba_unmultiplied(40, 40, 60, 200))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 100)))
                    .min_size(egui::vec2(28.0, 60.0));

                    if ui.add(button).on_hover_text(tooltip).clicked() {
                        self.menu_visible = !self.menu_visible;
                    }
                });
        }

        // Overlay menu panel (floats on top of viewport, doesn't resize it)
        if self.menu_visible && !self.show_landing && !self.control_panel.hide_menu_for_fullscreen {
            let screen = ctx.screen_rect();
            let panel_width = 420.0;  // Wider panel to prevent jumping
            let panel_height = screen.height() - 40.0; // Leave some margin

            egui::Area::new(egui::Id::new("controls_overlay"))
                .fixed_pos(egui::pos2(screen.max.x - panel_width - 10.0, 20.0))
                .order(egui::Order::Foreground)
                .interactable(true)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 30, 230))
                        .rounding(egui::Rounding::same(8.0))
                        .inner_margin(egui::Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(panel_width - 24.0, panel_height));
                            ui.set_max_size(egui::vec2(panel_width - 24.0, panel_height));

                            // Scrollable content area
                            egui::ScrollArea::vertical()
                                .max_height(panel_height - 40.0)
                                .show(ui, |ui| {
                                    let pipeline = self.render_pipeline.clone();
                                    let audio = self.audio_capture.clone();
                                    let audio_enabled = self.audio_enabled;
                                    let analysis = self.audio_analysis.lock().clone();

                                    // Sync buffer frozen state with control panel
                                    self.harmonic_state_buffer.set_frozen(self.control_panel.is_frozen());
                                    self.harmonic_state_buffer.set_scrub(self.control_panel.scrub_position());

                                    // Update theme/geometry: live when not frozen, from buffer when frozen
                                    if let Some(mut pipe_guard) = pipeline.try_lock() {
                                        if let Some(ref mut p) = *pipe_guard {
                                            let dt = 1.0 / 60.0; // ~60fps

                                            if self.control_panel.is_frozen() {
                                                // FROZEN: use buffer state, don't update from live audio
                                                if let Some(frozen_state) = self.harmonic_state_buffer.current().cloned() {
                                                    p.set_frozen_harmonic_state(frozen_state.clone());
                                                    self.control_panel.set_frozen_display_state(Some(frozen_state));
                                                }
                                            } else {
                                                // LIVE: update from audio analysis
                                                let theme_audio = crate::render::AudioAnalysis::with_chroma(
                                                    Vec::new(),
                                                    analysis.bass,
                                                    analysis.mid,
                                                    analysis.high,
                                                    analysis.kick,
                                                    analysis.energy,
                                                    analysis.energy_vis,
                                                    analysis.chroma,
                                                    analysis.display_chroma,
                                                    analysis.is_tonal,
                                                    analysis.dominant_pc,
                                                    analysis.tonal_separation,
                                                    analysis.tonal_energy,
                                                    analysis.stability,
                                                    analysis.band_centroids,
                                                    analysis.raw_bass,
                                                    analysis.raw_mid,
                                                    analysis.raw_high,
                                                    analysis.onset_strength,
                                                );
                                                p.update_theme(&theme_audio, dt);

                                                // Push state to history buffer
                                                if let Some(state) = p.get_harmonic_field_state() {
                                                    self.harmonic_state_buffer.push(state);
                                                }

                                                // Note: Lighting input (OSC/Hue/WLED) is sent from the
                                                // always-running block below (line ~5204) so it works
                                                // regardless of menu visibility.

                                                self.control_panel.set_frozen_display_state(None);
                                            }
                                        }
                                    }

                                    // Build output state for the control panel
                                    let ndi_tally = self.output_manager.ndi_tally();
                                    let rec_state = self.output_manager.get_recording_state();
                                    let seg_metrics = self.output_manager.get_segmented_recording_metrics();
                                    let audio_gains = self.output_manager.get_audio_gains();
                                    let output_state = crate::ui::OutputState {
                                        ndi_enabled: self.output_manager.has_ndi_output(),
                                        ndi_available: self.output_manager.is_ndi_available(),
                                        ndi_resolution: self.control_panel.ndi_resolution(),
                                        ndi_audio_enabled: self.output_manager.is_ndi_audio_enabled(),
                                        ndi_connections: self.output_manager.ndi_connections(),
                                        ndi_tally: crate::ui::NdiTallyInfo {
                                            on_program: ndi_tally.on_program,
                                            on_preview: ndi_tally.on_preview,
                                        },
                                        ndi_audio_gain: audio_gains.ndi_user,
                                        ndi_assist_gain: audio_gains.ndi_assist,
                                        ndi_effective_gain: audio_gains.ndi_effective,
                                        ndi_assist_hold: audio_gains.ndi_assist_hold,
                                        ndi_audio_present: audio_gains.ndi_audio_present,
                                        output_window_enabled: self.output_manager.has_native_window(),
                                        output_window_fullscreen: self.output_manager.is_native_fullscreen(),
                                        recording_enabled: self.output_manager.is_recording(),
                                        recording_available: self.output_manager.is_recording_available(),
                                        recording_duration_secs: rec_state.duration_secs,
                                        recording_processing: self.output_manager.is_processing(),
                                        rec_audio_gain: audio_gains.rec_user,
                                        rec_assist_gain: audio_gains.rec_assist,
                                        rec_effective_gain: audio_gains.rec_effective,
                                        rec_assist_hold: audio_gains.rec_assist_hold,
                                        rec_audio_present: audio_gains.rec_audio_present,
                                        // Segmented recording fields
                                        segmented_recording: seg_metrics.is_some(),
                                        current_segment: seg_metrics.as_ref().map(|m| m.current_segment).unwrap_or(0),
                                        committed_segments: seg_metrics.as_ref().map(|m| m.committed_segments).unwrap_or(0),
                                        last_commit_elapsed_ms: seg_metrics.as_ref().map(|m| m.last_commit_elapsed_ms).unwrap_or(0),
                                    };

                                    // Get CDJ device info from track sources
                                    let cdj_devices: Vec<_> = self.track_sources.iter()
                                        .flat_map(|s| s.get_cdj_devices())
                                        .collect();

                                    // Check for CDJ state changes and save to database (event-driven, non-blocking)
                                    let current_state: Vec<_> = cdj_devices.iter()
                                        .map(|d| (
                                            d.device_id,
                                            format!("{}:{}", d.track_title.as_deref().unwrap_or(""), d.track_artist.as_deref().unwrap_or("")),
                                            d.is_master,
                                            d.is_playing,
                                            d.is_looping,
                                        ))
                                        .collect();

                                    if current_state != self.last_cdj_state {
                                        self.last_cdj_state = current_state;

                                        // Spawn background thread to save state (non-blocking)
                                        if let Some(db) = self.db.clone() {
                                            let devices_to_save: Vec<_> = cdj_devices.iter()
                                                .map(|d| (
                                                    d.device_id,
                                                    d.name.clone(),
                                                    d.ip.to_string(),
                                                    d.track_title.clone(),
                                                    d.track_artist.clone(),
                                                    d.is_master,
                                                    d.is_playing,
                                                    d.is_looping,
                                                    d.is_on_air,
                                                    d.bpm,
                                                ))
                                                .collect();

                                            std::thread::spawn(move || {
                                                for (device_id, name, ip, title, artist, is_master, is_playing, is_looping, is_on_air, bpm) in devices_to_save {
                                                    let _ = db.upsert_cdj_device(
                                                        device_id,
                                                        &name,
                                                        &ip,
                                                        title.as_deref(),
                                                        artist.as_deref(),
                                                        is_master,
                                                        is_playing,
                                                        is_looping,
                                                        is_on_air,
                                                        bpm,
                                                    );
                                                }
                                                let _ = db.prune_stale_cdj_devices(30);
                                            });
                                        }
                                    }

                                    // Pass latest meter values to control panel for display
                                    self.control_panel.set_meter_values(self.meter_values);

                                    // Build MIDI state for UI (non-blocking - just reads current state)
                                    let midi_state = Some(MidiState {
                                        connected_devices: self.midi_controller.connection_count(),
                                        learn_state: self.midi_controller.learn_state().clone(),
                                        mappings: self.midi_controller.mappings().to_vec(),
                                        learning_preset_browser: self.learning_preset_browser_step2,
                                        learning_compel_browser: self.learning_compel_browser_step2,
                                    });

                                    // Build OSC state for UI (from thread's published OscUiState)
                                    let osc_ui = self.osc_thread.ui_state();
                                    let maestro_data = self.osc_thread.maestro_data();
                                    let osc_state = Some(OscState {
                                        enabled: self.osc_thread.is_enabled(),
                                        target_ip: self.osc_thread.config().target_ip.clone(),
                                        target_port: self.osc_thread.config().target_port,
                                        config: self.osc_thread.config().clone(),
                                        current_zone: osc_ui.current_zone,
                                        current_palette: osc_ui.current_palette,
                                        current_harmonic: osc_ui.current_harmonic,
                                        strobe_hold_active: osc_ui.strobe_hold_active,
                                        blackout_active: osc_ui.blackout_active,
                                        zone_activity: osc_ui.zone_activity,
                                        smoothed_metrics: osc_ui.smoothed_metrics,
                                        // Manual triggers state
                                        trigger_strobe_held: osc_ui.trigger_strobe_held,
                                        trigger_blackout_held: osc_ui.trigger_blackout_held,
                                        trigger_blinder_held: osc_ui.trigger_blinder_held,
                                        auto_blinder_active: osc_ui.auto_blinder_active,
                                        macro_phase: osc_ui.macro_phase,
                                        resolved_style: self.dmx_manager.effective_lighting_style(),
                                        trigger_fog_held: osc_ui.trigger_fog_held,
                                        trigger_fog_latched: osc_ui.trigger_fog_latched,
                                        trigger_uv_held: osc_ui.trigger_uv_held,
                                        trigger_uv_latched: osc_ui.trigger_uv_latched,
                                        trigger_strobe_rate: osc_ui.trigger_strobe_rate,
                                        current_pattern: osc_ui.current_pattern,
                                        // MaestroDMX API data
                                        maestro_patterns: maestro_data.patterns.iter().map(|p| (p.index, p.name.clone())).collect(),
                                        maestro_color_palettes: maestro_data.color_palettes.iter().map(|p| (p.index, p.name.clone())).collect(),
                                        maestro_fx_palettes: maestro_data.fx_palettes.iter().map(|p| (p.index, p.name.clone())).collect(),
                                        maestro_shows: maestro_data.shows.iter().map(|s| (s.uuid.clone(), s.name.clone())).collect(),
                                        maestro_loading: maestro_data.loading,
                                        maestro_error: maestro_data.error.clone(),
                                        maestro_last_fetch_ip: maestro_data.last_fetch_ip.clone(),
                                    });

                                    // Pick up pending Hue config changes from thread (for DB persistence)
                                    if let Some(hue_config) = self.hue_thread.take_pending_save() {
                                        if let Some(ref db) = self.db {
                                            let _ = db.save_hue_config(&hue_config);
                                        }
                                    }

                                    // Build Hue state for UI (from thread's published HueState)
                                    let hue_state = Some(self.hue_thread.state());

                                    // Pick up WLED device info from background HTTP fetches
                                    let wled_info_changed = self.wled_manager.apply_pending_info();
                                    if wled_info_changed {
                                        if let Some(ref db) = self.db {
                                            let _ = db.save_wled_config(self.wled_manager.config());
                                        }
                                    }

                                    // Build WLED state for UI
                                    let wled_state = Some(self.wled_manager.state());

                                    // Build camera state for UI
                                    let camera_state = Some(CameraState {
                                        enabled: self.camera_manager.is_active(),
                                        available_cameras: self.camera_manager.available_cameras().to_vec(),
                                        enabled_cameras: vec![self.camera_manager.program_camera()],
                                        program_camera: self.camera_manager.program_camera(),
                                        output: self.camera_manager.get_output(),
                                        camera_track_counts: self.camera_manager.get_camera_states(),
                                        preview: self.camera_manager.get_preview_frame(),
                                    });

                                    // Build YouTube state for UI
                                    let youtube_state = if self.auth_manager.is_authenticated() {
                                        Some(crate::ui::YouTubeState {
                                            auth: &self.youtube_auth,
                                            eligibility: &self.youtube_eligibility,
                                            session: self.youtube_session.as_ref(),
                                        })
                                    } else {
                                        None
                                    };

                                    // Lighting-clock status for the clock UI: live Link tempo + REACT's estimate.
                                    let link_bpm = self.ableton_link.tempo().and_then(|(bpm, seen)| {
                                        (seen.elapsed().as_secs_f32() < 2.0).then_some(bpm)
                                    });
                                    let (derived_bpm, derived_confidence) = self.dmx_manager.derived_tempo();
                                    self.control_panel.set_clock_status(link_bpm, derived_bpm, derived_confidence);

                                    let stage_ui_state = crate::app::StageUiState {
                                        stages: self.dmx_manager.available_stages().to_vec(),
                                        selected_stage_id: self.dmx_manager.selected_stage_id(),
                                        sync_status: self.dmx_manager.sync_status(),
                                        cue_defs: self.dmx_manager.cached_cue_defs(),
                                        active_local_cues: self.dmx_manager.active_local_cues(),
                                        dmx_status: self.dmx_manager.output_status(),
                                    };
                                    let (audio_action, output_action, midi_action, osc_action, hue_action, wled_action, camera_action, auth_action, youtube_action, twitch_action, kick_action, dual_stream_action) = self.control_panel.show_with_all(
                                        ui, pipeline, audio, audio_enabled, &analysis, output_state, cdj_devices, midi_state, osc_state, hue_state, wled_state, camera_state, Some(&self.auth_manager), youtube_state, Some(&stage_ui_state)
                                    );

                                    // Handle audio actions from UI
                                    match audio_action {
                                        AudioAction::None => {}
                                        AudioAction::Start => self.start_audio(),
                                        AudioAction::Stop => self.stop_audio(),
                                        AudioAction::SetSource(source) => self.set_audio_source(source),
                                        AudioAction::SetGain(gain) => {
                                            // Update the shared gain value (used by audio callback)
                                            self.audio_gain.store(gain_to_bits(gain), Ordering::Relaxed);
                                        }
                                    }

                                    // Handle output actions from UI
                                    match output_action {
                                        OutputAction::None => {}
                                        OutputAction::ToggleNdi => {
                                            match self.output_manager.toggle_ndi_output() {
                                                Ok(true) => log::info!("[NDI] Output started from menu"),
                                                Ok(false) => log::info!("[NDI] Output stopped from menu"),
                                                Err(e) => log::error!("[NDI] Failed to toggle: {}", e),
                                            }
                                        }
                                        OutputAction::ToggleNdiAudio => {
                                            let enabled = !self.output_manager.is_ndi_audio_enabled();
                                            self.output_manager.set_ndi_audio_enabled(enabled);
                                        }
                                        OutputAction::ToggleOutputWindow => {
                                            match self.output_manager.toggle_native_window() {
                                                Ok(true) => log::info!("[Output] Window opened from menu"),
                                                Ok(false) => log::info!("[Output] Window closed from menu"),
                                                Err(e) => log::error!("[Output] Failed to toggle: {}", e),
                                            }
                                        }
                                        OutputAction::ToggleOutputFullscreen => {
                                            let fs = !self.output_manager.is_native_fullscreen();
                                            self.output_manager.set_native_fullscreen(fs);
                                            log::info!("[Output] Fullscreen from menu: {}", fs);
                                        }
                                        OutputAction::SetNdiGain(g) => {
                                            self.output_manager.set_ndi_user_gain(g);
                                        }
                                        OutputAction::SetRecGain(g) => {
                                            self.output_manager.set_rec_user_gain(g);
                                            if let Some(ref db) = self.db {
                                                let _ = db.set_float("rec_audio_gain", g);
                                            }
                                        }
                                        OutputAction::SetYouTubeGain(g) => {
                                            self.youtube_audio_gain.store(g.to_bits(), Ordering::Relaxed);
                                        }
                                        OutputAction::SetTwitchGain(g) => {
                                            self.twitch_audio_gain.store(g.to_bits(), Ordering::Relaxed);
                                        }
                                        OutputAction::SetKickGain(g) => {
                                            self.kick_audio_gain.store(g.to_bits(), Ordering::Relaxed);
                                        }
                                        OutputAction::ToggleRecording => {
                                            log::info!("[Recording] Toggle recording clicked");
                                            if self.output_manager.is_recording() {
                                                // Stop recording
                                                if let Err(e) = self.output_manager.stop_recording() {
                                                    log::error!("[Recording] Failed to stop: {}", e);
                                                } else {
                                                    log::info!("[Recording] Stopped");
                                                    // Refresh recordings list to show new file
                                                    self.control_panel.refresh_recordings();
                                                }
                                            } else {
                                                // Start recording - generate default path
                                                let default_name = format!("react_{}.mp4",
                                                    chrono::Local::now().format("%Y%m%d_%H%M%S"));

                                                // Use saved recording directory if available
                                                let recording_dir = self.control_panel.get_recording_directory();

                                                if let Some(dir) = recording_dir {
                                                    // Auto-save to configured directory (no dialog)
                                                    let path = std::path::PathBuf::from(dir).join(&default_name);
                                                    log::info!("[Recording] Auto-saving");
                                                    match self.output_manager.start_recording(path.clone()) {
                                                        Ok(()) => {
                                                            log::info!("[Recording] Started");
                                                        }
                                                        Err(e) => {
                                                            log::error!("[Recording] Failed to start: {}", e);
                                                        }
                                                    }
                                                } else {
                                                    // No directory configured - show file save dialog
                                                    let default_dir = dirs::video_dir()
                                                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                                                    log::info!("[Recording] Opening file dialog...");
                                                    let dialog_result = std::panic::catch_unwind(|| {
                                                        rfd::FileDialog::new()
                                                            .set_title("Save Recording")
                                                            .set_file_name(&default_name)
                                                            .add_filter("MP4 Video", &["mp4"])
                                                            .set_directory(&default_dir)
                                                            .save_file()
                                                    });

                                                    match dialog_result {
                                                        Ok(Some(path)) => {
                                                            if let Err(e) = self.output_manager.start_recording(path.clone()) {
                                                                log::error!("[Recording] Failed to start: {}", e);
                                                            } else {
                                                                log::info!("[Recording] Started");
                                                            }
                                                        }
                                                        Ok(None) => {
                                                            log::info!("[Recording] User cancelled dialog");
                                                        }
                                                        Err(e) => {
                                                            log::error!("[Recording] File dialog panicked: {:?}", e);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Handle MIDI actions from UI (non-blocking)
                                    match midi_action {
                                        MidiAction::None => {}
                                        MidiAction::ArmLearn => {
                                            // Arm learn mode - waiting for user to click a control
                                            self.midi_controller.start_learn();
                                        }
                                        MidiAction::SetLearnTarget(target) => {
                                            // User clicked a control - now waiting for MIDI input
                                            self.midi_controller.set_learn_target(target);
                                        }
                                        MidiAction::CancelLearn => {
                                            self.midi_controller.cancel_learn();
                                        }
                                        MidiAction::RemoveMapping(idx) => {
                                            self.midi_controller.remove_mapping(idx);
                                            // Save to database
                                            if let Some(ref db) = self.db {
                                                let mappings = self.midi_controller.mappings();
                                                let _ = db.clear_midi_mappings();
                                                for mapping in mappings {
                                                    let msg_type = match mapping.key.msg_type {
                                                        crate::midi::MidiMsgType::ControlChange => "CC",
                                                        crate::midi::MidiMsgType::NoteOn => "NoteOn",
                                                        crate::midi::MidiMsgType::NoteOff => "NoteOff",
                                                        crate::midi::MidiMsgType::PitchBend => "PitchBend",
                                                    };
                                                    let target = format!("{:?}", mapping.target);
                                                    let row = crate::db::MidiMappingRow {
                                                        device_name: mapping.key.device_name.clone(),
                                                        channel: mapping.key.channel,
                                                        msg_type: msg_type.to_string(),
                                                        msg_index: mapping.key.index,
                                                        target,
                                                        min_value: mapping.min_value,
                                                        max_value: mapping.max_value,
                                                        inverted: mapping.inverted,
                                                    };
                                                    let _ = db.save_midi_mapping(&row);
                                                }
                                            }
                                        }
                                        MidiAction::RescanDevices => {
                                            self.midi_controller.disconnect_all();
                                            self.midi_controller.connect_all();
                                        }
                                        MidiAction::LearnPresetBrowserStart => {
                                            // Start two-step learn for preset browser
                                            // Step 1: Learn encoder for browsing
                                            self.midi_controller.start_learn();
                                            self.midi_controller.set_learn_target(crate::midi::ControlTarget::BrowsePresetEncoder);
                                            self.learning_preset_browser_step2 = true;
                                            log::info!("[MIDI] Started preset browser learning - step 1: encoder");
                                        }
                                        MidiAction::LearnCompelBrowserStart => {
                                            // Start two-step learn for compel browser
                                            // Step 1: Learn encoder for browsing
                                            self.midi_controller.start_learn();
                                            self.midi_controller.set_learn_target(crate::midi::ControlTarget::BrowseCompelEncoder);
                                            self.learning_compel_browser_step2 = true;
                                            log::info!("[MIDI] Started compel browser learning - step 1: encoder");
                                        }
                                    }

                                    // Handle OSC actions from UI
                                    match osc_action {
                                        OscAction::None => {}
                                        OscAction::SetEnabled(enabled) => {
                                            self.osc_thread.set_enabled(enabled);
                                            if let Some(ref db) = self.db {
                                                let _ = db.set_osc_enabled(enabled);
                                            }
                                        }
                                        OscAction::SetTarget(ip, port) => {
                                            self.osc_thread.set_target(&ip, port);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_osc_config(self.osc_thread.config());
                                            }
                                        }
                                        OscAction::UpdateConfig(config) => {
                                            self.osc_thread.set_config(config.clone());
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_osc_config(&config);
                                            }
                                        }
                                        OscAction::SetLfoBank(bank) => {
                                            self.osc_thread.set_lfo_bank(bank);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_lfo_bank(&bank);
                                            }
                                        }
                                        OscAction::TriggerPress(button) => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerPress(button));
                                        }
                                        OscAction::TriggerRelease(button) => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::TriggerRelease(button));
                                        }
                                        OscAction::SetStrobeRate(hz) => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::SetStrobeRate(hz));
                                        }
                                        OscAction::ToggleFogLatch => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::ToggleFogLatch);
                                        }
                                        OscAction::ToggleUvLatch => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::ToggleUvLatch);
                                        }
                                        OscAction::FetchMaestroData => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::FetchMaestroData);
                                        }
                                        OscAction::SelectMaestroPattern(index) => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::SendManualCue(
                                                "/live/1/pattern/index".to_string(),
                                                vec![rosc::OscType::Int(index)]
                                            ));
                                        }
                                        OscAction::SelectMaestroColorPalette(index) => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::SendManualCue(
                                                "/live/1/palette/index".to_string(),
                                                vec![rosc::OscType::Int(index)]
                                            ));
                                        }
                                        OscAction::SelectMaestroFxPalette(index) => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::SendManualCue(
                                                "/live/1/fx/index".to_string(),
                                                vec![rosc::OscType::Int(index)]
                                            ));
                                        }
                                        OscAction::ForceBuildup => {
                                            self.osc_thread.send_command(crate::osc::OscThreadCommand::ForceBuildup);
                                        }
                                        OscAction::SelectStage(stage_id) => {
                                            match stage_id {
                                                Some(id) => match self.auth_manager.get_token() {
                                                    Some(token) => {
                                                        // Read the stage's edit permission from the cached
                                                        // summary (server-authored; the server enforces it
                                                        // again on every write). Unknown → editable, the
                                                        // pre-split default. Mirror
                                                        // admin status first so a
                                                        // shared stage is editable for
                                                        // an admin even if its cached
                                                        // `editable` flag is stale.
                                                        let admin = self.auth_manager.is_admin();
                                                        self.dmx_manager.set_admin(admin);
                                                        let editable = self
                                                            .dmx_manager
                                                            .available_stages()
                                                            .iter()
                                                            .find(|s| s.id == id)
                                                            .map(|s| s.editable)
                                                            .unwrap_or(true);
                                                        log::info!("[DMX] selected stage {} (editable={})", id, editable);
                                                        self.dmx_manager.select_stage(id, editable, &token);
                                                    }
                                                    None => log::warn!("[DMX] log in to Compeller to load a stage"),
                                                },
                                                None => {
                                                    log::info!("[DMX] cleared selected stage");
                                                    self.dmx_manager.clear_selected_stage();
                                                }
                                            }
                                        }
                                        OscAction::RefreshStageList => {
                                            if let Some(token) = self.auth_manager.get_token() {
                                                let admin = self.auth_manager.is_admin();
                                                self.dmx_manager.set_admin(admin);
                                                self.dmx_manager.refresh_stage_list(&token);
                                            }
                                        }
                                        OscAction::CloneStage(source_id) => {
                                            match self.auth_manager.get_token() {
                                                Some(token) => self.dmx_manager.clone_stage(source_id, &token),
                                                None => log::warn!("[DMX] log in to Compeller to clone a stage"),
                                            }
                                        }
                                        OscAction::LaunchLocalCue(def_key) => {
                                            self.dmx_manager.launch_local_cue(&def_key);
                                        }
                                        OscAction::StopLocalCue(launch_id) => {
                                            self.dmx_manager.stop_local_cue(&launch_id);
                                        }
                                        OscAction::SetDmxBackend(backend, port) => {
                                            self.dmx_manager.set_output_backend(backend, port);
                                        }
                                    }

                                    // Handle Hue actions from UI (commands sent to Hue thread)
                                    // Config persistence is handled by take_pending_save() above.
                                    {
                                        use crate::hue::thread::HueCommand;
                                        match hue_action {
                                            HueAction::None => {}
                                            HueAction::SetEnabled(enabled) => {
                                                self.hue_thread.send_command(HueCommand::SetEnabled(enabled));
                                            }
                                            HueAction::SetBridgeIp(ip) => {
                                                self.hue_thread.send_command(HueCommand::SetBridgeIp(ip));
                                            }
                                            HueAction::StartPairing => {
                                                self.hue_thread.send_command(HueCommand::StartPairing);
                                            }
                                            HueAction::FetchAreas => {
                                                self.hue_thread.send_command(HueCommand::FetchAreas);
                                            }
                                            HueAction::SelectArea(area_id) => {
                                                self.hue_thread.send_command(HueCommand::SetEntertainmentArea(area_id));
                                            }
                                            HueAction::SetLightRole(channel_id, role) => {
                                                self.hue_thread.send_command(HueCommand::SetLightRole(channel_id, role));
                                            }
                                            HueAction::SetLightFormFactor(channel_id, form_factor) => {
                                                self.hue_thread.send_command(HueCommand::SetLightFormFactor(channel_id, form_factor));
                                            }
                                            HueAction::Connect => {
                                                self.hue_thread.send_command(HueCommand::Connect);
                                            }
                                            HueAction::Disconnect => {
                                                self.hue_thread.send_command(HueCommand::Disconnect);
                                            }
                                            HueAction::StoreCredentials(username, clientkey) => {
                                                self.hue_thread.send_command(HueCommand::StoreCredentials { username, clientkey });
                                            }
                                            HueAction::SetProtocolVersion(version) => {
                                                self.hue_thread.send_command(HueCommand::SetProtocolVersion(version));
                                            }
                                            HueAction::SetMappingMode(mode) => {
                                                self.hue_thread.send_command(HueCommand::SetMappingMode(mode));
                                            }
                                        }
                                    }

                                    // Handle WLED actions from UI
                                    match wled_action {
                                        WledAction::None => {}
                                        WledAction::SetEnabled(enabled) => {
                                            self.wled_manager.set_enabled(enabled);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_wled_config(self.wled_manager.config());
                                            }
                                        }
                                        WledAction::AddStrip(ip) => {
                                            self.wled_manager.add_strip(ip);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_wled_config(self.wled_manager.config());
                                            }
                                        }
                                        WledAction::RemoveStrip(idx) => {
                                            self.wled_manager.remove_strip(idx);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_wled_config(self.wled_manager.config());
                                            }
                                        }
                                        WledAction::SetStripMode(idx, mode) => {
                                            self.wled_manager.set_strip_mode(idx, mode);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_wled_config(self.wled_manager.config());
                                            }
                                        }
                                        WledAction::SetStripFormFactor(idx, form_factor) => {
                                            self.wled_manager.set_strip_form_factor(idx, form_factor);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_wled_config(self.wled_manager.config());
                                            }
                                        }
                                        WledAction::SetStripEnabled(idx, enabled) => {
                                            self.wled_manager.set_strip_enabled(idx, enabled);
                                            if let Some(ref db) = self.db {
                                                let _ = db.save_wled_config(self.wled_manager.config());
                                            }
                                        }
                                        WledAction::Connect(idx) => {
                                            self.wled_manager.connect(idx);
                                        }
                                        WledAction::Disconnect(idx) => {
                                            self.wled_manager.disconnect(idx);
                                        }
                                        WledAction::ConnectAll => {
                                            self.wled_manager.connect_all();
                                        }
                                        WledAction::DisconnectAll => {
                                            self.wled_manager.disconnect_all();
                                        }
                                    }

                                    // Handle camera actions from UI
                                    match camera_action {
                                        CameraAction::None => {}
                                        CameraAction::Start => {
                                            if let Err(e) = self.camera_manager.start() {
                                                log::error!("[Camera] Failed to start: {}", e);
                                            }
                                        }
                                        CameraAction::Stop => {
                                            self.camera_manager.stop();
                                        }
                                        CameraAction::ScanCameras => {
                                            if let Err(e) = self.camera_manager.scan_cameras() {
                                                log::error!("[Camera] Failed to scan: {}", e);
                                            }
                                        }
                                        CameraAction::SetProgramCamera(idx) => {
                                            self.camera_manager.set_program_camera(idx);
                                        }
                                        CameraAction::SetEnabled(idx, enabled) => {
                                            // V1: we just set the program camera
                                            if enabled {
                                                self.camera_manager.set_program_camera(idx);
                                            }
                                        }
                                        CameraAction::SetTrackFilter(filter) => {
                                            self.camera_manager.set_track_filter(filter);
                                        }
                                    }

                                    // Handle auth actions from UI
                                    match auth_action {
                                        crate::ui::AuthAction::None => {}
                                        crate::ui::AuthAction::Login { username, password } => {
                                            if let Some(ref db) = self.db {
                                                self.auth_manager.login(
                                                    crate::auth::LoginCredentials { username, password },
                                                    db.clone(),
                                                );
                                            }
                                        }
                                        crate::ui::AuthAction::GoogleSignIn => {
                                            // Browser handoff: opens the web login (Google
                                            // GSI or password); the unconditional frame-loop
                                            // poll below the menu completes the session.
                                            self.auth_manager.start_desktop_signin();
                                        }
                                        crate::ui::AuthAction::Logout => {
                                            if let Some(ref db) = self.db {
                                                self.auth_manager.logout(db);
                                            }
                                            // Lighting requires sign-in: stop the outputs on an
                                            // EXPLICIT sign-out (operator intent — unlike a
                                            // transient token failure, which never touches a
                                            // running rig). The section UI gate + boot-restore
                                            // gate handle re-entry.
                                            self.osc_thread.set_enabled(false);
                                            self.hue_thread.send_command(
                                                crate::hue::thread::HueCommand::SetEnabled(false),
                                            );
                                            self.wled_manager.set_enabled(false);
                                            // Reset YouTube state on logout
                                            self.youtube_auth.reset();
                                            self.youtube_eligibility.reset();
                                            self.youtube_control_plane.set_compeller_token(None);
                                            self.youtube_session = None;
                                        }
                                    }

                                    // Sync YouTube components when auth state changes to Authed
                                    if self.auth_manager.is_authenticated() {
                                        let token = self.auth_manager.get_token();
                                        // Only set token if it changed (avoid unnecessary updates)
                                        if token.is_some() {
                                            self.youtube_auth.set_compeller_token(token.clone());
                                            self.youtube_eligibility.set_compeller_token(token.clone());
                                            self.youtube_control_plane.set_compeller_token(token);
                                        }
                                    }

                                    // Poll YouTube auth status during OAuth flow
                                    if self.youtube_auth.should_poll() {
                                        self.youtube_auth.check_status();
                                    }

                                    // Check eligibility when YouTube is connected
                                    if self.youtube_auth.is_ready() && self.youtube_eligibility.should_check() {
                                        self.youtube_eligibility.check();
                                    }

                                    // Drive the YouTube session state machine
                                    if let Some(ref session) = self.youtube_session {
                                        // Execute creation steps if in Creating state
                                        self.youtube_control_plane.execute_creation_step(session);

                                        // Poll status if needed
                                        if session.should_poll() {
                                            self.youtube_control_plane.poll_status(session);
                                        }

                                        // Handle RTMP data plane based on session state
                                        let session_state = session.get_state();
                                        match session_state {
                                            crate::youtube::session::SessionState::Bound => {
                                                // Session is bound, start RTMP if not already streaming
                                                if self.youtube_rtmp_streamer.is_none() {
                                                    if let Some(rtmp_url) = session.get_rtmp_url() {
                                                        let frame_ring = self.output_manager.get_frame_ring();
                                                        let audio_sample_rate = self.audio_capture.lock().get_detected_sample_rate();

                                                        // Get output mode from YouTube session config
                                                        let output_mode = session.get_output_mode();
                                                        log::info!("[YouTube] Starting GStreamer stream: {}", output_mode.label());

                                                        match crate::output::GStreamerStreamer::new(frame_ring, output_mode, crate::output::StreamPlatform::YouTube, audio_sample_rate) {
                                                            Ok(mut streamer) => {
                                                                if let Err(e) = streamer.start(rtmp_url) {
                                                                    log::error!("[YouTube] Failed to start RTMP: {}", e);
                                                                    session.fail("Failed to start RTMP streaming".to_string());
                                                                } else {
                                                                    session.start_connecting();
                                                                    self.output_manager.add_rtmp_drain_target(streamer.get_audio_buffer_arc(), self.youtube_audio_gain.clone());
                                                                    self.youtube_rtmp_streamer = Some(streamer);
                                                                }
                                                            }
                                                            Err(e) => {
                                                                log::error!("[YouTube] Failed to create GStreamer streamer: {}", e);
                                                                session.fail(format!("Failed to create streamer: {}", e));
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            crate::youtube::session::SessionState::Connecting |
                                            crate::youtube::session::SessionState::Pushing |
                                            crate::youtube::session::SessionState::Testing |
                                            crate::youtube::session::SessionState::Live => {
                                                // Check GStreamer streamer health
                                                if let Some(ref streamer) = self.youtube_rtmp_streamer {
                                                    let stream_state = streamer.get_state();
                                                    log::debug!("[YouTube] Session: {:?}, Stream state: {:?}", session_state, stream_state);
                                                    match stream_state {
                                                        crate::output::GStreamerStreamState::Streaming => {
                                                            // All good - if still in Connecting, move to Pushing
                                                            if matches!(session_state, crate::youtube::session::SessionState::Connecting) {
                                                                log::info!("[YouTube] GStreamer is Streaming, calling rtmp_connected()");
                                                                session.rtmp_connected();
                                                            }
                                                            // If in Pushing, skip Testing and go directly to Live
                                                            // (YouTube handles auto-start, no polling needed)
                                                            if matches!(session_state, crate::youtube::session::SessionState::Pushing) {
                                                                log::info!("[YouTube] RTMP streaming, transitioning directly to Live");
                                                                // Skip testing - go straight to live since auto-start is enabled
                                                                if session.transition_to_testing() {
                                                                    // Immediately transition to live
                                                                    let _ = session.transition_to_live();
                                                                }
                                                            }
                                                            // If in Testing, transition to Live immediately
                                                            if matches!(session_state, crate::youtube::session::SessionState::Testing) {
                                                                log::info!("[YouTube] In Testing, transitioning to Live");
                                                                let _ = session.transition_to_live();
                                                            }
                                                        }
                                                        crate::output::GStreamerStreamState::Error => {
                                                            // Stream failed
                                                            log::error!("[YouTube] Stream state is Error");
                                                            session.fail("RTMP connection lost".to_string());
                                                            if let Some(mut s) = self.youtube_rtmp_streamer.take() {
                                                                let _ = s.stop();
                                                                self.output_manager.clear_rtmp_drain_targets();
                                                                if let Some(ref t) = self.twitch_ffmpeg_streamer {
                                                                    self.output_manager.add_rtmp_drain_target(t.get_audio_buffer_arc(), self.twitch_audio_gain.clone());
                                                                }
                                                            }
                                                        }
                                                        _ => {
                                                            log::debug!("[YouTube] Stream state: {:?}, waiting...", stream_state);
                                                        }
                                                    }
                                                } else {
                                                    log::warn!("[YouTube] Session is {:?} but no streamer!", session_state);
                                                }
                                            }
                                            crate::youtube::session::SessionState::Ending |
                                            crate::youtube::session::SessionState::Ended |
                                            crate::youtube::session::SessionState::Failed(_) |
                                            crate::youtube::session::SessionState::Idle => {
                                                // Stop RTMP if running
                                                if let Some(mut streamer) = self.youtube_rtmp_streamer.take() {
                                                    let _ = streamer.stop();
                                                    self.output_manager.clear_rtmp_drain_targets();
                                                    if let Some(ref s) = self.twitch_ffmpeg_streamer {
                                                        self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.twitch_audio_gain.clone());
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }

                                    // Handle YouTube actions from UI
                                    match youtube_action {
                                        crate::ui::YouTubeAction::None => {}
                                        crate::ui::YouTubeAction::ConnectYouTube => {
                                            self.youtube_auth.start_connect();
                                        }
                                        crate::ui::YouTubeAction::DisconnectYouTube => {
                                            // Stop RTMP streamer if running
                                            if let Some(mut streamer) = self.youtube_rtmp_streamer.take() {
                                                let _ = streamer.stop();
                                                self.output_manager.clear_rtmp_drain_targets();
                                                if let Some(ref s) = self.twitch_ffmpeg_streamer {
                                                    self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.twitch_audio_gain.clone());
                                                }
                                            }
                                            self.youtube_auth.disconnect();
                                            self.youtube_eligibility.reset();
                                            self.youtube_session = None;
                                        }
                                        crate::ui::YouTubeAction::GoLive { title, config } => {
                                            // Use quality preset for resolution, frame rate, and bitrate
                                            let resolution = config.quality.resolution();
                                            let frame_rate = config.quality.frame_rate();
                                            let bitrate = config.quality.bitrate_kbps();

                                            // Log actual dimensions from NDI for reference
                                            let ndi_res = self.control_panel.ndi_resolution();
                                            let (width, height) = ndi_res.dimensions();

                                            log::info!("[YouTube] GoLive: {}x{} -> {} @ {} ({}kbps)",
                                                width, height, resolution, frame_rate, bitrate);

                                            // Create a new session if none exists
                                            if self.youtube_session.is_none() {
                                                let session = crate::youtube::session::LiveSession::new();
                                                session.set_title(title);
                                                session.set_resolution(resolution.to_string());
                                                session.set_frame_rate(frame_rate.to_string());
                                                session.set_bitrate_kbps(bitrate);
                                                session.set_privacy(config.privacy.api_value().to_string());
                                                if session.start() {
                                                    self.youtube_session = Some(session);
                                                }
                                            } else if let Some(ref session) = self.youtube_session {
                                                // Try to start existing session (if Idle)
                                                session.set_title(title);
                                                session.set_resolution(resolution.to_string());
                                                session.set_frame_rate(frame_rate.to_string());
                                                session.set_bitrate_kbps(bitrate);
                                                session.set_privacy(config.privacy.api_value().to_string());
                                                session.start();
                                            }
                                        }
                                        crate::ui::YouTubeAction::StopLive => {
                                            // Stop RTMP streamer first
                                            if let Some(mut streamer) = self.youtube_rtmp_streamer.take() {
                                                let _ = streamer.stop();
                                                self.output_manager.clear_rtmp_drain_targets();
                                                if let Some(ref s) = self.twitch_ffmpeg_streamer {
                                                    self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.twitch_audio_gain.clone());
                                                }
                                            }
                                            // Then stop the session
                                            if let Some(ref session) = self.youtube_session {
                                                if session.stop() {
                                                    self.youtube_control_plane.end_broadcast(session);
                                                }
                                            }
                                        }
                                        crate::ui::YouTubeAction::RetryLive => {
                                            if let Some(ref session) = self.youtube_session {
                                                session.retry();
                                            }
                                        }
                                        crate::ui::YouTubeAction::Dismiss => {
                                            if let Some(ref session) = self.youtube_session {
                                                session.reset_to_idle();
                                            }
                                        }
                                    }

                                    // Handle Twitch ffmpeg-next streamer health monitoring
                                    if let Some(ref streamer) = self.twitch_ffmpeg_streamer {
                                        let stats = streamer.get_stats();
                                        let current_state = self.control_panel.get_twitch_state().clone();

                                        match stats.state {
                                            crate::output::GStreamerStreamState::Error => {
                                                let err_detail = stats.error_message.unwrap_or_else(|| "Unknown error".to_string());
                                                log::error!("[Twitch] Stream error: {}", err_detail);
                                                self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Error(err_detail));
                                                if let Some(mut s) = self.twitch_ffmpeg_streamer.take() {
                                                    let _ = s.stop();
                                                }
                                            }
                                            crate::output::GStreamerStreamState::Streaming => {
                                                // Only transition to Streaming after frames are actually being sent
                                                // This confirms the RTMP connection is working
                                                if matches!(current_state, crate::ui::TwitchStreamState::Connecting) {
                                                    // Log periodically for debugging
                                                    if stats.video_frames_sent > 0 && stats.video_frames_sent % 60 == 0 {
                                                        log::info!("[Twitch] Connecting... {} video frames sent", stats.video_frames_sent);
                                                    }

                                                    if stats.video_frames_sent > 30 {
                                                        // At least ~1 second of frames sent at 30fps - connection confirmed
                                                        log::info!("[Twitch] Connection confirmed, {} video frames sent", stats.video_frames_sent);
                                                        self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Streaming);
                                                    } else if let Some(elapsed) = self.control_panel.get_twitch_elapsed_secs() {
                                                        // Timeout: if connecting for >15 seconds without streaming, fail
                                                        if elapsed > 15.0 && stats.video_frames_sent < 10 {
                                                            log::error!("[Twitch] Connection timeout - {} frames in {:.1}s", stats.video_frames_sent, elapsed);
                                                            self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Error("Connection timeout - check stream key and network".to_string()));
                                                            if let Some(mut s) = self.twitch_ffmpeg_streamer.take() {
                                                                let _ = s.stop();
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            crate::output::GStreamerStreamState::Stopped => {
                                                // Streamer stopped unexpectedly
                                                if matches!(current_state, crate::ui::TwitchStreamState::Connecting | crate::ui::TwitchStreamState::Streaming) {
                                                    log::warn!("[Twitch] Stream stopped unexpectedly");
                                                    self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Error("Stream stopped unexpectedly".to_string()));
                                                    if let Some(mut s) = self.twitch_ffmpeg_streamer.take() {
                                                        let _ = s.stop();
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }

                                    // Handle Twitch actions from UI
                                    match twitch_action {
                                        crate::ui::TwitchAction::None => {}
                                        crate::ui::TwitchAction::GoLive { stream_key, output_mode, channel, title, category, tags } => {
                                            if self.twitch_ffmpeg_streamer.is_none() {
                                                log::info!("[Twitch] Starting GStreamer stream: {}", output_mode.label());

                                                // Save stream info for next time
                                                if let Some(ref db) = self.db {
                                                    let _ = db.set_twitch_stream_key(&stream_key);
                                                    let _ = db.set_twitch_channel(&channel);
                                                    let _ = db.set_twitch_stream_title(&title);
                                                    let _ = db.set_twitch_stream_category(&category);
                                                    let _ = db.set_twitch_stream_tags(&tags);
                                                }

                                                let frame_ring = self.output_manager.get_frame_ring();
                                                let audio_sample_rate = self.audio_capture.lock().get_detected_sample_rate();

                                                // Use best ingest server if available
                                                let ingest_host = crate::output::find_best_available_server();
                                                let rtmp_url = format!("rtmp://{}/app/{}", ingest_host, stream_key);
                                                log::info!("[Twitch] Using ingest: {}", ingest_host);

                                                match crate::output::GStreamerStreamer::new(frame_ring, output_mode, crate::output::StreamPlatform::Twitch, audio_sample_rate) {
                                                    Ok(mut streamer) => {
                                                        if let Err(e) = streamer.start(rtmp_url) {
                                                            log::error!("[Twitch] Failed to start RTMP: {}", e);
                                                            self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Error(e.to_string()));
                                                        } else {
                                                            log::info!("[Twitch] GStreamer streamer started, connecting...");
                                                            self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Connecting);
                                                            self.output_manager.add_rtmp_drain_target(streamer.get_audio_buffer_arc(), self.twitch_audio_gain.clone());
                                                            self.twitch_ffmpeg_streamer = Some(streamer);
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::error!("[Twitch] Failed to create GStreamer streamer: {}", e);
                                                        self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Error(e.to_string()));
                                                    }
                                                }
                                            }
                                        }
                                        crate::ui::TwitchAction::StopLive => {
                                            if let Some(mut streamer) = self.twitch_ffmpeg_streamer.take() {
                                                log::info!("[Twitch] Stopping stream");
                                                let _ = streamer.stop();
                                            }
                                            self.output_manager.clear_rtmp_drain_targets();
                                            // Re-add other streamers if still active
                                            if let Some(ref s) = self.youtube_rtmp_streamer {
                                                self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.youtube_audio_gain.clone());
                                            }
                                            if let Some(ref s) = self.kick_ffmpeg_streamer {
                                                self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.kick_audio_gain.clone());
                                            }
                                            self.control_panel.set_twitch_state(crate::ui::TwitchStreamState::Idle);
                                        }
                                    }

                                    // Handle Kick streamer health monitoring (mirrors Twitch;
                                    // Kick reuses TwitchStreamState for its lifecycle)
                                    if let Some(ref streamer) = self.kick_ffmpeg_streamer {
                                        let stats = streamer.get_stats();
                                        let current_state = self.control_panel.get_kick_state().clone();

                                        match stats.state {
                                            crate::output::GStreamerStreamState::Error => {
                                                let err_detail = stats.error_message.unwrap_or_else(|| "Unknown error".to_string());
                                                log::error!("[Kick] Stream error: {}", err_detail);
                                                self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Error(err_detail));
                                                if let Some(mut s) = self.kick_ffmpeg_streamer.take() {
                                                    let _ = s.stop();
                                                }
                                            }
                                            crate::output::GStreamerStreamState::Streaming => {
                                                // Only transition to Streaming after frames are actually being sent
                                                if matches!(current_state, crate::ui::TwitchStreamState::Connecting) {
                                                    if stats.video_frames_sent > 0 && stats.video_frames_sent % 60 == 0 {
                                                        log::info!("[Kick] Connecting... {} video frames sent", stats.video_frames_sent);
                                                    }

                                                    if stats.video_frames_sent > 30 {
                                                        log::info!("[Kick] Connection confirmed, {} video frames sent", stats.video_frames_sent);
                                                        self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Streaming);
                                                    } else if let Some(elapsed) = self.control_panel.get_kick_elapsed_secs() {
                                                        if elapsed > 15.0 && stats.video_frames_sent < 10 {
                                                            log::error!("[Kick] Connection timeout - {} frames in {:.1}s", stats.video_frames_sent, elapsed);
                                                            self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Error("Connection timeout - check Stream URL/Key and network".to_string()));
                                                            if let Some(mut s) = self.kick_ffmpeg_streamer.take() {
                                                                let _ = s.stop();
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            crate::output::GStreamerStreamState::Stopped => {
                                                if matches!(current_state, crate::ui::TwitchStreamState::Connecting | crate::ui::TwitchStreamState::Streaming) {
                                                    log::warn!("[Kick] Stream stopped unexpectedly");
                                                    self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Error("Stream stopped unexpectedly".to_string()));
                                                    if let Some(mut s) = self.kick_ffmpeg_streamer.take() {
                                                        let _ = s.stop();
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }

                                    // Handle Kick actions from UI
                                    match kick_action {
                                        crate::ui::KickAction::None => {}
                                        crate::ui::KickAction::GoLive { stream_url, stream_key, output_mode, title } => {
                                            if self.kick_ffmpeg_streamer.is_none() {
                                                log::info!("[Kick] Starting GStreamer stream: {}", output_mode.label());

                                                // Save stream info for next time
                                                if let Some(ref db) = self.db {
                                                    let _ = db.set_kick_stream_url(&stream_url);
                                                    let _ = db.set_kick_stream_key(&stream_key);
                                                    let _ = db.set_kick_stream_title(&title);
                                                }

                                                let frame_ring = self.output_manager.get_frame_ring();
                                                let audio_sample_rate = self.audio_capture.lock().get_detected_sample_rate();

                                                // Kick/AWS-IVS ingest is rtmps://<host>/app/<stream-key>.
                                                // Dashboards (and OBS' "Server" field) often show only the
                                                // host, so the pasted URL frequently has no application path.
                                                // RTMP can't publish without an application — without "/app"
                                                // rtmp2sink reads the key as the app name and publishes an
                                                // empty stream, which IVS drops. If the URL has no path,
                                                // default the application to "app" (IVS's only app name).
                                                let trimmed = stream_url.trim().trim_end_matches('/');
                                                let has_app_path = trimmed
                                                    .splitn(2, "://")
                                                    .nth(1)
                                                    .map(|host_path| host_path.contains('/'))
                                                    .unwrap_or(false);
                                                let base = if has_app_path {
                                                    trimmed.to_string()
                                                } else {
                                                    log::warn!("[Kick] Stream URL has no application path — defaulting to /app");
                                                    format!("{}/app", trimmed)
                                                };
                                                let rtmp_url = format!("{}/{}", base, stream_key.trim());
                                                log::info!("[Kick] Using ingest: {}", base);

                                                match crate::output::GStreamerStreamer::new(frame_ring, output_mode, crate::output::StreamPlatform::Kick, audio_sample_rate) {
                                                    Ok(mut streamer) => {
                                                        if let Err(e) = streamer.start(rtmp_url) {
                                                            log::error!("[Kick] Failed to start RTMP: {}", e);
                                                            self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Error(e.to_string()));
                                                        } else {
                                                            log::info!("[Kick] GStreamer streamer started, connecting...");
                                                            self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Connecting);
                                                            self.output_manager.add_rtmp_drain_target(streamer.get_audio_buffer_arc(), self.kick_audio_gain.clone());
                                                            self.kick_ffmpeg_streamer = Some(streamer);
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::error!("[Kick] Failed to create GStreamer streamer: {}", e);
                                                        self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Error(e.to_string()));
                                                    }
                                                }
                                            }
                                        }
                                        crate::ui::KickAction::StopLive => {
                                            if let Some(mut streamer) = self.kick_ffmpeg_streamer.take() {
                                                log::info!("[Kick] Stopping stream");
                                                let _ = streamer.stop();
                                            }
                                            self.output_manager.clear_rtmp_drain_targets();
                                            // Re-add other streamers if still active
                                            if let Some(ref s) = self.youtube_rtmp_streamer {
                                                self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.youtube_audio_gain.clone());
                                            }
                                            if let Some(ref s) = self.twitch_ffmpeg_streamer {
                                                self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.twitch_audio_gain.clone());
                                            }
                                            self.control_panel.set_kick_state(crate::ui::TwitchStreamState::Idle);
                                        }
                                    }

                                    // Handle NDI streaming actions (Twitch, YouTube, or both)
                                    // Uses NDI-based streaming for perfect A/V sync
                                    match dual_stream_action {
                                        crate::ui::DualStreamAction::None => {
                                            // Update UI state based on active streamer status
                                            // Handle Twitch (GStreamer) streamer state
                                            if let Some(ref s) = self.twitch_ffmpeg_streamer {
                                                match s.get_state() {
                                                    crate::output::GStreamerStreamState::Starting => {
                                                        self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Connecting(crate::ui::StreamTarget::Twitch));
                                                    }
                                                    crate::output::GStreamerStreamState::Streaming => {
                                                        self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Streaming(crate::ui::StreamTarget::Twitch));
                                                    }
                                                    crate::output::GStreamerStreamState::Error => {
                                                        if !matches!(self.control_panel.get_dual_stream_state(), crate::ui::DualStreamState::Error(_) | crate::ui::DualStreamState::Idle) {
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Error("Stream error".to_string()));
                                                        }
                                                        self.twitch_ffmpeg_streamer = None;
                                                        self.output_manager.clear_rtmp_drain_targets();
                                                        if let Some(ref s) = self.youtube_rtmp_streamer {
                                                            self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.youtube_audio_gain.clone());
                                                        }
                                                    }
                                                    crate::output::GStreamerStreamState::Stopped | crate::output::GStreamerStreamState::Idle => {
                                                        if matches!(self.control_panel.get_dual_stream_state(), crate::ui::DualStreamState::Streaming(_) | crate::ui::DualStreamState::Connecting(_)) {
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Idle);
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            // Handle YouTube (GStreamer) streamer
                                            else if let Some(ref s) = self.youtube_rtmp_streamer {
                                                match s.get_state() {
                                                    crate::output::GStreamerStreamState::Starting => {
                                                        self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Connecting(crate::ui::StreamTarget::YouTube));
                                                    }
                                                    crate::output::GStreamerStreamState::Streaming => {
                                                        self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Streaming(crate::ui::StreamTarget::YouTube));
                                                    }
                                                    crate::output::GStreamerStreamState::Error => {
                                                        if !matches!(self.control_panel.get_dual_stream_state(), crate::ui::DualStreamState::Error(_) | crate::ui::DualStreamState::Idle) {
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Error("Stream error".to_string()));
                                                        }
                                                        self.youtube_rtmp_streamer = None;
                                                        self.output_manager.clear_rtmp_drain_targets();
                                                        if let Some(ref s) = self.twitch_ffmpeg_streamer {
                                                            self.output_manager.add_rtmp_drain_target(s.get_audio_buffer_arc(), self.twitch_audio_gain.clone());
                                                        }
                                                    }
                                                    crate::output::GStreamerStreamState::Stopped | crate::output::GStreamerStreamState::Idle => {
                                                        if matches!(self.control_panel.get_dual_stream_state(), crate::ui::DualStreamState::Streaming(_) | crate::ui::DualStreamState::Connecting(_)) {
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Idle);
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        crate::ui::DualStreamAction::StartTwitch { twitch_stream_key, output_mode, channel, title, category, tags } => {
                                            if self.twitch_ffmpeg_streamer.is_none() {
                                                log::info!("[Live Stream] Starting Twitch GStreamer stream: {} ({})", output_mode.label(), output_mode.description());

                                                // Save stream info for next time
                                                if let Some(ref db) = self.db {
                                                    let _ = db.set_twitch_stream_key(&twitch_stream_key);
                                                    let _ = db.set_twitch_channel(&channel);
                                                    let _ = db.set_twitch_stream_title(&title);
                                                    let _ = db.set_twitch_stream_category(&category);
                                                    let _ = db.set_twitch_stream_tags(&tags);
                                                }
                                                self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Connecting(crate::ui::StreamTarget::Twitch));

                                                let frame_ring = self.output_manager.get_frame_ring();
                                                let audio_sample_rate = self.audio_capture.lock().get_detected_sample_rate();
                                                log::info!("[Live Stream] Audio sample rate: {}Hz", audio_sample_rate);

                                                // Create GStreamer streamer with user-selected output mode
                                                match crate::output::GStreamerStreamer::new(frame_ring, output_mode, crate::output::StreamPlatform::Twitch, audio_sample_rate) {
                                                    Ok(mut streamer) => {
                                                        // Use best ingest server if available, fallback to live.twitch.tv
                                                        let ingest_host = {
                                                            let selected = crate::output::get_selected_twitch_server();
                                                            if selected.is_empty() { "live.twitch.tv".to_string() } else { selected }
                                                        };
                                                        let twitch_url = format!("rtmp://{}/app/{}", ingest_host, twitch_stream_key);
                                                        log::info!("[Live Stream] Using Twitch ingest: {}", ingest_host);
                                                        if let Err(e) = streamer.start(twitch_url) {
                                                            log::error!("[Live Stream] Twitch start failed: {}", e);
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Error(format!("Failed: {}", e)));
                                                        } else {
                                                            self.twitch_ffmpeg_streamer = Some(streamer);
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Streaming(crate::ui::StreamTarget::Twitch));
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::error!("[Live Stream] Failed to create GStreamer streamer: {}", e);
                                                        self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Error(format!("Failed: {}", e)));
                                                    }
                                                }
                                            }
                                        }
                                        crate::ui::DualStreamAction::StartYouTube { youtube_rtmp_url } => {
                                            if self.youtube_rtmp_streamer.is_none() {
                                                log::info!("[Live Stream] Starting YouTube-only stream via GStreamer");
                                                self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Connecting(crate::ui::StreamTarget::YouTube));
                                                let frame_ring = self.output_manager.get_frame_ring();
                                                let audio_sample_rate = self.audio_capture.lock().get_detected_sample_rate();
                                                let output_mode = crate::output::OutputMode::P2160_30; // 4K30 for YouTube

                                                match crate::output::GStreamerStreamer::new(frame_ring, output_mode, crate::output::StreamPlatform::YouTube, audio_sample_rate) {
                                                    Ok(mut streamer) => {
                                                        if let Err(e) = streamer.start(youtube_rtmp_url) {
                                                            log::error!("[Live Stream] YouTube start failed: {}", e);
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Error(format!("Failed: {}", e)));
                                                        } else {
                                                            self.output_manager.add_rtmp_drain_target(streamer.get_audio_buffer_arc(), self.youtube_audio_gain.clone());
                                                            self.youtube_rtmp_streamer = Some(streamer);
                                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Streaming(crate::ui::StreamTarget::YouTube));
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::error!("[Live Stream] Failed to create GStreamer streamer: {}", e);
                                                        self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Error(format!("Failed: {}", e)));
                                                    }
                                                }
                                            }
                                        }
                                        crate::ui::DualStreamAction::StopStream => {
                                            // Stop any active streamer
                                            if let Some(mut streamer) = self.twitch_ffmpeg_streamer.take() {
                                                log::info!("[Live Stream] Stopping Twitch stream");
                                                let _ = streamer.stop();
                                            }
                                            if let Some(mut streamer) = self.youtube_rtmp_streamer.take() {
                                                log::info!("[Live Stream] Stopping YouTube stream");
                                                let _ = streamer.stop();
                                            }
                                            self.output_manager.clear_rtmp_drain_targets();
                                            self.control_panel.set_dual_stream_state(crate::ui::DualStreamState::Idle);
                                        }
                                    }

                                    // Save Twitch settings if changed
                                    if self.control_panel.is_twitch_settings_dirty() {
                                        if let Some(ref db) = self.db {
                                            let (channel, key, title, category, tags) = self.control_panel.get_twitch_settings();
                                            let _ = db.set_twitch_channel(&channel);
                                            let _ = db.set_twitch_stream_key(&key);
                                            let _ = db.set_twitch_stream_title(&title);
                                            let _ = db.set_twitch_stream_category(&category);
                                            let _ = db.set_twitch_stream_tags(&tags);
                                        }
                                        self.control_panel.clear_twitch_settings_dirty();
                                    }

                                    // Save Kick settings if changed
                                    if self.control_panel.is_kick_settings_dirty() {
                                        if let Some(ref db) = self.db {
                                            let (url, key, title) = self.control_panel.get_kick_settings();
                                            let _ = db.set_kick_stream_url(&url);
                                            let _ = db.set_kick_stream_key(&key);
                                            let _ = db.set_kick_stream_title(&title);
                                        }
                                        self.control_panel.clear_kick_settings_dirty();
                                    }

                                    // Sync shader HUD with UI checkbox (in case user toggled via menu)
                                    let ui_hud_visible = self.control_panel.is_chroma_hud_visible();
                                    if let Some(ref mut p) = *self.render_pipeline.lock() {
                                        let shader_hud_visible = p.is_symmetry_hud_visible();
                                        if shader_hud_visible != ui_hud_visible {
                                            p.toggle_symmetry_hud();
                                        }
                                    }
                                });

                            // Show hint at bottom
                            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new("Press M to hide menu").small().color(egui::Color32::from_rgb(100, 100, 120)));
                            });
                        });
                });
        }

        // Render Compel Manager window (floating window, independent of menu)
        if !self.show_landing {
            let mut pipeline_guard = self.render_pipeline.lock();
            self.control_panel.render_compel_manager(ctx, &mut *pipeline_guard);
        }

        // Render Recording Manager window (floating window, independent of menu)
        if !self.show_landing {
            self.control_panel.render_recording_manager(ctx);
        }

        // Render Camera Preview window (floating window, independent of menu)
        if !self.show_landing {
            let camera_state = CameraState {
                enabled: self.camera_manager.is_active(),
                available_cameras: self.camera_manager.available_cameras().to_vec(),
                enabled_cameras: vec![self.camera_manager.program_camera()],
                program_camera: self.camera_manager.program_camera(),
                output: self.camera_manager.get_output(),
                camera_track_counts: self.camera_manager.get_camera_states(),
                preview: self.camera_manager.get_preview_frame(),
            };
            self.control_panel.render_camera_preview(ctx, Some(&camera_state));
        }

        // Render Track Management window (floating window, independent of menu)
        if !self.show_landing {
            self.control_panel.render_track_management_window(ctx);
        }

        // Update theme/geometry every frame (regardless of menu visibility)
        // This ensures the shader HUD and animations keep running even when menu is hidden
        if !self.show_landing {
            let analysis = self.audio_analysis.lock().clone();
            let dt = 1.0 / 60.0;
            if let Some(ref mut p) = *self.render_pipeline.lock() {
                if !self.control_panel.is_frozen() {
                    let theme_audio = crate::render::AudioAnalysis::with_chroma(
                        Vec::new(),
                        analysis.bass,
                        analysis.mid,
                        analysis.high,
                        analysis.kick,
                        analysis.energy,
                        analysis.energy_vis,
                        analysis.chroma,
                        analysis.display_chroma,
                        analysis.is_tonal,
                        analysis.dominant_pc,
                        analysis.tonal_separation,
                        analysis.tonal_energy,
                        analysis.stability,
                        analysis.band_centroids,
                        analysis.raw_bass,
                        analysis.raw_mid,
                        analysis.raw_high,
                        analysis.onset_strength,
                    );
                    p.update_theme(&theme_audio, dt);

                    // Send lighting input every frame (runs regardless of menu visibility).
                    // key_signal and lighting_metrics are optional — fallback derives
                    // key/pitch/polarity from AudioAnalysis with latching when HG not active.
                    //
                    // Drive every ENABLED output, NO entitlement factor here. The old
                    // `lighting_licensed && (...)` term made this gate the ONLY
                    // enforcement of the lighting entitlement in the app — the UI
                    // happily enables DMX/Hue/WLED on a FREE plan — so its sole
                    // real effect was silently freezing the feed to an enabled rig
                    // (measured 2026-06-12: plan=FREE + preview closed froze the
                    // DMX wire AND starved the engine of input — the second gate
                    // behind "triggers need audio"). It also meant a transient
                    // entitlement degrade (token-refresh hiccup mid-show) would
                    // freeze a 24/7 rig. License enforcement belongs at the
                    // output enable/configure surface, with messaging — not here.
                    let virtual_stage_active = self.dmx_manager.virtual_stage_enabled();
                    let any_lighting_active = virtual_stage_active
                        || self.osc_thread.is_enabled()
                        || self.hue_thread.is_enabled()
                        || self.wled_manager.is_enabled()
                        || self.dmx_manager.is_enabled();
                    if any_lighting_active {
                        let key_signal = p.get_key_detector()
                            .map(|kd| crate::emerge::KeySignal::from_key_detector(kd));
                        let lighting_metrics = p.get_lighting_metrics();

                        // Last-published CueSnapshot (carries this loop's
                        // frame_time/kick for the clock resolve; the OSC thread
                        // republishes async after send_input).
                        let snapshot = self.osc_thread.snapshot();

                        // COM-499 Phase B2: resolve ONE authoritative show clock per
                        // frame, ABOVE the DMX render throttle, and ship it to every
                        // provider via the CueSnapshot stamp (+ the LFO bank). Clock
                        // candidates first so the resolve uses this-frame freshness;
                        // the Pro DJ Link master deck wins when a CDJ is master and
                        // its beat is fresh, else Ableton Link, else the derived clock.
                        let prolink = self
                            .track_sources
                            .iter()
                            .find_map(|s| s.master_beat())
                            .and_then(|mb| {
                                let age = mb.last_beat_at.elapsed().as_secs_f32();
                                (age < 2.0)
                                    .then(|| crate::emerge::prolink_clock(mb.bpm, mb.beat_in_measure, age))
                            });
                        let link = self.ableton_link.tempo().and_then(|(bpm, last_seen)| {
                            (last_seen.elapsed().as_secs_f32() < 2.0)
                                .then(|| crate::emerge::link_clock(bpm, snapshot.frame_time))
                        });
                        // Genre tempo prior for the autonomous estimator: when the
                        // operator's "Use Genre for Tempo" toggle is on, feed the
                        // selected genre's BPM range so REACT picks the right octave
                        // (75 vs 150) on melodic material. Toggle off — or a genre
                        // with no honest range (Custom / DJ Set) — => plain
                        // autocorrelation (85–170 fold). Copy values out of the
                        // immutable config() borrow before mutating dmx_manager.
                        let tempo_hint = if self.control_panel.genre_tempo_guidance() {
                            self.osc_thread.config().genre_profile.tempo_range_bpm()
                        } else {
                            None
                        };
                        self.dmx_manager.set_tempo_hint(tempo_hint);
                        // Same genre selection also shapes the DMX lane gates
                        // (strobe/laser lanes, gobo stillness, mover depth) —
                        // the dropdown changes the physical show, not just OSC.
                        let genre_fixture_profile =
                            self.osc_thread.config().genre_profile.fixture_profile();
                        self.dmx_manager.set_genre_fixture_profile(genre_fixture_profile);
                        // Style posture rides the same per-frame push: the
                        // show brain clamps the license band + composes the
                        // lane gates, so UI/MIDI style changes land <=1 frame.
                        self.dmx_manager
                            .set_lighting_style(self.osc_thread.config().lighting_style);
                        self.dmx_manager.set_clock_inputs(crate::emerge::ClockInputs {
                            prolink,
                            link,
                            // Operator's pick from the Lighting > Clock Source
                            // selector (defaults to Auto: Link > ProDJ > REACT).
                            mode: self.control_panel.clock_override(),
                        });
                        let mut show_clock =
                            self.dmx_manager.resolve_clock(&snapshot, lighting_metrics.as_ref());
                        // Zero confidence during silence so soft consumers (Hue/WLED/
                        // OSC/LFO) don't lock to a ghost tempo through a quiet passage
                        // — the estimator's confidence persists when no kicks land,
                        // so the app SilenceGate (raw band energy) is the truth.
                        if self.silence_gate == SilenceGateState::Silent {
                            show_clock.confidence = 0.0;
                        }

                        self.osc_thread.send_input(crate::osc::LightingInput {
                            analysis,
                            key_signal,
                            metrics: lighting_metrics,
                            show_clock,
                            arc_state: self.dmx_manager.current_arc(),
                            intensity_license: self.dmx_manager.intensity_license(),
                            effective_style: self.dmx_manager.effective_lighting_style(),
                            // The "Dom" checkbox governs the rig colour too:
                            // checked = dominant pitch, unchecked = all-notes blend.
                            dominant_only: self.control_panel.qs_dominant_only(),
                        });

                        // Update WLED - reads CueSnapshot produced by OSC thread
                        if self.wled_manager.is_enabled() {
                            let bands = crate::wled::FrequencyBands {
                                bass: analysis.bass,
                                mid: analysis.mid,
                                high: analysis.high,
                                kick: analysis.kick,
                                kick_intensity: analysis.kick_intensity,
                                chroma: analysis.chroma,
                            };
                            self.wled_manager.update(&snapshot, &bands);
                        }

                        // Update DMX/Art-Net demo output from the same lighting
                        // intent. `tick` consumes the clock already resolved above
                        // (same frame_time), so the estimator updates exactly once.
                        // Called UNCONDITIONALLY: the manager's brain tick (arc
                        // classification feeding the CueEngine buildup macro for
                        // every provider — OSC/Hue/WLED, not just the wire) runs
                        // even with DMX output disabled; rendering and emission
                        // are gated inside `update`.
                        {
                            // The app SilenceGate (raw band energy) is the silence
                            // truth; hand it to DMX so the rig settles dark in a quiet
                            // break (the smoothed snapshot energy floors too high to).
                            let silent = self.silence_gate == SilenceGateState::Silent;
                            self.dmx_manager.update(&snapshot, lighting_metrics.as_ref(), &self.fixture_overrides, silent);
                            // TRIG-DIAG: proves this consumer-driving block ran.
                            self.trig_diag_block_frames = self.trig_diag_block_frames.wrapping_add(1);
                        }
                        // Note: Hue thread reads CueSnapshot directly via shared Arc
                    }
                }
            }
        }

        // Update video playback (runs every frame, regardless of menu visibility)
        // When frozen, skip video updates to keep the current frame on screen
        if !self.control_panel.is_frozen() {
            // If user selected a new video in control panel, it replaces the landing video
            if self.control_panel.has_video() {
                // User selected a video - stop landing video and use control panel's
                if self.landing_video.is_playing() {
                    self.landing_video.close();
                }
                self.control_panel.update_video(self.render_pipeline.clone());
            } else if self.landing_video.is_playing() && !self.show_landing {
                // No user video selected - continue with landing video
                let pipeline = self.render_pipeline.clone();
                let mut frame_uploaded = false;
                #[cfg(target_os = "macos")]
                {
                    if self.landing_video.is_zero_copy() {
                        if let Some(pixel_buffer) = self.landing_video.get_frame_zero_copy() {
                            if let Some(ref mut p) = *pipeline.lock() {
                                frame_uploaded = p.set_video_pixel_buffer(&pixel_buffer);
                            }
                        }
                    }
                }
                if !frame_uploaded {
                    if let Some((frame_data, width, height)) = self.landing_video.get_frame() {
                        if let Some(ref mut p) = *pipeline.lock() {
                            p.set_video_frame(frame_data, width, height);
                        }
                    }
                }
            } else {
                // Fallback to control panel (even if no video)
                self.control_panel.update_video(self.render_pipeline.clone());
            }
        }
        // When frozen: video texture stays as-is on GPU (last frame before freeze)

        // Update LED Wall with audio (Metal path on macOS needs this separate call)
        // Note: update_theme() is now called before UI render in the egui frame above
        {
            use crate::render::RenderMode;
            let render_mode = self.control_panel.render_mode();
            if render_mode == RenderMode::LedWall {
                let analysis = self.audio_analysis.lock().clone();
                let dt = 1.0 / 60.0;
                if let Some(ref mut p) = *self.render_pipeline.lock() {
                    p.update_led_wall(analysis.stable_bass, analysis.stable_mid, analysis.stable_high, analysis.kick, dt);
                }
            }
        }

        // Update track announcement from music source
        // CDJ takes priority: if CDJ has a playing master deck, use that
        {
            let dt = 1.0 / 60.0; // Approximate frame time

            // Check sources in reverse order (CDJ added last = highest priority)
            // CDJ only takes over if it has a master deck that's playing
            let track = self.track_sources.iter_mut().rev().find_map(|source| {
                let track = source.get_current_track();
                if source.name() == "Pioneer CDJ" {
                    // CDJ only wins if master is actively playing
                    track.filter(|t| t.is_playing)
                } else {
                    track
                }
            });

            // Load artwork texture if available and changed
            if let Some(ref t) = track {
                if let Some(ref artwork_data) = t.artwork {
                    // Simple hash to detect changes
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    artwork_data.len().hash(&mut hasher);
                    if artwork_data.len() > 100 {
                        artwork_data[0..100].hash(&mut hasher);
                    }
                    let hash = hasher.finish();

                    if hash != self.last_artwork_hash {
                        self.last_artwork_hash = hash;
                        // Load artwork as texture
                        if let Ok(image) = image::load_from_memory(artwork_data) {
                            let rgba = image.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let pixels = rgba.into_raw();
                            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                            self.artwork_texture = Some(ctx.load_texture(
                                "album_artwork",
                                color_image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                }
            }

            let is_new_track = self.track_announcement.update(track.clone(), dt);

            // Auto-load matching compel when a new track is detected
            // Priority: 1) DB track associations, 2) 3-stage picker (deterministic → LLM)
            if is_new_track {
                if let Some(ref t) = track {
                    log::info!("[Track] New track detected: '{}' by '{}'", t.title, t.artist);

                    let artist = if t.artist.is_empty() { None } else { Some(t.artist.as_str()) };
                    let mut video_path: Option<String> = None;
                    let mut compel_picker_pending = false;

                    // Priority 1: Check database video_tracks associations (user-defined mappings)
                    if let Some(ref db) = self.db {
                        video_path = db.find_video_for_track_fuzzy(&t.title, artist);
                        if video_path.is_some() {
                            log::info!("[Track] DB track match found");
                        }
                    }

                    // Priority 2: 3-stage compel picker (deterministic → artist+LLM → global+LLM)
                    // Uses async resolution to avoid UI freeze during LLM inference
                    if video_path.is_none() {
                        let candidates = self.control_panel.get_compel_candidates(None);
                        log::info!("[Track] No DB match, trying picker with {} candidates", candidates.len());

                        // resolve_async returns immediately for deterministic matches,
                        // spawns background thread for LLM stages
                        if let Some(result) = self.compel_picker.resolve_async(&t.title, artist, &candidates) {
                            // Stage A deterministic match - available immediately
                            if let Some(ref path) = result.path {
                                log::info!(
                                    "[Track] Picker result ({:?}): {} - {}",
                                    result.confidence,
                                    path.split('/').last().unwrap_or(path),
                                    result.reason
                                );
                            }
                            video_path = result.path;
                        } else {
                            // Async LLM work started - will poll for result below
                            compel_picker_pending = true;
                            log::info!("[Track] LLM picker started (async)");
                        }
                    }

                    // Load the matched compel if found
                    if let Some(ref path) = video_path {
                        let currently_playing = self.control_panel.get_current_video_path();
                        if currently_playing.as_deref() != Some(path) {
                            log::info!(
                                "[Track] Auto-loading compel: {}",
                                path.split('/').last().unwrap_or(path.as_str())
                            );
                            self.control_panel.load_video(path);
                        }
                    }

                    self.auto_select_track_theme(
                        &t.title,
                        artist,
                        video_path.as_deref(),
                        !compel_picker_pending,
                    );
                }
            }

            // Poll for async compel picker results (LLM inference runs in background thread)
            if let Some(result) = self.compel_picker.poll_result() {
                let current_track = track.as_ref();
                if let Some(ref path) = result.path {
                    log::info!(
                        "[Track] Async picker result ({:?}): {} - {}",
                        result.confidence,
                        path.split('/').last().unwrap_or(path),
                        result.reason
                    );
                    // Load the compel if different from current
                    let currently_playing = self.control_panel.get_current_video_path();
                    if currently_playing.as_deref() != Some(path) {
                        log::info!(
                            "[Track] Auto-loading compel (async): {}",
                            path.split('/').last().unwrap_or(path)
                        );
                        self.control_panel.load_video(path);
                    }
                    if let Some(t) = current_track {
                        let artist = if t.artist.is_empty() { None } else { Some(t.artist.as_str()) };
                        self.auto_select_track_theme(&t.title, artist, Some(path.as_str()), true);
                    }
                } else {
                    log::info!("[Track] Async picker: no match found - {}", result.reason);
                    if let Some(t) = current_track {
                        let artist = if t.artist.is_empty() { None } else { Some(t.artist.as_str()) };
                        self.auto_select_track_theme(&t.title, artist, None, true);
                    }
                }
            }

            // Update preset browse overlay timer/opacity
            self.control_panel.update_browse_overlay(dt);
            // Update compel browse overlay timer/opacity
            self.control_panel.update_compel_browse_overlay(dt);
        }

        // === LOADING DIALOG FOR PRESET LOAD ===
        // Show loading dialog and process deferred preset loads (shader compilation can block UI)
        // Uses frame counter to ensure dialog renders for 2 frames before blocking load
        if self.control_panel.has_pending_preset_load() {
            let screen = ctx.screen_rect();
            let center = screen.center();

            // Show loading overlay
            egui::Area::new(egui::Id::new("preset_loading_overlay"))
                .fixed_pos(egui::pos2(center.x - 150.0, center.y - 50.0))
                .order(egui::Order::Foreground)
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgba_unmultiplied(10, 10, 20, 240))
                        .rounding(12.0)
                        .inner_margin(egui::Margin::symmetric(32.0, 24.0))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 38, 38)))  // Compeller red
                        .show(ui, |ui| {
                            ui.set_min_width(280.0);

                            if let Some((cat, preset)) = self.control_panel.get_pending_preset_load() {
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new("Loading...")
                                            .size(18.0)
                                            .color(egui::Color32::from_rgb(200, 200, 220))
                                    );
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(&preset)
                                            .size(14.0)
                                            .color(egui::Color32::from_rgb(220, 100, 100))  // Compeller red tint
                                    );
                                    ui.label(
                                        egui::RichText::new(&cat)
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(120, 120, 140))
                                    );
                                    ui.add_space(12.0);
                                    ui.spinner();
                                });
                            }
                        });
                });

            // Request repaint to keep UI responsive
            ctx.request_repaint();

            // Check if we've shown the dialog for enough frames before blocking
            if self.control_panel.should_process_pending_load() {
                // Process the load (this will block while shaders compile)
                let mut pipeline_guard = self.render_pipeline.lock();
                if let Some(ref mut pipeline) = *pipeline_guard {
                    self.control_panel.process_pending_preset_load(pipeline);
                }
            }
        }

        // Main visualization area (takes remaining space)
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(5, 5, 10)))
            .show(ctx, |ui| {
                let available = ui.available_size();
                self.render_visualization(ui, available.x, available.y);
            });

        // Track announcement now rendered via GPU overlay on REACT/NDI output only

        // Logo watermark removed - now using GPU overlay instead

        // Preset browser overlay (center screen, DJ only - not sent to output/NDI)
        {
            let pipeline_guard = self.render_pipeline.lock();
            if let Some(ref pipeline) = *pipeline_guard {
                if let Some((display_name, _file_name, category, current_idx, total, opacity)) =
                    self.control_panel.get_browse_overlay_state(pipeline)
                {
                    if opacity > 0.01 {
                        let screen = ctx.screen_rect();
                        let center = screen.center();

                        egui::Area::new(egui::Id::new("preset_browser_overlay"))
                            .fixed_pos(egui::pos2(center.x - 200.0, center.y - 40.0))
                            .order(egui::Order::Foreground)
                            .interactable(false)
                            .show(ctx, |ui| {
                                let alpha = (opacity * 255.0) as u8;

                                egui::Frame::none()
                                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 30, (alpha as f32 * 0.9) as u8))
                                    .rounding(8.0)
                                    .inner_margin(egui::Margin::symmetric(24.0, 16.0))
                                    .show(ui, |ui| {
                                        ui.set_min_width(350.0);

                                        // Category label
                                        ui.label(
                                            egui::RichText::new(&category)
                                                .size(11.0)
                                                .color(egui::Color32::from_rgba_unmultiplied(150, 150, 170, alpha))
                                        );

                                        // Preset name (large)
                                        ui.label(
                                            egui::RichText::new(&display_name)
                                                .size(22.0)
                                                .strong()
                                                .color(egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha))
                                        );

                                        ui.add_space(4.0);

                                        // Index indicator
                                        ui.label(
                                            egui::RichText::new(format!("{} / {}", current_idx, total))
                                                .size(12.0)
                                                .color(egui::Color32::from_rgba_unmultiplied(100, 200, 255, alpha))
                                        );
                                    });
                            });
                    }
                }

                // Compel browser overlay (for MIDI encoder browsing of videos)
                if let Some((display_name, file_path, current_idx, total, opacity)) = self.control_panel.get_compel_browse_overlay_state() {
                    if opacity > 0.01 {
                        let screen = ctx.screen_rect();
                        let center = screen.center();

                        // Get or load thumbnail texture for display
                        let thumbnail = self.control_panel.get_or_load_thumbnail_for_overlay(ctx, &file_path);

                        egui::Area::new(egui::Id::new("compel_browser_overlay"))
                            .fixed_pos(egui::pos2(center.x - 250.0, center.y - 60.0))
                            .order(egui::Order::Foreground)
                            .interactable(false)
                            .show(ctx, |ui| {
                                let alpha = (opacity * 255.0) as u8;

                                egui::Frame::none()
                                    .fill(egui::Color32::from_rgba_unmultiplied(20, 30, 40, (alpha as f32 * 0.9) as u8))
                                    .rounding(8.0)
                                    .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            // Thumbnail on the left
                                            let thumb_size = egui::vec2(120.0, 90.0);
                                            let (thumb_rect, _) = ui.allocate_exact_size(thumb_size, egui::Sense::hover());

                                            // Draw thumbnail background
                                            ui.painter().rect_filled(
                                                thumb_rect,
                                                4.0,
                                                egui::Color32::from_rgba_unmultiplied(40, 40, 50, alpha)
                                            );

                                            // Draw thumbnail if available
                                            if let Some(texture) = thumbnail {
                                                let tex_size = texture.size_vec2();
                                                let tex_aspect = tex_size.x / tex_size.y;
                                                let thumb_aspect = thumb_size.x / thumb_size.y;

                                                // Calculate display rect preserving aspect ratio
                                                let display_rect = if tex_aspect > thumb_aspect {
                                                    let display_height = thumb_size.x / tex_aspect;
                                                    let y_offset = (thumb_size.y - display_height) / 2.0;
                                                    egui::Rect::from_min_size(
                                                        thumb_rect.min + egui::vec2(0.0, y_offset),
                                                        egui::vec2(thumb_size.x, display_height)
                                                    )
                                                } else {
                                                    let display_width = thumb_size.y * tex_aspect;
                                                    let x_offset = (thumb_size.x - display_width) / 2.0;
                                                    egui::Rect::from_min_size(
                                                        thumb_rect.min + egui::vec2(x_offset, 0.0),
                                                        egui::vec2(display_width, thumb_size.y)
                                                    )
                                                };

                                                ui.painter().image(
                                                    texture.id(),
                                                    display_rect,
                                                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
                                                );
                                            } else {
                                                // Placeholder icon
                                                ui.painter().text(
                                                    thumb_rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    egui_phosphor::regular::VIDEO,
                                                    egui::FontId::proportional(32.0),
                                                    egui::Color32::from_rgba_unmultiplied(80, 80, 90, alpha),
                                                );
                                            }

                                            ui.add_space(12.0);

                                            // Text content on the right
                                            ui.vertical(|ui| {
                                                ui.set_min_width(280.0);

                                                // Category label (Compels)
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(egui_phosphor::regular::VIDEO)
                                                            .size(14.0)
                                                            .color(egui::Color32::from_rgba_unmultiplied(100, 200, 255, alpha))
                                                    );
                                                    ui.label(
                                                        egui::RichText::new("Compels")
                                                            .size(11.0)
                                                            .color(egui::Color32::from_rgba_unmultiplied(100, 200, 255, alpha))
                                                    );
                                                });

                                                // Video name (large)
                                                ui.label(
                                                    egui::RichText::new(&display_name)
                                                        .size(22.0)
                                                        .strong()
                                                        .color(egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha))
                                                );

                                                ui.add_space(4.0);

                                                // Index indicator
                                                ui.label(
                                                    egui::RichText::new(format!("{} / {}", current_idx, total))
                                                        .size(12.0)
                                                        .color(egui::Color32::from_rgba_unmultiplied(100, 200, 255, alpha))
                                                );
                                            });
                                        });
                                    });
                            });
                    }
                }
            }
        }

        // Math Overlay HUD (top-left corner - shows harmonic math state)
        if self.control_panel.show_math_overlay() && !self.show_landing {
            // Get harmonic field state (from buffer when frozen, live otherwise)
            let field_state = if self.control_panel.is_frozen() {
                self.harmonic_state_buffer.current().cloned().unwrap_or_default()
            } else {
                let pipeline_guard = self.render_pipeline.lock();
                pipeline_guard.as_ref()
                    .and_then(|p| p.get_harmonic_field_state())
                    .unwrap_or_default()
            };

            let is_frozen = self.control_panel.is_frozen();

            egui::Window::new("Math HUD")
                .id(egui::Id::new("math_overlay_hud"))
                .default_pos(egui::pos2(12.0, 12.0))
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .frame(egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0)))
                .show(ctx, |ui| {
                    ui.set_min_width(150.0);

                    // Drag handle header
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(egui_phosphor::regular::DOTS_SIX)
                            .size(10.0)
                            .color(egui::Color32::from_rgb(80, 80, 90)));
                        ui.label(egui::RichText::new("Math")
                            .size(9.0)
                            .color(egui::Color32::from_rgb(80, 80, 90)));
                    });
                    ui.add_space(2.0);

                    // Freeze indicator
                    if is_frozen {
                        let scrub_pct = self.control_panel.scrub_position() * 100.0;
                        let frame_idx = (self.control_panel.scrub_position() * (self.harmonic_state_buffer.len() as f32 - 1.0)) as usize;
                        let pulse = ((self.frame_count as f32 * 0.2).sin() * 0.5 + 0.5) as u8;
                        ui.label(egui::RichText::new(format!("FROZEN [{}/{}] {:.0}%",
                            frame_idx + 1,
                            self.harmonic_state_buffer.len(),
                            scrub_pct))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(100 + pulse * 50, 200, 255))
                            .strong());
                        ui.add_space(2.0);
                    }

                    // Gate status with pulse based on stability (less stable = more motion)
                    let (gate_text, gate_color) = if field_state.is_tonal && field_state.tonal_confidence > 0.25 && field_state.stability > 0.6 {
                        ("STABLE", egui::Color32::from_rgb(100, 255, 100))
                    } else if field_state.is_tonal {
                        // Pulse effect for AMBIGUOUS (unstable = more visible pulse)
                        let pulse = ((self.frame_count as f32 * 0.15).sin() * 0.5 + 0.5) * (1.0 - field_state.stability);
                        let g = (200.0 + pulse * 55.0) as u8;
                        ("AMBIGUOUS", egui::Color32::from_rgb(255, g, 50))
                    } else {
                        ("ATONAL", egui::Color32::from_rgb(120, 120, 140))
                    };
                    ui.label(egui::RichText::new(format!("Gate: {}", gate_text))
                        .size(11.0)
                        .color(gate_color)
                        .strong());

                    // Dominant pitch + confidence with mini bar
                    let pitch_color = if field_state.is_tonal {
                        egui::Color32::from_rgb(100, 255, 150)
                    } else {
                        egui::Color32::from_rgb(100, 100, 120)
                    };
                    // Gate unstable = show "(soft)" indicator
                    let is_ambiguous = field_state.is_tonal && (field_state.tonal_confidence <= 0.25 || field_state.stability <= 0.6);
                    let pitch_text = if is_ambiguous {
                        format!("Pitch: {} ({:.0}%) (soft)", field_state.dominant_name, field_state.tonal_confidence * 100.0)
                    } else {
                        format!("Pitch: {} ({:.0}%)", field_state.dominant_name, field_state.tonal_confidence * 100.0)
                    };
                    ui.label(egui::RichText::new(pitch_text)
                        .size(11.0)
                        .color(pitch_color));

                    // Mini confidence bar
                    let bar_rect = ui.available_rect_before_wrap();
                    let bar_height = 3.0;
                    let bar_width = 130.0;
                    let bar_start = egui::pos2(bar_rect.min.x, bar_rect.min.y);
                    let painter = ui.painter();
                    // Background
                    painter.rect_filled(
                        egui::Rect::from_min_size(bar_start, egui::vec2(bar_width, bar_height)),
                        1.0,
                        egui::Color32::from_rgb(40, 40, 50),
                    );
                    // Fill
                    let fill_width = bar_width * field_state.tonal_confidence.clamp(0.0, 1.0);
                    painter.rect_filled(
                        egui::Rect::from_min_size(bar_start, egui::vec2(fill_width, bar_height)),
                        1.0,
                        pitch_color,
                    );
                    ui.add_space(bar_height + 4.0);

                    // Phase
                    ui.label(egui::RichText::new(format!("Phase: {:.2}", field_state.phase))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(180, 150, 255)));

                    // Stability + Gate OK (shown together)
                    let stability_color = if field_state.stability > 0.6 {
                        egui::Color32::from_rgb(100, 200, 255)
                    } else {
                        egui::Color32::from_rgb(255, 150, 100)
                    };
                    let gate_ok_str = if field_state.gate_ok > 0.5 { "OK" } else { "AMB" };
                    let gate_ok_color = if field_state.gate_ok > 0.5 {
                        egui::Color32::from_rgb(100, 200, 100)
                    } else {
                        egui::Color32::from_rgb(255, 180, 80)
                    };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Stability: {:.0}%", field_state.stability * 100.0))
                            .size(11.0)
                            .color(stability_color));
                        ui.label(egui::RichText::new(format!(" Gate: {}", gate_ok_str))
                            .size(10.0)
                            .color(gate_ok_color));
                    });

                    // Energy
                    ui.label(egui::RichText::new(format!("Energy: {:.0}%", field_state.energy_vis * 100.0))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(150, 150, 170)));

                    // Render mode (visual style) + fold symmetry
                    ui.label(egui::RichText::new(format!("Render: {} {}-fold",
                        field_state.render_mode,
                        field_state.symmetry))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(120, 120, 140)));

                    // Fold24 engagement status - clarify WHY 24-fold is active
                    if field_state.symmetry == 24 || field_state.fold24_engaged {
                        let (fold_text, fold_color) = if field_state.fold24_engaged {
                            // Determine reason for engagement
                            let reason = if field_state.ring_dive_active {
                                "RingDive"
                            } else if field_state.fold24_request_latch_bars > 0 {
                                "requested"
                            } else {
                                "held"
                            };
                            (format!("Fold24: engaged ({})", reason), egui::Color32::from_rgb(180, 180, 100))
                        } else {
                            // 24-fold topology but not engaged (should be rare)
                            ("Fold24: transitioning".to_string(), egui::Color32::from_rgb(120, 120, 80))
                        };
                        ui.label(egui::RichText::new(fold_text)
                            .size(9.0)
                            .color(fold_color));
                    }

                    // Anchor θ — the sacred quantized symmetry angle (always on-grid)
                    {
                        let anchor_color = if field_state.gate_ok > 0.5 {
                            egui::Color32::from_rgb(100, 255, 200) // green when gate OK
                        } else {
                            egui::Color32::from_rgb(180, 180, 180) // dim when ambiguous
                        };
                        ui.label(egui::RichText::new(
                            format!("Anchor: k={}/{} \u{03B8}={:.0}\u{00B0} (QUANTIZED)",
                                field_state.anchor_k, field_state.symmetry, field_state.anchor_theta_deg))
                            .size(10.0)
                            .color(anchor_color));
                    }

                    // Polarity (INFO) - show quadrant names and REG harmonic integer
                    let quadrant_names = ["+1 (E)", "+i (N)", "-1 (W)", "-i (S)"];
                    let quadrant_colors = [
                        egui::Color32::from_rgb(255, 217, 77),   // E: gold
                        egui::Color32::from_rgb(77, 230, 255),   // N: cyan
                        egui::Color32::from_rgb(255, 102, 217),  // W: magenta
                        egui::Color32::from_rgb(102, 255, 153),  // S: green
                    ];
                    let curr_q = field_state.polarity_quadrant as usize;
                    let prev_q = field_state.polarity_prev_quadrant as usize;
                    let polarity_text = if curr_q != prev_q && field_state.reg_degree_a > 0 {
                        format!("Polarity: {} → {} | A={} (INFO)",
                            quadrant_names[prev_q.min(3)],
                            quadrant_names[curr_q.min(3)],
                            field_state.reg_degree_a)
                    } else if field_state.reg_degree_a > 0 {
                        format!("Polarity: {} | A={} (INFO)",
                            quadrant_names[curr_q.min(3)],
                            field_state.reg_degree_a)
                    } else {
                        "Polarity: —".to_string()
                    };
                    ui.label(egui::RichText::new(polarity_text)
                        .size(10.0)
                        .color(quadrant_colors[curr_q.min(3)]));

                    // Topology Router - show standby when fold24 not engaged
                    {
                        let is_standby = !field_state.fold24_engaged;
                        let topo_color = if is_standby {
                            egui::Color32::from_rgb(90, 90, 70) // dim when standby
                        } else if field_state.topology_name == "Scaffold24" {
                            egui::Color32::from_rgb(120, 120, 80)
                        } else {
                            egui::Color32::from_rgb(180, 180, 100)
                        };
                        let topo_text = if is_standby {
                            format!("Router(24): {} (standby)", field_state.topology_name)
                        } else if field_state.topology_xfade_t > 0.01 && field_state.topology_xfade_t < 0.99 {
                            format!("Router(24): {} (xf {:.0}%)", field_state.topology_name, field_state.topology_xfade_t * 100.0)
                        } else {
                            format!("Router(24): {} bar={}", field_state.topology_name, field_state.topology_bar_index)
                        };
                        ui.label(egui::RichText::new(topo_text)
                            .size(10.0)
                            .color(topo_color));

                        // Debug routing conditions - only show when fold24 engaged
                        if field_state.fold24_engaged {
                            let mid_str = if field_state.router_mid_dominant { "MID" } else { "---" };
                            let debug_text = format!("  E={:.2} {} slope={:.2} hold={:.1}s req={}",
                                field_state.router_energy_bar,
                                mid_str,
                                field_state.router_tension_slope,
                                field_state.router_hold_s,
                                field_state.fold24_request_latch_bars);
                            ui.label(egui::RichText::new(debug_text)
                                .size(9.0)
                                .color(egui::Color32::from_rgb(100, 100, 80)));
                        }
                    }

                    // Vortex state
                    if field_state.vortex_active || field_state.vortex_t > 0.01 {
                        let vortex_color = egui::Color32::from_rgb(100, 200, 255);
                        let vortex_text = format!("Vortex: G{} {} {:.0}%",
                            field_state.vortex_gate,
                            field_state.vortex_direction,
                            field_state.vortex_t * 100.0);
                        ui.label(egui::RichText::new(vortex_text)
                            .size(10.0)
                            .color(vortex_color));
                    }
                });
        }

        // Performance HUD (top-left, below math HUD)
        // Get perf stats for FPS monitoring (even if HUD not visible)
        let (frame_ms, sdf_ms, _avg_steps, _hit_rate, render_scale) = {
            let pipeline_guard = self.render_pipeline.lock();
            pipeline_guard.as_ref()
                .map(|p| p.get_perf_stats())
                .unwrap_or((16.0, 8.0, 24.0, 0.5, 1.0))
        };
        let fps = if frame_ms > 0.0 { 1000.0 / frame_ms } else { 0.0 };

        // Auto-adjust quality if sustained low FPS
        self.control_panel.check_fps_and_adjust(fps);

        // Auto-shuffle theme on sustained FPS dip (gated by auto-randomize toggle)
        if self.control_panel.should_shuffle_theme_for_fps(fps) {
            if let Some(mut pipeline_guard) = self.render_pipeline.try_lock() {
                if let Some(ref mut p) = *pipeline_guard {
                    self.control_panel.set_random_preset(p);
                }
            }
        }

        if self.control_panel.show_perf_hud() && !self.show_landing {
            egui::Window::new("Perf HUD")
                .id(egui::Id::new("perf_hud"))
                .default_pos(egui::pos2(12.0, 280.0))
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .frame(egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0)))
                .show(ctx, |ui| {
                    ui.set_min_width(100.0);

                    // Drag handle header
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(egui_phosphor::regular::DOTS_SIX)
                            .size(10.0)
                            .color(egui::Color32::from_rgb(80, 80, 90)));
                        ui.label(egui::RichText::new("Perf")
                            .size(9.0)
                            .color(egui::Color32::from_rgb(80, 80, 90)));
                    });
                    ui.add_space(2.0);

                    // FPS with color coding (green/orange/red)
                    let fps_color = if fps >= 58.0 {
                        egui::Color32::from_rgb(100, 255, 100)
                    } else if fps >= 45.0 {
                        egui::Color32::from_rgb(255, 200, 100)
                    } else {
                        egui::Color32::from_rgb(255, 100, 100)
                    };
                    ui.label(egui::RichText::new(format!("FPS: {:.0}", fps))
                        .size(12.0).color(fps_color).strong());

                    ui.label(egui::RichText::new(format!("Frame: {:.1}ms", frame_ms))
                        .size(10.0).color(egui::Color32::from_rgb(150, 200, 255)));

                    ui.label(egui::RichText::new(format!("SDF: {:.1}ms", sdf_ms))
                        .size(10.0).color(egui::Color32::from_rgb(255, 180, 120)));

                    ui.label(egui::RichText::new(format!("Scale: {:.0}%", render_scale * 100.0))
                        .size(10.0).color(egui::Color32::from_rgb(150, 150, 180)));

                    // Show low FPS warning if recently adjusted
                    if self.control_panel.low_fps_warning_frames > 0 {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("⚠ Performance adjusted to Low")
                            .size(9.0)
                            .color(egui::Color32::from_rgb(255, 200, 100)));
                    }
                });
        }

        // Debug HUD (top-right, displays REG/QC/Portal/FPS state)
        if self.control_panel.show_debug_hud() && !self.show_landing {
            let debug_lines = {
                let pipeline_guard = self.render_pipeline.lock();
                pipeline_guard.as_ref()
                    .map(|p| p.get_debug_hud_strings())
                    .unwrap_or_default()
            };

            let screen = ctx.screen_rect();
            egui::Window::new("Debug HUD")
                .id(egui::Id::new("debug_hud"))
                .default_pos(egui::pos2(screen.max.x - 280.0, 12.0))
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .frame(egui::Frame::none()
                    .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0)))
                .show(ctx, |ui| {
                    ui.set_min_width(250.0);

                    // Drag handle header
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(egui_phosphor::regular::DOTS_SIX)
                            .size(10.0)
                            .color(egui::Color32::from_rgb(80, 80, 90)));
                        ui.label(egui::RichText::new("REG Debug")
                            .size(9.0)
                            .color(egui::Color32::from_rgb(80, 80, 90)));
                    });
                    ui.add_space(2.0);

                    // Display debug lines
                    for line in &debug_lines {
                        // Color code by prefix
                        let color = if line.starts_with("===") {
                            egui::Color32::from_rgb(180, 180, 255)  // Section headers
                        } else if line.contains("ACTIVE") || line.contains("OK") {
                            egui::Color32::from_rgb(100, 255, 100)  // Active/OK = green
                        } else if line.contains("VIOLATION") || line.contains("ERROR") {
                            egui::Color32::from_rgb(255, 100, 100)  // Errors = red
                        } else {
                            egui::Color32::from_rgb(200, 200, 200)  // Normal
                        };
                        ui.label(egui::RichText::new(line).size(10.0).color(color).monospace());
                    }
                });
        }

        // Output window status indicator (bottom-left corner)
        if self.output_manager.has_native_window() && !self.show_landing {
            let screen = ctx.screen_rect();
            egui::Area::new(egui::Id::new("output_indicator"))
                .fixed_pos(egui::pos2(10.0, screen.max.y - 30.0))
                .order(egui::Order::Foreground)
                .interactable(false)
                .show(ctx, |ui| {
                    let fs_text = if self.output_manager.is_native_fullscreen() { " [FS]" } else { "" };
                    ui.label(
                        egui::RichText::new(format!("OUTPUT{}", fs_text))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(100, 255, 100))
                    );
                });
        }

        // CDJ status is now shown in the Pro DJ Link control panel section

        // TRIG-DIAG probe: while a manual trigger is held (or latched), log
        // the full consumer gate chain at ~1Hz. This measures the runtime
        // fact behind "triggers don't fire without audio": the engine-side
        // snapshot is proven audio-independent, so if a held trigger doesn't
        // reach the rig, one of THESE gates is the reason. Cheap: two mutex
        // clones per second, only while a trigger is engaged.
        {
            let ui = self.osc_thread.ui_state();
            let engaged = ui.trigger_strobe_held
                || ui.trigger_blinder_held
                || ui.trigger_uv_held
                || ui.trigger_uv_latched
                || ui.trigger_blackout_held;
            // 1Hz while a trigger is engaged; 10s heartbeat otherwise so a
            // log capture shows the gate states without needing a press.
            let interval = if engaged { 1.0 } else { 10.0 };
            if self
                .trig_diag_at
                .map(|t| t.elapsed().as_secs_f32() >= interval)
                .unwrap_or(true)
            {
                self.trig_diag_at = Some(std::time::Instant::now());
                let snap = self.osc_thread.snapshot();
                let lighting_licensed = self.auth_manager.get_entitlements().lighting_enabled;
                log::info!(
                    "[TRIG-DIAG] held(s={} b={} uv={} uvl={} ko={}) snap(s={} b={} uv={} strobe_on={}) gates(landing={} frozen={} vs={} lic={} osc={} hue={} wled={} dmx={}) audio(enabled={} gate={:?}) block_frames={}",
                    ui.trigger_strobe_held,
                    ui.trigger_blinder_held,
                    ui.trigger_uv_held,
                    ui.trigger_uv_latched,
                    ui.trigger_blackout_held,
                    snap.trigger_strobe,
                    snap.trigger_blinder,
                    snap.trigger_uv,
                    snap.strobe_on,
                    self.show_landing,
                    self.control_panel.is_frozen(),
                    self.dmx_manager.virtual_stage_enabled(),
                    lighting_licensed,
                    self.osc_thread.is_enabled(),
                    self.hue_thread.is_enabled(),
                    self.wled_manager.is_enabled(),
                    self.dmx_manager.is_enabled(),
                    self.audio_enabled,
                    self.silence_gate,
                    self.trig_diag_block_frames,
                );
            }
        }

        // Request continuous repaint for animation
        ctx.request_repaint();
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        log::info!("Shutting down Compeller REACT");

        // Save state to database (force save on shutdown)
        self.control_panel.save_settings();
        // Also save app-level settings (menu_visible)
        if let Some(ref db) = self.db {
            let _ = db.set_bool("menu_visible", self.menu_visible);
            let _ = db.set_setting("fixture_segment_fx", &self.fixture_overrides.segfx_json());
            log::info!("[DB] State saved on shutdown");
        }

        // Stop audio capture
        self.stop_audio();

        // Clean up render pipeline
        if let Some(ref mut pipeline) = *self.render_pipeline.lock() {
            if let Some(gl) = gl {
                pipeline.cleanup(gl);
            }
        }
    }
}

/// Actions that can be triggered from the UI for audio control
pub enum AudioAction {
    None,
    Start,
    Stop,
    SetSource(AudioSource),
    SetGain(f32),
}

/// Actions returned from control panel for output handling
#[derive(PartialEq)]
pub enum OutputAction {
    None,
    ToggleNdi,
    ToggleNdiAudio,
    ToggleOutputWindow,
    ToggleOutputFullscreen,
    ToggleRecording,
    SetNdiGain(f32),
    SetRecGain(f32),
    SetYouTubeGain(f32),
    SetTwitchGain(f32),
    SetKickGain(f32),
}

/// Actions returned from control panel for MIDI handling
#[derive(PartialEq, Clone)]
pub enum MidiAction {
    None,
    ArmLearn,  // Arm learn mode, waiting for user to click a control
    SetLearnTarget(crate::midi::ControlTarget),  // User clicked a control
    CancelLearn,
    RemoveMapping(usize),
    RescanDevices,
    LearnPresetBrowserStart,  // Start two-step learn: encoder, then push button
    LearnCompelBrowserStart,  // Start two-step learn for compel browser: encoder, then push button
}

/// MIDI state for UI display
#[derive(Clone)]
pub struct MidiState {
    pub connected_devices: usize,
    pub learn_state: crate::midi::LearnState,
    pub mappings: Vec<crate::midi::MidiMapping>,
    /// True if learning preset browser encoder (step 1), false otherwise
    pub learning_preset_browser: bool,
    /// True if learning compel browser encoder (step 1), false otherwise
    pub learning_compel_browser: bool,
}

impl MidiState {
    /// Check if learn mode is armed and waiting for the user to click a control
    pub fn is_waiting_for_target(&self) -> bool {
        matches!(&self.learn_state, crate::midi::LearnState::Armed { target: None, .. })
    }
}

/// Actions returned from control panel for Hue handling
#[derive(PartialEq, Clone)]
pub enum HueAction {
    None,
    SetEnabled(bool),
    SetBridgeIp(String),
    StartPairing,
    FetchAreas,
    SelectArea(String),
    SetLightRole(u16, crate::hue::LightRole),
    SetLightFormFactor(u16, Option<crate::lighting::FormFactor>),
    Connect,
    Disconnect,
    /// Store credentials after successful pairing (username, clientkey)
    StoreCredentials(String, String),
    /// Set protocol version (v1 legacy or v2 recommended)
    SetProtocolVersion(crate::hue::HueProtocolVersion),
    /// Set mapping mode (Reactive or MultiBand)
    SetMappingMode(crate::hue::HueMappingMode),
}

/// Actions returned from control panel for WLED handling
#[derive(PartialEq, Clone)]
pub enum WledAction {
    None,
    SetEnabled(bool),
    AddStrip(String),
    RemoveStrip(usize),
    SetStripMode(usize, crate::wled::StripMode),
    SetStripFormFactor(usize, Option<crate::lighting::FormFactor>),
    SetStripEnabled(usize, bool),
    Connect(usize),
    Disconnect(usize),
    ConnectAll,
    DisconnectAll,
}

/// Actions returned from control panel for OSC handling
#[derive(PartialEq, Clone)]
pub enum OscAction {
    None,
    SetEnabled(bool),
    SetTarget(String, u16),
    UpdateConfig(crate::osc::OscConfig),
    /// Manual trigger button pressed
    TriggerPress(crate::emerge::TriggerButton),
    /// Manual trigger button released
    TriggerRelease(crate::emerge::TriggerButton),
    /// Set strobe rate (Hz)
    SetStrobeRate(f32),
    /// Flip the fog latch (hands-free fog hold).
    ToggleFogLatch,
    /// Flip the UV latch (hands-free UV mode).
    ToggleUvLatch,
    /// Fetch MaestroDMX patterns/palettes from REST API
    FetchMaestroData,
    /// Select a MaestroDMX pattern by index
    SelectMaestroPattern(i32),
    /// Select a MaestroDMX color palette by index
    SelectMaestroColorPalette(i32),
    /// Select a MaestroDMX FX palette by index
    SelectMaestroFxPalette(i32),
    /// Force-enter buildup macro (BlindersHold) regardless of audio analysis
    ForceBuildup,
    /// Operator picked a stage in the Stage: dropdown — `None` is the
    /// "Select stage…" placeholder; `Some(id)` triggers an auto-pull and
    /// arms the auto-flush diff path. All persistence is automatic from
    /// here — no Sync / Save buttons.
    SelectStage(Option<i64>),
    /// Refresh the cached stage list (called when the Stage: dropdown opens).
    RefreshStageList,
    /// Operator clicked "Clone to My Stages" on a shared admin template — clone it
    /// into the account's own editable "My <venue>" stage and select the copy.
    CloneStage(i64),
    /// COM-499 operator-launch-local: fire a cached cue definition locally (zero
    /// SaaS) by its `def_key`.
    LaunchLocalCue(String),
    /// Stop an operator-local cue by its `launch_id`.
    StopLocalCue(String),
    /// COM-499 Phase B4: operator edited the LFO bank — push to the runtime + persist.
    SetLfoBank(crate::emerge::LfoBankConfig),
    /// Operator picked a DMX output backend in the DMX panel — switch the live
    /// output + persist. `Option<String>` is the explicit USB serial port (USB only).
    SetDmxBackend(crate::dmx::DmxBackend, Option<String>),
}

/// Read-only snapshot the Stage: dropdown + sync status line consume each frame.
#[derive(Clone)]
pub struct StageUiState {
    pub stages: Vec<crate::stage_sync::StageSummary>,
    pub selected_stage_id: Option<i64>,
    pub sync_status: crate::stage_sync_worker::SyncStatus,
    /// COM-499 operator-launch-local: cached cue definitions `(def_key, label)`
    /// the operator can fire locally (zero SaaS), and the currently-active local
    /// cues `(launch_id, label)` they can stop. Both empty without a selected stage.
    pub cue_defs: Vec<(String, String)>,
    pub active_local_cues: Vec<(String, String)>,
    /// Live DMX output status for the DMX panel (backend + connection + detail).
    pub dmx_status: crate::dmx::DmxOutputStatus,
}

/// OSC state for UI display
#[derive(Clone)]
pub struct OscState {
    pub enabled: bool,
    pub target_ip: String,
    pub target_port: u16,
    pub config: crate::osc::OscConfig,
    pub current_zone: crate::emerge::EnergyZone,
    pub current_palette: crate::emerge::Palette,
    pub current_harmonic: crate::render::HarmonicLabel,
    pub strobe_hold_active: bool,
    pub blackout_active: bool,
    /// Activity level for each zone [Silent, Still, Low, Mid, High] (0.0-1.0, fades after trigger)
    pub zone_activity: [f32; 5],
    /// Smoothed metrics (energy, tension, balance, stability)
    pub smoothed_metrics: (f32, f32, f32, f32),
    // === Manual Triggers State ===
    /// Manual strobe button held
    pub trigger_strobe_held: bool,
    /// Manual blackout button held
    pub trigger_blackout_held: bool,
    /// Manual blinder button held
    pub trigger_blinder_held: bool,
    /// Auto-blinder is active (BlindersHold — latch is on)
    pub auto_blinder_active: bool,
    /// Current macro phase (for UI display)
    pub macro_phase: crate::emerge::AutoBlinderPhase,
    /// The posture actually driving the engine right now (operator pin, or
    /// the Auto resolver's live flip) — the STYLE dropdown displays this so
    /// the operator can watch Auto move between Chill/Energetic/Banger.
    pub resolved_style: crate::emerge::LightingStyle,
    /// Manual fog button held
    pub trigger_fog_held: bool,
    /// Fog latch — when on, fog stays running regardless of `trigger_fog_held`.
    pub trigger_fog_latched: bool,
    /// Manual UV button held (UV mode — only UV-capable emitters lit)
    pub trigger_uv_held: bool,
    /// UV latch — when on, UV mode stays on regardless of `trigger_uv_held`.
    pub trigger_uv_latched: bool,
    /// Strobe rate (Hz)
    pub trigger_strobe_rate: f32,
    /// Last pattern index sent by CueEngine (-1 = none)
    pub current_pattern: i32,
    // === MaestroDMX API Data ===
    /// Available patterns from MaestroDMX (index, name)
    pub maestro_patterns: Vec<(i32, String)>,
    /// Available color palettes from MaestroDMX (index, name)
    pub maestro_color_palettes: Vec<(i32, String)>,
    /// Available FX palettes from MaestroDMX (index, name)
    pub maestro_fx_palettes: Vec<(i32, String)>,
    /// Available shows from MaestroDMX (uuid, name)
    pub maestro_shows: Vec<(String, String)>,
    /// Whether MaestroDMX data is currently loading
    pub maestro_loading: bool,
    /// Error message if MaestroDMX fetch failed
    pub maestro_error: Option<String>,
    /// Last IP used for MaestroDMX fetch (to detect changes)
    pub maestro_last_fetch_ip: Option<String>,
}

/// Camera tracking actions from UI
#[derive(PartialEq, Clone)]
pub enum CameraAction {
    None,
    Start,
    Stop,
    ScanCameras,
    SetProgramCamera(u32),
    SetEnabled(u32, bool),
    SetTrackFilter(String),
}

/// Camera tracking state for UI display
#[derive(Clone, Default)]
pub struct CameraState {
    pub enabled: bool,
    pub available_cameras: Vec<crate::camera::CameraInfo>,
    pub enabled_cameras: Vec<u32>,
    pub program_camera: u32,
    pub output: crate::camera::TrackingOutput,
    pub camera_track_counts: Vec<(u32, usize)>,
    pub preview: crate::camera::PreviewFrame,
}
