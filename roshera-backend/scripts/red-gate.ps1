<#
.SYNOPSIS
    geometry-engine integration red gate.

.DESCRIPTION
    Runs cargo test -p geometry-engine --no-fail-fast (or a scoped subset)
    and compares failures against geometry-engine/KNOWN_REDS.md.

    Exit codes:
      0  -- failures == allowlist exactly (known reds still red, nothing new)
      1  -- NEW_RED: at least one failure not in the allowlist
      2  -- RATCHET_VIOLATION: at least one allowlist entry is now passing
      3  -- Both NEW_RED and RATCHET_VIOLATION

    RATCHET RULE: when a test goes green, remove its line from KNOWN_REDS.md.
    Never add lines without a diagnosis doc.

.PARAMETER Scoped
    Comma-separated list of test binary names to run instead of the full suite.
    Example: -Scoped cf_beta_property,cf_beta_replay_determinism
    Useful for quick per-family checks without the full multi-hour run.

.PARAMETER AllowlistPath
    Path to the allowlist file. Defaults to geometry-engine/KNOWN_REDS.md
    relative to this script's backend directory. Override for testing with
    a modified copy.

.EXAMPLE
    powershell -File red-gate.ps1 -Scoped drawing_quality_oracle,gdt_oracle
    powershell -File red-gate.ps1 -Scoped cf_beta_property
    powershell -File red-gate.ps1   # full suite (runs long)
#>

param(
    [string]$Scoped = "",
    [string]$AllowlistPath = ""
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

# -- Resolve paths ------------------------------------------------------------

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Path
$BackendDir = Split-Path -Parent $ScriptDir
$GeDir      = Join-Path $BackendDir "geometry-engine"

if ($AllowlistPath -eq "") {
    $AllowlistPath = Join-Path $GeDir "KNOWN_REDS.md"
}

if (-not (Test-Path $AllowlistPath)) {
    Write-Error "Allowlist not found: $AllowlistPath"
    exit 1
}

# -- Parse allowlist ----------------------------------------------------------
# Lines that look like: <binary>::<test_name>  # diag: ...
# Lines starting with # or blank are skipped.

$allowlist = @{}   # key = "binary::test", value = $true

foreach ($line in (Get-Content $AllowlistPath)) {
    $trimmed = $line.Trim()
    if ($trimmed -eq "" -or $trimmed.StartsWith("#")) { continue }
    # Strip trailing comment
    $idx = $trimmed.IndexOf("  #")
    if ($idx -ge 0) { $trimmed = $trimmed.Substring(0, $idx).Trim() }
    if ($trimmed -eq "") { continue }
    if (-not $trimmed.Contains("::")) {
        Write-Warning "Skipping malformed allowlist line: $line"
        continue
    }
    $allowlist[$trimmed] = $true
}

Write-Host ""
Write-Host "=== red-gate.ps1 ===" -ForegroundColor Cyan
Write-Host "Allowlist: $AllowlistPath ($($allowlist.Count) entries)" -ForegroundColor Cyan

# -- Build cargo test arguments -----------------------------------------------

$cargoArgs = @("test", "-p", "geometry-engine", "-j", "4", "--no-fail-fast")

$scopedBinaries = [string[]]@()
if ($Scoped -ne "") {
    $scopedBinaries = [string[]]($Scoped.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" })
    foreach ($bin in $scopedBinaries) {
        $cargoArgs += "--test"
        $cargoArgs += $bin
    }
    Write-Host "Scope: $($scopedBinaries -join ', ')" -ForegroundColor Cyan
} else {
    Write-Host "Scope: full suite" -ForegroundColor Cyan
}

$cargoArgs += "--"
$cargoArgs += "--nocapture"

# -- Run cargo test -----------------------------------------------------------
# cargo test exits nonzero when tests fail; capture that without aborting the script.

Write-Host ""
Write-Host "Running: cargo $($cargoArgs -join ' ')" -ForegroundColor Yellow

# Unique per invocation: two gate instances (or a rerun started while a
# previous instance is still reporting) must never share capture files. A
# fixed path let a second run overwrite the first's captures BEFORE the
# first computed its verdict — the first instance then judged the second
# run's PARTIAL output and passed a gate that had a real failure in it
# (observed 2026-07-21, box_sphere_conquered_band_gate).
$stamp = "{0}-{1}" -f $PID, (Get-Date -Format "yyyyMMdd-HHmmss")
$stdoutFile = Join-Path $env:TEMP "red-gate-stdout-$stamp.txt"
$stderrFile = Join-Path $env:TEMP "red-gate-stderr-$stamp.txt"

$proc = Start-Process -FilePath "cargo" `
    -ArgumentList $cargoArgs `
    -WorkingDirectory $BackendDir `
    -NoNewWindow -Wait -PassThru `
    -RedirectStandardOutput $stdoutFile `
    -RedirectStandardError  $stderrFile

$stdoutLines = @()
$stderrLines = @()
if (Test-Path $stdoutFile) { $stdoutLines = Get-Content $stdoutFile -ErrorAction SilentlyContinue }
if (Test-Path $stderrFile) { $stderrLines = Get-Content $stderrFile -ErrorAction SilentlyContinue }

# Print all output for visibility — as TWO bulk writes, not per-line
# Write-Host: a full-suite run is >1 MB and the per-line loop took ~1 h on
# a redirected console, during which the fixed-path capture files were
# overwritten by a second instance (see stamp note above).
if ($stderrLines) { Write-Host ($stderrLines -join [Environment]::NewLine) }
if ($stdoutLines) { Write-Host ($stdoutLines -join [Environment]::NewLine) }
Write-Host "capture files: $stdoutFile · $stderrFile"

# Extract the ordered list of binaries from stderr.
# Cargo prints "Running tests/foo.rs (...)" to stderr, one per binary, in order.
$orderedBinaries = [System.Collections.Generic.List[string]]::new()
foreach ($line in $stderrLines) {
    if ($line -match "Running tests[/\\]([^./\\]+)\.rs\b") {
        $orderedBinaries.Add($Matches[1]) | Out-Null
    }
}

# Extract per-binary test blocks from stdout.
# Stdout contains N contiguous blocks, one per binary, in the same order as stderr.
# Each block starts with "running N tests" and ends with "test result: ...".
# We split stdout into blocks and associate each with its binary from $orderedBinaries.

# Build the stdout block list: each element is an array of lines for one binary.
$blocks = [System.Collections.Generic.List[object]]::new()
$currentBlock = [System.Collections.Generic.List[string]]::new()
$inBlock = $false

foreach ($line in $stdoutLines) {
    $trimLine = $line.Trim()
    if ($trimLine -match "^running \d+ test") {
        # Start of a new block
        if ($currentBlock.Count -gt 0) {
            $blocks.Add($currentBlock.ToArray())
        }
        $currentBlock = [System.Collections.Generic.List[string]]::new()
        $inBlock = $true
    }
    if ($inBlock) { $currentBlock.Add($line) | Out-Null }
    if ($trimLine -match "^test result:") {
        # End of block
        $blocks.Add($currentBlock.ToArray())
        $currentBlock = [System.Collections.Generic.List[string]]::new()
        $inBlock = $false
    }
}
# Catch any trailing block not terminated by "test result:"
if ($currentBlock.Count -gt 0) { $blocks.Add($currentBlock.ToArray()) }

# -- Parse failures from cargo output -----------------------------------------
#
# cargo test --no-fail-fast emits to STDOUT (per binary):
#   running N tests
#   test <name> ... ok
#   test <name> ... FAILED
#   failures:
#       <name>
#   test result: FAILED. ...
#
# and to STDERR: compiler warnings + "Running tests/<binary>.rs (...)" headers.
# We paired each stdout block with its binary (by index) above.

$observedFails = @{}   # key = "binary::test", value = $true

# Parse each stdout block paired with its binary name.
for ($bi = 0; $bi -lt $blocks.Count; $bi++) {
    $binary = if ($bi -lt $orderedBinaries.Count) { $orderedBinaries[$bi] } else { "__unknown__" }
    if ($binary -eq "__lib__") { continue }

    $inFailuresList = $false

    foreach ($rawLine in $blocks[$bi]) {
        $trimLine = $rawLine.Trim()

        # "test <name> ... FAILED" -- primary detection
        if ($trimLine -match "^test\s+(\S+)\s+\.\.\.\s+FAILED$") {
            $testName = $Matches[1]
            $key = "$binary::$testName"
            $observedFails[$key] = $true
            $inFailuresList = $false
            continue
        }

        # "failures:" block header -- secondary detection (proptest etc.)
        if ($trimLine -eq "failures:") {
            $inFailuresList = $true
            continue
        }

        if ($inFailuresList) {
            if ($trimLine -eq "") { continue }
            # Test name lines in the failures block: non-blank, no keywords.
            if ($trimLine -notmatch "^test result:" -and
                $trimLine -notmatch "^error" -and
                $trimLine -notmatch "^note" -and
                $trimLine -notmatch "^warning") {
                $key = "$binary::$trimLine"
                $observedFails[$key] = $true
            } else {
                $inFailuresList = $false
            }
            continue
        }
    }
}

# -- Compare observed vs allowlist --------------------------------------------

$newReds           = [string[]]@()  # in observedFails, not in allowlist
$ratchetViolations = [string[]]@()  # in allowlist, not in observedFails (now passing)

foreach ($key in $observedFails.Keys) {
    if (-not $allowlist.ContainsKey($key)) {
        $newReds += $key
    }
}

foreach ($key in $allowlist.Keys) {
    # Only check entries belonging to a binary we actually ran.
    $binary = $key.Split("::")[0]
    $inScope = $true
    if ($scopedBinaries.Count -gt 0) {
        $inScope = $scopedBinaries -contains $binary
    }
    if ($inScope -and (-not $observedFails.ContainsKey($key))) {
        $ratchetViolations += $key
    }
}

# -- Report -------------------------------------------------------------------

Write-Host ""
Write-Host "=== red-gate results ===" -ForegroundColor Cyan

if ($observedFails.Count -eq 0) {
    Write-Host "Observed failures: none" -ForegroundColor Green
} else {
    Write-Host "Observed failures ($($observedFails.Count)):" -ForegroundColor Yellow
    foreach ($k in ($observedFails.Keys | Sort-Object)) {
        if ($allowlist.ContainsKey($k)) {
            Write-Host "  [known]  $k"
        } else {
            Write-Host "  [NEW]    $k"
        }
    }
}

if ($newReds.Count -gt 0) {
    Write-Host ""
    Write-Host "NEW_RED -- $($newReds.Count) failure(s) not in allowlist:" -ForegroundColor Red
    foreach ($k in ($newReds | Sort-Object)) {
        Write-Host "  $k" -ForegroundColor Red
    }
    Write-Host "  -> Add a diagnosis doc to .superpowers/sdd/ and a KNOWN_REDS.md entry." -ForegroundColor Red
}

if ($ratchetViolations.Count -gt 0) {
    Write-Host ""
    Write-Host "RATCHET_VIOLATION -- $($ratchetViolations.Count) allowlist entry/entries now passing:" -ForegroundColor Magenta
    foreach ($k in ($ratchetViolations | Sort-Object)) {
        Write-Host "  $k" -ForegroundColor Magenta
    }
    Write-Host "  -> Remove these lines from KNOWN_REDS.md (never re-add without a new diagnosis)." -ForegroundColor Magenta
}

if ($newReds.Count -eq 0 -and $ratchetViolations.Count -eq 0) {
    Write-Host ""
    Write-Host "GATE PASSED - failures match allowlist exactly." -ForegroundColor Green
    exit 0
}

# Determine exit code: 1=NEW_RED, 2=RATCHET_VIOLATION, 3=both
$exitCode = 0
if ($newReds.Count -gt 0)           { $exitCode = $exitCode -bor 1 }
if ($ratchetViolations.Count -gt 0) { $exitCode = $exitCode -bor 2 }
exit $exitCode
