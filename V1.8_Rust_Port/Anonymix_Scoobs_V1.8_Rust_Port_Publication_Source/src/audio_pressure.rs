//! Runtime audio-pressure signal consumed by adaptive visual scheduling.
//! This is not a log/report collector: only the count needed by the original
//! visual budget survives. It cannot change DSP samples or open audio devices.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Independent callback-to-UI pressure counter, matching the development predicate.
#[derive(Default)]
pub struct AudioPressure {
    events: AtomicU64,
}
impl AudioPressure {
    /// Signal at most one event for a qualifying live host callback if it missed
    /// its processing budget or PortAudio reported an input/output discontinuity.
    /// Frames are mono frame counts, duration is microseconds, and rate is Hz.
    pub fn observe(&self, frames: u64, micros: u64, rate: f64, overflow: bool, underflow: bool) {
        let late = rate > 0.0 && micros as f64 * rate > frames as f64 * 1_000_000.0;
        if late || overflow || underflow {
            self.events.fetch_add(1, Relaxed);
        }
    }
    /// Consume the independent pressure window on the visual thread.
    pub fn take_visual_pressure_events(&self) -> u64 {
        self.events.swap(0, Relaxed)
    }
}
