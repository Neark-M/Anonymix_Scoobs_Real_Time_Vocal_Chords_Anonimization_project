//! SDL-thread device intent and finite button states from App.cpp/AppUpdate.cpp.
//! This model plans work only: COM/device calls are always performed by the
//! existing device owner. It never touches native callback DSP buffers.
use crate::{
    device_plan::RequestedMode,
    device_policy::{ActualMode, ReopenCoordinator, ReopenPhase},
    device_worker::{ActionKind, DeviceAction, DeviceEvent, DeviceSnapshot},
    ui_widgets::{UiAction, UiModel},
};

/// Main-thread action gates and captions driven by copied device results; owns no native stream.
pub struct UiController {
    pub last: Option<DeviceSnapshot>,
    pub reopen: ReopenCoordinator,
    pub startup: bool,
    pub pending: Option<ActionKind>,
    pub last_error: Option<String>,
}
impl Default for UiController {
    fn default() -> Self {
        Self {
            last: None,
            reopen: ReopenCoordinator::new(RequestedMode::Shared, ActualMode::Unavailable),
            startup: true,
            pending: None,
            last_error: None,
        }
    }
}
fn appearance(ui: &mut UiModel, name: &str, text: &str, rgb: [f32; 3]) {
    let w = ui.widget_mut(name);
    w.text = text.into();
    w.set_color(rgb);
}
impl UiController {
    /// Read the last acknowledged Stream/Recording mode.
    pub fn is_live(&self) -> bool {
        self.last.as_ref().is_some_and(|s| s.live)
    }
    /// Report whether native reopen or its deferred follow-up is gating the UI.
    pub fn reopening(&self) -> bool {
        self.reopen.phase() != ReopenPhase::Idle
    }
    fn captions(&self, ui: &mut UiModel) {
        if let Some(s) = &self.last {
            appearance(
                ui,
                "AudioMode",
                if s.live {
                    "Mode: Stream"
                } else {
                    "Mode: Recording"
                },
                if s.live {
                    [0.3, 1., 0.3]
                } else {
                    [0.8, 0.8, 0.8]
                },
            );
            let (label, color) = match s.actual {
                ActualMode::Exclusive => ("WASAPI: Excl.", [0.95, 0.85, 0.95]),
                ActualMode::Shared => ("WASAPI: Shared", [0.60, 0.85, 0.95]),
                ActualMode::OtherHost => ("Other Host", [0.60, 0.85, 0.95]),
                ActualMode::Unavailable => ("No Audio", [1., 0.85, 0.95]),
            };
            appearance(ui, "WasapiMode", label, color);
            appearance(
                ui,
                "PA_Record",
                if s.recording { "Stop Rec" } else { "Start Rec" },
                if s.recording {
                    [1., 0.3, 0.3]
                } else {
                    [1., 0.55, 0.55]
                },
            );
            appearance(
                ui,
                "PA_PlaySelected",
                if s.playing { "Stop" } else { "Play" },
                if s.playing {
                    [1., 0.55, 0.45]
                } else {
                    [0.9, 0.9, 0.55]
                },
            );
        }
    }
    /// Consume one owner result and update transitions, captions and effect availability.
    pub fn event(&mut self, event: DeviceEvent, now: u64, ui: &mut UiModel) {
        match event {
            DeviceEvent::Started(result) => {
                self.startup = false;
                self.pending = None;
                match result {
                    Ok(s) => {
                        self.reopen.complete(s.requested, s.actual, now);
                        self.last = Some(s);
                        self.last_error = None;
                    }
                    Err(e) => {
                        self.last_error = Some(e);
                        appearance(ui, "WasapiMode", "No Audio", [1., 0.85, 0.95]);
                    }
                }
                self.captions(ui);
            }
            DeviceEvent::Completed {
                action,
                result,
                state,
            } => {
                self.pending = None;
                let snapshot = match result {
                    Ok(s) => {
                        self.last_error = None;
                        Some(s)
                    }
                    Err(e) => {
                        self.last_error = Some(e);
                        state
                    }
                };
                if let Some(s) = snapshot {
                    if action == ActionKind::Reopen || self.reopen.phase() == ReopenPhase::Idle {
                        self.reopen.complete(s.requested, s.actual, now);
                    }
                    self.last = Some(s);
                } else if action == ActionKind::Reopen {
                    self.reopen.complete(
                        self.reopen.requested_mode(),
                        ActualMode::Unavailable,
                        now,
                    );
                }
                self.captions(ui);
                if action == ActionKind::Reopen {
                    appearance(
                        ui,
                        "AudioDevices",
                        "Reopen Audio",
                        if self.last_error.is_none() {
                            [0.55, 0.85, 1.]
                        } else {
                            [1., 0.85, 1.]
                        },
                    );
                }
                if action == ActionKind::Save && self.last_error.is_some() {
                    appearance(ui, "PA_SaveSelected", "Save WAV", [1., 0.88, 0.85]);
                }
                if self.reopening() {
                    appearance(ui, "AudioDevices", "Reopening...", [1., 0.85, 0.45]);
                    if self
                        .last
                        .as_ref()
                        .is_some_and(|s| !s.actual.matches(self.reopen.desired_mode()))
                    {
                        appearance(ui, "WasapiMode", "Switching...", [1., 0.85, 0.45]);
                    }
                }
            }
            DeviceEvent::Saved(result) => {
                let okay = result.is_ok();
                if let Err(e) = result {
                    self.last_error = Some(e);
                }
                if let Some(s) = &mut self.last {
                    s.export_busy = false;
                }
                appearance(
                    ui,
                    "PA_SaveSelected",
                    "Save WAV",
                    if okay {
                        [0.35, 0.88, 0.85]
                    } else {
                        [1., 0.88, 0.85]
                    },
                );
            }
            DeviceEvent::Stopped(result) => {
                if let Err(e) = result {
                    self.last_error = Some(e);
                }
            }
        }
    }
    fn planned(&mut self, action: DeviceAction, ui: &mut UiModel) -> Option<DeviceAction> {
        self.pending = Some(action.kind());
        let amber = [1., 0.85, 0.45];
        match action {
            DeviceAction::RecordStart => appearance(ui, "PA_Record", "Starting...", amber),
            DeviceAction::RecordStop => appearance(ui, "PA_Record", "Stopping...", amber),
            DeviceAction::Play(_) => appearance(ui, "PA_PlaySelected", "Starting...", amber),
            DeviceAction::StopPreview => appearance(ui, "PA_PlaySelected", "Stopping...", amber),
            DeviceAction::Save(_) => appearance(ui, "PA_SaveSelected", "Saving...", amber),
            DeviceAction::SetLive(_) => appearance(ui, "AudioMode", "Switching...", amber),
            DeviceAction::Reopen(_) => {
                appearance(ui, "AudioDevices", "Reopening...", amber);
                appearance(ui, "PA_PlaySelected", "Play", [0.9, 0.9, 0.55]);
            }
            _ => {}
        }
        Some(action)
    }
    /// Translate a released button into at most one permitted owner action; selectors publish locally.
    pub fn plan(
        &mut self,
        action: UiAction,
        now: u64,
        worker_pending: bool,
        ui: &mut UiModel,
    ) -> Option<DeviceAction> {
        if let UiAction::Effect(bit) = action {
            if !self.reopening() {
                ui.toggle_effect(bit);
            }
            return None;
        }
        if self.startup {
            return None;
        }
        let record = self.last.as_ref().is_some_and(|s| s.recording);
        if matches!(action, UiAction::Reopen | UiAction::WasapiMode) {
            if worker_pending && !self.reopening() {
                return None;
            }
            let request = if action == UiAction::Reopen {
                self.reopen.request_refresh(now, record)
            } else {
                let current = if self.reopening() {
                    self.reopen.desired_mode()
                } else if self
                    .last
                    .as_ref()
                    .is_some_and(|s| s.actual == ActualMode::Exclusive)
                {
                    RequestedMode::Exclusive
                } else {
                    RequestedMode::Shared
                };
                let next = if current == RequestedMode::Exclusive {
                    RequestedMode::Shared
                } else {
                    RequestedMode::Exclusive
                };
                let request = self.reopen.request_mode(next, now, record);
                if self.reopening() {
                    appearance(ui, "WasapiMode", "Switching...", [1., 0.85, 0.45]);
                }
                request
            };
            return request.and_then(|r| self.planned(DeviceAction::Reopen(r), ui));
        }
        if self.reopening() || worker_pending {
            return None;
        }
        let stream = self.is_live();
        let settings = ui.settings();
        let command = match action {
            UiAction::AudioMode => DeviceAction::SetLive(!stream),
            UiAction::Record if !stream => {
                if record {
                    DeviceAction::RecordStop
                } else {
                    DeviceAction::RecordStart
                }
            }
            UiAction::Play if !stream => {
                if self.last.as_ref().is_some_and(|s| s.playing) {
                    DeviceAction::StopPreview
                } else {
                    DeviceAction::Play(settings)
                }
            }
            UiAction::Save if !stream && !self.last.as_ref().is_some_and(|s| s.export_busy) => {
                DeviceAction::Save(settings)
            }
            _ => return None,
        };
        self.planned(command, ui)
    }
    /// Return a deferred reopen after the original quiet interval, provided no other action is pending.
    pub fn settled(
        &mut self,
        now: u64,
        worker_pending: bool,
        ui: &mut UiModel,
    ) -> Option<DeviceAction> {
        if worker_pending {
            return None;
        }
        self.reopen
            .tick(now)
            .and_then(|r| self.planned(DeviceAction::Reopen(r), ui))
    }
    /// The callback clears its shared completion flag. Closing/aborting a Shared
    /// preview handle still happens on the device owner before Play is restored.
    pub fn completed_preview(
        &mut self,
        playing: bool,
        worker_pending: bool,
        ui: &mut UiModel,
    ) -> Option<DeviceAction> {
        if !worker_pending
            && !self.reopening()
            && self.last.as_ref().is_some_and(|s| s.playing)
            && !playing
        {
            self.planned(DeviceAction::StopPreview, ui)
        } else {
            None
        }
    }
    /// Undo an unaccepted transition and retain its error without changing callback ownership.
    pub fn submission_failed(&mut self, error: String, now: u64, ui: &mut UiModel) {
        if self.pending == Some(ActionKind::Reopen) {
            self.reopen
                .complete(self.reopen.requested_mode(), ActualMode::Unavailable, now);
            appearance(ui, "AudioDevices", "Reopen Audio", [1., 0.85, 1.]);
        }
        self.pending = None;
        self.last_error = Some(error);
        self.captions(ui);
    }
}
