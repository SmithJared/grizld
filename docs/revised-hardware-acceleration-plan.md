# Revised Hardware Acceleration Plan
## Synthesis of Old and New Architecture

After analyzing the previous implementation in `/old`, we've identified a **much simpler approach** than using AVAssetReader directly.

## Key Insights from Old Implementation

### What Worked Well
1. **FFmpeg + VideoToolbox**: Used FFmpeg's built-in VideoToolbox support instead of raw AVFoundation APIs
2. **CVPixelBuffer extraction**: Extracted `CVPixelBuffer` from FFmpeg hardware frames via `frame.data[3]` pointer
3. **objc2 0.3.x APIs**: Used function-based Core Video APIs (e.g., `CVPixelBufferGetWidth()`)
4. **Metal rendering**: Separate Metal renderer with CVMetalTextureCache for zero-copy rendering

### Why This is Better
- **Less complexity**: No need to reimplement decoder with AVAssetReader
- **Better compatibility**: FFmpeg handles codec variations, seeking, timestamps
- **Proven approach**: Already working in the old codebase
- **Easier debugging**: FFmpeg's VideoToolbox integration is well-tested

## Revised Architecture

### Current Status
**Phase 1 Complete ✅**:
- Metal layer integrated with eframe
- CAMetalLayer rendering test colors
- Layer positioning working (70% opacity for testing)

**What We Built**:
```
editor/src/metal/
├── mod.rs              # MetalContext (device, command queue)
└── layer_manager.rs    # CAMetalLayer integration with NSView
```

### New Plan: FFmpeg + VideoToolbox Integration

Instead of AVAssetReader, we'll enhance our existing FFmpeg decoder:

```
Current Flow (Software):
FFmpeg decode → Vec<u8> → egui texture → GPU upload

Target Flow (Hardware):
FFmpeg decode → CVPixelBuffer → Metal texture (zero-copy) → CAMetalLayer
                ↓
           (IOSurface)
```

## Implementation Plan (Revised)

### Phase 2: VideoToolbox Hardware Decoder

**Goal**: Add VideoToolbox hardware acceleration to existing FFmpeg decoder

**Approach**:
1. Use FFmpeg's hardware device context APIs (already in ffmpeg-next)
2. Configure decoder for VideoToolbox (`AV_PIX_FMT_VIDEOTOOLBOX`)
3. Detect hardware frames and extract CVPixelBuffer
4. Update FrameData enum to support both CPU and GPU frames

**Files to Modify**:
- `vp-core/src/decoder/video.rs` - Add hardware decoder variant
- `vp-core/src/types/mod.rs` - Update FrameData enum
- `vp-core/src/types/pixel_buffer.rs` - **Simplify** to wrap CVPixelBuffer from FFmpeg

**Key Code Pattern** (from old implementation):
```rust
// In FFmpeg decoder
let hw_ctx = HardwareDeviceContext::new(
    AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX
)?;

let decoder = decoder
    .with_hw_ctx(hw_ctx)
    .with_pix_fmt(AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX);

// Extract CVPixelBuffer from frame
fn extract_pixel_buffer(frame: &ffmpeg::Frame) -> Option<PixelBuffer> {
    unsafe {
        let frame_ptr = frame.as_ptr();
        // VideoToolbox frames store CVPixelBuffer in data[3]
        let cv_pixel_buffer_ptr = (*frame_ptr).data[3] as *mut CVPixelBuffer;

        if cv_pixel_buffer_ptr.is_null() {
            return None;
        }

        // Retain and wrap
        Retained::retain(cv_pixel_buffer_ptr.cast()).map(PixelBuffer::new)
    }
}
```

### Phase 3: CVMetalTextureCache Integration

**Goal**: Create Metal textures directly from CVPixelBuffer (zero-copy)

**Approach**:
1. Create CVMetalTextureCache in Metal renderer
2. Convert CVPixelBuffer to Metal textures
3. Render YUV textures with shader (from old code)

**Key APIs** (function-based, objc2-core-video 0.3):
```rust
use objc2_core_video::{
    CVMetalTextureCacheCreate,
    CVMetalTextureCacheCreateTextureFromImage,
    CVPixelBufferGetWidth,
    CVPixelBufferGetHeight,
};

// Create cache
let cache = CVMetalTextureCacheCreate(
    None, // allocator
    None, // cache attributes
    &device,
    None, // texture attributes
)?;

// Create texture from pixel buffer (zero-copy!)
let texture = CVMetalTextureCacheCreateTextureFromImage(
    None, // allocator
    cache,
    pixel_buffer,
    None, // texture attributes
    pixel_format,
    width,
    height,
    plane_index,
)?;
```

### Phase 4: YUV to RGB Shader

**Goal**: Convert YUV pixel buffers to RGB for display

**Shader** (from old implementation):
```metal
// p210_to_rgb.metal
fragment float4 fragment_main(
    VertexOut in [[stage_in]],
    texture2d<float> yTexture [[texture(0)]],
    texture2d<float> uvTexture [[texture(1)]]
) {
    constexpr sampler textureSampler(
        mag_filter::linear,
        min_filter::linear
    );

    float y = yTexture.sample(textureSampler, in.texCoord).r;
    float2 uv = uvTexture.sample(textureSampler, in.texCoord).rg - 0.5;

    // BT.709 color matrix
    float r = y + 1.5748 * uv.y;
    float g = y - 0.1873 * uv.x - 0.4681 * uv.y;
    float b = y + 1.8556 * uv.x;

    return float4(r, g, b, 1.0);
}
```

## Dependency Strategy

### Use objc2 0.3.x (matching old implementation)

```toml
# Workspace Cargo.toml
[workspace.dependencies]
objc2 = "0.5"  # Keep current for compatibility
objc2-foundation = "0.3"
objc2-metal = "0.3"
objc2-quartz-core = "0.3"
objc2-app-kit = "0.3"
objc2-core-video = "0.3"
```

**Important**: Use **function-based** Core Video APIs (not methods):
- ✅ `CVPixelBufferGetWidth(buffer)`
- ❌ `buffer.getWidth()` (doesn't exist in objc2 0.3)

## Updated FrameData Enum

```rust
#[derive(Clone)]
pub enum FrameData {
    /// Software-decoded frame (CPU memory)
    Software {
        data: Vec<u8>,
        format: PixelFormat,
    },

    /// Hardware-decoded frame (GPU memory, macOS only)
    #[cfg(target_os = "macos")]
    Hardware(PixelBuffer),
}

impl FrameData {
    pub fn is_hardware(&self) -> bool {
        #[cfg(target_os = "macos")]
        { matches!(self, FrameData::Hardware(_)) }
        #[cfg(not(target_os = "macos"))]
        { false }
    }
}
```

## Simplified PixelBuffer Wrapper

Based on old implementation, **much simpler** than our current attempt:

```rust
use objc2::rc::Retained;
use objc2_core_video::{
    CVPixelBuffer,
    CVPixelBufferGetWidth,
    CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType,
};

#[derive(Clone)]
pub struct PixelBuffer(Retained<CVPixelBuffer>);

impl PixelBuffer {
    /// Create from FFmpeg hardware frame
    pub fn from_ffmpeg_frame(frame: &ffmpeg::Frame) -> Option<Self> {
        unsafe {
            let frame_ptr = frame.as_ptr();
            let cv_pixel_buffer_ptr = (*frame_ptr).data[3] as *mut CVPixelBuffer;

            if cv_pixel_buffer_ptr.is_null() {
                return None;
            }

            Retained::retain(cv_pixel_buffer_ptr.cast()).map(Self)
        }
    }

    pub fn width(&self) -> usize {
        CVPixelBufferGetWidth(&self.0)
    }

    pub fn height(&self) -> usize {
        CVPixelBufferGetHeight(&self.0)
    }

    pub fn pixel_format(&self) -> u32 {
        CVPixelBufferGetPixelFormatType(&self.0)
    }

    pub fn inner(&self) -> &Retained<CVPixelBuffer> {
        &self.0
    }
}

unsafe impl Send for PixelBuffer {}
unsafe impl Sync for PixelBuffer {}
```

## Comparison: Old vs New Approach

### What We Tried (AVAssetReader)
**Pros**:
- Direct access to Apple APIs
- Clean separation from FFmpeg

**Cons**:
- Complex objc2 API usage
- Reimplementing decoder logic
- Harder to handle seeking, timestamps
- More code to maintain

### Revised Approach (FFmpeg + VideoToolbox)
**Pros**:
- Leverages existing FFmpeg integration
- FFmpeg handles all codec complexity
- Proven pattern from old codebase
- Less code, easier maintenance
- Better error handling

**Cons**:
- Still depends on FFmpeg (but we already use it)

## Migration Path

### Step 1: Update Dependencies
- Already done ✅ (objc2 0.5, objc2-* 0.3)

### Step 2: Simplify PixelBuffer Wrapper
- Remove AVFoundation code
- Implement `from_ffmpeg_frame()` method
- Use function-based Core Video APIs

### Step 3: Add Hardware Decoder Variant
- Create `VideoToolboxDecoder` in `vp-core/src/decoder/`
- Use FFmpeg hardware device context
- Extract CVPixelBuffer from frames

### Step 4: Update VideoDecoder to Try Hardware First
```rust
pub enum DecoderBackend {
    #[cfg(target_os = "macos")]
    Hardware(VideoToolboxDecoder),
    Software(SoftwareVideoDecoder),
}

impl VideoDecoder {
    pub fn new(stream: &VideoStream) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            // Try hardware first
            match VideoToolboxDecoder::new(stream) {
                Ok(hw) => {
                    tracing::info!("Using VideoToolbox hardware decoder");
                    return Ok(DecoderBackend::Hardware(hw));
                }
                Err(e) => {
                    tracing::warn!("Hardware decoder unavailable: {}, using software", e);
                }
            }
        }

        // Fallback to software
        Ok(DecoderBackend::Software(SoftwareVideoDecoder::new(stream)?))
    }
}
```

### Step 5: Connect to Metal Renderer
- Update `editor/src/metal/video_renderer.rs`
- Add CVMetalTextureCache
- Implement YUV → RGB rendering

### Step 6: Remove Test Color, Enable Real Video
- Remove the blue test color rendering
- Connect video frames to Metal renderer
- Update layer opacity to 1.0 (opaque)

## Success Criteria

### Phase 2 Complete When:
- ✅ FFmpeg decoder can use VideoToolbox acceleration
- ✅ CVPixelBuffer successfully extracted from hardware frames
- ✅ Software fallback works when hardware unavailable
- ✅ No crashes or memory leaks

### Phase 3 Complete When:
- ✅ CVMetalTextureCache creates textures from CVPixelBuffer
- ✅ Zero-copy confirmed (no CPU memcpy in profiler)
- ✅ YUV frames render correctly with Metal shader

### Phase 4 Complete When:
- ✅ Video plays smoothly at 60fps (1080p and 4K)
- ✅ Colors accurate (BT.709 color space)
- ✅ Audio/video sync maintained
- ✅ Metal layer positioned correctly over video viewport only

## Timeline Estimate

- **Phase 2 (Hardware Decoder)**: 1-2 days
- **Phase 3 (Metal Texture Cache)**: 2-3 days
- **Phase 4 (YUV Shader + Polish)**: 1-2 days

**Total**: 4-7 days of focused work

Much faster than the AVAssetReader approach!

## References

- Old implementation: `/old/vp-core/src/decoder/video_toolbox.rs`
- Old PixelBuffer: `/old/video-sys/src/core_video/pixel_buffer.rs`
- Old Metal renderer: `/old/vp-core/src/render/metal/renderer.rs`
- Old shader: `/old/vp-core/src/render/shaders/p210_to_rgb.metal`
