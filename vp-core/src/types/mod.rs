use std::time::Duration;

#[cfg(target_os = "macos")]
mod pixel_buffer;

#[cfg(target_os = "macos")]
mod metal_texture_cache;

#[cfg(target_os = "macos")]
pub use pixel_buffer::PixelBuffer;

#[cfg(target_os = "macos")]
pub use metal_texture_cache::{MetalTextureCacheWrapper, MetalTexture, MetalTextureCacheError};

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

/// Frame data can be either software-decoded (CPU memory) or hardware-decoded (GPU memory).
#[derive(Clone)]
pub enum FrameData {
    /// Software-decoded frame in CPU memory (FFmpeg)
    Software {
        data: Vec<u8>,
        format: PixelFormat,
    },
    /// Hardware-decoded frame in GPU memory (AVFoundation + VideoToolbox)
    #[cfg(target_os = "macos")]
    Hardware(PixelBuffer),
}

impl FrameData {
    /// Returns true if this is a hardware-decoded frame.
    pub fn is_hardware(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            matches!(self, FrameData::Hardware(_))
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Returns true if this is a software-decoded frame.
    pub fn is_software(&self) -> bool {
        matches!(self, FrameData::Software { .. })
    }
}

impl std::fmt::Debug for FrameData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameData::Software { data, format } => f
                .debug_struct("Software")
                .field("format", format)
                .field("data_len", &data.len())
                .finish(),
            #[cfg(target_os = "macos")]
            FrameData::Hardware(pb) => f.debug_tuple("Hardware").field(pb).finish(),
        }
    }
}

/// A decoded video frame with presentation timestamp
#[derive(Clone)]
pub struct VideoFrame {
    /// Presentation timestamp in seconds
    pub pts: PTS,
    /// Frame data (either CPU or GPU memory)
    pub data: FrameData,
    /// Frame width in pixels
    pub width: u32,
    /// Frame height in pixels
    pub height: u32,
}

impl VideoFrame {
    /// Creates a software-decoded video frame.
    pub fn new_software(pts: PTS, data: Vec<u8>, width: u32, height: u32, format: PixelFormat) -> Self {
        Self {
            pts,
            data: FrameData::Software { data, format },
            width,
            height,
        }
    }

    /// Creates a hardware-decoded video frame (macOS only).
    #[cfg(target_os = "macos")]
    pub fn new_hardware(pts: PTS, pixel_buffer: PixelBuffer) -> Self {
        let width = pixel_buffer.width() as u32;
        let height = pixel_buffer.height() as u32;
        Self {
            pts,
            data: FrameData::Hardware(pixel_buffer),
            width,
            height,
        }
    }

    /// Legacy constructor for backward compatibility.
    pub fn new(pts: PTS, data: Vec<u8>, width: u32, height: u32, format: PixelFormat) -> Self {
        Self::new_software(pts, data, width, height, format)
    }

    /// Returns the expected size in bytes for software frames.
    pub fn expected_size(&self) -> Option<usize> {
        match &self.data {
            FrameData::Software { format, .. } => {
                let bytes_per_pixel = match format {
                    PixelFormat::Rgb24 | PixelFormat::Bgr24 => 3,
                    PixelFormat::Rgba | PixelFormat::Bgra => 4,
                };
                Some((self.width * self.height) as usize * bytes_per_pixel)
            }
            #[cfg(target_os = "macos")]
            FrameData::Hardware(_) => None, // No CPU buffer size for hardware frames
        }
    }
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("pts", &self.pts)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("data", &self.data)
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
