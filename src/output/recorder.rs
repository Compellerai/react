//! MP4 Video/Audio Recorder
//!
//! Records video frames and audio to MP4 using ffmpeg.
//! Video is piped with wallclock timestamps, audio is muxed after recording stops.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use anyhow::Result;
use parking_lot::Mutex;

use super::FrameRing;

/// Recording state shared between UI and recorder thread
#[derive(Clone, Default)]
pub struct RecordingState {
    pub active: bool,
    pub duration_secs: f32,
    pub frames_recorded: u64,
    pub audio_samples: u64,
    pub output_path: Option<PathBuf>,
}

/// MP4 Recorder
pub struct Recorder {
    active: Arc<AtomicBool>,
    start_time: Arc<Mutex<Option<Instant>>>,
    frames_recorded: Arc<AtomicU64>,
    audio_samples: Arc<AtomicU64>,
    output_path: Arc<Mutex<Option<PathBuf>>>,
    frame_ring: Arc<FrameRing>,
    audio_buffer: Arc<Mutex<Vec<f32>>>,
    thread_handle: Option<JoinHandle<()>>,
    available: bool,
    /// Audio volume boost for ffmpeg muxing (unity: writer thread pre-applies gain)
    audio_boost: Arc<Mutex<f32>>,
    /// Path to watermark logo PNG (None = no watermark)
    watermark_logo_path: Option<PathBuf>,
    /// Metadata comment to embed in MP4 (encrypted account fingerprint)
    metadata_comment: Option<String>,
}

use super::rtmp_streamer::find_ffmpeg;

impl Recorder {
    pub fn new(frame_ring: Arc<FrameRing>) -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            start_time: Arc::new(Mutex::new(None)),
            frames_recorded: Arc::new(AtomicU64::new(0)),
            audio_samples: Arc::new(AtomicU64::new(0)),
            output_path: Arc::new(Mutex::new(None)),
            frame_ring,
            audio_buffer: Arc::new(Mutex::new(Vec::with_capacity(65536))),
            thread_handle: None,
            available: true,
            audio_boost: Arc::new(Mutex::new(1.0)), // Unity: writer thread pre-applies gain
            watermark_logo_path: None,
            metadata_comment: None,
        }
    }

    /// Set watermark logo path and metadata comment for recording fingerprinting
    pub fn set_fingerprint(&mut self, watermark_logo_path: Option<PathBuf>, metadata_comment: Option<String>) {
        self.watermark_logo_path = watermark_logo_path;
        self.metadata_comment = metadata_comment;
    }

    /// Set the audio volume boost for recording (e.g., 2.0 for input devices, 15.0 for SCK)
    pub fn set_audio_boost(&self, boost: f32) {
        *self.audio_boost.lock() = boost;
        log::info!("[Recorder] Audio boost set to {}", boost);
    }

    pub fn push_audio(&self, samples: &[f32]) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        self.audio_buffer.lock().extend_from_slice(samples);
    }

    /// Get Arc reference to the audio buffer (for direct sink dispatch from capture callback)
    pub fn get_audio_buffer_arc(&self) -> Arc<Mutex<Vec<f32>>> {
        self.audio_buffer.clone()
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    pub fn is_recording(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn get_state(&self) -> RecordingState {
        let duration_secs = self.start_time.lock()
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);

        RecordingState {
            active: self.active.load(Ordering::Acquire),
            duration_secs,
            frames_recorded: self.frames_recorded.load(Ordering::Relaxed),
            audio_samples: self.audio_samples.load(Ordering::Relaxed),
            output_path: self.output_path.lock().clone(),
        }
    }

    pub fn start(&mut self, output_path: PathBuf) -> Result<()> {
        if self.active.load(Ordering::Acquire) {
            anyhow::bail!("Recording already in progress");
        }

        log::info!("[Recorder] Starting recording to {:?}", output_path);

        self.frames_recorded.store(0, Ordering::Relaxed);
        self.audio_samples.store(0, Ordering::Relaxed);
        *self.output_path.lock() = Some(output_path.clone());
        *self.start_time.lock() = Some(Instant::now());
        self.audio_buffer.lock().clear();
        self.active.store(true, Ordering::Release);

        let active = self.active.clone();
        let frame_ring = self.frame_ring.clone();
        let audio_buffer = self.audio_buffer.clone();
        let frames_recorded = self.frames_recorded.clone();
        let audio_samples = self.audio_samples.clone();
        let audio_boost = *self.audio_boost.lock();
        let watermark_logo_path = self.watermark_logo_path.clone();
        let metadata_comment = self.metadata_comment.clone();

        let handle = thread::spawn(move || {
            if let Err(e) = Self::recording_thread(
                active, frame_ring, audio_buffer, output_path, frames_recorded, audio_samples,
                audio_boost, watermark_logo_path, metadata_comment,
            ) {
                log::error!("[Recorder] Recording error: {}", e);
            }
        });

        self.thread_handle = Some(handle);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if !self.active.load(Ordering::Acquire) {
            return Ok(());
        }

        log::info!("[Recorder] Stopping recording...");
        self.active.store(false, Ordering::Release);

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        let state = self.get_state();
        log::info!("[Recorder] Stopped: {} frames, {:.1}s",
            state.frames_recorded, state.duration_secs);

        Ok(())
    }

    fn recording_thread(
        active: Arc<AtomicBool>,
        frame_ring: Arc<FrameRing>,
        audio_buffer: Arc<Mutex<Vec<f32>>>,
        output_path: PathBuf,
        frames_recorded: Arc<AtomicU64>,
        audio_samples: Arc<AtomicU64>,
        audio_boost: f32,
        watermark_logo_path: Option<PathBuf>,
        metadata_comment: Option<String>,
    ) -> Result<()> {
        use std::process::{Command, Stdio};

        // Wait for dimensions
        let (width, height) = loop {
            if !active.load(Ordering::Acquire) {
                return Ok(());
            }
            let dims = frame_ring.dimensions();
            if dims.0 > 0 && dims.1 > 0 {
                break dims;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        };

        log::info!("[Recorder] Recording {}x{}", width, height);

        // Create temp paths for video-only and audio
        let video_temp = output_path.with_extension("video.mp4");
        let audio_temp = output_path.with_extension("audio.raw");

        // Open audio temp file for writing
        let audio_file = File::create(&audio_temp)?;
        let mut audio_writer = BufWriter::new(audio_file);

        // Use hardware encoding (VideoToolbox on macOS, nvenc on Windows/Linux)
        #[cfg(target_os = "macos")]
        let encoder = "h264_videotoolbox";
        #[cfg(target_os = "windows")]
        let encoder = "h264_nvenc";
        #[cfg(target_os = "linux")]
        let encoder = "h264_nvenc";

        let ffmpeg_path = find_ffmpeg();
        let mut cmd = Command::new(&ffmpeg_path);
        cmd.arg("-y")
            .arg("-use_wallclock_as_timestamps").arg("1")
            .arg("-f").arg("rawvideo")
            .arg("-pixel_format").arg("rgba")
            .arg("-video_size").arg(format!("{}x{}", width, height))
            .arg("-i").arg("pipe:0");

        // If watermark enabled, add logo as second input with overlay filter
        if let Some(ref logo_path) = watermark_logo_path {
            cmd.arg("-i").arg(logo_path)
                .arg("-filter_complex")
                .arg("[1:v]format=rgba,colorchannelmixer=aa=0.20[logo];[0:v][logo]overlay=W-w-20:H-h-20");
        }

        cmd.arg("-c:v").arg(encoder)
            .arg("-b:v").arg("20M")  // 20 Mbps bitrate
            .arg("-pix_fmt").arg("yuv420p");

        // Embed metadata comment if provided
        if let Some(ref comment) = metadata_comment {
            cmd.arg("-metadata").arg(format!("comment={}", comment));
        }

        let mut ffmpeg = cmd
            .arg("-movflags").arg("+faststart")
            .arg(&video_temp)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut stdin = ffmpeg.stdin.take().expect("stdin");
        let mut last_frame_num: u32 = 0;
        let mut frame_buffer: Vec<u8> = vec![0u8; (width * height * 4) as usize];
        let mut total_audio_samples: u64 = 0;

        log::info!("[Recorder] Piping frames to ffmpeg, audio to temp file");

        while active.load(Ordering::Acquire) {
            if let Some((pixels, w, h, frame_num)) = frame_ring.try_read() {
                if frame_num != last_frame_num && w == width && h == height {
                    last_frame_num = frame_num;

                    // Flip Y
                    let stride = (width * 4) as usize;
                    for y in 0..height as usize {
                        let src_y = height as usize - 1 - y;
                        let src_off = src_y * stride;
                        let dst_off = y * stride;
                        if src_off + stride <= pixels.len() {
                            frame_buffer[dst_off..dst_off + stride]
                                .copy_from_slice(&pixels[src_off..src_off + stride]);
                        }
                    }

                    if stdin.write_all(&frame_buffer).is_err() {
                        log::warn!("[Recorder] ffmpeg stdin closed");
                        break;
                    }
                    frames_recorded.fetch_add(1, Ordering::Relaxed);
                }
            }

            // Write audio samples to temp file
            {
                let mut buf = audio_buffer.lock();
                if !buf.is_empty() {
                    // Convert f32 samples to bytes and write
                    let bytes: &[u8] = unsafe {
                        std::slice::from_raw_parts(
                            buf.as_ptr() as *const u8,
                            buf.len() * std::mem::size_of::<f32>()
                        )
                    };
                    if audio_writer.write_all(bytes).is_err() {
                        log::warn!("[Recorder] Failed to write audio");
                    }
                    total_audio_samples += buf.len() as u64;
                    audio_samples.store(total_audio_samples, Ordering::Relaxed);
                    buf.clear();
                }
            }

            thread::sleep(std::time::Duration::from_micros(500));
        }

        // Close video pipe
        drop(stdin);

        // Flush and close audio file
        audio_writer.flush()?;
        drop(audio_writer);

        log::info!("[Recorder] Finalizing video ({} audio samples)...", total_audio_samples);
        let output = ffmpeg.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::error!("[Recorder] ffmpeg video error: {}", stderr);
            return Ok(());
        }

        // Now mux audio with video
        if total_audio_samples > 0 {
            log::info!("[Recorder] Muxing audio into final MP4...");

            // Run ffmpeg to combine video + audio
            // Audio is f32 stereo 48kHz, boost volume by 6dB
            let ffmpeg_mux_path = find_ffmpeg();
            let mux_output = Command::new(&ffmpeg_mux_path)
                .arg("-y")
                .arg("-i").arg(&video_temp)
                .arg("-f").arg("f32le")
                .arg("-ar").arg("48000")
                .arg("-ac").arg("2")
                .arg("-i").arg(&audio_temp)
                .arg("-c:v").arg("copy")  // Just copy video, no re-encode
                .arg("-af").arg(format!("volume={}", audio_boost))  // Boost audio volume
                .arg("-c:a").arg("aac")
                .arg("-b:a").arg("192k")
                .arg("-shortest")  // Match shortest stream duration
                .arg("-movflags").arg("+faststart")
                .arg(&output_path)
                .stderr(Stdio::piped())
                .output()?;

            if !mux_output.status.success() {
                let stderr = String::from_utf8_lossy(&mux_output.stderr);
                log::error!("[Recorder] ffmpeg mux error: {}", stderr);
                // Fall back to video-only - rename temp to final
                std::fs::rename(&video_temp, &output_path)?;
            } else {
                log::info!("[Recorder] Audio muxed successfully");
                // Clean up temp files
                let _ = std::fs::remove_file(&video_temp);
            }
        } else {
            // No audio, just rename video temp to final output
            std::fs::rename(&video_temp, &output_path)?;
        }

        // Clean up audio temp file
        let _ = std::fs::remove_file(&audio_temp);

        log::info!("[Recorder] Recording complete: {:?}", output_path);
        Ok(())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
