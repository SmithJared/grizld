use ffmpeg_next as ffmpeg;
use ffmpeg::format::Sample;
use ffmpeg::software::resampling::context::Context as ResamplerContext;
use ffmpeg::util::frame::audio::Audio as AVAudioFrame;
use ffmpeg::ChannelLayout;

use crate::error::{VpError, VpResult};
use crate::types::{AudioSample, PTS};

/// Audio decoder with resampling support
pub struct AudioDecoder {
    decoder: ffmpeg::decoder::Audio,
    resampler: ResamplerContext,
    output_sample_rate: u32,
    time_base: f64,
}

impl AudioDecoder {
    /// Create a new audio decoder from a stream
    pub fn new(stream: &ffmpeg::Stream, output_sample_rate: u32) -> VpResult<Self> {
        let context_decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = context_decoder.decoder().audio()?;

        let input_format = decoder.format();
        let input_rate = decoder.rate();
        let input_channel_layout = decoder.channel_layout();

        // Create resampler to stereo f32 at output_sample_rate
        let resampler = ResamplerContext::get(
            input_format,
            input_channel_layout,
            input_rate,
            Sample::F32(ffmpeg::format::sample::Type::Packed),
            ChannelLayout::STEREO,
            output_sample_rate,
        )?;

        // Calculate time_base for PTS conversion
        let time_base = stream.time_base();
        let time_base_f64 = time_base.numerator() as f64 / time_base.denominator() as f64;

        tracing::info!(
            "Created audio decoder: {} Hz, format: {:?}, time_base: {:.6}",
            input_rate,
            input_format,
            time_base_f64
        );

        Ok(Self {
            decoder,
            resampler,
            output_sample_rate,
            time_base: time_base_f64,
        })
    }

    /// Decode a packet into audio samples
    ///
    /// Returns a vector of decoded audio samples (may be empty or contain multiple samples).
    pub fn decode(&mut self, packet: &ffmpeg::packet::Packet) -> VpResult<Vec<AudioSample>> {
        self.decoder.send_packet(packet)?;

        let mut samples = Vec::new();
        let mut decoded_frame = AVAudioFrame::empty();

        while self.decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = if let Some(pts) = decoded_frame.pts() {
                pts as f64 * self.time_base
            } else {
                tracing::warn!("Audio frame missing PTS, using 0.0");
                0.0
            };

            // Resample to stereo f32
            // The resampler may buffer internally, so we need to call it and potentially flush
            let mut resampled = AVAudioFrame::empty();

            // Run the resampler - it may not produce output immediately (buffering)
            let delay_result = self.resampler.run(&decoded_frame, &mut resampled);

            // Always check if we got output, regardless of delay
            let num_samples = resampled.samples();
            if num_samples > 0 {
                // We got output! Convert to Vec<f32>
                let data = resampled.data(0);
                let num_channels = 2; // Stereo

                // data is raw bytes, convert to f32 slice
                let sample_data = unsafe {
                    std::slice::from_raw_parts(
                        data.as_ptr() as *const f32,
                        num_samples * num_channels,
                    )
                };

                samples.push(AudioSample::new(
                    pts,
                    sample_data.to_vec(),
                    self.output_sample_rate,
                ));

                tracing::debug!("Decoded audio at PTS {:.3}, {} samples", pts, num_samples);
            } else {
                // Try flushing the resampler to get buffered samples
                let mut flush_frame = AVAudioFrame::empty();
                if self.resampler.flush(&mut flush_frame).is_ok() {
                    let flush_samples = flush_frame.samples();
                    if flush_samples > 0 {
                        let data = flush_frame.data(0);
                        let sample_data = unsafe {
                            std::slice::from_raw_parts(
                                data.as_ptr() as *const f32,
                                flush_samples * 2,
                            )
                        };

                        samples.push(AudioSample::new(
                            pts,
                            sample_data.to_vec(),
                            self.output_sample_rate,
                        ));

                        tracing::debug!("Flushed {} audio samples at PTS {:.3}", flush_samples, pts);
                    }
                }

                if samples.is_empty() {
                    tracing::debug!("Resampler buffering (no output yet)");
                }
            }
        }

        Ok(samples)
    }

    /// Flush any remaining frames from the decoder
    pub fn flush(&mut self) -> VpResult<Vec<AudioSample>> {
        self.decoder.send_eof()?;

        let mut samples = Vec::new();
        let mut decoded_frame = AVAudioFrame::empty();

        while self.decoder.receive_frame(&mut decoded_frame).is_ok() {
            let pts = decoded_frame.pts().unwrap_or(0) as f64 * self.time_base;

            let mut resampled = AVAudioFrame::empty();
            if let Ok(Some(_delay)) = self.resampler.run(&decoded_frame, &mut resampled) {
                let data = resampled.data(0);
                let num_samples = resampled.samples();
                let num_channels = 2;

                let sample_data = unsafe {
                    std::slice::from_raw_parts(
                        data.as_ptr() as *const f32,
                        num_samples * num_channels,
                    )
                };

                samples.push(AudioSample::new(
                    pts,
                    sample_data.to_vec(),
                    self.output_sample_rate,
                ));
            }
        }

        Ok(samples)
    }

    pub fn sample_rate(&self) -> u32 {
        self.output_sample_rate
    }
}
