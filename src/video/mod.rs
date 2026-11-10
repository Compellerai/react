//! Video decoding and playback
//!
//! On macOS, uses AVFoundation for hardware-accelerated video decoding.
//! No external dependencies required.
//!
//! For 4K video, uses IOSurface zero-copy via CVOpenGLTextureCache.

mod decoder;

#[cfg(target_os = "macos")]
mod avfoundation;

#[cfg(target_os = "macos")]
pub mod texture_cache;

#[cfg(target_os = "macos")]
pub mod thumbnail;

pub use decoder::VideoDecoder;

#[cfg(target_os = "macos")]
pub use texture_cache::{TextureCache, RetainedPixelBuffer};

#[cfg(target_os = "macos")]
pub use thumbnail::ThumbnailGenerator;
