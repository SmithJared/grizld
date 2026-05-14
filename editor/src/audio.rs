use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use std::sync::{Arc, Mutex};
use vp_core::cache::AudioCache;
use vp_core::sync::PlaybackClock;

/// Shared audio state that can be updated when switching buffers
#[derive(Clone)]
pub struct SharedAudioState {
    cache: Arc<Mutex<Option<Arc<AudioCache>>>>,
    clock: Arc<Mutex<Option<Arc<PlaybackClock>>>>,
}

impl SharedAudioState {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            clock: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the active audio buffer and clock
    pub fn set_active(&self, buffer: Arc<AudioCache>, clock: Arc<PlaybackClock>) {
        *self.cache.lock().unwrap() = Some(buffer);
        *self.clock.lock().unwrap() = Some(clock);
    }

    /// Clear the active buffer (when no video is playing)
    pub fn clear(&self) {
        *self.cache.lock().unwrap() = None;
        *self.clock.lock().unwrap() = None;
    }

    fn get_cache(&self) -> Option<Arc<AudioCache>> {
        self.cache.lock().unwrap().clone()
    }

    fn get_clock(&self) -> Option<Arc<PlaybackClock>> {
        self.clock.lock().unwrap().clone()
    }
}

pub struct AudioOutput {
    _stream: Stream,
    _shared_state: SharedAudioState,
}

impl AudioOutput {
    /// Initialize CPAL audio output with shared state
    pub fn new(shared_state: SharedAudioState) -> Result<Self, String> {
        let host = cpal::default_host();

        let device = host
            .default_output_device()
            .ok_or("No audio output device available")?;

        tracing::info!("Using audio device: {}", device.name().unwrap_or_default());

        let config = device
            .default_output_config()
            .map_err(|e| format!("Failed to get audio config: {}", e))?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        tracing::info!("Audio config: {} Hz, {} channels", sample_rate, channels);

        let config: StreamConfig = config.into();

        let state_for_callback = shared_state.clone();

        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    audio_callback(data, &state_for_callback)
                },
                |err| {
                    tracing::error!("Audio stream error: {}", err);
                },
                None,
            )
            .map_err(|e| format!("Failed to build audio stream: {}", e))?;

        stream
            .play()
            .map_err(|e| format!("Failed to start audio stream: {}", e))?;

        tracing::info!("Audio output initialized and playing");

        Ok(Self {
            _stream: stream,
            _shared_state: shared_state,
        })
    }
}

fn audio_callback(output: &mut [f32], shared_state: &SharedAudioState) {
    // Get the current active buffer and clock
    let audio_cache = match shared_state.get_cache() {
        Some(buffer) => buffer,
        None => {
            // No active buffer, output silence
            output.fill(0.0);
            return;
        }
    };

    let clock = match shared_state.get_clock() {
        Some(clock) => clock,
        None => {
            // No active clock, output silence
            output.fill(0.0);
            return;
        }
    };

    // Only output audio if playing
    let clock_state = clock.state();
    if !clock_state.is_playing() {
        // Fill with silence when paused/stopped
        output.fill(0.0);
        tracing::trace!(
            "🔇 Audio callback: outputting silence (state={:?})",
            clock_state
        );
        return;
    }

    tracing::trace!("🔊 Audio callback: playing (state={:?})", clock_state);

    // Pop samples from the buffer (returns actual samples + silence padding, plus count of real samples)
    let (samples, pts, actual_count) = audio_cache.pop(output.len());

    // Copy samples to output
    for (i, &sample) in samples.iter().enumerate() {
        if i < output.len() {
            output[i] = sample;
        }
    }

    // Only update the playback clock if we got real audio samples (not silence padding)
    if actual_count > 0 {
        // Calculate the PTS at the middle of the buffer being played
        // This accounts for the fact that these samples will be playing over the next callback period
        let frames_in_buffer = actual_count / 2; // Stereo
        let duration = frames_in_buffer as f64 / 48000.0;
        let mid_pts = pts + (duration * 0.5);

        clock.update_from_audio(mid_pts);
        tracing::trace!("Audio playing at PTS {:.2}s", pts);
    } else {
        // No audio data available - buffer underrun (keep warning, it's important)
        tracing::warn!("Audio buffer underrun",);
    }
}
