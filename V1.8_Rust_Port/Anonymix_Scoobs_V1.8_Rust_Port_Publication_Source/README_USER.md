# Anonymix Scoobs — V1.8 Rust port

Experimental local voice-effects application for Windows x64. This is a Rust
port of C++ V1.8, with the same 48-kHz processing and effects. It does not claim
better anonymity, sound quality or latency than the C++ application.

## Run

Extract the complete ZIP into a writable folder. Double-click
`anonymix-scoobs.exe`. Keep the three DLLs, `logo.bmp`, `THIRD_PARTY_NOTICES.txt`
and `LICENSES` folder with it. There is no Rust, Cargo or Visual Studio setup
for ordinary users. The system must supply the Microsoft Visual C++ x64
runtime used by the supplied PortAudio DLL; if Windows reports a missing
`VCRUNTIME140.dll`, install Microsoft's supported x64 redistributable.

## Use

- Recording mode: Start Rec, speak, Stop Rec, then Play or Save WAV.
- Use the five effect selection buttons and sliders. Effects can be combined.
- Play changes to Stop; the application uses one preview route at a time.
- Preview sliders/selections remain adjustable. Save WAV captures settings
  when requested. WAV files are mono PCM16 at 48000 Hz in `recordings`, beside
  the executable. Wait until saving finishes before deleting/moving files.
- Stream mode sends processed microphone audio to the output. Use headphones
  and a suitable volume to avoid acoustic feedback.
- Shared/Exclusive selects the requested WASAPI mode. Read the opened mode:
  unsupported requests may fall back. Exclusive can reserve the device and
  retains the C++ version's possible background hiss on some capture devices.
- Reopen Audio refreshes devices after a microphone/output change. Stop a
  recording before reopening. Sliders and graphs share the adaptive visual cadence.
- Hover an effect button or slider name for one second to read its description.

The FFT shows the cleaned microphone, not the recorded playback. Its normalized
peaks do not measure absolute loudness. The effect labelled Formant retains the
original variable-delay algorithm; it can move pitch/harmonics and does not
independently shift a vocal-tract spectral envelope.

The public application opens no console and writes no diagnostic log. Existing
button states still indicate normal transitions/failures. Close the window to
stop and join audio/export work. Keep the folder writable for WAV export.

This project is a creative/learning tool, not a validated voice-anonymization
system. Source, earlier versions and issues:
https://github.com/Yacos-MA/Anonymix_Scoobs_Real_Time_Vocal_Chords_Anonimization_project
