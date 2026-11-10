//! Segment Stitcher - Concatenate Segments into Single File
//!
//! Provides utilities for combining recorded segments into a single
//! playable MP4 file using FFmpeg's concat demuxer.
//!
//! Also supports creating M3U8 playlists for immediate playback
//! without re-encoding.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use super::rtmp_streamer::find_ffmpeg;
use crate::db::CommittedSegment;

/// Result of stitching operation
#[derive(Debug, Clone)]
pub struct StitchResult {
    /// Output file path
    pub output_path: PathBuf,
    /// Total duration in milliseconds
    pub total_duration_ms: u64,
    /// Number of segments stitched
    pub segment_count: usize,
    /// Total file size in bytes
    pub file_size: u64,
}

/// Stitch segments into a single MP4 file
///
/// Uses FFmpeg's concat demuxer with stream copy (no re-encoding).
/// All segments are independently-playable MP4s with identical codec parameters,
/// so bitstream concatenation is lossless and fast.
///
/// If `metadata_comment` is provided, it's embedded in the output MP4's comment field
/// (encrypted account fingerprint for forensic tracing).
pub fn stitch_segments(segments: &[CommittedSegment], output_path: &Path) -> Result<StitchResult> {
    stitch_segments_with_metadata(segments, output_path, None)
}

/// Stitch segments with optional metadata comment embedded in the output MP4.
pub fn stitch_segments_with_metadata(segments: &[CommittedSegment], output_path: &Path, metadata_comment: Option<&str>) -> Result<StitchResult> {
    if segments.is_empty() {
        anyhow::bail!("No segments to stitch");
    }

    log::info!(
        "[Stitcher] Stitching {} segments to {:?} (stream copy)",
        segments.len(),
        output_path
    );

    let ffmpeg_path = find_ffmpeg();

    // Write concat file list (required by concat demuxer)
    let concat_list_path = output_path.with_extension("concat.txt");
    {
        let mut list_file = File::create(&concat_list_path)
            .with_context(|| format!("Failed to create concat list: {:?}", concat_list_path))?;
        for seg in segments {
            let abs_path = seg.path.canonicalize().unwrap_or_else(|_| seg.path.clone());
            // Escape single quotes in path for FFmpeg concat format
            let path_str = abs_path.display().to_string().replace('\'', "'\\''");
            writeln!(list_file, "file '{}'", path_str)?;
        }
        list_file.flush()?;
    }

    // Use concat demuxer with stream copy — no re-encoding, preserves original quality
    let mut cmd = Command::new(&ffmpeg_path);
    cmd.arg("-y")
        .args(["-f", "concat"])
        .args(["-safe", "0"])
        .arg("-i").arg(&concat_list_path)
        .args(["-c", "copy"]);

    // Embed metadata comment if provided (encrypted account fingerprint)
    if let Some(comment) = metadata_comment {
        cmd.arg("-metadata").arg(format!("comment={}", comment));
    }

    cmd.args(["-movflags", "+faststart"])
        .arg(output_path)
        .stderr(Stdio::piped());

    let output = cmd.output()
        .with_context(|| format!("Failed to run FFmpeg: {:?}", ffmpeg_path))?;

    // Clean up concat list
    let _ = std::fs::remove_file(&concat_list_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("FFmpeg concat failed: {}", stderr);
    }

    // Get file size
    let file_size = std::fs::metadata(output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    let total_duration_ms: u64 = segments.iter().map(|s| s.duration_ms).sum();

    log::info!(
        "[Stitcher] Stitched {} segments ({}ms, {} bytes, stream copy)",
        segments.len(),
        total_duration_ms,
        file_size
    );

    Ok(StitchResult {
        output_path: output_path.to_owned(),
        total_duration_ms,
        segment_count: segments.len(),
        file_size,
    })
}

/// Create an M3U8 playlist for immediate playback without stitching
///
/// This allows playback via HLS-compatible players without concatenation.
pub fn create_playlist(segments: &[CommittedSegment], output_path: &Path) -> Result<()> {
    if segments.is_empty() {
        anyhow::bail!("No segments for playlist");
    }

    log::info!(
        "[Stitcher] Creating playlist with {} segments at {:?}",
        segments.len(),
        output_path
    );

    let mut file = File::create(output_path)
        .with_context(|| format!("Failed to create playlist: {:?}", output_path))?;

    // Calculate target duration (max segment duration, rounded up)
    let max_duration_secs = segments
        .iter()
        .map(|s| (s.duration_ms + 999) / 1000)
        .max()
        .unwrap_or(10);

    // Write M3U8 header
    writeln!(file, "#EXTM3U")?;
    writeln!(file, "#EXT-X-VERSION:3")?;
    writeln!(file, "#EXT-X-TARGETDURATION:{}", max_duration_secs)?;
    writeln!(file, "#EXT-X-MEDIA-SEQUENCE:0")?;
    writeln!(file, "#EXT-X-PLAYLIST-TYPE:VOD")?;
    writeln!(file)?;

    // Write segments
    for seg in segments {
        let duration_secs = seg.duration_ms as f64 / 1000.0;
        writeln!(file, "#EXTINF:{:.3},", duration_secs)?;

        // Use relative path if in same directory, otherwise absolute
        let playlist_dir = output_path.parent();
        let seg_dir = seg.path.parent();

        let display_path = if playlist_dir == seg_dir {
            seg.path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| seg.path.display().to_string())
        } else {
            seg.path.display().to_string()
        };

        writeln!(file, "{}", display_path)?;
    }

    // End of playlist
    writeln!(file, "#EXT-X-ENDLIST")?;

    file.sync_all()?;

    log::info!("[Stitcher] Created playlist: {:?}", output_path);
    Ok(())
}

/// Generate default output filename for stitched recording
pub fn default_stitch_filename(session_id: &str) -> String {
    // Extract timestamp from session_id if present
    // session_id format: "rec_YYYYMMDD_HHMMSS_PID"
    if session_id.starts_with("rec_") && session_id.len() > 18 {
        let timestamp = &session_id[4..19]; // "YYYYMMDD_HHMMSS"
        format!("react_{}.mp4", timestamp)
    } else {
        format!("react_{}.mp4", chrono::Utc::now().format("%Y%m%d_%H%M%S"))
    }
}

/// Suggest output path for stitched file
pub fn suggest_stitch_output(output_dir: &Path, session_id: &str) -> PathBuf {
    let filename = default_stitch_filename(session_id);
    output_dir.join(filename)
}

/// Clean up segment files after successful stitch
pub fn cleanup_segments(segments: &[CommittedSegment]) -> Result<usize> {
    let mut removed = 0;
    for seg in segments {
        if seg.path.exists() {
            if let Err(e) = std::fs::remove_file(&seg.path) {
                log::warn!("[Stitcher] Failed to remove segment {:?}: {}", seg.path, e);
            } else {
                removed += 1;
            }
        }
    }
    log::info!("[Stitcher] Cleaned up {} segment files", removed);
    Ok(removed)
}

/// Verify all segments exist and are readable
pub fn verify_segments(segments: &[CommittedSegment]) -> Result<Vec<String>> {
    let mut errors = Vec::new();

    for seg in segments {
        if !seg.path.exists() {
            errors.push(format!("Missing: {:?}", seg.path));
            continue;
        }

        match std::fs::metadata(&seg.path) {
            Ok(meta) => {
                if meta.len() == 0 {
                    errors.push(format!("Empty file: {:?}", seg.path));
                } else if meta.len() != seg.file_size_bytes {
                    errors.push(format!(
                        "Size mismatch for {:?}: expected {} bytes, got {}",
                        seg.path,
                        seg.file_size_bytes,
                        meta.len()
                    ));
                }
            }
            Err(e) => {
                errors.push(format!("Cannot read {:?}: {}", seg.path, e));
            }
        }
    }

    if errors.is_empty() {
        log::info!("[Stitcher] All {} segments verified", segments.len());
    } else {
        log::warn!("[Stitcher] {} segment verification errors", errors.len());
    }

    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_stitch_filename() {
        let filename = default_stitch_filename("rec_20260115_143022_12345");
        assert_eq!(filename, "react_20260115_143022.mp4");
    }

    #[test]
    fn test_default_stitch_filename_fallback() {
        let filename = default_stitch_filename("invalid_session");
        assert!(filename.starts_with("react_"));
        assert!(filename.ends_with(".mp4"));
    }

    #[test]
    fn test_suggest_stitch_output() {
        let output_dir = PathBuf::from("/tmp/recordings");
        let path = suggest_stitch_output(&output_dir, "rec_20260115_143022_12345");
        assert_eq!(path, PathBuf::from("/tmp/recordings/react_20260115_143022.mp4"));
    }
}
