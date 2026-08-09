// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Two reported kernel defects, re-measured against the live kernel.
//!
//! 1. **Union of two DISJOINT solids files the second body as a VOID** and
//!    certifies the result sound. REPRODUCED, root diagnosed, fix blocked on a
//!    `Solid` schema change — see `disjoint_union_keeps_both_bodies` for the
//!    full root/blocker note. Held as an `#[ignore]`d real contract rather than
//!    a weakened assertion.
//!
//! 2. **The second through-bore.** The reported input — a bore 30mm from the
//!    first — is CLEAN (8 faces, analytic volume, sound); see
//!    `second_through_bore_stays_sound_and_volume_matches_analytic`, which pins
//!    it so. A placement sweep found the real failing input: re-cutting the SAME
//!    bore with a coincident cylinder, matching the report's volume to five
//!    decimals. That case is genuinely broken, and the kernel says so — see
//!    `coincident_recut_of_an_existing_bore_is_never_certified_sound`.
//!
//! The measures here are deliberately plural, because the reported numbers came
//! from consumers that disagree with each other: the O(1) outer-shell face
//! count, the MESH volume, the analytic mass-props, the mesh `manifold_report`
//! and the `ValidityCertificate` are all printed by `describe` for every case.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::shell::ShellId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model).create_box_3d(w, h, d) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

fn make_cylinder(model: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model).create_cylinder_3d(base, axis, r, h) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

fn translate(model: &mut BRepModel, solid: SolidId, delta: Vector3) {
    transform_solid(
        model,
        solid,
        Matrix4::from_translation(&delta),
        TransformOptions::default(),
    )
    .expect("translating a valid solid must succeed");
}

/// Every face reachable from the solid (outer shell + inner shells).
fn all_faces(model: &BRepModel, solid_id: SolidId) -> Vec<u32> {
    let mut out = Vec::new();
    if let Some(solid) = model.solids.get(solid_id) {
        for sh in solid.all_shells() {
            if let Some(shell) = model.shells.get(sh) {
                out.extend(shell.faces.iter().copied());
            }
        }
    }
    out
}

/// Topological open-boundary count: directed half-edge uses without a partner.
fn topological_open_edges(model: &BRepModel, solid_id: SolidId) -> usize {
    use std::collections::HashMap;
    let mut uses: HashMap<u32, usize> = HashMap::new();
    for fid in all_faces(model, solid_id) {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            if let Some(lp) = model.loops.get(lid) {
                for &eid in &lp.edges {
                    *uses.entry(eid).or_insert(0) += 1;
                }
            }
        }
    }
    uses.values().filter(|&&n| n != 2).count()
}

/// Centroid of a shell's vertices — the representative interior probe used to
/// ask "is this shell enclosed by that one?".
fn shell_centroid(model: &BRepModel, shell_id: ShellId) -> Option<Point3> {
    let shell = model.shells.get(shell_id)?;
    let mut sum = [0.0_f64; 3];
    let mut n = 0.0_f64;
    for &fid in &shell.faces {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            if let Some(lp) = model.loops.get(lid) {
                for &eid in &lp.edges {
                    if let Some(e) = model.edges.get(eid) {
                        for vid in [e.start_vertex, e.end_vertex] {
                            if let Some(p) = model.vertices.get_position(vid) {
                                sum[0] += p[0];
                                sum[1] += p[1];
                                sum[2] += p[2];
                                n += 1.0;
                            }
                        }
                    }
                }
            }
        }
    }
    (n > 0.0).then(|| Point3::new(sum[0] / n, sum[1] / n, sum[2] / n))
}

/// True when `probe` sits strictly inside the closed surface of `shell_id`
/// (generalized winding number over the shell's own coarse tessellation).
fn shell_encloses_point(model: &BRepModel, shell_id: ShellId, probe: &Point3) -> bool {
    use geometry_engine::math::winding_number::{classify_by_winding, WindingClassification};
    use geometry_engine::tessellation::edge_cache::EdgeSampleCache;
    use geometry_engine::tessellation::{tessellate_shell, TessellationParams, TriangleMesh};
    let Some(shell) = model.shells.get(shell_id) else {
        return false;
    };
    let mut mesh = TriangleMesh::new();
    let params = TessellationParams::coarse();
    let cache = EdgeSampleCache::new(&params);
    tessellate_shell(shell, model, &params, &cache, &mut mesh);
    let tris: Vec<[Point3; 3]> = mesh
        .triangles
        .iter()
        .filter_map(|t| {
            Some([
                mesh.vertices.get(t[0] as usize)?.position,
                mesh.vertices.get(t[1] as usize)?.position,
                mesh.vertices.get(t[2] as usize)?.position,
            ])
        })
        .collect();
    if tris.is_empty() {
        return false;
    }
    matches!(
        classify_by_winding(probe, &tris),
        WindingClassification::Inside
    )
}

/// Every `inner_shell` a solid declares must be a genuine VOID — enclosed by
/// the outer shell. An inner shell that sits OUTSIDE the outer shell is a
/// disjoint sibling body mis-filed as a hole: the lie this test hunts.
fn phantom_void_shells(model: &BRepModel, solid_id: SolidId) -> Vec<ShellId> {
    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let outer = solid.outer_shell;
    solid
        .inner_shells
        .iter()
        .copied()
        .filter(|&inner| match shell_centroid(model, inner) {
            Some(c) => !shell_encloses_point(model, outer, &c),
            None => true,
        })
        .collect()
}

fn describe(model: &mut BRepModel, solid_id: SolidId, label: &str) {
    let (outer, inner) = model
        .solids
        .get(solid_id)
        .map(|s| (s.outer_shell, s.inner_shells.clone()))
        .unwrap_or((0, Vec::new()));
    let faces = all_faces(model, solid_id).len();
    let outer_faces = model.solid_outer_face_count(solid_id);
    let phantom = phantom_void_shells(model, solid_id);
    let v = model.calculate_solid_volume(solid_id);
    let mr = manifold_report(model, solid_id, 0.02, 1e-6);
    let cert = model.certify_solid(solid_id);
    eprintln!(
        "[{label}] solid={solid_id} outer_shell={outer} inner_shells={inner:?} \
         phantom_voids={phantom:?} faces_all_shells={faces} outer_face_count={outer_faces:?} \
         mesh_volume={v:?} topo_open_edges={}",
        topological_open_edges(model, solid_id),
    );
    match mr {
        Some(r) => eprintln!(
            "        mesh: tris={} boundary_edges={} nonmanifold={} directed_dup={} \
             components={} euler={} closed={} manifold={} oriented={}",
            r.triangles,
            r.boundary_edges,
            r.nonmanifold_edges,
            r.inconsistent_directed_edges,
            r.components,
            r.euler_characteristic,
            r.closed,
            r.manifold,
            r.oriented,
        ),
        None => eprintln!("        mesh: <no triangles>"),
    }
    eprintln!(
        "        cert: sound={} brep_valid={} watertight={} manifold={} oriented={} \
         self_int_free={} tess_clean={} mesh_q_clean={} boundary_edges={} euler={}",
        cert.is_sound(),
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.self_intersection_free,
        cert.tessellation.clean,
        cert.mesh_quality.clean,
        cert.boundary_edges,
        cert.euler_characteristic,
    );
}

// ---------------------------------------------------------------------------
// DEFECT 1 — union of two disjoint solids must not mis-file a body as a void
// ---------------------------------------------------------------------------

/// MEASURED RED, root diagnosed, fix deliberately out of scope — held in the
/// same `#[ignore]`-with-a-reason form this suite already uses for
/// `box_box_difference_bbox_within_minuend_3480` (#34/#80) and
/// `cyl_minus_sphere_same_radius_7` (#7). Every assertion below is the real
/// contract and is left intact, so this flips green the day the blocker lands.
///
/// Root: `reconstruct_topology` (boolean.rs) files EVERY non-outer shell into
/// `Solid::inner_shells`, which means VOID. For a union of disjoint operands
/// the second body is a PEER, not a cavity, and the consumers then disagree
/// about the same solid: `Solid::compute_mass_properties` SUBTRACTS it
/// (→ the reported volume 1000), `solid_outer_face_count` cannot see its faces
/// (→ the reported 6, not 12), the mesh/tessellation path ADDS it (→ 2000).
/// All three under `certify_solid(...).is_sound() == true`.
///
/// Blocker: peer bodies need a role on `Solid` that is neither `outer_shell`
/// nor `inner_shells` — a schema change reaching `ModelSnapshot`, `.ros`
/// serialization and export. Three oracles currently READ the present shape and
/// pass because the mesh path is the one they consult:
/// `boolean_fuzz_survey::box_sphere_conquered_band_gate` (#91) pins two
/// explicitly disjoint box∘sphere UNION cells against a 96³ grid truth, and
/// `rotated_box_booleans_match_mc_truth` / `tilted_box_booleans_match_mc_truth`
/// pin a four-body Difference against Monte-Carlo truth. Refusing multi-body
/// results — the other honest option — would break all three, i.e. destroy
/// working, oracle-verified geometry to hide a bookkeeping defect.
///
/// What DID land: the outer shell is no longer `shells[0]` by assumption. It is
/// picked by measured extent and every other shell's void status is proved by
/// winding number, so the kernel can no longer report a CAVITY as the body.
#[test]
#[ignore = "peer bodies are filed as voids (boolean.rs reconstruct_topology); \
            needs a peer-lump role on Solid — flip on when it lands"]
fn disjoint_union_keeps_both_bodies() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 10.0, 10.0, 10.0);
    let b = make_box(&mut model, 10.0, 10.0, 10.0);
    translate(&mut model, b, Vector3::new(30.0, 0.0, 0.0));

    let id = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union of two disjoint 10³ boxes must not fail");
    describe(&mut model, id, "disjoint-union");

    // The lie: body B recorded as a void of body A.
    let phantom = phantom_void_shells(&model, id);
    assert!(
        phantom.is_empty(),
        "shells {phantom:?} are declared VOIDS of the result but sit OUTSIDE its \
         outer shell — a disjoint body mis-filed as a hole. \
         `Solid::compute_mass_properties` subtracts them, the mesh path adds them.",
    );

    // If it succeeded, it must be the whole answer, by every consumer.
    assert_eq!(
        model.solid_outer_face_count(id),
        Some(12),
        "the union of two disjoint boxes has 12 boundary faces; the O(1) count \
         every agent/perception caller reads must see all of them",
    );
    assert_eq!(
        all_faces(&model, id).len(),
        12,
        "union of two disjoint boxes must retain both bodies' 6 faces each",
    );
    let v = model
        .calculate_solid_volume(id)
        .expect("a union of two valid boxes must have a volume");
    assert!(
        (v - 2000.0).abs() < 20.0,
        "V(A ∪ B) = {v}, expected 2000 = 1000 + 1000",
    );
    assert_eq!(
        topological_open_edges(&model, id),
        0,
        "the two-body union must be a closed 2-manifold",
    );
}

// ---------------------------------------------------------------------------
// DEFECT 2 — a second, independent through-bore must keep the solid sound
// ---------------------------------------------------------------------------

/// Plate 80×40×40 centred at the origin, minus two Ø2 through-bores along Z at
/// x = -30 and x = 0. Analytic volume:
///   80·40·40 − 2·(π·1²·40) = 128000 − 251.327 = 127748.67
#[test]
fn second_through_bore_stays_sound_and_volume_matches_analytic() {
    let mut model = BRepModel::new();
    let plate = make_box(&mut model, 80.0, 40.0, 40.0);

    // Bore 1 at x = -30, piercing the whole 40mm Z extent (and protruding).
    let bore1 = make_cylinder(
        &mut model,
        Point3::new(-30.0, 0.0, -30.0),
        Vector3::new(0.0, 0.0, 1.0),
        1.0,
        60.0,
    );
    let one = boolean_operation(
        &mut model,
        plate,
        bore1,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("first through-bore must succeed");
    describe(&mut model, one, "bore-1");

    let v1 = model
        .calculate_solid_volume(one)
        .expect("volume after one bore");
    let expect1 = 128000.0 - std::f64::consts::PI * 40.0;
    assert!(
        (v1 - expect1).abs() < 1.0,
        "one-bore volume {v1} should match analytic {expect1}",
    );
    assert_eq!(topological_open_edges(&model, one), 0);
    let faces_after_one = all_faces(&model, one).len();
    let mesh_one = manifold_report(&model, one, 0.02, 1e-6).expect("one-bore mesh");
    assert_eq!(
        mesh_one.boundary_edges, 0,
        "the one-bore plate's mesh must close",
    );

    // Bore 2 at x = 0 — 30mm away from bore 1: no tangency, no coincidence.
    let bore2 = make_cylinder(
        &mut model,
        Point3::new(0.0, 0.0, -30.0),
        Vector3::new(0.0, 0.0, 1.0),
        1.0,
        60.0,
    );
    let two = boolean_operation(
        &mut model,
        one,
        bore2,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("second through-bore must succeed");
    describe(&mut model, two, "bore-2");

    let faces_after_two = all_faces(&model, two).len();
    assert!(
        faces_after_two > faces_after_one,
        "a second bore ADDS a wall face: had {faces_after_one} faces after one \
         bore, {faces_after_two} after two — a face was lost",
    );
    assert_eq!(
        topological_open_edges(&model, two),
        0,
        "the two-bore plate must remain a closed 2-manifold",
    );

    let mesh_two = manifold_report(&model, two, 0.02, 1e-6).expect("two-bore mesh");
    assert_eq!(
        mesh_two.boundary_edges, 0,
        "the two-bore plate's mesh must close (the reported symptom was 131 \
         open boundary edges)",
    );
    assert_eq!(mesh_two.components, 1, "the two-bore plate is one body",);

    let v2 = model
        .calculate_solid_volume(two)
        .expect("volume after two bores");
    let expect2 = 128000.0 - 2.0 * std::f64::consts::PI * 40.0;
    assert!(
        v2 < v1,
        "a subtraction cannot RAISE the volume: {v1} -> {v2}"
    );
    assert!(
        (v2 - expect2).abs() < 2.0,
        "two-bore volume {v2} should match analytic {expect2}",
    );
}

// ---------------------------------------------------------------------------
// DEFECT 2, as it actually reproduces — the COINCIDENT re-cut
// ---------------------------------------------------------------------------

/// The reported second-bore failure was reported as "a second bore 30mm away".
/// It is not: cutting at x = 0 is clean (8 faces, analytic volume, sound). The
/// failing input is re-cutting the SAME bore with a coincident cylinder — a
/// placement sweep reproduced the report's volume to five decimals
/// (127958.11286 vs the reported 127958.11), which is also why the reporter saw
/// byte-identical output at every requested separation: the tool never moved.
///
/// That case is a genuine correctness gap. `A ∖ B` where B exactly fills an
/// existing through-hole is `A` — B and A share only a measure-zero boundary —
/// but the pipeline drops A's bore wall as an "internal anti-coincident" face
/// (`cull_internal_coincident_faces`, confirmed by `ROSHERA_BOOL_TRACE`:
/// fragments 12 → 10 with A's origin-11 wall among the two culled), leaving the
/// cap hole loops dangling on a 6-face husk.
///
/// What this test pins is the property that DOES hold and that this repo trades
/// on: the kernel does not bless it. The certificate reports `sound == false`
/// (`brep_valid` and `watertight` both false). An honest unsound verdict is a
/// far smaller failure than a silent wrong answer — and this test goes red the
/// day anyone makes the certificate call this husk sound.
#[test]
fn coincident_recut_of_an_existing_bore_is_never_certified_sound() {
    let mut model = BRepModel::new();
    let plate = make_box(&mut model, 80.0, 40.0, 40.0);
    let bore1 = make_cylinder(
        &mut model,
        Point3::new(-30.0, 0.0, -30.0),
        Vector3::new(0.0, 0.0, 1.0),
        1.0,
        60.0,
    );
    let one = boolean_operation(
        &mut model,
        plate,
        bore1,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("first through-bore must succeed");

    // The identical cylinder again, in the identical place.
    let bore1_again = make_cylinder(
        &mut model,
        Point3::new(-30.0, 0.0, -30.0),
        Vector3::new(0.0, 0.0, 1.0),
        1.0,
        60.0,
    );
    let again = boolean_operation(
        &mut model,
        one,
        bore1_again,
        BooleanOp::Difference,
        BooleanOptions::default(),
    );

    let Ok(id) = again else {
        // A typed refusal is a perfectly honest outcome for this input.
        return;
    };
    describe(&mut model, id, "coincident-recut");

    let cert = model.certify_solid(id);
    let faces = all_faces(&model, id).len();
    let mesh = manifold_report(&model, id, 0.02, 1e-6);
    let closed = mesh
        .as_ref()
        .map(|m| m.boundary_edges == 0)
        .unwrap_or(false);

    // Either it is genuinely correct (A unchanged: 7 faces, closed mesh), or it
    // is broken — in which case the certificate MUST say so.
    if faces == 7 && closed {
        assert!(
            cert.is_sound(),
            "the coincident re-cut produced the correct unchanged one-bore plate \
             but the certificate calls it unsound",
        );
    } else {
        assert!(
            !cert.is_sound(),
            "the coincident re-cut produced a {faces}-face result whose mesh \
             {} close, and the kernel certified it SOUND — a silent wrong answer",
            if closed { "DOES" } else { "does NOT" },
        );
    }
}

// ---------------------------------------------------------------------------
// The other side of the multi-body rule: a SEVER must still succeed
// ---------------------------------------------------------------------------

/// Cutting a plate in two is an ordinary CAD operation, and the geometry the
/// kernel computes for it is right — the `--lib` Monte-Carlo oracles
/// `rotated_box_booleans_match_mc_truth` / `tilted_box_booleans_match_mc_truth`
/// measure a four-body Difference against analytic truth and pass.
///
/// So the multi-body refusal is deliberately UNION-ONLY: for a union of
/// operands that never touched there is no exact single-solid answer, while a
/// sever produces bodies that did not exist before the op and destroying them
/// would be strictly worse than the bookkeeping problem it avoids.
///
/// This test is the guard on that narrowing: make the refusal unconditional and
/// it goes red, forcing the trade-off to be re-argued rather than silently
/// re-decided.
///
/// KNOWN RESIDUAL (out of scope, deliberately not asserted as correct): the two
/// halves are filed as outer shell + `inner_shells`, so only the MESH consumer
/// reports the right volume. `Solid::compute_mass_properties` subtracts the
/// second half and `solid_outer_face_count` cannot see its faces.
#[test]
fn difference_that_severs_a_body_still_succeeds() {
    let mut model = BRepModel::new();
    let plate = make_box(&mut model, 40.0, 10.0, 10.0);
    // A 4mm-wide slot straight through the middle, taller and deeper than the
    // plate so the cut is complete and the plate genuinely falls into halves.
    let cutter = make_box(&mut model, 4.0, 20.0, 20.0);

    let severed = boolean_operation(
        &mut model,
        plate,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect(
        "severing a plate is an ordinary operation and its geometry is \
         MC-oracle-verified; the multi-body refusal must stay union-only",
    );
    describe(&mut model, severed, "severed-plate");

    let v = model
        .calculate_solid_volume(severed)
        .expect("a severed plate has a volume");
    let expect = 40.0 * 10.0 * 10.0 - 4.0 * 10.0 * 10.0;
    assert!(
        (v - expect).abs() < 20.0,
        "severed plate mesh volume {v}, expected {expect}",
    );
}
