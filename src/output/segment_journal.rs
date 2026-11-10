//! Segment Journal - Two-Phase Commit for Crash-Resilient Recording
//!
//! Implements a write-ahead journal pattern to ensure segment durability:
//! 1. Begin segment: Record intent to create segment in journal BEFORE any writes
//! 2. Finalize segment: Mark segment as written with metadata
//! 3. Commit segment: Atomic rename + fsync + journal update = segment is safe
//!
//! On crash, segments with status='committed' are guaranteed recoverable.

use anyhow::{Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::db::{CommittedSegment, CrashedSession, Database, RecordingSessionConfig, SegmentMetadata};

/// Segment journal for crash-resilient recording
pub struct SegmentJournal {
    /// Database connection for journal persistence
    db: Database,
    /// Unique session ID
    session_id: String,
    /// Output directory for segment files
    output_dir: PathBuf,
    /// Target segment duration
    segment_duration_target_ms: u64,
    /// Current active segment
    current_segment: Option<ActiveSegment>,
    /// Next segment index (persists across commit cycles)
    next_segment_index: u32,
    /// Recording start time (for session-relative timestamps)
    start_time: Instant,
    /// Video dimensions
    width: u32,
    height: u32,
    /// Frame rate
    fps: u32,
}

/// An in-progress segment being written
struct ActiveSegment {
    /// Segment index (0-based)
    index: u32,
    /// Temporary file path (before atomic rename)
    tmp_path: PathBuf,
    /// Final file path (after atomic rename)
    final_path: PathBuf,
    /// First PTS in this segment (nanoseconds)
    first_pts_ns: u64,
    /// Video frames written to this segment
    video_frames: u64,
    /// Audio samples written to this segment
    audio_samples: u64,
    /// Segment start time
    start_time: Instant,
}

/// Recovery information for a crashed session
pub struct RecoveryInfo {
    /// Session ID
    pub session_id: String,
    /// Output directory
    pub output_dir: PathBuf,
    /// Committed segments (safe to use)
    pub committed_segments: Vec<CommittedSegment>,
    /// Last committed segment index
    pub last_committed_index: Option<u32>,
    /// Total recovered duration in milliseconds
    pub recovered_duration_ms: u64,
    /// Maximum potential loss (segment duration)
    pub max_loss_ms: u64,
    /// Video dimensions
    pub width: u32,
    pub height: u32,
    /// Frame rate
    pub fps: u32,
}

impl SegmentJournal {
    /// Create a new segment journal for a recording session
    pub fn new(
        db: Database,
        output_dir: PathBuf,
        config: &RecorderConfig,
    ) -> Result<Self> {
        // Generate unique session ID with timestamp
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let session_id = format!("rec_{}_{}", timestamp, std::process::id());

        // Create session-specific subdirectory to avoid overwriting previous sessions
        let session_dir = output_dir.join(format!("react_{}", timestamp));
        std::fs::create_dir_all(&session_dir)
            .with_context(|| format!("Failed to create session directory: {:?}", session_dir))?;

        // Create recording session in database
        let db_config = RecordingSessionConfig {
            segment_duration_ms: config.segment_duration_ms,
            width: config.width,
            height: config.height,
            fps: config.fps,
        };
        db.create_recording_session(&session_id, session_dir.to_str().unwrap_or(""), &db_config)?;

        log::info!(
            "[Journal] Created session {} in {:?} ({}x{} @ {}fps, {}ms segments)",
            session_id,
            session_dir,
            config.width,
            config.height,
            config.fps,
            config.segment_duration_ms
        );

        Ok(Self {
            db,
            session_id,
            output_dir: session_dir,
            segment_duration_target_ms: config.segment_duration_ms,
            current_segment: None,
            next_segment_index: 0,
            start_time: Instant::now(),
            width: config.width,
            height: config.height,
            fps: config.fps,
        })
    }

    /// Get session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get output directory
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Get current segment index
    pub fn current_segment_index(&self) -> Option<u32> {
        self.current_segment.as_ref().map(|s| s.index)
    }

    /// Check if we have an active segment
    pub fn has_active_segment(&self) -> bool {
        self.current_segment.is_some()
    }

    /// Begin a new segment (Phase 1 of two-phase commit)
    ///
    /// Called on keyframe boundaries. Creates temp file path and records
    /// BeginSegment in journal BEFORE any writes.
    pub fn begin_segment(&mut self, first_pts_ns: u64) -> Result<PathBuf> {
        // Use tracked segment index (persists across commit cycles)
        let index = self.next_segment_index;
        self.next_segment_index += 1;

        // Generate file paths
        let tmp_path = self.output_dir.join(format!("seg_{:05}.tmp.mp4", index));
        let final_path = self.output_dir.join(format!("seg_{:05}.mp4", index));

        // Record in journal BEFORE any writes (write-ahead)
        self.db.begin_segment(&self.session_id, index, tmp_path.to_str().unwrap_or(""), first_pts_ns)?;

        log::info!(
            "[Journal] Begin segment {} at PTS {}ns -> {:?}",
            index,
            first_pts_ns,
            tmp_path
        );

        // Track active segment
        self.current_segment = Some(ActiveSegment {
            index,
            tmp_path: tmp_path.clone(),
            final_path,
            first_pts_ns,
            video_frames: 0,
            audio_samples: 0,
            start_time: Instant::now(),
        });

        Ok(tmp_path)
    }

    /// Update segment frame/sample counts (for progress tracking)
    pub fn update_segment_progress(&mut self, video_frames: u64, audio_samples: u64) {
        if let Some(ref mut seg) = self.current_segment {
            seg.video_frames = video_frames;
            seg.audio_samples = audio_samples;
        }
    }

    /// Get current segment duration estimate in milliseconds
    pub fn current_segment_duration_ms(&self) -> u64 {
        self.current_segment
            .as_ref()
            .map(|s| s.start_time.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    /// Check if current segment has reached target duration
    pub fn should_rotate_segment(&self) -> bool {
        self.current_segment_duration_ms() >= self.segment_duration_target_ms
    }

    /// Finalize current segment (Phase 2a of two-phase commit)
    ///
    /// Called after muxer closes the segment file. Records metadata in journal.
    pub fn finalize_segment(&mut self, last_pts_ns: u64, file_size: u64) -> Result<()> {
        let seg = self.current_segment.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active segment to finalize"))?;

        let duration_ms = seg.start_time.elapsed().as_millis() as u64;

        let metadata = SegmentMetadata {
            duration_ms,
            first_pts_ns: seg.first_pts_ns,
            last_pts_ns,
            file_size_bytes: file_size,
            video_frames: seg.video_frames,
            audio_samples: seg.audio_samples,
        };

        self.db.finalize_segment(&self.session_id, seg.index, &metadata)?;

        log::info!(
            "[Journal] Finalized segment {} ({}ms, {} frames, {} bytes)",
            seg.index,
            duration_ms,
            seg.video_frames,
            file_size
        );

        Ok(())
    }

    /// Commit current segment (Phase 2b of two-phase commit)
    ///
    /// Performs atomic rename with fsync, then records CommitSegment in journal.
    /// Only after this call is the segment guaranteed recoverable.
    pub fn commit_segment(&mut self) -> Result<PathBuf> {
        let seg = self.current_segment.take()
            .ok_or_else(|| anyhow::anyhow!("No active segment to commit"))?;

        // Perform atomic rename with fsync
        atomic_commit(&seg.tmp_path, &seg.final_path)?;

        // Record commit in journal
        self.db.commit_segment(&self.session_id, seg.index, seg.final_path.to_str().unwrap_or(""))?;

        log::info!(
            "[Journal] Committed segment {} -> {:?}",
            seg.index,
            seg.final_path
        );

        Ok(seg.final_path)
    }

    /// Abort current segment (if recording stops mid-segment)
    pub fn abort_current_segment(&mut self) -> Result<()> {
        if let Some(seg) = self.current_segment.take() {
            // Try to remove temp file
            let _ = std::fs::remove_file(&seg.tmp_path);
            log::info!("[Journal] Aborted segment {}", seg.index);
        }
        Ok(())
    }

    /// Stop recording and mark session as stopped
    pub fn stop(&mut self) -> Result<()> {
        // Abort any in-progress segment
        self.abort_current_segment()?;

        // Mark session as stopped
        self.db.mark_session_status(&self.session_id, "stopped")?;

        log::info!("[Journal] Stopped session {}", self.session_id);
        Ok(())
    }

    /// Get committed segments for this session
    pub fn get_committed_segments(&self) -> Vec<CommittedSegment> {
        self.db.get_committed_segments(&self.session_id)
    }

    /// Get total committed duration in milliseconds
    pub fn total_committed_duration_ms(&self) -> u64 {
        self.get_committed_segments()
            .iter()
            .map(|s| s.duration_ms)
            .sum()
    }

    /// Get count of committed segments
    pub fn committed_segment_count(&self) -> usize {
        self.get_committed_segments().len()
    }

    /// Recover crashed sessions
    ///
    /// Called on app startup to find any sessions that crashed mid-recording.
    pub fn recover(db: &Database) -> Vec<RecoveryInfo> {
        let crashed = db.get_crashed_sessions();

        crashed
            .into_iter()
            .filter_map(|session| {
                if session.committed_segments.is_empty() {
                    // No committed segments, nothing to recover
                    let _ = db.delete_recording_session(&session.session_id);
                    return None;
                }

                let last_committed_index = session
                    .committed_segments
                    .iter()
                    .map(|s| s.index)
                    .max();

                let recovered_duration_ms = session
                    .committed_segments
                    .iter()
                    .map(|s| s.duration_ms)
                    .sum();

                Some(RecoveryInfo {
                    session_id: session.session_id,
                    output_dir: session.output_dir,
                    committed_segments: session.committed_segments,
                    last_committed_index,
                    recovered_duration_ms,
                    max_loss_ms: session.segment_duration_ms,
                    width: session.width,
                    height: session.height,
                    fps: session.fps,
                })
            })
            .collect()
    }

    /// Mark a recovered session as recovered
    pub fn mark_recovered(db: &Database, session_id: &str) -> Result<()> {
        db.mark_session_status(session_id, "recovered")?;
        // Clean up abandoned segments
        db.cleanup_abandoned_segments(session_id)?;
        Ok(())
    }

    /// Discard a recovered session (delete segments and journal entries)
    pub fn discard_session(db: &Database, session_id: &str, output_dir: &Path) -> Result<()> {
        // Get segments before deleting from DB
        let segments = db.get_committed_segments(session_id);

        // Delete segment files
        for seg in segments {
            if seg.path.exists() {
                let _ = std::fs::remove_file(&seg.path);
            }
        }

        // Delete any .tmp files
        if let Ok(entries) = std::fs::read_dir(output_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("seg_") && name.ends_with(".tmp.mp4") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }

        // Delete from database
        db.delete_recording_session(session_id)?;

        log::info!("[Journal] Discarded session {}", session_id);
        Ok(())
    }
}

/// Recorder configuration (shared with main recorder)
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub output_dir: PathBuf,
    pub segment_duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_bitrate_kbps: u32,
    pub audio_sample_rate: u32,
    pub audio_bitrate_kbps: u32,
    /// Audio volume boost for FFmpeg mux (1.0 = unity; writer thread handles gain via three-stage model)
    pub audio_volume_boost: f32,
    /// Path to watermark logo PNG (None = no watermark)
    pub watermark_logo_path: Option<PathBuf>,
    /// Metadata comment to embed in MP4 segments (encrypted account fingerprint)
    pub metadata_comment: Option<String>,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::new(),
            segment_duration_ms: 60_000, // 60 second segments
            width: 1920,
            height: 1080,
            fps: 60,
            video_bitrate_kbps: 20_000, // 20 Mbps
            audio_sample_rate: 48_000,
            audio_bitrate_kbps: 192,
            audio_volume_boost: 1.0,
            watermark_logo_path: None,
            metadata_comment: None,
        }
    }
}

/// Atomic rename with fsync for crash safety
///
/// Ensures:
/// 1. Temp file is synced to disk
/// 2. Atomic rename (metadata operation)
/// 3. Directory is synced (ensures rename is durable)
fn atomic_commit(tmp_path: &Path, final_path: &Path) -> Result<()> {
    // 1. Sync the temp file to ensure all data is on disk
    {
        let file = File::open(tmp_path)
            .with_context(|| format!("Failed to open temp file for sync: {:?}", tmp_path))?;
        file.sync_all()
            .with_context(|| format!("Failed to sync temp file: {:?}", tmp_path))?;
    }

    // 2. Atomic rename
    std::fs::rename(tmp_path, final_path)
        .with_context(|| format!("Failed to rename {:?} -> {:?}", tmp_path, final_path))?;

    // 3. Sync the directory (critical for crash safety)
    if let Some(dir) = final_path.parent() {
        let dir_file = File::open(dir)
            .with_context(|| format!("Failed to open directory for sync: {:?}", dir))?;
        dir_file
            .sync_all()
            .with_context(|| format!("Failed to sync directory: {:?}", dir))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_commit_path_generation() {
        let output_dir = PathBuf::from("/tmp/test_recording");
        let index = 42;
        let tmp_path = output_dir.join(format!("seg_{:05}.tmp.mp4", index));
        let final_path = output_dir.join(format!("seg_{:05}.mp4", index));

        assert_eq!(tmp_path.file_name().unwrap().to_str().unwrap(), "seg_00042.tmp.mp4");
        assert_eq!(final_path.file_name().unwrap().to_str().unwrap(), "seg_00042.mp4");
    }
}
