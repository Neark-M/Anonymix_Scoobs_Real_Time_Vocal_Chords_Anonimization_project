# Anonymix Scoobs V1.8 — source map

## Audio and worker ownership

- `App.cpp` owns SDL lifecycle, controls and device-worker coordination. Device operations stay on the worker; SDL continues rendering during reopen.
- `Audio.cpp` selects PortAudio devices, maintains the duplex stream and recorded playback, collects microphone PCM16 from `RecordingQueue.h`, and starts WAV export. `Wav.cpp` writes the resulting PCM16 file beside the executable in `recordings/`.
- `Stream.cpp` runs the 48 kHz callback. It selects the microphone channel, cleans the captured samples, feeds the display rings and processes live output. Callback blocks larger than the fixed DSP buffer are split into smaller chunks.
- `Formant.h`, `PitchJitter.h`, `Timbre.h` and `Prosody.h` implement the effects. The effect labeled Formant changes a delay-line read rate and may also move harmonics; it is not an independent spectral-envelope shifter. Prosody reacts to energy onsets, not linguistic meaning.
- The compact `AudioPressure.h` counts only callback overruns and device x-runs for visual adaptation. No audio-level or latency reports run in this publication copy.

## UI and display

- `AppRender.cpp` draws the interface; `AppUpdate.cpp` publishes slider values to audio atomics and handles asynchronous playback results.
- `UiLayout.h` fits the authored canvas into the physical window without stretching. `Widgets.h` owns widget geometry, actions and hit testing.
- `Dsp.cpp` analyzes a 4096-sample visual FFT window. `Visualization.cpp` draws the 20 Hz–20 kHz spectrum, including the direct physical-pixel composite for small windows, and the waveform. This FFT does not add a 4096-sample playback buffer.
- `RenderBudget.h` starts at 60 FPS and an 8x FFT raster. Sustained visual overload or audio pressure can move down its visual-quality ladder. Timing used for that decision remains in the release application; diagnostic printouts do not.
- `State.h` and `State.cpp` declare and define shared state. Most UI-to-callback controls use atomics. The callback never locks the recording vector; the SPSC queue publishes samples to its collector with release/acquire ordering.

The original `FFT_Direct_Pixel_Composition` archive preserves development diagnostics and remains the V2 base. Comments and identifiers in this publication source are in English.
