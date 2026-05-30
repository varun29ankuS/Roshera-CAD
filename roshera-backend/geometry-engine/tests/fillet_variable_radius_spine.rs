//! End-to-end variable-radius coverage for the F3 spine solver
//! (Task #6 / F3-ε.1).
//!
//! Before F3-ε.1 the chain-level entry point
//! [`solve_spine_for_chain`](geometry_engine::operations::spine_solver::solve_spine_for_chain)
//! rejected every [`BlendRadius`] variant except `Constant(_)` and
//! equal-endpoint `Linear`. The radius scalar that survived the
//! reject was the only value threaded through the analytic arms and
//! the marching corrector. This file pins the post-F3-ε.1 contract:
//!
//! * `BlendRadius::Linear { start, end }` with `start ≠ end` is
//!   accepted on plane/plane and plane/cylinder edges; the
//!   per-station rolling-ball contact sits exactly the per-station
//!   sampled radius away from the spine centre.
//! * `BlendRadius::Variable(samples)` is accepted on the same surface
//!   pairs and produces matching per-station contacts.
//! * `BlendRadius::Linear` whose endpoints collapse to a single
//!   value (within `f64::EPSILON`) routes through the constant
//!   fast path and emits exact `Line` / `Arc` primitives, not
//!   fitted NURBS — guarding against accidental regression of the
//!   F3-α / F3-β analytic outputs.
//!
//! All tests exercise the public chain-level entry point exactly
//! the way `fillet_edges` does in production: build a `BlendGraph`
//! with the schedule, call `solve_spine_for_chain` on the chain id
//! the graph returns, then inspect the rail.
//!
//! Tolerances:
//! * Contact-to-centre distance is asserted within `1e-7` plane
//!   units — analytic arms compute the contact algebraically and
//!   round-trip through `Arc`/`Line` curve evaluation; the residual
//!   is f64 round-off, not Newton convergence.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::blend_graph::{self, BlendRadius};
use geometry_engine::operations::edge_classification::find_adjacent_faces;
use geometry_engine::operations::spine_solver::{solve_spine_for_chain, SolverKind, SpineOptions};
use geometry_engine::primitives::curve::{Arc, Line};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ----------------------------------------------------------------
// Fixture helpers
// ----------------------------------------------------------------

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(w, h, d)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(_) => {}
        other => panic!("expected solid, got {:?}", other),
    }
}

fn make_cylinder(model: &mut BRepModel, radius: f64, height: f64) {
    let mut builder = TopologyBuilder::new(model);
    let _ = builder
        .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
        .expect("cylinder creation succeeds");
}

/// Pick the first manifold edge whose two adjacent faces are both
/// planes (the canonical plane/plane analytic-arm case).
fn first_plane_plane_edge(model: &BRepModel) -> (EdgeId, FaceId, FaceId) {
    for (edge_id, _e) in model.edges.iter() {
        let faces = find_adjacent_faces(model, edge_id);
        if faces.len() != 2 {
            continue;
        }
        let (f0, f1) = (faces[0], faces[1]);
        let t0 = model
            .surfaces
            .get(
                model
                    .faces
                    .get(f0)
                    .expect("face 0 in adjacency vector")
                    .surface_id,
            )
            .expect("surface 0")
            .surface_type();
        let t1 = model
            .surfaces
            .get(
                model
                    .faces
                    .get(f1)
                    .expect("face 1 in adjacency vector")
                    .surface_id,
            )
            .expect("surface 1")
            .surface_type();
        if t0 == SurfaceType::Plane && t1 == SurfaceType::Plane {
            return (edge_id, f0, f1);
        }
    }
    panic!("no plane/plane edge found");
}

/// Pick the first manifold edge whose two adjacent faces are a
/// plane and a cylinder (the canonical plane/cylinder analytic-arm
/// case — cap rims of a cylinder primitive).
fn first_plane_cyl_edge(model: &BRepModel) -> (EdgeId, FaceId, FaceId) {
    for (edge_id, _e) in model.edges.iter() {
        let faces = find_adjacent_faces(model, edge_id);
        if faces.len() != 2 {
            continue;
        }
        let (f0, f1) = (faces[0], faces[1]);
        let t0 = model
            .surfaces
            .get(
                model
                    .faces
                    .get(f0)
                    .expect("face 0 in adjacency vector")
                    .surface_id,
            )
            .expect("surface 0")
            .surface_type();
        let t1 = model
            .surfaces
            .get(
                model
                    .faces
                    .get(f1)
                    .expect("face 1 in adjacency vector")
                    .surface_id,
            )
            .expect("surface 1")
            .surface_type();
        let is_pc = (t0 == SurfaceType::Plane && t1 == SurfaceType::Cylinder)
            || (t0 == SurfaceType::Cylinder && t1 == SurfaceType::Plane);
        if is_pc {
            return (edge_id, f0, f1);
        }
    }
    panic!("no plane/cylinder edge found");
}

/// Sample a `BlendRadius` schedule the same way the solver does
/// (kept here as a test mirror so a behavioural regression in the
/// solver's `sample_radius` doesn't silently mask itself by both
/// sides drifting together).
fn schedule_sample(schedule: &BlendRadius, t: f64) -> f64 {
    match schedule {
        BlendRadius::Constant(r) => *r,
        BlendRadius::Linear { start, end } => {
            let t_c = t.clamp(0.0, 1.0);
            start + (end - start) * t_c
        }
        BlendRadius::Variable(samples) => {
            assert!(!samples.is_empty(), "variable schedule must be non-empty");
            let n = samples.len();
            if t <= samples[0].0 {
                return samples[0].1;
            }
            if t >= samples[n - 1].0 {
                return samples[n - 1].1;
            }
            for w in samples.windows(2) {
                let (t0, r0) = w[0];
                let (t1, r1) = w[1];
                if t >= t0 && t <= t1 {
                    let span = t1 - t0;
                    if span.abs() < f64::EPSILON {
                        return r0;
                    }
                    let alpha = (t - t0) / span;
                    return r0 + (r1 - r0) * alpha;
                }
            }
            samples[n - 1].1
        }
    }
}

/// Build the blend graph for a single-edge chain with `schedule`
/// and return the (graph, chain) handles the solver expects.
fn solve_single_edge_schedule(
    model: &mut BRepModel,
    edge_id: EdgeId,
    schedule: BlendRadius,
    options: &SpineOptions,
) -> geometry_engine::operations::spine_solver::SpineRail {
    let graph = blend_graph::build(model, &[(edge_id, schedule)])
        .expect("blend graph build for single edge");
    assert_eq!(
        graph.chains.len(),
        1,
        "single edge selection must produce exactly one chain"
    );
    let chain = graph.chains[0].clone();
    solve_spine_for_chain(model, &chain, &graph, options)
        .expect("spine solver did not error")
        .expect("single-edge chain with valid schedule must produce a rail")
}

// ----------------------------------------------------------------
// Plane / plane (box edge)
// ----------------------------------------------------------------

#[test]
fn linear_radius_on_box_edge_contacts_match_schedule_per_station() {
    // F3-ε.1 contract: for a non-constant `Linear` schedule on a
    // plane/plane edge each sample's |contact_a/b - center| equals
    // the schedule sampled at that station's edge parameter.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _f0, _f1) = first_plane_plane_edge(&model);

    let schedule = BlendRadius::Linear {
        start: 0.2,
        end: 0.4,
    };
    let opts = SpineOptions::default();
    let rail = solve_single_edge_schedule(&mut model, edge_id, schedule.clone(), &opts);

    assert_eq!(
        rail.solver_kind,
        SolverKind::AnalyticPlanePlane,
        "plane/plane must dispatch to the analytic plane/plane arm"
    );
    assert!(
        rail.samples.len() >= 4,
        "non-constant schedule must request at least 4 samples for NURBS fit; got {}",
        rail.samples.len()
    );

    for s in &rail.samples {
        let r_t = schedule_sample(&schedule, s.edge_parameter);
        // Per-station radius is recorded directly on the sample.
        assert!(
            (s.radius - r_t).abs() < 1e-12,
            "sample radius {} != schedule({})={} at t={}",
            s.radius,
            s.edge_parameter,
            r_t,
            s.edge_parameter
        );

        let da = (s.contact_a - s.center).magnitude();
        let db = (s.contact_b - s.center).magnitude();
        assert!(
            (da - r_t).abs() < 1e-7,
            "contact_a {} != r_t {} at t={}",
            da,
            r_t,
            s.edge_parameter
        );
        assert!(
            (db - r_t).abs() < 1e-7,
            "contact_b {} != r_t {} at t={}",
            db,
            r_t,
            s.edge_parameter
        );
    }
}

#[test]
fn variable_radius_on_box_edge_contacts_match_schedule_per_station() {
    // F3-ε.1 contract: `Variable` schedule on plane/plane edge —
    // per-station |contact - center| equals piecewise-linear
    // schedule sample.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _f0, _f1) = first_plane_plane_edge(&model);

    let schedule = BlendRadius::Variable(vec![(0.0, 0.2), (0.5, 0.35), (1.0, 0.15)]);
    let opts = SpineOptions::default();
    let rail = solve_single_edge_schedule(&mut model, edge_id, schedule.clone(), &opts);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlanePlane);
    assert!(rail.samples.len() >= 4);

    for s in &rail.samples {
        let r_t = schedule_sample(&schedule, s.edge_parameter);
        assert!(
            (s.radius - r_t).abs() < 1e-12,
            "sample.radius {} != schedule({})={} ",
            s.radius,
            s.edge_parameter,
            r_t
        );
        let da = (s.contact_a - s.center).magnitude();
        let db = (s.contact_b - s.center).magnitude();
        assert!(
            (da - r_t).abs() < 1e-7,
            "contact_a {} != r_t {} at t={}",
            da,
            r_t,
            s.edge_parameter
        );
        assert!(
            (db - r_t).abs() < 1e-7,
            "contact_b {} != r_t {} at t={}",
            db,
            r_t,
            s.edge_parameter
        );
    }
}

#[test]
fn linear_equal_endpoints_emits_constant_fast_path_lines() {
    // `BlendRadius::Linear { 0.25, 0.25 }` reduces to a constant
    // within f64::EPSILON — the analytic plane/plane arm must emit
    // an exact `Line` for the spine and both rails, *not* fall
    // through to the cubic NURBS sample-fit. Pinning this catches
    // an accidental regression where every Linear schedule
    // (including ones that are mathematically constant) hits the
    // sampling path.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _f0, _f1) = first_plane_plane_edge(&model);

    let schedule = BlendRadius::Linear {
        start: 0.25,
        end: 0.25,
    };
    let opts = SpineOptions::default();
    let rail = solve_single_edge_schedule(&mut model, edge_id, schedule, &opts);

    assert!(
        rail.spine.as_any().downcast_ref::<Line>().is_some(),
        "constant-equivalent Linear spine should be Line, got {}",
        rail.spine.type_name()
    );
    assert!(
        rail.rail_a.as_any().downcast_ref::<Line>().is_some(),
        "constant-equivalent Linear rail_a should be Line, got {}",
        rail.rail_a.type_name()
    );
    assert!(
        rail.rail_b.as_any().downcast_ref::<Line>().is_some(),
        "constant-equivalent Linear rail_b should be Line, got {}",
        rail.rail_b.type_name()
    );
}

// ----------------------------------------------------------------
// Plane / cylinder (cylinder rim)
// ----------------------------------------------------------------

#[test]
fn linear_radius_on_cylinder_rim_contacts_match_schedule_per_station() {
    // F3-ε.1 contract on plane/cylinder. The analytic arm
    // (perpendicular cap rim) parameterises the spine radius about
    // the cylinder axis as `r_cyl ± r_t`; the contact stays
    // exactly `r_t` from the spine centre at every station.
    let mut model = BRepModel::new();
    make_cylinder(&mut model, 2.0, 5.0);
    let (edge_id, _f0, _f1) = first_plane_cyl_edge(&model);

    let schedule = BlendRadius::Linear {
        start: 0.15,
        end: 0.35,
    };
    let opts = SpineOptions::default();
    let rail = solve_single_edge_schedule(&mut model, edge_id, schedule.clone(), &opts);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlaneCylinder);
    assert!(rail.samples.len() >= 4);

    for s in &rail.samples {
        let r_t = schedule_sample(&schedule, s.edge_parameter);
        assert!(
            (s.radius - r_t).abs() < 1e-12,
            "sample.radius {} != schedule({})={}",
            s.radius,
            s.edge_parameter,
            r_t
        );
        let da = (s.contact_a - s.center).magnitude();
        let db = (s.contact_b - s.center).magnitude();
        assert!(
            (da - r_t).abs() < 1e-7,
            "contact_a {} != r_t {} at t={}",
            da,
            r_t,
            s.edge_parameter
        );
        assert!(
            (db - r_t).abs() < 1e-7,
            "contact_b {} != r_t {} at t={}",
            db,
            r_t,
            s.edge_parameter
        );
    }
}

#[test]
fn variable_radius_on_cylinder_rim_contacts_match_schedule_per_station() {
    // F3-ε.1 contract: `Variable` schedule on a plane/cylinder rim.
    let mut model = BRepModel::new();
    make_cylinder(&mut model, 2.0, 5.0);
    let (edge_id, _f0, _f1) = first_plane_cyl_edge(&model);

    let schedule = BlendRadius::Variable(vec![(0.0, 0.1), (0.25, 0.2), (0.75, 0.3), (1.0, 0.2)]);
    let opts = SpineOptions::default();
    let rail = solve_single_edge_schedule(&mut model, edge_id, schedule.clone(), &opts);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlaneCylinder);
    assert!(rail.samples.len() >= 4);

    for s in &rail.samples {
        let r_t = schedule_sample(&schedule, s.edge_parameter);
        assert!(
            (s.radius - r_t).abs() < 1e-12,
            "sample.radius {} != schedule({})={}",
            s.radius,
            s.edge_parameter,
            r_t
        );
        let da = (s.contact_a - s.center).magnitude();
        let db = (s.contact_b - s.center).magnitude();
        assert!(
            (da - r_t).abs() < 1e-7,
            "contact_a {} != r_t {} at t={}",
            da,
            r_t,
            s.edge_parameter
        );
        assert!(
            (db - r_t).abs() < 1e-7,
            "contact_b {} != r_t {} at t={}",
            db,
            r_t,
            s.edge_parameter
        );
    }
}

#[test]
fn constant_radius_on_cylinder_rim_emits_arcs() {
    // Pin the constant fast path on plane/cylinder: spine and both
    // rails are exact `Arc` primitives (matching the pre-F3-ε.1
    // analytic arm output). Variable / non-constant Linear
    // schedules drop to NURBS fits — guarded by the fast-path
    // branch in `solve_plane_cyl_perpendicular`.
    let mut model = BRepModel::new();
    make_cylinder(&mut model, 2.0, 5.0);
    let (edge_id, _f0, _f1) = first_plane_cyl_edge(&model);

    let schedule = BlendRadius::Constant(0.3);
    let opts = SpineOptions::default();
    let rail = solve_single_edge_schedule(&mut model, edge_id, schedule, &opts);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlaneCylinder);
    assert!(
        rail.spine.as_any().downcast_ref::<Arc>().is_some(),
        "constant-radius cylinder-rim spine should be Arc, got {}",
        rail.spine.type_name()
    );
    assert!(
        rail.rail_a.as_any().downcast_ref::<Arc>().is_some(),
        "constant-radius cylinder-rim rail_a should be Arc, got {}",
        rail.rail_a.type_name()
    );
    assert!(
        rail.rail_b.as_any().downcast_ref::<Arc>().is_some(),
        "constant-radius cylinder-rim rail_b should be Arc, got {}",
        rail.rail_b.type_name()
    );
}

// ----------------------------------------------------------------
// Validation: a non-positive radius anywhere in the schedule is a
// hard reject. The dispatcher returns `Err(InvalidRadius)` from
// the up-front `validate_schedule_positive` call, never produces
// a partial rail.
// ----------------------------------------------------------------

#[test]
fn variable_schedule_with_zero_radius_rejected() {
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _f0, _f1) = first_plane_plane_edge(&model);

    let schedule = BlendRadius::Variable(vec![(0.0, 0.2), (0.5, 0.0), (1.0, 0.3)]);
    let opts = SpineOptions::default();
    let graph = blend_graph::build(&mut model, &[(edge_id, schedule)])
        .expect("blend graph build does not enforce positive radius");
    let chain = graph.chains[0].clone();
    let result = solve_spine_for_chain(&model, &chain, &graph, &opts);
    assert!(
        result.is_err(),
        "schedule containing a zero radius must be rejected by the spine solver, \
         got Ok({:?})",
        result.ok().flatten().map(|r| r.solver_kind)
    );
}
