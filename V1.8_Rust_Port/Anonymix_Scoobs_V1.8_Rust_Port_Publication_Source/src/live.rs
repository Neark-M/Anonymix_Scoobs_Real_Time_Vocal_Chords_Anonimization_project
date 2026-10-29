//! Live pitch processor and post-capture chain from Stream.cpp.
//! The redundant initial copy and separate volume pass are deliberately kept.

use crate::effects::{PitchJitter, ProsodyContour, StreamFormantShifter, TimbreShaper};
use crate::*;

/// Live 480-sample circular delay with a 144-sample crossfade and smoothed playback ratio.
pub struct StreamPitchShifter {
    delay_line: [f32; 480],
    xfade_buffer: [f32; 144],
    fade_out: [f32; 144],
    write_pos: usize,
    read_pos: f32,
    xfade_phase: usize,
    is_crossfading: bool,
    current_ratio: f32,
    smoothing: f32,
}

impl Default for StreamPitchShifter {
    fn default() -> Self {
        let mut result = Self {
            delay_line: [0.0; 480],
            xfade_buffer: [0.0; 144],
            fade_out: [0.0; 144],
            write_pos: 0,
            read_pos: 240.0,
            xfade_phase: 0,
            is_crossfading: false,
            current_ratio: 1.0,
            smoothing: rescale_legacy_smoothing(0.99),
        };
        for i in 0..144 {
            let t = i as f32 / 143.0;
            result.fade_out[i] = 0.5 * (1.0 + (3.14159265_f32 * t).cos());
        }
        result
    }
}

impl StreamPitchShifter {
    /// Write one sample, interpolate the moving reader, and crossfade around delay wrap boundaries.
    pub fn process_sample_ratio(&mut self, input: f32, target_ratio: f32) -> f32 {
        self.delay_line[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) % 480;
        self.current_ratio =
            self.smoothing * self.current_ratio + (1.0 - self.smoothing) * target_ratio;
        let output;
        if !self.is_crossfading {
            let idx = self.read_pos as usize;
            let frac = self.read_pos - idx as f32;
            let s0 = self.delay_line[idx % 480];
            let s1 = self.delay_line[(idx + 1) % 480];
            output = s0 + frac * (s1 - s0);
            self.read_pos += self.current_ratio;
            if self.read_pos >= 480.0 {
                self.read_pos -= 480.0;
            }
            let mut distance = self.write_pos as f32 - self.read_pos;
            if distance < 0.0 {
                distance += 480.0;
            }
            if distance < 144.0 + 30.0 || distance > 480.0 - 144.0 - 30.0 {
                self.is_crossfading = true;
                self.xfade_phase = 0;
                for i in 0..144 {
                    let mut pos = self.read_pos + i as f32 * self.current_ratio;
                    if pos >= 480.0 {
                        pos -= 480.0;
                    }
                    self.xfade_buffer[i] = self.delay_line[(pos as usize) % 480];
                }
                self.read_pos = self.write_pos as f32 - 240.0;
                if self.read_pos < 0.0 {
                    self.read_pos += 480.0;
                }
            }
        } else if self.xfade_phase < 144 {
            let out_weight = self.fade_out[self.xfade_phase];
            let fade_in = 1.0 - out_weight;
            let idx = self.read_pos as usize;
            let new_sample = self.delay_line[idx % 480];
            output = self.xfade_buffer[self.xfade_phase] * out_weight + new_sample * fade_in;
            self.read_pos += self.current_ratio;
            if self.read_pos >= 480.0 {
                self.read_pos -= 480.0;
            }
            self.xfade_phase += 1;
        } else {
            self.is_crossfading = false;
            output = self.delay_line[(self.read_pos as usize) % 480];
            self.read_pos += self.current_ratio;
            if self.read_pos >= 480.0 {
                self.read_pos -= 480.0;
            }
        }
        output
    }
}

fn mix_with_neutral_bypass(direct: f32, delayed: f32, wet: &mut f32, use_delayed: bool) -> f32 {
    let step = 1.0 / (SAMPLE_RATE * 0.010);
    *wet = (*wet + if use_delayed { step } else { -step }).clamp(0.0, 1.0);
    if *wet <= 0.0 {
        return direct;
    }
    if *wet >= 1.0 {
        return delayed;
    }
    let smooth_wet = *wet * *wet * (3.0 - 2.0 * *wet);
    direct + smooth_wet * (delayed - direct)
}

/// Separate instances preserve history exactly like the C++ live objects.
/// Do not reset this chain when sliders or effect selectors change.
pub struct LiveChain {
    pitch: StreamPitchShifter,
    formant: StreamFormantShifter,
    temp_pitch: [f32; STREAM_BLOCK_SIZE],
    temp_formant: [f32; STREAM_BLOCK_SIZE],
    output: [f32; STREAM_BLOCK_SIZE],
    timbre: TimbreShaper,
    prosody: ProsodyContour,
    jitter: PitchJitter,
    pitch_wet: f32,
    formant_wet: f32,
}

impl Default for LiveChain {
    fn default() -> Self {
        Self {
            pitch: StreamPitchShifter::default(),
            formant: StreamFormantShifter::default(),
            temp_pitch: [0.0; STREAM_BLOCK_SIZE],
            temp_formant: [0.0; STREAM_BLOCK_SIZE],
            output: [0.0; STREAM_BLOCK_SIZE],
            timbre: TimbreShaper::default(),
            prosody: ProsodyContour::default(),
            jitter: PitchJitter::default(),
            pitch_wet: 0.0,
            formant_wet: 0.0,
        }
    }
}

impl LiveChain {
    /// Process one cleaned mono DSP chunk (at most 256 frames). The duplex adapter
    /// handles host splitting and channel duplication outside this DSP chain.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32], s: Settings) {
        assert_eq!(input.len(), output.len());
        assert!(input.len() <= STREAM_BLOCK_SIZE);
        let count = input.len();
        self.temp_pitch[..count].copy_from_slice(input);
        let prosody = if s.mask & EFFECT_PROSODY != 0 {
            s.prosody
        } else {
            0.0
        };
        let jitter = if s.mask & EFFECT_JITTER != 0 {
            s.jitter
        } else {
            0.0
        };
        let pitch_needs_delay =
            (s.mask & EFFECT_PITCH != 0 && s.pitch.abs() > 0.01) || prosody != 0.0 || jitter > 0.0;
        let constant_ratio = 2.0_f32.powf(
            if s.mask & EFFECT_PITCH != 0 {
                s.pitch
            } else {
                0.0
            } / 12.0,
        );
        // Advance modulation even at neutral settings, preserving processor
        // history for the next control change. Neutral bypass itself crossfades.
        for i in 0..count {
            let step = self.prosody.advance(input[i], prosody);
            let jitter_ratio = self.jitter.next_ratio(jitter);
            let target = constant_ratio
                * if step.semitones == 0.0 {
                    1.0
                } else {
                    2.0_f32.powf(step.semitones / 12.0)
                }
                * jitter_ratio;
            let delayed = self.pitch.process_sample_ratio(input[i], target);
            self.temp_pitch[i] =
                mix_with_neutral_bypass(input[i], delayed, &mut self.pitch_wet, pitch_needs_delay);
            self.output[i] = step.gain;
        }
        // Formant-labelled delay follows pitch, then timbre, volume/prosody
        // gain, and the inherited final tanh saturation. This is all 48 kHz.
        let ratio = if s.mask & EFFECT_FORMANT != 0 {
            s.formant
        } else {
            1.0
        };
        let needs_delay = s.mask & EFFECT_FORMANT != 0 && (ratio - 1.0).abs() > 0.005;
        self.formant.process_frame(
            &self.temp_pitch[..count],
            &mut self.temp_formant[..count],
            ratio,
        );
        for i in 0..count {
            self.temp_formant[i] = mix_with_neutral_bypass(
                self.temp_pitch[i],
                self.temp_formant[i],
                &mut self.formant_wet,
                needs_delay,
            );
        }
        if s.mask & EFFECT_TIMBRE != 0 {
            self.timbre
                .process_buffer(&mut self.temp_formant[..count], s.timbre);
        }
        for i in 0..count {
            self.temp_formant[i] *= s.volume * self.output[i];
        }
        for i in 0..count {
            output[i] = (self.temp_formant[i] * 0.85).tanh();
        }
    }
}
