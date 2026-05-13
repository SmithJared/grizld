# VP Core - Video Player Core Library

A simplified, testable video player core focused on demuxing, decoding, buffering, and playback. Supports both software (FFmpeg) and hardware (VideoToolbox on macOS) decoding.

## Status

🚧 **Under Development** - Following the implementation plan in `../docs/vp_core_plan.md`

### Implementation Progress

- [x] **Phase 1: Foundation** (In Progress)
  - [x] Project structure created
  - [x] Cargo workspace configured
  - [x] Error types implemented
  - [x] Clock trait and implementations
  - [x] Unit tests for Clock
- [ ] **Phase 2: Demuxer** (Not Started)
- [ ] **Phase 3: Software Decoder** (Not Started)
- [ ] **Phase 3.5: Hardware Decoder** (Not Started)
- [ ] **Phase 4: Buffer** (Not Started)
- [ ] **Phase 5: Playback Controller** (Not Started)
- [ ] **Phase 6: CLI Interface** (Not Started)
- [ ] **Phase 7: Testing & Documentation** (Not Started)

## Architecture

```
vp_core/
├── clock       - Unified timing mechanism
├── error       - Custom error types
├── demux       - FFmpeg-based demuxer
├── decode      - Video decoder trait with SW/HW implementations
├── buffer      - Ring buffer for decoded frames
├── playback    - Playback controller and decode thread
└── cli         - Command-line player for testing
```

## Key Features

- **Unified Clock**: Single Clock trait used by all components (no dual-clock issue)
- **Hardware Acceleration**: First-class support for VideoToolbox on macOS
- **Trait-Based Design**: Easy to test and extend
- **CLI-First**: Testable without GUI complexity
- **Proper Error Handling**: Custom error types with thiserror

## Building

```bash
# From workspace root
cargo build -p vp_core

# Run tests
cargo test -p vp_core

# Run with all features
cargo build -p vp_core --all-features
```

## Usage (Planned)

```rust
use vp_core::{PlaybackController, BufferConfig, DecoderPreference};

let config = BufferConfig::default();
let mut controller = PlaybackController::new(
    "video.mp4",
    config,
    DecoderPreference::Auto, // Try hardware first, fallback to software
)?;

controller.play()?;

while controller.is_playing() {
    if let Some(frame) = controller.get_current_frame() {
        println!("Frame: PTS={:.3}s, {}x{}, HW={}", 
                 frame.pts(), frame.width(), frame.height(), frame.is_hardware());
    }
}
```

## License

MIT OR Apache-2.0
