//! Run the independent C++ fixtures directly with Cargo on Windows or Linux.
//! The fixtures are generated from frozen source, not from this Rust library.
use anonymix_v18_dsp_port::{
    capture::CaptureProcessor, live::LiveChain, pcm::export_recording,
    recorded::SelectedAudioProcessor, wav::write_pcm16_wav, Settings,
};
use std::io::{Cursor, Read};

fn fixture(name: &str) -> Cursor<Vec<u8>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("validation/fixtures")
        .join(name);
    Cursor::new(std::fs::read(path).expect("packaged C++ fixture missing"))
}
fn u32_value(input: &mut impl Read) -> u32 {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}
fn float_value(input: &mut impl Read) -> f32 {
    f32::from_bits(u32_value(input))
}
fn magic(input: &mut impl Read, expected: &[u8; 8]) {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, expected);
}
fn settings(input: &mut impl Read, mask: u8) -> Settings {
    Settings {
        mask,
        pitch: float_value(input),
        formant: float_value(input),
        timbre: float_value(input),
        prosody: float_value(input),
        jitter: float_value(input),
        volume: float_value(input),
    }
}
fn close(actual: f32, expected: f32, context: &str) {
    assert!(
        actual.is_finite() && expected.is_finite() && (actual - expected).abs() <= 2e-5,
        "{context}: Rust={actual}, C++={expected}"
    );
}

#[test]
fn all_66_dsp_scenarios_match_cpp_reference() {
    let mut input = fixture("dsp_input.bin");
    let mut expected = fixture("dsp_expected.bin");
    magic(&mut input, b"ANONV18\0");
    let cases = u32_value(&mut input);
    assert_eq!(cases, 66);
    let mut total = 0;
    let mut max_error = 0.0_f32;
    for case in 0..cases {
        let mode = u32_value(&mut input);
        let blocks = u32_value(&mut input);
        let mut live = LiveChain::default();
        let mut recorded = SelectedAudioProcessor::default();
        for block in 0..blocks {
            let count = u32_value(&mut input) as usize;
            let mask = u32_value(&mut input) as u8;
            let s = settings(&mut input, mask);
            let samples: Vec<f32> = (0..count).map(|_| float_value(&mut input)).collect();
            let mut output = vec![0.0; count];
            if mode == 0 {
                live.process_block(&samples, &mut output, s);
            } else {
                recorded.process_block(&samples, &mut output, s);
            }
            let context = format!("DSP case {case}, block {block}");
            for sample in output {
                let reference = float_value(&mut expected);
                close(sample, reference, &context);
                max_error = max_error.max((sample - reference).abs());
                total += 1;
            }
        }
    }
    assert_eq!(total, 1_980_000);
    println!("DSP: {cases} cases, {total} samples, maximum absolute error {max_error}");
    assert_eq!(input.position(), input.get_ref().len() as u64);
    assert_eq!(expected.position(), expected.get_ref().len() as u64);
}

#[test]
fn capture_shared_exclusive_and_generations_match_cpp_reference() {
    let mut input = fixture("capture_input.bin");
    let mut expected = fixture("capture_expected.bin");
    magic(&mut input, b"ANONCAP\0");
    let cases = u32_value(&mut input);
    assert_eq!(cases, 8);
    let mut total = 0;
    let mut max_error = 0.0_f32;
    for case in 0..cases {
        let mut capture = CaptureProcessor::default();
        for block in 0..u32_value(&mut input) {
            let count = u32_value(&mut input) as usize;
            let channels = u32_value(&mut input) as usize;
            let exclusive = u32_value(&mut input) != 0;
            let generation = u32_value(&mut input);
            let samples: Vec<f32> = (0..count * channels)
                .map(|_| float_value(&mut input))
                .collect();
            let mut output = vec![0.0; count];
            capture.process_chunk(&samples, channels, exclusive, generation, &mut output);
            let context = format!("capture case {case}, block {block}");
            for sample in output {
                let reference = float_value(&mut expected);
                close(sample, reference, &context);
                max_error = max_error.max((sample - reference).abs());
                total += 1;
            }
        }
    }
    assert_eq!(total, 480_000);
    println!("Capture: {cases} cases, {total} samples, maximum absolute error {max_error}");
    assert_eq!(input.position(), input.get_ref().len() as u64);
    assert_eq!(expected.position(), expected.get_ref().len() as u64);
}

#[test]
fn complete_export_snapshots_and_wav_headers_match_cpp_reference() {
    let mut input = fixture("export_input.bin");
    let mut expected = fixture("export_expected.bin");
    magic(&mut input, b"ANONEXP\0");
    let cases = u32_value(&mut input);
    assert_eq!(cases, 32);
    let mut max_error = 0_i32;
    for case in 0..cases {
        let mask = u32_value(&mut input) as u8;
        let length = u32_value(&mut input) as usize;
        let s = settings(&mut input, mask);
        let mut original = vec![0; length];
        input.read_exact(&mut original).unwrap();
        let before = original.clone();
        let result = export_recording(&original, s);
        assert_eq!(before, original, "export modified the original recording");
        assert_eq!(result.len(), length, "export changed duration");
        let mut actual = Vec::new();
        write_pcm16_wav(&mut actual, &result, 1, 48_000).unwrap();
        let mut reference = vec![0; length + 44];
        expected.read_exact(&mut reference).unwrap();
        assert_eq!(&actual[..44], &reference[..44], "WAV header, case {case}");
        for (index, (a, b)) in actual[44..]
            .chunks_exact(2)
            .zip(reference[44..].chunks_exact(2))
            .enumerate()
        {
            let a = i16::from_le_bytes([a[0], a[1]]) as i32;
            let b = i16::from_le_bytes([b[0], b[1]]) as i32;
            max_error = max_error.max((a - b).abs());
            assert!(
                (a - b).abs() <= 1,
                "export case {case}, sample {index}: Rust={a}, C++={b}"
            );
        }
    }
    assert_eq!(input.position(), input.get_ref().len() as u64);
    assert_eq!(expected.position(), expected.get_ref().len() as u64);
    println!(
        "Export: {cases} cases, exact WAV headers, maximum PCM error {max_error} integer units"
    );
}
