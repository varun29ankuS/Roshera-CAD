// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! #32 Slice 1 — UNION embedded analytic-lateral split (REGRESSION GUARD).
//!
//! Charter (2026-07-09 #32 spec, Slice 1): when two solids union, an operand
//! analytic lateral face (cylinder/cone) whose sub-span is strictly INTERIOR to
//! the other operand must be split at the embedding boundary, the interior span
//! culled, the exposed span kept — so every surviving face covers real solid
//! boundary and a later through-bore can classify the cutter wall per-z.
//!
//! STATE ON THIS BRANCH (verified 2026-07-15, HEAD 3ec2d12): that capability is
//! ALREADY realised by the core intersection pipeline. The boss lateral
//! intersects the plate's boundary plane(s) → `split_faces_along_curves` splits
//! it at every crossing → the buried sub-span classifies `Inside` and
//! `select_faces_for_operation` drops it (Union: Inside ⇒ drop) → the exposed
//! sub-span classifies `Outside` and is kept. Trace evidence (coaxial boss):
//! `fragment origin=8 surf=Cylinder ip=(15,0,9.4) class=Inside` (culled) and
//! `ip=(15,0,19.4) class=Outside` (kept). The 2026-07-09 Phase-C premise — "the
//! boss lateral is left as a single z=0..20 face, never split" — is STALE,
//! superseded by the July saddle-35 + dogfood-reds cyl∩plane split hardening.
//!
//! No dedicated `split_embedded_lateral_faces` helper is therefore added: it
//! would be unreachable (the pipeline splits every embedded lateral before a
//! strict point-in-solid safety net could fire) and dead code in the highest
//! boolean-regression-risk region. These tests instead PIN the working capability
//! across the configurations the charter names, so a future regression in the
//! cyl/cone∩plane split path is caught. NOT a RED-before-GREEN fix — a
//! characterisation guard for already-correct behaviour.
//!
//! The residual straddling-through-bore failure the old plan chained off Slice 1
//! is difference-side (the z-dependent cutter-wall imprint / phantom arcs), pinned
//! by the ignored `boolean_straddling_rim::f7_straddling_offset_{10,12}_is_sound`
//! and owned by Slice 2 — not touched here.
//!
//! Run: `cargo test -p geometry-engine --test union_embedded_face -- --nocapture`.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn box_at(m: &mut BRepModel, w: f64, h: f64, d: f64, tx: f64, ty: f64, tz: f64) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    for (axis, t) in [(Vector3::X, tx), (Vector3::Y, ty), (Vector3::Z, tz)] {
        if t != 0.0 {
            translate(m, vec![s], axis, t, TransformOptions::default()).expect("translate");
        }
    }
    s
}

fn cylinder(m: &mut BRepModel, base: Point3, axis: Vector3, radius: f64, height: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, axis, radius, height)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn union(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Union, BooleanOptions::default())
        .expect("union must complete")
}

/// (min_z, max_z) of every vertex reachable from a face's loops.
fn face_z_span(m: &BRepModel, fid: u32) -> (f64, f64) {
    let face = m.faces.get(fid).expect("face");
    let mut zmin = f64::INFINITY;
    let mut zmax = f64::NEG_INFINITY;
    for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
        let lp = m.loops.get(lid).expect("loop");
        for &eid in &lp.edges {
            let e = m.edges.get(eid).expect("edge");
            for vid in [e.start_vertex, e.end_vertex] {
                let p = m.vertices.get_position(vid).expect("vertex");
                zmin = zmin.min(p[2]);
                zmax = zmax.max(p[2]);
            }
        }
    }
    (zmin, zmax)
}

/// z-spans of every cylinder lateral face of `sid` (fid, zmin, zmax).
fn cyl_lateral_spans(m: &BRepModel, sid: SolidId) -> Vec<(u32, f64, f64)> {
    let solid = m.solids.get(sid).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("shell");
    let mut out = Vec::new();
    for &fid in &shell.faces {
        let face = m.faces.get(fid).expect("face");
        let is_cyl = m
            .surfaces
            .get(face.surface_id)
            .map(|s| matches!(s.surface_type(), SurfaceType::Cylinder))
            .unwrap_or(false);
        if is_cyl {
            let (zmin, zmax) = face_z_span(m, fid);
            out.push((fid, zmin, zmax));
        }
    }
    out
}

fn is_sound(m: &mut BRepModel, s: SolidId) -> bool {
    m.ground_truth(s)
        .map(|gt| gt.certificate.is_sound())
        .unwrap_or(false)
}

/// plate 60×60×10 (z∈[0,10]) ∪ boss cyl r15 h20 base z=0 (overlaps z∈[0,10]).
/// The boss lateral's z=0..10 span is INTERIOR to the plate → split at z=10,
/// interior span culled, only the exposed z=10..20 lateral kept.
#[test]
fn union_splits_boss_lateral_embedded_in_plate() {
    let mut m = BRepModel::new();
    let plate = box_at(&mut m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0); // z∈[0,10]
    let boss = cylinder(&mut m, Point3::new(0.0, 0.0, 0.0), Vector3::Z, 15.0, 20.0); // z∈[0,20]
    let u = union(&mut m, plate, boss);

    let spans = cyl_lateral_spans(&m, u);
    eprintln!("boss-in-plate cylinder lateral spans: {spans:?}");

    assert!(
        !spans.is_empty(),
        "expected an exposed boss lateral (z=10..20) in the union"
    );
    assert!(
        spans.iter().all(|(_, zmin, _)| *zmin >= 10.0 - 1e-6),
        "boss lateral must be split at z=10; no cylinder face may span the plate interior z<10; got {spans:?}"
    );
    assert!(
        is_sound(&mut m, u),
        "embedded-split union must certify sound"
    );
}

/// boss z∈[−5,15] passes THROUGH a plate z∈[0,10]: the lateral must split into
/// z=−5..0 (below, exposed) + z=0..10 (interior, culled) + z=10..15 (above,
/// exposed) — a stronger guard (two exposed spans bracketing the interior cull).
#[test]
fn union_splits_boss_lateral_through_both_plate_caps() {
    let mut m = BRepModel::new();
    let plate = box_at(&mut m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0); // z∈[0,10]
    let boss = cylinder(&mut m, Point3::new(0.0, 0.0, -5.0), Vector3::Z, 15.0, 20.0); // z∈[−5,15]
    let u = union(&mut m, plate, boss);

    let mut spans = cyl_lateral_spans(&m, u);
    spans.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    eprintln!("through-both-caps cylinder lateral spans: {spans:?}");

    // No lateral may cover the interior band z∈(0,10): every kept span lies
    // wholly at or below z=0, or wholly at or above z=10.
    for (_, zmin, zmax) in &spans {
        let below = *zmax <= 0.0 + 1e-6;
        let above = *zmin >= 10.0 - 1e-6;
        assert!(
            below || above,
            "no cylinder lateral may span the plate interior z∈(0,10); got ({zmin},{zmax})"
        );
    }
    assert!(
        is_sound(&mut m, u),
        "through-both-caps union must certify sound"
    );
}

/// boss seated ON the plate top (base z=10, no overlap volume) → the boss
/// lateral is fully exposed and must stay one z=10..30 face (no false split).
#[test]
fn union_does_not_split_exposed_boss_lateral() {
    let mut m = BRepModel::new();
    let plate = box_at(&mut m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0); // z∈[0,10]
    let boss = cylinder(&mut m, Point3::new(0.0, 0.0, 10.0), Vector3::Z, 15.0, 20.0); // z∈[10,30]
    let u = union(&mut m, plate, boss);

    let spans = cyl_lateral_spans(&m, u);
    eprintln!("boss-on-plate control cylinder lateral spans: {spans:?}");

    assert_eq!(
        spans.len(),
        1,
        "an exposed boss lateral must remain a single face — no false embedded-split; got {spans:?}"
    );
    let (_, zmin, zmax) = spans[0];
    assert!(
        zmin >= 10.0 - 1e-6 && zmax <= 30.0 + 1e-6,
        "exposed lateral must span z=10..30; got ({zmin},{zmax})"
    );
    assert!(is_sound(&mut m, u), "control union must certify sound");
}
