mod frame_buffer;
mod audio_buffer;

pub use frame_buffer::FrameBuffer;
pub use audio_buffer::AudioBuffer;

/// Common buffer operations shared between frame and audio buffers
pub trait Buffer {
    /// Clear all items from the buffer
    fn clear(&self);

    /// Get the number of items currently in the buffer
    fn len(&self) -> usize;

    /// Check if the buffer is empty
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
