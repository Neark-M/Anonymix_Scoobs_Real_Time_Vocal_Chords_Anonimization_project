//! PCM16 byte conversion and complete-snapshot export from the V1.8 baseline.
//! Capture truncation and processed-export rounding intentionally remain distinct.
use crate::{
    effects::StreamFormantShifter, recorded::SelectedAudioProcessor, Settings, EFFECT_FORMANT,
    SELECTED_BLOCK_SIZE,
};

/// Decode one little-endian signed PCM16 sample; the caller supplies a valid sample index.
pub fn read_pcm16_sample(bytes: &[u8], sample_index: usize) -> i16 {
    let offset = sample_index * 2;
    i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// Convert one normalized float to export PCM16 at the given valid sample index.
pub fn write_pcm16_sample(bytes: &mut [u8], sample_index: usize, sample: f32) {
    // f32::round, like C++ lround, rounds halfway values away from zero.
    let value = (sample * 32768.0).clamp(-32768.0, 32767.0).round() as i16;
    let offset = sample_index * 2;
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

/// Same 32767 scale and truncation as RecordingQueue::Push, not export's 32768.
pub fn capture_pcm16_sample(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Preserve the whole-take copy. Immutable shared takes and streamed WAV export
/// belong to the private V1.8.2 proposals and are deliberately not used here.
pub fn export_recording(original: &[u8], settings: Settings) -> Vec<u8> {
    assert_eq!(original.len() % 2, 0);
    let snapshot = original.to_vec();
    export_selected_snapshot(snapshot, settings)
}

/// Transform a private PCM16 mono snapshot in place with a fresh DSP history.
/// Preserve V1.8's nominal-delay drain/discard and original take duration.
pub fn export_selected_snapshot(mut snapshot: Vec<u8>, settings: Settings) -> Vec<u8> {
    assert_eq!(snapshot.len() % 2, 0);
    let mut processor = SelectedAudioProcessor::default();
    let mut input = [0.0; SELECTED_BLOCK_SIZE];
    let mut output = [0.0; SELECTED_BLOCK_SIZE];
    let total = snapshot.len() / 2;
    let delay = if settings.mask & EFFECT_FORMANT != 0 {
        StreamFormantShifter::NOMINAL_DELAY
    } else {
        0
    };
    for start in (0..total + delay).step_by(SELECTED_BLOCK_SIZE) {
        let count = input.len().min(total + delay - start);
        for i in 0..count {
            input[i] = if start + i < total {
                read_pcm16_sample(&snapshot, start + i) as f32 / 32768.0
            } else {
                0.0
            };
        }
        processor.process_block(&input[..count], &mut output[..count], settings);
        for i in 0..count {
            if start + i >= delay {
                write_pcm16_sample(&mut snapshot, start + i - delay, output[i]);
            }
        }
    }
    snapshot
}
