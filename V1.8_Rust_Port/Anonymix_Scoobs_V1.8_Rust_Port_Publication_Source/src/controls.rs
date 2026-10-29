//! Independent atomic controls from State.cpp, represented by f32 bit patterns.
//! This is not a transactional settings snapshot; C++ also reads independent
//! atomics. Mutable sliders/selectors never require an audio-thread mutex.
use crate::Settings;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// Independent callback controls. Floats are stored as IEEE-754 bits in AtomicU32.
/// A load may observe values from different UI updates; this is intentionally not a transaction.
pub struct AtomicSettings {
    mask: AtomicU8,
    pitch: AtomicU32,
    formant: AtomicU32,
    timbre: AtomicU32,
    prosody: AtomicU32,
    jitter: AtomicU32,
    volume: AtomicU32,
}
impl AtomicSettings {
    /// Initialize all controls before sharing their owner with an audio callback.
    pub fn new(settings: Settings) -> Self {
        Self {
            mask: AtomicU8::new(settings.mask),
            pitch: AtomicU32::new(settings.pitch.to_bits()),
            formant: AtomicU32::new(settings.formant.to_bits()),
            timbre: AtomicU32::new(settings.timbre.to_bits()),
            prosody: AtomicU32::new(settings.prosody.to_bits()),
            jitter: AtomicU32::new(settings.jitter.to_bits()),
            volume: AtomicU32::new(settings.volume.to_bits()),
        }
    }
    /// Publish the numeric values, then the selection mask. Never locks or queues device work.
    pub fn store(&self, settings: Settings) {
        self.pitch
            .store(settings.pitch.to_bits(), Ordering::Relaxed);
        self.formant
            .store(settings.formant.to_bits(), Ordering::Relaxed);
        self.timbre
            .store(settings.timbre.to_bits(), Ordering::Relaxed);
        self.prosody
            .store(settings.prosody.to_bits(), Ordering::Relaxed);
        self.jitter
            .store(settings.jitter.to_bits(), Ordering::Relaxed);
        self.volume
            .store(settings.volume.to_bits(), Ordering::Relaxed);
        self.mask.store(settings.mask, Ordering::Release);
    }
    /// Read the current independent controls without borrowing the UI or taking a mutex.
    pub fn load(&self) -> Settings {
        Settings {
            mask: self.mask.load(Ordering::Acquire),
            pitch: f32::from_bits(self.pitch.load(Ordering::Relaxed)),
            formant: f32::from_bits(self.formant.load(Ordering::Relaxed)),
            timbre: f32::from_bits(self.timbre.load(Ordering::Relaxed)),
            prosody: f32::from_bits(self.prosody.load(Ordering::Relaxed)),
            jitter: f32::from_bits(self.jitter.load(Ordering::Relaxed)),
            volume: f32::from_bits(self.volume.load(Ordering::Relaxed)),
        }
    }
}
