use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::types::{AudioSample, PTS};

const AUDIO_BUFFER_SECONDS: f64 = 2.0;
const SAMPLE_RATE: u32 = 48000;

/// Threshold for considering ahead cache "nearly full"
const NEARLY_FULL_THRESHOLD: f64 = 0.9;

/// Thread-safe split audio cache.
///
/// Architecture:
///
/// Ahead Cache (soft bounded)
/// - Future decoded audio waiting for playback
/// - May overflow under burst decode
/// - Decoder pressure handled externally
///
/// Behind Cache (hard bounded)
/// - Recently played audio history
/// - Used for rewind/scrubbing/debugging
/// - Aggressively pruned
///
/// Audio lifecycle:
///
/// decode -> ahead -> playback/pop -> behind -> eviction
#[derive(Clone)]
pub struct AudioCache {
    inner: Arc<Mutex<AudioCacheInner>>,
}

struct AudioCacheInner {
    /// Future audio samples waiting for playback
    ahead: VecDeque<f32>,

    /// Already-played audio history
    behind: VecDeque<f32>,

    /// Soft limit for ahead cache
    ahead_soft_capacity_samples: usize,

    /// Hard limit for behind cache
    behind_hard_capacity_samples: usize,

    /// Current playback PTS (PTS of samples being played now)
    current_playback_pts: PTS,

    /// PTS of the most recently decoded sample (end of ahead buffer)
    newest_decoded_pts: PTS,

    sample_rate: u32,
}

impl AudioCache {
    /// Create a new split audio cache.
    ///
    /// Capacities are specified in seconds.
    pub fn new(
        ahead_capacity_seconds: f64,
        behind_capacity_seconds: f64,
        sample_rate: u32,
    ) -> Self {
        let ahead_soft_capacity_samples =
            (ahead_capacity_seconds * sample_rate as f64 * 2.0) as usize;

        let behind_hard_capacity_samples =
            (behind_capacity_seconds * sample_rate as f64 * 2.0) as usize;

        Self {
            inner: Arc::new(Mutex::new(AudioCacheInner {
                ahead: VecDeque::with_capacity(ahead_soft_capacity_samples),

                behind: VecDeque::with_capacity(behind_hard_capacity_samples),

                ahead_soft_capacity_samples,
                behind_hard_capacity_samples,

                current_playback_pts: 0.0,
                newest_decoded_pts: 0.0,
                sample_rate,
            })),
        }
    }

    // =========================================================
    // PUSH
    // =========================================================

    /// Push decoded audio into ahead cache.
    ///
    /// Ahead cache may overflow its soft capacity.
    pub fn push(&self, audio: AudioSample) {
        let mut inner = self.inner.lock().unwrap();

        let sample_count = audio.data.len();

        let len_before = inner.ahead.len();

        // If buffer was empty, this is the first sample - set playback PTS
        if inner.ahead.is_empty() {
            inner.current_playback_pts = audio.pts;
            tracing::debug!("🔊 AudioCache: buffer was empty, setting playback_pts={:.3}", audio.pts);
        } else {
            // Calculate what the expected PTS should be based on buffer duration
            let buffered_frames = inner.ahead.len() / 2;
            let buffered_duration = buffered_frames as f64 / inner.sample_rate as f64;
            let expected_pts = inner.current_playback_pts + buffered_duration;
            let pts_diff = audio.pts - expected_pts;

            tracing::debug!(
                "🔊 AudioCache: decoder_pts={:.3}, expected_pts={:.3}, diff={:.3}, buffered_dur={:.3}",
                audio.pts, expected_pts, pts_diff, buffered_duration
            );
        }

        // Track the PTS of the newest decoded audio (end of buffer)
        inner.newest_decoded_pts = audio.pts;

        for sample in audio.data {
            inner.ahead.push_back(sample);
        }

        tracing::debug!(
            "🔊 AudioCache: pushed {} samples at PTS {:.3} (ahead {} -> {}, playback_pts={:.3})",
            sample_count,
            audio.pts,
            len_before,
            inner.ahead.len(),
            inner.current_playback_pts,
        );
    }

    // =========================================================
    // PLAYBACK / CONSUME
    // =========================================================

    /// Pop samples for audio playback.
    ///
    /// Behavior:
    /// - Removes samples from ahead cache
    /// - Moves consumed samples into behind cache
    /// - Hard caps behind cache
    /// - Fills underruns with silence
    ///
    /// Returns:
    /// - output samples
    /// - playback PTS
    /// - actual sample count (non-silence)
    pub fn pop(&self, count: usize) -> (Vec<f32>, PTS, usize) {
        let mut inner = self.inner.lock().unwrap();

        let available = inner.ahead.len().min(count);

        let mut output = Vec::with_capacity(count);

        for _ in 0..available {
            if let Some(sample) = inner.ahead.pop_front() {
                output.push(sample);

                // Move consumed sample into behind cache
                inner.behind.push_back(sample);
            }
        }

        // Hard-cap behind cache
        while inner.behind.len() > inner.behind_hard_capacity_samples {
            inner.behind.pop_front();
        }

        // Underrun protection
        if output.len() < count {
            output.resize(count, 0.0);
        }

        let frames_consumed = available / 2;

        let time_consumed = frames_consumed as f64 / inner.sample_rate as f64;

        // Return the PTS of the audio being played NOW (start of this buffer)
        let playback_pts = inner.current_playback_pts;

        // Advance playback PTS by the time consumed
        if available > 0 {
            inner.current_playback_pts += time_consumed;
        }

        tracing::trace!(
            "🔊 AudioCache: pop({}) -> actual={}, pts={:.3}, ahead={}, behind={}",
            count,
            available,
            playback_pts,
            inner.ahead.len(),
            inner.behind.len(),
        );

        (output, playback_pts, available)
    }

    // =========================================================
    // LOOKUPS
    // =========================================================

    /// Peek upcoming samples without consuming.
    pub fn peek_ahead(&self, count: usize) -> Vec<f32> {
        let inner = self.inner.lock().unwrap();

        inner.ahead.iter().take(count).copied().collect()
    }

    /// Peek recently played samples.
    pub fn peek_behind(&self, count: usize) -> Vec<f32> {
        let inner = self.inner.lock().unwrap();

        inner.behind.iter().rev().take(count).copied().collect()
    }

    pub fn current_pts(&self) -> PTS {
        let inner = self.inner.lock().unwrap();
        inner.current_playback_pts
    }

    // =========================================================
    // CACHE HEALTH
    // =========================================================

    /// Ahead cache exceeded soft capacity.
    pub fn ahead_is_full(&self) -> bool {
        let inner = self.inner.lock().unwrap();

        inner.ahead.len() >= inner.ahead_soft_capacity_samples
    }

    /// Ahead cache exceeded 90% soft capacity.
    pub fn ahead_is_nearly_full(&self) -> bool {
        let inner = self.inner.lock().unwrap();

        inner.ahead.len()
            > (inner.ahead_soft_capacity_samples as f64 * NEARLY_FULL_THRESHOLD) as usize
    }

    /// Samples over ahead soft capacity.
    pub fn ahead_overflow_amount(&self) -> usize {
        let inner = self.inner.lock().unwrap();

        inner
            .ahead
            .len()
            .saturating_sub(inner.ahead_soft_capacity_samples)
    }

    /// Decoder should refill when ahead cache low.
    pub fn needs_refill(&self) -> bool {
        self.ahead_duration() < 1.0
    }

    /// Critical starvation threshold.
    pub fn is_critically_low(&self) -> bool {
        self.ahead_duration() < 0.3
    }

    // =========================================================
    // DURATIONS
    // =========================================================

    pub fn ahead_duration(&self) -> f64 {
        let inner = self.inner.lock().unwrap();

        let frames = inner.ahead.len() / 2;

        frames as f64 / inner.sample_rate as f64
    }

    pub fn behind_duration(&self) -> f64 {
        let inner = self.inner.lock().unwrap();

        let frames = inner.behind.len() / 2;

        frames as f64 / inner.sample_rate as f64
    }

    pub fn ahead_capacity_duration(&self) -> f64 {
        let inner = self.inner.lock().unwrap();

        inner.ahead_soft_capacity_samples as f64 / (inner.sample_rate as f64 * 2.0)
    }

    pub fn behind_capacity_duration(&self) -> f64 {
        let inner = self.inner.lock().unwrap();

        inner.behind_hard_capacity_samples as f64 / (inner.sample_rate as f64 * 2.0)
    }

    // =========================================================
    // STATS
    // =========================================================

    pub fn ahead_len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.ahead.len()
    }

    pub fn behind_len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.behind.len()
    }

    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();

        inner.ahead.len() + inner.behind.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // =========================================================
    // MAINTENANCE
    // =========================================================

    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();

        inner.ahead.clear();
        inner.behind.clear();
    }

    pub fn reset(&self, pts: PTS) {
        let mut inner = self.inner.lock().unwrap();

        inner.ahead.clear();
        inner.behind.clear();

        inner.current_playback_pts = pts;
        inner.newest_decoded_pts = pts;
    }

    /// Aggressively trim behind cache.
    ///
    /// Useful during seeks or memory pressure.
    pub fn prune_behind(&self) {
        let mut inner = self.inner.lock().unwrap();

        while inner.behind.len() > inner.behind_hard_capacity_samples {
            inner.behind.pop_front();
        }
    }
}

impl Default for AudioCache {
    fn default() -> Self {
        let ahead_soft_capacity_samples =
            (AUDIO_BUFFER_SECONDS * SAMPLE_RATE as f64 * 2.0) as usize;

        let behind_hard_capacity_samples =
            (AUDIO_BUFFER_SECONDS * SAMPLE_RATE as f64 * 2.0) as usize;

        Self {
            inner: Arc::new(Mutex::new(AudioCacheInner {
                ahead: VecDeque::with_capacity(ahead_soft_capacity_samples),

                behind: VecDeque::with_capacity(behind_hard_capacity_samples),

                ahead_soft_capacity_samples,
                behind_hard_capacity_samples,

                current_playback_pts: 0.0,
                newest_decoded_pts: 0.0,
                sample_rate: SAMPLE_RATE,
            })),
        }
    }
}

impl super::Cache for AudioCache {
    fn clear(&self) {
        AudioCache::clear(self)
    }

    fn len(&self) -> usize {
        AudioCache::len(self)
    }
}
