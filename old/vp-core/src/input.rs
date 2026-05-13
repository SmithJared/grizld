use num_rational::Ratio;
use num_traits::ToPrimitive;
use std::path::Path;
use std::sync::{Arc, Mutex};
use video_sys::ffmpeg;

use thiserror::Error;

use crate::frame::{AV_TIME_BASE, Microseconds, Seconds, TimebaseUnits};

#[derive(Debug, Error)]
pub enum InputError {
    #[error("FFmpeg Error: {0}")]
    FFmpeg(#[from] ffmpeg::Error),
    #[error("Unsupported stream type")]
    UnsupportedStream,
    #[error("Input has no video stream")]
    NoVideoStream,
    #[error("Input has no audio stream")]
    NoAudioStream,
}

#[derive(Clone)]
pub struct Input {
    pub ctx: Arc<Mutex<ffmpeg::Input>>,
    pub video_stream: VideoStream,
    pub audio_stream: AudioStream,
}

impl Input {
    pub fn new(path: &Path) -> Result<Self, InputError> {
        ffmpeg::init()?;
        let ctx = ffmpeg::Input::new(path)?;

        let video_stream = ctx
            .video_stream()
            .ok_or(InputError::NoVideoStream)
            .and_then(|stream| VideoStream::new(&stream))?;

        let audio_stream = ctx
            .audio_stream()
            .ok_or(InputError::NoAudioStream)
            .and_then(|stream| AudioStream::new(&stream))?;

        Ok(Self {
            ctx: Arc::new(Mutex::new(ctx)),
            video_stream,
            audio_stream,
        })
    }
}

pub trait StreamInfo {
    fn codec_parameters(&self) -> &ffmpeg::Parameters;
    fn index(&self) -> usize;
    fn time_base(&self) -> Ratio<i64>;
    fn pts_duration(&self) -> TimebaseUnits;

    fn pts_to_seconds(&self, pts: TimebaseUnits) -> Option<Seconds> {
        let seconds = Ratio::from(pts.0) * self.time_base();
        seconds.to_f64().map(|s| Seconds(s))
    }

    fn pts_to_microseconds(&self, pts: TimebaseUnits) -> Option<Microseconds> {
        let seconds = Ratio::from(pts.0) * self.time_base();
        let microseconds = seconds * 1_000_000;
        microseconds.to_i64().map(|m| Microseconds(m))
    }

    fn seconds_to_timestamp(&self, seconds: f64) -> i64 {
        // Handle edge cases
        if seconds.is_nan() || seconds.is_infinite() {
            return 0;
        }

        // Convert seconds to a rational number with high precision
        // Using a denominator of 1_000_000 for microsecond precision
        let numer = (seconds * 1_000_000.0).round() as i64;
        let denom = 1_000_000i64;
        let seconds_ratio = Ratio::new(numer, denom);

        // Divide by time_base to get timestamp
        let timestamp_ratio = seconds_ratio / self.time_base();

        // Convert to integer, rounding to nearest
        timestamp_ratio.round().to_integer()
    }

    // Duration of the stream in seconds, returns 0 if unknown
    fn duration(&self) -> Microseconds {
        self.pts_to_microseconds(self.pts_duration()).unwrap_or(Microseconds::ZERO)
    }
}

#[derive(Debug, Clone)]
pub enum Stream {
    Video(VideoStream),
    Audio(AudioStream),
}

#[derive(Debug, Clone)]
pub struct VideoStream {
    codec_parameters: ffmpeg::Parameters,
    index: usize,
    time_base: Ratio<i64>,
    pts_duration: TimebaseUnits,
    pub width: u32,
    pub height: u32,
    frame_rate: Ratio<i64>,
}

impl VideoStream {
    pub fn new(stream: &ffmpeg::Stream) -> Result<Self, InputError> {
        let time_base = stream.time_base();
        let decoder = stream.video_decoder()?;

        // Calculate frame rate from stream as a Ratio
        let frame_rate = {
            let avg_frame_rate = stream.avg_frame_rate();
            if avg_frame_rate.1 != 0 {
                Ratio::new(avg_frame_rate.0 as i64, avg_frame_rate.1 as i64)
            } else {
                let r_frame_rate = stream.r_frame_rate();
                if r_frame_rate.1 != 0 {
                    Ratio::new(r_frame_rate.0 as i64, r_frame_rate.1 as i64)
                } else {
                    Ratio::new(30, 1) // Default fallback: 30fps
                }
            }
        };

        Ok(Self {
            codec_parameters: stream.parameters(),
            index: stream.index(),
            time_base: Ratio::new(time_base.0 as i64, time_base.1 as i64),
            pts_duration: TimebaseUnits(stream.duration() as i64),
            width: decoder.width().unwrap_or(0),
            height: decoder.height().unwrap_or(0),
            frame_rate,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Get the frame rate of the video stream as i64
    pub fn frame_rate(&self) -> Ratio<i64> {
        self.frame_rate
    }

    pub fn frame_to_microseconds(&self, frame_number: u64) -> Microseconds {
        // frame_number / frame_rate = seconds
        let frame_ratio = Ratio::new(frame_number as i64, 1);
        let seconds_ratio = frame_ratio / self.frame_rate;
        let microseconds_ratio = seconds_ratio * AV_TIME_BASE;
        Microseconds(microseconds_ratio.round().to_integer())
    }

    pub fn microseconds_to_frame(&self, microseconds: Microseconds) -> u64 {
        let microseconds_ratio = Ratio::new(microseconds.0, 1);
        let seconds_ratio = microseconds_ratio / AV_TIME_BASE;
        let frame_ratio = seconds_ratio * self.frame_rate;
        frame_ratio.round().to_integer() as u64
    }

    /// Convert a frame number to a timestamp in seconds
    pub fn frame_to_seconds(&self, frame_number: u64) -> Seconds {
        // frame_number / frame_rate = seconds
        let frame_ratio = Ratio::new(frame_number as i64, 1);
        let seconds_ratio = frame_ratio / self.frame_rate;
        Seconds(seconds_ratio.to_f64().unwrap_or(0.0))
    }

    /// Convert a timestamp in seconds to a frame number
    pub fn seconds_to_frame(&self, seconds: f64) -> u64 {
        // seconds * frame_rate = frame_number
        let numer = (seconds * 1_000_000.0).round() as i64;
        let seconds_ratio = Ratio::new(numer, 1_000_000);
        let frame_ratio = seconds_ratio * self.frame_rate;
        frame_ratio.round().to_integer() as u64
    }
}

impl StreamInfo for VideoStream {
    fn codec_parameters(&self) -> &ffmpeg::Parameters {
        &self.codec_parameters
    }

    fn index(&self) -> usize {
        self.index
    }

    fn time_base(&self) -> Ratio<i64> {
        self.time_base
    }

    fn pts_duration(&self) -> TimebaseUnits {
        self.pts_duration
    }
}

#[derive(Debug, Clone)]
pub struct AudioStream {
    codec_parameters: ffmpeg::Parameters,
    index: usize,
    time_base: Ratio<i64>,
    pts_duration: TimebaseUnits,
    channels: u16,
    sample_rate: u32,
}

impl AudioStream {
    pub fn new(stream: &ffmpeg::Stream) -> Result<Self, InputError> {
        let time_base = stream.time_base();
        let decoder = stream.audio_decoder()?;

        Ok(Self {
            codec_parameters: stream.parameters(),
            index: stream.index(),
            time_base: Ratio::new(time_base.0 as i64, time_base.1 as i64),
            pts_duration: TimebaseUnits(stream.duration() as i64),
            channels: decoder.channels().unwrap_or(0),
            sample_rate: decoder.sample_rate().unwrap_or(0),
        })
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl StreamInfo for AudioStream {
    fn codec_parameters(&self) -> &ffmpeg::Parameters {
        &self.codec_parameters
    }

    fn index(&self) -> usize {
        self.index
    }

    fn time_base(&self) -> Ratio<i64> {
        self.time_base
    }

    fn pts_duration(&self) -> TimebaseUnits {
        self.pts_duration
    }
}
