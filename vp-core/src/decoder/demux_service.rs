use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use ffmpeg_next as ffmpeg;

use crate::error::{VpError, VpResult};
use crate::types::PTS;

/// Packet queue capacity per stream
const PACKET_QUEUE_CAPACITY: usize = 100;

/// Commands for the demuxer
#[derive(Debug, Clone)]
pub enum DemuxCommand {
    Seek(PTS),
    Stop,
}

/// Demuxer service that owns the FFmpeg input and runs in its own thread
///
/// This eliminates lock contention by having a dedicated thread for demuxing.
/// Video and audio workers pull packets from separate queues.
pub struct DemuxService {
    // Packet queues (shared with workers)
    video_packets: Arc<(Mutex<VecDeque<ffmpeg::Packet>>, Condvar)>,
    audio_packets: Arc<(Mutex<VecDeque<ffmpeg::Packet>>, Condvar)>,

    // Control
    command_tx: crossbeam_channel::Sender<DemuxCommand>,
    stop_signal: Arc<AtomicBool>,

    // Stream info
    video_stream_index: usize,
    audio_stream_index: usize,
    duration: f64,
    path: PathBuf,
}

impl DemuxService {
    pub fn new(
        path: PathBuf,
        video_stream_index: usize,
        audio_stream_index: usize,
    ) -> VpResult<(Self, Option<JoinHandle<()>>)> {
        tracing::info!("DemuxService::new() for {:?}", path);

        // Open input to get duration
        let input = ffmpeg::format::input(&path)?;
        let duration = if input.duration() > 0 {
            input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
        } else {
            0.0
        };
        drop(input); // Close, will reopen in demux thread

        // Create packet queues
        let video_packets = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let audio_packets = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));

        // Create command channel
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let stop_signal = Arc::new(AtomicBool::new(false));

        // Spawn demux thread
        let demux_thread = {
            let video_packets = Arc::clone(&video_packets);
            let audio_packets = Arc::clone(&audio_packets);
            let stop_signal = Arc::clone(&stop_signal);
            let path_clone = path.clone();

            thread::Builder::new()
                .name("demux-thread".to_string())
                .spawn(move || {
                    if let Err(e) = demux_loop(
                        path_clone,
                        video_stream_index,
                        audio_stream_index,
                        video_packets,
                        audio_packets,
                        command_rx,
                        stop_signal,
                    ) {
                        tracing::error!("Demux thread error: {}", e);
                    }
                })
                .map_err(|e| VpError::Io(e))?
        };

        Ok((
            Self {
                video_packets,
                audio_packets,
                command_tx,
                stop_signal,
                video_stream_index,
                audio_stream_index,
                duration,
                path,
            },
            Some(demux_thread),
        ))
    }

    /// Get the file path
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Get the next video packet (blocks if queue is empty)
    pub fn get_video_packet(&self) -> Option<ffmpeg::Packet> {
        let (lock, cvar) = &*self.video_packets;
        let mut queue = lock.lock().unwrap();

        loop {
            if let Some(packet) = queue.pop_front() {
                // Notify demuxer that space is available
                cvar.notify_one();
                return Some(packet);
            }

            if self.stop_signal.load(Ordering::Relaxed) {
                return None;
            }

            // Wait for packet to be available
            queue = cvar.wait(queue).unwrap();
        }
    }

    /// Try to get the next video packet (non-blocking)
    pub fn try_get_video_packet(&self) -> Option<ffmpeg::Packet> {
        let (lock, cvar) = &*self.video_packets;
        let mut queue = lock.lock().unwrap();

        if let Some(packet) = queue.pop_front() {
            // Notify demuxer that space is available
            cvar.notify_one();
            Some(packet)
        } else {
            None
        }
    }

    /// Get the next audio packet (blocks if queue is empty)
    pub fn get_audio_packet(&self) -> Option<ffmpeg::Packet> {
        let (lock, cvar) = &*self.audio_packets;
        let mut queue = lock.lock().unwrap();

        loop {
            if let Some(packet) = queue.pop_front() {
                // Notify demuxer that space is available
                cvar.notify_one();
                return Some(packet);
            }

            if self.stop_signal.load(Ordering::Relaxed) {
                return None;
            }

            // Wait for packet to be available
            queue = cvar.wait(queue).unwrap();
        }
    }

    /// Try to get the next audio packet (non-blocking)
    pub fn try_get_audio_packet(&self) -> Option<ffmpeg::Packet> {
        let (lock, cvar) = &*self.audio_packets;
        let mut queue = lock.lock().unwrap();

        if let Some(packet) = queue.pop_front() {
            // Notify demuxer that space is available
            cvar.notify_one();
            Some(packet)
        } else {
            None
        }
    }

    /// Send a seek command to the demuxer
    pub fn seek(&self, target_pts: PTS) -> VpResult<()> {
        self.command_tx.send(DemuxCommand::Seek(target_pts))?;
        Ok(())
    }

    /// Get the duration of the media file
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Get video stream index
    pub fn video_stream_index(&self) -> usize {
        self.video_stream_index
    }

    /// Get audio stream index
    pub fn audio_stream_index(&self) -> usize {
        self.audio_stream_index
    }

    /// Stop the demuxer
    pub fn stop(&self) {
        self.stop_signal.store(true, Ordering::Relaxed);
        let _ = self.command_tx.send(DemuxCommand::Stop);

        // Wake up any waiting consumers
        let (_, cvar) = &*self.video_packets;
        cvar.notify_all();
        let (_, cvar) = &*self.audio_packets;
        cvar.notify_all();
    }
}

impl Drop for DemuxService {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Demux loop running in dedicated thread
fn demux_loop(
    path: PathBuf,
    video_stream_index: usize,
    audio_stream_index: usize,
    video_packets: Arc<(Mutex<VecDeque<ffmpeg::Packet>>, Condvar)>,
    audio_packets: Arc<(Mutex<VecDeque<ffmpeg::Packet>>, Condvar)>,
    command_rx: crossbeam_channel::Receiver<DemuxCommand>,
    stop_signal: Arc<AtomicBool>,
) -> VpResult<()> {
    tracing::info!("Demux thread started");

    // Initialize FFmpeg and open input
    ffmpeg::init().map_err(|e| VpError::Ffmpeg(e.to_string()))?;
    let mut input = ffmpeg::format::input(&path)?;

    let mut iteration = 0u64;
    loop {
        iteration += 1;

        // Check for stop signal
        if stop_signal.load(Ordering::Relaxed) {
            tracing::info!("Demux thread stopping");
            break;
        }

        // Log queue sizes periodically
        if iteration % 100 == 0 {
            let (v_lock, _) = &*video_packets;
            let (a_lock, _) = &*audio_packets;
            let v_len = v_lock.lock().unwrap().len();
            let a_len = a_lock.lock().unwrap().len();
            tracing::debug!("🎬 Demuxer: video_queue={}, audio_queue={}", v_len, a_len);
        }

        // Check for commands
        if let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                DemuxCommand::Seek(target_pts) => {
                    tracing::info!("Demux thread: seeking to {:.2}s", target_pts);

                    // Clear packet queues
                    {
                        let (lock, cvar) = &*video_packets;
                        let mut queue = lock.lock().unwrap();
                        queue.clear();
                        cvar.notify_all();
                    }
                    {
                        let (lock, cvar) = &*audio_packets;
                        let mut queue = lock.lock().unwrap();
                        queue.clear();
                        cvar.notify_all();
                    }

                    // Seek FFmpeg input
                    let timestamp = (target_pts * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
                    if let Err(e) = input.seek(timestamp, ..timestamp) {
                        tracing::error!("Demux seek error: {}", e);
                    }
                }
                DemuxCommand::Stop => {
                    stop_signal.store(true, Ordering::Relaxed);
                    tracing::info!("Demux thread: stop command received");
                    break;
                }
            }
        }

        // Read next packet
        match input.packets().next() {
            Some((stream, packet)) => {
                if stream.index() == video_stream_index {
                    // Push to video queue (blocking if full)
                    let (lock, cvar) = &*video_packets;
                    let mut queue = lock.lock().unwrap();

                    // Wait if queue is full
                    while queue.len() >= PACKET_QUEUE_CAPACITY {
                        tracing::trace!("🎬 Demuxer: video queue full, waiting");
                        if stop_signal.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        queue = cvar.wait(queue).unwrap();
                    }

                    queue.push_back(packet);
                    tracing::trace!("🎬 Demuxer: pushed video packet, queue_len={}", queue.len());
                    cvar.notify_one(); // Notify consumer
                } else if stream.index() == audio_stream_index {
                    // Push to audio queue (blocking if full)
                    let (lock, cvar) = &*audio_packets;
                    let mut queue = lock.lock().unwrap();

                    // Wait if queue is full
                    while queue.len() >= PACKET_QUEUE_CAPACITY {
                        tracing::trace!("🎬 Demuxer: audio queue full, waiting");
                        if stop_signal.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        queue = cvar.wait(queue).unwrap();
                    }

                    queue.push_back(packet);
                    tracing::trace!("🎬 Demuxer: pushed audio packet, queue_len={}", queue.len());
                    cvar.notify_one(); // Notify consumer
                }
            }
            None => {
                // End of stream - wait a bit then check for commands
                tracing::debug!("Demux thread: EOF reached");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }

    tracing::info!("Demux thread exited");
    Ok(())
}
