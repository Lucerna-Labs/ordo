@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Launch-Ordo-Servo.ps1" %*
if errorlevel 1 pause
