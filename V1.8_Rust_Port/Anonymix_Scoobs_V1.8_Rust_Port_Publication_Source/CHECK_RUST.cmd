@echo off
setlocal
cd /d "%~dp0"
cargo test --offline --locked
if errorlevel 1 exit /b 1
cargo build --release --offline --locked
if errorlevel 1 exit /b 1
