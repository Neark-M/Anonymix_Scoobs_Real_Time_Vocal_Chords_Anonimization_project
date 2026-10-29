//! V1.8 visual deadline arithmetic and CPU-observed frame-pressure classification.
//! The native SDL loop supplies monotonic timestamps and performs its existing
//! precise wait. No sleeping, audio processing or worker thread lives here.
use crate::ui_budget::FRAME_RATES;

#[derive(Clone, Copy, Debug, Default)]
/// Monotonic nanoseconds spent in CPU draw phases; presentation waits are excluded.
pub struct RenderFrameTiming {
    pub controls_ns: u64,
    pub spectrum_ns: u64,
    pub waveform_ns: u64,
    pub composite_ns: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Frame-load classification used by adaptive rendering. Present-only waits do not imply FFT overload.
pub struct FramePressure {
    pub slow_app: bool,
    pub fft_heavy: bool,
}
impl FramePressure {
    /// Compare update/draw phase durations with the current visual frame period.
    pub fn classify(update_ns: u64, timing: RenderFrameTiming, target_ns: u64) -> Self {
        let draw =
            timing.controls_ns + timing.spectrum_ns + timing.waveform_ns + timing.composite_ns;
        Self {
            slow_app: update_ns + draw > target_ns,
            fft_heavy: timing.spectrum_ns >= target_ns / 2
                && timing.spectrum_ns >= timing.controls_ns
                && timing.spectrum_ns >= timing.waveform_ns
                && timing.spectrum_ns >= timing.composite_ns,
        }
    }
}
#[derive(Clone, Copy, Debug)]
/// Elapsed frame time used by animation and display smoothing.
pub struct FrameStart {
    pub elapsed_seconds: f32,
}
#[derive(Default)]
/// Main-thread monotonic deadline state. Missing a deadline skips expired slots rather than catching up.
pub struct DeadlinePacer {
    next_deadline_ns: u64,
    previous_started_ns: Option<u64>,
}
impl DeadlinePacer {
    /// Return the integer nanosecond period for one supported shared visual cadence.
    pub fn period_ns(fps: u32) -> u64 {
        assert!(FRAME_RATES.contains(&fps));
        1_000_000_000 / fps as u64
    }
    /// Read the current scheduled finish deadline without advancing it.
    pub fn next_deadline_ns(&self) -> u64 {
        self.next_deadline_ns
    }
    /// Same start-of-frame ordering as SDL_AppIterate. Stalls longer than three
    /// additional slots rebase the timeline; ordinary scheduler jitter does not.
    pub fn begin(&mut self, started_ns: u64, fps: u32) -> FrameStart {
        let target = Self::period_ns(fps);
        if self.next_deadline_ns == 0 || started_ns > self.next_deadline_ns + target * 3 {
            self.next_deadline_ns = started_ns + target;
        }
        let interval = self
            .previous_started_ns
            .map(|previous| started_ns - previous);
        let elapsed = interval.map_or(1.0 / 60.0, |ns| ns as f32 / 1_000_000_000.0);
        self.previous_started_ns = Some(started_ns);
        FrameStart {
            elapsed_seconds: elapsed,
        }
    }
    /// Only a changed ten-second visual budget resets the current deadline.
    pub fn budget_changed(&mut self, frame_started_ns: u64, fps: u32) {
        self.next_deadline_ns = frame_started_ns + Self::period_ns(fps);
    }
    /// Return only the remaining wait; an already missed deadline never produces a negative duration.
    pub fn wait_ns(&self, after_render_ns: u64) -> u64 {
        self.next_deadline_ns.saturating_sub(after_render_ns)
    }
    /// `finished_ns` includes the native precise wait, when one was needed.
    /// Skip expired slots rather than rendering catch-up frames or FFT requests.
    pub fn finish(&mut self, _after_render_ns: u64, finished_ns: u64, fps: u32) {
        let target = Self::period_ns(fps);
        if finished_ns < self.next_deadline_ns {
            self.next_deadline_ns += target;
        } else {
            let missed = (finished_ns - self.next_deadline_ns) / target + 1;
            self.next_deadline_ns += missed * target;
        }
    }
}
