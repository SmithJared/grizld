use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{context::Context as ScalerContext, flag::Flags};
use ffmpeg::util::frame::video::Video as AVFrame;

use crate::error::VpResult;
use crate::types::{PixelFormat, VideoFrame};

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
    /// Create a new video decoder from a stream, attempting hardware acceleration
    pub fn new(stream: &ffmpeg::Stream) -> VpResult<Self> {
        let parameters = stream.parameters();
        let codec_id = parameters.id();

        // Try to find a hardware decoder first (e.g., h264_videotoolbox on macOS)
        let decoder_result: Result<ffmpeg::decoder::Video, ffmpeg::Error> =
            Self::try_hardware_decoder(codec_id, stream)
            .or_else(|e| {
                tracing::warn!("Hardware decoder not available: {}, falling back to software", e);
                let context = ffmpeg::codec::context::Context::from_parameters(parameters)?;
                context.decoder().video()
            });

        let decoder = decoder_result?;
        let codec_name = decoder.codec()
            .map(|c| c.name().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let is_hardware = codec_name.contains("videotoolbox") ||
                          codec_name.contains("qsv") ||
                          codec_name.contains("nvdec") ||
                          codec_name.contains("vaapi");

        tracing::info!(
            "Video decoder: {} ({})",
            codec_name,
            if is_hardware { "HARDWARE" } else { "software" }
        );

        let width = decoder.width();
        let height = decoder.height();
        let input_format = decoder.format();
        let output_format = Pixel::RGBA;

        // Use FAST_BILINEAR for better performance, especially on 4K
        let scaler = ScalerContext::get(
            input_format,
            width,
            height,
            output_format,
            width,
            height,
            Flags::FAST_BILINEAR,
        )?;

        let time_base = stream.time_base();
        let time_base_f64 = time_base.numerator() as f64 / time_base.denominator() as f64;

        tracing::info!(
            "Decoder setup: {}x{}, input: {:?}, output: {:?}, time_base: {:.6}",
            width, height, input_format, output_format, time_base_f64
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

    /// Try to create a hardware decoder for the given codec
    fn try_hardware_decoder(
        codec_id: ffmpeg::codec::Id,
        stream: &ffmpeg::Stream,
    ) -> Result<ffmpeg::decoder::Video, ffmpeg::Error> {
        // On macOS, try VideoToolbox variants
        #[cfg(target_os = "macos")]
        {
            let hw_codec_name = match codec_id {
                ffmpeg::codec::Id::H264 => Some("h264_videotoolbox"),
                ffmpeg::codec::Id::HEVC => Some("hevc_videotoolbox"),
                _ => None,
            };

            if let Some(name) = hw_codec_name {
                if let Some(codec) = ffmpeg::codec::decoder::find_by_name(name) {
                    tracing::info!("Found hardware codec: {}", name);
                    let mut context = ffmpeg::codec::context::Context::new_with_codec(codec);
                    unsafe {
                        (*context.as_mut_ptr()).time_base = stream.time_base().into();
                    }
                    return context.decoder().video();
                }
            }
        }

        // On Linux, try VAAPI variants
        #[cfg(target_os = "linux")]
        {
            let hw_codec_name = match codec_id {
                ffmpeg::codec::Id::H264 => Some("h264_vaapi"),
                ffmpeg::codec::Id::HEVC => Some("hevc_vaapi"),
                _ => None,
            };

            if let Some(name) = hw_codec_name {
                if let Some(codec) = ffmpeg::codec::decoder::find_by_name(name) {
                    tracing::info!("Found hardware codec: {}", name);
                    let mut context = ffmpeg::codec::context::Context::new_with_codec(codec);
                    unsafe {
                        (*context.as_mut_ptr()).time_base = stream.time_base().into();
                    }
                    return context.decoder().video();
                }
            }
        }

        Err(ffmpeg::Error::DecoderNotFound)
    }

    /// Decode a packet into video frames
    ///
    /// Returns a vector of decoded frames (may be empty or contain multiple frames).
    pub fn decode(&mut self, packet: &ffmpeg::packet::Packet) -> VpResult<Vec<VideoFrame>> {
        // Track decode timing for performance monitoring
        let decode_start = std::time::Instant::now();

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

        // Log decode performance with more detail
        if !frames.is_empty() {
            let total_time = decode_start.elapsed().as_secs_f64() * 1000.0; // ms
            let avg_time = total_time / frames.len() as f64;

            // Log every frame decode time
            static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let frame_num = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            if frame_num % 30 == 0 {  // Log every 30 frames (roughly once per second at 30fps)
                tracing::info!(
                    "DECODE: {}x{} frame took {:.2}ms (packet had {} frames)",
                    self.width, self.height, avg_time, frames.len()
                );
            }
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
