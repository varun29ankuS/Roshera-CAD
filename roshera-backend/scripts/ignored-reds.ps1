<#
.SYNOPSIS
    geometry-engine ignored-red gate -- the missing half of the red ratchet.

.DESCRIPTION
    `red-gate.ps1` compares FAILURES against `KNOWN_REDS.md`. An `#[ignore]`d
    test never fails, so a red parked behind `#[ignore = "..."]` is invisible to
    that gate: when the kernel is fixed and the parked test starts PASSING,
    nothing notices and the ignore stays forever. This script closes that hole
    from the other side.

    It enumerates every `#[ignore = "<reason>"]` site under
    `geometry-engine/src` and `geometry-engine/tests` whose REASON names a
    defect (red / fail / unsound / non-manifold / leak / defect / bug / known),
    runs exactly those tests with `--ignored`, and FAILS if any of them PASSES.

    A passing ignored red must lose its `#[ignore]` (and, if it was pinned,
    its `KNOWN_REDS.md` line).

    The keyword heuristic can pull in a site that is not a parked red (a
    diagnostic dump whose reason says "failing", an opt-in sweep that names a
    "hang regime"). Exclude such a site with a comment on ANY OF THE THREE LINES
    directly above the `#[ignore]` attribute -- three, not one, because the
    attribute order varies (`#[test]` may sit between the marker and the
    `#[ignore]`, and a `#[should_panic]` may sit between those):

        // ignored-reds: not-a-red -- diagnostic dump, nothing is asserted
        #[test]
        #[ignore = "diagnostic - renders failing cells (run with --ignored)"]

    The marker must carry a reason, and it is visible in the diff. Never widen
    the pattern to silence one site.

    JURISDICTION: red-gate.ps1 judges the RUNNING `KNOWN_REDS.md` entries (it
    sees them FAIL); ignored-reds.ps1 judges the PARKED ones (`#[ignore]`d,
    which can never fail).

    The script CROSS-CHECKS `geometry-engine/KNOWN_REDS.md` on that split. A
    PARKED entry this enumeration misses -- because its ignore reason dodges
    every keyword, or it carries a not-a-red marker -- has NO judge at all, and
    that is a refusal (exit 5). An entry that is NOT `#[ignore]`d is listed as
    RUNNING (judged by red-gate) and is not an error here.

    Exit codes:
      0  -- every enumerated ignored red is still red (failed or errored)
      1  -- PASSING_IGNORED_RED: at least one enumerated test PASSED
      2  -- UNRESOLVED: an enumerated test produced no result line (the filter
            matched nothing, or the runner died before reporting). The gate
            refuses to judge on a missing observation instead of scoring it
            green -- a filter that matches nothing is a stale enumeration, not
            a clean run.
      3  -- RUN_ERROR: a cargo invocation produced no test block at all
            (compile failure / harness could not start).
      4  -- PARSE_REFUSAL: an `#[ignore = "..."]` site could not be parsed to a
            test name and target. The gate never silently skips a site it
            cannot read.
      5  -- ALLOWLIST_UNJUDGED: a PARKED `KNOWN_REDS.md` entry is not in this
            enumeration, so neither gate judges it. (A RUNNING entry is not an
            error -- red-gate judges those.)

.PARAMETER Scoped
    Comma-separated target names to restrict the run to. A target is either an
    integration binary name (the `tests/<name>.rs` stem) or `__lib__` for the
    unit-test binary. Example: -Scoped cross_bore_mesh_wings,cross_bore_manifold

.PARAMETER Only
    Comma-separated test function names to restrict the run to. Combined with
    -Scoped this is how a single site is proved in seconds.

.PARAMETER List
    Enumerate and print the sites; run nothing. Always exits 0 unless a site
    fails to parse (4).

.EXAMPLE
    powershell -File ignored-reds.ps1 -List
    powershell -File ignored-reds.ps1 -Scoped cross_bore_mesh_wings
    powershell -File ignored-reds.ps1            # the whole enumerated set (long)
#>

param(
    [string]$Scoped = "",
    [string]$Only = "",
    [switch]$List
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = "Stop"

# -- Resolve paths ------------------------------------------------------------

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Path
$BackendDir = Split-Path -Parent $ScriptDir
$GeDir      = Join-Path $BackendDir "geometry-engine"

if (-not (Test-Path $GeDir)) {
    Write-Error "geometry-engine not found at: $GeDir"
    exit 3
}

# A reason that names a DEFECT. An ignore whose reason is "needs a fixture the
# builder cannot make yet" is scaffolding, not a parked red -- out of scope.
#
# The words are anchored on WORD BOUNDARIES, which is not cosmetic. A bare
# `red` alternative matched "--ignoRED", "stoRED" and "boundary-reduction",
# pulling four release-only perf tests and two diagnostic dumps into the gate;
# every one of them PASSES when run, so the gate would have demanded their
# `#[ignore]` be removed on the strength of a substring accident. Match the
# word, not the letters.
#
# `residual`, `corrupt`, `regression`, `incorrect`, `mis-` and `does not ` are
# added past the eight brief words because two reasons name a real parked
# defect without using any of them ("#35 Slice-3 residual: ... Ellipse
# slivers", "#24: bored revolved-flange mesh does not reflect the bolt hole").
# `hang` and `wrong` were tried and REJECTED: they pull in nine
# "fuzz survey - subprocess-isolated, HANG-honest" diagnostics that are not
# parked reds.
$DefectPattern = '\bred\b|\bfail|\bunsound\b|non-?manifold|\bleak|\bdefect|\bbug|\bknown\b|\bresidual\b|\bcorrupt|\bregression\b|\bincorrect|\bmis-|does not '

$scopedTargets = [string[]]@()
if ($Scoped -ne "") {
    $scopedTargets = [string[]]($Scoped.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" })
}
$onlyTests = [string[]]@()
if ($Only -ne "") {
    $onlyTests = [string[]]($Only.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" })
}

Write-Host ""
Write-Host "=== ignored-reds.ps1 ===" -ForegroundColor Cyan
Write-Host "Root: $GeDir" -ForegroundColor Cyan

# -- Enumerate ----------------------------------------------------------------
#
# An `#[ignore = "..."]` reason may span several source lines (backslash
# continuation inside the string literal), so the attribute is accumulated
# until it closes. A site that never closes, or that is not followed by an
# `fn`, is a PARSE_REFUSAL -- never a silent skip.

$sites      = [System.Collections.Generic.List[object]]::new()
$allIgnored = @{}   # key = "target::test" for EVERY #[ignore] site, judged or not
$refusals   = [System.Collections.Generic.List[string]]::new()

# Strip `//` comments and double-quoted strings so a brace inside either cannot
# move the module-nesting depth.
function Get-CodeOnly([string]$line) {
    $out = ""
    $inStr = $false
    $bs = [char]92
    $prev = [char]0
    for ($c = 0; $c -lt $line.Length; $c++) {
        $ch = $line[$c]
        if ($inStr) {
            if ($ch -eq '"' -and $prev -ne $bs) { $inStr = $false }
        } elseif ($ch -eq '"') {
            $inStr = $true
        } elseif ($ch -eq '/' -and $c + 1 -lt $line.Length -and $line[$c + 1] -eq '/') {
            break
        } else {
            $out += $ch
        }
        $prev = $ch
    }
    return $out
}

$files = [System.Collections.Generic.List[object]]::new()
foreach ($sub in @("tests", "src")) {
    $dir = Join-Path $GeDir $sub
    if (-not (Test-Path $dir)) { continue }
    foreach ($f in (Get-ChildItem -Path $dir -Filter *.rs -File -Recurse)) {
        $files.Add($f) | Out-Null
    }
}

foreach ($file in $files) {
    $lines = @(Get-Content -LiteralPath $file.FullName)

    # -- module nesting, one entry per line ----------------------------------
    # `--exact` needs the test's FULL path, not its fn name: the three lib
    # sites are `operations::boolean::tests::<fn>`. Walk the file once with a
    # brace-depth stack of enclosing `mod` blocks; a site's prefix is the stack
    # at its line. A derived prefix that is wrong makes the filter match
    # nothing, which this gate reports as UNRESOLVED (exit 2) -- loud, never
    # silently green.
    $modAtLine = New-Object string[] $lines.Count
    $depth = 0
    $modStack = [System.Collections.Generic.List[object]]::new()
    for ($m = 0; $m -lt $lines.Count; $m++) {
        $names = @()
        foreach ($e in $modStack) { $names += $e.Name }
        $modAtLine[$m] = ($names -join "::")

        $code = Get-CodeOnly $lines[$m]
        $pending = ""
        if ($code -match '(?:^|\s)mod\s+([A-Za-z0-9_]+)\s*\{') { $pending = $Matches[1] }
        foreach ($ch in $code.ToCharArray()) {
            if ($ch -eq '{') {
                $depth++
                if ($pending -ne "") {
                    $modStack.Add([pscustomobject]@{ Name = $pending; Depth = $depth }) | Out-Null
                    $pending = ""
                }
            } elseif ($ch -eq '}') {
                if ($modStack.Count -gt 0 -and $modStack[$modStack.Count - 1].Depth -eq $depth) {
                    $modStack.RemoveAt($modStack.Count - 1)
                }
                if ($depth -gt 0) { $depth-- }
            }
        }
    }

    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -notmatch '^\s*#\[\s*ignore\s*=\s*"') { continue }

        # Accumulate until the attribute closes.
        $buf = ""
        $j = $i
        $closed = $false
        while ($j -lt $lines.Count -and ($j - $i) -lt 40) {
            $buf = $buf + $lines[$j]
            if ($buf -match '^\s*#\[\s*ignore\s*=\s*"(?<reason>.*)"\s*\]\s*$') {
                $closed = $true
                break
            }
            $j++
        }
        if (-not $closed) {
            $refusals.Add(("{0}:{1} -- #[ignore = ...] attribute never closes within 40 lines" -f $file.FullName, ($i + 1))) | Out-Null
            continue
        }
        $reason = $Matches["reason"]

        # Every ignore site is recorded, judged or not: the KNOWN_REDS
        # cross-check below needs to tell "parked but unjudged" apart from
        # "not parked at all".
        # Separators normalised to '/' once, so every path regex below needs one
        # form only (a Windows '\' inside a regex character class is an escape
        # waiting to be miscounted).
        $relEarly = $file.FullName.Substring($GeDir.Length).TrimStart([char]92, [char]47).Replace([char]92, [char]47)
        $targetEarly = ""
        if ($relEarly -match '^tests/([^/]+)\.rs$') { $targetEarly = $Matches[1] }
        elseif ($relEarly -match '^src/') { $targetEarly = "__lib__" }
        $nameEarly = ""
        $ke = $j + 1
        while ($ke -lt $lines.Count -and ($ke - $j) -le 16) {
            if ($lines[$ke] -match '^\s*(?:pub\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z0-9_]+)') {
                $nameEarly = $Matches[1]
                break
            }
            $ke++
        }
        if ($targetEarly -ne "" -and $nameEarly -ne "") {
            $allIgnored["$targetEarly::$nameEarly"] = $true
        }

        # Only reasons that name a defect are this gate's business.
        if ($reason -notmatch $DefectPattern) { $i = $j; continue }

        # Explicit, reviewable escape for a site the keyword heuristic pulls in
        # wrongly (a diagnostic dump whose reason happens to say "failing", an
        # opt-in sweep that names a "hang regime"). It must sit on its own line
        # directly above the attribute so it shows up in the diff, and it must
        # carry a reason -- same shape as the disconnection gate's
        # `// gate: allow-ungated because ...`.
        $escaped = $false
        for ($e = [Math]::Max(0, $i - 3); $e -lt $i; $e++) {
            if ($lines[$e] -match 'ignored-reds:\s*not-a-red\s*--\s*\S') { $escaped = $true }
        }
        if ($escaped) { $i = $j; continue }

        # The test function is the next `fn` after the attribute block.
        $name = ""
        $k = $j + 1
        while ($k -lt $lines.Count -and ($k - $j) -le 16) {
            if ($lines[$k] -match '^\s*(?:pub\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z0-9_]+)') {
                $name = $Matches[1]
                break
            }
            $k++
        }
        if ($name -eq "") {
            $refusals.Add(("{0}:{1} -- no 'fn' item within 16 lines of the #[ignore] attribute" -f $file.FullName, ($i + 1))) | Out-Null
            $i = $j
            continue
        }

        # Which cargo target owns it: tests/<stem>.rs -> --test <stem>; src/** -> --lib.
        $rel = $file.FullName.Substring($GeDir.Length).TrimStart([char]92, [char]47).Replace([char]92, [char]47)
        $target = ""
        if ($rel -match '^tests/([^/]+)\.rs$') {
            $target = $Matches[1]
        } elseif ($rel -match '^src/') {
            $target = "__lib__"
        } else {
            $refusals.Add(("{0}:{1} -- site is in '{2}', which is not a top-level tests/<name>.rs nor src/**" -f $file.FullName, ($i + 1), $rel)) | Out-Null
            $i = $j
            continue
        }

        # Full test path for `--exact`: enclosing `mod` blocks, and for a src
        # site the file's own module path too (tests/<stem>.rs IS its binary's
        # crate root, so there is no file prefix there).
        $prefix = $modAtLine[$k]
        if ($target -eq "__lib__") {
            $fileMod = $rel -replace '^src/', '' -replace '\.rs$', ''
            $fileMod = $fileMod -replace '/mod$', ''
            $fileMod = $fileMod -replace '/', '::'
            if ($fileMod -eq "lib") { $fileMod = "" }
            if ($fileMod -ne "") {
                if ($prefix -ne "") { $prefix = "$fileMod::$prefix" } else { $prefix = $fileMod }
            }
        }
        $fullPath = $name
        if ($prefix -ne "") { $fullPath = "$prefix::$name" }

        $sites.Add([pscustomobject]@{
            Target   = $target
            Test     = $name
            FullPath = $fullPath
            File     = $rel
            Line     = $i + 1
            Reason   = $reason
        }) | Out-Null

        $i = $j
    }
}

if ($refusals.Count -gt 0) {
    Write-Host ""
    Write-Host "PARSE_REFUSAL -- $($refusals.Count) #[ignore] site(s) could not be read:" -ForegroundColor Red
    foreach ($r in $refusals) { Write-Host "  $r" -ForegroundColor Red }
    Write-Host "  -> The gate does not skip sites it cannot parse. Fix the site or the parser." -ForegroundColor Red
    exit 4
}

Write-Host "Enumerated: $($sites.Count) ignored red(s) whose reason names a defect" -ForegroundColor Cyan

# -- Cross-check KNOWN_REDS.md ------------------------------------------------
#
# THE JURISDICTION SPLIT: red-gate.ps1 judges RUNNING allowlist entries (it sees
# them FAIL); this script judges PARKED ones (`#[ignore]`d, which can never
# fail). So an allowlist entry is fine either way EXCEPT when it is parked and
# this enumeration misses it -- then red-gate skips it as PARKED, this gate
# never runs it, and NOTHING would notice it getting fixed. That is the hole,
# and only that case is a refusal.
#
# A running entry is listed as RUNNING and left to red-gate. Saying "unjudged"
# about an entry another gate judges would be a false alarm, and a gate that
# cries wolf gets ignored -- which is how the hole opened in the first place.

$AllowlistPath = Join-Path $GeDir "KNOWN_REDS.md"
$unjudged = [System.Collections.Generic.List[string]]::new()
$running  = [System.Collections.Generic.List[string]]::new()
if (Test-Path $AllowlistPath) {
    $enumerated = @{}
    foreach ($e in $sites) { $enumerated["$($e.Target)::$($e.Test)"] = $true }

    $allowCount = 0
    foreach ($line in (Get-Content $AllowlistPath)) {
        $t = $line.Trim()
        if ($t -eq "" -or $t.StartsWith("#")) { continue }
        $idx = $t.IndexOf("  #")
        if ($idx -ge 0) { $t = $t.Substring(0, $idx).Trim() }
        if ($t -eq "" -or -not $t.Contains("::")) { continue }
        $allowCount++
        if ($enumerated.ContainsKey($t)) { continue }
        # $allIgnored holds every `#[ignore = "..."]` site in the crate, judged
        # or not, so this asks exactly "does the entry's test carry #[ignore]?".
        # (A bare `#[ignore]` would not be in it; tests/bare_ignore_gate.rs
        # already fails the build for one, so none exists.)
        if ($allIgnored.ContainsKey($t)) {
            $unjudged.Add(("{0} -- PARKED (#[ignore]d) but NOT enumerated: its reason names no defect keyword, or it carries a not-a-red marker" -f $t)) | Out-Null
        } else {
            $running.Add($t) | Out-Null
        }
    }
    Write-Host "Allowlist: $AllowlistPath ($allowCount entries)" -ForegroundColor Cyan
} else {
    Write-Host "Allowlist: $AllowlistPath NOT FOUND" -ForegroundColor Red
    $unjudged.Add("KNOWN_REDS.md is missing; the cross-check cannot run") | Out-Null
}

if ($running.Count -gt 0) {
    Write-Host ""
    Write-Host "RUNNING (judged by red-gate) -- $($running.Count) allowlist entry/entries are not #[ignore]d:" -ForegroundColor Cyan
    foreach ($k in ($running | Sort-Object)) { Write-Host "  $k" -ForegroundColor Cyan }
    Write-Host "  -> They FAIL in an ordinary run, so red-gate.ps1 observes them. Not this gate's business." -ForegroundColor Cyan
}

if ($unjudged.Count -gt 0) {
    Write-Host ""
    Write-Host "ALLOWLIST_UNJUDGED -- $($unjudged.Count) PARKED KNOWN_REDS.md entry/entries have no judge:" -ForegroundColor Red
    foreach ($u in $unjudged) { Write-Host "  $u" -ForegroundColor Red }
    Write-Host "  -> red-gate.ps1 skips a PARKED entry and this gate does not run it, so" -ForegroundColor Red
    Write-Host "     nothing would notice it being fixed. Make the #[ignore] reason name" -ForegroundColor Red
    Write-Host "     the defect, or drop the not-a-red marker." -ForegroundColor Red
    exit 5
}

$selected = $sites
if ($scopedTargets.Count -gt 0) {
    $selected = @($selected | Where-Object { $scopedTargets -contains $_.Target })
    Write-Host "Scope (targets): $($scopedTargets -join ', ')" -ForegroundColor Cyan
}
if ($onlyTests.Count -gt 0) {
    $selected = @($selected | Where-Object { $onlyTests -contains $_.Test })
    Write-Host "Scope (tests):   $($onlyTests -join ', ')" -ForegroundColor Cyan
}
$selected = @($selected)

if ($List) {
    Write-Host ""
    foreach ($s in ($selected | Sort-Object Target, Test)) {
        Write-Host ("  {0,-30} {1,-56} {2}:{3}" -f $s.Target, $s.FullPath, $s.File, $s.Line)
    }
    Write-Host ""
    Write-Host "Total listed: $($selected.Count)" -ForegroundColor Cyan
    exit 0
}

if ($selected.Count -eq 0) {
    Write-Host ""
    Write-Host "UNRESOLVED -- the scope selected 0 sites; nothing was run." -ForegroundColor Red
    Write-Host "  -> A gate that runs nothing must not report success. Check -Scoped/-Only." -ForegroundColor Red
    exit 2
}

# -- Run, one cargo invocation per target -------------------------------------

$env:CARGO_PROFILE_DEV_DEBUG  = "false"
$env:CARGO_PROFILE_TEST_DEBUG = "false"

$passing    = [System.Collections.Generic.List[string]]::new()
$stillRed   = [System.Collections.Generic.List[string]]::new()
$unresolved = [System.Collections.Generic.List[string]]::new()
$runErrors  = [System.Collections.Generic.List[string]]::new()

$targets = @($selected | Select-Object -ExpandProperty Target -Unique | Sort-Object)

foreach ($target in $targets) {
    $group = @($selected | Where-Object { $_.Target -eq $target })
    # `--exact` with the FULL test path: a bare fn name is a substring filter,
    # which would drag in every test whose name contains it.
    $names = @($group | Select-Object -ExpandProperty FullPath -Unique | Sort-Object)

    $cargoArgs = @("test", "-p", "geometry-engine", "-j", "4")
    if ($target -eq "__lib__") {
        $cargoArgs += "--lib"
    } else {
        $cargoArgs += @("--test", $target)
    }
    $cargoArgs += "--no-fail-fast"
    $cargoArgs += "--"
    $cargoArgs += "--ignored"
    $cargoArgs += "--exact"
    foreach ($n in $names) { $cargoArgs += $n }

    Write-Host ""
    Write-Host ("--- {0} ({1} test(s)) ---" -f $target, $names.Count) -ForegroundColor Yellow
    Write-Host "Running: cargo $($cargoArgs -join ' ')"

    # Unique capture files per invocation: two gate instances must never share
    # them (the lesson red-gate.ps1 records at its own $stamp).
    $stamp = "{0}-{1}-{2}" -f $PID, $target, (Get-Date -Format "yyyyMMdd-HHmmssfff")
    $stdoutFile = Join-Path $env:TEMP "ignored-reds-stdout-$stamp.txt"
    $stderrFile = Join-Path $env:TEMP "ignored-reds-stderr-$stamp.txt"

    $proc = Start-Process -FilePath "cargo" `
        -ArgumentList $cargoArgs `
        -WorkingDirectory $BackendDir `
        -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $stdoutFile `
        -RedirectStandardError  $stderrFile

    $stdoutLines = @()
    $stderrLines = @()
    if (Test-Path $stdoutFile) { $stdoutLines = @(Get-Content $stdoutFile -ErrorAction SilentlyContinue) }
    if (Test-Path $stderrFile) { $stderrLines = @(Get-Content $stderrFile -ErrorAction SilentlyContinue) }

    # Observed results, keyed by the FULL test path -- the same key the
    # `--exact` filter used, so an observation can only belong to the site that
    # asked for it.
    $observed = @{}
    $sawBlock = $false
    foreach ($line in $stdoutLines) {
        $t = $line.Trim()
        if ($t -match "^running \d+ test") { $sawBlock = $true }
        if ($t -match '^test\s+(\S+)\s+\.\.\.\s+(ok|FAILED|ignored)\b') {
            $full = $Matches[1]
            $res  = $Matches[2]
            if (-not $observed.ContainsKey($full)) {
                $observed[$full] = [System.Collections.Generic.List[string]]::new()
            }
            $observed[$full].Add("$full=$res") | Out-Null
        }
    }

    if (-not $sawBlock) {
        $runErrors.Add(("{0} -- cargo produced no test block (exit {1}); see {2} / {3}" -f $target, $proc.ExitCode, $stdoutFile, $stderrFile)) | Out-Null
        if ($stderrLines) { Write-Host ($stderrLines -join [Environment]::NewLine) -ForegroundColor Red }
        continue
    }

    foreach ($s in $group) {
        $key = "$($s.Target)::$($s.FullPath)"
        if (-not $observed.ContainsKey($s.FullPath)) {
            $unresolved.Add(("{0}  ({1}:{2})  -- no 'test ... ok/FAILED' line; the --exact filter matched nothing (stale path?) or the runner died" -f $key, $s.File, $s.Line)) | Out-Null
            continue
        }
        $obs = $observed[$s.FullPath]
        $anyOk = $false
        $anyFailed = $false
        foreach ($o in $obs) {
            if ($o.EndsWith("=ok")) { $anyOk = $true }
            if ($o.EndsWith("=FAILED")) { $anyFailed = $true }
        }
        if ($anyOk) {
            $passing.Add(("{0}  ({1}:{2})  observed: {3}" -f $key, $s.File, $s.Line, ($obs -join " "))) | Out-Null
        } elseif ($anyFailed) {
            $stillRed.Add($key) | Out-Null
        } else {
            $unresolved.Add(("{0}  ({1}:{2})  -- only 'ignored' observed: {3}" -f $key, $s.File, $s.Line, ($obs -join " "))) | Out-Null
        }
    }

    Write-Host "capture files: $stdoutFile / $stderrFile"
}

# -- Verdict ------------------------------------------------------------------

Write-Host ""
Write-Host "=== ignored-reds results ===" -ForegroundColor Cyan
Write-Host "still red:  $($stillRed.Count)"
Write-Host "PASSING:    $($passing.Count)"
Write-Host "unresolved: $($unresolved.Count)"
Write-Host "run errors: $($runErrors.Count)"

foreach ($k in ($stillRed | Sort-Object)) { Write-Host "  [still red] $k" -ForegroundColor DarkGray }

if ($runErrors.Count -gt 0) {
    Write-Host ""
    Write-Host "RUN_ERROR -- $($runErrors.Count) target(s) never produced a test block:" -ForegroundColor Red
    foreach ($k in $runErrors) { Write-Host "  $k" -ForegroundColor Red }
    exit 3
}

if ($passing.Count -gt 0) {
    Write-Host ""
    Write-Host "PASSING_IGNORED_RED -- $($passing.Count) parked red now PASSES:" -ForegroundColor Magenta
    foreach ($k in ($passing | Sort-Object)) { Write-Host "  $k" -ForegroundColor Magenta }
    Write-Host "  -> Remove its #[ignore] (and its KNOWN_REDS.md line, if pinned)." -ForegroundColor Magenta
    Write-Host "     A red that is fixed and still parked hides the fix from every gate." -ForegroundColor Magenta
    exit 1
}

if ($unresolved.Count -gt 0) {
    Write-Host ""
    Write-Host "UNRESOLVED -- $($unresolved.Count) enumerated test(s) produced no verdict:" -ForegroundColor Red
    foreach ($k in ($unresolved | Sort-Object)) { Write-Host "  $k" -ForegroundColor Red }
    Write-Host "  -> A missing observation is not a pass. Re-check the name or the runner." -ForegroundColor Red
    exit 2
}

Write-Host ""
Write-Host "GATE PASSED - every enumerated ignored red is still red." -ForegroundColor Green
exit 0
