@echo off
:: Desktop shortcut installer for this Ordo Studio workspace.
:: Creates a shortcut that launches Launch-Ordo-Studio.cmd, which starts
:: the Tauri UXI from ordo-studio.

setlocal
set "ORDO_DIR=%~dp0"
if "%ORDO_DIR:~-1%"=="\" set "ORDO_DIR=%ORDO_DIR:~0,-1%"
set "LAUNCHER=%ORDO_DIR%\Launch-Ordo-Studio.cmd"
set "ICON=%ORDO_DIR%\ordo-studio\src-tauri\icons\icon.ico"

if not exist "%LAUNCHER%" (
  echo ERROR: Launch-Ordo-Studio.cmd not found.
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
  "$sc.Description = 'Launch Ordo Studio from this workspace';" ^
  "if (Test-Path -LiteralPath '%ICON%') { $sc.IconLocation = '%ICON%' };" ^
  "$sc.Save();" ^
  "Write-Host ('Desktop shortcut created at: ' + $lnk) -ForegroundColor Green;" ^
  "Write-Host ('Target: %LAUNCHER%') -ForegroundColor Cyan"

echo.
echo Done. Press any key to close.
pause >nul
