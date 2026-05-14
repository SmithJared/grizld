use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::types::{VideoFrame, PTS};

const FRAME_BUFFER_CAPACITY: usize = 15; // Reduced for 4K video (15 frames = ~500MB)

/// Thread-safe split frame cache.
///
/// Architecture:
///
/// Behind Cache (hard bounded)
/// - Contains frames already displayed/requested.
/// - Acts like playback history.
/// - Old/stale frames are aggressively removed.
///
/// Ahead Cache (soft bounded)
/// - Contains future frames ready for playback.
/// - May overflow during burst decode.
/// - Decoder pressure is managed externally.
///
/// Frame lifecycle:
///
/// decode -> ahead_cache -> requested/displayed -> behind_cache -> evicted
#[derive(Clone)]
pub struct FrameCache {
    inner: Arc<Mutex<FrameCacheInner>>,
}

struct FrameCacheInner {
    /// Future frames waiting to be displayed
    ahead: VecDeque<VideoFrame>,

    /// Previously displayed frames
    behind: VecDeque<VideoFrame>,

    /// Soft limit for ahead cache
    ahead_soft_capacity: usize,

    /// Hard limit for behind cache
    behind_hard_capacity: usize,
}

impl FrameCache {
    pub fn new(ahead_soft_capacity: usize, behind_hard_capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FrameCacheInner {
                ahead: VecDeque::with_capacity(ahead_soft_capacity),
                behind: VecDeque::with_capacity(behind_hard_capacity),
                ahead_soft_capacity,
                behind_hard_capacity,
            })),
        }
    }

    // =========================================================
    // PUSH
    // =========================================================

    /// Push a decoded frame into the ahead cache.
    ///
    /// Frames are kept sorted by PTS to handle B-frames.
    pub fn push(&self, frame: VideoFrame) {
        let mut inner = self.inner.lock().unwrap();

        let pts = frame.pts;

        let insert_pos = inner
            .ahead
            .iter()
            .position(|f| f.pts > pts)
            .unwrap_or(inner.ahead.len());

        inner.ahead.insert(insert_pos, frame);

        tracing::trace!(
            "📦 FrameCache: pushed frame {:.3} into ahead cache (ahead_len={})",
            pts,
            inner.ahead.len()
        );
    }

    // =========================================================
    // REQUEST / CONSUME
    // =========================================================

    /// Request the frame for playback.
    ///
    /// Behavior (idempotent):
    /// - If the last frame in behind cache is still appropriate (PTS <= target_pts),
    ///   return it again without consuming a new frame
    /// - Otherwise, find the latest frame in ahead with PTS <= target_pts
    /// - Move all frames up to and including that frame from ahead to behind
    /// - Return cloned frame
    ///
    /// This ensures calling request_frame() multiple times at the same playback
    /// position returns the same frame without draining the ahead cache.
    pub fn request_frame(&self, target_pts: PTS) -> Option<VideoFrame> {
        let mut inner = self.inner.lock().unwrap();

        // Check if we already have a frame in behind cache that's still valid
        if let Some(current_frame) = inner.behind.back() {
            // If the current frame is still appropriate for target_pts, return it again
            // We need to check if there's a frame in ahead that would be better
            if let Some(next_frame) = inner.ahead.front() {
                // Only advance if the next frame's PTS is <= target_pts
                if next_frame.pts <= target_pts {
                    // Need to advance to next frame
                } else {
                    // Current frame is still the best match, return it
                    tracing::trace!(
                        "📦 FrameCache: request_frame({:.3}) -> returning current frame {:.3} (no advance needed)",
                        target_pts,
                        current_frame.pts
                    );
                    return Some(current_frame.clone());
                }
            } else {
                // No more frames ahead, keep returning current
                tracing::trace!(
                    "📦 FrameCache: request_frame({:.3}) -> returning current frame {:.3} (no ahead frames)",
                    target_pts,
                    current_frame.pts
                );
                return Some(current_frame.clone());
            }
        }

        // Need to pull a new frame from ahead cache
        let index = inner.ahead.iter().rposition(|f| f.pts <= target_pts)?;

        // Move all frames up to and including the target index from ahead to behind
        for _ in 0..=index {
            if let Some(frame) = inner.ahead.pop_front() {
                inner.behind.push_back(frame);
            }
        }

        // Get the frame we just moved (last in behind)
        let frame = inner.behind.back()?.clone();

        // Hard-cap behind cache
        while inner.behind.len() > inner.behind_hard_capacity {
            inner.behind.pop_front();
        }

        tracing::trace!(
            "📦 FrameCache: request_frame({:.3}) -> advanced to frame {:.3} (ahead={}, behind={})",
            target_pts,
            frame.pts,
            inner.ahead.len(),
            inner.behind.len()
        );

        Some(frame)
    }

    // =========================================================
    // LOOKUPS
    // =========================================================

    /// Get latest displayed frame.
    pub fn latest_behind(&self) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();
        inner.behind.back().cloned()
    }

    /// Get next frame waiting for playback.
    pub fn next_ahead(&self) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();
        inner.ahead.front().cloned()
    }

    /// Find closest frame in behind cache.
    ///
    /// Useful for reverse scrubbing or frame stepping backward.
    pub fn get_behind_frame(&self, target_pts: PTS) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();

        inner
            .behind
            .iter()
            .rev()
            .find(|f| f.pts <= target_pts)
            .cloned()
    }

    // =========================================================
    // CACHE HEALTH
    // =========================================================

    /// Ahead cache reached/exceeded soft capacity.
    pub fn ahead_is_full(&self) -> bool {
        let inner = self.inner.lock().unwrap();

        inner.ahead.len() >= inner.ahead_soft_capacity
    }

    /// Frames over ahead soft capacity.
    pub fn ahead_overflow_amount(&self) -> usize {
        let inner = self.inner.lock().unwrap();

        inner.ahead.len().saturating_sub(inner.ahead_soft_capacity)
    }

    /// Decoder should refill when ahead cache drops low.
    pub fn needs_refill(&self) -> bool {
        let inner = self.inner.lock().unwrap();

        inner.ahead.len() <= inner.ahead_soft_capacity / 2
    }

    /// Critical playback starvation.
    pub fn is_critically_low(&self) -> bool {
        let inner = self.inner.lock().unwrap();

        inner.ahead.len() < 3
    }

    pub fn frames_needed(&self) -> usize {
        let inner = self.inner.lock().unwrap();

        inner.ahead_soft_capacity.saturating_sub(inner.ahead.len())
    }

    // =========================================================
    // PRUNING
    // =========================================================

    /// Remove stale frames from behind cache.
    ///
    /// Keeps recent playback history only.
    pub fn prune_behind(&self, current_pts: PTS) {
        let mut inner = self.inner.lock().unwrap();

        const KEEP_BEHIND_SECS: f64 = 2.0;

        let cutoff = current_pts - KEEP_BEHIND_SECS;

        let mut removed = 0;

        while inner.behind.len() > 1 {
            match inner.behind.front() {
                Some(frame) if frame.pts < cutoff => {
                    inner.behind.pop_front();
                    removed += 1;
                }
                _ => break,
            }
        }

        if removed > 0 {
            tracing::debug!("📦 FrameCache: pruned {} stale behind frames", removed);
        }
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

    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();

        inner.ahead.clear();
        inner.behind.clear();
    }

    pub fn ahead_pts_range(&self) -> Option<(PTS, PTS)> {
        let inner = self.inner.lock().unwrap();

        if inner.ahead.is_empty() {
            return None;
        }

        Some((inner.ahead.front()?.pts, inner.ahead.back()?.pts))
    }

    pub fn behind_pts_range(&self) -> Option<(PTS, PTS)> {
        let inner = self.inner.lock().unwrap();

        if inner.behind.is_empty() {
            return None;
        }

        Some((inner.behind.front()?.pts, inner.behind.back()?.pts))
    }
}

impl Default for FrameCache {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FrameCacheInner {
                ahead: VecDeque::with_capacity(FRAME_BUFFER_CAPACITY),
                behind: VecDeque::with_capacity(FRAME_BUFFER_CAPACITY),
                ahead_soft_capacity: FRAME_BUFFER_CAPACITY,
                behind_hard_capacity: FRAME_BUFFER_CAPACITY,
            })),
        }
    }
}

impl super::Cache for FrameCache {
    fn clear(&self) {
        FrameCache::clear(self)
    }

    fn len(&self) -> usize {
        FrameCache::len(self)
    }
}

