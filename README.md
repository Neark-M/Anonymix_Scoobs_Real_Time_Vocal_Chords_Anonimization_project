# Anonymix Scoobs

Anonymix Scoobs is an experimental Windows application for live voice effects, recording, playback, and audio visualization. It began as a capstone project for exploring audio processing and has continued as a personal software project. Its audio features run locally and do not require an internet connection.

**Available versions: V1.8 Rust port and the original C++ V1.8.** The application is a creative tool and a learning project. It has not been validated as a voice anonymization system. Do not rely on it to conceal your identity in a sensitive situation.

## Download and run

Download the Windows x64 **Rust application ZIP** from the [V1.8 Rust port release](https://github.com/Yacos-MA/Anonymix_Scoobs_Real_Time_Vocal_Chords_Anonimization_project/releases/tag/v1.8-rust-port). It includes the EXE and runtime files; users do not need Rust, Cargo or Visual Studio. The system Microsoft Visual C++ x64 runtime is required by the supplied PortAudio DLL.

The original C++ Windows application remains available from the [V1.8 release](https://github.com/Yacos-MA/Anonymix_Scoobs_Real_Time_Vocal_Chords_Anonimization_project/releases/tag/v1.8). Extract the entire application ZIP into a writable folder before running the executable. Keep the supplied DLLs, icon, and third-party notices with the application.

Earlier versions remain available on the [Releases page](https://github.com/Yacos-MA/Anonymix_Scoobs_Real_Time_Vocal_Chords_Anonimization_project/releases). The source files in this repository are for development; they are not themselves a ready-to-run Windows application.

## Features

- **Live voice processing:** Stream mode captures microphone audio, applies the selected effects, and sends it to the active audio output.
- **Recording and playback:** Recording mode captures a take. One Play button plays it with the currently selected effects; one Save WAV button exports it. Effects can be combined. During playback, sliders and selections can change; Save WAV captures their values when pressed.
- **Voice effects:** Pitch, the transformation labelled Formant, Timbre, Prosody, and Jitter each have a selection button and a slider.
- **Audio devices:** PortAudio uses Windows WASAPI. The interface offers Shared and Exclusive modes, with a fallback if the requested mode cannot be opened. Reopen Audio requests device discovery after a microphone or output change. Exclusive mode may prevent other applications from using the same audio device.
- **Visuals:** SDL3 displays a waveform, an FFT spectrum covering approximately 20 Hz to 20 kHz, and animated interface elements. The window can be resized within its minimum dimensions. The interface targets 60 frames per second and can reduce visual work in steps when under sustained load; actual performance depends on the computer and drivers.

Capture, DSP, playback, and WAV export use **48 kHz** inside the application. There is no internal 48-to-16-to-48 kHz conversion path. Windows Shared mode or device drivers may still resample outside the application. A particular microphone-to-speaker latency is not guaranteed on every computer.

### What the voice effects mean

**Pitch** changes the perceived pitch. **Timbre** changes the spectral balance. The effect labelled **Formant** uses the same variable-delay processor in Stream mode, recorded playback, and WAV export. It changes the read rate of stored audio, including harmonics, and may change perceived pitch. It does **not** independently shift the vocal-tract spectral envelope while preserving the fundamental pitch. **Prosody** adds a speech-onset pitch and loudness gesture; it does not understand linguistic intonation. **Jitter** adds creative pitch instability; it does not measure vocal-fold jitter.

These effects change how a voice sounds, but their effectiveness against speaker identification has not been established.

## V1.8 Rust port

The desktop application code is ported to Rust. SDL3/SDL3_ttf and PortAudio remain
native runtime libraries. The same five effects, Recording/Stream modes,
48-kHz signal path, WASAPI fallbacks, resizing, animation and shared adaptive
visual scheduling are preserved. This is not a new voice-processing algorithm.

The public executable has no application console, startup/failure reports or
diagnostic log. Internal error containment and the existing interface states
remain. The continuous Exclusive-mode capture hiss observed on some devices is
inherited from the C++ baseline. Shared mode remains available.

The [Rust publication sources](V1.8_Rust_Port/Anonymix_Scoobs_V1.8_Rust_Port_Publication_Source/)
include an English [code map](V1.8_Rust_Port/Anonymix_Scoobs_V1.8_Rust_Port_Publication_Source/CODE_MAP.md),
API comments, regression fixtures and `BUILD_PUBLIC.cmd`. Maintainers can build
with the Windows x64 MSVC Rust toolchain (Rust 1.85 or newer) and Visual Studio
C++ build tools. Run `BUILD_PUBLIC.cmd` to test, compile and produce the portable
application ZIP. Cargo has no registry dependencies in this source set.

Ordinary users should download the ready-to-run application ZIP from Releases.
The separate development snapshot is retained outside the public release.

## Build the C++ version from source

The Windows source set targets **Visual Studio Community 2026**, **x64**, and **C++20**. The application uses **SDL3** for the interface, **SDL3_ttf** for text, and **PortAudio** for audio. The official V1.8 build does not require JUCE.

The [V1.8 publication sources](V1.8/Anonymix_Scoobs_V1.8_Publication_Silent_Windows_Source/) contain the application's `.cpp` and `.h` files, a [code map](V1.8/Anonymix_Scoobs_V1.8_Publication_Silent_Windows_Source/CODE_MAP.md), and [build and test instructions](V1.8/Anonymix_Scoobs_V1.8_Publication_Silent_Windows_Source/BUILD_AND_TEST.md). They do not contain a Visual Studio solution or a prebuilt executable. To compile them, use a Visual Studio project with suitable dependency headers, import libraries, and linker settings.

The supplied runtime package includes SDL3 3.4.16, SDL3_ttf 3.2.2, and the PortAudio DLL carried forward from the earlier Windows packages. Keep `runtime/THIRD_PARTY_NOTICES.txt` and `runtime/LICENSES/` with distributed third-party DLLs. The published V1.8 source variant omits application diagnostic output. A separate development baseline retains diagnostics for future work.

The [V1.7 sources](V1.7/Anonymix_Scoobs_V1.7_Publication_Clean/) and [V1.5 build notes](V1.5/Anonymix_Scoobs_V1.5_Dev/BUILD_AND_TEST.md) document earlier versions.

## Project history and plans

| Version | Main changes |
| --- | --- |
| V1 | Initial capstone exploration of voice processing and visualization. |
| V1.1 | Source organization, English documentation, and a Windows package. |
| V1.2 | 48 kHz application processing and experiments with JUCE and PortAudio. The official application continued with PortAudio for audio and SDL3 for the interface. |
| V1.5 | Unified playback and WAV export, selectable and combinable effects, prosody and jitter, updated visuals, and the same formant-labelled processing path for Stream and Recording modes. |
| V1.7 | WASAPI Shared/Exclusive selection, capture-level correction, recording collection outside the audio callback, audio-chain refinements, asynchronous device reopening, and interface scheduling improvements. |
| V1.8 | Additional asynchronous and preloaded work, adaptive visual scheduling, resizable window, and refined FFT and rotated-shape rendering. The publication build omits application diagnostics. |
| V1.8 Rust port | Rust desktop port of V1.8 using the existing native SDL3/SDL3_ttf and PortAudio libraries. Original behavior, English source documentation, a separate development snapshot, and a silent public executable. |

The Rust port is named **V1.8 Rust port**, replacing its earlier development name V2.0. It preserves the existing application behavior; it does not promise lower latency, better anonymity or new effects. Future development may continue separately from either baseline. No next-version scope or release date has been set.

The project name may change as the application evolves. A voice that sounds different is not necessarily anonymous.
