//! Exercise real producer/collector threads and the stop handoff without devices.
use anonymix_v18_dsp_port::{
    pcm::capture_pcm16_sample,
    playback::{PlaybackResult, RecordedPlayback},
    recording::{collect_recording, RecordingControl},
    recording_queue::{RecordingQueue, CAPACITY},
    Settings,
};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

#[test]
fn collector_keeps_order_through_concurrent_ring_wraps() {
    let mut queue = RecordingQueue::default();
    let (mut producer, mut consumer) = queue.split();
    let mut control = RecordingControl::default();
    control.begin_after_join();
    let saved = Mutex::new(Vec::with_capacity(48_000 * 2 * 60));
    let expected = std::thread::scope(|scope| {
        let control = &control;
        let saved = &saved;
        let worker = scope.spawn(move || collect_recording(&mut consumer, control, saved));
        let source = scope.spawn(move || {
            let mut expected = Vec::with_capacity(1_048_576 * 2);
            let start = Instant::now();
            for block in 0..4096 {
                // Test-only pacing prevents intentional overflow in the order test.
                // The product callback still drops full blocks and never waits.
                while producer.status().current_frames as usize > CAPACITY - 512 {
                    assert!(
                        start.elapsed() < Duration::from_secs(15),
                        "collector stalled"
                    );
                    std::thread::yield_now();
                }
                let mut samples = [0.0; 256];
                for (i, sample) in samples.iter_mut().enumerate() {
                    *sample = ((block * 256 + i) % 1024) as f32 / 512.0 - 1.0;
                    expected.extend_from_slice(&capture_pcm16_sample(*sample).to_le_bytes());
                }
                {
                    let callback = control.enter_callback();
                    callback.queue_if_recording(&mut producer, &samples);
                }
            }
            control.stop_and_wait_for_callbacks();
            assert_eq!(producer.status().dropped_frames, 0);
            expected
        });
        let expected = source.join().unwrap();
        worker.join().unwrap().unwrap();
        expected
    });
    assert_eq!(*saved.lock().unwrap(), expected);
    assert!(!control.incomplete(queue.status().dropped_frames));
}

#[test]
fn stop_waits_for_callback_and_disables_late_enqueue() {
    let mut queue = RecordingQueue::default();
    let (mut producer, mut consumer) = queue.split();
    let mut control = RecordingControl::default();
    control.begin_after_join();
    let entered = Barrier::new(2);
    let release = Barrier::new(2);
    let saved = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        let control = &control;
        let entered = &entered;
        let release = &release;
        let source = scope.spawn(move || {
            let callback = control.enter_callback();
            assert!(callback.recording_at_entry);
            entered.wait();
            release.wait();
            callback.queue_if_recording(&mut producer, &[0.5; 256]);
        });
        entered.wait();
        let worker = scope.spawn(|| collect_recording(&mut consumer, control, &saved));
        let stopper = scope.spawn(|| control.stop_and_wait_for_callbacks());
        let start = Instant::now();
        while control.is_recording() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
        release.wait();
        source.join().unwrap();
        stopper.join().unwrap();
        worker.join().unwrap().unwrap();
    });
    assert!(saved.lock().unwrap().is_empty());
}

#[test]
fn recording_drops_mark_take_incomplete() {
    let mut queue = RecordingQueue::default();
    let (mut producer, mut consumer) = queue.split();
    let mut control = RecordingControl::default();
    control.begin_after_join();
    {
        let callback = control.enter_callback();
        callback.queue_if_recording(&mut producer, &vec![0.5; CAPACITY]);
        callback.queue_if_recording(&mut producer, &[0.25; 10]);
    }
    control.stop_and_wait_for_callbacks();
    let saved = Mutex::new(Vec::new());
    collect_recording(&mut consumer, &control, &saved).unwrap();
    assert_eq!(saved.lock().unwrap().len(), CAPACITY * 2);
    assert_eq!(consumer.status().dropped_frames, 10);
    assert!(control.incomplete(consumer.status().dropped_frames));
}

#[test]
fn playback_snapshot_is_copied_and_completed_output_is_padded() {
    let mut original = vec![0; 258];
    for bytes in original.chunks_exact_mut(2) {
        bytes.copy_from_slice(&16384_i16.to_le_bytes());
    }
    let mut playback = RecordedPlayback::default();
    assert!(playback.start_after_join(&original));
    assert!(!playback.start_after_join(&original));
    original.fill(0);
    let mut output = vec![99.0; 624 * 2];
    assert_eq!(
        playback.render(Some(&mut output), 624, 2, Settings::default()),
        PlaybackResult::Complete
    );
    assert!(output[..129 * 2].iter().all(|&s| s == 0.5));
    assert!(output[129 * 2..].iter().all(|&s| s == 0.0));
    playback.stop_after_join();
    assert!(playback.start_after_join(&original));
    playback.render(Some(&mut output), 624, 2, Settings::default());
    assert!(output.iter().all(|&s| s == 0.0));
}
