 Overview

    Build a full MVP with vim-style
    commands, video playback, and basic
     UI. Simplified architecture with
    single-buffer management, basic A/V
     sync, and macOS hardware
    acceleration support.

    Architecture Simplifications for
    MVP

    1. A/V Sync: Basic audio-driven
    sync with Arc<Mutex<PlaybackClock>>
     (not lock-free atomic yet)
    2. Frame Buffer: Single
    VecDeque<VideoFrame> with 60 frames
     (not dual-buffer with history)
    3. Buffer Management: One active
    video file at a time (not
    multi-buffer vim-style yet)
    4. Threading: Simplified model with
     Demuxer → Decoder → Main thread
    5. Hardware Acceleration: Use
    ffmpeg-next's automatic detection
    (VideoToolbox on macOS)

    Implementation Phases

    Phase 0: Project Foundation

    Goal: Set up workspace structure
    and dependencies

    Actions:
    1. Transform single crate into
    Cargo workspace:
      - Root Cargo.toml with
    [workspace] members
      - vp-core/ - Reusable video
    playback library (no UI)
      - editor/ - GUI application built
     on egui/eframe
    2. Configure dependencies in
    workspace:
    [workspace.dependencies]
    ffmpeg-next = "8.0"
    cpal = "0.15"
    egui = "0.30"
    eframe = "0.30"
    crossbeam-channel = "0.5"
    thiserror = "1.0"
    tracing = "0.1"
    tracing-subscriber = "0.3"
    3. Fix edition in Cargo.toml:
    edition = "2021" (2024 doesn't
    exist)

    Files to create/modify:
    - /Users/jaredsmith/Projects/grizld
    /Cargo.toml - Workspace manifest
    - /Users/jaredsmith/Projects/grizld
    /vp-core/Cargo.toml - Playback
    library manifest
    - /Users/jaredsmith/Projects/grizld
    /editor/Cargo.toml - GUI app
    manifest

    ---
    Phase 1: Core Playback
    Infrastructure (vp-core)

    Goal: Build reusable video playback
     library with hardware acceleration

    vp-core Structure:
    vp-core/src/
    ├── lib.rs              # Public
    API
    ├── error.rs            # VpError
    enum with thiserror
    ├── types.rs            #
    VideoFrame, AudioSample, PTS,
    PlaybackState
    ├── decoder/
    │   ├── mod.rs
    │   ├── video.rs        #
    VideoDecoder (ffmpeg-next wrapper,
    hw accel)
    │   └── audio.rs        #
    AudioDecoder (ffmpeg-next wrapper)
    ├── buffer/
    │   ├── mod.rs
    │   ├── frame_buffer.rs # VecDeque
    with PTS-ordered insertion (handles
     B-frames)
    │   └── audio_buffer.rs # Ring
    buffer for audio samples
    ├── sync.rs             #
    PlaybackClock (audio-driven time
    source)
    └── player.rs           #
    VideoPlayer (orchestrates
    everything)

    Key Components:

    1. Error Handling (error.rs):
      - VpError enum: Ffmpeg, Io,
    NoVideoStream, NoAudioStream,
    Decoder, ChannelSend/Receive
      - Use thiserror for clean error
    messages
    2. Core Types (types.rs):
      - PTS = f64 (presentation
    timestamp in seconds)
      - VideoFrame { pts, data:
    Vec<u8>, width, height, format }
      - AudioSample { pts, data:
    Vec<f32>, sample_rate }
      - PlaybackState enum: Stopped,
    Playing, Paused
    3. PlaybackClock (sync.rs):
      - Arc<Mutex<ClockState>> with
    current_pts, state, audio_base_pts,
     audio_start_time
      - Methods: current_time(),
    update_from_audio(pts),
    set_state(), seek()
      - Audio-driven: CPAL callback
    updates clock, video frames render
    when PTS <= clock
    4. FrameBuffer
    (buffer/frame_buffer.rs):
      - VecDeque<VideoFrame> with
    capacity 60
      - push(): Insert in PTS order
    (handles out-of-order B-frames)
      - get_frame_at(pts): Binary
    search for closest frame <= pts
    5. VideoPlayer (player.rs):
      - Owns FFmpeg input context,
    stream indices, decoders, buffers,
    clock
      - Manages decode thread with
    channels (crossbeam-channel,
    capacity 64)
      - Public API: new(file_path),
    play(), pause(), seek(target),
    get_current_frame(),
    current_time(), duration()

    Threading Model:
    Main Thread          Decode Thread
           CPAL Audio Thread
      UI              → Demuxer Loop
     ←   Audio Callback
      ↓                   ↓
              ↓
    Render Frame      Decode Packet
        Pop AudioBuffer
      ↓                   ↓
              ↓
    Get from          Push to Buffers
      Update PlaybackClock
    FrameBuffer

    Files to create:
    - All files in vp-core/src/
    structure above
    - Focus on /Users/jaredsmith/Projec
    ts/grizld/vp-core/src/player.rs
    (orchestrator)
    - Focus on /Users/jaredsmith/Projec
    ts/grizld/vp-core/src/sync.rs (A/V
    sync)

    ---
    Phase 2: GUI & Command Interface
    (editor)

    Goal: Build egui UI with vim-style
    command input

    editor Structure:
    editor/src/
    ├── main.rs           # eframe
    setup, main loop
    ├── app.rs            # EditorApp
    (owns VideoPlayer, UI state)
    ├── ui/
    │   ├── mod.rs
    │   ├── viewport.rs   # Video
    rendering viewport
    │   └── command.rs    # Command
    input widget
    ├── command/
    │   ├── mod.rs
    │   ├── parser.rs     # Parse
    ":open file.mp4" → Command enum
    │   └── executor.rs   # Execute
    commands on App
    └── renderer.rs       # Convert
    VideoFrame → egui texture

    Command System:

    1. Command Enum (command/mod.rs):
    enum Command {
        Open(PathBuf),
        Play,
        Pause,
        Seek(SeekTarget),  // Absolute,
     Relative, Percentage
        Quit,
    }
    2. Parser (command/parser.rs):
      - Parse :open /path/to/file.mp4,
    :play, :pause, :seek 10.5, :q
      - Return Result<Command, String>
    3. Executor (command/executor.rs):
      - CommandExecutor owns
    Option<VideoPlayer>
      - execute(cmd) -> Result<(),
    String>
      - Handle errors gracefully (e.g.,
     "No file loaded")

    UI Layout:
    ┌──────────────────────────────────
    ──────┐
    │  Video Viewport (centered, 16:9)
         │
    │  ┌───────────────────────────────
    ───┐ │
    │  │      [Video Frame Rendered]
       │ │
    │  └───────────────────────────────
    ───┘ │
    │  Playback: ⏸ 00:12.4 / 01:23.5
    50%  │
    │  Command:  : _
         │
    └──────────────────────────────────
    ──────┘

    EditorApp (app.rs):
    - Implements eframe::App
    - State: executor, command_input,
    command_mode, video_texture,
    last_error
    - update(): Handle keyboard, render
     viewport, update texture from
    VideoPlayer
    - Call ctx.request_repaint() for
    continuous video updates

    Files to create:
    - All files in editor/src/
    structure above
    - Focus on /Users/jaredsmith/Projec
    ts/grizld/editor/src/app.rs (UI
    integration)
    - Focus on
    /Users/jaredsmith/Projects/grizld/e
    ditor/src/command/parser.rs (vim
    commands)

    ---
    Phase 3: Keyboard Controls & Audio
    Integration

    Goal: Wire up keyboard shortcuts
    and CPAL audio output

    Keyboard Mapping:
    - Normal mode:
      - : - Enter command mode
      - Space - Toggle play/pause
      - h - Seek 5s backward
      - l - Seek 5s forward
      - j - Seek 1s backward
      - k - Seek 1s forward
    - Command mode:
      - Enter - Execute command
      - Escape - Exit command mode

    Audio Output Setup:
    - Initialize CPAL stream in
    EditorApp::new()
    - Audio callback:
      a. Pop samples from AudioBuffer
      b. Update PlaybackClock with
    current PTS
      c. Fill output buffer with audio
    data
      d. Handle underruns (fill with
    silence)

    Integration Points:
    - Keyboard input in
    EditorApp::update()
    - CPAL stream stored in EditorApp
    (must live as long as app)
    - Share Arc<AudioBuffer> and
    Arc<PlaybackClock> between main
    thread and CPAL

    ---
    Phase 4: Video Rendering Pipeline

    Goal: Display decoded video frames
    in egui

    VideoRenderer (renderer.rs):
    1. Convert VideoFrame →
    egui::ColorImage
    2. Upload to GPU as TextureHandle
    3. Reuse texture handle for
    efficiency (call tex.set() instead
    of creating new)
    4. Render with aspect ratio
    preservation

    Rendering in EditorApp:
    fn update(&mut self, ctx:
    &egui::Context, _frame: &mut
    eframe::Frame) {
        // Get current frame from
    VideoPlayer
        if let Some(frame) = self.execu
    tor.player.get_current_frame() {
            let texture =
    self.renderer.update_texture(ctx,
    frame);

            egui::CentralPanel::default
    ().show(ctx, |ui| {

    self.renderer.render(ui); //
    Display video

    self.render_controls(ui);  //
    Playback controls

    self.render_command_input(ui); //
    Command line
            });
        }
    }

    ---
    Phase 5: Testing & Polish

    Goal: Verify functionality and
    handle edge cases

    Testing Strategy:

    1. Unit Tests (in vp-core):
      - decoder/video.rs: Hardware
    acceleration detection
      - buffer/frame_buffer.rs:
    Out-of-order B-frame insertion
      - sync.rs: Clock state
    transitions
      - command/parser.rs: Command
    parsing edge cases
    2. Integration Tests (in editor):
      - Open and play video
      - Seek forward/backward
      - Play/pause/resume
      - Error handling (missing file,
    corrupted video)
    3. Manual Testing:
      - Load H.264, HEVC, VP9 videos
      - Verify A/V sync (audio-heavy
    content)
      - Check GPU usage in Activity
    Monitor (hardware acceleration)
      - Test with 4K video
      - Test corrupted files (graceful
    error messages)
    4. Test Assets:
      - Create test_assets/ with sample
     videos
      - Short H.264 clip (5-10 seconds)
      - Video with B-frames
      - 4K sample for hardware test

    ---
    Critical Technical Details

    FFmpeg Setup on macOS

    brew install ffmpeg pkg-config
    export PKG_CONFIG_PATH="/opt/homebr
    ew/lib/pkgconfig"

    Hardware Acceleration
    (VideoToolbox)

    - ffmpeg-next attempts hardware
    decode automatically
    - Verify with: Activity Monitor →
    GPU usage during playback
    - Fallback to software decode if
    hardware unavailable

    B-Frame Handling

    - Insert frames into VecDeque in
    PTS order (not decode order)
    - Handle out-of-order frames
    transparently

    Preventing Audio Starvation

    - Audio callback checks buffer fill
     level
    - If underrun: fill with silence,
    log warning
    - Buffer size: 2 seconds of audio
    (conservative)

    ---
    Migration Path to Full Architecture

    After MVP is working, incrementally
     add:

    1. Lock-free AudioClock: Replace
    Mutex with AtomicU64
    2. Dual-buffer: Add history buffer
    (20 frames behind) for backward
    scrubbing
    3. Multi-buffer: Wrap VideoPlayer
    in VideoBufferManager, vim-style
    buffer switching
    4. Advanced rendering: Direct
    CVPixelBuffer → Metal (zero-copy)
    5. Min-heap frame buffer: Replace
    VecDeque with BinaryHeap (ahead
    buffer)

    ---
    Success Criteria

    MVP is complete when:

    1. ✅ Can load and play H.264 video
     via :open command
    2. ✅ Audio and video synchronized
    (drift < 100ms over 1 minute)
    3. ✅ Space bar toggles play/pause
    4. ✅ Vim keys (h/j/k/l) seek
    through video
    5. ✅ Hardware acceleration active
    (GPU usage visible)
    6. ✅ Graceful error handling
    (missing files, corrupted videos)
    7. ✅ No audio stuttering or frame
    drops

    ---
    Key Files to Create/Modify

    Phase 0:

    - /Users/jaredsmith/Projects/grizld
    /Cargo.toml - Workspace manifest
    - /Users/jaredsmith/Projects/grizld
    /vp-core/Cargo.toml
    - /Users/jaredsmith/Projects/grizld
    /editor/Cargo.toml

    Phase 1 (vp-core):

    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/lib.rs
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/error.rs
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/types.rs
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/sync.rs ⭐ (A/V sync)
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/player.rs ⭐
    (orchestrator)
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/decoder/video.rs
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/decoder/audio.rs
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/buffer/frame_buffer.rs
    - /Users/jaredsmith/Projects/grizld
    /vp-core/src/buffer/audio_buffer.rs

    Phase 2 (editor):

    - /Users/jaredsmith/Projects/grizld
    /editor/src/main.rs
    - /Users/jaredsmith/Projects/grizld
    /editor/src/app.rs ⭐ (UI
    integration)
    - /Users/jaredsmith/Projects/grizld
    /editor/src/command/parser.rs ⭐
    (vim commands)
    - /Users/jaredsmith/Projects/grizld
    /editor/src/command/executor.rs
    - /Users/jaredsmith/Projects/grizld
    /editor/src/renderer.rs

    Verification:

    1. Build workspace: cargo build
    --workspace
    2. Run editor: cargo run --bin
    grizld
    3. Test with sample video: :open
    test.mp4, :play, test keyboard
    controls
    4. Check GPU usage in Activity
    Monitor
    5. Verify A/V sync with audio-heavy
     content
