# Hardware Rendering Optimization - Handoff Brief

**Date**: 2025-10-10  
**Status**: Hardware decoding complete ✅ | Renderer optimization needed ⏳  
**Priority**: High - Unlocks full 4K/HEVC performance

---

## Executive Summary

We've successfully implemented hardware-accelerated video decoding using VideoToolbox on macOS, achieving **10.3x performance improvement** (6.87 FPS → 70.86 FPS for 4K HEVC). The decoder now outputs **100% hardware frames** (IOSurface references) with zero CPU conversion.

**Next step**: Optimize the rendering pipeline to use these hardware frames directly, avoiding expensive GPU→CPU→GPU round-trips that currently limit performance.

---

## What We Accomplished

### 1. Hardware Decoding Pipeline ✅

**Files Modified:**
- `vp_core/src/decode/hardware_decoder.rs` - VideoToolbox decoder implementation
- `vp_core/build.rs` - Links CoreVideo and VideoToolbox frameworks
- `vp_core/Cargo.toml` - Added build script configuration
- `vp_core/examples/decode_benchmark.rs` - Performance measurement tool

**Key Achievements:**

1. **Hardware Device Context Creation**
   ```rust
   // Create VideoToolbox hardware device context
   av_hwdevice_ctx_create(
       &mut hw_device_ctx,
       AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
       ...
   );
   (*decoder.as_mut_ptr()).hw_device_ctx = hw_device_ctx;
   ```

2. **Hardware Pixel Format Selection**
   ```rust
   // Request hardware frames (not software YUV)
   (*decoder.as_mut_ptr()).pix_fmt = AV_PIX_FMT_VIDEOTOOLBOX;
   (*decoder.as_mut_ptr()).get_format = Some(Self::get_hw_format);
   ```

3. **IOSurface Extraction**
   ```rust
   // Extract IOSurface using CoreVideo C API
   extern "C" {
       fn CVPixelBufferGetIOSurface(pixelBuffer: CVPixelBufferRef) -> *mut c_void;
   }
   let iosurface = CVPixelBufferGetIOSurface(pixel_buffer);
   ```

4. **Framework Linking** (build.rs)
   ```rust
   println!("cargo:rustc-link-lib=framework=CoreVideo");
   println!("cargo:rustc-link-lib=framework=VideoToolbox");
   ```

### 2. Performance Results ✅

**Benchmark Command:**
```bash
cargo run --example decode_benchmark --release
```

**Results (4K HEVC @ 60fps):**
```
Hardware frames: 309 (100.0%)
Software frames: 0 (0.0%)

Decode FPS: 70.86 (vs 6.87 software)
Average time per frame: 14.11ms (vs 145ms software)
Speedup vs real-time: 1.19x
Performance improvement: 10.3x faster!
```

### 3. Architecture Overview ✅

**Current Data Flow:**
```
Demuxer → Hardware Decoder → VideoFrame::Hardware(IOSurface) → FrameBuffer
                                                                      ↓
                                                              PlaybackController
```

**VideoFrame Enum:**
```rust
pub enum VideoFrame {
    Software(SoftwareFrame),  // YUV data in CPU memory
    Hardware(HardwareFrame),  // IOSurface pointer (GPU memory)
}
```

**HardwareFrame Structure:**
```rust
pub struct HardwareFrame {
    pub pts: f64,
    pub width: u32,
    pub height: u32,
    iosurface: *mut c_void,  // IOSurface reference
}
```

---

## What Needs to Be Done

### Objective: Zero-Copy Hardware Rendering

Enable the renderer to use `VideoFrame::Hardware` directly, rendering from IOSurface without CPU conversion.

### Current Problem

The existing renderer (in the `player` crate, not `vp_core`) expects software frames with YUV data:

```rust
// Current renderer expects this:
struct SoftwareFrame {
    y_plane: Vec<u8>,  // CPU memory
    u_plane: Vec<u8>,
    v_plane: Vec<u8>,
    // ...
}
```

When hardware frames are used, they must be converted to software format, which:
- Copies 8.3 million pixels from GPU → CPU (slow!)
- Negates all hardware acceleration benefits
- Limits performance to ~70 FPS instead of 200+ FPS

### Solution: IOSurface Rendering

Modify the renderer to accept `VideoFrame::Hardware` and render directly from IOSurface.

---

## Implementation Guide

### Step 1: Understand IOSurface on macOS

**What is IOSurface?**
- macOS framework for sharing GPU surfaces between processes
- Zero-copy sharing between VideoToolbox and Metal/OpenGL
- Backed by GPU memory, not CPU memory

**Key APIs:**
```rust
// CoreFoundation/IOSurface
extern "C" {
    fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
    fn IOSurfaceGetPixelFormat(surface: *mut c_void) -> u32;
}
```

### Step 2: Modify Renderer to Accept Hardware Frames

**Location**: `player/src/renderer/` (or equivalent)

**Current Signature:**
```rust
fn render_frame(&mut self, frame: &SoftwareFrame) { ... }
```

**New Signature:**
```rust
fn render_frame(&mut self, frame: &VideoFrame) {
    match frame {
        VideoFrame::Software(sw_frame) => self.render_software(sw_frame),
        VideoFrame::Hardware(hw_frame) => self.render_hardware(hw_frame),
    }
}
```

### Step 3: Implement IOSurface → Metal/wgpu Texture

**Option A: Metal (Recommended for macOS)**

```rust
use metal::*;

fn render_hardware(&mut self, hw_frame: &HardwareFrame) {
    // Get IOSurface pointer
    let iosurface = hw_frame.iosurface_ptr();
    
    // Create Metal texture from IOSurface
    let texture_descriptor = TextureDescriptor::new();
    texture_descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    texture_descriptor.set_width(hw_frame.width as u64);
    texture_descriptor.set_height(hw_frame.height as u64);
    texture_descriptor.set_usage(MTLTextureUsage::ShaderRead);
    
    let texture = device.new_texture_with_descriptor_iosurface(
        &texture_descriptor,
        iosurface,
        0, // plane
    );
    
    // Use texture for rendering
    // ...
}
```

**Option B: wgpu (Cross-platform, more complex)**

wgpu doesn't have direct IOSurface support, but you can:
1. Use `wgpu-hal` for low-level Metal access
2. Create Metal texture from IOSurface
3. Wrap it in wgpu texture

**Reference**: See `player/src/renderer/wgpu_renderer.rs` for existing wgpu setup

### Step 4: Handle Pixel Format Conversion

**IOSurface Pixel Formats:**
- VideoToolbox typically outputs: `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange` (NV12)
- Or: `kCVPixelFormatType_32BGRA` (BGRA)

**Shader Adjustments:**
- If NV12: Use 2-plane YUV shader (Y plane + UV interleaved)
- If BGRA: Direct RGB rendering (no conversion needed)

**Check format:**
```rust
let pixel_format = unsafe { IOSurfaceGetPixelFormat(iosurface) };
match pixel_format {
    0x34323076 => { /* NV12 */ },
    0x42475241 => { /* BGRA */ },
    _ => { /* Fallback to software */ },
}
```

### Step 5: Update PlaybackController Integration

**Ensure hardware frames flow through:**

```rust
// In PlaybackController::get_current_frame()
pub fn get_current_frame(&self) -> Option<VideoFrame> {
    let time = self.clock.lock().unwrap().time();
    let buffer = self.buffer.lock().unwrap();
    
    // This already returns VideoFrame (HW or SW)
    buffer.get_frame_at(time).map(|f| f.clone())
}
```

**In application update loop:**
```rust
if let Some(frame) = playback_controller.get_current_frame() {
    // Pass VideoFrame directly to renderer
    renderer.render_frame(&frame);  // Handles both HW and SW
}
```

---

## Testing Strategy

### 1. Verify Hardware Frame Flow

Add logging to track frame types:

```rust
match frame {
    VideoFrame::Hardware(_) => {
        tracing::info!("Rendering hardware frame (IOSurface)");
    }
    VideoFrame::Software(_) => {
        tracing::info!("Rendering software frame (YUV)");
    }
}
```

**Expected**: 100% hardware frames for H.264/HEVC on macOS

### 2. Performance Benchmarks

**Before (with CPU conversion):**
- Render FPS: ~60 FPS (limited by conversion)
- CPU usage: High (YUV conversion)

**After (direct IOSurface):**
- Render FPS: 200+ FPS (GPU-only)
- CPU usage: Low (no conversion)

### 3. Visual Quality Check

Ensure no visual artifacts:
- Color accuracy (YUV→RGB conversion correct)
- No tearing or flickering
- Proper aspect ratio

---

## Code Locations

### vp_core (Library - Complete ✅)

```
vp_core/
├── src/
│   ├── decode/
│   │   ├── mod.rs                    # VideoFrame enum, VideoDecoder trait
│   │   ├── hardware_decoder.rs       # VideoToolbox implementation ✅
│   │   └── ffmpeg_decoder.rs         # Software decoder ✅
│   ├── buffer/mod.rs                 # FrameBuffer (supports HW frames) ✅
│   └── playback/mod.rs               # PlaybackController ✅
├── build.rs                          # Framework linking ✅
└── examples/decode_benchmark.rs      # Performance testing ✅
```

### player (Application - Needs Work ⏳)

```
player/
├── src/
│   ├── renderer/
│   │   ├── wgpu_renderer.rs          # Current renderer (SW frames only) ⏳
│   │   └── texture.rs                # YUV texture management ⏳
│   └── main.rs                       # Application loop ⏳
```

---

## Dependencies

### Already Added ✅
- `ffmpeg-next` - FFmpeg bindings
- `ffmpeg-sys-next` - Low-level FFmpeg API
- `objc` - Objective-C runtime (macOS)

### May Need to Add ⏳
- `metal` - Metal graphics API (for IOSurface rendering)
- `core-foundation` - CoreFoundation types
- `io-surface` - IOSurface bindings (or use raw FFI)
- `wgpu-hal` - Low-level wgpu API for Metal integration

**Add to Cargo.toml:**
```toml
[dependencies]
wgpu-hal = "25.0"  # Must match wgpu version for compatibility

[target.'cfg(target_os = "macos")'.dependencies]
metal = "0.29"
core-foundation = "0.9"
```

---

## IOSurface Management Details

### Reference Counting

**Critical**: IOSurface uses CoreFoundation reference counting. You must properly retain/release to avoid memory leaks or crashes.

```rust
// CoreFoundation reference counting
extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

// In HardwareFrame implementation
impl HardwareFrame {
    pub fn new(iosurface: *mut c_void, pts: f64, width: u32, height: u32) -> Self {
        // Retain the IOSurface (increment ref count)
        unsafe { CFRetain(iosurface as *const c_void) };
        
        Self {
            pts,
            width,
            height,
            iosurface,
        }
    }
}

impl Drop for HardwareFrame {
    fn drop(&mut self) {
        // Release the IOSurface (decrement ref count)
        if !self.iosurface.is_null() {
            unsafe { CFRelease(self.iosurface as *const c_void) };
        }
    }
}

impl Clone for HardwareFrame {
    fn clone(&self) -> Self {
        // Retain on clone
        unsafe { CFRetain(self.iosurface as *const c_void) };
        Self {
            pts: self.pts,
            width: self.width,
            height: self.height,
            iosurface: self.iosurface,
        }
    }
}
```

### Texture Descriptor Requirements

**Important**: IOSurface-backed textures require specific usage flags in wgpu.

```rust
use wgpu::*;

fn create_texture_descriptor_for_iosurface(width: u32, height: u32) -> TextureDescriptor {
    TextureDescriptor {
        label: Some("IOSurface Texture"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        // Format depends on IOSurface pixel format (NV12, BGRA, etc.)
        format: TextureFormat::Rgba8Unorm,  // Or appropriate format
        // CRITICAL: Must include TEXTURE_BINDING for sampling in shaders
        // May also need RENDER_ATTACHMENT if rendering to it
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    }
}
```

### wgpu-hal Integration Pattern

**The bridge between Metal and wgpu:**

```rust
use wgpu_hal::api::Metal as MetalApi;
use metal::*;

#[cfg(target_os = "macos")]
fn create_wgpu_texture_from_iosurface(
    device: &wgpu::Device,
    iosurface: *mut c_void,
    width: u32,
    height: u32,
) -> Result<wgpu::Texture, Box<dyn std::error::Error>> {
    // 1. Get the underlying Metal device from wgpu
    let hal_device = device.as_hal::<MetalApi, _, _>(|hal_device| {
        hal_device.ok_or("Failed to get Metal device")
    })?;
    
    // 2. Create Metal texture descriptor
    let metal_desc = TextureDescriptor::new();
    metal_desc.set_width(width as u64);
    metal_desc.set_height(height as u64);
    metal_desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);  // Or NV12
    metal_desc.set_usage(MTLTextureUsage::ShaderRead);
    
    // 3. Create Metal texture from IOSurface
    let metal_texture = hal_device.new_texture_with_descriptor_iosurface(
        &metal_desc,
        iosurface,
        0,  // plane index
    )?;
    
    // 4. Wrap Metal texture in wgpu using wgpu-hal
    let wgpu_texture = unsafe {
        device.create_texture_from_hal::<MetalApi>(
            metal_texture,
            &create_texture_descriptor_for_iosurface(width, height),
        )
    };
    
    Ok(wgpu_texture)
}
```

**Key Points:**
- `as_hal()` provides access to the underlying Metal device
- `create_texture_from_hal()` wraps a Metal texture in wgpu
- This is an `unsafe` operation - you're responsible for lifetime management
- The Metal texture must remain valid for the lifetime of the wgpu texture

### Pixel Format Detection

**IOSurface can have different pixel formats. Detect and handle appropriately:**

```rust
extern "C" {
    fn IOSurfaceGetPixelFormat(surface: *mut c_void) -> u32;
}

// Common pixel format FourCC codes
const kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: u32 = 0x34323076; // 'NV12'
const kCVPixelFormatType_32BGRA: u32 = 0x42475241; // 'BGRA'

fn get_texture_format_for_iosurface(iosurface: *mut c_void) -> Option<TextureFormat> {
    let pixel_format = unsafe { IOSurfaceGetPixelFormat(iosurface) };
    
    match pixel_format {
        kCVPixelFormatType_32BGRA => Some(TextureFormat::Bgra8Unorm),
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => {
            // NV12 requires 2 textures (Y plane + UV plane)
            // Return Y plane format here
            Some(TextureFormat::R8Unorm)
        },
        _ => {
            eprintln!("Unsupported pixel format: 0x{:08X}", pixel_format);
            None
        }
    }
}
```

---

## Potential Challenges

### 1. wgpu Limitations

**Issue**: wgpu doesn't expose IOSurface APIs directly

**Solutions**:
- Use `wgpu-hal` for low-level Metal access
- Create Metal texture, wrap in wgpu
- Or: Switch to pure Metal for macOS (better performance)

### 2. Pixel Format Handling

**Issue**: VideoToolbox may output different formats (NV12, BGRA, etc.)

**Solution**: Check pixel format and use appropriate shader:
```rust
match pixel_format {
    NV12 => use_nv12_shader(),
    BGRA => use_bgra_shader(),
    _ => fallback_to_software(),
}
```

### 3. Cross-Platform Compatibility

**Issue**: IOSurface is macOS-only

**Solution**: Keep software rendering path for other platforms:
```rust
#[cfg(target_os = "macos")]
fn render_hardware(...) { /* IOSurface */ }

#[cfg(not(target_os = "macos"))]
fn render_hardware(...) { /* Fallback to software */ }
```

---

## Success Criteria

### Performance Targets

- **4K HEVC @ 60fps**: Maintain 60+ FPS rendering
- **CPU usage**: < 20% (vs 80%+ with conversion)
- **Memory**: No GPU→CPU copies (zero-copy)

### Functional Requirements

- ✅ Hardware frames render correctly
- ✅ Software frames still work (fallback)
- ✅ No visual artifacts or quality loss
- ✅ Smooth playback with no stuttering

### Code Quality

- ✅ Clean separation of HW/SW rendering paths
- ✅ Proper error handling (fallback to software)
- ✅ Cross-platform compatibility maintained
- ✅ Well-documented with examples

---

## Resources

### Documentation

- [IOSurface Framework](https://developer.apple.com/documentation/iosurface)
- [Metal Texture Creation](https://developer.apple.com/documentation/metal/mtldevice/1433425-maketexture)
- [VideoToolbox Programming Guide](https://developer.apple.com/documentation/videotoolbox)
- [wgpu-hal Documentation](https://docs.rs/wgpu-hal/)

### Example Code

- `vp_core/examples/decode_benchmark.rs` - Hardware decoding benchmark
- `vp_core/tests/decode_tests.rs` - Hardware frame tests
- `player/src/renderer/wgpu_renderer.rs` - Current renderer (software)

### Helpful Commands

```bash
# Run hardware decode benchmark
cd vp_core
cargo run --example decode_benchmark --release

# Run with software decoder (comparison)
cargo run --example decode_benchmark --release -- tests/resources/test_video.mp4 --software

# Check IOSurface availability
system_profiler SPDisplaysDataType | grep Metal
```

---

## Questions to Answer

1. **Which rendering API?**
   - Pure Metal (macOS-only, best performance)
   - wgpu with Metal backend (cross-platform, more complex)

2. **Pixel format strategy?**
   - Support NV12 only (most common)
   - Support multiple formats (more robust)

3. **Fallback behavior?**
   - Always fallback to software on error
   - Or fail fast and report error

4. **Testing approach?**
   - Unit tests for IOSurface handling
   - Integration tests with real video files
   - Performance benchmarks

---

## Next Steps

### Immediate (Week 1)

1. **Research**: Study IOSurface + Metal/wgpu integration
2. **Prototype**: Create minimal IOSurface → Metal texture example
3. **Test**: Verify IOSurface can be rendered correctly

### Short-term (Week 2-3)

4. **Implement**: Add hardware rendering path to renderer
5. **Integrate**: Connect PlaybackController → Renderer with HW frames
6. **Benchmark**: Measure performance improvement

### Long-term (Week 4+)

7. **Polish**: Handle edge cases, error conditions
8. **Document**: Update architecture docs, add examples
9. **Test**: Comprehensive testing across different videos/formats

---

## Contact & Handoff

**Current State**: Hardware decoding fully functional, renderer needs IOSurface support

**Key Files to Review**:
1. `vp_core/src/decode/hardware_decoder.rs` - How hardware frames are created
2. `vp_core/src/decode/mod.rs` - VideoFrame enum definition
3. `player/src/renderer/wgpu_renderer.rs` - Current renderer to modify

**Questions?** Review the benchmark output and test files for working examples.

**Good luck!** 🚀

---

*Last Updated: 2025-10-10*  
*Status: Ready for renderer optimization work*
