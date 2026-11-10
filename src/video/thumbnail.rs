//! Video thumbnail generation using AVFoundation
//!
//! Generates thumbnail images from video files for UI preview.
//! Uses native macOS APIs for efficient frame extraction.

use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use objc2::rc::Id;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{NSNumber, NSString};
use objc2::encode::{Encode, Encoding, RefEncode};

// Thumbnail maximum dimensions (retina-ready)
// Thumbnails preserve original aspect ratio within these bounds
pub const THUMBNAIL_MAX_WIDTH: u32 = 240;
pub const THUMBNAIL_MAX_HEIGHT: u32 = 240;

// CoreVideo constants
const K_CV_PIXEL_FORMAT_TYPE_32BGRA: u32 = 0x42475241; // 'BGRA'

// CoreVideo functions (FFI)
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferLockBaseAddress(pixelBuffer: *mut c_void, lockFlags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pixelBuffer: *mut c_void, lockFlags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pixelBuffer: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(pixelBuffer: *mut c_void) -> usize;
    fn CVPixelBufferGetWidth(pixelBuffer: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(pixelBuffer: *mut c_void) -> usize;
}

// CoreFoundation
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *mut c_void);
}

/// CMTime structure (must match Apple's layout exactly)
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    fn make(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: 1, // kCMTimeFlags_Valid
            epoch: 0,
        }
    }
}

unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[
            Encoding::LongLong,
            Encoding::Int,
            Encoding::UInt,
            Encoding::LongLong,
        ],
    );
}

unsafe impl RefEncode for CMTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&CMTime::ENCODING);
}

/// CGSize structure
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct CGSize {
    width: f64,
    height: f64,
}

unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[Encoding::Double, Encoding::Double]);
}

unsafe impl RefEncode for CGSize {
    const ENCODING_REF: Encoding = Encoding::Pointer(&CGSize::ENCODING);
}

/// CGPoint structure
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct CGPoint {
    x: f64,
    y: f64,
}

/// CGRect structure
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

/// Opaque CGImage type for proper Objective-C encoding
#[repr(C)]
struct CGImage {
    _private: [u8; 0],
}

unsafe impl RefEncode for CGImage {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

/// Background thumbnail generator
pub struct ThumbnailGenerator {
    /// Queue of video paths to generate thumbnails for
    queue: Arc<Mutex<Vec<String>>>,
    /// Set of videos currently being processed (to avoid duplicates)
    processing: Arc<Mutex<HashSet<String>>>,
    /// Cache directory for thumbnails
    cache_dir: PathBuf,
    /// Flag to stop background thread
    running: Arc<Mutex<bool>>,
    /// Thread handle
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl ThumbnailGenerator {
    /// Create a new thumbnail generator
    pub fn new() -> Result<Self> {
        // Create cache directory
        let cache_dir = Self::get_cache_dir()?;
        std::fs::create_dir_all(&cache_dir)?;

        log::info!("[Thumbnail] Cache directory: {:?}", cache_dir);

        Ok(Self {
            queue: Arc::new(Mutex::new(Vec::new())),
            processing: Arc::new(Mutex::new(HashSet::new())),
            cache_dir,
            running: Arc::new(Mutex::new(false)),
            thread_handle: None,
        })
    }

    /// Get the cache directory path
    fn get_cache_dir() -> Result<PathBuf> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow!("Could not find cache directory"))?;
        Ok(cache_dir.join("compeller").join("thumbnails"))
    }

    /// Get thumbnail path for a video file
    pub fn get_thumbnail_path(&self, video_path: &str) -> PathBuf {
        // Use a hash of the video path for the filename
        let hash = Self::hash_path(video_path);
        self.cache_dir.join(format!("{}.png", hash))
    }

    /// Check if thumbnail exists
    pub fn has_thumbnail(&self, video_path: &str) -> bool {
        self.get_thumbnail_path(video_path).exists()
    }

    /// Hash a path to create a unique filename
    fn hash_path(path: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Queue a video for thumbnail generation
    pub fn queue_video(&self, video_path: &str) {
        // Skip if already has thumbnail
        if self.has_thumbnail(video_path) {
            return;
        }

        // Skip if already in processing set
        {
            let processing = self.processing.lock().unwrap();
            if processing.contains(video_path) {
                return;
            }
        }

        // Add to queue
        {
            let mut queue = self.queue.lock().unwrap();
            if !queue.contains(&video_path.to_string()) {
                queue.push(video_path.to_string());
                log::debug!("[Thumbnail] Queued: {}", video_path);
            }
        }
    }

    /// Queue multiple videos for thumbnail generation
    pub fn queue_videos(&self, video_paths: &[String]) {
        for path in video_paths {
            self.queue_video(path);
        }
    }

    /// Start the background generation thread
    pub fn start(&mut self) {
        {
            let mut running = self.running.lock().unwrap();
            if *running {
                return; // Already running
            }
            *running = true;
        }

        let queue = self.queue.clone();
        let processing = self.processing.clone();
        let cache_dir = self.cache_dir.clone();
        let running = self.running.clone();

        let handle = std::thread::spawn(move || {
            log::info!("[Thumbnail] Background thread started");

            while *running.lock().unwrap() {
                // Get next video from queue
                let video_path = {
                    let mut queue = queue.lock().unwrap();
                    queue.pop()
                };

                if let Some(path) = video_path {
                    // Mark as processing
                    {
                        let mut proc = processing.lock().unwrap();
                        proc.insert(path.clone());
                    }

                    // Generate thumbnail
                    let thumb_path = {
                        let hash = Self::hash_path(&path);
                        cache_dir.join(format!("{}.png", hash))
                    };

                    match Self::generate_thumbnail(&path, &thumb_path) {
                        Ok(()) => {
                            log::debug!("[Thumbnail] Generated: {}", path);
                        }
                        Err(e) => {
                            log::warn!("[Thumbnail] Failed for {}: {}", path, e);
                        }
                    }

                    // Remove from processing
                    {
                        let mut proc = processing.lock().unwrap();
                        proc.remove(&path);
                    }
                } else {
                    // No work, sleep briefly
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }

            log::info!("[Thumbnail] Background thread stopped");
        });

        self.thread_handle = Some(handle);
    }

    /// Stop the background generation thread
    pub fn stop(&mut self) {
        {
            let mut running = self.running.lock().unwrap();
            *running = false;
        }

        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    /// Get number of videos in queue
    pub fn queue_len(&self) -> usize {
        self.queue.lock().unwrap().len()
    }

    /// Clean up orphaned thumbnails (for videos that no longer exist)
    pub fn cleanup_orphaned(&self, valid_video_paths: &[String]) {
        let valid_hashes: std::collections::HashSet<String> = valid_video_paths
            .iter()
            .map(|p| Self::hash_path(p))
            .collect();

        // Scan cache directory and remove orphaned thumbnails
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "png").unwrap_or(false) {
                    if let Some(stem) = path.file_stem() {
                        let hash = stem.to_string_lossy().to_string();
                        if !valid_hashes.contains(&hash) {
                            log::debug!("[Thumbnail] Removing orphaned thumbnail: {:?}", path);
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
            }
        }
    }

    /// Generate a thumbnail for a video file
    fn generate_thumbnail(video_path: &str, output_path: &Path) -> Result<()> {
        if !Path::new(video_path).exists() {
            return Err(anyhow!("Video file not found: {}", video_path));
        }

        unsafe {
            // Create NSURL from path
            let ns_path = NSString::from_str(video_path);
            let url: Id<AnyObject> = msg_send_id![class!(NSURL), fileURLWithPath: &*ns_path];

            // Create AVAsset
            let asset: Id<AnyObject> = msg_send_id![class!(AVAsset), assetWithURL: &*url];

            // Get duration
            let duration: CMTime = msg_send![&*asset, duration];
            let total_secs = duration.value as f64 / duration.timescale as f64;

            // Seek to ~10% of video for thumbnail
            let seek_secs = total_secs * 0.10;
            let seek_time = CMTime::make(
                (seek_secs * 600.0) as i64,
                600
            );

            // Create AVAssetImageGenerator
            let generator: Id<AnyObject> = {
                let cls = class!(AVAssetImageGenerator);
                let obj: *mut AnyObject = msg_send![cls, alloc];
                let obj: *mut AnyObject = msg_send![obj, initWithAsset: &*asset];
                Id::retain(obj).expect("AVAssetImageGenerator alloc returned nil")
            };

            // Set max size - image will be scaled to fit within bounds preserving aspect ratio
            let max_size = CGSize {
                width: THUMBNAIL_MAX_WIDTH as f64,
                height: THUMBNAIL_MAX_HEIGHT as f64,
            };
            let _: () = msg_send![&*generator, setMaximumSize: max_size];

            // Apply presentation time for correct frame
            let _: () = msg_send![&*generator, setAppliesPreferredTrackTransform: Bool::YES];

            // Generate CGImage at time
            let mut actual_time = CMTime::make(0, 1);
            let mut error: *mut AnyObject = std::ptr::null_mut();

            let cg_image: *mut CGImage = msg_send![
                &*generator,
                copyCGImageAtTime: seek_time
                actualTime: &mut actual_time as *mut CMTime
                error: &mut error as *mut *mut AnyObject
            ];

            if cg_image.is_null() || !error.is_null() {
                return Err(anyhow!("Failed to generate image from video"));
            }

            // Cast to c_void for CoreGraphics functions
            let cg_image_ptr = cg_image as *const c_void;

            // Save as PNG using CGImageDestination
            let output_str = output_path.to_string_lossy();
            let output_nsstr = NSString::from_str(&output_str);
            let output_url: Id<AnyObject> = msg_send_id![class!(NSURL), fileURLWithPath: &*output_nsstr];

            // Get PNG UTI
            let png_uti = NSString::from_str("public.png");

            // Create CGImageDestination
            #[link(name = "ImageIO", kind = "framework")]
            extern "C" {
                fn CGImageDestinationCreateWithURL(
                    url: *const c_void,
                    uti: *const c_void,
                    count: usize,
                    options: *const c_void,
                ) -> *mut c_void;
                fn CGImageDestinationAddImage(
                    dest: *mut c_void,
                    image: *const c_void,
                    properties: *const c_void,
                );
                fn CGImageDestinationFinalize(dest: *mut c_void) -> bool;
            }

            #[link(name = "CoreGraphics", kind = "framework")]
            extern "C" {
                fn CGImageRelease(image: *const c_void);
            }

            // Use the image directly - setMaximumSize already scaled it to fit within bounds
            // while preserving the original aspect ratio

            // Convert Id<AnyObject> to raw pointer for CF functions
            let url_ptr = &*output_url as *const AnyObject as *const c_void;
            let uti_ptr = &*png_uti as *const NSString as *const c_void;

            let dest = CGImageDestinationCreateWithURL(
                url_ptr,
                uti_ptr,
                1,
                std::ptr::null(),
            );

            if dest.is_null() {
                CGImageRelease(cg_image_ptr);
                return Err(anyhow!("Failed to create image destination"));
            }

            CGImageDestinationAddImage(dest, cg_image_ptr, std::ptr::null());

            let success = CGImageDestinationFinalize(dest);

            // Cleanup
            CFRelease(dest);
            CGImageRelease(cg_image_ptr);

            if !success {
                return Err(anyhow!("Failed to write PNG file"));
            }

            Ok(())
        }
    }
}

impl Drop for ThumbnailGenerator {
    fn drop(&mut self) {
        self.stop();
    }
}
