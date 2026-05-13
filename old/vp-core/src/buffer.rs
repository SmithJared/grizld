//! Frame buffer module for video playback
//!
//! Provides a ring buffer for storing decoded video frames with automatic
//! overflow handling, frame selection by PTS, and buffer health monitoring.
//!
//! Supports both software (YUV) and hardware (BGRA) frames seamlessly.

use crate::frame::{AV_TIME_BASE, Microseconds, VideoFrame};
use crate::util::mutex_ext::MutexExt;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use thiserror::Error;

type Result<T> = std::result::Result<T, BufferError>;

/// Main error type for vp_core
#[derive(Debug, Error)]
pub enum BufferError {
    #[error("Buffer is full")]
    Full,
}

pub enum BufferResult {
    Ok,
    Full,
}

/// Configuration for frame buffer behavior
#[derive(Debug, Clone)]
pub struct BufferConfig {
    /// Number of frames to store before playhead (safety limit)
    pub frames_capacity: usize,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            frames_capacity: 120, // ~4 seconds at 30fps
        }
    }
}

/// Buffer for video frames with automatic overflow handling
///
/// The FrameBuffer maintains a queue of decoded video frames, automatically
/// dropping old frames when the buffer exceeds its maximum duration.
/// It supports both software (YUV) and hardware (IOSurface) frames.
///
/// ```

#[derive(Debug, Clone)]
pub struct FrameBuffer {
    ahead: Arc<(Mutex<BoundedMinHeapBuffer>, Condvar)>,
    behind: Arc<Mutex<BoundedFrameBuffer>>,
    _config: BufferConfig,
}

impl FrameBuffer {
    /// Create a new frame buffer with the given configuration
    pub fn new(config: BufferConfig) -> Self {
        tracing::info!(
            "Creating FrameBuffer: frames_capacity={}",
            config.frames_capacity,
        );
        let ahead = Arc::new((
            Mutex::new(BoundedMinHeapBuffer::new(config.frames_capacity)),
            Condvar::new(),
        ));
        // Use smaller capacity for behind buffer (trailing frame history)
        let behind_capacity = 20;
        let behind = Arc::new(Mutex::new(BoundedFrameBuffer::new(behind_capacity)));

        Self {
            ahead,
            behind,
            _config: config,
        }
    }

    /// Push a new frame into the buffer
    ///
    /// Is a thread-blocking operation if the buffer is full
    ///
    /// If the buffer exceeds max_duration or max_frames, the oldest frames
    /// will be automatically dropped.
    ///
    /// Frames are automatically sorted by PTS to handle out-of-order decoding
    /// (common with B-frames in hardware decoders).
    pub fn push(&self, frame: VideoFrame) -> Result<()> {
        let (lock, cvar) = &*self.ahead;
        let mut ahead = lock.safe_lock();

        while ahead.is_full() {
            ahead = cvar.wait(ahead).unwrap();
        }

        // tracing::debug!(
        //     "Pushing frame with PTS: {}, buffer len: {}",
        //     frame.pts(),
        //     ahead.len()
        // );

        ahead.push(frame);
        cvar.notify_all(); // wake consumers

        Ok(())
    }

    /// Grab the next frame to display from the ahead of the playhead
    ///
    /// This will move the frame from the ahead buffer to the behind buffer
    /// and return it for display.
    pub fn display(&self) -> Option<VideoFrame> {
        // If there is a leading frame in the behind buffer then that is our next pts
        {
            let lock = &*self.behind;
            let mut behind = lock.safe_lock();
            if behind.leading_frame().is_some() {
                return behind.forward_pop(); 
            }
        }

        // Pop from ahead buffer (minimize lock time)
        let frame = {
            let (lock, cvar) = &*self.ahead;
            let mut ahead = lock.safe_lock();
            let frame = ahead.pop_front();
            // Notify waiting decoder threads BEFORE we do anything else
            cvar.notify_all();
            frame
        }; // ahead lock released here

        // If we got a frame, push to behind buffer (separate lock)
        if let Some(ref frame) = frame {
            let frame_clone = frame.clone();
            let mut behind = self.behind.safe_lock();
            behind.push(frame_clone);
        }

        frame
    }

    pub fn peek_next_pts(&self) -> Option<Microseconds> {

        // If there is a leading frame in the behind buffer then that is our next pts
        {
            let lock = &*self.behind;
            let behind = lock.safe_lock();
            if behind.leading_frame().is_some() {
                return behind.leading_frame();
            }
        }

        // Otherwise, peek from the ahead buffer
        let (lock, _) = &*self.ahead;
        lock.safe_lock().front().map(|f| f.pts())
    }

    /// Clear all frames from the buffer
    pub fn clear(&self) {
        tracing::debug!("Clearing buffer with {} frames", self.len());
        let (ahead_lock, cvar) = &*self.ahead;
        let mut ahead = ahead_lock.safe_lock();
        let mut behind = self.behind.safe_lock();
        behind.clear();
        ahead.clear();
        cvar.notify_all();
    }

    /// Get the number of frames in the buffer
    pub fn len(&self) -> usize {
        let (ahead_lock, _) = &*self.ahead;
        let behind = self.behind.safe_lock();
        let ahead = ahead_lock.safe_lock();
        behind.len() + ahead.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        let (ahead_lock, _) = &*self.ahead;
        let behind = self.behind.safe_lock();
        let ahead = ahead_lock.safe_lock();
        behind.is_empty() && ahead.is_empty()
    }

    pub fn is_full(&self) -> bool {
        let (ahead_lock, _) = &*self.ahead;
        let ahead = ahead_lock.safe_lock();
        ahead.is_full()
    }

    /// Get the oldest PTS in the buffer
    pub fn oldest_pts(&self) -> Option<Microseconds> {
        let behind = self.behind.safe_lock();
        behind.front().map(|f| f.pts())
    }

    /// Restore the previous frame from behind buffer for backward stepping
    ///
    /// Pops two frames from behind buffer (current and previous),
    /// restores both to ahead buffer in correct order, and returns the previous frame's PTS.
    /// Returns None if there aren't at least two frames in behind buffer.
    pub fn restore_previous_frame(&self) -> Option<Microseconds> {
        let mut behind = self.behind.safe_lock();

        // Pop current frame
        let _current_frame = behind.rewind_pop()?;

        // Pop previous frame
        let prev_frame = match behind.rewind_pop() {
            Some(frame) => frame,
            None => {
                // Only one frame in buffer, put current back
                behind.forward_pop();
                return None;
            }
        };

        let prev_frame_pts = prev_frame.pts();

        Some(prev_frame_pts)
    }

}

impl Drop for FrameBuffer {
    fn drop(&mut self) {
        // Wake up any threads blocked on push() so they can exit
        // We need to clear the buffer while holding the lock so that
        // when threads wake up, the buffer is no longer full
        let (lock, cvar) = &*self.ahead;
        let mut ahead = lock.safe_lock();
        ahead.clear();
        cvar.notify_all();
        drop(ahead); // Explicitly release the lock

        // Also clear the behind buffer
        let mut behind = self.behind.safe_lock();
        behind.clear();
    }
}


// This is a simplified ring buffer implementation
// The trailing frames are all frames that have been displayed
// If the user ever steps backwards, we pop the trailing frames and push them to the leading buffer
// The FrameBuffer struct detects if there are frames in the leading buffer and will return them before displaying from the min heap
// When a frame is displayed from the leading buffer it is moved to the trailing buffer again
#[derive(Debug, Clone)]
struct BoundedFrameBuffer {
    leading: VecDeque<VideoFrame>,
    trailing: VecDeque<VideoFrame>,
    capacity: usize,
}

impl BoundedFrameBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            leading: VecDeque::with_capacity(capacity),
            trailing: VecDeque::with_capacity(capacity),
        }
    }

    // Pushes a video frame to the trailing buffer 
    fn push(&mut self, frame: VideoFrame) {
        if self.trailing.len() >= self.capacity {
            self.trailing.pop_front();
        }
        self.trailing.push_back(frame);
    }

    // Pops the most recent frame from the trailing buffer and stores a copy in the leading buffer
    fn rewind_pop(&mut self) -> Option<VideoFrame> {
        let frame = self.trailing.pop_back()?;
        self.leading.push_back(frame.clone());
        Some(frame)
    }

    // Pops the oldest frame from the leading buffer and stores a copy in the trailing buffer
    fn forward_pop(&mut self) -> Option<VideoFrame> {
        let frame = self.leading.pop_back()?;
        self.trailing.push_back(frame.clone());
        Some(frame)
    }

    // Return the oldest frame in the buffer
    fn front(&self) -> Option<&VideoFrame> {
        self.trailing.front()
    }

    // If there is a frame in the leading buffer, return its PTS
    fn leading_frame(&self) -> Option<Microseconds> {
        self.leading.back().map(|f| f.pts())
    }

    fn len(&self) -> usize {
        self.leading.len() + self.trailing.len()
    }

    fn clear(&mut self) {
        self.leading.clear();
        self.trailing.clear();
    }

    fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_empty()
    }
}

// ----------------------------------
// Min Heap Buffer
// ----------------------------------

// Wrapper to reverse ordering for min heap
#[derive(Debug, Clone, Eq, PartialEq)]
struct MinHeapFrame(VideoFrame);

impl Ord for MinHeapFrame {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse the ordering to create a min heap
        other.0.cmp(&self.0)
    }
}

impl PartialOrd for MinHeapFrame {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
struct BoundedMinHeapBuffer {
    inner: BinaryHeap<MinHeapFrame>,
    capacity: usize,
}

impl BoundedMinHeapBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            inner: BinaryHeap::with_capacity(capacity),
        }
    }

    fn push(&mut self, frame: VideoFrame) {
        if self.inner.len() >= self.capacity {
            // Remove the frame with the lowest PTS (the min element)
            self.inner.pop();
        }
        self.inner.push(MinHeapFrame(frame));
    }

    fn _insert(&mut self, _pos: usize, frame: VideoFrame) {
        // For a heap, position doesn't matter - just push
        self.push(frame);
    }

    fn pop_front(&mut self) -> Option<VideoFrame> {
        // Pop the minimum element (lowest PTS)
        self.inner.pop().map(|mhf| mhf.0)
    }

    fn _pop_back(&mut self) -> Option<VideoFrame> {
        // For a heap, we can't efficiently pop the max element
        // This would require converting to vec, sorting, and rebuilding
        // Since this is unused (prefixed with _), we'll leave it unimplemented
        unimplemented!("pop_back is not efficient with a heap structure")
    }

    fn front(&self) -> Option<&VideoFrame> {
        // Peek at the minimum element (lowest PTS)
        self.inner.peek().map(|mhf| &mhf.0)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn clear(&mut self) {
        self.inner.clear();
    }

    fn is_full(&self) -> bool {
        self.inner.len() >= self.capacity
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

// ----------------------------------
// Audio Buffer
// ----------------------------------

/// Audio buffer error types
#[derive(Debug, Error)]
pub enum AudioBufferError {
    #[error("Audio buffer is full")]
    Full,
    #[error("Audio buffer underrun")]
    Underrun,
}

/// Result type for audio buffer operations
pub enum AudioBufferResult {
    Ok,
    Full,
}

/// Configuration for audio buffer behavior
#[derive(Debug, Clone)]
pub struct AudioBufferConfig {
    /// Maximum duration of audio to buffer (in seconds)
    pub max_duration_secs: f64,
    /// Sample rate (samples per second)
    pub sample_rate: u32,
    /// Number of channels (1=mono, 2=stereo, etc.)
    pub channels: u16,
}

impl Default for AudioBufferConfig {
    fn default() -> Self {
        Self {
            max_duration_secs: 2.0, // 2 seconds of audio
            sample_rate: 48000,
            channels: 2,
        }
    }
}

impl AudioBufferConfig {
    /// Calculate maximum number of samples to buffer
    pub fn max_samples(&self) -> usize {
        (self.max_duration_secs * self.sample_rate as f64 * self.channels as f64) as usize
    }
}

/// A decoded audio frame containing PCM samples
#[derive(Clone, Debug)]
pub struct AudioFrame {
    /// Presentation timestamp in seconds
    pub pts: Microseconds,
    /// Interleaved PCM samples (f32 format, normalized to -1.0 to 1.0)
    /// For stereo: [L, R, L, R, L, R, ...]
    pub samples: Vec<f32>,
    /// Sample rate
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u16,
}

impl AudioFrame {
    /// Create a new audio frame
    pub fn new(pts: Microseconds, samples: Vec<f32>, sample_rate: u32, channels: u16) -> Self {
        Self {
            pts,
            samples,
            sample_rate,
            channels,
        }
    }

    /// Get the duration of this frame in seconds
    pub fn duration(&self) -> f64 {
        let num_samples = self.samples.len() / self.channels as usize;
        num_samples as f64 / self.sample_rate as f64
    }

    /// Get the number of samples per channel
    pub fn num_samples(&self) -> usize {
        self.samples.len() / self.channels as usize
    }
}

/// Ring buffer for audio samples with automatic overflow handling
///
/// The AudioBuffer maintains a queue of decoded audio frames, automatically
/// dropping old frames when the buffer exceeds its maximum duration.
///
/// # Example
///
/// ```ignore
/// let config = AudioBufferConfig::default();
/// let buffer = AudioBuffer::new(config);
///
/// // Push audio frames
/// buffer.push(frame1)?;
/// buffer.push(frame2)?;
///
/// // Pull samples for playback
/// if let Some(samples) = buffer.pull_samples(1024) {
///     // Send to audio output device
/// }
/// ```
#[derive(Clone)]
pub struct AudioBuffer {
    inner: Arc<(Mutex<AudioBufferInner>, Condvar)>,
    config: AudioBufferConfig,
}

struct AudioBufferInner {
    /// Queue of audio frames
    frames: VecDeque<AudioFrame>,
    /// Total number of samples currently buffered
    total_samples: usize,
    /// Maximum samples allowed
    max_samples: usize,
    /// Current read position within the first frame (sample index)
    read_position: usize,
}

impl AudioBuffer {
    /// Create a new audio buffer with the given configuration
    pub fn new(config: AudioBufferConfig) -> Self {
        let max_samples = config.max_samples();

        tracing::info!(
            "Creating AudioBuffer: max_duration={:.2}s, sample_rate={}, channels={}, max_samples={}",
            config.max_duration_secs,
            config.sample_rate,
            config.channels,
            max_samples
        );

        let inner = AudioBufferInner {
            frames: VecDeque::new(),
            total_samples: 0,
            max_samples,
            read_position: 0,
        };

        Self {
            inner: Arc::new((Mutex::new(inner), Condvar::new())),
            config,
        }
    }

    /// Push a new audio frame into the buffer
    ///
    /// This is a blocking operation if the buffer is full.
    /// Frames are expected to arrive in order by PTS.
    pub fn push(&self, frame: AudioFrame) -> Result<()> {
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().unwrap();

        // Block while buffer is full
        while inner.total_samples + frame.samples.len() > inner.max_samples {
            inner = cvar.wait(inner).unwrap();
        }

        inner.total_samples += frame.samples.len();
        inner.frames.push_back(frame);

        cvar.notify_all(); // Wake consumers

        Ok(())
    }

    /// Try to push a frame without blocking
    pub fn try_push(&self, frame: AudioFrame) -> Result<AudioBufferResult> {
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().unwrap();

        if inner.total_samples + frame.samples.len() > inner.max_samples {
            return Ok(AudioBufferResult::Full);
        }

        inner.total_samples += frame.samples.len();
        inner.frames.push_back(frame);
        cvar.notify_all();

        Ok(AudioBufferResult::Ok)
    }

    /// Pull samples from the buffer and return the PTS of what was pulled
    ///
    /// Returns both the samples and the PTS timestamp atomically.
    /// This prevents race conditions when updating the audio clock.
    pub fn pull_samples_with_pts(&self, num_samples: usize) -> Option<(Vec<f32>, Microseconds)> {
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().unwrap();

        // Capture the PTS BEFORE pulling any samples
        let pts = if let Some(frame) = inner.frames.front() {
            let samples_per_channel = inner.read_position / frame.channels as usize;
            let offset = (samples_per_channel as f64 / frame.sample_rate as f64 * AV_TIME_BASE as f64) as i64;
            let calculated_pts = Microseconds(frame.pts.0 + offset);

            calculated_pts
        } else {
            return None;
        };

        let mut output = Vec::with_capacity(num_samples * self.config.channels as usize);
        let mut remaining = num_samples * self.config.channels as usize;

        while remaining > 0 && !inner.frames.is_empty() {
            let read_pos = inner.read_position;

            // Extract the slice we need, then drop the mutable borrow
            let samples_to_copy = {
                let frame = inner.frames.front().unwrap();
                let available = frame.samples.len() - read_pos;
                let to_copy = remaining.min(available);
                &frame.samples[read_pos..read_pos + to_copy]
            };

            // Copy samples from current frame
            output.extend_from_slice(samples_to_copy);

            let copied = samples_to_copy.len();
            inner.read_position += copied;
            inner.total_samples -= copied;
            remaining -= copied;

            // If we've consumed the entire frame, remove it
            let frame_len = inner.frames.front().unwrap().samples.len();
            if inner.read_position >= frame_len {
                let consumed_frame_pts = inner.frames.front().unwrap().pts;

                inner.frames.pop_front();
                inner.read_position = 0;
                // Log the transition to help debug timing issues
                if let Some(next_frame) = inner.frames.front() {
                    tracing::trace!(
                        "Frame transition: consumed {:.3}s, next {:.3}s (delta: {:.3}s)",
                        consumed_frame_pts,
                        next_frame.pts,
                        next_frame.pts.0 - consumed_frame_pts.0
                    );

                    if next_frame.pts < consumed_frame_pts {
                        tracing::error!(
                            "FRAME TRANSITION WENT BACKWARD! Consumed: {:.3}s, Next: {:.3}s",
                            consumed_frame_pts,
                            next_frame.pts
                        );
                    }
                }
            }
        }

        cvar.notify_all(); // Wake producers

        Some((output, pts))
    }

    /// Get the current playback PTS (timestamp of next sample to be read)
    ///
    /// This is used to update the audio clock with the exact position
    /// of audio playback. Returns None if buffer is empty.
    pub fn current_pts(&self) -> Option<Microseconds> {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();

        if let Some(frame) = inner.frames.front() {
            // Calculate PTS based on current read position within the frame
            let samples_per_channel = inner.read_position / frame.channels as usize;
            let offset = (samples_per_channel as f64 / frame.sample_rate as f64 * AV_TIME_BASE as f64) as i64;
            let pts = Microseconds(frame.pts.0 + offset);

            tracing::trace!(
                "current_pts: frame.pts={:.3}s, read_pos={}, offset={:.3}s, result={:.3}s",
                frame.pts,
                inner.read_position,
                offset,
                pts
            );

            Some(pts)
        } else {
            None
        }
    }

    /// Get the current buffered duration in seconds
    pub fn buffered_duration(&self) -> f64 {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();

        let samples_per_channel = inner.total_samples / self.config.channels as usize;
        samples_per_channel as f64 / self.config.sample_rate as f64
    }

    /// Get the number of frames currently buffered
    pub fn frame_count(&self) -> usize {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();
        inner.frames.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();
        inner.frames.is_empty()
    }

    /// Check if the buffer is full
    pub fn is_full(&self) -> bool {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();
        inner.total_samples >= inner.max_samples
    }

    /// Clear all frames from the buffer
    pub fn clear(&self) {
        let (lock, cvar) = &*self.inner;
        let mut inner = lock.lock().unwrap();

        inner.frames.clear();
        inner.total_samples = 0;
        inner.read_position = 0;

        cvar.notify_all(); // Wake any waiting threads

        tracing::debug!("Audio buffer cleared");
    }

    /// Get the PTS of the next sample to be read
    pub fn next_pts(&self) -> Option<Microseconds> {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();

        inner.frames.front().map(|frame| {
            // Calculate PTS based on read position
            let samples_read = inner.read_position / frame.channels as usize;
            let time_offset = (samples_read as f64 / frame.sample_rate as f64 * AV_TIME_BASE as f64) as i64;
            Microseconds(frame.pts.0 + time_offset)
        })
    }

    /// Get buffer statistics for monitoring
    pub fn stats(&self) -> AudioBufferStats {
        let (lock, _) = &*self.inner;
        let inner = lock.lock().unwrap();

        let buffered_duration = {
            let samples_per_channel = inner.total_samples / self.config.channels as usize;
            samples_per_channel as f64 / self.config.sample_rate as f64
        };

        AudioBufferStats {
            frame_count: inner.frames.len(),
            total_samples: inner.total_samples,
            buffered_duration,
            is_full: inner.total_samples >= inner.max_samples,
            is_empty: inner.frames.is_empty(),
        }
    }
}

/// Statistics about the audio buffer state
#[derive(Debug, Clone)]
pub struct AudioBufferStats {
    pub frame_count: usize,
    pub total_samples: usize,
    pub buffered_duration: f64,
    pub is_full: bool,
    pub is_empty: bool,
}

/// High-performance double-buffer for video frames optimized for 60Hz decode/render loops.
/// 100% safe Rust implementation
///
/// Architecture:
/// - **Write Buffer**: Decoders push frames here (protected by mutex, minimal contention)
/// - **Read Buffer**: Timeline reads from here (Arc<RwLock> for safe concurrent access)
///
/// The key insight: decoders write to a buffer that's NOT being read by the timeline,
/// and buffers swap automatically when the read buffer is exhausted.
///
/// Usage: Clone to share between decode threads and timeline.
#[derive(Clone)]
pub struct DoubleFrameBuffer {
    /// Shared read buffer that timeline accesses (safe concurrent access via RwLock)
    read_buffer: Arc<RwLock<BinaryHeap<VideoFrame>>>,

    /// Write buffer where decoders push frames (protected by mutex with condvar for blocking)
    write_buffer: Arc<(Mutex<BinaryHeap<VideoFrame>>, Condvar)>,

    /// Maximum number of frames allowed in both buffers combined
    max_capacity: usize,
}

impl DoubleFrameBuffer {
    pub fn new() -> Self {
        Self::with_capacity(120) // Default: ~4 seconds at 30fps
    }

    pub fn with_capacity(max_capacity: usize) -> Self {
        Self {
            read_buffer: Arc::new(RwLock::new(BinaryHeap::new())),
            write_buffer: Arc::new((Mutex::new(BinaryHeap::new()), Condvar::new())),
            max_capacity,
        }
    }

    /// Push a frame from a decoder thread (multiple decoders can call this concurrently)
    /// Frames are automatically sorted by PTS in the BinaryHeap.
    /// **Blocks if the buffer is full** until space becomes available.
    pub fn push_frame(&self, frame: VideoFrame) {
        let (lock, cvar) = &*self.write_buffer;
        let mut write = lock.lock().unwrap();

        // Block while buffer is full
        while self.total_frame_count_locked(&write) >= self.max_capacity {
            write = cvar.wait(write).unwrap();
        }

        write.push(frame);
        // Notify consumers that a frame is available
        cvar.notify_all();
    }

    /// Get the total number of frames in both buffers (must be called with write lock held)
    fn total_frame_count_locked(&self, write: &BinaryHeap<VideoFrame>) -> usize {
        let read = self.read_buffer.read().unwrap();
        write.len() + read.len()
    }

    /// Get the next frame for rendering (minimal lock contention)
    /// Automatically swaps buffers when the read buffer is empty.
    /// Returns None if no frames are available in any buffer.
    pub fn get_next_frame(&self) -> Option<VideoFrame> {
        // Try to pop from the current read buffer (brief write lock)
        let frame = {
            let mut read = self.read_buffer.write().unwrap();
            if let Some(frame) = read.pop() {
                Some(frame)
            } else {
                None
            }
        }; // Lock released here

        if frame.is_some() {
            // Notify blocked decoder threads that space is available
            let (_, cvar) = &*self.write_buffer;
            cvar.notify_all();
            return frame;
        }

        // Read buffer is empty, try to swap in new frames
        self.swap_buffers();

        // Try again after swap
        let frame = {
            let mut read = self.read_buffer.write().unwrap();
            read.pop()
        };

        // Notify again after swap since we may have freed up space
        if frame.is_some() {
            let (_, cvar) = &*self.write_buffer;
            cvar.notify_all();
        }

        frame
    }

    /// Get the next frame only if it's ready to be displayed based on the current time.
    /// This is used by the timeline to ensure frames are displayed at the correct time.
    /// Returns None if no frame is ready or if the buffer is empty.
    pub fn get_next_frame_if_ready(&self, current_time: Microseconds) -> Option<VideoFrame> {
        // Check if the next frame is ready
        if let Some(next_pts) = self.peek_next_pts() {
            if next_pts <= current_time {
                return self.get_next_frame();
            }
        }
        None
    }

    /// Peek at the PTS of the next frame without removing it.
    /// Returns None if no frames are available.
    pub fn peek_next_pts(&self) -> Option<Microseconds> {
        // Try read buffer first
        {
            let read = self.read_buffer.read().unwrap();
            if let Some(frame) = read.peek() {
                return Some(frame.pts());
            }
        }

        // If read buffer is empty, check write buffer
        let (lock, _) = &*self.write_buffer;
        let write = lock.lock().unwrap();
        write.peek().map(|frame| frame.pts())
    }

    /// Internal method to swap buffers when read buffer is exhausted
    /// This is called automatically by get_next_frame()
    fn swap_buffers(&self) {
        // Lock both buffers
        let (lock, _) = &*self.write_buffer;
        let mut write = lock.lock().unwrap();
        let mut read = self.read_buffer.write().unwrap();

        // Swap their contents directly
        std::mem::swap(&mut *write, &mut *read);

        // Now:
        // - read has the new frames (what was in write)
        // - write is empty (what was in read)
        // Decoders can immediately start filling the empty write buffer
    }

    /// Get the approximate number of frames waiting to be rendered
    pub fn pending_frame_count(&self) -> usize {
        let (lock, _) = &*self.write_buffer;
        lock.lock().unwrap().len()
    }

    /// Get the total number of frames in both buffers
    pub fn total_frame_count(&self) -> usize {
        let (lock, _) = &*self.write_buffer;
        let write = lock.lock().unwrap();
        let read = self.read_buffer.read().unwrap();
        write.len() + read.len()
    }

    /// Check if the buffer is full
    pub fn is_full(&self) -> bool {
        self.total_frame_count() >= self.max_capacity
    }

    /// Check if the read buffer is empty (useful for monitoring/debugging)
    pub fn is_read_buffer_empty(&self) -> bool {
        self.read_buffer.read().unwrap().is_empty()
    }

    /// Clear all frames from both buffers.
    /// This is useful when seeking or resetting playback state.
    /// Notifies any blocked decoder threads that space is now available.
    pub fn clear(&self) {
        // Clear the write buffer first
        let (lock, cvar) = &*self.write_buffer;
        {
            let mut write = lock.lock().unwrap();
            write.clear();
        }

        // Clear the read buffer
        {
            let mut read = self.read_buffer.write().unwrap();
            read.clear();
        }

        // Notify all waiting decoder threads that space is available
        cvar.notify_all();
    }
}

// Safety: All fields use safe synchronization primitives
// No manual Send/Sync impls needed - derived automatically
