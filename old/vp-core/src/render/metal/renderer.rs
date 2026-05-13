//! Independent Metal renderer for video playback.
//!
//! This module provides a dedicated Metal rendering pipeline that operates
//! alongside the wgpu/egui renderer. It uses CVDisplayLink for display-synchronized
//! rendering and directly consumes CVPixelBuffers from VideoToolbox.

use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject};
use objc2_app_kit::NSView;
use objc2_foundation::{ns_string, NSString, NSUInteger};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCompileOptions, MTLDevice,
    MTLDrawable, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState,
    MTLRenderPipelineDescriptor, MTLResourceOptions, MTLStoreAction, MTLTexture,
    MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction, MTLBuffer, MTLClearColor,
};
use objc2_quartz_core::{CAMetalDrawable};
use video_sys::core_video::{MetalTextureCache};
use video_sys::metal::{MetalDevice, MetalCommandQueue, MetalLayer, MetalLayerConfig};
use std::ffi::c_void;
use std::ptr::NonNull;

use crate::frame::VideoFrame;
use crate::render::layout::VideoViewport;
use super::{P210Textures, TextureError, MetalLayerBridge};

#[derive(Debug, thiserror::Error)]
pub enum MetalRendererError {
    #[error("Failed to create Metal device")]
    DeviceCreation,
    #[error("Failed to create command queue")]
    CommandQueueCreation,
    #[error("Failed to compile shader: {0}")]
    ShaderCompilation(String),
    #[error("Failed to create render pipeline: {0}")]
    PipelineCreation(String),
    #[error("Failed to create buffer")]
    BufferCreation,
    #[error("Failed to get next drawable")]
    DrawableError,
    #[error("Core Video error: {0}")]
    CoreVideo(#[from] video_sys::core_video::CoreVideoError),
    #[error("Texture error: {0}")]
    TextureError(#[from] TextureError),
}

pub struct MetalVideoRenderer {
    _device: MetalDevice,
    command_queue: MetalCommandQueue,
    metal_layer: MetalLayer,
    pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    vertex_buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    texture_cache: MetalTextureCache,
    _frame_count: std::sync::atomic::AtomicU64,
    viewport: Option<VideoViewport>,
    drawable_size: (u32, u32),
}

#[repr(C)]
struct Vertex {
    position: [f32; 2],
    tex_coord: [f32; 2],
}

impl MetalVideoRenderer {
    pub fn new(view: &NSView, width: u32, height: u32) -> Result<Self, MetalRendererError> {
        // 1. Create Device
        let device = MetalDevice::system_default()
            .map_err(|_| MetalRendererError::DeviceCreation)?;

        // 2. Create Command Queue
        let command_queue = MetalCommandQueue::new(&device)
            .map_err(|_| MetalRendererError::CommandQueueCreation)?;

        // 3. Create Metal Layer
        let metal_layer = MetalLayer::new(&device, MetalLayerConfig {
            pixel_format: MTLPixelFormat::BGRA8Unorm,
            drawable_size: (width as f64, height as f64),
            display_sync_enabled: true,
            presents_with_transaction: false,
        });

        // Additional layer configuration
        metal_layer.as_layer().setContentsGravity(ns_string!("resizeAspect"));
        metal_layer.as_layer().setZPosition(-1.0); // Place video layer below GUI

        // 4. Attach to NSView
        MetalLayerBridge::attach_layer_to_view(view, &metal_layer)
            .map_err(|_| MetalRendererError::DeviceCreation)?;

        // 5. Compile Shaders
        let shader_source = include_str!("../shaders/p210_to_rgb.metal");
        let shader_nsstring = NSString::from_str(shader_source);
        let options = MTLCompileOptions::new();
        let library = device.as_device()
            .newLibraryWithSource_options_error(&shader_nsstring, Some(&options))
            .map_err(|e| MetalRendererError::ShaderCompilation(e.to_string()))?;

        let vertex_fn = library
            .newFunctionWithName(ns_string!("vertex_main"))
            .ok_or(MetalRendererError::ShaderCompilation("Missing vertex_main".into()))?;
        let fragment_fn = library
            .newFunctionWithName(ns_string!("fragment_main"))
            .ok_or(MetalRendererError::ShaderCompilation("Missing fragment_main".into()))?;

        // 6. Pipeline Descriptor
        let pipeline_desc = MTLRenderPipelineDescriptor::new();
        pipeline_desc.setVertexFunction(Some(&vertex_fn));
        pipeline_desc.setFragmentFunction(Some(&fragment_fn));
        
        let color_attachments = pipeline_desc.colorAttachments();
        let attachment0 = unsafe { color_attachments.objectAtIndexedSubscript(0) };
        attachment0.setPixelFormat(MTLPixelFormat::BGRA8Unorm);

        // 7. Vertex Descriptor
        let vertex_desc = MTLVertexDescriptor::new();
        
        // Attr 0: Position
        let attr0 = unsafe { vertex_desc.attributes().objectAtIndexedSubscript(0) };
        attr0.setFormat(MTLVertexFormat::Float2);
        unsafe {
            attr0.setOffset(0);
            attr0.setBufferIndex(0);
        }

        // Attr 1: TexCoord
        let attr1 = unsafe { vertex_desc.attributes().objectAtIndexedSubscript(1) };
        attr1.setFormat(MTLVertexFormat::Float2);
        unsafe {
            attr1.setOffset(8); // 2 * f32
            attr1.setBufferIndex(0);
        }

        // Layout
        let layout0 = unsafe { vertex_desc.layouts().objectAtIndexedSubscript(0) };
        unsafe {
            layout0.setStride(16); // 4 * f32
            layout0.setStepRate(1);
        }
        layout0.setStepFunction(MTLVertexStepFunction::PerVertex);

        pipeline_desc.setVertexDescriptor(Some(&vertex_desc));

        let pipeline_state = device.as_device()
            .newRenderPipelineStateWithDescriptor_error(&pipeline_desc)
            .map_err(|e| MetalRendererError::PipelineCreation(e.to_string()))?;

        // 8. Vertex Buffer
        let vertices = [
            Vertex { position: [-1.0, -1.0], tex_coord: [0.0, 1.0] },
            Vertex { position: [1.0, -1.0], tex_coord: [1.0, 1.0] },
            Vertex { position: [-1.0, 1.0], tex_coord: [0.0, 0.0] },
            Vertex { position: [1.0, 1.0], tex_coord: [1.0, 0.0] },
        ];

        let vertex_buffer = unsafe { device.as_device().newBufferWithBytes_length_options(
            NonNull::new(vertices.as_ptr() as *mut c_void).unwrap(),
            (vertices.len() * std::mem::size_of::<Vertex>()) as NSUInteger,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        )}.ok_or(MetalRendererError::BufferCreation)?;

        // 9. Texture Cache
        let texture_cache = MetalTextureCache::new(device.as_device())?;

        Ok(Self {
            _device: device,
            command_queue,
            metal_layer,
            pipeline_state,
            vertex_buffer,
            texture_cache,
            _frame_count: std::sync::atomic::AtomicU64::new(0),
            viewport: None,
            drawable_size: (width, height),
        })
    }

    /// Updates the renderer with a new viewport configuration.
    ///
    /// The viewport should be constructed by the caller with the desired window
    /// and video dimensions. This method applies the viewport to the Metal layer,
    /// handling coordinate conversions and scale factors.
    ///
    /// # Arguments
    /// * `viewport` - The pre-calculated viewport to apply
    pub fn resize(&mut self, viewport: &VideoViewport) {

        // Update Metal layer frame to match viewport
        let frame_pixels = viewport.to_metal_frame();

        // I needed the below for the video to render at the correct size before I added a side panels.
        // Now I don't need it?

        // Convert frame from pixels to points for CALayer
        // Get the scale factor from the parent layer
        // let scale_factor = if let Some(superlayer) = self.metal_layer.as_layer().superlayer() {
        //     superlayer.contentsScale()
        // } else {
        //     2.0 // Default to Retina scale
        // };

        // let frame_points = CGRect {
        //     origin: CGPoint {
        //         x: frame_pixels.origin.x / scale_factor,
        //         y: frame_pixels.origin.y / scale_factor,
        //     },
        //     size: CGSize {
        //         width: frame_pixels.size.width / scale_factor,
        //         height: frame_pixels.size.height / scale_factor,
        //     },
        // };

        // self.metal_layer.set_frame(frame_points);
        self.metal_layer.set_frame(frame_pixels);

        // Verify the frame was actually set
        let _actual_frame = self.metal_layer.frame();

        // Set drawable size to match viewport dimensions
        let (_, _, vp_width, vp_height) = viewport.dimensions();

        self.metal_layer.set_drawable_size(vp_width as f64, vp_height as f64);

        // IMPORTANT: Update drawable_size to match the viewport, not the window
        self.drawable_size = (vp_width as u32, vp_height as u32);

        self.viewport = Some(viewport.clone());
    }

    /// Returns the current viewport, if set.
    pub fn viewport(&self) -> Option<&VideoViewport> {
        self.viewport.as_ref()
    }


    pub fn render_frame(
        &self,
        frame: &VideoFrame,
    ) -> Result<(), MetalRendererError> {
        // 1. Get Drawable
        let drawable = self.metal_layer.next_drawable().ok_or(MetalRendererError::DrawableError)?;

        let drawable_texture = drawable.texture();
        if drawable_texture.width() as u32 != self.drawable_size.0 {
            // Handle mismatch during resize, skip frame
            return Ok(());
        }

        // 2. Create P210 (10-bit 4:2:2 YUV) textures from the CVPixelBuffer
        let p210_textures = P210Textures::from_video_frame(&self.texture_cache, frame)?;

        // 3. Command Buffer
        let command_buffer = self.command_queue.as_queue().commandBuffer()
            .ok_or_else(|| MetalRendererError::PipelineCreation("Failed to create command buffer".into()))?;

        // 4. Render Pass Descriptor
        let pass_desc = MTLRenderPassDescriptor::new();
        let color_attachment = unsafe { pass_desc.colorAttachments().objectAtIndexedSubscript(0) };

        color_attachment.setTexture(Some(&drawable_texture));
        color_attachment.setLoadAction(MTLLoadAction::Clear);
        color_attachment.setClearColor(MTLClearColor {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        });
        color_attachment.setStoreAction(MTLStoreAction::Store);

        // 5. Encoding
        let encoder = command_buffer.renderCommandEncoderWithDescriptor(&pass_desc)
            .ok_or_else(|| MetalRendererError::PipelineCreation("Failed to create render encoder".into()))?;
        encoder.setRenderPipelineState(&self.pipeline_state);
        unsafe { encoder.setVertexBuffer_offset_atIndex(Some(&self.vertex_buffer), 0, 0) };

        // Set the Y and UV textures for the P210 format
        let y_texture = p210_textures.y_texture.texture();
        let uv_texture = p210_textures.uv_texture.texture();

        unsafe { encoder.setFragmentTexture_atIndex(y_texture.as_deref(), 0) };
        unsafe { encoder.setFragmentTexture_atIndex(uv_texture.as_deref(), 1) };

        unsafe { encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4) };
        (&*encoder).endEncoding();

        // 6. Present and commit
        // CAMetalDrawable conforms to MTLDrawable, cast to the base protocol
        let drawable_as_mtl: &ProtocolObject<dyn MTLDrawable> =
            unsafe { &*(drawable.as_drawable() as *const ProtocolObject<dyn CAMetalDrawable> as *const ProtocolObject<dyn MTLDrawable>) };
        command_buffer.presentDrawable(drawable_as_mtl);
        command_buffer.commit();

        Ok(())
    }
}

// Implement Drop to remove layer
impl Drop for MetalVideoRenderer {
    fn drop(&mut self) {
        self.metal_layer.remove_from_superlayer();
    }
}
