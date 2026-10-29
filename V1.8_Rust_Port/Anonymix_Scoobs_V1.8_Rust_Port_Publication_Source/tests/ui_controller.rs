//! Verify state transitions that connect UI actions to the already tested
//! native device owner. No audio driver, renderer or new thread is needed.
use anonymix_v18_dsp_port::{
    device_plan::RequestedMode,
    device_policy::{ActualMode, ReopenPhase},
    device_worker::{ActionKind, DeviceAction, DeviceEvent, DeviceSnapshot},
    ui_controller::UiController,
    ui_geometry::Point,
    ui_widgets::{UiAction, UiModel},
};
fn snapshot() -> DeviceSnapshot {
    DeviceSnapshot {
        ready: true,
        active: true,
        live: false,
        recording: false,
        playing: false,
        export_busy: false,
        frames: 48000,
        incomplete: false,
        requested: RequestedMode::Shared,
        actual: ActualMode::Shared,
        selection: None,
    }
}
fn ready() -> (UiController, UiModel) {
    let mut c = UiController::default();
    let mut ui = UiModel::default();
    c.event(DeviceEvent::Started(Ok(snapshot())), 1, &mut ui);
    (c, ui)
}
fn complete(
    c: &mut UiController,
    ui: &mut UiModel,
    action: ActionKind,
    state: DeviceSnapshot,
    now: u64,
) {
    c.event(
        DeviceEvent::Completed {
            action,
            result: Ok(state),
            state: None,
        },
        now,
        ui,
    );
}
#[test]
fn recording_and_live_switches_gate_duplicate_requests_and_update_same_buttons() {
    let (mut c, mut ui) = ready();
    assert!(matches!(
        c.plan(UiAction::Record, 2, false, &mut ui),
        Some(DeviceAction::RecordStart)
    ));
    assert_eq!(ui.widget("PA_Record").text, "Starting...");
    assert!(c.plan(UiAction::Record, 3, true, &mut ui).is_none());
    let mut s = snapshot();
    s.recording = true;
    complete(&mut c, &mut ui, ActionKind::RecordStart, s.clone(), 4);
    assert_eq!(ui.widget("PA_Record").text, "Stop Rec");
    assert!(matches!(
        c.plan(UiAction::AudioMode, 5, false, &mut ui),
        Some(DeviceAction::SetLive(true))
    ));
    s.recording = false;
    s.live = true;
    complete(&mut c, &mut ui, ActionKind::SetLive, s, 6);
    assert_eq!(ui.widget("AudioMode").text, "Mode: Stream");
    assert!(c.plan(UiAction::Play, 7, false, &mut ui).is_none());
    assert!(c.plan(UiAction::Save, 7, false, &mut ui).is_none());
}
#[test]
fn completed_preview_closes_on_owner_before_play_is_restored_and_never_overlaps() {
    let (mut c, mut ui) = ready();
    assert!(matches!(
        c.plan(UiAction::Play, 2, false, &mut ui),
        Some(DeviceAction::Play(_))
    ));
    assert!(c.plan(UiAction::Play, 3, true, &mut ui).is_none());
    let mut s = snapshot();
    s.playing = true;
    complete(&mut c, &mut ui, ActionKind::Play, s, 4);
    assert_eq!(ui.widget("PA_PlaySelected").text, "Stop");
    assert!(c.completed_preview(false, true, &mut ui).is_none());
    assert!(matches!(
        c.completed_preview(false, false, &mut ui),
        Some(DeviceAction::StopPreview)
    ));
    assert_eq!(ui.widget("PA_PlaySelected").text, "Stopping...");
    complete(&mut c, &mut ui, ActionKind::StopPreview, snapshot(), 5);
    assert_eq!(ui.widget("PA_PlaySelected").text, "Play");
}
#[test]
fn deferred_mode_clicks_use_latest_intent_and_settle_before_owner_reopen() {
    let (mut c, mut ui) = ready();
    let Some(DeviceAction::Reopen(r)) = c.plan(UiAction::WasapiMode, 10, false, &mut ui) else {
        panic!()
    };
    assert_eq!(r.mode, RequestedMode::Exclusive);
    assert!(c.plan(UiAction::WasapiMode, 20, true, &mut ui).is_none());
    assert_eq!(c.reopen.desired_mode(), RequestedMode::Shared);
    let mut s = snapshot();
    s.requested = RequestedMode::Exclusive;
    s.actual = ActualMode::Exclusive;
    complete(&mut c, &mut ui, ActionKind::Reopen, s, 30);
    assert_eq!(c.reopen.phase(), ReopenPhase::Settling);
    assert!(c.settled(499_999_999, false, &mut ui).is_none());
    let Some(DeviceAction::Reopen(r)) = c.settled(500_000_030, false, &mut ui) else {
        panic!()
    };
    assert_eq!(r.mode, RequestedMode::Shared);
    assert!(!r.refresh_devices);
}
#[test]
fn selectors_publish_during_other_transitions_but_reopen_gates_them_and_recording_blocks_close() {
    let (mut c, mut ui) = ready();
    c.plan(UiAction::Effect(4), 2, true, &mut ui);
    assert_eq!(ui.mask, 7);
    let mut s = snapshot();
    s.recording = true;
    complete(&mut c, &mut ui, ActionKind::RecordStart, s, 3);
    assert!(c.plan(UiAction::Reopen, 4, false, &mut ui).is_none());
    assert_eq!(c.reopen.phase(), ReopenPhase::Idle);
    complete(&mut c, &mut ui, ActionKind::RecordStop, snapshot(), 5);
    c.plan(UiAction::Reopen, 6, false, &mut ui);
    c.plan(UiAction::Effect(4), 7, true, &mut ui);
    assert_eq!(ui.mask, 7);
}
#[test]
fn fast_export_does_not_flash_failure_before_success_delivery() {
    let (mut c, mut ui) = ready();
    assert!(matches!(
        c.plan(UiAction::Save, 2, false, &mut ui),
        Some(DeviceAction::Save(_))
    ));
    // The job can finish before the owner creates its action snapshot.
    complete(&mut c, &mut ui, ActionKind::Save, snapshot(), 3);
    assert_eq!(ui.widget("PA_SaveSelected").text, "Saving...");
    c.event(DeviceEvent::Saved(Ok("take.wav".into())), 4, &mut ui);
    assert_eq!(ui.widget("PA_SaveSelected").text, "Save WAV");
    assert!(c.last_error.is_none());
}
#[test]
fn release_uses_fresh_pointer_and_delayed_help_requires_same_target_for_one_second() {
    let mut ui = UiModel::default();
    ui.refresh_layout(1430, 1030);
    let play = Point { x: 1320., y: 300. };
    ui.mouse_down(play, false, false);
    assert_eq!(
        ui.release_action(Point { x: 1429., y: 1029. }, false, false),
        None
    );
    ui.end_press();
    ui.mouse_down(play, false, false);
    assert_eq!(ui.release_action(play, false, false), Some(UiAction::Play));
    ui.end_press();
    let pitch = Point { x: 750., y: 300. };
    ui.hovered = ui.hit_test(pitch, false, false);
    let sizes = [(60., 27.); 6];
    assert_eq!(ui.help(pitch, &sizes, 100), None);
    assert_eq!(ui.help(pitch, &sizes, 1099), None);
    assert_eq!(ui.help(pitch, &sizes, 1100), Some(0));
    let formant = Point { x: 940., y: 300. };
    ui.hovered = ui.hit_test(formant, false, false);
    assert_eq!(ui.help(formant, &sizes, 1101), None);
}
