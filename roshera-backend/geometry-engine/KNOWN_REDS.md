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

# Family: coaxial-bore boolean (4 tests) -- diag: burndown-diag-boolean.md#sub-group-a
agent_build_eval::extrude_boss_coaxial_bore_keeps_wall  # diag: burndown-diag-boolean.md#sub-group-a
agent_build_eval::bearing_housing_coaxial_bore_is_sound  # diag: burndown-diag-boolean.md#sub-group-a
agent_build_eval::eval_bossed_plate_with_coaxial_bore  # diag: burndown-diag-boolean.md#sub-group-a
agent_build_eval::gen_flanged_housing  # diag: burndown-diag-boolean.md#sub-group-a

# Family: boolean bracket / F4 sever (1 test) -- diag: burndown-diag-boolean.md#sub-group-b
boolean_bracket_robustness::f4_oversized_bore_severs_into_two_bodies  # diag: burndown-diag-boolean.md#sub-group-b

# Family: boolean parity proptests (2 tests) -- diag: burndown-diag-boolean.md#sub-group-c
boolean_proptest::union_commutativity_parity  # diag: burndown-diag-boolean.md#sub-group-c
boolean_proptest::intersection_commutativity_parity  # diag: burndown-diag-boolean.md#sub-group-c

# Family: hexagon commutativity (1 test) -- diag: burndown-diag-boolean.md#sub-group-d
polyline_cut_harness::union_commutative_polyline_hexagons  # diag: burndown-diag-boolean.md#sub-group-d

# Family: CF-beta mixed-kind corner (3 tests) -- diag: burndown-diag-cf.md#sub-group-a and #sub-group-b
cf_beta_mixed_kind_corner::box_corner_two_fillets_then_chamfer_synthesises_mixed_cap  # diag: burndown-diag-cf.md#sub-group-a
cf_beta_mixed_kind_corner::box_corner_mixed_kind_intermediate_state_skips_watertight_validation  # diag: burndown-diag-cf.md#sub-group-a
cf_beta_mixed_kind_corner::box_corner_mixed_kind_topology_hash_order_invariant  # diag: burndown-diag-cf.md#sub-group-b

# Family: CF-beta property (1 test) -- diag: burndown-diag-cf.md#sub-group-a
cf_beta_property::prop_mixed_kind_corner_topology_order_invariant  # diag: burndown-diag-cf.md#sub-group-a

# Family: CF-beta replay determinism (2 tests) -- diag: burndown-diag-cf.md#sub-group-a and #sub-group-b
cf_beta_replay_determinism::cf_beta_chamfer_first_ordering_is_deterministic_across_ten_runs  # diag: burndown-diag-cf.md#sub-group-a
cf_beta_replay_determinism::cf_beta_fillet_first_ordering_is_deterministic_across_ten_runs  # diag: burndown-diag-cf.md#sub-group-b

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

# Family: drawing dash pair (2 tests) -- diag: burndown-diag-drawing-tess.md#family-3
drawing_centerlines::bored_plate_drawing_carries_recoverable_centerlines  # diag: burndown-diag-drawing-tess.md#family-3
drawing_hlr::hlr_drawing_has_dashed_hidden_edges_wireframe_does_not  # diag: burndown-diag-drawing-tess.md#family-3

# Family: tessellation CDT (2 tests) -- diag: burndown-diag-drawing-tess.md#family-41
tess_curved_cdt::high_curvature_nurbs_no_skinny_triangles  # diag: burndown-diag-drawing-tess.md#family-41
tess_curved_cdt::chord_tolerance_actually_enforced_after_refinement  # diag: burndown-diag-drawing-tess.md#family-41

# Family: cone corner boolean (1 test) -- diag: burndown-diag-drawing-tess.md#family-42
diag_cone_radial::cone_corner_gate  # diag: burndown-diag-drawing-tess.md#family-42

# Family: gap finder chamfer (1 test) -- diag: burndown-diag-drawing-tess.md#family-43
gap_finder_fuzz::gap_finder_smoke  # diag: burndown-diag-drawing-tess.md#family-43

# Family: torus B1 false positive (1 test) -- diag: burndown-diag-drawing-tess.md#family-44
operation_composition_invariants::torus_mass_props_rigid_invariant  # diag: burndown-diag-drawing-tess.md#family-44

# Family: extrude wall orientation (1 test) -- diag: burndown-diag-drawing-tess.md#family-45
fillet_chamfer_dihedral_matrix::matrix_helpers_build_valid_prisms_with_expected_vertical_edge_counts  # diag: burndown-diag-drawing-tess.md#family-45
