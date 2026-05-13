//! Video renderer using Metal for hardware-accelerated rendering
//!
//! This renderer takes hardware-decoded video frames (CVPixelBuffer) and
//! renders them to a Metal layer using zero-copy texture mapping.

use super::{MetalContext, MetalError};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferGetPixelFormatType,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::CAMetalLayer;
use vp_core::types::{MetalTexture, MetalTextureCacheWrapper, PixelBuffer};

/// Video renderer that converts CVPixelBuffer to Metal textures
pub struct VideoRenderer {
    texture_cache: MetalTextureCacheWrapper,
    _context: MetalContext,
}

impl VideoRenderer {
    /// Create a new video renderer
    pub fn new(context: MetalContext) -> Result<Self, MetalError> {
        let device = context.device();

        // Create texture cache for zero-copy texture creation
        let texture_cache = MetalTextureCacheWrapper::new(device)
            .map_err(|e| {
                tracing::error!("Failed to create texture cache: {}", e);
                MetalError::LayerCreation
            })?;

        tracing::info!("Video renderer initialized with texture cache");

        Ok(Self {
            texture_cache,
            _context: context,
        })
    }

    /// Convert a CVPixelBuffer to Metal textures
    ///
    /// Returns (Y texture, UV texture) for YUV 420v format, or None if conversion fails.
    /// The Y texture is R8Unorm (single channel), UV texture is RG8Unorm (two channels).
    pub fn create_textures_from_pixel_buffer(
        &self,
        pixel_buffer: &PixelBuffer,
    ) -> Option<(MetalTexture, MetalTexture)> {
        let width = CVPixelBufferGetWidth(pixel_buffer.inner()) as usize;
        let height = CVPixelBufferGetHeight(pixel_buffer.inner()) as usize;
        let format = CVPixelBufferGetPixelFormatType(pixel_buffer.inner());

        // Check if this is a YUV 420v (NV12) format
        if format != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
            tracing::warn!(
                "Unsupported pixel format: {} (expected 420v)",
                pixel_buffer.pixel_format_name()
            );
            return None;
        }

        // Create Y plane texture (luma, plane 0)
        let y_texture = self
            .texture_cache
            .create_texture_from_buffer(
                pixel_buffer.inner(),
                0, // plane 0 = Y
                MTLPixelFormat::R8Unorm,
                width,
                height,
            )
            .ok()?;

        // Create UV plane texture (chroma, plane 1)
        // UV plane is half resolution (subsampled)
        let uv_texture = self
            .texture_cache
            .create_texture_from_buffer(
                pixel_buffer.inner(),
                1, // plane 1 = UV
                MTLPixelFormat::RG8Unorm,
                width / 2,
                height / 2,
            )
            .ok()?;

        tracing::trace!(
            "Created Metal textures from {}x{} pixel buffer (format: {})",
            width,
            height,
            pixel_buffer.pixel_format_name()
        );

        Some((y_texture, uv_texture))
    }

    /// Render a hardware frame to the Metal layer
    ///
    /// This is a placeholder that will be implemented in Phase 4 with the YUV shader.
    /// For now, it just creates the textures to verify zero-copy conversion works.
    pub fn render_frame(
        &self,
        pixel_buffer: &PixelBuffer,
        _layer: &CAMetalLayer,
    ) -> Result<(), MetalError> {
        // Convert pixel buffer to Metal textures (zero-copy)
        let Some((y_texture, uv_texture)) = self.create_textures_from_pixel_buffer(pixel_buffer)
        else {
            tracing::warn!("Failed to create textures from pixel buffer");
            return Err(MetalError::LayerCreation);
        };

        // Verify textures were created
        if y_texture.texture().is_none() || uv_texture.texture().is_none() {
            tracing::warn!("Texture extraction failed");
            return Err(MetalError::LayerCreation);
        }

        // TODO Phase 4: Render YUV textures with shader
        // For now, just log that we successfully created the textures
        static FIRST_FRAME: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(true);
        if FIRST_FRAME.swap(false, std::sync::atomic::Ordering::Relaxed) {
            tracing::info!("Successfully created Metal textures from hardware frame (zero-copy)");
        }

        Ok(())
    }

    /// Flush the texture cache
    ///
    /// Call this periodically (e.g., once per frame) to recycle unused textures.
    pub fn flush_cache(&self) {
        self.texture_cache.flush();
    }
}
