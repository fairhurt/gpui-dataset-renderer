@echo off
setlocal
cd /d "%~dp0"
if not defined GPUI_FXC_PATH (
  for /f "usebackq delims=" %%F in (`powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\find-fxc.ps1"`) do set "GPUI_FXC_PATH=%%F"
)
if not defined GPUI_FXC_PATH (
  echo fxc.exe not found. Install the "Windows 11 SDK" component from the Visual Studio Installer.
  pause
  exit /b 1
)
cargo run --release -- %*
if errorlevel 1 pause
