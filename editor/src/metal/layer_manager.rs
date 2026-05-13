//! CAMetalLayer management and window integration.

use super::{MetalContext, MetalError};
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_core_foundation::{CGRect, CGSize, CGPoint};
use objc2_metal::{
    MTLPixelFormat, MTLClearColor,
    MTLCommandBuffer, MTLCommandEncoder,
    MTLDrawable,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Configuration for the Metal layer.
#[derive(Debug, Clone)]
pub struct LayerConfig {
    /// Pixel format for the layer (typically BGRA8Unorm).
    pub pixel_format: MTLPixelFormat,
    /// Whether the layer's textures are only used for rendering (optimization).
    pub framebuffer_only: bool,
    /// Display sync enabled (vsync).
    pub display_sync_enabled: bool,
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self {
            pixel_format: MTLPixelFormat::BGRA8Unorm,
            framebuffer_only: true,
            display_sync_enabled: true,
        }
    }
}

/// Manages the CAMetalLayer and its integration with the window.
pub struct LayerManager {
    layer: Retained<CAMetalLayer>,
    view: Retained<NSView>,
    context: MetalContext,
}

impl LayerManager {
    /// Creates a new LayerManager and attaches a Metal layer to the given window.
    ///
    /// # Arguments
    /// * `window` - Window with a raw window handle (from eframe/winit)
    /// * `config` - Layer configuration
    pub fn new(
        window: &impl HasWindowHandle,
        config: LayerConfig,
    ) -> Result<Self, MetalError> {
        // Initialize Metal context first
        let context = MetalContext::new()?;

        // Extract NSView from the window handle
        let view = Self::get_nsview_from_window(window)?;

        // Create and configure the Metal layer
        let layer = unsafe { CAMetalLayer::new() };

        // Configure layer
        unsafe {
            layer.setDevice(Some(context.device()));
            layer.setPixelFormat(config.pixel_format);
            layer.setFramebufferOnly(config.framebuffer_only);
            layer.setDisplaySyncEnabled(config.display_sync_enabled);

            // Set fully opaque for production video rendering
            layer.setOpaque(true);
            layer.setOpacity(1.0);

            // Add as sublayer instead of replacing the view's layer
            // This allows it to coexist with eframe's rendering
            view.setWantsLayer(true);
            if let Some(root_layer) = view.layer() {
                // Add our Metal layer on top of the existing layer
                root_layer.addSublayer(&layer);

                // Start with zero size - will be positioned by app.rs
                layer.setFrame(CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize { width: 0.0, height: 0.0 },
                });
            } else {
                tracing::warn!("View has no layer, setting Metal layer as main layer");
                view.setLayer(Some(&layer));
            }
        }

        tracing::info!(
            "CAMetalLayer created and attached to NSView ({}x{})",
            view.frame().size.width,
            view.frame().size.height
        );

        Ok(Self {
            layer,
            view,
            context,
        })
    }

    /// Extracts NSView from a window handle.
    fn get_nsview_from_window(
        window: &impl HasWindowHandle,
    ) -> Result<Retained<NSView>, MetalError> {
        let window_handle = window
            .window_handle()
            .map_err(|_| MetalError::NoView)?;

        match window_handle.as_raw() {
            RawWindowHandle::AppKit(handle) => {
                // Safety: We're on macOS and have a valid AppKit window handle
                let view_ptr = handle.ns_view.as_ptr() as *mut NSView;
                let view = unsafe {
                    Retained::retain(view_ptr)
                        .ok_or(MetalError::NoView)?
                };
                Ok(view)
            }
            _ => Err(MetalError::InvalidWindowHandle),
        }
    }

    /// Updates the layer bounds to match a specific rect.
    ///
    /// Use this to position the video layer within the window
    /// (e.g., centered viewport for video playback).
    pub fn set_bounds(&self, x: f64, y: f64, width: f64, height: f64) {
        let rect = CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        };

        self.layer.setFrame(rect);

        tracing::trace!("Metal layer bounds updated: {:?}", rect);
    }

    /// Resizes the layer to fill the entire view.
    pub fn resize_to_fit_view(&self) {
        let view_frame = self.view.frame();
        self.layer.setFrame(view_frame);
        tracing::debug!(
            "Metal layer resized to view: {}x{}",
            view_frame.size.width,
            view_frame.size.height
        );
    }

    /// Returns the Metal context.
    pub fn context(&self) -> &MetalContext {
        &self.context
    }

    /// Returns the CAMetalLayer.
    pub fn layer(&self) -> &Retained<CAMetalLayer> {
        &self.layer
    }

    /// Renders a test frame with a solid color.
    ///
    /// This is useful for validating that the Metal layer is working.
    /// Call this method to see if the layer is visible.
    pub fn render_test_color(&self, r: f64, g: f64, b: f64, a: f64) {
        unsafe {
            // Get the next drawable from the layer
            let Some(drawable) = self.layer.nextDrawable() else {
                tracing::warn!("Failed to get next drawable");
                return;
            };

            // Create a command buffer
            use objc2::msg_send_id;
            let command_buffer: Option<Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLCommandBuffer>>> =
                msg_send_id![self.context.command_queue(), commandBuffer];

            let Some(command_buffer) = command_buffer else {
                tracing::warn!("Failed to create command buffer");
                return;
            };

            // Create render pass descriptor with clear color
            let render_pass_descriptor = objc2_metal::MTLRenderPassDescriptor::new();
            let color_attachment = render_pass_descriptor
                .colorAttachments()
                .objectAtIndexedSubscript(0);

            color_attachment.setTexture(Some(&drawable.texture()));
            color_attachment.setLoadAction(objc2_metal::MTLLoadAction::Clear);
            color_attachment.setStoreAction(objc2_metal::MTLStoreAction::Store);
            color_attachment.setClearColor(MTLClearColor { red: r, green: g, blue: b, alpha: a });

            // Create render command encoder
            let Some(render_encoder) = command_buffer
                .renderCommandEncoderWithDescriptor(&render_pass_descriptor) else {
                tracing::warn!("Failed to create render encoder");
                return;
            };

            // End encoding (we're just clearing, no actual drawing)
            render_encoder.endEncoding();

            // Present drawable and commit
            // Cast CAMetalDrawable to MTLDrawable (CAMetalDrawable conforms to MTLDrawable)
            use objc2::runtime::ProtocolObject;
            let drawable_ref: &ProtocolObject<dyn CAMetalDrawable> = &*drawable;
            let drawable_as_mtl: &ProtocolObject<dyn MTLDrawable> = std::mem::transmute(drawable_ref);
            command_buffer.presentDrawable(drawable_as_mtl);
            command_buffer.commit();
        }

        tracing::trace!("Test color rendered: rgba({}, {}, {}, {})", r, g, b, a);
    }
}
