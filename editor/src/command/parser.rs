use std::path::PathBuf;

/// Commands that can be executed in the editor
#[derive(Debug, Clone)]
pub enum Command {
    /// Open a video file (:open <path>)
    Open(PathBuf),
    /// Open file dialog (:open with no args)
    OpenDialog,
    /// Start playback (:play)
    Play,
    /// Pause playback (:pause)
    Pause,
    /// Seek to a position (:seek <target>)
    Seek(SeekTarget),
    /// Switch to buffer by ID (:buffer <id> or :b <id>)
    Buffer(usize),
    /// Next buffer (:bnext or :bn)
    BufferNext,
    /// Previous buffer (:bprev or :bp)
    BufferPrev,
    /// List all buffers (:buffers or :ls)
    BufferList,
    /// Delete buffer by ID (:bdelete <id> or :bd <id>)
    BufferDelete(usize),
    /// Quit the editor (:quit or :q)
    Quit,
    /// No operation (empty command)
    NoOp,
}

/// Seek target specification
#[derive(Debug, Clone)]
pub enum SeekTarget {
    /// Absolute time in seconds
    Absolute(f64),
    /// Relative offset in seconds (positive or negative)
    Relative(f64),
    /// Percentage of total duration (0-100)
    Percentage(u8),
}

/// Parse a vim-style command string
///
/// Examples:
/// - `:open /path/to/video.mp4`
/// - `:play`
/// - `:pause`
/// - `:seek 10.5` (absolute)
/// - `:seek +5` (relative forward)
/// - `:seek -5` (relative backward)
/// - `:seek 50%` (percentage)
/// - `:quit` or `:q`
pub fn parse_command(input: &str) -> Result<Command, String> {
    let input = input.trim();

    // Empty input
    if input.is_empty() {
        return Ok(Command::NoOp);
    }

    // Must start with ':'
    if !input.starts_with(':') {
        return Err("Commands must start with ':'".to_string());
    }

    let input = &input[1..]; // Remove leading ':'
    let parts: Vec<&str> = input.split_whitespace().collect();

    if parts.is_empty() {
        return Ok(Command::NoOp);
    }

    match parts[0] {
        "open" | "o" => {
            if parts.len() < 2 {
                // No path provided - open file dialog
                Ok(Command::OpenDialog)
            } else {
                let path = parts[1..].join(" "); // Handle paths with spaces
                Ok(Command::Open(PathBuf::from(path)))
            }
        }

        "play" => Ok(Command::Play),

        "pause" => Ok(Command::Pause),

        "seek" | "s" => {
            if parts.len() < 2 {
                return Err("Usage: :seek <target> (e.g., :seek 10.5, :seek +5, :seek 50%)".to_string());
            }

            let target_str = parts[1];

            // Check for percentage
            if target_str.ends_with('%') {
                let percent = target_str[..target_str.len() - 1]
                    .parse::<u8>()
                    .map_err(|_| "Invalid percentage")?;

                if percent > 100 {
                    return Err("Percentage must be 0-100".to_string());
                }

                return Ok(Command::Seek(SeekTarget::Percentage(percent)));
            }

            // Check for relative
            if target_str.starts_with('+') || target_str.starts_with('-') {
                let offset = target_str
                    .parse::<f64>()
                    .map_err(|_| "Invalid seek offset")?;

                return Ok(Command::Seek(SeekTarget::Relative(offset)));
            }

            // Absolute
            let time = target_str
                .parse::<f64>()
                .map_err(|_| "Invalid seek time")?;

            if time < 0.0 {
                return Err("Seek time cannot be negative".to_string());
            }

            Ok(Command::Seek(SeekTarget::Absolute(time)))
        }

        "buffer" | "b" => {
            if parts.len() < 2 {
                return Err("Usage: :buffer <id> (e.g., :buffer 1)".to_string());
            }

            let buffer_id = parts[1]
                .parse::<usize>()
                .map_err(|_| "Invalid buffer ID")?;

            Ok(Command::Buffer(buffer_id))
        }

        "bnext" | "bn" => Ok(Command::BufferNext),

        "bprev" | "bp" => Ok(Command::BufferPrev),

        "buffers" | "ls" => Ok(Command::BufferList),

        "bdelete" | "bd" => {
            if parts.len() < 2 {
                return Err("Usage: :bdelete <id> (e.g., :bdelete 1)".to_string());
            }

            let buffer_id = parts[1]
                .parse::<usize>()
                .map_err(|_| "Invalid buffer ID")?;

            Ok(Command::BufferDelete(buffer_id))
        }

        "quit" | "q" => Ok(Command::Quit),

        unknown => Err(format!("Unknown command: {}", unknown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_open() {
        // Open with path
        let cmd = parse_command(":open /path/to/video.mp4").unwrap();
        match cmd {
            Command::Open(path) => assert_eq!(path, PathBuf::from("/path/to/video.mp4")),
            _ => panic!("Expected Open command"),
        }

        // Open without path (should trigger file dialog)
        let cmd = parse_command(":open").unwrap();
        assert!(matches!(cmd, Command::OpenDialog));
    }

    #[test]
    fn test_parse_play_pause() {
        assert!(matches!(parse_command(":play").unwrap(), Command::Play));
        assert!(matches!(parse_command(":pause").unwrap(), Command::Pause));
    }

    #[test]
    fn test_parse_seek() {
        // Absolute
        match parse_command(":seek 10.5").unwrap() {
            Command::Seek(SeekTarget::Absolute(t)) => assert_eq!(t, 10.5),
            _ => panic!("Expected Seek Absolute"),
        }

        // Relative forward
        match parse_command(":seek +5").unwrap() {
            Command::Seek(SeekTarget::Relative(t)) => assert_eq!(t, 5.0),
            _ => panic!("Expected Seek Relative"),
        }

        // Relative backward
        match parse_command(":seek -3").unwrap() {
            Command::Seek(SeekTarget::Relative(t)) => assert_eq!(t, -3.0),
            _ => panic!("Expected Seek Relative"),
        }

        // Percentage
        match parse_command(":seek 50%").unwrap() {
            Command::Seek(SeekTarget::Percentage(p)) => assert_eq!(p, 50),
            _ => panic!("Expected Seek Percentage"),
        }
    }

    #[test]
    fn test_parse_quit() {
        assert!(matches!(parse_command(":quit").unwrap(), Command::Quit));
        assert!(matches!(parse_command(":q").unwrap(), Command::Quit));
    }

    #[test]
    fn test_parse_errors() {
        assert!(parse_command("invalid").is_err());
        assert!(parse_command(":seek").is_err()); // seek requires an argument
        assert!(parse_command(":unknown").is_err()); // unknown command
    }
}
