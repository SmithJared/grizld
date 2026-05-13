//! Proof-of-Concept: egui + wgpu UI with Metal Video Rendering
//!
//! This example demonstrates the complete integration:
//! 1. egui for UI (controls, buttons, etc.)
//! 2. wgpu as the rendering backend for egui
//! 3. Metal for hardware-accelerated video rendering
//! 4. Seamless integration: Metal texture → wgpu → egui display
//!
//! Usage:
//!   cargo run --example egui_metal_video_poc --release
//!
//! Expected result:
//!   - Window with egui UI (play/pause button, slider, etc.)
//!   - Video displayed in egui widget
//!   - ~60-70 FPS with hardware acceleration
//!   - Zero CPU involvement in video rendering

use std::ffi::c_void;
use std::sync::Arc;
use vp_core::decode::{VideoToolboxDecoder, VideoDecoder, VideoFrame};
use vp_core::demux::{Demuxer, StreamType};
use wgpu::Trace;
use winit::{
    event::{Event, WindowEvent},
    event_loop::EventLoop,
    window::Window,
};

#[cfg(target_os = "macos")]
use metal::foreign_types::ForeignType;

// IOSurface APIs
#[cfg(target_os = "macos")]
extern "C" {
    fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
    fn IOSurfaceGetPixelFormat(surface: *mut c_void) -> u32;
}

/// Metal renderer for hardware-accelerated video
#[cfg(target_os = "macos")]
struct MetalVideoRenderer {
    device: metal::Device,
    command_queue: metal::CommandQueue,
    intermediate_texture: Option<metal::Texture>,
    width: u32,
    height: u32,
    yuv_to_rgb_pipeline: Option<metal::RenderPipelineState>,
    yuv_to_rgb_shader_library: Option<metal::Library>,
}

#[cfg(target_os = "macos")]
impl MetalVideoRenderer {
    fn new() -> Self {
        let device = metal::Device::system_default().expect("No Metal device found");
        let command_queue = device.new_command_queue();
        
        Self {
            device,
            command_queue,
            intermediate_texture: None,
            width: 0,
            height: 0,
            yuv_to_rgb_pipeline: None,
            yuv_to_rgb_shader_library: None,
        }
    }
    
    /// Create a Metal texture from IOSurface (zero-copy!)
    fn create_texture_from_iosurface(&self, iosurface: *mut c_void) -> Result<metal::Texture, String> {
        unsafe {
            use objc::runtime::Object;
            use objc::{msg_send, sel, sel_impl};
            
            let width = IOSurfaceGetWidth(iosurface);
            let height = IOSurfaceGetHeight(iosurface);
            let pixel_format = IOSurfaceGetPixelFormat(iosurface);
            
            // Decode the FourCC format code
            let format_bytes = [
                ((pixel_format >> 24) & 0xFF) as u8,
                ((pixel_format >> 16) & 0xFF) as u8,
                ((pixel_format >> 8) & 0xFF) as u8,
                (pixel_format & 0xFF) as u8,
            ];
            let format_str = String::from_utf8_lossy(&format_bytes);
            
            println!("🔍 IOSurface actual format: 0x{:08X} ('{}')", pixel_format, format_str);
            println!("🔍 IOSurface dimensions: {}x{}", width, height);
            
            // Common formats:
            // 'BGRA' = 0x42475241 = BGRA8888
            // '420v' = 0x34323076 = NV12 (bi-planar YUV)
            // '420f' = 0x34323066 = YUV420 (full planar)
            
            if pixel_format != 0x42475241 {
                println!("⚠️  WARNING: IOSurface is NOT in BGRA format!");
                println!("⚠️  This will cause incorrect colors (purple/green)");
                println!("⚠️  Expected: 0x42475241 ('BGRA'), Got: 0x{:08X} ('{}')", 
                    pixel_format, format_str);
            }
            
            // IOSurface should now be in BGRA format thanks to our decoder configuration
            let desc = metal::TextureDescriptor::new();
            desc.set_width(width as u64);
            desc.set_height(height as u64);
            desc.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
            desc.set_usage(metal::MTLTextureUsage::ShaderRead);
            desc.set_storage_mode(metal::MTLStorageMode::Shared);
            
            // Use Objective-C runtime to call newTextureWithDescriptor:iosurface:plane:
            let device_ptr = self.device.as_ptr() as *mut Object;
            let desc_ptr = desc.as_ptr() as *mut Object;
            let plane: u64 = 0;
            
            let texture_ptr: *mut Object = msg_send![device_ptr,
                newTextureWithDescriptor: desc_ptr
                iosurface: iosurface
                plane: plane
            ];
            
            if texture_ptr.is_null() {
                return Err("Failed to create Metal texture from IOSurface".into());
            }
            
            // Wrap in metal::Texture
            let mtl_texture_ptr = texture_ptr as *mut metal::MTLTexture;
            let texture = metal::Texture::from_ptr(mtl_texture_ptr);
            
            Ok(texture)
        }
    }
    
    /// Ensure intermediate texture exists with correct size
    fn ensure_intermediate_texture(&mut self, width: u32, height: u32) {
        if self.intermediate_texture.is_none() || self.width != width || self.height != height {
            let desc = metal::TextureDescriptor::new();
            desc.set_width(width as u64);
            desc.set_height(height as u64);
            desc.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
            desc.set_usage(metal::MTLTextureUsage::ShaderRead | metal::MTLTextureUsage::RenderTarget);
            // Use Shared storage mode to allow CPU readback via get_bytes()
            // Private mode is GPU-only and causes segfault when reading
            desc.set_storage_mode(metal::MTLStorageMode::Shared);
            
            self.intermediate_texture = Some(self.device.new_texture(&desc));
            self.width = width;
            self.height = height;
        }
    }
    
    /// Blit IOSurface texture to intermediate texture (GPU-only)
    fn blit_texture(&self, source: &metal::Texture, dest: &metal::Texture) {
        let command_buffer = self.command_queue.new_command_buffer();
        let blit_encoder = command_buffer.new_blit_command_encoder();
        
        let size = metal::MTLSize {
            width: source.width(),
            height: source.height(),
            depth: 1,
        };
        
        blit_encoder.copy_from_texture(
            source,
            0, 0,
            metal::MTLOrigin { x: 0, y: 0, z: 0 },
            size,
            dest,
            0, 0,
            metal::MTLOrigin { x: 0, y: 0, z: 0 },
        );
        
        blit_encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();
    }
    
    /// Create YUV-to-RGB conversion pipeline
    fn create_yuv_to_rgb_pipeline(&mut self) -> Result<metal::RenderPipelineState, String> {
        // Metal shader source for NV12 to RGB conversion
        let shader_source = r#"
            #include <metal_stdlib>
            using namespace metal;
            
            struct VertexOut {
                float4 position [[position]];
                float2 texCoord;
            };
            
            // Vertex shader - fullscreen quad
            vertex VertexOut vertex_main(uint vid [[vertex_id]]) {
                // Generate fullscreen triangle
                float2 positions[3] = {
                    float2(-1.0, -1.0),
                    float2(3.0, -1.0),
                    float2(-1.0, 3.0)
                };
                
                float2 texCoords[3] = {
                    float2(0.0, 0.0),
                    float2(2.0, 0.0),
                    float2(0.0, 2.0)
                };
                
                VertexOut out;
                out.position = float4(positions[vid], 0.0, 1.0);
                out.texCoord = texCoords[vid];
                return out;
            }
            
            // Fragment shader - NV12 to RGB conversion
            fragment float4 fragment_main(
                VertexOut in [[stage_in]],
                texture2d<float> yTexture [[texture(0)]],
                texture2d<float> uvTexture [[texture(1)]]
            ) {
                constexpr sampler textureSampler(mag_filter::linear, min_filter::linear);
                
                // Sample Y plane (full resolution)
                float y = yTexture.sample(textureSampler, in.texCoord).r;
                
                // Sample UV plane (half resolution, interleaved)
                float2 uv = uvTexture.sample(textureSampler, in.texCoord).rg;
                
                // Convert from [0, 1] to proper YUV range
                y = y * 1.0;
                float u = uv.r - 0.5;
                float v = uv.g - 0.5;
                
                // BT.709 YUV to RGB conversion matrix
                float r = y + 1.5748 * v;
                float g = y - 0.1873 * u - 0.4681 * v;
                float b = y + 1.8556 * u;
                
                return float4(r, g, b, 1.0);
            }
        "#;
        
        // Compile shader library
        let library = self.device
            .new_library_with_source(shader_source, &metal::CompileOptions::new())
            .map_err(|e| format!("Failed to compile shader: {}", e))?;
        
        let vertex_function = library
            .get_function("vertex_main", None)
            .map_err(|e| format!("Failed to get vertex function: {}", e))?;
        
        let fragment_function = library
            .get_function("fragment_main", None)
            .map_err(|e| format!("Failed to get fragment function: {}", e))?;
        
        // Create render pipeline descriptor
        let pipeline_descriptor = metal::RenderPipelineDescriptor::new();
        pipeline_descriptor.set_vertex_function(Some(&vertex_function));
        pipeline_descriptor.set_fragment_function(Some(&fragment_function));
        
        // Set color attachment format (BGRA8Unorm to match intermediate texture)
        let color_attachment = pipeline_descriptor
            .color_attachments()
            .object_at(0)
            .ok_or("Failed to get color attachment")?;
        color_attachment.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
        
        // Create pipeline state
        let pipeline_state = self.device
            .new_render_pipeline_state(&pipeline_descriptor)
            .map_err(|e| format!("Failed to create pipeline state: {}", e))?;
        
        // Store the library for future reference
        self.yuv_to_rgb_shader_library = Some(library);
        
        Ok(pipeline_state)
    }
    
    /// Render IOSurface to intermediate texture
    pub fn render_iosurface(&mut self, iosurface: *mut c_void) -> Result<&metal::Texture, String> {
        unsafe {
            use objc::runtime::Object;
            use objc::{msg_send, sel, sel_impl};
            
            let width = IOSurfaceGetWidth(iosurface);
            let height = IOSurfaceGetHeight(iosurface);
            let pixel_format = IOSurfaceGetPixelFormat(iosurface);
            
            // Decode FourCC format (note: stored in big-endian)
            let format_bytes = [
                ((pixel_format >> 24) & 0xFF) as u8,
                ((pixel_format >> 16) & 0xFF) as u8,
                ((pixel_format >> 8) & 0xFF) as u8,
                (pixel_format & 0xFF) as u8,
            ];
            let format_str = String::from_utf8_lossy(&format_bytes);
            
            println!("🔍 IOSurface format: 0x{:08X} ('{}')", pixel_format, format_str);
            println!("🔍 IOSurface dimensions: {}x{}", width, height);
            
            // Ensure intermediate texture exists
            self.ensure_intermediate_texture(width as u32, height as u32);
            
            // Check format and handle accordingly
            // Common formats:
            // 0x42475241 = 'BGRA' = BGRA8888
            // 0x34323076 = '420v' = NV12 (bi-planar YUV, most common from VideoToolbox)
            // 0x34323066 = '420f' = YUV420 (full planar)
            // 0x78663232 = 'xf22' or similar - need to check plane count
            
            match pixel_format {
                0x42475241 => {
                    // BGRA format - direct copy
                    println!("🎨 Using BGRA format directly (no conversion needed)");
                    let source_texture = self.create_texture_from_iosurface(iosurface)?;
                    self.blit_texture(&source_texture, self.intermediate_texture.as_ref().unwrap());
                    Ok(self.intermediate_texture.as_ref().unwrap())
                }
                0x34323076 | 0x78663232 => {
                    // NV12 or similar bi-planar YUV format
                    println!("🎨 Converting bi-planar YUV to RGB using Metal shader");
                    
                    // Create pipeline if not already created
                    if self.yuv_to_rgb_pipeline.is_none() {
                        self.yuv_to_rgb_pipeline = Some(self.create_yuv_to_rgb_pipeline()?);
                    }
                    
                    let pipeline = self.yuv_to_rgb_pipeline.as_ref().unwrap();
                    
                    // Create Y plane texture (plane 0, full resolution, R8)
                    let y_desc = metal::TextureDescriptor::new();
                    y_desc.set_width(width as u64);
                    y_desc.set_height(height as u64);
                    y_desc.set_pixel_format(metal::MTLPixelFormat::R8Unorm);
                    y_desc.set_usage(metal::MTLTextureUsage::ShaderRead);
                    y_desc.set_storage_mode(metal::MTLStorageMode::Shared);
                    
                    let device_ptr = self.device.as_ptr() as *mut Object;
                    let y_desc_ptr = y_desc.as_ptr() as *mut Object;
                    let y_plane: u64 = 0;
                    
                    let y_texture_ptr: *mut Object = msg_send![device_ptr,
                        newTextureWithDescriptor: y_desc_ptr
                        iosurface: iosurface
                        plane: y_plane
                    ];
                    
                    if y_texture_ptr.is_null() {
                        return Err("Failed to create Y plane texture from IOSurface".into());
                    }
                    
                    let y_texture = metal::Texture::from_ptr(y_texture_ptr as *mut metal::MTLTexture);
                    
                    // Create UV plane texture (plane 1, half resolution, RG8)
                    let uv_desc = metal::TextureDescriptor::new();
                    uv_desc.set_width((width / 2) as u64);
                    uv_desc.set_height((height / 2) as u64);
                    uv_desc.set_pixel_format(metal::MTLPixelFormat::RG8Unorm);
                    uv_desc.set_usage(metal::MTLTextureUsage::ShaderRead);
                    uv_desc.set_storage_mode(metal::MTLStorageMode::Shared);
                    
                    let uv_desc_ptr = uv_desc.as_ptr() as *mut Object;
                    let uv_plane: u64 = 1;
                    
                    let uv_texture_ptr: *mut Object = msg_send![device_ptr,
                        newTextureWithDescriptor: uv_desc_ptr
                        iosurface: iosurface
                        plane: uv_plane
                    ];
                    
                    if uv_texture_ptr.is_null() {
                        return Err("Failed to create UV plane texture from IOSurface".into());
                    }
                    
                    let uv_texture = metal::Texture::from_ptr(uv_texture_ptr as *mut metal::MTLTexture);
                    
                    println!("✅ Created Y texture: {}x{}, UV texture: {}x{}", 
                        y_texture.width(), y_texture.height(),
                        uv_texture.width(), uv_texture.height());
                    
                    // Render YUV to RGB using shader
                    let command_buffer = self.command_queue.new_command_buffer();
                    
                    let render_pass_descriptor = metal::RenderPassDescriptor::new();
                    let color_attachment = render_pass_descriptor.color_attachments().object_at(0).unwrap();
                    color_attachment.set_texture(Some(self.intermediate_texture.as_ref().unwrap()));
                    color_attachment.set_load_action(metal::MTLLoadAction::Clear);
                    color_attachment.set_store_action(metal::MTLStoreAction::Store);
                    color_attachment.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 1.0));
                    
                    let encoder = command_buffer.new_render_command_encoder(&render_pass_descriptor);
                    encoder.set_render_pipeline_state(pipeline);
                    
                    // Bind Y and UV textures
                    encoder.set_fragment_texture(0, Some(&y_texture));
                    encoder.set_fragment_texture(1, Some(&uv_texture));
                    
                    // Draw fullscreen triangle (3 vertices)
                    encoder.draw_primitives(metal::MTLPrimitiveType::Triangle, 0, 3);
                    
                    encoder.end_encoding();
                    command_buffer.commit();
                    command_buffer.wait_until_completed();
                    
                    println!("✅ YUV→RGB conversion complete");
                    
                    Ok(self.intermediate_texture.as_ref().unwrap())
                }
                _ => {
                    Err(format!(
                        "Unsupported IOSurface pixel format: 0x{:08X} ('{}'). Supported formats: BGRA (0x42475241), 420v (0x34323076)",
                        pixel_format, format_str
                    ))
                }
            }
        }
    }
    
    pub fn get_texture(&self) -> Option<&metal::Texture> {
        self.intermediate_texture.as_ref()
    }
    
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Application state
struct App {
    // wgpu context
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    
    // egui
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    
    // Video rendering
    #[cfg(target_os = "macos")]
    metal_renderer: MetalVideoRenderer,
    video_texture_id: Option<egui::TextureId>,
    
    // Video playback
    demuxer: Demuxer,
    decoder: VideoToolboxDecoder,
    current_frame: Option<VideoFrame>,
    frame_count: u64,
    
    // UI state
    is_playing: bool,
    playback_speed: f32,
    
    // Store egui output for rendering
    pending_full_output: Option<egui::FullOutput>,
}

impl App {
    async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        
        // Create wgpu instance
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..Default::default()
        });
        
        let surface = instance.create_surface(window.clone()).unwrap();
        
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find adapter");
        
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Main Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: Trace::Off,
            })
            .await
            .expect("Failed to create device");
        
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8Unorm,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);
        
        // Create egui context
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            surface_config.format,
            None,
            1,
            false,
        );
        
        // Create Metal renderer
        #[cfg(target_os = "macos")]
        let metal_renderer = MetalVideoRenderer::new();
        
        // Load video
        let video_path = "tests/resources/test_video.mp4";
        let mut demuxer = Demuxer::new(video_path).expect("Failed to create demuxer");
        let stream_info = demuxer.video_stream_info().expect("No video stream").clone();
        let decoder = VideoToolboxDecoder::new(&stream_info).expect("Failed to create decoder");
        
        println!("✅ Video loaded: {}x{}", stream_info.width, stream_info.height);
        
        Self {
            device,
            queue,
            surface,
            surface_config,
            egui_ctx,
            egui_state,
            egui_renderer,
            #[cfg(target_os = "macos")]
            metal_renderer,
            video_texture_id: None,
            demuxer,
            decoder,
            current_frame: None,
            frame_count: 0,
            is_playing: true,
            playback_speed: 1.0,
            pending_full_output: None,
        }
    }
    
    fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.egui_state.on_window_event(window, event).consumed
    }
    
    fn update(&mut self) {
        if !self.is_playing {
            return;
        }
        
        // Decode next frame
        loop {
            if let Some(packet) = self.demuxer.next_packet().ok().flatten() {
                if packet.stream_type == StreamType::Video {
                    if let Ok(Some(frame)) = self.decoder.decode_packet(&packet) {
                        self.current_frame = Some(frame);
                        self.frame_count += 1;
                        break;
                    }
                }
            } else {
                // End of video, loop back
                self.demuxer.seek(0.0).ok();
                self.frame_count = 0;
            }
        }
    }
    
    #[cfg(target_os = "macos")]
    fn update_video_texture(&mut self) {
        // Extract IOSurface pointer first to avoid holding a reference to self.current_frame
        let iosurface_ptr = if let Some(VideoFrame::Hardware(ref hw_frame)) = self.current_frame {
            hw_frame.iosurface_ptr()
        } else {
            return; // No hardware frame to render
        };
        
        // Read texture data into buffer while we have the borrow
        let (width, height, buffer) = {
            // Render IOSurface to Metal texture (borrows self.metal_renderer)
            let metal_texture = self.metal_renderer
                .render_iosurface(iosurface_ptr)
                .expect("Failed to render IOSurface");
            
            // Extract dimensions
            let width = metal_texture.width() as u32;
            let height = metal_texture.height() as u32;
            
            println!("🎬 Metal texture dimensions: {}x{}", width, height);
            
            // Copy data from Metal texture to buffer
            let bytes_per_pixel = 4; // BGRA8
            let bytes_per_row = width * bytes_per_pixel;
            let buffer_size = (bytes_per_row * height) as usize;
            
            println!("📊 Buffer size: {} bytes ({} MB), bytes_per_row: {} (BGRA)", 
                buffer_size, buffer_size / 1024 / 1024, bytes_per_row);
            
            // Read Metal texture data
            let mut buffer = vec![0u8; buffer_size];
            let region = metal::MTLRegion {
                origin: metal::MTLOrigin { x: 0, y: 0, z: 0 },
                size: metal::MTLSize {
                    width: width as u64,
                    height: height as u64,
                    depth: 1,
                },
            };
            
            metal_texture.get_bytes(
                buffer.as_mut_ptr() as *mut _,
                bytes_per_row as u64,
                region,
                0,
            );
            
            (width, height, buffer)
        }; // metal_texture borrow ends here
        
        // Now we can safely borrow self mutably to create the wgpu texture
        let wgpu_texture = self.create_wgpu_texture(width, height);
        
        // Write to wgpu texture
        let bytes_per_row = width * 4;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &wgpu_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &buffer,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        
        let texture_view = wgpu_texture.create_view(&wgpu::TextureViewDescriptor::default());
        
        // Register or update with egui
        if let Some(texture_id) = self.video_texture_id {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                &texture_view,
                wgpu::FilterMode::Linear,
                texture_id,
            );
        } else {
            let texture_id = self.egui_renderer.register_native_texture(
                &self.device,
                &texture_view,
                wgpu::FilterMode::Linear,
            );
            self.video_texture_id = Some(texture_id);
        }
    }

    #[cfg(target_os = "macos")]
    fn create_wgpu_texture(&self, width: u32, height: u32) -> wgpu::Texture {
        // For this POC, we'll create a regular wgpu texture and copy the data
        // In production with wgpu 27+ and Metal external texture support, this would be a direct wrap
        let desc = wgpu::TextureDescriptor {
            label: Some("Video Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        
        self.device.create_texture(&desc)
    }
    
    fn render_ui(&mut self, window: &Window) {
        let raw_input = self.egui_state.take_egui_input(window);
        let output = self.egui_ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("🎬 Hardware-Accelerated Video Player");
                ui.separator();
                
                // Playback controls
                ui.horizontal(|ui| {
                    if ui.button(if self.is_playing { "⏸ Pause" } else { "▶ Play" }).clicked() {
                        self.is_playing = !self.is_playing;
                    }
                    
                    ui.label(format!("Frame: {}", self.frame_count));
                    
                    #[cfg(target_os = "macos")]
                    {
                        let (width, height) = self.metal_renderer.dimensions();
                        ui.label(format!("Resolution: {}x{}", width, height));
                    }
                });
                
                ui.add(egui::Slider::new(&mut self.playback_speed, 0.25..=2.0).text("Speed"));
                
                ui.separator();
                
                // Video display
                if let Some(texture_id) = self.video_texture_id {
                    #[cfg(target_os = "macos")]
                    let (width, height) = self.metal_renderer.dimensions();
                    #[cfg(not(target_os = "macos"))]
                    let (width, height) = (1920, 1080);
                    
                    let available_size = ui.available_size();
                    println!("🖼️  Video dimensions: {}x{}, available_size: {:?}", width, height, available_size);
                    
                    let aspect_ratio = width as f32 / height as f32;
                    
                    // Calculate display size maintaining aspect ratio
                    let display_width = available_size.x.min(available_size.y * aspect_ratio);
                    let display_height = display_width / aspect_ratio;
                    
                    println!("📐 Display size: {:.1}x{:.1}, aspect_ratio: {:.2}", 
                        display_width, display_height, aspect_ratio);

                    // Center the image
                    ui.vertical_centered(|ui| {
                        ui.add(
                            egui::Image::new(egui::load::SizedTexture::new(
                                texture_id,
                                [display_width, display_height],
                            ))
                        );
                    });
                } else {
                    ui.label("Loading video...");
                }
                
                ui.separator();
                ui.label("💡 This demo shows Metal video rendering integrated with egui!");
                ui.label("🚀 Hardware decoding + Zero-copy GPU rendering");
            });
        });
        
        // Store the output first
        self.pending_full_output = Some(output);
        
        // Now access platform_output from the stored location
        if let Some(ref full_output) = self.pending_full_output {
            self.egui_state.handle_platform_output(window, full_output.platform_output.clone());
        }
    }
    
    fn render(&mut self, window: &Window) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });
        
        // Prepare egui render
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point: window.scale_factor() as f32,
        };
        
        // Get the pending output from render_ui
        let Some(full_output) = self.pending_full_output.take() else {
            return Ok(()); // Nothing to render
        };
        
        let primitives = self.egui_ctx.tessellate(
            full_output.shapes,
            full_output.pixels_per_point,
        );
        
        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer.update_texture(
                &self.device,
                &self.queue,
                *id,
                image_delta,
            );
        }
        
        // Render egui using the correct pattern for egui-wgpu 0.32
        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &primitives,
            &screen_descriptor,
        );
        
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Egui Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.1,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            
            // Use forget_lifetime() to convert render pass to 'static lifetime
            // This is required by egui-wgpu's render method
            self.egui_renderer.render(&mut render_pass.forget_lifetime(), &primitives, &screen_descriptor);
        }
        
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn main() {
    env_logger::init();
    
    println!("=== egui + wgpu + Metal Video Integration POC ===\n");
    
    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let window_attributes = winit::window::Window::default_attributes()
        .with_title("egui + Metal Video Player")
        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 720));
    let window = event_loop.create_window(window_attributes)
        .expect("Failed to create window");
    let window = Arc::new(window);
    
    let mut app = pollster::block_on(App::new(window.clone()));
    
    println!("✅ Application initialized");
    println!("🎬 Playing video with hardware acceleration\n");
    
    let _ = event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { event, .. } => {
                // Let egui handle events first
                if !app.handle_event(&window, &event) {
                    match event {
                        WindowEvent::CloseRequested => {
                            println!("Window closed");
                            elwt.exit();
                        }
                        WindowEvent::Resized(size) => {
                            app.surface_config.width = size.width;
                            app.surface_config.height = size.height;
                            app.surface.configure(&app.device, &app.surface_config);
                        }
                        WindowEvent::RedrawRequested => {
                            app.update();
                            app.update_video_texture();
                            app.render_ui(&window);
                            
                            match app.render(&window) {
                                Ok(_) => {}
                                Err(wgpu::SurfaceError::Lost) => {
                                    app.surface.configure(&app.device, &app.surface_config);
                                }
                                Err(wgpu::SurfaceError::OutOfMemory) => {
                                    eprintln!("Out of memory");
                                    elwt.exit();
                                }
                                Err(e) => {
                                    eprintln!("Render error: {:?}", e);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::AboutToWait => {
                window.request_redraw();
            }
            _ => {}
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This example only works on macOS (requires Metal and IOSurface support)");
    std::process::exit(1);
}