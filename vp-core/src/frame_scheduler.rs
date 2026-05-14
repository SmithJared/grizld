use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};

use crate::cache::{AudioCache, FrameCache};
use crate::decoder::{AudioDecoder, DemuxService, VideoDecoder};
use crate::error::VpResult;
use crate::sync::PlaybackClock;

/// Frame drop threshold - drop frames older than this
const FRAME_DROP_THRESHOLD_SECS: f64 = 0.1;

/// Commands for the pull coordinator
#[derive(Debug, Clone)]
pub enum DecoderCommand {
    /// Flush Decoder
    Flush,
    /// Request immediate buffer refill
    FrameRequested,
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
pub struct FrameScheduler {
    // Demuxer service (runs in its own thread)
    demux_service: Arc<DemuxService>,

    // Buffers
    frame_cache: FrameCache,
    audio_cache: AudioCache,
    clock: PlaybackClock,

    // Worker threads
    video_worker: Option<JoinHandle<()>>,
    audio_worker: Option<JoinHandle<()>>,
    demux_worker: Option<JoinHandle<()>>,

    // Command channels
    video_command_tx: Sender<DecoderCommand>,
    video_command_rx: Receiver<DecoderCommand>,
    audio_command_tx: Sender<DecoderCommand>,
    audio_command_rx: Receiver<DecoderCommand>,
}

impl FrameScheduler {
    /// Create a new pull coordinator
    pub fn new(
        demux_service: DemuxService,
        demux_worker: Option<JoinHandle<()>>,
        clock: PlaybackClock,
        frame_cache: FrameCache,
        audio_cache: AudioCache,
    ) -> Self {
        let (video_command_tx, video_command_rx) = crossbeam_channel::unbounded();
        let (audio_command_tx, audio_command_rx) = crossbeam_channel::unbounded();

        Self {
            demux_service: Arc::new(demux_service),
            demux_worker,
            frame_cache,
            audio_cache,
            clock,
            video_worker: None,
            audio_worker: None,
            video_command_tx,
            video_command_rx,
            audio_command_tx,
            audio_command_rx,
        }
    }

    /// Start the pull workers
    pub fn start(&mut self) -> VpResult<()> {
        // Start video worker
        let video_worker = self.spawn_video_worker()?;
        self.video_worker = Some(video_worker);

        // Start audio worker
        let audio_worker = self.spawn_audio_worker()?;
        self.audio_worker = Some(audio_worker);
        Ok(())
    }

    // TODO: Add demuxer to the stop
    //
    /// Stop the pull workers
    pub fn stop_all(&mut self) {
        tracing::info!("Stopping Decoder/Demuxer workers");

        self.video_command_tx.try_send(DecoderCommand::Stop);
        self.audio_command_tx.try_send(DecoderCommand::Stop);
        self.demux_service.stop();
        // Wait for workers to finish
        if let Some(worker) = self.video_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.audio_worker.take() {
            let _ = worker.join();
        }
        if let Some(thread) = self.demux_worker.take() {
            let _ = thread.join();
        }

        tracing::info!("Workers stopped");
    }

    /// Request a frame for the current playback time and trigger refilling if needed
    ///
    /// This method:
    /// 1. Gets the frame for current playback position
    /// 2. Checks cache health and PTS coverage
    /// 3. Triggers video worker to decode more frames if needed
    /// 4. Prunes old frames from behind cache
    ///
    /// Returns the frame for display, or None if no frame available
    pub fn request_frame(&self) -> Option<crate::types::VideoFrame> {
        // TODO: If the frame is nonexistent then we need to handle that
        let current_time = self.clock.current_time();

        // Get frame for current time
        let frame = self.frame_cache.request_frame(current_time);

        // Trigger video worker if cache needs frames
        if self.frame_cache.needs_refill() {
            let frames_to_request = self.frame_cache.frames_needed().max(5);

            tracing::debug!(
                "🎥 Requesting {} frames:  current_time={:.3}",
                frames_to_request,
                current_time
            );

            // Send multiple decode requests to video worker
            for _ in 0..frames_to_request {
                let _ = self.video_command_tx.send(DecoderCommand::FrameRequested);
            }
        }

        if self.audio_cache.needs_refill() {
            // Calculate how many samples to request based on shortage
            let duration_needed = 2.0 - self.audio_cache.ahead_duration(); // Fill to 2 seconds
            let packets_needed = (duration_needed / 0.021).ceil() as usize; // ~21ms per packet
            let packets_to_request = packets_needed.max(5).min(50); // Between 5 and 50 packets

            tracing::debug!("🔊 Requesting {} audio packets", packets_to_request,);

            // Send decode requests to audio worker
            for _ in 0..packets_to_request {
                let _ = self.audio_command_tx.send(DecoderCommand::FrameRequested);
            }
        }

        frame
    }

    /// Check audio cache health and trigger refilling if needed
    ///
    /// This should be called periodically (e.g., from the audio callback or main loop)
    /// to ensure audio cache stays filled during playback.
    pub fn check_audio_health(&self) {
        let is_playing = self.clock.state().is_playing();

        if !is_playing {
            return;
        }
    }

    /// Get audio cache for external access (e.g., audio callback)
    pub fn audio_cache(&self) -> &AudioCache {
        &self.audio_cache
    }

    /// Get frame cache for external access
    pub fn frame_cache(&self) -> &FrameCache {
        &self.frame_cache
    }

    /// Spawn the video pull worker thread
    fn spawn_video_worker(&self) -> VpResult<JoinHandle<()>> {
        let demux_service = Arc::clone(&self.demux_service);
        let frame_cache = self.frame_cache.clone();
        let command_rx = self.video_command_rx.clone();

        // Create video decoder
        let video_decoder = create_video_decoder(&demux_service)?;

        Ok(thread::Builder::new()
            .name("video-pull-worker".to_string())
            .spawn(move || video_worker_loop(demux_service, video_decoder, frame_cache, command_rx))
            .map_err(|e| crate::error::VpError::Io(e))?)
    }

    /// Spawn the audio pull worker thread
    fn spawn_audio_worker(&self) -> VpResult<JoinHandle<()>> {
        let demux_service = Arc::clone(&self.demux_service);
        let audio_cache = self.audio_cache.clone();
        let command_rx = self.audio_command_rx.clone();

        // Create audio decoder
        let audio_decoder = create_audio_decoder(&demux_service)?;

        Ok(thread::Builder::new()
            .name("audio-pull-worker".to_string())
            .spawn(move || audio_worker_loop(demux_service, audio_decoder, audio_cache, command_rx))
            .map_err(|e| crate::error::VpError::Io(e))?)
    }
}

impl Drop for FrameScheduler {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Create video decoder for the worker
fn create_video_decoder(demux_service: &DemuxService) -> VpResult<VideoDecoder> {
    // Open input to get stream info
    let input = ffmpeg_next::format::input(demux_service.path())?;
    let stream = input
        .stream(demux_service.video_stream_index())
        .ok_or(crate::error::VpError::NoVideoStream)?;
    VideoDecoder::new(&stream)
}

/// Create audio decoder for the worker
fn create_audio_decoder(demux_service: &DemuxService) -> VpResult<AudioDecoder> {
    // Open input to get stream info
    let input = ffmpeg_next::format::input(demux_service.path())?;
    let stream = input
        .stream(demux_service.audio_stream_index())
        .ok_or(crate::error::VpError::NoAudioStream)?;
    AudioDecoder::new(&stream, 48000)
}

/// Video worker loop
fn video_worker_loop(
    demux_service: Arc<DemuxService>,
    mut video_decoder: VideoDecoder,
    frame_cache: FrameCache,
    command_rx: Receiver<DecoderCommand>,
) {
    tracing::info!("Video worker started");

    let mut packets_decoded = 0u64;
    let mut frames_produced = 0u64;

    // Check for commands
    while let Ok(cmd) = command_rx.recv() {
        match cmd {
            DecoderCommand::Flush => {
                tracing::info!("🎥 Video worker: flushing decoder");
                let _ = video_decoder.flush();
            }
            DecoderCommand::FrameRequested => {
                // Get packet from demuxer (non-blocking to avoid deadlock)
                let packet = match demux_service.try_get_video_packet() {
                    Some(p) => {
                        tracing::trace!("🎥 Video worker: got packet");
                        p
                    }
                    None => {
                        // No packet available, try again
                        tracing::trace!("🎥 Video worker: no packet available");
                        continue;
                    }
                };

                // Decode packet
                tracing::trace!("🎥 Video worker: decoding packet");
                packets_decoded += 1;

                match video_decoder.decode(&packet) {
                    Ok(frames) => {
                        let frame_count = frames.len();
                        frames_produced += frame_count as u64;

                        for frame in frames {
                            tracing::debug!(
                                "🎥 Video worker: pushing frame at PTS {:.3}",
                                frame.pts
                            );
                            frame_cache.push(frame);
                        }

                        // Log stats periodically
                        if packets_decoded % 100 == 0 {
                            tracing::debug!(
                                "🎥 Video worker stats: packets_decoded={}, frames_produced={}, ahead_cache={}",
                                packets_decoded,
                                frames_produced,
                                frame_cache.ahead_len()
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("🎥 Video decode error: {}", e);
                    }
                }
            }
            DecoderCommand::Stop => {
                tracing::info!("🎥 Video worker stopping");
                break;
            }
        }
    }

    tracing::info!("🎥 Video worker stopped");
}

/// Audio worker loop
fn audio_worker_loop(
    demux_service: Arc<DemuxService>,
    mut audio_decoder: AudioDecoder,
    audio_cache: AudioCache,
    command_rx: Receiver<DecoderCommand>,
) {
    tracing::info!("Audio worker started");

    let mut packets_decoded = 0u64;
    let mut samples_produced = 0u64;

    // Check for commands
    while let Ok(cmd) = command_rx.recv() {
        match cmd {
            DecoderCommand::Flush => {
                tracing::info!("🔊 Audio worker: flushing decoder");
                let _ = audio_decoder.flush();
            }
            DecoderCommand::FrameRequested => {
                // Get packet from demuxer (non-blocking to avoid deadlock)
                let packet = match demux_service.try_get_audio_packet() {
                    Some(p) => p,
                    None => {
                        // No packet available, try again
                        tracing::trace!("🔊 Audio worker: no packet available");
                        continue;
                    }
                };

                // Decode packet
                packets_decoded += 1;

                match audio_decoder.decode(&packet) {
                    Ok(samples) => {
                        for sample in samples {
                            let sample_count = sample.data.len();
                            samples_produced += sample_count as u64;
                            audio_cache.push(sample);
                        }

                        // Log stats periodically
                        if packets_decoded % 100 == 0 {
                            tracing::debug!(
                                "🔊 Audio worker stats: packets_decoded={}, samples_produced={}, ahead_duration={:.3}s",
                                packets_decoded,
                                samples_produced,
                                audio_cache.ahead_duration()
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("🔊 Audio decode error: {}", e);
                    }
                }
            }
            DecoderCommand::Stop => {
                tracing::info!("🔊 Audio worker stopping");
                break;
            }
        }
    }

    tracing::info!("🔊 Audio worker stopped");
}
