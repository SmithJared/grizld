//! CVMetalTextureCache wrapper for zero-copy video rendering
//!
//! This module provides a safe wrapper around Core Video's CVMetalTextureCache,
//! which converts CVPixelBuffer frames into Metal textures without copying memory.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::{
    CVImageBuffer, CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture,
    kCVReturnSuccess,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};
use std::ptr::NonNull;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetalTextureCacheError {
    #[error("Failed to create texture cache: {0}")]
    CacheCreation(i32),
    #[error("Failed to create texture from pixel buffer: {0}")]
    TextureCreation(i32),
    #[error("Null pointer returned")]
    NullPointer,
}

/// Safe wrapper around CVMetalTexture
pub struct MetalTexture(Retained<CVMetalTexture>);

impl MetalTexture {
    /// Get the underlying Metal texture
    pub fn texture(&self) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
        CVMetalTextureGetTexture(&self.0)
    }

    /// Get a reference to the underlying CVMetalTexture
    pub fn as_cv_metal_texture(&self) -> &Retained<CVMetalTexture> {
        &self.0
    }
}

/// Safe wrapper around CVMetalTextureCache
///
/// CVMetalTextureCache creates Metal textures directly from CVPixelBuffer,
/// enabling zero-copy video rendering. This is essential for hardware-accelerated
/// video playback.
pub struct MetalTextureCacheWrapper(Retained<CVMetalTextureCache>);

impl MetalTextureCacheWrapper {
    /// Create a new texture cache for the given Metal device
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self, MetalTextureCacheError> {
        unsafe {
            let mut cache_ptr: *mut CVMetalTextureCache = std::ptr::null_mut();
            let cache_out = NonNull::from(&mut cache_ptr);

            let status = CVMetalTextureCache::create(
                None, // default allocator
                None, // cache attributes
                device,
                None, // texture attributes
                cache_out,
            );

            if status != kCVReturnSuccess {
                return Err(MetalTextureCacheError::CacheCreation(status));
            }

            Retained::from_raw(cache_ptr)
                .map(Self)
                .ok_or(MetalTextureCacheError::NullPointer)
        }
    }

    /// Create a Metal texture from a CVPixelBuffer plane
    ///
    /// # Arguments
    /// * `pixel_buffer` - The CVPixelBuffer to create texture from
    /// * `plane_index` - Plane index (0 for Y, 1 for UV in NV12/420v format)
    /// * `pixel_format` - Metal pixel format (e.g., R8Unorm for Y, RG8Unorm for UV)
    /// * `width` - Texture width
    /// * `height` - Texture height
    ///
    /// # Example
    /// ```ignore
    /// // For 420v (NV12) pixel buffer:
    /// // Y plane (luma)
    /// let y_texture = cache.create_texture_from_buffer(
    ///     pixel_buffer,
    ///     0,
    ///     MTLPixelFormat::R8Unorm,
    ///     width,
    ///     height
    /// )?;
    ///
    /// // UV plane (chroma)
    /// let uv_texture = cache.create_texture_from_buffer(
    ///     pixel_buffer,
    ///     1,
    ///     MTLPixelFormat::RG8Unorm,
    ///     width / 2,
    ///     height / 2
    /// )?;
    /// ```
    pub fn create_texture_from_buffer(
        &self,
        pixel_buffer: &CVImageBuffer,
        plane_index: usize,
        pixel_format: MTLPixelFormat,
        width: usize,
        height: usize,
    ) -> Result<MetalTexture, MetalTextureCacheError> {
        unsafe {
            let mut texture_ptr: *mut CVMetalTexture = std::ptr::null_mut();
            let texture_out = NonNull::from(&mut texture_ptr);

            let status = CVMetalTextureCache::create_texture_from_image(
                None,         // default allocator
                &self.0,      // texture cache
                pixel_buffer, // source pixel buffer
                None,         // texture attributes
                pixel_format,
                width,
                height,
                plane_index,
                texture_out,
            );

            if status != kCVReturnSuccess {
                return Err(MetalTextureCacheError::TextureCreation(status));
            }

            Retained::from_raw(texture_ptr)
                .map(MetalTexture)
                .ok_or(MetalTextureCacheError::NullPointer)
        }
    }

    /// Flush the texture cache
    ///
    /// Call this periodically to recycle unused texture objects.
    /// Typically called once per frame or when memory pressure is high.
    pub fn flush(&self) {
        CVMetalTextureCache::flush(&self.0, 0);
    }
}

unsafe impl Send for MetalTextureCacheWrapper {}
unsafe impl Sync for MetalTextureCacheWrapper {}
