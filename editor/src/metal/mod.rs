//! Metal rendering infrastructure for hardware-accelerated video playback.
//!
//! This module provides zero-copy video rendering using Metal and CAMetalLayer,
//! overlaying the video on top of the eframe/egui application.

#![cfg(target_os = "macos")]

mod layer_manager;
mod video_renderer;

pub use layer_manager::{LayerManager, LayerConfig};
pub use video_renderer::VideoRenderer;

use objc2::rc::Retained;
use objc2_metal::{
    MTLDevice, MTLCommandQueue, MTLPixelFormat,
    MTLCreateSystemDefaultDevice,
};
use std::error::Error;
use std::fmt;

/// Metal rendering context holding device and command queue.
pub struct MetalContext {
    device: Retained<objc2::runtime::ProtocolObject<dyn MTLDevice>>,
    command_queue: Retained<objc2::runtime::ProtocolObject<dyn MTLCommandQueue>>,
}

impl MetalContext {
    /// Creates a new Metal context with the default GPU device.
    pub fn new() -> Result<Self, MetalError> {
        // Get the default Metal device (primary GPU)
        let device = unsafe { MTLCreateSystemDefaultDevice() }
            .ok_or(MetalError::NoDevice)?;

        // Create command queue for submitting rendering commands
        let command_queue = device
            .newCommandQueue()
            .ok_or(MetalError::CommandQueueCreation)?;

        tracing::info!(
            "Metal context initialized with device: {}",
            device.name()
        );

        Ok(Self {
            device,
            command_queue,
        })
    }

    /// Returns the Metal device.
    pub fn device(&self) -> &Retained<objc2::runtime::ProtocolObject<dyn MTLDevice>> {
        &self.device
    }

    /// Returns the command queue.
    pub fn command_queue(&self) -> &Retained<objc2::runtime::ProtocolObject<dyn MTLCommandQueue>> {
        &self.command_queue
    }
}

/// Errors that can occur during Metal initialization and rendering.
#[derive(Debug)]
pub enum MetalError {
    /// No Metal-capable GPU found on the system.
    NoDevice,
    /// Failed to create command queue.
    CommandQueueCreation,
    /// Failed to create Metal layer.
    LayerCreation,
    /// Failed to get NSView from window.
    NoView,
    /// Window handle is not a macOS AppKit window.
    InvalidWindowHandle,
}

impl fmt::Display for MetalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetalError::NoDevice => write!(f, "No Metal-capable GPU device found"),
            MetalError::CommandQueueCreation => write!(f, "Failed to create Metal command queue"),
            MetalError::LayerCreation => write!(f, "Failed to create CAMetalLayer"),
            MetalError::NoView => write!(f, "Failed to get NSView from window"),
            MetalError::InvalidWindowHandle => write!(f, "Window handle is not a macOS AppKit window"),
        }
    }
}

impl Error for MetalError {}
