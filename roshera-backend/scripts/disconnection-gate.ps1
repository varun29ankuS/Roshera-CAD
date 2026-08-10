<#
.SYNOPSIS
    Disconnection gate -- ratchet on public surface with no production consumer.

.DESCRIPTION
    Generalises the two gates this repo already trusts:

      * geometry-engine/KNOWN_REDS.md + red-gate.ps1  -- the RATCHET shape
        (a new violation blocks; a listed violation that stops violating ALSO
        blocks, forcing the entry's deliberate removal).
      * api-server/src/agent_registry.rs               -- the DRIFT shape
        (two independently-maintained surfaces, asserted equal).

    WHAT IT CHECKS (the dead-symbol class, and only that class):
    for every `pub fn|struct|enum|trait|const|type|static` declared in the
    PRODUCTION half of the scoped crates' `src/`, does at least one production
    file OTHER THAN THE DECLARING FILE mention that name anywhere in the
    workspace (backend crates, roshera-mcp/src, roshera-app/src, roshera-eval)?
    Zero such mentions => the symbol is public surface nobody consumes.

    It is pure text analysis. It never invokes cargo, never compiles, never
    installs anything -- so it is unaffected by the disk/toolchain constraints
    that block cargo-udeps / ts-prune today.

    WHAT IT CANNOT CHECK: the wiring-shape class -- two independently produced
    pieces of state (claim/artifact, route/consumer, field/registry-entry,
    type/handler) that each compile, each pass a symbol-reachability check, and
    only disagree at runtime. Those are 8 of the 12 classified instances. They
    are recorded in the allowlist with `class=wiring-shape` and the gate
    NEVER JUDGES THEM -- it prints them as informational only. Do not "fix"
    that: a wiring-shape entry IS referenced by definition, so judging it would
    fire a permanent, meaningless RATCHET_VIOLATION.

    Exit codes (mirroring red-gate.ps1):
      0  -- observed disconnections == allowlist exactly (nothing new, nothing
            silently wired)
      1  -- NEW_DISCONNECTED: a public symbol with no production consumer that
            is not in the allowlist. Wire it, make it pub(crate)/private, or
            add an allowlist line with a diagnosis.
      2  -- RATCHET_VIOLATION: an allowlist entry now HAS a production
            consumer. Remove the line from KNOWN_DISCONNECTED.md.
      3  -- Both.
      4  -- SCAN_REFUSED: the scan produced no candidates, or a scoped crate
            has no src/ directory. The gate refuses to report "all clear" on a
            scan that did not actually run.

.PARAMETER Crates
    Comma-separated crate names to enumerate candidates from. Defaults to the
    eight tractable crates (see $DefaultCrates below). geometry-engine and
    api-server are deliberately OUT of the default scope -- see
    roshera-backend/KNOWN_DISCONNECTED.md "Scope" for the rationale. Widening
    the scope is a one-line change here plus a re-seed.

.PARAMETER AllowlistPath
    Path to the allowlist. Defaults to roshera-backend/KNOWN_DISCONNECTED.md.
    Override to test the gate against a modified copy.

.PARAMETER Seed
    Emit the BASELINE STOCK block (final line format, sorted) to stdout and
    exit 0 without judging. Used once to seed the allowlist, and again when
    the scope changes. ALWAYS seed from this script, never from a separate
    prototype -- a second implementation's tokenizer will not agree with this
    one and the first "clean" run will not be clean.

.EXAMPLE
    powershell -File roshera-backend/scripts/disconnection-gate.ps1
    powershell -File roshera-backend/scripts/disconnection-gate.ps1 -Seed
    powershell -File roshera-backend/scripts/disconnection-gate.ps1 -Crates ros-format
#>

param(
    [string]$Crates = "",
    [string]$AllowlistPath = "",
    [switch]$Seed
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

# -- Scope --------------------------------------------------------------------
#
# The eight crates whose public surface is small enough to enumerate honestly.
# geometry-engine (3725 pub items across 267 files) is excluded: the design doc
# measured the false-positive trimming that scope needs (macro-registered ops,
# trait-object dispatch through ai_operations_registry, legitimate cross-crate
# API) and explicitly deferred it. api-server is excluded because it is a BIN
# crate -- rustc's own dead_code lint already fires there (~234 warnings), which
# is exactly the coverage lib crates structurally do not get.

$DefaultCrates = @(
    "shared-types",
    "ros-format",
    "timeline-engine",
    "export-engine",
    "assembly-engine",
    "session-manager",
    "ai-integration",
    "verdict-harness"
)

# -- Resolve paths ------------------------------------------------------------

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Path
$BackendDir = Split-Path -Parent $ScriptDir
$RepoRoot   = Split-Path -Parent $BackendDir

if ($AllowlistPath -eq "") {
    $AllowlistPath = Join-Path $BackendDir "KNOWN_DISCONNECTED.md"
}

$scopedCrates = $DefaultCrates
if ($Crates -ne "") {
    $scopedCrates = [string[]]($Crates.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" })
}

# -- Shared file classification -----------------------------------------------
#
# "Test code" boundary, per roshera-backend/CLAUDE.md: a file's first
# `#[cfg(test)]` line is the boundary; everything below it is test code. Files
# under tests/ benches/ examples/ primitive_tests/ test_math/ and files named
# *_tests.rs / *.test.ts / *.spec.ts are test code by definition.

$TestPathSeg = [regex]::new('[\\/](tests|benches|examples|primitive_tests|test_math|__tests__)[\\/]',
                            [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
# `state` and `logs` hold live runtime artefacts (goose session config, server
# state) -- not source, and frequently held open by a running process.
$SkipDirSeg  = [regex]::new('[\\/](target|node_modules|dist|\.git|coverage|state|logs|\.goose)[\\/]',
                            [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)

function Test-IsTestFile([string]$path) {
    if ($TestPathSeg.IsMatch($path)) { return $true }
    $leaf = Split-Path -Leaf $path
    if ($leaf -like "*_tests.rs")  { return $true }
    if ($leaf -like "*.test.ts")   { return $true }
    if ($leaf -like "*.test.tsx")  { return $true }
    if ($leaf -like "*.spec.ts")   { return $true }
    if ($leaf -like "*.spec.tsx")  { return $true }
    return $false
}

# Production text of a file: for Rust, everything above the first `#[cfg(test)]`.
function Get-ProductionText([string]$path) {
    try {
        $txt = [System.IO.File]::ReadAllText($path)
    } catch {
        # A file held open by another process contributes no references. That
        # biases toward reporting MORE disconnections (the blocking direction),
        # never fewer -- but say so out loud rather than swallowing it.
        Write-Warning "Unreadable, contributing no references: $path"
        return ""
    }
    if ($path -like "*.rs") {
        $idx = $txt.IndexOf("#[cfg(test)]")
        if ($idx -ge 0) { $txt = $txt.Substring(0, $idx) }
    }
    return $txt
}

function Get-SourceFiles([string]$root, [string[]]$extensions) {
    if (-not (Test-Path $root)) { return @() }
    return Get-ChildItem -Path $root -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object {
            ($extensions -contains $_.Extension.ToLower()) -and
            (-not $SkipDirSeg.IsMatch($_.FullName)) -and
            (-not (Test-IsTestFile $_.FullName))
        }
}

# -- 1. Enumerate candidate public symbols ------------------------------------

$DeclRe = [regex]::new('(?m)^[ \t]*pub +(?:fn|struct|enum|trait|const|type|static) +([A-Za-z_][A-Za-z0-9_]*)')

$candidates = @{}   # key "crate::Symbol" -> @{ Crate; Symbol; File; Line }

foreach ($crate in $scopedCrates) {
    $src = Join-Path (Join-Path $BackendDir $crate) "src"
    if (-not (Test-Path $src)) {
        Write-Host ""
        Write-Host "SCAN_REFUSED -- scoped crate '$crate' has no src/ at $src" -ForegroundColor Red
        exit 4
    }
    foreach ($f in (Get-SourceFiles $src @(".rs"))) {
        $txt = Get-ProductionText $f.FullName
        if ($txt -eq "") { continue }
        foreach ($m in $DeclRe.Matches($txt)) {
            $sym = $m.Groups[1].Value
            $key = "$crate::$sym"
            if ($candidates.ContainsKey($key)) { continue }   # first declaration wins
            $line = ($txt.Substring(0, $m.Index) -split "`n").Count
            $candidates[$key] = @{
                Crate  = $crate
                Symbol = $sym
                File   = $f.FullName
                Line   = $line
            }
        }
    }
}

if ($candidates.Count -eq 0) {
    Write-Host ""
    Write-Host "SCAN_REFUSED -- zero public symbols enumerated across: $($scopedCrates -join ', ')" -ForegroundColor Red
    Write-Host "The gate will not report 'all clear' on a scan that did not run." -ForegroundColor Red
    exit 4
}

# -- 2. Build the production reference index ----------------------------------
#
# ident -> declaring-file-if-seen-exactly-one-file, or "__MULTI__".
# We only ever need "does this name appear in a production file OTHER than the
# one that declares it", so collapsing past two distinct files saves the memory
# a full ident->file-set index would cost.

$CodeExt = @(".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs", ".json")
$refRoots = @(
    $BackendDir,
    (Join-Path (Join-Path $RepoRoot "roshera-mcp") "src"),
    (Join-Path (Join-Path $RepoRoot "roshera-app") "src"),
    (Join-Path $RepoRoot "roshera-eval")
)

$NonIdent = [regex]::new('[^A-Za-z0-9_]+')
$refIndex = New-Object 'System.Collections.Generic.Dictionary[string,string]'

$scannedFiles = 0
foreach ($root in $refRoots) {
    foreach ($f in (Get-SourceFiles $root $CodeExt)) {
        $txt = Get-ProductionText $f.FullName
        if ($txt -eq "") { continue }
        $scannedFiles++
        $path = $f.FullName.ToLowerInvariant()
        $seen = New-Object 'System.Collections.Generic.HashSet[string]'
        foreach ($tok in $NonIdent.Split($txt)) {
            if ($tok.Length -eq 0) { continue }
            [void]$seen.Add($tok)
        }
        foreach ($tok in $seen) {
            $prev = $null
            if ($refIndex.TryGetValue($tok, [ref]$prev)) {
                if ($prev -ne "__MULTI__" -and $prev -ne $path) { $refIndex[$tok] = "__MULTI__" }
            } else {
                $refIndex[$tok] = $path
            }
        }
    }
}

# -- 3. Observed disconnections -----------------------------------------------

$observed = @{}   # key -> @{ Crate; Symbol; RelPath; Line }

foreach ($key in $candidates.Keys) {
    $c    = $candidates[$key]
    $homeFile = $c.File.ToLowerInvariant()   # NB: $home is a reserved PS variable
    $where = $null
    $hasOther = $false
    if ($refIndex.TryGetValue($c.Symbol, [ref]$where)) {
        if ($where -eq "__MULTI__" -or $where -ne $homeFile) { $hasOther = $true }
    }
    if (-not $hasOther) {
        $rel = $c.File
        if ($rel.StartsWith($RepoRoot)) { $rel = $rel.Substring($RepoRoot.Length).TrimStart('\', '/') }
        $observed[$key] = @{
            Crate   = $c.Crate
            Symbol  = $c.Symbol
            RelPath = ($rel -replace '\\', '/')
            Line    = $c.Line
        }
    }
}

# -- Seed mode ----------------------------------------------------------------

if ($Seed) {
    $today = Get-Date -Format "yyyy-MM-dd"
    foreach ($key in ($observed.Keys | Sort-Object)) {
        $o = $observed[$key]
        Write-Output ("{0}  # class=dead-symbol file={1}:{2} date={3}" -f $key, $o.RelPath, $o.Line, $today)
    }
    Write-Host ""
    Write-Host ("seeded {0} dead-symbol entries from {1} candidates over {2} production files" -f `
        $observed.Count, $candidates.Count, $scannedFiles) -ForegroundColor Cyan
    Write-Host "These are SECTION B lines ONLY. Paste them below the SECTION B marker," -ForegroundColor Yellow
    Write-Host "replacing the previous block. NEVER redirect this over KNOWN_DISCONNECTED.md --" -ForegroundColor Yellow
    Write-Host "that wipes the ratchet rule, the scope rationale and all fourteen diagnoses," -ForegroundColor Yellow
    Write-Host "and the gate would then pass clean forever with zero classified stock." -ForegroundColor Yellow
    exit 0
}

# -- 4. Parse the allowlist ---------------------------------------------------
#
# Line format (mirrors KNOWN_REDS.md: everything from the first "  #" onward is
# metadata and is NOT part of the comparison key -- so a file:line that drifts
# with an unrelated edit never fires a spurious NEW/RATCHET pair):
#
#   <crate>::<Symbol>  # class=dead-symbol|wiring-shape file=<path>:<line> date=<yyyy-mm-dd> [diag: ...]

if (-not (Test-Path $AllowlistPath)) {
    Write-Host "Allowlist not found: $AllowlistPath" -ForegroundColor Red
    exit 4
}

$allowDead   = @{}   # key -> $true   (judged)
$allowWiring = @{}   # key -> meta    (informational only, never judged)

foreach ($line in (Get-Content $AllowlistPath)) {
    $trimmed = $line.Trim()
    if ($trimmed -eq "" -or $trimmed.StartsWith("#")) { continue }
    $idx = $trimmed.IndexOf("  #")
    $meta = ""
    if ($idx -ge 0) {
        $meta    = $trimmed.Substring($idx + 2)
        $trimmed = $trimmed.Substring(0, $idx).Trim()
    }
    if ($trimmed -eq "") { continue }
    if (-not $trimmed.Contains("::")) {
        Write-Warning "Skipping malformed allowlist line (no '::'): $line"
        continue
    }
    if ($meta -match 'class=([A-Za-z-]+)') {
        $class = $Matches[1]
    } else {
        Write-Warning "Skipping allowlist line with no class= field: $line"
        continue
    }
    switch ($class) {
        "dead-symbol"  { $allowDead[$trimmed]   = $true }
        "wiring-shape" { $allowWiring[$trimmed] = $meta }
        # A confirmed dead symbol in a crate outside this gate's scope. Never
        # judged: the scan does not enumerate it, and its bare name may well
        # collide with an unrelated identifier elsewhere. Widening -Crates is
        # therefore a deliberate act that must re-classify these to
        # dead-symbol, not something that silently starts judging them.
        "out-of-scope" { $allowWiring[$trimmed] = $meta }
        default        { Write-Warning "Skipping allowlist line with unknown class '$class': $line" }
    }
}

Write-Host ""
Write-Host "=== disconnection-gate.ps1 ===" -ForegroundColor Cyan
Write-Host "Scope:     $($scopedCrates -join ', ')" -ForegroundColor Cyan
Write-Host "Allowlist: $AllowlistPath" -ForegroundColor Cyan
Write-Host ("           {0} dead-symbol (judged) + {1} wiring-shape (informational)" -f `
    $allowDead.Count, $allowWiring.Count) -ForegroundColor Cyan
Write-Host ("Scanned:   {0} candidate pub items; {1} production files indexed" -f `
    $candidates.Count, $scannedFiles) -ForegroundColor Cyan

# -- 5. Compare ---------------------------------------------------------------

$newDisconnected   = [string[]]@()   # observed, not allowlisted
$ratchetViolations = [string[]]@()   # allowlisted dead-symbol, now consumed

foreach ($key in $observed.Keys) {
    if (-not $allowDead.ContainsKey($key)) { $newDisconnected += $key }
}

foreach ($key in $allowDead.Keys) {
    # Only judge entries whose crate is actually in this run's scope.
    $crate = $key.Split("::")[0]
    if ($scopedCrates -notcontains $crate) { continue }
    if (-not $observed.ContainsKey($key)) { $ratchetViolations += $key }
}

# -- 6. Report ----------------------------------------------------------------

Write-Host ""
Write-Host "=== disconnection-gate results ===" -ForegroundColor Cyan
Write-Host ("Observed disconnected public symbols: {0}" -f $observed.Count)

if ($allowWiring.Count -gt 0) {
    Write-Host ""
    Write-Host "wiring-shape entries (INFORMATIONAL -- not machine-checkable):" -ForegroundColor DarkGray
    foreach ($k in ($allowWiring.Keys | Sort-Object)) {
        Write-Host ("  [wiring] {0}" -f $k) -ForegroundColor DarkGray
    }
    Write-Host "  -> each needs a production-call-site assertion test, not a symbol scan." -ForegroundColor DarkGray
}

if ($newDisconnected.Count -gt 0) {
    Write-Host ""
    Write-Host "NEW_DISCONNECTED -- $($newDisconnected.Count) public symbol(s) with no production consumer, not in the allowlist:" -ForegroundColor Red
    foreach ($k in ($newDisconnected | Sort-Object)) {
        $o = $observed[$k]
        Write-Host ("  {0}   ({1}:{2})" -f $k, $o.RelPath, $o.Line) -ForegroundColor Red
    }
    Write-Host "  -> Wire it to a production call site, demote it to pub(crate)/private," -ForegroundColor Red
    Write-Host "     or add a KNOWN_DISCONNECTED.md line with a diagnosis." -ForegroundColor Red
}

if ($ratchetViolations.Count -gt 0) {
    Write-Host ""
    Write-Host "RATCHET_VIOLATION -- $($ratchetViolations.Count) allowlist entry/entries now HAVE a production consumer:" -ForegroundColor Magenta
    foreach ($k in ($ratchetViolations | Sort-Object)) {
        Write-Host ("  {0}" -f $k) -ForegroundColor Magenta
    }
    Write-Host "  -> Remove these lines from KNOWN_DISCONNECTED.md (never re-add without a new diagnosis)." -ForegroundColor Magenta
}

if ($newDisconnected.Count -eq 0 -and $ratchetViolations.Count -eq 0) {
    Write-Host ""
    Write-Host "GATE PASSED - disconnected symbols match the allowlist exactly." -ForegroundColor Green
    exit 0
}

$exitCode = 0
if ($newDisconnected.Count -gt 0)   { $exitCode = $exitCode -bor 1 }
if ($ratchetViolations.Count -gt 0) { $exitCode = $exitCode -bor 2 }
exit $exitCode
