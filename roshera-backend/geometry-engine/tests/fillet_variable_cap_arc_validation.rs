// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Cross-section validation for variable-radius fillet cap arcs
//! (Task #84).
//!
//! For a constant-radius fillet, the rolling-ball cross-section at the
//! V0 and V1 caps is exactly a planar circular arc of the constant
//! radius, sitting in the plane perpendicular to the edge tangent at
//! that cap. The four-sided blend loop's cap edges are built directly
//! as those arcs and are guaranteed to lie on the swept fillet surface
//! at `u = 0` and `u = 1`.
//!
//! For a variable-radius fillet `(start_radius → end_radius)` with a
//! linear ramp the situation is more subtle. The rolling-ball surface
//! itself is still well-defined — the rolling sphere's centre traces
//! the spine and the cross-section at each `u` is a circle of the
//! interpolated radius. However, because `dr/du ≠ 0`, the cross-section
//! plane is no longer strictly perpendicular to the spine tangent —
//! the surface "tilts" in proportion to the radius gradient. A planar
//! circular `Arc` cap in the perpendicular plane would drift off the
//! surface.
//!
//! The production fix (this commit) replaces the perpendicular-plane
//! `Arc` cap on the variable-radius path with a NURBS curve sampled
//! along the swept-surface boundary — `u = u_min` / `u = u_max`,
//! `v ∈ [v_min, v_max]` — so the cap lies on the surface by
//! construction. These tests pin that invariant.
//!
//! Assertions:
//!   1. Sampling each cap edge at `t ∈ {0, 0.5, 1}` yields a point
//!      that lies on the variable-radius fillet surface within
//!      `≈ tessellation tolerance` (`1e-3` in plane units).
//!   2. The closest-point `(u, v)` returned by the surface lies on the
//!      `u = u_min` (resp. `u = u_max`) boundary within a small
//!      parameter tolerance — i.e. each cap edge sits along the
//!      surface's parametric boundary, not on some interior iso-curve.
//!   3. The cap edges start and end exactly at the trim-curve
//!      endpoints (contact1/contact2 at V0 and V1), pinning the
//!      blend loop's topological wiring.

use geometry_engine::math::Tolerance;
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(w, h, d)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

fn first_open_edge(model: &BRepModel) -> EdgeId {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| if !edge.is_loop() { Some(id) } else { None })
        .next()
        .expect("box must have at least one open edge")
}

/// Identify the cap edges in a blend face's outer loop by chord
/// length. The 4-sided blend loop has exactly two trim edges (running
/// from V0 to V1 along the adjacent faces, chord ≈ edge length) and
/// two cap edges (closing the loop at each end of the original edge,
/// chord ≈ rolling-ball cross-section diameter). Sorting the 4 edges
/// by endpoint chord and taking the 2 shortest robustly identifies the
/// caps regardless of curve primitive type (`Arc` for constant radius,
/// `NurbsCurve` for variable / function radius after the Task #84
/// production fix).
fn cap_edges(model: &BRepModel, fillet_face: FaceId) -> Vec<EdgeId> {
    let face = model.faces.get(fillet_face).expect("fillet face exists");
    let outer = model
        .loops
        .get(face.outer_loop)
        .expect("fillet outer loop exists");
    let mut by_chord: Vec<(EdgeId, f64)> = outer
        .edges
        .iter()
        .map(|&eid| {
            let edge = model.edges.get(eid).expect("loop edge exists");
            let v0 = model.vertices.get(edge.start_vertex).expect("v0").position;
            let v1 = model.vertices.get(edge.end_vertex).expect("v1").position;
            let chord =
                ((v1[0] - v0[0]).powi(2) + (v1[1] - v0[1]).powi(2) + (v1[2] - v0[2]).powi(2))
                    .sqrt();
            (eid, chord)
        })
        .collect();
    by_chord.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    by_chord.into_iter().take(2).map(|(eid, _)| eid).collect()
}

#[test]
fn variable_radius_cap_edges_lie_on_fillet_surface() {
    // 10×10×10 box, variable fillet 0.4 → 0.8 on the first open edge.
    // The radius ramp is significant enough (2× gradient) that any
    // out-of-surface offset in the cap would show up as a real
    // distance, not just floating-point noise. Edge length 10.0,
    // half-bound 5.0 — both endpoint radii satisfy precondition D.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 10.0, 10.0, 10.0);
    let edge = first_open_edge(&model);

    // `validate_result: false` — the subject here is the Task #84
    // cap-arc CONSTRUCTION, which requires the op to land so the arcs
    // can be inspected. The unequal-end variable band itself does not
    // yet weld watertight against its neighbours (pre-existing kernel
    // gap; the certificate has always reported these solids
    // watertight=false), and the D-1 geometric-closure post-flight now
    // honestly refuses that open result on the default path. Opting
    // out of the post-flight keeps this geometry pin alive; the
    // closure refusal for this family is pinned in
    // `api-server::fillet_radius_harness::linear_profile_drives_kernel_variable_endpoints`.
    let opts = FilletOptions {
        fillet_type: FilletType::Variable(0.4, 0.8),
        radius: 0.4,
        propagation: PropagationMode::None,
        common: geometry_engine::operations::CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let faces = fillet_edges(&mut model, solid, vec![edge], opts)
        .expect("variable-radius fillet on box edge succeeds");
    assert_eq!(
        faces.len(),
        1,
        "expected exactly one variable-radius fillet face, got {}",
        faces.len()
    );
    let fillet_face = faces[0];

    let caps = cap_edges(&model, fillet_face);
    assert_eq!(
        caps.len(),
        2,
        "variable-radius fillet blend loop must carry exactly two cap edges; got {}",
        caps.len()
    );

    // Surface for the blend.
    let face = model.faces.get(fillet_face).expect("fillet face");
    let surface = model.surfaces.get(face.surface_id).expect("fillet surface");
    let bounds = surface.parameter_bounds();
    let u_min = bounds.0 .0;
    let u_max = bounds.0 .1;
    let u_span = (u_max - u_min).abs().max(1e-12);

    // Surface-sampled NURBS caps lie on the surface by construction,
    // so the residual is dominated by Newton convergence and NURBS
    // round-trip error. 1e-3 plane units on a 10×10×10 box is well
    // inside tessellation tolerance.
    let on_surface_tol = 1e-3;
    // Boundary parameter tolerance is normalized to the u-span so the
    // assertion stays meaningful for any surface parameter convention
    // (NurbsSurface returns the raw knot range, which may not be
    // [0, 1]).
    let on_boundary_tol_normalized = 1e-2;

    for &cap_eid in &caps {
        let cap_edge = model.edges.get(cap_eid).expect("cap edge");
        let cap_curve = model.curves.get(cap_edge.curve_id).expect("cap curve");

        // Sample the cap at three points along its parameter — t = 0,
        // 0.5, 1 — and verify each lies on the variable-radius surface.
        // Sampling more than just the midpoint catches degenerate
        // cases where one endpoint sits on the surface but the
        // mid-cap does not.
        for &t in &[0.0_f64, 0.5, 1.0] {
            let p = cap_curve.point_at(t).expect("cap point evaluable at t");

            // (a) On-surface check.
            let (u, v) = surface
                .closest_point(&p, Tolerance::from_distance(1e-9))
                .expect("closest point on variable-radius surface converges");
            let p_surf = surface
                .point_at(u, v)
                .expect("variable-radius surface evaluable at returned (u,v)");
            let dist = p.distance(&p_surf);
            assert!(
                dist <= on_surface_tol,
                "cap edge {cap_eid} at t={t} lies {dist:.3e} off the variable-radius surface \
                 (tol={on_surface_tol:.3e}); cap cross-section disagrees with the swept \
                 rolling-ball surface — the Task #84 surface-sampled cap NURBS may have \
                 regressed back to a perpendicular-plane Arc"
            );

            // (b) Boundary-parameter check: the cap must sit on the
            // surface's u_min or u_max boundary, within a tolerance
            // normalized to the u-span.
            let on_umin = ((u - u_min) / u_span).abs() <= on_boundary_tol_normalized;
            let on_umax = ((u - u_max) / u_span).abs() <= on_boundary_tol_normalized;
            assert!(
                on_umin || on_umax,
                "cap edge {cap_eid} at t={t} projects to u={u:.6} (normalized \
                 {:.3e}); expected ≈ u_min={u_min:.3} or u_max={u_max:.3}; cap is \
                 not on the surface's parametric boundary",
                (u - u_min) / u_span,
            );
        }
    }
}

#[test]
fn variable_radius_cap_edges_join_trim_endpoints() {
    // Topology invariant: each cap edge's endpoints must coincide
    // exactly with the trim-curve endpoints (one contact on face1 and
    // one on face2 at the same edge end). If the cap drifts in
    // construction the blend loop would no longer close, which downstream
    // shell/face stitching would surface as a boundary-edge error.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 10.0, 10.0, 10.0);
    let edge = first_open_edge(&model);

    // `validate_result: false` — see the sibling test above: the
    // subject is cap/trim topology wiring, and the unequal-end
    // variable band's known open-mesh weld would otherwise be
    // (correctly) refused by the D-1 closure post-flight.
    let opts = FilletOptions {
        fillet_type: FilletType::Variable(0.4, 0.8),
        radius: 0.4,
        propagation: PropagationMode::None,
        common: geometry_engine::operations::CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let faces =
        fillet_edges(&mut model, solid, vec![edge], opts).expect("variable-radius fillet succeeds");
    let fillet_face = faces[0];

    let caps = cap_edges(&model, fillet_face);
    assert_eq!(caps.len(), 2);

    // For each cap, its curve's t=0 and t=1 points must match the
    // start/end vertex positions exactly (within fit tolerance — NURBS
    // fit_to_points pins endpoints by construction). This pins both
    // the curve fit and the topology wiring.
    let endpoint_tol = 1e-9;
    for &cap_eid in &caps {
        let cap_edge = model.edges.get(cap_eid).expect("cap edge");
        let cap_curve = model.curves.get(cap_edge.curve_id).expect("cap curve");
        let v0_pos = model
            .vertices
            .get(cap_edge.start_vertex)
            .expect("v0")
            .position;
        let v1_pos = model
            .vertices
            .get(cap_edge.end_vertex)
            .expect("v1")
            .position;
        let p_at_0 = cap_curve.point_at(0.0).expect("point at 0");
        let p_at_1 = cap_curve.point_at(1.0).expect("point at 1");
        let dist_to = |p: geometry_engine::math::Point3, q: [f64; 3]| -> f64 {
            ((p.x - q[0]).powi(2) + (p.y - q[1]).powi(2) + (p.z - q[2]).powi(2)).sqrt()
        };
        let d0 = dist_to(p_at_0, v0_pos);
        let d1 = dist_to(p_at_1, v1_pos);
        assert!(
            d0 <= endpoint_tol,
            "cap edge {cap_eid}: curve(0)={p_at_0:?} but start vertex at {v0_pos:?} (Δ={d0:.3e})"
        );
        assert!(
            d1 <= endpoint_tol,
            "cap edge {cap_eid}: curve(1)={p_at_1:?} but end vertex at {v1_pos:?} (Δ={d1:.3e})"
        );
    }
}
