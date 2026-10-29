# Documentation and publication audit — V1.8 Rust port

## Scope

The active Rust source, module headers, named public type/method documentation,
callback ownership comments, API units and maintainer guides were reviewed in
English. 304 named public types/functions have explicit API documentation
in this variant. Public fields/variants are explained by their surrounding
type/module documentation; this is not a claim that every code line needs its
own comment. CODE_MAP.md supplies the beginner-oriented cross-module explanation.
Historical C++ sources and reports in the development archive remain frozen;
they are not compiled or linked into this Rust application.

Development retains all 35 original implementation modules' executable tokens,
except formatting/trailing commas; documentation and the desktop product name
are updated. The public copy removes only report/probe/capture instrumentation
and associated unused calculations/state. Original input energy required by
channel selection/Exclusive gain remains. Original DSP effects, recorded/live
block sizes, ring/take policy, UI geometry and native runtime binaries remain.

## Verification

- Linux Debug and Release: 62 passing checks in each profile.
- Adapted Windows-body type check and 14 simulated native/owner tests: passed.
- Offline library/API documentation build with broken-link errors enabled: passed.
- No compiler/documentation warnings in these final verification runs.
- Windows count expected when rebuilt locally: 67 checks.
- DLL/asset/notice bytes match the last tested stage-9 package.

The public source contains no print/eprint/dbg calls, no diagnostics collector and only the desktop executable target. Its Windows GUI subsystem and silent panic hook are present. The one runtime pressure counter and necessary scheduling timings remain; they control the original visual adaptation.

The user supplied passing Windows stage-9 build/test output and reported some
manual desktop checks OK. These new archives were verified on Linux, including
adapted native bodies; no Windows executable or hardware test was run here.
PACKAGE_WINDOWS.ps1 is source-reviewed, not executed on Windows here. It checks
the PE x64 GUI-subsystem header before creating the runtime-only application ZIP.
Use BUILD_PUBLIC.cmd on Windows and launch the extracted resulting package
before distributing that binary. A source ZIP is not a prebuilt application.

No private V1.8.2 change, new DSP effect, latency improvement or elimination of
inherited Exclusive capture hiss is claimed. Reports and exact validation
hashes are summarized in validation/V18_RUST_AUDIT.json. The development archive
retains full verification logs; the public source retains regression fixtures
and this summary, not historical log files.
