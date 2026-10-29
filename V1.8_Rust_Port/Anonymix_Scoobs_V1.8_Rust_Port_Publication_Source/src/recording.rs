//! V1.8 collector body and capture/stop handshake.
//! take.rs owns the worker; the native adapter owns its callback producer.
use crate::recording_queue::{SampleConsumer, SampleProducer};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Condvar, Mutex,
};
use std::time::Duration;

/// Atomic callback admission and stop/join handshake shared with the recording collector.
pub struct RecordingControl {
    recording: AtomicBool,
    callbacks_in_flight: AtomicU32,
    collector_failed: AtomicBool,
    wake_mutex: Mutex<()>,
    wake: Condvar,
}

impl Default for RecordingControl {
    fn default() -> Self {
        Self {
            recording: AtomicBool::new(false),
            callbacks_in_flight: AtomicU32::new(0),
            collector_failed: AtomicBool::new(false),
            wake_mutex: Mutex::new(()),
            wake: Condvar::new(),
        }
    }
}

impl RecordingControl {
    /// Application worker only, after previous users have stopped and joined.
    /// Mutable access excludes outstanding callback/collector borrows.
    pub fn begin_after_join(&mut self) {
        self.collector_failed.store(false, Ordering::Release);
        self.recording.store(true, Ordering::SeqCst);
    }
    /// Enter before reading the recording flag, like DuplexCallbackLifetime.
    pub fn enter_callback(&self) -> RecordingCallback<'_> {
        self.callbacks_in_flight.fetch_add(1, Ordering::SeqCst);
        let recording_at_entry = self.recording.load(Ordering::SeqCst);
        RecordingCallback {
            control: self,
            recording_at_entry,
        }
    }
    /// Device worker only, never from inside a registered callback. Join the
    /// collector after this method, then inspect drops/failure before using PCM.
    pub fn stop_and_wait_for_callbacks(&self) {
        self.recording.store(false, Ordering::SeqCst);
        while self.callbacks_in_flight.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
        self.wake.notify_one();
    }
    /// Read whether allocation or mutex failure interrupted the collector.
    pub fn collector_failed(&self) -> bool {
        self.collector_failed.load(Ordering::Acquire)
    }
    /// Read current enqueue admission. Already entered callbacks still participate in the stop handshake.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }
    /// Reject a take if any queue frames were dropped or the collector failed.
    pub fn incomplete(&self, dropped_frames: u64) -> bool {
        dropped_frames != 0 || self.collector_failed()
    }
    fn fail_collector(&self) {
        self.collector_failed.store(true, Ordering::Release);
        self.recording.store(false, Ordering::SeqCst);
    }
}

/// RAII guard counting one in-flight recording callback; dropping it releases stop waiters.
pub struct RecordingCallback<'a> {
    control: &'a RecordingControl,
    pub recording_at_entry: bool,
}
impl RecordingCallback<'_> {
    /// Re-read the flag at enqueue time, as QueuePortAudioRecording does.
    /// Stop may have disabled recording since this callback started.
    pub fn queue_if_recording(&self, producer: &mut impl SampleProducer, samples: &[f32]) {
        if self.control.recording.load(Ordering::SeqCst) {
            producer.push(samples);
        }
    }
}
impl Drop for RecordingCallback<'_> {
    fn drop(&mut self) {
        self.control
            .callbacks_in_flight
            .fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, PartialEq, Eq)]
/// Failures that stop collection and mark the recorded take incomplete.
pub enum CollectorError {
    AllocationFailed,
    PoisonedMutex,
}

/// Run on the recording worker. Only this consumer appends PCM bytes; the audio
/// callback never locks the recording vector. The 4096-sample batch and 10 ms
/// poll interval are inherited unchanged from CollectPortAudioRecording.
pub fn collect_recording(
    consumer: &mut impl SampleConsumer,
    control: &RecordingControl,
    saved_pcm: &Mutex<Vec<u8>>,
) -> Result<(), CollectorError> {
    let result = collect_inner(consumer, control, saved_pcm);
    if result.is_err() {
        control.fail_collector();
    }
    result
}

fn collect_inner(
    consumer: &mut impl SampleConsumer,
    control: &RecordingControl,
    saved_pcm: &Mutex<Vec<u8>>,
) -> Result<(), CollectorError> {
    let mut batch = [0_i16; 4096];
    loop {
        let count = consumer.pull(&mut batch);
        if count != 0 {
            let mut bytes = saved_pcm
                .lock()
                .map_err(|_| CollectorError::PoisonedMutex)?;
            let first = bytes.len();
            bytes
                .try_reserve(count * 2)
                .map_err(|_| CollectorError::AllocationFailed)?;
            bytes.resize(first + count * 2, 0);
            for i in 0..count {
                let sample = batch[i] as i32;
                bytes[first + i * 2] = (sample & 0xff) as u8;
                bytes[first + i * 2 + 1] = ((sample >> 8) & 0xff) as u8;
            }
            continue;
        }
        if !control.recording.load(Ordering::SeqCst)
            && control.callbacks_in_flight.load(Ordering::SeqCst) == 0
            && consumer.empty()
        {
            return Ok(());
        }
        let lock = control
            .wake_mutex
            .lock()
            .map_err(|_| CollectorError::PoisonedMutex)?;
        // Only the worker waits. The callback never notifies or calls the OS.
        drop(
            control
                .wake
                .wait_timeout_while(lock, Duration::from_millis(10), |_| {
                    control.recording.load(Ordering::SeqCst)
                })
                .map_err(|_| CollectorError::PoisonedMutex)?,
        );
    }
}
