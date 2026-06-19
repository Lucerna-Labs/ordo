@echo off
:: Desktop shortcut installer for this Ordo workspace.
:: Creates a shortcut that launches the current Servo-backed Ordo shell.

setlocal
set "ORDO_DIR=%~dp0"
if "%ORDO_DIR:~-1%"=="\" set "ORDO_DIR=%ORDO_DIR:~0,-1%"
set "LAUNCHER=%ORDO_DIR%\Launch-Ordo-Servo.vbs"
set "ICON=%ORDO_DIR%\ordo-studio\public\favicon.ico"
set "WSCRIPT=%SystemRoot%\System32\wscript.exe"

if not exist "%LAUNCHER%" (
  echo ERROR: Launch-Ordo-Servo.vbs not found.
  echo Expected: %LAUNCHER%
  echo.
  pause
  exit /b 1
)

if not exist "%WSCRIPT%" (
  echo ERROR: wscript.exe not found.
  echo Expected: %WSCRIPT%
  echo.
  pause
  exit /b 1
)

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ws = New-Object -ComObject WScript.Shell;" ^
  "$desktop = [Environment]::GetFolderPath('Desktop');" ^
  "$lnk = Join-Path $desktop 'Ordo.lnk';" ^
  "$sc = $ws.CreateShortcut($lnk);" ^
  "$sc.TargetPath = $env:WSCRIPT;" ^
  "$q = [char]34;" ^
  "$sc.Arguments = '//B //Nologo ' + $q + $env:LAUNCHER + $q;" ^
  "$sc.WorkingDirectory = $env:ORDO_DIR;" ^
  "$sc.Description = 'Launch Ordo from this workspace';" ^
  "if (Test-Path -LiteralPath $env:ICON) { $sc.IconLocation = $env:ICON } else { $sc.IconLocation = '%%SystemRoot%%\System32\imageres.dll,109' };" ^
  "$sc.Save();" ^
  "Write-Host ('Desktop shortcut created at: ' + $lnk) -ForegroundColor Green;" ^
  "Write-Host ('Target: ' + $env:WSCRIPT + ' ' + $sc.Arguments) -ForegroundColor Cyan"

echo.
echo Done. Press any key to close.
pause >nul
