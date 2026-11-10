//! Video decoder with async decoding
//!
//! On macOS, uses AVFoundation for hardware-accelerated video decoding.
//! Decoding happens on a background thread to avoid blocking the render loop.
//! Uses a ring buffer of pre-allocated frames to avoid per-frame allocations.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

#[cfg(target_os = "macos")]
use super::avfoundation::AVFoundationDecoder;

#[cfg(target_os = "macos")]
use super::texture_cache::RetainedPixelBuffer;

/// Number of frames in the ring buffer (must be power of 2 for fast modulo)
const FRAME_BUFFER_SIZE: usize = 4;

/// A single frame in the ring buffer
struct FrameSlot {
    data: Vec<u8>,
    width: u32,
    height: u32,
    ready: bool,
}

/// Ring buffer of pre-allocated frames
struct FrameRingBuffer {
    slots: Vec<FrameSlot>,
    write_index: usize,
    read_index: usize,
    frame_size: usize,
}

impl FrameRingBuffer {
    fn new(frame_size: usize) -> Self {
        let slots = (0..FRAME_BUFFER_SIZE)
            .map(|_| FrameSlot {
                data: vec![0u8; frame_size],
                width: 0,
                height: 0,
                ready: false,
            })
            .collect();

        Self {
            slots,
            write_index: 0,
            read_index: 0,
            frame_size,
        }
    }

    /// Get mutable reference to next write slot's buffer (for zero-copy decode)
    fn get_write_buffer(&mut self) -> Option<&mut [u8]> {
        let slot = &mut self.slots[self.write_index];
        if slot.data.len() == self.frame_size {
            Some(&mut slot.data)
        } else {
            None
        }
    }

    /// Mark current write slot as ready and advance write index
    fn commit_write(&mut self, width: u32, height: u32) {
        let slot = &mut self.slots[self.write_index];
        slot.width = width;
        slot.height = height;
        slot.ready = true;
        self.write_index = (self.write_index + 1) % FRAME_BUFFER_SIZE;
    }

    /// Get the latest ready frame for reading
    fn get_latest_frame(&mut self) -> Option<(&[u8], u32, u32)> {
        // Find the most recent ready frame
        let mut best_idx = None;
        for i in 0..FRAME_BUFFER_SIZE {
            // Check backwards from write_index to find most recent ready frame
            let idx = (self.write_index + FRAME_BUFFER_SIZE - 1 - i) % FRAME_BUFFER_SIZE;
            if self.slots[idx].ready {
                best_idx = Some(idx);
                break;
            }
        }

        if let Some(idx) = best_idx {
            let slot = &self.slots[idx];
            Some((&slot.data, slot.width, slot.height))
        } else {
            None
        }
    }

    fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.ready = false;
        }
        self.write_index = 0;
        self.read_index = 0;
    }
}

/// Zero-copy pixel buffer ring (for IOSurface path)
#[cfg(target_os = "macos")]
struct PixelBufferSlot {
    buffer: Option<RetainedPixelBuffer>,
    ready: bool,
}

#[cfg(target_os = "macos")]
struct PixelBufferRing {
    slots: Vec<PixelBufferSlot>,
    write_index: usize,
}

#[cfg(target_os = "macos")]
impl PixelBufferRing {
    fn new() -> Self {
        let slots = (0..FRAME_BUFFER_SIZE)
            .map(|_| PixelBufferSlot {
                buffer: None,
                ready: false,
            })
            .collect();

        Self {
            slots,
            write_index: 0,
        }
    }

    /// Store a pixel buffer in the next slot
    fn push(&mut self, buffer: RetainedPixelBuffer) {
        let slot = &mut self.slots[self.write_index];
        slot.buffer = Some(buffer);
        slot.ready = true;
        self.write_index = (self.write_index + 1) % FRAME_BUFFER_SIZE;
    }

    /// Get the most recent ready pixel buffer (clones the RetainedPixelBuffer)
    fn get_latest(&self) -> Option<RetainedPixelBuffer> {
        // Find the most recent ready frame (just before write_index)
        for i in 0..FRAME_BUFFER_SIZE {
            let idx = (self.write_index + FRAME_BUFFER_SIZE - 1 - i) % FRAME_BUFFER_SIZE;
            if self.slots[idx].ready {
                return self.slots[idx].buffer.clone();
            }
        }
        None
    }

    fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.buffer = None;
            slot.ready = false;
        }
        self.write_index = 0;
    }
}

/// Video decoder with async background decoding
pub struct VideoDecoder {
    /// Path to currently loaded video
    path: Option<String>,

    /// Video width
    width: u32,

    /// Video height
    height: u32,

    /// Duration in seconds
    duration: f64,

    /// Frame rate (fps)
    frame_rate: f64,

    /// Current playback position in seconds (shared with decode thread)
    position: Arc<AtomicU64>,

    /// Whether video is playing
    playing: Arc<AtomicBool>,

    /// Whether to loop playback
    looping: Arc<AtomicBool>,

    /// Signal to stop decode thread
    stop_signal: Arc<AtomicBool>,

    /// Ring buffer for decoded frames (no per-frame allocation)
    frame_ring: Arc<Mutex<FrameRingBuffer>>,

    /// Zero-copy pixel buffer ring (IOSurface path)
    #[cfg(target_os = "macos")]
    pixel_buffer_ring: Arc<Mutex<PixelBufferRing>>,

    /// Whether to use zero-copy IOSurface path
    #[cfg(target_os = "macos")]
    use_zero_copy: bool,

    /// Last frame number we rendered (to detect new frames)
    last_rendered_frame: u64,

    /// Frame counter from decode thread
    frame_counter: Arc<AtomicU64>,

    /// Decode thread handle
    decode_thread: Option<thread::JoinHandle<()>>,

    /// Local frame buffer for returning to caller
    local_buffer: Vec<u8>,
}

impl VideoDecoder {
    /// Create a new video decoder
    pub fn new() -> Self {
        Self {
            path: None,
            width: 0,
            height: 0,
            duration: 0.0,
            frame_rate: 30.0,
            position: Arc::new(AtomicU64::new(0)),
            playing: Arc::new(AtomicBool::new(false)),
            looping: Arc::new(AtomicBool::new(true)),
            stop_signal: Arc::new(AtomicBool::new(false)),
            frame_ring: Arc::new(Mutex::new(FrameRingBuffer::new(0))),
            #[cfg(target_os = "macos")]
            pixel_buffer_ring: Arc::new(Mutex::new(PixelBufferRing::new())),
            #[cfg(target_os = "macos")]
            use_zero_copy: true, // Enable zero-copy by default on macOS
            last_rendered_frame: 0,
            frame_counter: Arc::new(AtomicU64::new(0)),
            decode_thread: None,
            local_buffer: Vec::new(),
        }
    }

    /// Check if video decoding is available
    pub fn is_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            true // AVFoundation is always available on macOS
        }
        #[cfg(not(target_os = "macos"))]
        {
            cfg!(feature = "video") // FFmpeg required on other platforms
        }
    }

    /// Open a video file
    pub fn open(&mut self, path: &str) -> Result<()> {
        log::info!("[VideoDecoder] Opening video: {}", path);

        // Close any existing video
        self.close();

        // Verify file exists
        if !Path::new(path).exists() {
            return Err(anyhow!("Video file not found: {}", path));
        }

        #[cfg(target_os = "macos")]
        {
            // Open decoder to get metadata
            let mut decoder = AVFoundationDecoder::new();
            decoder.open(path)?;

            let (width, height) = decoder.dimensions();
            let frame_size = (width * height * 4) as usize;

            self.width = width;
            self.height = height;
            self.duration = decoder.duration();
            self.frame_rate = decoder.frame_rate();
            self.local_buffer = vec![0u8; frame_size];
            self.path = Some(path.to_string());
            self.position.store(0, Ordering::SeqCst);

            // Reset state
            self.stop_signal.store(false, Ordering::SeqCst);
            self.frame_counter.store(0, Ordering::SeqCst);
            self.last_rendered_frame = 0;

            // Initialize ring buffer with pre-allocated frames
            *self.frame_ring.lock() = FrameRingBuffer::new(frame_size);
            self.pixel_buffer_ring.lock().clear();

            // Start decode thread
            let path_clone = path.to_string();
            let playing = self.playing.clone();
            let looping = self.looping.clone();
            let stop_signal = self.stop_signal.clone();
            let frame_ring = self.frame_ring.clone();
            let pixel_buffer_ring = self.pixel_buffer_ring.clone();
            let use_zero_copy = self.use_zero_copy;
            let frame_counter = self.frame_counter.clone();
            let position = self.position.clone();
            let frame_rate = self.frame_rate;
            let duration = self.duration;

            self.decode_thread = Some(thread::spawn(move || {
                Self::decode_loop(
                    path_clone,
                    playing,
                    looping,
                    stop_signal,
                    frame_ring,
                    pixel_buffer_ring,
                    use_zero_copy,
                    frame_counter,
                    position,
                    frame_rate,
                    duration,
                );
            }));

            log::info!("[VideoDecoder] Video loaded: {}x{}, {:.2} fps, {:.2}s (ring buffer decode)",
                self.width, self.height, self.frame_rate, self.duration);

            Ok(())
        }

        #[cfg(not(target_os = "macos"))]
        {
            Err(anyhow!("Video decoding not available on this platform"))
        }
    }

    /// Background decode loop
    #[cfg(target_os = "macos")]
    fn decode_loop(
        path: String,
        playing: Arc<AtomicBool>,
        looping: Arc<AtomicBool>,
        stop_signal: Arc<AtomicBool>,
        frame_ring: Arc<Mutex<FrameRingBuffer>>,
        pixel_buffer_ring: Arc<Mutex<PixelBufferRing>>,
        use_zero_copy: bool,
        frame_counter: Arc<AtomicU64>,
        position_atomic: Arc<AtomicU64>,
        frame_rate: f64,
        duration: f64,
    ) {
        let mut decoder = AVFoundationDecoder::new();
        if decoder.open(&path).is_err() {
            log::error!("[VideoDecoder] Decode thread failed to open video");
            return;
        }

        let frame_duration = std::time::Duration::from_secs_f64(1.0 / frame_rate);
        let mut position = 0.0f64;
        let mut last_decode = std::time::Instant::now();

        log::info!("[VideoDecoder] Decode thread started (zero_copy={})", use_zero_copy);

        while !stop_signal.load(Ordering::SeqCst) {
            // Wait if not playing
            if !playing.load(Ordering::SeqCst) {
                thread::sleep(std::time::Duration::from_millis(10));
                last_decode = std::time::Instant::now();
                continue;
            }

            // Frame rate limiting with spin-wait for better precision
            let elapsed = last_decode.elapsed();
            if elapsed < frame_duration {
                let sleep_time = frame_duration - elapsed;
                // Sleep for most of the time, then spin-wait for precision
                if sleep_time > std::time::Duration::from_millis(2) {
                    thread::sleep(sleep_time - std::time::Duration::from_millis(1));
                }
                // Spin-wait for the remaining time (more precise than sleep)
                while last_decode.elapsed() < frame_duration {
                    std::hint::spin_loop();
                }
            }
            last_decode = std::time::Instant::now();

            // Decode next frame - use zero-copy path if enabled
            let got_frame = if use_zero_copy {
                // Zero-copy path: get retained pixel buffer directly
                if let Some(pixel_buffer) = decoder.decode_frame_zero_copy() {
                    frame_counter.fetch_add(1, Ordering::SeqCst);
                    pixel_buffer_ring.lock().push(pixel_buffer);
                    true
                } else {
                    false
                }
            } else {
                // Fallback path: copy frame data
                if let Some((data, width, height)) = decoder.decode_frame() {
                    frame_counter.fetch_add(1, Ordering::SeqCst);

                    // Copy directly to pre-allocated ring buffer slot (no allocation!)
                    let mut ring = frame_ring.lock();
                    if let Some(dst_buffer) = ring.get_write_buffer() {
                        if data.len() == dst_buffer.len() {
                            dst_buffer.copy_from_slice(data);
                            ring.commit_write(width, height);
                        }
                    }
                    drop(ring);
                    true
                } else {
                    false
                }
            };

            if got_frame {
                position += 1.0 / frame_rate;
                position_atomic.store(position.to_bits(), Ordering::SeqCst);

                // Check for end of video
                if position >= duration {
                    if looping.load(Ordering::SeqCst) {
                        log::info!("[VideoDecoder] Looping video");
                        decoder.close();
                        if decoder.open(&path).is_err() {
                            log::error!("[VideoDecoder] Failed to reopen video for loop");
                            break;
                        }
                        position = 0.0;
                        position_atomic.store(0, Ordering::SeqCst);
                    } else {
                        playing.store(false, Ordering::SeqCst);
                    }
                }
            } else {
                // No more frames
                if looping.load(Ordering::SeqCst) {
                    log::info!("[VideoDecoder] End of video, restarting");
                    decoder.close();
                    if decoder.open(&path).is_err() {
                        break;
                    }
                    position = 0.0;
                    position_atomic.store(0, Ordering::SeqCst);
                } else {
                    playing.store(false, Ordering::SeqCst);
                }
            }
        }

        decoder.close();
        log::info!("[VideoDecoder] Decode thread stopped");
    }

    /// Close the current video
    pub fn close(&mut self) {
        // Signal decode thread to stop
        self.stop_signal.store(true, Ordering::SeqCst);

        // Wait for thread to finish
        if let Some(handle) = self.decode_thread.take() {
            let _ = handle.join();
        }

        self.path = None;
        self.width = 0;
        self.height = 0;
        self.duration = 0.0;
        self.position.store(0, Ordering::SeqCst);
        self.playing.store(false, Ordering::SeqCst);
        self.frame_ring.lock().clear();
        #[cfg(target_os = "macos")]
        self.pixel_buffer_ring.lock().clear();
        self.local_buffer.clear();
    }

    /// Start playback
    pub fn play(&mut self) {
        if self.path.is_some() {
            self.playing.store(true, Ordering::SeqCst);
            log::info!("[VideoDecoder] Playback started");
        }
    }

    /// Pause playback
    pub fn pause(&mut self) {
        self.playing.store(false, Ordering::SeqCst);
        log::info!("[VideoDecoder] Playback paused");
    }

    /// Toggle play/pause
    pub fn toggle(&mut self) {
        if self.playing.load(Ordering::SeqCst) {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Seek to position (seconds) - Note: seek is approximate with async decode
    pub fn seek(&mut self, _position: f64) -> Result<()> {
        // TODO: Implement seek by signaling decode thread
        log::warn!("[VideoDecoder] Seek not yet implemented for async decode");
        Ok(())
    }

    /// Restart video from beginning
    pub fn restart(&mut self) -> Result<()> {
        if let Some(path) = self.path.clone() {
            let was_playing = self.playing.load(Ordering::SeqCst);
            self.open(&path)?;
            if was_playing {
                self.play();
            }
        }
        Ok(())
    }

    /// Set looping mode
    pub fn set_looping(&mut self, looping: bool) {
        self.looping.store(looping, Ordering::SeqCst);
    }

    /// Force the CPU frame path (vs the macOS IOSurface zero-copy path). Callers
    /// that read frames via [`Self::get_frame`] (CPU RGBA/BGRA bytes) — e.g. the
    /// virtual-stage screen — must disable zero-copy BEFORE `open`, since the
    /// zero-copy decode loop only fills the pixel-buffer ring, never `frame_ring`.
    pub fn set_zero_copy(&mut self, enabled: bool) {
        self.use_zero_copy = enabled;
    }

    /// Get the latest frame as RGBA data (non-blocking)
    pub fn get_frame(&mut self) -> Option<(&[u8], u32, u32)> {
        if !self.playing.load(Ordering::SeqCst) || self.path.is_none() {
            return None;
        }

        // Check if there's a new frame
        let current_frame = self.frame_counter.load(Ordering::SeqCst);
        if current_frame == self.last_rendered_frame {
            // No new frame, return cached if available
            if !self.local_buffer.is_empty() && self.width > 0 && self.height > 0 {
                return Some((&self.local_buffer, self.width, self.height));
            }
            return None;
        }

        // Get the latest frame from ring buffer
        // Using try_lock for non-blocking access
        if let Some(mut ring) = self.frame_ring.try_lock() {
            if let Some((data, width, height)) = ring.get_latest_frame() {
                // Copy to local buffer
                if data.len() == self.local_buffer.len() {
                    self.local_buffer.copy_from_slice(data);
                    self.last_rendered_frame = current_frame;
                    self.width = width;
                    self.height = height;
                }
            }
        }

        if !self.local_buffer.is_empty() && self.width > 0 && self.height > 0 {
            Some((&self.local_buffer, self.width, self.height))
        } else {
            None
        }
    }

    /// Get the latest frame as a zero-copy pixel buffer (IOSurface path)
    /// Returns a cloned RetainedPixelBuffer that can be passed to TextureCache
    #[cfg(target_os = "macos")]
    pub fn get_frame_zero_copy(&mut self) -> Option<RetainedPixelBuffer> {
        if !self.use_zero_copy {
            return None;
        }

        if !self.playing.load(Ordering::SeqCst) || self.path.is_none() {
            return None;
        }

        // Check if there's a new frame
        let current_frame = self.frame_counter.load(Ordering::SeqCst);
        if current_frame == self.last_rendered_frame {
            return None;
        }

        // Get the latest pixel buffer from ring (non-blocking)
        if let Some(ring) = self.pixel_buffer_ring.try_lock() {
            if let Some(pixel_buffer) = ring.get_latest() {
                self.last_rendered_frame = current_frame;
                return Some(pixel_buffer);
            }
        }

        None
    }

    /// Check if zero-copy mode is enabled
    #[cfg(target_os = "macos")]
    pub fn is_zero_copy(&self) -> bool {
        self.use_zero_copy
    }

    /// Check if video is loaded
    pub fn is_loaded(&self) -> bool {
        self.path.is_some() && self.decode_thread.is_some()
    }

    /// Check if video is playing
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::SeqCst)
    }

    /// Get video dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get video duration in seconds
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Get current position in seconds
    pub fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::SeqCst))
    }

    /// Get frame rate
    pub fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    /// Get current video path
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

impl Default for VideoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.close();
    }
}
