mod audio;
mod ffmpeg;
mod video_toolbox;

use tracing::{error};
use video_sys::ffmpeg::{self as ffmpeg_util, IsEgain, IsEof};

pub use video_toolbox::VideoToolboxDecoder;
pub use audio::FFmpegAudioDecoder;

use crate::{
    frame::ExtractedFrame,
    decoder::{
        ffmpeg::FFmpegDecoderError,
        video_toolbox::VideoToolboxDecoderError,
        audio::{AudioDecoderError},
    },
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecoderError {
    #[error("Hardware decoder error {0}")]
    VideoToolboxDecoder(#[from] VideoToolboxDecoderError),
    #[error("Software decoder error")]
    FFmpegDecoder(#[from] FFmpegDecoderError),
    #[error("Audio decoder error")]
    AudioDecoder(#[from] AudioDecoderError),
}

impl IsEgain for DecoderError {
    fn is_egain(&self) -> bool {
        match self {
            DecoderError::FFmpegDecoder(e) => e.is_egain(),
            DecoderError::VideoToolboxDecoder(e) => e.is_egain(),
            _ => false,
        }
    }
}

impl IsEof for DecoderError {
    fn is_eof(&self) -> bool {
        match self {
            DecoderError::FFmpegDecoder(e) => e.is_eof(),
            DecoderError::VideoToolboxDecoder(e) => e.is_eof(),
            _ => false,
        }
    }
}

/// Video decoder trait
pub trait VideoDecoder: Send {
    // Send the packet to the decoder
    fn send_packet(&mut self, packet: &ffmpeg_util::Packet) -> Result<(), DecoderError>;
    // Receive a decoded frame from the decoder
    fn receive_frame(&mut self, frame: &mut ffmpeg_util::Frame) -> Result<(), DecoderError>;
    // Extract the frame source (ie. CVPixelBuffer) from the decoded frame
    fn extract_frame_source(
        &mut self,
        frame: &ffmpeg_util::Frame,
    ) -> Result<ExtractedFrame, DecoderError>;
    fn flush(&mut self) -> ();

    fn decode_packet(&mut self, packet: &ffmpeg_util::Packet) -> Result<ExtractedFrame, DecoderError> {
        self.send_packet(packet)?;
        let mut decoded_frame = ffmpeg_util::Frame::empty();
        self.receive_frame(&mut decoded_frame)?;
        self.extract_frame_source(&decoded_frame)
    }
}

