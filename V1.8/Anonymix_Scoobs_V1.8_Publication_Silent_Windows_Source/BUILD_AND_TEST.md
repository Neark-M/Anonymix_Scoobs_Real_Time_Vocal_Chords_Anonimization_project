# Anonymix Scoobs V1.8 — publication source

This archive starts from `Anonymix_Scoobs_V1.7.2_FFT_Direct_Pixel_Composition_Windows_Test.zip`. The separately supplied V1.8 development archive remains the starting point for V2. This publication copy removes application log messages and diagnostic-only timing and level collection. The small audio-pressure counter remains because it drives the adaptive visual budget. The UI may still show its normal states such as `No Audio` or `Saving...`.

## Build with Visual Studio Community 2026

1. Extract the complete archive. In the existing x64 project, replace every project `.cpp` and `.h` with the matching file in `src/`. Remove the old `AudioDiagnostics.h` from the project and its source folder, and add `AudioPressure.h`. Add any other headers that the project does not already include. The project requires C++20, SDL3, SDL3_ttf and PortAudio as in the tested development archive.
2. Build **x64 Release**. Keep `runtime/SDL3.dll`, `runtime/SDL3_ttf.dll`, `runtime/portaudio.dll` and `runtime/logo.bmp` beside the resulting executable. Preserve `runtime/LICENSES` and `runtime/THIRD_PARTY_NOTICES.txt` when distributing the application.
3. Test Recording, Stream, Shared and Exclusive modes, Play/Stop, WAV export, device reopen, resizing from minimum to maximum size and the direct-pixel FFT composite. Confirm that the application emits no messages of its own into Visual Studio's Output window. Third-party DLLs or Visual Studio may emit messages independently.

The archive includes source files and runtime assets, but not a prebuilt executable or Visual Studio project file. The source was checked for syntax with local SDL/PortAudio stubs; this environment cannot compile or run the Windows application.
