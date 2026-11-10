//! RTMP Streamer for YouTube Live and Twitch
//!
//! Streams video frames and audio to RTMP endpoints using ffmpeg.
//!
//! Features:
//! - Real audio streaming via named FIFO pipe with monotonic timestamps
//! - Low latency encoding for live streaming
//! - Automatic reconnection on drops
//! - Frame dropping to maintain stream health
//!
//! Audio Timestamp Fix:
//! Audio is written to FIFO in bursts (when samples arrive from NDI/audio source).
//! This caused "Queue input is backward in time" errors in FFmpeg because timestamps
//! weren't monotonically increasing. The fix is using `asetpts=N/SR/TB` filter which
//! regenerates timestamps based on sample count (N) divided by sample rate (SR),
//! ensuring perfectly monotonic audio timing regardless of write timing.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use anyhow::Result;
use parking_lot::Mutex;

use super::FrameRing;

/// RTMP streaming state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtmpStreamState {
    /// Not streaming
    Idle,
    /// Starting ffmpeg process
    Starting,
    /// Connected and streaming
    Streaming,
    /// Reconnecting after drop
    Reconnecting,
    /// Stopped
    Stopped,
    /// Error state
    Error,
}

impl Default for RtmpStreamState {
    fn default() -> Self {
        RtmpStreamState::Idle
    }
}

/// RTMP streaming statistics
#[derive(Clone, Default)]
pub struct RtmpStats {
    pub state: RtmpStreamState,
    pub duration_secs: f32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub reconnect_count: u32,
}

/// RTMP Streamer configuration
#[derive(Clone)]
pub struct RtmpConfig {
    /// Target bitrate in kbps (default 6000)
    pub bitrate_kbps: u32,
    /// Max GOP size (keyframe interval in frames)
    pub gop_size: u32,
    /// Target framerate
    pub fps: u32,
    /// Output width (for scaling, 0 = use input)
    pub output_width: u32,
    /// Output height (for scaling, 0 = use input)
    pub output_height: u32,
    /// Preset (fast for quality, veryfast for speed)
    pub preset: String,
    /// Use software encoder (libx264) instead of hardware (VideoToolbox)
    /// Software is more predictable for strict CBR streaming
    pub use_software_encoder: bool,
    /// Audio sample rate (from capture device)
    pub audio_sample_rate: u32,
}

impl Default for RtmpConfig {
    fn default() -> Self {
        Self {
            // Twitch recommended for 720p30
            bitrate_kbps: 3000,  // 3000 kbps for 720p30
            gop_size: 60,        // Keyframe every 2 seconds at 30fps
            fps: 30,
            output_width: 1280,  // 720p
            output_height: 720,
            preset: "medium".to_string(), // Balance of quality and speed
            use_software_encoder: true, // libx264 for predictable CBR
            audio_sample_rate: 48000, // Default, should be set from audio capture
        }
    }
}

/// RTMP Streamer
pub struct RtmpStreamer {
    state: Arc<Mutex<RtmpStreamState>>,
    start_time: Arc<Mutex<Option<Instant>>>,
    frames_sent: Arc<AtomicU64>,
    frames_dropped: Arc<AtomicU64>,
    reconnect_count: Arc<Mutex<u32>>,
    frame_ring: Arc<FrameRing>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    thread_handle: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    rtmp_url: Arc<Mutex<Option<String>>>,
    config: RtmpConfig,
}

/// Find ffmpeg binary - prefers embedded, then system
/// Used by YouTube/Dual streaming modules (Twitch uses ffmpeg-next Rust bindings)
pub fn find_ffmpeg() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            // macOS .app bundle: Contents/MacOS/../Resources/ffmpeg
            let bundle_path = exe_dir.join("../Resources/ffmpeg");
            if bundle_path.exists() {
                if let Some(path) = bundle_path.to_str() {
                    log::info!("[FFMPEG] Using embedded: {}", path);
                    return path.to_string();
                }
            }

            // Alongside executable (for development)
            let local_path = exe_dir.join("ffmpeg");
            if local_path.exists() {
                if let Some(path) = local_path.to_str() {
                    log::info!("[FFMPEG] Using local: {}", path);
                    return path.to_string();
                }
            }
        }
    }

    // Fall back to system ffmpeg
    #[cfg(target_os = "macos")]
    {
        for path in &["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "ffmpeg"] {
            if std::path::Path::new(path).exists() || *path == "ffmpeg" {
                log::info!("[FFMPEG] Using system: {}", path);
                return path.to_string();
            }
        }
        "/opt/homebrew/bin/ffmpeg".to_string()
    }

    #[cfg(not(target_os = "macos"))]
    {
        log::info!("[FFMPEG] Using system: ffmpeg");
        "ffmpeg".to_string()
    }
}

impl RtmpStreamer {
    pub fn new(frame_ring: Arc<FrameRing>, config: RtmpConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(RtmpStreamState::Idle)),
            start_time: Arc::new(Mutex::new(None)),
            frames_sent: Arc::new(AtomicU64::new(0)),
            frames_dropped: Arc::new(AtomicU64::new(0)),
            reconnect_count: Arc::new(Mutex::new(0)),
            frame_ring,
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(65536))),
            thread_handle: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            rtmp_url: Arc::new(Mutex::new(None)),
            config,
        }
    }

    /// Push audio samples for streaming
    pub fn push_audio(&self, samples: &[f32]) {
        let state = *self.state.lock();
        // Accept audio during Starting (buffering) and Streaming states
        if !matches!(state, RtmpStreamState::Starting | RtmpStreamState::Streaming) {
            return;
        }
        // Limit buffer size to ~2 seconds of audio (48000 * 2 channels * 2 seconds)
        let mut buf = self.audio_buffer.lock();
        if buf.len() < 192000 {
            buf.extend_from_slice(samples);
        }
    }

    /// Get Arc reference to the audio buffer (for direct sink dispatch from capture callback)
    pub fn get_audio_buffer_arc(&self) -> Arc<Mutex<Vec<f32>>> {
        self.audio_buffer.clone()
    }

    /// Get current streaming state
    pub fn get_state(&self) -> RtmpStreamState {
        *self.state.lock()
    }

    /// Get streaming statistics
    pub fn get_stats(&self) -> RtmpStats {
        let duration_secs = self.start_time.lock()
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);

        RtmpStats {
            state: *self.state.lock(),
            duration_secs,
            frames_sent: self.frames_sent.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            reconnect_count: *self.reconnect_count.lock(),
        }
    }

    /// Check if streaming
    pub fn is_streaming(&self) -> bool {
        let state = *self.state.lock();
        matches!(state, RtmpStreamState::Starting | RtmpStreamState::Streaming | RtmpStreamState::Reconnecting)
    }

    /// Start streaming to RTMP URL
    /// SECURITY: Never log the full rtmp_url (contains stream key)
    pub fn start(&mut self, rtmp_url: String) -> Result<()> {
        if self.is_streaming() {
            anyhow::bail!("Already streaming");
        }

        log::info!("[RTMP] start() called, spawning streaming thread...");

        self.frames_sent.store(0, Ordering::Relaxed);
        self.frames_dropped.store(0, Ordering::Relaxed);
        *self.reconnect_count.lock() = 0;
        *self.start_time.lock() = Some(Instant::now());
        *self.rtmp_url.lock() = Some(rtmp_url.clone());
        self.audio_buffer.lock().clear();
        self.stop_flag.store(false, Ordering::Release);
        *self.state.lock() = RtmpStreamState::Starting;

        let state = self.state.clone();
        let frame_ring = self.frame_ring.clone();
        let audio_buffer = self.audio_buffer.clone();
        let frames_sent = self.frames_sent.clone();
        let frames_dropped = self.frames_dropped.clone();
        let reconnect_count = self.reconnect_count.clone();
        let stop_flag = self.stop_flag.clone();
        let config = self.config.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = Self::streaming_thread(
                state, frame_ring, audio_buffer, rtmp_url,
                frames_sent, frames_dropped, reconnect_count, stop_flag, config,
            ) {
                log::error!("[RTMP] Streaming error: {}", e);
            }
        });

        self.thread_handle = Some(handle);
        Ok(())
    }

    /// Stop streaming
    pub fn stop(&mut self) -> Result<()> {
        if !self.is_streaming() {
            return Ok(());
        }

        log::info!("[RTMP] Stopping stream...");
        self.stop_flag.store(true, Ordering::Release);

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        *self.state.lock() = RtmpStreamState::Stopped;
        *self.rtmp_url.lock() = None;

        let stats = self.get_stats();
        log::info!("[RTMP] Stopped: {} frames sent, {} dropped, {:.1}s",
            stats.frames_sent, stats.frames_dropped, stats.duration_secs);

        Ok(())
    }

    fn streaming_thread(
        state: Arc<Mutex<RtmpStreamState>>,
        frame_ring: Arc<FrameRing>,
        audio_buffer: Arc<Mutex<Vec<f32>>>,
        rtmp_url: String,
        frames_sent: Arc<AtomicU64>,
        frames_dropped: Arc<AtomicU64>,
        _reconnect_count: Arc<Mutex<u32>>,
        stop_flag: Arc<AtomicBool>,
        config: RtmpConfig,
    ) -> Result<()> {
        use std::process::{Command, Stdio};

        log::info!("[RTMP] Streaming thread started, waiting for frame dimensions...");

        // Wait for dimensions
        let mut wait_count = 0u32;
        let (width, height) = loop {
            if stop_flag.load(Ordering::Acquire) {
                log::info!("[RTMP] Stop flag set while waiting for dimensions");
                *state.lock() = RtmpStreamState::Stopped;
                return Ok(());
            }
            let dims = frame_ring.dimensions();
            if dims.0 > 0 && dims.1 > 0 {
                break dims;
            }
            wait_count += 1;
            if wait_count % 100 == 0 {
                log::info!("[RTMP] Still waiting for dimensions... ({}ms)", wait_count * 10);
            }
            thread::sleep(std::time::Duration::from_millis(10));
        };
        log::info!("[RTMP] Got dimensions: {}x{}", width, height);

        log::info!("[RTMP] Streaming {}x{} at {}kbps (software_encoder={})",
            width, height, config.bitrate_kbps, config.use_software_encoder);

        // Choose encoder based on config
        // libx264 is more predictable for strict CBR streaming
        // VideoToolbox can be faster but less consistent bitrate control
        let encoder = if config.use_software_encoder {
            "libx264"
        } else {
            #[cfg(target_os = "macos")]
            { "h264_videotoolbox" }
            #[cfg(not(target_os = "macos"))]
            { "libx264" }
        };

        // Build ffmpeg command for RTMP streaming
        // Settings follow Twitch best practices: https://help.twitch.tv/s/article/broadcasting-guidelines
        let bitrate = format!("{}k", config.bitrate_kbps);
        // CBR mode: bufsize = bitrate for strict CBR (Twitch recommended)
        let bufsize = format!("{}k", config.bitrate_kbps);
        let gop = config.gop_size.to_string();
        let fps = config.fps.to_string();

        // Create audio FIFO for real-time audio input
        let audio_fifo_path = format!("/tmp/compeller_rtmp_audio_{}.fifo", std::process::id());

        // Remove existing FIFO if present (from crashed previous run)
        let _ = fs::remove_file(&audio_fifo_path);

        // Create named pipe (FIFO) for audio
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let fifo_result = std::process::Command::new("mkfifo")
                .arg(&audio_fifo_path)
                .status();
            if let Err(e) = fifo_result {
                log::error!("[RTMP] Failed to create audio FIFO: {}", e);
                *state.lock() = RtmpStreamState::Error;
                return Err(anyhow::anyhow!("Failed to create audio FIFO: {}", e));
            }
        }
        log::info!("[RTMP] Created audio FIFO: {}", audio_fifo_path);

        log::info!("[RTMP] Starting ffmpeg with real audio...");

        let ffmpeg_path = find_ffmpeg();
        let mut cmd = Command::new(&ffmpeg_path);
        cmd.arg("-y")
            // Minimize probing delay - we know exact format, start immediately
            .arg("-probesize").arg("32")
            .arg("-analyzeduration").arg("0")
            // Input 0: raw video from pipe
            .arg("-thread_queue_size").arg("512")
            .arg("-f").arg("rawvideo")
            .arg("-pixel_format").arg("rgba")
            .arg("-video_size").arg(format!("{}x{}", width, height))
            .arg("-framerate").arg(&fps)
            .arg("-i").arg("pipe:0")
            // Input 1: real audio from FIFO (f32le stereo)
            .arg("-f").arg("f32le")
            .arg("-ar").arg(config.audio_sample_rate.to_string())
            .arg("-ac").arg("2")
            .arg("-i").arg(&audio_fifo_path)
            // Explicit stream mapping
            .arg("-map").arg("0:v:0")   // Map video from input 0
            .arg("-map").arg("1:a:0");  // Map audio from FIFO

        // Scale to output resolution if specified AND different from input
        // Skip scaling if input already matches target
        let needs_scale = config.output_width > 0 && config.output_height > 0
            && (config.output_width != width || config.output_height != height);

        if needs_scale {
            // Use lanczos for high quality scaling + format to yuv420p
            cmd.arg("-vf").arg(format!(
                "scale={}:{}:flags=lanczos,format=yuv420p",
                config.output_width, config.output_height
            ));
            log::info!("[RTMP] Scaling {}x{} -> {}x{}", width, height, config.output_width, config.output_height);
        } else {
            // No scaling needed, just convert pixel format
            cmd.arg("-vf").arg("format=yuv420p");
            log::info!("[RTMP] No scaling needed, input {}x{} matches target", width, height);
        }

        cmd
            // Output framerate
            .arg("-r").arg(&fps)
            // Video encoding - strict CBR mode per Twitch best practices
            .arg("-c:v").arg(encoder)
            .arg("-b:v").arg(&bitrate)
            .arg("-minrate").arg(&bitrate)
            .arg("-maxrate").arg(&bitrate)
            .arg("-bufsize").arg(&bufsize)
            // Keyframe every 2 seconds
            .arg("-g").arg(&gop)
            .arg("-keyint_min").arg(&gop)
            .arg("-sc_threshold").arg("0")
            .arg("-profile:v").arg("high")
            .arg("-level").arg("4.2");

        // Encoder-specific quality settings
        if config.use_software_encoder || encoder == "libx264" {
            // libx264 settings for high quality streaming
            // - fast preset: good balance of quality and encoding speed
            // - NO zerolatency tune: it hurts quality significantly
            cmd.arg("-preset").arg(&config.preset)
               .arg("-threads").arg("0");  // Auto-detect threads
        } else {
            // VideoToolbox settings (hardware encoding) - like OBS
            // Much faster than libx264, uses dedicated GPU hardware
            #[cfg(target_os = "macos")]
            {
                cmd.arg("-realtime").arg("true")           // Low latency mode for live streaming
                   .arg("-allow_sw").arg("false")          // Use hardware only
                   .arg("-constant_bit_rate").arg("true")  // CBR mode (macOS 13+)
                   .arg("-coder").arg("cabac");            // Better compression
            }
        }

        cmd
            // Audio encoding - AAC 96k stereo at 48kHz
            .arg("-c:a").arg("aac")
            .arg("-b:a").arg("96k")
            .arg("-ar").arg("48000")
            .arg("-ac").arg("2")
            // Use shortest to stop when video ends
            .arg("-shortest")
            // Output format
            .arg("-f").arg("flv")
            .arg(&rtmp_url)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        // Spawn FFmpeg
        log::info!("[RTMP] Spawning FFmpeg...");
        let ffmpeg = cmd.spawn();

        let mut ffmpeg = match ffmpeg {
            Ok(f) => {
                log::info!("[RTMP] ffmpeg process spawned successfully (pid: {:?})", f.id());
                f
            }
            Err(e) => {
                log::error!("[RTMP] Failed to start ffmpeg: {}", e);
                stop_flag.store(true, Ordering::Release);
                *state.lock() = RtmpStreamState::Error;
                return Err(anyhow::anyhow!("Failed to start ffmpeg: {}", e));
            }
        };

        // Spawn thread to read FFmpeg stderr (prevents deadlock and shows errors)
        let stderr = ffmpeg.stderr.take();
        let _stderr_thread = thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            if let Some(stderr) = stderr {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        // Log important FFmpeg messages
                        if line.contains("error") || line.contains("Error") || line.contains("failed") {
                            log::error!("[FFMPEG] {}", line);
                        } else if line.contains("Output") || line.contains("Stream") {
                            log::info!("[FFMPEG] {}", line);
                        }
                    }
                }
            }
        });

        // Spawn audio writer thread - reads from audio_buffer, writes to FIFO
        // No silence padding - just write audio as it arrives
        let audio_stop = stop_flag.clone();
        let audio_buf = audio_buffer.clone();
        let fifo_path = audio_fifo_path.clone();
        let audio_rate = config.audio_sample_rate;
        let _audio_thread = thread::spawn(move || {
            log::info!("[AUDIO] Opening FIFO for writing: {} (sample rate: {}Hz)", fifo_path, audio_rate);

            // Open FIFO - this will block until FFmpeg opens it for reading
            let fifo = match File::options().write(true).open(&fifo_path) {
                Ok(f) => f,
                Err(e) => {
                    log::error!("[AUDIO] Failed to open FIFO: {}", e);
                    return;
                }
            };
            log::info!("[AUDIO] FIFO opened successfully");

            let mut fifo = std::io::BufWriter::with_capacity(65536, fifo);
            let mut samples_written: u64 = 0;

            while !audio_stop.load(Ordering::Acquire) {
                // Drain ALL available samples from buffer
                let samples_to_write: Vec<f32> = {
                    let mut buf = audio_buf.lock();
                    if !buf.is_empty() {
                        buf.drain(..).collect()
                    } else {
                        Vec::new()
                    }
                };

                if !samples_to_write.is_empty() {
                    // Convert f32 samples to bytes (little-endian)
                    let bytes: Vec<u8> = samples_to_write.iter()
                        .flat_map(|&s| s.to_le_bytes())
                        .collect();

                    if fifo.write_all(&bytes).is_err() {
                        log::warn!("[AUDIO] FIFO write failed, FFmpeg may have closed");
                        break;
                    }
                    let _ = fifo.flush();

                    samples_written += samples_to_write.len() as u64 / 2;

                    // Log every ~5 seconds
                    if samples_written % (audio_rate as u64 * 5) < 1000 {
                        log::info!("[AUDIO] Written {} samples (~{:.1}s)",
                            samples_written, samples_written as f64 / audio_rate as f64);
                    }
                }

                // Small sleep to avoid busy-waiting, but don't pace the writes
                thread::sleep(std::time::Duration::from_millis(5));
            }

            log::info!("[AUDIO] Writer thread ending, wrote {} samples total", samples_written);
        });

        // Set state to Streaming
        log::info!("[RTMP] Setting state to Streaming...");
        *state.lock() = RtmpStreamState::Streaming;
        log::info!("[RTMP] State is now Streaming with real audio");

        let mut stdin = ffmpeg.stdin.take().expect("stdin");
        let mut last_frame_num: u32 = 0;
        let frame_size = (width * height * 4) as usize;
        let mut frame_buffer: Vec<u8> = vec![0u8; frame_size];

        // Frame timing for consistent rate - like OBS, use fixed intervals
        let frame_duration = std::time::Duration::from_secs_f64(1.0 / config.fps as f64);
        let stream_start = Instant::now();
        let mut frame_count: u64 = 0;

        log::info!("[RTMP] Streaming at {}fps", config.fps);

        while !stop_flag.load(Ordering::Acquire) {
            // Calculate when this frame SHOULD be sent (fixed interval from start)
            let target_time = stream_start + frame_duration * frame_count as u32;
            let now = Instant::now();

            // Sleep until target time (if we're ahead)
            if now < target_time {
                thread::sleep(target_time - now);
            }

            // === WRITE VIDEO FRAME ===
            let frame_result = frame_ring.try_read();
            // Log every 60 frames (1 second) for debugging
            if frame_count % 60 == 0 {
                let sent = frames_sent.load(Ordering::Relaxed);
                if let Some((_, w, h, frame_num)) = frame_result {
                    log::info!("[RTMP] @{}: ring_fn={}, last={}, sent={}, dims={}x{}",
                        frame_count, frame_num, last_frame_num, sent, w, h);
                } else {
                    log::info!("[RTMP] @{}: NO FRAME, sent={}", frame_count, sent);
                }
            }
            if let Some((pixels, w, h, frame_num)) = frame_result {
                let should_write = frame_num != last_frame_num && w == width && h == height;
                if frame_count < 5 {
                    log::info!("[RTMP] Frame check @{}: fn={}, last={}, should_write={}",
                        frame_count, frame_num, last_frame_num, should_write);
                }
                if should_write {
                    let frames_behind = frame_num.saturating_sub(last_frame_num);
                    if frames_behind > 5 {
                        frames_dropped.fetch_add(frames_behind as u64 - 1, Ordering::Relaxed);
                    }

                    last_frame_num = frame_num;

                    // Flip Y (OpenGL to top-down)
                    let stride = (width * 4) as usize;
                    for y in 0..height as usize {
                        let src_y = height as usize - 1 - y;
                        let src_off = src_y * stride;
                        let dst_off = y * stride;
                        if src_off + stride <= pixels.len() && dst_off + stride <= frame_buffer.len() {
                            frame_buffer[dst_off..dst_off + stride]
                                .copy_from_slice(&pixels[src_off..src_off + stride]);
                        }
                    }

                    log::debug!("[RTMP] Writing frame {} to stdin...", frame_num);
                    if stdin.write_all(&frame_buffer).is_err() {
                        log::warn!("[RTMP] ffmpeg video stdin closed");
                        *state.lock() = RtmpStreamState::Error;
                        break;
                    }
                    log::debug!("[RTMP] Frame {} written successfully", frame_num);
                    frames_sent.fetch_add(1, Ordering::Relaxed);
                }
            }

            // Audio is handled by the audio writer thread via FIFO

            frame_count += 1;
        }

        // Close video stdin
        drop(stdin);

        // Wait for ffmpeg to exit
        let output = ffmpeg.wait_with_output()?;

        // Clean up audio FIFO
        let _ = fs::remove_file(&audio_fifo_path);
        log::info!("[RTMP] Cleaned up audio FIFO");

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Don't log the full error which might contain the stream key URL
            log::warn!("[RTMP] ffmpeg exited with error (check encoding settings)");
            log::debug!("[RTMP] stderr (first 200 chars): {}", &stderr.chars().take(200).collect::<String>());
        }

        *state.lock() = RtmpStreamState::Stopped;
        log::info!("[RTMP] Stream ended");
        Ok(())
    }
}

impl Drop for RtmpStreamer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
