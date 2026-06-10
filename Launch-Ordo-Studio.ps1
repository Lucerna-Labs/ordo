param(
    [switch]$Check
)

$ErrorActionPreference = "Stop"

$ordoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$studioDir = Join-Path $ordoRoot "ordo-studio"
$runtimeUserFiles = Join-Path $ordoRoot "ordo-runtime\user-files"
$modesPath = Join-Path $runtimeUserFiles "modes"
$controlUrl = "http://127.0.0.1:4141"
$runtimeOut = Join-Path $ordoRoot "runtime-dev.out.log"
$runtimeErr = Join-Path $ordoRoot "runtime-dev.err.log"
$studioOut = Join-Path $studioDir "studio-dev.out.log"
$studioErr = Join-Path $studioDir "studio-dev.err.log"

Write-Host ""
Write-Host "Ordo Studio launcher" -ForegroundColor Cyan
Write-Host "Workspace: $ordoRoot"
Write-Host "Runtime user files: $runtimeUserFiles"
Write-Host "Modes: $modesPath"
Write-Host ""

if (-not (Test-Path -LiteralPath (Join-Path $studioDir "package.json"))) {
    throw "ordo-studio\package.json was not found at $studioDir"
}

if (-not (Test-Path -LiteralPath $modesPath)) {
    New-Item -ItemType Directory -Force -Path $modesPath | Out-Null
    Write-Host "Mode directory did not exist; created $modesPath. Runtime defaults will materialize on first launch." -ForegroundColor Yellow
} elseif (-not (Test-Path -LiteralPath (Join-Path $modesPath "general.json"))) {
    Write-Host "Mode directory exists but general.json is missing. Runtime defaults will materialize missing defaults on launch." -ForegroundColor Yellow
}

foreach ($tool in "npm", "cargo") {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool was not found on PATH"
    }
}

if ($Check) {
    Write-Host "Launcher check passed. No processes were started." -ForegroundColor Green
    exit 0
}

Get-ChildItem -Path (Join-Path $env:APPDATA "Python") -Directory -Filter "Python*" -ErrorAction SilentlyContinue |
    ForEach-Object {
        $scripts = Join-Path $_.FullName "Scripts"
        if (Test-Path -LiteralPath (Join-Path $scripts "tvly.exe")) {
            $env:PATH = "$scripts;$env:PATH"
        }
    }

$tavily = [Environment]::GetEnvironmentVariable("TAVILY_API_KEY", "User")
if (-not [string]::IsNullOrWhiteSpace($tavily)) {
    $env:TAVILY_API_KEY = $tavily
}

$serp = [Environment]::GetEnvironmentVariable("SERPAPI_API_KEY", "User")
if (-not [string]::IsNullOrWhiteSpace($serp)) {
    $env:SERPAPI_API_KEY = $serp
    $env:SERPAPI_KEY = $serp
}

$env:ORDO_USER_FILES_PATH = $runtimeUserFiles
$env:ORDO_MODES_PATH = $modesPath
$env:ORDO_RUNTIME_PROFILE = "standard"
$env:ORDO_CONTROL_URL = $controlUrl
# Run the avatar performance driver so the avatar pop-out window
# (the Bot button next to the voice controls) lip-syncs out of the
# box. One ~30Hz task; set to "0" to disable. See docs/avatar.md.
$env:ORDO_ENABLE_AVATAR = "1"
$env:RUSTFLAGS = "-D warnings"

Push-Location $studioDir
try {
    if (-not (Test-Path -LiteralPath "node_modules")) {
        Write-Host "Installing Ordo Studio frontend dependencies..." -ForegroundColor Yellow
        & npm ci
        if ($LASTEXITCODE -ne 0) {
            throw "npm ci failed"
        }
    }
} finally {
    Pop-Location
}

Write-Host "Clearing stale Ordo dev listeners on ports 4141 and 1420..." -ForegroundColor Yellow
foreach ($port in 4141, 1420) {
    $listeners = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
    foreach ($listener in $listeners) {
        $process = Get-Process -Id $listener.OwningProcess -ErrorAction SilentlyContinue
        Stop-Process -Id $listener.OwningProcess -Force -ErrorAction Stop
        if ($process) {
            Write-Host "Stopped process on port ${port}: $($process.Id) $($process.ProcessName)"
        }
    }
}

Start-Sleep -Milliseconds 800
foreach ($port in 4141, 1420) {
    $listener = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($listener) {
        $process = Get-Process -Id $listener.OwningProcess -ErrorAction SilentlyContinue
        $path = if ($process) { $process.Path } else { "" }
        throw "port $port is still in use by PID $($listener.OwningProcess) $path"
    }
}

Remove-Item -LiteralPath $runtimeOut, $runtimeErr, $studioOut, $studioErr -Force -ErrorAction SilentlyContinue

Write-Host "Starting Ordo runtime from current workspace..." -ForegroundColor Cyan
$runtimeProcess = Start-Process `
    -FilePath "cargo" `
    -ArgumentList @("run", "--", "serve") `
    -WorkingDirectory $ordoRoot `
    -WindowStyle Minimized `
    -RedirectStandardOutput $runtimeOut `
    -RedirectStandardError $runtimeErr `
    -PassThru

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
    Write-Host ""
    Write-Host "ERROR: Ordo runtime did not become healthy." -ForegroundColor Red
    if (Test-Path -LiteralPath $runtimeErr) {
        Write-Host "--- runtime-dev.err.log ---" -ForegroundColor Yellow
        Get-Content -LiteralPath $runtimeErr -Tail 120
    }
    throw "runtime health check failed"
}

Write-Host "Runtime is healthy." -ForegroundColor Green
Write-Host "Starting Ordo Studio..." -ForegroundColor Cyan
Push-Location $studioDir
try {
    & npm run tauri:dev
    if ($LASTEXITCODE -ne 0) {
        if (Test-Path -LiteralPath $studioErr) {
            Write-Host "--- studio-dev.err.log ---" -ForegroundColor Yellow
            Get-Content -LiteralPath $studioErr -Tail 120
        }
        throw "Ordo Studio exited with code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
