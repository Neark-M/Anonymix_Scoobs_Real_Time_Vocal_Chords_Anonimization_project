//! Capture cleanup from Stream.cpp, before display rings, recording and live DSP.
//! This module does not open PortAudio devices or own the callback lifecycle.

use crate::{rescale_legacy_smoothing, SAMPLE_RATE, STREAM_BLOCK_SIZE};

/// Mirrors CaptureProcessingState. A device generation change resets the entire
/// state on the processing thread, just before the next block is processed.
pub struct CaptureProcessor {
    device_generation: u32,
    high_pass_input: f32,
    high_pass_output: f32,
    smoothed_power: f32,
    expander_envelope: f32,
    second_channel_mix: f32,
    exclusive_input_gain: f32,
    high_pass_alpha: f32,
    rms_alpha: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

/// Input energy required by channel selection and Exclusive capture gain.
/// Keep double accumulation: these powers directly influence the audio path.
#[derive(Default)]
pub struct CaptureLevels {
    pub first_power: f64,
    pub second_power: f64,
}

impl Default for CaptureProcessor {
    fn default() -> Self {
        Self {
            device_generation: u32::MAX,
            high_pass_input: 0.0,
            high_pass_output: 0.0,
            smoothed_power: 0.0,
            expander_envelope: 1.0,
            second_channel_mix: 0.0,
            exclusive_input_gain: 1.0,
            high_pass_alpha: rescale_legacy_smoothing(0.950),
            rms_alpha: rescale_legacy_smoothing(0.8825),
            attack_coeff: (-1.0_f32 / (0.001 * SAMPLE_RATE)).exp(),
            release_coeff: (-1.0_f32 / (0.050 * SAMPLE_RATE)).exp(),
        }
    }
}

impl CaptureProcessor {
    /// Process one non-null, mono or stereo input chunk (at most 256 frames).
    /// The device adapter splits larger host callbacks exactly as V1.8
    /// does. Chunk size affects gain adaptation, so callers must not regroup it.
    pub fn process_chunk(
        &mut self,
        input: &[f32],
        channels: usize,
        exclusive: bool,
        generation: u32,
        output: &mut [f32],
    ) -> CaptureLevels {
        assert!(channels == 1 || channels == 2);
        assert_eq!(input.len(), output.len() * channels);
        assert!(output.len() <= STREAM_BLOCK_SIZE);
        if self.device_generation != generation {
            *self = Self::default();
            self.device_generation = generation;
        }
        let count = output.len();
        let mut levels = CaptureLevels::default();
        if channels == 2 {
            for i in 0..count {
                let first = input[2 * i];
                let second = input[2 * i + 1];
                levels.first_power += first as f64 * first as f64;
                levels.second_power += second as f64 * second as f64;
            }
        } else {
            for &first in input {
                levels.first_power += first as f64 * first as f64;
            }
        }
        // Channel energy selects the physical microphone channel and drives
        // Exclusive gain. Keep this arithmetic even in the silent public build.
        let channel_target = if channels == 2 && levels.second_power > levels.first_power * 1.5 {
            1.0
        } else if levels.first_power > levels.second_power * 1.5 {
            0.0
        } else if self.second_channel_mix >= 0.5 {
            1.0
        } else {
            0.0
        };
        let strongest = if channels == 2 {
            levels.first_power.max(levels.second_power)
        } else {
            levels.first_power
        };
        let gain_at_start = self.exclusive_input_gain;
        if exclusive {
            let block_rms = ((strongest / count.max(1) as f64) as f32).sqrt();
            let wanted = (0.060 / block_rms.max(0.0015)).clamp(1.0, 42.0);
            let seconds = if wanted < gain_at_start { 0.040 } else { 0.250 };
            let remaining = (-(count as f32) / (SAMPLE_RATE * seconds)).exp();
            self.exclusive_input_gain = wanted + (gain_at_start - wanted) * remaining;
        } else {
            self.exclusive_input_gain = 1.0;
        }
        // Apply channel crossfade, high-pass, mode-specific gain/limiting, then
        // the Shared-only expander. The order is audible and must stay fixed.
        for i in 0..count {
            let first = input[i * channels];
            let second = if channels == 2 {
                input[i * 2 + 1]
            } else {
                first
            };
            self.second_channel_mix +=
                (channel_target - self.second_channel_mix) * (1.0 / (SAMPLE_RATE * 0.004));
            let mut sample = first + (second - first) * self.second_channel_mix;
            let high_pass =
                self.high_pass_alpha * (self.high_pass_output + sample - self.high_pass_input);
            self.high_pass_input = sample;
            self.high_pass_output = high_pass;
            sample = high_pass;
            if exclusive {
                let progress = (i + 1) as f32 / count as f32;
                let gain = gain_at_start + (self.exclusive_input_gain - gain_at_start) * progress;
                let scaled = sample * gain;
                let absolute = scaled.abs();
                sample = if absolute <= 0.75 {
                    scaled
                } else {
                    (0.75 + 0.20 * ((absolute - 0.75) / 0.20).tanh()).copysign(scaled)
                };
            }
            self.smoothed_power =
                self.rms_alpha * self.smoothed_power + (1.0 - self.rms_alpha) * (sample * sample);
            let rms_current = self.smoothed_power.sqrt();
            if !exclusive && rms_current < 0.015 {
                let target_gain = 1.0 - ((0.015 - rms_current) / 0.015) * (1.0 - 1.0 / 2.0);
                if target_gain < self.expander_envelope {
                    self.expander_envelope = self.attack_coeff * self.expander_envelope
                        + (1.0 - self.attack_coeff) * target_gain;
                } else {
                    self.expander_envelope = self.release_coeff * self.expander_envelope
                        + (1.0 - self.release_coeff) * target_gain;
                }
                sample *= self.expander_envelope;
            } else {
                self.expander_envelope = 1.0;
            }
            output[i] = sample;
        }
        levels
    }
}
