// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! #17 — boolean of a NURBS-lateral solid (the F1 cockpit-cut). Differencing a
//! box out of a `nurbs_loft` barrel used to NON-TERMINATE: the generic dual-
//! surface marcher hit its 200k-step cap on the NURBS×plane pair and discarded
//! the curve, so the wall never split and the boolean rejected the result.
//!
//! With the marching-squares plane↔freeform SSI (math::surface_plane_intersection
//! wired into surface_surface_intersection's (Planar,Other) arm) the cut now
//! COMPLETES and imprints onto the NURBS wall — bounded, no hang. Producing a
//! fully WATERTIGHT result (sharing the cut edges between operands) is the
//! remaining #17 work, pinned by the ignored test below.

use geometry_engine::math::Point3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn barrel(m: &mut BRepModel) -> SolidId {
    let ring = |r: f64, z: f64| {
        (0..20)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 20.0;
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect::<Vec<_>>()
    };
    let sections = vec![
        ring(3.0, 0.0),
        ring(4.0, 2.0),
        ring(4.0, 4.0),
        ring(3.0, 6.0),
    ];
    nurbs_loft(m, sections, NurbsLoftOptions::default()).expect("barrel")
}
fn boxs(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}
fn face_count(m: &BRepModel, s: SolidId) -> usize {
    m.solids
        .get(s)
        .map(|sol| {
            sol.shell_ids()
                .iter()
                .filter_map(|sh| m.shells.get(*sh))
                .map(|sh| sh.faces.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Regression guard: the NURBS∖box difference must COMPLETE (no hang, no
/// rejection) and imprint the cut onto the freeform wall. This locks in the
/// marching-squares SSI fix.
#[test]
fn nurbs_minus_box_completes_and_imprints() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let faces_before = face_count(&m, b);
    let cutter = boxs(&mut m, 3.0, 8.0, 3.0);
    let result = boolean_operation(
        &mut m,
        b,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("#17: NURBS∖box must complete (was: non-terminating SSI → rejection)");
    assert!(
        face_count(&m, result) >= faces_before,
        "the cut must imprint onto the NURBS solid (faces {} -> {})",
        faces_before,
        face_count(&m, result)
    );
}

/// #17 WELD GUARD (the mission's reported symptom, pinned GREEN): when a NURBS
/// lateral wall and a box (planar) face are cut by the SAME intersection curve,
/// the two cut edges must merge into ONE shared B-Rep edge used by exactly two
/// face-loops — never two coincident-but-distinct edges (a duplicate) or an
/// unmated boundary edge (a gap).
///
/// This is the corefinement identity discipline the shared-corner registry
/// (`build_shared_corner_endpoints` + the per-`curve_id` shared-edge map in
/// `split_faces_along_curves`) enforces. The assertion is stronger and more
/// targeted than `is_sound()`: it inspects the B-Rep directly and requires
///   * `coincident_edge_groups == 0` — no two distinct edges share endpoints
///     (the exact "cut edges don't merge" failure #17 was opened for), and
///   * `duplicate_vertex_groups == 0` — no unmerged coincident vertices,
/// on a clean seam-free +Y blind pocket (the well-posed reported case). If a
/// future change reintroduces per-face cut-edge creation without sharing, this
/// goes red at the weld level immediately, independent of mesh cleanliness.
#[test]
fn nurbs_box_cut_edges_are_shared_not_coincident() {
    use geometry_engine::harness::brep_integrity::brep_integrity;
    use geometry_engine::math::Vector3;
    use geometry_engine::operations::transform::{translate, TransformOptions};

    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    // Seam-free +Y wall, mid-height blind pocket — the well-posed reported case.
    let cutter = boxs(&mut m, 2.0, 2.0, 2.0);
    translate(
        &mut m,
        vec![cutter],
        Vector3::Y,
        4.0,
        TransformOptions::default(),
    )
    .expect("ty");
    translate(
        &mut m,
        vec![cutter],
        Vector3::Z,
        3.0,
        TransformOptions::default(),
    )
    .expect("tz");
    let result = boolean_operation(
        &mut m,
        b,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("#17: NURBS∖box blind pocket must complete");

    let rep = brep_integrity(&m, result, 1.0e-6);
    assert!(
        rep.coincident_edge_groups.is_empty(),
        "#17 WELD: NURBS-cut and box-cut edges must merge, found {} coincident-but-distinct edge group(s): {:?}",
        rep.coincident_edge_groups.len(),
        rep.coincident_edge_groups,
    );
    assert!(
        rep.duplicate_vertex_groups.is_empty(),
        "#17 WELD: cut endpoints must be shared, found {} unmerged duplicate-vertex group(s)",
        rep.duplicate_vertex_groups.len(),
    );
    // The pocket is a well-posed cut: it must also close (no open seam) and be
    // Euler-balanced — the shared cut edges are each used by exactly two faces.
    assert!(
        rep.edges_used_once.is_empty(),
        "#17 WELD: shared cut edges must each be used by two faces, found {} boundary edge(s): {:?}",
        rep.edges_used_once.len(),
        rep.edges_used_once,
    );
    assert_eq!(
        rep.euler_poincare_genus0_residual(),
        0,
        "#17 WELD: welded genus-0 pocket must be Euler-balanced (V−E+2F−L−2S=0)"
    );
}

/// DESIRED end state (pinned, currently RED → #[ignore]): this NASTY cutter
/// (3×8×3 straddling the base cap at z=0 and poking fully through BOTH ±Y walls)
/// must difference to a sound, watertight solid.
///
/// FAILURE CLASS (measured 2026-07-18, branch feat/sketch-dcm-45): NOT a weld/
/// corefinement gap. `brep_integrity` reports `coincident_edge_groups=0` and
/// `duplicate_vertex_groups=0` — the cut edges ARE shared (the shared-corner
/// registry works). The result is an OPEN SHELL: 16 edges-used-once, euler=-2.
/// The box x-wall fragments are mis-classified — the parts OUTSIDE the barrel
/// (|y|≈4, beyond radius) are kept while the INSIDE notch-wall fragments (which
/// should mate with the NURBS cut edges e17–e20) are dropped. This is a fragment
/// inside/outside classification / arrangement-completeness problem on the
/// base-cap-straddling through-cut (the GWN-against-a-tapered-NURBS-tessellation
/// family), tracked distinctly from the (now-resolved) corefinement weld — see
/// `nurbs_box_cut_edges_are_shared_not_coincident` for the weld guard.
#[test]
#[ignore = "#17: base-cap-straddle through-cut — open shell from mis-classified box-wall notch fragments (fragment classification, NOT weld: coincident_edge_groups=0)"]
fn nurbs_minus_box_should_be_watertight() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let cutter = boxs(&mut m, 3.0, 8.0, 3.0);
    let result = boolean_operation(
        &mut m,
        b,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("#17: NURBS∖box should succeed");
    let gt = m.ground_truth(result).expect("gt");
    assert!(
        gt.certificate.is_sound(),
        "#17: result must be sound: {}",
        gt.summary()
    );
}

/// EXPLORATION (#[ignore]): a clean blind pocket driven into the +X PERIODIC
/// SEAM wall at mid-height. The corefinement/weld is SOUND here — measured
/// 2026-07-18: `brep_valid=true watertight=true manifold=true euler=2`. The
/// ONLY reason `is_sound()` is false is MESH QUALITY on the seam wall
/// (`mesh_clean=false`, worst_aspect≈60, min_angle≈0.5°) — a tessellation
/// (seam-wall facet aspect) issue, NOT corefinement. The seam-free +Y variant
/// of exactly this cut is `nurbs_boolean_watertight::w01_clean_blind_pocket`,
/// which is fully GREEN. Un-ignore once the seam-wall tessellation quality
/// clears the mesh-cleanliness bar.
#[test]
#[ignore = "#17: topology is watertight (euler=2); fails is_sound only on SEAM-WALL MESH QUALITY (tessellation, not corefinement)"]
fn nurbs_clean_blind_pocket() {
    use geometry_engine::math::Vector3;
    use geometry_engine::operations::transform::{translate, TransformOptions};

    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let cutter = boxs(&mut m, 2.0, 2.0, 2.0); // centred at origin: [-1,1]^3
                                              // Drive into the +X wall (barrel radius ~4 at mid-height) at z = 3.
    translate(
        &mut m,
        vec![cutter],
        Vector3::X,
        4.0,
        TransformOptions::default(),
    )
    .expect("tx");
    translate(
        &mut m,
        vec![cutter],
        Vector3::Z,
        3.0,
        TransformOptions::default(),
    )
    .expect("tz");
    let result = boolean_operation(
        &mut m,
        b,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("clean pocket should complete");
    let gt = m.ground_truth(result).expect("gt");
    eprintln!("CLEAN POCKET: {}", gt.summary());
    assert!(
        gt.certificate.is_sound(),
        "clean blind pocket should be watertight: {}",
        gt.summary()
    );
}
