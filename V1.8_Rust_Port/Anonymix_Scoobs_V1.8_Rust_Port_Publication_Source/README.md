# Anonymix Scoobs — V1.8 Rust port (publication source)

Windows x64 public source for the faithful C++ V1.8 Rust port. This is the
silent application variant. It contains no diagnostics collector, console
probe executable, synthetic rendering probe or frozen C++ source tree.
The separate development archive preserves those materials for future work.

## Maintainer build and portable package

Keep the complete `runtime` folder. Use the existing Windows x64 MSVC Rust and
Visual Studio build tools, then run from this directory:

```cmd
BUILD_PUBLIC.cmd
```

The script runs offline tests, builds the Release desktop only, checks the
Windows x64 GUI-subsystem PE header, and creates:

`dist\Anonymix_Scoobs_V1.8_Rust_Port_Windows_x64.zip`

It includes the executable, original runtime DLLs, icon, supplied notices,
user guide and recordings folder. It contains no Rust tools, source, test
fixtures, development probes or debug symbols. No Cargo registry dependencies
are required. A normal user only extracts this application ZIP and runs the EXE.
The supplied PortAudio DLL requires the system Microsoft Visual C++ x64 runtime.

There is no precompiled Windows EXE in this source archive: the final binary is
produced on your Windows computer. Build/package helper output is intentional
maintainer output; the packaged application emits no startup/error/report text,
opens no console, suppresses the default Rust panic-hook text and writes no log.
Error containment, worker joins, UI state/failure captions and runtime visual
pressure remain. The original third-party DLL binaries are unchanged.

Expected Windows checks: 51 integration, 14 unit and 2 compile-fail documentation
tests (67 total). Fewer than the development archive because probe/parser and
report-only API tests were removed. Audio-pressure behavior is checked against
the same original C++ fixture independently of the removed report collector.

Before distributing the new binary, extract its application ZIP and launch
that copy. It must find its neighboring DLLs and save WAV files in its own
`recordings` folder. A successful Cargo test alone cannot test physical devices.

## Documentation and scope

- [CODE_MAP.md](CODE_MAP.md): English beginner-oriented source/ownership guide.
- [README_USER.md](README_USER.md): guide included in the executable package.
- [RELEASE_NOTES.md](RELEASE_NOTES.md): public GitHub release description.
- [PUBLICATION_AUDIT.md](PUBLICATION_AUDIT.md): scope of the source audit.
- `SAFETY_*.md`: native, queue and GUI ownership boundaries.
- `cargo doc --offline --no-deps`: English API documentation.

This is V1.8 Rust port, not V2.0. There is no private V1.8.2 work, new effect,
resampling stage or promised performance improvement. The same original DSP,
device policy and approved visual design remain. Keep runtime notices/licenses
with the DLLs. This is not a validated voice-anonymization system.
