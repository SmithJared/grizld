use std::path::Path;

use ffmpeg_next as ffmpeg;

use crate::buffer::{AudioBuffer, FrameBuffer};
use crate::decoder::{DemuxService, VideoDecoder, AudioDecoder};
use crate::error::{VpError, VpResult};
use crate::pull_coordinator::{CoordinatorCommand, PullCoordinator};
use crate::sync::{LoadingState, PlaybackClock};
use crate::types::{PlaybackState, VideoFrame, PTS};

const FRAME_BUFFER_CAPACITY: usize = 15; // Reduced for 4K video (15 frames = ~500MB)
const AUDIO_BUFFER_SECONDS: f64 = 2.0;
const SAMPLE_RATE: u32 = 48000;

/// Main video player that orchestrates decoding and playback
pub struct VideoPlayer {
    // File info
    duration: f64,

    // Buffers
    frame_buffer: FrameBuffer,
    audio_buffer: AudioBuffer,

    // Sync
    clock: PlaybackClock,

    // Pull-based decode coordinator
    coordinator: PullCoordinator,
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

        // Validate decoders can be created (will be recreated by workers)
        let _video_decoder = VideoDecoder::new(&video_stream)?;
        let _audio_decoder = AudioDecoder::new(&audio_stream, SAMPLE_RATE)?;

        drop(input); // Close input, DemuxService will reopen it

        // Create buffers
        let frame_buffer = FrameBuffer::new(FRAME_BUFFER_CAPACITY);
        let audio_buffer = AudioBuffer::new(AUDIO_BUFFER_SECONDS, SAMPLE_RATE);

        // Create clock
        let clock = PlaybackClock::new();

        // Create demux service (owns FFmpeg input, runs in its own thread)
        let demux_service = DemuxService::new(
            path.to_path_buf(),
            video_stream_index,
            audio_stream_index,
        )?;

        // Create pull coordinator (video/audio workers pull from demuxer)
        let mut coordinator = PullCoordinator::new(
            demux_service,
            frame_buffer.clone(),
            audio_buffer.clone(),
            clock.clone(),
        );

        // Start the coordinator workers
        coordinator.start()?;

        // Initial buffer fill - request immediate refill
        coordinator.request_refill();

        let player = Self {
            duration,
            frame_buffer,
            audio_buffer,
            clock,
            coordinator,
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

        // Reset audio buffer to current video position to ensure sync
        // This prevents audio from being ahead of video when play is pressed
        self.audio_buffer.reset(current_pts);
        self.clock.set_state(PlaybackState::Playing);

        // Request buffer refill to start decoding
        self.coordinator.request_refill();

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

    /// Stop playback
    pub fn stop(&mut self) {
        self.clock.set_state(PlaybackState::Stopped);
        self.clock.reset();
        self.frame_buffer.clear();
        self.audio_buffer.clear();
        tracing::info!("Playback stopped");
    }

    /// Seek to a specific time (async with loading state)
    pub fn seek(&mut self, target_pts: PTS) -> VpResult<()> {
        if target_pts < 0.0 || target_pts > self.duration {
            return Err(VpError::InvalidSeek(format!(
                "Target {} out of range [0, {}]",
                target_pts, self.duration
            )));
        }

        tracing::info!("Seeking to {:.2}s", target_pts);

        // Set loading state immediately
        self.clock.set_loading_state(LoadingState::Seeking);

        // Update clock position immediately (for UI responsiveness)
        self.clock.seek(target_pts);

        // Send async seek command to coordinator
        self.coordinator.send_command(CoordinatorCommand::Seek(target_pts))?;

        Ok(())
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
        let current_state = self.state();
        let current_time = self.current_time();
        let buffer_len = self.frame_buffer.len();
        let buffer_empty = self.frame_buffer.is_empty();

        tracing::debug!(
            "🎞️  get_current_frame: state={:?}, clock={:.3}s, buffer_len={}, buffer_empty={}",
            current_state,
            current_time,
            buffer_len,
            buffer_empty
        );

        let frame = self.frame_buffer.get_frame_at(current_time);

        // If we can't find a frame at current time, use appropriate fallback
        if frame.is_none() && !buffer_empty {
            let buffer_range = self.frame_buffer.pts_range();
            tracing::debug!(
                "🎞️  No exact frame match, using fallback. Buffer range: {:?}",
                buffer_range
            );

            let fallback = if current_state.is_playing() {
                // When playing, use latest frame (helps with videos that don't start at PTS 0.0)
                let fb = self.frame_buffer.get_latest();
                if let Some(ref f) = fb {
                    tracing::debug!("🎞️  FALLBACK (PLAYING): returning latest frame at PTS {:.3}", f.pts);
                }
                fb
            } else {
                // When stopped/paused, use frame closest to current time to freeze properly
                let fb = self.frame_buffer.get_frame_closest(current_time);
                if let Some(ref f) = fb {
                    tracing::debug!("🎞️  FALLBACK (PAUSED/STOPPED): returning closest frame at PTS {:.3}", f.pts);
                }
                fb
            };
            return fallback;
        }

        if let Some(ref f) = frame {
            tracing::debug!("🎞️  EXACT MATCH: returning frame at PTS {:.3}", f.pts);
        } else {
            tracing::debug!("🎞️  NO FRAME: buffer is empty");
        }

        frame
    }

    /// Get a reference to the audio buffer (for CPAL)
    pub fn audio_buffer(&self) -> &AudioBuffer {
        &self.audio_buffer
    }

    /// Get a reference to the playback clock (for CPAL)
    pub fn clock(&self) -> &PlaybackClock {
        &self.clock
    }

    /// Get frame buffer stats
    pub fn buffer_stats(&self) -> (usize, f64) {
        let frame_count = self.frame_buffer.len();
        let audio_duration = self.audio_buffer.buffered_duration();
        (frame_count, audio_duration)
    }

    /// Get the current loading state (for UI feedback)
    pub fn loading_state(&self) -> LoadingState {
        self.clock.loading_state()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // Stop the coordinator (stops worker threads)
        self.coordinator.stop();

        tracing::info!("VideoPlayer dropped");
    }
}

// Old decode_loop removed - now using PullCoordinator with on-demand decoding
