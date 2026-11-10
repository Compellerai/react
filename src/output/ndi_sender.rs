//! NDI Output Sender
//!
//! Streams video frames via NDI to external applications like OBS, Resolume, etc.
//! Uses embedded libndi.dylib loaded at runtime via libloading.
//!
//! Audio handling:
//! - External-clocked: audio arrives pre-paced at 20ms cadence from drain/writer threads
//! - Sends immediately to NDI SDK with synthesized timecodes (no internal pacing)
//! - AudioBus delivers canonical 48kHz stereo interleaved f32 (no resampling needed)
//! - Per-output gain controlled via UI slider (default: unity)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::path::PathBuf;
use parking_lot::Mutex;
use anyhow::Result;
use libloading::{Library, Symbol};

use super::FrameRing;

// NDI type definitions matching the C API
type NDIlib_initialize = unsafe extern "C" fn() -> bool;
type NDIlib_destroy = unsafe extern "C" fn();
type NDIlib_send_create = unsafe extern "C" fn(*const NDIlib_send_create_t) -> *mut std::ffi::c_void;
type NDIlib_send_destroy = unsafe extern "C" fn(*mut std::ffi::c_void);
type NDIlib_send_send_video_v2 = unsafe extern "C" fn(*mut std::ffi::c_void, *const NDIlib_video_frame_v2_t);
type NDIlib_util_send_send_audio_interleaved_32f = unsafe extern "C" fn(*mut std::ffi::c_void, *const NDIlib_audio_frame_interleaved_32f_t);
type NDIlib_send_get_no_connections = unsafe extern "C" fn(*mut std::ffi::c_void, u32) -> i32;
type NDIlib_send_get_tally = unsafe extern "C" fn(*mut std::ffi::c_void, *mut NDIlib_tally_t, u32) -> bool;

/// NDI audio frame - interleaved 32-bit float (for utility function)
#[repr(C)]
struct NDIlib_audio_frame_interleaved_32f_t {
    sample_rate: i32,
    no_channels: i32,
    no_samples: i32,
    timecode: i64,
    p_data: *const f32,
}

/// NDI send creation settings
#[repr(C)]
struct NDIlib_send_create_t {
    p_ndi_name: *const std::ffi::c_char,
    p_groups: *const std::ffi::c_char,
    clock_video: bool,
    clock_audio: bool,
}

/// NDI video frame
#[repr(C)]
struct NDIlib_video_frame_v2_t {
    xres: i32,
    yres: i32,
    four_cc: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    picture_aspect_ratio: f32,
    frame_format_type: i32,
    timecode: i64,
    p_data: *const u8,
    line_stride_in_bytes: i32,
    p_metadata: *const std::ffi::c_char,
    timestamp: i64,
}


/// NDI tally state
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct NDIlib_tally_t {
    on_program: bool,
    on_preview: bool,
}

// NDI FourCC codes (using NDI_LIB_FOURCC macro byte order)
const FOURCC_BGRA: u32 = 0x41524742; // 'BGRA'

/// NDI tally information (public)
#[derive(Clone, Copy, Debug, Default)]
pub struct NdiTally {
    pub on_program: bool,
    pub on_preview: bool,
}

/// NDI sender configuration
#[derive(Clone, Debug)]
pub struct NdiConfig {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate_n: u32,
    pub frame_rate_d: u32,
    /// Source audio sample rate (for resampling to NDI's 48kHz)
    pub source_sample_rate: u32,
}

impl Default for NdiConfig {
    fn default() -> Self {
        Self {
            name: "REACT by Compeller.ai".to_string(),
            width: 1920,
            height: 1080,
            frame_rate_n: 60,
            frame_rate_d: 1,
            source_sample_rate: 48000, // Default assumes 48kHz
        }
    }
}

/// NDI library wrapper
struct NdiLib {
    _library: Library,
    initialize: NDIlib_initialize,
    destroy: NDIlib_destroy,
    send_create: NDIlib_send_create,
    send_destroy: NDIlib_send_destroy,
    send_video: NDIlib_send_send_video_v2,
    send_audio_interleaved: NDIlib_util_send_send_audio_interleaved_32f,
    get_no_connections: NDIlib_send_get_no_connections,
    get_tally: NDIlib_send_get_tally,
}

impl NdiLib {
    fn load() -> Result<Self> {
        let lib_path = Self::find_library_path()?;
        log::info!("[NDI] Loading library from: {:?}", lib_path);

        unsafe {
            let library = Library::new(&lib_path)
                .map_err(|e| anyhow::anyhow!("Failed to load libndi: {}", e))?;

            let initialize: Symbol<NDIlib_initialize> = library.get(b"NDIlib_initialize")?;
            let destroy: Symbol<NDIlib_destroy> = library.get(b"NDIlib_destroy")?;
            let send_create: Symbol<NDIlib_send_create> = library.get(b"NDIlib_send_create")?;
            let send_destroy: Symbol<NDIlib_send_destroy> = library.get(b"NDIlib_send_destroy")?;
            let send_video: Symbol<NDIlib_send_send_video_v2> = library.get(b"NDIlib_send_send_video_v2")?;
            let send_audio_interleaved: Symbol<NDIlib_util_send_send_audio_interleaved_32f> = library.get(b"NDIlib_util_send_send_audio_interleaved_32f")?;
            let get_no_connections: Symbol<NDIlib_send_get_no_connections> = library.get(b"NDIlib_send_get_no_connections")?;
            let get_tally: Symbol<NDIlib_send_get_tally> = library.get(b"NDIlib_send_get_tally")?;

            Ok(Self {
                initialize: *initialize,
                destroy: *destroy,
                send_create: *send_create,
                send_destroy: *send_destroy,
                send_video: *send_video,
                send_audio_interleaved: *send_audio_interleaved,
                get_no_connections: *get_no_connections,
                get_tally: *get_tally,
                _library: library,
            })
        }
    }

    fn find_library_path() -> Result<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();

        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.canonicalize().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
                paths.push(exe_dir.join("libndi.dylib"));
                paths.push(exe_dir.join("ndi-sdk/lib/libndi.dylib"));
                paths.push(exe_dir.join("../Resources/libndi.dylib"));
                paths.push(exe_dir.join("../Frameworks/libndi.dylib"));
                if let Some(target) = exe_dir.parent() {
                    if let Some(repo_root) = target.parent() {
                        paths.push(repo_root.join("ndi-sdk/lib/libndi.dylib"));
                    }
                }
            }
        }
        paths.push(PathBuf::from("ndi-sdk/lib/libndi.dylib"));

        for path in &paths {
            if path.exists() {
                return Ok(path.clone());
            }
        }
        anyhow::bail!("Could not find libndi.dylib")
    }

    fn init(&self) -> bool {
        unsafe { (self.initialize)() }
    }

    fn create_sender(&self, name: &str, clock_video: bool, clock_audio: bool) -> Result<*mut std::ffi::c_void> {
        let name_cstr = std::ffi::CString::new(name)?;
        let settings = NDIlib_send_create_t {
            p_ndi_name: name_cstr.as_ptr(),
            p_groups: std::ptr::null(),
            clock_video,
            clock_audio,
        };
        let sender = unsafe { (self.send_create)(&settings) };
        if sender.is_null() {
            anyhow::bail!("Failed to create NDI sender");
        }
        Ok(sender)
    }

    fn send_frame(&self, sender: *mut std::ffi::c_void, frame: &NDIlib_video_frame_v2_t) {
        unsafe { (self.send_video)(sender, frame) }
    }

    fn send_audio_interleaved(&self, sender: *mut std::ffi::c_void, frame: &NDIlib_audio_frame_interleaved_32f_t) {
        unsafe { (self.send_audio_interleaved)(sender, frame) }
    }

    fn get_connections(&self, sender: *mut std::ffi::c_void) -> i32 {
        unsafe { (self.get_no_connections)(sender, 0) }
    }

    fn get_tally(&self, sender: *mut std::ffi::c_void) -> NdiTally {
        let mut tally = NDIlib_tally_t::default();
        unsafe { (self.get_tally)(sender, &mut tally, 0) };
        NdiTally { on_program: tally.on_program, on_preview: tally.on_preview }
    }

    fn destroy_sender(&self, sender: *mut std::ffi::c_void) {
        unsafe { (self.send_destroy)(sender) }
    }
}

impl Drop for NdiLib {
    fn drop(&mut self) {
        unsafe { (self.destroy)() }
    }
}

/// NDI output sender
pub struct NdiSender {
    config: NdiConfig,
    active: Arc<AtomicBool>,
    audio_enabled: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,
    frame_ring: Arc<FrameRing>,
    /// Push-based audio buffer (collects samples from writer thread, pre-gained)
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    frames_sent: Arc<AtomicU32>,
    connections: Arc<AtomicU32>,
    tally: Arc<Mutex<NdiTally>>,
    ndi_available: bool,
}

impl NdiSender {
    pub fn new(
        config: NdiConfig,
        frame_ring: Arc<FrameRing>,
    ) -> Result<Self> {
        let ndi_available = Self::check_ndi_available();
        if !ndi_available {
            log::warn!("[NDI] NDI library not available");
        }
        log::info!("[NDI] Audio: external-clocked 48kHz stereo f32 (no internal resampling)");

        Ok(Self {
            config,
            active: Arc::new(AtomicBool::new(false)),
            audio_enabled: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
            frame_ring,
            // Pre-allocate ~1 second buffer at 48kHz stereo
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(96000))),
            frames_sent: Arc::new(AtomicU32::new(0)),
            connections: Arc::new(AtomicU32::new(0)),
            tally: Arc::new(Mutex::new(NdiTally::default())),
            ndi_available,
        })
    }

    /// Push audio samples to the NDI sender (called from render loop)
    /// samples: interleaved stereo f32 [L0, R0, L1, R1, ...]
    pub fn push_audio(&self, samples: &[f32]) {
        if !self.active.load(Ordering::Acquire) || !self.audio_enabled.load(Ordering::Acquire) {
            return;
        }
        self.audio_buffer.lock().extend_from_slice(samples);
    }

    /// Get Arc reference to the audio buffer (for direct sink dispatch from capture callback)
    pub fn get_audio_buffer_arc(&self) -> Arc<Mutex<Vec<f32>>> {
        self.audio_buffer.clone()
    }

    fn check_ndi_available() -> bool {
        match NdiLib::load() {
            Ok(lib) => lib.init(),
            Err(_) => false,
        }
    }

    pub fn set_audio_enabled(&self, enabled: bool) {
        self.audio_enabled.store(enabled, Ordering::Release);
        log::info!("[NDI] Audio {}", if enabled { "enabled" } else { "disabled" });
    }

    pub fn is_audio_enabled(&self) -> bool {
        self.audio_enabled.load(Ordering::Acquire)
    }

    pub fn start(&mut self) -> Result<()> {
        if !self.ndi_available {
            anyhow::bail!("NDI not available");
        }
        if self.active.load(Ordering::Acquire) {
            return Ok(());
        }

        log::info!("[NDI] Starting: {} @ {}x{} {}fps",
            self.config.name, self.config.width, self.config.height,
            self.config.frame_rate_n / self.config.frame_rate_d);

        self.active.store(true, Ordering::Release);

        let active = self.active.clone();
        let audio_enabled = self.audio_enabled.clone();
        let frame_ring = self.frame_ring.clone();
        let audio_buffer = self.audio_buffer.clone();
        let frames_sent = self.frames_sent.clone();
        let connections = self.connections.clone();
        let tally = self.tally.clone();
        let config = self.config.clone();

        let handle = thread::Builder::new()
            .name("ndi-sender".to_string())
            .spawn(move || {
                Self::sender_thread(active, audio_enabled, frame_ring, audio_buffer,
                    frames_sent, connections, tally, config);
            })?;

        self.thread_handle = Some(handle);
        Ok(())
    }

    fn sender_thread(
        active: Arc<AtomicBool>,
        audio_enabled: Arc<AtomicBool>,
        frame_ring: Arc<FrameRing>,
        audio_buffer: Arc<Mutex<Vec<f32>>>,
        frames_sent: Arc<AtomicU32>,
        connections: Arc<AtomicU32>,
        tally: Arc<Mutex<NdiTally>>,
        config: NdiConfig,
    ) {
        log::info!("[NDI] Thread started");

        let lib = match NdiLib::load() {
            Ok(lib) => lib,
            Err(e) => {
                log::error!("[NDI] Failed to load: {}", e);
                active.store(false, Ordering::Release);
                return;
            }
        };

        if !lib.init() {
            log::error!("[NDI] Init failed");
            active.store(false, Ordering::Release);
            return;
        }

        // Use clock_video=true for frame rate pacing, clock_audio=false to avoid blocking
        // Audio timing is handled by our time-based sending logic instead
        let sender = match lib.create_sender(&config.name, true, false) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[NDI] Create sender failed: {}", e);
                active.store(false, Ordering::Release);
                return;
            }
        };

        log::info!("[NDI] Sender created: {}", config.name);

        // Video state - pre-allocate for HD (1920x1080 BGRA), will grow if 4K
        let mut bgra_buffer: Vec<u8> = Vec::with_capacity(1920 * 1080 * 4);
        let mut last_frame_num: u32 = 0;
        let mut fps_counter = 0u32;
        let mut last_fps_time = std::time::Instant::now();
        let mut last_stats_time = std::time::Instant::now();
        let startup_time = std::time::Instant::now();

        // Video timecode tracking - use explicit monotonic timecodes
        // NDI timecodes are in 100-nanosecond units (10MHz)
        // For 60fps video: 1 frame = 1/60 sec = 10_000_000/60 = 166666.67 ticks
        let mut video_timecode: i64 = 0;
        let ticks_per_frame: i64 = 10_000_000 * config.frame_rate_d as i64 / config.frame_rate_n as i64;

        // Audio: Fixed NDI output rate and chunk size (canonical from AudioBus)
        const NDI_SAMPLE_RATE: i32 = 48000;
        const CHANNELS: i32 = 2;
        const SAMPLES_PER_CHUNK: usize = 960; // 20ms at 48kHz
        const NEEDED: usize = SAMPLES_PER_CHUNK * 2; // stereo interleaved samples per chunk

        // NDI SDK synthesized timecode: lets SDK generate monotonic timestamps
        const TIMECODE_SYNTHESIZE: i64 = i64::MAX;

        // Pre-allocate NDI send buffer (gain-applied copy)
        let mut ndi_audio_chunk: [f32; NEEDED] = [0.0; NEEDED];

        // Audio stats
        let mut audio_sends = 0u32;
        let mut audio_stats_time = std::time::Instant::now();
        let mut audio_first_send = true;

        log::info!("[NDI] Audio: external-clocked, rate=48000 channels=2 layout=interleaved sample_type=f32 timecode=synthesize");

        while active.load(Ordering::Acquire) {
            // === AUDIO - external-clocked, unity passthrough ===
            // Audio arrives pre-gained and limited from the writer thread.
            // Just drain and send immediately — no gain, no processing.
            if audio_enabled.load(Ordering::Acquire) {
                // Drain all available samples from audio_buffer
                let mut buf = audio_buffer.lock();
                while buf.len() >= NEEDED {
                    ndi_audio_chunk.copy_from_slice(&buf[..NEEDED]);
                    buf.drain(..NEEDED);

                    // One-time semantic verification log
                    if audio_first_send {
                        audio_first_send = false;
                        let peak = ndi_audio_chunk.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                        log::info!("[NDI] Audio SEND verification: no_samples={} no_channels={} interleaved_len={} peak={:.3} timecode=SYNTHESIZE",
                            SAMPLES_PER_CHUNK, CHANNELS, NEEDED, peak);
                    }

                    // Send to NDI SDK with synthesized timecode (SDK handles timing)
                    let audio_frame = NDIlib_audio_frame_interleaved_32f_t {
                        sample_rate: NDI_SAMPLE_RATE,
                        no_channels: CHANNELS,
                        no_samples: SAMPLES_PER_CHUNK as i32,
                        timecode: TIMECODE_SYNTHESIZE,
                        p_data: ndi_audio_chunk.as_ptr(),
                    };
                    lib.send_audio_interleaved(sender, &audio_frame);
                    audio_sends += 1;
                }
                drop(buf);

                // Log audio stats every 5 seconds
                if audio_stats_time.elapsed().as_secs() >= 5 {
                    let elapsed = audio_stats_time.elapsed().as_secs_f32();
                    if audio_sends > 0 {
                        let max_l = (0..SAMPLES_PER_CHUNK).map(|i| ndi_audio_chunk[i * 2].abs()).fold(0.0f32, f32::max);
                        let max_r = (0..SAMPLES_PER_CHUNK).map(|i| ndi_audio_chunk[i * 2 + 1].abs()).fold(0.0f32, f32::max);
                        log::info!("[NDI] Audio: {:.1} sends/sec, peak L={:.3} R={:.3}",
                            audio_sends as f32 / elapsed, max_l, max_r);
                    } else {
                        log::warn!("[NDI] No audio sent in 5s");
                    }
                    audio_sends = 0;
                    audio_stats_time = std::time::Instant::now();
                }
            }

            // === VIDEO ===
            if let Some((rgba, width, height, frame_num)) = frame_ring.try_read() {
                if frame_num != last_frame_num {
                    last_frame_num = frame_num;

                    let size = (width * height * 4) as usize;
                    if bgra_buffer.len() != size {
                        bgra_buffer.resize(size, 0);
                    }

                    Self::rgba_to_bgra_flip(rgba, &mut bgra_buffer, width, height);

                    // Use explicit monotonic timecode (NOT -1 which can cause issues)
                    let frame = NDIlib_video_frame_v2_t {
                        xres: width as i32,
                        yres: height as i32,
                        four_cc: FOURCC_BGRA,
                        frame_rate_n: config.frame_rate_n as i32,
                        frame_rate_d: config.frame_rate_d as i32,
                        picture_aspect_ratio: width as f32 / height as f32,
                        frame_format_type: 1, // progressive
                        timecode: video_timecode,
                        p_data: bgra_buffer.as_ptr(),
                        line_stride_in_bytes: (width * 4) as i32,
                        p_metadata: std::ptr::null(),
                        timestamp: -1,
                    };

                    lib.send_frame(sender, &frame);
                    frames_sent.fetch_add(1, Ordering::Relaxed);
                    // Advance video timecode by one frame duration
                    video_timecode += ticks_per_frame;
                    fps_counter += 1;
                }
            }

            // Update stats once per second (avoid jitter from per-frame updates)
            // Wait 3 seconds after startup for NDI to stabilize before counting
            if last_stats_time.elapsed().as_millis() >= 1000 {
                if startup_time.elapsed().as_secs() >= 3 {
                    // NDI reports ~2x actual receivers (internal connection handling)
                    let raw_count = lib.get_connections(sender).max(0) as u32;
                    let receiver_count = (raw_count + 1) / 2;
                    connections.store(receiver_count, Ordering::Relaxed);
                }
                *tally.lock() = lib.get_tally(sender);
                last_stats_time = std::time::Instant::now();
            }

            // Log FPS
            if last_fps_time.elapsed().as_secs() >= 5 {
                let raw_conn = lib.get_connections(sender).max(0) as u32;
                log::info!("[NDI] {}fps, {} receivers (raw: {})", fps_counter / 5, connections.load(Ordering::Relaxed), raw_conn);
                fps_counter = 0;
                last_fps_time = std::time::Instant::now();
            }

            thread::sleep(std::time::Duration::from_micros(500));
        }

        lib.destroy_sender(sender);
        log::info!("[NDI] Thread stopped");
    }

    fn rgba_to_bgra_flip(rgba: &[u8], bgra: &mut [u8], width: u32, height: u32) {
        let row_size = (width * 4) as usize;
        for y in 0..height as usize {
            let src_start = (height as usize - 1 - y) * row_size;
            let dst_start = y * row_size;
            for (src, dst) in rgba[src_start..src_start + row_size].chunks_exact(4)
                .zip(bgra[dst_start..dst_start + row_size].chunks_exact_mut(4)) {
                dst[0] = src[2];
                dst[1] = src[1];
                dst[2] = src[0];
                dst[3] = src[3];
            }
        }
    }

    pub fn stop(&mut self) {
        if !self.active.load(Ordering::Acquire) { return; }
        log::info!("[NDI] Stopping...");
        self.active.store(false, Ordering::Release);
        if let Some(h) = self.thread_handle.take() { let _ = h.join(); }
        log::info!("[NDI] Stopped");
    }

    pub fn is_active(&self) -> bool { self.active.load(Ordering::Acquire) }
    pub fn is_available(&self) -> bool { self.ndi_available }
    pub fn frames_sent(&self) -> u32 { self.frames_sent.load(Ordering::Relaxed) }
    pub fn connections(&self) -> u32 { self.connections.load(Ordering::Relaxed) }
    pub fn tally(&self) -> NdiTally { *self.tally.lock() }
    pub fn config(&self) -> &NdiConfig { &self.config }
    pub fn set_config(&mut self, config: NdiConfig) { self.config = config; }
}

impl Drop for NdiSender {
    fn drop(&mut self) { self.stop(); }
}
