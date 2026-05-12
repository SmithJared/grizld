use egui::{ColorImage, TextureHandle, TextureOptions};
use vp_core::types::VideoFrame;

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
        let image = ColorImage::from_rgba_unmultiplied(
            [frame.width as _, frame.height as _],
            &frame.data,
        );

        // Create/recreate texture
        // Note: egui caches textures by name, so this is reasonably efficient
        self.texture = Some(ctx.load_texture("video_frame", image, TextureOptions::LINEAR));
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
