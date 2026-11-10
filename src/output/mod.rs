//! Output management for REACT
//!
//! Supports multiple simultaneous outputs:
//! - Native Window: Borderless/fullscreen window for projectors, LED walls
//! - NDI Output: Stream frames via NDI to external applications
//! - Recording: Crash-resilient segmented MP4 recording
//!
//! Uses IOSurface for zero-copy GPU texture sharing between contexts.

#[cfg(target_os = "macos")]
pub mod native_window;

pub mod ndi_sender;

// Legacy monolithic recorder (kept for fallback)
pub mod recorder;
pub use recorder::{Recorder, RecordingState};

// New segmented recording system (crash-resilient)
pub mod segment_journal;
pub mod segmented_recorder;
pub mod segment_stitcher;
pub mod fmp4_muxer;
pub mod videotoolbox_encoder;

pub use segment_journal::{RecorderConfig, RecoveryInfo, SegmentJournal};
pub use segmented_recorder::{SegmentedRecorder, SegmentedRecordingMetrics, SegmentedRecordingState};
pub use segment_stitcher::{stitch_segments, create_playlist, StitchResult, suggest_stitch_output, cleanup_segments};

pub mod rtmp_streamer;
pub use rtmp_streamer::{RtmpStreamState, find_ffmpeg};

pub mod twitch_servers;
pub use twitch_servers::{
    TwitchServer, get_twitch_servers, get_selected_twitch_server, find_best_twitch_server,
    find_best_available_server, find_best_server_fresh, ping_all_servers,
    mark_server_degraded, clear_server_degraded, clear_all_degraded, is_server_degraded,
    init_db_table as init_twitch_db_table, save_servers_to_db, load_servers_from_db,
};

pub mod ffmpeg_streamer;
pub use ffmpeg_streamer::{FfmpegStreamer, StreamConfig, StreamState, StreamStats, OutputMode};

pub mod gstreamer_streamer;
pub use gstreamer_streamer::{
    GStreamerStreamer,
    StreamPlatform,
    StreamState as GStreamerStreamState,
    StreamStats as GStreamerStreamStats,
};

use anyhow::Context;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use parking_lot::Mutex;

pub mod audio_fingerprint;

use crate::audio::audio_bus::{AudioFrame, CHUNK_FRAMES};

/// Interleaved stereo sample count per chunk (960 frames * 2 channels = 1920 samples = 7680 bytes)
const CHUNK_SAMPLES: usize = CHUNK_FRAMES * 2;

/// Embedded watermark logo PNG (white "compeller.ai" on transparent background).
/// Written to a temp file when recording starts with watermark enabled.
const WATERMARK_LOGO_PNG: &[u8] = include_bytes!("../../assets/compeller_logo_watermark.png");

/// XOR key for metadata comment obfuscation (not crypto — just prevents casual reading).
const METADATA_XOR_KEY: &[u8] = b"compeller.ai/react";

/// Encode metadata for MP4 comment field: XOR-encrypt then base64.
fn encode_metadata(account_id: u32, timestamp: u32) -> String {
    let plain = format!("cai:{}:{}", account_id, timestamp);
    let xored: Vec<u8> = plain.bytes()
        .enumerate()
        .map(|(i, b)| b ^ METADATA_XOR_KEY[i % METADATA_XOR_KEY.len()])
        .collect();
    base64_encode(&xored)
}

/// Simple base64 encode (no extra dependencies).
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// SPSC queue capacity for NDI/recorder (13 chunks = 260ms).
const QUEUE_SLOTS_NDI: usize = 13;

/// SPSC queue capacity for RTMP (16 chunks = 320ms).
/// Encoders can block longer than NDI due to FFmpeg pipeline pressure.
const QUEUE_SLOTS_RTMP: usize = 16;

/// Fixed-size audio chunk type for SPSC output queues.
/// Always exactly CHUNK_FRAMES stereo frames of interleaved audio.
type AudioChunk = [f32; CHUNK_SAMPLES];

/// Drain target: list of SPSC chunk producers for active outputs.
/// Drain thread pushes fixed-size chunks without blocking.
/// Writer threads pop and forward to the real audio sinks.
type DrainTarget = Arc<Mutex<Vec<rtrb::Producer<AudioChunk>>>>;

/// Per-output audio gain values for UI display.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AudioGains {
    pub ndi_user: f32,
    pub ndi_assist: f32,
    pub ndi_effective: f32,
    pub ndi_assist_hold: bool,
    pub ndi_audio_present: bool,
    pub rec_user: f32,
    pub rec_assist: f32,
    pub rec_effective: f32,
    pub rec_assist_hold: bool,
    pub rec_audio_present: bool,
}

// ==================== Three-Stage Gain Model ====================

/// Three-stage gain set for an output: calibration × assist × user.
///
/// - calibration: Fixed per source (SCK=2.0, mic=1.5, device=profile). Set on source change.
/// - assist: Computed by LevelAssist. One-shot measurement, latched. Caps at assist_cap.
/// - user: UI slider (default 1.0, range 0.1–16.0). User's "make it loud" knob.
#[derive(Clone)]
pub struct OutputGainSet {
    pub calibration: Arc<AtomicU32>,
    pub assist: Arc<AtomicU32>,
    pub user: Arc<AtomicU32>,
    pub assist_cap: Arc<AtomicU32>,
    pub reset_flag: Arc<AtomicBool>,
    /// True when assist is in hold/cooldown (for UI badge)
    pub assist_hold: Arc<AtomicBool>,
    /// True when audio signal is present (above floor threshold)
    pub audio_present: Arc<AtomicBool>,
}

impl OutputGainSet {
    /// Create a new gain set with given calibration and assist cap.
    pub fn new(cal: f32, cap: f32) -> Self {
        Self {
            calibration: Arc::new(AtomicU32::new(cal.to_bits())),
            assist: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            user: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            assist_cap: Arc::new(AtomicU32::new(cap.to_bits())),
            reset_flag: Arc::new(AtomicBool::new(false)),
            assist_hold: Arc::new(AtomicBool::new(false)),
            audio_present: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Update calibration and cap, reset assist to 1.0, signal writer to reset LevelAssist.
    pub fn set_calibration(&self, cal: f32, cap: f32) {
        self.calibration.store(cal.to_bits(), Ordering::Relaxed);
        self.assist_cap.store(cap.to_bits(), Ordering::Relaxed);
        self.assist.store(1.0f32.to_bits(), Ordering::Relaxed);
        self.reset_flag.store(true, Ordering::Release);
    }

    /// Set user gain (from UI slider).
    pub fn set_user(&self, gain: f32) {
        self.user.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Compute effective gain: calibration × assist × user.
    pub fn effective(&self) -> f32 {
        let cal = f32::from_bits(self.calibration.load(Ordering::Relaxed));
        let assist = f32::from_bits(self.assist.load(Ordering::Relaxed));
        let user = f32::from_bits(self.user.load(Ordering::Relaxed));
        (cal * assist * user).min(128.0)
    }

    /// Get individual gain components: (user, assist, effective).
    pub fn components(&self) -> (f32, f32, f32) {
        let cal = f32::from_bits(self.calibration.load(Ordering::Relaxed));
        let assist = f32::from_bits(self.assist.load(Ordering::Relaxed));
        let user = f32::from_bits(self.user.load(Ordering::Relaxed));
        let effective = (cal * assist * user).min(128.0);
        (user, assist, effective)
    }
}

/// Floor hysteresis thresholds: prevents flapping around the presence boundary.
/// Enter audio-present when peak >= ENTER, exit when peak <= EXIT.
const ASSIST_FLOOR_ENTER: f32 = 0.012;
const ASSIST_FLOOR_EXIT: f32 = 0.008;

/// Per-update step limits: gain factor is clamped to [MIN_STEP, MAX_STEP].
/// This prevents audible pumping — assist can only change ±10% per evaluation.
const ASSIST_MIN_STEP: f32 = 0.90;
const ASSIST_MAX_STEP: f32 = 1.10;

/// Hold time: minimum seconds between assist updates (prevents oscillation).
const ASSIST_HOLD_SECS: f32 = 15.0;

/// Cooldown: seconds after any assist change before another is allowed.
const ASSIST_COOLDOWN_SECS: f32 = 10.0;

/// Grace period after reset: seconds before assist is allowed to update.
const ASSIST_GRACE_SECS: f32 = 2.0;

/// Level Assist: automatic gain normalization for quiet sources.
///
/// Stability-first design: brings quiet sources to -12 dBFS target slowly,
/// with audio presence gating, step-limited updates, cooldown timers,
/// and never attenuates (assist >= 1.0 always).
///
/// Should feel like "set it once and forget it," not "chasing the music."
struct LevelAssist {
    target_peak: f32,
    cap_gain: f32,
    window: std::collections::VecDeque<f32>,
    current_gain: f32,
    last_update: std::time::Instant,
    reset_time: std::time::Instant,
    initialized: bool,
    eval_counter: u32,
    /// True when in hold/cooldown (communicated to UI via atomic)
    in_hold: bool,
    /// True when audio signal is present (above floor with hysteresis)
    audio_present: bool,
}

impl LevelAssist {
    fn new(cap: f32) -> Self {
        let now = std::time::Instant::now();
        Self {
            target_peak: 0.25, // -12 dBFS
            cap_gain: cap,
            window: std::collections::VecDeque::with_capacity(100),
            current_gain: 1.0,
            last_update: now,
            reset_time: now,
            initialized: false,
            eval_counter: 0,
            in_hold: false,
            audio_present: false,
        }
    }

    /// Push a post-calibration peak value and potentially update gain.
    /// Called once per chunk (~50 chunks/sec). Returns (current_assist_gain, in_hold).
    fn push_and_evaluate(&mut self, post_cal_peak: f32) -> (f32, bool) {
        self.window.push_back(post_cal_peak);
        if self.window.len() > 100 {
            self.window.pop_front();
        }

        self.eval_counter += 1;
        // Evaluate every 50 chunks (approximately once per second)
        if self.eval_counter < 50 {
            return (self.current_gain, self.in_hold);
        }
        self.eval_counter = 0;

        // Need a full window before evaluating
        if self.window.len() < 100 {
            return (self.current_gain, self.in_hold);
        }

        let elapsed_since_reset = self.reset_time.elapsed().as_secs_f32();
        let elapsed_since_update = self.last_update.elapsed().as_secs_f32();

        // Grace period after reset: no updates allowed
        if elapsed_since_reset < ASSIST_GRACE_SECS {
            self.in_hold = true;
            return (self.current_gain, self.in_hold);
        }

        // Hold time: minimum gap between updates (prevents oscillation)
        if self.initialized && elapsed_since_update < ASSIST_HOLD_SECS {
            self.in_hold = true;
            return (self.current_gain, self.in_hold);
        }

        // Cooldown: separate from hold, prevents back-to-back edge cases
        if self.initialized && elapsed_since_update < ASSIST_COOLDOWN_SECS {
            self.in_hold = true;
            return (self.current_gain, self.in_hold);
        }

        self.in_hold = false;

        // Audio presence gate with hysteresis (prevents flapping at threshold)
        let window_peak = self.window.iter().copied().fold(0.0f32, f32::max);
        if self.audio_present {
            if window_peak <= ASSIST_FLOOR_EXIT {
                self.audio_present = false;
            }
        } else {
            if window_peak >= ASSIST_FLOOR_ENTER {
                self.audio_present = true;
            }
        }

        // If audio not present, hold assist at current value
        if !self.audio_present {
            return (self.current_gain, self.in_hold);
        }

        // Compute desired gain factor: target / observed peak
        // This is the "ideal" assist if we could jump to it instantly
        let ideal = (self.target_peak / window_peak).max(1.0); // never attenuate

        // Hysteresis on the need (ideal vs current), not the clamped step.
        // This allows gradual convergence over multiple evaluations when the
        // target is far away, while preventing jitter when close to target.
        let need_ratio = ideal / self.current_gain.max(0.01);
        if (need_ratio - 1.0).abs() < 0.10 && self.initialized {
            return (self.current_gain, self.in_hold);
        }

        // Compute step factor: how much to multiply current_gain by
        // Clamp to [MIN_STEP, MAX_STEP] — no pumping guarantee
        let step = need_ratio.clamp(ASSIST_MIN_STEP, ASSIST_MAX_STEP);

        // Apply step, clamp to [1.0, cap] — assist never attenuates
        let new_gain = (self.current_gain * step).clamp(1.0, self.cap_gain);

        // Only log and update if there's an actual change
        if (new_gain - self.current_gain).abs() > 0.001 {
            let old = self.current_gain;
            self.current_gain = new_gain;
            self.last_update = std::time::Instant::now();
            self.initialized = true;
            self.in_hold = true; // enter hold after update
            log::info!("[LevelAssist] gain {:.2} -> {:.2} (step={:.3}, window_peak={:.3}, target={:.2}, cap={:.1})",
                old, new_gain, step, window_peak, self.target_peak, self.cap_gain);
        }

        (self.current_gain, self.in_hold)
    }

    /// Reset state (called on source change).
    fn reset(&mut self) {
        self.window.clear();
        self.current_gain = 1.0;
        self.initialized = false;
        self.eval_counter = 0;
        self.in_hold = false;
        self.audio_present = false;
        self.reset_time = std::time::Instant::now();
        log::info!("[LevelAssist] Reset (source changed)");
    }
}

// ==================== Audio Limiter ====================

/// Limiter ceiling (peak target). Signal above this triggers gain reduction.
const LIMITER_CEILING: f32 = 0.95;

/// Attack coefficient: how fast the limiter responds to transients.
/// At 48kHz, 5ms attack ≈ 240 samples → coeff = 1 - exp(-1/(48000*0.005)) ≈ 0.042
const LIMITER_ATTACK: f32 = 0.042;

/// Release coefficient: how fast the limiter releases after transient passes.
/// At 48kHz, 100ms release ≈ 4800 samples → coeff = 1 - exp(-1/(48000*0.1)) ≈ 0.00021
const LIMITER_RELEASE: f32 = 0.00021;

/// Per-output audio limiter with gain application.
///
/// Applies user gain, then a peak limiter that prevents overs without hard clipping.
/// Envelope follower with fast attack / slow release ensures no pumping.
/// The limiter should be "safety-only" — if it's active more than ~5% of samples,
/// the calibration gain is too high.
struct AudioLimiter {
    /// Limiter envelope (gain reduction factor, 0.0..=1.0). Starts at 1.0 (no reduction).
    envelope: f32,
    /// Peak sample before gain application (raw input level).
    pre_gain_peak: f32,
    /// Peak sample after gain but before limiter (what the limiter sees).
    post_gain_peak: f32,
    /// Peak sample after limiter (final output level).
    post_limiter_peak: f32,
    /// Number of chunks where post-gain peak exceeded ceiling.
    clip_chunks: u64,
    /// Number of samples where limiter was active (envelope < 0.99 ≈ 0.09dB GR).
    active_samples: u64,
    /// Total samples processed this period.
    total_samples: u64,
    /// Minimum envelope seen this period (worst gain reduction).
    min_envelope: f32,
}

impl AudioLimiter {
    fn new() -> Self {
        Self {
            envelope: 1.0,
            pre_gain_peak: 0.0,
            post_gain_peak: 0.0,
            post_limiter_peak: 0.0,
            clip_chunks: 0,
            active_samples: 0,
            total_samples: 0,
            min_envelope: 1.0,
        }
    }

    /// Apply gain + limiter to a chunk of interleaved stereo samples in-place.
    #[inline]
    fn process(&mut self, chunk: &mut [f32], gain: f32) {
        let mut chunk_pre_peak: f32 = 0.0;
        let mut chunk_post_gain_peak: f32 = 0.0;
        let mut chunk_post_limiter_peak: f32 = 0.0;
        let mut chunk_min_env: f32 = 1.0;

        for sample in chunk.iter_mut() {
            let abs_raw = sample.abs();
            if abs_raw > chunk_pre_peak {
                chunk_pre_peak = abs_raw;
            }

            // Apply user gain
            let gained = *sample * gain;
            let abs_gained = gained.abs();
            if abs_gained > chunk_post_gain_peak {
                chunk_post_gain_peak = abs_gained;
            }

            // Envelope follower: track required gain reduction
            if abs_gained > LIMITER_CEILING {
                let target = LIMITER_CEILING / abs_gained;
                self.envelope += (target - self.envelope) * LIMITER_ATTACK;
            } else {
                self.envelope += (1.0 - self.envelope) * LIMITER_RELEASE;
            }
            self.envelope = self.envelope.clamp(0.001, 1.0);

            if self.envelope < chunk_min_env {
                chunk_min_env = self.envelope;
            }

            // Track limiter health: active when envelope < 0.99 (≈0.09dB GR)
            self.total_samples += 1;
            if self.envelope < 0.99 {
                self.active_samples += 1;
            }

            // Apply limiter envelope + safety clamp
            let limited = (gained * self.envelope).clamp(-1.0, 1.0);
            if limited.abs() > chunk_post_limiter_peak {
                chunk_post_limiter_peak = limited.abs();
            }
            *sample = limited;
        }

        // Update period-wide diagnostics
        if chunk_pre_peak > self.pre_gain_peak {
            self.pre_gain_peak = chunk_pre_peak;
        }
        if chunk_post_gain_peak > self.post_gain_peak {
            self.post_gain_peak = chunk_post_gain_peak;
        }
        if chunk_post_limiter_peak > self.post_limiter_peak {
            self.post_limiter_peak = chunk_post_limiter_peak;
        }
        if chunk_min_env < self.min_envelope {
            self.min_envelope = chunk_min_env;
        }
        if chunk_post_gain_peak > LIMITER_CEILING {
            self.clip_chunks += 1;
        }
    }

    /// Get and reset diagnostic stats.
    fn take_stats(&mut self) -> LimiterStats {
        let active_ratio = if self.total_samples > 0 {
            self.active_samples as f32 / self.total_samples as f32
        } else {
            0.0
        };
        let max_gr_db = if self.min_envelope > 0.001 {
            20.0 * self.min_envelope.log10()
        } else {
            -60.0
        };
        let stats = LimiterStats {
            pre_gain_peak: self.pre_gain_peak,
            post_gain_peak: self.post_gain_peak,
            post_limiter_peak: self.post_limiter_peak,
            clip_chunks: self.clip_chunks,
            active_ratio,
            max_gr_db,
        };
        self.pre_gain_peak = 0.0;
        self.post_gain_peak = 0.0;
        self.post_limiter_peak = 0.0;
        self.clip_chunks = 0;
        self.active_samples = 0;
        self.total_samples = 0;
        self.min_envelope = 1.0;
        stats
    }
}

/// Diagnostic stats from one reporting period.
struct LimiterStats {
    /// Peak sample before gain (raw input).
    pre_gain_peak: f32,
    /// Peak sample after gain, before limiter (what limiter sees).
    post_gain_peak: f32,
    /// Peak sample after limiter (final output). Should never exceed 1.0.
    post_limiter_peak: f32,
    /// Chunks where post-gain peak exceeded ceiling.
    clip_chunks: u64,
    /// Fraction of samples where limiter was active (0.0..1.0).
    /// If > 0.05 (5%), calibration gain is too high.
    active_ratio: f32,
    /// Maximum gain reduction in dB (negative = reducing).
    max_gr_db: f32,
}

#[cfg(target_os = "macos")]
pub use native_window::NativeOutputWindow;

pub use ndi_sender::{NdiSender, NdiConfig, NdiTally};

#[cfg(target_os = "macos")]
use io_surface::{self, IOSurface};
#[cfg(target_os = "macos")]
use core_foundation::base::{TCFType, CFType};
#[cfg(target_os = "macos")]
use core_foundation::dictionary::CFDictionary;
#[cfg(target_os = "macos")]
use core_foundation::number::CFNumber;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;

/// IOSurface property keys
#[cfg(target_os = "macos")]
const K_IO_SURFACE_WIDTH: &str = "IOSurfaceWidth";
#[cfg(target_os = "macos")]
const K_IO_SURFACE_HEIGHT: &str = "IOSurfaceHeight";
#[cfg(target_os = "macos")]
const K_IO_SURFACE_BYTES_PER_ELEMENT: &str = "IOSurfaceBytesPerElement";
#[cfg(target_os = "macos")]
const K_IO_SURFACE_PIXEL_FORMAT: &str = "IOSurfacePixelFormat";

/// Manages all output destinations
pub struct OutputManager {
    /// Native output window (projector/LED wall) - macOS only for now
    #[cfg(target_os = "macos")]
    native_window: Option<NativeOutputWindow>,
    /// NDI output sender
    ndi_sender: Option<NdiSender>,
    /// Legacy MP4 recorder (fallback)
    recorder: Option<Recorder>,
    /// Segmented recorder for crash-resilient recording
    segmented_recorder: Option<SegmentedRecorder>,
    /// Database for segment journal (set via set_database)
    db: Option<crate::db::Database>,
    /// Use segmented recording (default: true)
    use_segmented_recording: bool,
    /// Shared frame buffer for outputs (RGBA pixels) - fallback for non-IOSurface
    frame_buffer: Arc<Mutex<FrameBuffer>>,
    /// Signal that a new frame is ready
    frame_ready: Arc<AtomicBool>,
    /// Lock-free frame ring for NDI (no mutex on hot path)
    frame_ring: Arc<FrameRing>,
    /// Lock-free audio ring for NDI audio output
    audio_ring: Arc<AudioRing>,
    /// Source audio sample rate for NDI resampling
    ndi_source_sample_rate: u32,
    /// Shared IOSurface for zero-copy GPU sharing
    #[cfg(target_os = "macos")]
    shared_surface: Arc<Mutex<Option<SharedIOSurface>>>,
    /// Frame counter for synchronization
    #[cfg(target_os = "macos")]
    frame_number: Arc<AtomicU32>,
    /// Processing flag - true while stitching segments after recording stops
    is_processing: Arc<AtomicBool>,
    /// Drain thread targets: updated when outputs start/stop
    ndi_drain_target: DrainTarget,
    rec_drain_target: DrainTarget,
    rtmp_drain_target: DrainTarget,
    /// Drain threads running flag
    drain_running: Arc<AtomicBool>,
    /// Drain thread join handles
    drain_handles: Vec<JoinHandle<()>>,
    /// Writer thread handles per output (joined when output stops)
    ndi_writer_handles: Vec<JoinHandle<()>>,
    rec_writer_handles: Vec<JoinHandle<()>>,
    rtmp_writer_handles: Vec<JoinHandle<()>>,
    /// Three-stage gain sets: calibration × assist × user
    ndi_gains: OutputGainSet,
    rec_gains: OutputGainSet,
    /// Audio volume boost for recording mux (1.0 = unity; writer thread pre-applies three-stage gain)
    rec_audio_volume_boost: AtomicU32,
    /// Watermark logo temp file (kept alive while recording, auto-cleaned on Drop)
    watermark_tempfile: Option<tempfile::NamedTempFile>,
    /// Cached entitlements for recording pipeline (watermark flag + account_id)
    rec_watermark_enabled: bool,
    rec_account_id: u32,
}

/// Shared IOSurface for zero-copy GPU texture sharing
#[cfg(target_os = "macos")]
pub struct SharedIOSurface {
    pub surface: IOSurface,
    pub width: u32,
    pub height: u32,
}

// IOSurface is a system-level resource that's thread-safe when accessed by ID.
// The underlying IOSurface can be safely shared across threads via IOSurfaceLookup.
#[cfg(target_os = "macos")]
unsafe impl Send for SharedIOSurface {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SharedIOSurface {}

#[cfg(target_os = "macos")]
impl SharedIOSurface {
    pub fn new(width: u32, height: u32) -> Self {
        let surface = create_io_surface(width, height);
        log::info!("[Output] Created shared IOSurface {}x{} (ID: {})", width, height, surface.get_id());
        Self { surface, width, height }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width != width || self.height != height {
            self.surface = create_io_surface(width, height);
            self.width = width;
            self.height = height;
            log::info!("[Output] Resized shared IOSurface to {}x{} (ID: {})", width, height, self.surface.get_id());
        }
    }

    pub fn surface_id(&self) -> u32 {
        self.surface.get_id()
    }
}

/// Create an IOSurface with the specified dimensions
#[cfg(target_os = "macos")]
fn create_io_surface(width: u32, height: u32) -> IOSurface {
    // BGRA format (0x42475241 = 'BGRA')
    let pixel_format: u32 = 0x42475241;

    let properties: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
        (
            CFString::new(K_IO_SURFACE_WIDTH),
            CFNumber::from(width as i32).as_CFType(),
        ),
        (
            CFString::new(K_IO_SURFACE_HEIGHT),
            CFNumber::from(height as i32).as_CFType(),
        ),
        (
            CFString::new(K_IO_SURFACE_BYTES_PER_ELEMENT),
            CFNumber::from(4i32).as_CFType(),
        ),
        (
            CFString::new(K_IO_SURFACE_PIXEL_FORMAT),
            CFNumber::from(pixel_format as i32).as_CFType(),
        ),
    ]);

    io_surface::new(&properties)
}

/// Frame buffer for sharing rendered frames with output threads (fallback)
pub struct FrameBuffer {
    /// RGBA pixel data
    pub pixels: Vec<u8>,
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
    /// Frame number (for sync)
    pub frame_number: u64,
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self {
            pixels: Vec::new(),
            width: 0,
            height: 0,
            frame_number: 0,
        }
    }
}

/// Maximum supported resolution - buffers pre-allocated for largest option
/// Supports: 4K (3840x2160 = 8.3M px), Ultrawide (5120x1440 = 7.4M px)
pub const NDI_MAX_WIDTH: u32 = 5120;
pub const NDI_MAX_HEIGHT: u32 = 2160;
const NDI_MAX_FRAME_SIZE: usize = (NDI_MAX_WIDTH * NDI_MAX_HEIGHT * 4) as usize;

/// Number of ring buffer slots (5 = 4 frame tolerance = 66ms at 60fps)
const RING_SLOTS: usize = 5;

/// Lock-free frame ring for NDI output
///
/// Uses 5 pre-allocated frame slots with atomic indices:
/// - Writer (render thread) writes to `write_idx`, then advances it
/// - Reader (NDI thread) reads from oldest ready slot (4 frames behind)
/// - No mutex on the hot path, just atomic index updates
/// - Pre-allocated for 4K max, actual resolution stored in atomics
/// - 5 slots gives 4-frame tolerance (~66ms at 60fps) before reader gets overwritten
pub struct FrameRing {
    /// Pre-allocated frame slots (RGBA pixels, max 4K size)
    slots: [FrameSlot; RING_SLOTS],
    /// Current write index (wraps 0..RING_SLOTS)
    write_idx: AtomicU32,
    /// Current frame width (can change, but buffers are always big enough)
    width: AtomicU32,
    /// Current frame height
    height: AtomicU32,
    /// Total frames written (for debugging)
    total_written: AtomicU32,
}

/// A single frame slot in the ring buffer
struct FrameSlot {
    /// Pre-allocated pixel buffer (max 4K = 33MB)
    pixels: std::cell::UnsafeCell<Vec<u8>>,
    /// Frame number when this slot was written
    frame_num: AtomicU32,
}

// FrameRing is safe to share across threads:
// - Writer only writes to write_idx slot
// - Reader only reads from (write_idx + 1) % 3 slot (2 frames behind)
// - Atomic indices ensure proper synchronization
// - Fixed size buffers (no runtime resize = no data race)
unsafe impl Send for FrameRing {}
unsafe impl Sync for FrameRing {}

impl FrameRing {
    /// Create a new frame ring with pre-allocated 4K buffers
    pub fn new() -> Self {
        log::info!("[FrameRing] Pre-allocating {} slots of {} bytes each for max {}x{} ({}MB total)",
            RING_SLOTS, NDI_MAX_FRAME_SIZE, NDI_MAX_WIDTH, NDI_MAX_HEIGHT,
            (RING_SLOTS * NDI_MAX_FRAME_SIZE) / (1024 * 1024));
        Self {
            slots: std::array::from_fn(|_| FrameSlot {
                pixels: std::cell::UnsafeCell::new(vec![0u8; NDI_MAX_FRAME_SIZE]),
                frame_num: AtomicU32::new(0),
            }),
            write_idx: AtomicU32::new(0),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            total_written: AtomicU32::new(0),
        }
    }

    /// Write a frame to the current slot (called by render thread)
    /// Returns the slot index that was written
    /// Supports up to 4K (3840x2160)
    pub fn write(&self, pixels: &[u8], width: u32, height: u32) -> u32 {
        let frame_size = (width * height * 4) as usize;
        debug_assert!(frame_size <= NDI_MAX_FRAME_SIZE,
            "FrameRing: {}x{} exceeds max {}x{}", width, height, NDI_MAX_WIDTH, NDI_MAX_HEIGHT);

        let idx = self.write_idx.load(Ordering::Relaxed) as usize;
        let frame_num = self.total_written.fetch_add(1, Ordering::Relaxed) + 1;

        // Store current dimensions (safe: atomics, buffers never resize)
        self.width.store(width, Ordering::Relaxed);
        self.height.store(height, Ordering::Relaxed);

        // Copy pixels to current slot (no lock needed - we own this slot)
        let slot_pixels = unsafe { &mut *self.slots[idx].pixels.get() };
        if pixels.len() <= slot_pixels.len() {
            slot_pixels[..pixels.len()].copy_from_slice(pixels);
        }

        // Mark slot with frame number
        self.slots[idx].frame_num.store(frame_num, Ordering::Release);

        // Advance write index
        let next_idx = ((idx + 1) % RING_SLOTS) as u32;
        self.write_idx.store(next_idx, Ordering::Release);

        idx as u32
    }

    /// Try to read the oldest ready frame (called by NDI thread)
    /// Returns (pixels_ptr, width, height, frame_num) or None if no frame ready
    pub fn try_read(&self) -> Option<(&[u8], u32, u32, u32)> {
        let write_idx = self.write_idx.load(Ordering::Acquire) as usize;

        // Read from oldest complete slot (RING_SLOTS - 1 frames behind writer)
        let read_idx = (write_idx + 1) % RING_SLOTS;
        let frame_num = self.slots[read_idx].frame_num.load(Ordering::Acquire);

        if frame_num == 0 {
            return None; // Slot hasn't been written yet
        }

        let width = self.width.load(Ordering::Acquire);
        let height = self.height.load(Ordering::Acquire);

        if width == 0 || height == 0 {
            return None;
        }

        let frame_size = (width * height * 4) as usize;
        let slot_pixels = unsafe { &*self.slots[read_idx].pixels.get() };
        Some((&slot_pixels[..frame_size], width, height, frame_num))
    }

    /// Get current dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width.load(Ordering::Relaxed), self.height.load(Ordering::Relaxed))
    }

    /// Get total frames written
    pub fn frames_written(&self) -> u32 {
        self.total_written.load(Ordering::Relaxed)
    }
}

impl Default for FrameRing {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// Audio Ring Buffer for NDI Audio Output
// =========================================================================

/// Audio ring buffer constants
const AUDIO_RING_SLOTS: usize = 32;  // Adequate slots for 20ms chunks
/// Audio chunk size (samples per channel per write)
/// At 48kHz, 960 samples = 20ms of audio per slot (matches SCK delivery)
const AUDIO_CHUNK_SIZE: usize = 960;
/// Max channels (stereo)
const AUDIO_MAX_CHANNELS: usize = 2;
/// Max audio slot size (samples * channels)
const AUDIO_SLOT_SIZE: usize = AUDIO_CHUNK_SIZE * AUDIO_MAX_CHANNELS;

/// Lock-free audio ring for NDI audio output
///
/// Similar to FrameRing but for float32 audio samples.
/// - Audio capture writes interleaved stereo float32 samples
/// - NDI sender reads and sends to network
pub struct AudioRing {
    /// Pre-allocated audio slots (float32 samples)
    slots: [AudioSlot; AUDIO_RING_SLOTS],
    /// Current write index
    write_idx: AtomicU32,
    /// Current read index
    read_idx: AtomicU32,
    /// Sample rate (typically 48000)
    sample_rate: AtomicU32,
    /// Number of channels (1 or 2)
    channels: AtomicU32,
    /// Total samples written
    total_written: AtomicU32,
    /// Audio enabled flag
    enabled: AtomicBool,
}

/// A single audio slot
struct AudioSlot {
    /// Audio samples (interleaved float32)
    samples: std::cell::UnsafeCell<[f32; AUDIO_SLOT_SIZE]>,
    /// Number of samples in this slot
    num_samples: AtomicU32,
    /// Slot sequence number
    seq: AtomicU32,
}

unsafe impl Send for AudioRing {}
unsafe impl Sync for AudioRing {}

impl AudioRing {
    /// Create a new audio ring
    pub fn new() -> Self {
        log::info!("[AudioRing] Pre-allocating {} slots of {} samples each",
            AUDIO_RING_SLOTS, AUDIO_CHUNK_SIZE);
        Self {
            slots: std::array::from_fn(|_| AudioSlot {
                samples: std::cell::UnsafeCell::new([0.0f32; AUDIO_SLOT_SIZE]),
                num_samples: AtomicU32::new(0),
                seq: AtomicU32::new(0),
            }),
            write_idx: AtomicU32::new(0),
            read_idx: AtomicU32::new(0),
            sample_rate: AtomicU32::new(48000),
            channels: AtomicU32::new(2),
            total_written: AtomicU32::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    /// Enable/disable audio
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    /// Check if audio is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Write audio samples (called by audio capture thread)
    /// samples: interleaved float32 (L,R,L,R,...)
    /// Splits large writes into multiple slots if needed
    pub fn write(&self, samples: &[f32], sample_rate: u32, channels: u32) {
        if !self.enabled.load(Ordering::Acquire) || samples.is_empty() {
            return;
        }

        // Update format info
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
        self.channels.store(channels, Ordering::Relaxed);

        // Write samples, splitting into multiple slots if needed
        let mut offset = 0;
        while offset < samples.len() {
            let read_idx = self.read_idx.load(Ordering::Acquire) as usize;
            let write_idx = self.write_idx.load(Ordering::Relaxed) as usize;

            // Check if ring is full (would overwrite unread data)
            let next_write = (write_idx + 1) % AUDIO_RING_SLOTS;
            if next_write == read_idx {
                // Ring full - drop this chunk (better than corrupting)
                return;
            }

            let seq = self.total_written.fetch_add(1, Ordering::Relaxed) + 1;
            let remaining = samples.len() - offset;
            let copy_len = remaining.min(AUDIO_SLOT_SIZE);

            // Copy samples to slot
            let slot_samples = unsafe { &mut *self.slots[write_idx].samples.get() };
            slot_samples[..copy_len].copy_from_slice(&samples[offset..offset + copy_len]);

            // Store metadata
            let samples_per_ch = copy_len / channels.max(1) as usize;
            self.slots[write_idx].num_samples.store(samples_per_ch as u32, Ordering::Relaxed);
            self.slots[write_idx].seq.store(seq, Ordering::Release);

            // Advance write index
            self.write_idx.store(next_write as u32, Ordering::Release);

            offset += copy_len;
        }
    }

    /// Try to read audio samples (called by NDI sender thread)
    /// Returns (samples, num_samples_per_channel, sample_rate, channels) or None
    pub fn try_read(&self) -> Option<(&[f32], u32, u32, u32)> {
        if !self.enabled.load(Ordering::Acquire) {
            return None;
        }

        let write_idx = self.write_idx.load(Ordering::Acquire) as usize;
        let read_idx = self.read_idx.load(Ordering::Relaxed) as usize;

        // Check if there's data to read
        if read_idx == write_idx {
            return None; // Empty
        }

        let slot = &self.slots[read_idx];
        let seq = slot.seq.load(Ordering::Acquire);
        if seq == 0 {
            return None; // Not written yet
        }

        let num_samples = slot.num_samples.load(Ordering::Acquire);
        let sample_rate = self.sample_rate.load(Ordering::Acquire);
        let channels = self.channels.load(Ordering::Acquire);

        if num_samples == 0 || channels == 0 {
            return None;
        }

        let total_samples = (num_samples * channels) as usize;
        let slot_samples = unsafe { &*slot.samples.get() };

        // Advance read index
        let next_read = ((read_idx + 1) % AUDIO_RING_SLOTS) as u32;
        self.read_idx.store(next_read, Ordering::Release);

        Some((&slot_samples[..total_samples], num_samples, sample_rate, channels))
    }

    /// Get audio format info
    pub fn format(&self) -> (u32, u32) {
        (self.sample_rate.load(Ordering::Relaxed), self.channels.load(Ordering::Relaxed))
    }
}

impl Default for AudioRing {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputManager {
    /// Create a new output manager
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            native_window: None,
            ndi_sender: None,
            recorder: None,
            segmented_recorder: None,
            db: None,
            use_segmented_recording: true, // Default to crash-resilient recording
            frame_buffer: Arc::new(Mutex::new(FrameBuffer::default())),
            frame_ready: Arc::new(AtomicBool::new(false)),
            frame_ring: Arc::new(FrameRing::new()),
            audio_ring: Arc::new(AudioRing::new()),
            ndi_source_sample_rate: 48000, // Default, updated from audio capture
            #[cfg(target_os = "macos")]
            shared_surface: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "macos")]
            frame_number: Arc::new(AtomicU32::new(0)),
            is_processing: Arc::new(AtomicBool::new(false)),
            ndi_drain_target: Arc::new(Mutex::new(Vec::new())),
            rec_drain_target: Arc::new(Mutex::new(Vec::new())),
            rtmp_drain_target: Arc::new(Mutex::new(Vec::new())),
            drain_running: Arc::new(AtomicBool::new(false)),
            drain_handles: Vec::new(),
            ndi_writer_handles: Vec::new(),
            rec_writer_handles: Vec::new(),
            rtmp_writer_handles: Vec::new(),
            ndi_gains: OutputGainSet::new(2.0, 8.0), // SCK default: cal=2.0, cap=8.0
            rec_gains: OutputGainSet::new(2.0, 8.0),
            rec_audio_volume_boost: AtomicU32::new(1.0f32.to_bits()), // Unity: writer thread pre-applies three-stage gain + limiter
            watermark_tempfile: None,
            rec_watermark_enabled: true,  // Default: watermark ON (free/anon)
            rec_account_id: 0,
        }
    }

    /// Update recording entitlements (call when entitlements change or before recording starts).
    pub fn set_recording_entitlements(&mut self, watermark: bool, account_id: u32) {
        self.rec_watermark_enabled = watermark;
        self.rec_account_id = account_id;
        log::info!("[Output] Recording entitlements: watermark={}, account_id={}", watermark, account_id);
    }

    /// Check if recording is processing (stitching segments)
    pub fn is_processing(&self) -> bool {
        self.is_processing.load(Ordering::Relaxed)
    }

    /// Set the database for segment journal
    pub fn set_database(&mut self, db: crate::db::Database) {
        log::info!("[Output] Database set for segmented recording");
        self.db = Some(db);
    }

    /// Enable/disable segmented recording (crash-resilient mode)
    pub fn set_use_segmented_recording(&mut self, enabled: bool) {
        log::info!("[Output] Segmented recording: {}", if enabled { "enabled" } else { "disabled (legacy mode)" });
        self.use_segmented_recording = enabled;
    }

    /// Set the source audio sample rate for NDI resampling
    pub fn set_ndi_source_sample_rate(&mut self, sample_rate: u32) {
        log::info!("[Output] NDI source sample rate set to {}Hz", sample_rate);
        self.ndi_source_sample_rate = sample_rate;
    }

    /// Check if native output window is active
    #[cfg(target_os = "macos")]
    pub fn has_native_window(&self) -> bool {
        self.native_window.is_some()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn has_native_window(&self) -> bool {
        false
    }

    /// Open native output window
    #[cfg(target_os = "macos")]
    pub fn open_native_window(&mut self) -> anyhow::Result<()> {
        if self.native_window.is_some() {
            log::info!("[Output] Native window already open");
            return Ok(());
        }

        log::info!("[Output] Opening native output window");

        // Create shared IOSurface if not exists
        {
            let mut surface_guard = self.shared_surface.lock();
            if surface_guard.is_none() {
                *surface_guard = Some(SharedIOSurface::new(1920, 1080));
            }
        }

        let window = NativeOutputWindow::new(
            self.shared_surface.clone(),
            self.frame_number.clone(),
            self.frame_buffer.clone(),
            self.frame_ready.clone(),
        )?;
        self.native_window = Some(window);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn open_native_window(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("Native output window not supported on this platform")
    }

    /// Close native output window
    #[cfg(target_os = "macos")]
    pub fn close_native_window(&mut self) {
        if let Some(window) = self.native_window.take() {
            log::info!("[Output] Closing native output window");
            window.close();
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn close_native_window(&mut self) {
        // No-op on non-macOS
    }

    /// Toggle native output window
    #[cfg(target_os = "macos")]
    pub fn toggle_native_window(&mut self) -> anyhow::Result<bool> {
        if self.native_window.is_some() {
            self.close_native_window();
            Ok(false)
        } else {
            self.open_native_window()?;
            Ok(true)
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn toggle_native_window(&mut self) -> anyhow::Result<bool> {
        anyhow::bail!("Native output window not supported on this platform")
    }

    /// Get the shared IOSurface ID for binding in main app's GL context
    /// Returns (surface_id, width, height) or None if no surface
    #[cfg(target_os = "macos")]
    pub fn get_iosurface_id(&self) -> Option<(u32, u32, u32)> {
        let guard = self.shared_surface.lock();
        guard.as_ref().map(|s| (s.surface_id(), s.width, s.height))
    }

    /// Ensure shared IOSurface is the correct size, resizing if needed
    #[cfg(target_os = "macos")]
    pub fn ensure_surface_size(&self, width: u32, height: u32) -> Option<u32> {
        let mut guard = self.shared_surface.lock();
        if let Some(ref mut surface) = *guard {
            surface.resize(width, height);
            Some(surface.surface_id())
        } else {
            *guard = Some(SharedIOSurface::new(width, height));
            guard.as_ref().map(|s| s.surface_id())
        }
    }

    /// Signal that a new frame has been rendered to the IOSurface
    #[cfg(target_os = "macos")]
    pub fn signal_frame_ready(&self) {
        self.frame_number.fetch_add(1, Ordering::Release);
    }

    /// Get the current frame number
    #[cfg(target_os = "macos")]
    pub fn get_frame_number(&self) -> u32 {
        self.frame_number.load(Ordering::Acquire)
    }

    /// Get frame number Arc for passing to callbacks (zero-copy signaling)
    #[cfg(target_os = "macos")]
    pub fn get_frame_number_arc(&self) -> Arc<AtomicU32> {
        self.frame_number.clone()
    }

    /// Update frame buffer with new pixels (fallback for non-IOSurface path)
    pub fn update_frame(&self, pixels: &[u8], width: u32, height: u32) {
        let mut buffer = self.frame_buffer.lock();

        // Resize if needed
        let size = (width * height * 4) as usize;
        if buffer.pixels.len() != size {
            buffer.pixels.resize(size, 0);
        }

        buffer.pixels.copy_from_slice(pixels);
        buffer.width = width;
        buffer.height = height;
        buffer.frame_number += 1;

        // Signal new frame ready
        self.frame_ready.store(true, Ordering::Release);
    }

    /// Set native window fullscreen mode
    #[cfg(target_os = "macos")]
    pub fn set_native_fullscreen(&mut self, fullscreen: bool) {
        if let Some(ref window) = self.native_window {
            window.set_fullscreen(fullscreen);
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_native_fullscreen(&mut self, _fullscreen: bool) {
        // No-op on non-macOS
    }

    /// Check if native window is fullscreen
    #[cfg(target_os = "macos")]
    pub fn is_native_fullscreen(&self) -> bool {
        self.native_window
            .as_ref()
            .map(|w| w.is_fullscreen())
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn is_native_fullscreen(&self) -> bool {
        false
    }

    // ==================== NDI Output Methods ====================

    /// Check if NDI output is active
    pub fn has_ndi_output(&self) -> bool {
        self.ndi_sender.as_ref().map(|s| s.is_active()).unwrap_or(false)
    }

    /// Check if NDI library is available
    pub fn is_ndi_available(&self) -> bool {
        self.ndi_sender.as_ref().map(|s| s.is_available()).unwrap_or(false)
    }

    /// Start NDI output
    pub fn start_ndi_output(&mut self) -> anyhow::Result<()> {
        if self.has_ndi_output() {
            log::info!("[Output] NDI output already active");
            return Ok(());
        }

        log::info!("[Output] Starting NDI output (source_rate={}Hz)", self.ndi_source_sample_rate);

        let mut config = NdiConfig::default();
        config.source_sample_rate = self.ndi_source_sample_rate;

        let mut sender = NdiSender::new(
            config,
            self.frame_ring.clone(),
        )?;

        sender.start()?;

        // Create SPSC queue between drain thread and writer thread
        let (producer, consumer) = rtrb::RingBuffer::<AudioChunk>::new(QUEUE_SLOTS_NDI);

        // Give producer to drain thread
        let mut targets = self.ndi_drain_target.lock();
        targets.clear();
        targets.push(producer);
        drop(targets);

        // Spawn writer thread: pops chunks from SPSC → applies gain+limiter → extends NDI audio buffer
        let audio_buf = sender.get_audio_buffer_arc();
        let handle = spawn_output_writer("ndi", consumer, audio_buf, self.ndi_gains.clone(), 0);
        self.ndi_writer_handles.push(handle);

        self.ndi_sender = Some(sender);
        Ok(())
    }

    /// Set NDI calibration and assist cap (called on source change).
    pub fn set_ndi_calibration(&self, cal: f32, cap: f32) {
        self.ndi_gains.set_calibration(cal, cap);
        log::info!("[Output] NDI calibration set: cal={:.2} cap={:.1}", cal, cap);
    }

    /// Set NDI user gain (controlled by UI slider).
    pub fn set_ndi_user_gain(&self, gain: f32) {
        self.ndi_gains.set_user(gain);
    }

    /// Set recorder calibration and assist cap (called on source change).
    pub fn set_rec_calibration(&self, cal: f32, cap: f32) {
        self.rec_gains.set_calibration(cal, cap);
        log::info!("[Output] Recorder calibration set: cal={:.2} cap={:.1}", cal, cap);
    }

    /// Set audio volume boost for recording FFmpeg mux (1.0 = unity, writer pre-applies gain).
    pub fn set_rec_audio_volume_boost(&self, boost: f32) {
        self.rec_audio_volume_boost.store(boost.to_bits(), Ordering::Relaxed);
        log::info!("[Output] Recorder audio volume boost set: {:.1}x", boost);
    }

    /// Set recorder user gain (controlled by UI slider).
    pub fn set_rec_user_gain(&self, gain: f32) {
        self.rec_gains.set_user(gain);
    }

    /// Get current gain values for UI display.
    pub fn get_audio_gains(&self) -> AudioGains {
        let (ndi_user, ndi_assist, ndi_effective) = self.ndi_gains.components();
        let (rec_user, rec_assist, rec_effective) = self.rec_gains.components();
        AudioGains {
            ndi_user,
            ndi_assist,
            ndi_effective,
            ndi_assist_hold: self.ndi_gains.assist_hold.load(Ordering::Relaxed),
            ndi_audio_present: self.ndi_gains.audio_present.load(Ordering::Relaxed),
            rec_user,
            rec_assist,
            rec_effective,
            rec_assist_hold: self.rec_gains.assist_hold.load(Ordering::Relaxed),
            rec_audio_present: self.rec_gains.audio_present.load(Ordering::Relaxed),
        }
    }

    /// Enable/disable NDI audio output
    pub fn set_ndi_audio_enabled(&mut self, enabled: bool) {
        if let Some(ref sender) = self.ndi_sender {
            sender.set_audio_enabled(enabled);
        }
        // Also update the ring directly (in case NDI is started later)
        self.audio_ring.set_enabled(enabled);
    }

    /// Check if NDI audio is enabled
    pub fn is_ndi_audio_enabled(&self) -> bool {
        self.ndi_sender.as_ref().map(|s| s.is_audio_enabled()).unwrap_or(false)
    }

    /// Get audio ring for writing audio samples
    pub fn get_audio_ring(&self) -> Arc<AudioRing> {
        self.audio_ring.clone()
    }

    /// Stop NDI output
    pub fn stop_ndi_output(&mut self) {
        // Drop producers (signals writer threads to exit)
        self.ndi_drain_target.lock().clear();

        // Join writer threads
        for handle in self.ndi_writer_handles.drain(..) {
            let _ = handle.join();
        }

        if let Some(mut sender) = self.ndi_sender.take() {
            log::info!("[Output] Stopping NDI output");
            sender.stop();
        }
    }

    /// Toggle NDI output
    pub fn toggle_ndi_output(&mut self) -> anyhow::Result<bool> {
        if self.has_ndi_output() {
            self.stop_ndi_output();
            Ok(false)
        } else {
            self.start_ndi_output()?;
            Ok(true)
        }
    }

    /// Get NDI frames sent count
    pub fn ndi_frames_sent(&self) -> u32 {
        self.ndi_sender.as_ref().map(|s| s.frames_sent()).unwrap_or(0)
    }

    /// Get NDI connected receivers count
    pub fn ndi_connections(&self) -> u32 {
        self.ndi_sender.as_ref().map(|s| s.connections()).unwrap_or(0)
    }

    /// Get NDI tally state
    pub fn ndi_tally(&self) -> NdiTally {
        self.ndi_sender.as_ref().map(|s| s.tally()).unwrap_or_default()
    }

    // ==================== Recording Methods ====================

    /// Check if recording is available (ffmpeg installed)
    pub fn is_recording_available(&self) -> bool {
        // Recording is available if we have a database (for segmented) or ffmpeg (for legacy)
        // Always allow fallback to legacy recording if segmented isn't available
        let segmented_available = self.use_segmented_recording && self.db.is_some();
        let legacy_available = self.recorder.as_ref().map(|r| r.is_available()).unwrap_or(true);

        let available = segmented_available || legacy_available;

        // Log once at startup or when state changes (avoid spamming)
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log::info!(
                "[Output] Recording availability: segmented={} (db={}), legacy={}, result={}",
                segmented_available,
                self.db.is_some(),
                legacy_available,
                available
            );
        }

        available
    }

    /// Check if recording is active
    pub fn is_recording(&self) -> bool {
        // Check segmented recorder first
        if let Some(ref recorder) = self.segmented_recorder {
            if recorder.is_recording() {
                return true;
            }
        }
        // Fall back to legacy recorder
        self.recorder.as_ref().map(|r| r.is_recording()).unwrap_or(false)
    }

    /// Get recording state for UI (legacy format)
    pub fn get_recording_state(&self) -> RecordingState {
        // If using segmented recorder, convert metrics to RecordingState
        if let Some(ref recorder) = self.segmented_recorder {
            if recorder.is_recording() {
                let metrics = recorder.get_metrics();
                return RecordingState {
                    active: true,
                    duration_secs: metrics.total_duration_ms as f32 / 1000.0,
                    frames_recorded: metrics.video_frames_encoded,
                    audio_samples: metrics.audio_samples_encoded,
                    output_path: metrics.output_dir,
                };
            }
        }
        // Fall back to legacy recorder
        self.recorder.as_ref()
            .map(|r| r.get_state())
            .unwrap_or_default()
    }

    /// Prepare watermark temp file and metadata comment based on current entitlements.
    /// Returns (watermark_logo_path, metadata_comment).
    fn prepare_recording_fingerprint(&mut self) -> (Option<std::path::PathBuf>, Option<String>) {
        // Watermark logo temp file
        let watermark_path = if self.rec_watermark_enabled {
            match tempfile::Builder::new()
                .prefix("compeller_wm_")
                .suffix(".png")
                .tempfile()
            {
                Ok(mut tf) => {
                    use std::io::Write;
                    if tf.write_all(WATERMARK_LOGO_PNG).is_ok() {
                        let path = tf.path().to_path_buf();
                        self.watermark_tempfile = Some(tf); // Keep alive
                        log::info!("[Output] Watermark logo written to {:?}", path);
                        Some(path)
                    } else {
                        log::warn!("[Output] Failed to write watermark logo to temp file");
                        None
                    }
                }
                Err(e) => {
                    log::warn!("[Output] Failed to create watermark temp file: {}", e);
                    None
                }
            }
        } else {
            self.watermark_tempfile = None;
            None
        };

        // Metadata comment (always embedded if account_id > 0)
        let metadata_comment = if self.rec_account_id > 0 {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;
            Some(encode_metadata(self.rec_account_id, ts))
        } else {
            None
        };

        (watermark_path, metadata_comment)
    }

    /// Start recording to MP4 file
    ///
    /// For segmented recording, `output_path` is treated as the output directory.
    /// For legacy recording, it's the full file path.
    pub fn start_recording(&mut self, output_path: std::path::PathBuf) -> anyhow::Result<()> {
        if self.is_recording() {
            anyhow::bail!("Recording already in progress");
        }

        // Prepare watermark + metadata based on current entitlements
        let (watermark_path, metadata_comment) = self.prepare_recording_fingerprint();

        // Use segmented recording if enabled and database is available
        if self.use_segmented_recording {
            if let Some(ref db) = self.db {
                log::info!("[Output] Starting segmented recording to {:?}", output_path);

                // Determine output directory
                let base_dir = if output_path.is_dir() {
                    output_path
                } else {
                    output_path.parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                };

                // Ensure recordings go into a "recordings" subdirectory (if not already there)
                let output_dir = if base_dir.ends_with("recordings") {
                    base_dir
                } else {
                    base_dir.join("recordings")
                };
                std::fs::create_dir_all(&output_dir)
                    .with_context(|| format!("Failed to create recordings directory: {:?}", output_dir))?;

                // Get frame dimensions from ring
                let (width, height) = self.frame_ring.dimensions();
                let (width, height) = if width > 0 && height > 0 {
                    (width, height)
                } else {
                    (1920, 1080) // Default if not yet available
                };

                // Create recorder config
                let config = RecorderConfig {
                    output_dir,
                    segment_duration_ms: 60_000, // 60 second segments
                    width,
                    height,
                    fps: 60,
                    video_bitrate_kbps: 20_000,
                    audio_sample_rate: 48_000,
                    audio_bitrate_kbps: 192,
                    audio_volume_boost: f32::from_bits(self.rec_audio_volume_boost.load(Ordering::Relaxed)),
                    watermark_logo_path: watermark_path.clone(),
                    metadata_comment: metadata_comment.clone(),
                };

                // Create and start segmented recorder
                let mut recorder = SegmentedRecorder::new(
                    self.frame_ring.clone(),
                    db.clone(),
                    config,
                );
                recorder.start()?;

                // Create SPSC queue between drain thread and writer thread
                let (producer, consumer) = rtrb::RingBuffer::<AudioChunk>::new(QUEUE_SLOTS_NDI);

                let mut targets = self.rec_drain_target.lock();
                targets.clear();
                targets.push(producer);
                drop(targets);

                // Spawn writer thread: pops chunks → applies gain+limiter → extends recorder audio buffer
                let audio_buf = recorder.get_audio_buffer_arc();
                let handle = spawn_output_writer("rec-seg", consumer, audio_buf, self.rec_gains.clone(), self.rec_account_id);
                self.rec_writer_handles.push(handle);

                self.segmented_recorder = Some(recorder);
                return Ok(());
            } else {
                log::warn!("[Output] Database not set, falling back to legacy recording");
            }
        }

        // Fall back to legacy recording
        log::info!("[Output] Starting legacy recording to {:?}", output_path);

        // Create recorder if needed
        if self.recorder.is_none() {
            let mut rec = Recorder::new(self.frame_ring.clone());
            rec.set_audio_boost(1.0); // Unity: writer thread pre-applies gain
            rec.set_fingerprint(watermark_path, metadata_comment);
            self.recorder = Some(rec);
        } else if let Some(ref mut rec) = self.recorder {
            rec.set_fingerprint(watermark_path, metadata_comment);
        }

        // Start recording and wire drain target with SPSC queue + writer
        if let Some(ref mut recorder) = self.recorder {
            recorder.start(output_path)?;

            let (producer, consumer) = rtrb::RingBuffer::<AudioChunk>::new(QUEUE_SLOTS_NDI);

            let mut targets = self.rec_drain_target.lock();
            targets.clear();
            targets.push(producer);
            drop(targets);

            let audio_buf = recorder.get_audio_buffer_arc();
            let handle = spawn_output_writer("rec-legacy", consumer, audio_buf, self.rec_gains.clone(), self.rec_account_id);
            self.rec_writer_handles.push(handle);
        }

        Ok(())
    }

    /// Stop recording
    pub fn stop_recording(&mut self) -> anyhow::Result<()> {
        // Drop producers (signals writer threads to exit)
        self.rec_drain_target.lock().clear();

        // Join writer threads
        for handle in self.rec_writer_handles.drain(..) {
            let _ = handle.join();
        }

        // Stop segmented recorder if active
        if let Some(ref mut recorder) = self.segmented_recorder {
            if recorder.is_recording() {
                let metrics = recorder.stop()?;
                log::info!(
                    "[Output] Segmented recording stopped: {} segments, {}ms",
                    metrics.committed_segments,
                    metrics.total_duration_ms
                );

                // Stitch segments in background thread to avoid blocking UI
                if metrics.committed_segments > 0 {
                    if let (Some(ref db), Some(ref session_id)) = (&self.db, &metrics.session_id) {
                        // Get committed segments from database
                        let segments = db.get_committed_segments(session_id);
                        if !segments.is_empty() {
                            // Get session directory from first segment's path
                            // Segment path: .../recordings/react_20260121_234509/seg_00000.mp4
                            // Session dir:  .../recordings/react_20260121_234509/
                            let session_dir = segments[0].path.parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_default();
                            // Recordings dir is parent of session dir
                            let recordings_dir = session_dir.parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| session_dir.clone());

                            let session_id = session_id.clone();

                            log::info!(
                                "[Output] Stitch paths: session_dir={:?}, recordings_dir={:?}",
                                session_dir, recordings_dir
                            );

                            // Set processing flag before spawning stitch thread
                            self.is_processing.store(true, Ordering::Relaxed);
                            let is_processing = self.is_processing.clone();

                            // Spawn background thread for stitching
                            std::thread::spawn(move || {
                                let output_path = suggest_stitch_output(&recordings_dir, &session_id);
                                log::info!("[Output] Stitching {} segments to {:?}...", segments.len(), output_path);

                                match stitch_segments(&segments, &output_path) {
                                    Ok(result) => {
                                        log::info!(
                                            "[Output] Stitched {} segments to {:?} ({}ms)",
                                            result.segment_count,
                                            result.output_path,
                                            result.total_duration_ms
                                        );

                                        // Clean up segment files and directory
                                        let _ = cleanup_segments(&segments);
                                        let _ = std::fs::remove_dir(&session_dir);
                                    }
                                    Err(e) => {
                                        log::error!("[Output] Failed to stitch segments: {}", e);
                                        // Segments are still available in the session directory
                                    }
                                }

                                // Clear processing flag when done (success or error)
                                is_processing.store(false, Ordering::Relaxed);
                            });
                        }
                    }
                }

                self.segmented_recorder = None;
                self.watermark_tempfile = None; // Clean up temp logo
                return Ok(());
            }
        }

        // Stop legacy recorder
        if let Some(ref mut recorder) = self.recorder {
            recorder.stop()?;
        }
        self.watermark_tempfile = None; // Clean up temp logo
        Ok(())
    }

    /// Toggle recording (start or stop)
    pub fn toggle_recording(&mut self, output_path: Option<std::path::PathBuf>) -> anyhow::Result<bool> {
        if self.is_recording() {
            self.stop_recording()?;
            Ok(false)
        } else {
            let path = output_path.ok_or_else(|| anyhow::anyhow!("Output path required to start recording"))?;
            self.start_recording(path)?;
            Ok(true)
        }
    }

    // ==================== Segmented Recording Methods ====================

    /// Get segmented recording metrics (for UI indicator)
    /// Returns None if not using segmented recording or not recording
    pub fn get_segmented_recording_metrics(&self) -> Option<SegmentedRecordingMetrics> {
        self.segmented_recorder.as_ref()
            .filter(|r| r.is_recording())
            .map(|r| r.get_metrics())
    }

    /// Check for crashed recording sessions that can be recovered
    pub fn check_recording_recovery(&self) -> Vec<RecoveryInfo> {
        if let Some(ref db) = self.db {
            SegmentJournal::recover(db)
        } else {
            Vec::new()
        }
    }

    /// Export recovered segments to a single file
    pub fn export_recovered_session(&self, session_id: &str, output_path: &std::path::Path) -> anyhow::Result<StitchResult> {
        let db = self.db.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Database not available"))?;

        let segments = db.get_committed_segments(session_id);
        if segments.is_empty() {
            anyhow::bail!("No committed segments found for session {}", session_id);
        }

        let result = stitch_segments(&segments, output_path)?;

        // Mark session as recovered
        SegmentJournal::mark_recovered(db, session_id)?;

        Ok(result)
    }

    /// Discard a recovered session (delete segments)
    pub fn discard_recovered_session(&self, session_id: &str, output_dir: &std::path::Path) -> anyhow::Result<()> {
        let db = self.db.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Database not available"))?;

        SegmentJournal::discard_session(db, session_id, output_dir)
    }

    // ==================== General Methods ====================

    /// Check if any output is active (needs frame capture)
    pub fn needs_frame_capture(&self) -> bool {
        self.has_native_window() || self.has_ndi_output() || self.is_recording()
    }

    /// Get frame buffer and ready flag for passing to render callback (fallback path)
    pub fn get_frame_handles(&self) -> (Arc<Mutex<FrameBuffer>>, Arc<AtomicBool>) {
        (self.frame_buffer.clone(), self.frame_ready.clone())
    }

    /// Get lock-free frame ring for NDI (preferred path)
    pub fn get_frame_ring(&self) -> Arc<FrameRing> {
        self.frame_ring.clone()
    }

    /// Get shared IOSurface Arc for output window
    #[cfg(target_os = "macos")]
    pub fn get_shared_surface(&self) -> Arc<Mutex<Option<SharedIOSurface>>> {
        self.shared_surface.clone()
    }

    // ==================== Audio Drain Threads ====================

    /// Start independent audio drain threads for NDI, recorder, and RTMP.
    ///
    /// Each thread drains its AudioBus consumer at a stable 20ms cadence,
    /// completely independent of the GPU render loop. If the output is active
    /// (drain target is Some), samples are forwarded; otherwise discarded.
    pub fn start_audio_drains(
        &mut self,
        ndi_consumer: rtrb::Consumer<AudioFrame>,
        rec_consumer: rtrb::Consumer<AudioFrame>,
        rtmp_consumer: rtrb::Consumer<AudioFrame>,
    ) {
        self.stop_audio_drains();
        self.drain_running.store(true, Ordering::SeqCst);

        log::info!("[Output] Starting audio drain threads (20ms cadence, {} frames/chunk)", CHUNK_FRAMES);

        let h1 = spawn_drain_thread("ndi", ndi_consumer, self.ndi_drain_target.clone(), self.drain_running.clone());
        let h2 = spawn_drain_thread("rec", rec_consumer, self.rec_drain_target.clone(), self.drain_running.clone());
        let h3 = spawn_drain_thread("rtmp", rtmp_consumer, self.rtmp_drain_target.clone(), self.drain_running.clone());

        self.drain_handles = vec![h1, h2, h3];
    }

    /// Stop all audio drain threads and writer threads.
    pub fn stop_audio_drains(&mut self) {
        if !self.drain_handles.is_empty() {
            log::info!("[Output] Stopping audio drain threads...");

            // First drop all producers so writer threads can exit
            self.ndi_drain_target.lock().clear();
            self.rec_drain_target.lock().clear();
            self.rtmp_drain_target.lock().clear();

            // Stop drain threads
            self.drain_running.store(false, Ordering::SeqCst);
            for handle in self.drain_handles.drain(..) {
                let _ = handle.join();
            }

            // Join all writer threads (they'll exit once producers are dropped)
            for handle in self.ndi_writer_handles.drain(..) {
                let _ = handle.join();
            }
            for handle in self.rec_writer_handles.drain(..) {
                let _ = handle.join();
            }
            for handle in self.rtmp_writer_handles.drain(..) {
                let _ = handle.join();
            }

            log::info!("[Output] Audio drain + writer threads stopped");
        }
    }

    /// Add an RTMP drain target (called when a streamer starts).
    /// Caller provides the gain Arc so the UI can control it persistently.
    /// RTMP uses cal=1.0, cap=1.0 (Level Assist disabled — user gain only).
    pub fn add_rtmp_drain_target(&mut self, audio_buffer: Arc<Mutex<Vec<f32>>>, gain: Arc<AtomicU32>) {
        let (producer, consumer) = rtrb::RingBuffer::<AudioChunk>::new(QUEUE_SLOTS_RTMP);
        self.rtmp_drain_target.lock().push(producer);

        // Wrap caller's gain Arc in an OutputGainSet with cal=1.0, cap=1.0 (no Level Assist).
        let gains = OutputGainSet {
            calibration: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            assist: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            user: gain.clone(),
            assist_cap: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            reset_flag: Arc::new(AtomicBool::new(false)),
            assist_hold: Arc::new(AtomicBool::new(false)),
            audio_present: Arc::new(AtomicBool::new(false)),
        };
        let g = f32::from_bits(gain.load(Ordering::Relaxed));
        let handle = spawn_output_writer("rtmp", consumer, audio_buffer, gains, 0);
        self.rtmp_writer_handles.push(handle);
        log::info!("[Output] Added RTMP drain target (gain={:.1})", g);
    }

    /// Remove all RTMP drain targets (called when all streamers stop).
    /// Drops producers, joins writer threads.
    pub fn clear_rtmp_drain_targets(&mut self) {
        self.rtmp_drain_target.lock().clear();
        for handle in self.rtmp_writer_handles.drain(..) {
            let _ = handle.join();
        }
        log::info!("[Output] Cleared all RTMP drain targets");
    }
}

/// Chunk duration for drain threads (20ms at 48kHz = 960 frames).
const DRAIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Spawn a single audio drain thread.
///
/// Deadline-based scheduling: emits exactly one CHUNK_FRAMES chunk per tick.
/// Always sends the chunk (even if padded with silence) — never skips ticks.
/// Pushes fixed-size AudioChunk into per-output SPSC producers (structurally non-blocking).
/// If a producer's queue is full, the chunk is dropped for that output only.
/// When >100ms behind deadline: sheds backlog from consumer, resets deadline.
fn spawn_drain_thread(
    name: &'static str,
    mut consumer: rtrb::Consumer<AudioFrame>,
    target: DrainTarget,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("audio-drain-{}", name))
        .spawn(move || {
            log::info!("[AudioDrain-{}] Started: rate=48000 channels=2 layout=interleaved sample_type=f32 chunk={}frames/{}ms",
                name, CHUNK_FRAMES, DRAIN_INTERVAL.as_millis());

            // Fixed-size chunk buffer on stack
            let mut chunk: AudioChunk = [0.0f32; CHUNK_SAMPLES];

            // Accumulator position: tracks how many frames have been written into
            // the current chunk. Only sends when a full CHUNK_FRAMES is ready.
            let mut accum_pos: usize = 0;

            // Deadline scheduler state
            let mut next_deadline = std::time::Instant::now() + DRAIN_INTERVAL;

            // Per-second diagnostics
            let mut stat_chunks_sent: u64 = 0;
            let mut stat_drops: u64 = 0;
            let mut stat_frames_drained: u64 = 0;
            let mut stat_last_log = std::time::Instant::now();

            while running.load(Ordering::Acquire) {
                // Sleep until next deadline
                let now = std::time::Instant::now();
                if now < next_deadline {
                    std::thread::sleep(next_deadline - now);
                }
                next_deadline += DRAIN_INTERVAL;

                // Cap drift: if we fall more than 200ms behind, reset deadline
                // (don't shed — just catch up on next ticks)
                let now2 = std::time::Instant::now();
                if now2 > next_deadline + std::time::Duration::from_millis(200) {
                    next_deadline = now2 + DRAIN_INTERVAL;
                }

                // Drain ALL available frames from ring buffer into chunk accumulator.
                // Only send complete chunks — never pad with silence.
                loop {
                    match consumer.pop() {
                        Ok(frame) => {
                            chunk[accum_pos * 2] = frame[0];
                            chunk[accum_pos * 2 + 1] = frame[1];
                            accum_pos += 1;
                            stat_frames_drained += 1;

                            // Full chunk ready — send to all targets
                            if accum_pos >= CHUNK_FRAMES {
                                accum_pos = 0;
                                let mut targets = target.lock();
                                for producer in targets.iter_mut() {
                                    match producer.push(chunk) {
                                        Ok(()) => stat_chunks_sent += 1,
                                        Err(_) => stat_drops += 1,
                                    }
                                }
                                drop(targets);
                            }
                        }
                        Err(_) => break, // Ring empty — partial chunk stays in accumulator
                    }
                }

                // Log diagnostics once per 5 seconds
                if stat_last_log.elapsed() >= std::time::Duration::from_secs(5) {
                    let rate = stat_frames_drained as f64 / stat_last_log.elapsed().as_secs_f64();
                    log::info!("[AudioDrain-{}] rate={:.0}Hz chunks={} frames={} drops={} accum={}",
                        name, rate, stat_chunks_sent, stat_frames_drained, stat_drops, accum_pos);
                    stat_chunks_sent = 0;
                    stat_drops = 0;
                    stat_frames_drained = 0;
                    stat_last_log = std::time::Instant::now();
                }
            }
            log::info!("[AudioDrain-{}] Thread exiting", name);
        })
        .expect("Failed to spawn audio drain thread")
}

/// Spawn an output writer thread with three-stage gain + Level Assist + safety limiter.
///
/// Pops AudioChunk arrays from the SPSC consumer, computes effective gain
/// (calibration × assist × user), applies gain + peak limiter, then extends
/// the output's real audio buffer. Level Assist automatically brings quiet
/// sources to -12 dBFS target without per-beat AGC.
///
/// If `account_id > 0`, an ultrasonic DSSS fingerprint is embedded after the
/// limiter (always-on forensic tracing, not a premium feature).
///
/// Exits when the SPSC producer is dropped (consumer.is_abandoned() == true).
fn spawn_output_writer(
    name: &'static str,
    mut consumer: rtrb::Consumer<AudioChunk>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    gains: OutputGainSet,
    account_id: u32,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("audio-writer-{}", name))
        .spawn(move || {
            log::info!("[AudioWriter-{}] Started: rate=48000 channels=2 layout=interleaved sample_type=f32 chunk={}samples ceiling={:.2} account_id={}",
                name, CHUNK_SAMPLES, LIMITER_CEILING, account_id);

            let mut limiter = AudioLimiter::new();
            let cap = f32::from_bits(gains.assist_cap.load(Ordering::Relaxed));
            let mut level_assist = LevelAssist::new(cap);

            // Audio fingerprint: always embedded if account_id > 0 (forensic tracing)
            let mut fingerprinter = if account_id > 0 {
                Some(audio_fingerprint::AudioFingerprint::new(account_id, 48000))
            } else {
                None
            };

            // Per-second diagnostics
            let mut stat_chunks_written: u64 = 0;
            let mut stat_max_write_us: u64 = 0;
            let mut stat_last_log = std::time::Instant::now();

            loop {
                match consumer.pop() {
                    Ok(mut chunk) => {
                        // Check reset flag (source changed)
                        if gains.reset_flag.swap(false, Ordering::Relaxed) {
                            level_assist.reset();
                        }

                        // Read gain components
                        let cal = f32::from_bits(gains.calibration.load(Ordering::Relaxed));
                        let cap = f32::from_bits(gains.assist_cap.load(Ordering::Relaxed));
                        level_assist.cap_gain = cap;

                        // Compute post-calibration peak for Level Assist
                        let chunk_peak = chunk.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                        let post_cal_peak = chunk_peak * cal;

                        // Feed Level Assist (returns current assist gain + hold state)
                        let (assist, in_hold) = level_assist.push_and_evaluate(post_cal_peak);
                        gains.assist.store(assist.to_bits(), Ordering::Relaxed);
                        gains.assist_hold.store(in_hold, Ordering::Relaxed);
                        gains.audio_present.store(level_assist.audio_present, Ordering::Relaxed);

                        // Compute effective gain and apply
                        let user = f32::from_bits(gains.user.load(Ordering::Relaxed));
                        let effective = (cal * assist * user).min(128.0);
                        limiter.process(&mut chunk, effective);

                        // Apply audio fingerprint after limiter (ultrasonic, -40dB)
                        if let Some(ref mut fp) = fingerprinter {
                            fp.process(&mut chunk);
                        }

                        let t0 = std::time::Instant::now();
                        {
                            let mut buf = audio_buffer.lock();
                            buf.extend_from_slice(&chunk);
                        }
                        let write_us = t0.elapsed().as_micros() as u64;
                        stat_chunks_written += 1;
                        if write_us > stat_max_write_us {
                            stat_max_write_us = write_us;
                        }
                    }
                    Err(_) => {
                        if consumer.is_abandoned() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }

                // Log diagnostics once per second
                if stat_last_log.elapsed() >= std::time::Duration::from_secs(1) {
                    if stat_chunks_written > 0 {
                        let queue_depth = consumer.slots();
                        let cal = f32::from_bits(gains.calibration.load(Ordering::Relaxed));
                        let assist = f32::from_bits(gains.assist.load(Ordering::Relaxed));
                        let user = f32::from_bits(gains.user.load(Ordering::Relaxed));
                        let effective = (cal * assist * user).min(128.0);
                        let stats = limiter.take_stats();

                        // clip_chunks > 0: should be impossible with proper calibration
                        if stats.clip_chunks > 0 {
                            log::warn!("[AudioWriter-{}] CLIPS={} — pre={:.3} post_gain={:.3} post_lim={:.3} GR={:.1}dB cal={:.1} assist={:.1} user={:.1} eff={:.1}",
                                name, stats.clip_chunks, stats.pre_gain_peak, stats.post_gain_peak,
                                stats.post_limiter_peak, stats.max_gr_db, cal, assist, user, effective);
                        }
                        // Limiter working too hard: calibration gain too high
                        else if stats.active_ratio > 0.05 {
                            log::warn!("[AudioWriter-{}] Limiter too active: {:.1}% limited, GR={:.1}dB, pre={:.3} post_gain={:.3} post_lim={:.3} cal={:.1} assist={:.1} user={:.1} eff={:.1}",
                                name, stats.active_ratio * 100.0, stats.max_gr_db,
                                stats.pre_gain_peak, stats.post_gain_peak, stats.post_limiter_peak,
                                cal, assist, user, effective);
                        }
                        // Peaky material with occasional limiter engagement (normal)
                        else if stats.max_gr_db < -1.0 {
                            log::info!("[AudioWriter-{}] cal={:.1} assist={:.1} user={:.1} eff={:.1} pre={:.3} post_gain={:.3} post_lim={:.3} GR={:.1}dB active={:.1}%",
                                name, cal, assist, user, effective, stats.pre_gain_peak, stats.post_gain_peak,
                                stats.post_limiter_peak, stats.max_gr_db, stats.active_ratio * 100.0);
                        }
                        // Normal operation: quiet log with pre/post peaks
                        else if stats.pre_gain_peak > 0.001 {
                            log::info!("[AudioWriter-{}] cal={:.1} assist={:.1} user={:.1} eff={:.1} pre={:.3} post_gain={:.3} post_lim={:.3} written={} queue={}",
                                name, cal, assist, user, effective, stats.pre_gain_peak, stats.post_gain_peak,
                                stats.post_limiter_peak, stat_chunks_written, queue_depth);
                        }
                    }
                    stat_chunks_written = 0;
                    stat_max_write_us = 0;
                    stat_last_log = std::time::Instant::now();
                }
            }
            log::info!("[AudioWriter-{}] Exiting (producer dropped)", name);
        })
        .expect("Failed to spawn audio writer thread")
}

impl Default for OutputManager {
    fn default() -> Self {
        Self::new()
    }
}
