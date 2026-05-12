// vp-core: Video playback library
// Provides hardware-accelerated video decoding, audio sync, and buffer management

pub mod error;
pub mod types;
pub mod sync;
pub mod buffer;
pub mod decoder;
pub mod player;

// Re-export main types
pub use error::{VpError, VpResult};
pub use types::{VideoFrame, AudioSample, PlaybackState, PTS};
pub use sync::PlaybackClock;
pub use player::VideoPlayer;
