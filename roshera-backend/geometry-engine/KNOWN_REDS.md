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

# Family: extrude wall orientation (1 test) -- diag: burndown-diag-drawing-tess.md#family-45
fillet_chamfer_dihedral_matrix::matrix_helpers_build_valid_prisms_with_expected_vertical_edge_counts  # diag: burndown-diag-drawing-tess.md#family-45
