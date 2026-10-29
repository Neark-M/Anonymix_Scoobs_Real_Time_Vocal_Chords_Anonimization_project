//! Anonymix Scoobs V1.8 Rust port: the silent public application.
//!
//! The Windows SDL3 desktop is connected to the previous audio/device and
//! analysis modules. Original DSP and visual geometry remain reference-based.
//! Development report counters/probes are removed. The visual-pressure signal
//! remains because adaptive rendering consumes it; see CODE_MAP.md.

pub mod audio_pressure;
pub mod capture;
pub mod controls;
#[cfg(all(windows, target_pointer_width = "64"))]
pub mod desktop;
pub mod device_plan;
pub mod device_policy;
pub mod device_worker;
pub mod display_worker;
pub mod duplex;
pub mod effects;
pub mod live;
pub mod native_library;
pub mod observation;
pub mod pcm;
pub mod playback;
pub mod portaudio_abi;
#[cfg(all(windows, target_pointer_width = "64"))]
pub mod portaudio_windows;
pub mod recorded;
pub mod recording;
pub mod recording_queue;
pub mod sdl_abi;
pub mod sdl_api;
pub mod spectrum;
pub mod take;
pub mod ui_animation;
pub mod ui_budget;
pub mod ui_controller;
pub mod ui_geometry;
pub mod ui_layout;
pub mod ui_raster;
pub mod ui_renderer;
pub mod ui_timing;
pub mod ui_widgets;
pub mod wav;
#[cfg(all(windows, target_pointer_width = "64"))]
pub mod windows_endpoints;

/// These are V1.8 constants, not tunable quality settings introduced by the port.
pub const SAMPLE_RATE: f32 = 48_000.0;
pub const STREAM_BLOCK_SIZE: usize = 256;
pub const SELECTED_BLOCK_SIZE: usize = 128;
pub const EFFECT_PITCH: u8 = 1;
pub const EFFECT_FORMANT: u8 = 2;
pub const EFFECT_TIMBRE: u8 = 4;
pub const EFFECT_PROSODY: u8 = 8;
pub const EFFECT_JITTER: u8 = 16;

/// Per-block values read from the application's independent AtomicSettings.
/// DSP functions consume these plain values without acquiring a control mutex.
#[derive(Clone, Copy)]
pub struct Settings {
    /// Effect bit mask: Pitch 1, Formant 2, Timbre 4, Prosody 8, Jitter 16.
    pub mask: u8,
    /// Pitch offset in semitones, with zero as the neutral setting.
    pub pitch: f32,
    /// Formant-labelled variable-delay ratio, with 1.0 as the neutral setting.
    pub formant: f32,
    /// Signed Timbre slider percentage; zero leaves spectral balance neutral.
    pub timbre: f32,
    /// Signed Prosody gesture percentage; zero disables its audible modulation.
    pub prosody: f32,
    /// Pitch-ratio instability amount in the original 0.0..0.020 range.
    pub jitter: f32,
    /// Linear output multiplier. The desktop starts at 0.5, unlike this neutral test default.
    pub volume: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mask: 0,
            pitch: 0.0,
            formant: 1.0,
            timbre: 0.0,
            prosody: 0.0,
            jitter: 0.0,
            volume: 1.0,
        }
    }
}

/// Preserve the inherited smoothing time constant; 16 kHz is only a reference
/// in this exponent. Every signal sample still belongs to a 48 kHz stream.
pub fn rescale_legacy_smoothing(coefficient: f32) -> f32 {
    coefficient.powf(16_000.0 / SAMPLE_RATE)
}
