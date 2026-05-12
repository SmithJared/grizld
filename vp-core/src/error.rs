use thiserror::Error;

#[derive(Debug, Error)]
pub enum VpError {
    #[error("FFmpeg error: {0}")]
    Ffmpeg(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("No video stream found in input")]
    NoVideoStream,

    #[error("No audio stream found in input")]
    NoAudioStream,

    #[error("Decoder error: {0}")]
    Decoder(String),

    #[error("Channel send error")]
    ChannelSend,

    #[error("Channel receive error")]
    ChannelReceive,

    #[error("Invalid seek target: {0}")]
    InvalidSeek(String),

    #[error("Playback not initialized")]
    NotInitialized,
}

pub type VpResult<T> = Result<T, VpError>;

impl From<ffmpeg_next::Error> for VpError {
    fn from(err: ffmpeg_next::Error) -> Self {
        VpError::Ffmpeg(err.to_string())
    }
}

impl<T> From<crossbeam_channel::SendError<T>> for VpError {
    fn from(_: crossbeam_channel::SendError<T>) -> Self {
        VpError::ChannelSend
    }
}

impl From<crossbeam_channel::RecvError> for VpError {
    fn from(_: crossbeam_channel::RecvError) -> Self {
        VpError::ChannelReceive
    }
}
