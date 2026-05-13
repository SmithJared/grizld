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
    }

    /// Get the frame to display at the given PTS
    ///
    /// Returns the most recent frame with PTS <= target_pts
    pub fn get_frame_at(&self, target_pts: PTS) -> Option<VideoFrame> {
        let inner = self.inner.lock().unwrap();

        // Find the latest frame with PTS <= target_pts
        inner
            .frames
            .iter()
            .rev()
            .find(|f| f.pts <= target_pts)
            .cloned()
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
