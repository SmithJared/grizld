//! FFmpeg-based software video decoder
//!
//! Decodes compressed video packets into raw YUV420p frames.

use video_sys::ffmpeg;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FFmpegDecoderError {
    #[error("FFmpeg Error: {0}")]
    FFmpeg(#[from] ffmpeg::Error),
    #[error("Scaler initialization failed: {0}")]
    ScalerInit(ffmpeg::Error),
    #[error("Scale frame failed: {0}")]
    ScaleFrame(ffmpeg::Error),
}

impl ffmpeg::IsEgain for FFmpegDecoderError {
    fn is_egain(&self) -> bool {
        match self {
            FFmpegDecoderError::FFmpeg(e) => e.is_egain(),
            _ => false,
        }
    }
}

impl ffmpeg::IsEof for FFmpegDecoderError {
    fn is_eof(&self) -> bool {
        match self {
            FFmpegDecoderError::FFmpeg(e) => e.is_eof(),
            _ => false,
        }
    }
}

