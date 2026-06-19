param(
    [ValidateSet("quick", "standard", "full")]
    [string]$Suite = "standard",

    [string]$BaseUrl = "http://127.0.0.1:4142",
    [string]$OutDir = "",

    [switch]$SkipStudio,
    [switch]$SkipServo,
    [switch]$SkipRuntimeHarness,
    [switch]$IncludeNetwork,
    [switch]$KeepGoing,
    [switch]$NoLaunchRuntimeHarness,
    [switch]$Check
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutDir = Join-Path $repoRoot "target\ordo-function-test\$stamp"
}
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$results = New-Object System.Collections.Generic.List[object]

function Add-Result {
    param(
        [string]$Name,
        [string]$Status,
        [string]$Summary,
        [int]$ExitCode = 0,
        [string]$Stdout = "",
        [string]$Stderr = ""
    )

    $row = [ordered]@{
        name = $Name
        status = $Status
        summary = $Summary
        exit_code = $ExitCode
        stdout = $Stdout
        stderr = $Stderr
    }
    $results.Add($row) | Out-Null

    $color = switch ($Status) {
        "passed" { "Green" }
        "skipped" { "Yellow" }
        default { "Red" }
    }
    Write-Host "[$Status] $Name - $Summary" -ForegroundColor $color
}

function Get-PythonCommand {
    if (Get-Command python -ErrorAction SilentlyContinue) {
        return "python"
    }
    if (Get-Command py -ErrorAction SilentlyContinue) {
        return "py"
    }
    throw "Neither python nor py was found on PATH"
}

function Invoke-OrdoCommand {
    param(
        [string]$Name,
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$WorkingDirectory = $repoRoot,
        [hashtable]$Environment = @{}
    )

    function Join-ProcessArguments {
        param([string[]]$Items)
        $quoted = foreach ($item in $Items) {
            if ($null -eq $item) {
                '""'
            } elseif ($item -match '[\s"]') {
                '"' + ($item -replace '"', '\"') + '"'
            } else {
                $item
            }
        }
        return ($quoted -join " ")
    }

    $safeName = ($Name -replace "[^A-Za-z0-9_.-]", "_")
    $stdoutPath = Join-Path $OutDir "$safeName.out.txt"
    $stderrPath = Join-Path $OutDir "$safeName.err.txt"

    if ($Check) {
        Add-Result $Name "skipped" ("check mode: {0} {1}" -f $FilePath, ($Arguments -join " "))
        return
    }

    Write-Host ""
    Write-Host "==> $Name" -ForegroundColor Cyan
    Write-Host ("    {0} {1}" -f $FilePath, ($Arguments -join " "))

    $resolvedCommand = Get-Command $FilePath -ErrorAction SilentlyContinue
    if ($resolvedCommand) {
        $FilePath = $resolvedCommand.Source
    }
    if ([System.IO.Path]::GetExtension($FilePath) -eq ".ps1") {
        $cmdSibling = [System.IO.Path]::ChangeExtension($FilePath, ".cmd")
        if (Test-Path -LiteralPath $cmdSibling) {
            $FilePath = $cmdSibling
        }
    }
    $processArgs = Join-ProcessArguments $Arguments
    if ([System.IO.Path]::GetExtension($FilePath) -in @(".cmd", ".bat")) {
        $processArgs = "/d /c " + '"' + (Join-ProcessArguments @($FilePath)) + " " + $processArgs + '"'
        $FilePath = "$env:ComSpec"
    }

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $FilePath
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.Arguments = $processArgs
    foreach ($key in $Environment.Keys) {
        $psi.Environment[$key] = [string]$Environment[$key]
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $psi
    [void]$process.Start()
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $stdout = $stdoutTask.Result
    $stderr = $stderrTask.Result

    Set-Content -LiteralPath $stdoutPath -Value $stdout -Encoding UTF8
    Set-Content -LiteralPath $stderrPath -Value $stderr -Encoding UTF8

    if ($process.ExitCode -eq 0) {
        Add-Result $Name "passed" "exit 0" $process.ExitCode $stdoutPath $stderrPath
    } else {
        Add-Result $Name "failed" "exit $($process.ExitCode)" $process.ExitCode $stdoutPath $stderrPath
        if (-not $KeepGoing) {
            throw "$Name failed with exit code $($process.ExitCode). Logs: $stdoutPath / $stderrPath"
        }
    }
}

function Test-Tool {
    param([string]$Name)
    if (Get-Command $Name -ErrorAction SilentlyContinue) {
        Add-Result "tool:$Name" "passed" "found on PATH"
    } else {
        Add-Result "tool:$Name" "failed" "missing from PATH"
        if (-not $KeepGoing) {
            throw "$Name was not found on PATH"
        }
    }
}

function Resolve-OrdoBinary {
    $exe = Join-Path $repoRoot "target\debug\ordo.exe"
    if (Test-Path -LiteralPath $exe) {
        return $exe
    }
    $unix = Join-Path $repoRoot "target\debug\ordo"
    if (Test-Path -LiteralPath $unix) {
        return $unix
    }
    return $exe
}

Write-Host ""
Write-Host "Ordo function test suite" -ForegroundColor Cyan
Write-Host "Workspace: $repoRoot"
Write-Host "Suite: $Suite"
Write-Host "Logs: $OutDir"
Write-Host "Runtime test URL: $BaseUrl"
Write-Host ""

Test-Tool "cargo"
Test-Tool "npm"
$python = Get-PythonCommand
Add-Result "tool:python" "passed" $python

Invoke-OrdoCommand "cargo-fmt-check" "cargo" @("fmt", "--check")

if ($Suite -in @("quick", "standard")) {
    Invoke-OrdoCommand "cargo-check-core" "cargo" @(
        "check",
        "-p", "ordo-cli",
        "-p", "ordo-runtime",
        "-p", "ordo-control",
        "-p", "ordo-assistant"
    )
} else {
    Invoke-OrdoCommand "cargo-check-workspace" "cargo" @("check", "--workspace")
}

if ($Suite -eq "full") {
    Invoke-OrdoCommand "cargo-test-workspace" "cargo" @("test", "--workspace")
} else {
    Invoke-OrdoCommand "cargo-test-runtime-smoke" "cargo" @("test", "-p", "ordo-runtime", "--test", "smoke")
}

if (-not $SkipStudio) {
    $studioDir = Join-Path $repoRoot "ordo-studio"
    if (-not (Test-Path -LiteralPath (Join-Path $studioDir "node_modules"))) {
        Invoke-OrdoCommand "studio-npm-ci" "npm" @("ci") $studioDir
    }
    Invoke-OrdoCommand "studio-build" "npm" @("run", "build") $studioDir
} else {
    Add-Result "studio-build" "skipped" "-SkipStudio"
}

if (-not $SkipServo) {
    Invoke-OrdoCommand "servo-shell-check" "cargo" @(
        "check",
        "--manifest-path", "ordo-servo-shell\Cargo.toml",
        "--features", "servo-engine"
    )
} else {
    Add-Result "servo-shell-check" "skipped" "-SkipServo"
}

if (-not $SkipRuntimeHarness) {
    $ordoBin = Resolve-OrdoBinary
    if (Test-Path -LiteralPath $ordoBin) {
        Add-Result "ordo-cli-binary" "passed" "using existing $ordoBin"
    } else {
        Invoke-OrdoCommand "cargo-build-ordo-cli" "cargo" @("build", "-p", "ordo-cli")
        $ordoBin = Resolve-OrdoBinary
    }
    $fullArgs = @("scripts\ordo_full_test.py", "--base-url", $BaseUrl, "--bin", $ordoBin)
    if ($NoLaunchRuntimeHarness) {
        $fullArgs += "--no-launch"
    }
    if ($IncludeNetwork) {
        $fullArgs += "--include-network"
    }
    Invoke-OrdoCommand "runtime-full-harness" $python $fullArgs
} else {
    Add-Result "runtime-full-harness" "skipped" "-SkipRuntimeHarness"
}

$summary = [ordered]@{
    workspace = $repoRoot
    suite = $Suite
    base_url = $BaseUrl
    generated_at = (Get-Date).ToString("o")
    passed = @($results | Where-Object { $_.status -eq "passed" }).Count
    failed = @($results | Where-Object { $_.status -eq "failed" }).Count
    skipped = @($results | Where-Object { $_.status -eq "skipped" }).Count
    results = $results
}

$jsonPath = Join-Path $OutDir "ordo-function-test-report.json"
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $jsonPath -Encoding UTF8

Write-Host ""
Write-Host "Report: $jsonPath" -ForegroundColor Cyan
Write-Host ("Summary: {0} passed, {1} failed, {2} skipped" -f $summary.passed, $summary.failed, $summary.skipped)

if ($summary.failed -gt 0) {
    exit 1
}
exit 0
