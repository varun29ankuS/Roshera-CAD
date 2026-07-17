// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! #70 CHAMFER-CROSSES-FILLET — the sequential order: fillet an edge FIRST,
//! then chamfer an edge whose chain terminates against the fillet's
//! cylindrical surface.
//!
//! Canonical fixture: box 10³ centred on the origin → fillet the vertical
//! edge at (5, 5, ·) with r = 1 → chamfer the (now trimmed) top edge along
//! (y = 5, z = 5) with d = 0.5. The chamfer wall lies in the plane
//! y + z = 9.5, which crosses the fillet cylinder (axis (4, 4, ·), r = 1);
//! plane × cylinder is an ELLIPTICAL ARC — analytic, from
//! (4 + √3⁄2, 4.5, 5) [θ = 30°, on the top face's fillet arc] to
//! (4, 5, 4.5) [θ = 90°, on the y = 5 tangent seam]. The exact enclosed
//! volume for this fixture is closed-form (see [`canonical_expected_volume`]).
//!
//! Distinct from the 1C2F mixed-kind CORNER (chamfer-first, solved by
//! CF-γ.7 `retract_mixed_1c2f_corner`): here there is no shared corner
//! vertex at chamfer time — the fillet already consumed it. The chamfer's
//! far end must TERMINATE AGAINST A CURVED FACE, not cap a planar corner.

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;
use blend_fixtures::*;

use std::f64::consts::PI;

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::Tolerance;
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::OperationError;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

const CERT_CHORD: f64 = 0.1;
const WELD_EPS: f64 = 1e-6;

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

/// The straight edge whose two endpoints both satisfy the predicate — used to
/// find the trimmed top edge after the fillet consumed one of its original
/// endpoints.
fn straight_edge_where(
    model: &BRepModel,
    mut pred: impl FnMut([f64; 3]) -> bool,
) -> Option<EdgeId> {
    for (id, edge) in model.edges.iter() {
        let a = model.vertices.get(edge.start_vertex)?.position;
        let b = model.vertices.get(edge.end_vertex)?.position;
        if pred(a) && pred(b) {
            let curve = model.curves.get(edge.curve_id)?;
            if curve.type_name() == "Line" {
                return Some(id);
            }
        }
    }
    None
}

fn edge_between(model: &BRepModel, a: [f64; 3], b: [f64; 3]) -> EdgeId {
    let near = |p: [f64; 3], q: [f64; 3]| {
        (p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9 && (p[2] - q[2]).abs() < 1e-9
    };
    for (id, edge) in model.edges.iter() {
        let pa = model.vertices.get(edge.start_vertex).expect("v").position;
        let pb = model.vertices.get(edge.end_vertex).expect("v").position;
        if (near(pa, a) && near(pb, b)) || (near(pa, b) && near(pb, a)) {
            return id;
        }
    }
    panic!("no edge between {a:?} and {b:?}");
}

fn volume(model: &mut BRepModel, id: SolidId) -> f64 {
    model
        .mass_properties_for(id)
        .expect("mass properties")
        .volume
}

/// Full-stack soundness: B-Rep valid + welded mesh closed 2-manifold oriented
/// + self-certificate sound. Panics with a precise defect line on failure.
fn assert_sound(model: &mut BRepModel, id: SolidId, label: &str) {
    let brep = validate_solid_scoped(model, id, Tolerance::default(), ValidationLevel::Standard);
    assert!(
        brep.is_valid,
        "{label}: B-Rep invalid: {:?}",
        brep.errors.iter().take(4).collect::<Vec<_>>()
    );
    let mesh = manifold_report(model, id, CERT_CHORD, WELD_EPS).expect("tessellates");
    assert!(
        mesh.boundary_edges == 0 && mesh.nonmanifold_edges == 0 && mesh.oriented,
        "{label}: mesh not closed-2-manifold-oriented: boundary={} nonmanifold={} oriented={} \
         euler={} components={}",
        mesh.boundary_edges,
        mesh.nonmanifold_edges,
        mesh.oriented,
        mesh.euler_characteristic,
        mesh.components
    );
    let cert = model.certify_solid(id);
    assert!(
        cert.is_sound(),
        "{label}: self-certificate NOT sound: brep={} wt={} manif={} orient={} self_int_free={} \
         tess_clean={} mesh_q_clean={}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.self_intersection_free,
        cert.tessellation.clean,
        cert.mesh_quality.clean,
    );
}

/// Exact enclosed volume of the canonical fixture (box 10³, fillet r = 1 on
/// one vertical edge, chamfer d = 0.5 on the adjacent top edge, chamfer
/// terminating against the fillet cylinder):
///
///   V = 1000 − 10·(1 − π/4)                 (fillet: quarter-round prism)
///        − ½·d²·10 + V_ov                    (chamfer wedge, minus overlap)
///
/// V_ov = wedge ∩ fillet-removed region, in two parts (u = x − 4):
///   u ∈ [0, √3/2]  — wedge triangle clipped by the fillet circle:
///                    ½∫₀^{√3/2} (√(1−u²) − 1 + u²) du = π/12 − √3/8
///   u ∈ (√3/2, 1]  — the WHOLE wedge triangle (area ⅛) is already
///                    outside the circle: ⅛·(1 − √3/2)
///
///   V_ov = π/12 − √3/8 + (1 − √3/2)/8 ≈ 0.0620398
fn overlap_wedge_fillet() -> f64 {
    PI / 12.0 - 3.0f64.sqrt() / 8.0 + (1.0 - 3.0f64.sqrt() / 2.0) / 8.0
}

fn canonical_expected_volume() -> f64 {
    1000.0 - 10.0 * (1.0 - PI / 4.0) - 1.25 + overlap_wedge_fillet()
}

/// Build the canonical fixture: returns (model, solid, mesh volume after the
/// fillet, chamfer result) — fillet r=1 on the vertical edge at (5,5,·), then
/// chamfer d on the trimmed top edge (y=5, z=5). The pre-chamfer mesh volume
/// lets gates assert the chamfer-step DELTA against the closed form, so the
/// fillet's own mesh-chord error cancels out of the oracle.
fn build_canonical(
    d: f64,
    r: f64,
) -> (
    BRepModel,
    SolidId,
    f64,
    Result<Vec<geometry_engine::primitives::face::FaceId>, OperationError>,
) {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 10.0, 10.0, 10.0);
    let fe = edge_between(&mut model, [5.0, 5.0, -5.0], [5.0, 5.0, 5.0]);
    fillet_edges(&mut model, solid, vec![fe], fillet_opts(r)).expect("fillet vertical edge");
    let v_fillet = volume(&mut model, solid);

    // The top edge along (y=5, z=5) — trimmed by the fillet at x = 5 − r.
    let ce = straight_edge_where(&model, |p| {
        (p[1] - 5.0).abs() < 1e-9 && (p[2] - 5.0).abs() < 1e-9
    })
    .expect("trimmed top edge exists");
    let res = chamfer_edges(&mut model, solid, vec![ce], chamfer_opts(d));
    (model, solid, v_fillet, res)
}

// ---------------------------------------------------------------------------
// GATE (a) — the canonical crossing fixture builds ground-truth-SOUND with
// volume oracle agreement.
// ---------------------------------------------------------------------------
#[test]
fn canonical_crossing_builds_sound_with_exact_volume() {
    let (mut model, solid, v_fillet, res) = build_canonical(0.5, 1.0);
    match &res {
        Ok(_) => {}
        Err(e) => panic!("#70 canonical crossing chamfer REFUSED: {e:?}"),
    }
    assert_sound(&mut model, solid, "canonical fillet→chamfer crossing");

    let v = volume(&mut model, solid);

    // Chamfer-step DELTA vs the closed form. The wedge prism is ½d²·10 =
    // 1.25, less the overlap the fillet already removed: the fillet's own
    // mesh-chord error is common to both measurements and cancels, so this
    // bar is tight (observed residual ≈ 4.9e-4 at default chord).
    let removed = v_fillet - v;
    let removed_expected = 1.25 - overlap_wedge_fillet();
    let delta_resid = (removed - removed_expected).abs();
    assert!(
        delta_resid < 5e-3,
        "chamfer removed {removed:.6} vs closed-form {removed_expected:.6} \
         (residual {delta_resid:.2e})"
    );

    // Absolute closed-form agreement (observed residual ≈ 6.1e-4).
    let expected = canonical_expected_volume();
    let resid = (v - expected).abs();
    assert!(
        resid < 5e-3,
        "volume {v:.6} vs closed-form {expected:.6} (residual {resid:.2e})"
    );
}

// ---------------------------------------------------------------------------
// GATE (c) — chamfer chain crossing TWO fillets (both ends terminate against
// a fillet cylinder).
// ---------------------------------------------------------------------------
#[test]
fn crossing_two_fillets_both_ends_sound() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 10.0, 10.0, 10.0);
    let f1 = edge_between(&mut model, [5.0, 5.0, -5.0], [5.0, 5.0, 5.0]);
    let f2 = edge_between(&mut model, [-5.0, 5.0, -5.0], [-5.0, 5.0, 5.0]);
    fillet_edges(&mut model, solid, vec![f1, f2], fillet_opts(1.0)).expect("fillet both");

    let v_fillets = volume(&mut model, solid);
    let ce = straight_edge_where(&model, |p| {
        (p[1] - 5.0).abs() < 1e-9 && (p[2] - 5.0).abs() < 1e-9
    })
    .expect("doubly-trimmed top edge");
    chamfer_edges(&mut model, solid, vec![ce], chamfer_opts(0.5)).expect("crossing chamfer");
    assert_sound(&mut model, solid, "chamfer crossing two fillets");

    // Chamfer-step delta: full wedge prism less BOTH end overlaps with the
    // fillet-removed regions (mirror symmetry; observed residual ≈ 9.7e-4).
    let v = volume(&mut model, solid);
    let removed = v_fillets - v;
    let removed_expected = 1.25 - 2.0 * overlap_wedge_fillet();
    let delta_resid = (removed - removed_expected).abs();
    assert!(
        delta_resid < 5e-3,
        "two-fillet chamfer removed {removed:.6} vs closed-form {removed_expected:.6} \
         (residual {delta_resid:.2e})"
    );

    // Absolute closed-form agreement.
    let expected = 1000.0 - 2.0 * 10.0 * (1.0 - PI / 4.0) - 1.25 + 2.0 * overlap_wedge_fillet();
    let resid = (v - expected).abs();
    assert!(
        resid < 5e-3,
        "two-fillet volume {v:.6} vs closed-form {expected:.6} (residual {resid:.2e})"
    );
}

// ---------------------------------------------------------------------------
// GATE (b) — order independence: the reverse order (chamfer the top edge
// first, then fillet the vertical edge so the FILLET chain crosses the
// chamfer wall) must produce a sound solid or refuse TYPED — never emit an
// unsound solid and never panic.
// ---------------------------------------------------------------------------
#[test]
fn reverse_order_sound_or_typed_refusal() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 10.0, 10.0, 10.0);
    let ce = edge_between(&mut model, [-5.0, 5.0, 5.0], [5.0, 5.0, 5.0]);
    chamfer_edges(&mut model, solid, vec![ce], chamfer_opts(0.5)).expect("chamfer top edge");
    assert_sound(&mut model, solid, "box→chamfer");

    let before = volume(&mut model, solid);
    let fe = straight_edge_where(&model, |p| {
        (p[0] - 5.0).abs() < 1e-9 && (p[1] - 5.0).abs() < 1e-9
    })
    .expect("trimmed vertical edge");
    match fillet_edges(&mut model, solid, vec![fe], fillet_opts(1.0)) {
        Ok(_) => {
            assert_sound(&mut model, solid, "chamfer→fillet (reverse order)");
            let v = volume(&mut model, solid);
            assert!(
                v < before && v > before - 3.0,
                "reverse-order fillet volume {v:.6} implausible (before {before:.6})"
            );
        }
        Err(OperationError::BlendFailed(f)) => {
            // Typed refusal is acceptable; model must be rolled back sound.
            eprintln!("reverse order refused (typed): {f:?}");
            assert_sound(&mut model, solid, "reverse order after rollback");
            let v = volume(&mut model, solid);
            assert!(
                (v - before).abs() < 1e-9,
                "rollback must restore volume exactly: {v} vs {before}"
            );
        }
        Err(e) => panic!("reverse order failed UNTYPED: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// GATE (d)/(4) — honest refusals for genuinely degenerate configurations.
// ---------------------------------------------------------------------------

/// Chamfer setback ≥ the fillet radius: the chamfer wall would cross the
/// ENTIRE fillet zone and continue into the far tangent face — out of scope,
/// must refuse typed and roll back.
#[test]
fn chamfer_wider_than_fillet_zone_refuses_typed() {
    let (mut model, solid, v_fillet, res) = build_canonical(1.5, 1.0);
    match res {
        Ok(_) => panic!("d=1.5 > r=1 chamfer must refuse (crosses entire fillet zone)"),
        Err(OperationError::BlendFailed(f)) => {
            eprintln!("wider-than-zone refused (typed): {f:?}");
        }
        Err(e) => panic!("expected typed BlendFailed, got {e:?}"),
    }
    // Rollback soundness: the pre-chamfer solid restored exactly.
    assert_sound(&mut model, solid, "after wider-than-zone rollback");
    let v = volume(&mut model, solid);
    assert!(
        (v - v_fillet).abs() < 1e-9,
        "rollback must restore the fillet-only volume exactly: {v} vs {v_fillet}"
    );
}

/// Tangential grazing (d == r): the crossing arc degenerates to a tangent
/// point — must refuse typed, not emit a degenerate sliver.
#[test]
fn tangential_grazing_refuses_typed() {
    let (mut model, solid, _v_fillet, res) = build_canonical(1.0, 1.0);
    match res {
        Ok(_) => panic!("d == r grazing chamfer must refuse (degenerate tangency)"),
        Err(OperationError::BlendFailed(f)) => {
            eprintln!("grazing refused (typed): {f:?}");
        }
        Err(e) => panic!("expected typed BlendFailed, got {e:?}"),
    }
    assert_sound(&mut model, solid, "after grazing rollback");
}

// ---------------------------------------------------------------------------
// ROOT-CAUSE PIN — `Arc::closest_point` on a NEGATIVE-sweep arc. The fillet
// cap arcs sweep clockwise (negative sweep); the historical implementation
// wrapped the angular offset unconditionally into [0, 2π] before dividing by
// the (negative) sweep, so EVERY projection onto such an arc clamped to
// t = 0 (the arc start). That broke the #70 planner's rail∩arc acceptance
// (and any retrim projecting onto a fillet cap arc). Pins the fixed wrap.
// ---------------------------------------------------------------------------
#[test]
fn arc_closest_point_handles_negative_sweep() {
    use geometry_engine::math::{Point3, Vector3};
    use geometry_engine::primitives::curve::{Arc, Curve};
    use std::f64::consts::FRAC_PI_2;

    // Quarter arc from 90° down to 0° (sweep −90°) about +Z, r = 1 — the
    // exact shape of a fillet end-cap arc.
    let arc = Arc::with_x_axis(
        Point3::new(0.0, 0.0, 0.0),
        Vector3::Z,
        Vector3::X,
        1.0,
        FRAC_PI_2,
        -FRAC_PI_2,
    )
    .expect("arc");

    // A point ON the arc at 30°: the projection must return the point
    // itself at an INTERIOR parameter (t = (90 − 30)/90 = 2/3).
    let a30 = 30f64.to_radians();
    let p = Point3::new(a30.cos(), a30.sin(), 0.0);
    let (t, proj) = arc
        .closest_point(&p, Tolerance::default())
        .expect("closest_point");
    assert!(
        (proj - p).magnitude() < 1e-9,
        "projection {proj:?} must equal the on-arc query point {p:?} (t={t})"
    );
    assert!(
        (t - 2.0 / 3.0).abs() < 1e-9,
        "interior on-arc point must project to t=2/3, got {t}"
    );
}

// ---------------------------------------------------------------------------
// GATE (e) — determinism: 5 identical runs produce identical topology.
// ---------------------------------------------------------------------------
#[test]
fn canonical_crossing_deterministic_5x() {
    let mut hashes = Vec::new();
    for _ in 0..5 {
        let (model, solid, _v_fillet, res) = build_canonical(0.5, 1.0);
        res.expect("canonical crossing builds");
        hashes.push(topology_hash(&model, solid));
    }
    assert!(
        hashes.windows(2).all(|w| w[0] == w[1]),
        "non-deterministic topology across 5 runs: {hashes:?}"
    );
}
