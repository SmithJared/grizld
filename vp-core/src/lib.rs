// vp-core: Video playback library
// Provides hardware-accelerated video decoding, audio sync, and buffer management

pub mod cache;
pub mod decoder;
pub mod error;
pub mod frame_scheduler;
pub mod player;
pub mod sync;
pub mod types;

// Re-export main types
pub use error::{VpError, VpResult};
pub use frame_scheduler::{DecoderCommand, FrameScheduler};
pub use player::VideoPlayer;
pub use sync::{LoadingState, PlaybackClock};
pub use types::{AudioSample, PlaybackState, VideoFrame, PTS};
