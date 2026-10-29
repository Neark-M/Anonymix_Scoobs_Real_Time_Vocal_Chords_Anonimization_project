//! Windows x64 PortAudio V19 loader, duplex capture/live and recorded preview.
//! Device methods stay on one owner thread; rendering runs in native callbacks.
use crate::{
    controls::AtomicSettings,
    device_plan::{latency_profiles, wasapi_attempts, RequestedMode},
    device_policy::{EndpointIds, PreviewCacheKey},
    duplex::DuplexProcessor,
    observation::ObservationHub,
    playback::{PlaybackResult, RecordedPlayback},
    portaudio_abi::*,
    take::{RecordedTake, RecordingInput, RecordingSession},
    windows_endpoints::{default_endpoint_id, default_endpoint_ids},
    Settings, SAMPLE_RATE,
};
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    ffi::{c_char, c_void, CStr},
    marker::PhantomData,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
    sync::Arc,
};

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(path: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}
#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    fn CoUninitialize();
}

struct ComApartment {
    keep_initialized: Cell<bool>,
}
impl ComApartment {
    fn new() -> Result<Self, String> {
        // SAFETY: null reserved pointer and COINIT_MULTITHREADED, as in App.cpp.
        let hr = unsafe { CoInitializeEx(ptr::null_mut(), 0) };
        if hr < 0 {
            return Err(format!("CoInitializeEx(MTA): 0x{:08x}", hr as u32));
        }
        Ok(Self {
            keep_initialized: Cell::new(false),
        })
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        if !self.keep_initialized.get() {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

struct Library {
    handle: *mut c_void,
    keep_loaded: Cell<bool>,
}
impl Library {
    fn open(path: &Path) -> Result<Self, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("PortAudio DLL {}: {e}", path.display()))?;
        let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err("DLL path contains a NUL".into());
        }
        wide.push(0);
        // SAFETY: stable NUL-terminated absolute path. Search dependencies in the
        // DLL directory and normal application/system directories on Windows 10+.
        let handle = unsafe { LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), 0x00001100) };
        if handle.is_null() {
            return Err(format!(
                "LoadLibraryExW: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self {
            handle,
            keep_loaded: Cell::new(false),
        })
    }
    fn symbol(&self, name: &[u8]) -> Result<*mut c_void, String> {
        // SAFETY: each caller supplies a fixed, NUL-terminated API symbol name.
        let result = unsafe { GetProcAddress(self.handle, name.as_ptr()) };
        if result.is_null() {
            return Err(format!(
                "Missing PortAudio export: {}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            ));
        }
        Ok(result)
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        if !self.keep_loaded.get() {
            unsafe {
                FreeLibrary(self.handle);
            }
        }
    }
}

macro_rules! define_api{
    ($($field:ident:$signature:ty=>$symbol:literal),+ $(,)?)=>{
        struct Api{$($field:$signature),+}
        impl Api{
            fn load(module:&Library)->Result<Self,String>{Ok(Self{$(
                // SAFETY: the named PortAudio V19 export has this documented
                // C ABI signature. Windows x64 sizes are checked in abi tests.
                $field:unsafe{std::mem::transmute::<*mut c_void,$signature>(module.symbol(concat!($symbol,"\0").as_bytes())?)},
            )+})}
        }
    }
}
define_api! {
    initialize:unsafe extern "C" fn()->i32=>"Pa_Initialize",
    terminate:unsafe extern "C" fn()->i32=>"Pa_Terminate",
    version:unsafe extern "C" fn()->*const c_char=>"Pa_GetVersionText",
    error_text:unsafe extern "C" fn(i32)->*const c_char=>"Pa_GetErrorText",
    device_count:unsafe extern "C" fn()->i32=>"Pa_GetDeviceCount",
    device_info:unsafe extern "C" fn(i32)->*const DeviceInfo=>"Pa_GetDeviceInfo",
    default_input:unsafe extern "C" fn()->i32=>"Pa_GetDefaultInputDevice",
    default_output:unsafe extern "C" fn()->i32=>"Pa_GetDefaultOutputDevice",
    host_index:unsafe extern "C" fn(i32)->i32=>"Pa_HostApiTypeIdToHostApiIndex",
    host_info:unsafe extern "C" fn(i32)->*const HostApiInfo=>"Pa_GetHostApiInfo",
    format_supported:unsafe extern "C" fn(*const StreamParameters,*const StreamParameters,f64)->i32=>"Pa_IsFormatSupported",
    open_stream:unsafe extern "C" fn(*mut *mut c_void,*const StreamParameters,*const StreamParameters,f64,u32,u32,Option<Callback>,*mut c_void)->i32=>"Pa_OpenStream",
    start_stream:unsafe extern "C" fn(*mut c_void)->i32=>"Pa_StartStream",
    stop_stream:unsafe extern "C" fn(*mut c_void)->i32=>"Pa_StopStream",
    abort_stream:unsafe extern "C" fn(*mut c_void)->i32=>"Pa_AbortStream",
    close_stream:unsafe extern "C" fn(*mut c_void)->i32=>"Pa_CloseStream",
    stream_info:unsafe extern "C" fn(*mut c_void)->*const StreamInfo=>"Pa_GetStreamInfo",
    stream_active:unsafe extern "C" fn(*mut c_void)->i32=>"Pa_IsStreamActive",
}

fn c_text(value: *const c_char) -> String {
    if value.is_null() {
        return "(none)".into();
    }
    // SAFETY: caller supplies a PortAudio-owned NUL-terminated string while its
    // initialized library is alive. No borrowed pointer escapes this function.
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
}

/// Device-thread-only COM, DLL and PortAudio owner. Live streams borrow it to prevent premature unload.
pub struct PortAudio {
    api: Api,
    module: Library,
    _com: ComApartment,
    initialized: Cell<bool>,
    observations: Arc<ObservationHub>,
    shared_controls: Option<Arc<AtomicSettings>>,
    live_streams: Cell<usize>,
    endpoints: RefCell<EndpointIds>,
    last_requested: Cell<RequestedMode>,
    _same_thread: PhantomData<Rc<()>>,
}
impl PortAudio {
    /// Initialize COM and PortAudio on the calling owner thread with fresh observer state.
    pub fn initialize(path: &Path) -> Result<Self, String> {
        Self::initialize_with_observations(path, Arc::new(ObservationHub::default()))
    }
    /// Initialize the native owner using the supplied shared display/report observers.
    pub fn initialize_with_observations(
        path: &Path,
        observations: Arc<ObservationHub>,
    ) -> Result<Self, String> {
        Self::initialize_inner(path, observations, None)
    }
    /// SDL owns only the atomic publication handle; COM and native streams
    /// remain on this PortAudio owner. No Arc operation occurs in a callback.
    pub fn initialize_with_shared_controls(
        path: &Path,
        observations: Arc<ObservationHub>,
        controls: Arc<AtomicSettings>,
    ) -> Result<Self, String> {
        Self::initialize_inner(path, observations, Some(controls))
    }
    /// Read externally shared UI controls when this owner was created for the desktop.
    pub fn shared_settings(&self) -> Option<Settings> {
        self.shared_controls.as_ref().map(|c| c.load())
    }
    /// Identify the direct-atomic UI route instead of the development probe command route.
    pub fn has_shared_controls(&self) -> bool {
        self.shared_controls.is_some()
    }
    fn initialize_inner(
        path: &Path,
        observations: Arc<ObservationHub>,
        shared_controls: Option<Arc<AtomicSettings>>,
    ) -> Result<Self, String> {
        let com = ComApartment::new()?;
        let module = Library::open(path)?;
        let api = Api::load(&module)?;
        let result = Self {
            api,
            module,
            _com: com,
            initialized: Cell::new(false),
            observations,
            shared_controls,
            live_streams: Cell::new(0),
            endpoints: RefCell::new(EndpointIds::default()),
            last_requested: Cell::new(RequestedMode::Shared),
            _same_thread: PhantomData,
        };
        // SAFETY: symbols are loaded, and this owner thread has an MTA apartment.
        let code = unsafe { (result.api.initialize)() };
        if code != 0 {
            return Err(result.error(code));
        }
        result.initialized.set(true);
        Ok(result)
    }
    /// Copy the library's version text while its borrowed C string remains valid.
    pub fn version(&self) -> String {
        c_text(unsafe { (self.api.version)() })
    }
    fn error(&self, code: i32) -> String {
        format!(
            "{} ({code})",
            c_text(unsafe { (self.api.error_text)(code) })
        )
    }
    fn info(&self, index: i32) -> Option<DeviceInfo> {
        if index < 0 {
            return None;
        }
        // SAFETY: valid initialized API; returned struct remains owned by PA.
        unsafe { (self.api.device_info)(index).as_ref().copied() }
    }
    /// Enumerate device descriptions on the initialized native owner thread.
    pub fn list_devices(&self) -> Result<Vec<String>, String> {
        let count = unsafe { (self.api.device_count)() };
        if count < 0 {
            return Err(self.error(count));
        }
        let input = unsafe { (self.api.default_input)() };
        let output = unsafe { (self.api.default_output)() };
        let mut result = Vec::new();
        for index in 0..count {
            if let Some(info) = self.info(index) {
                let host = unsafe { (self.api.host_info)(info.host_api) };
                let host_name = unsafe { host.as_ref() }
                    .map(|h| c_text(h.name))
                    .unwrap_or_default();
                result.push(format!(
                    "{index}: {} [{host_name}], in {}, out {}, default rate {:.0} Hz{}{}",
                    c_text(info.name),
                    info.max_input_channels,
                    info.max_output_channels,
                    info.default_sample_rate,
                    if input == index {
                        " [global input]"
                    } else {
                        ""
                    },
                    if output == index {
                        " [global output]"
                    } else {
                        ""
                    }
                ));
            }
        }
        Ok(result)
    }
    /// Open the original duplex with live monitoring initially inactive.
    pub fn open_idle(
        &self,
        mode: RequestedMode,
        settings: Settings,
    ) -> Result<LiveStream<'_>, String> {
        let context = Arc::new(CallbackContext::with_controls(
            None,
            Arc::clone(&self.observations),
            self.shared_controls
                .as_ref()
                .map(Arc::clone)
                .unwrap_or_else(|| Arc::new(AtomicSettings::new(settings))),
        ));
        match self.open_live_inner(mode, &context) {
            Ok((stream, selection)) => {
                self.live_streams.set(self.live_streams.get() + 1);
                Ok(LiveStream {
                    owner: self,
                    stream,
                    context: Some(context),
                    preview_stream: ptr::null_mut(),
                    preview_cache: None,
                    recording_control: None,
                    selection,
                })
            }
            Err(error) => {
                if self.module.keep_loaded.get() {
                    std::mem::forget(context);
                }
                Err(error)
            }
        }
    }
    /// Open the inherited duplex route and enable live output using the requested fallback policy.
    pub fn open_live(
        &self,
        mode: RequestedMode,
        settings: Settings,
    ) -> Result<LiveStream<'_>, String> {
        let mut stream = self.open_idle(mode, settings)?;
        stream.set_live(true)?;
        Ok(stream)
    }
    /// Open the inherited duplex capture route with a recording producer; idle output remains silent.
    pub fn open_recording(
        &self,
        mode: RequestedMode,
        settings: Settings,
    ) -> Result<(LiveStream<'_>, RecordingSession), String> {
        let mut stream = self.open_idle(mode, settings)?;
        let recording = stream.begin_recording()?;
        Ok((stream, recording))
    }
    /// Read the owner's fallback-adjusted requested mode after an open attempt.
    pub fn last_requested_mode(&self) -> RequestedMode {
        self.last_requested.get()
    }
    /// Compare current default endpoint IDs with the cached enumeration identities.
    pub fn device_list_needs_refresh(&self) -> bool {
        self.endpoints
            .borrow()
            .needs_refresh(&default_endpoint_ids())
    }
    /// Only after every live wrapper has closed its duplex and preview streams.
    /// Interior mutability expresses the C API owner state, not callback access.
    pub(crate) fn refresh_device_list(&self) -> Result<(), String> {
        if self.live_streams.get() != 0 {
            return Err("Close all audio streams before refresh".into());
        }
        if self.module.keep_loaded.get() {
            return Err("Native close failed; restart the application".into());
        }
        if self.initialized.get() {
            let code = unsafe { (self.api.terminate)() };
            if code != 0 {
                self.module.keep_loaded.set(true);
                return Err(format!("PortAudio terminate failed: {}", self.error(code)));
            }
            self.initialized.set(false);
        }
        let code = unsafe { (self.api.initialize)() };
        if code != 0 {
            return Err(format!("PortAudio refresh failed: {}", self.error(code)));
        }
        self.initialized.set(true);
        Ok(())
    }

    fn open_live_inner(
        &self,
        mode: RequestedMode,
        context: &CallbackContext,
    ) -> Result<(*mut c_void, StreamSelection), String> {
        if !self.initialized.get() {
            return Err("PortAudio is not initialized".into());
        }
        if self.module.keep_loaded.get() {
            return Err("Native close failed; restart the application".into());
        }
        self.last_requested.set(mode);
        let wasapi_index = unsafe { (self.api.host_index)(PA_WASAPI) };
        if wasapi_index >= 0 {
            let host = unsafe { (self.api.host_info)(wasapi_index).as_ref().copied() };
            if let Some(host) = host {
                if host.default_input_device >= 0 && host.default_output_device >= 0 {
                    if let (Some(input), Some(output)) = (
                        self.info(host.default_input_device),
                        self.info(host.default_output_device),
                    ) {
                        for attempt in wasapi_attempts(
                            mode,
                            input.max_input_channels,
                            output.max_output_channels,
                        ) {
                            if mode == RequestedMode::Exclusive && !attempt.exclusive {
                                self.last_requested.set(RequestedMode::Shared);
                            }
                            if let Some((stream, selection)) = self.try_duplex(
                                context,
                                host.default_input_device,
                                host.default_output_device,
                                true,
                                attempt.exclusive,
                                attempt.input,
                                attempt.output,
                            )? {
                                return Ok((stream, selection));
                            }
                        }
                    }
                }
            }
        }
        let input = unsafe { (self.api.default_input)() };
        let output = unsafe { (self.api.default_output)() };
        if input >= 0 && output >= 0 {
            if let Some((stream, selection)) =
                self.try_duplex(context, input, output, false, false, 1, 1)?
            {
                return Ok((stream, selection));
            }
        }
        Err("No available device pair could start the V1.8 48 kHz duplex format; see attempts above".into())
    }
    fn try_duplex(
        &self,
        context: &CallbackContext,
        input: i32,
        output: i32,
        wasapi: bool,
        exclusive: bool,
        input_channels: i32,
        output_channels: i32,
    ) -> Result<Option<(*mut c_void, StreamSelection)>, String> {
        let (Some(ii), Some(oi)) = (self.info(input), self.info(output)) else {
            return Ok(None);
        };
        if input_channels > ii.max_input_channels || output_channels > oi.max_output_channels {
            return Ok(None);
        }
        let mut input_specific = WasapiStreamInfo::exclusive(false);
        let mut output_specific = WasapiStreamInfo::exclusive(output_channels == 2);
        let mut ip = StreamParameters {
            device: input,
            channel_count: input_channels,
            sample_format: PA_FLOAT32,
            suggested_latency: ii.default_low_input_latency,
            host_api_specific_stream_info: ptr::null_mut(),
        };
        let mut op = StreamParameters {
            device: output,
            channel_count: output_channels,
            sample_format: PA_FLOAT32,
            suggested_latency: oi.default_low_output_latency,
            host_api_specific_stream_info: ptr::null_mut(),
        };
        if exclusive {
            ip.host_api_specific_stream_info =
                (&mut input_specific as *mut WasapiStreamInfo).cast();
            op.host_api_specific_stream_info =
                (&mut output_specific as *mut WasapiStreamInfo).cast();
        }
        let label = if wasapi {
            if exclusive {
                "WASAPI Exclusive"
            } else {
                "WASAPI Shared"
            }
        } else {
            "PortAudio defaults"
        };

        let code = unsafe { (self.api.format_supported)(&ip, &op, SAMPLE_RATE as f64) };
        if code != 0 {
            return Ok(None);
        }
        for (input_latency, output_latency) in latency_profiles(
            wasapi,
            exclusive,
            ii.default_low_input_latency,
            oi.default_low_output_latency,
        ) {
            ip.suggested_latency = input_latency;
            op.suggested_latency = output_latency;

            let mut stream = ptr::null_mut();
            // SAFETY: parameters live through OpenStream. Context has a stable shared owner and
            // will remain at this address until the corresponding Close joins.
            let code = unsafe {
                (self.api.open_stream)(
                    &mut stream,
                    &ip,
                    &op,
                    SAMPLE_RATE as f64,
                    0,
                    PA_CLIP_OFF,
                    Some(live_callback),
                    (context as *const CallbackContext as *mut CallbackContext).cast(),
                )
            };
            if code != 0 {
                continue;
            }
            context
                .input_channels
                .store(input_channels, Ordering::Release);
            context
                .output_channels
                .store(output_channels, Ordering::Release);
            context.exclusive.store(exclusive, Ordering::Release);
            context.generation.fetch_add(1, Ordering::Release);
            let code = unsafe { (self.api.start_stream)(stream) };
            if code != 0 {
                self.close_attempt(stream)?;
                continue;
            }
            let info = unsafe { (self.api.stream_info)(stream).as_ref().copied() };
            let Some(info) = info.filter(|v| (v.sample_rate - SAMPLE_RATE as f64).abs() <= 0.5)
            else {
                unsafe {
                    (self.api.stop_stream)(stream);
                }
                self.close_attempt(stream)?;
                continue;
            };
            let selection = StreamSelection {
                label: label.into(),
                input_name: c_text(ii.name),
                output_name: c_text(oi.name),
                input_channels,
                output_channels,
                input_requested: input_latency,
                output_requested: output_latency,
                input_reported: info.input_latency,
                output_reported: info.output_latency,
                sample_rate: info.sample_rate,
            };
            *self.endpoints.borrow_mut() = default_endpoint_ids();
            return Ok(Some((stream, selection)));
        }
        Ok(None)
    }
    fn close_attempt(&self, stream: *mut c_void) -> Result<(), String> {
        let code = unsafe { (self.api.close_stream)(stream) };
        if code == 0 {
            return Ok(());
        }
        // Failed close: caller retains the callback owner; Drop retains the DLL.
        self.module.keep_loaded.set(true);
        Err(format!(
            "Close failed; callback state retained for safety: {}",
            self.error(code)
        ))
    }
}
impl Drop for PortAudio {
    fn drop(&mut self) {
        if self.module.keep_loaded.get() {
            self._com.keep_initialized.set(true);
        } else if self.initialized.get() {
            unsafe {
                (self.api.terminate)();
            }
        }
    }
}

pub use crate::device_policy::StreamSelection;
struct PreviewState {
    playback: RecordedPlayback,
}
struct PreviewContext {
    controls: Arc<AtomicSettings>,
    state: UnsafeCell<PreviewState>,
    playing: Arc<AtomicBool>,
    in_flight: AtomicU32,
    failed: AtomicBool,
    channels: AtomicU32,
}
impl PreviewContext {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_observations(Arc::new(ObservationHub::default()))
    }
    #[cfg(test)]
    fn with_observations(observations: Arc<ObservationHub>) -> Self {
        Self::with_controls(
            observations,
            Arc::new(AtomicSettings::new(Settings::default())),
        )
    }
    fn with_controls(observations: Arc<ObservationHub>, controls: Arc<AtomicSettings>) -> Self {
        let mut playback = RecordedPlayback::default();
        playback.preload_first_minute();
        let playing = Arc::clone(&observations.preview_active);
        Self {
            controls,
            state: UnsafeCell::new(PreviewState { playback }),
            playing,
            in_flight: AtomicU32::new(0),
            failed: AtomicBool::new(false),
            channels: AtomicU32::new(1),
        }
    }
    fn disable_and_wait(&self) {
        self.playing.store(false, Ordering::SeqCst);
        while self.in_flight.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }
    /// SAFETY: the caller permits only one serialized native stream to render
    /// this state. Shared preview and Exclusive inline preview never overlap.
    /// The owner disables/waits before touching state and publishes with SeqCst.
    unsafe fn render(
        &self,
        output: Option<&mut [f32]>,
        frames: usize,
        channels: usize,
    ) -> PlaybackResult {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        struct Guard<'a>(&'a AtomicU32);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _guard = Guard(&self.in_flight);
        if !self.playing.load(Ordering::SeqCst) {
            if let Some(output) = output {
                output.fill(0.0);
            }
            return PlaybackResult::Continue;
        }
        let state = unsafe { &mut *self.state.get() };
        let result = state
            .playback
            .render(output, frames, channels, self.controls.load());
        if result == PlaybackResult::Complete {
            self.playing.store(false, Ordering::SeqCst);
        }
        result
    }
}

struct CallbackContext {
    observations: Arc<ObservationHub>,
    // Only PortAudio's serialized callback accesses this cell. Owner code never
    // obtains a reference to its DSP state while a stream can call back.
    processor: UnsafeCell<DuplexProcessor>,
    controls: Arc<AtomicSettings>,
    callback_readers: AtomicU32,
    recording: UnsafeCell<Option<RecordingInput>>,
    capture_enabled: AtomicBool,
    preview: Arc<PreviewContext>,
    input_channels: AtomicI32,
    output_channels: AtomicI32,
    exclusive: AtomicBool,
    generation: AtomicU32,
    live_active: AtomicBool,
}
impl CallbackContext {
    #[cfg(test)]
    fn new(settings: Settings, recording: Option<RecordingInput>) -> Self {
        Self::with_observations(settings, recording, Arc::new(ObservationHub::default()))
    }
    #[cfg(test)]
    fn with_observations(
        settings: Settings,
        recording: Option<RecordingInput>,
        observations: Arc<ObservationHub>,
    ) -> Self {
        Self::with_controls(
            recording,
            observations,
            Arc::new(AtomicSettings::new(settings)),
        )
    }
    fn with_controls(
        recording: Option<RecordingInput>,
        observations: Arc<ObservationHub>,
        controls: Arc<AtomicSettings>,
    ) -> Self {
        Self {
            observations: Arc::clone(&observations),
            processor: UnsafeCell::new(DuplexProcessor::default()),
            controls: Arc::clone(&controls),
            callback_readers: AtomicU32::new(0),
            recording: UnsafeCell::new(recording),
            capture_enabled: AtomicBool::new(false),
            preview: Arc::new(PreviewContext::with_controls(observations, controls)),
            input_channels: AtomicI32::new(1),
            output_channels: AtomicI32::new(1),
            exclusive: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            live_active: AtomicBool::new(false),
        }
    }
}

unsafe extern "C" fn live_callback(
    input: *const c_void,
    output: *mut c_void,
    frames: u32,
    _time: *const CallbackTimeInfo,
    flags: u32,
    userdata: *mut c_void,
) -> i32 {
    if userdata.is_null() {
        return PA_ABORT;
    }
    // SAFETY: OpenStream receives a stable shared context; its owner retains it
    // through CloseStream. Only the atomics are accessed by the owner meanwhile.
    let context = unsafe { &*userdata.cast::<CallbackContext>() };
    let callback_start = std::time::Instant::now();
    context.callback_readers.fetch_add(1, Ordering::SeqCst);
    struct CallbackReader<'a>(&'a AtomicU32);
    impl Drop for CallbackReader<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _callback_reader = CallbackReader(&context.callback_readers);
    let input_channels = context.input_channels.load(Ordering::Acquire) as usize;
    let output_channels = context.output_channels.load(Ordering::Acquire) as usize;
    let count = frames as usize;
    if !matches!(input_channels, 1 | 2)
        || !matches!(output_channels, 1 | 2)
        || count > isize::MAX as usize / 8
    {
        return PA_ABORT;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: PortAudio supplies float32 interleaved buffers of the configured
        // channel count for this call; input/output are disjoint for duplex.
        let input = if input.is_null() {
            None
        } else {
            Some(unsafe { std::slice::from_raw_parts(input.cast::<f32>(), count * input_channels) })
        };
        let output = if output.is_null() {
            None
        } else {
            Some(unsafe {
                std::slice::from_raw_parts_mut(output.cast::<f32>(), count * output_channels)
            })
        };
        // SAFETY: PortAudio serializes callbacks for this one stream. No owner
        // code or second stream ever accesses this processor while it is active.
        let processor = unsafe { &mut *context.processor.get() };
        // SAFETY: like the DSP cell, this producer is accessed only by this
        // stream's serialized callback. The collector owns the other endpoint.
        let enabled_at_entry = context.capture_enabled.load(Ordering::SeqCst);
        // No reference into the producer cell is formed while admission is off.
        // The owner may replace the endpoint only after old readers have left.
        let recording = if enabled_at_entry {
            unsafe { &mut *context.recording.get() }.as_mut()
        } else {
            None
        };
        let (control, mut producer) = match recording {
            Some(input) => (Some(&input.control), Some(&mut input.producer)),
            None => (None, None),
        };
        let guard = control.map(|control| control.enter_callback());
        processor.process_host_observed(
            input,
            output,
            count,
            input_channels,
            output_channels,
            context.exclusive.load(Ordering::Acquire),
            context.generation.load(Ordering::Acquire),
            || {
                (
                    context.live_active.load(Ordering::Acquire),
                    context.controls.load(),
                )
            },
            |samples| {
                if enabled_at_entry {
                    if let (Some(guard), Some(producer)) = (&guard, producer.as_deref_mut()) {
                        if guard.recording_at_entry {
                            guard.queue_if_recording(producer, samples);
                        }
                    }
                }
            },
            |output, frames, channels| {
                // SAFETY: this route is used only for actual Exclusive mode;
                // no separate Shared output stream exists at the same time.
                unsafe {
                    context.preview.render(output, frames, channels);
                }
            },
            |samples| {
                context.observations.rings.try_push(samples);
            },
        );
    }));
    if result.is_err() {
        if !output.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(output.cast::<f32>(), count * output_channels)
            }
            .fill(0.0);
        }
        return PA_ABORT;
    }
    if context.live_active.load(Ordering::Acquire) && (frames > 256 || !input.is_null()) {
        let micros = callback_start.elapsed().as_micros().min(u64::MAX as u128) as u64;
        // This duration is still required by the original visual-pressure
        // policy. Host timestamps and report-only counters are not collected.
        context.observations.audio.observe(
            frames as u64,
            micros,
            SAMPLE_RATE as f64,
            flags & 2 != 0,
            flags & 4 != 0,
        );
    }
    PA_CONTINUE
}

/// The same preview context is referenced by the duplex owner, but rendering
/// takes this route only in actual Shared mode. Exclusive never opens this stream.
unsafe extern "C" fn preview_callback(
    _input: *const c_void,
    output: *mut c_void,
    frames: u32,
    _time: *const CallbackTimeInfo,
    _flags: u32,
    userdata: *mut c_void,
) -> i32 {
    if userdata.is_null() {
        return PA_ABORT;
    }
    let preview = unsafe { &*userdata.cast::<PreviewContext>() };
    let channels = preview.channels.load(Ordering::Acquire) as usize;
    if !matches!(channels, 1 | 2) || frames as usize > isize::MAX as usize / 8 {
        preview.failed.store(true, Ordering::Release);
        return PA_ABORT;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let buffer = if output.is_null() {
            None
        } else {
            Some(unsafe {
                std::slice::from_raw_parts_mut(output.cast::<f32>(), frames as usize * channels)
            })
        };
        // SAFETY: the owner never starts a second preview stream or the inline
        // Exclusive renderer while this Shared stream can run.
        unsafe { preview.render(buffer, frames as usize, channels) }
    }));
    match result {
        Ok(PlaybackResult::Continue) => PA_CONTINUE,
        Ok(PlaybackResult::Complete) => 1, // paComplete: submit final padded block.
        Err(_) => {
            preview.failed.store(true, Ordering::Release);
            preview.playing.store(false, Ordering::SeqCst);
            if !output.is_null() {
                unsafe {
                    std::slice::from_raw_parts_mut(output.cast::<f32>(), frames as usize * channels)
                }
                .fill(0.0);
            }
            PA_ABORT
        }
    }
}

/// Borrowed native stream owner. Stop/close callbacks before touching their mutable DSP or recording state.
pub struct LiveStream<'a> {
    owner: &'a PortAudio,
    stream: *mut c_void,
    context: Option<Arc<CallbackContext>>,
    preview_stream: *mut c_void,
    preview_cache: Option<PreviewCacheKey>,
    recording_control: Option<Arc<crate::recording::RecordingControl>>,
    pub selection: StreamSelection,
}
impl LiveStream<'_> {
    /// Check whether this owner still holds a native stream pointer.
    pub fn is_open(&self) -> bool {
        !self.stream.is_null()
    }
    /// Query PortAudio activity on the device owner; propagate native query errors.
    pub fn is_active(&self) -> Result<bool, String> {
        if self.stream.is_null() {
            return Ok(false);
        }
        let code = unsafe { (self.owner.api.stream_active)(self.stream) };
        if code < 0 {
            Err(self.owner.error(code))
        } else {
            Ok(code == 1)
        }
    }
    /// Read the atomic live-output gate, independent of whether duplex capture is open.
    pub fn live_active(&self) -> bool {
        self.context
            .as_ref()
            .is_some_and(|c| c.live_active.load(Ordering::Acquire))
    }
    /// Read recording admission; a recording prevents device reopening.
    pub fn recording_active(&self) -> bool {
        self.recording_control
            .as_ref()
            .is_some_and(|c| c.is_recording())
    }
    /// Publish independent atomic controls without borrowing the callback processor.
    pub fn update_settings(&self, settings: Settings) {
        if let Some(context) = &self.context {
            context.controls.store(settings);
            context.preview.controls.store(settings);
        }
    }
    /// Change live routing on the owner after enforcing recording/preview transition rules.
    pub fn set_live(&mut self, active: bool) -> Result<(), String> {
        if active && self.recording_active() {
            return Err("Stop recording before Stream mode".into());
        }
        if active {
            self.stop_preview()?;
        }
        if self.stream.is_null() {
            return Err("Audio is unavailable; reopen devices first".into());
        }
        self.context
            .as_ref()
            .ok_or("Callback context unavailable")?
            .live_active
            .store(active, Ordering::Release);
        Ok(())
    }
    /// Start a collector and attach its producer after the inherited capture admission handshake.
    pub fn begin_recording(&mut self) -> Result<RecordingSession, String> {
        self.begin_recording_reusing(None)
    }
    /// Reuse the previous queue/take capacity only after its producer and collector have stopped.
    pub(crate) fn begin_recording_reusing(
        &mut self,
        previous: Option<RecordedTake>,
    ) -> Result<RecordingSession, String> {
        if self.live_active() {
            return Err("Switch to Recording mode before starting a take".into());
        }
        if self.recording_active() {
            return Err("Recording is already active".into());
        }
        if !self.is_active()? {
            return Err("Duplex input is unavailable".into());
        }
        if self.owner.module.keep_loaded.get() {
            return Err("Native close failed; restart the application".into());
        }
        self.stop_preview()?;
        let context = self
            .context
            .as_ref()
            .ok_or("Callback context unavailable")?;
        context.capture_enabled.store(false, Ordering::SeqCst);
        while context.callback_readers.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
        // SAFETY: admission is off and every older callback has left; new
        // callbacks skip this cell entirely until the final publication below.
        let slot = unsafe { &mut *context.recording.get() };
        let queue = if let Some(old) = slot.take() {
            match old.producer.try_reclaim() {
                Ok(queue) => queue,
                Err(producer) => {
                    *slot = Some(RecordingInput {
                        producer,
                        control: old.control,
                    });
                    return Err(
                        "The previous recording collector has not released its queue".into(),
                    );
                }
            }
        } else {
            crate::recording_queue::RecordingQueue::default()
        };
        let (session, input) = RecordingSession::begin_reusing(queue, previous)?;
        self.recording_control = Some(Arc::clone(&input.control));
        *slot = Some(input);
        context.capture_enabled.store(true, Ordering::SeqCst);
        Ok(session)
    }
    /// Close only the native streams, retaining live DSP histories and the take.
    /// A mode-only switch refreshes enumeration only if Windows IDs changed or
    /// became unknown. Explicit Reopen always requests full enumeration.
    pub fn reopen(&mut self, mode: RequestedMode, refresh_devices: bool) -> Result<bool, String> {
        if self.recording_active() {
            return Err("Stop recording before reopening audio".into());
        }
        self.owner.last_requested.set(mode);
        let restore_live = self.live_active();
        let refresh = refresh_devices
            || !self.is_open()
            || self.owner.device_list_needs_refresh()
            || !self.owner.initialized.get();
        self.close()?;
        self.selection.label = "No Audio".into();
        if refresh {
            self.owner.refresh_device_list()?;
        }
        let context = self
            .context
            .as_ref()
            .ok_or("Callback context unavailable after close")?;
        let (stream, selection) = self.owner.open_live_inner(mode, context)?;
        self.owner
            .live_streams
            .set(self.owner.live_streams.get() + 1);
        self.stream = stream;
        self.selection = selection;
        context.live_active.store(restore_live, Ordering::Release);
        Ok(refresh)
    }
    /// Read the shared atomic preview-completion flag.
    pub fn preview_playing(&self) -> bool {
        self.context
            .as_ref()
            .is_some_and(|c| c.preview.playing.load(Ordering::SeqCst))
    }
    /// Read the callback failure flag without borrowing the preview processor.
    pub fn preview_failed(&self) -> bool {
        self.context
            .as_ref()
            .is_some_and(|c| c.preview.failed.load(Ordering::Acquire))
    }
    /// One selected playback. Retain and restart a stopped Shared stream only
    /// when both its WASAPI index and opaque Windows endpoint ID still match.
    pub fn start_preview(&mut self, take: &RecordedTake, settings: Settings) -> Result<(), String> {
        if self.owner.module.keep_loaded.get() {
            return Err("Native close failed; restart the application".into());
        }
        if self.live_active() {
            return Err("Recorded playback is unavailable in Stream mode".into());
        }
        if self.recording_active() {
            return Err("Stop recording before selected playback".into());
        }
        if self.preview_playing() {
            return Err("Selected playback is already active".into());
        }
        self.stop_preview()?;
        let context = self
            .context
            .as_ref()
            .ok_or("Callback context unavailable")?;
        let exclusive = context.exclusive.load(Ordering::Acquire);
        let preview = Arc::clone(&context.preview);
        let wasapi_index = unsafe { (self.owner.api.host_index)(PA_WASAPI) };
        let wasapi_output = if wasapi_index >= 0 {
            unsafe { (self.owner.api.host_info)(wasapi_index).as_ref() }
                .map(|h| h.default_output_device)
                .unwrap_or(-1)
        } else {
            -1
        };
        let endpoint = default_endpoint_id(false);
        if !self.preview_stream.is_null()
            && (exclusive
                || !self.preview_cache.as_ref().is_some_and(|key| {
                    key.matches(
                        wasapi_output,
                        endpoint.as_deref(),
                        self.selection.input_channels,
                    )
                }))
        {
            self.close_preview_handle()?;
        }
        // SAFETY: old rendering was disabled and joined/waited before preparing
        // this buffer; only &mut LiveStream can run owner-side transitions.
        let state = unsafe { &mut *preview.state.get() };
        state.playback.stop_after_join();
        if !take.prepare_playback(&mut state.playback)? {
            return Err("The recording is empty".into());
        }
        if !self.owner.has_shared_controls() {
            preview.controls.store(settings);
        }
        preview.failed.store(false, Ordering::Release);
        if exclusive {
            if !self.is_active()? {
                return Err("Exclusive duplex output is unavailable".into());
            }
            preview
                .channels
                .store(self.selection.output_channels as u32, Ordering::Release);
            preview.playing.store(true, Ordering::SeqCst);

            return Ok(());
        }
        if !self.preview_stream.is_null() {
            preview.playing.store(true, Ordering::SeqCst);
            let code = unsafe { (self.owner.api.start_stream)(self.preview_stream) };
            if code == 0 {
                return Ok(());
            }
            preview.disable_and_wait();
            self.close_preview_handle()?;
        }
        let default_output = unsafe { (self.owner.api.default_output)() };
        for (index, device) in [wasapi_output, default_output].into_iter().enumerate() {
            if device < 0 || (index == 1 && device == wasapi_output) {
                continue;
            }
            let Some(info) = self.owner.info(device) else {
                continue;
            };
            let channels = self.selection.input_channels;
            let parameters = StreamParameters {
                device,
                channel_count: channels,
                sample_format: PA_FLOAT32,
                suggested_latency: info.default_low_output_latency,
                host_api_specific_stream_info: ptr::null_mut(),
            };
            if unsafe {
                (self.owner.api.format_supported)(ptr::null(), &parameters, SAMPLE_RATE as f64)
            } != 0
            {
                continue;
            }
            let mut stream = ptr::null_mut();
            preview.channels.store(channels as u32, Ordering::Release);
            let code = unsafe {
                (self.owner.api.open_stream)(
                    &mut stream,
                    ptr::null(),
                    &parameters,
                    SAMPLE_RATE as f64,
                    0,
                    PA_CLIP_OFF,
                    Some(preview_callback),
                    Arc::as_ptr(&preview).cast_mut().cast(),
                )
            };
            if code != 0 {
                continue;
            }
            self.preview_stream = stream;
            let rate_ok = unsafe { (self.owner.api.stream_info)(stream).as_ref() }
                .is_some_and(|v| (v.sample_rate - SAMPLE_RATE as f64).abs() <= 0.5);
            if rate_ok {
                preview.playing.store(true, Ordering::SeqCst);
                let code = unsafe { (self.owner.api.start_stream)(stream) };
                if code == 0 {
                    self.preview_cache = if device == wasapi_output {
                        endpoint
                            .as_ref()
                            .filter(|id| !id.is_empty())
                            .map(|id| PreviewCacheKey {
                                device,
                                endpoint: id.clone(),
                                channels,
                            })
                    } else {
                        None
                    };

                    return Ok(());
                }
            }
            self.close_preview_handle()?;
        }
        Err("No available 48 kHz output could start selected playback".into())
    }
    /// Disable/wait and abort to make a naturally completed paComplete stream
    /// restartable. Keep its handle/cache on normal stop, matching Audio.cpp.
    pub fn stop_preview(&mut self) -> Result<(), String> {
        let Some(context) = &self.context else {
            return Ok(());
        };
        context.preview.disable_and_wait();
        if !self.preview_stream.is_null() {
            let code = unsafe { (self.owner.api.abort_stream)(self.preview_stream) };
            if code != 0 && code != -9983 {
                self.close_preview_handle()?;
            } // paStreamIsStopped
        }
        let Some(context) = &self.context else {
            return Ok(());
        };
        unsafe { &mut *context.preview.state.get() }
            .playback
            .stop_after_join();
        Ok(())
    }
    fn close_preview_handle(&mut self) -> Result<(), String> {
        if self.preview_stream.is_null() {
            self.preview_cache = None;
            return Ok(());
        }
        let context = self
            .context
            .as_ref()
            .ok_or("Callback context unavailable")?;
        let preview = &context.preview;
        preview.disable_and_wait();
        unsafe {
            (self.owner.api.abort_stream)(self.preview_stream);
        }
        let code = unsafe { (self.owner.api.close_stream)(self.preview_stream) };
        self.preview_stream = ptr::null_mut();
        self.preview_cache = None;
        if code != 0 {
            self.owner.module.keep_loaded.set(true);
            std::mem::forget(Arc::clone(preview));
            return Err(format!(
                "Playback close failed; state retained: {}",
                self.owner.error(code)
            ));
        }
        Ok(())
    }
    fn close_preview(&mut self) -> Result<(), String> {
        let error = self.stop_preview().err();
        self.close_preview_handle()?;
        error.map_or(Ok(()), Err)
    }
    fn close(&mut self) -> Result<(), String> {
        let preview_error = self.close_preview().err();
        if self.stream.is_null() {
            return preview_error.map_or(Ok(()), Err);
        }
        if let Some(context) = &self.context {
            context.live_active.store(false, Ordering::Release);
            context.capture_enabled.store(false, Ordering::SeqCst);
        }
        // Device operations stay on the owner thread. Close joins the callback
        // before the Rust context is released, including when Abort says stopped.
        unsafe {
            (self.owner.api.abort_stream)(self.stream);
        }
        let code = unsafe { (self.owner.api.close_stream)(self.stream) };
        self.stream = ptr::null_mut();
        self.owner
            .live_streams
            .set(self.owner.live_streams.get().saturating_sub(1));
        if code != 0 {
            self.owner.module.keep_loaded.set(true);
            if let Some(context) = self.context.take() {
                std::mem::forget(context);
            }
            return Err(format!(
                "Close failed; DLL and context retained: {}",
                self.owner.error(code)
            ));
        }
        preview_error.map_or(Ok(()), Err)
    }
    /// Close the native stream. A native close failure still retains unsafe-to-free callback state.
    pub fn finish(mut self) -> Result<(), String> {
        self.close()?;
        Ok(())
    }
}
impl Drop for LiveStream<'_> {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod callback_tests {
    use super::*;
    use crate::{capture::CaptureProcessor, pcm::capture_pcm16_sample};

    #[test]
    fn gui_atomic_controls_reach_both_callbacks_and_completion_without_refcount_work() {
        let hub = Arc::new(ObservationHub::default());
        let controls = Arc::new(AtomicSettings::new(Settings::default()));
        let context = CallbackContext::with_controls(None, Arc::clone(&hub), Arc::clone(&controls));
        assert!(Arc::ptr_eq(&context.controls, &context.preview.controls));
        context.live_active.store(true, Ordering::Release);
        let input = [0.2_f32; 256];
        let mut output = [0.0_f32; 256];
        let count = Arc::strong_count(&controls);
        let userdata = (&context as *const CallbackContext).cast_mut().cast();
        unsafe {
            live_callback(
                input.as_ptr().cast(),
                output.as_mut_ptr().cast(),
                256,
                ptr::null(),
                0,
                userdata,
            );
        }
        assert!(output.iter().any(|v| v.abs() > 0.001));
        controls.store(Settings {
            volume: 0.0,
            ..Settings::default()
        });
        unsafe {
            live_callback(
                input.as_ptr().cast(),
                output.as_mut_ptr().cast(),
                256,
                ptr::null(),
                0,
                userdata,
            );
        }
        assert!(output.iter().all(|v| *v == 0.0));
        assert_eq!(Arc::strong_count(&controls), count);
        let preview = &context.preview;
        assert!(unsafe { &mut *preview.state.get() }
            .playback
            .start_after_join(&[0, 64, 0, 32]));
        preview.playing.store(true, Ordering::SeqCst);
        assert!(hub.preview_active.load(Ordering::Acquire));
        let code = unsafe {
            preview_callback(
                ptr::null(),
                output.as_mut_ptr().cast(),
                256,
                ptr::null(),
                0,
                Arc::as_ptr(preview).cast_mut().cast(),
            )
        };
        assert_eq!(code, 1);
        assert!(output.iter().all(|v| *v == 0.0));
        assert!(!hub.preview_active.load(Ordering::Acquire));
        assert_eq!(Arc::strong_count(&controls), count);
    }

    #[test]
    fn ffi_duplex_callback_reaches_collector_with_original_chunk_boundaries() {
        let (session, input) = RecordingSession::begin().unwrap();
        let context = CallbackContext::new(Settings::default(), Some(input));
        context.input_channels.store(2, Ordering::Release);
        context.output_channels.store(2, Ordering::Release);
        context.exclusive.store(true, Ordering::Release);
        context.capture_enabled.store(true, Ordering::SeqCst);
        let source: Vec<f32> = (0..624 * 2)
            .map(|i| (i % 67) as f32 / 200.0 - 0.1)
            .collect();
        let mut output = vec![9.0_f32; 624 * 2];
        // SAFETY: stable context, correctly sized disjoint buffers, serialized
        // call, no device or background thread holds a native callback pointer.
        let code = unsafe {
            live_callback(
                source.as_ptr().cast(),
                output.as_mut_ptr().cast(),
                624,
                ptr::null(),
                0,
                (&context as *const CallbackContext).cast_mut().cast(),
            )
        };
        assert_eq!(code, PA_CONTINUE);
        assert!(output.iter().all(|&v| v == 0.0));
        let take = session.finish().unwrap();
        let mut capture = CaptureProcessor::default();
        let mut expected = Vec::new();
        for chunk in source.chunks(256 * 2) {
            let mut clean = vec![0.0; chunk.len() / 2];
            capture.process_chunk(chunk, 2, true, 0, &mut clean);
            for value in clean {
                expected.extend_from_slice(&capture_pcm16_sample(value).to_le_bytes());
            }
        }
        assert_eq!(take.snapshot_for_playback().unwrap(), expected);
        assert!(context.observations.rings.snapshot_waveform().is_some());
    }

    #[test]
    fn ffi_preview_submits_complete_block_and_disabled_route_is_silent() {
        let preview = PreviewContext::new();
        let original = vec![0, 64, 0, 32]; // Two mono samples: 0.5 and 0.25.
                                           // SAFETY: no native stream exists and no renderer is active.
        assert!(unsafe { &mut *preview.state.get() }
            .playback
            .start_after_join(&original));
        preview.channels.store(2, Ordering::Release);
        preview.playing.store(true, Ordering::SeqCst);
        let mut output = [9.0_f32; 16];
        let userdata = (&preview as *const PreviewContext).cast_mut().cast();
        // SAFETY: valid context and 8 stereo frames, only this serialized caller.
        let code = unsafe {
            preview_callback(
                ptr::null(),
                output.as_mut_ptr().cast(),
                8,
                ptr::null(),
                0,
                userdata,
            )
        };
        assert_eq!(code, 1);
        assert_eq!(&output[..4], &[0.5, 0.5, 0.25, 0.25]);
        assert!(output[4..].iter().all(|&v| v == 0.0));
        assert!(!preview.playing.load(Ordering::SeqCst));
        preview.disable_and_wait();
        output.fill(9.0);
        let code = unsafe {
            preview_callback(
                ptr::null(),
                output.as_mut_ptr().cast(),
                8,
                ptr::null(),
                0,
                userdata,
            )
        };
        assert_eq!(code, PA_CONTINUE);
        assert!(output.iter().all(|&v| v == 0.0));
        assert_eq!(preview.in_flight.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn live_host_pressure_is_published_once_and_stereo_output_is_preserved() {
        let context = CallbackContext::new(Settings::default(), None);
        context.live_active.store(true, Ordering::Release);
        context.output_channels.store(2, Ordering::Release);
        let input = [0.05_f32; 624];
        let mut output = [0.0_f32; 1248];
        let time = CallbackTimeInfo {
            input_adc_time: 1.0,
            current_time: 1.03,
            output_dac_time: 1.054,
        };
        let code = unsafe {
            live_callback(
                input.as_ptr().cast(),
                output.as_mut_ptr().cast(),
                624,
                &time,
                2 | 4,
                (&context as *const CallbackContext).cast_mut().cast(),
            )
        };
        assert_eq!(code, PA_CONTINUE);
        assert_eq!(context.observations.audio.take_visual_pressure_events(), 1);
        assert!(output.chunks_exact(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn null_preview_output_keeps_take_position_and_callback_lifetime_balanced() {
        let preview = PreviewContext::new();
        assert!(unsafe { &mut *preview.state.get() }
            .playback
            .start_after_join(&[0, 64]));
        preview.playing.store(true, Ordering::SeqCst);
        let userdata = (&preview as *const PreviewContext).cast_mut().cast();
        let code =
            unsafe { preview_callback(ptr::null(), ptr::null_mut(), 8, ptr::null(), 0, userdata) };
        assert_eq!(code, PA_CONTINUE);
        assert!(preview.playing.load(Ordering::SeqCst));
        let mut output = [0.0_f32; 8];
        assert_eq!(
            unsafe {
                preview_callback(
                    ptr::null(),
                    output.as_mut_ptr().cast(),
                    8,
                    ptr::null(),
                    0,
                    userdata,
                )
            },
            1
        );
        assert_eq!(output[0], 0.5);
        assert_eq!(preview.in_flight.load(Ordering::SeqCst), 0);
    }
}
