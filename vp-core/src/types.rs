use std::time::Duration;

/// Presentation timestamp in seconds
pub type PTS = f64;

/// Pixel format for video frames
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb24,
    Rgba,
    Bgr24,
    Bgra,
}

/// A decoded video frame with presentation timestamp
#[derive(Clone)]
pub struct VideoFrame {
    /// Presentation timestamp in seconds
    pub pts: PTS,
    /// Raw pixel data (format-dependent)
    pub data: Vec<u8>,
    /// Frame width in pixels
    pub width: u32,
    /// Frame height in pixels
    pub height: u32,
    /// Pixel format
    pub format: PixelFormat,
}

impl VideoFrame {
    pub fn new(pts: PTS, data: Vec<u8>, width: u32, height: u32, format: PixelFormat) -> Self {
        Self {
            pts,
            data,
            width,
            height,
            format,
        }
    }

    /// Returns the expected size in bytes for this frame
    pub fn expected_size(&self) -> usize {
        let bytes_per_pixel = match self.format {
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => 3,
            PixelFormat::Rgba | PixelFormat::Bgra => 4,
        };
        (self.width * self.height) as usize * bytes_per_pixel
    }
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("pts", &self.pts)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// A chunk of decoded audio samples
#[derive(Clone, Debug)]
pub struct AudioSample {
    /// Presentation timestamp in seconds
    pub pts: PTS,
    /// Interleaved stereo samples (L, R, L, R, ...)
    pub data: Vec<f32>,
    /// Sample rate in Hz
    pub sample_rate: u32,
}

impl AudioSample {
    pub fn new(pts: PTS, data: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            pts,
            data,
            sample_rate,
        }
    }

    /// Returns the duration of this audio sample
    pub fn duration(&self) -> Duration {
        let num_frames = self.data.len() / 2; // Stereo
        let duration_secs = num_frames as f64 / self.sample_rate as f64;
        Duration::from_secs_f64(duration_secs)
    }
}

/// Playback state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

impl PlaybackState {
    pub fn is_playing(&self) -> bool {
        matches!(self, PlaybackState::Playing)
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self, PlaybackState::Stopped)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self, PlaybackState::Paused)
    }
}
