use std::sync::Arc;

use crate::{
    Clock, buffer::AudioBuffer, clock::AudioClock, frame::{AV_TIME_BASE, Microseconds}
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioStreamError {
    #[error("Failed to create audio stream: {0}")]
    CreateStreamError(String),
    #[error("Audio Stream Error: {0}")]
    AudioStreamPlay(#[from] cpal::PlayStreamError),
    #[error("Audio Stream Error: {0}")]
    AudioStreamPause(#[from] cpal::PauseStreamError),
}

#[derive(Clone)]
pub struct CpalStream {
    buffer: AudioBuffer,
    device: cpal::Device,
    internal_buffer_size: u32,
    stream: Arc<Option<cpal::Stream>>,
}

impl CpalStream {
    pub fn new(buffer: AudioBuffer) -> Result<Self, AudioStreamError> {
        let host = cpal::default_host();

        let device = match host.default_output_device() {
            Some(device) => {
                tracing::info!(
                    "Using audio device: {}",
                    device.name().unwrap_or_else(|_| "Unknown".to_string())
                );
                device
            }
            None => {
                tracing::warn!("No audio output device found");
                return Err(AudioStreamError::CreateStreamError(
                    "No audio output device found".to_string(),
                ));
            }
        };

        Ok(Self {
            buffer,
            device,
            internal_buffer_size: 512,
            stream: Arc::new(None),
        })
    }

    pub fn start(&mut self) -> Result<(), AudioStreamError> {
        if let Some(stream) = &*self.stream {
            stream
                .play()
                .map_err(|e| AudioStreamError::CreateStreamError(e.to_string()))?;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), AudioStreamError> {
        if let Some(stream) = &*self.stream {
            stream
                .pause()
                .map_err(|e| AudioStreamError::CreateStreamError(e.to_string()))?;
        }
        Ok(())
    }

    pub fn switch_buffer(
        &mut self,
        new_buffer: AudioBuffer,
        channels: u16,
        sample_rate: u32,
        audio_clock: AudioClock,
    ) -> Result<(), AudioStreamError> {
        // Stop current stream
        self.stop()?;

        // Update buffer reference
        self.buffer = new_buffer;

        // Reload with new configuration
        self.load_file(channels, sample_rate, audio_clock)?;

        Ok(())
    }

    pub fn load_file(
        &mut self,
        channels: u16,
        sample_rate: u32,
        audio_clock: AudioClock,
    ) -> Result<(), AudioStreamError> {
        let config = cpal::StreamConfig {
            channels: channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Fixed(self.internal_buffer_size),
        };
        self.stop()?;

        let buffer = self.buffer.clone();

        let stream = self
            .device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // This callback runs on cpal's audio thread

                    // If paused, just fill with silence
                    if audio_clock.is_paused() {
                        data.fill(0.0);
                        return;
                    }

                    let samples_needed = data.len() / channels as usize;
                    let current_time = audio_clock.time();

                    // Pull samples and skip any that are before the current clock time
                    // This handles the case where we've seeked forward but old samples
                    // are still in the buffer
                    let mut samples_to_use = None;
                    
                    while samples_to_use.is_none() {
                        if let Some((samples, pts)) = buffer.pull_samples_with_pts(samples_needed) {
                            // Check if these samples are in the past
                            // Calculate the end time of this sample block
                            let sample_duration_us = (samples.len() as f64 / (channels as f64 * sample_rate as f64) * AV_TIME_BASE as f64) as i64;
                            let sample_end_time = Microseconds(pts.0 + sample_duration_us);
                            
                            if sample_end_time < current_time {
                                // These samples are entirely in the past, skip them
                                tracing::trace!(
                                    "Skipping old audio samples: pts={:.3}s, end={:.3}s, current={:.3}s",
                                    pts, sample_end_time, current_time
                                );
                                continue;
                            } else if pts < current_time && sample_end_time >= current_time {
                                // Partial overlap - we need to skip the beginning of these samples
                                let time_to_skip_us = current_time.0 - pts.0;
                                let time_to_skip_seconds = time_to_skip_us as f64 / AV_TIME_BASE as f64;
                                let samples_to_skip = (time_to_skip_seconds * sample_rate as f64 * channels as f64) as usize;
                                let samples_to_skip = samples_to_skip.min(samples.len());
                                
                                tracing::trace!(
                                    "Partially skipping audio samples: skipping {:.3}s ({} samples)",
                                    time_to_skip_seconds, samples_to_skip
                                );
                                
                                // Use only the samples after the skip point
                                let adjusted_samples = samples[samples_to_skip..].to_vec();
                                let adjusted_pts = current_time;
                                samples_to_use = Some((adjusted_samples, adjusted_pts));
                            } else {
                                // These samples are current or in the future, use them
                                samples_to_use = Some((samples, pts));
                            }
                        } else {
                            // Buffer underrun - no samples available
                            break;
                        }
                    }

                    if let Some((samples, pts)) = samples_to_use {
                        // Update clock with the PTS that was captured atomically
                        audio_clock.update_pts(pts);

                        let copy_len = samples.len().min(data.len());
                        data[..copy_len].copy_from_slice(&samples[..copy_len]);

                        // Fill remaining with silence if needed
                        if copy_len < data.len() {
                            data[copy_len..].fill(0.0);
                        }
                    } else {
                        // Buffer underrun - fill with silence and DON'T update clock
                        data.fill(0.0);
                    }
                },
                |err| {
                    tracing::error!("Audio stream error: {}", err);
                },
                None,
            )
            .map_err(|e| {
                AudioStreamError::CreateStreamError(format!("Failed to create audio stream: {}", e))
            })?;

        self.stream = Arc::new(Some(stream));
        self.start()?;
        Ok(())
    }
}

/// Create cpal audio stream with callback
pub fn create_audio_stream(
    buffer: AudioBuffer,
    channels: u16,
    sample_rate: u32,
    audio_clock: AudioClock,
) -> std::result::Result<Option<cpal::Stream>, AudioStreamError> {
    let host = cpal::default_host();

    let device = match host.default_output_device() {
        Some(device) => {
            tracing::info!(
                "Using audio device: {}",
                device.name().unwrap_or_else(|_| "Unknown".to_string())
            );
            device
        }
        None => {
            tracing::warn!("No audio output device found");
            return Ok(None);
        }
    };

    let config = cpal::StreamConfig {
        channels: channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(512),
    };

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                // This callback runs on cpal's audio thread

                // If paused, just fill with silence
                if audio_clock.is_paused() {
                    data.fill(0.0);
                    return;
                }

                let samples_needed = data.len() / channels as usize;
                let current_time = audio_clock.time();

                // Pull samples and skip any that are before the current clock time
                // This handles the case where we've seeked forward but old samples
                // are still in the buffer
                let mut samples_to_use = None;
                
                while samples_to_use.is_none() {
                    if let Some((samples, pts)) = buffer.pull_samples_with_pts(samples_needed) {
                        // Check if these samples are in the past
                        // Calculate the end time of this sample block
                        let sample_duration_us = (samples.len() as f64 / (channels as f64 * sample_rate as f64) * AV_TIME_BASE as f64) as i64;
                        let sample_end_time = Microseconds(pts.0 + sample_duration_us);
                        
                        if sample_end_time < current_time {
                            // These samples are entirely in the past, skip them
                            tracing::trace!(
                                "Skipping old audio samples: pts={:.3}s, end={:.3}s, current={:.3}s",
                                pts, sample_end_time, current_time
                            );
                            continue;
                        } else if pts < current_time && sample_end_time >= current_time {
                            // Partial overlap - we need to skip the beginning of these samples
                            let time_to_skip_us = current_time.0 - pts.0;
                            let time_to_skip_seconds = time_to_skip_us as f64 / AV_TIME_BASE as f64;
                            let samples_to_skip = (time_to_skip_seconds * sample_rate as f64 * channels as f64) as usize;
                            let samples_to_skip = samples_to_skip.min(samples.len());
                            
                            tracing::trace!(
                                "Partially skipping audio samples: skipping {:.3}s ({} samples)",
                                time_to_skip_seconds, samples_to_skip
                            );
                            
                            // Use only the samples after the skip point
                            let adjusted_samples = samples[samples_to_skip..].to_vec();
                            let adjusted_pts = current_time;
                            samples_to_use = Some((adjusted_samples, adjusted_pts));
                        } else {
                            // These samples are current or in the future, use them
                            samples_to_use = Some((samples, pts));
                        }
                    } else {
                        // Buffer underrun - no samples available
                        break;
                    }
                }

                if let Some((samples, pts)) = samples_to_use {
                    // Update clock with the PTS that was captured atomically
                    audio_clock.update_pts(pts);

                    let copy_len = samples.len().min(data.len());
                    data[..copy_len].copy_from_slice(&samples[..copy_len]);

                    // Fill remaining with silence if needed
                    if copy_len < data.len() {
                        data[copy_len..].fill(0.0);
                    }
                } else {
                    // Buffer underrun - fill with silence and DON'T update clock
                    data.fill(0.0);
                }
            },
            |err| {
                tracing::error!("Audio stream error: {}", err);
            },
            None,
        )
        .map_err(|e| {
            AudioStreamError::CreateStreamError(format!("Failed to create audio stream: {}", e))
        })?;

    tracing::info!("Audio stream created successfully");
    Ok(Some(stream))
}