use crate::{
    frame::{ExtractedAudioFrame, TimebaseUnits},
    input::{AudioStream, StreamInfo}
};
use video_sys::ffmpeg;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioDecoderError {
    #[error("Failed to create audio stream: {0}")]
    CreateStreamError(String),
    #[error("Failed to create resampler")]
    ResamplerInit(ffmpeg::Error),
    #[error("Failed to resample frame")]
    ResampleFrame(ffmpeg::Error),
    #[error("FFmpeg Error: {0}")]
    FFmpeg(#[from] ffmpeg::Error),
}



/// FFmpeg-based audio decoder implementation
pub struct FFmpegAudioDecoder {
    decoder: ffmpeg::Decoder,
}

impl FFmpegAudioDecoder {
    pub fn new(stream_info: &AudioStream) -> Result<Self, AudioDecoderError> {
        // Create decoder from codec parameters
        let decoder = stream_info.codec_parameters().audio_decoder()?;

        Ok(Self {
            decoder,
        })
    }

    /// Extract and convert audio samples from FFmpeg frame to f32
    /// This now handles resampling to 48kHz
    fn extract_samples(
        &mut self,
        frame: &ffmpeg::AudioFrame,
    ) -> Result<Vec<f32>, AudioDecoderError> {
        let samples = frame.samples();
        let channels = frame.channels() as usize;
        let format = frame.format();

        let mut result = Vec::with_capacity(samples * channels);

        // Handle different audio formats and convert to f32
        match format {
            ffmpeg::Sample::F32(ffmpeg::sample::Type::Packed) => {
                // F32 packed - direct copy
                let data = frame.data(0);
                for i in 0..(samples * channels) {
                    let offset = i * 4;
                    if offset + 4 <= data.len() {
                        let bytes = [
                            data[offset],
                            data[offset + 1],
                            data[offset + 2],
                            data[offset + 3],
                        ];
                        result.push(f32::from_le_bytes(bytes));
                    }
                }
            }
            ffmpeg::Sample::I16(ffmpeg::sample::Type::Packed) => {
                // I16 packed - convert to f32
                let data = frame.data(0);
                for i in 0..(samples * channels) {
                    let offset = i * 2;
                    if offset + 2 <= data.len() {
                        let bytes = [data[offset], data[offset + 1]];
                        let sample_i16 = i16::from_le_bytes(bytes);
                        result.push(sample_i16 as f32 / 32768.0);
                    }
                }
            }
            ffmpeg::Sample::I16(ffmpeg::sample::Type::Planar) => {
                // I16 planar - interleave channels
                for i in 0..samples {
                    for ch in 0..channels {
                        let data = frame.data(ch);
                        let offset = i * 2;
                        if offset + 2 <= data.len() {
                            let bytes = [data[offset], data[offset + 1]];
                            let sample_i16 = i16::from_le_bytes(bytes);
                            result.push(sample_i16 as f32 / 32768.0);
                        }
                    }
                }
            }
            ffmpeg::Sample::F32(ffmpeg::sample::Type::Planar) => {
                // F32 planar - interleave channels
                for i in 0..samples {
                    for ch in 0..channels {
                        let data = frame.data(ch);
                        let offset = i * 4;
                        if offset + 4 <= data.len() {
                            let bytes = [
                                data[offset],
                                data[offset + 1],
                                data[offset + 2],
                                data[offset + 3],
                            ];
                            result.push(f32::from_le_bytes(bytes));
                        }
                    }
                }
            }
            _ => {
                // Unsupported format - return silence
                tracing::warn!("Unsupported audio format {:?}, returning silence", format);
                result.resize(samples * channels, 0.0);
            }
        }

        Ok(result)
    }

    pub fn send_packet(&mut self, packet: &ffmpeg::Packet) -> Result<(), AudioDecoderError> {
        self.decoder.send_packet(packet).map_err(|e| AudioDecoderError::FFmpeg(e))
    } 

    pub fn receive_frame(&mut self, frame: &mut ffmpeg::Frame) -> Result<(), AudioDecoderError> {
        self.decoder.receive_frame(frame).map_err(|e| AudioDecoderError::FFmpeg(e))
    }

    pub fn decode_packet(
        &mut self,
        packet: &ffmpeg::Packet,
    ) -> Result<Vec<ExtractedAudioFrame>, AudioDecoderError> {
        // Send packet to decoder
        self.send_packet(packet)?;

        // Receive decoded frames
        let mut frames = Vec::new();
        loop {
            let mut decoded_frame = ffmpeg::AudioFrame::empty();
            match self.decoder.receive_frame(&mut decoded_frame) {
                Ok(_) => {
                    // Extract and resample samples
                    let samples = self.extract_samples(&decoded_frame)?;

                    let pts = TimebaseUnits(decoded_frame.pts().unwrap_or(0));

                    let frame = ExtractedAudioFrame {
                        pts,
                        samples,
                        sample_rate: decoded_frame.rate(),
                        channels: decoded_frame.channels(),
                    };
                    frames.push(frame);
                }
                Err(_) => {
                    // No more frames available
                    break;
                }
            }
        }

        Ok(frames)
    }

    pub fn flush(&mut self) -> () {
        self.decoder.flush()
    }
}
