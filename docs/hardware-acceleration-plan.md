# Hardware-Accelerated Video Rendering Plan

## Overview

**Problem**: Current video playback is choppy due to CPU-bound operations:
- FFmpeg decodes to CPU memory (Vec<u8>)
- Software downscaling for high-res content (renderer.rs:69-106)
- Full memory copy every frame to create egui ColorImage
- Texture upload happens on main thread
- Total overhead: 5-10ms per frame

**Solution**: Hardware decode directly to GPU memory with zero-copy Metal rendering
- AVFoundation decodes to CVPixelBuffer (GPU memory)
- Metal renders directly from pixel buffers (zero-copy via IOSurface)
- CAMetalLayer overlays eframe window
- Expected overhead: 1-2ms per frame → smooth 60fps playback

## Architecture

### Current Flow (Slow)
```
FFmpeg (VideoToolbox decode)
    ↓
Vec<u8> (CPU memory copy)
    ↓
Software downscaling (if needed)
    ↓
egui::ColorImage (another CPU copy)
    ↓
ctx.load_texture() (GPU upload)
    ↓
egui render (texture quad)
```

### Target Flow (Fast)
```
AVAssetReader
    ↓
CVPixelBuffer (IOSurface-backed GPU memory)
    ↓
CVMetalTextureCache (zero-copy texture creation)
    ↓
Metal Texture
    ↓
CAMetalLayer (hardware-accelerated render)
    ↓
Compositor (blend with eframe UI)
```

## Implementation Phases

### Phase 1: Metal Layer Integration

**Goal**: Overlay CAMetalLayer on eframe window for custom rendering

**Tasks**:
1. Add Metal dependencies to editor/Cargo.toml:
   ```toml
   objc2 = "0.5"
   objc2-metal = "0.2"
   objc2-quartz-core = "0.2"
   objc2-app-kit = "0.2"
   objc2-foundation = "0.2"
   ```

2. Create `editor/src/metal/mod.rs`:
   - `MetalContext` - Device, command queue setup
   - `MetalLayer` - CAMetalLayer wrapper with window integration
   - Safe Rust abstractions over Objective-C APIs

3. Create `editor/src/metal/layer_manager.rs`:
   - Extract NSView from eframe window (via winit)
   - Create and configure CAMetalLayer
   - Position layer (below UI for video, or above with transparency)
   - Handle window resizing

4. Modify `editor/src/app.rs`:
   - Initialize Metal context in EditorApp::new()
   - Store metal_layer: Option<MetalLayer>
   - Create layer when video is loaded
   - Sync layer bounds with video viewport

**Files**:
- NEW: editor/src/metal/mod.rs
- NEW: editor/src/metal/layer_manager.rs
- MODIFY: editor/src/app.rs (add Metal initialization)
- MODIFY: editor/Cargo.toml (add dependencies)

**Validation**:
- Can create CAMetalLayer on eframe window
- Layer visible with test color (clear to red/blue)
- Layer resizes with window
- egui UI still renders (transparency working)

---

### Phase 2: CVPixelBuffer Decoder

**Goal**: Replace FFmpeg with AVFoundation to decode directly to GPU memory

**Tasks**:
1. Add Core Video/AVFoundation dependencies to vp-core/Cargo.toml:
   ```toml
   objc2 = "0.5"
   objc2-core-video = "0.2"
   objc2-av-foundation = "0.2"
   objc2-foundation = "0.2"
   block2 = "0.5"  # For Objective-C blocks
   ```

2. Create `vp-core/src/decoder/hardware.rs`:
   - `HardwareVideoDecoder` struct
   - Use AVAssetReader + AVAssetReaderTrackOutput
   - Configure for hardware decode (kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)
   - Output CMSampleBuffer → CVPixelBuffer

3. Create `vp-core/src/types/pixel_buffer.rs`:
   - `PixelBuffer` wrapper around CVPixelBufferRef
   - Safe Rust API (retain/release, lock/unlock)
   - Extract width, height, format
   - Convert to IOSurfaceRef for Metal

4. Modify `vp-core/src/types.rs`:
   - Change VideoFrame to hold `PixelBuffer` instead of Vec<u8>
   - Remove redundant fields (width/height stored in buffer)
   - Keep PTS and frame metadata

5. Modify `vp-core/src/player.rs`:
   - Add decoder selection: try HardwareVideoDecoder first, fallback to FFmpeg
   - Pass PixelBuffer through frame buffer
   - Remove RGBA conversion logic

**Files**:
- NEW: vp-core/src/decoder/hardware.rs
- NEW: vp-core/src/types/pixel_buffer.rs
- MODIFY: vp-core/src/types.rs (VideoFrame definition)
- MODIFY: vp-core/src/player.rs (decoder selection)
- MODIFY: vp-core/Cargo.toml (dependencies)

**Validation**:
- Can decode H.264/HEVC to CVPixelBuffer
- Frames have correct PTS and dimensions
- IOSurface backing confirmed (check CVPixelBufferGetIOSurface)
- Memory usage stable (no leaks from retain/release)

---

### Phase 3: Zero-Copy Metal Renderer

**Goal**: Render CVPixelBuffer to CAMetalLayer without CPU copies

**Tasks**:
1. Create `editor/src/metal/texture_cache.rs`:
   - `CVMetalTextureCache` wrapper
   - Create Metal texture from CVPixelBuffer (zero-copy)
   - Handle YUV → RGB conversion in shader
   - Manage texture cache lifecycle

2. Create `editor/src/metal/video_renderer.rs`:
   - `MetalVideoRenderer` struct
   - Render pipeline for YUV → RGB conversion
   - Vertex/fragment shaders for texture sampling
   - Aspect ratio preservation (letterbox/pillarbox)
   - Color space conversion (BT.709/BT.601)

3. Create `editor/src/metal/shaders.metal`:
   ```metal
   // Vertex shader: fullscreen quad
   // Fragment shader: YUV texture sampling + color matrix
   // Support for 420v (2-plane), 420f (3-plane)
   ```

4. Modify `editor/src/renderer.rs`:
   - Replace egui texture logic with Metal rendering
   - Accept PixelBuffer instead of Vec<u8>
   - Delegate to MetalVideoRenderer
   - Remove software downscaling

5. Modify `editor/src/app.rs`:
   - Pass PixelBuffer to Metal renderer instead of egui
   - Remove ctx.load_texture() call
   - Keep egui for UI controls only

**Files**:
- NEW: editor/src/metal/texture_cache.rs
- NEW: editor/src/metal/video_renderer.rs
- NEW: editor/src/metal/shaders.metal
- MODIFY: editor/src/renderer.rs (use Metal instead of egui)
- MODIFY: editor/src/app.rs (integration)

**Validation**:
- Video renders in Metal layer
- Colors correct (YUV conversion accurate)
- Aspect ratio preserved
- Frame timing smooth (measure with Instruments)
- Zero CPU copies confirmed (profile with Xcode)

---

### Phase 4: Display Sync & Frame Timing

**Goal**: Sync Metal rendering with display refresh for perfect 60fps

**Tasks**:
1. Create `editor/src/metal/display_link.rs`:
   - CVDisplayLink wrapper for macOS
   - Callback on every display refresh (16.67ms @ 60Hz)
   - Pass frame to Metal renderer at exact vsync time

2. Modify `editor/src/app.rs`:
   - Replace egui's request_repaint() with display link
   - Metal renders on display link callback
   - egui updates on its own schedule (or sync with Metal)
   - Coordinate timing between video clock and display

3. Modify `vp-core/src/player.rs`:
   - Adjust frame buffer strategy for display-driven playback
   - Predict next frame time based on display refresh
   - Add frame interpolation hint (optional for future)

4. Performance monitoring:
   - Log frame render times
   - Track dropped frames
   - Measure GPU utilization

**Files**:
- NEW: editor/src/metal/display_link.rs
- MODIFY: editor/src/app.rs (display sync)
- MODIFY: vp-core/src/player.rs (timing strategy)

**Validation**:
- Rendering happens at display refresh rate
- No frame drops during playback
- Video/audio sync maintained (drift < 50ms)
- GPU usage efficient (Instruments profile)

---

### Phase 5: UI Integration & Polish

**Goal**: Seamlessly blend Metal video with egui controls

**Tasks**:
1. Layer composition strategy:
   - Option A: Video below UI (opaque egui panels on top)
   - Option B: Video above UI (transparent regions for controls)
   - Implement chosen approach with proper z-ordering

2. Viewport management:
   - Calculate video viewport from egui layout
   - Update Metal layer frame to match
   - Handle window resize smoothly

3. Create `editor/src/metal/compositor.rs`:
   - Manage layer hierarchy
   - Handle transparency/blending
   - Coordinate egui and Metal render passes

4. Add UI feedback:
   - Show hardware acceleration status
   - Display frame timing metrics
   - GPU decoder indicator

5. Error handling:
   - Graceful fallback to software if Metal unavailable
   - Handle CVPixelBuffer format mismatches
   - Metal device loss recovery

**Files**:
- NEW: editor/src/metal/compositor.rs
- MODIFY: editor/src/app.rs (UI layout coordination)
- MODIFY: editor/src/ui/viewport.rs (Metal viewport)

**Validation**:
- Video and UI render correctly together
- No z-fighting or flicker
- Controls responsive
- Smooth resizing

---

### Phase 6: Testing & Optimization

**Goal**: Verify performance gains and stability

**Tasks**:
1. Performance benchmarks:
   - Measure frame decode time (before: ~2ms, after: ~0.3ms)
   - Measure texture upload time (before: ~3ms, after: ~0ms)
   - Measure total frame time (before: ~8ms, after: ~1.5ms)
   - Profile with Xcode Instruments

2. Compatibility testing:
   - H.264 (8-bit, 10-bit)
   - HEVC/H.265
   - Different resolutions (720p, 1080p, 4K)
   - Various pixel formats (420v, 420f, 422)

3. Stress testing:
   - 4K60fps playback
   - Rapid seeking
   - Long playback sessions (memory leaks)
   - Multiple video opens/closes

4. Edge cases:
   - Videos without hardware decode support
   - Corrupted pixel buffers
   - Metal device not available
   - Window minimized/backgrounded

5. Documentation:
   - Update README with Metal requirements
   - Document decoder selection logic
   - Add architecture diagrams
   - Performance comparison table

**Validation**:
- 4K video plays at 60fps without drops
- Memory usage stable over 10+ minute playback
- No crashes or hangs
- Graceful fallback when hardware unavailable

---

## Technical Details

### Why objc2 Ecosystem?

The objc2 ecosystem provides modern, safe Rust bindings to Apple frameworks with several advantages over older crates:

**Safety & Ergonomics**:
- Type-safe Objective-C runtime with proper lifetime management
- `Retained<T>` smart pointer prevents use-after-free and memory leaks
- Compiler-enforced memory safety (no manual CFRetain/CFRelease)
- Better error handling with Result types

**Modern API Design**:
- Idiomatic Rust APIs generated from Apple's framework headers
- Direct mapping to Apple documentation (easier to follow examples)
- Support for latest macOS features and frameworks
- Active maintenance and community support

**Performance**:
- Zero-cost abstractions over Objective-C runtime
- No unnecessary allocations or copies
- Inline-able method calls when possible

**Ecosystem Integration**:
- Consistent API across all Apple framework bindings
- Works well with winit and other modern Rust GUI libraries
- Better support for raw-window-handle 0.6+

### Required Dependencies

```toml
# vp-core/Cargo.toml
[dependencies]
objc2 = "0.5"
objc2-core-video = "0.2"
objc2-av-foundation = "0.2"
objc2-foundation = "0.2"
block2 = "0.5"

# editor/Cargo.toml
[dependencies]
objc2 = "0.5"
objc2-metal = "0.2"
objc2-quartz-core = "0.2"
objc2-app-kit = "0.2"
objc2-foundation = "0.2"
```

### Key Rust/Objective-C Bindings

**CVPixelBuffer**:
```rust
use objc2_core_video::{
    CVPixelBuffer,
    CVPixelBufferLockFlags,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::NSString;

// Lock pixel buffer for CPU access
pixel_buffer.lock_base_address(CVPixelBufferLockFlags::ReadOnly)?;
// Access pixel data...
pixel_buffer.unlock_base_address(CVPixelBufferLockFlags::ReadOnly)?;
```

**Metal Texture Cache**:
```rust
use objc2_metal::{MTLDevice, MTLTexture, MTLPixelFormat};
use objc2_core_video::CVMetalTextureCache;

// Create texture cache
let cache = CVMetalTextureCache::new(&device)?;

// Create Metal texture from CVPixelBuffer (zero-copy)
let texture = cache.create_texture_from_image(
    &pixel_buffer,
    None, // texture attributes
    MTLPixelFormat::R8Unorm, // Y plane format
    width,
    height,
    0, // plane index
)?;
```

**CAMetalLayer**:
```rust
use objc2_app_kit::NSView;
use objc2_quartz_core::CAMetalLayer;
use objc2_metal::MTLDevice;
use objc2::rc::Retained;

// Create Metal layer
let layer = unsafe { CAMetalLayer::new() };
layer.setDevice(Some(&device));
layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
layer.setFramebufferOnly(true);

// Attach to NSView
nsview.setLayer(Some(&layer));
nsview.setWantsLayer(true);
```

### YUV to RGB Conversion Shader

```metal
fragment float4 fragmentShader(
    VertexOut in [[stage_in]],
    texture2d<float> textureY [[texture(0)]],
    texture2d<float> textureCbCr [[texture(1)]]
) {
    constexpr sampler s(address::clamp_to_edge, filter::linear);

    float y = textureY.sample(s, in.texCoord).r;
    float2 uv = textureCbCr.sample(s, in.texCoord).rg;

    // BT.709 color matrix
    float3x3 colorMatrix = float3x3(
        1.164,  1.164,  1.164,
        0.000, -0.213,  2.112,
        1.793, -0.533,  0.000
    );

    float3 yuv = float3(y - 0.0625, uv - 0.5);
    float3 rgb = colorMatrix * yuv;

    return float4(rgb, 1.0);
}
```

### CVPixelBuffer Format Support

Primary target: `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`
- 2-plane YUV (Y plane + interleaved CbCr plane)
- Hardware decode output format
- Efficient for Metal texture creation

Fallback: `kCVPixelFormatType_32BGRA`
- RGB format (no shader conversion needed)
- Higher memory bandwidth
- Software decode fallback

### Memory Management with objc2

**Retained<T> Smart Pointer**:
```rust
use objc2::rc::Retained;
use objc2_core_video::CVPixelBuffer;

// objc2 manages reference counting automatically
let pixel_buffer: Retained<CVPixelBuffer> = /* ... */;
// Automatically releases on drop - no manual CFRelease needed!

// Share between threads with Arc
let shared_buffer = Arc::new(pixel_buffer);
```

**CVPixelBuffer lifecycle**:
1. Decoded by AVAssetReader → `Retained<CVPixelBuffer>` (ref count = 1)
2. Wrapped in Rust struct → stored as `Retained<CVPixelBuffer>` (automatic retain)
3. Passed through frame buffer → `Arc<Retained<CVPixelBuffer>>`
4. Rendered by Metal → texture cache holds reference
5. Released when dropped → automatic (Drop impl on Retained)

**Benefits over old approach**:
- No manual CFRetain/CFRelease calls
- Compiler prevents use-after-free
- Drop trait ensures cleanup
- Send + Sync when appropriate

### AVAssetReader with objc2

**Example decoder implementation**:
```rust
use objc2_av_foundation::{AVAsset, AVAssetReader, AVAssetReaderTrackOutput};
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSError, NSURL};
use objc2::rc::Retained;

struct HardwareVideoDecoder {
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
}

impl HardwareVideoDecoder {
    fn new(path: &Path) -> Result<Self, Box<dyn Error>> {
        // Create URL from file path
        let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));

        // Create asset
        let asset = AVAsset::assetWithURL(&url);

        // Get video track
        let tracks = asset.tracksWithMediaType(AVMediaTypeVideo);
        let video_track = tracks.firstObject()
            .ok_or("No video track found")?;

        // Create reader
        let reader = AVAssetReader::assetReaderWithAsset_error(&asset)?;

        // Configure output for hardware decode
        let output_settings = NSDictionary::from([
            (kCVPixelBufferPixelFormatTypeKey,
             kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
        ]);

        let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
            &video_track,
            Some(&output_settings)
        );

        reader.addOutput(&output);
        reader.startReading();

        Ok(Self { reader, output })
    }

    fn next_frame(&mut self) -> Option<Retained<CVPixelBuffer>> {
        // Copy next sample buffer
        let sample_buffer = self.output.copyNextSampleBuffer()?;

        // Extract pixel buffer from sample
        let pixel_buffer = sample_buffer.getImageBuffer()?;

        // Retained<T> ensures proper reference counting
        Some(pixel_buffer)
    }
}
```

**Key objc2 patterns**:
- `Retained<T>` for all Objective-C objects
- `?` operator works with Result types
- No unsafe blocks needed for most operations
- Automatic memory management

### Window Integration

**Get NSView from eframe/winit**:
```rust
use objc2_app_kit::{NSView, NSWindow};
use objc2::rc::Retained;
use winit::platform::macos::WindowExtMacOS;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

// Get NSView from winit window
let window_handle = window.window_handle().unwrap();
let nsview = match window_handle.as_raw() {
    RawWindowHandle::AppKit(handle) => {
        let view_ptr = handle.ns_view.as_ptr() as *mut NSView;
        unsafe { Retained::retain(view_ptr).unwrap() }
    }
    _ => panic!("Not a macOS window"),
};
```

**Add Metal layer**:
```rust
use objc2_quartz_core::CAMetalLayer;
use objc2_metal::{MTLDevice, MTLPixelFormat};

let metal_layer = unsafe { CAMetalLayer::new() };
metal_layer.setDevice(Some(&device));
metal_layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
metal_layer.setFramebufferOnly(true);

// Attach layer to view
nsview.setLayer(Some(&metal_layer));
nsview.setWantsLayer(true);
```

---

## Migration Strategy

### Fallback to Current Implementation

Keep FFmpeg path as backup:
```rust
use objc2::rc::Retained;
use objc2_core_video::CVPixelBuffer;

enum FrameData {
    Hardware(Retained<CVPixelBuffer>),  // Zero-copy GPU memory
    Software(Vec<u8>),                   // CPU memory fallback
}

struct VideoFrame {
    pts: f64,
    data: FrameData,
    width: u32,
    height: u32,
}

enum DecoderBackend {
    Hardware(HardwareVideoDecoder),  // AVFoundation
    Software(VideoDecoder),          // FFmpeg
}

impl VideoPlayer {
    fn new(path: &Path) -> Result<Self> {
        // Try hardware decode first (objc2-based)
        let backend = match HardwareVideoDecoder::new(path) {
            Ok(hw) => {
                log::info!("Using hardware decoder with CVPixelBuffer");
                DecoderBackend::Hardware(hw)
            }
            Err(e) => {
                log::warn!("Hardware decode unavailable: {}, using FFmpeg", e);
                DecoderBackend::Software(VideoDecoder::new(path)?)
            }
        };
        // ...
    }

    fn next_frame(&mut self) -> Option<VideoFrame> {
        match &mut self.backend {
            DecoderBackend::Hardware(hw) => {
                let pixel_buffer = hw.next_frame()?;
                Some(VideoFrame {
                    pts: /* ... */,
                    data: FrameData::Hardware(pixel_buffer),
                    width: pixel_buffer.width(),
                    height: pixel_buffer.height(),
                })
            }
            DecoderBackend::Software(sw) => {
                let (data, pts) = sw.decode_frame()?;
                Some(VideoFrame {
                    pts,
                    data: FrameData::Software(data),
                    width: sw.width(),
                    height: sw.height(),
                })
            }
        }
    }
}
```

### Dual Renderer Support

```rust
use objc2::rc::Retained;
use objc2_core_video::CVPixelBuffer;

enum RenderBackend {
    Metal(MetalVideoRenderer),
    Egui(EguiVideoRenderer),
}

struct VideoRenderer {
    backend: RenderBackend,
}

impl VideoRenderer {
    fn render(&mut self, frame: &VideoFrame, ctx: &egui::Context, ui: &mut egui::Ui) {
        match (&mut self.backend, &frame.data) {
            // Hardware path: zero-copy Metal rendering
            (RenderBackend::Metal(renderer), FrameData::Hardware(pixel_buffer)) => {
                renderer.render_pixel_buffer(pixel_buffer);
                // Metal renders to CAMetalLayer directly
                // egui just needs to know to skip the video region
            }

            // Software path: traditional egui texture upload
            (RenderBackend::Egui(renderer), FrameData::Software(rgba_data)) => {
                let texture = renderer.update_texture(ctx, rgba_data, frame.width, frame.height);
                ui.image(texture);
            }

            // Fallback: convert CVPixelBuffer to RGBA if Metal unavailable
            (RenderBackend::Egui(renderer), FrameData::Hardware(pixel_buffer)) => {
                log::warn!("Converting CVPixelBuffer to RGBA for software rendering");
                let rgba_data = pixel_buffer_to_rgba(pixel_buffer);
                let texture = renderer.update_texture(ctx, &rgba_data, frame.width, frame.height);
                ui.image(texture);
            }

            // This shouldn't happen
            (RenderBackend::Metal(_), FrameData::Software(_)) => {
                log::error!("Cannot render software frame with Metal renderer");
            }
        }
    }
}

// Helper function for fallback path
fn pixel_buffer_to_rgba(pb: &Retained<CVPixelBuffer>) -> Vec<u8> {
    // Lock pixel buffer for CPU access
    pb.lock_base_address(CVPixelBufferLockFlags::ReadOnly).unwrap();

    // Convert YUV to RGBA (slow, but works as fallback)
    let rgba = /* conversion logic */;

    pb.unlock_base_address(CVPixelBufferLockFlags::ReadOnly).unwrap();
    rgba
}
```

---

## Success Criteria

### Performance Targets

| Metric | Current | Target | Validation |
|--------|---------|--------|------------|
| Frame decode time | ~2ms | <0.5ms | Instruments profile |
| Texture upload | ~3ms | 0ms (zero-copy) | No CPU memcpy |
| Render time | ~2ms | <1ms | Metal frame capture |
| Total frame time | 8-10ms | 1.5-2ms | Frame timing logs |
| 4K playback | Choppy (~30fps) | Smooth 60fps | Visual + metrics |
| Memory usage | Stable | Stable | Instruments leaks |

### Functional Requirements

- ✅ Smooth 60fps playback for 1080p/4K H.264 content
- ✅ Video/audio sync maintained (drift < 50ms)
- ✅ Zero-copy from decode to render (verified in profiler)
- ✅ Hardware acceleration confirmed (GPU usage in Activity Monitor)
- ✅ UI controls remain responsive (egui still renders)
- ✅ Graceful fallback to software decode if needed
- ✅ No memory leaks during extended playback
- ✅ Window resize/minimize handled correctly

### Technical Validation

1. **Zero-copy confirmed**: Instruments shows no memcpy from CVPixelBuffer to Metal texture
2. **Hardware decode active**: Activity Monitor shows Video Decode Engine usage
3. **Display sync working**: Frame timing aligned with vsync (16.67ms intervals)
4. **Memory stable**: No growth over 30 minutes of playback
5. **UI integration clean**: No z-fighting, flicker, or layout issues

---

## Risk Mitigation

### Risk: Metal not available on older Macs
**Mitigation**: Keep FFmpeg + egui path as fallback

### Risk: CVPixelBuffer format incompatible with Metal
**Mitigation**: Support multiple pixel formats in shader, convert if needed

### Risk: Window integration issues with eframe/winit
**Mitigation**: Thorough testing on multiple macOS versions, handle edge cases

### Risk: Audio/video sync breaks with new timing
**Mitigation**: Keep existing PlaybackClock, only change render path

### Risk: Performance worse than expected
**Mitigation**: Profile early (Phase 3), optimize before proceeding

---

## Files to Create/Modify

### New Files (18)
```
editor/src/metal/
├── mod.rs                    # Metal module root
├── context.rs                # Device, command queue setup
├── layer_manager.rs          # CAMetalLayer integration
├── texture_cache.rs          # CVMetalTextureCache wrapper
├── video_renderer.rs         # Main rendering logic
├── display_link.rs           # CVDisplayLink for vsync
├── compositor.rs             # Layer hierarchy management
└── shaders.metal             # YUV→RGB shader

vp-core/src/decoder/
└── hardware.rs               # AVAssetReader decoder

vp-core/src/types/
└── pixel_buffer.rs           # CVPixelBuffer wrapper
```

### Modified Files (8)
```
editor/Cargo.toml             # Add Metal dependencies
editor/src/app.rs             # Metal initialization, integration
editor/src/renderer.rs        # Use Metal instead of egui texture

vp-core/Cargo.toml            # Add Core Video dependencies
vp-core/src/types.rs          # VideoFrame with PixelBuffer
vp-core/src/player.rs         # Decoder selection
vp-core/src/decoder/mod.rs    # Export hardware decoder
```

### Documentation
```
README.md                     # Update with Metal requirements
docs/architecture.md          # Add hardware rendering diagram
docs/performance.md           # Benchmark results
```

---

## Timeline & Priorities

### Phase 1: Metal Layer (Foundation)
- Priority: HIGH
- Estimated: 2-3 days
- Blocker for all subsequent work

### Phase 2: CVPixelBuffer Decoder (Core)
- Priority: HIGH
- Estimated: 3-4 days
- Can start partially in parallel with Phase 1

### Phase 3: Zero-Copy Renderer (Critical Path)
- Priority: HIGH
- Estimated: 4-5 days
- Most complex phase, YUV shader tricky

### Phase 4: Display Sync (Polish)
- Priority: MEDIUM
- Estimated: 2-3 days
- Can defer if Phase 3 already smooth

### Phase 5: UI Integration (Essential)
- Priority: HIGH
- Estimated: 2-3 days
- Needed for usable editor

### Phase 6: Testing (Critical)
- Priority: HIGH
- Estimated: 3-4 days
- Ongoing throughout development

**Total Estimated Time**: 3-4 weeks of focused work

---

## Build Configuration

### Cargo.toml Setup

```toml
[workspace]
members = ["vp-core", "editor"]

[workspace.dependencies]
# objc2 ecosystem
objc2 = "0.5"
objc2-foundation = "0.2"

# Existing dependencies
ffmpeg-next = "8.0"
cpal = "0.15"
egui = "0.30"
eframe = "0.30"
```

```toml
# editor/Cargo.toml
[package]
name = "grizld-editor"

[dependencies]
# objc2 ecosystem for Metal rendering
objc2 = { workspace = true }
objc2-metal = "0.2"
objc2-quartz-core = "0.2"
objc2-app-kit = "0.2"
objc2-foundation = { workspace = true }

# Existing
egui = { workspace = true }
eframe = { workspace = true }
vp-core = { path = "../vp-core" }

[target.'cfg(target_os = "macos")'.dependencies]
# Metal rendering only on macOS
objc2-metal = "0.2"
objc2-quartz-core = "0.2"
```

```toml
# vp-core/Cargo.toml
[package]
name = "vp-core"

[dependencies]
# objc2 ecosystem for hardware decode
objc2 = { workspace = true }
objc2-core-video = "0.2"
objc2-av-foundation = "0.2"
objc2-foundation = { workspace = true }
block2 = "0.5"

# Existing
ffmpeg-next = { workspace = true }
cpal = { workspace = true }

[target.'cfg(target_os = "macos")'.dependencies]
# Hardware decode only on macOS
objc2-core-video = "0.2"
objc2-av-foundation = "0.2"
block2 = "0.5"
```

### Feature Flags (Future)

Consider adding feature flags for gradual rollout:

```toml
[features]
default = ["hardware-accel"]
hardware-accel = ["dep:objc2-metal", "dep:objc2-core-video"]
metal-renderer = ["hardware-accel"]
```

This allows:
- Building without hardware acceleration on non-macOS
- Testing software-only path on macOS
- Gradual feature rollout

## Next Steps

1. Review this plan with stakeholders
2. Set up development environment:
   - Xcode Command Line Tools (for Metal SDK)
   - Xcode Instruments (for profiling)
   - rust-analyzer with objc2 support
3. Add objc2 dependencies to Cargo.toml files
4. Create feature branch: `feature/hardware-acceleration`
5. Start Phase 1: Metal layer integration
6. Test incrementally after each phase
7. Keep FFmpeg fallback until Phase 6 complete
