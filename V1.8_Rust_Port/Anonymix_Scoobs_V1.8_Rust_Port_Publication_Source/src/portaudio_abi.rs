//! Minimal PortAudio V19 / Windows x64 C layouts used by the live test adapter.
//! Windows C unsigned long is 32 bits, even in a 64-bit process. These structs
//! deliberately use u32, not the Linux c_ulong alias. Native calls are Windows-only.
use std::ffi::{c_char, c_void};
pub const PA_WASAPI: i32 = 13;
pub const PA_FLOAT32: u32 = 1;
pub const PA_CLIP_OFF: u32 = 1;
pub const PA_CONTINUE: i32 = 0;
pub const PA_ABORT: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
/// PortAudio device-info C layout. String pointers are borrowed from the initialized library.
pub struct DeviceInfo {
    pub struct_version: i32,
    pub name: *const c_char,
    pub host_api: i32,
    pub max_input_channels: i32,
    pub max_output_channels: i32,
    pub default_low_input_latency: f64,
    pub default_low_output_latency: f64,
    pub default_high_input_latency: f64,
    pub default_high_output_latency: f64,
    pub default_sample_rate: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
/// PortAudio host-API C layout, including that host's default native device indexes.
pub struct HostApiInfo {
    pub struct_version: i32,
    pub api_type: i32,
    pub name: *const c_char,
    pub device_count: i32,
    pub default_input_device: i32,
    pub default_output_device: i32,
}
#[repr(C)]
/// PortAudio interleaved float32 stream parameters, borrowed for the open/format call.
pub struct StreamParameters {
    pub device: i32,
    pub channel_count: i32,
    pub sample_format: u32,
    pub suggested_latency: f64,
    pub host_api_specific_stream_info: *mut c_void,
}
#[repr(C)]
#[derive(Clone, Copy)]
/// PortAudio-reported rate and buffering delays in seconds, not a measured microphone-to-speaker delay.
pub struct StreamInfo {
    pub struct_version: i32,
    pub input_latency: f64,
    pub output_latency: f64,
    pub sample_rate: f64,
}
#[repr(C)]
/// PortAudio timestamps in seconds for this callback. Their difference is only a host observation.
pub struct CallbackTimeInfo {
    pub input_adc_time: f64,
    pub current_time: f64,
    pub output_dac_time: f64,
}
pub type Callback = unsafe extern "C" fn(
    *const c_void,
    *mut c_void,
    u32,
    *const CallbackTimeInfo,
    u32,
    *mut c_void,
) -> i32;
pub type HostProcessor = unsafe extern "C" fn(*mut c_void, i32, *mut c_void, i32, *mut c_void);
#[repr(C)]
/// Windows x64 WASAPI extension layout, including Exclusive and channel-mask flags.
pub struct WasapiStreamInfo {
    pub size: u32,
    pub host_api_type: i32,
    pub version: u32,
    pub flags: u32,
    pub channel_mask: u32,
    pub host_processor_output: Option<HostProcessor>,
    pub host_processor_input: Option<HostProcessor>,
    pub thread_priority: i32,
    pub stream_category: i32,
    pub stream_option: i32,
}
impl WasapiStreamInfo {
    /// Same version-1 descriptor as WasapiPortAudio.h, including stereo FL/FR mask.
    pub fn exclusive(stereo_output: bool) -> Self {
        Self {
            size: std::mem::size_of::<Self>() as u32,
            host_api_type: PA_WASAPI,
            version: 1,
            flags: 1 | if stereo_output { 4 } else { 0 },
            channel_mask: if stereo_output { 3 } else { 0 },
            host_processor_output: None,
            host_processor_input: None,
            thread_priority: 0,
            stream_category: 0,
            stream_option: 0,
        }
    }
}
#[cfg(target_pointer_width = "64")]
const _: [(); 56] = [(); std::mem::size_of::<WasapiStreamInfo>()];
#[cfg(target_pointer_width = "64")]
const _: [(); 32] = [(); std::mem::size_of::<StreamParameters>()];
#[cfg(target_pointer_width = "64")]
const _: [(); 72] = [(); std::mem::size_of::<DeviceInfo>()];
#[cfg(target_pointer_width = "64")]
const _: [(); 32] = [(); std::mem::size_of::<HostApiInfo>()];
