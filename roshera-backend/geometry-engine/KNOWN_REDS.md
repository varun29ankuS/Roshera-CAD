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

# Family: CF-gamma G1 mixed-kind corner (5 tests) -- diag: burndown-diag-cf.md#sub-group-a #sub-group-b #sub-group-c
cf_gamma_g1_mixed_kind_corner::g1_1c2f_fillet_first_emits_three_subpatch_cap  # diag: burndown-diag-cf.md#sub-group-a
cf_gamma_g1_mixed_kind_corner::g1_2c1f_chamfer_first_emits_three_subpatch_cap  # diag: burndown-diag-cf.md#sub-group-b
cf_gamma_g1_mixed_kind_corner::g1_2c1f_fillet_first_emits_three_subpatch_cap  # diag: burndown-diag-cf.md#sub-group-b
cf_gamma_g1_mixed_kind_corner::g1_1c2f_chamfer_first_emits_three_subpatch_cap  # diag: burndown-diag-cf.md#sub-group-c
cf_gamma_g1_mixed_kind_corner::c0_default_still_produces_planar_cap  # diag: burndown-diag-cf.md#sub-group-c

# Family: CF-gamma G1 seam audit (2 tests) -- diag: burndown-diag-cf.md#sub-group-a and #sub-group-c
cf_gamma_g1_mixed_kind_seam_audit::audit_passes_after_cf_gamma_g1_synthesis_1c2f  # diag: burndown-diag-cf.md#sub-group-c
cf_gamma_g1_mixed_kind_seam_audit::audit_passes_after_cf_gamma_g1_synthesis_2c1f  # diag: burndown-diag-cf.md#sub-group-a

# Family: CF-gamma G1 replay determinism (1 test) -- diag: burndown-diag-cf.md#sub-group-c
cf_gamma_g1_replay_determinism::cf_gamma_g1_1c2f_chamfer_first_subpatch_cps_byte_equal_across_ten_runs  # diag: burndown-diag-cf.md#sub-group-c


# Family: tessellation CDT (2 tests) -- diag: burndown-diag-drawing-tess.md#family-41
tess_curved_cdt::high_curvature_nurbs_no_skinny_triangles  # diag: burndown-diag-drawing-tess.md#family-41
tess_curved_cdt::chord_tolerance_actually_enforced_after_refinement  # diag: burndown-diag-drawing-tess.md#family-41

# Family: cone corner boolean (1 test) -- diag: burndown-diag-drawing-tess.md#family-42
diag_cone_radial::cone_corner_gate  # diag: burndown-diag-drawing-tess.md#family-42

# Family: gap finder chamfer (1 test) -- diag: burndown-diag-drawing-tess.md#family-43
gap_finder_fuzz::gap_finder_smoke  # diag: burndown-diag-drawing-tess.md#family-43

# Family: extrude wall orientation (1 test) -- diag: burndown-diag-drawing-tess.md#family-45
fillet_chamfer_dihedral_matrix::matrix_helpers_build_valid_prisms_with_expected_vertical_edge_counts  # diag: burndown-diag-drawing-tess.md#family-45
