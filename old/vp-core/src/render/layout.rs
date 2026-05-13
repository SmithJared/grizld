//! Video viewport layout and aspect ratio management.
//!
//! This module handles calculating the correct viewport for video playback,
//! including aspect ratio preservation, letterboxing/pillarboxing, and
//! coordinate system conversions between Metal (bottom-left origin) and
//! egui (top-left origin).

use objc2_core_foundation::{CGPoint, CGRect, CGSize};

/// Information about letterboxing/pillarboxing bars.
#[derive(Debug, Clone, Copy)]
pub struct LetterboxInfo {
    /// Top bar height (in pixels)
    pub top: f32,
    /// Bottom bar height (in pixels)
    pub bottom: f32,
    /// Left bar width (in pixels)
    pub left: f32,
    /// Right bar width (in pixels)
    pub right: f32,
}

/// Video viewport with aspect ratio preservation.
///
/// Calculates the correct rectangle for rendering video within a window,
/// maintaining the video's aspect ratio and adding letterboxing or pillarboxing
/// as needed.
#[derive(Debug, Clone, Copy)]
pub struct VideoViewport {
    /// Window dimensions
    window_width: u32,
    window_height: u32,
    window_x: u32,
    window_y: u32,

    /// Video dimensions
    video_width: u32,
    video_height: u32,
}

impl VideoViewport {
    /// Creates a new VideoViewport with the given dimensions.
    ///
    /// # Arguments
    /// * `window_width` - Width of the window in pixels
    /// * `window_height` - Height of the window in pixels
    /// * `video_width` - Native width of the video in pixels
    /// * `video_height` - Native height of the video in pixels
    pub fn new(window_width: u32, window_height: u32, video_width: u32, video_height: u32, window_x: u32, window_y: u32 ) -> Self {
        // Prevent division by zero
        let video_width = video_width.max(1);
        let video_height = video_height.max(1);
        let window_width = window_width.max(1);
        let window_height = window_height.max(1);
        Self {
            window_width,
            window_height,
            window_x,
            window_y,
            video_width,
            video_height,
        }
    }


    /// Returns true if the window dimensions have changed.
    pub fn update_window_dimensions(&mut self, width: u32, height: u32, x: u32, y: u32 ) -> bool {
        if self.window_width != width || self.window_height != height || self.window_x != x || self.window_y != y {
            self.window_width = width;
            self.window_height = height;
            self.window_x = x;
            self.window_y = y;
            true
        } else {
            false
        }
    }

    pub fn update_video_dimensions(&mut self, width: u32, height: u32) -> bool {
        if self.video_width != width || self.video_height != height {
            self.video_width = width;
            self.video_height = height;
            true
        } else {
            false
        }
    }

    pub fn video_aspect(&self) -> f32 {
        self.video_width as f32 / self.video_height as f32
    }

    pub fn window_aspect(&self) -> f32 {
        self.window_width as f32 / self.window_height as f32
    }

    /// Returns the viewport dimensions (x, y, width, height) in window coordinates.
    pub fn dimensions(&self) -> (f32, f32, f32, f32) {
        let video_aspect = self.video_aspect();
        let window_aspect = self.window_aspect();

        if video_aspect > window_aspect {
            // Video is wider than window - letterbox (bars on top/bottom)
            let width = self.window_width as f32;
            let height = width / video_aspect;
            let x = self.window_x as f32;
            let y = (self.window_height as f32 - height) / 2.0 + self.window_y as f32;
            (x, y, width, height)
        } else {
            // Video is taller than window - pillarbox (bars on left/right)
            let height = self.window_height as f32;
            let width = height * video_aspect;
            let x = (self.window_width as f32 - width) / 2.0 + self.window_x as f32;
            let y = self.window_y as f32;
            (x, y, width, height)
        }
    }

    pub fn has_video(&self) -> bool {
        self.video_width > 0 && self.video_height > 0
    }

    // /// Returns the letterbox/pillarbox bar dimensions.
    // pub fn calculate_letterbox(&self) -> LetterboxInfo {
    //     LetterboxInfo {
    //         top: self.viewport_y,
    //         bottom: self.window_height as f32 - (self.viewport_y + self.viewport_height),
    //         left: self.viewport_x,
    //         right: self.window_width as f32 - (self.viewport_x + self.viewport_width),
    //     }
    // }

    /// Converts the viewport to a CALayer CGRect.
    ///
    /// Note: CALayer on macOS uses top-left origin (same as window coordinates),
    /// not bottom-left like Metal rendering coordinates.
    pub fn to_metal_frame(&self) -> CGRect {
        let (x, y, width, height) = self.dimensions();
        CGRect {
            origin: CGPoint {
                x: x as f64,
                y: y as f64,
            },
            size: CGSize {
                width: width as f64,
                height: height as f64,
            },
        }
    }

    /// Converts the viewport to an egui Rect (top-left origin).
    pub fn to_egui_rect(&self) -> egui::Rect {
        let (x, y, width, height) = self.dimensions();
        egui::Rect::from_min_size(
            egui::pos2(x, y),
            egui::vec2(width, height),
        )
    }

    /// Returns the video's native dimensions.
    pub fn video_dimensions(&self) -> (u32, u32) {
        (self.video_width, self.video_height)
    }

    /// Returns the window dimensions.
    pub fn window_dimensions(&self) -> (u32, u32) {
        (self.window_width, self.window_height)
    }
}

impl Default for VideoViewport {
    fn default() -> Self {
        Self {
            window_width: 0,
            window_height: 0,
            window_x: 0,
            window_y: 0,
            video_width: 0,
            video_height: 0,
        }
    }
}