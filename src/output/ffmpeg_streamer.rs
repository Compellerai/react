//! RTMP Streamer using ffmpeg-next for proper A/V sync
//!
//! Uses FFmpeg's libav* libraries directly for encoding and muxing,
//! providing correct timestamps and A/V synchronization.
//!
//! Output Mode Architecture:
//! - 4 discrete presets: 720p30, 720p60, 1080p30, 1080p60
//! - Mode changes require full encoder reinit (no hot switching)
//! - All timing derived from frame counters (CFR), never wall clock
//! - Audio stays constant at 48kHz stereo AAC-LC 160kbps

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use anyhow::{Result, Context};
use parking_lot::Mutex;

use super::FrameRing;

extern crate ffmpeg_next as ffmpeg;

use ffmpeg::{
    codec, encoder, format, frame, media, picture, software, ChannelLayout, Dictionary, Packet, Rational,
};

/// Output mode - discrete presets for Twitch streaming
/// Mode changes are event-driven and require full encoder reinit
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum OutputMode {
    /// 720p @ 30fps, 3000 kbps (safe default)
    #[default]
    P720_30 = 0,
    /// 720p @ 60fps, 6000 kbps
    P720_60 = 1,
    /// 1080p @ 30fps, 4500 kbps
    P1080_30 = 2,
    /// 1080p @ 60fps, 6000 kbps
    P1080_60 = 3,
    /// 1440p (2K) @ 30fps, 9000 kbps
    P1440_30 = 4,
    /// 1440p (2K) @ 60fps, 12000 kbps
    P1440_60 = 5,
    /// 2160p (4K) @ 30fps, 20000 kbps
    P2160_30 = 6,
    /// 2160p (4K) @ 60fps, 30000 kbps
    P2160_60 = 7,
}

impl OutputMode {
    /// Get resolution (width, height)
    pub fn resolution(&self) -> (u32, u32) {
        match self {
            OutputMode::P720_30 | OutputMode::P720_60 => (1280, 720),
            OutputMode::P1080_30 | OutputMode::P1080_60 => (1920, 1080),
            OutputMode::P1440_30 | OutputMode::P1440_60 => (2560, 1440),
            OutputMode::P2160_30 | OutputMode::P2160_60 => (3840, 2160),
        }
    }

    /// Get frame rate
    pub fn fps(&self) -> u32 {
        match self {
            OutputMode::P720_30 | OutputMode::P1080_30 | OutputMode::P1440_30 | OutputMode::P2160_30 => 30,
            OutputMode::P720_60 | OutputMode::P1080_60 | OutputMode::P1440_60 | OutputMode::P2160_60 => 60,
        }
    }

    /// Get video bitrate in kbps
    pub fn video_bitrate_kbps(&self) -> u32 {
        match self {
            OutputMode::P720_30 => 3000,
            OutputMode::P720_60 => 6000,
            OutputMode::P1080_30 => 4500,
            OutputMode::P1080_60 => 6000,
            OutputMode::P1440_30 => 18000,
            OutputMode::P1440_60 => 24000,
            OutputMode::P2160_30 => 25000,
            OutputMode::P2160_60 => 35000,
        }
    }

    /// Twitch-recommended CBR bitrate in kbps. Capped at 6000 (Twitch's
    /// recommended maximum for non-partners).
    pub fn twitch_video_bitrate_kbps(&self) -> u32 {
        match self {
            OutputMode::P720_30 => 3000,
            OutputMode::P720_60 => 4500,
            OutputMode::P1080_30 => 4500,
            OutputMode::P1080_60 => 6000,
            // Above 1080p — Twitch caps recommended bitrate at 6000 CBR.
            OutputMode::P1440_30
            | OutputMode::P1440_60
            | OutputMode::P2160_30
            | OutputMode::P2160_60 => 6000,
        }
    }

    /// Kick CBR bitrate in kbps. Kick runs on AWS IVS, whose standard channels
    /// enforce a ~8.5 Mbps ceiling. 8000 in ABR mode overshoots past it on
    /// busy visuals and IVS drops the stream; 6000 true-CBR stays safely under
    /// the ceiling regardless of content.
    pub fn kick_video_bitrate_kbps(&self) -> u32 {
        match self {
            OutputMode::P720_30 => 3000,
            OutputMode::P720_60 => 4500,
            OutputMode::P1080_30 => 4500,
            OutputMode::P1080_60 => 6000,
            // Above 1080p — hold at the 6000 CBR ceiling.
            OutputMode::P1440_30
            | OutputMode::P1440_60
            | OutputMode::P2160_30
            | OutputMode::P2160_60 => 6000,
        }
    }

    /// Get GOP size (keyframe every 2 seconds)
    pub fn gop_frames(&self) -> u32 {
        self.fps() * 2
    }

    /// Get VBV buffer size in bits (2 second buffer)
    pub fn bufsize_bits(&self) -> u64 {
        self.video_bitrate_kbps() as u64 * 1000 * 2
    }

    /// Get max rate in bits (equals bitrate for CBR)
    pub fn maxrate_bits(&self) -> u64 {
        self.video_bitrate_kbps() as u64 * 1000
    }

    /// Audio sample rate (constant across all modes)
    pub fn audio_sample_rate(&self) -> u32 {
        48000
    }

    /// Audio bitrate in kbps (constant across all modes)
    pub fn audio_bitrate_kbps(&self) -> u32 {
        160
    }

    /// Audio channels (constant: stereo)
    pub fn audio_channels(&self) -> u32 {
        2
    }

    /// User-friendly label
    pub fn label(&self) -> &'static str {
        match self {
            OutputMode::P720_30 => "720p30",
            OutputMode::P720_60 => "720p60",
            OutputMode::P1080_30 => "1080p30",
            OutputMode::P1080_60 => "1080p60",
            OutputMode::P1440_30 => "1440p30",
            OutputMode::P1440_60 => "1440p60",
            OutputMode::P2160_30 => "2160p30",
            OutputMode::P2160_60 => "2160p60",
        }
    }

    /// Detailed description for UI
    pub fn description(&self) -> String {
        let (w, h) = self.resolution();
        format!("{}x{} @ {}fps, {} kbps", w, h, self.fps(), self.video_bitrate_kbps())
    }

    /// Convert to StreamConfig
    pub fn to_config(&self, source_audio_sample_rate: u32) -> StreamConfig {
        let (width, height) = self.resolution();
        StreamConfig {
            width,
            height,
            fps: self.fps(),
            video_bitrate_kbps: self.video_bitrate_kbps(),
            audio_sample_rate: source_audio_sample_rate, // Use source rate, will be resampled if needed
            audio_bitrate_kbps: self.audio_bitrate_kbps(),
            preset: "fast".to_string(),  // Better quality per bit at 720p60
        }
    }

    /// From u8 value
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => OutputMode::P720_30,
            1 => OutputMode::P720_60,
            2 => OutputMode::P1080_30,
            3 => OutputMode::P1080_60,
            4 => OutputMode::P1440_30,
            5 => OutputMode::P1440_60,
            6 => OutputMode::P2160_30,
            7 => OutputMode::P2160_60,
            _ => OutputMode::P720_30,
        }
    }
}

/// Streaming state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Starting,
    Streaming,
    /// Reconnecting after connection drop (with attempt count)
    Reconnecting(u32),
    Stopping,
    Stopped,
    Error,
}

impl Default for StreamState {
    fn default() -> Self {
        StreamState::Idle
    }
}

/// Maximum reconnection attempts before giving up
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Base delay for exponential backoff (seconds)
const RECONNECT_BASE_DELAY_SECS: f64 = 2.0;

/// Maximum delay between reconnection attempts (seconds)
const RECONNECT_MAX_DELAY_SECS: f64 = 60.0;

// ============================================================================
// Backlog Tracking Constants
// ============================================================================

/// Maximum backlog in seconds before we start dropping frames
const BACKLOG_DROP_THRESHOLD_SECS: f64 = 5.0;

/// Duration backlog must exceed threshold before dropping frames (milliseconds)
const BACKLOG_DROP_DELAY_MS: u64 = 1000;

/// Duration backlog must exceed threshold before triggering reconnect (milliseconds)
const BACKLOG_RECONNECT_DELAY_MS: u64 = 5000;

/// How often to log backlog status when elevated (milliseconds)
const BACKLOG_LOG_INTERVAL_MS: u64 = 1000;

/// Backlog tracking for congestion control
///
/// Estimates how much data is "in flight" (queued in OS/network buffers).
/// When backlog exceeds threshold, we drop P-frames to prevent latency creep.
struct BacklogTracker {
    /// Estimated bytes in send buffer (queued but not ACKed)
    estimated_bytes: u64,
    /// Target bitrate in bytes per second
    target_bps: u64,
    /// When backlog first exceeded threshold
    exceeded_since: Option<Instant>,
    /// Last time we logged elevated backlog
    last_log_time: Option<Instant>,
    /// Total frames dropped due to backlog
    frames_dropped: u64,
    /// Total bytes dropped
    bytes_dropped: u64,
    /// Write latency tracking (detects buffer fullness)
    last_write_duration_us: u64,
}

impl BacklogTracker {
    fn new(bitrate_kbps: u32) -> Self {
        Self {
            estimated_bytes: 0,
            target_bps: bitrate_kbps as u64 * 1000 / 8,
            exceeded_since: None,
            last_log_time: None,
            frames_dropped: 0,
            bytes_dropped: 0,
            last_write_duration_us: 0,
        }
    }

    /// Update bitrate (for mode changes)
    fn set_bitrate(&mut self, bitrate_kbps: u32) {
        self.target_bps = bitrate_kbps as u64 * 1000 / 8;
    }

    /// Estimate current backlog in seconds
    fn backlog_secs(&self) -> f64 {
        if self.target_bps == 0 {
            return 0.0;
        }
        self.estimated_bytes as f64 / self.target_bps as f64
    }

    /// Called when a packet is queued for sending
    fn packet_queued(&mut self, bytes: u64) {
        self.estimated_bytes = self.estimated_bytes.saturating_add(bytes);
    }

    /// Called after a packet is successfully written
    /// Also updates drain estimate based on time elapsed
    fn packet_sent(&mut self, bytes: u64, write_duration_us: u64) {
        self.estimated_bytes = self.estimated_bytes.saturating_sub(bytes);
        self.last_write_duration_us = write_duration_us;
    }

    /// Simulate buffer drain based on elapsed time
    /// Call this periodically (e.g., once per frame) to account for network drain
    fn drain_estimate(&mut self, elapsed_secs: f64) {
        let drained = (self.target_bps as f64 * elapsed_secs) as u64;
        self.estimated_bytes = self.estimated_bytes.saturating_sub(drained);
    }

    /// Decide what to do with a packet based on current backlog
    fn evaluate(&mut self, packet_bytes: u64, is_keyframe: bool) -> BacklogAction {
        let backlog = self.backlog_secs();
        let now = Instant::now();

        // Under threshold - clear state and send
        if backlog < BACKLOG_DROP_THRESHOLD_SECS {
            if self.exceeded_since.is_some() {
                log::info!("[Backlog] Recovered: {:.2}s (dropped {} frames, {} KB)",
                    backlog, self.frames_dropped, self.bytes_dropped / 1024);
            }
            self.exceeded_since = None;
            return BacklogAction::Send;
        }

        // Over threshold - track how long
        let exceeded_duration = match self.exceeded_since {
            Some(since) => now.duration_since(since),
            None => {
                self.exceeded_since = Some(now);
                log::warn!("[Backlog] Exceeded threshold: {:.2}s > {:.1}s",
                    backlog, BACKLOG_DROP_THRESHOLD_SECS);
                std::time::Duration::ZERO
            }
        };

        // Periodic logging while elevated
        if self.last_log_time.map(|t| now.duration_since(t).as_millis() as u64 > BACKLOG_LOG_INTERVAL_MS).unwrap_or(true) {
            log::warn!("[Backlog] Elevated for {:.1}s: {:.2}s backlog, {} frames dropped",
                exceeded_duration.as_secs_f64(), backlog, self.frames_dropped);
            self.last_log_time = Some(now);
        }

        // Check if we need to reconnect
        if exceeded_duration.as_millis() as u64 > BACKLOG_RECONNECT_DELAY_MS {
            log::error!("[Backlog] Severe congestion for {:.1}s, triggering reconnect",
                exceeded_duration.as_secs_f64());
            return BacklogAction::Reconnect;
        }

        // Check if we should drop this frame
        if exceeded_duration.as_millis() as u64 > BACKLOG_DROP_DELAY_MS {
            if is_keyframe {
                // Never drop keyframes - they're needed for recovery
                return BacklogAction::Send;
            } else {
                // Drop P-frame to reduce backlog
                self.frames_dropped += 1;
                self.bytes_dropped += packet_bytes;
                return BacklogAction::Drop;
            }
        }

        // Haven't exceeded delay threshold yet, send normally
        BacklogAction::Send
    }

    /// Get stats for reporting
    fn stats(&self) -> BacklogStats {
        BacklogStats {
            backlog_secs: self.backlog_secs(),
            frames_dropped: self.frames_dropped,
            bytes_dropped: self.bytes_dropped,
            last_write_latency_us: self.last_write_duration_us,
        }
    }
}

/// Action to take based on backlog evaluation
#[derive(Clone, Copy, Debug, PartialEq)]
enum BacklogAction {
    /// Send the packet normally
    Send,
    /// Drop this packet (P-frame) to reduce backlog
    Drop,
    /// Backlog is severe, trigger reconnect
    Reconnect,
}

/// Backlog statistics for reporting
#[derive(Clone, Copy, Debug, Default)]
pub struct BacklogStats {
    /// Current estimated backlog in seconds
    pub backlog_secs: f64,
    /// Total frames dropped due to congestion
    pub frames_dropped: u64,
    /// Total bytes dropped
    pub bytes_dropped: u64,
    /// Last write latency in microseconds
    pub last_write_latency_us: u64,
}

/// Streaming statistics
#[derive(Clone, Default)]
pub struct StreamStats {
    pub state: StreamState,
    pub duration_secs: f32,
    pub video_frames_sent: u64,
    pub audio_frames_sent: u64,
    pub current_mode: OutputMode,
    /// Number of reconnection attempts during this session
    pub reconnect_count: u32,
    /// Backlog statistics
    pub backlog: BacklogStats,
}

/// Stream configuration (derived from OutputMode)
#[derive(Clone)]
pub struct StreamConfig {
    /// Video width
    pub width: u32,
    /// Video height
    pub height: u32,
    /// Frames per second
    pub fps: u32,
    /// Video bitrate in kbps
    pub video_bitrate_kbps: u32,
    /// Audio sample rate
    pub audio_sample_rate: u32,
    /// Audio bitrate in kbps
    pub audio_bitrate_kbps: u32,
    /// H.264 preset (ultrafast, superfast, veryfast, faster, fast, medium, slow, slower, veryslow)
    pub preset: String,
}

impl Default for StreamConfig {
    fn default() -> Self {
        OutputMode::default().to_config(48000)
    }
}

/// FFmpeg-based RTMP streamer
pub struct FfmpegStreamer {
    state: Arc<Mutex<StreamState>>,
    start_time: Arc<Mutex<Option<Instant>>>,
    video_frames_sent: Arc<AtomicU64>,
    audio_frames_sent: Arc<AtomicU64>,
    reconnect_count: Arc<AtomicU64>,
    frame_ring: Arc<FrameRing>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    thread_handle: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    config: StreamConfig,
    /// Current output mode
    current_mode: Arc<AtomicU8>,
}

impl FfmpegStreamer {
    /// Create a new streamer with the specified output mode
    pub fn new_with_mode(frame_ring: Arc<FrameRing>, mode: OutputMode, source_audio_sample_rate: u32) -> Self {
        // Initialize FFmpeg
        ffmpeg::init().expect("Failed to initialize FFmpeg");

        let config = mode.to_config(source_audio_sample_rate);

        Self {
            state: Arc::new(Mutex::new(StreamState::Idle)),
            start_time: Arc::new(Mutex::new(None)),
            video_frames_sent: Arc::new(AtomicU64::new(0)),
            audio_frames_sent: Arc::new(AtomicU64::new(0)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            frame_ring,
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(65536))),
            thread_handle: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            config,
            current_mode: Arc::new(AtomicU8::new(mode as u8)),
        }
    }

    /// Create with default mode (720p30)
    pub fn new(frame_ring: Arc<FrameRing>, config: StreamConfig) -> Self {
        // Initialize FFmpeg
        ffmpeg::init().expect("Failed to initialize FFmpeg");

        Self {
            state: Arc::new(Mutex::new(StreamState::Idle)),
            start_time: Arc::new(Mutex::new(None)),
            video_frames_sent: Arc::new(AtomicU64::new(0)),
            audio_frames_sent: Arc::new(AtomicU64::new(0)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            frame_ring,
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(65536))),
            thread_handle: None,
            stop_flag: Arc::new(AtomicBool::new(false)),
            config,
            current_mode: Arc::new(AtomicU8::new(OutputMode::P720_30 as u8)),
        }
    }

    /// Get current output mode
    pub fn get_mode(&self) -> OutputMode {
        OutputMode::from_u8(self.current_mode.load(Ordering::Relaxed))
    }

    /// Push audio samples for streaming
    pub fn push_audio(&self, samples: &[f32]) {
        let state = *self.state.lock();
        // Accept audio during streaming and reconnecting (buffer for when we reconnect)
        if !matches!(state, StreamState::Starting | StreamState::Streaming | StreamState::Reconnecting(_)) {
            return;
        }
        let mut buf = self.audio_buffer.lock();
        if buf.len() < 192000 {
            buf.extend_from_slice(samples);
        }
    }

    /// Get current state
    pub fn get_state(&self) -> StreamState {
        *self.state.lock()
    }

    /// Get statistics
    pub fn get_stats(&self) -> StreamStats {
        let duration_secs = self.start_time.lock()
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);

        StreamStats {
            state: *self.state.lock(),
            duration_secs,
            video_frames_sent: self.video_frames_sent.load(Ordering::Relaxed),
            audio_frames_sent: self.audio_frames_sent.load(Ordering::Relaxed),
            current_mode: self.get_mode(),
            reconnect_count: self.reconnect_count.load(Ordering::Relaxed) as u32,
            backlog: BacklogStats::default(),  // Updated via logs, not real-time accessible yet
        }
    }

    /// Check if streaming (includes reconnecting state)
    pub fn is_streaming(&self) -> bool {
        let state = *self.state.lock();
        matches!(state, StreamState::Starting | StreamState::Streaming | StreamState::Reconnecting(_))
    }

    /// Start streaming to RTMP URL
    pub fn start(&mut self, rtmp_url: String) -> Result<()> {
        if self.is_streaming() {
            anyhow::bail!("Already streaming");
        }

        log::info!("[FfmpegStreamer] Starting stream to RTMP...");

        self.video_frames_sent.store(0, Ordering::Relaxed);
        self.audio_frames_sent.store(0, Ordering::Relaxed);
        self.reconnect_count.store(0, Ordering::Relaxed);
        *self.start_time.lock() = Some(Instant::now());
        self.audio_buffer.lock().clear();
        self.stop_flag.store(false, Ordering::Release);
        *self.state.lock() = StreamState::Starting;

        let state = self.state.clone();
        let frame_ring = self.frame_ring.clone();
        let audio_buffer = self.audio_buffer.clone();
        let video_frames_sent = self.video_frames_sent.clone();
        let audio_frames_sent = self.audio_frames_sent.clone();
        let reconnect_count = self.reconnect_count.clone();
        let stop_flag = self.stop_flag.clone();
        let config = self.config.clone();

        let handle = thread::spawn(move || {
            streaming_thread_with_reconnect(
                state,
                frame_ring,
                audio_buffer,
                rtmp_url,
                video_frames_sent,
                audio_frames_sent,
                reconnect_count,
                stop_flag,
                config,
            );
        });

        self.thread_handle = Some(handle);
        Ok(())
    }

    /// Stop streaming
    pub fn stop(&mut self) -> Result<()> {
        if !self.is_streaming() {
            return Ok(());
        }

        log::info!("[FfmpegStreamer] Stopping stream...");
        *self.state.lock() = StreamState::Stopping;
        self.stop_flag.store(true, Ordering::Release);

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        *self.state.lock() = StreamState::Stopped;

        let stats = self.get_stats();
        log::info!("[FfmpegStreamer] Stopped: {} video frames, {} audio frames, {:.1}s",
            stats.video_frames_sent, stats.audio_frames_sent, stats.duration_secs);

        Ok(())
    }
}

/// Extract stream key from RTMP URL (e.g., rtmp://host/app/key -> key)
fn extract_stream_key(rtmp_url: &str) -> Option<String> {
    // Format: rtmp://host/app/stream_key
    // We need to get just the stream_key part (after the last /)
    if let Some(after_proto) = rtmp_url.strip_prefix("rtmp://") {
        // after_proto = "host/app/stream_key"
        // Find the last / to get the stream key
        if let Some(last_slash) = after_proto.rfind('/') {
            let stream_key = &after_proto[last_slash + 1..];
            if !stream_key.is_empty() {
                return Some(stream_key.to_string());
            }
        }
    }
    None
}

/// Extract host from RTMP URL
fn extract_host(rtmp_url: &str) -> Option<String> {
    // Format: rtmp://host/app/stream_key
    if let Some(after_proto) = rtmp_url.strip_prefix("rtmp://") {
        if let Some(host_end) = after_proto.find('/') {
            return Some(after_proto[..host_end].to_string());
        }
    }
    None
}

/// Build RTMP URL from host and stream key
fn build_rtmp_url(host: &str, stream_key: &str) -> String {
    format!("rtmp://{}/app/{}", host, stream_key)
}

/// Streaming thread with automatic reconnection on failures
/// Uses smart server selection: re-pings servers and avoids degraded ones
fn streaming_thread_with_reconnect(
    state: Arc<Mutex<StreamState>>,
    frame_ring: Arc<FrameRing>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    rtmp_url: String,
    video_frames_sent: Arc<AtomicU64>,
    audio_frames_sent: Arc<AtomicU64>,
    reconnect_count: Arc<AtomicU64>,
    stop_flag: Arc<AtomicBool>,
    config: StreamConfig,
) {
    use super::twitch_servers::{mark_server_degraded, find_best_server_fresh, clear_server_degraded};

    let mut attempt = 0u32;
    let mut current_url = rtmp_url.clone();
    let mut current_host = extract_host(&rtmp_url).unwrap_or_default();
    let stream_key = extract_stream_key(&rtmp_url).unwrap_or_default();

    // Track successful streaming time for clearing degraded status
    let mut stream_start: Instant;
    let mut degraded_cleared = false;

    loop {
        if stop_flag.load(Ordering::Acquire) {
            log::info!("[FfmpegStreamer] Stop requested, exiting reconnect loop");
            *state.lock() = StreamState::Stopped;
            return;
        }

        // If this is a reconnection attempt, handle server selection and backoff
        if attempt > 0 {
            // Mark current server as degraded
            if !current_host.is_empty() {
                mark_server_degraded(&current_host);
            }

            let delay_secs = (RECONNECT_BASE_DELAY_SECS * 2.0_f64.powi(attempt as i32 - 1))
                .min(RECONNECT_MAX_DELAY_SECS);
            log::info!("[FfmpegStreamer] Reconnection attempt {} of {}, waiting {:.1}s...",
                attempt, MAX_RECONNECT_ATTEMPTS, delay_secs);

            *state.lock() = StreamState::Reconnecting(attempt);
            reconnect_count.store(attempt as u64, Ordering::Relaxed);

            // Wait with periodic stop checks
            let wait_until = Instant::now() + std::time::Duration::from_secs_f64(delay_secs);
            while Instant::now() < wait_until {
                if stop_flag.load(Ordering::Acquire) {
                    log::info!("[FfmpegStreamer] Stop requested during reconnect wait");
                    *state.lock() = StreamState::Stopped;
                    return;
                }
                thread::sleep(std::time::Duration::from_millis(100));
            }

            // Re-ping and find best available server (excludes degraded ones)
            log::info!("[FfmpegStreamer] Finding best available server...");
            let new_host = find_best_server_fresh();

            if new_host != current_host {
                log::info!("[FfmpegStreamer] Switching from {} to {}", current_host, new_host);
                current_host = new_host;
            }

            // Build new URL with potentially different server
            current_url = build_rtmp_url(&current_host, &stream_key);

            // Clear audio buffer before reconnect (stale samples)
            audio_buffer.lock().clear();

            // Reset stream tracking
            degraded_cleared = false;
        }

        // Attempt to stream
        stream_start = Instant::now();

        match streaming_thread(
            state.clone(),
            frame_ring.clone(),
            audio_buffer.clone(),
            current_url.clone(),
            video_frames_sent.clone(),
            audio_frames_sent.clone(),
            stop_flag.clone(),
            config.clone(),
        ) {
            Ok(()) => {
                // Clean exit (user stopped)
                // If we streamed successfully for 30+ seconds, clear degraded status
                if stream_start.elapsed().as_secs() >= 30 && !degraded_cleared {
                    clear_server_degraded(&current_host);
                }
                log::info!("[FfmpegStreamer] Stream ended cleanly");
                *state.lock() = StreamState::Stopped;
                return;
            }
            Err(e) => {
                attempt += 1;
                log::error!("[FfmpegStreamer] Stream error (attempt {}): {}", attempt, e);

                if attempt >= MAX_RECONNECT_ATTEMPTS {
                    log::error!("[FfmpegStreamer] Max reconnection attempts ({}) reached, giving up",
                        MAX_RECONNECT_ATTEMPTS);
                    *state.lock() = StreamState::Error;
                    return;
                }

                // Continue loop to attempt reconnection
            }
        }
    }
}

/// Main streaming thread (single connection attempt)
fn streaming_thread(
    state: Arc<Mutex<StreamState>>,
    frame_ring: Arc<FrameRing>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    rtmp_url: String,
    video_frames_sent: Arc<AtomicU64>,
    audio_frames_sent: Arc<AtomicU64>,
    stop_flag: Arc<AtomicBool>,
    config: StreamConfig,
) -> Result<()> {
    log::info!("[FfmpegStreamer] Thread started, setting up encoder...");

    // Wait for frame dimensions from the frame ring
    let (input_width, input_height) = loop {
        if stop_flag.load(Ordering::Acquire) {
            return Ok(());
        }
        let dims = frame_ring.dimensions();
        if dims.0 > 0 && dims.1 > 0 {
            break dims;
        }
        thread::sleep(std::time::Duration::from_millis(10));
    };
    log::info!("[FfmpegStreamer] Input dimensions: {}x{}", input_width, input_height);

    // Create output context for RTMP (explicitly specify FLV format)
    // Note: Protocol options (TCP buffer, etc.) are passed via write_header_with(), not URL params
    let mut output = format::output_as(&rtmp_url, "flv")
        .context("Failed to create RTMP output context")?;

    // Setup video encoder - use libx264 explicitly
    let video_codec = encoder::find_by_name("libx264")
        .context("libx264 encoder not found")?;

    let mut video_stream = output.add_stream(video_codec)?;
    let video_stream_index = video_stream.index();

    // Create encoder context WITH codec (critical for x264 options to work)
    let mut video_encoder = codec::context::Context::new_with_codec(video_codec)
        .encoder()
        .video()?;

    video_encoder.set_width(config.width);
    video_encoder.set_height(config.height);
    video_encoder.set_format(format::Pixel::YUV420P);
    video_encoder.set_time_base(Rational::new(1, config.fps as i32));
    video_encoder.set_frame_rate(Some(Rational::new(config.fps as i32, 1)));

    // CBR settings: bitrate = maxrate, bufsize for VBV rate control
    let bitrate_bps = config.video_bitrate_kbps as usize * 1000;
    video_encoder.set_bit_rate(bitrate_bps);
    video_encoder.set_max_bit_rate(bitrate_bps);
    video_encoder.set_gop(config.fps * 2); // Keyframe every 2 seconds

    // x264 options as key-value pairs (Twitch CBR settings)
    let gop_frames = config.fps * 2; // Keyframe every 2 seconds
    let bufsize_bits = config.video_bitrate_kbps as u64 * 1000 * 2; // 2 second VBV buffer
    let maxrate_bits = config.video_bitrate_kbps as u64 * 1000;

    let mut x264_opts = Dictionary::new();
    x264_opts.set("preset", &config.preset);
    x264_opts.set("profile", "high");         // H.264 High profile (enables B-frames)

    // CBR/VBV rate control (FFmpeg codec options)
    x264_opts.set("bufsize", &bufsize_bits.to_string());
    x264_opts.set("maxrate", &maxrate_bits.to_string());

    // GOP control via FFmpeg options
    // g = keyint (GOP size) - also set via set_gop() above
    x264_opts.set("g", &gop_frames.to_string());
    // keyint_min = min-keyint (force regular keyframes)
    x264_opts.set("keyint_min", &gop_frames.to_string());
    // sc_threshold = scenecut threshold (0 disables scene detection)
    x264_opts.set("sc_threshold", "0");

    // Additional x264 params for strict keyframe control
    // Pass directly to x264 as colon-separated string
    let x264_params = format!(
        "keyint={}:min-keyint={}:scenecut=0:force-cfr=1:bframes=3:b-adapt=1:rc-lookahead=20",
        gop_frames, gop_frames
    );
    x264_opts.set("x264-params", &x264_params);

    log::info!("[FfmpegStreamer] Video: {}x{} @ {}fps, {}kbps CBR, GOP={}, profile=high, bframes=3",
        config.width, config.height, config.fps, config.video_bitrate_kbps, gop_frames);

    // Open encoder with options
    let mut video_encoder = video_encoder.open_with(x264_opts)
        .context("Failed to open video encoder")?;

    video_stream.set_parameters(&video_encoder);

    // Setup audio encoder
    let audio_codec = encoder::find(codec::Id::AAC)
        .context("AAC encoder not found")?;

    let mut audio_stream = output.add_stream(audio_codec)?;
    let audio_stream_index = audio_stream.index();

    // Create encoder context WITH codec
    let mut audio_encoder = codec::context::Context::new_with_codec(audio_codec)
        .encoder()
        .audio()?;

    audio_encoder.set_rate(config.audio_sample_rate as i32);
    audio_encoder.set_channel_layout(ChannelLayout::STEREO);
    audio_encoder.set_format(format::Sample::F32(format::sample::Type::Planar));
    audio_encoder.set_time_base(Rational::new(1, config.audio_sample_rate as i32));
    audio_encoder.set_bit_rate(config.audio_bitrate_kbps as usize * 1000);

    log::info!("[FfmpegStreamer] Audio: AAC {}Hz stereo, {}kbps",
        config.audio_sample_rate, config.audio_bitrate_kbps);

    // Open encoder
    let mut audio_encoder = audio_encoder.open()
        .context("Failed to open audio encoder")?;

    audio_stream.set_parameters(&audio_encoder);

    // Log actual encoder settings after open
    log::info!("[FfmpegStreamer] Audio encoder opened: rate={}, channels={}, frame_size={}",
        audio_encoder.rate(), audio_encoder.channels(), audio_encoder.frame_size());

    // Write header and open RTMP connection
    // Note: FFmpeg's RTMP protocol doesn't expose TCP buffer options through avio_open2
    // The OS TCP stack handles buffering automatically
    log::info!("[FfmpegStreamer] Opening RTMP connection...");
    output.write_header()
        .context("Failed to write RTMP header")?;

    log::info!("[FfmpegStreamer] Encoder setup complete, starting stream...");
    *state.lock() = StreamState::Streaming;

    // Create software scaler for RGBA -> YUV420P conversion
    let mut scaler = software::scaling::Context::get(
        format::Pixel::RGBA,
        input_width,
        input_height,
        format::Pixel::YUV420P,
        config.width,
        config.height,
        software::scaling::Flags::LANCZOS,
    ).context("Failed to create scaler")?;

    // Allocate frames
    let mut rgba_frame = frame::Video::new(format::Pixel::RGBA, input_width, input_height);
    let mut yuv_frame = frame::Video::new(format::Pixel::YUV420P, config.width, config.height);
    // Get AAC frame size from encoder (typically 1024)
    let aac_frame_size = audio_encoder.frame_size() as usize;
    log::info!("[FfmpegStreamer] AAC frame size: {} samples", aac_frame_size);

    let mut audio_frame = frame::Audio::new(
        format::Sample::F32(format::sample::Type::Planar),
        aac_frame_size,
        audio_encoder.channel_layout(),
    );

    let frame_duration = std::time::Duration::from_secs_f64(1.0 / config.fps as f64);
    let stream_start = Instant::now();
    let mut video_pts: i64 = 0;
    let mut audio_pts: i64 = 0;
    let mut last_frame_num: u32 = 0;

    // Stereo samples needed per AAC frame (frame_size samples per channel * 2 channels)
    let stereo_samples_per_aac_frame = aac_frame_size * 2;

    log::info!("[FfmpegStreamer] Streaming at {}fps, {}x{}, audio {}Hz {}kbps",
        config.fps, config.width, config.height, config.audio_sample_rate, config.audio_bitrate_kbps);

    // Initialize backlog tracker for congestion control
    let total_bitrate_kbps = config.video_bitrate_kbps + config.audio_bitrate_kbps;
    let mut backlog = BacklogTracker::new(total_bitrate_kbps);
    let mut last_backlog_drain = Instant::now();

    // Timing diagnostics
    let mut max_encode_time_us: u64 = 0;
    let mut max_write_time_us: u64 = 0;
    let mut max_scale_time_us: u64 = 0;
    let mut slow_frame_count: u64 = 0;
    let frame_budget_us = 1_000_000 / config.fps as u64;  // e.g., 33333µs for 30fps
    let mut last_stats_log = Instant::now();

    while !stop_flag.load(Ordering::Acquire) {
        let frame_start = Instant::now();

        // === VIDEO FRAME ===
        if let Some((pixels, w, h, frame_num)) = frame_ring.try_read() {
            if frame_num != last_frame_num && w == input_width && h == input_height {
                last_frame_num = frame_num;

                // Copy RGBA pixels to frame (flip Y)
                let copy_start = Instant::now();
                let stride = (input_width * 4) as usize;
                let dst_data = rgba_frame.data_mut(0);
                for y in 0..input_height as usize {
                    let src_y = input_height as usize - 1 - y;
                    let src_off = src_y * stride;
                    let dst_off = y * stride;
                    if src_off + stride <= pixels.len() && dst_off + stride <= dst_data.len() {
                        dst_data[dst_off..dst_off + stride]
                            .copy_from_slice(&pixels[src_off..src_off + stride]);
                    }
                }
                let copy_time = copy_start.elapsed();

                // Scale RGBA to YUV420P
                let scale_start = Instant::now();
                scaler.run(&rgba_frame, &mut yuv_frame)?;
                let scale_time = scale_start.elapsed();
                max_scale_time_us = max_scale_time_us.max(scale_time.as_micros() as u64);

                // Set PTS
                yuv_frame.set_pts(Some(video_pts));
                video_pts += 1;

                // Encode
                let encode_start = Instant::now();
                video_encoder.send_frame(&yuv_frame)?;

                // Receive encoded packets
                let mut packet = Packet::empty();
                while video_encoder.receive_packet(&mut packet).is_ok() {
                    let packet_size = packet.size() as u64;
                    let is_keyframe = packet.is_key();

                    // Evaluate backlog before sending
                    match backlog.evaluate(packet_size, is_keyframe) {
                        BacklogAction::Send => {
                            packet.set_stream(video_stream_index);
                            packet.rescale_ts(
                                video_encoder.time_base(),
                                output.stream(video_stream_index).unwrap().time_base(),
                            );

                            // Track queued bytes and time the write
                            backlog.packet_queued(packet_size);
                            let write_start = Instant::now();
                            packet.write_interleaved(&mut output)?;
                            let write_duration = write_start.elapsed();
                            backlog.packet_sent(packet_size, write_duration.as_micros() as u64);

                            video_frames_sent.fetch_add(1, Ordering::Relaxed);
                            max_write_time_us = max_write_time_us.max(write_duration.as_micros() as u64);
                        }
                        BacklogAction::Drop => {
                            // P-frame dropped to reduce backlog - don't send
                            // Note: This may cause visual artifacts until next keyframe
                        }
                        BacklogAction::Reconnect => {
                            // Severe backlog - trigger reconnect
                            log::error!("[FfmpegStreamer] Backlog reconnect triggered");
                            return Err(anyhow::anyhow!("Backlog exceeded, reconnecting"));
                        }
                    }
                }
                let encode_time = encode_start.elapsed();
                max_encode_time_us = max_encode_time_us.max(encode_time.as_micros() as u64);

                // Check for slow frames
                let total_frame_time = frame_start.elapsed();
                if total_frame_time.as_micros() as u64 > frame_budget_us {
                    slow_frame_count += 1;
                }
            }
        }

        // Periodically drain backlog estimate based on elapsed time
        let drain_elapsed = last_backlog_drain.elapsed();
        if drain_elapsed.as_millis() > 100 {
            backlog.drain_estimate(drain_elapsed.as_secs_f64());
            last_backlog_drain = Instant::now();
        }

        // === AUDIO FRAMES ===
        // Encode as many AAC frames as we have samples for
        loop {
            let samples: Vec<f32> = {
                let mut buf = audio_buffer.lock();
                if buf.len() >= stereo_samples_per_aac_frame {
                    buf.drain(..stereo_samples_per_aac_frame).collect()
                } else {
                    break; // Not enough samples for a full frame
                }
            };

            // Fill audio frame with samples (deinterleave interleaved stereo to planar)
            // Left channel (plane 0)
            {
                let left = audio_frame.plane_mut::<f32>(0);
                for (i, chunk) in samples.chunks(2).enumerate() {
                    if i < left.len() {
                        left[i] = chunk[0];
                    }
                }
            }
            // Right channel (plane 1)
            {
                let right = audio_frame.plane_mut::<f32>(1);
                for (i, chunk) in samples.chunks(2).enumerate() {
                    if i < right.len() {
                        right[i] = chunk.get(1).copied().unwrap_or(chunk[0]);
                    }
                }
            }

            // PTS is cumulative sample count (at encoder sample rate)
            audio_frame.set_pts(Some(audio_pts));
            audio_pts += aac_frame_size as i64;

            // Encode
            audio_encoder.send_frame(&audio_frame)?;

            // Receive encoded packets
            let mut packet = Packet::empty();
            while audio_encoder.receive_packet(&mut packet).is_ok() {
                let packet_size = packet.size() as u64;

                packet.set_stream(audio_stream_index);
                packet.rescale_ts(
                    audio_encoder.time_base(),
                    output.stream(audio_stream_index).unwrap().time_base(),
                );

                // Track audio packets in backlog (never drop audio)
                backlog.packet_queued(packet_size);
                let write_start = Instant::now();
                packet.write_interleaved(&mut output)?;
                let write_duration = write_start.elapsed();
                backlog.packet_sent(packet_size, write_duration.as_micros() as u64);

                audio_frames_sent.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Pace to frame rate
        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            thread::sleep(frame_duration - elapsed);
        }

        // Log stats every 5 seconds
        if last_stats_log.elapsed().as_secs() >= 5 {
            last_stats_log = Instant::now();
            let vf = video_frames_sent.load(Ordering::Relaxed);
            let af = audio_frames_sent.load(Ordering::Relaxed);
            let total_elapsed = stream_start.elapsed().as_secs();
            let stats = backlog.stats();
            // Log timing diagnostics to identify bottlenecks
            log::info!("[FfmpegStreamer] {}s: {} video, {} audio | backlog: {:.2}s, dropped: {}",
                total_elapsed, vf, af, stats.backlog_secs, stats.frames_dropped);
            log::info!("[FfmpegStreamer] TIMING: scale={}µs, encode={}µs, write={}µs, slow_frames={} (budget={}µs)",
                max_scale_time_us, max_encode_time_us, max_write_time_us, slow_frame_count, frame_budget_us);
            // Reset max values for next interval
            max_scale_time_us = 0;
            max_encode_time_us = 0;
            max_write_time_us = 0;
        }
    }

    // Flush encoders
    log::info!("[FfmpegStreamer] Flushing encoders...");

    video_encoder.send_eof()?;
    let mut packet = Packet::empty();
    while video_encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(video_stream_index);
        packet.write_interleaved(&mut output)?;
    }

    audio_encoder.send_eof()?;
    while audio_encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(audio_stream_index);
        packet.write_interleaved(&mut output)?;
    }

    // Write trailer
    output.write_trailer()?;

    log::info!("[FfmpegStreamer] Stream ended cleanly");
    *state.lock() = StreamState::Stopped;

    Ok(())
}
