//! Recording collector and full-snapshot export worker ownership from Audio.cpp.
//! Device calls remain with the PortAudio owner; neither worker renders audio.
use crate::{
    pcm::export_selected_snapshot,
    recording::{collect_recording, CollectorError, RecordingControl},
    recording_queue::{OwnedProducer, QueueStatus, RecordingQueue},
    wav::write_pcm16_wav,
    Settings,
};
use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

/// The native callback owns this endpoint. Its control survives failed native
/// closes too, so a stopped/leaked callback cannot enqueue into a freed queue.
pub struct RecordingInput {
    pub(crate) producer: OwnedProducer,
    pub(crate) control: Arc<RecordingControl>,
}

/// Collector lifetime plus the original shared mutable PCM buffer. Drop drains and joins collection.
pub struct RecordingSession {
    control: Arc<RecordingControl>,
    saved: Arc<Mutex<Vec<u8>>>,
    collector: Option<JoinHandle<Result<QueueStatus, CollectorError>>>,
}

impl RecordingSession {
    /// Reserve the inherited first minute before any callback can start. The
    /// collector retains V1.8's 4096-sample batches and 10 ms wait policy.
    pub fn begin() -> Result<(Self, RecordingInput), String> {
        Self::begin_reusing(RecordingQueue::default(), None)
    }
    /// Restore V1.8's queue reset and retained PCM capacity across takes. The
    /// device owner has joined the previous collector and disabled its producer.
    pub(crate) fn begin_reusing(
        mut queue: RecordingQueue,
        previous: Option<RecordedTake>,
    ) -> Result<(Self, RecordingInput), String> {
        let saved = previous
            .map(|take| take.saved)
            .unwrap_or_else(|| Arc::new(Mutex::new(Vec::new())));
        {
            let mut pcm = saved.lock().map_err(|_| "Recorded buffer poisoned")?;
            pcm.clear();
            if pcm.capacity() < 48_000 * 2 * 60 {
                pcm.try_reserve(48_000 * 2 * 60)
                    .map_err(|e| e.to_string())?;
            }
        }
        queue.reset();
        let mut control = RecordingControl::default();
        control.begin_after_join();
        let control = Arc::new(control);
        let (producer, mut consumer) = queue.into_owned_split();
        let worker_control = Arc::clone(&control);
        let worker_saved = Arc::clone(&saved);
        let collector = thread::Builder::new()
            .name("recording-collector".into())
            .spawn(move || {
                collect_recording(&mut consumer, &worker_control, &worker_saved)?;
                Ok(consumer.status())
            })
            .map_err(|e| format!("Cannot launch recording collector: {e}"))?;
        Ok((
            Self {
                control: Arc::clone(&control),
                saved,
                collector: Some(collector),
            },
            RecordingInput { producer, control },
        ))
    }

    /// Disable enqueueing, wait for callbacks, drain and join the collector.
    /// The duplex device may remain open for Exclusive recorded playback.
    pub fn finish(mut self) -> Result<RecordedTake, String> {
        let status = self.stop()?;
        Ok(RecordedTake {
            saved: Arc::clone(&self.saved),
            status,
            incomplete: self.control.incomplete(status.dropped_frames),
        })
    }

    fn stop(&mut self) -> Result<QueueStatus, String> {
        self.control.stop_and_wait_for_callbacks();
        let worker = self.collector.take().ok_or("Recording already joined")?;
        worker
            .join()
            .map_err(|_| "Recording collector panicked".to_string())?
            .map_err(|e| format!("Recording collector failed: {e:?}"))
    }
}
impl Drop for RecordingSession {
    fn drop(&mut self) {
        if self.collector.is_some() {
            let _ = self.stop();
        }
    }
}

/// The original mutable PCM buffer remains protected by a mutex. Playback and
/// export each make their own complete copy, exactly as in the C++ application.
pub struct RecordedTake {
    saved: Arc<Mutex<Vec<u8>>>,
    pub status: QueueStatus,
    pub incomplete: bool,
}
impl RecordedTake {
    /// Return the number of mono PCM16 frames in the retained take, under its non-callback mutex.
    pub fn frames(&self) -> Result<usize, String> {
        Ok(self
            .saved
            .lock()
            .map_err(|_| "Recorded buffer poisoned")?
            .len()
            / 2)
    }
    /// Validate the completed take and make the original full PCM copy.
    pub fn snapshot_for_playback(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        Ok(self
            .saved
            .lock()
            .map_err(|_| "Recorded buffer poisoned")?
            .clone())
    }
    /// Copy directly into the retained preview buffer while holding the source
    /// mutex; avoid adding a second whole-take copy to the C++ playback path.
    pub fn prepare_playback(
        &self,
        playback: &mut crate::playback::RecordedPlayback,
    ) -> Result<bool, String> {
        self.validate()?;
        let original = self.saved.lock().map_err(|_| "Recorded buffer poisoned")?;
        Ok(playback.start_after_join(&original))
    }
    fn validate(&self) -> Result<(), String> {
        if self.incomplete {
            return Err("Recording lost frames; make a new take".into());
        }
        if self.frames()? == 0 {
            return Err("The recording is empty".into());
        }
        Ok(())
    }
}

/// One export at a time. Dropping this owner joins its worker before releasing
/// the source buffer. The caller disables new recording while an export runs.
#[derive(Default)]
pub struct ExportWorker {
    worker: Option<JoinHandle<Result<PathBuf, String>>>,
}
impl ExportWorker {
    /// Read whether the export thread is still running; a finished result still needs to be joined.
    pub fn is_busy(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }
    /// Validate the take and start one WAV export with a copied settings value.
    /// The worker makes a full PCM snapshot before processing; no recording buffer redesign is introduced.
    pub fn start(
        &mut self,
        take: &RecordedTake,
        settings: Settings,
        path: PathBuf,
    ) -> Result<(), String> {
        if self.is_busy() {
            return Err("A WAV export is already running".into());
        }
        if self.worker.is_some() {
            self.join()?;
        }
        take.validate()?;
        let saved = Arc::clone(&take.saved);
        self.worker = Some(
            thread::Builder::new()
                .name("wav-export".into())
                .spawn(move || {
                    // Keep the full copy on the export worker, before in-place DSP.
                    let snapshot = saved
                        .lock()
                        .map_err(|_| "Recorded buffer poisoned")?
                        .clone();
                    let processed = export_selected_snapshot(snapshot, settings);
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut file = File::create(&path).map_err(|e| e.to_string())?;
                    write_pcm16_wav(&mut file, &processed, 1, 48_000).map_err(|e| e.to_string())?;
                    file.flush().map_err(|e| e.to_string())?;
                    Ok(path)
                })
                .map_err(|e| format!("Cannot launch WAV export worker: {e}"))?,
        );
        Ok(())
    }
    /// Wait for any export worker and return its file path or error, consuming that pending result.
    pub fn join(&mut self) -> Result<Option<PathBuf>, String> {
        self.worker
            .take()
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| "WAV export worker panicked".to_string())?
            })
            .transpose()
    }
}
impl Drop for ExportWorker {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

impl RecordingInput {
    /// Portable callback entry used by lifecycle tests. The native adapter also
    /// holds a guard for the entire host call before processing its subblocks.
    pub fn queue_cleaned(&mut self, samples: &[f32]) {
        let callback = self.control.enter_callback();
        if callback.recording_at_entry {
            callback.queue_if_recording(&mut self.producer, samples);
        }
    }
}

#[cfg(test)]
mod retained_take_tests {
    use super::*;
    use crate::playback::RecordedPlayback;

    #[test]
    fn new_take_retains_original_pcm_capacity_without_altering_preview_copy() {
        let (session, mut input) = RecordingSession::begin().unwrap();
        input.queue_cleaned(&[0.5; 256]);
        let take = session.finish().unwrap();
        let mut playback = RecordedPlayback::default();
        take.prepare_playback(&mut playback).unwrap();
        let (allocation, capacity) = {
            let saved = take.saved.lock().unwrap();
            (saved.as_ptr(), saved.capacity())
        };
        let queue = match input.producer.try_reclaim() {
            Ok(queue) => queue,
            Err(_) => panic!("collector has joined"),
        };
        let (session, mut input) = RecordingSession::begin_reusing(queue, Some(take)).unwrap();
        input.queue_cleaned(&[0.25; 128]);
        let next = session.finish().unwrap();
        {
            let saved = next.saved.lock().unwrap();
            assert_eq!(saved.as_ptr(), allocation);
            assert_eq!(saved.capacity(), capacity);
        }
        assert_eq!(next.frames().unwrap(), 128);
        let mut old_preview = [0.0; 256];
        playback.render(Some(&mut old_preview), 256, 1, Settings::default());
        assert!(old_preview.iter().all(|&value| value == 16383.0 / 32768.0));
        assert_eq!(
            next.snapshot_for_playback().unwrap(),
            [8191_i16.to_le_bytes()].repeat(128).concat()
        );
    }
}
