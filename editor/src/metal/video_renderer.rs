//! Video renderer using Metal for hardware-accelerated rendering
//!
//! This renderer takes hardware-decoded video frames (CVPixelBuffer) and
//! renders them to a Metal layer using zero-copy texture mapping with YUV to RGB conversion.

use super::{MetalContext, MetalError};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferGetPixelFormatType,
};
use objc2_foundation::{ns_string, NSString};
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCompileOptions, MTLDevice,
    MTLDrawable, MTLLibrary, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLResourceOptions, MTLStoreAction, MTLVertexDescriptor,
    MTLVertexFormat, MTLVertexStepFunction,
};
use objc2_quartz_core::CAMetalLayer;
use vp_core::types::{MetalTexture, MetalTextureCacheWrapper, PixelBuffer};

/// Vertex data for full-screen quad
#[repr(C)]
struct Vertex {
    position: [f32; 2],
    tex_coord: [f32; 2],
}

/// Video renderer that converts CVPixelBuffer to Metal textures and renders them
pub struct VideoRenderer {
    texture_cache: MetalTextureCacheWrapper,
    context: MetalContext,
    pipeline_state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

impl VideoRenderer {
    /// Create a new video renderer with compiled shader and render pipeline
    pub fn new(context: MetalContext) -> Result<Self, MetalError> {
        let device = context.device();

        // Create texture cache for zero-copy texture creation
        let texture_cache = MetalTextureCacheWrapper::new(device).map_err(|e| {
            tracing::error!("Failed to create texture cache: {}", e);
            MetalError::LayerCreation
        })?;

        // Compile shaders from source
        let shader_source = include_str!("shaders/yuv_to_rgb.metal");
        let shader_nsstring = NSString::from_str(shader_source);
        let options = MTLCompileOptions::new();

        let library = device
            .newLibraryWithSource_options_error(&shader_nsstring, Some(&options))
            .map_err(|e| {
                tracing::error!("Failed to compile shader: {}", e);
                MetalError::LayerCreation
            })?;

        // Get shader functions
        let vertex_fn = library
            .newFunctionWithName(ns_string!("vertex_main"))
            .ok_or_else(|| {
                tracing::error!("Missing vertex_main function in shader");
                MetalError::LayerCreation
            })?;

        let fragment_fn = library
            .newFunctionWithName(ns_string!("fragment_main"))
            .ok_or_else(|| {
                tracing::error!("Missing fragment_main function in shader");
                MetalError::LayerCreation
            })?;

        // Create render pipeline descriptor
        let pipeline_desc = MTLRenderPipelineDescriptor::new();
        pipeline_desc.setVertexFunction(Some(&vertex_fn));
        pipeline_desc.setFragmentFunction(Some(&fragment_fn));

        // Configure color attachment
        let color_attachments = pipeline_desc.colorAttachments();
        let attachment0 = unsafe { color_attachments.objectAtIndexedSubscript(0) };
        attachment0.setPixelFormat(MTLPixelFormat::BGRA8Unorm);

        // Create vertex descriptor
        let vertex_desc = MTLVertexDescriptor::new();

        // Attribute 0: Position (float2)
        let attr0 = unsafe { vertex_desc.attributes().objectAtIndexedSubscript(0) };
        attr0.setFormat(MTLVertexFormat::Float2);
        unsafe {
            attr0.setOffset(0);
            attr0.setBufferIndex(0);
        }

        // Attribute 1: TexCoord (float2)
        let attr1 = unsafe { vertex_desc.attributes().objectAtIndexedSubscript(1) };
        attr1.setFormat(MTLVertexFormat::Float2);
        unsafe {
            attr1.setOffset(8); // 2 floats * 4 bytes
            attr1.setBufferIndex(0);
        }

        // Buffer layout
        let layout0 = unsafe { vertex_desc.layouts().objectAtIndexedSubscript(0) };
        unsafe {
            layout0.setStride(16); // 4 floats * 4 bytes
            layout0.setStepRate(1);
        }
        layout0.setStepFunction(MTLVertexStepFunction::PerVertex);

        pipeline_desc.setVertexDescriptor(Some(&vertex_desc));

        // Create pipeline state
        let pipeline_state = device
            .newRenderPipelineStateWithDescriptor_error(&pipeline_desc)
            .map_err(|e| {
                tracing::error!("Failed to create render pipeline: {}", e);
                MetalError::LayerCreation
            })?;

        tracing::info!("Video renderer initialized with YUV shader pipeline");

        Ok(Self {
            texture_cache,
            context,
            pipeline_state,
        })
    }

    /// Convert a CVPixelBuffer to Metal textures
    ///
    /// Returns (Y texture, UV texture) for YUV 420v format, or None if conversion fails.
    pub fn create_textures_from_pixel_buffer(
        &self,
        pixel_buffer: &PixelBuffer,
    ) -> Option<(MetalTexture, MetalTexture)> {
        let width = CVPixelBufferGetWidth(pixel_buffer.inner()) as usize;
        let height = CVPixelBufferGetHeight(pixel_buffer.inner()) as usize;
        let format = CVPixelBufferGetPixelFormatType(pixel_buffer.inner());

        // Check if this is a supported YUV 420 format (NV12)
        // Support both video range (420v) and full range (420f)
        let is_supported = format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;

        if !is_supported {
            tracing::warn!(
                "Unsupported pixel format: {} (expected 420v or 420f)",
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

        Some((y_texture, uv_texture))
    }

    /// Calculate vertex positions for aspect-ratio-correct rendering
    ///
    /// Returns vertex positions in NDC that will letterbox/pillarbox the video
    fn calculate_aspect_correct_vertices(
        video_width: f64,
        video_height: f64,
        drawable_width: f64,
        drawable_height: f64,
    ) -> [Vertex; 6] {
        let video_aspect = video_width / video_height;
        let drawable_aspect = drawable_width / drawable_height;

        // Calculate the scale to fit the video within the drawable
        let (scale_x, scale_y) = if drawable_aspect > video_aspect {
            // Drawable is wider - pillarbox (black bars on sides)
            let scale = video_aspect / drawable_aspect;
            (scale, 1.0)
        } else {
            // Drawable is taller - letterbox (black bars on top/bottom)
            let scale = drawable_aspect / video_aspect;
            (1.0, scale)
        };

        // Create vertices with adjusted positions
        [
            // Triangle 1
            Vertex {
                position: [-scale_x as f32, -scale_y as f32],
                tex_coord: [0.0, 1.0],
            }, // Bottom-left
            Vertex {
                position: [scale_x as f32, -scale_y as f32],
                tex_coord: [1.0, 1.0],
            }, // Bottom-right
            Vertex {
                position: [-scale_x as f32, scale_y as f32],
                tex_coord: [0.0, 0.0],
            }, // Top-left
            // Triangle 2
            Vertex {
                position: [-scale_x as f32, scale_y as f32],
                tex_coord: [0.0, 0.0],
            }, // Top-left
            Vertex {
                position: [scale_x as f32, -scale_y as f32],
                tex_coord: [1.0, 1.0],
            }, // Bottom-right
            Vertex {
                position: [scale_x as f32, scale_y as f32],
                tex_coord: [1.0, 0.0],
            }, // Top-right
        ]
    }

    /// Render a hardware frame to the Metal layer with correct aspect ratio
    pub fn render_frame(
        &self,
        pixel_buffer: &PixelBuffer,
        layer: &CAMetalLayer,
    ) -> Result<(), MetalError> {
        // Convert pixel buffer to Metal textures (zero-copy)
        let Some((y_texture, uv_texture)) = self.create_textures_from_pixel_buffer(pixel_buffer)
        else {
            tracing::warn!("Failed to create textures from pixel buffer");
            return Err(MetalError::LayerCreation);
        };

        // Extract Metal textures
        let Some(y_mtl_texture) = y_texture.texture() else {
            tracing::warn!("Failed to extract Y texture");
            return Err(MetalError::LayerCreation);
        };

        let Some(uv_mtl_texture) = uv_texture.texture() else {
            tracing::warn!("Failed to extract UV texture");
            return Err(MetalError::LayerCreation);
        };

        // Get next drawable from layer
        let Some(drawable) = layer.nextDrawable() else {
            tracing::warn!("Failed to get next drawable");
            return Err(MetalError::LayerCreation);
        };

        // Get drawable size for aspect ratio calculation
        let drawable_size = layer.drawableSize();
        let drawable_width = drawable_size.width;
        let drawable_height = drawable_size.height;

        // Get video dimensions
        let video_width = CVPixelBufferGetWidth(pixel_buffer.inner()) as f64;
        let video_height = CVPixelBufferGetHeight(pixel_buffer.inner()) as f64;

        // Calculate aspect-ratio-correct vertices
        let vertices = Self::calculate_aspect_correct_vertices(
            video_width,
            video_height,
            drawable_width,
            drawable_height,
        );

        // Create dynamic vertex buffer with aspect-correct vertices
        let vertex_data = unsafe {
            std::slice::from_raw_parts(
                vertices.as_ptr() as *const u8,
                std::mem::size_of_val(&vertices),
            )
        };

        let dynamic_vertex_buffer = unsafe {
            self.context.device().newBufferWithBytes_length_options(
                std::ptr::NonNull::new(vertex_data.as_ptr() as *mut _).unwrap(),
                vertex_data.len(),
                MTLResourceOptions::CPUCacheModeDefaultCache,
            )
        }
        .ok_or_else(|| {
            tracing::warn!("Failed to create dynamic vertex buffer");
            MetalError::LayerCreation
        })?;

        // Create command buffer
        use objc2::msg_send_id;
        let command_buffer: Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>> =
            unsafe { msg_send_id![self.context.command_queue(), commandBuffer] };

        let Some(command_buffer) = command_buffer else {
            tracing::warn!("Failed to create command buffer");
            return Err(MetalError::LayerCreation);
        };

        // Create render pass descriptor
        let render_pass_desc = MTLRenderPassDescriptor::new();
        let color_attachment = unsafe {
            render_pass_desc.colorAttachments().objectAtIndexedSubscript(0)
        };

        color_attachment.setTexture(Some(&drawable.texture()));
        color_attachment.setLoadAction(MTLLoadAction::Clear);
        color_attachment.setStoreAction(MTLStoreAction::Store);
        color_attachment.setClearColor(MTLClearColor {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        });

        // Create render command encoder
        let Some(render_encoder) =
            command_buffer.renderCommandEncoderWithDescriptor(&render_pass_desc)
        else {
            tracing::warn!("Failed to create render encoder");
            return Err(MetalError::LayerCreation);
        };

        // Set pipeline state
        render_encoder.setRenderPipelineState(&self.pipeline_state);

        // Set dynamic vertex buffer (with aspect-correct vertices)
        unsafe {
            render_encoder.setVertexBuffer_offset_atIndex(Some(&dynamic_vertex_buffer), 0, 0);
        }

        // Set textures
        unsafe {
            render_encoder.setFragmentTexture_atIndex(Some(&y_mtl_texture), 0); // Y texture
            render_encoder.setFragmentTexture_atIndex(Some(&uv_mtl_texture), 1); // UV texture
        }

        // Draw full-screen quad (2 triangles = 6 vertices)
        unsafe {
            render_encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 6);
        }

        // End encoding
        render_encoder.endEncoding();

        // Present drawable
        use objc2_quartz_core::CAMetalDrawable;
        let drawable_ref: &ProtocolObject<dyn CAMetalDrawable> = &*drawable;
        let drawable_as_mtl: &ProtocolObject<dyn MTLDrawable> =
            unsafe { std::mem::transmute(drawable_ref) };
        command_buffer.presentDrawable(drawable_as_mtl);

        // Commit command buffer
        command_buffer.commit();

        // Log first successful frame
        static FIRST_FRAME: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(true);
        if FIRST_FRAME.swap(false, std::sync::atomic::Ordering::Relaxed) {
            tracing::info!(
                "First hardware frame rendered successfully ({}x{}, zero-copy YUV→RGB)",
                CVPixelBufferGetWidth(pixel_buffer.inner()),
                CVPixelBufferGetHeight(pixel_buffer.inner())
            );
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
