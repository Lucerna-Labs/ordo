@echo off
:: Desktop shortcut installer for this Ordo workspace.
:: Creates a shortcut that launches the current Servo-backed Ordo shell.

setlocal
set "ORDO_DIR=%~dp0"
if "%ORDO_DIR:~-1%"=="\" set "ORDO_DIR=%ORDO_DIR:~0,-1%"
set "LAUNCHER=%ORDO_DIR%\Launch-Ordo-Servo.cmd"
set "ICON=%ORDO_DIR%\ordo-studio\public\favicon.ico"

if not exist "%LAUNCHER%" (
  echo ERROR: Launch-Ordo-Servo.cmd not found.
  echo Expected: %LAUNCHER%
  echo.
  pause
  exit /b 1
)

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ws = New-Object -ComObject WScript.Shell;" ^
  "$lnk = Join-Path $env:USERPROFILE 'Desktop\Ordo Studio.lnk';" ^
  "$sc = $ws.CreateShortcut($lnk);" ^
  "$sc.TargetPath = '%LAUNCHER%';" ^
  "$sc.WorkingDirectory = '%ORDO_DIR%';" ^
  "$sc.Description = 'Launch Ordo from this workspace';" ^
  "if (Test-Path -LiteralPath '%ICON%') { $sc.IconLocation = '%ICON%' };" ^
  "$sc.Save();" ^
  "Write-Host ('Desktop shortcut created at: ' + $lnk) -ForegroundColor Green;" ^
  "Write-Host ('Target: %LAUNCHER%') -ForegroundColor Cyan"

echo.
echo Done. Press any key to close.
pause >nul
