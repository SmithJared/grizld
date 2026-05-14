mod audio_cache;
mod frame_cache;

pub use audio_cache::AudioCache;
pub use frame_cache::FrameCache;

/// Common buffer operations shared between frame and audio buffers
pub trait Cache {
    /// Clear all items from the buffer
    fn clear(&self);

    /// Get the number of items currently in the buffer
    fn len(&self) -> usize;

    /// Check if the buffer is empty
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
