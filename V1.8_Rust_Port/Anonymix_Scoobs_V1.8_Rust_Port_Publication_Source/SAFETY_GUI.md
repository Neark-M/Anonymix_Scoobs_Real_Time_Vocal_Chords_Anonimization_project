# SDL GUI safety boundary — V1.8 Rust port

## C ABI and DLL lifetime

The SDL3/SDL_ttf wrappers load only fixed official symbols with fixed C
signatures. Event storage has 128 bytes and 8-byte alignment. Used event
variants are read only after checking their discriminant. Surface dimensions
are read while an SDL-owned surface is alive, then its texture is created and
that surface is destroyed. Official header facts and DLL exports are checked.

Each texture retains an Rc to its loaded SDL API. Rc and raw renderer handles
keep the GUI objects non-Send/non-Sync. The renderer explicitly drops all cached
text, interface, spectrum and oblique textures before destroying its SDL
renderer/window. Fonts close before TTF_Quit. The outer desktop loader remains
alive until SDL_EnterAppMainCallbacks returns, including its internal final
SDL_Quit call. No user callback/symbol signature is accepted by a safe wrapper.

## SDL callbacks and reentry

The application uses the same SDL callback architecture as the C++ reference,
not a new blocking event-poll loop. Its live DesktopApp resides in main-thread
thread-local RefCell storage. A callback arriving on another thread cannot
obtain that main-thread object. A reentrant callback cannot acquire an already
borrowed GUI state. Normal mouse/window events and iteration are handled in the
SDL main callback sequence; no reference to an SDL event is retained.

Every C entry point contains ordinary Rust unwinding. Failure returns an SDL
failure result and tears down through AppQuit; unwinding never crosses C. This
does not recover allocation abort, invalid native pointers or driver failure.

## Audio ownership

The UI owns controls and copied status, not streams. DeviceWorker creates COM
and PortAudio on its existing owner thread. The UI publishes an Arc-owned set
of independent atomic controls, shared with the installed callback contexts.
Arc cloning is performed during setup/owner transitions only. Both output
routes read the same controls. The shared preview-completion atomic does not
expose PreviewState, processors or recording bytes to SDL.

The callback marks completion; the UI schedules the existing stop/close action
before restoring Play. Native close/wait/join rules, failed-close retention,
recording producer/collector lifetime and export full-copy ownership remain as
documented in SAFETY_NATIVE_AUDIO.md. Drop joins display/device/export work
before renderer/application state is released.

## Visual scheduling and data

FFT requests are made once per scheduled SDL frame. The original visual budget
owns the single 60-to-30 ladder; there is no independent FFT limiter. Geometry,
render targets, label caches, responsive layout and mouse state belong only to
SDL. Audio/display histories retain their preexisting independent try-lock
writes, so rendering does not hold a blocking lock needed by audio callbacks.

## Validation limits

The deterministic Linux rendering comparisons belong to the separate development
archive. The publication source removes the synthetic capture constructor and
readback executable. It retains the original monitor-fit creation and native
resize path. Prior software-GL captures do not establish Windows GPU/Arial,
multi-monitor DPI, physical audio or real-time performance on other machines.

Hardware validation remains necessary. Native ABI/COM/DLL boundaries retain
unsafe code; Rust ownership is not a claim that every external driver is safe.
