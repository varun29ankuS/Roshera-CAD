#!/usr/bin/env bash
#
# Roshera kernel self-test runner.
#
# Runs every demo example in geometry-engine/examples/, captures stdout/stderr
# and exit status per demo, and writes target/demos/run_status.json with a
# machine-readable summary. Combined with the per-STL stats already emitted by
# common::tess_and_write into target/demos/manifest.json, the pair gives a
# one-call "is the kernel healthy?" answer:
#
#   manifest.json   — what STLs exist, with verts/tris/tess_ms per file
#   run_status.json — which demos ran, exit code, duration, log path
#
# Usage:
#   ./scripts/run_kernel_demos.sh           # run all demos in release mode
#   ./scripts/run_kernel_demos.sh quick     # run only quick_demo
#   ./scripts/run_kernel_demos.sh --debug   # debug build (faster compile)
#
# Exit code: 0 if every demo exited 0, 1 otherwise.
#
# Why a script and not a Rust harness binary: each demo's `main()` calls
# `assert!` for regression bounds. Running them as separate processes means
# one failing demo does not crash the entire run. A single Rust binary
# linking all demos would lose that isolation.
#
# This script does NOT invoke cargo build/run on its own; the caller controls
# build cadence by running `cargo build --release --examples` first or by
# letting the run-on-demand path below recompile if needed. Set
# ROSHERA_DEMO_NO_BUILD=1 to skip the implicit `cargo build` and require a
# pre-built binary in target/release/examples/.

set -u
set -o pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 2

PROFILE="release"
PROFILE_DIR="release"
SELECTOR=""

for arg in "$@"; do
    case "$arg" in
        --debug)
            PROFILE="dev"
            PROFILE_DIR="debug"
            ;;
        --help|-h)
            sed -n '2,30p' "$0"
            exit 0
            ;;
        *)
            SELECTOR="$arg"
            ;;
    esac
done

DEMO_DIR="geometry-engine/examples"
OUT_ROOT="target/demos"
LOGS_DIR="$OUT_ROOT/runs"
STATUS_JSON="$OUT_ROOT/run_status.json"

mkdir -p "$LOGS_DIR"

# Discover demos. Anything starting with `demo_` or named `quick_demo`
# qualifies; benchmarks and stress tests are explicitly excluded so the
# self-test stays fast.
mapfile -t DEMOS < <(
    find "$DEMO_DIR" -maxdepth 1 -type f -name '*.rs' -printf '%f\n' \
        | sed 's/\.rs$//' \
        | grep -E '^(demo_|quick_demo$)' \
        | sort
)

if [ -n "$SELECTOR" ]; then
    FILTERED=()
    for d in "${DEMOS[@]}"; do
        if [[ "$d" == *"$SELECTOR"* ]]; then
            FILTERED+=("$d")
        fi
    done
    DEMOS=("${FILTERED[@]}")
fi

if [ "${#DEMOS[@]}" -eq 0 ]; then
    echo "no demos matched selector '$SELECTOR'" >&2
    exit 2
fi

echo "Roshera kernel self-test"
echo "  profile : $PROFILE"
echo "  demos   : ${#DEMOS[@]}"
echo "  log dir : $LOGS_DIR"
echo

# Build all examples once up front (unless caller opted out). Building per-demo
# is wasteful — cargo will reuse compilations across binaries with shared deps.
if [ -z "${ROSHERA_DEMO_NO_BUILD:-}" ]; then
    echo "  building examples (one-time)…"
    if [ "$PROFILE" = "release" ]; then
        cargo build -p geometry-engine --release --examples
    else
        cargo build -p geometry-engine --examples
    fi
    echo
fi

# Per-demo run loop. JSON is assembled by hand (jq not always available on
# Windows + git-bash); keys are kept simple to keep the assembler trivial.
{
    printf '{\n'
    printf '  "started_at": "%s",\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '  "profile": "%s",\n' "$PROFILE"
    printf '  "results": [\n'
} > "$STATUS_JSON"

OVERALL_OK=1
FIRST=1

for demo in "${DEMOS[@]}"; do
    LOG="$LOGS_DIR/$demo.log"
    echo "  ▶ $demo"

    BIN="target/$PROFILE_DIR/examples/$demo"
    # On Windows, examples have .exe suffix.
    if [ ! -x "$BIN" ] && [ -x "$BIN.exe" ]; then
        BIN="$BIN.exe"
    fi

    if [ ! -x "$BIN" ]; then
        echo "      ✗ binary not found at $BIN (build skipped or failed)"
        OVERALL_OK=0
        STATUS_LINE='{"demo":"'"$demo"'","ok":false,"exit":-1,"duration_ms":0,"log":"'"$LOG"'","error":"binary missing"}'
    else
        # Time in seconds with millisecond precision via $SECONDS would round
        # to integer; use date +%s%N if available (GNU), else fall back to
        # plain seconds. Windows git-bash supports %N.
        START_NS=$(date +%s%N 2>/dev/null || echo "")
        "$BIN" >"$LOG" 2>&1
        EXIT=$?
        END_NS=$(date +%s%N 2>/dev/null || echo "")

        if [ -n "$START_NS" ] && [ -n "$END_NS" ]; then
            DUR_MS=$(( (END_NS - START_NS) / 1000000 ))
        else
            DUR_MS=0
        fi

        if [ "$EXIT" -eq 0 ]; then
            echo "      ✓ exit 0  (${DUR_MS} ms)"
        else
            echo "      ✗ exit $EXIT  (${DUR_MS} ms)  — see $LOG"
            OVERALL_OK=0
        fi

        STATUS_LINE='{"demo":"'"$demo"'","ok":'"$([ "$EXIT" -eq 0 ] && echo true || echo false)"',"exit":'"$EXIT"',"duration_ms":'"$DUR_MS"',"log":"'"$LOG"'"}'
    fi

    if [ "$FIRST" -eq 1 ]; then
        FIRST=0
        printf '    %s' "$STATUS_LINE" >> "$STATUS_JSON"
    else
        printf ',\n    %s' "$STATUS_LINE" >> "$STATUS_JSON"
    fi
done

{
    printf '\n  ],\n'
    printf '  "ok": %s,\n' "$([ "$OVERALL_OK" -eq 1 ] && echo true || echo false)"
    printf '  "finished_at": "%s"\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '}\n'
} >> "$STATUS_JSON"

echo
echo "  status   : $STATUS_JSON"
echo "  manifest : $OUT_ROOT/manifest.json"
echo

if [ "$OVERALL_OK" -eq 1 ]; then
    echo "  ✓ all demos passed"
    exit 0
else
    echo "  ✗ at least one demo failed — inspect $LOGS_DIR/<demo>.log for details"
    exit 1
fi
