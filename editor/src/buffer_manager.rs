use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use vp_core::player::VideoPlayer;
use vp_core::types::PlaybackState;

pub type BufferId = usize;

/// A single video buffer (like a vim buffer)
pub struct VideoBuffer {
    pub id: BufferId,
    pub file_path: PathBuf,
    pub player: VideoPlayer,
    /// Track if this buffer was playing before being switched away
    was_playing: bool,
}

impl VideoBuffer {
    pub fn new(id: BufferId, file_path: PathBuf, player: VideoPlayer) -> Self {
        Self {
            id,
            file_path,
            player,
            was_playing: false,
        }
    }

    pub fn file_name(&self) -> String {
        self.file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string()
    }
}

use crate::audio::SharedAudioState;

/// Manages multiple video buffers (vim-style)
pub struct VideoBufferManager {
    buffers: HashMap<BufferId, VideoBuffer>,
    active_buffer_id: Option<BufferId>,
    next_buffer_id: BufferId,
    audio_state: Option<SharedAudioState>,
}

impl VideoBufferManager {
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            active_buffer_id: None,
            next_buffer_id: 1,
            audio_state: None,
        }
    }

    /// Set the shared audio state (called once after AudioOutput is created)
    pub fn set_audio_state(&mut self, audio_state: SharedAudioState) {
        self.audio_state = Some(audio_state);
    }

    /// Update the audio system to point to the active buffer
    fn update_audio_buffer(&self) {
        if let Some(audio_state) = &self.audio_state {
            if let Some(buffer) = self.active_buffer() {
                let audio_buffer = Arc::new(buffer.player.audio_buffer().clone());
                let clock = Arc::new(buffer.player.clock().clone());
                audio_state.set_active(audio_buffer, clock);
                tracing::debug!("Updated audio to buffer {}", buffer.id);
            } else {
                audio_state.clear();
                tracing::debug!("Cleared audio (no active buffer)");
            }
        }
    }

    /// Open a new video file and add it as a buffer
    pub fn open(&mut self, file_path: PathBuf) -> Result<BufferId, String> {
        // Pause the current buffer before opening a new one
        // When opening a new file (not just switching), permanently pause the old one
        if let Some(current_id) = self.active_buffer_id {
            if let Some(buffer) = self.buffers.get_mut(&current_id) {
                buffer.player.pause();
                buffer.was_playing = false; // Don't resume when switching back
                tracing::info!("Paused buffer {} due to opening new file", current_id);
            }
        }

        // Create new video player
        let player = VideoPlayer::new(&file_path)
            .map_err(|e| format!("Failed to open video: {}", e))?;

        let buffer_id = self.next_buffer_id;
        self.next_buffer_id += 1;

        let buffer = VideoBuffer::new(buffer_id, file_path.clone(), player);

        tracing::info!("Opened buffer {}: {}", buffer_id, file_path.display());

        self.buffers.insert(buffer_id, buffer);
        self.active_buffer_id = Some(buffer_id);

        // Update audio to point to this new buffer
        self.update_audio_buffer();

        Ok(buffer_id)
    }

    /// Switch to a specific buffer by ID
    pub fn switch_to(&mut self, buffer_id: BufferId) -> Result<(), String> {
        if !self.buffers.contains_key(&buffer_id) {
            return Err(format!("Buffer {} does not exist", buffer_id));
        }

        if self.active_buffer_id == Some(buffer_id) {
            return Ok(()); // Already active
        }

        // Pause current buffer
        if let Some(current_id) = self.active_buffer_id {
            if let Some(buffer) = self.buffers.get_mut(&current_id) {
                buffer.was_playing = buffer.player.state().is_playing();
                buffer.player.pause();
            }
        }

        // Activate new buffer
        self.active_buffer_id = Some(buffer_id);

        // Update audio to point to the new active buffer
        self.update_audio_buffer();

        // Resume if it was playing before
        if let Some(buffer) = self.buffers.get_mut(&buffer_id) {
            if buffer.was_playing {
                buffer.player.play();
            }
            tracing::info!("Switched to buffer {}: {}", buffer_id, buffer.file_name());
        }

        Ok(())
    }

    /// Switch to next buffer
    pub fn next_buffer(&mut self) -> Result<(), String> {
        if self.buffers.is_empty() {
            return Err("No buffers open".to_string());
        }

        let current_id = self.active_buffer_id.unwrap_or(0);
        let mut buffer_ids: Vec<_> = self.buffers.keys().copied().collect();
        buffer_ids.sort();

        let current_index = buffer_ids.iter().position(|&id| id == current_id).unwrap_or(0);
        let next_index = (current_index + 1) % buffer_ids.len();
        let next_id = buffer_ids[next_index];

        self.switch_to(next_id)
    }

    /// Switch to previous buffer
    pub fn prev_buffer(&mut self) -> Result<(), String> {
        if self.buffers.is_empty() {
            return Err("No buffers open".to_string());
        }

        let current_id = self.active_buffer_id.unwrap_or(0);
        let mut buffer_ids: Vec<_> = self.buffers.keys().copied().collect();
        buffer_ids.sort();

        let current_index = buffer_ids.iter().position(|&id| id == current_id).unwrap_or(0);
        let prev_index = if current_index == 0 {
            buffer_ids.len() - 1
        } else {
            current_index - 1
        };
        let prev_id = buffer_ids[prev_index];

        self.switch_to(prev_id)
    }

    /// Delete a buffer by ID
    pub fn delete(&mut self, buffer_id: BufferId) -> Result<(), String> {
        if !self.buffers.contains_key(&buffer_id) {
            return Err(format!("Buffer {} does not exist", buffer_id));
        }

        // If deleting the active buffer, switch to another one first
        if self.active_buffer_id == Some(buffer_id) {
            // Find another buffer to switch to
            let other_buffer = self.buffers.keys()
                .find(|&&id| id != buffer_id)
                .copied();

            if let Some(other_id) = other_buffer {
                self.switch_to(other_id)?;
            } else {
                // This was the last buffer
                self.active_buffer_id = None;
            }
        }

        self.buffers.remove(&buffer_id);
        tracing::info!("Deleted buffer {}", buffer_id);

        // Update audio (might clear if no buffers left)
        self.update_audio_buffer();

        Ok(())
    }

    /// Get list of all buffers with their info
    pub fn list_buffers(&self) -> Vec<(BufferId, String, String, bool)> {
        let mut buffers: Vec<_> = self.buffers.values()
            .map(|b| {
                let state = match b.player.state() {
                    PlaybackState::Playing => "playing",
                    PlaybackState::Paused => "paused",
                    PlaybackState::Stopped => "stopped",
                };
                let is_active = self.active_buffer_id == Some(b.id);
                (b.id, b.file_name(), state.to_string(), is_active)
            })
            .collect();

        buffers.sort_by_key(|(id, _, _, _)| *id);
        buffers
    }

    /// Get the active buffer (mutable)
    pub fn active_buffer_mut(&mut self) -> Option<&mut VideoBuffer> {
        self.active_buffer_id
            .and_then(|id| self.buffers.get_mut(&id))
    }

    /// Get the active buffer (immutable)
    pub fn active_buffer(&self) -> Option<&VideoBuffer> {
        self.active_buffer_id
            .and_then(|id| self.buffers.get(&id))
    }

    /// Check if any buffer is open
    pub fn has_buffers(&self) -> bool {
        !self.buffers.is_empty()
    }

    /// Get the active buffer ID
    pub fn active_buffer_id(&self) -> Option<BufferId> {
        self.active_buffer_id
    }
}
