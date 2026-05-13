//! CVPixelBuffer wrapper for hardware-decoded video frames.
//!
//! This module provides a safe Rust wrapper around Core Video's CVPixelBuffer,
//! which is extracted from FFmpeg VideoToolbox hardware frames.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
};
use std::fmt;
use std::os::raw::c_void;

type CVPixelBufferRef = *mut c_void;

/// A safe wrapper around CVPixelBuffer with automatic memory management.
///
/// CVPixelBuffer holds video frame data in GPU memory (IOSurface-backed),
/// allowing zero-copy rendering with Metal.
#[derive(Clone)]
pub struct PixelBuffer(Retained<CVPixelBuffer>);

impl PixelBuffer {
    /// Creates a PixelBuffer from a raw CVPixelBufferRef pointer.
    ///
    /// This is used to extract CVPixelBuffer from FFmpeg VideoToolbox frames,
    /// where the pointer is stored in frame.data[3].
    ///
    /// # Safety
    /// The caller must ensure the pointer is valid and points to a CVPixelBuffer.
    pub unsafe fn from_raw_ptr(ptr: CVPixelBufferRef) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }
        Retained::retain(ptr.cast()).map(Self)
    }

    /// Returns the pixel buffer width.
    pub fn width(&self) -> usize {
        CVPixelBufferGetWidth(&self.0)
    }

    /// Returns the pixel buffer height.
    pub fn height(&self) -> usize {
        CVPixelBufferGetHeight(&self.0)
    }

    /// Returns the pixel format type (e.g., 420v, BGRA).
    pub fn pixel_format_type(&self) -> u32 {
        CVPixelBufferGetPixelFormatType(&self.0)
    }

    /// Returns a human-readable pixel format name.
    pub fn pixel_format_name(&self) -> &'static str {
        let format_type = self.pixel_format_type();

        // Try to decode as FourCC
        let format_bytes = [
            ((format_type >> 24) & 0xFF) as u8,
            ((format_type >> 16) & 0xFF) as u8,
            ((format_type >> 8) & 0xFF) as u8,
            (format_type & 0xFF) as u8,
        ];

        // Check known formats
        match format_type {
            kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => "420v (YUV 2-plane)",
            kCVPixelFormatType_32BGRA => "BGRA",
            _ => {
                // Return FourCC string for unknown formats
                if let Ok(s) = std::str::from_utf8(&format_bytes) {
                    if s.chars().all(|c| c.is_ascii_graphic()) {
                        return Box::leak(format!("{}", s).into_boxed_str());
                    }
                }
                "Unknown"
            }
        }
    }

    /// Returns a reference to the inner Retained<CVPixelBuffer>.
    ///
    /// This is useful for passing to Metal rendering code.
    pub fn inner(&self) -> &Retained<CVPixelBuffer> {
        &self.0
    }

    /// Consumes self and returns the inner Retained<CVPixelBuffer>.
    pub fn into_inner(self) -> Retained<CVPixelBuffer> {
        self.0
    }
}

impl std::ops::Deref for PixelBuffer {
    type Target = Retained<CVPixelBuffer>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for PixelBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PixelBuffer")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("format", &self.pixel_format_name())
            .finish()
    }
}

// Safety: CVPixelBuffer is thread-safe for reading
unsafe impl Send for PixelBuffer {}
unsafe impl Sync for PixelBuffer {}
