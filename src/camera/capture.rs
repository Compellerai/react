//! Camera capture using nokhwa
//!
//! Provides USB webcam capture with frame ring buffers for non-blocking access.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use parking_lot::Mutex;
use anyhow::Result;

use super::CameraInfo;

/// Frame data from camera
#[derive(Clone)]
pub struct CameraFrame {
    pub data: Vec<u8>,       // RGB or RGBA data
    pub width: u32,
    pub height: u32,
    pub frame_num: u64,
}

impl CameraFrame {
    pub fn empty() -> Self {
        Self {
            data: Vec::new(),
            width: 0,
            height: 0,
            frame_num: 0,
        }
    }
}

/// Ring buffer for camera frames
struct FrameRing {
    frames: [CameraFrame; 2],
    write_idx: usize,
    read_idx: usize,
}

impl FrameRing {
    fn new() -> Self {
        Self {
            frames: [CameraFrame::empty(), CameraFrame::empty()],
            write_idx: 0,
            read_idx: 0,
        }
    }

    fn push(&mut self, frame: CameraFrame) {
        self.frames[self.write_idx] = frame;
        self.write_idx = (self.write_idx + 1) % 2;
    }

    fn get_latest(&self) -> Option<&CameraFrame> {
        let idx = if self.write_idx == 0 { 1 } else { 0 };
        let frame = &self.frames[idx];
        if frame.frame_num > 0 {
            Some(frame)
        } else {
            None
        }
    }
}

/// Camera capture instance
pub struct CameraCapture {
    camera_id: u32,
    resolution: (u32, u32),
    active: Arc<AtomicBool>,
    frame_ring: Arc<Mutex<FrameRing>>,
    frame_count: Arc<AtomicU64>,
    thread_handle: Option<JoinHandle<()>>,
}

impl CameraCapture {
    /// Create and start camera capture using stable device ID
    pub fn new(camera_id: u32, resolution: (u32, u32)) -> Result<Self> {
        Self::new_with_device_id(camera_id, resolution, None)
    }

    /// Create and start camera capture with explicit device ID for stable camera mapping
    pub fn new_with_device_id(camera_id: u32, resolution: (u32, u32), device_id: Option<String>) -> Result<Self> {
        let active = Arc::new(AtomicBool::new(true));
        let frame_ring = Arc::new(Mutex::new(FrameRing::new()));
        let frame_count = Arc::new(AtomicU64::new(0));

        let active_clone = active.clone();
        let ring_clone = frame_ring.clone();
        let count_clone = frame_count.clone();

        let handle = thread::Builder::new()
            .name(format!("camera-{}", camera_id))
            .spawn(move || {
                Self::capture_thread(
                    camera_id,
                    resolution,
                    active_clone,
                    ring_clone,
                    count_clone,
                    device_id,
                );
            })?;

        Ok(Self {
            camera_id,
            resolution,
            active,
            frame_ring,
            frame_count,
            thread_handle: Some(handle),
        })
    }

    pub fn camera_id(&self) -> u32 {
        self.camera_id
    }

    /// Get latest frame (non-blocking)
    pub fn get_frame(&self) -> Option<CameraFrame> {
        self.frame_ring.try_lock().and_then(|ring| ring.get_latest().cloned())
    }

    /// Stop capture (non-blocking — signals thread to exit, doesn't wait)
    pub fn stop(&mut self) {
        self.active.store(false, Ordering::Release);
        // Don't join — let capture thread exit naturally after camera.frame() returns.
        // Joining here would block the main render thread while the capture thread
        // waits on the hardware-blocking camera.frame() call (33ms+ or indefinite).
        self.thread_handle.take();
    }

    fn capture_thread(
        camera_id: u32,
        resolution: (u32, u32),
        active: Arc<AtomicBool>,
        frame_ring: Arc<Mutex<FrameRing>>,
        frame_count: Arc<AtomicU64>,
        device_id: Option<String>,
    ) {
        log::info!("[Camera {}] Capture thread started (device_id={:?})", camera_id, device_id);

        // Try to open camera with nokhwa
        #[cfg(feature = "camera")]
        {
            use nokhwa::pixel_format::RgbFormat;
            use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};
            use nokhwa::{Camera, query};

            // CameraIndex::String crashes on macOS AVFoundation backend.
            // Instead, re-enumerate cameras and match by stable device_id (misc())
            // to find the correct positional index at open time.
            let index = if let Some(ref target_id) = device_id {
                let mut matched_index = None;
                if let Ok(devices) = query(ApiBackend::Auto) {
                    for (i, device) in devices.iter().enumerate() {
                        let misc = device.misc().trim().to_string();
                        let dev_id = if misc.is_empty() {
                            device.index().as_string()
                        } else {
                            misc
                        };
                        if &dev_id == target_id {
                            matched_index = Some(i as u32);
                            log::info!("[Camera {}] Matched device_id '{}' to position {}", camera_id, target_id, i);
                            break;
                        }
                    }
                }
                match matched_index {
                    Some(pos) => CameraIndex::Index(pos),
                    None => {
                        log::warn!("[Camera {}] device_id '{}' not found, falling back to index {}", camera_id, target_id, camera_id);
                        CameraIndex::Index(camera_id)
                    }
                }
            } else {
                CameraIndex::Index(camera_id)
            };
            log::info!("[Camera {}] Opening with index: {:?}", camera_id, index);
            // Use AbsoluteHighestResolution and let nokhwa pick the best format
            let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);

            let mut camera = match Camera::new(index, format) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("[Camera {}] Failed to open: {}", camera_id, e);
                    active.store(false, Ordering::Release);
                    return;
                }
            };

            if let Err(e) = camera.open_stream() {
                log::error!("[Camera {}] Failed to start stream: {}", camera_id, e);
                active.store(false, Ordering::Release);
                return;
            }

            let cam_res = camera.resolution();
            log::info!("[Camera {}] Stream started at {}x{}", camera_id, cam_res.width(), cam_res.height());

            while active.load(Ordering::Acquire) {
                match camera.frame() {
                    Ok(frame) => {
                        let decoded = frame.decode_image::<RgbFormat>();
                        match decoded {
                            Ok(img) => {
                                let count = frame_count.fetch_add(1, Ordering::Relaxed) + 1;
                                let camera_frame = CameraFrame {
                                    data: img.to_vec(),
                                    width: img.width(),
                                    height: img.height(),
                                    frame_num: count,
                                };
                                if let Some(mut ring) = frame_ring.try_lock() {
                                    ring.push(camera_frame);
                                }
                            }
                            Err(e) => {
                                log::warn!("[Camera {}] Decode error: {}", camera_id, e);
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("[Camera {}] Frame error: {}", camera_id, e);
                        thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
            }

            let _ = camera.stop_stream();
        }

        #[cfg(not(feature = "camera"))]
        {
            // Stub: generate test pattern when nokhwa not available
            log::warn!("[Camera {}] nokhwa not available, using test pattern", camera_id);

            let width = resolution.0;
            let height = resolution.1;
            let mut frame_num = 0u64;

            while active.load(Ordering::Acquire) {
                frame_num += 1;

                // Generate simple moving gradient test pattern
                let mut data = vec![0u8; (width * height * 3) as usize];
                let offset = (frame_num % 256) as u8;

                for y in 0..height {
                    for x in 0..width {
                        let idx = ((y * width + x) * 3) as usize;
                        data[idx] = ((x as u8).wrapping_add(offset)) % 255;     // R
                        data[idx + 1] = ((y as u8).wrapping_add(offset)) % 255; // G
                        data[idx + 2] = offset;                                  // B
                    }
                }

                let frame = CameraFrame {
                    data,
                    width,
                    height,
                    frame_num,
                };

                if let Some(mut ring) = frame_ring.try_lock() {
                    ring.push(frame);
                }

                frame_count.store(frame_num, Ordering::Relaxed);

                // Simulate ~30fps capture
                thread::sleep(std::time::Duration::from_millis(33));
            }
        }

        log::info!("[Camera {}] Capture thread stopped", camera_id);
    }
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

/// Enumerate available cameras
pub fn enumerate_cameras() -> Result<Vec<CameraInfo>> {
    let mut cameras = Vec::new();

    #[cfg(feature = "camera")]
    {
        use nokhwa::query;
        use nokhwa::utils::ApiBackend;

        // Use default backend for platform
        let backend = ApiBackend::Auto;

        match query(backend) {
            Ok(devices) => {
                for (i, device) in devices.iter().enumerate() {
                    // Use misc() as stable device ID (AVFoundation uniqueID on macOS)
                    // Fall back to the original index string
                    let device_id = {
                        let misc = device.misc().trim().to_string();
                        if misc.is_empty() {
                            device.index().as_string()
                        } else {
                            misc
                        }
                    };
                    log::info!("[Camera] Found device {}: name={}, device_id={}", i, device.human_name(), device_id);
                    cameras.push(CameraInfo {
                        index: i as u32,
                        name: device.human_name().to_string(),
                        resolution: (640, 480), // Default, will be negotiated on open
                        device_id,
                    });
                }
            }
            Err(e) => {
                log::warn!("[Camera] Failed to enumerate: {}", e);
            }
        }
    }

    #[cfg(not(feature = "camera"))]
    {
        // Stub: return fake camera for testing
        cameras.push(CameraInfo {
            index: 0,
            name: "Test Camera (nokhwa disabled)".to_string(),
            resolution: (640, 480),
            device_id: "test-camera-0".to_string(),
        });
    }

    log::info!("[Camera] Found {} cameras", cameras.len());
    Ok(cameras)
}
