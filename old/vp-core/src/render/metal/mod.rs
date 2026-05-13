//! Metal texture bridge for CVPixelBuffer
//!
//! This module provides zero-copy conversion from VideoToolbox's CVPixelBuffer
//! to Metal textures, enabling efficient GPU-to-GPU frame transfer.

mod renderer;
pub use renderer::*;

use objc2_metal::MTLPixelFormat;
use video_sys::core_video::{MetalTexture, MetalTextureCache};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGRect;
use objc2_quartz_core::CALayer;
use thiserror::Error;
use video_sys::metal::MetalLayer;

use crate::frame::{FrameData, VideoFrame};


#[derive(Debug, Error)]
pub enum MetalBridgeError {
    #[error("Failed to attach layer to view")]
    LayerAttachment,
    #[error("Invalid view")]
    InvalidView,
    #[error("Failed to get root layer")]
    NoRootLayer,
}

/// Bridge for NSView and CAMetalLayer integration.
pub struct MetalLayerBridge;

impl MetalLayerBridge {
    /// Attaches a CAMetalLayer to an NSView.
    ///
    /// This function configures the NSView for layer backing and adds the Metal layer
    /// as a sublayer to the view's root layer.
    ///
    /// # Panics
    /// This function must be called on the main thread (AppKit requirement).
    /// Calling from a background thread may cause undefined behavior.
    ///
    /// # Errors
    /// Returns an error if the layer cannot be attached or the view is invalid.
    pub fn attach_layer_to_view(
        view: &NSView,
        layer: &MetalLayer,
    ) -> Result<(), MetalBridgeError> {
        // Convert NSView to AnyObject for msg_send
        let view_obj = view as *const NSView as *const AnyObject;

        // Enable layer backing on the NSView
        // SAFETY: view_obj is a valid pointer derived from a safe &NSView reference
        unsafe {
            let _: () = msg_send![view_obj, setWantsLayer: true];
            let _: () = msg_send![view_obj, setLayerContentsRedrawPolicy: 2_isize]; // DuringViewResize
        }

        // Get the root layer from the view
        // SAFETY: Calling layer on a valid NSView with layer backing enabled is safe
        let root_layer_ptr: *mut AnyObject = unsafe { msg_send![view_obj, layer] };
        if root_layer_ptr.is_null() {
            return Err(MetalBridgeError::NoRootLayer);
        }

        // Convert to Retained CALayer
        // SAFETY: root_layer_ptr is non-null and points to a valid CALayer
        let root_layer = unsafe { Retained::<CALayer>::retain(root_layer_ptr.cast()) }
            .ok_or(MetalBridgeError::LayerAttachment)?;

        // Add the Metal layer as a sublayer
        root_layer.addSublayer(layer.as_layer());

        // Set initial frame to match view bounds
        let bounds: CGRect = Self::get_view_bounds(view);
        layer.set_frame(bounds);
        
        Ok(())
    }
    
    /// Gets the bounds of an NSView.
    /// SAFETY: view_obj is valid, calling bounds is a standard AppKit operation
    ///
    /// # Panics
    /// This function must be called on the main thread (AppKit requirement).
    pub fn get_view_bounds(view: &NSView) -> CGRect {
        // 1. Cast the safe Rust reference to a raw pointer the runtime understands
        let view_obj = view as *const NSView as *const AnyObject;
        // 2. SAFETY: view_obj is a valid pointer derived from a safe &NSView reference
        unsafe { msg_send![view_obj, bounds] }
    }
}

#[derive(Debug, Error)]
pub enum TextureError {
    #[error("Invalid frame data")]
    InvalidFrameData,
    #[error("Core Video error: {0}")]
    CoreVideoError(#[from] video_sys::core_video::CoreVideoError),
}

/// A pair of Metal textures representing P210 format (10-bit 4:2:2 YUV with Y and UV planes)
/// Note: Despite the name, this struct handles P210 (10-bit 4:2:2) not NV12 (8-bit 4:2:0)
pub struct P210Textures {
    /// Y plane (luminance) - R16Unorm format (10-bit data)
    pub y_texture: MetalTexture,

    /// UV plane (chrominance) - RG16Unorm format (10-bit U and V, 4:2:2 subsampling)
    pub uv_texture: MetalTexture,

    /// Width of the full frame
    pub width: u32,

    /// Height of the full frame
    pub height: u32,
}

impl P210Textures {
    /// Creates P210 (10-bit 4:2:2 YUV) textures from a CVPixelBuffer
    ///
    /// # Arguments
    /// * `cache` - The Metal texture cache to use
    /// * `frame` - The VideoFrame containing P210 format data
    pub fn from_video_frame(
        cache: &MetalTextureCache,
        frame: &VideoFrame,
    ) -> Result<Self, TextureError> {
        let pixel_buffer = match frame.data() {
            FrameData::CVPixelBuffer { buffer } => buffer,
            _ => return Err(TextureError::InvalidFrameData),
        };
        let width = frame.width();
        let height = frame.height();
        // Create Y plane texture (full resolution, R16 for 10-bit data)
        let y_texture = cache.create_texture_from_buffer(
            pixel_buffer,
            0,  // plane 0 = Y
            MTLPixelFormat::R16Unorm,  // 16-bit per pixel (10-bit data + padding)
            width as usize,
            height as usize,
        )?;

        // Create UV plane texture (4:2:2 subsampling: half width, FULL height, RG16)
        let uv_texture = cache.create_texture_from_buffer(
            pixel_buffer,
            1,  // plane 1 = UV
            MTLPixelFormat::RG16Unorm,  // 16-bit per U and V component
            (width / 2) as usize,
            height as usize,  // FULL height for 4:2:2 (not height/2!)
        )?;

        Ok(Self {
            y_texture,
            uv_texture,
            width,
            height,
        })
    }
}