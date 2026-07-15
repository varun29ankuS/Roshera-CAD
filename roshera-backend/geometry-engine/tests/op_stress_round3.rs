// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! op_stress_round3 — broad-spectrum kernel bug HUNT (round 3).
//!
//! Goal: surface NEW corruption classes across op families NOT stressed in the
//! prior two loop iterations (which closed a blend-weld class — cone-rim fillet,
//! bore-rim chamfer — and a sweep scratch-face B-Rep defect). We deliberately
//! avoid re-stressing fillet/chamfer blend rims here.
//!
//! For every case we run the FULL ground-truth triple the kernel itself uses:
//!   1. `validate_solid_scoped(Standard)`   — B-Rep structural validity
//!   2. `manifold_report`                    — mesh closure / orientation
//!        (boundary_edges == 0, nonmanifold_edges == 0, oriented == true)
//!   3. `certify_solid().is_sound()`         — the intrinsic certificate
//!
//! Each case prints exactly one verdict line:
//!   BUILT+SOUND       — op succeeded and all three checks pass
//!   BUILT-BUT-CORRUPT — op succeeded but a check FAILS (a bug); the line
//!                       carries the exact defect numbers + cert dimension
//!   DID-NOT-BUILD     — op returned Err (honest reject or a build failure);
//!                       the line carries the typed error string
//!
//! Nothing here weakens an existing harness: these are NEW cases. The battery is
//! a hunt, so a CORRUPT verdict is REPORTED (printed + counted), not asserted —
//! the run completes and prints the full table even when bugs are present. A
//! single guard test at the end fails only if a case PANICKED (a hard crash),
//! which is itself a finding.

use std::f64::consts::{PI, TAU};
use std::sync::Mutex;

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};
use geometry_engine::operations::deep_clone::deep_clone_solid;
use geometry_engine::operations::draft::{DraftType, NeutralElement};
use geometry_engine::operations::sweep::ScaleControl;
use geometry_engine::operations::{
    apply_draft, boolean_operation, loft_profiles, offset_solid, revolve_profile, sweep_profile,
    transform_solid, BooleanOp, BooleanOptions, CommonOptions, DraftOptions, LoftOptions,
    OffsetOptions, RevolveOptions, SweepOptions, TransformOptions,
};
use geometry_engine::primitives::curve::{Circle, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

// ---------------------------------------------------------------------------
// Verdict plumbing
// ---------------------------------------------------------------------------

static CORRUPT_COUNT: Mutex<usize> = Mutex::new(0);

fn bump_corrupt() {
    if let Ok(mut g) = CORRUPT_COUNT.lock() {
        *g += 1;
    }
}

/// Run the full ground-truth triple on `solid` and print a single verdict line.
/// `case` is the human label.
fn report(model: &mut BRepModel, solid: SolidId, case: &str) {
    let val = validate_solid_scoped(
        model,
        solid,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    // chord 0.5, weld 1e-6 — matches the harness convention for 1..30-unit solids.
    let mr = manifold_report(model, solid, 0.5, 1e-6);
    let cert = model.certify_solid(solid);

    let brep_ok = val.is_valid;
    let (be, nme, oriented, mesh_ok) = match &mr {
        Some(r) => (r.boundary_edges, r.nonmanifold_edges, r.oriented, true),
        None => (usize::MAX, usize::MAX, false, false),
    };
    let sound = cert.is_sound();

    let all_ok = brep_ok && mesh_ok && be == 0 && nme == 0 && oriented && sound;

    if all_ok {
        println!("BUILT+SOUND        | {case}");
    } else {
        bump_corrupt();
        // Which cert dimension failed (the certificate is the richest signal).
        let mut bad = Vec::new();
        if !cert.brep_valid {
            bad.push("brep_valid");
        }
        if !cert.watertight {
            bad.push("watertight");
        }
        if !cert.manifold {
            bad.push("manifold");
        }
        if !cert.oriented {
            bad.push("oriented");
        }
        if !cert.self_intersection_free {
            bad.push("self_intersection_free");
        }
        if !cert.construction_consistent.is_sound() {
            bad.push("construction_consistent");
        }
        if !cert.tessellation.clean {
            bad.push("tessellation");
        }
        if !cert.mesh_quality.clean {
            bad.push("mesh_quality");
        }
        let mesh_str = if mesh_ok {
            format!("be={be} nme={nme} oriented={oriented}")
        } else {
            "mesh=NONE(tessellate-empty)".to_string()
        };
        let val_str = if brep_ok {
            "scoped-valid".to_string()
        } else {
            format!(
                "scoped-INVALID({} errs: {:?})",
                val.errors.len(),
                val.errors.iter().take(2).collect::<Vec<_>>()
            )
        };
        println!(
            "BUILT-BUT-CORRUPT  | {case} | {mesh_str} | {val_str} | cert.is_sound={sound} fails=[{}]",
            bad.join(",")
        );
    }
}

/// For ops that may legitimately refuse: print DID-NOT-BUILD with the typed error.
fn report_err(case: &str, err: &impl std::fmt::Debug) {
    println!("DID-NOT-BUILD      | {case} | err={err:?}");
}

// ---------------------------------------------------------------------------
// Build helpers (mirroring existing harness fixtures)
// ---------------------------------------------------------------------------

fn box_solid(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("create_box_3d")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn cylinder_solid(
    model: &mut BRepModel,
    base: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(base, axis, radius, height)
        .expect("create_cylinder_3d")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn sphere_solid(model: &mut BRepModel, center: Point3, radius: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(center, radius)
        .expect("create_sphere_3d")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn translate_in_place(model: &mut BRepModel, id: SolidId, t: Vector3) {
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&t),
        TransformOptions::default(),
    )
    .expect("translate solid");
}

fn rotate_in_place(model: &mut BRepModel, id: SolidId, axis: Vector3, angle: f64) {
    let m = Matrix4::from_axis_angle(&axis, angle).expect("axis-angle");
    transform_solid(model, id, m, TransformOptions::default()).expect("rotate solid");
}

fn add_line_edge(m: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let pa = m.vertices.get(a).expect("a").position;
    let pb = m.vertices.get(b).expect("b").position;
    let cid = m
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    m.edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
}

/// CCW rectangle in the XY plane (z = 0).
fn rect_xy(m: &mut BRepModel, w: f64, h: f64) -> Vec<EdgeId> {
    let v0 = m.vertices.add(0.0, 0.0, 0.0);
    let v1 = m.vertices.add(w, 0.0, 0.0);
    let v2 = m.vertices.add(w, h, 0.0);
    let v3 = m.vertices.add(0.0, h, 0.0);
    vec![
        add_line_edge(m, v0, v1),
        add_line_edge(m, v1, v2),
        add_line_edge(m, v2, v3),
        add_line_edge(m, v3, v0),
    ]
}

/// CCW rectangle in the XZ plane offset from the Z axis: x∈[x0,x1], z∈[z0,z1].
fn rect_xz(m: &mut BRepModel, x0: f64, x1: f64, z0: f64, z1: f64) -> Vec<EdgeId> {
    let v0 = m.vertices.add(x0, 0.0, z0);
    let v1 = m.vertices.add(x1, 0.0, z0);
    let v2 = m.vertices.add(x1, 0.0, z1);
    let v3 = m.vertices.add(x0, 0.0, z1);
    vec![
        add_line_edge(m, v0, v1),
        add_line_edge(m, v1, v2),
        add_line_edge(m, v2, v3),
        add_line_edge(m, v3, v0),
    ]
}

fn circle_profile(m: &mut BRepModel, center: Point3, radius: f64) -> Vec<EdgeId> {
    let seam = m
        .vertices
        .add_or_find(center.x + radius, center.y, center.z, 1e-6);
    let cid = m.curves.add(Box::new(
        Circle::new(center, Vector3::new(0.0, 0.0, 1.0), radius).expect("circle"),
    ));
    vec![m.edges.add(Edge::new(
        0,
        seam,
        seam,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ))]
}

fn square_profile(m: &mut BRepModel, origin: Point3, side: f64) -> Vec<EdgeId> {
    let v0 = m.vertices.add(origin.x, origin.y, origin.z);
    let v1 = m.vertices.add(origin.x + side, origin.y, origin.z);
    let v2 = m.vertices.add(origin.x + side, origin.y + side, origin.z);
    let v3 = m.vertices.add(origin.x, origin.y + side, origin.z);
    vec![
        add_line_edge(m, v0, v1),
        add_line_edge(m, v1, v2),
        add_line_edge(m, v2, v3),
        add_line_edge(m, v3, v0),
    ]
}

/// Build a w×h×d box and return (solid, +X face id) — located by surface normal.
fn box_with_plus_x_face(m: &mut BRepModel, w: f64, h: f64, d: f64) -> (SolidId, FaceId) {
    let solid_id = box_solid(m, w, h, d);
    let solid = m.solids.get(solid_id).expect("solid").clone();
    let shell = m.shells.get(solid.outer_shell).expect("shell").clone();
    let mut target = None;
    for &fid in &shell.faces {
        let face = m.faces.get(fid).expect("face");
        let surface = m.surfaces.get(face.surface_id).expect("surface");
        if let Ok(n) = surface.normal_at(0.5, 0.5) {
            if (n.x - 1.0).abs() < 1e-9 && n.y.abs() < 1e-9 && n.z.abs() < 1e-9 {
                target = Some(fid);
                break;
            }
        }
    }
    (solid_id, target.expect("box must have +X face"))
}

fn bool_opts() -> BooleanOptions {
    BooleanOptions::default()
}

// ===========================================================================
// 1. BOOLEAN ROBUSTNESS
// ===========================================================================

#[test]
fn boolean_offset_rotated_boxes() {
    // Union / difference / intersection of an axis-aligned box with a box that is
    // both offset AND rotated 30° about Z (general-position imprint, no
    // coincident faces). The classic over-inclusion / rotated-input class.
    for &op in &[
        BooleanOp::Union,
        BooleanOp::Difference,
        BooleanOp::Intersection,
    ] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 4.0, 4.0, 4.0);
        let b = box_solid(&mut m, 3.0, 3.0, 3.0);
        rotate_in_place(&mut m, b, Vector3::Z, 30f64.to_radians());
        translate_in_place(&mut m, b, Vector3::new(1.5, 1.0, 0.5));
        match boolean_operation(&mut m, a, b, op, bool_opts()) {
            Ok(r) => {
                report(&mut m, r, &format!("bool {op:?} offset+rot30 boxes"));
            }
            Err(e) => report_err(&format!("bool {op:?} offset+rot30 boxes"), &e),
        }
    }
}

#[test]
fn boolean_box_cylinder_non_saddle() {
    // box ± cylinder, axis along Z, cylinder fully through the box top face
    // (a through-hole for difference; a boss-overlap for union). NOT the #35
    // cyl-cyl saddle — one operand is a box, so the SSI is plane×cylinder.
    for &op in &[
        BooleanOp::Difference,
        BooleanOp::Union,
        BooleanOp::Intersection,
    ] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 10.0, 10.0, 10.0);
        // center the box at origin: box spans [-5,5]^3 (create_box_3d is centred).
        let cyl = cylinder_solid(&mut m, Point3::new(0.0, 0.0, -6.0), Vector3::Z, 2.0, 12.0);
        match boolean_operation(&mut m, a, cyl, op, bool_opts()) {
            Ok(r) => {
                report(&mut m, r, &format!("bool {op:?} box/cyl through-Z"));
            }
            Err(e) => report_err(&format!("bool {op:?} box/cyl through-Z"), &e),
        }
    }
}

#[test]
fn boolean_box_sphere() {
    // box ± sphere, sphere centred on a box face (partial poke). plane×sphere SSI.
    for &op in &[
        BooleanOp::Difference,
        BooleanOp::Union,
        BooleanOp::Intersection,
    ] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 10.0, 10.0, 10.0);
        let sph = sphere_solid(&mut m, Point3::new(0.0, 0.0, 5.0), 3.0);
        match boolean_operation(&mut m, a, sph, op, bool_opts()) {
            Ok(r) => {
                report(&mut m, r, &format!("bool {op:?} box/sphere face-poke"));
            }
            Err(e) => report_err(&format!("bool {op:?} box/sphere face-poke"), &e),
        }
    }
}

#[test]
fn boolean_partial_overlap_boxes() {
    // Two axis-aligned boxes overlapping on one corner octant (general 3-axis
    // partial overlap, but axis-aligned so faces are parallel not coincident).
    for &op in &[
        BooleanOp::Union,
        BooleanOp::Difference,
        BooleanOp::Intersection,
    ] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 4.0, 4.0, 4.0);
        let b = box_solid(&mut m, 4.0, 4.0, 4.0);
        translate_in_place(&mut m, b, Vector3::new(2.0, 2.0, 2.0));
        match boolean_operation(&mut m, a, b, op, bool_opts()) {
            Ok(r) => {
                report(&mut m, r, &format!("bool {op:?} partial-overlap boxes"));
            }
            Err(e) => report_err(&format!("bool {op:?} partial-overlap boxes"), &e),
        }
    }
}

/// HARD GATE — the #34/#80 corner-octant classification fix.
///
/// Two IDENTICAL axis-aligned 4×4×4 boxes, B offset from A by exactly (+2,+2,+2):
/// a clean corner-octant overlap with NO rotation and NO degeneracy — the smallest
/// repro of the boolean over-inclusion / face-classification-ceiling class. Before
/// the fix, `box ∖ box` leaked open (odd Euler χ, ~10 boundary edges) and
/// `box ∩ box` went non-manifold (edges shared by 3 faces, non-oriented), because
/// the L-shaped outer fragments at the triple-overlap corner took their
/// representative interior point at the reflex notch vertex — which sits on B's
/// edge — and so spuriously classified `OnBoundary` (dropped for ∖, double-counted
/// for ∩). The fix makes `get_face_interior_point` return a genuinely interior
/// point for non-convex planar fragments.
///
/// This asserts BOTH operations are watertight + manifold + oriented + B-Rep-valid
/// + certificate-sound, so a regression here fails the build (boolean is gated).
#[test]
fn boolean_corner_octant_difference_and_intersection_sound() {
    for &op in &[BooleanOp::Difference, BooleanOp::Intersection] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 4.0, 4.0, 4.0);
        let b = box_solid(&mut m, 4.0, 4.0, 4.0);
        translate_in_place(&mut m, b, Vector3::new(2.0, 2.0, 2.0));
        let r = boolean_operation(&mut m, a, b, op, bool_opts())
            .unwrap_or_else(|e| panic!("corner-octant {op:?} must build, got err: {e:?}"));

        let val = validate_solid_scoped(&mut m, r, Tolerance::default(), ValidationLevel::Standard);
        let mr = manifold_report(&mut m, r, 0.5, 1e-6)
            .unwrap_or_else(|| panic!("corner-octant {op:?} produced an empty mesh"));
        let cert = m.certify_solid(r);

        assert!(
            val.is_valid,
            "corner-octant {op:?}: B-Rep invalid: {:?}",
            val.errors.iter().take(3).collect::<Vec<_>>()
        );
        assert_eq!(
            mr.boundary_edges, 0,
            "corner-octant {op:?}: {} boundary edges (must be watertight)",
            mr.boundary_edges
        );
        assert_eq!(
            mr.nonmanifold_edges, 0,
            "corner-octant {op:?}: {} non-manifold edges (must be manifold)",
            mr.nonmanifold_edges
        );
        assert!(
            mr.oriented,
            "corner-octant {op:?}: mesh not consistently oriented"
        );
        assert!(
            cert.is_sound(),
            "corner-octant {op:?}: certificate not sound (cert={cert:?})"
        );
    }
}

#[test]
fn boolean_adjacent_face_touch_boxes() {
    // Two boxes sharing exactly one coincident face (union of stacked blocks).
    // This is the known coincident-face class (#32-adjacent) — note if hit.
    for &op in &[BooleanOp::Union] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 4.0, 4.0, 4.0);
        let b = box_solid(&mut m, 4.0, 4.0, 4.0);
        translate_in_place(&mut m, b, Vector3::new(4.0, 0.0, 0.0)); // share +X / -X face
        match boolean_operation(&mut m, a, b, op, bool_opts()) {
            Ok(r) => {
                report(
                    &mut m,
                    r,
                    &format!("bool {op:?} face-adjacent boxes (coincident)"),
                );
            }
            Err(e) => report_err(&format!("bool {op:?} face-adjacent boxes (coincident)"), &e),
        }
    }
}

#[test]
fn boolean_coincident_full_overlap() {
    // Two identical coincident boxes: A∪A, A∩A, A∖A. A∖A should be the EMPTY
    // solid (honest EmptyResult); A∪A and A∩A should equal A. Tests idempotence
    // + the coincident-everywhere degenerate.
    for &op in &[
        BooleanOp::Union,
        BooleanOp::Intersection,
        BooleanOp::Difference,
    ] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 3.0, 3.0, 3.0);
        let b = box_solid(&mut m, 3.0, 3.0, 3.0);
        // b coincident with a (both centred at origin). Nudge b's vertices apart
        // via an identity round-trip is unnecessary; they share verts but
        // transform_solid isolates topology, so leave coincident.
        match boolean_operation(&mut m, a, b, op, bool_opts()) {
            Ok(r) => {
                report(&mut m, r, &format!("bool {op:?} fully-coincident boxes"));
            }
            Err(e) => report_err(&format!("bool {op:?} fully-coincident boxes"), &e),
        }
    }
}

#[test]
fn boolean_disjoint_boxes() {
    // Disjoint boxes: union = two shells (multi-body), intersection = empty,
    // difference = A unchanged. Tests the empty/disjoint honest paths.
    for &op in &[
        BooleanOp::Union,
        BooleanOp::Intersection,
        BooleanOp::Difference,
    ] {
        let mut m = BRepModel::new();
        let a = box_solid(&mut m, 2.0, 2.0, 2.0);
        let b = box_solid(&mut m, 2.0, 2.0, 2.0);
        translate_in_place(&mut m, b, Vector3::new(10.0, 0.0, 0.0));
        match boolean_operation(&mut m, a, b, op, bool_opts()) {
            Ok(r) => {
                report(&mut m, r, &format!("bool {op:?} disjoint boxes"));
            }
            Err(e) => report_err(&format!("bool {op:?} disjoint boxes"), &e),
        }
    }
}

// ===========================================================================
// 2. TRANSFORM + PATTERN
// ===========================================================================

#[test]
fn pattern_linear_boxes_each_and_union() {
    // Linear pattern of 4 boxes via deep_clone + offset, validate each instance,
    // then union the whole array and validate the combined result.
    let mut m = BRepModel::new();
    let base = box_solid(&mut m, 2.0, 2.0, 2.0);
    report(&mut m, base, "pattern-linear base instance");
    let mut acc = base;
    for i in 1..4 {
        match deep_clone_solid(&mut m, base, Some(Vector3::new(3.0 * i as f64, 0.0, 0.0))) {
            Ok(inst) => {
                report(&mut m, inst, &format!("pattern-linear instance {i}"));
                // Union into the running accumulator (disjoint copies → multi-body).
                match boolean_operation(&mut m, acc, inst, BooleanOp::Union, bool_opts()) {
                    Ok(u) => {
                        acc = u;
                    }
                    Err(e) => report_err(&format!("pattern-linear union step {i}"), &e),
                }
            }
            Err(e) => report_err(&format!("pattern-linear clone {i}"), &e),
        }
    }
    report(&mut m, acc, "pattern-linear union(4 disjoint boxes)");
}

#[test]
fn pattern_circular_boxes_each_and_union() {
    // Circular pattern: 6 boxes around Z, each translated out +X then rotated
    // about the origin. Validate each instance and the union.
    let mut m = BRepModel::new();
    let count = 6;
    let mut instances = Vec::new();
    for i in 0..count {
        let inst = box_solid(&mut m, 1.5, 1.5, 1.5);
        translate_in_place(&mut m, inst, Vector3::new(5.0, 0.0, 0.0));
        let angle = TAU * (i as f64) / (count as f64);
        rotate_in_place(&mut m, inst, Vector3::Z, angle);
        report(&mut m, inst, &format!("pattern-circular instance {i}"));
        instances.push(inst);
    }
    let mut acc = instances[0];
    for (i, &inst) in instances.iter().enumerate().skip(1) {
        match boolean_operation(&mut m, acc, inst, BooleanOp::Union, bool_opts()) {
            Ok(u) => acc = u,
            Err(e) => report_err(&format!("pattern-circular union step {i}"), &e),
        }
    }
    report(&mut m, acc, "pattern-circular union(6 disjoint boxes)");
}

#[test]
fn transform_mirror_box() {
    // Mirror a box across a plane offset from it (so the mirror image is a
    // distinct solid). Validate the mirrored solid.
    let mut m = BRepModel::new();
    let b = box_solid(&mut m, 2.0, 3.0, 4.0);
    translate_in_place(&mut m, b, Vector3::new(5.0, 0.0, 0.0));
    let mirror_mat = Matrix4::mirror(Point3::ZERO, Vector3::X).expect("mirror matrix");
    match transform_solid(&mut m, b, mirror_mat, TransformOptions::default()) {
        Ok(_) => {
            report(&mut m, b, "transform mirror-across-X box");
        }
        Err(e) => report_err("transform mirror-across-X box", &e),
    }
}

#[test]
fn transform_chained_rotate_translate_scale() {
    // Chain: rotate 45° about an arbitrary axis, translate, then scale-about-point.
    // Validate after the chain (each transform validates its own result too).
    let mut m = BRepModel::new();
    let b = box_solid(&mut m, 2.0, 2.0, 2.0);
    let axis = Vector3::new(1.0, 1.0, 0.0).normalize().expect("axis");
    rotate_in_place(&mut m, b, axis, 45f64.to_radians());
    translate_in_place(&mut m, b, Vector3::new(3.0, -2.0, 1.0));
    let scale =
        Matrix4::scale_about_point(Point3::new(3.0, -2.0, 1.0), Vector3::new(1.5, 1.5, 1.5));
    match transform_solid(&mut m, b, scale, TransformOptions::default()) {
        Ok(_) => report(&mut m, b, "transform chained rot+trans+scale box"),
        Err(e) => report_err("transform chained rot+trans+scale box", &e),
    };
}

// ===========================================================================
// 3. DRAFT / OFFSET
// ===========================================================================

#[test]
fn draft_box_side_face() {
    // Draft the +X side face of a box about its mid-plane (the documented
    // in-place prismatic path). Re-stresses with validate_result OFF so the
    // cert/manifold checks are the ones doing the verification here.
    let mut m = BRepModel::new();
    let (solid, face) = box_with_plus_x_face(&mut m, 10.0, 10.0, 10.0);
    let opts = DraftOptions {
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        draft_type: DraftType::Angle(8f64.to_radians()),
        neutral: NeutralElement::Plane(Point3::ZERO, Vector3::Z),
        pull_direction: Vector3::Z,
        ..Default::default()
    };
    match apply_draft(&mut m, solid, vec![face], opts) {
        Ok(_) => report(&mut m, solid, "draft +X face about mid-plane @8deg"),
        Err(e) => report_err("draft +X face about mid-plane @8deg", &e),
    };
}

#[test]
fn offset_box_shell() {
    // Offset (shell/hollow) a box: remove the top (+Z) face and hollow to a 1.0
    // wall. The classic shell self-intersection class lives here.
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 10.0, 10.0, 10.0);
    // Locate the +Z face to remove.
    let s = m.solids.get(solid).expect("solid").clone();
    let shell = m.shells.get(s.outer_shell).expect("shell").clone();
    let mut top = None;
    for &fid in &shell.faces {
        let face = m.faces.get(fid).expect("face");
        let surf = m.surfaces.get(face.surface_id).expect("surf");
        if let Ok(n) = surf.normal_at(0.5, 0.5) {
            if (n.z - 1.0).abs() < 1e-9 && n.x.abs() < 1e-9 && n.y.abs() < 1e-9 {
                top = Some(fid);
                break;
            }
        }
    }
    let top = top.expect("box must have +Z face");
    match offset_solid(&mut m, solid, 1.0, vec![top], OffsetOptions::default()) {
        Ok(r) => report(&mut m, r, "offset/shell box wall=1 open-top"),
        Err(e) => report_err("offset/shell box wall=1 open-top", &e),
    };
}

#[test]
fn offset_box_shell_closed() {
    // Shell a box with NO removed faces (fully-enclosed hollow → cavity).
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 10.0, 10.0, 10.0);
    match offset_solid(&mut m, solid, 1.0, vec![], OffsetOptions::default()) {
        Ok(r) => report(&mut m, r, "offset/shell box wall=1 fully-closed cavity"),
        Err(e) => report_err("offset/shell box wall=1 fully-closed cavity", &e),
    };
}

// ===========================================================================
// 4. LOFT
// ===========================================================================

#[test]
fn loft_three_circles() {
    // 3 coaxial circles of decreasing radius (a smooth bulge → waist → tip).
    let mut m = BRepModel::new();
    let c0 = circle_profile(&mut m, Point3::new(0.0, 0.0, 0.0), 10.0);
    let c1 = circle_profile(&mut m, Point3::new(0.0, 0.0, 20.0), 6.0);
    let c2 = circle_profile(&mut m, Point3::new(0.0, 0.0, 40.0), 3.0);
    match loft_profiles(
        &mut m,
        vec![c0, c1, c2],
        LoftOptions {
            create_solid: true,
            ..Default::default()
        },
    ) {
        Ok(r) => report(&mut m, r, "loft 3 coaxial circles"),
        Err(e) => report_err("loft 3 coaxial circles", &e),
    };
}

#[test]
fn loft_square_circle_square_dissimilar() {
    // 3 sections alternating square→circle→square (dissimilar-shape correspondence).
    let mut m = BRepModel::new();
    let s0 = square_profile(&mut m, Point3::new(-5.0, -5.0, 0.0), 10.0);
    let c1 = circle_profile(&mut m, Point3::new(0.0, 0.0, 15.0), 5.0);
    let s2 = square_profile(&mut m, Point3::new(-4.0, -4.0, 30.0), 8.0);
    match loft_profiles(
        &mut m,
        vec![s0, c1, s2],
        LoftOptions {
            create_solid: true,
            ..Default::default()
        },
    ) {
        Ok(r) => report(&mut m, r, "loft square-circle-square dissimilar"),
        Err(e) => report_err("loft square-circle-square dissimilar", &e),
    };
}

#[test]
fn loft_four_sections() {
    // 4 sections (circle→square→circle→square) — longer chain.
    let mut m = BRepModel::new();
    let c0 = circle_profile(&mut m, Point3::new(0.0, 0.0, 0.0), 8.0);
    let s1 = square_profile(&mut m, Point3::new(-6.0, -6.0, 12.0), 12.0);
    let c2 = circle_profile(&mut m, Point3::new(0.0, 0.0, 24.0), 5.0);
    let s3 = square_profile(&mut m, Point3::new(-4.0, -4.0, 36.0), 8.0);
    match loft_profiles(
        &mut m,
        vec![c0, s1, c2, s3],
        LoftOptions {
            create_solid: true,
            ..Default::default()
        },
    ) {
        Ok(r) => report(&mut m, r, "loft 4 sections circle/square alternating"),
        Err(e) => report_err("loft 4 sections circle/square alternating", &e),
    };
}

// ===========================================================================
// 5. REVOLVE (partial angles, with/without caps)
// ===========================================================================

#[test]
fn revolve_partial_angles() {
    for (deg, caps) in [
        (90.0f64, true),
        (180.0, true),
        (270.0, true),
        (180.0, false),
    ] {
        let mut m = BRepModel::new();
        let edges = rect_xz(&mut m, 2.0, 5.0, 0.0, 3.0);
        let opts = RevolveOptions {
            angle: deg.to_radians(),
            segments: 24,
            cap_ends: caps,
            ..Default::default()
        };
        match revolve_profile(&mut m, edges, opts) {
            Ok(r) => report(
                &mut m,
                r,
                &format!("revolve {deg}deg caps={caps} offset-rect"),
            ),
            Err(e) => report_err(&format!("revolve {deg}deg caps={caps} offset-rect"), &e),
        };
    }
}

#[test]
fn revolve_full_no_caps() {
    let mut m = BRepModel::new();
    let edges = rect_xz(&mut m, 3.0, 6.0, 0.0, 4.0);
    let opts = RevolveOptions {
        angle: TAU,
        segments: 32,
        cap_ends: false,
        ..Default::default()
    };
    match revolve_profile(&mut m, edges, opts) {
        Ok(r) => report(&mut m, r, "revolve 360deg no-caps tube"),
        Err(e) => report_err("revolve 360deg no-caps tube", &e),
    };
}

#[test]
fn revolve_axis_touching_profile() {
    // Profile touching the axis (x0 = 0) → revolve makes a solid disc/cone, not a
    // tube. A degenerate-radius seam class.
    let mut m = BRepModel::new();
    let edges = rect_xz(&mut m, 0.0, 4.0, 0.0, 3.0);
    let opts = RevolveOptions {
        angle: PI,
        segments: 24,
        cap_ends: true,
        ..Default::default()
    };
    match revolve_profile(&mut m, edges, opts) {
        Ok(r) => report(&mut m, r, "revolve 180deg axis-touching profile"),
        Err(e) => report_err("revolve 180deg axis-touching profile", &e),
    };
}

// ===========================================================================
// 6. SWEEP (taper, twist)
// ===========================================================================

#[test]
fn sweep_tapered_prism() {
    // Sweep a rectangle along +Z with a linear scale taper 1.0 → 0.5.
    let mut m = BRepModel::new();
    let profile = rect_xy(&mut m, 4.0, 4.0);
    let va = m.vertices.add(0.0, 0.0, 0.0);
    let vb = m.vertices.add(0.0, 0.0, 10.0);
    let path = add_line_edge(&mut m, va, vb);
    let opts = SweepOptions {
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        scale: ScaleControl::Linear(1.0, 0.5),
        ..Default::default()
    };
    match sweep_profile(&mut m, profile, path, opts) {
        Ok(r) => report(&mut m, r, "sweep tapered prism scale 1.0->0.5"),
        Err(e) => report_err("sweep tapered prism scale 1.0->0.5", &e),
    };
}

#[test]
fn sweep_straight_prism_baseline() {
    // Plain straight sweep (baseline; should be solidly BUILT+SOUND).
    let mut m = BRepModel::new();
    let profile = rect_xy(&mut m, 3.0, 2.0);
    let va = m.vertices.add(0.0, 0.0, 0.0);
    let vb = m.vertices.add(0.0, 0.0, 8.0);
    let path = add_line_edge(&mut m, va, vb);
    match sweep_profile(&mut m, profile, path, SweepOptions::default()) {
        Ok(r) => report(&mut m, r, "sweep straight prism baseline"),
        Err(e) => report_err("sweep straight prism baseline", &e),
    };
}

// ===========================================================================
// Guard: no case may PANIC. (A hard crash is itself a finding worth failing on.)
// CORRUPT verdicts are reported, not asserted — this is a hunt.
// ===========================================================================

#[test]
fn zz_summary() {
    // Runs last alphabetically; prints the corrupt count seen so far in THIS
    // binary's run. (Each #[test] runs in the same process; ordering is not
    // guaranteed across threads, so this is informational only.)
    let n = CORRUPT_COUNT.lock().map(|g| *g).unwrap_or(0);
    println!("---- op_stress_round3 corrupt-verdicts-so-far (informational): {n}");
}
