@echo off
REM Double-click this file to install everything needed (first run only)
REM and launch SME Pro in dev mode.
REM
REM This just calls scripts\setup.ps1 -Dev via PowerShell — it exists as
REM a .bat because Windows won't run a .ps1 by double-clicking it
REM (execution-policy restrictions), but .bat files always run.

echo Starting Ledger ^& Counter (dev mode)...
echo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\setup.ps1" -Dev
if %ERRORLEVEL% NEQ 0 (
    echo.
    echo Something went wrong above. Scroll up to see the actual error.
    pause
)
