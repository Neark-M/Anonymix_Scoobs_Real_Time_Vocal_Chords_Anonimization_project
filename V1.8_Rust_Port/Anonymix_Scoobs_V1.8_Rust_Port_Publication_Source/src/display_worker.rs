//! Dsp.cpp's one-request/latest-result display worker. The SDL frame
//! scheduler calls request once per visual frame; there is no separate FFT cap.
use crate::{
    observation::ObservationHub,
    spectrum::{SpectrumAnalyzer, SpectrumFrame, FFT_SIZE},
};
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};
#[derive(Default)]
struct Mailbox {
    requested: bool,
    stopping: bool,
    latest: Option<SpectrumFrame>,
}
#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
}
/// Coalesced FFT analysis worker with one pending request and one latest result.
/// If spawning fails, analysis uses the inherited caller-thread fallback.
pub struct DisplayFftWorker {
    hub: Arc<ObservationHub>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    fallback: Option<SpectrumAnalyzer>,
}
impl DisplayFftWorker {
    /// Create analysis state and try to start the display worker. No audio device is opened.
    pub fn start(hub: Arc<ObservationHub>) -> Self {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let thread_hub = Arc::clone(&hub);
        let worker = thread::Builder::new()
            .name("display-fft".into())
            .spawn(move || {
                let mut analyzer = None;
                let mut samples = [0.0; FFT_SIZE];
                loop {
                    {
                        let lock = thread_shared
                            .mailbox
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let mut lock = thread_shared
                            .wake
                            .wait_while(lock, |state| !state.stopping && !state.requested)
                            .unwrap_or_else(|e| e.into_inner());
                        if lock.stopping {
                            break;
                        }
                        lock.requested = false;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if !thread_hub.rings.snapshot_fft(&mut samples) {
                            return None;
                        }
                        Some(
                            analyzer
                                .get_or_insert_with(SpectrumAnalyzer::default)
                                .analyze(&samples),
                        )
                    }));
                    match result {
                        Ok(Some(frame)) => {
                            thread_shared
                                .mailbox
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .latest = Some(frame)
                        }
                        Err(_) => {
                            // Keep containment; the public application collects no report counter.
                        }
                        _ => {}
                    }
                }
            })
            .ok();
        Self {
            hub,
            shared,
            worker,
            fallback: None,
        }
    }
    /// Request one analysis update at the current visual cadence; repeated pending requests are combined.
    pub fn request(&mut self) {
        if self.worker.is_none() {
            let mut samples = [0.0; FFT_SIZE];
            if self.hub.rings.snapshot_fft(&mut samples) {
                let frame = self
                    .fallback
                    .get_or_insert_with(SpectrumAnalyzer::default)
                    .analyze(&samples);
                self.shared
                    .mailbox
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .latest = Some(frame);
            }
            return;
        }
        self.shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .requested = true;
        self.shared.wake.notify_one();
    }
    /// Take a completed latest spectrum without waiting for a running analysis.
    pub fn publish(&self) -> Option<SpectrumFrame> {
        self.shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .latest
            .take()
    }
    /// Distinguish the background worker from the original synchronous fallback.
    pub fn using_worker(&self) -> bool {
        self.worker.is_some()
    }
    /// Signal termination and join the FFT thread before releasing its shared histories.
    pub fn stop(&mut self) {
        self.shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stopping = true;
        self.shared.wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for DisplayFftWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn request_burst_has_one_pending_followup_and_shutdown_joins() {
        let hub = Arc::new(ObservationHub::default());
        let mut worker = DisplayFftWorker::start(Arc::clone(&hub));
        assert!(
            worker.using_worker(),
            "test requires a display worker thread"
        );
        hub.rings.with_fft_locked_for_test(|| {
            worker.request();
            let deadline = Instant::now() + Duration::from_secs(5);
            while worker.shared.mailbox.lock().unwrap().requested {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            // The first computation is held at the snapshot mutex. Repeated
            // frame requests must become one follow-up, never a growing queue.
            for _ in 0..10_000 {
                worker.request();
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(frame) = worker.publish() {
                assert!(frame.calibration_frames <= 2);
                if frame.calibration_frames == 2 {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "coalesced result missing");
            std::thread::sleep(Duration::from_millis(1));
        }
        worker.stop();
        assert!(!worker.using_worker());
    }
    #[test]
    fn unavailable_thread_uses_original_synchronous_display_fallback() {
        let hub = Arc::new(ObservationHub::default());
        let mut worker = DisplayFftWorker {
            hub,
            shared: Arc::new(Shared::default()),
            worker: None,
            fallback: None,
        };
        worker.request();
        let first = worker.publish().unwrap();
        assert_eq!(first.calibration_frames, 1);
        assert!(first.magnitudes.iter().all(|&value| value == 0.0));
        assert!(worker.publish().is_none());
        worker.request();
        assert_eq!(worker.publish().unwrap().calibration_frames, 2);
        worker.stop();
    }
}
