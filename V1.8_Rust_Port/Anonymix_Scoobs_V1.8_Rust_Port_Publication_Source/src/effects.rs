//! Direct ports of Prosody.h, PitchJitter.h, Timbre.h and Formant.h.
//! Constants, state resets and operation order intentionally follow V1.8.

use crate::{rescale_legacy_smoothing, SAMPLE_RATE};

/// One sample's onset-driven pitch offset in semitones and linear loudness multiplier.
pub struct ProsodyStep {
    pub semitones: f32,
    pub gain: f32,
}

/// Fast/slow envelope onset detector and 240-ms gesture; this does not analyze linguistic intonation.
pub struct ProsodyContour {
    fast_envelope: f32,
    slow_envelope: f32,
    gesture_sample: u32,
    refractory_samples: u32,
}

impl Default for ProsodyContour {
    fn default() -> Self {
        Self {
            fast_envelope: 0.0,
            slow_envelope: 0.0,
            gesture_sample: (SAMPLE_RATE * 0.24) as u32,
            refractory_samples: 0,
        }
    }
}

impl ProsodyContour {
    /// Advance once per sample, including when the slider is neutral. Tracking
    /// onsets while disabled is an inherited behavior used by both chains.
    pub fn advance(&mut self, input: f32, slider_percent: f32) -> ProsodyStep {
        let energy = input.abs();
        self.fast_envelope += (energy - self.fast_envelope) / (SAMPLE_RATE * 0.005);
        self.slow_envelope += (energy - self.slow_envelope) / (SAMPLE_RATE * 0.10);
        if self.refractory_samples != 0 {
            self.refractory_samples -= 1;
        }
        if self.refractory_samples == 0
            && self.fast_envelope > 0.018
            && self.fast_envelope > self.slow_envelope * 1.45
        {
            self.gesture_sample = 0;
            self.refractory_samples = (SAMPLE_RATE * 0.11) as u32;
        }
        let mut contour = 0.0;
        let length = (SAMPLE_RATE * 0.24) as u32;
        if self.gesture_sample < length {
            let progress = self.gesture_sample as f32 / length as f32;
            self.gesture_sample += 1;
            contour = if progress < 0.30 {
                progress / 0.30
            } else {
                (1.0 - progress) / 0.70
            };
        }
        let amount = (slider_percent * 0.01).clamp(-1.0, 1.0);
        ProsodyStep {
            semitones: amount * 1.5 * contour,
            gain: 1.0 + amount * 0.12 * contour,
        }
    }
}

/// Deterministic, smoothed random pitch-ratio modulation with the original xorshift seed.
pub struct PitchJitter {
    state: u32,
    remaining: u32,
    current: f32,
    target: f32,
}

impl Default for PitchJitter {
    fn default() -> Self {
        Self {
            state: 0xa3618c7d,
            remaining: 0,
            current: 0.0,
            target: 0.0,
        }
    }
}

impl PitchJitter {
    /// Xorshift uses the same unsigned 32-bit seed and shifts as C++. Turning
    /// jitter off clears its envelope, but does not reset its random seed.
    pub fn next_ratio(&mut self, amount: f32) -> f32 {
        let amount = amount.clamp(0.0, 0.020);
        if amount == 0.0 {
            self.current = 0.0;
            self.target = 0.0;
            self.remaining = 0;
            return 1.0;
        }
        if self.remaining == 0 {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 17;
            self.state ^= self.state << 5;
            let unit = (self.state & 0xffff) as f32 / 65535.0;
            self.target = (2.0 * unit - 1.0) * amount;
            self.remaining = 48000 / 18;
        }
        self.remaining -= 1;
        self.current += (self.target - self.current) * 0.0075;
        1.0 + self.current
    }
}

/// Smoothed spectral tilt around the inherited 1600-Hz biquad. Coefficients and histories stay local.
pub struct TimbreShaper {
    coefficients: [f32; 5],
    amount_smoothing: f32,
    smoothed_amount: f32,
    state1: f32,
    state2: f32,
}

impl Default for TimbreShaper {
    fn default() -> Self {
        // V1.8 constructs coefficients in double precision and stores floats.
        let omega = 2.0 * std::f64::consts::PI * 1600.0 / SAMPLE_RATE as f64;
        let cosine = omega.cos();
        let alpha = omega.sin() / (2.0 * 0.7071067811865476);
        let inverse_a0 = 1.0 / (1.0 + alpha);
        Self {
            coefficients: [
                ((1.0 - cosine) * 0.5 * inverse_a0) as f32,
                ((1.0 - cosine) * inverse_a0) as f32,
                ((1.0 - cosine) * 0.5 * inverse_a0) as f32,
                (-2.0 * cosine * inverse_a0) as f32,
                ((1.0 - alpha) * inverse_a0) as f32,
            ],
            amount_smoothing: (-1.0_f32 / (0.020 * SAMPLE_RATE)).exp(),
            smoothed_amount: 0.0,
            state1: 0.0,
            state2: 0.0,
        }
    }
}

impl TimbreShaper {
    /// Clear this processor's histories and modulation state before a new recorded preview or export.
    pub fn reset(&mut self) {
        self.smoothed_amount = 0.0;
        self.state1 = 0.0;
        self.state2 = 0.0;
    }
    /// Keep V1.8's end-of-block denormal cleanup, so block boundaries matter.
    pub fn process_buffer(&mut self, samples: &mut [f32], slider_percent: f32) {
        let target_amount = slider_percent.clamp(-100.0, 100.0) * 0.006;
        let [b0, b1, b2, a1, a2] = self.coefficients;
        for sample in samples {
            self.smoothed_amount = self.amount_smoothing * self.smoothed_amount
                + (1.0 - self.amount_smoothing) * target_amount;
            let input = *sample;
            let low_frequency = b0 * input + self.state1;
            self.state1 = b1 * input - a1 * low_frequency + self.state2;
            self.state2 = b2 * input - a2 * low_frequency;
            *sample = input + self.smoothed_amount * (input - low_frequency);
        }
        if self.state1.abs() < 1.0e-20 {
            self.state1 = 0.0;
        }
        if self.state2.abs() < 1.0e-20 {
            self.state2 = 0.0;
        }
    }
}

/// The inherited UI calls this a formant shifter. Its variable-delay read rate
/// also moves harmonics; this port does not replace it with envelope shifting.
pub struct StreamFormantShifter {
    delay_line: [f32; 768],
    xfade_buffer: [f32; 288],
    fade_out: [f32; 288],
    write_pos: usize,
    read_pos: f32,
    xfade_phase: usize,
    is_crossfading: bool,
    current_ratio: f32,
    smoothing: f32,
}

impl Default for StreamFormantShifter {
    fn default() -> Self {
        let mut result = Self {
            delay_line: [0.0; 768],
            xfade_buffer: [0.0; 288],
            fade_out: [0.0; 288],
            write_pos: 0,
            read_pos: 384.0,
            xfade_phase: 0,
            is_crossfading: false,
            current_ratio: 1.0,
            smoothing: rescale_legacy_smoothing(0.995),
        };
        for i in 0..288 {
            let t = i as f32 / 287.0;
            result.fade_out[i] = 0.5 * (1.0 + (3.14159265_f32 * t).cos());
        }
        result
    }
}

impl StreamFormantShifter {
    pub const NOMINAL_DELAY: usize = 384;
    pub const MAX_DELAY: usize = 768;
    /// Clear this processor's histories and modulation state before a new recorded preview or export.
    pub fn reset(&mut self) {
        self.delay_line.fill(0.0);
        self.xfade_buffer.fill(0.0);
        self.write_pos = 0;
        self.read_pos = 384.0;
        self.xfade_phase = 0;
        self.is_crossfading = false;
        self.current_ratio = 1.0;
    }
    fn read_delay_interpolated(&self, mut position: f32) -> f32 {
        while position < 0.0 {
            position += 768.0;
        }
        while position >= 768.0 {
            position -= 768.0;
        }
        let idx = position as usize;
        let frac = position - idx as f32;
        let ym1 = self.delay_line[(idx + 767) % 768];
        let y0 = self.delay_line[idx];
        let y1 = self.delay_line[(idx + 1) % 768];
        let y2 = self.delay_line[(idx + 2) % 768];
        let c0 = y0;
        let c1 = 0.5 * (y1 - ym1);
        let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
        c0 + c1 * frac + c2 * frac * frac + c3 * frac * frac * frac
    }
    /// Advance the variable-delay reader by one sample using the smoothed shift ratio.
    /// Crossfades prevent an abrupt jump when the reader approaches the writer.
    pub fn process_sample(&mut self, input: f32, shift_ratio: f32) -> f32 {
        self.delay_line[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) % 768;
        self.current_ratio =
            self.smoothing * self.current_ratio + (1.0 - self.smoothing) * shift_ratio;
        let output;
        if !self.is_crossfading {
            output = self.read_delay_interpolated(self.read_pos);
            self.read_pos += self.current_ratio;
            if self.read_pos >= 768.0 {
                self.read_pos -= 768.0;
            }
            let mut distance = self.write_pos as f32 - self.read_pos;
            if distance < 0.0 {
                distance += 768.0;
            }
            if distance < 288.0 + 72.0 || distance > 768.0 - 288.0 - 72.0 {
                self.is_crossfading = true;
                self.xfade_phase = 0;
                for i in 0..288 {
                    let mut pos = self.read_pos + i as f32 * self.current_ratio;
                    if pos >= 768.0 {
                        pos -= 768.0;
                    }
                    self.xfade_buffer[i] = self.read_delay_interpolated(pos);
                }
                self.read_pos = self.write_pos as f32 - 384.0;
                if self.read_pos < 0.0 {
                    self.read_pos += 768.0;
                }
            }
        } else if self.xfade_phase < 288 {
            let out_weight = self.fade_out[self.xfade_phase];
            let fade_in = 1.0 - out_weight;
            let old_sample = self.xfade_buffer[self.xfade_phase];
            let new_sample = self.read_delay_interpolated(self.read_pos);
            output = old_sample * out_weight + new_sample * fade_in;
            self.read_pos += self.current_ratio;
            if self.read_pos >= 768.0 {
                self.read_pos -= 768.0;
            }
            self.xfade_phase += 1;
        } else {
            self.is_crossfading = false;
            output = self.read_delay_interpolated(self.read_pos);
            self.read_pos += self.current_ratio;
            if self.read_pos >= 768.0 {
                self.read_pos -= 768.0;
            }
        }
        output
    }
    /// Apply the same stateful formant-labelled delay processor to every mono sample in this block.
    pub fn process_frame(&mut self, input: &[f32], output: &mut [f32], ratio: f32) {
        assert_eq!(input.len(), output.len());
        for (input, output) in input.iter().zip(output) {
            *output = self.process_sample(*input, ratio);
        }
    }
}
