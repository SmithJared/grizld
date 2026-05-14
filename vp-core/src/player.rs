use std::path::{self, Path};

use ffmpeg_next as ffmpeg;

use crate::cache::{AudioCache, FrameCache};
use crate::decoder::DemuxService;
use crate::error::{VpError, VpResult};
use crate::sync::{LoadingState, PlaybackClock};
use crate::types::{PlaybackState, VideoFrame, PTS};
use crate::FrameScheduler;

/// Main video player that orchestrates decoding and playback
pub struct VideoPlayer {
    // File info
    duration: f64,

    clock: PlaybackClock,

    audio_cache: AudioCache,

    // Pull-based decode coordinator
    scheduler: FrameScheduler,
}

impl VideoPlayer {
    /// Create a new video player and open a file
    pub fn new<P: AsRef<Path>>(file_path: P) -> VpResult<Self> {
        // Initialize FFmpeg
        ffmpeg::init().map_err(|e| VpError::Ffmpeg(format!("FFmpeg init failed: {}", e)))?;

        let path = file_path.as_ref();
        tracing::info!("Opening video file: {}", path.display());

        // Open input to validate file and get stream info
        let input = ffmpeg::format::input(&path)?;
        let duration = input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);

        // Find video and audio streams
        let video_stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(VpError::NoVideoStream)?;
        let video_stream_index = video_stream.index();

        let audio_stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or(VpError::NoAudioStream)?;
        let audio_stream_index = audio_stream.index();

        tracing::info!(
            "Found video stream {} and audio stream {}, duration: {:.2}s",
            video_stream_index,
            audio_stream_index,
            duration
        );

        drop(input); // Close input, DemuxService will reopen it

        // Create clock
        let clock = PlaybackClock::new();

        // Create demux service (owns FFmpeg input, runs in its own thread)
        let demux_service =
            DemuxService::new(path.to_path_buf(), video_stream_index, audio_stream_index)?;

        // Create cache
        let frame_cache = FrameCache::default();
        let audio_cache = AudioCache::default();

        // Create pull coordinator (video/audio workers pull from demuxer)
        let mut scheduler = FrameScheduler::new(
            demux_service,
            clock.clone(),
            frame_cache,
            audio_cache.clone(),
        );

        // Start the coordinator workers
        scheduler.start()?;

        // Initial buffer fill - request immediate refill
        // scheduler.request_refill();

        let player = Self {
            duration,
            clock,
            audio_cache,
            scheduler,
        };

        tracing::info!(
            "🎬 VideoPlayer created: duration={:.2}s, initial_state={:?}",
            player.duration,
            player.state()
        );

        Ok(player)
    }

    /// Start playback
    pub fn play(&mut self) {
        let state_before = self.state();
        let current_pts = self.current_time();

        tracing::info!(
            "▶️  PLAY called: state_before={:?}, current_pts={:.3}",
            state_before,
            current_pts
        );

        self.clock.set_state(PlaybackState::Playing);

        tracing::info!("▶️  PLAY completed: state_after={:?}", self.state());
    }

    /// Pause playback
    pub fn pause(&mut self) {
        let state_before = self.state();
        let current_pts = self.current_time();

        tracing::info!(
            "⏸️  PAUSE called: state_before={:?}, current_pts={:.3}",
            state_before,
            current_pts
        );

        self.clock.set_state(PlaybackState::Paused);

        tracing::info!("⏸️  PAUSE completed: state_after={:?}", self.state());
    }

    // TODO what is this for?
    /// Stop playback
    pub fn stop(&mut self) {
        self.clock.set_state(PlaybackState::Stopped);
        self.clock.reset();
        tracing::info!("Playback stopped");
    }

    // TODO handle seek
    /// Seek to a specific time (async with loading state)
    // pub fn seek(&mut self, target_pts: PTS) -> VpResult<()> {
    //     if target_pts < 0.0 || target_pts > self.duration {
    //         return Err(VpError::InvalidSeek(format!(
    //             "Target {} out of range [0, {}]",
    //             target_pts, self.duration
    //         )));
    //     }
    //
    //     tracing::info!("Seeking to {:.2}s", target_pts);
    //
    //     // Set loading state immediately
    //     self.clock.set_loading_state(LoadingState::Seeking);
    //
    //     // Update clock position immediately (for UI responsiveness)
    //     self.clock.seek(target_pts);
    //
    //     // Send async seek command to coordinator
    //     self.coordinator
    //         .send_command(CoordinatorCommand::Seek(target_pts))?;
    //
    //     Ok(())
    // }
    //

    pub fn audio_cache(&self) -> AudioCache {
        self.audio_cache.clone()
    }

    /// Get the current playback time
    pub fn current_time(&self) -> PTS {
        self.clock.current_time()
    }

    /// Get the total duration
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Get the current playback state
    pub fn state(&self) -> PlaybackState {
        self.clock.state()
    }

    /// Get the frame to display at the current time
    pub fn get_current_frame(&self) -> Option<VideoFrame> {
        match self.state() {
            PlaybackState::Playing => {
                let current_time = self.current_time();

                tracing::debug!(
                    "🎞️  get_current_frame: state=Playing, clock={:.3}s",
                    current_time,
                );

                let frame = self.scheduler.request_frame();
                frame
            }
            PlaybackState::Paused => None,
            PlaybackState::Stopped => None,
        }
    }

    /// Get a reference to the playback clock (for CPAL)
    pub fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    /// Get the current loading state (for UI feedback)
    pub fn loading_state(&self) -> LoadingState {
        self.clock.loading_state()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // Stop the coordinator (stops worker threads)
        self.scheduler.stop_all();

        tracing::info!("VideoPlayer dropped");
    }
}
