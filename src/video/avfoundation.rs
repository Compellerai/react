//! AVFoundation video decoder for macOS
//!
//! Uses native macOS APIs for hardware-accelerated video decoding.
//! Built with objc2 bindings for modern, type-safe Objective-C interop.
//!
//! For 4K video, use decode_frame_zero_copy() which returns a CVPixelBuffer
//! that can be directly mapped to an OpenGL texture via IOSurface (no memcpy).

use anyhow::{anyhow, Result};
use std::ffi::c_void;
use std::path::Path;

use objc2::encode::{Encode, Encoding};
use objc2::rc::Id;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{NSNumber, NSString};

use super::texture_cache::{RetainedPixelBuffer, CVPixelBufferRef};

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

// CoreMedia functions (FFI)
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetImageBuffer(sbuf: *mut CMSampleBuffer) -> *mut c_void;
}

// CoreFoundation release function
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *mut c_void);
}

/// Opaque CMSampleBuffer type (CoreMedia)
#[repr(C)]
pub struct CMSampleBuffer {
    _opaque: [u8; 0],
}

// Implement Encode for *mut CMSampleBuffer so msg_send can return it
unsafe impl objc2::encode::RefEncode for CMSampleBuffer {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
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

// Implement Encode for CMTime so it can be used with msg_send
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

/// CGSize structure
#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct CGSize {
    width: f64,
    height: f64,
}

// Implement Encode for CGSize
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[Encoding::Double, Encoding::Double]);
}

/// AVFoundation video decoder
pub struct AVFoundationDecoder {
    asset_reader: Option<Id<AnyObject>>,
    track_output: Option<Id<AnyObject>>,
    width: u32,
    height: u32,
    duration: f64,
    frame_rate: f64,
    position: f64,
    frame_buffer: Vec<u8>,
    is_valid: bool,
}

// Safety: AVFoundation objects can be sent between threads when properly managed
unsafe impl Send for AVFoundationDecoder {}

impl AVFoundationDecoder {
    /// Create a new decoder
    pub fn new() -> Self {
        Self {
            asset_reader: None,
            track_output: None,
            width: 0,
            height: 0,
            duration: 0.0,
            frame_rate: 30.0,
            position: 0.0,
            frame_buffer: Vec::new(),
            is_valid: false,
        }
    }

    /// Check if AVFoundation is available (always true on macOS)
    pub fn is_available() -> bool {
        true
    }

    /// Open a video file
    pub fn open(&mut self, path: &str) -> Result<()> {
        log::info!("[AVFoundation] Opening video: {}", path);

        // Close any existing video
        self.close();

        // Verify file exists
        if !Path::new(path).exists() {
            return Err(anyhow!("Video file not found: {}", path));
        }

        unsafe {
            // Create NSURL from path
            let ns_path = NSString::from_str(path);
            let url: Id<AnyObject> = msg_send_id![class!(NSURL), fileURLWithPath: &*ns_path];

            // Create AVAsset
            let asset: Id<AnyObject> = msg_send_id![class!(AVAsset), assetWithURL: &*url];

            // Get video tracks using AVMediaTypeVideo string constant ("vide")
            let video_type = NSString::from_str("vide");
            let video_tracks: Id<AnyObject> = msg_send_id![&*asset, tracksWithMediaType: &*video_type];
            let track_count: usize = msg_send![&*video_tracks, count];
            if track_count == 0 {
                return Err(anyhow!("No video tracks found in file"));
            }

            let video_track: Id<AnyObject> = msg_send_id![&*video_tracks, objectAtIndex: 0usize];

            // Get video properties
            let natural_size: CGSize = msg_send![&*video_track, naturalSize];
            self.width = natural_size.width as u32;
            self.height = natural_size.height as u32;

            // Get duration (CMTime)
            let duration: CMTime = msg_send![&*asset, duration];
            self.duration = duration.value as f64 / duration.timescale as f64;

            // Get frame rate
            let frame_rate: f32 = msg_send![&*video_track, nominalFrameRate];
            self.frame_rate = if frame_rate > 0.0 { frame_rate as f64 } else { 30.0 };

            log::info!("[AVFoundation] Video: {}x{}, {:.2} fps, {:.2}s duration",
                self.width, self.height, self.frame_rate, self.duration);

            // Create output settings for BGRA format
            let pixel_format_key = NSString::from_str("PixelFormatType");
            let pixel_format_value = NSNumber::new_u32(K_CV_PIXEL_FORMAT_TYPE_32BGRA);

            let output_settings: Id<AnyObject> = msg_send_id![
                class!(NSDictionary),
                dictionaryWithObject: &*pixel_format_value
                forKey: &*pixel_format_key
            ];

            // Create AVAssetReaderTrackOutput
            let track_output: Id<AnyObject> = {
                let cls = class!(AVAssetReaderTrackOutput);
                let obj: *mut AnyObject = msg_send![cls, alloc];
                let obj: *mut AnyObject = msg_send![obj, initWithTrack: &*video_track outputSettings: &*output_settings];
                Id::retain(obj).expect("initWithTrack returned nil")
            };

            // Create AVAssetReader
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let asset_reader: Id<AnyObject> = {
                let cls = class!(AVAssetReader);
                let obj: *mut AnyObject = msg_send![cls, alloc];
                let obj: *mut AnyObject = msg_send![obj, initWithAsset: &*asset error: &mut error];
                Id::retain(obj).expect("initWithAsset returned nil")
            };

            if !error.is_null() {
                return Err(anyhow!("Failed to create AVAssetReader"));
            }

            // Add output to reader
            let can_add: Bool = msg_send![&*asset_reader, canAddOutput: &*track_output];
            if !can_add.as_bool() {
                return Err(anyhow!("Cannot add track output to reader"));
            }
            let _: () = msg_send![&*asset_reader, addOutput: &*track_output];

            // Start reading
            let started: Bool = msg_send![&*asset_reader, startReading];
            if !started.as_bool() {
                return Err(anyhow!("Failed to start reading"));
            }

            // Allocate frame buffer
            self.frame_buffer = vec![0u8; (self.width * self.height * 4) as usize];

            self.asset_reader = Some(asset_reader);
            self.track_output = Some(track_output);
            self.position = 0.0;
            self.is_valid = true;

            log::info!("[AVFoundation] Video loaded successfully");
            Ok(())
        }
    }

    /// Close the video
    pub fn close(&mut self) {
        if let Some(ref reader) = self.asset_reader {
            unsafe {
                let _: () = msg_send![&**reader, cancelReading];
            }
        }
        self.asset_reader = None;
        self.track_output = None;
        self.width = 0;
        self.height = 0;
        self.duration = 0.0;
        self.position = 0.0;
        self.frame_buffer.clear();
        self.is_valid = false;
        log::info!("[AVFoundation] Video closed");
    }

    /// Decode the next frame
    pub fn decode_frame(&mut self) -> Option<(&[u8], u32, u32)> {
        if !self.is_valid {
            return None;
        }

        let track_output = self.track_output.as_ref()?;

        unsafe {
            // Get next sample buffer (returns CMSampleBufferRef - a CFTypeRef)
            let sample_buffer: *mut CMSampleBuffer = msg_send![&**track_output, copyNextSampleBuffer];
            if sample_buffer.is_null() {
                return None;
            }

            // Get pixel buffer from sample buffer
            let pixel_buffer = CMSampleBufferGetImageBuffer(sample_buffer);
            if pixel_buffer.is_null() {
                // Release sample buffer (CF type, use CFRelease)
                CFRelease(sample_buffer as *mut c_void);
                return None;
            }

            // Lock pixel buffer for reading (flag 0x01 = read only)
            let lock_result = CVPixelBufferLockBaseAddress(pixel_buffer, 0x01);
            if lock_result != 0 {
                CFRelease(sample_buffer as *mut c_void);
                return None;
            }

            // Get buffer properties
            let base_address = CVPixelBufferGetBaseAddress(pixel_buffer);
            let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
            let height = CVPixelBufferGetHeight(pixel_buffer);
            let width = CVPixelBufferGetWidth(pixel_buffer);

            if !base_address.is_null() {
                // Ultra-fast copy - BGRA format, NO Y-flip (GPU shader handles both)
                // This minimizes CPU work - just a straight memory copy
                let src_ptr = base_address as *const u8;
                let row_bytes = width * 4;
                let total_bytes = row_bytes * height;

                // Check if we can do a single bulk copy (when stride == row_bytes)
                if bytes_per_row == row_bytes {
                    // Single memcpy for the entire frame - fastest path!
                    std::ptr::copy_nonoverlapping(
                        src_ptr,
                        self.frame_buffer.as_mut_ptr(),
                        total_bytes,
                    );
                } else {
                    // Stride differs from row width, need per-row copy
                    for y in 0..height {
                        let src_row_start = y * bytes_per_row;
                        let dst_row_start = y * row_bytes;
                        std::ptr::copy_nonoverlapping(
                            src_ptr.add(src_row_start),
                            self.frame_buffer.as_mut_ptr().add(dst_row_start),
                            row_bytes,
                        );
                    }
                }
            }

            // Unlock pixel buffer
            CVPixelBufferUnlockBaseAddress(pixel_buffer, 0x01);

            // Release sample buffer (CF type)
            CFRelease(sample_buffer as *mut c_void);

            // Update position
            self.position += 1.0 / self.frame_rate;

            Some((&self.frame_buffer, self.width, self.height))
        }
    }

    /// Decode the next frame (zero-copy version)
    /// Returns a retained CVPixelBuffer that can be used directly with TextureCache
    /// for GPU texture creation without any CPU memcpy.
    pub fn decode_frame_zero_copy(&mut self) -> Option<RetainedPixelBuffer> {
        if !self.is_valid {
            return None;
        }

        let track_output = self.track_output.as_ref()?;

        unsafe {
            // Get next sample buffer (returns CMSampleBufferRef - a CFTypeRef)
            let sample_buffer: *mut CMSampleBuffer = msg_send![&**track_output, copyNextSampleBuffer];
            if sample_buffer.is_null() {
                return None;
            }

            // Get pixel buffer from sample buffer
            let pixel_buffer = CMSampleBufferGetImageBuffer(sample_buffer);
            if pixel_buffer.is_null() {
                CFRelease(sample_buffer as *mut c_void);
                return None;
            }

            // Create retained pixel buffer (this calls CVPixelBufferRetain internally)
            // IMPORTANT: Must retain BEFORE releasing sample buffer!
            let retained = RetainedPixelBuffer::new(pixel_buffer);

            // Release sample buffer (pixel buffer stays alive due to our retain)
            CFRelease(sample_buffer as *mut c_void);

            // Update position
            self.position += 1.0 / self.frame_rate;

            retained
        }
    }

    /// Seek to position (requires recreating the reader)
    pub fn seek(&mut self, _position: f64) -> Result<()> {
        log::warn!("[AVFoundation] Seek not yet implemented");
        Ok(())
    }

    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get duration
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Get frame rate
    pub fn frame_rate(&self) -> f64 {
        self.frame_rate
    }

    /// Check if video is loaded
    pub fn is_loaded(&self) -> bool {
        self.is_valid && self.width > 0 && self.height > 0
    }
}

impl Drop for AVFoundationDecoder {
    fn drop(&mut self) {
        self.close();
    }
}
