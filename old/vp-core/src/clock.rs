//! Clock module for unified time tracking
//!
//! Provides a Clock trait and implementations for timing control.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::frame::Microseconds;

/// Unified clock trait for all timing components
pub trait Clock: Send + Sync {
    /// Get current time in microseconds
    fn time(&self) -> Microseconds;

    /// Set the current time
    fn set_time(&mut self, time: Microseconds);

    /// Pause the clock
    fn pause(&mut self);

    /// Resume the clock
    fn resume(&mut self);

    /// Check if the clock is paused
    fn is_paused(&self) -> bool;
}

/// Audio-driven clock that tracks actual audio playback position
///
/// This clock is updated by the audio callback thread and serves as the
/// master clock for A/V synchronization. It uses atomic operations for
/// lock-free updates from the audio thread.
#[derive(Clone)]
pub struct AudioClock {
    /// Current audio PTS stored as f64 bits for atomic access
    current_pts: Arc<AtomicI64>,
    /// Pause state
    paused: Arc<AtomicBool>,
}

impl AudioClock {
    /// Create a new audio clock
    pub fn new() -> Self {
        Self {
            current_pts: Arc::new(AtomicI64::new(0)),
            paused: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn update_pts(&self, pts: Microseconds) {
        self.current_pts.store(pts.0, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

impl Clock for AudioClock {
    fn time(&self) -> Microseconds {
        Microseconds(self.current_pts.load(Ordering::Relaxed))
    }

    fn set_time(&mut self, time: Microseconds) {
        self.current_pts.store(time.0, Ordering::Relaxed);
    }

    fn pause(&mut self) {
        tracing::debug!("AudioClock: pausing");
        self.paused.store(true, Ordering::Relaxed);
    }

    fn resume(&mut self) {
        tracing::debug!("AudioClock: resuming");
        self.paused.store(false, Ordering::Relaxed);
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// System clock implementation using real time
#[derive(Clone)]
pub struct SystemClock {
    inner: Arc<Mutex<SystemClockInner>>,
}

struct SystemClockInner {
    start_time: Instant,
    paused_time: Option<Instant>,
    offset: i64,
}

impl SystemClock {
    /// Create a new system clock
    pub fn new() -> Self {
        let start_time = Instant::now();
        Self {
            inner: Arc::new(Mutex::new(SystemClockInner {
                start_time,
                paused_time: Some(start_time),
                offset: 0,
            })),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn time(&self) -> Microseconds {
        let inner = self.inner.lock().unwrap();
        if let Some(paused) = inner.paused_time {
            let elapsed_us = paused.duration_since(inner.start_time).as_micros() as i64;
            Microseconds(inner.offset + elapsed_us)
        } else {
            let elapsed_us = Instant::now()
                .duration_since(inner.start_time)
                .as_micros() as i64;
            Microseconds(inner.offset + elapsed_us)
        }
    }

    fn set_time(&mut self, time: Microseconds) {
        let mut inner = self.inner.lock().unwrap();
        inner.offset = time.0;
        inner.start_time = Instant::now();
        if inner.paused_time.is_some() {
            inner.paused_time = Some(Instant::now());
        }
    }

    fn pause(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.paused_time.is_none() {
            inner.paused_time = Some(Instant::now());
        }
    }

    fn resume(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(paused) = inner.paused_time {
            inner.offset += paused.duration_since(inner.start_time).as_micros() as i64;
            inner.start_time = Instant::now();
            inner.paused_time = None;
        }
    }

    fn is_paused(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.paused_time.is_some()
    }
}
