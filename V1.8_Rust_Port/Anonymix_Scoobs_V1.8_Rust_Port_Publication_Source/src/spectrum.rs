//! Display-only Dsp.cpp port. This transform never processes speaker output.
//! Preserve calibration, impulse replacement, band averaging and normalization.
pub const FFT_SIZE: usize = 4096;
pub const BARS: usize = 96;
pub const MIN_HZ: f32 = 20.0;
pub const MAX_HZ: f32 = 20_000.0;
pub const CALIBRATION_FRAMES: u32 = 60;

#[derive(Clone, Copy, Default)]
/// Float real/imaginary pair used only by the display FFT.
pub struct Complex {
    pub real: f32,
    pub imaginary: f32,
}
impl Complex {
    fn multiply(self, other: Self) -> Self {
        Self {
            real: self.real * other.real - self.imaginary * other.imaginary,
            imaginary: self.real * other.imaginary + self.imaginary * other.real,
        }
    }
    fn magnitude(self) -> f32 {
        self.real.hypot(self.imaginary)
    }
}

/// Original iterative radix-2 order and per-stage twiddle evaluation. No new
/// FFT plan, cached bit-reversal map or different transform library is used.
pub fn fft_cooley_tukey(data: &mut [Complex]) -> bool {
    let n = data.len();
    if n < 2 || !n.is_power_of_two() {
        return false;
    }
    let mut j = 0;
    for i in 0..n - 1 {
        if i < j {
            data.swap(i, j);
        }
        let mut k = n / 2;
        while k <= j {
            j -= k;
            k /= 2;
        }
        j += k;
    }
    let mut m = 2;
    while m <= n {
        let half = m / 2;
        let angle = (-2.0 * std::f64::consts::PI / m as f64) as f32;
        let wm = Complex {
            real: angle.cos(),
            imaginary: angle.sin(),
        };
        let mut w = Complex {
            real: 1.0,
            imaginary: 0.0,
        };
        for j in 0..half {
            for k in (j..n).step_by(m) {
                let t = w.multiply(data[k + half]);
                let u = data[k];
                data[k] = Complex {
                    real: u.real + t.real,
                    imaginary: u.imaginary + t.imaginary,
                };
                data[k + half] = Complex {
                    real: u.real - t.real,
                    imaginary: u.imaginary - t.imaginary,
                };
            }
            w = w.multiply(wm);
        }
        m *= 2;
    }
    true
}

/// These are the constants already cached by V1.8, not a new optimization.
pub struct DisplayFftConstants {
    pub hann: Vec<f32>,
    pub bands: [(usize, usize); BARS],
    pub calibration_boost: [f32; BARS],
}
impl Default for DisplayFftConstants {
    fn default() -> Self {
        let mut result = Self {
            hann: vec![0.0; FFT_SIZE],
            bands: [(0, 0); BARS],
            calibration_boost: [0.0; BARS],
        };
        for i in 0..FFT_SIZE {
            result.hann[i] = (0.5
                * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (FFT_SIZE - 1) as f64).cos()))
                as f32;
        }
        let bins = FFT_SIZE / 2;
        let bin_width = crate::SAMPLE_RATE / FFT_SIZE as f32;
        for bar in 0..BARS {
            let start_hz = MIN_HZ * (MAX_HZ / MIN_HZ).powf(bar as f32 / BARS as f32);
            let end_hz = MIN_HZ * (MAX_HZ / MIN_HZ).powf((bar + 1) as f32 / BARS as f32);
            let start = ((start_hz / bin_width).round() as usize).clamp(1, bins - 1);
            let end = bins.min((start + 1).max((end_hz / bin_width).ceil() as usize));
            result.bands[bar] = (start, end);
            let center = (start_hz + end_hz) / 2.0;
            result.calibration_boost[bar] = if center < 200.0 {
                1.6
            } else if center < 1000.0 {
                1.4
            } else {
                1.3
            };
        }
        result
    }
}

#[derive(Clone, Debug)]
/// Copied 96-band normalized display result and calibration progress, not absolute audio loudness.
pub struct SpectrumFrame {
    pub magnitudes: [f32; BARS],
    pub calibrating: bool,
    pub calibration_frames: u32,
}

/// Persistent Hann/FFT/calibration/history state owned by the display worker, never the audio callback.
pub struct SpectrumAnalyzer {
    constants: DisplayFftConstants,
    local: Vec<f32>,
    complex: Vec<Complex>,
    raw: [f32; BARS],
    noise_floor: [f32; BARS],
    history: [f32; BARS],
    calibrating: bool,
    calibration_frames: u32,
}
impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self {
            constants: DisplayFftConstants::default(),
            local: vec![0.0; FFT_SIZE],
            complex: vec![Complex::default(); FFT_SIZE],
            raw: [0.0; BARS],
            noise_floor: [0.0; BARS],
            history: [0.0; BARS],
            calibrating: true,
            calibration_frames: 0,
        }
    }
}
impl SpectrumAnalyzer {
    /// The caller supplies the most recent full chronological microphone window.
    /// State persists across device reopen, as the C++ function's statics did.
    pub fn analyze(&mut self, samples: &[f32; FFT_SIZE]) -> SpectrumFrame {
        self.local.copy_from_slice(samples);
        let mut mean_energy = 0.0;
        for &sample in &self.local {
            mean_energy += sample * sample;
        }
        mean_energy /= FFT_SIZE as f32;
        let mut std_dev = 0.0;
        for &sample in &self.local {
            let diff = sample * sample - mean_energy;
            std_dev += diff * diff;
        }
        std_dev = (std_dev / FFT_SIZE as f32).sqrt();
        let threshold = mean_energy + 4.0 * std_dev;
        for i in 1..FFT_SIZE - 1 {
            let energy = self.local[i] * self.local[i];
            if energy > threshold {
                let left = self.local[i - 1] * self.local[i - 1];
                let right = self.local[i + 1] * self.local[i + 1];
                if (left + right) / 2.0 < threshold * 0.25 {
                    self.local[i] = (self.local[i - 1] + self.local[i + 1]) / 2.0;
                }
            }
        }
        // Windowing and FFT affect the copied display history only. Neither
        // this impulse suppression nor calibration modifies recorded/live audio.
        for i in 0..FFT_SIZE {
            self.complex[i] = Complex {
                real: self.local[i] * self.constants.hann[i],
                imaginary: 0.0,
            };
        }
        fft_cooley_tukey(&mut self.complex);
        for bar in 0..BARS {
            let (start, end) = self.constants.bands[bar];
            let mut sum = 0.0;
            let mut count = 0;
            for bin in start..end {
                sum += self.complex[bin].magnitude();
                count += 1;
            }
            self.raw[bar] = if count > 0 { sum / count as f32 } else { 0.0 };
        }
        // First learn the original per-band floor; subsequent frames retain
        // the approved peak normalization/history instead of measuring dB SPL.
        if self.calibrating {
            for bar in 0..BARS {
                self.noise_floor[bar] = self.noise_floor[bar].max(self.raw[bar]);
            }
            self.calibration_frames += 1;
            if self.calibration_frames >= CALIBRATION_FRAMES {
                self.calibrating = false;
                for bar in 0..BARS {
                    self.noise_floor[bar] *= self.constants.calibration_boost[bar];
                }
            }
            return SpectrumFrame {
                magnitudes: self.raw,
                calibrating: self.calibrating,
                calibration_frames: self.calibration_frames,
            };
        }
        for bar in 0..BARS {
            let factor = if self.raw[bar] > self.history[bar] {
                0.72
            } else {
                0.30
            };
            self.history[bar] += (self.raw[bar] - self.history[bar]) * factor;
        }
        let mut magnitudes = [0.0_f32; BARS];
        for bar in 0..BARS {
            magnitudes[bar] = (self.history[bar] - self.noise_floor[bar]).max(0.0);
        }
        let maximum = magnitudes.iter().copied().fold(0.0_f32, f32::max);
        if maximum > 0.00001 {
            for value in &mut magnitudes {
                *value = (*value / maximum).min(1.0).powf(0.78);
            }
        }
        SpectrumFrame {
            magnitudes,
            calibrating: self.calibrating,
            calibration_frames: self.calibration_frames,
        }
    }
}

/// Main/render-thread smoothing, using the original elapsed-time compensation.
pub fn smooth_display(
    magnitudes: &[f32; BARS],
    smoothed: &mut [f32; BARS],
    factor: f32,
    elapsed: f32,
) {
    let frames_at_60 = (elapsed * 60.0).clamp(0.0, 4.0);
    let falling = 1.0 - (1.0 - factor).powf(frames_at_60);
    let rising = 1.0 - (1.0 - 0.82_f32).powf(frames_at_60);
    for i in 0..BARS {
        let blend = if magnitudes[i] > smoothed[i] {
            rising
        } else {
            falling
        };
        smoothed[i] += (magnitudes[i] - smoothed[i]) * blend;
    }
}
