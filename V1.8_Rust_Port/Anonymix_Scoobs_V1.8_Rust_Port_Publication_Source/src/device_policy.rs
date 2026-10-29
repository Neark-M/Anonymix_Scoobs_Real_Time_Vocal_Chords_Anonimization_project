//! Endpoint/cache/reopen decisions copied from V1.8 Audio.cpp and App.cpp.
//! These functions do not query Windows, call PortAudio or touch callback data.
use crate::device_plan::RequestedMode;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Opaque Windows capture/render endpoint identifiers, kept as UTF-16 code units.
pub struct EndpointIds {
    pub capture: Option<Vec<u16>>,
    pub render: Option<Vec<u16>>,
}
impl EndpointIds {
    /// Unknown or empty IDs force the same conservative full refresh as C++.
    pub fn needs_refresh(&self, current: &Self) -> bool {
        current.capture.as_ref().is_none_or(|id| id.is_empty())
            || current.render.as_ref().is_none_or(|id| id.is_empty())
            || self != current
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Identity of a reusable Shared preview output. Index alone is insufficient after hot-plug.
pub struct PreviewCacheKey {
    pub device: i32,
    pub endpoint: Vec<u16>,
    pub channels: i32,
}
impl PreviewCacheKey {
    /// Compare the supplied identity against the stored selection; unknown endpoints cannot reuse a cache.
    pub fn matches(&self, device: i32, endpoint: Option<&[u16]>, channels: i32) -> bool {
        device >= 0
            && self.device == device
            && self.channels == channels
            && endpoint.is_some_and(|id| !id.is_empty() && id == self.endpoint)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The mode that actually opened, including unavailable and non-WASAPI fallback states.
pub enum ActualMode {
    Unavailable,
    OtherHost,
    Shared,
    Exclusive,
}
impl ActualMode {
    /// Check whether the opened mode satisfies this request; fallback hosts do not match.
    pub fn matches(self, requested: RequestedMode) -> bool {
        matches!(
            (self, requested),
            (Self::Shared, RequestedMode::Shared) | (Self::Exclusive, RequestedMode::Exclusive)
        )
    }
}

#[derive(Debug, Clone)]
/// Copied device names, channel counts, sample rate and requested/reported latencies in seconds.
pub struct StreamSelection {
    pub label: String,
    pub input_name: String,
    pub output_name: String,
    pub input_channels: i32,
    pub output_channels: i32,
    pub input_requested: f64,
    pub output_requested: f64,
    pub input_reported: f64,
    pub output_reported: f64,
    pub sample_rate: f64,
}
impl StreamSelection {
    /// Translate the opened stream label to the controller's mode enum.
    pub fn actual_mode(&self) -> ActualMode {
        match self.label.as_str() {
            "WASAPI Exclusive" => ActualMode::Exclusive,
            "WASAPI Shared" => ActualMode::Shared,
            "PortAudio defaults" => ActualMode::OtherHost,
            _ => ActualMode::Unavailable,
        }
    }
}

pub const REOPEN_QUIET_NS: u64 = 500_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Main-thread reopen lifecycle: idle, owner working, or waiting for a quiet follow-up interval.
pub enum ReopenPhase {
    Idle,
    Working,
    Settling,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// One owner-thread reopen operation, with an optional full device enumeration refresh.
pub struct ReopenRequest {
    pub mode: RequestedMode,
    pub refresh_devices: bool,
}

/// Main/UI-thread intent only. A running request completes before a follow-up
/// starts; mode clicks are combined after the inherited 500 ms quiet interval.
pub struct ReopenCoordinator {
    phase: ReopenPhase,
    requested: RequestedMode,
    actual: ActualMode,
    deferred_mode: Option<RequestedMode>,
    deferred_refresh: bool,
    deadline_ns: u64,
}
impl ReopenCoordinator {
    /// Start with the selected request and last opened mode, with no deferred operation.
    pub fn new(requested: RequestedMode, actual: ActualMode) -> Self {
        Self {
            phase: ReopenPhase::Idle,
            requested,
            actual,
            deferred_mode: None,
            deferred_refresh: false,
            deadline_ns: 0,
        }
    }
    /// Return the request associated with the current or completed native operation.
    pub fn requested_mode(&self) -> RequestedMode {
        self.requested
    }
    /// SDL toggles the most recent deferred intent during a running reopen.
    pub fn desired_mode(&self) -> RequestedMode {
        self.deferred_mode.unwrap_or(self.requested)
    }
    /// Read whether a reopen is idle, running, or settling.
    pub fn phase(&self) -> ReopenPhase {
        self.phase
    }
    /// Start an unblocked request or retain only the latest mode while another reopen is active.
    /// `now` is monotonic nanoseconds; a recording blocks a new idle request.
    pub fn request_mode(
        &mut self,
        mode: RequestedMode,
        now: u64,
        blocked: bool,
    ) -> Option<ReopenRequest> {
        if blocked && self.phase == ReopenPhase::Idle {
            return None;
        }
        if self.phase != ReopenPhase::Idle {
            self.deferred_mode = Some(mode);
            self.deadline_ns = now.saturating_add(REOPEN_QUIET_NS);
            return None;
        }
        self.requested = mode;
        self.phase = ReopenPhase::Working;
        Some(ReopenRequest {
            mode,
            refresh_devices: false,
        })
    }
    /// Request device rediscovery; ignore duplicate clicks while the owner is already reopening.
    pub fn request_refresh(&mut self, now: u64, blocked: bool) -> Option<ReopenRequest> {
        match self.phase {
            ReopenPhase::Working => None, // Duplicate refresh click is ignored.
            ReopenPhase::Settling => {
                self.deferred_refresh = true;
                self.deadline_ns = now.saturating_add(REOPEN_QUIET_NS);
                None
            }
            ReopenPhase::Idle if blocked => None,
            ReopenPhase::Idle => {
                self.phase = ReopenPhase::Working;
                Some(ReopenRequest {
                    mode: self.requested,
                    refresh_devices: true,
                })
            }
        }
    }
    /// The owner result supplies both actual mode and any fallback-updated
    /// requested mode. No follow-up is opened before this result is consumed.
    pub fn complete(&mut self, requested: RequestedMode, actual: ActualMode, now: u64) {
        self.requested = requested;
        self.actual = actual;
        if self.deferred_refresh || self.deferred_mode.is_some_and(|mode| !actual.matches(mode)) {
            self.phase = ReopenPhase::Settling;
            self.deadline_ns = now.saturating_add(REOPEN_QUIET_NS);
        } else {
            self.phase = ReopenPhase::Idle;
            self.deferred_mode = None;
            self.deferred_refresh = false;
        }
    }
    /// After the 500-ms quiet interval, return one necessary follow-up operation, if any.
    pub fn tick(&mut self, now: u64) -> Option<ReopenRequest> {
        if self.phase != ReopenPhase::Settling || now < self.deadline_ns {
            return None;
        }
        let mode = self.deferred_mode.take().unwrap_or(self.requested);
        let refresh = std::mem::take(&mut self.deferred_refresh);
        self.phase = ReopenPhase::Idle;
        if refresh || !self.actual.matches(mode) {
            self.requested = mode;
            self.phase = ReopenPhase::Working;
            Some(ReopenRequest {
                mode,
                refresh_devices: refresh,
            })
        } else {
            None
        }
    }
}
