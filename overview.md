 # Architecture Highlights

  1. Vim-Style Buffer Management (Recent Evolution)

  Your latest commits show a significant architectural shift:
  - Each video file gets its own complete playback infrastructure (demuxer, video decoder, audio decoder, buffer
  - Multiple files can be loaded simultaneously; only the active one plays
  - Instant switching between files without reload latency
  - Files act like Vim buffers - load multiple, switch instantly

  3. Key Design Patterns

  Command Pattern (UI → App):
  User Input → EditorUI → AppCommand → App → VideoBufferManager

  Audio-Driven A/V Sync:
  - AudioClock (atomic, lock-free) is the master clock
  - CPAL audio callback updates clock atomically
  - Video frames wait for audio time, never vice versa
  - Prevents audio stuttering (primary user complaint)

  Dual-Buffer Architecture:
  - FrameBuffer.ahead: Min-heap (120 frames) handles B-frame out-of-order decoding
  - FrameBuffer.behind: History buffer (20 frames) for backward scrubbing
  - AudioBuffer: 2-second ring buffer with atomic PTS extraction

  4. Key Technical Decisions
  Decision: Lock-free AudioClock
  Rationale: CPAL thread can't block; atomic
    updates prevent jitter
  ────────────────────────────────────────
  Decision: Min-heap frame buffer
  Rationale: Handles out-of-order B-frames
    transparently
  ────────────────────────────────────────
  Decision: Per-file threads vs thread pool
  Rationale: Isolation + simplicity + clean
    pause/resume
  ────────────────────────────────────────
  Decision: Hardware acceleration
  Rationale: VideoToolbox (macOS) for
    H.264/HEVC → CVPixelBuffer
  ────────────────────────────────────────
  Decision: Bounded channels (128)
  Rationale: Natural backpressure; prevents
    runaway memory
  5. Core Components

  Workspace Structure:
  - vp-core/: Battle-tested playback library (reusable)
  - editor/: UI application + coordination

  Key Dependencies:
  - ffmpeg-next 8.0 (video/audio decoding)
  - cpal  (audio output)
  - egui/eframe  (UI)
  - wgpu/Metal (GPU rendering)
  - crossbeam-channel (thread communication)
  - thiserror
  - tracing 
  - tracing-subscriber

  6. Data Flow

  User Input → Command → VideoBufferManager
                             ↓
                      Active VideoFile
                      /      |        \
              Demuxer   VideoDecoder  AudioDecoder
                  \        |          /
                 FrameBuffer    AudioBuffer
                      |              |
                  Timeline      CPAL Thread
                      ↓              ↓
                Render Frame   Update AudioClock

  7. Playback Flow

  Load File:
  1. Open FFmpeg container
  2. Create buffers (120 frames video, 2 sec audio)
  4. Set as active buffer

  Play:
  1. Resume AudioClock
  3. Decoders decode → push to buffers
  4. Main loop renders frames when PTS <= AudioClock

  Seek:
  1. Pause AudioClock
  2. FFmpeg seek
  3. Flush decoders
  4. Clear buffers
  5. Set AudioClock to target
  6. Resume if was playing

  8. Notable Features

  ✅ Hardware acceleration (VideoToolbox on macOS)
  ✅ Lock-free synchronization (atomic AudioClock)
  ✅ Handles B-frames (min-heap auto-sorts by PTS)
  ✅ Backward scrubbing (frame history buffer)
  ✅ Multi-file editing (Vim-style buffers)
  ✅ Clean error propagation (unified VpError hierarchy)
  ✅ Zero-copy where possible (CVPixelBuffer direct to Metal)

  9. Scalability

  Scales Well:
  - Multiple files loaded simultaneously
  - High resolution (hardware decode + Metal)
  - Rapid scrubbing (frame history)

  Current Limitations:
  - Single audio output (one active file at a time)
  - macOS-primary (Metal rendering)
  - Local files only (no streaming)

  ---
  This architecture represents a sophisticated, production-grade video playback system with a clear evolution toward professional multi-file editing workflows. The Vim-style buffer management is particularly innovative for video editing applications.
