use std::path::PathBuf;
use vp_core::VideoPlayer;

use super::{Command, SeekTarget};

/// Executes commands on the video player
pub struct CommandExecutor {
    player: Option<VideoPlayer>,
}

impl CommandExecutor {
    pub fn new() -> Self {
        Self { player: None }
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

        match VideoPlayer::new(&path) {
            Ok(player) => {
                let duration = player.duration();
                self.player = Some(player);
                Ok(format!(
                    "Opened: {} (duration: {:.1}s)",
                    path.display(),
                    duration
                ))
            }
            Err(e) => Err(format!("Failed to open: {}", e)),
        }
    }

    fn play(&mut self) -> Result<String, String> {
        match &mut self.player {
            Some(player) => {
                player.play();
                Ok("Playing".to_string())
            }
            None => Err("No file loaded. Use :open <file> first".to_string()),
        }
    }

    fn pause(&mut self) -> Result<String, String> {
        match &mut self.player {
            Some(player) => {
                player.pause();
                Ok("Paused".to_string())
            }
            None => Err("No file loaded".to_string()),
        }
    }

    fn seek(&mut self, target: SeekTarget) -> Result<String, String> {
        match &mut self.player {
            Some(player) => {
                let target_pts = match target {
                    SeekTarget::Absolute(t) => t,
                    SeekTarget::Relative(offset) => {
                        let current = player.current_time();
                        (current + offset).max(0.0).min(player.duration())
                    }
                    SeekTarget::Percentage(percent) => {
                        (percent as f64 / 100.0) * player.duration()
                    }
                };

                player
                    .seek(target_pts)
                    .map_err(|e| format!("Seek failed: {}", e))?;

                Ok(format!("Seeked to {:.1}s", target_pts))
            }
            None => Err("No file loaded".to_string()),
        }
    }

    /// Get a reference to the current player
    pub fn player(&self) -> Option<&VideoPlayer> {
        self.player.as_ref()
    }

    /// Get a mutable reference to the current player
    pub fn player_mut(&mut self) -> Option<&mut VideoPlayer> {
        self.player.as_mut()
    }

    /// Check if a file is loaded
    pub fn has_player(&self) -> bool {
        self.player.is_some()
    }
}

impl Default for CommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}
