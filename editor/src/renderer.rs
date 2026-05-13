use egui::{ColorImage, TextureHandle, TextureOptions};
use vp_core::types::VideoFrame;

/// Maximum texture dimension (downscale 4K to this for performance)
const MAX_TEXTURE_WIDTH: u32 = 1920;
const MAX_TEXTURE_HEIGHT: u32 = 1080;

/// Renders video frames as egui textures
pub struct VideoRenderer {
    texture: Option<TextureHandle>,
}

impl VideoRenderer {
    pub fn new() -> Self {
        Self { texture: None }
    }

    /// Update the texture with a new frame
    pub fn update_texture(&mut self, ctx: &egui::Context, frame: &VideoFrame) {
        // Downscale if needed
        let (width, height, data) = if frame.width > MAX_TEXTURE_WIDTH || frame.height > MAX_TEXTURE_HEIGHT {
            let result = Self::downscale_frame(frame);

            static LOG_DOWNSCALE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
            if LOG_DOWNSCALE.swap(false, std::sync::atomic::Ordering::Relaxed) {
                tracing::info!("Downscaling {}x{} → {}x{}", frame.width, frame.height, result.0, result.1);
            }

            result
        } else {
            // Extract pixel data from FrameData enum
            let pixel_data = match &frame.data {
                vp_core::types::FrameData::Software { data, .. } => data.clone(),
                #[cfg(target_os = "macos")]
                vp_core::types::FrameData::Hardware(_) => {
                    tracing::warn!("Hardware frame in software renderer");
                    Vec::new()
                }
            };
            (frame.width, frame.height, pixel_data)
        };

        // Convert to ColorImage
        let image = ColorImage::from_rgba_unmultiplied([width as _, height as _], &data);

        // Upload to GPU
        self.texture = Some(ctx.load_texture("video_frame", image, TextureOptions::LINEAR));
    }

    /// Downscale a frame using nearest-neighbor sampling
    fn downscale_frame(frame: &VideoFrame) -> (u32, u32, Vec<u8>) {
        // Extract pixel data from FrameData enum
        let pixel_data = match &frame.data {
            vp_core::types::FrameData::Software { data, .. } => data,
            #[cfg(target_os = "macos")]
            vp_core::types::FrameData::Hardware(_) => {
                // Hardware frames should be rendered via Metal, not egui
                // Return empty data for now
                tracing::warn!("Attempted to downscale hardware frame in software renderer");
                return (0, 0, Vec::new());
            }
        };

        let src_width = frame.width;
        let src_height = frame.height;

        // Calculate target dimensions maintaining aspect ratio
        let aspect_ratio = src_width as f32 / src_height as f32;
        let (target_width, target_height) = if aspect_ratio > (MAX_TEXTURE_WIDTH as f32 / MAX_TEXTURE_HEIGHT as f32) {
            // Width-constrained
            let w = MAX_TEXTURE_WIDTH;
            let h = (w as f32 / aspect_ratio) as u32;
            (w, h)
        } else {
            // Height-constrained
            let h = MAX_TEXTURE_HEIGHT;
            let w = (h as f32 * aspect_ratio) as u32;
            (w, h)
        };

        let mut downscaled = Vec::with_capacity((target_width * target_height * 4) as usize);

        let x_ratio = src_width as f32 / target_width as f32;
        let y_ratio = src_height as f32 / target_height as f32;

        for ty in 0..target_height {
            let sy = (ty as f32 * y_ratio) as u32;
            for tx in 0..target_width {
                let sx = (tx as f32 * x_ratio) as u32;

                let src_idx = ((sy * src_width + sx) * 4) as usize;
                downscaled.push(pixel_data[src_idx]);     // R
                downscaled.push(pixel_data[src_idx + 1]); // G
                downscaled.push(pixel_data[src_idx + 2]); // B
                downscaled.push(pixel_data[src_idx + 3]); // A
            }
        }

        (target_width, target_height, downscaled)
    }

    /// Render the video texture in a UI
    ///
    /// Centers the video and maintains aspect ratio.
    pub fn render(&self, ui: &mut egui::Ui) {
        if let Some(texture) = &self.texture {
            let size = texture.size_vec2();
            let aspect_ratio = size.x / size.y;

            // Calculate display size to fit within available space
            let available = ui.available_size();
            let display_size = if available.x / available.y > aspect_ratio {
                // Constrained by height
                egui::vec2(available.y * aspect_ratio, available.y)
            } else {
                // Constrained by width
                egui::vec2(available.x, available.x / aspect_ratio)
            };

            // Center the video
            ui.centered_and_justified(|ui| {
                ui.image((texture.id(), display_size));
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("No video loaded");
            });
        }
    }

    /// Check if a texture is loaded
    pub fn has_texture(&self) -> bool {
        self.texture.is_some()
    }
}

impl Default for VideoRenderer {
    fn default() -> Self {
        Self::new()
    }
}
