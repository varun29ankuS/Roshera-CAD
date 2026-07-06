# geometry-engine KNOWN_REDS -- pre-existing red integration tests
#
# RATCHET RULE (NON-NEGOTIABLE)
# Entries in this file may only be REMOVED (when a test goes green and stays green).
# They may NEVER be added without a corresponding diagnosis document in
# .superpowers/sdd/burndown-diag-<family>.md naming the breaking commit and root cause.
# A gate script enforces this: any new failure not listed here exits nonzero (NEW_RED);
# any listed entry that now passes exits nonzero (RATCHET_VIOLATION -- remove it).
#
# Entry format (one per line; all comment lines start with #):
#   <binary>::<test_name>  # diag: <doc>#<section>
#
# Gate script: roshera-backend/scripts/red-gate.ps1


# ALLOWLIST EMPTY as of 2026-07-07 (red-burndown campaign complete): all 30
# pre-existing reds fixed at root. Any future entry requires a diagnosis doc
# per the ratchet rule above.
