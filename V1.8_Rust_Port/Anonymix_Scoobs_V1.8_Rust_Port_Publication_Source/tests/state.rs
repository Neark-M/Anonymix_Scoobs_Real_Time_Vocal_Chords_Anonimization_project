//! Behavioral invariants complement the independent C++ reference comparison.
use anonymix_v18_dsp_port::{
    effects::PitchJitter, live::LiveChain, recorded::SelectedAudioProcessor, Settings,
    EFFECT_FORMANT,
};

#[test]
fn reset_restores_recorded_processor_history() {
    let input = [0.25; 128];
    let mut processor = SelectedAudioProcessor::default();
    let mut first = [0.0; 128];
    let mut reset = [0.0; 128];
    let settings = Settings {
        mask: 31,
        pitch: 5.0,
        formant: 1.18,
        timbre: 65.0,
        prosody: 100.0,
        jitter: 0.02,
        ..Settings::default()
    };
    processor.process_block(&input, &mut first, settings);
    for _ in 0..100 {
        processor.process_block(&input, &mut reset, settings);
    }
    processor.reset();
    processor.process_block(&input, &mut reset, settings);
    assert_eq!(first, reset);
}

#[test]
fn neutral_stream_keeps_legacy_saturation() {
    let mut processor = LiveChain::default();
    let mut result = [0.0; 256];
    processor.process_block(&[0.25; 256], &mut result, Settings::default());
    assert!(result.iter().all(|&s| s == (0.25_f32 * 0.85).tanh()));
}

#[test]
fn neutral_formant_delay_is_retained_in_recording() {
    let mut processor = SelectedAudioProcessor::default();
    let settings = Settings {
        mask: EFFECT_FORMANT,
        ..Settings::default()
    };
    let mut output = [0.0; 128];
    for _ in 0..3 {
        processor.process_block(&[0.25; 128], &mut output, settings);
        assert!(output.iter().all(|&s| s == 0.0));
    }
    processor.process_block(&[0.25; 128], &mut output, settings);
    assert!(output.iter().all(|&s| s == 0.25));
}

#[test]
fn jitter_disabled_does_not_reseed_generator() {
    let mut reused = PitchJitter::default();
    let mut fresh = PitchJitter::default();
    reused.next_ratio(0.02);
    assert_eq!(reused.next_ratio(0.0), 1.0);
    assert_ne!(reused.next_ratio(0.02), fresh.next_ratio(0.02));
}
