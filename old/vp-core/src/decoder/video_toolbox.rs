//! Hardware video decoder (VideoToolbox for macOS)
//!
//! This module provides hardware-accelerated video decoding using FFmpeg's VideoToolbox integration.
//! FFmpeg handles the VideoToolbox API calls, and we extract IOSurface references from decoded frames.

use super::{VideoDecoder, DecoderError};
use crate::{
    frame::{ExtractedFrame, FrameData},
    input::{StreamInfo, VideoStream},
};
use thiserror::Error;
use video_sys::{
    core_video::PixelBuffer,
    ffmpeg,
    ffmpeg_sys::{
        AVHWDeviceType, AVPixelFormat, FFmpegSysError, HardwareDeviceContext, HardwareFrameBuilder,
        VideoDecoderExt,
    },
};

#[derive(Debug, Error)]
pub enum VideoToolboxDecoderError {
    #[error("FFmpeg Error: {0}")]
    FFmpeg(#[from] ffmpeg::Error),
    #[error("FFmpegSysError: {0}")]
    FFmpegSysError(#[from] FFmpegSysError),
    #[error("Frame data[3] is null - not a hardware frame")]
    NotAHardwareFrame,
}

impl ffmpeg::IsEgain for VideoToolboxDecoderError {
    fn is_egain(&self) -> bool {
        match self {
            VideoToolboxDecoderError::FFmpeg(e) => e.is_egain(),
            _ => false,
        }
    }
}

impl ffmpeg::IsEof for VideoToolboxDecoderError {
    fn is_eof(&self) -> bool {
        match self {
            VideoToolboxDecoderError::FFmpeg(e) => e.is_eof(),
            _ => false,
        }
    }
}

/// Hardware decoder using VideoToolbox (macOS)
///
/// This decoder uses FFmpeg's built-in VideoToolbox support. When FFmpeg is configured
/// with VideoToolbox, it can automatically use hardware acceleration for H.264 and HEVC.
/// We detect hardware-decoded frames and extract the IOSurface for GPU rendering.
#[cfg(target_os = "macos")]
pub struct VideoToolboxDecoder {
    decoder: ffmpeg::Decoder,
}

#[cfg(target_os = "macos")]
impl VideoToolboxDecoder {
    /// Create a new VideoToolbox decoder from stream info
    pub fn new(stream_info: &VideoStream) -> Result<Self, VideoToolboxDecoderError> {
        let decoder = stream_info.codec_parameters().video_decoder()?;

        let width = decoder.width().unwrap_or(0) as i32;
        let height = decoder.height().unwrap_or(0) as i32;

        let hw_ctx = HardwareDeviceContext::new(AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX)?;

        let mut hw_frames_ctx = HardwareFrameBuilder::new(&hw_ctx)?;
        hw_frames_ctx
            .set_format(AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX)
            .set_sw_format(AVPixelFormat::AV_PIX_FMT_NV12)
            .set_resolution(width, height);

        let hw_frames_ctx = hw_frames_ctx.init()?;

        let decoder = decoder
            .with_hw_ctx(hw_ctx)
            .with_hw_frames_ctx(hw_frames_ctx)
            .with_pix_fmt(AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX)
            .with_hw_format(AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX);

        Ok(Self { decoder })
    }

    /// Extract hardware frame and preserve CVPixelBuffer for Metal rendering
    fn extract_pixel_buffer(
        frame: &ffmpeg::Frame,
    ) -> Result<ExtractedFrame, VideoToolboxDecoderError> {
        use crate::frame::TimebaseUnits;

        let Some(cv_pixel_buffer) = PixelBuffer::from_vt_frame(frame) else {
            return Err(VideoToolboxDecoderError::NotAHardwareFrame);
        };

        let pts = TimebaseUnits(frame.pts().unwrap_or(0));

        Ok(ExtractedFrame::new(
            pts,
            frame.width(),
            frame.height(),
            FrameData::new_cvpixelbuffer(cv_pixel_buffer),
        ))
    }
}

#[cfg(target_os = "macos")]
impl VideoDecoder for VideoToolboxDecoder {
    fn send_packet(&mut self, packet: &ffmpeg::Packet) -> Result<(), DecoderError> {
        self.decoder
            .send_packet(packet)
            .map_err(|e| DecoderError::VideoToolboxDecoder(VideoToolboxDecoderError::FFmpeg(e)))
    }

    fn receive_frame(&mut self, frame: &mut ffmpeg::Frame) -> Result<(), DecoderError> {
        self.decoder
            .receive_frame(frame)
            .map_err(|e| DecoderError::VideoToolboxDecoder(VideoToolboxDecoderError::FFmpeg(e)))
    }

    fn extract_frame_source(
        &mut self,
        frame: &ffmpeg::Frame,
    ) -> Result<ExtractedFrame, DecoderError> {
        Self::extract_pixel_buffer(frame)
            .map_err(|e| DecoderError::VideoToolboxDecoder(e))
    }

    fn flush(&mut self) -> () {
        self.decoder.flush()
    }
}
