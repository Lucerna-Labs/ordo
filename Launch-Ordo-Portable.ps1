param(
    [switch]$Check
)

$ErrorActionPreference = "Stop"

$ordoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$controlUrl = "http://127.0.0.1:4141"
$runtimeUserFiles = Join-Path $ordoRoot "ordo-runtime\user-files"
$modesPath = Join-Path $runtimeUserFiles "modes"
$runtimeOut = Join-Path $ordoRoot "runtime-portable.out.log"
$runtimeErr = Join-Path $ordoRoot "runtime-portable.err.log"
$appExe = Join-Path $ordoRoot "bin\windows\Ordo.exe"
$runtimeExe = Join-Path $ordoRoot "target\debug\ordo.exe"
$dumpFolder = Join-Path $ordoRoot "crash-dumps"

Write-Host ""
Write-Host "Ordo portable launcher" -ForegroundColor Cyan
Write-Host "Workspace: $ordoRoot"
Write-Host "Runtime user files: $runtimeUserFiles"
Write-Host ""

if (-not (Test-Path -LiteralPath $appExe)) {
    throw "Portable app binary was not found at $appExe"
}

if (-not (Test-Path -LiteralPath (Join-Path $ordoRoot "Cargo.toml"))) {
    throw "Cargo.toml was not found at $ordoRoot"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found on PATH. Install Rust or use the full installer package."
}

if ($Check) {
    Write-Host "Portable launcher check passed. No processes were started." -ForegroundColor Green
    exit 0
}

New-Item -ItemType Directory -Force -Path $runtimeUserFiles, $modesPath | Out-Null

$env:ORDO_USER_FILES_PATH = $runtimeUserFiles
$env:ORDO_MODES_PATH = $modesPath
$env:ORDO_RUNTIME_PROFILE = "standard"
$env:ORDO_CONTROL_URL = $controlUrl
# Avatar performance driver — serves /sse/avatar + /avatar.html so a
# browser can open http://127.0.0.1:4141/avatar.html as the pop-out.
# Set to "0" to disable. See docs/avatar.md.
$env:ORDO_ENABLE_AVATAR = "1"
$env:RUSTFLAGS = "-D warnings"

# Code-execution capability (code.* / workspace.*). The native runner is
# compiled in via the `native-exec` feature (in the build step below) and
# ARMED at runtime by ORDO_CODE_ALLOW_NATIVE. It runs real cargo/python/
# node/shell commands confined to the workspace dir, with network allowed.
# To DISABLE native execution while keeping the WASM runner + the workspace
# read/write tools, set ORDO_CODE_ALLOW_NATIVE to "false".
$env:ORDO_CODE_ALLOW_NATIVE = "true"
$env:ORDO_CODE_WORKSPACE_PATH = Join-Path $runtimeUserFiles "workspace"

# Real embeddings via the local Ollama server (replaces the weak 'hashing'
# fallback) so memory/RAG recall actually matches on meaning. Uses the
# nomic-embed-text model (768-dim) you already have pulled. To revert to the
# hashing fallback, clear ORDO_EMBEDDING_OLLAMA_MODEL.
$env:ORDO_EMBEDDING_OLLAMA_MODEL = "nomic-embed-text"
$env:ORDO_EMBEDDING_DIMENSIONS = "768"

foreach ($port in 4141) {
    $listeners = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue
    foreach ($listener in $listeners) {
        $process = Get-Process -Id $listener.OwningProcess -ErrorAction SilentlyContinue
        $path = if ($process) { $process.Path } else { "" }
        if ($path -like "$ordoRoot*") {
            Stop-Process -Id $listener.OwningProcess -Force -ErrorAction Stop
            Write-Host "Stopped stale Ordo runtime on port ${port}: PID $($listener.OwningProcess)"
        }
    }
}

Remove-Item -LiteralPath $runtimeOut, $runtimeErr -Force -ErrorAction SilentlyContinue

# Configure Windows Error Reporting local crash dumps for ordo.exe so any
# FUTURE termination is provable. A genuine native fault (e.g. access
# violation 0xC0000005, abort 0xC0000409) leaves a full dump here; an external
# kill that supplies exit -1 (console-close, taskkill, antivirus) leaves NO
# dump. That difference is exactly how the 2026-06-07 incident was pinned to
# an external kill rather than a native crash -- see
# docs/incidents/2026-06-07-runtime-exit-minus1-runaway-credentials.md.
try {
    $werKey = "HKCU:\Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\ordo.exe"
    New-Item -ItemType Directory -Force -Path $dumpFolder | Out-Null
    New-Item -Path $werKey -Force | Out-Null
    New-ItemProperty -Path $werKey -Name "DumpFolder" -PropertyType ExpandString -Value $dumpFolder -Force | Out-Null
    New-ItemProperty -Path $werKey -Name "DumpType"   -PropertyType DWord       -Value 2          -Force | Out-Null
    New-ItemProperty -Path $werKey -Name "DumpCount"  -PropertyType DWord       -Value 10         -Force | Out-Null
    Write-Host "WER crash dumps enabled for ordo.exe -> $dumpFolder"
} catch {
    Write-Host "WARN: could not configure WER crash dumps: $($_.Exception.Message)" -ForegroundColor Yellow
}

# Build the runtime as a separate, foreground step, THEN launch the built
# binary detached. The runtime used to be launched as `cargo run -- serve`
# inside a *minimized* console window; closing that window delivered
# CTRL_CLOSE and hard-killed the runtime with exit 0xFFFFFFFF (the 2026-06-07
# incident). Launching the built exe in its OWN HIDDEN console means there is
# no visible/closeable window tied to the runtime, and closing this launcher's
# shell cannot orphan-kill it.
Write-Host "Building Ordo runtime (the first build can take a while)..." -ForegroundColor Cyan
# `sandbox-wasm` lights up the in-process WASM runner (code.run); `native-exec`
# compiles in the native subprocess runner (code.run_native). Both off in the
# crate defaults; enabled here for this portable build.
& cargo build --bin ordo --features sandbox-wasm,native-exec
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed (exit $LASTEXITCODE). Fix the build and re-run."
}
if (-not (Test-Path -LiteralPath $runtimeExe)) {
    throw "Runtime binary not found at $runtimeExe after a successful build."
}

Write-Host "Starting Ordo runtime (hidden, detached)..." -ForegroundColor Cyan
$runtimeProcess = Start-Process `
    -FilePath $runtimeExe `
    -ArgumentList @("serve") `
    -WorkingDirectory $ordoRoot `
    -WindowStyle Hidden `
    -RedirectStandardOutput $runtimeOut `
    -RedirectStandardError $runtimeErr `
    -PassThru

Write-Host "Runtime PID: $($runtimeProcess.Id)"
Write-Host "Waiting for runtime health at $controlUrl/health..."

$healthy = $false
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
        Write-Host "--- runtime-portable.err.log ---" -ForegroundColor Yellow
        Get-Content -LiteralPath $runtimeErr -Tail 120
    }
    throw "runtime health check failed"
}

Write-Host "Runtime is healthy." -ForegroundColor Green
Write-Host "Launching Ordo app..." -ForegroundColor Cyan
Start-Process -FilePath $appExe -WorkingDirectory $ordoRoot
