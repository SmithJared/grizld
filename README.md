# Grizld - Vim-Based Video Editor

A video editing platform inspired by Vim, featuring keyboard-driven controls and hardware-accelerated playback.

## Features

- **Vim-style command interface** - Familiar `:` command mode
- **Hardware-accelerated video decoding** - Uses VideoToolbox on macOS
- **Keyboard-first workflow** - Navigate and control without touching the mouse
- **Audio-video synchronization** - Smooth playback with minimal drift
- **Native file picker** - Easy file selection with `:open`

## Installation

### Prerequisites

1. Install FFmpeg (required for video/audio decoding):
   ```bash
   brew install ffmpeg pkg-config
   ```

2. Build the project:
   ```bash
   cargo build --release
   ```

## Usage

### Running Grizld

```bash
cargo run --release --bin grizld
```

### Vim Commands

Enter command mode by pressing `:` (colon)

| Command | Description |
|---------|-------------|
| `:open` or `:o` | Opens native file picker to select a video |
| `:open <path>` | Open a specific video file path |
| `:play` | Start playback |
| `:pause` | Pause playback |
| `:seek <target>` | Seek to position (see examples below) |
| `:quit` or `:q` | Exit the editor |

#### Seek Examples

- `:seek 10.5` - Jump to 10.5 seconds (absolute)
- `:seek +5` - Jump forward 5 seconds (relative)
- `:seek -3` - Jump backward 3 seconds (relative)
- `:seek 50%` - Jump to 50% of video duration (percentage)

### Keyboard Shortcuts (Normal Mode)

| Key | Action |
|-----|--------|
| `:` | Enter command mode |
| `Space` | Toggle play/pause |
| `h` | Seek 5 seconds backward |
| `l` | Seek 5 seconds forward |
| `j` | Seek 1 second backward |
| `k` | Seek 1 second forward |

### Command Mode

| Key | Action |
|-----|--------|
| `Enter` | Execute command |
| `Escape` | Exit command mode |

## Architecture

The project is organized as a Cargo workspace:

### `vp-core/` - Video Playback Library
- Hardware-accelerated video/audio decoding via `ffmpeg-next`
- Frame buffer with PTS-ordered insertion (handles B-frames)
- Audio ring buffer with 2-second capacity
- Audio-driven playback clock for A/V sync
- Multi-threaded decode loop

### `editor/` - GUI Application
- Built with `egui` and `eframe`
- Vim-style command parser and executor
- Video renderer with aspect-ratio preservation
- CPAL audio output integration
- Native file picker via `rfd`

## Supported Video Formats

- MP4 (H.264, H.265/HEVC)
- MKV
- AVI
- MOV
- WebM
- FLV
- WMV
- M4V

## Development

### Running Tests

```bash
cargo test --workspace
```

### Building for Release

```bash
cargo build --release --bin grizld
```

The binary will be at `target/release/grizld`

## Roadmap

This is a simplified MVP. Future enhancements from the full architecture vision:

- [ ] Lock-free atomic AudioClock (currently uses Mutex)
- [ ] Dual-buffer system with history for backward scrubbing
- [ ] Multi-buffer Vim-style management (load multiple videos, switch instantly)
- [ ] Min-heap frame buffer for better B-frame handling
- [ ] Direct CVPixelBuffer → Metal rendering (zero-copy)
- [ ] Timeline editing features
- [ ] Video filters and effects

## License

MIT
