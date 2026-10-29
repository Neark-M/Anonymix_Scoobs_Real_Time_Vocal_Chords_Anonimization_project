//! Recorded playback/export DSP from Audio.cpp. Its 32 ms pitch algorithm is
//! deliberately distinct from the live 10 ms algorithm, as in V1.8.

use crate::effects::{PitchJitter, ProsodyContour, StreamFormantShifter, TimbreShaper};
use crate::*;

/// Recorded-path 32-ms delay reader, kept distinct from the live 10-ms pitch processor.
pub struct RealtimePitchShifter {
    delay_line: Vec<f32>,
    next_segment: Vec<f32>,
    fade_out: [f32; 288],
    fade_in: [f32; 288],
    write_pos: usize,
    read_pos: f32,
    crossfade_phase: usize,
    is_crossfading: bool,
    read_pos_prev: f32,
    read_pos_next: f32,
    current_pitch_ratio: f32,
    cooldown_frames: i32,
    smoothing: f32,
}

impl Default for RealtimePitchShifter {
    fn default() -> Self {
        let mut result = Self {
            delay_line: vec![0.0; 1536],
            next_segment: vec![0.0; 288],
            fade_out: [0.0; 288],
            fade_in: [0.0; 288],
            write_pos: 0,
            read_pos: 768.0,
            crossfade_phase: 0,
            is_crossfading: false,
            read_pos_prev: 0.0,
            read_pos_next: 0.0,
            current_pitch_ratio: 1.0,
            cooldown_frames: 0,
            smoothing: rescale_legacy_smoothing(0.995),
        };
        for i in 0..288 {
            let t = i as f32 / 287.0;
            let cosine = (3.14159265_f32 * t).cos();
            result.fade_out[i] = 0.5 * (1.0 + cosine);
            result.fade_in[i] = 0.5 * (1.0 - cosine);
        }
        result
    }
}

impl RealtimePitchShifter {
    /// Restore the recorded processor's initial history before starting a new take or WAV export.
    pub fn reset(&mut self) {
        self.delay_line.fill(0.0);
        self.next_segment.fill(0.0);
        self.write_pos = 0;
        self.read_pos = 768.0;
        self.crossfade_phase = 0;
        self.is_crossfading = false;
        self.read_pos_prev = 0.0;
        self.read_pos_next = 0.0;
        self.current_pitch_ratio = 1.0;
        self.cooldown_frames = 0;
    }
    fn read_delay_interpolated(&self, mut position: f32) -> f32 {
        while position < 0.0 {
            position += 1536.0;
        }
        while position >= 1536.0 {
            position -= 1536.0;
        }
        let idx = position as usize;
        let frac = position - idx as f32;
        let ym1 = self.delay_line[(idx + 1535) % 1536];
        let y0 = self.delay_line[idx];
        let y1 = self.delay_line[(idx + 1) % 1536];
        let y2 = self.delay_line[(idx + 2) % 1536];
        let c0 = y0;
        let c1 = 0.5 * (y1 - ym1);
        let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
        c0 + c1 * frac + c2 * frac * frac + c3 * frac * frac * frac
    }
    /// Read one recorded-path sample at the requested ratio, preserving its original crossfade rules.
    pub fn process_sample_ratio(&mut self, input: f32, target_ratio: f32) -> f32 {
        self.delay_line[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) % 1536;
        self.current_pitch_ratio =
            self.smoothing * self.current_pitch_ratio + (1.0 - self.smoothing) * target_ratio;
        let output;
        if !self.is_crossfading {
            output = self.read_delay_interpolated(self.read_pos);
            self.read_pos += self.current_pitch_ratio;
            if self.read_pos >= 1536.0 {
                self.read_pos -= 1536.0;
            }
            let mut distance = self.write_pos as f32 - self.read_pos;
            if distance < 0.0 {
                distance += 1536.0;
            }
            if (distance < 288.0 + 72.0 || distance > 1536.0 - 288.0 - 72.0)
                && self.cooldown_frames == 0
            {
                self.is_crossfading = true;
                self.crossfade_phase = 0;
                for i in 0..288 {
                    let mut pos = self.read_pos + i as f32 * self.current_pitch_ratio;
                    if pos >= 1536.0 {
                        pos -= 1536.0;
                    }
                    self.next_segment[i] = self.read_delay_interpolated(pos);
                }
                self.read_pos_prev = self.read_pos;
                self.read_pos_next = self.write_pos as f32 - 768.0;
                if self.read_pos_next < 0.0 {
                    self.read_pos_next += 1536.0;
                }
                self.cooldown_frames = (SAMPLE_RATE * 0.008) as i32;
            }
        } else if self.crossfade_phase < 288 {
            let old_sample = self.next_segment[self.crossfade_phase];
            let new_sample = self.read_delay_interpolated(self.read_pos_next);
            output = old_sample * self.fade_out[self.crossfade_phase]
                + new_sample * self.fade_in[self.crossfade_phase];
            self.read_pos_next += self.current_pitch_ratio;
            if self.read_pos_next >= 1536.0 {
                self.read_pos_next -= 1536.0;
            }
            self.crossfade_phase += 1;
        } else {
            self.is_crossfading = false;
            self.read_pos = self.read_pos_next;
            output = self.read_delay_interpolated(self.read_pos);
            self.read_pos += self.current_pitch_ratio;
            if self.read_pos >= 1536.0 {
                self.read_pos -= 1536.0;
            }
        }
        if self.cooldown_frames > 0 {
            self.cooldown_frames -= 1;
        }
        output
    }
}

/// V1.8's SelectedAudioProcessor, shared algorithm for preview and WAV DSP.
/// Reset between takes; preview and export must have independent instances.
pub struct SelectedAudioProcessor {
    pitch: RealtimePitchShifter,
    formant: StreamFormantShifter,
    timbre: TimbreShaper,
    prosody: ProsodyContour,
    jitter: PitchJitter,
}

impl Default for SelectedAudioProcessor {
    fn default() -> Self {
        Self {
            pitch: RealtimePitchShifter::default(),
            formant: StreamFormantShifter::default(),
            timbre: TimbreShaper::default(),
            prosody: ProsodyContour::default(),
            jitter: PitchJitter::default(),
        }
    }
}

impl SelectedAudioProcessor {
    /// Restore the recorded processor's initial history before starting a new take or WAV export.
    pub fn reset(&mut self) {
        self.pitch.reset();
        self.formant.reset();
        self.timbre.reset();
        self.prosody = ProsodyContour::default();
        self.jitter = PitchJitter::default();
    }
    /// Input and output are floats here. PCM16 capture, snapshots, playback tail
    /// management and WAV framing remain in the C++ reference at this stage.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32], s: Settings) {
        assert_eq!(input.len(), output.len());
        assert!(input.len() <= SELECTED_BLOCK_SIZE);
        let count = input.len();
        let mut pitched = [0.0; SELECTED_BLOCK_SIZE];
        let mut shaped = [0.0; SELECTED_BLOCK_SIZE];
        let mut accent = [0.0; SELECTED_BLOCK_SIZE];
        let pitch_enabled = s.mask & EFFECT_PITCH != 0;
        let prosody_enabled = s.mask & EFFECT_PROSODY != 0 && s.prosody != 0.0;
        let jitter_enabled = s.mask & EFFECT_JITTER != 0 && s.jitter > 0.0;
        let base_ratio = 2.0_f32.powf(if pitch_enabled { s.pitch } else { 0.0 } / 12.0);
        for i in 0..count {
            let step = self.prosody.advance(
                input[i],
                if s.mask & EFFECT_PROSODY != 0 {
                    s.prosody
                } else {
                    0.0
                },
            );
            accent[i] = step.gain;
            let jitter_ratio = self
                .jitter
                .next_ratio(if jitter_enabled { s.jitter } else { 0.0 });
            let ratio = base_ratio
                * if prosody_enabled && step.semitones != 0.0 {
                    2.0_f32.powf(step.semitones / 12.0)
                } else {
                    1.0
                }
                * jitter_ratio;
            pitched[i] = if pitch_enabled || prosody_enabled || jitter_enabled {
                self.pitch.process_sample_ratio(input[i], ratio)
            } else {
                input[i]
            };
        }
        if s.mask & EFFECT_FORMANT != 0 {
            self.formant
                .process_frame(&pitched[..count], &mut shaped[..count], s.formant);
        } else {
            shaped[..count].copy_from_slice(&pitched[..count]);
        }
        if s.mask & EFFECT_TIMBRE != 0 {
            self.timbre.process_buffer(&mut shaped[..count], s.timbre);
        }
        for i in 0..count {
            output[i] = (shaped[i] * accent[i] * s.volume).clamp(-0.95, 0.95);
        }
    }
}
