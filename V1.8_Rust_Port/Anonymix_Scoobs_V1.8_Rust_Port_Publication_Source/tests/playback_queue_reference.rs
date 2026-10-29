//! Compare callback completion/tails and the fixed recording queue with C++.
use anonymix_v18_dsp_port::{
    playback::{PlaybackResult, RecordedPlayback},
    recording_queue::RecordingQueue,
    Settings,
};
use std::io::{Cursor, Read};

fn fixture(name: &str) -> Cursor<Vec<u8>> {
    Cursor::new(
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("validation/fixtures")
                .join(name),
        )
        .unwrap(),
    )
}
fn u32_value(input: &mut impl Read) -> u32 {
    let mut v = [0; 4];
    input.read_exact(&mut v).unwrap();
    u32::from_le_bytes(v)
}
fn u64_value(input: &mut impl Read) -> u64 {
    let mut v = [0; 8];
    input.read_exact(&mut v).unwrap();
    u64::from_le_bytes(v)
}
fn float(input: &mut impl Read) -> f32 {
    f32::from_bits(u32_value(input))
}
fn magic(input: &mut impl Read, expected: &[u8; 8]) {
    let mut v = [0; 8];
    input.read_exact(&mut v).unwrap();
    assert_eq!(&v, expected);
}

#[test]
fn preview_callbacks_stop_restart_channels_and_tails_match_cpp() {
    let mut input = fixture("playback_input.bin");
    let mut expected = fixture("playback_expected.bin");
    magic(&mut input, b"ANONPLY\0");
    let cases = u32_value(&mut input);
    assert_eq!(cases, 33);
    let mut maximum = 0.0_f32;
    let mut requests = 0;
    for case in 0..cases {
        let mut original = vec![0; u32_value(&mut input) as usize];
        input.read_exact(&mut original).unwrap();
        let mut playback = RecordedPlayback::default();
        assert!(playback.start_after_join(&original));
        for request in 0..u32_value(&mut input) {
            let frames = u32_value(&mut input) as usize;
            let channels = u32_value(&mut input) as usize;
            let command = u32_value(&mut input);
            let null_output = u32_value(&mut input) != 0;
            let s = Settings {
                mask: u32_value(&mut input) as u8,
                pitch: float(&mut input),
                formant: float(&mut input),
                timbre: float(&mut input),
                prosody: float(&mut input),
                jitter: float(&mut input),
                volume: float(&mut input),
            };
            if command == 1 {
                playback.stop_after_join();
            }
            if command == 2 {
                playback.start_after_join(&original);
            }
            let mut output = vec![0.123; frames * channels];
            let code = playback.render(
                if null_output { None } else { Some(&mut output) },
                frames,
                channels,
                s,
            );
            assert_eq!(
                if code == PlaybackResult::Continue {
                    0
                } else {
                    1
                },
                u32_value(&mut expected),
                "status case {case}, request {request}"
            );
            assert_eq!(
                playback.is_playing() as u32,
                u32_value(&mut expected),
                "active case {case}, request {request}"
            );
            for (sample_index, sample) in output.into_iter().enumerate() {
                let reference = float(&mut expected);
                let error = (sample - reference).abs();
                assert!(sample.is_finite()&&reference.is_finite()&&error<=2e-5,
                    "case {case}, request {request}, sample {sample_index}: Rust={sample}, C++={reference}");
                maximum = maximum.max(error);
            }
            requests += 1;
        }
    }
    assert_eq!(input.position(), input.get_ref().len() as u64);
    assert_eq!(expected.position(), expected.get_ref().len() as u64);
    println!("Playback: {cases} cases, {requests} requests, exact statuses, maximum absolute error {maximum}");
}

#[test]
fn queue_wrap_overflow_reset_and_pcm_match_cpp() {
    let mut input = fixture("queue_input.bin");
    let mut expected = fixture("queue_expected.bin");
    magic(&mut input, b"ANONQUE\0");
    let operations = u32_value(&mut input);
    let mut queue = RecordingQueue::default();
    for operation in 0..operations {
        match u32_value(&mut input) {
            0 => queue.reset(),
            1 => {
                let samples: Vec<_> = (0..u32_value(&mut input))
                    .map(|_| float(&mut input))
                    .collect();
                let (mut producer, _consumer) = queue.split();
                producer.push(&samples);
            }
            2 => {
                let mut samples = vec![0; u32_value(&mut input) as usize];
                let (_producer, mut consumer) = queue.split();
                let count = consumer.pull(&mut samples);
                assert_eq!(
                    count,
                    u32_value(&mut expected) as usize,
                    "pull count at {operation}"
                );
                for actual in &samples[..count] {
                    let mut bytes = [0; 2];
                    expected.read_exact(&mut bytes).unwrap();
                    assert_eq!(*actual, i16::from_le_bytes(bytes));
                }
            }
            _ => panic!("invalid fixture operation"),
        }
        let status = queue.status();
        assert_eq!(
            status.current_frames,
            u64_value(&mut expected),
            "occupancy at {operation}"
        );
        assert_eq!(
            status.dropped_frames,
            u64_value(&mut expected),
            "drops at {operation}"
        );
        assert_eq!(
            status.peak_frames,
            u64_value(&mut expected),
            "peak at {operation}"
        );
    }
    assert_eq!(input.position(), input.get_ref().len() as u64);
    assert_eq!(expected.position(), expected.get_ref().len() as u64);
    println!("Queue: {operations} operations, exact PCM, occupancy, drop and peak counts");
}
