//! Real collector/export workers with the native callback's portable routing.
//! No physical endpoint is opened by these tests.
use anonymix_v18_dsp_port::{
    capture::CaptureProcessor,
    duplex::DuplexProcessor,
    pcm::{capture_pcm16_sample, export_recording},
    playback::RecordedPlayback,
    recording_queue::{RecordingQueue, CAPACITY},
    take::{ExportWorker, RecordingSession},
    Settings,
};
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

fn temporary_directory() -> std::path::PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "anonymix-stage5-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn owned_queue_keeps_capacity_conversion_and_full_block_drop() {
    let (mut producer, mut consumer) = RecordingQueue::default().into_owned_split();
    let source: Vec<_> = (0..CAPACITY)
        .map(|i| (i % 127) as f32 / 63.0 - 1.0)
        .collect();
    producer.push(&source);
    producer.push(&[0.2; 17]);
    assert_eq!(producer.status().dropped_frames, 17);
    let mut output = vec![0; CAPACITY];
    assert_eq!(consumer.pull(&mut output), CAPACITY);
    assert!(consumer.empty());
    assert_eq!(
        output,
        source
            .iter()
            .map(|&v| capture_pcm16_sample(v))
            .collect::<Vec<_>>()
    );
    for _ in 0..4 {
        producer.push(&source);
        assert_eq!(consumer.pull(&mut output), CAPACITY);
    }
    assert_eq!(consumer.status().peak_frames, CAPACITY as u64);
}

#[test]
fn native_capture_route_records_cleaned_pcm_and_keeps_shared_output_silent() {
    for exclusive in [false, true] {
        let (session, mut input) = RecordingSession::begin().unwrap();
        let mut duplex = DuplexProcessor::default();
        let mut reference = CaptureProcessor::default();
        let mut expected = Vec::new();
        let mut routed = Vec::new();
        let mut idle_sizes = Vec::new();
        let samples: Vec<_> = (0..624 * 2)
            .map(|i| (i % 99) as f32 / 220.0 - 0.2)
            .collect();
        let mut output = vec![9.0; 624 * 2];
        for _ in 0..5 {
            duplex.process_host_routed(
                Some(&samples),
                Some(&mut output),
                624,
                2,
                2,
                exclusive,
                4,
                false,
                Settings::default(),
                |clean| {
                    routed.push(clean.len());
                    input.queue_cleaned(clean);
                },
                |out, count, channels| {
                    idle_sizes.push(count);
                    let out = out.unwrap();
                    assert_eq!(out.len(), count * channels);
                    out.fill(0.125);
                },
            );
            for chunk in samples.chunks(256 * 2) {
                let mut clean = vec![0.0; chunk.len() / 2];
                reference.process_chunk(chunk, 2, exclusive, 4, &mut clean);
                for value in clean {
                    expected.extend_from_slice(&capture_pcm16_sample(value).to_le_bytes());
                }
            }
            assert!(output
                .iter()
                .all(|&v| v == if exclusive { 0.125 } else { 0.0 }));
        }
        let take = session.finish().unwrap();
        assert!(!take.incomplete);
        assert_eq!(take.frames().unwrap(), 5 * 624);
        assert_eq!(take.snapshot_for_playback().unwrap(), expected);
        assert_eq!(routed, [256, 256, 112].repeat(5));
        assert_eq!(
            idle_sizes,
            if exclusive {
                [256, 256, 112].repeat(5)
            } else {
                vec![]
            }
        );
        // The native producer may remain alive with the duplex stream. Stop
        // must reject later samples without appending after the collector joins.
        input.queue_cleaned(&[0.9; 256]);
        assert_eq!(take.snapshot_for_playback().unwrap(), expected);
    }
}

#[test]
fn absent_capture_still_routes_exclusive_preview_in_original_subblocks() {
    let mut duplex = DuplexProcessor::default();
    let mut output = vec![99.0; 624];
    let mut counts = Vec::new();
    duplex.process_host_routed(
        None,
        Some(&mut output),
        624,
        1,
        1,
        true,
        0,
        false,
        Settings::default(),
        |_| panic!("null input must not record"),
        |out, frames, _| {
            counts.push(frames);
            out.unwrap().fill(0.2);
        },
    );
    assert_eq!(counts, [256, 256, 112]);
    assert!(output.iter().all(|&v| v == 0.2));
}

#[test]
fn live_route_never_records_or_renders_idle_preview() {
    let mut duplex = DuplexProcessor::default();
    let mut output = vec![0.0; 624];
    duplex.process_host_routed(
        Some(&[0.1; 624]),
        Some(&mut output),
        624,
        1,
        1,
        true,
        0,
        true,
        Settings::default(),
        |_| panic!("live mode must not record"),
        |_, _, _| panic!("live mode must not preview"),
    );
    assert!(output.iter().any(|&v| v != 0.0));
}

#[test]
fn real_collector_snapshot_preview_and_async_wav_retain_original_take() {
    let (session, mut input) = RecordingSession::begin().unwrap();
    for block in 0..12 {
        let clean: Vec<_> = (0..256)
            .map(|i| ((i + block) % 63) as f32 / 100.0 - 0.3)
            .collect();
        input.queue_cleaned(&clean);
    }
    let take = session.finish().unwrap();
    let original = take.snapshot_for_playback().unwrap();
    let settings = Settings {
        mask: 31,
        pitch: 3.0,
        formant: 1.25,
        timbre: 40.0,
        prosody: 50.0,
        jitter: 0.01,
        volume: 0.5,
    };
    let expected = export_recording(&original, settings);
    let directory = temporary_directory();
    let path = directory.join("recordings/test.wav");
    let mut worker = ExportWorker::default();
    worker.start(&take, settings, path.clone()).unwrap();
    // Preview can take its own copy while export runs on its own worker.
    let mut playback = RecordedPlayback::default();
    assert!(take.prepare_playback(&mut playback).unwrap());
    assert_eq!(worker.join().unwrap(), Some(path.clone()));
    assert!(!worker.is_busy());
    let wav = fs::read(path).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48_000);
    assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
    assert_eq!(&wav[44..], expected);
    assert_eq!(take.snapshot_for_playback().unwrap(), original);
    worker
        .start(&take, Settings::default(), directory.join("second.wav"))
        .unwrap();
    worker.join().unwrap();
    assert_eq!(take.snapshot_for_playback().unwrap(), original);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn incomplete_take_refuses_both_preview_and_export() {
    let (session, mut input) = RecordingSession::begin().unwrap();
    input.queue_cleaned(&vec![0.2; CAPACITY + 1]);
    let take = session.finish().unwrap();
    assert!(take.incomplete);
    let mut playback = RecordedPlayback::default();
    assert!(take.prepare_playback(&mut playback).is_err());
    assert!(ExportWorker::default()
        .start(&take, Settings::default(), "unused.wav".into())
        .is_err());
}

#[test]
fn export_worker_reports_real_file_errors_and_joins() {
    let (session, mut input) = RecordingSession::begin().unwrap();
    input.queue_cleaned(&[0.2; 256]);
    let take = session.finish().unwrap();
    let directory = temporary_directory();
    let mut worker = ExportWorker::default();
    // A directory cannot be opened as the destination WAV file.
    worker
        .start(&take, Settings::default(), directory.clone())
        .unwrap();
    assert!(worker.join().is_err());
    assert!(!worker.is_busy());
    fs::remove_dir(directory).unwrap();
}
