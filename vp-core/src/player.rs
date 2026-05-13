use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{bounded, Receiver, Sender};
use ffmpeg_next as ffmpeg;

use crate::buffer::{AudioBuffer, FrameBuffer};
use crate::decoder::{AudioDecoder, VideoDecoder};
use crate::error::{VpError, VpResult};
use crate::sync::PlaybackClock;
use crate::types::{PlaybackState, VideoFrame, PTS};

const FRAME_BUFFER_CAPACITY: usize = 15; // Reduced for 4K video (15 frames = ~500MB)
const AUDIO_BUFFER_SECONDS: f64 = 2.0;
const SAMPLE_RATE: u32 = 48000;
const FRAME_DROP_THRESHOLD_SECS: f64 = 0.1; // Drop frames more than 100ms late

/// Main video player that orchestrates decoding and playback
pub struct VideoPlayer {
    // File info
    duration: f64,
    _video_stream_index: usize,
    _audio_stream_index: usize,

    // Buffers (shared with decode thread)
    frame_buffer: FrameBuffer,
    audio_buffer: AudioBuffer,

    // Sync
    clock: PlaybackClock,

    // Track if we've synced to first frame
    _initial_sync_done: Arc<AtomicBool>,

    // Threading
    decode_thread: Option<JoinHandle<()>>,
    stop_signal: Arc<AtomicBool>,

    // Communication
    command_tx: Sender<PlayerCommand>,
}

enum PlayerCommand {
    Seek(PTS),
    Stop,
}

impl VideoPlayer {
    /// Create a new video player and open a file
    pub fn new<P: AsRef<Path>>(file_path: P) -> VpResult<Self> {
        // Initialize FFmpeg
        ffmpeg::init().map_err(|e| VpError::Ffmpeg(format!("FFmpeg init failed: {}", e)))?;

        let path = file_path.as_ref();
        tracing::info!("Opening video file: {}", path.display());

        // Open input
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

        // Create decoders (just to validate they can be created)
        let _video_decoder = VideoDecoder::new(&video_stream)?;
        let _audio_decoder = AudioDecoder::new(&audio_stream, SAMPLE_RATE)?;

        // Create buffers
        let frame_buffer = FrameBuffer::new(FRAME_BUFFER_CAPACITY);
        let audio_buffer = AudioBuffer::new(AUDIO_BUFFER_SECONDS, SAMPLE_RATE);

        // Create clock
        let clock = PlaybackClock::new();

        // Create communication channels
        let (command_tx, command_rx) = bounded(10);

        let stop_signal = Arc::new(AtomicBool::new(false));
        let initial_sync_done = Arc::new(AtomicBool::new(false));

        // Spawn decode thread
        let decode_thread = {
            let frame_buffer = frame_buffer.clone();
            let audio_buffer = audio_buffer.clone();
            let clock = clock.clone();
            let stop_signal = stop_signal.clone();
            let initial_sync_done_clone = initial_sync_done.clone();
            let path = path.to_path_buf();

            thread::Builder::new()
                .name("decode-thread".to_string())
                .spawn(move || {
                    if let Err(e) = decode_loop(
                        path,
                        video_stream_index,
                        audio_stream_index,
                        frame_buffer,
                        audio_buffer,
                        clock,
                        command_rx,
                        stop_signal,
                        initial_sync_done_clone,
                    ) {
                        tracing::error!("Decode thread error: {}", e);
                    }
                })
                .map_err(|e| VpError::Io(e))?
        };

        let player = Self {
            duration,
            _video_stream_index: video_stream_index,
            _audio_stream_index: audio_stream_index,
            frame_buffer,
            audio_buffer,
            clock,
            _initial_sync_done: initial_sync_done,
            decode_thread: Some(decode_thread),
            stop_signal,
            command_tx,
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

    /// Seek to a specific time
    pub fn seek(&mut self, target_pts: PTS) -> VpResult<()> {
        if target_pts < 0.0 || target_pts > self.duration {
            return Err(VpError::InvalidSeek(format!(
                "Target {} out of range [0, {}]",
                target_pts, self.duration
            )));
        }

        tracing::info!("Seeking to {:.2}s", target_pts);

        // Send seek command to decode thread
        self.command_tx
            .send(PlayerCommand::Seek(target_pts))
            .map_err(|_| VpError::ChannelSend)?;

        // Update clock
        self.clock.seek(target_pts);

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
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // Signal decode thread to stop
        self.stop_signal.store(true, Ordering::Relaxed);

        let _ = self.command_tx.send(PlayerCommand::Stop);

        // Wait for thread to finish
        if let Some(thread) = self.decode_thread.take() {
            let _ = thread.join();
        }

        tracing::info!("VideoPlayer dropped");
    }
}

/// Main decode loop running in a separate thread
fn decode_loop(
    path: std::path::PathBuf,
    video_stream_index: usize,
    audio_stream_index: usize,
    frame_buffer: FrameBuffer,
    audio_buffer: AudioBuffer,
    clock: PlaybackClock,
    command_rx: Receiver<PlayerCommand>,
    stop_signal: Arc<AtomicBool>,
    initial_sync_done: Arc<AtomicBool>,
) -> VpResult<()> {
    let mut input = ffmpeg::format::input(&path)?;

    let video_stream = input
        .stream(video_stream_index)
        .ok_or(VpError::NoVideoStream)?;
    let audio_stream = input
        .stream(audio_stream_index)
        .ok_or(VpError::NoAudioStream)?;

    let mut video_decoder = VideoDecoder::new(&video_stream)?;
    let mut audio_decoder = AudioDecoder::new(&audio_stream, SAMPLE_RATE)?;

    loop {
        // Check for stop signal
        if stop_signal.load(Ordering::Relaxed) {
            break;
        }

        // Check for commands (non-blocking)
        if let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                PlayerCommand::Seek(target_pts) => {
                    // Seek in the input
                    let timestamp = (target_pts / f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
                    input.seek(timestamp, ..timestamp)?;

                    // Clear buffers
                    frame_buffer.clear();
                    audio_buffer.reset(target_pts);

                    tracing::debug!("Decode thread: Seeked to {:.2}", target_pts);
                }
                PlayerCommand::Stop => {
                    break;
                }
            }
        }

        // Read packet
        match input.packets().next() {
            Some((stream, packet)) => {
                if stream.index() == video_stream_index {
                    // Decode video
                    match video_decoder.decode(&packet) {
                        Ok(frames) => {
                            for frame in frames {
                                // Sync clock to first frame on initial load
                                if !initial_sync_done.load(Ordering::Relaxed) && frame_buffer.is_empty() {
                                    tracing::info!(
                                        "🎬 INITIAL SYNC: first_frame_pts={:.3}, clock_state_before={:?}",
                                        frame.pts,
                                        clock.state()
                                    );
                                    clock.seek(frame.pts);
                                    // Also sync audio buffer to ensure A/V alignment from the start
                                    audio_buffer.reset(frame.pts);
                                    initial_sync_done.store(true, Ordering::Relaxed);
                                    tracing::info!(
                                        "🎬 INITIAL SYNC complete: clock_state_after={:?}, clock_pts={:.3}",
                                        clock.state(),
                                        clock.current_time()
                                    );
                                }

                                // Frame dropping: if this frame is already late, drop it
                                let current_time = clock.current_time();
                                if clock.state().is_playing() && frame.pts < current_time - FRAME_DROP_THRESHOLD_SECS {
                                    tracing::debug!("Dropping late frame: PTS {:.3} < clock {:.3}", frame.pts, current_time);
                                    continue;
                                }

                                // Only push if we have room or if paused (need to buffer ahead)
                                if !clock.state().is_playing() || !frame_buffer.is_full() {
                                    frame_buffer.push(frame);
                                } else {
                                    tracing::debug!("Frame buffer full, dropping frame at PTS {:.3}", frame.pts);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Video decode error: {}", e);
                        }
                    }
                } else if stream.index() == audio_stream_index {
                    // Decode audio
                    match audio_decoder.decode(&packet) {
                        Ok(samples) => {
                            for sample in samples {
                                audio_buffer.push(sample);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Audio decode error: {}", e);
                        }
                    }
                }
            }
            None => {
                // End of stream - pause playback and wait
                tracing::info!("End of stream reached");

                // Pause playback
                clock.set_state(PlaybackState::Paused);

                // Sleep and wait for user interaction (seek, play, etc)
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        }

        // Backpressure: audio buffer must NOT overflow (breaks clock sync)
        // Video buffer can overflow (we drop frames gracefully)
        if clock.state().is_playing() {
            // CRITICAL: Audio buffer overflow breaks PTS tracking
            // Sleep aggressively when audio buffer is nearly full
            if audio_buffer.is_nearly_full() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            } else if audio_buffer.buffered_duration() > 1.0 {
                // More than 1 second buffered, slow down
                std::thread::sleep(std::time::Duration::from_millis(5));
            } else {
                // Less than 1 second buffered, decode fast to stay ahead
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        } else {
            // When paused, fill buffers but don't spin too fast
            if frame_buffer.is_full() || audio_buffer.is_nearly_full() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            } else {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    tracing::info!("Decode loop exited");
    Ok(())
}
