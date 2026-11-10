//! CVOpenGLTextureCache for zero-copy GPU texture creation from CVPixelBuffer
//!
//! Uses IOSurface-backed CVPixelBuffers to create OpenGL textures directly
//! without any CPU memcpy. Essential for 4K video performance.

use std::ffi::c_void;

// CoreVideo types
pub type CVPixelBufferRef = *mut c_void;
pub type CVImageBufferRef = *mut c_void;
pub type CVOpenGLTextureCacheRef = *mut c_void;
pub type CVOpenGLTextureRef = *mut c_void;

// IOSurface type
pub type IOSurfaceRef = *mut c_void;

// CoreFoundation types
pub type CFAllocatorRef = *mut c_void;
pub type CFDictionaryRef = *mut c_void;

// CGL types
pub type CGLContextObj = *mut c_void;
pub type CGLPixelFormatObj = *mut c_void;

// OpenGL types
pub type GLenum = u32;
pub type GLint = i32;
pub type GLuint = u32;

// OpenGL constants
pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
pub const GL_TEXTURE_RECTANGLE: GLenum = 0x84F5;
pub const GL_RGBA: GLenum = 0x1908;
pub const GL_RGBA8: GLint = 0x8058;
pub const GL_BGRA: GLenum = 0x80E1;
pub const GL_UNSIGNED_INT_8_8_8_8_REV: GLenum = 0x8367;
pub const GL_UNSIGNED_BYTE: GLenum = 0x1401;

// CoreVideo error codes
pub const K_CV_RETURN_SUCCESS: i32 = 0;

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    // CVPixelBuffer functions
    pub fn CVPixelBufferRetain(pixelBuffer: CVPixelBufferRef) -> CVPixelBufferRef;
    pub fn CVPixelBufferRelease(pixelBuffer: CVPixelBufferRef);
    pub fn CVPixelBufferGetIOSurface(pixelBuffer: CVPixelBufferRef) -> IOSurfaceRef;
    pub fn CVPixelBufferGetWidth(pixelBuffer: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetHeight(pixelBuffer: CVPixelBufferRef) -> usize;

    // CVOpenGLTextureCache functions
    pub fn CVOpenGLTextureCacheCreate(
        allocator: CFAllocatorRef,
        cacheAttributes: CFDictionaryRef,
        cglContext: CGLContextObj,
        cglPixelFormat: CGLPixelFormatObj,
        textureAttributes: CFDictionaryRef,
        cacheOut: *mut CVOpenGLTextureCacheRef,
    ) -> i32;

    pub fn CVOpenGLTextureCacheCreateTextureFromImage(
        allocator: CFAllocatorRef,
        textureCache: CVOpenGLTextureCacheRef,
        sourceImage: CVImageBufferRef,
        attributes: CFDictionaryRef,
        textureOut: *mut CVOpenGLTextureRef,
    ) -> i32;

    pub fn CVOpenGLTextureCacheFlush(
        textureCache: CVOpenGLTextureCacheRef,
        options: u64,
    );

    pub fn CVOpenGLTextureCacheRelease(textureCache: CVOpenGLTextureCacheRef);

    // CVOpenGLTexture functions
    pub fn CVOpenGLTextureGetName(texture: CVOpenGLTextureRef) -> GLuint;
    pub fn CVOpenGLTextureGetTarget(texture: CVOpenGLTextureRef) -> GLenum;
    pub fn CVOpenGLTextureRelease(texture: CVOpenGLTextureRef);
    pub fn CVOpenGLTextureRetain(texture: CVOpenGLTextureRef) -> CVOpenGLTextureRef;
}

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    pub fn IOSurfaceGetID(ioSurface: IOSurfaceRef) -> u32;
}

#[link(name = "OpenGL", kind = "framework")]
extern "C" {
    pub fn CGLGetCurrentContext() -> CGLContextObj;
    pub fn CGLGetPixelFormat(ctx: CGLContextObj) -> CGLPixelFormatObj;
}

/// Wrapper around CVOpenGLTextureCache for zero-copy texture creation
pub struct TextureCache {
    cache: CVOpenGLTextureCacheRef,
    /// Current texture (we release old one when creating new)
    current_texture: Option<CVOpenGLTextureRef>,
}

// Safety: TextureCache can be sent between threads when properly synchronized
// The actual OpenGL operations must happen on the render thread
unsafe impl Send for TextureCache {}

impl TextureCache {
    /// Create a new texture cache using the current CGL context
    pub fn new() -> Result<Self, String> {
        unsafe {
            let cgl_context = CGLGetCurrentContext();
            if cgl_context.is_null() {
                return Err("No current CGL context".to_string());
            }

            let cgl_pixel_format = CGLGetPixelFormat(cgl_context);

            let mut cache: CVOpenGLTextureCacheRef = std::ptr::null_mut();
            let result = CVOpenGLTextureCacheCreate(
                std::ptr::null_mut(), // allocator
                std::ptr::null_mut(), // cache attributes
                cgl_context,
                cgl_pixel_format,
                std::ptr::null_mut(), // texture attributes
                &mut cache,
            );

            if result != K_CV_RETURN_SUCCESS || cache.is_null() {
                return Err(format!("CVOpenGLTextureCacheCreate failed: {}", result));
            }

            log::info!("[TextureCache] Created CVOpenGLTextureCache for zero-copy GPU textures");

            Ok(Self {
                cache,
                current_texture: None,
            })
        }
    }

    /// Create an OpenGL texture from a CVPixelBuffer (zero-copy if IOSurface-backed)
    /// Returns (texture_name, texture_target, width, height)
    pub fn create_texture_from_pixel_buffer(
        &mut self,
        pixel_buffer: CVPixelBufferRef,
    ) -> Result<(GLuint, GLenum, u32, u32), String> {
        if pixel_buffer.is_null() {
            return Err("Null pixel buffer".to_string());
        }

        unsafe {
            // Check if this CVPixelBuffer is IOSurface-backed
            let io_surface = CVPixelBufferGetIOSurface(pixel_buffer);
            if io_surface.is_null() {
                return Err("CVPixelBuffer is not IOSurface-backed".to_string());
            }

            // Get dimensions
            let width = CVPixelBufferGetWidth(pixel_buffer) as u32;
            let height = CVPixelBufferGetHeight(pixel_buffer) as u32;

            // Release previous texture if any
            if let Some(old_tex) = self.current_texture.take() {
                CVOpenGLTextureRelease(old_tex);
            }

            // Create new texture from pixel buffer
            let mut texture: CVOpenGLTextureRef = std::ptr::null_mut();
            let result = CVOpenGLTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(), // allocator
                self.cache,
                pixel_buffer,
                std::ptr::null_mut(), // texture attributes
                &mut texture,
            );

            if result != K_CV_RETURN_SUCCESS || texture.is_null() {
                return Err(format!("CVOpenGLTextureCacheCreateTextureFromImage failed: {}", result));
            }

            // Get OpenGL texture name and target
            let texture_name = CVOpenGLTextureGetName(texture);
            let texture_target = CVOpenGLTextureGetTarget(texture);

            // Store texture reference for cleanup
            self.current_texture = Some(texture);

            Ok((texture_name, texture_target, width, height))
        }
    }

    /// Flush the texture cache to ensure GPU has finished with textures
    pub fn flush(&self) {
        unsafe {
            CVOpenGLTextureCacheFlush(self.cache, 0);
        }
    }

    /// Check if a pixel buffer is IOSurface-backed (zero-copy capable)
    pub fn is_iosurface_backed(pixel_buffer: CVPixelBufferRef) -> bool {
        if pixel_buffer.is_null() {
            return false;
        }
        unsafe {
            let io_surface = CVPixelBufferGetIOSurface(pixel_buffer);
            !io_surface.is_null()
        }
    }
}

impl Drop for TextureCache {
    fn drop(&mut self) {
        unsafe {
            // Release current texture if any
            if let Some(tex) = self.current_texture.take() {
                CVOpenGLTextureRelease(tex);
            }

            // Release the cache
            if !self.cache.is_null() {
                CVOpenGLTextureCacheRelease(self.cache);
            }
        }
        log::info!("[TextureCache] Released CVOpenGLTextureCache");
    }
}

/// Wrapper for a retained CVPixelBuffer that can be passed between threads
pub struct RetainedPixelBuffer {
    buffer: CVPixelBufferRef,
    width: u32,
    height: u32,
}

// Safety: CVPixelBuffer is reference-counted and thread-safe when retained
unsafe impl Send for RetainedPixelBuffer {}
unsafe impl Sync for RetainedPixelBuffer {}

impl RetainedPixelBuffer {
    /// Create a retained pixel buffer (increments reference count)
    pub fn new(buffer: CVPixelBufferRef) -> Option<Self> {
        if buffer.is_null() {
            return None;
        }
        unsafe {
            // Retain the buffer so it stays alive
            CVPixelBufferRetain(buffer);
            let width = CVPixelBufferGetWidth(buffer) as u32;
            let height = CVPixelBufferGetHeight(buffer) as u32;
            Some(Self { buffer, width, height })
        }
    }

    /// Get the raw pixel buffer pointer
    pub fn as_ptr(&self) -> CVPixelBufferRef {
        self.buffer
    }

    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Check if IOSurface-backed
    pub fn is_iosurface_backed(&self) -> bool {
        TextureCache::is_iosurface_backed(self.buffer)
    }
}

impl Drop for RetainedPixelBuffer {
    fn drop(&mut self) {
        unsafe {
            CVPixelBufferRelease(self.buffer);
        }
    }
}

impl Clone for RetainedPixelBuffer {
    fn clone(&self) -> Self {
        unsafe {
            CVPixelBufferRetain(self.buffer);
        }
        Self {
            buffer: self.buffer,
            width: self.width,
            height: self.height,
        }
    }
}
