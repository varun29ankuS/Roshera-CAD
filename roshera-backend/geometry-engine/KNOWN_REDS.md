# geometry-engine KNOWN_REDS -- pre-existing red integration tests
#
# RATCHET RULE (NON-NEGOTIABLE)
# Entries in this file may only be REMOVED (when a test goes green and stays green).
# They may NEVER be added without a corresponding diagnosis document naming the
# breaking commit and root cause. Diagnosis docs live in
# geometry-engine/docs/burndown-diag-<family>.md -- IN VERSION CONTROL. The three
# 2026-07-07 campaign diags (burndown-diag-cf / -boolean / -drawing-tess) were
# written to .superpowers/sdd/, which .gitignore excludes; an entry pointing there
# points outside the repo, so new diags go in geometry-engine/docs/ instead.
# When a diagnosis has NO breaking commit -- a pre-existing defect that only
# became visible when a gate started asserting -- the doc says so explicitly
# rather than naming an innocent commit.
# A gate script enforces this: any new failure not listed here exits nonzero (NEW_RED);
# any listed entry that now passes exits nonzero (RATCHET_VIOLATION -- remove it).
#
# Entry format (one per line; all comment lines start with #):
#   <binary>::<test_name>  # diag: <doc>#<section>
#
# PARKED ENTRIES
# An entry whose test is #[ignore]d is PARKED: red-gate.ps1 reports it as PARKED
# and does not treat it as a violation, because an ignored test is never given
# the chance to FAIL and its silence is therefore not evidence that it passes.
# Parked entries are judged by roshera-backend/scripts/ignored-reds.ps1, which
# runs them with --ignored and exits nonzero when one PASSES. A parked red that
# is fixed must lose BOTH its #[ignore] and its line here.
#
# Gate scripts: roshera-backend/scripts/red-gate.ps1 (failures)
#               roshera-backend/scripts/ignored-reds.ps1 (parked reds)


# The 2026-07-07 red-burndown campaign emptied this file: all 30 pre-existing
# reds were fixed at root. The three entries below are the first since, added
# 2026-09-06 when the loft/blend stress harnesses stopped discarding their own
# verdict counts and the defects they had been printing (and passing on) since
# they were written became red tests. They are one defect family, diagnosed in
# geometry-engine/docs/burndown-diag-loft-family.md. Any future entry requires a
# diagnosis doc per the ratchet rule above.

blend_weld_stress::broaden_loft_varied_sections  # diag: geometry-engine/docs/burndown-diag-loft-family.md -- PARKED (#[ignore]d): 2 of 4 lofts BUILD and certify UNSOUND
op_stress_round3::loft_four_sections  # diag: geometry-engine/docs/burndown-diag-loft-family.md -- PARKED (#[ignore]d): builds, scoped-valid, welded mesh NOT oriented, cert unsound
op_stress_round3::loft_square_circle_square_dissimilar  # diag: geometry-engine/docs/burndown-diag-loft-family.md -- PARKED (#[ignore]d): builds, scoped-valid, welded mesh NOT oriented, cert unsound
