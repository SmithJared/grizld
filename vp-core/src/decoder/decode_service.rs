use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use ffmpeg_next as ffmpeg;

use crate::error::{VpError, VpResult};
use crate::types::{AudioSample, PTS, VideoFrame};

use super::{AudioDecoder, VideoDecoder};

/// Maximum packets to buffer per stream (demuxing read-ahead)
/// Increased to reduce lock contention between video/audio workers
const MAX_PACKET_QUEUE_SIZE: usize = 50;

/// Stateful decode service for on-demand video/audio decoding
///
/// This service maintains FFmpeg decoder state and allows pull-based
/// decoding instead of continuous push-based decoding.
pub struct DecodeService {
    // FFmpeg input context
    input: ffmpeg::format::context::Input,

    // Decoders
    video_decoder: VideoDecoder,
    audio_decoder: AudioDecoder,

    // Stream indices
    video_stream_index: usize,
    audio_stream_index: usize,

    // Packet queues (demuxed packets waiting to be decoded)
    video_packet_queue: VecDeque<ffmpeg::Packet>,
    audio_packet_queue: VecDeque<ffmpeg::Packet>,

    // State tracking
    eof_reached: bool,
    current_demux_position: PTS,

    // File info
    duration: f64,
    path: PathBuf,
}

impl DecodeService {
    /// Create a new decode service for the given file
    pub fn new(
        path: PathBuf,
        video_stream_index: usize,
        audio_stream_index: usize,
        sample_rate: u32,
    ) -> VpResult<Self> {
        tracing::info!("DecodeService::new() called for {:?}", path);

        // Initialize FFmpeg
        ffmpeg::init().map_err(|e| VpError::Ffmpeg(e.to_string()))?;
        tracing::debug!("FFmpeg initialized");

        // Open input file
        tracing::debug!("Opening input file");
        let input = ffmpeg::format::input(&path)?;
        tracing::debug!("Input file opened successfully");

        // Get video stream
        let video_stream = input
            .stream(video_stream_index)
            .ok_or_else(|| VpError::NoVideoStream)?;

        // Get audio stream
        let audio_stream = input
            .stream(audio_stream_index)
            .ok_or_else(|| VpError::NoAudioStream)?;

        // Create decoders
        let video_decoder = VideoDecoder::new(&video_stream)?;
        let audio_decoder = AudioDecoder::new(&audio_stream, sample_rate)?;

        // Get duration
        let duration = if input.duration() > 0 {
            input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
        } else {
            0.0
        };

        tracing::info!(
            "DecodeService created for {:?}, duration: {:.2}s",
            path,
            duration
        );

        Ok(Self {
            input,
            video_decoder,
            audio_decoder,
            video_stream_index,
            audio_stream_index,
            video_packet_queue: VecDeque::new(),
            audio_packet_queue: VecDeque::new(),
            eof_reached: false,
            current_demux_position: 0.0,
            duration,
            path,
        })
    }

    /// Decode video frames on-demand
    ///
    /// Attempts to decode `count` frames. May return fewer if EOF is reached
    /// or if packets need to be demuxed first.
    pub fn decode_video_frames(&mut self, count: usize) -> VpResult<Vec<VideoFrame>> {
        tracing::debug!("decode_video_frames: requesting {} frames", count);
        let mut frames = Vec::new();

        // Ensure we have packets to decode
        self.refill_packet_queues()?;
        tracing::debug!("After refill: video_queue={}, audio_queue={}",
            self.video_packet_queue.len(), self.audio_packet_queue.len());

        // Decode up to `count` frames
        while frames.len() < count {
            // Get next video packet
            let packet = match self.video_packet_queue.pop_front() {
                Some(packet) => packet,
                None => {
                    // No more packets, try to refill
                    self.refill_packet_queues()?;
                    match self.video_packet_queue.pop_front() {
                        Some(packet) => packet,
                        None => {
                            // Still no packets, we've reached EOF
                            if !self.eof_reached {
                                // Flush decoder to get remaining frames
                                let flushed_frames = self.video_decoder.flush()?;
                                frames.extend(flushed_frames);
                                self.eof_reached = true;
                            }
                            break;
                        }
                    }
                }
            };

            // Decode packet
            match self.video_decoder.decode(&packet) {
                Ok(decoded_frames) => {
                    frames.extend(decoded_frames);
                }
                Err(e) => {
                    tracing::warn!("Video decode error: {}", e);
                    // Continue decoding despite errors
                }
            }
        }

        Ok(frames)
    }

    /// Decode audio samples on-demand
    ///
    /// Attempts to decode approximately `duration` worth of audio samples.
    /// May return more or less depending on packet boundaries.
    pub fn decode_audio_samples(&mut self, duration: Duration) -> VpResult<Vec<AudioSample>> {
        let mut samples = Vec::new();
        let target_duration = duration.as_secs_f64();
        let mut accumulated_duration = 0.0;

        // Ensure we have packets to decode
        self.refill_packet_queues()?;

        // Decode until we've accumulated enough audio
        while accumulated_duration < target_duration {
            // Get next audio packet
            let packet = match self.audio_packet_queue.pop_front() {
                Some(packet) => packet,
                None => {
                    // No more packets, try to refill
                    self.refill_packet_queues()?;
                    match self.audio_packet_queue.pop_front() {
                        Some(packet) => packet,
                        None => {
                            // Still no packets, we've reached EOF
                            if !self.eof_reached {
                                // Flush decoder to get remaining samples
                                let flushed_samples = self.audio_decoder.flush()?;
                                samples.extend(flushed_samples);
                                self.eof_reached = true;
                            }
                            break;
                        }
                    }
                }
            };

            // Decode packet
            match self.audio_decoder.decode(&packet) {
                Ok(decoded_samples) => {
                    for sample in decoded_samples {
                        accumulated_duration += sample.duration().as_secs_f64();
                        samples.push(sample);
                    }
                }
                Err(e) => {
                    tracing::warn!("Audio decode error: {}", e);
                    // Continue decoding despite errors
                }
            }
        }

        Ok(samples)
    }

    /// Seek to a specific timestamp
    ///
    /// Clears packet queues and seeks the input stream. Decoders are flushed.
    pub fn seek(&mut self, target_pts: PTS) -> VpResult<()> {
        tracing::info!("DecodeService seeking to {:.2}s", target_pts);

        // Clear packet queues
        self.video_packet_queue.clear();
        self.audio_packet_queue.clear();

        // Convert PTS to FFmpeg timestamp
        let timestamp = (target_pts * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;

        // Seek input (seeks to nearest keyframe before timestamp)
        self.input
            .seek(timestamp, ..timestamp)
            .map_err(|e| VpError::Ffmpeg(format!("Seek failed: {}", e)))?;

        // Flush decoders to clear internal state
        self.video_decoder.flush()?;
        self.audio_decoder.flush()?;

        // Reset EOF flag
        self.eof_reached = false;
        self.current_demux_position = target_pts;

        Ok(())
    }

    /// Check if more content is available
    pub fn has_more(&self) -> bool {
        !self.eof_reached
            || !self.video_packet_queue.is_empty()
            || !self.audio_packet_queue.is_empty()
    }

    /// Get the total duration of the media file
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Refill packet queues by demuxing from input
    ///
    /// Reads packets from input stream and distributes them to video/audio queues
    /// until both queues have at least some packets or EOF is reached.
    fn refill_packet_queues(&mut self) -> VpResult<()> {
        // Only refill if at least one queue is getting low
        let video_low = self.video_packet_queue.len() < MAX_PACKET_QUEUE_SIZE / 2;
        let audio_low = self.audio_packet_queue.len() < MAX_PACKET_QUEUE_SIZE / 2;

        if !video_low && !audio_low {
            return Ok(());
        }

        if self.eof_reached {
            return Ok(());
        }

        // Demux packets until queues are sufficiently filled
        let mut packets_read = 0;
        const MAX_PACKETS_PER_REFILL: usize = 50;

        for _ in 0..MAX_PACKETS_PER_REFILL {
            match self.input.packets().next() {
                Some((stream, packet)) => {
                    packets_read += 1;

                    if stream.index() == self.video_stream_index {
                        // Only queue if not full
                        if self.video_packet_queue.len() < MAX_PACKET_QUEUE_SIZE {
                            self.video_packet_queue.push_back(packet);
                        }
                    } else if stream.index() == self.audio_stream_index {
                        // Only queue if not full
                        if self.audio_packet_queue.len() < MAX_PACKET_QUEUE_SIZE {
                            self.audio_packet_queue.push_back(packet);
                        }
                    }

                    // Stop if we've read enough packets (don't need both queues full)
                    if packets_read >= 20 {
                        break;
                    }
                }
                None => {
                    // End of stream
                    tracing::debug!("DecodeService reached EOF during demux");
                    self.eof_reached = true;
                    break;
                }
            }
        }

        if packets_read > 0 {
            tracing::trace!(
                "Refilled packet queues: {} packets, video queue: {}, audio queue: {}",
                packets_read,
                self.video_packet_queue.len(),
                self.audio_packet_queue.len()
            );
        }

        Ok(())
    }
}

impl std::fmt::Debug for DecodeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeService")
            .field("path", &self.path)
            .field("duration", &self.duration)
            .field("video_queue_len", &self.video_packet_queue.len())
            .field("audio_queue_len", &self.audio_packet_queue.len())
            .field("eof_reached", &self.eof_reached)
            .finish()
    }
}
