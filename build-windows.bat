@echo off
REM Build script for OSH Daemon on Windows
REM Run this on Windows to build osh-daemon

echo Building OSH Daemon for Windows x64...

cargo build --release -p osh-daemon

echo.
echo Build completed successfully!
echo.
echo Binary is located at:
echo   - target\release\osh-daemon.exe
