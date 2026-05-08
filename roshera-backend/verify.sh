#!/usr/bin/env bash
# verify.sh — one-shot workspace verification.
#
# What this does:
#   1. cargo check --workspace --tests
#   2. cargo test  --workspace --lib --no-fail-fast
#   3. cargo test  -p geometry-engine --tests --no-fail-fast
#   4. cd ../roshera-app && tsc -b
#
# Why not the full `cargo test --workspace`:
#   - bench targets need the `mock-providers` feature
#   - some integration suites need the `fdb` feature and silently skip
#
# Exit code: non-zero on any failure.

set -u
set -o pipefail

BACKEND_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "$BACKEND_DIR/../roshera-app" && pwd)"

FAILURES=()

run() {
    local name=$1; shift
    printf '\n=== %s ===\n' "$name"
    if ! "$@"; then
        FAILURES+=("$name")
    fi
}

cd "$BACKEND_DIR"
run "cargo check --workspace --tests" cargo check --workspace --tests --quiet
run "cargo test --workspace --lib"     cargo test --workspace --lib --no-fail-fast --quiet
run "cargo test -p geometry-engine --tests" cargo test -p geometry-engine --tests --no-fail-fast --quiet

if [ -d "$APP_DIR" ]; then
    cd "$APP_DIR"
    run "frontend tsc -b" npx --no-install tsc -b
    cd "$BACKEND_DIR"
fi

echo ""
if [ ${#FAILURES[@]} -eq 0 ]; then
    echo "verify.sh: OK"
    exit 0
else
    echo "verify.sh: FAILED"
    for f in "${FAILURES[@]}"; do echo "  - $f"; done
    exit 1
fi
