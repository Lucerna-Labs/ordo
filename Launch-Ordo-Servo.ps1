param(
    [switch]$Check,
    [switch]$SkipBuild,
    [switch]$SkipServoNetworkLock,
    [int]$Width = 1560,
    [int]$Height = 980
)

$ErrorActionPreference = "Stop"

$ordoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$studioDir = Join-Path $ordoRoot "ordo-studio"
$runtimeUserFiles = Join-Path $ordoRoot "ordo-runtime\user-files"
$modesPath = Join-Path $runtimeUserFiles "modes"
$controlUrl = "http://127.0.0.1:4141"
$studioUrl = "$controlUrl/"
$runtimeOut = Join-Path $ordoRoot "runtime-servo.out.log"
$runtimeErr = Join-Path $ordoRoot "runtime-servo.err.log"
$servoOut = Join-Path $ordoRoot "servo-shell.out.log"
$servoErr = Join-Path $ordoRoot "servo-shell.err.log"
$servoShellDir = Join-Path $ordoRoot "ordo-servo-shell"
$portableBinDir = Join-Path $ordoRoot "bin\portable"
$portableRuntimeExe = Join-Path $portableBinDir "ordo.exe"
$portableServoShellExe = Join-Path $portableBinDir "ordo-servo-shell.exe"
$builtServoShellExe = Join-Path $servoShellDir "target\debug\ordo-servo-shell.exe"
$servoShellExe = $builtServoShellExe
$servoShellTargetDir = Split-Path -Parent $servoShellExe
$servoDir = Join-Path $ordoRoot "bin\servo-nightly\servo"
$servoZip = Join-Path $ordoRoot "bin\servo-nightly\servo-x86_64-windows-msvc.zip"
$servoFirewallRuleName = "Ordo Servo Embedded Renderer - Block Internet"

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Enable-ServoLocalRendererNetworkGuard {
    param(
        [string]$ServoShellPath
    )

    if ($SkipServoNetworkLock) {
        Write-Host "Servo network lock skipped by operator." -ForegroundColor Yellow
        return
    }

    if (-not (Get-Command New-NetFirewallRule -ErrorAction SilentlyContinue)) {
        Write-Warning "Windows Firewall cmdlets are unavailable; Servo will still inherit the local-only proxy guard."
        return
    }

    if (-not (Test-IsAdministrator)) {
        Write-Warning "Run this launcher as Administrator to install the Servo local-only firewall rule. Servo will still inherit the local-only proxy guard."
        return
    }

    try {
        Get-NetFirewallRule -DisplayName $servoFirewallRuleName -ErrorAction SilentlyContinue |
            Remove-NetFirewallRule -ErrorAction SilentlyContinue

        New-NetFirewallRule `
            -DisplayName $servoFirewallRuleName `
            -Direction Outbound `
            -Program $ServoShellPath `
            -Action Block `
            -RemoteAddress Internet `
            -Profile Any `
            -Description "Keeps Ordo's Servo renderer on localhost while allowing the Ordo runtime to own all external communication." |
            Out-Null

        Write-Host "Servo firewall guard installed: blocks ordo-servo-shell.exe outbound Internet, leaves localhost available." -ForegroundColor Green
    } catch {
        Write-Warning "Could not install Servo firewall guard: $($_.Exception.Message)"
    }
}

function Ensure-ServoAngleDlls {
    $requiredDlls = @("libEGL.dll", "libGLESv2.dll")
    $missing = @()
    foreach ($dll in $requiredDlls) {
        if (-not (Test-Path -LiteralPath (Join-Path $servoShellTargetDir $dll))) {
            $missing += $dll
        }
    }
    if ($missing.Count -eq 0) {
        return
    }

    $servoNightlyDir = Join-Path $ordoRoot "bin\servo-nightly"
    $nightlyServoExe = Join-Path $servoDir "servoshell.exe"
    if (-not (Test-Path -LiteralPath $nightlyServoExe)) {
        New-Item -ItemType Directory -Force -Path $servoNightlyDir | Out-Null
        Write-Host "Downloading Servo nightly for ANGLE runtime DLLs..." -ForegroundColor Cyan
        Invoke-WebRequest `
            -Uri "https://download.servo.org/nightly/windows-msvc/servo-x86_64-windows-msvc.zip" `
            -OutFile $servoZip
        Write-Host "Extracting Servo nightly ANGLE runtime..." -ForegroundColor Cyan
        Expand-Archive -LiteralPath $servoZip -DestinationPath $servoNightlyDir -Force
    }

    New-Item -ItemType Directory -Force -Path $servoShellTargetDir | Out-Null
    foreach ($dll in $requiredDlls) {
        $source = Join-Path $servoDir $dll
        if (-not (Test-Path -LiteralPath $source)) {
            throw "Servo ANGLE DLL not found: $source"
        }
        Copy-Item -LiteralPath $source -Destination (Join-Path $servoShellTargetDir $dll) -Force
    }
}

Write-Host ""
Write-Host "Ordo Servo launcher" -ForegroundColor Cyan
Write-Host "Workspace: $ordoRoot"
Write-Host "Runtime user files: $runtimeUserFiles"
Write-Host "Modes: $modesPath"
Write-Host ""

if ($Check) {
    Write-Host "Launcher check passed. No processes were started." -ForegroundColor Green
    exit 0
}

if (-not (Test-Path -LiteralPath $modesPath)) {
    New-Item -ItemType Directory -Force -Path $modesPath | Out-Null
}

$env:ORDO_USER_FILES_PATH = $runtimeUserFiles
$env:ORDO_MODES_PATH = $modesPath
$env:ORDO_RUNTIME_PROFILE = "standard"
$env:ORDO_CONTROL_URL = $controlUrl
$env:ORDO_ENABLE_AVATAR = "1"

$studioIndex = Join-Path $studioDir "dist\index.html"
$studioNodeModules = Join-Path $studioDir "node_modules"
$hasPortableRuntime = Test-Path -LiteralPath $portableRuntimeExe
$hasPortableServoShell = Test-Path -LiteralPath $portableServoShellExe
$needsStudioBuild = -not $SkipBuild -and -not (Test-Path -LiteralPath $studioIndex)
$needsCargo = -not $hasPortableRuntime -or -not $hasPortableServoShell

if ($needsStudioBuild -and -not (Get-Command "npm" -ErrorAction SilentlyContinue)) {
    throw "npm was not found on PATH and the Studio bundle is missing. Build or copy ordo-studio/dist first."
}

if ($needsCargo -and -not (Get-Command "cargo" -ErrorAction SilentlyContinue)) {
    throw "cargo was not found on PATH and no portable Ordo runtime/Servo shell binaries were found in bin\portable."
}

if (-not $SkipBuild) {
    if ((Test-Path -LiteralPath $studioNodeModules) -or $needsStudioBuild) {
        Push-Location $studioDir
        try {
            if (-not (Test-Path -LiteralPath "node_modules")) {
                Write-Host "Installing Ordo Studio frontend dependencies..." -ForegroundColor Yellow
                & npm ci
                if ($LASTEXITCODE -ne 0) {
                    throw "npm ci failed"
                }
            }

            Write-Host "Building Ordo Studio bundle for Servo..." -ForegroundColor Cyan
            & npm run build
            if ($LASTEXITCODE -ne 0) {
                throw "npm run build failed"
            }
        } finally {
            Pop-Location
        }
    } else {
        Write-Host "Using existing Ordo Studio bundle at $studioIndex." -ForegroundColor Green
    }
}

if ($hasPortableServoShell) {
    $servoShellExe = $portableServoShellExe
    $servoShellTargetDir = Split-Path -Parent $servoShellExe
    Write-Host "Using portable embedded Servo shell: $servoShellExe" -ForegroundColor Green
} else {
    $servoShellExe = $builtServoShellExe
    $servoShellTargetDir = Split-Path -Parent $servoShellExe
    Write-Host "Building embedded Ordo Servo shell..." -ForegroundColor Cyan
    & cargo build --manifest-path (Join-Path $servoShellDir "Cargo.toml") --features servo-engine
    if ($LASTEXITCODE -ne 0) {
        throw "ordo-servo-shell build failed"
    }
    if (-not (Test-Path -LiteralPath $servoShellExe)) {
        throw "Embedded Servo shell did not build to $servoShellExe"
    }
}

Ensure-ServoAngleDlls
Enable-ServoLocalRendererNetworkGuard -ServoShellPath $servoShellExe

Write-Host "Clearing stale Ordo listeners on ports 4141, 1420, and 4150..." -ForegroundColor Yellow
foreach ($port in 4141, 1420, 4150) {
    $listeners = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
    foreach ($listener in $listeners) {
        $process = Get-Process -Id $listener.OwningProcess -ErrorAction SilentlyContinue
        Stop-Process -Id $listener.OwningProcess -Force -ErrorAction Stop
        if ($process) {
            Write-Host "Stopped process on port ${port}: $($process.Id) $($process.ProcessName)"
        }
    }
}

Get-Process servoshell, ordo-servo-shell -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

Remove-Item -LiteralPath $runtimeOut, $runtimeErr, $servoOut, $servoErr -Force -ErrorAction SilentlyContinue

Write-Host "Starting Ordo runtime from current workspace..." -ForegroundColor Cyan
if ($hasPortableRuntime) {
    Write-Host "Using portable Ordo runtime: $portableRuntimeExe" -ForegroundColor Green
    $runtimeProcess = Start-Process `
        -FilePath $portableRuntimeExe `
        -ArgumentList @("serve") `
        -WorkingDirectory $ordoRoot `
        -WindowStyle Minimized `
        -RedirectStandardOutput $runtimeOut `
        -RedirectStandardError $runtimeErr `
        -PassThru
} else {
    $runtimeProcess = Start-Process `
        -FilePath "cargo" `
        -ArgumentList @("run", "--", "serve") `
        -WorkingDirectory $ordoRoot `
        -WindowStyle Minimized `
        -RedirectStandardOutput $runtimeOut `
        -RedirectStandardError $runtimeErr `
        -PassThru
}

Write-Host "Runtime PID: $($runtimeProcess.Id)"
Write-Host "Waiting for runtime health at $controlUrl/health..."
$deadline = (Get-Date).AddSeconds(120)
do {
    try {
        Invoke-WebRequest -UseBasicParsing "$controlUrl/health" -TimeoutSec 2 | Out-Null
        $healthy = $true
        break
    } catch {
        Start-Sleep -Seconds 2
    }
} while ((Get-Date) -lt $deadline)

if (-not $healthy) {
    if (Test-Path -LiteralPath $runtimeErr) {
        Write-Host "--- runtime-servo.err.log ---" -ForegroundColor Yellow
        Get-Content -LiteralPath $runtimeErr -Tail 120
    }
    throw "runtime health check failed"
}

Write-Host "Runtime is healthy." -ForegroundColor Green

Write-Host "Checking Ordo-served Studio bundle at $studioUrl..." -ForegroundColor Cyan
$studioResponse = Invoke-WebRequest -UseBasicParsing $studioUrl -TimeoutSec 10
if ($studioResponse.StatusCode -ne 200 -or $studioResponse.Content -notmatch "<title>Ordo</title>") {
    throw "Ordo runtime did not serve the Studio bundle at $studioUrl"
}

Write-Host "Opening Ordo Studio through embedded Servo shell..." -ForegroundColor Cyan
$servoArgs = @(
    "--target",
    $studioUrl,
    "--width",
    "$Width",
    "--height",
    "$Height"
)

$servoProxyEnvNames = @(
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "NO_PROXY",
    "no_proxy"
)
$previousServoProxyEnv = @{}
foreach ($name in $servoProxyEnvNames) {
    $previousServoProxyEnv[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

try {
    $env:HTTP_PROXY = "http://127.0.0.1:9"
    $env:HTTPS_PROXY = "http://127.0.0.1:9"
    $env:ALL_PROXY = "socks5://127.0.0.1:9"
    $env:http_proxy = $env:HTTP_PROXY
    $env:https_proxy = $env:HTTPS_PROXY
    $env:all_proxy = $env:ALL_PROXY
    $env:NO_PROXY = "127.0.0.1,localhost,::1,[::1]"
    $env:no_proxy = $env:NO_PROXY

    $servoProcess = Start-Process `
        -FilePath $servoShellExe `
        -ArgumentList $servoArgs `
        -WorkingDirectory $ordoRoot `
        -RedirectStandardOutput $servoOut `
        -RedirectStandardError $servoErr `
        -PassThru
} finally {
    foreach ($name in $servoProxyEnvNames) {
        $previous = $previousServoProxyEnv[$name]
        if ($null -eq $previous) {
            Remove-Item -Path "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item -Path "Env:$name" -Value $previous
        }
    }
}

Write-Host "Embedded Servo shell PID: $($servoProcess.Id)" -ForegroundColor Green
Write-Host "Logs: $servoOut / $servoErr"
