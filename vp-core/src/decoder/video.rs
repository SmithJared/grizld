use ffmpeg::format::Pixel;
use ffmpeg::util::frame::video::Video as AVFrame;
use ffmpeg_next as ffmpeg;

use crate::error::VpResult;
use crate::types::{PixelFormat, VideoFrame};

#[cfg(target_os = "macos")]
use crate::types::PixelBuffer;

#[cfg(target_os = "macos")]
use std::os::raw::c_void;

/// Video decoder with hardware acceleration support
pub struct VideoDecoder {
    decoder: ffmpeg::decoder::Video,
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
            Self::try_hardware_decoder(codec_id, stream).or_else(|e| {
                tracing::warn!(
                    "Hardware decoder not available: {}, falling back to software",
                    e
                );
                let context = ffmpeg::codec::context::Context::from_parameters(parameters)?;
                context.decoder().video()
            });

        let decoder = decoder_result?;

        let width = decoder.width();
        let height = decoder.height();
        let input_format = decoder.format();
        let output_format = Pixel::RGBA;

        let time_base = stream.time_base();
        let time_base_f64 = time_base.numerator() as f64 / time_base.denominator() as f64;

        tracing::info!(
            "Decoder setup: {}x{}, input: {:?}, output: {:?}, time_base: {:.6}",
            width,
            height,
            input_format,
            output_format,
            time_base_f64
        );

        Ok(Self {
            decoder,
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
        // On macOS, use VideoToolbox via hardware device context
        #[cfg(target_os = "macos")]
        {
            use super::hw_accel::{HardwareDeviceContext, HardwareFramesBuilder, VideoDecoderExt};
            use ffmpeg_sys_next::{AVHWDeviceType, AVPixelFormat};

            // Only try hardware acceleration for supported codecs
            match codec_id {
                ffmpeg::codec::Id::H264 | ffmpeg::codec::Id::HEVC => {
                    tracing::info!(
                        "Attempting VideoToolbox hardware acceleration for {:?}",
                        codec_id
                    );

                    // Create hardware device context
                    let hw_device_ctx =
                        HardwareDeviceContext::new(AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX)
                            .map_err(|e| {
                            tracing::warn!("Failed to create hardware device context: {}", e);
                            ffmpeg::Error::DecoderNotFound
                        })?;

                    // Get decoder from stream parameters
                    let parameters = stream.parameters();
                    let context = ffmpeg::codec::context::Context::from_parameters(parameters)?;
                    let mut decoder = context.decoder().video()?;

                    let width = decoder.width() as i32;
                    let height = decoder.height() as i32;

                    // Create hardware frames context
                    let mut hw_frames_builder = HardwareFramesBuilder::new(&hw_device_ctx)
                        .map_err(|e| {
                            tracing::warn!("Failed to create hardware frames builder: {}", e);
                            ffmpeg::Error::DecoderNotFound
                        })?;

                    hw_frames_builder
                        .set_format(AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX)
                        .set_sw_format(AVPixelFormat::AV_PIX_FMT_NV12)
                        .set_resolution(width, height);

                    let hw_frames_ctx = hw_frames_builder.init().map_err(|e| {
                        tracing::warn!("Failed to initialize hardware frames context: {}", e);
                        ffmpeg::Error::DecoderNotFound
                    })?;

                    // Configure decoder for hardware acceleration
                    decoder = decoder
                        .with_hw_device_ctx(hw_device_ctx)
                        .with_hw_frames_ctx(hw_frames_ctx)
                        .with_pix_fmt(AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX)
                        .with_hw_format_callback(AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX);

                    tracing::info!("VideoToolbox hardware decoder configured successfully");
                    return Ok(decoder);
                }
                _ => {}
            }
        }

        // On Linux, try VAAPI (not implemented yet)
        #[cfg(target_os = "linux")]
        {
            let _ = codec_id;
            let _ = stream;
            // TODO: Implement VAAPI hardware acceleration
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

            let _width = decoded_frame.width();
            let _height = decoded_frame.height();

            // Check if this is a hardware frame (macOS VideoToolbox)
            #[cfg(target_os = "macos")]
            let is_hardware_frame = unsafe {
                let frame_ptr = decoded_frame.as_ptr();
                !(*frame_ptr).data[3].is_null()
            };

            #[cfg(target_os = "macos")]
            if is_hardware_frame {
                // Extract CVPixelBuffer from hardware frame
                let pixel_buffer = unsafe {
                    let frame_ptr = decoded_frame.as_ptr();
                    let cv_pixel_buffer_ptr = (*frame_ptr).data[3] as *mut c_void;
                    PixelBuffer::from_raw_ptr(cv_pixel_buffer_ptr)
                };

                if let Some(pb) = pixel_buffer {
                    tracing::trace!(
                        "Hardware frame decoded: {}x{}, format: {}, pts: {:.3}",
                        pb.width(),
                        pb.height(),
                        pb.pixel_format_name(),
                        pts
                    );

                    let frame = VideoFrame::new_hardware(pts, pb);
                    frames.push(frame);
                    continue; // Skip software decoding path
                } else {
                    tracing::warn!("Failed to extract CVPixelBuffer from hardware frame, falling back to software");
                }
            }
        }

        // Log slow decodes at trace level
        if !frames.is_empty() {
            let total_time = decode_start.elapsed().as_secs_f64() * 1000.0; // ms
            if total_time > 50.0 {
                let avg_time = total_time / frames.len() as f64;
                tracing::warn!(
                    "Slow decode: {}x{} took {:.2}ms (packet had {} frames)",
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

        // Process remaining frames in decoder
        while self.decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = decoded_frame.pts().unwrap_or(0) as f64 * self.time_base;

            // Handle hardware frames on macOS
            #[cfg(target_os = "macos")]
            {
                let is_hardware_frame = unsafe {
                    let frame_ptr = decoded_frame.as_ptr();
                    !(*frame_ptr).data[3].is_null()
                };

                if is_hardware_frame {
                    let pixel_buffer = unsafe {
                        let frame_ptr = decoded_frame.as_ptr();
                        let cv_pixel_buffer_ptr = (*frame_ptr).data[3] as *mut c_void;
                        PixelBuffer::from_raw_ptr(cv_pixel_buffer_ptr)
                    };

                    if let Some(pb) = pixel_buffer {
                        frames.push(VideoFrame::new_hardware(pts, pb));
                        continue;
                    }
                }
            }

            // Software decoding path would go here but is not currently implemented
            tracing::warn!("Software frame in flush() - not implemented");
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
