//! V1.8's fixed-capacity SPSC PCM16 recording queue.
//!
//! Unsafe sample access is confined here. Split handles enforce exactly one
//! producer and one consumer; sample publication follows the original atomics.
use crate::pcm::capture_pcm16_sample;
use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(not(target_has_atomic = "64"))]
compile_error!("The V1.8 recording queue requires native 64-bit atomics");

pub const CAPACITY: usize = 1 << 18;

/// Fixed PCM16 SPSC ring. Exactly one producer and one consumer own their respective cursors.
pub struct RecordingQueue {
    // Allocate once before starting any callback. Boxed storage avoids placing
    // the original half-megabyte static C++ array on a Windows thread stack.
    // Its capacity, indices, drop policy and reuse behavior remain unchanged.
    data: Box<[UnsafeCell<i16>]>,
    read: AtomicU64,
    written: AtomicU64,
    dropped: AtomicU64,
    high_water_frames: AtomicU64,
}

// SAFETY: raw slot access is private to push_samples and pull_samples. A mutable
// queue borrow or consuming into_owned_split creates the only endpoint pair;
// neither handle can be cloned or shared for concurrent slot access. Each uses
// &mut self and is not Sync. Acquire/release counters publish initialized slots
// and prevent concurrent producer/consumer access to the same slot.
unsafe impl Sync for RecordingQueue {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Conservative copied queue occupancy and drop totals. Drops make the take incomplete.
pub struct QueueStatus {
    pub current_frames: u64,
    pub dropped_frames: u64,
    pub peak_frames: u64,
}

impl Default for RecordingQueue {
    fn default() -> Self {
        let data: Vec<_> = (0..CAPACITY).map(|_| UnsafeCell::new(0)).collect();
        Self {
            data: data.into_boxed_slice(),
            read: AtomicU64::new(0),
            written: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            high_water_frames: AtomicU64::new(0),
        }
    }
}

impl RecordingQueue {
    /// Valid only after the previous producer and consumer have relinquished
    /// their handles. Rust's mutable borrow prevents resetting a split queue.
    pub fn reset(&mut self) {
        self.read.store(0, Ordering::Relaxed);
        self.written.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.high_water_frames.store(0, Ordering::Relaxed);
    }
    /// The pair prevents a reset while either endpoint will be used again:
    /// ```compile_fail
    /// use anonymix_v18_dsp_port::recording_queue::RecordingQueue;
    /// let mut queue = RecordingQueue::default();
    /// let (mut producer, consumer) = queue.split();
    /// queue.reset();
    /// producer.push(&[0.5]);
    /// ```
    /// Producers cannot be duplicated:
    /// ```compile_fail
    /// use anonymix_v18_dsp_port::recording_queue::RecordingQueue;
    /// let mut queue = RecordingQueue::default();
    /// let (producer, consumer) = queue.split();
    /// let duplicate = producer.clone();
    /// ```
    pub fn split(&mut self) -> (Producer<'_>, Consumer<'_>) {
        (
            Producer {
                queue: self,
                not_sync: PhantomData,
            },
            Consumer {
                queue: self,
                not_sync: PhantomData,
            },
        )
    }
    /// Read the independent queue counters; the snapshot may straddle concurrent producer/consumer work.
    pub fn status(&self) -> QueueStatus {
        // Same conservative diagnostic snapshot as the baseline: read producer
        // position first. This observation is not a synchronization gate.
        let write = self.written.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        QueueStatus {
            current_frames: if write >= read { write - read } else { 0 },
            dropped_frames: self.dropped.load(Ordering::Relaxed),
            peak_frames: self.high_water_frames.load(Ordering::Relaxed),
        }
    }
}

/// Borrowed single-producer endpoint. It cannot be cloned or shared for concurrent producers.
pub struct Producer<'a> {
    queue: &'a RecordingQueue,
    not_sync: PhantomData<Cell<()>>,
}
/// Borrowed single-consumer endpoint. It cannot be cloned or shared for concurrent consumers.
pub struct Consumer<'a> {
    queue: &'a RecordingQueue,
    not_sync: PhantomData<Cell<()>>,
}

impl Producer<'_> {
    /// Drop the entire incoming block when full, exactly like V1.8. No wait,
    /// notification, lock, allocation or retry occurs on this path.
    pub fn push(&mut self, input: &[f32]) {
        self.queue.push_samples(input);
    }

    /// Read the independent queue counters; the snapshot may straddle concurrent producer/consumer work.
    pub fn status(&self) -> QueueStatus {
        self.queue.status()
    }
}

impl Consumer<'_> {
    /// Copy available PCM16 samples into the destination and release those ring slots to the producer.
    pub fn pull(&mut self, destination: &mut [i16]) -> usize {
        self.queue.pull_samples(destination)
    }

    /// Check whether the consumer currently sees no readable samples.
    pub fn empty(&self) -> bool {
        self.queue.read.load(Ordering::Acquire) == self.queue.written.load(Ordering::Acquire)
    }
    /// Read the independent queue counters; the snapshot may straddle concurrent producer/consumer work.
    pub fn status(&self) -> QueueStatus {
        self.queue.status()
    }
}

// Native callbacks and the collector outlive the call that creates them. These
// owned endpoints keep the same queue alive without self-referential lifetimes.
// Neither endpoint is Clone or Sync; Arc shares only the queue allocation.
/// Single callback producer retaining the ring allocation until native callback ownership ends.
pub struct OwnedProducer {
    queue: Arc<RecordingQueue>,
    not_sync: PhantomData<Cell<()>>,
}
/// Single collector consumer retaining the ring allocation while it drains recorded samples.
pub struct OwnedConsumer {
    queue: Arc<RecordingQueue>,
    not_sync: PhantomData<Cell<()>>,
}

impl RecordingQueue {
    /// Consume the queue and create its only producer/consumer pair. Consuming
    /// self excludes outstanding borrowed endpoints and any later reset.
    pub fn into_owned_split(self) -> (OwnedProducer, OwnedConsumer) {
        let queue = Arc::new(self);
        (
            OwnedProducer {
                queue: Arc::clone(&queue),
                not_sync: PhantomData,
            },
            OwnedConsumer {
                queue,
                not_sync: PhantomData,
            },
        )
    }
    fn push_samples(&self, input: &[f32]) {
        let write = self.written.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if input.len() > CAPACITY - write.wrapping_sub(read) as usize {
            self.dropped
                .fetch_add(input.len() as u64, Ordering::Relaxed);
            return;
        }
        for (i, &sample) in input.iter().enumerate() {
            let index = write.wrapping_add(i as u64) as usize & (CAPACITY - 1);
            // SAFETY: this producer exclusively owns all unpublished free slots.
            // The acquired read counter proves the consumer has finished them.
            unsafe {
                *self.data[index].get() = capture_pcm16_sample(sample);
            }
        }
        self.written
            .store(write.wrapping_add(input.len() as u64), Ordering::Release);
        let occupancy = write.wrapping_add(input.len() as u64).wrapping_sub(read);
        if occupancy > self.high_water_frames.load(Ordering::Relaxed) {
            self.high_water_frames.store(occupancy, Ordering::Relaxed);
        }
    }
    fn pull_samples(&self, destination: &mut [i16]) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        let write = self.written.load(Ordering::Acquire);
        let count = destination.len().min(write.wrapping_sub(read) as usize);
        for (i, sample) in destination[..count].iter_mut().enumerate() {
            let index = read.wrapping_add(i as u64) as usize & (CAPACITY - 1);
            // SAFETY: the acquired write index publishes these initialized slots.
            // They remain owned by this consumer until its release store below.
            *sample = unsafe { *self.data[index].get() };
        }
        self.read
            .store(read.wrapping_add(count as u64), Ordering::Release);
        count
    }
}

/// Sealed endpoint traits let the unchanged collector accept either ownership
/// form. External code cannot introduce a second producer or consumer.
mod sealed {
    pub trait Sealed {}
}
pub trait SampleProducer: sealed::Sealed {
    fn push(&mut self, input: &[f32]);
}
pub trait SampleConsumer: sealed::Sealed {
    fn pull(&mut self, output: &mut [i16]) -> usize;
    fn empty(&self) -> bool;
}
impl sealed::Sealed for Producer<'_> {}
impl sealed::Sealed for Consumer<'_> {}
impl sealed::Sealed for OwnedProducer {}
impl sealed::Sealed for OwnedConsumer {}
impl SampleProducer for Producer<'_> {
    fn push(&mut self, input: &[f32]) {
        Producer::push(self, input);
    }
}
impl SampleConsumer for Consumer<'_> {
    fn pull(&mut self, output: &mut [i16]) -> usize {
        Consumer::pull(self, output)
    }
    fn empty(&self) -> bool {
        Consumer::empty(self)
    }
}
impl OwnedProducer {
    /// Recover the original fixed allocation after the consumer has joined and
    /// dropped its endpoint. Failure preserves the sole producer unchanged.
    pub fn try_reclaim(self) -> Result<RecordingQueue, Self> {
        match Arc::try_unwrap(self.queue) {
            Ok(queue) => Ok(queue),
            Err(queue) => Err(Self {
                queue,
                not_sync: PhantomData,
            }),
        }
    }
    /// Convert and enqueue one entire mono block; insufficient capacity drops the whole block without waiting.
    pub fn push(&mut self, input: &[f32]) {
        self.queue.push_samples(input);
    }
    /// Read the independent queue counters; the snapshot may straddle concurrent producer/consumer work.
    pub fn status(&self) -> QueueStatus {
        self.queue.status()
    }
}
impl SampleProducer for OwnedProducer {
    fn push(&mut self, input: &[f32]) {
        OwnedProducer::push(self, input);
    }
}
impl OwnedConsumer {
    /// Copy available PCM16 samples into the destination and release those ring slots to the producer.
    pub fn pull(&mut self, output: &mut [i16]) -> usize {
        self.queue.pull_samples(output)
    }
    /// Check whether the consumer currently sees no readable samples.
    pub fn empty(&self) -> bool {
        self.queue.read.load(Ordering::Acquire) == self.queue.written.load(Ordering::Acquire)
    }
    /// Read the independent queue counters; the snapshot may straddle concurrent producer/consumer work.
    pub fn status(&self) -> QueueStatus {
        self.queue.status()
    }
}
impl SampleConsumer for OwnedConsumer {
    fn pull(&mut self, output: &mut [i16]) -> usize {
        OwnedConsumer::pull(self, output)
    }
    fn empty(&self) -> bool {
        OwnedConsumer::empty(self)
    }
}

#[cfg(test)]
mod owned_lifetime_tests {
    use super::*;
    #[test]
    fn queue_allocation_can_be_reset_only_after_consumer_releases_its_endpoint() {
        let original = RecordingQueue::default();
        let allocation = original.data.as_ptr();
        let (producer, mut consumer) = original.into_owned_split();
        let mut producer = match producer.try_reclaim() {
            Err(producer) => producer,
            Ok(_) => panic!("consumer still exists"),
        };
        producer.push(&[0.5; 17]);
        let mut output = [0; 17];
        assert_eq!(consumer.pull(&mut output), 17);
        drop(consumer);
        let mut queue = match producer.try_reclaim() {
            Ok(queue) => queue,
            Err(_) => panic!("joined consumer must release queue"),
        };
        assert_eq!(queue.data.as_ptr(), allocation);
        queue.reset();
        let (mut producer, mut consumer) = queue.into_owned_split();
        producer.push(&[0.25; 9]);
        assert_eq!(consumer.pull(&mut output), 9);
        assert_eq!(&output[..9], &[8191; 9]);
        assert_eq!(consumer.status().peak_frames, 9);
    }
}
