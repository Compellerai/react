//! Fragmented MP4 Segment Muxer
//!
//! Creates independently-playable MP4 segments using FFmpeg.
//! Each segment is a complete MP4 file with its own moov atom,
//! allowing playback without requiring other segments.
//!
//! Uses fragmented MP4 (fMP4) format for crash resilience:
//! - moov atom at start for immediate playability
//! - moof+mdat fragments for incremental data
//! - faststart flag ensures playable even if writing interrupted

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;

use super::rtmp_streamer::find_ffmpeg;

/// Muxer configuration
#[derive(Debug, Clone)]
pub struct MuxerConfig {
    /// Video width
    pub width: u32,
    /// Video height
    pub height: u32,
    /// Frame rate
    pub fps: u32,
    /// Video bitrate in kbps
    pub video_bitrate_kbps: u32,
    /// Audio sample rate
    pub audio_sample_rate: u32,
    /// Audio bitrate in kbps
    pub audio_bitrate_kbps: u32,
    /// Use hardware encoder (VideoToolbox on macOS)
    pub use_hardware_encoder: bool,
    /// Audio volume multiplier applied during mux (1.0 = unity, >1.0 = boost).
    /// System audio sources need boost (~15x) since ScreenCaptureKit delivers low-level signals.
    /// Line-level input devices (DJ mixers) should use 1.0.
    pub audio_volume_boost: f32,
    /// Path to watermark logo PNG. When set, overlays at 20% opacity in bottom-right.
    /// Driven by entitlements.watermark (FREE/anon users get watermark, paid tiers don't).
    pub watermark_logo_path: Option<PathBuf>,
    /// Metadata comment to embed in MP4 (encrypted account fingerprint).
    pub metadata_comment: Option<String>,
}

impl Default for MuxerConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            video_bitrate_kbps: 20_000,
            audio_sample_rate: 48_000,
            audio_bitrate_kbps: 192,
            use_hardware_encoder: true,
            audio_volume_boost: 1.0,
            watermark_logo_path: None,
            metadata_comment: None,
        }
    }
}

/// Metadata for a completed segment
#[derive(Debug, Clone)]
pub struct SegmentResult {
    /// Segment duration in milliseconds
    pub duration_ms: u64,
    /// First PTS in nanoseconds
    pub first_pts_ns: u64,
    /// Last PTS in nanoseconds
    pub last_pts_ns: u64,
    /// File size in bytes
    pub file_size: u64,
    /// Number of video frames
    pub video_frames: u64,
    /// Number of audio samples
    pub audio_samples: u64,
}

/// FFmpeg-based segment muxer
///
/// Creates one MP4 segment file using FFmpeg subprocess.
/// Video is piped via stdin, audio is accumulated and muxed on finalize.
pub struct Fmp4SegmentMuxer {
    /// Output file path
    output_path: PathBuf,
    /// Muxer configuration
    config: MuxerConfig,
    /// FFmpeg video process
    ffmpeg_video: Option<Child>,
    /// Video stdin pipe
    video_stdin: Option<std::process::ChildStdin>,
    /// Audio temp file
    audio_temp_path: PathBuf,
    /// Audio writer
    audio_writer: Option<BufWriter<File>>,
    /// First PTS in this segment
    first_pts_ns: Option<u64>,
    /// Last PTS in this segment
    last_pts_ns: u64,
    /// Video frames written
    video_frames: AtomicU64,
    /// Audio samples written
    audio_samples: AtomicU64,
    /// Whether muxer is active
    active: AtomicBool,
    /// Start time for duration tracking
    start_time: std::time::Instant,
}

impl Fmp4SegmentMuxer {
    /// Create a new segment muxer
    pub fn new(output_path: PathBuf, config: MuxerConfig) -> Result<Self> {
        // Create temp path for video-only output
        let video_temp_path = output_path.with_extension("video.tmp.mp4");
        let audio_temp_path = output_path.with_extension("audio.raw");

        // Open audio temp file
        let audio_file = File::create(&audio_temp_path)
            .with_context(|| format!("Failed to create audio temp file: {:?}", audio_temp_path))?;
        let audio_writer = BufWriter::new(audio_file);

        // Select encoder
        #[cfg(target_os = "macos")]
        let encoder = if config.use_hardware_encoder {
            "h264_videotoolbox"
        } else {
            "libx264"
        };
        #[cfg(not(target_os = "macos"))]
        let encoder = if config.use_hardware_encoder {
            "h264_nvenc"
        } else {
            "libx264"
        };

        // Start FFmpeg for video encoding
        let ffmpeg_path = find_ffmpeg();
        let mut cmd = Command::new(&ffmpeg_path);
        cmd.arg("-y")
            // Input 0: raw RGBA video from pipe
            .arg("-f").arg("rawvideo")
            .arg("-pixel_format").arg("rgba")
            .arg("-video_size").arg(format!("{}x{}", config.width, config.height))
            .arg("-framerate").arg(format!("{}", config.fps))
            .arg("-i").arg("pipe:0");

        // If watermark enabled, add logo as second input with overlay filter
        if let Some(ref logo_path) = config.watermark_logo_path {
            cmd.arg("-i").arg(logo_path)
                .arg("-filter_complex")
                .arg("[1:v]format=rgba,colorchannelmixer=aa=0.20[logo];[0:v][logo]overlay=W-w-20:H-h-20");
        }

        // Video encoding
        cmd.arg("-c:v").arg(encoder)
            .arg("-b:v").arg(format!("{}k", config.video_bitrate_kbps))
            .arg("-pix_fmt").arg("yuv420p")
            // Force keyframe at start for segment independence
            .arg("-force_key_frames").arg("expr:eq(n,0)");

        // Embed metadata comment if provided
        if let Some(ref comment) = config.metadata_comment {
            cmd.arg("-metadata").arg(format!("comment={}", comment));
        }

        // MP4 output with faststart for playability
        let mut ffmpeg = cmd
            .arg("-movflags").arg("+faststart+frag_keyframe")
            .arg(&video_temp_path)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn FFmpeg: {:?}", ffmpeg_path))?;

        let video_stdin = ffmpeg.stdin.take();

        log::info!(
            "[Muxer] Created segment muxer for {:?} ({}x{} @ {}fps)",
            output_path,
            config.width,
            config.height,
            config.fps
        );

        Ok(Self {
            output_path,
            config,
            ffmpeg_video: Some(ffmpeg),
            video_stdin,
            audio_temp_path,
            audio_writer: Some(audio_writer),
            first_pts_ns: None,
            last_pts_ns: 0,
            video_frames: AtomicU64::new(0),
            audio_samples: AtomicU64::new(0),
            active: AtomicBool::new(true),
            start_time: std::time::Instant::now(),
        })
    }

    /// Get output path
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    /// Write a video frame (RGBA pixels, Y-flipped)
    pub fn write_video_frame(&mut self, rgba_data: &[u8], pts_ns: u64) -> Result<()> {
        if !self.active.load(Ordering::Acquire) {
            anyhow::bail!("Muxer is not active");
        }

        // Track first/last PTS
        if self.first_pts_ns.is_none() {
            self.first_pts_ns = Some(pts_ns);
        }
        self.last_pts_ns = pts_ns;

        // Write to FFmpeg stdin
        if let Some(ref mut stdin) = self.video_stdin {
            // Flip Y axis (OpenGL to video convention)
            let width = self.config.width as usize;
            let height = self.config.height as usize;
            let stride = width * 4;

            let mut flipped = vec![0u8; rgba_data.len()];
            for y in 0..height {
                let src_y = height - 1 - y;
                let src_off = src_y * stride;
                let dst_off = y * stride;
                if src_off + stride <= rgba_data.len() {
                    flipped[dst_off..dst_off + stride]
                        .copy_from_slice(&rgba_data[src_off..src_off + stride]);
                }
            }

            stdin.write_all(&flipped)
                .context("Failed to write video frame to FFmpeg")?;

            self.video_frames.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Write audio samples (f32 interleaved stereo)
    pub fn write_audio(&mut self, samples: &[f32]) -> Result<()> {
        if !self.active.load(Ordering::Acquire) {
            anyhow::bail!("Muxer is not active");
        }

        if let Some(ref mut writer) = self.audio_writer {
            // Write raw f32 samples
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    samples.as_ptr() as *const u8,
                    samples.len() * std::mem::size_of::<f32>(),
                )
            };
            writer.write_all(bytes)
                .context("Failed to write audio samples")?;

            self.audio_samples.fetch_add(samples.len() as u64 / 2, Ordering::Relaxed); // Stereo = /2
        }

        Ok(())
    }

    /// Get current video frame count
    pub fn video_frames(&self) -> u64 {
        self.video_frames.load(Ordering::Relaxed)
    }

    /// Get current audio sample count
    pub fn audio_samples(&self) -> u64 {
        self.audio_samples.load(Ordering::Relaxed)
    }

    /// Finalize segment (close files, mux audio, return metadata)
    pub fn finalize(mut self) -> Result<SegmentResult> {
        self.active.store(false, Ordering::Release);

        // Close video stdin
        drop(self.video_stdin.take());

        // Flush and close audio writer
        if let Some(mut writer) = self.audio_writer.take() {
            writer.flush()?;
        }

        // Wait for video FFmpeg to finish
        let video_temp_path = self.output_path.with_extension("video.tmp.mp4");
        if let Some(ffmpeg) = self.ffmpeg_video.take() {
            let output = ffmpeg.wait_with_output()
                .context("Failed to wait for FFmpeg")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                log::error!("[Muxer] FFmpeg video error: {}", stderr);
                // Continue anyway - we may have partial video
            }
        }

        let video_frames = self.video_frames.load(Ordering::Relaxed);
        let audio_samples = self.audio_samples.load(Ordering::Relaxed);
        let duration_ms = self.start_time.elapsed().as_millis() as u64;

        // Mux audio with video
        if audio_samples > 0 && video_temp_path.exists() {
            log::info!("[Muxer] Muxing audio into segment...");

            let ffmpeg_path = find_ffmpeg();
            let mut cmd = Command::new(&ffmpeg_path);
            cmd.arg("-y")
                // Video input
                .arg("-i").arg(&video_temp_path)
                // Audio input (raw f32 stereo)
                .arg("-f").arg("f32le")
                .arg("-ar").arg(format!("{}", self.config.audio_sample_rate))
                .arg("-ac").arg("2")
                .arg("-i").arg(&self.audio_temp_path)
                // Copy video, encode audio
                .arg("-c:v").arg("copy")
                .arg("-c:a").arg("aac")
                .arg("-b:a").arg(format!("{}k", self.config.audio_bitrate_kbps));

            // Apply volume boost only for quiet sources (system audio).
            // Line-level inputs (DJ mixers) use 1.0 — no boost needed.
            if self.config.audio_volume_boost > 1.01 {
                cmd.arg("-af").arg(format!("volume={}", self.config.audio_volume_boost as i32));
            }

            let mux_output = cmd
                .arg("-shortest")
                .arg("-movflags").arg("+faststart")
                .arg(&self.output_path)
                .stderr(Stdio::piped())
                .output()
                .context("Failed to run FFmpeg mux")?;

            if !mux_output.status.success() {
                let stderr = String::from_utf8_lossy(&mux_output.stderr);
                log::error!("[Muxer] FFmpeg mux error: {}", stderr);
                // Fall back to video-only
                std::fs::rename(&video_temp_path, &self.output_path)?;
            } else {
                // Clean up video temp
                let _ = std::fs::remove_file(&video_temp_path);
            }
        } else if video_temp_path.exists() {
            // No audio, just rename video temp to final
            std::fs::rename(&video_temp_path, &self.output_path)?;
        }

        // Clean up audio temp
        let _ = std::fs::remove_file(&self.audio_temp_path);

        // Get final file size
        let file_size = std::fs::metadata(&self.output_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let result = SegmentResult {
            duration_ms,
            first_pts_ns: self.first_pts_ns.unwrap_or(0),
            last_pts_ns: self.last_pts_ns,
            file_size,
            video_frames,
            audio_samples,
        };

        log::info!(
            "[Muxer] Finalized segment: {:?} ({}ms, {} frames, {} bytes)",
            self.output_path,
            duration_ms,
            video_frames,
            file_size
        );

        Ok(result)
    }

    /// Abort segment (discard without finalizing)
    pub fn abort(mut self) {
        self.active.store(false, Ordering::Release);

        // Close pipes to terminate FFmpeg
        drop(self.video_stdin.take());
        drop(self.audio_writer.take());

        // Kill FFmpeg if still running
        if let Some(mut ffmpeg) = self.ffmpeg_video.take() {
            let _ = ffmpeg.kill();
            let _ = ffmpeg.wait();
        }

        // Clean up temp files
        let video_temp_path = self.output_path.with_extension("video.tmp.mp4");
        let _ = std::fs::remove_file(&video_temp_path);
        let _ = std::fs::remove_file(&self.audio_temp_path);

        log::info!("[Muxer] Aborted segment: {:?}", self.output_path);
    }
}

impl Drop for Fmp4SegmentMuxer {
    fn drop(&mut self) {
        // Ensure FFmpeg is cleaned up
        if let Some(mut ffmpeg) = self.ffmpeg_video.take() {
            let _ = ffmpeg.kill();
            let _ = ffmpeg.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_muxer_config_default() {
        let config = MuxerConfig::default();
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.fps, 60);
        assert_eq!(config.video_bitrate_kbps, 20_000);
        assert_eq!(config.audio_sample_rate, 48_000);
        assert!(config.use_hardware_encoder);
    }

    #[test]
    fn test_segment_result() {
        let result = SegmentResult {
            duration_ms: 10_000,
            first_pts_ns: 0,
            last_pts_ns: 10_000_000_000,
            file_size: 25_000_000,
            video_frames: 600,
            audio_samples: 480_000,
        };

        assert_eq!(result.duration_ms, 10_000);
        assert_eq!(result.video_frames, 600);
    }
}
