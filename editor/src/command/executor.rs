use std::path::PathBuf;
use vp_core::VideoPlayer;

use crate::buffer_manager::{VideoBufferManager, BufferId};
use super::{Command, SeekTarget};

/// Executes commands on the video buffer manager
pub struct CommandExecutor {
    buffer_manager: VideoBufferManager,
}

impl CommandExecutor {
    pub fn new() -> Self {
        Self {
            buffer_manager: VideoBufferManager::new(),
        }
    }

    /// Execute a command
    pub fn execute(&mut self, cmd: Command) -> Result<String, String> {
        match cmd {
            Command::Open(path) => self.open(path),
            Command::OpenDialog => {
                // OpenDialog is handled by the app, not here
                Ok("Opening file dialog...".to_string())
            }
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::Seek(target) => self.seek(target),
            Command::Buffer(id) => self.switch_buffer(id),
            Command::BufferNext => self.next_buffer(),
            Command::BufferPrev => self.prev_buffer(),
            Command::BufferList => self.list_buffers(),
            Command::BufferDelete(id) => self.delete_buffer(id),
            Command::Quit => {
                // Quit is handled by the app, not here
                Ok("Quit command received".to_string())
            }
            Command::NoOp => Ok(String::new()),
        }
    }

    fn open(&mut self, path: PathBuf) -> Result<String, String> {
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        let buffer_id = self.buffer_manager.open(path.clone())?;
        let buffer = self.buffer_manager.active_buffer()
            .ok_or("Failed to get buffer")?;

        Ok(format!(
            "Opened buffer {}: {} (duration: {:.1}s)",
            buffer_id,
            path.display(),
            buffer.player.duration()
        ))
    }

    fn play(&mut self) -> Result<String, String> {
        if let Some(buffer) = self.buffer_manager.active_buffer_mut() {
            buffer.player.play();
            Ok(format!("Playing buffer {}", buffer.id))
        } else {
            Err("No buffer open. Use :open <file> first".to_string())
        }
    }

    fn pause(&mut self) -> Result<String, String> {
        if let Some(buffer) = self.buffer_manager.active_buffer_mut() {
            buffer.player.pause();
            Ok(format!("Paused buffer {}", buffer.id))
        } else {
            Err("No buffer open".to_string())
        }
    }

    fn seek(&mut self, target: SeekTarget) -> Result<String, String> {
        if let Some(buffer) = self.buffer_manager.active_buffer_mut() {
            let target_pts = match target {
                SeekTarget::Absolute(t) => t,
                SeekTarget::Relative(offset) => {
                    let current = buffer.player.current_time();
                    (current + offset).max(0.0).min(buffer.player.duration())
                }
                SeekTarget::Percentage(percent) => {
                    (percent as f64 / 100.0) * buffer.player.duration()
                }
            };

            buffer.player
                .seek(target_pts)
                .map_err(|e| format!("Seek failed: {}", e))?;

            Ok(format!("Seeked to {:.1}s", target_pts))
        } else {
            Err("No buffer open".to_string())
        }
    }

    fn switch_buffer(&mut self, buffer_id: BufferId) -> Result<String, String> {
        self.buffer_manager.switch_to(buffer_id)?;
        let buffer = self.buffer_manager.active_buffer()
            .ok_or("Failed to get buffer")?;
        Ok(format!("Switched to buffer {}: {}", buffer_id, buffer.file_name()))
    }

    fn next_buffer(&mut self) -> Result<String, String> {
        self.buffer_manager.next_buffer()?;
        if let Some(buffer) = self.buffer_manager.active_buffer() {
            Ok(format!("Buffer {}: {}", buffer.id, buffer.file_name()))
        } else {
            Ok("No buffers".to_string())
        }
    }

    fn prev_buffer(&mut self) -> Result<String, String> {
        self.buffer_manager.prev_buffer()?;
        if let Some(buffer) = self.buffer_manager.active_buffer() {
            Ok(format!("Buffer {}: {}", buffer.id, buffer.file_name()))
        } else {
            Ok("No buffers".to_string())
        }
    }

    fn list_buffers(&self) -> Result<String, String> {
        let buffers = self.buffer_manager.list_buffers();
        if buffers.is_empty() {
            return Ok("No buffers open".to_string());
        }

        let mut output = String::from("Buffers:\n");
        for (id, name, state, is_active) in buffers {
            let marker = if is_active { " *" } else { "  " };
            output.push_str(&format!("{}{}  {}  [{}]\n", marker, id, name, state));
        }

        Ok(output)
    }

    fn delete_buffer(&mut self, buffer_id: BufferId) -> Result<String, String> {
        self.buffer_manager.delete(buffer_id)?;
        Ok(format!("Deleted buffer {}", buffer_id))
    }

    /// Get a reference to the active player
    pub fn player(&self) -> Option<&VideoPlayer> {
        self.buffer_manager.active_buffer().map(|b| &b.player)
    }

    /// Get a mutable reference to the active player
    pub fn player_mut(&mut self) -> Option<&mut VideoPlayer> {
        self.buffer_manager.active_buffer_mut().map(|b| &mut b.player)
    }

    /// Check if any buffer is open
    pub fn has_player(&self) -> bool {
        self.buffer_manager.has_buffers()
    }

    /// Get the buffer manager
    pub fn buffer_manager(&self) -> &VideoBufferManager {
        &self.buffer_manager
    }

    /// Get the buffer manager mutably
    pub fn buffer_manager_mut(&mut self) -> &mut VideoBufferManager {
        &mut self.buffer_manager
    }
}

impl Default for CommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}
