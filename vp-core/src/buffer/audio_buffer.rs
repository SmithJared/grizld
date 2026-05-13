use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::types::{AudioSample, PTS};

/// Threshold for considering buffer "nearly full" (90%)
const NEARLY_FULL_THRESHOLD: f64 = 0.9;

/// Thread-safe ring buffer for audio samples
///
/// Stores decoded audio samples for playback by the CPAL audio thread.
/// Simplified implementation using VecDeque.
#[derive(Clone)]
pub struct AudioBuffer {
    inner: Arc<Mutex<AudioBufferInner>>,
}

struct AudioBufferInner {
    samples: VecDeque<f32>, // Interleaved stereo samples (L, R, L, R, ...)
    capacity_samples: usize,
    current_pts: PTS,
    sample_rate: u32,
}

impl AudioBuffer {
    /// Create a new audio buffer with capacity in seconds
    pub fn new(capacity_seconds: f64, sample_rate: u32) -> Self {
        let capacity_samples = (capacity_seconds * sample_rate as f64 * 2.0) as usize; // *2 for stereo

        Self {
            inner: Arc::new(Mutex::new(AudioBufferInner {
                samples: VecDeque::with_capacity(capacity_samples),
                capacity_samples,
                current_pts: 0.0,
                sample_rate,
            })),
        }
    }

    /// Push audio samples into the buffer
    pub fn push(&self, audio: AudioSample) {
        let mut inner = self.inner.lock().unwrap();

        let sample_count = audio.data.len();

        // Check if buffer is nearly full - if so, reject the entire sample
        // This prevents PTS discontinuities from dropping samples in the middle
        let would_overflow = inner.samples.len() + sample_count > inner.capacity_samples;
        if would_overflow {
            // CRITICAL: Update PTS even when rejecting to maintain clock continuity
            // Calculate the duration of the rejected samples and advance PTS accordingly
            let frames_in_sample = sample_count / 2; // Stereo
            let sample_duration = frames_in_sample as f64 / inner.sample_rate as f64;
            inner.current_pts = audio.pts + sample_duration;

            tracing::warn!(
                "Audio buffer nearly full ({:.2}s), rejecting new samples at PTS {:.3}, advancing PTS to {:.3}",
                inner.samples.len() as f64 / (inner.sample_rate as f64 * 2.0),
                audio.pts,
                inner.current_pts
            );
            return;
        }

        // Update PTS to the start of this sample
        inner.current_pts = audio.pts;

        for sample in audio.data {
            inner.samples.push_back(sample);
        }

        tracing::debug!("Pushed {} audio samples at PTS {:.3}, buffer now has {} samples ({:.2}s)",
            sample_count, audio.pts, inner.samples.len(),
            inner.samples.len() as f64 / (inner.sample_rate as f64 * 2.0));
    }

    /// Pop samples for audio output
    ///
    /// Returns the requested number of samples (filled with silence if not enough available),
    /// the current PTS for clock synchronization, and the number of actual (non-silence) samples.
    pub fn pop(&self, count: usize) -> (Vec<f32>, PTS, usize) {
        let mut inner = self.inner.lock().unwrap();

        let available = inner.samples.len().min(count);
        let mut output = Vec::with_capacity(count);

        for _ in 0..available {
            if let Some(sample) = inner.samples.pop_front() {
                output.push(sample);
            }
        }

        // Fill remaining with silence if we don't have enough samples
        if output.len() < count {
            output.resize(count, 0.0);
        }

        // Calculate PTS based on how many samples we've consumed
        let frames_consumed = available / 2; // Stereo
        let time_consumed = frames_consumed as f64 / inner.sample_rate as f64;
        let current_pts = inner.current_pts;

        // Update PTS to reflect consumed samples (only if we actually consumed samples)
        if available > 0 {
            inner.current_pts += time_consumed;
        }

        (output, current_pts, available)
    }

    /// Get the number of samples currently in the buffer
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.samples.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check if the buffer is nearly full (>90%)
    pub fn is_nearly_full(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.samples.len() > (inner.capacity_samples as f64 * NEARLY_FULL_THRESHOLD) as usize
    }

    /// Get the current PTS
    pub fn current_pts(&self) -> PTS {
        let inner = self.inner.lock().unwrap();
        inner.current_pts
    }

    /// Clear all samples from the buffer
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.samples.clear();
    }

    /// Reset the buffer and PTS
    pub fn reset(&self, pts: PTS) {
        let mut inner = self.inner.lock().unwrap();
        inner.samples.clear();
        inner.current_pts = pts;
    }

    /// Get the approximate duration of buffered audio in seconds
    pub fn buffered_duration(&self) -> f64 {
        let inner = self.inner.lock().unwrap();
        let frames = inner.samples.len() / 2; // Stereo
        frames as f64 / inner.sample_rate as f64
    }
}

impl super::Buffer for AudioBuffer {
    fn clear(&self) {
        AudioBuffer::clear(self)
    }

    fn len(&self) -> usize {
        AudioBuffer::len(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_buffer_push_pop() {
        let buffer = AudioBuffer::new(1.0, 44100);

        // Push some samples
        let samples = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]; // 3 stereo frames
        let audio = AudioSample::new(0.0, samples.clone(), 44100);
        buffer.push(audio);

        assert_eq!(buffer.len(), 6);

        // Pop 4 samples
        let (popped, _pts, actual) = buffer.pop(4);
        assert_eq!(popped, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(actual, 4);
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn test_audio_buffer_underrun() {
        let buffer = AudioBuffer::new(1.0, 44100);

        // Request more samples than available (should fill with silence)
        let (popped, _pts, actual) = buffer.pop(100);
        assert_eq!(popped.len(), 100);
        assert_eq!(actual, 0); // No real samples
        assert!(popped.iter().all(|&s| s == 0.0));
    }
}
