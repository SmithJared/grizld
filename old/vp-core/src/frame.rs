use video_sys::core_video::PixelBuffer;


/// Unified video frame type
#[derive(Clone, Debug)]
pub struct ExtractedFrame {
    pub pts: TimebaseUnits, // In PTS units
    pub width: u32,
    pub height: u32,
    pub data: FrameData,
}

#[derive(Clone, Debug)]
pub struct ExtractedAudioFrame {
    pub pts: TimebaseUnits, // In PTS units
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl ExtractedFrame {
    pub fn new(pts: TimebaseUnits, width: u32, height: u32, data: FrameData) -> Self {
        Self {
            pts,
            width,
            height,
            data,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub pts: Microseconds, // PTS in microseconds
    pub extracted_frame: ExtractedFrame,
}

impl VideoFrame {
    pub fn pts(&self) -> Microseconds {
        self.pts
    }

    pub fn width(&self) -> u32 {
        self.extracted_frame.width
    }

    pub fn height(&self) -> u32 {
        self.extracted_frame.height
    }

    pub fn data(&self) -> &FrameData {
        &self.extracted_frame.data
    }
}

impl PartialEq for VideoFrame {
    fn eq(&self, other: &Self) -> bool {
        self.pts == other.pts
    }
}

impl Eq for VideoFrame {}

impl PartialOrd for VideoFrame {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.pts.partial_cmp(&other.pts)
    }
}

impl Ord for VideoFrame {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Use total_cmp for f64 to handle NaN, infinity, etc. properly
        self.pts.cmp(&other.pts)
    }
}


#[derive(Debug)]
pub enum FrameData {
    YUV {
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
        stride_y: usize,
        stride_u: usize,
        stride_v: usize,
    },
    RGBA {
        bytes: Vec<u8>,
    }, // interleaved RGBA
    BGRA {
        bytes: Vec<u8>,
    }, // interleaved BGRA
    NV12 {
        y: Vec<u8>,
        uv: Vec<u8>,
        stride_y: usize,
        stride_uv: usize,
    }, // bi-planar YUV (for future hardware frames)
    #[cfg(target_os = "macos")]
    CVPixelBuffer {
        buffer: PixelBuffer,
    }, // Hardware-decoded frame (macOS only)
}

impl Clone for FrameData {
    fn clone(&self) -> Self {
        match self {
            FrameData::YUV { y, u, v, stride_y, stride_u, stride_v } => {
                FrameData::YUV {
                    y: y.clone(),
                    u: u.clone(),
                    v: v.clone(),
                    stride_y: *stride_y,
                    stride_u: *stride_u,
                    stride_v: *stride_v,
                }
            }
            FrameData::RGBA { bytes } => {
                FrameData::RGBA { bytes: bytes.clone() }
            }
            FrameData::BGRA { bytes } => {
                FrameData::BGRA { bytes: bytes.clone() }
            }
            FrameData::NV12 { y, uv, stride_y, stride_uv } => {
                FrameData::NV12 {
                    y: y.clone(),
                    uv: uv.clone(),
                    stride_y: *stride_y,
                    stride_uv: *stride_uv,
                }
            }
            #[cfg(target_os = "macos")]
            FrameData::CVPixelBuffer { buffer } => {
                FrameData::CVPixelBuffer { buffer: buffer.clone() }
            }
        }
    }
}

impl FrameData {

    pub fn new_yuv(y: Vec<u8>, u: Vec<u8>, v: Vec<u8>, stride_y: usize, stride_u: usize, stride_v: usize) -> Self {
        Self::YUV {
            y,
            u,
            v,
            stride_y,
            stride_u,
            stride_v,
        }
    }

    pub fn new_rgba(bytes: Vec<u8>) -> Self {
        Self::RGBA { bytes }
    }

    pub fn new_bgra(bytes: Vec<u8>) -> Self {
        Self::BGRA { bytes }
    }

    pub fn new_nv12(y: Vec<u8>, uv: Vec<u8>, stride_y: usize, stride_uv: usize) -> Self {
        Self::NV12 { y, uv, stride_y, stride_uv }
    }

    pub fn new_cvpixelbuffer(buffer: PixelBuffer) -> Self {
        Self::CVPixelBuffer { buffer }
    }

    /// Check if this frame data is a CVPixelBuffer (hardware frame)
    pub fn is_cvpixelbuffer(&self) -> bool {
        matches!(self, FrameData::CVPixelBuffer { .. })
    }

    /// Get the pixel format name for this frame data
    pub fn format_name(&self) -> &'static str {
        match self {
            FrameData::YUV { .. } => "YUV420p",
            FrameData::RGBA { .. } => "RGBA",
            FrameData::BGRA { .. } => "BGRA",
            FrameData::NV12 { .. } => "NV12",
            FrameData::CVPixelBuffer { .. } => "CVPixelBuffer",
        }
    }
}

pub const AV_TIME_BASE: i64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimebaseUnits(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Seconds(pub f64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Microseconds(pub i64);

impl Seconds {
    pub const ZERO: Self = Self(0.0);
    pub fn new(value: f64) -> Self {
        Self(value)
    }

    /// Returns 0 if NaN or Infinity
    pub fn as_microseconds(&self) -> i64 {
        if self.0.is_nan() || self.0.is_infinite() {
            return 0;
        }
        (self.0 * AV_TIME_BASE as f64).round() as i64
    }

    pub fn from_microseconds(microseconds: i64) -> Self {
        Self(microseconds as f64 / AV_TIME_BASE as f64)
    }

    pub fn to_microseconds(&self) -> Microseconds {
        Microseconds(self.as_microseconds())
    }
}

impl std::fmt::Display for Seconds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}s", self.0)
    }
}

impl Microseconds {
    pub const ZERO: Self = Self(0);
    pub fn new(value: i64) -> Self {
        Self(value)
    }

    pub fn as_seconds(&self) -> f64 {
        self.0 as f64 / AV_TIME_BASE as f64    
    }

    /// Returns O if NaN or Infinity
    pub fn from_seconds(seconds: f64) -> Self {
        if seconds.is_nan() || seconds.is_infinite() {
            return Self(0);
        }
        Self((seconds * AV_TIME_BASE as f64).round() as i64)
    }

    pub fn to_seconds(&self) -> Seconds {
        Seconds(self.as_seconds())
    }
}

impl std::fmt::Display for Microseconds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}μs", self.0)
    }
}
