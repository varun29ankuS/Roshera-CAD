//! op_chain_degenerate_stress — kernel bug HUNT targeting OP CHAINS, DEGENERATE
//! inputs, and the iteration-3 "did-not-build" rejects.
//!
//! The single-op frontier (fillet / chamfer / blend / sweep / loft / revolve /
//! boolean / transform on clean inputs) was cleared by prior loops. New bugs now
//! hide in (1) COMPOSITION — real-part-like op sequences where an early step
//! plants a defect a later step trips over, (2) DEGENERATE / near-degenerate
//! geometry (near-coincident faces, near-tangent blends, sub-tolerance features,
//! extreme aspect ratios, near-zero shells), and (3) the iteration-3 cases that
//! returned `Err` — which may be HONEST refusals or real bugs masked as rejects.
//!
//! Every solid (each chain INTERMEDIATE and the FINAL) is judged by the exact
//! ground-truth triple the kernel itself uses — nothing here is weakened:
//!   1. `validate_solid_scoped(Standard)`  — B-Rep structural validity
//!   2. `manifold_report`                  — welded-mesh closure / orientation
//!        (boundary_edges == 0, nonmanifold_edges == 0, oriented == true)
//!   3. `certify_solid().is_sound()`        — the intrinsic certificate
//!
//! Each judged solid prints exactly one verdict line:
//!   BUILT+SOUND       — op succeeded and all three checks pass
//!   BUILT-BUT-CORRUPT — op succeeded but a check FAILS (a bug); the line carries
//!                       the exact defect numbers + the failed cert dimensions
//!   DID-NOT-BUILD     — op returned Err (honest reject or build failure); the
//!                       line carries the typed error string
//!
//! For an op CHAIN, every step is judged with its position in the chain, so a
//! CORRUPT verdict names WHICH step introduced the defect (the first step whose
//! verdict turns CORRUPT is the culprit).
//!
//! This is a HUNT: CORRUPT verdicts are REPORTED (printed + counted), not
//! asserted, so the full table always prints even with bugs present. The only
//! hard assertion is the final guard — it fails if any case PANICKED (a crash is
//! itself a finding). These are all NEW cases; no existing harness is weakened.

#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use std::f64::consts::{PI, TAU};
use std::sync::Mutex;

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::extrude::{extrude_face, ExtrudeOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::{
    boolean_operation, loft_profiles, offset_solid, revolve_profile, transform_solid, BooleanOp,
    BooleanOptions, CommonOptions, LoftOptions, OffsetOptions, RevolveOptions, TransformOptions,
};
use geometry_engine::primitives::curve::{Circle, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceId, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use geometry_engine::primitives::vertex::VertexId;

// ---------------------------------------------------------------------------
// Verdict plumbing (mirrors op_stress_round3.rs)
// ---------------------------------------------------------------------------

static CORRUPT_COUNT: Mutex<usize> = Mutex::new(0);
static PANIC_COUNT: Mutex<usize> = Mutex::new(0);

fn bump_corrupt() {
    if let Ok(mut g) = CORRUPT_COUNT.lock() {
        *g += 1;
    }
}

// chord 0.5, weld 1e-6 — matches the harness convention for ~1..40-unit solids.
const CHORD: f64 = 0.5;
const WELD: f64 = 1e-6;

/// Judge a solid and print exactly one verdict line (BUILT+SOUND or CORRUPT).
fn report(model: &mut BRepModel, solid: SolidId, case: &str) {
    let val = validate_solid_scoped(
        model,
        solid,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    let mr = manifold_report(model, solid, CHORD, WELD);
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
        return;
    }

    bump_corrupt();
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

fn report_err(case: &str, err: &impl std::fmt::Debug) {
    println!("DID-NOT-BUILD      | {case} | err={err:?}");
}

// ---------------------------------------------------------------------------
// Build helpers
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

fn translate_in_place(model: &mut BRepModel, id: SolidId, t: Vector3) {
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&t),
        TransformOptions::default(),
    )
    .expect("translate solid");
}

fn add_line_edge(m: &mut BRepModel, a: VertexId, b: VertexId) -> EdgeId {
    let pa = m.vertices.get(a).expect("a").position;
    let pb = m.vertices.get(b).expect("b").position;
    let cid = m
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    m.edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
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

/// CCW rectangle loop in XY at z=0.
fn outer_rect_loop(m: &mut BRepModel, x0: f64, y0: f64, x1: f64, y1: f64) -> u32 {
    let v0 = m.vertices.add(x0, y0, 0.0);
    let v1 = m.vertices.add(x1, y0, 0.0);
    let v2 = m.vertices.add(x1, y1, 0.0);
    let v3 = m.vertices.add(x0, y1, 0.0);
    let e0 = add_line_edge(m, v0, v1);
    let e1 = add_line_edge(m, v1, v2);
    let e2 = add_line_edge(m, v2, v3);
    let e3 = add_line_edge(m, v3, v0);
    let mut l = Loop::new(0, LoopType::Outer);
    l.add_edge(e0, true);
    l.add_edge(e1, true);
    l.add_edge(e2, true);
    l.add_edge(e3, true);
    m.loops.add(l)
}

/// CW (hole-winding) rectangle loop in XY at z=0.
fn inner_rect_loop(m: &mut BRepModel, x0: f64, y0: f64, x1: f64, y1: f64) -> u32 {
    let v0 = m.vertices.add(x0, y0, 0.0);
    let v1 = m.vertices.add(x0, y1, 0.0);
    let v2 = m.vertices.add(x1, y1, 0.0);
    let v3 = m.vertices.add(x1, y0, 0.0);
    let e0 = add_line_edge(m, v0, v1);
    let e1 = add_line_edge(m, v1, v2);
    let e2 = add_line_edge(m, v2, v3);
    let e3 = add_line_edge(m, v3, v0);
    let mut l = Loop::new(0, LoopType::Inner);
    l.add_edge(e0, true);
    l.add_edge(e1, true);
    l.add_edge(e2, true);
    l.add_edge(e3, true);
    m.loops.add(l)
}

fn build_xy_face(m: &mut BRepModel, outer: u32, inners: &[u32]) -> FaceId {
    let plane = Plane::from_point_normal(Point3::ZERO, Vector3::Z).expect("XY plane");
    let surface_id = m.surfaces.add(Box::new(plane));
    let mut face = Face::new(0, surface_id, outer, FaceOrientation::Forward);
    for &inner in inners {
        face.add_inner_loop(inner);
    }
    m.faces.add(face)
}

fn extrude_opts(distance: f64) -> ExtrudeOptions {
    ExtrudeOptions {
        distance,
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        ..Default::default()
    }
}

fn chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        symmetric: true,
        ..Default::default()
    }
}

/// All outer-shell edges of a solid, de-duplicated.
fn all_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let Some(shell) = model.shells.get(s.outer_shell) else {
        return out;
    };
    for &fid in &shell.faces {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        for lid in face.all_loops() {
            let Some(lp) = model.loops.get(lid) else {
                continue;
            };
            for &eid in &lp.edges {
                if seen.insert(eid) {
                    out.push(eid);
                }
            }
        }
    }
    out
}

/// Straight edges whose midpoint Z is within `eps` of `z` — used to pick the
/// "top" or "bottom" rim of a box/prism robustly across chain steps.
fn straight_edges_at_z(model: &BRepModel, solid: SolidId, z: f64, eps: f64) -> Vec<EdgeId> {
    all_edges(model, solid)
        .into_iter()
        .filter(|&eid| {
            let Some(edge) = model.edges.get(eid) else {
                return false;
            };
            if edge.is_loop() {
                return false;
            }
            let lin = model
                .curves
                .get(edge.curve_id)
                .map(|c| c.is_linear(Tolerance::default()))
                .unwrap_or(false);
            if !lin {
                return false;
            }
            let (Some(a), Some(b)) = (
                model.vertices.get(edge.start_vertex),
                model.vertices.get(edge.end_vertex),
            ) else {
                return false;
            };
            let mz = 0.5 * (a.position[2] + b.position[2]);
            (mz - z).abs() < eps
        })
        .collect()
}

/// The +Z face of a box-like solid (by surface normal), for shell open-face.
fn plus_z_face(m: &BRepModel, solid: SolidId) -> Option<FaceId> {
    let s = m.solids.get(solid)?;
    let shell = m.shells.get(s.outer_shell)?;
    for &fid in &shell.faces {
        let face = m.faces.get(fid)?;
        let surf = m.surfaces.get(face.surface_id)?;
        if let Ok(n) = surf.normal_at(0.5, 0.5) {
            if (n.z - 1.0).abs() < 1e-9 && n.x.abs() < 1e-9 && n.y.abs() < 1e-9 {
                return Some(fid);
            }
        }
    }
    None
}

// ===========================================================================
// 1. OP CHAINS — real-part-like sequences. Each step judged + position noted.
// ===========================================================================

#[test]
fn chain_box_fillet_chamfer() {
    // box → fillet (top 4 edges, r=1.0) → chamfer (bottom 4 edges, d=1.0).
    // Distinct top/bottom edge sets so the two blends do not interact directly;
    // any corruption is from a step planting topology the next op mis-reads.
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 10.0, 10.0, 10.0); // centred: z ∈ [-5,5]
    report(&mut m, solid, "chain[1/3] box base");

    let top = straight_edges_at_z(&m, solid, 5.0, 1e-6);
    match fillet_edges(&mut m, solid, top.clone(), fillet_opts(1.0)) {
        Ok(_) => {
            report(&mut m, solid, "chain[2/3] box→fillet(top4 r=1.0)");
        }
        Err(e) => report_err("chain[2/3] box→fillet(top4 r=1.0)", &e),
    }

    let bot = straight_edges_at_z(&m, solid, -5.0, 1e-6);
    match chamfer_edges(&mut m, solid, bot.clone(), chamfer_opts(1.0)) {
        Ok(_) => {
            report(
                &mut m,
                solid,
                "chain[3/3] box→fillet→chamfer(bot4 d=1.0) FINAL",
            );
        }
        Err(e) => report_err("chain[3/3] box→fillet→chamfer(bot4 d=1.0) FINAL", &e),
    }
}

#[test]
fn chain_box_union_shell() {
    // box ∪ box (corner-octant) → shell open-top wall=1.
    let mut m = BRepModel::new();
    let a = box_solid(&mut m, 8.0, 8.0, 8.0);
    let b = box_solid(&mut m, 8.0, 8.0, 8.0);
    translate_in_place(&mut m, b, Vector3::new(4.0, 4.0, 0.0));
    let u = match boolean_operation(&mut m, a, b, BooleanOp::Union, BooleanOptions::default()) {
        Ok(u) => {
            report(&mut m, u, "chain[1/2] box∪box (corner overlap)");
            u
        }
        Err(e) => {
            report_err("chain[1/2] box∪box (corner overlap)", &e);
            return;
        }
    };
    match plus_z_face(&m, u) {
        Some(top) => match offset_solid(&mut m, u, 1.0, vec![top], OffsetOptions::default()) {
            Ok(r) => {
                report(&mut m, r, "chain[2/2] (box∪box)→shell open-top w=1 FINAL");
            }
            Err(e) => report_err("chain[2/2] (box∪box)→shell open-top w=1 FINAL", &e),
        },
        None => report_err(
            "chain[2/2] (box∪box)→shell: no +Z face",
            &"union lost the planar +Z face",
        ),
    }
}

#[test]
fn chain_box_difference_shell() {
    // box ∖ cylinder (through-hole) → shell open-top wall=1.
    let mut m = BRepModel::new();
    let a = box_solid(&mut m, 12.0, 12.0, 12.0);
    let cyl = cylinder_solid(&mut m, Point3::new(0.0, 0.0, -7.0), Vector3::Z, 3.0, 14.0);
    let d = match boolean_operation(
        &mut m,
        a,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    ) {
        Ok(d) => {
            report(&mut m, d, "chain[1/2] box∖cyl through-hole");
            d
        }
        Err(e) => {
            report_err("chain[1/2] box∖cyl through-hole", &e);
            return;
        }
    };
    match plus_z_face(&m, d) {
        Some(top) => match offset_solid(&mut m, d, 1.0, vec![top], OffsetOptions::default()) {
            Ok(r) => {
                report(&mut m, r, "chain[2/2] (box∖cyl)→shell open-top w=1 FINAL");
            }
            Err(e) => report_err("chain[2/2] (box∖cyl)→shell open-top w=1 FINAL", &e),
        },
        None => report_err(
            "chain[2/2] (box∖cyl)→shell: no +Z face",
            &"difference lost the planar +Z face",
        ),
    }
}

#[test]
fn chain_box_boolean_fillet() {
    // box ∖ cylinder (through-hole) → fillet the top OUTER rim (4 straight edges).
    let mut m = BRepModel::new();
    let a = box_solid(&mut m, 12.0, 12.0, 12.0);
    let cyl = cylinder_solid(&mut m, Point3::new(0.0, 0.0, -7.0), Vector3::Z, 3.0, 14.0);
    let d = match boolean_operation(
        &mut m,
        a,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    ) {
        Ok(d) => {
            report(&mut m, d, "chain[1/2] box∖cyl through-hole");
            d
        }
        Err(e) => {
            report_err("chain[1/2] box∖cyl through-hole", &e);
            return;
        }
    };
    let top = straight_edges_at_z(&m, d, 6.0, 1e-6);
    match fillet_edges(&mut m, d, top.clone(), fillet_opts(1.0)) {
        Ok(_) => report(
            &mut m,
            d,
            &format!(
                "chain[2/2] (box∖cyl)→fillet(top {} edges r=1.0) FINAL",
                top.len()
            ),
        ),
        Err(e) => report_err("chain[2/2] (box∖cyl)→fillet top rim FINAL", &e),
    };
}

#[test]
fn chain_extrude_fillet_boolean() {
    // extrude rect → fillet 4 top edges → union with a boss cylinder.
    let mut m = BRepModel::new();
    let outer = outer_rect_loop(&mut m, 0.0, 0.0, 20.0, 20.0);
    let face = build_xy_face(&mut m, outer, &[]);
    let solid = match extrude_face(&mut m, face, extrude_opts(8.0)) {
        Ok(s) => {
            report(&mut m, s, "chain[1/3] extrude 20x20x8");
            s
        }
        Err(e) => {
            report_err("chain[1/3] extrude 20x20x8", &e);
            return;
        }
    };
    let top = straight_edges_at_z(&m, solid, 8.0, 1e-6);
    if let Err(e) = fillet_edges(&mut m, solid, top.clone(), fillet_opts(1.5)) {
        report_err("chain[2/3] extrude→fillet(top4 r=1.5)", &e);
        return;
    }
    report(&mut m, solid, "chain[2/3] extrude→fillet(top4 r=1.5)");

    let boss = cylinder_solid(&mut m, Point3::new(10.0, 10.0, 6.0), Vector3::Z, 3.0, 6.0);
    match boolean_operation(
        &mut m,
        solid,
        boss,
        BooleanOp::Union,
        BooleanOptions::default(),
    ) {
        Ok(u) => report(&mut m, u, "chain[3/3] (extrude→fillet)∪boss-cyl FINAL"),
        Err(e) => report_err("chain[3/3] (extrude→fillet)∪boss-cyl FINAL", &e),
    };
}

#[test]
fn chain_revolve_boolean_fillet() {
    // revolve a tube profile → difference a radial cylinder (cross-hole) → fillet
    // the resulting top rim. Stresses curved-base boolean then blend.
    let mut m = BRepModel::new();
    let edges = rect_xz(&mut m, 4.0, 8.0, 0.0, 12.0);
    let rev = match revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            angle: TAU,
            segments: 32,
            cap_ends: true,
            ..Default::default()
        },
    ) {
        Ok(r) => {
            report(&mut m, r, "chain[1/3] revolve 360 tube (r 4..8, h 12)");
            r
        }
        Err(e) => {
            report_err("chain[1/3] revolve 360 tube", &e);
            return;
        }
    };
    // Radial cylinder through the tube wall (axis along X).
    let pin = cylinder_solid(&mut m, Point3::new(-12.0, 0.0, 6.0), Vector3::X, 2.0, 24.0);
    let cut = match boolean_operation(
        &mut m,
        rev,
        pin,
        BooleanOp::Difference,
        BooleanOptions::default(),
    ) {
        Ok(c) => {
            report(&mut m, c, "chain[2/3] revolve∖radial-cyl (cross-hole)");
            c
        }
        Err(e) => {
            report_err("chain[2/3] revolve∖radial-cyl (cross-hole)", &e);
            return;
        }
    };
    // Fillet the top circular rim(s).
    let top_circular: Vec<EdgeId> = all_edges(&m, cut)
        .into_iter()
        .filter(|&eid| {
            let Some(edge) = m.edges.get(eid) else {
                return false;
            };
            let circular = m
                .curves
                .get(edge.curve_id)
                .map(|c| !c.is_linear(Tolerance::default()))
                .unwrap_or(false);
            if !circular {
                return false;
            }
            // top rim near z=12
            m.vertices
                .get(edge.start_vertex)
                .map(|v| (v.position[2] - 12.0).abs() < 1e-4)
                .unwrap_or(false)
        })
        .collect();
    if top_circular.is_empty() {
        report_err(
            "chain[3/3] revolve∖cyl→fillet: no top rim edge found",
            &"top circular rim not located",
        );
        return;
    }
    match fillet_edges(&mut m, cut, top_circular.clone(), fillet_opts(0.5)) {
        Ok(_) => report(
            &mut m,
            cut,
            &format!(
                "chain[3/3] (revolve∖cyl)→fillet({} rim edges r=0.5) FINAL",
                top_circular.len()
            ),
        ),
        Err(e) => report_err("chain[3/3] (revolve∖cyl)→fillet rim FINAL", &e),
    };
}

#[test]
fn chain_loft_shell() {
    // loft 3 coaxial circles → shell open-top wall=0.5.
    let mut m = BRepModel::new();
    let c0 = circle_profile(&mut m, Point3::new(0.0, 0.0, 0.0), 10.0);
    let c1 = circle_profile(&mut m, Point3::new(0.0, 0.0, 20.0), 6.0);
    let c2 = circle_profile(&mut m, Point3::new(0.0, 0.0, 40.0), 8.0);
    let lofted = match loft_profiles(
        &mut m,
        vec![c0, c1, c2],
        LoftOptions {
            create_solid: true,
            ..Default::default()
        },
    ) {
        Ok(l) => {
            report(&mut m, l, "chain[1/2] loft 3 circles (10→6→8)");
            l
        }
        Err(e) => {
            report_err("chain[1/2] loft 3 circles", &e);
            return;
        }
    };
    // Open the top planar cap (the loft caps are planar discs).
    let top = plus_z_face(&m, lofted);
    let removed = top.into_iter().collect::<Vec<_>>();
    match offset_solid(&mut m, lofted, 0.5, removed, OffsetOptions::default()) {
        Ok(r) => report(&mut m, r, "chain[2/2] loft→shell open-top w=0.5 FINAL"),
        Err(e) => report_err("chain[2/2] loft→shell open-top w=0.5 FINAL", &e),
    };
}

#[test]
fn chain_plate_with_holes_fillet() {
    // plate (20×12) with two square holes, extruded → fillet the 4 top OUTER
    // edges. Multi-loop topology survives a blend?
    let mut m = BRepModel::new();
    let outer = outer_rect_loop(&mut m, 0.0, 0.0, 20.0, 12.0);
    let hole_a = inner_rect_loop(&mut m, 3.0, 3.0, 6.0, 9.0);
    let hole_b = inner_rect_loop(&mut m, 14.0, 3.0, 17.0, 9.0);
    let face = build_xy_face(&mut m, outer, &[hole_a, hole_b]);
    let plate = match extrude_face(&mut m, face, extrude_opts(4.0)) {
        Ok(p) => {
            report(&mut m, p, "chain[1/2] plate 20x12x4 w/ two square holes");
            p
        }
        Err(e) => {
            report_err("chain[1/2] plate with holes", &e);
            return;
        }
    };
    let top = straight_edges_at_z(&m, plate, 4.0, 1e-6);
    // Restrict to the outer rim (length ≈ 20 or 12) to avoid the hole rims here.
    let outer_top: Vec<EdgeId> = top
        .into_iter()
        .filter(|&eid| {
            let Some(edge) = m.edges.get(eid) else {
                return false;
            };
            let (Some(a), Some(b)) = (
                m.vertices.get(edge.start_vertex),
                m.vertices.get(edge.end_vertex),
            ) else {
                return false;
            };
            let len = ((a.position[0] - b.position[0]).powi(2)
                + (a.position[1] - b.position[1]).powi(2))
            .sqrt();
            len > 11.0 // outer edges are 20 or 12; hole edges are 3 or 6
        })
        .collect();
    match fillet_edges(&mut m, plate, outer_top.clone(), fillet_opts(0.8)) {
        Ok(_) => report(
            &mut m,
            plate,
            &format!(
                "chain[2/2] plate-holes→fillet({} outer-top edges r=0.8) FINAL",
                outer_top.len()
            ),
        ),
        Err(e) => report_err("chain[2/2] plate-holes→fillet outer-top FINAL", &e),
    };
}

// ===========================================================================
// 2. DEGENERATE / near-degenerate inputs.
// ===========================================================================

#[test]
fn degen_near_coincident_faces_union() {
    // Two boxes whose facing planes are 1e-4 apart (a near-coincident, almost-
    // but-not-quite touching union). Stresses the imprint tolerance near the
    // coincident-face cliff without being exactly coincident.
    let mut m = BRepModel::new();
    let a = box_solid(&mut m, 6.0, 6.0, 6.0); // +X face at x=3
    let b = box_solid(&mut m, 6.0, 6.0, 6.0);
    // b's -X face at x=3+1e-4 ⇒ gap 1e-4 between the two facing walls.
    translate_in_place(&mut m, b, Vector3::new(6.0 + 1e-4, 0.0, 0.0));
    for &op in &[BooleanOp::Union, BooleanOp::Difference] {
        let mut mm = BRepModel::new();
        let a2 = box_solid(&mut mm, 6.0, 6.0, 6.0);
        let b2 = box_solid(&mut mm, 6.0, 6.0, 6.0);
        translate_in_place(&mut mm, b2, Vector3::new(6.0 + 1e-4, 0.0, 0.0));
        match boolean_operation(&mut mm, a2, b2, op, BooleanOptions::default()) {
            Ok(r) => report(
                &mut mm,
                r,
                &format!("degen near-coincident(gap 1e-4) {op:?}"),
            ),
            Err(e) => report_err(&format!("degen near-coincident(gap 1e-4) {op:?}"), &e),
        };
    }
    let _ = (a, b);
}

#[test]
fn degen_near_tangent_fillet() {
    // Two fillets on adjacent top edges with r just below the half-face ceiling,
    // so the two round-overs become near-tangent along the shared top face.
    // Box 10×10×10: top face is 10×10; r=4.9 ⇒ opposite round-overs separated by
    // 10-2*4.9 = 0.2 (near-tangent). All 4 top edges → 4 near-tangent blends.
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 10.0, 10.0, 10.0);
    let top = straight_edges_at_z(&m, solid, 5.0, 1e-6);
    match fillet_edges(&mut m, solid, top.clone(), fillet_opts(4.9)) {
        Ok(_) => report(
            &mut m,
            solid,
            &format!(
                "degen near-tangent fillet (4 top edges r=4.9, gap 0.2) [{} edges]",
                top.len()
            ),
        ),
        Err(e) => report_err("degen near-tangent fillet (r=4.9)", &e),
    };
}

#[test]
fn degen_tiny_fillet_on_long_edge() {
    // r=0.01 fillet on a 20-unit edge (radius ≪ edge length). Sub-feature blend
    // — does the tessellator drop / over-collapse the tiny blend strip?
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 20.0, 20.0, 20.0);
    let top = straight_edges_at_z(&m, solid, 10.0, 1e-6);
    match fillet_edges(&mut m, solid, vec![top[0]], fillet_opts(0.01)) {
        Ok(_) => report(&mut m, solid, "degen tiny fillet r=0.01 on 20-unit edge"),
        Err(e) => report_err("degen tiny fillet r=0.01 on 20-unit edge", &e),
    };
}

/// GATE (tiny-fillet watertightness regression). A SUB-tolerance-radius fillet
/// (r=0.01) on ONE edge of a clean 20³ box must be watertight + manifold +
/// oriented + sound — NOT merely reported. Root cause (fixed in
/// `edge_cache::sample_count_from_length_angle` +
/// `compute_face_boundary_sample_count`): the fillet's ~r·√2-long end-cap arc
/// was densified by the angle/curvature constraints to ~16 samples packed into a
/// cluster smaller than the chord tolerance; the trimmed planar neighbour's CDT
/// (resolving relative to the 20-unit host face) collapsed that cluster and
/// DROPPED the cap chord on its side while the fillet face still emitted it,
/// leaving the shared cap edge un-welded — 6 boundary (open) edges,
/// `watertight=false`. The fix collapses a feature whose total chord length is
/// at/below the faceting tolerance to its straight chord (within-tolerance by
/// construction) consistently on BOTH faces. This is a HARD assertion: any
/// regression of the thin-strip weld fails the build (no oracle is weakened —
/// the same `manifold_report` + `certify_solid` the hunt uses).
#[test]
fn gate_tiny_fillet_on_long_edge_is_watertight() {
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 20.0, 20.0, 20.0);
    let top = straight_edges_at_z(&m, solid, 10.0, 1e-6);
    fillet_edges(&mut m, solid, vec![top[0]], fillet_opts(0.01))
        .expect("tiny r=0.01 fillet on a clean box edge must BUILD");

    let mr =
        manifold_report(&mut m, solid, CHORD, WELD).expect("tiny-fillet solid must tessellate");
    assert_eq!(
        mr.boundary_edges, 0,
        "tiny r=0.01 fillet leaks: {} boundary (open) edges (the thin-strip cap weld regressed): {mr:?}",
        mr.boundary_edges
    );
    assert_eq!(mr.nonmanifold_edges, 0, "tiny fillet non-manifold: {mr:?}");
    assert!(mr.oriented, "tiny fillet not consistently oriented: {mr:?}");

    let cert = m.certify_solid(solid);
    assert!(
        cert.is_sound(),
        "tiny r=0.01 fillet certificate not sound (watertight={}): {cert:?}",
        cert.watertight
    );
}

/// COMPANION GATE: normal fillet radii on the SAME edge must remain watertight +
/// sound — the sub-tolerance collapse must NOT spill over and break ordinary
/// fillets. (r=0.2, 1.0, 4.9 all have a cap chord above the harness chord
/// tolerance, so they tessellate their arc fully and the guard never fires.)
#[test]
fn gate_normal_fillet_radii_stay_watertight() {
    for r in [0.2_f64, 1.0, 4.9] {
        let mut m = BRepModel::new();
        let solid = box_solid(&mut m, 20.0, 20.0, 20.0);
        let top = straight_edges_at_z(&m, solid, 10.0, 1e-6);
        fillet_edges(&mut m, solid, vec![top[0]], fillet_opts(r))
            .unwrap_or_else(|e| panic!("fillet r={r} must build: {e:?}"));
        let mr = manifold_report(&mut m, solid, CHORD, WELD)
            .unwrap_or_else(|| panic!("fillet r={r} must tessellate"));
        assert_eq!(mr.boundary_edges, 0, "fillet r={r} leaks: {mr:?}");
        assert_eq!(mr.nonmanifold_edges, 0, "fillet r={r} non-manifold: {mr:?}");
        assert!(mr.oriented, "fillet r={r} not oriented: {mr:?}");
        assert!(
            m.certify_solid(solid).is_sound(),
            "fillet r={r} certificate not sound"
        );
    }
}

#[test]
fn degen_tiny_chamfer_on_long_edge() {
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 20.0, 20.0, 20.0);
    let top = straight_edges_at_z(&m, solid, 10.0, 1e-6);
    match chamfer_edges(&mut m, solid, vec![top[0]], chamfer_opts(0.01)) {
        Ok(_) => report(&mut m, solid, "degen tiny chamfer d=0.01 on 20-unit edge"),
        Err(e) => report_err("degen tiny chamfer d=0.01 on 20-unit edge", &e),
    };
}

#[test]
fn degen_extreme_aspect_box_fillet() {
    // 1000×1×1 sliver box → fillet a long top edge r=0.2. Extreme aspect ratio
    // stresses parametrization / tessellation density on a near-1D solid.
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 1000.0, 1.0, 1.0);
    report(&mut m, solid, "degen extreme-aspect 1000x1x1 box (base)");
    let top = straight_edges_at_z(&m, solid, 0.5, 1e-6);
    // long top edges run along X (length 1000)
    let long: Vec<EdgeId> = top
        .into_iter()
        .filter(|&eid| {
            let Some(edge) = m.edges.get(eid) else {
                return false;
            };
            let (Some(a), Some(b)) = (
                m.vertices.get(edge.start_vertex),
                m.vertices.get(edge.end_vertex),
            ) else {
                return false;
            };
            (a.position[0] - b.position[0]).abs() > 100.0
        })
        .collect();
    if long.is_empty() {
        report_err("degen extreme-aspect fillet: no long top edge", &"none");
        return;
    }
    match fillet_edges(&mut m, solid, vec![long[0]], fillet_opts(0.2)) {
        Ok(_) => report(
            &mut m,
            solid,
            "degen extreme-aspect 1000x1x1 → fillet r=0.2",
        ),
        Err(e) => report_err("degen extreme-aspect 1000x1x1 → fillet r=0.2", &e),
    };
}

#[test]
fn degen_extreme_aspect_box_boolean() {
    // 1000×1×1 sliver ∖ cylinder through the thin dimension.
    let mut m = BRepModel::new();
    let a = box_solid(&mut m, 1000.0, 1.0, 1.0);
    let cyl = cylinder_solid(&mut m, Point3::new(0.0, 0.0, -2.0), Vector3::Z, 0.3, 4.0);
    match boolean_operation(
        &mut m,
        a,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    ) {
        Ok(r) => report(&mut m, r, "degen extreme-aspect 1000x1x1 ∖ cyl(r=0.3)"),
        Err(e) => report_err("degen extreme-aspect 1000x1x1 ∖ cyl(r=0.3)", &e),
    };
}

#[test]
fn degen_near_zero_thickness_shell() {
    // Shell a box to t=0.01 (near-zero wall). Self-intersection / collapse class.
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 10.0, 10.0, 10.0);
    match plus_z_face(&m, solid) {
        Some(top) => match offset_solid(&mut m, solid, 0.01, vec![top], OffsetOptions::default()) {
            Ok(r) => report(&mut m, r, "degen near-zero-thickness shell t=0.01 open-top"),
            Err(e) => report_err("degen near-zero-thickness shell t=0.01 open-top", &e),
        },
        None => report_err("degen near-zero shell: no +Z face", &"none"),
    };
}

#[test]
fn degen_thick_shell_self_overlap() {
    // Shell a box with a wall thicker than HALF the box (t=6 on a 10-box) — the
    // inner offset surfaces cross the centre and self-intersect. Should be a
    // refusal OR a flagged cert; a clean BUILT+SOUND here would be a false-pass.
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 10.0, 10.0, 10.0);
    match plus_z_face(&m, solid) {
        Some(top) => match offset_solid(&mut m, solid, 6.0, vec![top], OffsetOptions::default()) {
            Ok(r) => report(
                &mut m,
                r,
                "degen over-thick shell t=6 on 10-box (must self-intersect)",
            ),
            Err(e) => report_err("degen over-thick shell t=6 on 10-box", &e),
        },
        None => report_err("degen over-thick shell: no +Z face", &"none"),
    };
}

// ===========================================================================
// 3. ITERATION-3 DID-NOT-BUILD cases — real bug masked as reject, or honest?
// ===========================================================================

#[test]
fn iter3_partial_revolve_no_caps() {
    // 180° revolve with cap_ends=false ⇒ open ends. An open shell is NOT a closed
    // solid. EXPECTED: either an honest reject (legit — caps required for a solid)
    // OR a BUILT result that is correctly flagged non-watertight. A BUILT+SOUND
    // here would be a false-pass (claiming an open shell is sound).
    let mut m = BRepModel::new();
    let edges = rect_xz(&mut m, 2.0, 5.0, 0.0, 3.0);
    match revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            angle: PI,
            segments: 24,
            cap_ends: false,
            ..Default::default()
        },
    ) {
        Ok(r) => report(&mut m, r, "iter3 partial-revolve 180 NO-CAPS (open ends)"),
        Err(e) => report_err("iter3 partial-revolve 180 NO-CAPS (open ends)", &e),
    };
}

#[test]
fn iter3_revolve_axis_touching_profile() {
    // Profile touching the axis (x0=0) ⇒ revolve makes a solid disc/cone with a
    // degenerate-radius seam at the axis. EXPECTED: BUILT+SOUND if the axis seam
    // is handled, else a flagged defect at the seam. A reject would be suspicious
    // (this is a legitimate solid-of-revolution).
    let mut m = BRepModel::new();
    let edges = rect_xz(&mut m, 0.0, 4.0, 0.0, 3.0);
    match revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            angle: PI,
            segments: 24,
            cap_ends: true,
            ..Default::default()
        },
    ) {
        Ok(r) => report(&mut m, r, "iter3 revolve 180 AXIS-TOUCHING profile (x0=0)"),
        Err(e) => report_err("iter3 revolve 180 AXIS-TOUCHING profile (x0=0)", &e),
    };
}

#[test]
fn iter3_revolve_full_axis_touching() {
    // Same but full 360° — the classic solid-of-revolution (e.g. a cone/dome).
    let mut m = BRepModel::new();
    let edges = rect_xz(&mut m, 0.0, 4.0, 0.0, 6.0);
    match revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            angle: TAU,
            segments: 32,
            cap_ends: true,
            ..Default::default()
        },
    ) {
        Ok(r) => report(
            &mut m,
            r,
            "iter3 revolve 360 AXIS-TOUCHING profile (x0=0) cylinder",
        ),
        Err(e) => report_err("iter3 revolve 360 AXIS-TOUCHING profile (x0=0)", &e),
    };
}

#[test]
fn iter3_shell_fully_closed_cavity() {
    // Shell with NO removed faces ⇒ a fully-enclosed hollow cavity (a void inside
    // a solid). This is a valid 2-shell solid (outer + inner shell), but a naive
    // Euler check on a single shell rejects it (negative-genus-looking χ).
    // EXPECTED: BUILT (two-shell solid) if the kernel models cavities, else a
    // reject — which would be a real-bug-masked-as-reject (cavities are valid).
    let mut m = BRepModel::new();
    let solid = box_solid(&mut m, 10.0, 10.0, 10.0);
    match offset_solid(&mut m, solid, 1.0, vec![], OffsetOptions::default()) {
        Ok(r) => report(
            &mut m,
            r,
            "iter3 shell FULLY-CLOSED cavity (no removed faces)",
        ),
        Err(e) => report_err("iter3 shell FULLY-CLOSED cavity (no removed faces)", &e),
    };
}

// ===========================================================================
// Guard — runs last alphabetically. The hunt REPORTS corrupt verdicts (counted,
// not asserted). Only a hard PANIC fails the build, since a crash is a finding.
// We catch panics per-case is not feasible here (ops borrow &mut model), so a
// panic propagates and fails its own test — which the runner surfaces. This
// summary just prints the corrupt tally.
// ===========================================================================

#[test]
fn zz_summary() {
    let n = CORRUPT_COUNT.lock().map(|g| *g).unwrap_or(0);
    let p = PANIC_COUNT.lock().map(|g| *g).unwrap_or(0);
    println!("---- op_chain_degenerate_stress corrupt-verdicts-so-far (informational): {n}; panics-recorded: {p}");
}
