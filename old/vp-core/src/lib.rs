//! VP Core - Video Player Core Library
//!
//! A simplified, testable video player core focused on demuxing, decoding,
//! buffering, and playback. Supports both software (FFmpeg) and hardware
//! (VideoToolbox on macOS) decoding.
//!
//! # Architecture
//!
//! - **clock**: Unified timing mechanism
//! - **error**: Custom error types
//! - **demux**: FFmpeg-based demuxer
//! - **decode**: Video decoder trait with software/hardware implementations
//! - **buffer**: Ring buffer for decoded video frames
//! - **playback**: Playback controller and background decode thread
//! - **cli**: Command-line player for testing
//!
//! # Example
//!
//! ```no_run
//! use vp_core::{PlaybackController, BufferConfig, DecoderPreference};
//!
//! let config = BufferConfig::default();
//! let mut controller = PlaybackController::new(
//!     "video.mp4",
//!     config,
//!     DecoderPreference::Auto,
//! ).unwrap();
//!
//! controller.play().unwrap();
//! ```

// Core modules
pub mod clock;
pub mod error;

// Media processing modules
pub mod buffer;
pub mod frame;
pub mod decoder;
pub mod input;
pub mod audio;
pub mod render;
pub mod util;

// Re-export commonly used types
pub use clock::{Clock, SystemClock};
pub use error::VpError;
pub use buffer::{FrameBuffer, BufferConfig};
pub use decoder::{VideoToolboxDecoder, FFmpegAudioDecoder, DecoderError};
