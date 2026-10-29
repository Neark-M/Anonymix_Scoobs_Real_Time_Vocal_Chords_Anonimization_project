@echo off
setlocal
cd /d "%~dp0"
if not exist "target\release\anonymix-scoobs.exe" (
    echo Build first with: cargo build --release --offline
    exit /b 1
)
"target\release\anonymix-scoobs.exe"
