//! Dual RTMP Streamer for simultaneous Twitch + YouTube streaming
//!
//! Uses a single ffmpeg process with split filter and tee muxer to send
//! optimized streams to both platforms:
//! - Twitch: 6000kbps (platform max for most users)
//! - YouTube: 12000kbps (higher quality allowed)
//!
//! This approach is more efficient than running two separate encoders
//! and ensures perfect audio/video sync between platforms.

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
use super::rtmp_streamer::RtmpStreamState;

/// Dual RTMP streaming configuration
/// Twitch = 1080p60 @ 6000k, YouTube = 4K30 @ 30000k
#[derive(Clone)]
pub struct DualRtmpConfig {
    /// Twitch bitrate in kbps (1080p60 = 6000k)
    pub twitch_bitrate_kbps: u32,
    /// Twitch frame rate (60fps for smooth playback)
    pub twitch_fps: u32,
    /// YouTube bitrate in kbps (4K30 = 30000k)
    pub youtube_bitrate_kbps: u32,
    /// YouTube frame rate (30fps for 4K)
    pub youtube_fps: u32,
    /// Audio bitrate in kbps (160k AAC)
    pub audio_bitrate_kbps: u32,
}

impl Default for DualRtmpConfig {
    fn default() -> Self {
        Self {
            twitch_bitrate_kbps: 6000,   // 1080p60 - Twitch max
            twitch_fps: 60,
            youtube_bitrate_kbps: 30000, // 4K30 - high quality YouTube
            youtube_fps: 30,
            audio_bitrate_kbps: 160,     // AAC 160k stereo
        }
    }
}

/// Dual RTMP Streamer statistics
#[derive(Clone, Default)]
pub struct DualRtmpStats {
    pub state: RtmpStreamState,
    pub duration_secs: f32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
}

/// Dual RTMP Streamer - streams to both Twitch and YouTube simultaneously
pub struct DualRtmpStreamer {
    state: Arc<Mutex<RtmpStreamState>>,
    start_time: Arc<Mutex<Option<Instant>>>,
    frames_sent: Arc<AtomicU64>,
    frames_dropped: Arc<AtomicU64>,
    frame_ring: Arc<FrameRing>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    thread_handle: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    config: DualRtmpConfig,
}

use super::rtmp_streamer::find_ffmpeg;

impl DualRtmpStreamer {
    pub fn new(frame_ring: Arc<FrameRing>, config: DualRtmpConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(RtmpStreamState::Idle)),
            start_time: Arc::new(Mutex::new(None)),
            frames_sent: Arc::new(AtomicU64::new(0)),
            frames_dropped: Arc::new(AtomicU64::new(0)),
            frame_ring,
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(65536))),
            thread_handle: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    /// Push audio samples for streaming
    pub fn push_audio(&self, samples: &[f32]) {
        let state = *self.state.lock();
        if !matches!(state, RtmpStreamState::Starting | RtmpStreamState::Streaming) {
            return;
        }
        let mut buf = self.audio_buffer.lock();
        if buf.len() < 192000 {
            buf.extend_from_slice(samples);
        }
    }

    /// Get current streaming state
    pub fn get_state(&self) -> RtmpStreamState {
        *self.state.lock()
    }

    /// Get streaming statistics
    pub fn get_stats(&self) -> DualRtmpStats {
        let duration_secs = self.start_time.lock()
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);

        DualRtmpStats {
            state: *self.state.lock(),
            duration_secs,
            frames_sent: self.frames_sent.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
        }
    }

    /// Check if streaming
    pub fn is_streaming(&self) -> bool {
        let state = *self.state.lock();
        matches!(state, RtmpStreamState::Starting | RtmpStreamState::Streaming | RtmpStreamState::Reconnecting)
    }

    /// Start dual streaming to both Twitch and YouTube
    /// SECURITY: Never log the full URLs (they contain stream keys)
    pub fn start(&mut self, twitch_rtmp_url: String, youtube_rtmp_url: String) -> Result<()> {
        if self.is_streaming() {
            anyhow::bail!("Already streaming");
        }

        log::info!("[DualRTMP] Starting dual stream to Twitch + YouTube...");

        self.frames_sent.store(0, Ordering::Relaxed);
        self.frames_dropped.store(0, Ordering::Relaxed);
        *self.start_time.lock() = Some(Instant::now());
        self.audio_buffer.lock().clear();
        self.stop_flag.store(false, Ordering::Release);
        *self.state.lock() = RtmpStreamState::Starting;

        let state = self.state.clone();
        let frame_ring = self.frame_ring.clone();
        let audio_buffer = self.audio_buffer.clone();
        let frames_sent = self.frames_sent.clone();
        let frames_dropped = self.frames_dropped.clone();
        let stop_flag = self.stop_flag.clone();
        let config = self.config.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = Self::streaming_thread(
                state, frame_ring, audio_buffer,
                twitch_rtmp_url, youtube_rtmp_url,
                frames_sent, frames_dropped, stop_flag, config,
            ) {
                log::error!("[DualRTMP] Streaming error: {}", e);
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

        log::info!("[DualRTMP] Stopping dual stream...");
        self.stop_flag.store(true, Ordering::Release);

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        *self.state.lock() = RtmpStreamState::Stopped;

        let stats = self.get_stats();
        log::info!("[DualRTMP] Stopped: {} frames sent, {} dropped, {:.1}s",
            stats.frames_sent, stats.frames_dropped, stats.duration_secs);

        Ok(())
    }

    fn streaming_thread(
        state: Arc<Mutex<RtmpStreamState>>,
        frame_ring: Arc<FrameRing>,
        audio_buffer: Arc<Mutex<Vec<f32>>>,
        twitch_rtmp_url: String,
        youtube_rtmp_url: String,
        frames_sent: Arc<AtomicU64>,
        frames_dropped: Arc<AtomicU64>,
        stop_flag: Arc<AtomicBool>,
        config: DualRtmpConfig,
    ) -> Result<()> {
        use std::process::{Command, Stdio};

        log::info!("[DualRTMP] Streaming thread started, waiting for frame dimensions...");

        // Wait for dimensions
        let mut wait_count = 0u32;
        let (width, height) = loop {
            if stop_flag.load(Ordering::Acquire) {
                log::info!("[DualRTMP] Stop flag set while waiting for dimensions");
                *state.lock() = RtmpStreamState::Stopped;
                return Ok(());
            }
            let dims = frame_ring.dimensions();
            if dims.0 > 0 && dims.1 > 0 {
                break dims;
            }
            wait_count += 1;
            if wait_count % 100 == 0 {
                log::info!("[DualRTMP] Still waiting for dimensions... ({}ms)", wait_count * 10);
            }
            thread::sleep(std::time::Duration::from_millis(10));
        };
        log::info!("[DualRTMP] Got dimensions: {}x{}", width, height);

        // Calculate settings based on spec:
        // Twitch = 1080p60 @ 6000k, YouTube = 4K30 @ 30000k
        let twitch_fps = config.twitch_fps;
        let youtube_fps = config.youtube_fps;
        let twitch_gop = twitch_fps * 2; // 2-second GOP = 120 frames at 60fps
        let youtube_gop = youtube_fps * 2; // 2-second GOP = 60 frames at 30fps
        let twitch_bitrate = format!("{}k", config.twitch_bitrate_kbps);
        let twitch_bufsize = format!("{}k", config.twitch_bitrate_kbps * 2);
        let youtube_bitrate = format!("{}k", config.youtube_bitrate_kbps);
        let youtube_bufsize = format!("{}k", config.youtube_bitrate_kbps * 2);
        let audio_bitrate = format!("{}k", config.audio_bitrate_kbps);

        log::info!("[DualRTMP] Dual encode: Twitch 1080p{}@{}k, YouTube 4K{}@{}k",
            twitch_fps, config.twitch_bitrate_kbps, youtube_fps, config.youtube_bitrate_kbps);

        // Create audio FIFO pipe
        let audio_fifo_path = PathBuf::from("/tmp/react_dual_audio_fifo");
        let _ = fs::remove_file(&audio_fifo_path);

        unsafe {
            let path_cstr = std::ffi::CString::new(audio_fifo_path.to_str().unwrap()).unwrap();
            if libc::mkfifo(path_cstr.as_ptr(), 0o644) != 0 {
                log::error!("[DualRTMP] Failed to create audio FIFO");
                *state.lock() = RtmpStreamState::Error;
                return Err(anyhow::anyhow!("Failed to create audio FIFO"));
            }
        }
        log::info!("[DualRTMP] Created audio FIFO at {:?}", audio_fifo_path);

        // Audio will be written inline with video (same timing loop) to prevent drift
        let fifo_path_clone = audio_fifo_path.clone();
        let (fifo_tx, fifo_rx) = std::sync::mpsc::channel::<File>();

        let fifo_opener = thread::spawn(move || {
            log::info!("[DualRTMP Audio] Opening FIFO (blocks until ffmpeg ready)...");
            match File::options().write(true).open(&fifo_path_clone) {
                Ok(f) => {
                    log::info!("[DualRTMP Audio] FIFO connected");
                    let _ = fifo_tx.send(f);
                }
                Err(e) => {
                    log::error!("[DualRTMP Audio] Failed to open FIFO: {}", e);
                }
            }
        });

        // Build the dual-encode ffmpeg command matching the spec exactly:
        // Twitch = 1080p60 @ 6000k, YouTube = 4K30 @ 30000k
        log::info!("[DualRTMP] Starting ffmpeg with dual-encode...");

        // Filter complex: split → fps conversion → scale with lanczos → yuv420p
        let filter_complex = format!(
            "[0:v]split=2[vT][vY];\
             [vT]fps={},scale=1920:1080:flags=lanczos,format=yuv420p[v0];\
             [vY]fps={},scale=3840:2160:flags=lanczos,format=yuv420p[v1]",
            twitch_fps, youtube_fps
        );

        let ffmpeg_path = find_ffmpeg();
        let mut cmd = Command::new(&ffmpeg_path);
        cmd.arg("-y")
            // Use wall clock for timestamps - keeps A/V in sync
            .arg("-use_wallclock_as_timestamps").arg("1")
            // Input 0: raw video from pipe
            .arg("-thread_queue_size").arg("512")
            .arg("-f").arg("rawvideo")
            .arg("-pixel_format").arg("rgba")
            .arg("-video_size").arg(format!("{}x{}", width, height))
            .arg("-framerate").arg("60")
            .arg("-i").arg("pipe:0")
            // Input 1: raw audio from FIFO
            .arg("-thread_queue_size").arg("512")
            .arg("-f").arg("f32le")
            .arg("-ar").arg("48000")
            .arg("-ac").arg("2")
            .arg("-i").arg(audio_fifo_path.to_str().unwrap())
            // Filter: split, fps convert, scale with lanczos, format yuv420p
            .arg("-filter_complex").arg(&filter_complex)
            // Map: Twitch video [v0], YouTube video [v1], audio from input 1 (FIFO)
            .arg("-map").arg("[v0]")
            .arg("-map").arg("[v1]")
            .arg("-map").arg("1:a:0?") // Map audio from FIFO (optional)
            // Twitch video encoder (v:0) - 1080p60 @ 6000k CBR
            .arg("-c:v:0").arg("libx264")
            .arg("-preset:v:0").arg("fast")
            .arg("-profile:v:0").arg("high")
            .arg("-b:v:0").arg(&twitch_bitrate)
            .arg("-minrate:v:0").arg(&twitch_bitrate)
            .arg("-maxrate:v:0").arg(&twitch_bitrate)
            .arg("-bufsize:v:0").arg(&twitch_bufsize)
            .arg("-g:v:0").arg(twitch_gop.to_string())
            .arg("-keyint_min:v:0").arg(twitch_gop.to_string())
            .arg("-sc_threshold:v:0").arg("0")
            // YouTube video encoder (v:1) - 4K30 @ 30000k CBR
            .arg("-c:v:1").arg("libx264")
            .arg("-preset:v:1").arg("fast")
            .arg("-profile:v:1").arg("high")
            .arg("-b:v:1").arg(&youtube_bitrate)
            .arg("-minrate:v:1").arg(&youtube_bitrate)
            .arg("-maxrate:v:1").arg(&youtube_bitrate)
            .arg("-bufsize:v:1").arg(&youtube_bufsize)
            .arg("-g:v:1").arg(youtube_gop.to_string())
            .arg("-keyint_min:v:1").arg(youtube_gop.to_string())
            .arg("-sc_threshold:v:1").arg("0")
            // Audio filter - resample to sync with video
            .arg("-af").arg("aresample=async=1:first_pts=0")
            // Audio encoder - AAC 160k stereo 48kHz
            .arg("-c:a").arg("aac")
            .arg("-b:a").arg(&audio_bitrate)
            .arg("-ar").arg("48000")
            .arg("-ac").arg("2")
            // Tee muxer output to both platforms
            // select=v:0,a:0 means: first video stream + first audio for Twitch
            // select=v:1,a:0 means: second video stream + first audio for YouTube
            .arg("-f").arg("tee")
            .arg(format!(
                "[select=v\\:0,a\\:0:f=flv]{}|[select=v\\:1,a\\:0:f=flv]{}",
                twitch_rtmp_url, youtube_rtmp_url
            ))
            .stdin(Stdio::piped())
            .stderr(Stdio::piped());

        let ffmpeg = cmd.spawn();

        let mut ffmpeg = match ffmpeg {
            Ok(f) => {
                log::info!("[DualRTMP] ffmpeg process spawned successfully (pid: {:?})", f.id());
                f
            }
            Err(e) => {
                log::error!("[DualRTMP] Failed to start ffmpeg: {}", e);
                stop_flag.store(true, Ordering::Release);
                let _ = fifo_opener.join();
                let _ = fs::remove_file(&audio_fifo_path);
                *state.lock() = RtmpStreamState::Error;
                return Err(anyhow::anyhow!("Failed to start ffmpeg: {}", e));
            }
        };

        // Set state to Streaming - video first, audio FIFO connects async
        log::info!("[DualRTMP] Setting state to Streaming...");

        // Audio FIFO connects async - ffmpeg needs video first
        let mut audio_fifo: Option<File> = None;
        let mut fifo_check_count = 0u32;
        *state.lock() = RtmpStreamState::Streaming;

        let mut stdin = ffmpeg.stdin.take().expect("stdin");
        let mut last_frame_num: u32 = 0;
        let frame_size = (width * height * 4) as usize;
        let mut frame_buffer: Vec<u8> = vec![0u8; frame_size];

        // Audio params - write exactly this many samples per video frame (60fps)
        // 48kHz stereo / 60fps = 1600 stereo samples per frame
        let audio_samples_per_frame = (96000 / 60) as usize;
        let silence_buffer = vec![0.0f32; audio_samples_per_frame];
        const AUDIO_GAIN: f32 = 4.0;

        // Feed frames at 60fps - ffmpeg filter_complex handles fps conversion for each branch
        let frame_duration = std::time::Duration::from_secs_f64(1.0 / 60.0);
        let stream_start = Instant::now();
        let mut frame_count: u64 = 0;

        log::info!("[DualRTMP] Streaming to both platforms, {} audio samples/frame", audio_samples_per_frame);

        while !stop_flag.load(Ordering::Acquire) {
            let target_time = stream_start + frame_duration * frame_count as u32;
            let now = Instant::now();

            if now < target_time {
                thread::sleep(target_time - now);
            }

            // === WRITE VIDEO FRAME ===
            if let Some((pixels, w, h, frame_num)) = frame_ring.try_read() {
                if frame_num != last_frame_num && w == width && h == height {
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

                    if stdin.write_all(&frame_buffer).is_err() {
                        log::warn!("[DualRTMP] ffmpeg stdin closed");
                        *state.lock() = RtmpStreamState::Error;
                        break;
                    }
                    frames_sent.fetch_add(1, Ordering::Relaxed);
                }
            }

            // === WRITE AUDIO (same timing as video) ===
            // Check if FIFO is connected yet (ffmpeg opens it after video probe)
            if audio_fifo.is_none() {
                fifo_check_count += 1;
                if fifo_check_count % 60 == 1 {
                    if let Ok(f) = fifo_rx.try_recv() {
                        log::info!("[DualRTMP] Audio FIFO connected after {} frames", fifo_check_count);
                        audio_fifo = Some(f);
                    }
                }
            }

            if let Some(ref mut fifo) = audio_fifo {
                let samples: Vec<f32> = {
                    let mut buf = audio_buffer.lock();
                    if buf.len() >= audio_samples_per_frame {
                        buf.drain(..audio_samples_per_frame).collect()
                    } else {
                        silence_buffer.clone()
                    }
                };

                let bytes: Vec<u8> = samples.iter()
                    .map(|&s| (s * AUDIO_GAIN).clamp(-1.0, 1.0))
                    .flat_map(|s| s.to_le_bytes())
                    .collect();

                if fifo.write_all(&bytes).is_err() {
                    log::warn!("[DualRTMP] Audio FIFO write failed");
                    audio_fifo = None;
                }
            }

            frame_count += 1;
        }

        drop(stdin);
        drop(audio_fifo);
        let _ = fifo_opener.join();
        let _ = fs::remove_file(&audio_fifo_path);

        let output = ffmpeg.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::warn!("[DualRTMP] ffmpeg exited with error");
            log::debug!("[DualRTMP] stderr: {}", &stderr.chars().take(500).collect::<String>());
        }

        *state.lock() = RtmpStreamState::Stopped;
        log::info!("[DualRTMP] Dual stream ended");
        Ok(())
    }
}

impl Drop for DualRtmpStreamer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
