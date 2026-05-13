//! Hardware acceleration support for video decoding via FFmpeg
//!
//! This module provides safe wrappers around FFmpeg's hardware device context API
//! to enable VideoToolbox acceleration on macOS.

use ffmpeg_sys_next::{
    av_buffer_unref, av_hwdevice_ctx_create, av_hwframe_ctx_alloc, av_hwframe_ctx_init,
    AVBufferRef, AVHWDeviceType, AVHWFramesContext, AVPixelFormat,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HwAccelError {
    #[error("Hardware device context creation failed: {0}")]
    DeviceContextCreation(i32),
    #[error("Hardware frames context allocation failed")]
    FramesContextAllocation,
    #[error("Hardware frames context initialization failed: {0}")]
    FramesContextInit(i32),
}

/// A safe wrapper for an FFmpeg `AVBufferRef` representing a hardware device context.
///
/// This struct ensures that `av_buffer_unref` is called when the context is no longer needed,
/// preventing memory leaks.
pub struct HardwareDeviceContext {
    pub(crate) ptr: *mut AVBufferRef,
}

impl HardwareDeviceContext {
    /// Creates a new hardware device context of the specified type.
    pub fn new(r#type: AVHWDeviceType) -> Result<Self, HwAccelError> {
        let mut hw_device_ctx: *mut AVBufferRef = std::ptr::null_mut();
        let ret = unsafe {
            av_hwdevice_ctx_create(
                &mut hw_device_ctx,
                r#type,
                std::ptr::null(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ret < 0 {
            return Err(HwAccelError::DeviceContextCreation(ret));
        }
        Ok(HardwareDeviceContext { ptr: hw_device_ctx })
    }

    /// Consumes the `HardwareDeviceContext`, returning the raw `AVBufferRef` pointer.
    ///
    /// # Safety
    /// The caller is responsible for managing the memory of the returned pointer,
    /// typically by calling `av_buffer_unref`.
    pub unsafe fn into_raw(self) -> *mut AVBufferRef {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }
}

impl Drop for HardwareDeviceContext {
    fn drop(&mut self) {
        unsafe {
            av_buffer_unref(&mut self.ptr);
        }
    }
}

// The underlying pointer is thread-safe.
unsafe impl Send for HardwareDeviceContext {}
unsafe impl Sync for HardwareDeviceContext {}

/// A builder for an FFmpeg `AVBufferRef` representing a hardware frames context.
///
/// This builder simplifies the creation and configuration of a `HardwareFramesContext`.
pub struct HardwareFramesBuilder {
    pub(crate) ptr: *mut AVBufferRef,
}

impl HardwareFramesBuilder {
    /// Creates a new `HardwareFrameBuilder` from a given `HardwareDeviceContext`.
    pub fn new(device_ctx: &HardwareDeviceContext) -> Result<Self, HwAccelError> {
        let hw_frames_ctx = unsafe { av_hwframe_ctx_alloc(device_ctx.ptr) };
        if hw_frames_ctx.is_null() {
            return Err(HwAccelError::FramesContextAllocation);
        }
        Ok(HardwareFramesBuilder { ptr: hw_frames_ctx })
    }

    /// Returns a raw pointer to the `AVHWFramesContext`.
    ///
    /// # Safety
    /// The caller must ensure that the pointer is used correctly and does not outlive the builder.
    unsafe fn frames_ctx(&self) -> *mut AVHWFramesContext {
        (*self.ptr).data as *mut AVHWFramesContext
    }

    /// Consumes the builder, returning the raw `AVBufferRef` pointer.
    ///
    /// # Safety
    /// The caller is responsible for managing the memory of the returned pointer.
    unsafe fn into_raw(self) -> *mut AVBufferRef {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }

    /// Sets the pixel format for the hardware frames.
    pub fn set_format(&mut self, format: AVPixelFormat) -> &mut Self {
        unsafe {
            (*self.frames_ctx()).format = format;
        }
        self
    }

    /// Sets the software pixel format for the hardware frames.
    pub fn set_sw_format(&mut self, format: AVPixelFormat) -> &mut Self {
        unsafe {
            (*self.frames_ctx()).sw_format = format;
        }
        self
    }

    /// Sets the resolution for the hardware frames.
    pub fn set_resolution(&mut self, width: i32, height: i32) -> &mut Self {
        unsafe {
            (*self.frames_ctx()).width = width;
            (*self.frames_ctx()).height = height;
        }
        self
    }

    /// Initializes the hardware frames context and returns a `HardwareFramesContext` consuming the builder.
    pub fn init(self) -> Result<HardwareFramesContext, HwAccelError> {
        unsafe { HardwareFramesContext::new(self.into_raw()) }
    }
}

impl Drop for HardwareFramesBuilder {
    fn drop(&mut self) {
        unsafe {
            av_buffer_unref(&mut self.ptr);
        }
    }
}

/// A safe wrapper for an FFmpeg `AVBufferRef` representing a hardware frames context.
pub struct HardwareFramesContext {
    pub(crate) ptr: *mut AVBufferRef,
}

impl HardwareFramesContext {
    /// Creates a new `HardwareFramesContext` from a raw `AVBufferRef` pointer.
    ///
    /// # Safety
    /// The caller must ensure that the provided pointer is a valid, uninitialized `AVHWFramesContext`.
    /// This function will call `av_hwframe_ctx_init` on the pointer.
    pub fn new(ptr: *mut AVBufferRef) -> Result<Self, HwAccelError> {
        let ret = unsafe { av_hwframe_ctx_init(ptr) };
        if ret < 0 {
            return Err(HwAccelError::FramesContextInit(ret));
        }
        Ok(HardwareFramesContext { ptr })
    }

    /// Consumes the `HardwareFramesContext`, returning the raw `AVBufferRef` pointer.
    ///
    /// # Safety
    /// The caller is responsible for managing the memory of the returned pointer,
    /// typically by calling `av_buffer_unref`.
    pub unsafe fn into_raw(self) -> *mut AVBufferRef {
        let ptr = self.ptr;
        std::mem::forget(self);
        ptr
    }
}

impl Drop for HardwareFramesContext {
    fn drop(&mut self) {
        unsafe {
            av_buffer_unref(&mut self.ptr);
        }
    }
}

/// Extension trait for ffmpeg::decoder::Video to add hardware acceleration support
pub trait VideoDecoderExt {
    /// Sets the hardware device context
    fn with_hw_device_ctx(self, ctx: HardwareDeviceContext) -> Self;

    /// Sets the hardware frames context
    fn with_hw_frames_ctx(self, ctx: HardwareFramesContext) -> Self;

    /// Sets the pixel format
    fn with_pix_fmt(self, pix_fmt: AVPixelFormat) -> Self;

    /// Sets up get_format callback for hardware format selection
    fn with_hw_format_callback(self, format: AVPixelFormat) -> Self;
}

impl VideoDecoderExt for ffmpeg_next::decoder::Video {
    fn with_hw_device_ctx(mut self, ctx: HardwareDeviceContext) -> Self {
        unsafe {
            (*self.as_mut_ptr()).hw_device_ctx = ctx.into_raw();
        }
        self
    }

    fn with_hw_frames_ctx(mut self, ctx: HardwareFramesContext) -> Self {
        unsafe {
            (*self.as_mut_ptr()).hw_frames_ctx = ctx.into_raw();
        }
        self
    }

    fn with_pix_fmt(mut self, pix_fmt: AVPixelFormat) -> Self {
        unsafe {
            (*self.as_mut_ptr()).pix_fmt = pix_fmt;
        }
        self
    }

    fn with_hw_format_callback(mut self, _format: AVPixelFormat) -> Self {
        unsafe {
            // Set up get_format callback to return the hardware format
            extern "C" fn get_hw_format(
                _ctx: *mut ffmpeg_sys_next::AVCodecContext,
                fmt: *const AVPixelFormat,
            ) -> AVPixelFormat {
                // Return the first format in the list (should be the hardware format)
                unsafe {
                    if !fmt.is_null() {
                        *fmt
                    } else {
                        ffmpeg_sys_next::AVPixelFormat::AV_PIX_FMT_NONE
                    }
                }
            }

            (*self.as_mut_ptr()).get_format = Some(get_hw_format);
        }
        self
    }
}
