# Code map — Anonymix Scoobs V1.8 Rust port

## Start here

This is a direct port of the V1.8 development baseline. It is not the private
V1.8.2 branch, and it does not introduce a new DSP algorithm, resampler, audio
backend, memory policy or latency target. Read this map before changing block
sizes, processor reset points, atomics, buffer ownership or renderer geometry:
those choices affect observable behavior and the reference tests.

`src/bin/anonymix-scoobs.rs` starts the application. `desktop.rs` connects the
SDL callbacks, UI state, device owner and display analysis. `lib.rs` exposes the
modules and common constants. The internal Cargo crate name remains
`anonymix-v18-dsp-port` so existing imports and tests remain valid; its package
version is now **1.8.0** and its product name is **V1.8 Rust port**.

## Units and sample representations

| Value | Meaning |
| --- | --- |
| Sample rate | 48000 frames/second throughout application capture/DSP/playback/export. |
| Frame | One point in time across all channels, not one interleaved scalar. |
| Native input/output | Interleaved `f32`, one or two channels. Stereo output duplicates processed mono. |
| Cleaned DSP blocks | Mono, up to 256 frames; preserve the last partial block. |
| Recorded processing | Mono, up to 128 frames per internal preview/export block. |
| Stored take | Signed PCM16 bytes, little-endian, two bytes per mono frame. |
| Pitch | Semitones; zero is neutral. |
| Formant slider | Dimensionless variable-delay read ratio; 1.00 is neutral. |
| Timbre / Prosody | Signed slider percentages. |
| Jitter | Fractional pitch-ratio amount, 0 to 0.020; not a measured medical jitter value. |
| Volume | Linear multiplier; UI starts at 0.5. |
| Device latency hints | Seconds; requested and driver-reported values are not acoustic measurements. |
| SDL scheduling | Monotonic nanoseconds except the original animation input in seconds. |
| Geometry | Logical UI coordinates until the aspect-fit viewport maps to window/device pixels. |

`Settings::default()` is a neutral library/test value (no effects, volume 1).
The desktop's `UiModel` sets the actual original startup selection mask 3 and
volume 0.5. Do not mistake these two defaults for a UI discrepancy.

The 16000 value in `rescale_legacy_smoothing` references an old coefficient
time constant. It does not convert audio to 16 kHz. Variable-delay readers
change playback positions as an effect; the application's sample clock remains
48 kHz. Windows or drivers may still convert formats outside this code.

## Threads and owners

| Owner | Work | Boundary |
| --- | --- | --- |
| SDL main thread | Events, widget state, text/textures, animation, rendering, frame pacing. | `Rc` and native-library owners keep GUI handles off worker threads. |
| Device owner | COM, PortAudio initialization, open/reopen/close, recording/preview transitions. | Commands/results contain copied application data, never audio sample streams. |
| PortAudio callbacks | Capture cleanup, live/preview DSP, recording enqueue, display-history publication. | No UI access; mutable DSP/preview state has one serialized native writer. |
| Recording collector | Drain PCM16 SPSC ring and append to the original mutable take. | Only this worker grows the recording vector; callbacks never lock that vector. |
| Display FFT worker | Analyze a copied microphone window. | One pending request/latest result; synchronous fallback if worker creation fails. |
| WAV export worker | Full take snapshot, recorded DSP, filesystem write. | One export at a time; owner joins before resources disappear. |

Rust ownership does not replace the rules of the C APIs. `portaudio_windows.rs`
uses narrowly documented `UnsafeCell` access for serialized callbacks. The owner
turns off admission, waits for in-flight readers and closes/joins native work
before changing that state. A failed native close retains callback/library
ownership instead of freeing memory that an external callback might still use.
See `SAFETY_NATIVE_AUDIO.md`, `SAFETY_RECORDING_QUEUE.md` and `SAFETY_GUI.md`.

## Audio path

1. `device_plan.rs` and `device_policy.rs` preserve the original format attempts,
   endpoint identity checks, Shared preview cache and deferred reopen policy.
2. `portaudio_abi.rs`, `windows_endpoints.rs` and `portaudio_windows.rs` provide
   exact native declarations, owner-thread COM identity lookup and stream lifetime.
3. `duplex.rs` splits an arbitrary host callback into the original 256-frame
   pieces. Missing input does not advance capture/live DSP history. Exclusive
   idle output may render recorded preview; Shared duplex idle output is silent.
4. `capture.rs` selects/crossfades physical input channels, applies the high-pass
   and mode-specific gain/limiter/expander. Channel powers are functional inputs:
   removing them would change Exclusive gain and microphone-channel selection.
5. Recording mode queues the cleaned samples through `recording_queue.rs`.
   `recording.rs` implements admission/stop/drain. `take.rs` owns collector,
   retained mutable PCM and export worker. Whole-block drops mark a take
   incomplete; preview/export reject that take instead of silently using it.
6. Stream mode uses `live.rs`: pitch with prosody/jitter ratio modulation,
   formant-labelled delay, timbre, volume/prosody gain and inherited saturation.
7. `playback.rs` reads a full copied PCM take and calls `recorded.rs`. That chain
   intentionally uses a distinct recorded pitch processor. Formant uses the same
   inherited delay algorithm as live processing, with the original tail policy.
8. `pcm.rs` and `wav.rs` preserve capture/export conversion differences and the
   WAV header. `Save WAV` uses settings captured when requested; preview controls
   remain live independent atomics while it plays.

`effects.rs` contains onset-based Prosody, deterministic pitch Jitter, the Timbre
biquad and the formant-labelled variable-delay shifter. The Formant effect is
not independent vocal-tract envelope shifting: changing the delay read rate also
moves harmonics and may change perceived pitch. Do not document it otherwise.

`controls.rs` stores float bit patterns in independent atomics. Setup clones
`Arc` owners before native callbacks start; callbacks only borrow/read them.
An atomic load of all controls is not a transaction. Preserving this property
matches the independent C++ atomics. UI slider changes do not queue device opens.

## Display and interaction

`observation.rs` keeps waveform and FFT histories separately. A callback uses
`try_lock`: a busy reader may drop one visualization update, never delay audio
for a graph. `spectrum.rs` performs the original 4096-sample Hann FFT,
60-frame floor calibration, 96 logarithmic bands (20–20000 Hz), history and
peak normalization. These are display calculations, not an audio denoiser or
an absolute loudness meter. Do not add a second independent FFT frame limiter.

`ui_widgets.rs` owns stable widget indexes, integer slider handles, floating
legends, hit tests and one-second help. Handle/legend rounding is intentionally
different in a few cases. `ui_controller.rs` gates conflicting operations and
uses owner results to update captions. Preview completion crosses to SDL as
one atomic flag; the device owner closes the preview before Play is restored.

`ui_layout.rs` uses uniform aspect fitting and responsive placement.
`ui_animation.rs` retains the original quantized/glitching motion.
`ui_geometry.rs` creates spectrum/wave/quad triangles. `ui_raster.rs` describes
8x/4x resolves and size-dependent composition. `ui_renderer.rs` owns the actual
textures/fonts and preserves the approved varying apparent FFT halo thickness.
Straight rectangles do not acquire an extra outline.

`ui_timing.rs` supplies deadlines and frame-load classification;
`ui_budget.rs` consumes that load. The one interface/FFT ladder is
60/54/50/48/40/36/32/30 FPS, with the original raster reduction/recovery policy.
Audio sample rate, buffers and effects never change to meet a visual target.
This target is not a guarantee that arbitrary hardware always delivers it.

## Development versus publication

The development copy keeps `diagnostics.rs`, `audio-probe`, `parity-runner`,
`ui-render-probe`, frozen C++ reference sources, fixture regeneration tools,
historical validation and the visible console. Those historical records may
contain the previous V2/alpha milestone names; they are not product version IDs.
Existing stage-9 report behavior is retained; exact legacy C++ T1–T4 text was
not added as a new feature during this documentation/publication task.

The publication copy removes report collection/output, energy sums used only
for reports, the probe/parser executables and synthetic framebuffer capture.
There is no `diagnostics.rs` in that copy. `audio_pressure.rs` retains only the
callback-pressure counter consumed by the existing visual budget. Callback
timing needed by that budget, native error handling, incomplete-take protection,
worker joins and the interface's original state/error captions remain.
The Windows executable has the GUI subsystem and a silent panic hook. There is
no application startup/failure text on stdout/stderr and no automatic log file.
Third-party DLLs remain original binaries, outside this Rust source's logging control.

## Verification and safe maintenance

Run `cargo test --offline` before compiling. Tests use packaged binary outputs
from the frozen C++ baseline, not newly generated Rust expectations. They cover
DSP, capture, PCM/WAV, ring ordering/lifecycle, device policy, widgets/layout,
FFT and rendering calculations. Intentional `compile fail ... ok` documentation
tests enforce endpoint ownership; they are successful tests.

Use `cargo doc --offline --no-deps` for the English API reference. Read this map
and the safety notes before changing a callback or native function signature.
Keep ABI layout tests alongside any intentional SDL/PortAudio declaration change.
No user runs Cargo: `BUILD_PUBLIC.cmd` is a maintainer step producing the
ready-to-run ZIP. Keep runtime DLLs, icon and supplied notices together.
