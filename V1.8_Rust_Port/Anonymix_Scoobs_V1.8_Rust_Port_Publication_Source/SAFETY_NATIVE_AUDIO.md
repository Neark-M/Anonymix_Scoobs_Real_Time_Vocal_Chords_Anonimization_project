# Native audio safety boundary — Windows x64

## ABI

The adapter uses PortAudio V19's C ABI. Windows long/unsigned long are 32 bits
on x64, so these arguments and fields use i32/u32, not a Linux c_ulong type.
repr(C), fixed pointer sizes, WasapiStreamInfo size/offsets and stream/device
struct layouts are checked. Flags and the version-1 WASAPI descriptor match
the fallback in V1.8 WasapiPortAudio.h and the public C header.

Sources: https://portaudio.com/docs/v19-doxydocs/portaudio_8h.html and
https://portaudio.com/docs/v19-doxydocs/pa__win__wasapi_8h_source.html .
The packaged DLL is inspected without executing it; SHA-256/export metadata
is recorded under validation. No caller-supplied callback or arbitrary symbol
signature is exposed by the safe adapter API.

## Lifetimes and threads

The Rust adapter uses one COM-initialized device-owner thread (the persistent device worker).
The Rust PortAudio owner cannot be sent or shared across Rust threads. Each
LiveStream borrows that owner, preventing library teardown while it exists.

Callback userdata has a stable shared allocation. Only the serialized native
callback for that stream touches DuplexProcessor inside UnsafeCell. Owner-thread
observations use atomics; settings use the independent atomics published by the frontend. The
callback never changes reference counts or performs normal-path heap allocation,
locking, console output or waiting. Callback buffers remain borrowed only for
the duration and channel count supplied by PortAudio. Channels are published
before StartStream, which can invoke callbacks before returning.

Device stop/close runs on the owner thread. Callback state is released only
after successful CloseStream joins callback users. On a native close failure,
the adapter deliberately retains the context, DLL and COM initialization rather
than risking a callback into freed state/code. This exceptional retention is
reported as an error; it is not the normal cleanup path or a buffer policy.

The callback catches ordinary Rust unwinding, silences the valid output buffer
and returns paAbort. This is a final FFI boundary guard, not a substitute for
valid input/control contracts. Allocation failure aborts or other process-fatal
conditions are not made recoverable by catch_unwind.

## Current limits

Physical streams cannot be exercised on the Linux verification host. The
adapted native-body check does not prove that DLL initialization, native linking,
device acquisition, driver timing or shutdown succeeds on the user's hardware.
Those require Windows hardware tests. The desktop now connects the earlier
recording/playback/export and device lifecycle through SDL. Native-body validation
uses an adapted Linux copy, not a Windows target or SDK. The public variant deliberately contains no diagnostic reports.

## Stage-5 recording lifetime

The sole producer lives inside the duplex callback context; the sole owned
consumer moves into the collector. A shared queue allocation protects their
lifetimes without cloning either endpoint. Slot/counter operations and atomic
orders are the same as the borrowed endpoints. Only the collector locks or grows
the PCM vector. Recording stop disables enqueueing, waits for registered calls,
drains and joins the consumer before returning a take. A leaked native context
retains its producer/control; session teardown still disables future enqueueing.

## Stage-5 preview ownership

PreviewState is inside UnsafeCell. One device owner prepares it with no admitted
reader and publishes playing with sequentially consistent ordering. Every render
increments the sequentially consistent in-flight count before testing playing;
a disabled render never touches state. Stop stores false, waits out admitted
renders, and joins/closes separate Shared output before the state is reused.
The handshake also covers Exclusive inline rendering while the duplex remains
open. Each count has a drop guard, including unwinding.

Only actual Exclusive mode invokes the inline route. Shared mode uses one
separate serialized preview callback. Mutable LiveStream methods prohibit
concurrent owner actions. A failed close retains its preview allocation and DLL,
then prevents another playback start on that owner. Duplex cleanup still runs
even if separate preview cleanup failed. These rules prevent a leaked old native
callback from racing a new renderer or dereferencing freed state.

The ordinary C callbacks catch Rust unwinding before returning to native code.
The three native callback unit tests use valid disjoint simulated C buffers;
they do not call device functions. They run directly on Windows with cargo test,
and via an explicitly adapted temporary source copy on the Linux check host.

## Export lifetime

The completed source remains a mutex-protected mutable Vec, as in C++. The
export worker locks it only for a complete snapshot copy, processes that private
copy and writes the WAV. Playback makes its own full copy. ExportWorker joins on
explicit completion or drop. Sharing the buffer owner here is not the private
V1.8.2 immutable take-store optimization.

## Stage-6 device owner and COM IDs

The desktop device owner constructs the COM apartment, DLL/API owner and all streams
inside one long-lived worker. The UI/front thread sends typed actions and reads
copied results; it never owns PortAudio or COM pointers. Shutdown queues behind
an active transaction and joins the owner. Development-only console probes are kept in the separate development archive.

MMDevice queries use CoCreateInstance with the SDK's enumerator class/interface
IDs, eMultimedia role and the original render/capture directions. Counted COM
interfaces are released on that apartment. GetId allocations are copied as
opaque UTF-16 and released with CoTaskMemFree. These operations never run in an
audio callback. Only the SDK-defined vtable slots are invoked; callers cannot
supply arbitrary COM pointers or slot numbers through the public interface.

Primary API references:
- https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdeviceenumerator-getdefaultaudioendpoint
- https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdevice-getid
- https://portaudio.com/docs/v19-doxydocs/portaudio_8h_source.html

## Stage-6 retained preview and refresh

Normal Shared stop disables admission, waits, and aborts/joins the separate
stream while retaining its handle. Reuse additionally requires a matching
PortAudio WASAPI index, nonempty Windows endpoint ID and output channel count.
A failed restart or uncertain identity closes before the next native open.
Reopen closes the cached preview before closing the duplex. The live wrapper
count must reach zero before PortAudio termination/enumeration refresh. A failed
native close poisons that owner and retains userdata/code; reuse/refresh is then
refused instead of creating competing old/new renderers.

A successful reopen keeps the same callback allocation and live histories.
Capture generation resets the capture cleanup as in C++. Channel/mode values
are republished before StartStream. No raw endpoint/device-info pointer survives
termination; reports contain copied strings and numbers.

## Stage-6 recording endpoint replacement

Callbacks register an in-flight reader before observing capture admission. When
admission is false, they do not form any reference into the RecordingInput cell.
The owner disables admission, waits out prior readers, reclaims the sole producer
only after the joined consumer releases its Arc, resets the original queue and
publishes the new endpoint. New disabled callbacks cannot touch the cell while
the owner replaces it. The final sequentially consistent publication admits
callbacks to the initialized endpoint. No Arc clone/drop, allocation, mutex or
wait is added to the audio callback. The same slot payload/counter operations
are used by all queue endpoint forms.

The mutable recorded Vec and its capacity are retained across takes, matching
the C++ global buffer. Export gates prevent resetting a source in use by a save
job. Preview owns a separate full copy, so later captures do not change it.

## Stage-6 controls and frontend

AtomicSettings stores f32 bit patterns in native AtomicU32 values. Independent
loads/stores preserve the original independent control model; no coherent
multi-field snapshot is promised. Live state/controls are loaded at original
256-frame subdivisions. Preview settings are loaded once per render callback.
Only application commands/results use channels; microphone samples still use
the inherited lock-free SPSC queue. The frontend's deferred mode intent is
single-thread state and cannot mutate native callback state directly.

The worker's panic boundary corresponds to the original action exception guard.
It does not recover process-fatal allocation failure or make invalid native
pointers safe. Mock owner tests prove dispatch/gating/join behavior; physical
COM/cache/reopen behavior still requires Windows hardware tests.

## Stage-7 observation lifetime and callback boundary

An ObservationHub is allocated before native startup and shared by owned Arc
handles on device/main/display threads. Native callbacks borrow their already
retained hub without cloning/dropping Arc. Report-only capture levels are removed;
channel energy needed by the capture algorithm remains. The waveform and FFT histories use independent try-lock writes,
with fixed buffers and no notification from the callback. Blocking snapshots,
FFT computation and result publication happen only on display/main threads.
Readonly observer slices cannot change the speaker DSP samples.

The callback clock is observed once per qualifying live host callback solely
for the original functional audio-pressure predicate. Input/output discontinuity
flags and deadline misses increment one independent counter consumed by the
visual budget. Host timestamp reports, squared-output sums and per-stream totals
are absent. Errors still silence/abort callbacks and retain unsafe-to-free owners.

The hub survives mode changes/reopen. The display worker owns its analyzer,
coalesces requests to one bit and publishes one latest frame under a separate
mutex. No native callback holds that result mutex or waits for FFT. Worker
shutdown wakes and joins before releasing its own data. The original fallback
computes only on the requesting main/display thread if spawning failed.

The desktop uses the original SDL visual scheduler and one shared interface/FFT
cadence. No console probe or independent final FFT FPS cap is packaged here.
