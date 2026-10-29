//! Observable cache/default-device/burst behavior inherited from Audio/App.cpp.
use anonymix_v18_dsp_port::{
    capture::CaptureProcessor,
    controls::AtomicSettings,
    device_plan::RequestedMode as Mode,
    device_policy::{
        ActualMode, EndpointIds, PreviewCacheKey, ReopenCoordinator, ReopenPhase, REOPEN_QUIET_NS,
    },
    duplex::DuplexProcessor,
    live::LiveChain,
    Settings,
};
use std::sync::Arc;

fn ids(capture: &str, render: &str) -> EndpointIds {
    EndpointIds {
        capture: Some(capture.encode_utf16().collect()),
        render: Some(render.encode_utf16().collect()),
    }
}

#[test]
fn unchanged_devices_skip_enumeration_but_unknown_or_changed_ids_refresh() {
    let opened = ids("mic-a", "speaker-a");
    assert!(!opened.needs_refresh(&ids("mic-a", "speaker-a")));
    assert!(opened.needs_refresh(&ids("mic-b", "speaker-a")));
    assert!(opened.needs_refresh(&ids("mic-a", "speaker-b")));
    assert!(opened.needs_refresh(&EndpointIds {
        capture: None,
        render: opened.render.clone()
    }));
    assert!(opened.needs_refresh(&EndpointIds {
        capture: opened.capture.clone(),
        render: Some(vec![])
    }));
}

#[test]
fn shared_cache_requires_index_opaque_identity_and_channel_count() {
    let endpoint: Vec<_> = "endpoint-a".encode_utf16().collect();
    let other: Vec<_> = "endpoint-b".encode_utf16().collect();
    let cached = PreviewCacheKey {
        device: 7,
        endpoint: endpoint.clone(),
        channels: 1,
    };
    assert!(cached.matches(7, Some(&endpoint), 1));
    assert!(!cached.matches(8, Some(&endpoint), 1));
    assert!(!cached.matches(7, Some(&other), 1));
    assert!(!cached.matches(7, Some(&endpoint), 2));
    assert!(!cached.matches(7, None, 1));
    assert!(!cached.matches(7, Some(&[]), 1));
    assert!(!cached.matches(-1, Some(&endpoint), 1));
}

#[test]
fn running_reopen_combines_last_mode_only_after_completion_and_quiet_interval() {
    let mut c = ReopenCoordinator::new(Mode::Shared, ActualMode::Shared);
    assert!(c.request_refresh(0, false).unwrap().refresh_devices);
    assert!(c.request_mode(Mode::Exclusive, 1, true).is_none());
    assert!(c.tick(REOPEN_QUIET_NS * 2).is_none()); // Worker still running.
    c.complete(Mode::Shared, ActualMode::Shared, REOPEN_QUIET_NS * 3);
    assert_eq!(c.phase(), ReopenPhase::Settling);
    assert!(c.tick(REOPEN_QUIET_NS * 4 - 1).is_none());
    let request = c.tick(REOPEN_QUIET_NS * 4).unwrap();
    assert_eq!(request.mode, Mode::Exclusive);
    assert!(!request.refresh_devices);
    c.complete(Mode::Exclusive, ActualMode::Exclusive, REOPEN_QUIET_NS * 5);
    assert_eq!(c.phase(), ReopenPhase::Idle);
    assert!(c.tick(u64::MAX).is_none());
}

#[test]
fn reverting_to_opened_mode_cancels_followup_and_duplicate_refresh_is_ignored() {
    let mut c = ReopenCoordinator::new(Mode::Shared, ActualMode::Shared);
    c.request_refresh(0, false).unwrap();
    assert!(c.request_refresh(1, true).is_none());
    c.request_mode(Mode::Exclusive, 2, true);
    c.request_mode(Mode::Shared, 3, true);
    c.complete(Mode::Shared, ActualMode::Shared, 4);
    assert_eq!(c.phase(), ReopenPhase::Idle);
    assert!(c.tick(u64::MAX).is_none());
}

#[test]
fn refresh_during_settling_combines_with_mode_and_resets_deadline() {
    let mut c = ReopenCoordinator::new(Mode::Shared, ActualMode::Shared);
    c.request_refresh(0, false);
    c.request_mode(Mode::Exclusive, 1, true);
    c.complete(Mode::Shared, ActualMode::Shared, 2);
    c.request_refresh(REOPEN_QUIET_NS, true);
    assert!(c.tick(REOPEN_QUIET_NS * 2 - 1).is_none());
    let request = c.tick(REOPEN_QUIET_NS * 2).unwrap();
    assert!(request.refresh_devices);
    assert_eq!(request.mode, Mode::Exclusive);
}

#[test]
fn recording_blocks_new_reopen_without_corrupting_idle_intent() {
    let mut c = ReopenCoordinator::new(Mode::Shared, ActualMode::Shared);
    assert!(c.request_mode(Mode::Exclusive, 0, true).is_none());
    assert!(c.request_refresh(1, true).is_none());
    assert_eq!(c.phase(), ReopenPhase::Idle);
    assert_eq!(c.requested_mode(), Mode::Shared);
    assert_eq!(
        c.request_mode(Mode::Exclusive, 2, false).unwrap().mode,
        Mode::Exclusive
    );
}

#[test]
fn exclusive_fallback_does_not_schedule_an_unrequested_retry() {
    let mut c = ReopenCoordinator::new(Mode::Shared, ActualMode::Shared);
    c.request_mode(Mode::Exclusive, 0, false).unwrap();
    c.complete(Mode::Shared, ActualMode::Shared, 1);
    assert_eq!(c.phase(), ReopenPhase::Idle);
    assert!(c.tick(u64::MAX).is_none());
    assert_eq!(c.request_refresh(2, false).unwrap().mode, Mode::Shared);
}

#[test]
fn independent_atomic_controls_are_not_torn_or_claimed_transactional() {
    let a = Settings {
        mask: 1,
        pitch: -12.0,
        formant: 0.8,
        timbre: -40.0,
        prosody: -50.0,
        jitter: 0.0,
        volume: 0.25,
    };
    let b = Settings {
        mask: 31,
        pitch: 7.0,
        formant: 1.3,
        timbre: 60.0,
        prosody: 70.0,
        jitter: 0.015,
        volume: 0.75,
    };
    let controls = Arc::new(AtomicSettings::new(a));
    let writer = Arc::clone(&controls);
    let worker = std::thread::spawn(move || {
        for _ in 0..20_000 {
            writer.store(b);
            writer.store(a);
        }
    });
    for _ in 0..20_000 {
        let read = controls.load();
        assert!([a.mask, b.mask].contains(&read.mask));
        for (read, first, second) in [
            (read.pitch, a.pitch, b.pitch),
            (read.formant, a.formant, b.formant),
            (read.timbre, a.timbre, b.timbre),
            (read.prosody, a.prosody, b.prosody),
            (read.jitter, a.jitter, b.jitter),
            (read.volume, a.volume, b.volume),
        ] {
            assert!([first.to_bits(), second.to_bits()].contains(&read.to_bits()));
        }
    }
    worker.join().unwrap();
}

#[test]
fn live_mode_and_controls_are_observed_at_original_subblock_boundaries() {
    let source: Vec<_> = (0..624).map(|i| (i % 57) as f32 / 100.0 - 0.28).collect();
    let shifted = Settings {
        mask: 1,
        pitch: 4.0,
        ..Settings::default()
    };
    let neutral = Settings::default();
    let mut duplex = DuplexProcessor::default();
    let mut output = vec![0.0; 624];
    let mut read = 0;
    let mut recorded = Vec::new();
    duplex.process_host_dynamic(
        Some(&source),
        Some(&mut output),
        624,
        1,
        1,
        false,
        1,
        || {
            let index = read;
            read += 1;
            (index != 1, if index == 0 { shifted } else { neutral })
        },
        |clean| recorded.extend_from_slice(clean),
        |_, _, _| panic!("Shared never uses inline preview"),
    );
    let mut capture = CaptureProcessor::default();
    let mut live = LiveChain::default();
    let mut expected = Vec::new();
    let mut expected_recorded = Vec::new();
    for (index, chunk) in source.chunks(256).enumerate() {
        let mut clean = vec![0.0; chunk.len()];
        capture.process_chunk(chunk, 1, false, 1, &mut clean);
        let mut out = vec![0.0; clean.len()];
        if index == 1 {
            expected_recorded.extend_from_slice(&clean);
        } else {
            live.process_block(&clean, &mut out, if index == 0 { shifted } else { neutral });
        }
        expected.extend_from_slice(&out);
    }
    assert_eq!(read, 3);
    assert_eq!(output, expected);
    assert_eq!(recorded, expected_recorded);
}
