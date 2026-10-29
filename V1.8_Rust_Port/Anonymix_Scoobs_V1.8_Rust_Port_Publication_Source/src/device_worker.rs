//! The long-lived device owner from App.cpp, independent of SDL widgets.
//! Commands/results are application traffic, never audio sample transport.
use crate::{
    device_plan::RequestedMode,
    device_policy::{ActualMode, ReopenRequest, StreamSelection},
    observation::ObservationHub,
    Settings,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone)]
/// Small application commands sent to the device owner. Audio samples never travel in this channel.
pub enum DeviceAction {
    Status,
    RecordStart,
    RecordStop,
    SetLive(bool),
    SetSettings(Settings),
    Play(Settings),
    StopPreview,
    Save(Settings),
    Reopen(ReopenRequest),
    Shutdown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Payload-free action identifier used to match owner results to UI transitions.
pub enum ActionKind {
    Status,
    RecordStart,
    RecordStop,
    SetLive,
    SetSettings,
    Play,
    StopPreview,
    Save,
    Reopen,
    Shutdown,
}
impl DeviceAction {
    /// Identify a command without copying its settings or reopen payload.
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::Status => ActionKind::Status,
            Self::RecordStart => ActionKind::RecordStart,
            Self::RecordStop => ActionKind::RecordStop,
            Self::SetLive(_) => ActionKind::SetLive,
            Self::SetSettings(_) => ActionKind::SetSettings,
            Self::Play(_) => ActionKind::Play,
            Self::StopPreview => ActionKind::StopPreview,
            Self::Save(_) => ActionKind::Save,
            Self::Reopen(_) => ActionKind::Reopen,
            Self::Shutdown => ActionKind::Shutdown,
        }
    }
}
#[derive(Clone, Debug)]
/// Copied owner state for UI decisions; it contains no native handle or borrowed callback data.
pub struct DeviceSnapshot {
    pub ready: bool,
    pub active: bool,
    pub live: bool,
    pub recording: bool,
    pub playing: bool,
    pub export_busy: bool,
    pub frames: usize,
    pub incomplete: bool,
    pub requested: RequestedMode,
    pub actual: ActualMode,
    pub selection: Option<StreamSelection>,
}
/// Asynchronous owner results. Export completion is separate from accepting a Save action.
pub enum DeviceEvent {
    Started(Result<DeviceSnapshot, String>),
    Completed {
        action: ActionKind,
        result: Result<DeviceSnapshot, String>,
        state: Option<DeviceSnapshot>,
    },
    Saved(Result<PathBuf, String>),
    Stopped(Result<(), String>),
}

/// One pending action until its result is consumed by the UI/main thread, as
/// with the original recording/playback transition flags. Drop joins the owner.
pub struct DeviceWorker {
    commands: Sender<DeviceAction>,
    events: Receiver<DeviceEvent>,
    worker: Option<JoinHandle<()>>,
    pending: bool,
    observations: std::sync::Arc<ObservationHub>,
    // UI publishes to the same independent atomics read by native callbacks.
    controls: std::sync::Arc<crate::controls::AtomicSettings>,
}
impl DeviceWorker {
    #[cfg(all(windows, target_pointer_width = "64"))]
    /// Create the native owner on its own thread for the development command-line probe.
    pub fn spawn(dll: PathBuf, mode: RequestedMode, settings: Settings) -> Result<Self, String> {
        Self::spawn_inner(dll, mode, settings, false)
    }
    #[cfg(all(windows, target_pointer_width = "64"))]
    /// Create the native owner with the same independent control atomics used by SDL sliders.
    pub fn spawn_for_ui(
        dll: PathBuf,
        mode: RequestedMode,
        settings: Settings,
    ) -> Result<Self, String> {
        Self::spawn_inner(dll, mode, settings, true)
    }
    #[cfg(all(windows, target_pointer_width = "64"))]
    fn spawn_inner(
        dll: PathBuf,
        mode: RequestedMode,
        settings: Settings,
        shared_ui: bool,
    ) -> Result<Self, String> {
        let observations = std::sync::Arc::new(ObservationHub::default());
        let worker_observations = std::sync::Arc::clone(&observations);
        let controls = std::sync::Arc::new(crate::controls::AtomicSettings::new(settings));
        let worker_controls = std::sync::Arc::clone(&controls);
        let mut owner =
            Self::start_thread_with_observations(observations, move |commands, events| {
                // Construct COM/PortAudio on this thread, never move an existing
                // apartment/library owner from the UI into the worker.
                let initialized = if shared_ui {
                    crate::portaudio_windows::PortAudio::initialize_with_shared_controls(
                        &dll,
                        worker_observations,
                        worker_controls,
                    )
                } else {
                    crate::portaudio_windows::PortAudio::initialize_with_observations(
                        &dll,
                        worker_observations,
                    )
                };
                match initialized {
                    Ok(audio) => {
                        let mut backend = native::NativeDeviceState::new(&audio, mode, settings);
                        run_worker(&mut backend, commands, events);
                    }
                    Err(error) => {
                        let _ = events.send(DeviceEvent::Started(Err(error)));
                    }
                }
            })?;
        owner.controls = controls;
        Ok(owner)
    }
    /// Publish sliders/selectors without queuing device work or accessing a
    /// PortAudio handle. Arc ownership is established before any callback starts.
    pub fn publish_settings(&self, settings: Settings) {
        self.controls.store(settings);
    }
    #[cfg(test)]
    fn start_thread(
        body: impl FnOnce(Receiver<DeviceAction>, Sender<DeviceEvent>) + Send + 'static,
    ) -> Result<Self, String> {
        Self::start_thread_with_observations(std::sync::Arc::new(ObservationHub::default()), body)
    }
    /// Clone the observer owner on the application thread, never inside a sample callback.
    pub fn observations(&self) -> std::sync::Arc<ObservationHub> {
        std::sync::Arc::clone(&self.observations)
    }
    #[cfg_attr(not(windows), allow(dead_code))]
    fn start_thread_with_observations(
        observations: std::sync::Arc<ObservationHub>,
        body: impl FnOnce(Receiver<DeviceAction>, Sender<DeviceEvent>) + Send + 'static,
    ) -> Result<Self, String> {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("portaudio-device-owner".into())
            .spawn(move || body(command_rx, event_tx))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
            worker: Some(worker),
            pending: true,
            observations,
            controls: std::sync::Arc::new(
                crate::controls::AtomicSettings::new(Settings::default()),
            ),
        })
    }
    /// Report whether an accepted command still awaits result consumption.
    pub fn pending(&self) -> bool {
        self.pending
    }
    /// Queue one action or reject it while the previous result is outstanding. Does not open devices here.
    pub fn request(&mut self, action: DeviceAction) -> Result<(), String> {
        if self.pending {
            return Err("An audio action is still pending; wait for its result".into());
        }
        self.commands
            .send(action)
            .map_err(|_| "The audio device owner is unavailable")?;
        self.pending = true;
        Ok(())
    }
    /// Drain available results without waiting and release the pending gate for completed owner actions.
    pub fn poll(&mut self) -> Vec<DeviceEvent> {
        let mut result = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    if matches!(
                        &event,
                        DeviceEvent::Started(_)
                            | DeviceEvent::Completed { .. }
                            | DeviceEvent::Stopped(_)
                    ) {
                        self.pending = false;
                    }
                    result.push(event);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        result
    }
    /// Shutdown is queued after the running operation even when ordinary audio
    /// controls are gated. The worker drains/joins before native ownership ends.
    pub fn shutdown(&mut self) -> Result<(), String> {
        if let Some(worker) = self.worker.take() {
            let _ = self.commands.send(DeviceAction::Shutdown);
            worker
                .join()
                .map_err(|_| "Audio device owner panicked".to_string())?;
        }
        Ok(())
    }
}
impl Drop for DeviceWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
trait DeviceBackend {
    fn initialize(&mut self) -> Result<DeviceSnapshot, String>;
    fn perform(&mut self, action: DeviceAction) -> Result<DeviceSnapshot, String>;
    fn current_state(&self) -> Option<DeviceSnapshot> {
        None
    }
    fn poll_export(&mut self) -> Option<Result<PathBuf, String>>;
    fn shutdown(&mut self) -> Result<(), String>;
}
#[cfg_attr(not(windows), allow(dead_code))]
fn run_worker(
    backend: &mut impl DeviceBackend,
    commands: Receiver<DeviceAction>,
    events: Sender<DeviceEvent>,
) {
    let _ = events.send(DeviceEvent::Started(backend.initialize()));
    loop {
        if let Some(result) = backend.poll_export() {
            let _ = events.send(DeviceEvent::Saved(result));
        }
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(DeviceAction::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(action) => {
                let kind = action.kind();
                // C++ catches exceptions around each queued device action.
                // Keep ordinary Rust unwinding inside the owner boundary too.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    backend.perform(action)
                }))
                .unwrap_or_else(|_| Err("Audio device action panicked".into()));
                let state = backend.current_state();
                let _ = events.send(DeviceEvent::Completed {
                    action: kind,
                    result,
                    state,
                });
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = events.send(DeviceEvent::Stopped(backend.shutdown()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    type Trace = Arc<Mutex<Vec<(&'static str, thread::ThreadId)>>>;

    struct MockBackend {
        trace: Trace,
        entered: Sender<()>,
        release: Receiver<()>,
        snapshot: DeviceSnapshot,
    }
    impl MockBackend {
        fn log(&self, operation: &'static str) {
            self.trace
                .lock()
                .unwrap()
                .push((operation, thread::current().id()));
        }
    }
    impl DeviceBackend for MockBackend {
        fn initialize(&mut self) -> Result<DeviceSnapshot, String> {
            self.log("initialize");
            Ok(self.snapshot.clone())
        }
        fn perform(&mut self, action: DeviceAction) -> Result<DeviceSnapshot, String> {
            match action {
                DeviceAction::Reopen(_) => {
                    self.log("reopen begin");
                    self.entered.send(()).unwrap();
                    self.release
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(|e| e.to_string())?;
                    self.log("reopen end");
                }
                DeviceAction::Play(_) => panic!("test-only device action failure"),
                DeviceAction::Status => self.log("status"),
                _ => self.log("action"),
            }
            Ok(self.snapshot.clone())
        }
        fn current_state(&self) -> Option<DeviceSnapshot> {
            Some(self.snapshot.clone())
        }
        fn poll_export(&mut self) -> Option<Result<PathBuf, String>> {
            None
        }
        fn shutdown(&mut self) -> Result<(), String> {
            self.log("shutdown");
            Ok(())
        }
    }
    fn mock_worker() -> (DeviceWorker, Trace, Receiver<()>, Sender<()>) {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_trace = Arc::clone(&trace);
        let worker = DeviceWorker::start_thread(move |commands, events| {
            let mut backend = MockBackend {
                trace: worker_trace,
                entered: entered_tx,
                release: release_rx,
                snapshot: DeviceSnapshot {
                    ready: true,
                    active: true,
                    live: false,
                    recording: false,
                    playing: false,
                    export_busy: false,
                    frames: 0,
                    incomplete: false,
                    requested: RequestedMode::Shared,
                    actual: ActualMode::Shared,
                    selection: None,
                },
            };
            run_worker(&mut backend, commands, events);
        })
        .unwrap();
        (worker, trace, entered_rx, release_tx)
    }
    fn wait_event(worker: &mut DeviceWorker) -> DeviceEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = worker.poll();
            if let Some(event) = events.into_iter().next() {
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "device worker did not publish a result"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn device_calls_run_on_one_owner_and_actions_wait_for_result_consumption() {
        let caller = thread::current().id();
        let (mut worker, trace, _, _) = mock_worker();
        assert!(worker.request(DeviceAction::Status).is_err()); // Startup not consumed.
        assert!(matches!(
            wait_event(&mut worker),
            DeviceEvent::Started(Ok(_))
        ));
        worker.request(DeviceAction::Status).unwrap();
        assert!(worker.request(DeviceAction::RecordStart).is_err());
        assert!(matches!(
            wait_event(&mut worker),
            DeviceEvent::Completed {
                action: ActionKind::Status,
                result: Ok(_),
                ..
            }
        ));
        worker.shutdown().unwrap();
        let operations = trace.lock().unwrap();
        assert_eq!(
            operations.iter().map(|v| v.0).collect::<Vec<_>>(),
            ["initialize", "status", "shutdown"]
        );
        assert_ne!(operations[0].1, caller);
        assert!(operations
            .iter()
            .all(|operation| operation.1 == operations[0].1));
    }

    #[test]
    fn slow_reopen_keeps_caller_available_and_refuses_overlapping_actions() {
        let (mut worker, trace, entered, release) = mock_worker();
        wait_event(&mut worker);
        worker
            .request(DeviceAction::Reopen(ReopenRequest {
                mode: RequestedMode::Exclusive,
                refresh_devices: false,
            }))
            .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(worker.pending());
        assert!(worker.poll().is_empty());
        assert!(worker.request(DeviceAction::RecordStart).is_err());
        // The caller continues while the owner is deliberately held in reopen.
        assert_eq!(trace.lock().unwrap().last().unwrap().0, "reopen begin");
        release.send(()).unwrap();
        assert!(matches!(
            wait_event(&mut worker),
            DeviceEvent::Completed {
                action: ActionKind::Reopen,
                result: Ok(_),
                ..
            }
        ));
        worker.shutdown().unwrap();
    }

    #[test]
    fn shutdown_joins_running_reopen_before_owner_cleanup() {
        let (mut worker, trace, entered, release) = mock_worker();
        wait_event(&mut worker);
        worker
            .request(DeviceAction::Reopen(ReopenRequest {
                mode: RequestedMode::Shared,
                refresh_devices: true,
            }))
            .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let stopper = thread::spawn(move || worker.shutdown());
        assert!(!trace
            .lock()
            .unwrap()
            .iter()
            .any(|operation| operation.0 == "shutdown"));
        release.send(()).unwrap();
        stopper.join().unwrap().unwrap();
        assert_eq!(
            trace
                .lock()
                .unwrap()
                .iter()
                .map(|v| v.0)
                .collect::<Vec<_>>(),
            ["initialize", "reopen begin", "reopen end", "shutdown"]
        );
    }

    #[test]
    fn queued_action_panic_is_reported_and_does_not_escape_the_owner_loop() {
        let (mut worker, trace, _, _) = mock_worker();
        wait_event(&mut worker);
        worker
            .request(DeviceAction::Play(Settings::default()))
            .unwrap();
        assert!(matches!(
            wait_event(&mut worker),
            DeviceEvent::Completed {
                action: ActionKind::Play,
                result: Err(_),
                ..
            }
        ));
        worker.request(DeviceAction::Status).unwrap();
        assert!(matches!(
            wait_event(&mut worker),
            DeviceEvent::Completed { result: Ok(_), .. }
        ));
        worker.shutdown().unwrap();
        assert_eq!(trace.lock().unwrap().last().unwrap().0, "shutdown");
    }
}

#[cfg(all(windows, target_pointer_width = "64"))]
mod native {
    use super::*;
    use crate::{
        portaudio_windows::{LiveStream, PortAudio},
        take::{ExportWorker, RecordedTake, RecordingSession},
    };
    /// Owner-thread native handles, current take, recording session and export worker.
    pub(super) struct NativeDeviceState<'a> {
        audio: &'a PortAudio,
        stream: Option<LiveStream<'a>>,
        recording: Option<RecordingSession>,
        take: Option<RecordedTake>,
        export: ExportWorker,
        save_counter: u64,
        mode: RequestedMode,
        settings: Settings,
    }
    impl<'a> NativeDeviceState<'a> {
        /// Construct owner state before opening streams; native work stays on the current device thread.
        pub fn new(audio: &'a PortAudio, mode: RequestedMode, settings: Settings) -> Self {
            Self {
                audio,
                stream: None,
                recording: None,
                take: None,
                export: ExportWorker::default(),
                save_counter: 0,
                mode,
                settings,
            }
        }
        fn snapshot(&self) -> Result<DeviceSnapshot, String> {
            let selection = self
                .stream
                .as_ref()
                .filter(|s| s.is_open())
                .map(|s| s.selection.clone());
            Ok(DeviceSnapshot {
                ready: selection.is_some(),
                active: self
                    .stream
                    .as_ref()
                    .is_some_and(|s| s.is_active().unwrap_or(false)),
                live: self.stream.as_ref().is_some_and(|s| s.live_active()),
                recording: self.recording.is_some(),
                playing: self.stream.as_ref().is_some_and(|s| s.preview_playing()),
                export_busy: self.export.is_busy(),
                frames: self
                    .take
                    .as_ref()
                    .map(|take| take.frames())
                    .transpose()?
                    .unwrap_or(0),
                incomplete: self.take.as_ref().is_some_and(|take| take.incomplete),
                requested: self.audio.last_requested_mode(),
                actual: selection
                    .as_ref()
                    .map(|s| s.actual_mode())
                    .unwrap_or(ActualMode::Unavailable),
                selection,
            })
        }
        fn finish_recording(&mut self) -> Result<(), String> {
            if let Some(session) = self.recording.take() {
                self.take = Some(session.finish()?);
            }
            Ok(())
        }
    }
    impl DeviceBackend for NativeDeviceState<'_> {
        fn initialize(&mut self) -> Result<DeviceSnapshot, String> {
            self.stream = Some(self.audio.open_idle(self.mode, self.settings)?);
            self.mode = self.audio.last_requested_mode();
            self.snapshot()
        }
        fn perform(&mut self, action: DeviceAction) -> Result<DeviceSnapshot, String> {
            if let Some(settings) = self.audio.shared_settings() {
                self.settings = settings;
            }
            match action {
                DeviceAction::Status => {}
                DeviceAction::RecordStart => {
                    if self.export.is_busy() {
                        return Err("Wait for WAV export before a new recording".into());
                    }
                    if self.recording.is_some() {
                        return Err("A recording/collector is already active".into());
                    }
                    let stream = self
                        .stream
                        .as_mut()
                        .ok_or("Audio is unavailable; reopen first")?;
                    if stream.live_active() {
                        return Err("Switch to Recording mode before a new take".into());
                    }
                    if !stream.is_active()? {
                        return Err("Duplex capture is unavailable; reopen first".into());
                    }
                    self.recording = Some(stream.begin_recording_reusing(self.take.take())?);
                }
                DeviceAction::RecordStop => self.finish_recording()?,
                DeviceAction::SetLive(active) => {
                    if active {
                        self.finish_recording()?;
                    }
                    self.stream
                        .as_mut()
                        .ok_or("Audio is unavailable; reopen first")?
                        .set_live(active)?;
                }
                DeviceAction::SetSettings(settings) => {
                    self.settings = settings;
                    if let Some(stream) = &self.stream {
                        stream.update_settings(settings);
                    }
                }
                DeviceAction::Play(settings) => {
                    if self.recording.is_some() {
                        return Err("Stop recording before playback".into());
                    }
                    self.stream
                        .as_mut()
                        .ok_or("Audio is unavailable; reopen first")?
                        .start_preview(
                            self.take.as_ref().ok_or("Record a take first")?,
                            settings,
                        )?;
                    self.settings = settings;
                    if !self.audio.has_shared_controls() {
                        self.stream.as_ref().unwrap().update_settings(settings);
                    }
                }
                DeviceAction::StopPreview => {
                    if let Some(stream) = &mut self.stream {
                        stream.stop_preview()?;
                    }
                }
                DeviceAction::Save(settings) => {
                    if self.recording.is_some() {
                        return Err("Stop recording before WAV export".into());
                    }
                    if self.stream.as_ref().is_some_and(|s| s.live_active()) {
                        return Err("Switch to Recording mode before saving".into());
                    }
                    let take = self.take.as_ref().ok_or("Record a take first")?;
                    let prefix = format!(
                        "audio_selected_{}_{}_mono",
                        settings.mask, self.save_counter
                    );
                    let path = std::env::current_exe()
                        .map_err(|e| e.to_string())?
                        .parent()
                        .ok_or("Cannot locate executable folder")?
                        .join("recordings")
                        .join(crate::wav::local_wav_filename(&prefix));
                    self.export.start(take, settings, path)?;
                    self.save_counter += 1;
                }
                DeviceAction::Reopen(request) => {
                    if self.recording.is_some() {
                        return Err("Stop recording before reopening audio".into());
                    }
                    self.mode = request.mode;
                    if let Some(stream) = &mut self.stream {
                        stream.reopen(request.mode, request.refresh_devices)?;
                    } else {
                        self.audio.refresh_device_list()?;
                        self.stream = Some(self.audio.open_idle(request.mode, self.settings)?);
                    }
                    self.mode = self.audio.last_requested_mode();
                }
                DeviceAction::Shutdown => unreachable!("handled by worker loop"),
            }
            self.snapshot()
        }
        fn current_state(&self) -> Option<DeviceSnapshot> {
            self.snapshot().ok()
        }
        fn poll_export(&mut self) -> Option<Result<PathBuf, String>> {
            if self.export.is_busy() {
                return None;
            }
            match self.export.join() {
                Ok(Some(path)) => Some(Ok(path)),
                Err(error) => Some(Err(error)),
                _ => None,
            }
        }
        fn shutdown(&mut self) -> Result<(), String> {
            let mut errors = Vec::new();
            if let Err(error) = self.finish_recording() {
                errors.push(error);
            }
            if let Some(stream) = self.stream.take() {
                if let Err(error) = stream.finish() {
                    errors.push(error);
                }
            }
            match self.export.join() {
                Ok(Some(_path)) => {}
                Err(error) => errors.push(error),
                _ => {}
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; "))
            }
        }
    }
}
