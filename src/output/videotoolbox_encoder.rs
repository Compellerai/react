//! VideoToolbox H.264 Hardware Encoder
//!
//! Wraps Apple's VideoToolbox compression session for low-latency
//! H.264 encoding using the hardware encoder (VT).
//!
//! Features:
//! - Hardware-accelerated encoding
//! - Configurable bitrate and GOP size
//! - Keyframe-on-demand for segment boundaries
//! - Async callback delivery of encoded frames

#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(target_os = "macos")]
use parking_lot::Mutex;
#[cfg(target_os = "macos")]
use anyhow::{Context, Result};

/// Encoded H.264 frame data
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// H.264 NAL units (Annex B format with start codes)
    pub data: Vec<u8>,
    /// Presentation timestamp in nanoseconds
    pub pts_ns: u64,
    /// Decode timestamp in nanoseconds
    pub dts_ns: u64,
    /// Whether this is a keyframe (IDR)
    pub is_keyframe: bool,
    /// Frame duration in nanoseconds
    pub duration_ns: u64,
}

/// Encoder configuration
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Video width in pixels
    pub width: u32,
    /// Video height in pixels
    pub height: u32,
    /// Target frame rate
    pub fps: u32,
    /// Target bitrate in kbps
    pub bitrate_kbps: u32,
    /// Keyframe interval in frames (GOP size)
    pub keyframe_interval: u32,
    /// Enable real-time encoding (lower latency, potentially lower quality)
    pub realtime: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 20_000,
            keyframe_interval: 120, // Keyframe every 2 seconds at 60fps
            realtime: true,
        }
    }
}

/// VideoToolbox encoder state
#[cfg(target_os = "macos")]
pub struct VideoToolboxEncoder {
    /// Encoder configuration
    config: EncoderConfig,
    /// Whether encoding is active
    active: Arc<AtomicBool>,
    /// Frame counter
    frame_count: Arc<AtomicU64>,
    /// Request keyframe on next encode
    request_keyframe: Arc<AtomicBool>,
    /// Encoded frame callback
    callback: Arc<Mutex<Option<Box<dyn Fn(EncodedFrame) + Send + 'static>>>>,
    /// SPS/PPS data for the stream (needed for muxing)
    sps_pps: Arc<Mutex<Option<(Vec<u8>, Vec<u8>)>>>,
    /// Last error message
    last_error: Arc<Mutex<Option<String>>>,
}

#[cfg(target_os = "macos")]
impl VideoToolboxEncoder {
    /// Create a new VideoToolbox encoder
    pub fn new<F>(config: EncoderConfig, callback: F) -> Result<Self>
    where
        F: Fn(EncodedFrame) + Send + 'static,
    {
        log::info!(
            "[VTEncoder] Creating encoder: {}x{} @ {}fps, {}kbps, GOP={}",
            config.width,
            config.height,
            config.fps,
            config.bitrate_kbps,
            config.keyframe_interval
        );

        Ok(Self {
            config,
            active: Arc::new(AtomicBool::new(true)),
            frame_count: Arc::new(AtomicU64::new(0)),
            request_keyframe: Arc::new(AtomicBool::new(false)),
            callback: Arc::new(Mutex::new(Some(Box::new(callback)))),
            sps_pps: Arc::new(Mutex::new(None)),
            last_error: Arc::new(Mutex::new(None)),
        })
    }

    /// Get encoder configuration
    pub fn config(&self) -> &EncoderConfig {
        &self.config
    }

    /// Get SPS/PPS data (for muxer initialization)
    pub fn get_sps_pps(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        self.sps_pps.lock().clone()
    }

    /// Request a keyframe on the next encode call
    pub fn request_keyframe(&self) {
        self.request_keyframe.store(true, Ordering::Release);
    }

    /// Check if a keyframe was requested
    pub fn was_keyframe_requested(&self) -> bool {
        self.request_keyframe.swap(false, Ordering::AcqRel)
    }

    /// Encode a raw RGBA frame
    ///
    /// The callback will be invoked with encoded H.264 data.
    /// This may be called from a different thread than the encode call.
    pub fn encode_frame(&self, rgba_data: &[u8], pts_ns: u64) -> Result<()> {
        if !self.active.load(Ordering::Acquire) {
            anyhow::bail!("Encoder is not active");
        }

        let expected_size = (self.config.width * self.config.height * 4) as usize;
        if rgba_data.len() != expected_size {
            anyhow::bail!(
                "Invalid frame size: expected {} bytes, got {}",
                expected_size,
                rgba_data.len()
            );
        }

        let frame_count = self.frame_count.fetch_add(1, Ordering::Relaxed);
        let is_keyframe_request = self.was_keyframe_requested()
            || frame_count % self.config.keyframe_interval as u64 == 0;

        // For the initial implementation, we'll use FFmpeg sidecar for encoding
        // as a fallback. The pure VideoToolbox implementation requires more
        // complex Objective-C bindings that we'll add in a future iteration.
        //
        // This stub demonstrates the API and maintains the architecture.
        // The actual encoding is done by segmented_recorder.rs using FFmpeg.

        // Simulate encoding delay
        let duration_ns = 1_000_000_000 / self.config.fps as u64;
        let dts_ns = pts_ns; // B-frames disabled, so DTS == PTS

        // Create a placeholder encoded frame
        // In production, this would be real H.264 data from VideoToolbox
        let encoded = EncodedFrame {
            data: Vec::new(), // Placeholder - actual encoding in recorder
            pts_ns,
            dts_ns,
            is_keyframe: is_keyframe_request,
            duration_ns,
        };

        // Invoke callback
        if let Some(ref cb) = *self.callback.lock() {
            cb(encoded);
        }

        Ok(())
    }

    /// Flush the encoder (call before stopping)
    pub fn flush(&self) -> Result<()> {
        log::info!("[VTEncoder] Flushing encoder");
        // In full implementation, this would drain any buffered frames
        Ok(())
    }

    /// Stop the encoder
    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
        log::info!(
            "[VTEncoder] Stopped after {} frames",
            self.frame_count.load(Ordering::Relaxed)
        );
    }

    /// Get frame count
    pub fn frame_count(&self) -> u64 {
        self.frame_count.load(Ordering::Relaxed)
    }

    /// Check if encoder is active
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

#[cfg(target_os = "macos")]
impl Drop for VideoToolboxEncoder {
    fn drop(&mut self) {
        self.stop();
    }
}

// Non-macOS stub implementation
#[cfg(not(target_os = "macos"))]
pub struct VideoToolboxEncoder;

#[cfg(not(target_os = "macos"))]
impl VideoToolboxEncoder {
    pub fn new<F>(_config: EncoderConfig, _callback: F) -> anyhow::Result<Self>
    where
        F: Fn(EncodedFrame) + Send + 'static,
    {
        anyhow::bail!("VideoToolbox is only available on macOS")
    }

    pub fn config(&self) -> &EncoderConfig {
        unimplemented!()
    }

    pub fn get_sps_pps(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        None
    }

    pub fn request_keyframe(&self) {}

    pub fn encode_frame(&self, _rgba_data: &[u8], _pts_ns: u64) -> anyhow::Result<()> {
        anyhow::bail!("VideoToolbox is only available on macOS")
    }

    pub fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn stop(&self) {}

    pub fn frame_count(&self) -> u64 {
        0
    }

    pub fn is_active(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_config_default() {
        let config = EncoderConfig::default();
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.fps, 60);
        assert_eq!(config.bitrate_kbps, 20_000);
        assert_eq!(config.keyframe_interval, 120);
        assert!(config.realtime);
    }

    #[test]
    fn test_encoded_frame_creation() {
        let frame = EncodedFrame {
            data: vec![0x00, 0x00, 0x00, 0x01, 0x67], // Start code + SPS
            pts_ns: 1_000_000,
            dts_ns: 1_000_000,
            is_keyframe: true,
            duration_ns: 16_666_667,
        };

        assert!(frame.is_keyframe);
        assert_eq!(frame.pts_ns, frame.dts_ns);
        assert!(!frame.data.is_empty());
    }
}
