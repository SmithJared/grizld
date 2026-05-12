use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{context::Context as ScalerContext, flag::Flags};
use ffmpeg::util::frame::video::Video as AVFrame;

use crate::error::{VpError, VpResult};
use crate::types::{PixelFormat, VideoFrame, PTS};

/// Video decoder with hardware acceleration support
pub struct VideoDecoder {
    decoder: ffmpeg::decoder::Video,
    scaler: ScalerContext,
    output_format: Pixel,
    width: u32,
    height: u32,
    time_base: f64,
}

impl VideoDecoder {
    /// Create a new video decoder from a stream
    pub fn new(stream: &ffmpeg::Stream) -> VpResult<Self> {
        let context_decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = context_decoder.decoder().video()?;

        let width = decoder.width();
        let height = decoder.height();
        let output_format = Pixel::RGBA; // Always output RGBA for simplicity

        // Create scaler for pixel format conversion
        let scaler = ScalerContext::get(
            decoder.format(),
            width,
            height,
            output_format,
            width,
            height,
            Flags::BILINEAR,
        )?;

        // Calculate time_base for PTS conversion
        let time_base = stream.time_base();
        let time_base_f64 = time_base.numerator() as f64 / time_base.denominator() as f64;

        tracing::info!(
            "Created video decoder: {}x{}, format: {:?}, time_base: {:.6}",
            width,
            height,
            decoder.format(),
            time_base_f64
        );

        Ok(Self {
            decoder,
            scaler,
            output_format,
            width,
            height,
            time_base: time_base_f64,
        })
    }

    /// Decode a packet into video frames
    ///
    /// Returns a vector of decoded frames (may be empty or contain multiple frames).
    pub fn decode(&mut self, packet: &ffmpeg::packet::Packet) -> VpResult<Vec<VideoFrame>> {
        if let Err(e) = self.decoder.send_packet(packet) {
            tracing::warn!("Failed to send packet to video decoder: {}", e);
            return Ok(Vec::new());
        }

        let mut frames = Vec::new();
        let mut decoded_frame = AVFrame::empty();

        while self.decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = if let Some(pts) = decoded_frame.pts() {
                pts as f64 * self.time_base
            } else {
                tracing::warn!("Frame missing PTS, using 0.0");
                0.0
            };

            // Scale and convert to RGBA
            let mut rgb_frame = AVFrame::empty();
            if let Err(e) = self.scaler.run(&decoded_frame, &mut rgb_frame) {
                tracing::warn!("Failed to scale frame: {}", e);
                continue;
            }

            // Copy pixel data
            let data = rgb_frame.data(0);
            let linesize = rgb_frame.stride(0);
            let height = rgb_frame.height() as usize;
            let width = rgb_frame.width() as usize;

            let mut pixel_data = Vec::with_capacity(width * height * 4);

            for y in 0..height {
                let start = y * linesize;
                let end = start + (width * 4);
                pixel_data.extend_from_slice(&data[start..end]);
            }

            frames.push(VideoFrame::new(
                pts,
                pixel_data,
                self.width,
                self.height,
                PixelFormat::Rgba,
            ));

            tracing::debug!("Decoded video frame at PTS {:.3}", pts);
        }

        Ok(frames)
    }

    /// Flush any remaining frames from the decoder
    pub fn flush(&mut self) -> VpResult<Vec<VideoFrame>> {
        self.decoder.send_eof()?;

        let mut frames = Vec::new();
        let mut decoded_frame = AVFrame::empty();

        while self.decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = decoded_frame.pts().unwrap_or(0) as f64 * self.time_base;

            let mut rgb_frame = AVFrame::empty();
            self.scaler.run(&decoded_frame, &mut rgb_frame)?;

            let data = rgb_frame.data(0);
            let linesize = rgb_frame.stride(0);
            let height = rgb_frame.height() as usize;
            let width = rgb_frame.width() as usize;

            let mut pixel_data = Vec::with_capacity(width * height * 4);
            for y in 0..height {
                let start = y * linesize;
                let end = start + (width * 4);
                pixel_data.extend_from_slice(&data[start..end]);
            }

            frames.push(VideoFrame::new(
                pts,
                pixel_data,
                self.width,
                self.height,
                PixelFormat::Rgba,
            ));
        }

        Ok(frames)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}
