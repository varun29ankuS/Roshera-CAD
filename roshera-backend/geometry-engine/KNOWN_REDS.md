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

# Family: tessellation CDT (2 tests) -- diag: burndown-diag-drawing-tess.md#family-41
tess_curved_cdt::high_curvature_nurbs_no_skinny_triangles  # diag: burndown-diag-drawing-tess.md#family-41
tess_curved_cdt::chord_tolerance_actually_enforced_after_refinement  # diag: burndown-diag-drawing-tess.md#family-41

# Family: gap finder chamfer (1 test) -- diag: burndown-diag-drawing-tess.md#family-43
gap_finder_fuzz::gap_finder_smoke  # diag: burndown-diag-drawing-tess.md#family-43

# Family: extrude wall orientation (1 test) -- diag: burndown-diag-drawing-tess.md#family-45
fillet_chamfer_dihedral_matrix::matrix_helpers_build_valid_prisms_with_expected_vertical_edge_counts  # diag: burndown-diag-drawing-tess.md#family-45
