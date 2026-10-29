//! Fixed display histories from State.cpp/Stream.cpp. The audio writer uses
//! try_lock and drops only a visual update when a reader owns its mutex.
use crate::{audio_pressure::AudioPressure, spectrum::FFT_SIZE, STREAM_BLOCK_SIZE};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
pub const WAVEFORM_SAMPLES: usize = 2048;
struct Ring {
    samples: Vec<f32>,
    written: usize,
    available: usize,
}
/// Separate fixed microphone histories for waveform and FFT. Only visual updates may be dropped.
pub struct DisplayRings {
    waveform: Mutex<Ring>,
    fft: Mutex<Ring>,
    waveform_ready: AtomicBool,
}
impl Default for DisplayRings {
    fn default() -> Self {
        Self {
            waveform: Mutex::new(Ring {
                samples: vec![0.0; WAVEFORM_SAMPLES],
                written: 0,
                available: 0,
            }),
            // App.cpp preloads one zero window at startup, not an empty FFT.
            fft: Mutex::new(Ring {
                samples: vec![0.0; FFT_SIZE * 2],
                written: FFT_SIZE,
                available: FFT_SIZE,
            }),
            waveform_ready: AtomicBool::new(false),
        }
    }
}
impl DisplayRings {
    #[cfg(test)]
    /// Hold the FFT reader lock while testing that the callback never waits for visualization.
    pub(crate) fn with_fft_locked_for_test(&self, action: impl FnOnce()) {
        let _lock = self.fft.lock().unwrap();
        action();
    }
    /// Called once per cleaned chunk in both Recording and Stream modes. Never
    /// allocates, waits for a lock or wakes the display analysis thread.
    pub fn try_push(&self, cleaned: &[f32]) {
        assert!(cleaned.len() <= STREAM_BLOCK_SIZE);
        if let Ok(mut ring) = self.waveform.try_lock() {
            for &sample in cleaned {
                let written = ring.written;
                ring.samples[written] = sample;
                ring.written = (written + 1) % WAVEFORM_SAMPLES;
            }
            self.waveform_ready.store(true, Ordering::Release);
        }
        if let Ok(mut ring) = self.fft.try_lock() {
            let capacity = ring.samples.len();
            if capacity != 0 {
                let first = cleaned.len().min(capacity - ring.written);
                let written = ring.written;
                ring.samples[written..written + first].copy_from_slice(&cleaned[..first]);
                ring.samples[..cleaned.len() - first].copy_from_slice(&cleaned[first..]);
                ring.written = (written + cleaned.len()) % capacity;
                ring.available = capacity.min(ring.available + cleaned.len());
            }
        }
    }
    /// Copy the latest full chronological FFT window on a non-audio thread. May wait for its mutex.
    pub fn snapshot_fft(&self, output: &mut [f32; FFT_SIZE]) -> bool {
        let Ok(ring) = self.fft.lock() else {
            return false;
        };
        if ring.available < FFT_SIZE || ring.samples.is_empty() {
            return false;
        }
        let capacity = ring.samples.len();
        let oldest = (ring.written + capacity - FFT_SIZE) % capacity;
        let first = FFT_SIZE.min(capacity - oldest);
        output[..first].copy_from_slice(&ring.samples[oldest..oldest + first]);
        output[first..].copy_from_slice(&ring.samples[..FFT_SIZE - first]);
        true
    }
    /// Copy the latest chronological waveform and compute display RMS/peak. Non-audio thread only.
    pub fn snapshot_waveform(&self) -> Option<WaveformFrame> {
        if !self.waveform_ready.load(Ordering::Acquire) {
            return None;
        }
        let mut samples = [0.0; WAVEFORM_SAMPLES];
        {
            let ring = self.waveform.lock().ok()?;
            let start = ring.written % WAVEFORM_SAMPLES;
            let first = WAVEFORM_SAMPLES - start;
            samples[..first].copy_from_slice(&ring.samples[start..]);
            if start != 0 {
                samples[first..].copy_from_slice(&ring.samples[..start]);
            }
        }
        let mut rms = 0.0;
        for &value in &samples {
            let clipped = value.clamp(-1.0, 1.0);
            rms += clipped * clipped;
        }
        rms = (rms / WAVEFORM_SAMPLES as f32).sqrt();
        let mut peak = 0.0_f32;
        for &value in &samples {
            peak = peak.max(value.abs());
        }
        Some(WaveformFrame { samples, rms, peak })
    }
}
#[derive(Clone)]
/// One copied microphone waveform, clipped-sample RMS and raw absolute peak for rendering.
pub struct WaveformFrame {
    pub samples: [f32; WAVEFORM_SAMPLES],
    pub rms: f32,
    pub peak: f32,
}
#[derive(Default)]
/// Shared display histories, callback completion and runtime pressure. Owns no audio handle.
pub struct ObservationHub {
    pub rings: DisplayRings,
    pub audio: AudioPressure,
    // The original global playback flag, shared with SDL for completion only.
    // Native rendering still owns all mutable preview buffers/processors.
    pub preview_active: std::sync::Arc<AtomicBool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn occupied_display_mutex_drops_only_its_own_update_without_waiting() {
        let rings = DisplayRings::default();
        let fft_lock = rings.fft.lock().unwrap();
        rings.try_push(&[0.25; 256]);
        drop(fft_lock);
        let mut fft = [9.0; FFT_SIZE];
        assert!(rings.snapshot_fft(&mut fft));
        assert!(fft.iter().all(|&value| value == 0.0));
        let wave = rings.snapshot_waveform().unwrap();
        assert_eq!(&wave.samples[WAVEFORM_SAMPLES - 256..], &[0.25; 256]);
        let wave_lock = rings.waveform.lock().unwrap();
        rings.try_push(&[0.5; 256]);
        drop(wave_lock);
        rings.snapshot_fft(&mut fft);
        assert_eq!(&fft[FFT_SIZE - 256..], &[0.5; 256]);
        assert_eq!(rings.snapshot_waveform().unwrap().samples, wave.samples);
    }
}
