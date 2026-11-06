@echo off
rem Rebuild + relaunch loop. Leave this window open while developing.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\dev.ps1"
