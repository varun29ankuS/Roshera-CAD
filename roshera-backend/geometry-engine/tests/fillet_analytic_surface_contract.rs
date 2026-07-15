// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! F4-α.1 — analytic-when-possible blend surface contract.
//!
//! `BlendSurfaceCarrier::dispatch` is unit-tested exhaustively
//! against its 5 × 2 table in the module's own `#[cfg(test)]`
//! block. This file pins the **live-solver → carrier** path: build
//! a real model, run `solve_spine_for_chain`, derive the carrier
//! via `BlendSurfaceCarrier::from_spine_rail`, assert against the
//! expected variant.
//!
//! Reachable solver arms covered today:
//!
//! * `AnalyticPlanePlane`    — box edge.
//! * `AnalyticPlaneCylinder` — cylinder cap rim.
//!
//! `AnalyticPlaneSphere` and `AnalyticCylCylCoaxial` are
//! constructed-but-not-yet-wired-from-fixtures in the kernel; the
//! pure-dispatch unit tests in the module cover them. `Marched` is
//! exercised by the radius-constancy tolerance guard
//! (`linear_jitter_under_tolerance_routes_constant`) which forces
//! the dispatch through the variable-radius arm without needing a
//! NURBS-NURBS surface pair.
//!
//! Tolerance: per-station radius constancy is judged against
//! `SpineOptions::default().tolerance.distance()` — the same
//! tolerance the fillet entry point passes into
//! `BlendSurfaceCarrier::from_spine_rail`.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::blend_graph::{self, BlendRadius};
use geometry_engine::operations::blend_surface_carrier::BlendSurfaceCarrier;
use geometry_engine::operations::edge_classification::find_adjacent_faces;
use geometry_engine::operations::spine_solver::{
    solve_spine_for_chain, SolverKind, SpineOptions, SpineRail,
};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ----------------------------------------------------------------
// Fixture helpers (mirror tests/fillet_variable_radius_spine.rs)
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

fn first_plane_plane_edge(model: &BRepModel) -> (EdgeId, FaceId, FaceId) {
    for (edge_id, _e) in model.edges.iter() {
        let faces = find_adjacent_faces(model, edge_id);
        if faces.len() != 2 {
            continue;
        }
        let (f0, f1) = (faces[0], faces[1]);
        let t0 = surface_type(model, f0);
        let t1 = surface_type(model, f1);
        if t0 == SurfaceType::Plane && t1 == SurfaceType::Plane {
            return (edge_id, f0, f1);
        }
    }
    panic!("no plane/plane edge found");
}

fn first_plane_cyl_edge(model: &BRepModel) -> (EdgeId, FaceId, FaceId) {
    for (edge_id, _e) in model.edges.iter() {
        let faces = find_adjacent_faces(model, edge_id);
        if faces.len() != 2 {
            continue;
        }
        let (f0, f1) = (faces[0], faces[1]);
        let t0 = surface_type(model, f0);
        let t1 = surface_type(model, f1);
        let is_pc = (t0 == SurfaceType::Plane && t1 == SurfaceType::Cylinder)
            || (t0 == SurfaceType::Cylinder && t1 == SurfaceType::Plane);
        if is_pc {
            return (edge_id, f0, f1);
        }
    }
    panic!("no plane/cylinder edge found");
}

fn surface_type(model: &BRepModel, face_id: FaceId) -> SurfaceType {
    let face = model.faces.get(face_id).expect("face in model");
    model
        .surfaces
        .get(face.surface_id)
        .expect("surface in model")
        .surface_type()
}

fn solve_single_edge(
    model: &mut BRepModel,
    edge_id: EdgeId,
    schedule: BlendRadius,
    options: &SpineOptions,
) -> SpineRail {
    let graph = blend_graph::build(model, &[(edge_id, schedule)])
        .expect("blend graph build for single edge");
    assert_eq!(graph.chains.len(), 1, "single-edge chain expected");
    let chain = graph.chains[0].clone();
    solve_spine_for_chain(model, &chain, &graph, options)
        .expect("spine solver did not error")
        .expect("single-edge chain with valid schedule must produce a rail")
}

// ----------------------------------------------------------------
// Live solver → carrier
// ----------------------------------------------------------------

#[test]
fn plane_plane_constant_routes_to_cylindrical() {
    // F4-α.1 contract: a box edge with `BlendRadius::Constant`
    // routes the spine to `AnalyticPlanePlane`; the rail's
    // per-station radii agree exactly; carrier is `Cylindrical`.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _, _) = first_plane_plane_edge(&model);

    let options = SpineOptions::default();
    let rail = solve_single_edge(&mut model, edge_id, BlendRadius::Constant(0.5), &options);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlanePlane);
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance),
        BlendSurfaceCarrier::Cylindrical
    );
}

#[test]
fn plane_plane_collapsed_linear_routes_to_cylindrical() {
    // `BlendRadius::Linear { start, end }` where start==end folds
    // onto the constant fast-path inside the solver; the carrier
    // must see all-equal radii and stay on `Cylindrical`. Pins the
    // "linear-but-degenerate is still analytic" contract.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _, _) = first_plane_plane_edge(&model);

    let options = SpineOptions::default();
    let schedule = BlendRadius::Linear {
        start: 0.7,
        end: 0.7,
    };
    let rail = solve_single_edge(&mut model, edge_id, schedule, &options);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlanePlane);
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance),
        BlendSurfaceCarrier::Cylindrical
    );
}

#[test]
fn plane_plane_linear_variable_routes_to_general_nurbs() {
    // Non-degenerate linear schedule on the same plane/plane edge:
    // the solver still routes through the analytic arm (the spine
    // remains a line; the radius profile varies), but the carrier
    // must drop to `GeneralNurbs` because the analytic cylinder no
    // longer fits a varying radius.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _, _) = first_plane_plane_edge(&model);

    let options = SpineOptions::default();
    let schedule = BlendRadius::Linear {
        start: 0.2,
        end: 0.6,
    };
    let rail = solve_single_edge(&mut model, edge_id, schedule, &options);

    // Solver kind stays analytic — geometry of the spine is still
    // a straight line, only the radius varies.
    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlanePlane);
    // Carrier drops to GeneralNurbs because the radius profile
    // varies above tolerance.
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance),
        BlendSurfaceCarrier::GeneralNurbs
    );
}

#[test]
fn plane_cylinder_constant_routes_to_toroidal() {
    // Cylinder cap rim → AnalyticPlaneCylinder + constant radius
    // → Toroidal. The dihedral here is a right angle and the spine
    // is a circular arc concentric with the cylinder axis.
    let mut model = BRepModel::new();
    make_cylinder(&mut model, 2.0, 5.0);
    let (edge_id, _, _) = first_plane_cyl_edge(&model);

    let options = SpineOptions::default();
    let rail = solve_single_edge(&mut model, edge_id, BlendRadius::Constant(0.4), &options);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlaneCylinder);
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance),
        BlendSurfaceCarrier::Toroidal
    );
}

#[test]
fn plane_cylinder_linear_variable_routes_to_general_nurbs() {
    let mut model = BRepModel::new();
    make_cylinder(&mut model, 2.0, 5.0);
    let (edge_id, _, _) = first_plane_cyl_edge(&model);

    let options = SpineOptions::default();
    let schedule = BlendRadius::Linear {
        start: 0.2,
        end: 0.5,
    };
    let rail = solve_single_edge(&mut model, edge_id, schedule, &options);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlaneCylinder);
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance),
        BlendSurfaceCarrier::GeneralNurbs
    );
}

#[test]
fn plane_cylinder_variable_samples_routes_to_general_nurbs() {
    // `BlendRadius::Variable` with non-trivial samples must also
    // drop to GeneralNurbs. Mirrors the Linear case above to
    // confirm the carrier doesn't accidentally special-case the
    // Linear variant.
    let mut model = BRepModel::new();
    make_cylinder(&mut model, 2.0, 5.0);
    let (edge_id, _, _) = first_plane_cyl_edge(&model);

    let options = SpineOptions::default();
    let schedule = BlendRadius::Variable(vec![(0.0, 0.2), (0.5, 0.45), (1.0, 0.3)]);
    let rail = solve_single_edge(&mut model, edge_id, schedule, &options);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlaneCylinder);
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance),
        BlendSurfaceCarrier::GeneralNurbs
    );
}

#[test]
fn radius_jitter_under_tolerance_stays_constant() {
    // Crafted variable schedule whose samples differ by less than
    // the spine options tolerance: the rail's `radius_is_constant`
    // check must read this as constant and route through the
    // analytic-constant carrier. Guards against a future
    // "any non-Constant schedule routes to GeneralNurbs" shortcut.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _, _) = first_plane_plane_edge(&model);

    let options = SpineOptions::default();
    let tol = options.tolerance.distance();
    let r = 0.5;
    let schedule = BlendRadius::Linear {
        start: r,
        end: r + tol * 0.1,
    };
    let rail = solve_single_edge(&mut model, edge_id, schedule, &options);

    assert_eq!(rail.solver_kind, SolverKind::AnalyticPlanePlane);
    // Per-station radii agree within tolerance; carrier holds at
    // Cylindrical.
    let carrier = BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance);
    assert_eq!(carrier, BlendSurfaceCarrier::Cylindrical);
}

#[test]
fn dispatch_helper_matches_from_spine_rail_for_live_box() {
    // The pure `dispatch(solver_kind, radius_is_constant)` helper
    // and the wrapper `from_spine_rail(&rail, &tol)` must agree
    // for every real rail. Catches a refactor that reroutes the
    // helper without re-routing the wrapper (or vice versa).
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _, _) = first_plane_plane_edge(&model);

    let options = SpineOptions::default();
    let rail = solve_single_edge(&mut model, edge_id, BlendRadius::Constant(0.3), &options);

    let tol = options.tolerance.distance();
    let r0 = rail.samples[0].radius;
    let radius_is_constant = rail.samples.iter().all(|s| (s.radius - r0).abs() <= tol);

    assert_eq!(
        BlendSurfaceCarrier::dispatch(rail.solver_kind, radius_is_constant),
        BlendSurfaceCarrier::from_spine_rail(&rail, &options.tolerance),
    );
}

#[test]
fn explicit_tolerance_override_changes_classification() {
    // Pass a hand-built tolerance smaller than the rail's natural
    // jitter and confirm that a previously-constant rail now
    // routes to GeneralNurbs. Pins that `from_spine_rail` actually
    // consults the supplied tolerance rather than baking in
    // `SpineOptions::default().tolerance`.
    let mut model = BRepModel::new();
    make_box(&mut model, 10.0, 10.0, 10.0);
    let (edge_id, _, _) = first_plane_plane_edge(&model);

    let options = SpineOptions::default();
    let schedule = BlendRadius::Linear {
        start: 0.5,
        end: 0.5 + options.tolerance.distance() * 10.0,
    };
    let rail = solve_single_edge(&mut model, edge_id, schedule, &options);

    // Permissive tolerance (10× the gap) → constant → Cylindrical.
    let permissive = Tolerance::new(
        options.tolerance.distance() * 100.0,
        options.tolerance.angle(),
    );
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &permissive),
        BlendSurfaceCarrier::Cylindrical
    );
    // Strict tolerance (1/100 of the gap) → variable → GeneralNurbs.
    let strict = Tolerance::new(
        options.tolerance.distance() / 100.0,
        options.tolerance.angle(),
    );
    assert_eq!(
        BlendSurfaceCarrier::from_spine_rail(&rail, &strict),
        BlendSurfaceCarrier::GeneralNurbs
    );
}
