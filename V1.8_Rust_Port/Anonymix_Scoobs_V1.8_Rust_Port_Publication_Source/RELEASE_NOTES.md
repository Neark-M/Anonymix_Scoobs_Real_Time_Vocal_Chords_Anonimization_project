# Anonymix Scoobs V1.8 Rust port

This release provides a Rust port of the existing C++ V1.8 application for
Windows x64. The C++ V1.8 release remains available separately.

- Same 48000-Hz capture, DSP, recorded playback and mono PCM16 WAV export.
- Pitch, Formant-labelled delay, Timbre, Prosody and Jitter controls.
- PortAudio/WASAPI Shared and Exclusive modes with the original fallbacks.
- SDL3 interface, microphone FFT/waveform, animation and resizable layout.
- Original shared adaptive visual cadence and raster policy.
- Silent public executable: no application console or diagnostic reports.
- English Rust source documentation, ownership notes and reference regression tests.

This is a port, not a new audio-quality, anonymity or latency optimization.
The known Exclusive capture background hiss on some devices is inherited.
Private V1.8.2 work is not included. The project remains experimental and
has not been validated as a voice-anonymization system.

Download `Anonymix_Scoobs_V1.8_Rust_Port_Windows_x64.zip`, extract all files and
run `anonymix-scoobs.exe`. The source ZIP is for developers only.
