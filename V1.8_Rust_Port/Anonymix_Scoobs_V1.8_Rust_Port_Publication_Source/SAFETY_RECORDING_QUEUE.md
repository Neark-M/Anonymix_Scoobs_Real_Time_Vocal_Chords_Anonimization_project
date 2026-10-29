# Recording queue safety boundary

The C++ queue uses ordinary i16 slots published with release/acquire indices.
Rust needs an explicit UnsafeCell boundary to express the same SPSC algorithm.
This module contains the port's current unsafe code; the rest of the DSP core
uses ordinary safe Rust. Future C library bindings will have separate FFI boundaries.

## Invariants enforced by the safe API

1. Storage is allocated once, fully initialized, and never resized while split.
2. split requires a mutable queue borrow and returns exactly one producer and
   one consumer. Their fields are private and neither type implements Clone.
3. Each endpoint requires mutable access for sample operations. PhantomData<Cell>
   makes endpoints non-Sync, while allowing ownership transfer to a worker thread.
4. Safe code cannot reset, move or drop a queue while an endpoint will be used.
5. Producer acquires read before reusing freed slots; it releases written only
   after initializing all samples of an accepted block.
6. Consumer acquires written before reading published slots; it releases read
   only after its reads are complete. No slot is read while being overwritten.
7. Bounds and masking retain V1.8's fixed power-of-two capacity and full-block
   overflow rejection. Native 64-bit atomics are required by the target.

The unsafe Sync implementation relies on those private access paths. Public
status reads touch atomics only. Raw slot pointers never escape the module;
no mutable references to shared slots are constructed.

## Evidence and limits

Sequential C++ fixtures check queue PCM/counters over wraparound, over-capacity
blocks, empty pulls and reset. Actual producer/collector threads check ordering
across multiple ring wraps and stop during a registered callback. Compile-fail
documentation tests check reset borrowing and producer duplication restrictions.

These checks complement the invariant argument; tests do not enumerate every
possible scheduler interleaving. Device callback ownership and FFI safety are
not validated by this standalone queue milestone.

## Native owned endpoint pair (stage 5)

Consuming RecordingQueue::into_owned_split creates exactly one OwnedProducer and
one OwnedConsumer sharing the allocation. Neither handle is Clone or Sync.
They delegate to the same private slot/counter operations as borrowed endpoints.
The consumed queue cannot be reset or split again while these handles exist.
The consumer can move to the recording collector; the producer remains in the
native duplex callback state until a successful close, or is retained with that
state on a native close failure. No Arc clone/drop occurs in the callback.

## Reclaiming the original allocation (stage 6)

OwnedProducer::try_reclaim consumes the producer and succeeds only when the
consumer has released the other Arc. Failure returns the same sole producer;
no endpoint is duplicated. Mutable reset is available again only after success.
The native owner also disables callback admission and waits out old readers
before moving that producer. Its next split uses the same original fixed slot
allocation and inherited counters/policy, as C++ did across repeated recordings.
