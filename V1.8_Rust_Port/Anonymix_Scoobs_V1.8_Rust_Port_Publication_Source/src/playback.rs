//! Recorded preview callback logic from Audio.cpp, not a PortAudio stream owner.
use crate::{
    effects::StreamFormantShifter, pcm::read_pcm16_sample, recorded::SelectedAudioProcessor,
    Settings, EFFECT_FORMANT, SELECTED_BLOCK_SIZE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Native callback outcome: keep rendering, or finish after the take and its original effect tail.
pub enum PlaybackResult {
    Continue,
    Complete,
}

/// A complete copied take and its independent recorded DSP state. Only one renderer may access it.
pub struct RecordedPlayback {
    buffer: Vec<u8>,
    position: usize,
    frames_emitted: usize,
    tail_frames: usize,
    processor: SelectedAudioProcessor,
    playing: bool,
}

impl Default for RecordedPlayback {
    fn default() -> Self {
        Self {
            buffer: Vec::new(),
            position: 0,
            frames_emitted: 0,
            tail_frames: 0,
            processor: SelectedAudioProcessor::default(),
            playing: false,
        }
    }
}

impl RecordedPlayback {
    /// Call on the application/device worker before callbacks can access this
    /// object. Keep the same first-minute preload as the C++ preview buffer.
    pub fn preload_first_minute(&mut self) {
        self.buffer.reserve(48_000 * 2 * 60);
    }
    /// Caller must enforce V1.8's recording/export restrictions and stop/join the
    /// previous device callback. Keep a complete copy; do not share the take.
    pub fn start_after_join(&mut self, original: &[u8]) -> bool {
        assert_eq!(original.len() % 2, 0);
        if self.playing {
            return false;
        }
        self.buffer.clear();
        self.buffer.extend_from_slice(original);
        if self.buffer.is_empty() {
            return false;
        }
        self.processor.reset();
        self.position = 0;
        self.frames_emitted = 0;
        self.tail_frames = 0;
        self.playing = true;
        true
    }
    /// The PortAudio wrapper must abort/join or disable/wait before this call.
    pub fn stop_after_join(&mut self) {
        self.playing = false;
        self.position = 0;
        self.frames_emitted = 0;
        self.tail_frames = 0;
    }
    /// Read the local preview state; cross-thread completion uses the separate atomic flag.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Settings are captured once per host callback. The same values drive each
    /// internal 128-frame block; Formant tail stays enabled once selected.
    pub fn render(
        &mut self,
        output: Option<&mut [f32]>,
        frames: usize,
        channels: usize,
        settings: Settings,
    ) -> PlaybackResult {
        let Some(output) = output else {
            return PlaybackResult::Continue;
        };
        assert!(channels == 1 || channels == 2);
        assert_eq!(output.len(), frames * channels);
        if !self.playing {
            output.fill(0.0);
            return PlaybackResult::Continue;
        }
        if settings.mask & EFFECT_FORMANT != 0 {
            self.tail_frames = StreamFormantShifter::MAX_DELAY;
        }
        let total = self.buffer.len() / 2;
        let output_total = total + self.tail_frames;
        let mut input = [0.0; SELECTED_BLOCK_SIZE];
        let mut processed = [0.0; SELECTED_BLOCK_SIZE];
        for offset in (0..frames).step_by(SELECTED_BLOCK_SIZE) {
            let count = input.len().min(frames - offset);
            let valid = count.min(total - self.position);
            for i in 0..count {
                input[i] = if i < valid {
                    let sample = read_pcm16_sample(&self.buffer, self.position) as f32 / 32768.0;
                    self.position += 1;
                    sample
                } else {
                    0.0
                };
            }
            self.processor
                .process_block(&input[..count], &mut processed[..count], settings);
            for i in 0..count {
                let sample = if self.frames_emitted < output_total {
                    processed[i]
                } else {
                    0.0
                };
                for channel in 0..channels {
                    output[(offset + i) * channels + channel] = sample;
                }
                self.frames_emitted += 1;
            }
        }
        if self.frames_emitted >= output_total {
            self.playing = false;
            PlaybackResult::Complete
        } else {
            PlaybackResult::Continue
        }
    }
}
