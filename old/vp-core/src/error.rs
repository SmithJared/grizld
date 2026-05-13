//! Error types for vp_core
//!
//! Provides a unified error type for all operations in the video player core.

use thiserror::Error;
use crate::buffer::BufferError;
use crate::decoder::DecoderError;
use crate::audio::AudioStreamError;
use crate::input::InputError;

/// Main error type for vp_core
#[derive(Debug, Error)]
pub enum VpError {
    #[error("Input error: {0}")]
    Input(#[from] InputError),

    #[error("Decoder error: {0}")]
    Decode(#[from] DecoderError),
    
    #[error("Buffer error: {0}")]
    Buffer(#[from] BufferError),
    
    #[error("Audio Stream error: {0}")]
    AudioStream(#[from] AudioStreamError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Other error: {0}")]
    Other(String),
}

// impl<T> From<crossbeam_channel::SendError<T>> for VpError {
//     fn from(err: crossbeam_channel::SendError<T>) -> Self {
//         VpError::SendError(err.to_string())
//     }
// }

