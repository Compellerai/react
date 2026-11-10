//! Segmented Recorder - Crash-Resilient Video Recording
//!
//! Records video in independent MP4 segments with two-phase commit.
//! A crash at 53:00 yields ~52:30+ of usable video (loss bounded by segment duration).
//!
//! Architecture:
//! - SegmentJournal: Two-phase commit for crash recovery
//! - Fmp4SegmentMuxer: Creates independently-playable MP4 segments
//! - Recording thread: Reads from FrameRing, manages segment rotation
//!
//! Key features:
//! - Keyframe-aligned segment boundaries
//! - Atomic rename + fsync for crash safety
//! - Push-based audio accumulation
//! - Bounded memory usage with backpressure

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::Mutex;

use super::fmp4_muxer::{Fmp4SegmentMuxer, MuxerConfig};
use super::segment_journal::{RecorderConfig, SegmentJournal};
use super::FrameRing;
use crate::db::{Database, SegmentMetadata};

/// Recording state for UI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentedRecordingState {
    /// Not recording
    Idle,
    /// Starting up
    Starting,
    /// Actively recording
    Recording,
    /// Finalizing current segment
    FinalizingSegment,
    /// Stopping recording
    Stopping,
    /// Recording stopped
    Stopped,
    /// Error occurred
    Error,
}

impl Default for SegmentedRecordingState {
    fn default() -> Self {
        Self::Idle
    }
}

/// Recording metrics for UI
#[derive(Debug, Clone)]
pub struct SegmentedRecordingMetrics {
    /// Current state
    pub state: SegmentedRecordingState,
    /// Current segment index
    pub current_segment: u32,
    /// Number of committed (safe) segments
    pub committed_segments: u32,
    /// Total recording duration in milliseconds
    pub total_duration_ms: u64,
    /// Current segment duration in milliseconds
    pub current_segment_duration_ms: u64,
    /// Total video frames encoded
    pub video_frames_encoded: u64,
    /// Video frames dropped (backpressure)
    pub video_frames_dropped: u64,
    /// Total audio samples encoded
    pub audio_samples_encoded: u64,
    /// Time of last segment commit
    pub last_commit_elapsed_ms: u64,
    /// Output directory
    pub output_dir: Option<PathBuf>,
    /// Session ID
    pub session_id: Option<String>,
}

impl Default for SegmentedRecordingMetrics {
    fn default() -> Self {
        Self {
            state: SegmentedRecordingState::Idle,
            current_segment: 0,
            committed_segments: 0,
            total_duration_ms: 0,
            current_segment_duration_ms: 0,
            video_frames_encoded: 0,
            video_frames_dropped: 0,
            audio_samples_encoded: 0,
            last_commit_elapsed_ms: 0,
            output_dir: None,
            session_id: None,
        }
    }
}

/// Backpressure constants
const MAX_PENDING_FRAMES: u64 = 30; // ~0.5s at 60fps

/// Segmented recorder for crash-resilient recording
pub struct SegmentedRecorder {
    /// Recording state
    state: Arc<Mutex<SegmentedRecordingState>>,
    /// Stop flag
    stop_flag: Arc<AtomicBool>,

    /// Frame ring buffer (shared with render thread)
    frame_ring: Arc<FrameRing>,
    /// Audio buffer (push-based)
    audio_buffer: Arc<Mutex<Vec<f32>>>,

    /// Configuration
    config: RecorderConfig,
    /// Database for journal
    db: Database,

    /// Metrics
    current_segment: Arc<AtomicU32>,
    committed_segments: Arc<AtomicU32>,
    video_frames_encoded: Arc<AtomicU64>,
    video_frames_dropped: Arc<AtomicU64>,
    audio_samples_encoded: Arc<AtomicU64>,
    start_time: Arc<Mutex<Option<Instant>>>,
    last_commit_time: Arc<Mutex<Option<Instant>>>,
    current_segment_start: Arc<Mutex<Option<Instant>>>,
    output_dir: Arc<Mutex<Option<PathBuf>>>,
    session_id: Arc<Mutex<Option<String>>>,

    /// Recording thread handle
    thread_handle: Option<JoinHandle<()>>,
}

impl SegmentedRecorder {
    /// Create a new segmented recorder
    pub fn new(frame_ring: Arc<FrameRing>, db: Database, config: RecorderConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(SegmentedRecordingState::Idle)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            frame_ring,
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(65536))),
            config,
            db,
            current_segment: Arc::new(AtomicU32::new(0)),
            committed_segments: Arc::new(AtomicU32::new(0)),
            video_frames_encoded: Arc::new(AtomicU64::new(0)),
            video_frames_dropped: Arc::new(AtomicU64::new(0)),
            audio_samples_encoded: Arc::new(AtomicU64::new(0)),
            start_time: Arc::new(Mutex::new(None)),
            last_commit_time: Arc::new(Mutex::new(None)),
            current_segment_start: Arc::new(Mutex::new(None)),
            output_dir: Arc::new(Mutex::new(None)),
            session_id: Arc::new(Mutex::new(None)),
            thread_handle: None,
        }
    }

    /// Push audio samples (called from main thread)
    pub fn push_audio(&self, samples: &[f32]) {
        if *self.state.lock() != SegmentedRecordingState::Recording {
            return;
        }
        self.audio_buffer.lock().extend_from_slice(samples);
    }

    /// Get Arc reference to the audio buffer (for direct sink dispatch from capture callback)
    pub fn get_audio_buffer_arc(&self) -> Arc<Mutex<Vec<f32>>> {
        self.audio_buffer.clone()
    }

    /// Check if recording is active (includes Starting state to ensure frames are captured)
    pub fn is_recording(&self) -> bool {
        matches!(
            *self.state.lock(),
            SegmentedRecordingState::Starting
                | SegmentedRecordingState::Recording
                | SegmentedRecordingState::FinalizingSegment
        )
    }

    /// Get current metrics
    pub fn get_metrics(&self) -> SegmentedRecordingMetrics {
        let state = *self.state.lock();
        let total_duration_ms = self
            .start_time
            .lock()
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let current_segment_duration_ms = self
            .current_segment_start
            .lock()
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let last_commit_elapsed_ms = self
            .last_commit_time
            .lock()
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);

        SegmentedRecordingMetrics {
            state,
            current_segment: self.current_segment.load(Ordering::Relaxed),
            committed_segments: self.committed_segments.load(Ordering::Relaxed),
            total_duration_ms,
            current_segment_duration_ms,
            video_frames_encoded: self.video_frames_encoded.load(Ordering::Relaxed),
            video_frames_dropped: self.video_frames_dropped.load(Ordering::Relaxed),
            audio_samples_encoded: self.audio_samples_encoded.load(Ordering::Relaxed),
            last_commit_elapsed_ms,
            output_dir: self.output_dir.lock().clone(),
            session_id: self.session_id.lock().clone(),
        }
    }

    /// Start recording
    pub fn start(&mut self) -> Result<()> {
        if self.is_recording() {
            anyhow::bail!("Already recording");
        }

        log::info!(
            "[SegmentedRecorder] Starting recording to {:?}",
            self.config.output_dir
        );

        // Reset state
        self.stop_flag.store(false, Ordering::Release);
        self.current_segment.store(0, Ordering::Relaxed);
        self.committed_segments.store(0, Ordering::Relaxed);
        self.video_frames_encoded.store(0, Ordering::Relaxed);
        self.video_frames_dropped.store(0, Ordering::Relaxed);
        self.audio_samples_encoded.store(0, Ordering::Relaxed);
        *self.start_time.lock() = Some(Instant::now());
        *self.last_commit_time.lock() = None;
        *self.current_segment_start.lock() = None;
        *self.output_dir.lock() = Some(self.config.output_dir.clone());
        self.audio_buffer.lock().clear();

        *self.state.lock() = SegmentedRecordingState::Starting;

        // Spawn recording thread
        let state = self.state.clone();
        let stop_flag = self.stop_flag.clone();
        let frame_ring = self.frame_ring.clone();
        let audio_buffer = self.audio_buffer.clone();
        let db = self.db.clone();
        let config = self.config.clone();
        let current_segment = self.current_segment.clone();
        let committed_segments = self.committed_segments.clone();
        let video_frames_encoded = self.video_frames_encoded.clone();
        let video_frames_dropped = self.video_frames_dropped.clone();
        let audio_samples_encoded = self.audio_samples_encoded.clone();
        let last_commit_time = self.last_commit_time.clone();
        let current_segment_start = self.current_segment_start.clone();
        let session_id_arc = self.session_id.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = Self::recording_thread(
                state.clone(),
                stop_flag,
                frame_ring,
                audio_buffer,
                db,
                config,
                current_segment,
                committed_segments,
                video_frames_encoded,
                video_frames_dropped,
                audio_samples_encoded,
                last_commit_time,
                current_segment_start,
                session_id_arc,
            ) {
                log::error!("[SegmentedRecorder] Recording error: {}", e);
                *state.lock() = SegmentedRecordingState::Error;
            }
        });

        self.thread_handle = Some(handle);
        Ok(())
    }

    /// Stop recording
    pub fn stop(&mut self) -> Result<SegmentedRecordingMetrics> {
        if !self.is_recording() {
            return Ok(self.get_metrics());
        }

        log::info!("[SegmentedRecorder] Stopping recording...");
        *self.state.lock() = SegmentedRecordingState::Stopping;
        self.stop_flag.store(true, Ordering::Release);

        // Wait for thread to finish
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        *self.state.lock() = SegmentedRecordingState::Stopped;

        let metrics = self.get_metrics();
        log::info!(
            "[SegmentedRecorder] Stopped: {} segments, {}ms",
            metrics.committed_segments,
            metrics.total_duration_ms
        );

        Ok(metrics)
    }

    /// Recording thread main loop
    fn recording_thread(
        state: Arc<Mutex<SegmentedRecordingState>>,
        stop_flag: Arc<AtomicBool>,
        frame_ring: Arc<FrameRing>,
        audio_buffer: Arc<Mutex<Vec<f32>>>,
        db: Database,
        config: RecorderConfig,
        current_segment: Arc<AtomicU32>,
        committed_segments: Arc<AtomicU32>,
        video_frames_encoded: Arc<AtomicU64>,
        video_frames_dropped: Arc<AtomicU64>,
        audio_samples_encoded: Arc<AtomicU64>,
        last_commit_time: Arc<Mutex<Option<Instant>>>,
        current_segment_start: Arc<Mutex<Option<Instant>>>,
        session_id_arc: Arc<Mutex<Option<String>>>,
    ) -> Result<()> {
        // Wait for first frame to get dimensions
        let (width, height) = loop {
            if stop_flag.load(Ordering::Acquire) {
                return Ok(());
            }
            let dims = frame_ring.dimensions();
            if dims.0 > 0 && dims.1 > 0 {
                break dims;
            }
            thread::sleep(Duration::from_millis(10));
        };

        // Wait for ring buffer to fill up before starting segment timer
        // The ring reader is 4 frames behind writer, so we need at least 5 frames
        // before try_read() will return data
        log::info!("[SegmentedRecorder] Waiting for frame ring to fill...");
        let mut wait_count = 0;
        loop {
            if stop_flag.load(Ordering::Acquire) {
                return Ok(());
            }
            // Check if we can actually read a frame
            if frame_ring.try_read().is_some() {
                break;
            }
            wait_count += 1;
            if wait_count % 100 == 0 {
                log::debug!("[SegmentedRecorder] Still waiting for frames... ({}ms)", wait_count * 10);
            }
            thread::sleep(Duration::from_millis(10));
        }
        log::info!("[SegmentedRecorder] Frame ring ready after {}ms", wait_count * 10);

        log::info!(
            "[SegmentedRecorder] Recording {}x{} @ {}fps",
            width,
            height,
            config.fps
        );

        // Update config with actual dimensions
        let mut config = config.clone();
        config.width = width;
        config.height = height;

        // Create segment journal
        let mut journal = SegmentJournal::new(db, config.output_dir.clone(), &config)?;
        *session_id_arc.lock() = Some(journal.session_id().to_string());

        // Create muxer config
        let muxer_config = MuxerConfig {
            width,
            height,
            fps: config.fps,
            video_bitrate_kbps: config.video_bitrate_kbps,
            audio_sample_rate: config.audio_sample_rate,
            audio_bitrate_kbps: config.audio_bitrate_kbps,
            use_hardware_encoder: true,
            audio_volume_boost: config.audio_volume_boost,
            watermark_logo_path: config.watermark_logo_path.clone(),
            metadata_comment: config.metadata_comment.clone(),
        };

        *state.lock() = SegmentedRecordingState::Recording;

        // Begin first segment
        let mut pts_ns: u64 = 0;
        let frame_duration_ns = 1_000_000_000 / config.fps as u64;
        let segment_start_time = Instant::now();
        *current_segment_start.lock() = Some(segment_start_time);

        let tmp_path = journal.begin_segment(pts_ns)?;
        let mut muxer = Fmp4SegmentMuxer::new(tmp_path, muxer_config.clone())?;
        current_segment.store(0, Ordering::Relaxed);

        let mut segment_frames: u64 = 0;
        let mut segment_audio_samples: u64 = 0;
        let mut total_frames: u64 = 0;

        // Wall-clock pacing: only consume one frame per frame_duration
        let recording_start = Instant::now();

        // Calculate frames per segment (e.g., 60 seconds @ 60fps = 3600 frames)
        let frames_per_segment = (config.segment_duration_ms * config.fps as u64) / 1000;
        log::info!("[SegmentedRecorder] Segment rotation every {} frames ({}ms @ {}fps)", frames_per_segment, config.segment_duration_ms, config.fps);

        // Main recording loop
        while !stop_flag.load(Ordering::Acquire) {
            // Check for segment rotation (frame count based, not wall-clock time)
            // This ensures each segment has the expected number of frames
            if segment_frames >= frames_per_segment {
                let seg_idx = current_segment.load(Ordering::Relaxed);
                log::debug!(">>> [SegmentedRecorder] Rotating segment {} -> {}", seg_idx, seg_idx + 1);
                *state.lock() = SegmentedRecordingState::FinalizingSegment;

                // Drain audio buffer into current segment before finalizing
                // This ensures all audio received so far goes to the correct segment
                {
                    let mut buf = audio_buffer.lock();
                    if !buf.is_empty() {
                        let samples: Vec<f32> = buf.drain(..).collect();
                        drop(buf);
                        if let Err(e) = muxer.write_audio(&samples) {
                            log::warn!("[SegmentedRecorder] Pre-finalize audio drain error: {}", e);
                        } else {
                            let sample_count = samples.len() as u64 / 2;
                            audio_samples_encoded.fetch_add(sample_count, Ordering::Relaxed);
                        }
                    }
                }

                // Finalize current segment
                let result = muxer.finalize()?;
                log::debug!(">>> [SegmentedRecorder] Segment {} finalized: {} frames, {}ms", seg_idx, result.video_frames, result.duration_ms);

                journal.finalize_segment(
                    result.last_pts_ns,
                    result.file_size,
                )?;

                // Commit segment (atomic rename + fsync)
                let final_path = journal.commit_segment()?;
                log::debug!(">>> [SegmentedRecorder] Segment {} committed to {:?}", seg_idx, final_path);

                committed_segments.fetch_add(1, Ordering::Relaxed);
                *last_commit_time.lock() = Some(Instant::now());

                log::info!(
                    "[SegmentedRecorder] Committed segment {} ({} frames, {}ms)",
                    seg_idx,
                    segment_frames,
                    result.duration_ms
                );

                // Begin new segment
                let new_segment_idx = seg_idx + 1;
                current_segment.store(new_segment_idx, Ordering::Relaxed);
                *current_segment_start.lock() = Some(Instant::now());

                let tmp_path = journal.begin_segment(pts_ns)?;
                log::debug!(">>> [SegmentedRecorder] Started segment {} at {:?}", new_segment_idx, tmp_path);
                muxer = Fmp4SegmentMuxer::new(tmp_path, muxer_config.clone())?;
                segment_frames = 0;
                segment_audio_samples = 0;

                *state.lock() = SegmentedRecordingState::Recording;
            }

            // Wall-clock pacing: wait until it's time for the next frame
            let next_frame_time = recording_start + Duration::from_nanos(frame_duration_ns * total_frames);
            let now = Instant::now();
            if now < next_frame_time {
                // Sleep until next frame time (with 1ms granularity to avoid busy-spin)
                let wait = next_frame_time - now;
                if wait > Duration::from_millis(1) {
                    thread::sleep(wait - Duration::from_millis(1));
                }
                // Spin-wait for remaining sub-millisecond precision
                while Instant::now() < next_frame_time {
                    thread::yield_now();
                }
            }

            // Read latest video frame from ring (skip to most recent)
            if let Some((pixels, w, h, _frame_num)) = frame_ring.try_read() {
                if w == width && h == height {
                    // Write video frame at paced rate
                    if let Err(e) = muxer.write_video_frame(pixels, pts_ns) {
                        log::warn!("[SegmentedRecorder] Write error: {}", e);
                    } else {
                        video_frames_encoded.fetch_add(1, Ordering::Relaxed);
                        segment_frames += 1;
                        total_frames += 1;
                        pts_ns += frame_duration_ns;
                    }
                }
            } else {
                // No frame available — render is behind, wait and retry
                thread::sleep(Duration::from_millis(1));
                continue;
            }

            // Write accumulated audio
            {
                let mut buf = audio_buffer.lock();
                if !buf.is_empty() {
                    let samples: Vec<f32> = buf.drain(..).collect();
                    drop(buf); // Release lock before muxer write

                    if let Err(e) = muxer.write_audio(&samples) {
                        log::warn!("[SegmentedRecorder] Audio write error: {}", e);
                    } else {
                        let sample_count = samples.len() as u64 / 2; // Stereo
                        audio_samples_encoded.fetch_add(sample_count, Ordering::Relaxed);
                        segment_audio_samples += sample_count;
                    }
                }
            }

            // Update journal progress
            journal.update_segment_progress(segment_frames, segment_audio_samples);
        }

        // Finalize last segment
        log::debug!(">>> [SegmentedRecorder] Finalizing last segment (index={})...", current_segment.load(Ordering::Relaxed));
        log::info!("[SegmentedRecorder] Finalizing last segment...");
        *state.lock() = SegmentedRecordingState::FinalizingSegment;

        // Drain remaining audio
        {
            let mut buf = audio_buffer.lock();
            if !buf.is_empty() {
                let samples: Vec<f32> = buf.drain(..).collect();
                drop(buf);
                let _ = muxer.write_audio(&samples);
            }
        }

        log::debug!(">>> [SegmentedRecorder] Calling muxer.finalize()...");
        let result = muxer.finalize()?;
        log::debug!(">>> [SegmentedRecorder] Muxer finalized: {}ms, {} frames, {} bytes", result.duration_ms, result.video_frames, result.file_size);

        journal.finalize_segment(result.last_pts_ns, result.file_size)?;
        log::debug!(">>> [SegmentedRecorder] Journal finalized segment");

        journal.commit_segment()?;
        log::debug!(">>> [SegmentedRecorder] Journal committed segment");

        committed_segments.fetch_add(1, Ordering::Relaxed);
        *last_commit_time.lock() = Some(Instant::now());

        // Mark session as stopped
        log::debug!(">>> [SegmentedRecorder] Calling journal.stop()...");
        journal.stop()?;
        log::debug!(">>> [SegmentedRecorder] Journal stopped");

        log::info!(
            "[SegmentedRecorder] Recording complete: {} segments",
            committed_segments.load(Ordering::Relaxed)
        );

        Ok(())
    }
}

impl Drop for SegmentedRecorder {
    fn drop(&mut self) {
        if self.is_recording() {
            let _ = self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recording_state_default() {
        assert_eq!(SegmentedRecordingState::default(), SegmentedRecordingState::Idle);
    }

    #[test]
    fn test_metrics_default() {
        let metrics = SegmentedRecordingMetrics::default();
        assert_eq!(metrics.state, SegmentedRecordingState::Idle);
        assert_eq!(metrics.current_segment, 0);
        assert_eq!(metrics.committed_segments, 0);
    }
}
