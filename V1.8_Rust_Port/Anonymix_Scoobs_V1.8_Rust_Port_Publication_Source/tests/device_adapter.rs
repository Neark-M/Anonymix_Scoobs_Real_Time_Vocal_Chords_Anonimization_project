//! Device policy and callback integration checks; no physical device is opened.
use anonymix_v18_dsp_port::{
    capture::CaptureProcessor,
    device_plan::{latency_profiles, wasapi_attempts, RequestedMode},
    duplex::DuplexProcessor,
    live::LiveChain,
    portaudio_abi::*,
    Settings,
};

#[test]
fn exclusive_attempt_order_matches_v18() {
    let attempts = wasapi_attempts(RequestedMode::Exclusive, 2, 2);
    let tuples: Vec<_> = attempts
        .iter()
        .map(|a| (a.input, a.output, a.exclusive))
        .collect();
    assert_eq!(tuples, [(2, 2, true), (1, 2, true), (1, 1, false)]);
    let attempts = wasapi_attempts(RequestedMode::Exclusive, 1, 1);
    assert_eq!(
        attempts
            .iter()
            .map(|a| (a.input, a.output, a.exclusive))
            .collect::<Vec<_>>(),
        [(1, 1, true), (1, 1, false)]
    );
}
#[test]
fn shared_and_latency_hints_keep_original_predicate() {
    assert_eq!(wasapi_attempts(RequestedMode::Shared, 2, 2).len(), 1);
    assert_eq!(
        latency_profiles(true, false, 0.002, 0.003),
        [(0.002, 0.003)]
    );
    assert_eq!(
        latency_profiles(true, false, 0.020, 0.003),
        [(0.005, 0.003), (0.020, 0.003)]
    );
    assert_eq!(latency_profiles(true, true, 0.020, 0.030), [(0.020, 0.030)]);
    assert_eq!(
        latency_profiles(false, false, 0.020, 0.030),
        [(0.020, 0.030)]
    );
}
#[test]
fn windows_x64_c_layouts_and_descriptor_flags() {
    if std::mem::size_of::<usize>() != 8 {
        return;
    }
    assert_eq!(std::mem::size_of::<WasapiStreamInfo>(), 56);
    assert_eq!(
        std::mem::offset_of!(WasapiStreamInfo, host_processor_output),
        24
    );
    assert_eq!(std::mem::size_of::<DeviceInfo>(), 72);
    assert_eq!(
        std::mem::offset_of!(DeviceInfo, default_low_input_latency),
        32
    );
    assert_eq!(std::mem::size_of::<StreamParameters>(), 32);
    assert_eq!(
        std::mem::offset_of!(StreamParameters, suggested_latency),
        16
    );
    assert_eq!(std::mem::size_of::<StreamInfo>(), 32);
    assert_eq!(std::mem::size_of::<CallbackTimeInfo>(), 24);
    let info = WasapiStreamInfo::exclusive(true);
    assert_eq!((info.size, info.flags, info.channel_mask), (56, 5, 3));
    let mono = WasapiStreamInfo::exclusive(false);
    assert_eq!((mono.flags, mono.channel_mask), (1, 0));
}
#[test]
fn host_blocks_keep_256_frame_capture_boundaries() {
    let settings = Settings {
        mask: 31,
        pitch: 4.0,
        formant: 1.18,
        timbre: 60.0,
        prosody: 80.0,
        jitter: 0.02,
        volume: 0.5,
    };
    let input: Vec<_> = (0..624 * 2)
        .map(|i| ((i % 89) as f32 / 89.0 - 0.5) * 0.04)
        .collect();
    let mut host = DuplexProcessor::default();
    let mut output = vec![0.0; 624 * 2];
    let mut capture = CaptureProcessor::default();
    let mut live = LiveChain::default();
    let mut expected = Vec::new();
    for _ in 0..15 {
        host.process_host(
            Some(&input),
            Some(&mut output),
            624,
            2,
            2,
            true,
            0,
            true,
            settings,
        );
        expected.clear();
        for chunk in input.chunks(256 * 2) {
            let mut clean = vec![0.0; chunk.len() / 2];
            capture.process_chunk(chunk, 2, true, 0, &mut clean);
            let mut processed = vec![0.0; clean.len()];
            live.process_block(&clean, &mut processed, settings);
            for sample in processed {
                expected.extend_from_slice(&[sample, sample]);
            }
        }
        assert_eq!(output, expected);
    }
}
#[test]
fn missing_input_is_silence_and_does_not_advance_dsp() {
    let mut a = DuplexProcessor::default();
    let mut b = DuplexProcessor::default();
    let input = vec![0.25; 624];
    let mut first = vec![1.0; 624];
    let mut second = vec![0.0; 624];
    a.process_host(
        None,
        Some(&mut first),
        624,
        1,
        1,
        false,
        0,
        true,
        Settings::default(),
    );
    assert!(first.iter().all(|&v| v == 0.0));
    a.process_host(
        Some(&input),
        Some(&mut first),
        624,
        1,
        1,
        false,
        0,
        true,
        Settings::default(),
    );
    b.process_host(
        Some(&input),
        Some(&mut second),
        624,
        1,
        1,
        false,
        0,
        true,
        Settings::default(),
    );
    assert_eq!(first, second);
}
