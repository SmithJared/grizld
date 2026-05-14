use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::types::{PlaybackState, PTS};

/// Loading state for async operations (seeks, buffering)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadingState {
    /// Buffers ready, playback can proceed normally
    Ready,
    /// Seek in progress, buffers being refilled
    Seeking,
    /// Playing but buffers critically low, need urgent refill
    Buffering,
}

impl LoadingState {
    fn to_u8(self) -> u8 {
        match self {
            LoadingState::Ready => 0,
            LoadingState::Seeking => 1,
            LoadingState::Buffering => 2,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => LoadingState::Seeking,
            2 => LoadingState::Buffering,
            _ => LoadingState::Ready,
        }
    }
}

/// Audio-driven playback clock for A/V synchronization
///
/// This is a simplified MVP implementation using Mutex.
/// The CPAL audio callback updates the clock, and the video renderer
/// checks the clock to determine when to display frames.
#[derive(Clone)]
pub struct PlaybackClock {
    state: Arc<Mutex<ClockState>>,
    /// Loading state for UI feedback (lock-free for fast access)
    loading_state: Arc<AtomicU8>,
}

struct ClockState {
    /// Current playback position
    current_pts: PTS,
    /// Playback state
    state: PlaybackState,
    /// Base PTS from audio
    audio_base_pts: PTS,
    /// When the audio started playing (for calculating drift)
    audio_start_time: Instant,
}

impl PlaybackClock {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ClockState {
                current_pts: 0.0,
                state: PlaybackState::Stopped,
                audio_base_pts: 0.0,
                audio_start_time: Instant::now(),
            })),
            loading_state: Arc::new(AtomicU8::new(LoadingState::Ready.to_u8())),
        }
    }

    /// Get the current playback time
    ///
    /// If playing, this calculates the time based on how long audio has been running.
    /// If paused/stopped, returns the last set PTS.
    pub fn current_time(&self) -> PTS {
        let state = self.state.lock().unwrap();
        let pts = if state.state.is_playing() {
            // Calculate current time based on elapsed time since audio started
            let elapsed = state.audio_start_time.elapsed().as_secs_f64();
            state.audio_base_pts + elapsed
        } else {
            state.current_pts
        };

        tracing::trace!(
            "🕐 current_time: state={:?}, pts={:.3}, base={:.3}",
            state.state,
            pts,
            state.audio_base_pts
        );

        pts
    }

    /// Update the clock from the audio callback
    ///
    /// This should be called by the CPAL audio thread when it outputs audio samples.
    /// It applies a gentle correction to keep the clock in sync with audio playback
    /// without causing jarring resets.
    pub fn update_from_audio(&self, pts: PTS) {
        let mut state = self.state.lock().unwrap();
        if state.state.is_playing() {
            // Calculate how much the clock has drifted from audio
            let elapsed = state.audio_start_time.elapsed().as_secs_f64();
            let clock_pts = state.audio_base_pts + elapsed;
            let drift = pts - clock_pts;

            // Only apply correction if drift is significant (> 50ms)
            // Otherwise let the clock run freely to avoid constant resets
            if drift.abs() > 0.05 {
                tracing::debug!("🕐 Clock drift {:.3}s, correcting to audio PTS {:.3}", drift, pts);
                state.audio_base_pts = pts;
                state.audio_start_time = Instant::now();
            }

            state.current_pts = pts;
        }
    }

    /// Get the current playback state
    pub fn state(&self) -> PlaybackState {
        self.state.lock().unwrap().state
    }

    /// Set the playback state
    pub fn set_state(&self, new_state: PlaybackState) {
        let mut state = self.state.lock().unwrap();
        let old_state = state.state;

        tracing::info!(
            "🔄 set_state: {:?} → {:?} (current_pts={:.3})",
            old_state,
            new_state,
            state.current_pts
        );

        match new_state {
            PlaybackState::Playing => {
                if !state.state.is_playing() {
                    // Starting playback - reset the audio timer
                    state.audio_start_time = Instant::now();
                    state.audio_base_pts = state.current_pts;
                    tracing::info!("🔄 Starting playback: base_pts={:.3}", state.audio_base_pts);
                }
            }
            PlaybackState::Paused | PlaybackState::Stopped => {
                if state.state.is_playing() {
                    // Pausing - capture the current time
                    let elapsed = state.audio_start_time.elapsed().as_secs_f64();
                    state.current_pts = state.audio_base_pts + elapsed;
                    tracing::info!("🔄 Pausing: captured_pts={:.3}", state.current_pts);
                }
            }
        }

        state.state = new_state;

        tracing::info!("🔄 State change complete: {:?}", new_state);
    }

    /// Seek to a specific time
    pub fn seek(&self, target_pts: PTS) {
        let mut state = self.state.lock().unwrap();
        state.current_pts = target_pts;
        state.audio_base_pts = target_pts;
        state.audio_start_time = Instant::now();

        tracing::debug!("Seeked to PTS {:.3}", target_pts);
    }

    /// Reset the clock to the beginning
    pub fn reset(&self) {
        let mut state = self.state.lock().unwrap();
        state.current_pts = 0.0;
        state.audio_base_pts = 0.0;
        state.audio_start_time = Instant::now();
        state.state = PlaybackState::Stopped;
        self.loading_state.store(LoadingState::Ready.to_u8(), Ordering::Relaxed);
    }

    /// Get the current loading state
    pub fn loading_state(&self) -> LoadingState {
        LoadingState::from_u8(self.loading_state.load(Ordering::Relaxed))
    }

    /// Set the loading state
    ///
    /// Used by pull coordinator to signal seeking/buffering state to UI
    pub fn set_loading_state(&self, state: LoadingState) {
        tracing::debug!("Loading state: {:?}", state);
        self.loading_state.store(state.to_u8(), Ordering::Relaxed);
    }
}

impl Default for PlaybackClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_clock_starts_at_zero() {
        let clock = PlaybackClock::new();
        assert_eq!(clock.current_time(), 0.0);
        assert!(clock.state().is_stopped());
    }

    #[test]
    fn test_clock_play_pause() {
        let clock = PlaybackClock::new();

        clock.set_state(PlaybackState::Playing);
        assert!(clock.state().is_playing());

        thread::sleep(Duration::from_millis(100));
        let time1 = clock.current_time();
        assert!(time1 > 0.0 && time1 < 0.2);

        clock.set_state(PlaybackState::Paused);
        assert!(clock.state().is_paused());

        thread::sleep(Duration::from_millis(100));
        let time2 = clock.current_time();
        // Time should not advance when paused
        assert!((time2 - time1).abs() < 0.01);
    }

    #[test]
    fn test_clock_seek() {
        let clock = PlaybackClock::new();

        clock.seek(10.5);
        assert_eq!(clock.current_time(), 10.5);

        clock.set_state(PlaybackState::Playing);
        thread::sleep(Duration::from_millis(100));
        let time = clock.current_time();
        assert!(time > 10.5 && time < 10.7);
    }
}
