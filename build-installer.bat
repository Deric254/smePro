@echo off
REM Double-click this file to build a real, installable .msi/.exe for
REM Windows. The finished installer will be in:
REM   src-tauri\target\release\bundle\
REM
REM First build takes a while (compiling Rust from scratch). Later
REM builds are much faster.

echo Building Ledger ^& Counter installer...
echo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\setup.ps1" -Build
if %ERRORLEVEL% NEQ 0 (
    echo.
    echo Build failed. Scroll up to see the actual error.
    pause
) else (
    echo.
    echo Done. Look in src-tauri\target\release\bundle\ for the installer.
    pause
)
