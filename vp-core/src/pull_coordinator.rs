use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use crate::buffer::{AudioBuffer, FrameBuffer};
use crate::decoder::{AudioDecoder, DemuxService, VideoDecoder};
use crate::error::VpResult;
use crate::sync::{LoadingState, PlaybackClock};
use crate::types::PTS;

/// Frame drop threshold - drop frames older than this
const FRAME_DROP_THRESHOLD_SECS: f64 = 0.1;

/// Commands for the pull coordinator
#[derive(Debug, Clone)]
pub enum CoordinatorCommand {
    /// Seek to a specific timestamp
    Seek(PTS),
    /// Request immediate buffer refill
    Refill,
    /// Stop the coordinator
    Stop,
}

/// Pull-based decode coordinator with separate demuxer thread
///
/// Architecture:
/// - Demuxer thread: Owns FFmpeg input, distributes packets to queues
/// - Video worker: Pulls video packets, decodes with own VideoDecoder
/// - Audio worker: Pulls audio packets, decodes with own AudioDecoder
///
/// This eliminates lock contention between video and audio decoding.
pub struct PullCoordinator {
    // Demuxer service (runs in its own thread)
    demux_service: Arc<DemuxService>,

    // Buffers
    frame_buffer: FrameBuffer,
    audio_buffer: AudioBuffer,
    clock: PlaybackClock,

    // Worker threads
    video_worker: Option<JoinHandle<()>>,
    audio_worker: Option<JoinHandle<()>>,

    // Coordination
    stop_signal: Arc<AtomicBool>,
    video_wake: Arc<(Mutex<bool>, Condvar)>,
    audio_wake: Arc<(Mutex<bool>, Condvar)>,

    // Command channel
    command_tx: Sender<CoordinatorCommand>,
    command_rx: Receiver<CoordinatorCommand>,
}

impl PullCoordinator {
    /// Create a new pull coordinator
    pub fn new(
        demux_service: DemuxService,
        frame_buffer: FrameBuffer,
        audio_buffer: AudioBuffer,
        clock: PlaybackClock,
    ) -> Self {
        let (command_tx, command_rx) = crossbeam_channel::unbounded();

        Self {
            demux_service: Arc::new(demux_service),
            frame_buffer,
            audio_buffer,
            clock,
            video_worker: None,
            audio_worker: None,
            stop_signal: Arc::new(AtomicBool::new(false)),
            video_wake: Arc::new((Mutex::new(false), Condvar::new())),
            audio_wake: Arc::new((Mutex::new(false), Condvar::new())),
            command_tx,
            command_rx,
        }
    }

    /// Start the pull workers
    pub fn start(&mut self) -> VpResult<()> {
        tracing::info!("Starting PullCoordinator workers");

        // Reset stop signal
        self.stop_signal.store(false, Ordering::Relaxed);

        // Start video worker
        let video_worker = self.spawn_video_worker()?;
        self.video_worker = Some(video_worker);

        // Start audio worker
        let audio_worker = self.spawn_audio_worker()?;
        self.audio_worker = Some(audio_worker);

        tracing::info!("PullCoordinator workers started");
        Ok(())
    }

    /// Stop the pull workers
    pub fn stop(&mut self) {
        tracing::info!("Stopping PullCoordinator workers");

        // Set stop signal
        self.stop_signal.store(true, Ordering::Relaxed);

        // Wake workers so they can check stop signal
        self.wake_video();
        self.wake_audio();

        // Wait for workers to finish
        if let Some(worker) = self.video_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.audio_worker.take() {
            let _ = worker.join();
        }

        tracing::info!("PullCoordinator workers stopped");
    }

    /// Request buffer refill (wake both workers)
    pub fn request_refill(&self) {
        self.wake_video();
        self.wake_audio();
    }

    /// Send a command to the coordinator
    pub fn send_command(&self, cmd: CoordinatorCommand) -> VpResult<()> {
        self.command_tx.send(cmd)?;
        // Wake workers so they process the command
        self.wake_video();
        self.wake_audio();
        Ok(())
    }

    /// Wake video worker
    fn wake_video(&self) {
        let (lock, cvar) = &*self.video_wake;
        let mut notified = lock.lock().unwrap();
        *notified = true;
        cvar.notify_one();
    }

    /// Wake audio worker
    fn wake_audio(&self) {
        let (lock, cvar) = &*self.audio_wake;
        let mut notified = lock.lock().unwrap();
        *notified = true;
        cvar.notify_one();
    }

    /// Spawn the video pull worker thread
    fn spawn_video_worker(&self) -> VpResult<JoinHandle<()>> {
        let demux_service = Arc::clone(&self.demux_service);
        let frame_buffer = self.frame_buffer.clone();
        let clock = self.clock.clone();
        let stop_signal = Arc::clone(&self.stop_signal);
        let wake = Arc::clone(&self.video_wake);
        let command_rx = self.command_rx.clone();

        // Create video decoder
        let video_decoder = create_video_decoder(&demux_service)?;

        Ok(thread::Builder::new()
            .name("video-pull-worker".to_string())
            .spawn(move || {
                video_worker_loop(
                    demux_service,
                    video_decoder,
                    frame_buffer,
                    clock,
                    stop_signal,
                    wake,
                    command_rx,
                )
            })
            .map_err(|e| crate::error::VpError::Io(e))?)
    }

    /// Spawn the audio pull worker thread
    fn spawn_audio_worker(&self) -> VpResult<JoinHandle<()>> {
        let demux_service = Arc::clone(&self.demux_service);
        let audio_buffer = self.audio_buffer.clone();
        let clock = self.clock.clone();
        let stop_signal = Arc::clone(&self.stop_signal);
        let wake = Arc::clone(&self.audio_wake);
        let command_rx = self.command_rx.clone();

        // Create audio decoder
        let audio_decoder = create_audio_decoder(&demux_service)?;

        Ok(thread::Builder::new()
            .name("audio-pull-worker".to_string())
            .spawn(move || {
                audio_worker_loop(
                    demux_service,
                    audio_decoder,
                    audio_buffer,
                    clock,
                    stop_signal,
                    wake,
                    command_rx,
                )
            })
            .map_err(|e| crate::error::VpError::Io(e))?)
    }
}

impl Drop for PullCoordinator {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Create video decoder for the worker
fn create_video_decoder(demux_service: &DemuxService) -> VpResult<VideoDecoder> {
    // Open input to get stream info
    let input = ffmpeg_next::format::input(demux_service.path())?;
    let stream = input.stream(demux_service.video_stream_index())
        .ok_or(crate::error::VpError::NoVideoStream)?;
    VideoDecoder::new(&stream)
}

/// Create audio decoder for the worker
fn create_audio_decoder(demux_service: &DemuxService) -> VpResult<AudioDecoder> {
    // Open input to get stream info
    let input = ffmpeg_next::format::input(demux_service.path())?;
    let stream = input.stream(demux_service.audio_stream_index())
        .ok_or(crate::error::VpError::NoAudioStream)?;
    AudioDecoder::new(&stream, 48000)
}

/// Video worker loop
fn video_worker_loop(
    demux_service: Arc<DemuxService>,
    mut video_decoder: VideoDecoder,
    frame_buffer: FrameBuffer,
    clock: PlaybackClock,
    stop_signal: Arc<AtomicBool>,
    wake: Arc<(Mutex<bool>, Condvar)>,
    command_rx: Receiver<CoordinatorCommand>,
) {
    tracing::info!("Video worker started");

    let mut iteration = 0u64;
    let mut packets_decoded = 0u64;
    let mut frames_produced = 0u64;

    loop {
        iteration += 1;

        // Log stats periodically
        if iteration % 100 == 0 {
            tracing::debug!(
                "🎥 Video worker stats: iteration={}, packets_decoded={}, frames_produced={}, buffer_len={}",
                iteration, packets_decoded, frames_produced, frame_buffer.len()
            );
        }
        // Check for stop signal
        if stop_signal.load(Ordering::Relaxed) {
            tracing::info!("Video worker stopping");
            break;
        }

        // Check for commands
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                CoordinatorCommand::Seek(target_pts) => {
                    tracing::info!("Video worker handling seek to {:.2}s", target_pts);
                    clock.set_loading_state(LoadingState::Seeking);
                    frame_buffer.clear();

                    // Send seek to demuxer
                    if let Err(e) = demux_service.seek(target_pts) {
                        tracing::error!("Video worker seek error: {}", e);
                    }

                    // Flush decoder
                    let _ = video_decoder.flush();

                    clock.set_loading_state(LoadingState::Ready);
                }
                CoordinatorCommand::Refill => {
                    // Just continue to refill logic below
                }
                CoordinatorCommand::Stop => {
                    stop_signal.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        // Check if buffer needs refilling
        let needs_refill = frame_buffer.needs_refill();
        let is_critically_low = frame_buffer.is_critically_low();
        let is_playing = clock.state().is_playing();

        // During playback, also check if buffered frames cover current playback position
        let current_time = clock.current_time();
        let buffer_range = frame_buffer.pts_range();
        let needs_frames_for_current_time = if is_playing {
            if let Some((min_pts, max_pts)) = buffer_range {
                // Need to decode if latest frame is behind current time or doesn't have enough lookahead
                const LOOKAHEAD_SECONDS: f64 = 0.5; // Buffer at least 0.5s ahead
                max_pts < current_time + LOOKAHEAD_SECONDS
            } else {
                // Buffer is empty
                true
            }
        } else {
            false
        };

        if !needs_refill && !is_critically_low && !needs_frames_for_current_time {
            // Buffer is healthy, wait for wake signal
            let (lock, cvar) = &*wake;
            let mut notified = match lock.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::error!("Video worker: wake lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };

            // Wait with timeout
            let wait_result = match cvar.wait_timeout(notified, Duration::from_millis(50)) {
                Ok(result) => result,
                Err(_) => {
                    tracing::error!("Video worker: condvar wait error, stopping");
                    break;
                }
            };
            notified = wait_result.0;

            if *notified {
                *notified = false;
            }

            continue;
        }

        if needs_frames_for_current_time {
            tracing::debug!("🎥 Video worker: needs frames for current time {:.3}, buffer_range={:?}",
                current_time, buffer_range);
        }

        // Periodically remove old frames during playback to free memory
        if is_playing && iteration % 50 == 0 {
            frame_buffer.remove_old_frames(current_time);
        }

        // Update loading state if critically low
        if is_critically_low && is_playing {
            clock.set_loading_state(LoadingState::Buffering);
        }

        // Get packet from demuxer (non-blocking to avoid deadlock)
        let packet = match demux_service.try_get_video_packet() {
            Some(p) => {
                tracing::trace!("🎥 Video worker: got packet");
                p
            }
            None => {
                // No packet available, wait a bit
                tracing::trace!("🎥 Video worker: no packet available, sleeping");
                thread::sleep(Duration::from_millis(5));
                continue;
            }
        };

        // Decode packet
        tracing::trace!("🎥 Video worker: decoding packet");
        packets_decoded += 1;
        match video_decoder.decode(&packet) {
            Ok(frames) => {
                let current_time = clock.current_time();
                let frame_count = frames.len();
                frames_produced += frame_count as u64;

                tracing::debug!("🎥 Video worker: decoded {} frames, current_time={:.3}, buffer_before={}",
                    frame_count, current_time, frame_buffer.len());

                for frame in frames {
                    // Frame dropping: skip if already late
                    if is_playing && frame.pts < current_time - FRAME_DROP_THRESHOLD_SECS {
                        tracing::debug!("🎥 Dropping late frame at PTS {:.3}", frame.pts);
                        continue;
                    }

                    tracing::debug!("🎥 Video worker: pushing frame at PTS {:.3}", frame.pts);
                    frame_buffer.push(frame);
                }

                let buffer_after = frame_buffer.len();
                let buffer_range = frame_buffer.pts_range();
                tracing::debug!("🎥 Video worker: buffer_after={}, range={:?}", buffer_after, buffer_range);

                // Clear buffering state if healthy
                if !frame_buffer.is_critically_low() && clock.loading_state() == LoadingState::Buffering {
                    clock.set_loading_state(LoadingState::Ready);
                }
            }
            Err(e) => {
                tracing::warn!("🎥 Video decode error: {}", e);
            }
        }
    }

    tracing::info!("Video worker stopped");
}

/// Audio worker loop
fn audio_worker_loop(
    demux_service: Arc<DemuxService>,
    mut audio_decoder: AudioDecoder,
    audio_buffer: AudioBuffer,
    clock: PlaybackClock,
    stop_signal: Arc<AtomicBool>,
    wake: Arc<(Mutex<bool>, Condvar)>,
    command_rx: Receiver<CoordinatorCommand>,
) {
    tracing::info!("Audio worker started");

    loop {
        // Check for stop signal
        if stop_signal.load(Ordering::Relaxed) {
            tracing::info!("Audio worker stopping");
            break;
        }

        // Check for commands
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                CoordinatorCommand::Seek(target_pts) => {
                    tracing::info!("Audio worker handling seek to {:.2}s", target_pts);
                    audio_buffer.reset(target_pts);

                    // Flush decoder
                    let _ = audio_decoder.flush();
                }
                CoordinatorCommand::Refill => {
                    // Just continue to refill logic below
                }
                CoordinatorCommand::Stop => {
                    stop_signal.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        // Check if buffer needs refilling
        let needs_refill = audio_buffer.needs_refill();
        let is_critically_low = audio_buffer.is_critically_low();
        let is_playing = clock.state().is_playing();

        if !needs_refill && !is_critically_low {
            // Buffer is healthy, wait for wake signal
            let (lock, cvar) = &*wake;
            let mut notified = match lock.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::error!("Audio worker: wake lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };

            // Wait with shorter timeout for audio
            let wait_result = match cvar.wait_timeout(notified, Duration::from_millis(20)) {
                Ok(result) => result,
                Err(_) => {
                    tracing::error!("Audio worker: condvar wait error, stopping");
                    break;
                }
            };
            notified = wait_result.0;

            if *notified {
                *notified = false;
            }

            continue;
        }

        // Update loading state if critically low
        if is_critically_low && is_playing {
            clock.set_loading_state(LoadingState::Buffering);
        }

        // Get packet from demuxer (non-blocking to avoid deadlock)
        let packet = match demux_service.try_get_audio_packet() {
            Some(p) => p,
            None => {
                // No packet available, wait a bit
                thread::sleep(Duration::from_millis(5));
                continue;
            }
        };

        // Decode packet
        match audio_decoder.decode(&packet) {
            Ok(samples) => {
                for sample in samples {
                    audio_buffer.push(sample);
                }

                // Clear buffering state if healthy
                if !audio_buffer.is_critically_low() && clock.loading_state() == LoadingState::Buffering {
                    clock.set_loading_state(LoadingState::Ready);
                }
            }
            Err(e) => {
                tracing::warn!("Audio decode error: {}", e);
            }
        }
    }

    tracing::info!("Audio worker stopped");
}
