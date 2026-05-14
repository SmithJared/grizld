use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::types::{VideoFrame, PTS};

/// Thread-safe frame buffer that handles out-of-order B-frames
///
/// Frames are inserted in PTS order, which handles B-frames that arrive
/// out of decode order. This is a simplified single-buffer implementation.
#[derive(Clone)]
pub struct FrameBuffer {
    inner: Arc<Mutex<FrameBufferInner>>,
}

struct FrameBufferInner {
    frames: VecDeque<VideoFrame>,
    capacity: usize,
}

impl FrameBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FrameBufferInner {
                frames: VecDeque::with_capacity(capacity),
                capacity,
            })),
        }
    }

    /// Push a frame into the buffer, maintaining PTS order
    ///
    /// This handles out-of-order B-frames by inserting them in the correct position.
    pub fn push(&self, frame: VideoFrame) {
        let mut inner = self.inner.lock().unwrap();

        let pts = frame.pts;
        let len_before = inner.frames.len();

        // Find insertion position to maintain PTS order
        let insert_pos = inner
            .frames
            .iter()
            .position(|f| f.pts > frame.pts)
            .unwrap_or(inner.frames.len());

        inner.frames.insert(insert_pos, frame);

        // Enforce capacity by removing oldest frames
        while inner.frames.len() > inner.capacity {
            inner.frames.pop_front();
        }

        tracing::trace!("📦 FrameBuffer: pushed frame PTS {:.3}, len {} -> {}",
            pts, len_before, inner.frames.len());
    }

    /// Get the frame to display at the given PTS
    ///
    /// Returns the most recent frame with PTS <= target_pts
    pub fn get_frame_at(&self, target_pts: PTS) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();

        // Find the latest frame with PTS <= target_pts
        let frame = inner
            .frames
            .iter()
            .rev()
            .find(|f| f.pts <= target_pts)
            .cloned();

        if let Some(ref f) = frame {
            tracing::trace!("📦 FrameBuffer: get_frame_at({:.3}) -> PTS {:.3} (buffer_len={})",
                target_pts, f.pts, inner.frames.len());
        } else {
            tracing::trace!("📦 FrameBuffer: get_frame_at({:.3}) -> None (buffer_len={})",
                target_pts, inner.frames.len());
        }

        frame
    }

    /// Get the latest frame in the buffer
    pub fn get_latest(&self) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();
        inner.frames.back().cloned()
    }

    /// Get the first frame in the buffer (lowest PTS)
    pub fn get_first(&self) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();
        inner.frames.front().cloned()
    }

    /// Clear all frames from the buffer
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.frames.clear();
    }

    /// Get the number of frames currently in the buffer
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.frames.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check if the buffer is full
    pub fn is_full(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.frames.len() >= inner.capacity
    }

    /// Get the PTS range of frames in the buffer
    pub fn pts_range(&self) -> Option<(PTS, PTS)> {
        let inner = self.inner.lock().unwrap();
        if inner.frames.is_empty() {
            None
        } else {
            let min_pts = inner.frames.front().unwrap().pts;
            let max_pts = inner.frames.back().unwrap().pts;
            Some((min_pts, max_pts))
        }
    }

    /// Get the frame closest to the target PTS
    ///
    /// Prefers frames with PTS <= target, but will return the earliest frame
    /// if target is before all frames in the buffer.
    pub fn get_frame_closest(&self, target_pts: PTS) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();

        if inner.frames.is_empty() {
            return None;
        }

        // Try to find exact or closest frame before target
        if let Some(frame) = inner.frames.iter().rev().find(|f| f.pts <= target_pts).cloned() {
            return Some(frame);
        }

        // If target is before all frames, return the first frame
        inner.frames.front().cloned()
    }

    // === Buffer Health Metrics for Pull-Based Decoding ===

    /// Check if buffer needs refilling (50% threshold)
    ///
    /// Returns true when buffer has 50% or fewer frames remaining.
    /// This is the trigger point for pull-based decode to start refilling.
    pub fn needs_refill(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.frames.len() <= inner.capacity / 2
    }

    /// Get the number of frames needed to fill the buffer
    pub fn frames_needed(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.capacity.saturating_sub(inner.frames.len())
    }

    /// Check if buffer is critically low (< 3 frames)
    ///
    /// This indicates urgent refill is needed to avoid playback stutter.
    pub fn is_critically_low(&self) -> bool {
        self.len() < 3
    }

    /// Get the buffer capacity
    pub fn capacity(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.capacity
    }

    /// Remove old frames that are far behind the current playback position
    ///
    /// This prevents the buffer from accumulating frames that will never be displayed again.
    /// Keeps at least one frame before current_pts for seamless playback.
    pub fn remove_old_frames(&self, current_pts: PTS) {
        let mut inner = self.inner.lock().unwrap();

        // Keep frames within 2 seconds behind current position (for backwards seek)
        const KEEP_BEHIND_SECS: f64 = 2.0;
        let cutoff_pts = current_pts - KEEP_BEHIND_SECS;

        // Remove frames older than cutoff, but keep at least one frame before current_pts
        let mut removed_count = 0;
        while inner.frames.len() > 1 {
            if let Some(oldest) = inner.frames.front() {
                // Keep at least one frame at or before current_pts
                if oldest.pts < cutoff_pts {
                    // Check if there's a frame after this one that's still before current_pts
                    if let Some(next) = inner.frames.get(1) {
                        if next.pts <= current_pts {
                            // Safe to remove this old frame
                            inner.frames.pop_front();
                            removed_count += 1;
                            continue;
                        }
                    }
                }
            }
            break;
        }

        if removed_count > 0 {
            tracing::debug!("📦 FrameBuffer: removed {} old frames (current_pts={:.3}, new_len={})",
                removed_count, current_pts, inner.frames.len());
        }
    }
}

impl super::Buffer for FrameBuffer {
    fn clear(&self) {
        FrameBuffer::clear(self)
    }

    fn len(&self) -> usize {
        FrameBuffer::len(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PixelFormat;

    fn create_test_frame(pts: f64) -> VideoFrame {
        VideoFrame::new(pts, vec![0; 100], 10, 10, PixelFormat::Rgba)
    }

    #[test]
    fn test_frame_buffer_push_order() {
        let buffer = FrameBuffer::new(10);

        // Push frames out of order (simulating B-frames)
        buffer.push(create_test_frame(0.0));
        buffer.push(create_test_frame(0.04)); // Frame 1
        buffer.push(create_test_frame(0.02)); // B-frame (out of order)
        buffer.push(create_test_frame(0.06)); // Frame 2

        // Verify frames are in PTS order
        let frame1 = buffer.get_frame_at(0.01).unwrap();
        assert_eq!(frame1.pts, 0.0);

        let frame2 = buffer.get_frame_at(0.03).unwrap();
        assert_eq!(frame2.pts, 0.02);

        let frame3 = buffer.get_frame_at(0.05).unwrap();
        assert_eq!(frame3.pts, 0.04);
    }

    #[test]
    fn test_frame_buffer_capacity() {
        let buffer = FrameBuffer::new(3);

        buffer.push(create_test_frame(0.0));
        buffer.push(create_test_frame(1.0));
        buffer.push(create_test_frame(2.0));
        buffer.push(create_test_frame(3.0));

        // Should only keep the last 3 frames
        assert_eq!(buffer.len(), 3);

        let (min, max) = buffer.pts_range().unwrap();
        assert_eq!(min, 1.0);
        assert_eq!(max, 3.0);
    }
}
