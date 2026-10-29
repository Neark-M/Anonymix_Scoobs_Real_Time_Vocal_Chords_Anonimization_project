//! Windows SDL callback entry points and original visual/audio scheduling.
//! Callback-owned GUI state is main-thread TLS, never a Send/Sync renderer.
//! Unexpected SDL callbacks from another thread cannot borrow main-thread UI
//! state; no Rust panic is allowed to unwind through the C boundary.
use crate::{
    device_plan::RequestedMode,
    device_worker::{DeviceAction, DeviceWorker},
    display_worker::DisplayFftWorker,
    observation::ObservationHub,
    sdl_abi::*,
    sdl_api::Sdl,
    spectrum::{smooth_display, SpectrumFrame},
    ui_budget::AdaptiveVisualBudget,
    ui_controller::UiController,
    ui_geometry::Point,
    ui_layout,
    ui_renderer::UiRenderer,
    ui_timing::{DeadlinePacer, FramePressure},
    ui_widgets::UiModel,
};
use std::{
    cell::RefCell,
    path::PathBuf,
    ptr,
    sync::{atomic::Ordering, Arc},
};

thread_local! {static APP:RefCell<Option<DesktopApp>>=const{RefCell::new(None)};}
fn runtime_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = executable.parent().ok_or("Executable folder unavailable")?;
    for directory in [
        parent.to_path_buf(),
        parent.join("runtime"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime"),
    ] {
        if directory.join("SDL3.dll").is_file() {
            return Ok(directory);
        }
    }
    Err("SDL3.dll is missing; keep the supplied runtime folder or place its files beside the executable".into())
}
/// Load the supplied SDL runtime and run its main-thread application callbacks.
/// Return an error after initialization or a contained callback failure.
pub fn run() -> Result<(), String> {
    let runtime = runtime_directory()?;
    let library = Sdl::load(&runtime.join("SDL3.dll"))?;
    // Keep this loader on the stack until SDL_QuitMainCallbacks returns, even
    // though AppQuit drops its own renderer and DLL references before then.
    let code = unsafe {
        (library.enter_callbacks)(
            0,
            ptr::null_mut(),
            app_init,
            app_iterate,
            app_event,
            app_quit,
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err("SDL application stopped after an initialization or callback failure".into())
    }
}
struct DesktopApp {
    renderer: UiRenderer,
    ui: UiModel,
    controller: UiController,
    worker: DeviceWorker,
    hub: Arc<ObservationHub>,
    fft: DisplayFftWorker,
    spectrum: Option<SpectrumFrame>,
    smoothed: [f32; 96],
    budget: AdaptiveVisualBudget,
    pacer: DeadlinePacer,
    budget_since: u64,
    frames: u32,
    slow: u32,
    heavy: u32,
}
impl DesktopApp {
    fn create() -> Result<Self, String> {
        let runtime = runtime_directory()?;
        let renderer = UiRenderer::create(
            &runtime.join("SDL3.dll"),
            &runtime.join("SDL3_ttf.dll"),
            &runtime,
        )?;
        let mut ui = UiModel::default();
        let (w, h) = renderer.window_size();
        ui.refresh_layout(w, h);
        let worker = DeviceWorker::spawn_for_ui(
            runtime.join("portaudio.dll"),
            RequestedMode::Shared,
            ui.settings(),
        )?;
        let hub = worker.observations();
        let fft = DisplayFftWorker::start(Arc::clone(&hub));
        let now = unsafe { (renderer.api().ticks_ns)() };

        Ok(Self {
            renderer,
            ui,
            controller: UiController::default(),
            worker,
            hub,
            fft,
            spectrum: None,
            smoothed: [0.; 96],
            budget: AdaptiveVisualBudget::default(),
            pacer: DeadlinePacer::default(),
            budget_since: now,
            frames: 0,
            slow: 0,
            heavy: 0,
        })
    }
    fn send(&mut self, action: Option<DeviceAction>, now: u64) {
        if let Some(action) = action {
            if let Err(error) = self.worker.request(action) {
                self.controller.submission_failed(error, now, &mut self.ui);
            }
        }
    }
    fn mapped_pointer(&self, p: Point) -> Point {
        let (w, h) = self.renderer.window_size();
        ui_layout::window_point_to_ui(w, h, p).unwrap_or(Point { x: -1., y: -1. })
    }
    fn event(&mut self, event: &Event) -> i32 {
        // SAFETY: SDL populated the discriminant and matching variant before this
        // borrowed event was delivered. The event storage never escapes this call.
        let kind = unsafe { event.kind };
        if kind == EVENT_QUIT || kind == EVENT_WINDOW_CLOSE_REQUESTED {
            return 1;
        }
        if matches!(
            kind,
            EVENT_WINDOW_DISPLAY_CHANGED | EVENT_DISPLAY_USABLE_BOUNDS_CHANGED
        ) {
            self.renderer.update_monitor_limits();
        }
        if matches!(kind, EVENT_MOUSE_MOTION | EVENT_MOUSE_DOWN | EVENT_MOUSE_UP) {
            let (w, h) = self.renderer.window_size();
            self.ui.refresh_layout(w, h);
            let (window, button) = unsafe {
                if kind == EVENT_MOUSE_MOTION {
                    (
                        Point {
                            x: event.motion.x,
                            y: event.motion.y,
                        },
                        0,
                    )
                } else {
                    (
                        Point {
                            x: event.button.x,
                            y: event.button.y,
                        },
                        event.button.button,
                    )
                }
            };
            let point = self.mapped_pointer(window);
            let reopening = self.controller.reopening();
            let stream = self.controller.is_live();
            if kind == EVENT_MOUSE_DOWN && button == 1 {
                self.ui.mouse_down(point, reopening, stream);
            } else if kind == EVENT_MOUSE_UP && button == 1 {
                if let Some(action) = self.ui.release_action(point, reopening, stream) {
                    let now = unsafe { (self.renderer.api().ticks_ns)() };
                    let plan =
                        self.controller
                            .plan(action, now, self.worker.pending(), &mut self.ui);
                    self.send(plan, now);
                    self.worker.publish_settings(self.ui.settings());
                }
                self.ui.end_press();
            } else {
                self.ui.hovered = self.ui.hit_test(point, reopening, stream);
            }
        }
        0
    }
    fn iterate(&mut self) -> i32 {
        let started = unsafe { (self.renderer.api().ticks_ns)() };
        let mut fps = self.budget.fps();
        let frame = self.pacer.begin(started, fps);
        self.frames = self.frames.wrapping_add(1);
        if started - self.budget_since >= 10_000_000_000 {
            let pressure = self.hub.audio.take_visual_pressure_events();
            if self
                .budget
                .observe(self.frames, self.slow, self.heavy, u32::from(pressure != 0))
            {
                fps = self.budget.fps();
                self.pacer.budget_changed(started, fps);
                self.renderer.set_fft_scale(self.budget.fft_scale());
            }
            self.frames = 0;
            self.slow = 0;
            self.heavy = 0;
            self.budget_since = started;
        }
        for event in self.worker.poll() {
            self.controller.event(event, started, &mut self.ui);
        }
        let plan = self
            .controller
            .settled(started, self.worker.pending(), &mut self.ui);
        self.send(plan, started);
        let plan = self.controller.completed_preview(
            self.hub.preview_active.load(Ordering::Acquire),
            self.worker.pending(),
            &mut self.ui,
        );
        self.send(plan, started);
        // Match AppUpdate ordering: publish before RenderFrame's drag update, and
        // suspend this settings copy while the owner is reopening devices.
        if !self.controller.reopening() {
            self.worker.publish_settings(self.ui.settings());
        }
        if let Some(frame) = self.fft.publish() {
            self.spectrum = Some(frame);
        }
        self.fft.request(); // Exactly one request per scheduled visual frame.
        if let Some(spectrum) = &self.spectrum {
            smooth_display(
                &spectrum.magnitudes,
                &mut self.smoothed,
                0.34,
                frame.elapsed_seconds,
            );
        }
        let wave = self.hub.rings.snapshot_waveform();
        let mouse = self.mapped_pointer(self.renderer.pointer());
        let before = unsafe { (self.renderer.api().ticks_ns)() };
        let now = unsafe { (self.renderer.api().ticks)() } as f64 / 1000.;
        let timing = self.renderer.render(
            &mut self.ui,
            now,
            mouse,
            self.controller.reopening(),
            self.controller.is_live(),
            &self.smoothed,
            self.spectrum.as_ref(),
            wave.as_ref(),
            true,
        );
        let after = unsafe { (self.renderer.api().ticks_ns)() };
        if timing.spectrum_ns > 150_000_000 && self.budget.reduce_fft_after_stall() {
            self.renderer.set_fft_scale(self.budget.fft_scale());
        }
        let pressure =
            FramePressure::classify(before - started, timing, DeadlinePacer::period_ns(fps));
        if pressure.slow_app {
            self.slow = self.slow.wrapping_add(1);
        }
        if pressure.fft_heavy {
            self.heavy = self.heavy.wrapping_add(1);
        }
        let wait = self.pacer.wait_ns(after);
        if wait != 0 {
            unsafe {
                (self.renderer.api().delay_precise)(wait);
            }
        }
        self.pacer
            .finish(after, unsafe { (self.renderer.api().ticks_ns)() }, fps);
        0
    }
}
impl Drop for DesktopApp {
    fn drop(&mut self) {
        self.fft.stop();
        let _ = self.worker.shutdown();
    }
}
fn boundary(call: impl FnOnce() -> i32) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).unwrap_or_else(|_| 2)
}
unsafe extern "C" fn app_init(
    state: *mut Handle,
    _argc: i32,
    _argv: *mut *mut std::ffi::c_char,
) -> i32 {
    boundary(|| {
        if !state.is_null() {
            unsafe {
                *state = ptr::null_mut();
            }
        }
        match DesktopApp::create() {
            Ok(app) => {
                APP.with(|slot| *slot.borrow_mut() = Some(app));
                0
            }
            Err(_error) => 2,
        }
    })
}
unsafe extern "C" fn app_iterate(_state: Handle) -> i32 {
    boundary(|| {
        APP.with(|slot| {
            let Ok(mut app) = slot.try_borrow_mut() else {
                return 0;
            };
            app.as_mut().map_or(0, DesktopApp::iterate)
        })
    })
}
unsafe extern "C" fn app_event(_state: Handle, event: *mut Event) -> i32 {
    boundary(|| {
        if event.is_null() {
            return 0;
        }
        APP.with(|slot| {
            let Ok(mut app) = slot.try_borrow_mut() else {
                return 0;
            };
            app.as_mut().map_or(0, |app| app.event(unsafe { &*event }))
        })
    })
}
unsafe extern "C" fn app_quit(_state: Handle, _result: i32) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        APP.with(|slot| {
            if let Ok(mut app) = slot.try_borrow_mut() {
                drop(app.take());
            }
        })
    }));
}
