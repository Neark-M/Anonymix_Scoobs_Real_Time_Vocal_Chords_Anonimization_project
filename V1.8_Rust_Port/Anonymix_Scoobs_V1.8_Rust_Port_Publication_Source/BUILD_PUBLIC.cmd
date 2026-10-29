@echo off
setlocal
cd /d "%~dp0"
where cargo >nul 2>nul
if errorlevel 1 (
    echo Cargo is missing. Use the installed Windows x64 MSVC Rust toolchain.
    exit /b 1
)
cargo test --offline --locked
if errorlevel 1 exit /b 1
cargo build --release --offline --locked --bin anonymix-scoobs
if errorlevel 1 exit /b 1
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0PACKAGE_WINDOWS.ps1"
if errorlevel 1 exit /b 1
echo Public Windows package is ready in dist.
