// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 — Slice 6: sketch ops (spec §3.4) + construction
//! geometry.
//!
//! Construction geometry: `is_construction` existed on every entity
//! wrapper but the topology walker consumed construction entities into
//! profiles — a construction guide line would close (or break) a loop
//! and a construction circle would punch a bore. The gate here pins
//! the fix at both levels: topology (construction entities contribute
//! NO edges) and extrude (a profile with construction guides extrudes
//! the same solid as one without, typed-edge path intact).
//!
//! Ops: trim / extend / offset / mirror / linear + circular patterns —
//! each a kernel `sketch2d` operation returning a typed
//! `SketchOpOutcome` (created/deleted entities, minted constraints,
//! provenance), leaving the sketch re-certifiable, with maintenance
//! constraints so the op result is MAINTAINED by the solver, not a
//! one-shot copy.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::sketch2d::sketch_topology::{AnalyticLoop, ProfileExtractor, SketchTopology};
use geometry_engine::sketch2d::{EntityRef, Point2d, Sketch, SketchAnchor, Tolerance2d};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

const RECT_W: f64 = 40.0;
const RECT_H: f64 = 30.0;
const EXTRUDE_H: f64 = 10.0;

fn fresh(name: &str) -> Sketch {
    Sketch::new(name.to_string(), SketchAnchor::xy())
}

/// Closed 40×30 outline from four shared-endpoint lines.
fn rectangle_outline(sketch: &Sketch) -> Vec<EntityRef> {
    let p = [
        sketch.add_point(Point2d::new(0.0, 0.0)),
        sketch.add_point(Point2d::new(RECT_W, 0.0)),
        sketch.add_point(Point2d::new(RECT_W, RECT_H)),
        sketch.add_point(Point2d::new(0.0, RECT_H)),
    ];
    (0..4)
        .map(|i| EntityRef::Line(sketch.add_line(p[i], p[(i + 1) % 4]).expect("outline line")))
        .collect()
}

fn analytic_regions(sketch: &Sketch) -> Vec<ProfileRegion> {
    let topo = SketchTopology::analyze(sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    profiles
        .iter()
        .map(|profile| {
            let outer =
                match ProfileExtractor::analytic_loop_edges(sketch, &topo, &profile.outer_boundary)
                    .expect("outer loop extraction")
                {
                    AnalyticLoop::Edges(edges) => edges,
                    AnalyticLoop::Unsupported { entity, edge_type } => {
                        panic!("analytic refusal: {entity} ({edge_type:?})")
                    }
                };
            let holes = profile
                .holes
                .iter()
                .map(|hole| {
                    match ProfileExtractor::analytic_loop_edges(sketch, &topo, hole)
                        .expect("hole loop extraction")
                    {
                        AnalyticLoop::Edges(edges) => ProfileLoop::Edges(edges),
                        AnalyticLoop::Unsupported { entity, edge_type } => {
                            panic!("analytic refusal: {entity} ({edge_type:?})")
                        }
                    }
                })
                .collect();
            ProfileRegion {
                outer: ProfileLoop::Edges(outer),
                holes,
            }
        })
        .collect()
}

fn extrude_measured_volume(sketch: &Sketch) -> f64 {
    let regions = analytic_regions(sketch);
    let mut model = geometry_engine::primitives::topology_builder::BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &regions,
        EXTRUDE_H,
        None,
        Tolerance::default(),
    )
    .expect("extrude");
    let solid_ref = model.solids.get(solid).expect("solid present");
    let mesh = tessellate_solid(solid_ref, &model, &TessellationParams::default());
    assert!(!mesh.triangles.is_empty(), "solid must tessellate");
    // Mesh volume oracle — same as the slice-5 gate.
    model
        .calculate_solid_volume(solid)
        .expect("solid volume must be computable")
}

// ── Construction geometry: walker + extrude skip (spec §3.4) ────────

#[test]
fn red_construction_entities_contribute_no_topology_edges() {
    let s = fresh("slice6_construction_topology");
    rectangle_outline(&s);
    // Construction guides: a diagonal line and a centered circle. The
    // circle would otherwise nest as a HOLE; the diagonal would break
    // the loop walk with T-junction-like connectivity.
    let d1 = s.add_point(Point2d::new(0.0, 0.0));
    let d2 = s.add_point(Point2d::new(RECT_W, RECT_H));
    let diag = s.add_line(d1, d2).expect("diagonal");
    let circle = s
        .add_circle(Point2d::new(RECT_W / 2.0, RECT_H / 2.0), 6.0)
        .expect("guide circle");
    s.set_construction(&EntityRef::Line(diag), true)
        .expect("mark diagonal construction");
    s.set_construction(&EntityRef::Circle(circle), true)
        .expect("mark circle construction");

    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    assert_eq!(
        topo.loops().len(),
        1,
        "construction circle must not form a loop"
    );
    assert_eq!(topo.regions().len(), 1, "one region, no construction hole");
    assert!(
        topo.regions()[0].inner_loops.is_empty(),
        "construction circle must NOT nest as a hole"
    );
    assert!(topo.is_valid_for_extrusion());
}

#[test]
fn red_construction_gate_extrude_ignores_guides_typed_path_intact() {
    // The spec gate: a sketch WITH construction guides extrudes the
    // exact solid the guide-free sketch produces — a plain box, no
    // bore, no wall from the diagonal — through the analytic
    // typed-edge path (slice 5) with no sampled fallback.
    let with_guides = fresh("slice6_construction_extrude");
    rectangle_outline(&with_guides);
    let d1 = with_guides.add_point(Point2d::new(0.0, 0.0));
    let d2 = with_guides.add_point(Point2d::new(RECT_W, RECT_H));
    let diag = with_guides.add_line(d1, d2).expect("diagonal");
    let circle = with_guides
        .add_circle(Point2d::new(RECT_W / 2.0, RECT_H / 2.0), 6.0)
        .expect("guide circle");
    with_guides
        .set_construction(&EntityRef::Line(diag), true)
        .expect("construction diag");
    with_guides
        .set_construction(&EntityRef::Circle(circle), true)
        .expect("construction circle");

    let vol = extrude_measured_volume(&with_guides);
    let expected = RECT_W * RECT_H * EXTRUDE_H;
    assert!(
        (vol - expected).abs() / expected < 1e-9,
        "construction guides must not affect the extruded solid: \
         vol {vol} vs box {expected}"
    );
}

#[test]
fn set_construction_round_trips_and_rejects_missing_entities() {
    let s = fresh("slice6_construction_setter");
    let p = s.add_point(Point2d::new(1.0, 2.0));
    let q = s.add_point(Point2d::new(4.0, 2.0));
    let line = s.add_line(p, q).expect("line");
    let lref = EntityRef::Line(line);
    assert!(!s.lines().get(&line).expect("line").is_construction);
    s.set_construction(&lref, true).expect("set");
    assert!(s.lines().get(&line).expect("line").is_construction);
    s.set_construction(&lref, false).expect("clear");
    assert!(!s.lines().get(&line).expect("line").is_construction);

    let phantom = EntityRef::Line(geometry_engine::sketch2d::Line2dId::new());
    assert!(
        s.set_construction(&phantom, true).is_err(),
        "missing entity must be a typed error"
    );
}

#[test]
fn construction_entities_still_participate_in_the_solver() {
    // Construction geometry is solver-real (a mirror axis must stay
    // solvable) — only PROFILE extraction ignores it.
    let s = fresh("slice6_construction_solves");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(10.0, 3.0));
    let axis = s.add_line(a, b).expect("axis");
    s.set_construction(&EntityRef::Line(axis), true)
        .expect("construction");
    s.add_constraint(geometry_engine::sketch2d::Constraint::new_geometric(
        geometry_engine::sketch2d::GeometricConstraint::Horizontal,
        vec![EntityRef::Line(axis)],
        geometry_engine::sketch2d::ConstraintPriority::Required,
    ));
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let (pa, pb) = (s.get_point(&a).expect("a"), s.get_point(&b).expect("b"));
    assert!(
        (pa.y - pb.y).abs() < 1e-6,
        "construction line must be driven horizontal by the solver: {pa:?} {pb:?}"
    );
}

// ── Sketch ops (spec §3.4) ──────────────────────────────────────────

use geometry_engine::primitives::surface::Cylinder;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_ops::{self, LineEnd, SketchOpError, SketchOpKind};
use geometry_engine::sketch2d::{
    Constraint, ConstraintPriority, ConstraintType, DimensionalConstraint, DofStatus,
    GeometricConstraint,
};

fn cylinder_faces(model: &BRepModel, solid: u32) -> usize {
    let solid_ref = model.solids.get(solid).expect("solid");
    let shell = model.shells.get(solid_ref.outer_shell).expect("shell");
    shell
        .faces
        .iter()
        .filter(|&&fid| {
            let face = model.faces.get(fid).expect("face");
            let surface = model.surfaces.get(face.surface_id).expect("surface");
            surface.as_any().downcast_ref::<Cylinder>().is_some()
        })
        .count()
}

fn required_dim(dc: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(dc, entities, ConstraintPriority::Required)
}

fn fix_point(sketch: &Sketch, id: &geometry_engine::sketch2d::Point2dId) {
    sketch
        .points()
        .get_mut(id)
        .expect("point present")
        .value_mut()
        .fix();
}

fn count_constraints_of(sketch: &Sketch, pred: impl Fn(&ConstraintType) -> bool) -> usize {
    sketch
        .all_constraints()
        .iter()
        .filter(|c| pred(&c.constraint_type))
        .count()
}

// ── trim ────────────────────────────────────────────────────────────

#[test]
fn red_trim_line_middle_span_by_circle_cutter() {
    // Line (0,0)->(20,0) crossed by a circle (center (10,3), r 5) at
    // x = 6 and x = 14. Picking the middle removes [6, 14] and leaves
    // TWO lines, re-using the original endpoints, with the two new cut
    // points held ON the cutter by minted PointOnCurve constraints.
    let s = fresh("slice6_trim_line_middle");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(20.0, 0.0));
    let line = s.add_line(a, b).expect("target line");
    let cutter = s.add_circle(Point2d::new(10.0, 3.0), 5.0).expect("cutter");

    let outcome = sketch_ops::trim(
        &s,
        &EntityRef::Line(line),
        &EntityRef::Circle(cutter),
        Point2d::new(10.0, 0.0),
    )
    .expect("trim");

    assert_eq!(outcome.op, SketchOpKind::Trim);
    assert!(outcome.deleted.contains(&EntityRef::Line(line)));
    assert!(!s.lines().contains_key(&line), "target line deleted");
    let new_lines: Vec<_> = outcome
        .created
        .iter()
        .filter(|e| matches!(e, EntityRef::Line(_)))
        .collect();
    assert_eq!(new_lines.len(), 2, "middle trim splits into two lines");
    assert_eq!(s.lines().len(), 2);
    // Original endpoints survive and are reused.
    assert!(s.points().contains_key(&a));
    assert!(s.points().contains_key(&b));
    // The two cut points sit at x = 6 and x = 14 on y = 0.
    let mut cut_xs: Vec<f64> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Point(id) => s.get_point(id).map(|p| {
                assert!(p.y.abs() < 1e-9, "cut point off the line: {p:?}");
                p.x
            }),
            _ => None,
        })
        .collect();
    cut_xs.sort_by(|x, y| x.partial_cmp(y).unwrap());
    assert_eq!(cut_xs.len(), 2);
    assert!((cut_xs[0] - 6.0).abs() < 1e-9, "got {cut_xs:?}");
    assert!((cut_xs[1] - 14.0).abs() < 1e-9, "got {cut_xs:?}");
    // Maintenance: each cut point rides the cutter.
    let poc = count_constraints_of(&s, |t| {
        matches!(
            t,
            ConstraintType::Geometric(GeometricConstraint::PointOnCurve)
        )
    });
    assert_eq!(poc, 2, "two minted PointOnCurve constraints");
    assert_eq!(outcome.constraints_added.len(), 2);
    // The sketch stays solvable and re-certifiable after the op.
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let cert = s.certify();
    assert!(cert.constraint_consistent, "certificate stays honest");
}

#[test]
fn red_trim_line_end_span_shrinks_and_prunes_the_orphan_endpoint() {
    // One crossing at x = 10; picking near the far end removes
    // [10, 20]: ONE line remains and the unused far endpoint (b) is
    // pruned (no other entity or constraint references it).
    let s = fresh("slice6_trim_line_end");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(20.0, 0.0));
    let line = s.add_line(a, b).expect("target");
    let c1 = s.add_point(Point2d::new(10.0, -5.0));
    let c2 = s.add_point(Point2d::new(10.0, 5.0));
    let cutter = s.add_line(c1, c2).expect("cutter");

    let outcome = sketch_ops::trim(
        &s,
        &EntityRef::Line(line),
        &EntityRef::Line(cutter),
        Point2d::new(18.0, 0.0),
    )
    .expect("trim");

    let new_lines: Vec<_> = outcome
        .created
        .iter()
        .filter(|e| matches!(e, EntityRef::Line(_)))
        .collect();
    assert_eq!(new_lines.len(), 1, "end trim shrinks to one line");
    assert!(s.points().contains_key(&a), "kept endpoint survives");
    assert!(
        !s.points().contains_key(&b),
        "orphaned far endpoint must be pruned"
    );
    assert!(outcome.deleted.contains(&EntityRef::Point(b)));
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

#[test]
fn red_trim_circle_to_arc_between_two_intersections() {
    // Circle r 6 at origin crossed by the vertical chord x = 3.
    // Picking the RIGHT side (near x = 6) removes the minor arc and
    // leaves the major arc, radius preserved, endpoints held on the
    // cutter.
    let s = fresh("slice6_trim_circle");
    let circle = s.add_circle(Point2d::new(0.0, 0.0), 6.0).expect("target");
    let c1 = s.add_point(Point2d::new(3.0, -10.0));
    let c2 = s.add_point(Point2d::new(3.0, 10.0));
    let cutter = s.add_line(c1, c2).expect("cutter");

    let outcome = sketch_ops::trim(
        &s,
        &EntityRef::Circle(circle),
        &EntityRef::Line(cutter),
        Point2d::new(6.0, 0.0),
    )
    .expect("trim");

    assert!(!s.circles().contains_key(&circle), "circle deleted");
    let arcs: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Arc(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(arcs.len(), 1, "circle trim leaves one arc");
    let arc = s.arcs().get(&arcs[0]).expect("arc").arc;
    assert!((arc.radius - 6.0).abs() < 1e-9, "radius preserved");
    assert!(
        (arc.center.x).abs() < 1e-9 && (arc.center.y).abs() < 1e-9,
        "center preserved: {:?}",
        arc.center
    );
    // The surviving arc must NOT contain the picked region (x = +6)
    // and must contain the far side (x = -6).
    assert!(!arc.contains_angle(0.0), "picked span must be removed");
    assert!(
        arc.contains_angle(std::f64::consts::PI),
        "complement span must survive"
    );
    // Both endpoints on the chord x = 3.
    let (sp, ep) = (arc.start_point(), arc.end_point());
    assert!((sp.x - 3.0).abs() < 1e-9, "{sp:?}");
    assert!((ep.x - 3.0).abs() < 1e-9, "{ep:?}");
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

#[test]
fn trim_refuses_when_cutter_never_crosses() {
    let s = fresh("slice6_trim_no_intersection");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(20.0, 0.0));
    let line = s.add_line(a, b).expect("target");
    let cutter = s.add_circle(Point2d::new(10.0, 30.0), 2.0).expect("cutter");
    let err = sketch_ops::trim(
        &s,
        &EntityRef::Line(line),
        &EntityRef::Circle(cutter),
        Point2d::new(10.0, 0.0),
    )
    .expect_err("must refuse");
    assert!(
        matches!(err, SketchOpError::NoIntersection { .. }),
        "typed refusal expected, got {err:?}"
    );
    assert!(s.lines().contains_key(&line), "target untouched on refusal");
}

// ── extend ──────────────────────────────────────────────────────────

#[test]
fn red_extend_line_end_to_boundary_and_maintain_contact() {
    let s = fresh("slice6_extend");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(5.0, 0.0));
    let line = s.add_line(a, b).expect("target");
    let c1 = s.add_point(Point2d::new(10.0, -5.0));
    let c2 = s.add_point(Point2d::new(10.0, 5.0));
    let boundary = s.add_line(c1, c2).expect("boundary");

    let outcome =
        sketch_ops::extend(&s, &line, LineEnd::End, &EntityRef::Line(boundary)).expect("extend");

    let moved = s.get_point(&b).expect("b");
    assert!(
        (moved.x - 10.0).abs() < 1e-9 && moved.y.abs() < 1e-9,
        "endpoint must land on the boundary: {moved:?}"
    );
    assert!(outcome.modified.contains(&EntityRef::Line(line)));
    assert!(outcome.modified.contains(&EntityRef::Point(b)));
    assert_eq!(
        outcome.constraints_added.len(),
        1,
        "one minted PointOnCurve keeps the extended end on the boundary"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

#[test]
fn extend_refuses_when_nothing_lies_ahead() {
    let s = fresh("slice6_extend_refuse");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(5.0, 0.0));
    let line = s.add_line(a, b).expect("target");
    // Boundary BEHIND the extended end (x = -10): extending the END
    // (+X direction) finds nothing ahead.
    let c1 = s.add_point(Point2d::new(-10.0, -5.0));
    let c2 = s.add_point(Point2d::new(-10.0, 5.0));
    let boundary = s.add_line(c1, c2).expect("boundary");
    let err = sketch_ops::extend(&s, &line, LineEnd::End, &EntityRef::Line(boundary))
        .expect_err("must refuse");
    assert!(matches!(err, SketchOpError::NoIntersection { .. }));
    let pos = s.get_point(&b).expect("b");
    assert_eq!((pos.x, pos.y), (5.0, 0.0), "no mutation on refusal");
}

// ── offset ──────────────────────────────────────────────────────────

#[test]
fn red_offset_rectangle_outward_full_gate() {
    // The flagship: closed 40x30 line loop, fixed source corners,
    // offset +5 outward. Expect 4 offset lines + 4 corner arcs, all
    // maintained by minted Offset/OffsetDistance/Radius constraints:
    // the certificate must call the offset geometry FULLY constrained
    // relative to the (fixed) source, the solver must converge, and
    // extruding the two nested loops must produce the exact ring
    // volume with analytic arc corners.
    let d = 5.0;
    let s = fresh("slice6_offset_rectangle");
    let outline = rectangle_outline(&s);
    // Two-pass: collect ids first — holding a DashMap iter guard
    // across get_mut on the same shard would deadlock.
    let point_ids: Vec<_> = s.points().iter().map(|e| *e.key()).collect();
    for id in point_ids {
        fix_point(&s, &id);
    }

    let outcome = sketch_ops::offset(&s, &outline[0], d).expect("offset");

    let new_lines = outcome
        .created
        .iter()
        .filter(|e| matches!(e, EntityRef::Line(_)))
        .count();
    let new_arcs = outcome
        .created
        .iter()
        .filter(|e| matches!(e, EntityRef::Arc(_)))
        .count();
    assert_eq!(new_lines, 4, "four offset lines");
    assert_eq!(new_arcs, 4, "four corner arcs");

    // Maintenance constraints: one Offset + one OffsetDistance per
    // source/offset line pair, one Radius(|d|) per corner arc.
    let offsets = count_constraints_of(&s, |t| {
        matches!(t, ConstraintType::Geometric(GeometricConstraint::Offset))
    });
    let gaps = count_constraints_of(&s, |t| {
        matches!(
            t,
            ConstraintType::Dimensional(DimensionalConstraint::OffsetDistance(_))
        )
    });
    let radii = count_constraints_of(&s, |t| {
        matches!(
            t,
            ConstraintType::Dimensional(DimensionalConstraint::Radius(_))
        )
    });
    assert_eq!(offsets, 4);
    assert_eq!(gaps, 4);
    assert_eq!(radii, 4);

    // Certificate honesty: hand-count -- 8 junction points (16 DOF) +
    // 4 corner-arc chord offsets (4) = 20 free; 4x(Offset 3 +
    // OffsetDistance 1) + 4xRadius 1 = 20 removed.
    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "offset loop must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // Geometry: ring area = 2d(W+H) + pi d^2; volume = ring x height.
    // Mesh volume tolerance covers the tessellated arc corners.
    let vol = extrude_measured_volume(&s);
    let ring = 2.0 * d * (RECT_W + RECT_H) + std::f64::consts::PI * d * d;
    let expected = ring * EXTRUDE_H;
    assert!(
        (vol - expected).abs() / expected < 1e-3,
        "ring volume: got {vol}, want {expected}"
    );
}

#[test]
fn red_offset_slot_keeps_tangent_joins_and_grows_arc_radii() {
    // Stadium slot: lines y = +-r for x in [-L, L] + semicircular
    // caps. Offsetting outward keeps the tangent joins (NO corner
    // arcs) and grows the cap radii by d.
    let (l, r, d) = (10.0, 5.0, 2.0);
    let s = fresh("slice6_offset_slot");
    let bl = s.add_point(Point2d::new(-l, -r));
    let br = s.add_point(Point2d::new(l, -r));
    let tr = s.add_point(Point2d::new(l, r));
    let tl = s.add_point(Point2d::new(-l, r));
    let bottom = s.add_line(bl, br).expect("bottom");
    s.add_line(tr, tl).expect("top");
    let cap_r = s.add_arc(br, tr, r, true, false).expect("right cap");
    let cap_l = s.add_arc(tl, bl, r, true, false).expect("left cap");
    for id in [bl, br, tr, tl] {
        fix_point(&s, &id);
    }
    // Fully dimension the source: fixed endpoints pin everything
    // except each cap arc's own chord-offset DOF — pin those with
    // Radius dimensions so the source is rigid.
    for cap in [cap_r, cap_l] {
        s.add_constraint(required_dim(
            DimensionalConstraint::Radius(r),
            vec![EntityRef::Arc(cap)],
        ));
    }

    let outcome = sketch_ops::offset(&s, &EntityRef::Line(bottom), d).expect("offset");

    let new_arcs: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Arc(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(new_arcs.len(), 2, "two offset cap arcs, no corner arcs");
    for id in &new_arcs {
        let arc = s.arcs().get(id).expect("arc").arc;
        assert!(
            (arc.radius - (r + d)).abs() < 1e-9,
            "cap radius must grow by d: {}",
            arc.radius
        );
    }
    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "slot offset must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // Inward past the cap radius must refuse -- the caps would invert.
    let err = sketch_ops::offset(&s, &EntityRef::Line(bottom), -(r + 1.0))
        .expect_err("inversion must refuse");
    assert!(
        matches!(err, SketchOpError::OffsetTooLarge { .. }),
        "typed refusal expected, got {err:?}"
    );
}

#[test]
fn red_offset_circle_loop_shares_the_center_structurally() {
    let s = fresh("slice6_offset_circle");
    let center = s.add_point(Point2d::new(7.0, 3.0));
    let circle = s.add_circle_centered(center, 4.0).expect("circle");

    let outcome = sketch_ops::offset(&s, &EntityRef::Circle(circle), 1.5).expect("offset");

    let created_circles: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Circle(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(created_circles.len(), 1);
    let entry = s.circles().get(&created_circles[0]).expect("offset circle");
    assert_eq!(
        entry.center_point,
        Some(center),
        "offset circle must SHARE the source's center point (concentric \
         by construction)"
    );
    assert!((entry.circle.radius - 5.5).abs() < 1e-9);
    // Structural concentricity: only the radial gap needs minting.
    let gaps = count_constraints_of(&s, |t| {
        matches!(
            t,
            ConstraintType::Dimensional(DimensionalConstraint::OffsetDistance(_))
        )
    });
    assert_eq!(gaps, 1);
    let offsets = count_constraints_of(&s, |t| {
        matches!(t, ConstraintType::Geometric(GeometricConstraint::Offset))
    });
    assert_eq!(
        offsets, 0,
        "no Offset constraint needed when the center is shared"
    );
}

#[test]
fn offset_refuses_spline_loops_typed() {
    let s = fresh("slice6_offset_spline_refuse");
    // Closed loop: open cubic B-Spline bridged by a line.
    s.add_bspline(
        3,
        vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(10.0, 25.0),
            Point2d::new(30.0, 25.0),
            Point2d::new(40.0, 0.0),
        ],
        vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
    )
    .expect("bspline");
    let a = s.add_point(Point2d::new(40.0, 0.0));
    let b = s.add_point(Point2d::new(0.0, 0.0));
    let line = s.add_line(a, b).expect("closing line");
    let err = sketch_ops::offset(&s, &EntityRef::Line(line), 2.0).expect_err("must refuse");
    assert!(
        matches!(err, SketchOpError::Unsupported { .. }),
        "NURBS offset approximation is out of scope -- typed refuse, got {err:?}"
    );
}

// ── mirror ──────────────────────────────────────────────────────────

#[test]
fn red_mirror_about_construction_line_is_maintained_not_a_copy() {
    let s = fresh("slice6_mirror_maintained");
    // Vertical construction axis x = 0.
    let a1 = s.add_point(Point2d::new(0.0, -10.0));
    let a2 = s.add_point(Point2d::new(0.0, 10.0));
    let axis = s.add_line(a1, a2).expect("axis");
    s.set_construction(&EntityRef::Line(axis), true)
        .expect("construction");
    fix_point(&s, &a1);
    fix_point(&s, &a2);
    // Source L-shape, pinned by editable coordinates.
    let p1 = s.add_point(Point2d::new(2.0, 0.0));
    let p2 = s.add_point(Point2d::new(6.0, 0.0));
    let p3 = s.add_point(Point2d::new(6.0, 3.0));
    let l1 = s.add_line(p1, p2).expect("l1");
    let l2 = s.add_line(p2, p3).expect("l2");
    let mut pins = Vec::new();
    for (p, x, y) in [(p1, 2.0, 0.0), (p2, 6.0, 0.0), (p3, 6.0, 3.0)] {
        pins.push(s.add_constraint(required_dim(
            DimensionalConstraint::XCoordinate(x),
            vec![EntityRef::Point(p)],
        )));
        s.add_constraint(required_dim(
            DimensionalConstraint::YCoordinate(y),
            vec![EntityRef::Point(p)],
        ));
    }

    let outcome =
        sketch_ops::mirror(&s, &[EntityRef::Line(l1), EntityRef::Line(l2)], &axis).expect("mirror");

    let mirrored_lines = outcome
        .created
        .iter()
        .filter(|e| matches!(e, EntityRef::Line(_)))
        .count();
    let mirrored_points: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Point(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(mirrored_lines, 2);
    assert_eq!(
        mirrored_points.len(),
        3,
        "shared corner point must be mirrored ONCE"
    );
    let symmetric = count_constraints_of(&s, |t| {
        matches!(t, ConstraintType::Geometric(GeometricConstraint::Symmetric))
    });
    assert_eq!(symmetric, 3, "one Symmetric per mirrored point");

    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    // EXACT reflections: (2,0)→(−2,0), (6,0)→(−6,0), (6,3)→(−6,3).
    // (Pinned exactly — a scaled/broken reflection formula converged
    // to a wildly wrong point while still reporting zero violations.)
    for (ex, ey) in [(-2.0, 0.0), (-6.0, 0.0), (-6.0, 3.0)] {
        assert!(
            mirrored_points.iter().any(|id| {
                let p = s.get_point(id).expect("point");
                (p.x - ex).abs() < 1e-6 && (p.y - ey).abs() < 1e-6
            }),
            "expected a mirrored point at ({ex}, {ey}); got {:?}",
            mirrored_points
                .iter()
                .map(|id| s.get_point(id).expect("point"))
                .collect::<Vec<_>>()
        );
    }

    // MAINTAINED, not one-shot: edit the source (move p1.x 2 -> 4) and
    // re-solve -- the mirrored partner must follow to -4.
    s.update_dimensional_value(&pins[0], 4.0).expect("edit pin");
    let report = s.solve_constraints().expect("re-solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let mirrored_p1 = mirrored_points
        .iter()
        .find(|id| {
            let p = s.get_point(id).expect("point");
            (p.y - 0.0).abs() < 1e-6 && p.x < -3.0
        })
        .unwrap_or_else(|| panic!("mirrored p1 must have tracked the edit"));
    let p = s.get_point(mirrored_p1).expect("point");
    assert!(
        (p.x + 4.0).abs() < 1e-6,
        "mirror must be maintained through source edits: {p:?}"
    );
}

#[test]
fn mirror_refuses_a_profile_axis() {
    let s = fresh("slice6_mirror_axis_refuse");
    let a1 = s.add_point(Point2d::new(0.0, -10.0));
    let a2 = s.add_point(Point2d::new(0.0, 10.0));
    let axis = s.add_line(a1, a2).expect("axis"); // NOT construction
    let p = s.add_point(Point2d::new(2.0, 0.0));
    let err = sketch_ops::mirror(&s, &[EntityRef::Point(p)], &axis).expect_err("must refuse");
    assert!(
        matches!(err, SketchOpError::AxisNotConstruction { .. }),
        "spec: mirror is about a CONSTRUCTION line, got {err:?}"
    );
}

#[test]
fn red_mirror_circle_and_arc_flip_winding_and_share_radius() {
    let s = fresh("slice6_mirror_arc_circle");
    let a1 = s.add_point(Point2d::new(0.0, -10.0));
    let a2 = s.add_point(Point2d::new(0.0, 10.0));
    let axis = s.add_line(a1, a2).expect("axis");
    s.set_construction(&EntityRef::Line(axis), true)
        .expect("construction");
    let center = s.add_point(Point2d::new(5.0, 2.0));
    let circle = s.add_circle_centered(center, 1.5).expect("circle");
    let e1 = s.add_point(Point2d::new(3.0, 0.0));
    let e2 = s.add_point(Point2d::new(7.0, 0.0));
    let arc = s.add_arc(e1, e2, 2.5, true, false).expect("arc");

    let outcome = sketch_ops::mirror(&s, &[EntityRef::Circle(circle), EntityRef::Arc(arc)], &axis)
        .expect("mirror");

    let m_circle = outcome
        .created
        .iter()
        .find_map(|e| match e {
            EntityRef::Circle(id) => Some(*id),
            _ => None,
        })
        .expect("mirrored circle");
    let m_arc = outcome
        .created
        .iter()
        .find_map(|e| match e {
            EntityRef::Arc(id) => Some(*id),
            _ => None,
        })
        .expect("mirrored arc");

    let mc = s.circle_center_position(&m_circle).expect("center");
    assert!(
        (mc.x + 5.0).abs() < 1e-9 && (mc.y - 2.0).abs() < 1e-9,
        "mirrored circle center: {mc:?}"
    );
    let src_arc = s.arcs().get(&arc).expect("arc").arc;
    let dst_arc = s.arcs().get(&m_arc).expect("m_arc").arc;
    assert_eq!(
        dst_arc.ccw, !src_arc.ccw,
        "reflection must flip the winding"
    );
    assert!((dst_arc.radius - src_arc.radius).abs() < 1e-9);
    // The mirrored arc's midpoint is the reflection of the source's.
    let (sm, dm) = (src_arc.midpoint(), dst_arc.midpoint());
    assert!(
        (dm.x + sm.x).abs() < 1e-9 && (dm.y - sm.y).abs() < 1e-9,
        "arc midpoint must reflect: {sm:?} vs {dm:?}"
    );
    // Equal radii keep the pair in lock-step.
    let equals = count_constraints_of(&s, |t| {
        matches!(t, ConstraintType::Geometric(GeometricConstraint::Equal))
    });
    assert_eq!(equals, 2, "Equal radius for the circle pair and arc pair");
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

// ── patterns ────────────────────────────────────────────────────────

#[test]
fn red_linear_pattern_of_bores_full_gate() {
    // Plate outline + one dimensioned source hole -> 3-hole linear
    // pattern. The gate covers: shared-value Equal radius chain,
    // spacing Distance chain on a construction guide, DOF accounting
    // (fully constrained), solve, provenance lineage, construction
    // guide invisible to the profile, and 3 TRUE cylindrical bores in
    // the extrude.
    let s = fresh("slice6_linear_pattern");
    rectangle_outline(&s);
    // Two-pass: collect ids first — holding a DashMap iter guard
    // across get_mut on the same shard would deadlock.
    let point_ids: Vec<_> = s.points().iter().map(|e| *e.key()).collect();
    for id in point_ids {
        fix_point(&s, &id);
    }
    let center = s.add_point(Point2d::new(8.0, 15.0));
    let hole = s.add_circle_centered(center, 3.0).expect("hole");
    s.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(8.0),
        vec![EntityRef::Point(center)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(15.0),
        vec![EntityRef::Point(center)],
    ));
    let radius_pin = s.add_constraint(required_dim(
        DimensionalConstraint::Radius(3.0),
        vec![EntityRef::Circle(hole)],
    ));

    let outcome =
        sketch_ops::linear_pattern(&s, &[EntityRef::Circle(hole)], 3, 12.0, 0.0).expect("pattern");

    let instance_circles: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Circle(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(instance_circles.len(), 2, "count 3 = source + 2 instances");
    // Provenance lineage minted from day one (persistent-ids #11 note).
    for (k, id) in instance_circles.iter().enumerate() {
        let prov = s
            .provenance_of(&EntityRef::Circle(*id))
            .expect("instance carries provenance");
        assert_eq!(prov.op, SketchOpKind::LinearPattern);
        assert_eq!(prov.source, Some(EntityRef::Circle(hole)));
        assert_eq!(prov.instance, Some(k + 1));
    }
    // The guide is construction and invisible to the profile.
    let guides: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Line(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(guides.len(), 1, "one construction guide line");
    assert!(s.lines().get(&guides[0]).expect("guide").is_construction);

    // DOF: each instance (center 2 + radius 1) is pinned by
    // Distance + guide/PointOnCurve + Equal -- fully constrained.
    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "pattern must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // Instance centers at +12 and +24 along +X.
    let mut xs: Vec<f64> = instance_circles
        .iter()
        .map(|id| s.circle_center_position(id).expect("center").x)
        .collect();
    xs.sort_by(|x, y| x.partial_cmp(y).unwrap());
    assert!(
        (xs[0] - 20.0).abs() < 1e-6 && (xs[1] - 32.0).abs() < 1e-6,
        "{xs:?}"
    );

    // Shared-value radius: edit the SOURCE dimension -> every instance
    // follows through the Equal chain.
    s.update_dimensional_value(&radius_pin, 2.0).expect("edit");
    let report = s.solve_constraints().expect("re-solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    for id in &instance_circles {
        let r = s.circles().get(id).expect("circle").circle.radius;
        assert!(
            (r - 2.0).abs() < 1e-6,
            "Equal chain must propagate the radius edit: {r}"
        );
    }

    // Extrude: plate + 3 bores, all TRUE cylinders; the construction
    // guide contributes nothing.
    let regions = analytic_regions(&s);
    assert_eq!(regions.len(), 1);
    let mut model = BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &regions,
        EXTRUDE_H,
        None,
        Tolerance::default(),
    )
    .expect("extrude");
    assert_eq!(
        cylinder_faces(&model, solid),
        3,
        "three analytic bores expected"
    );
}

#[test]
fn red_circular_pattern_spokes_hold_the_bolt_circle() {
    let s = fresh("slice6_circular_pattern");
    let hub = s.add_point(Point2d::new(0.0, 0.0));
    fix_point(&s, &hub);
    let center = s.add_point(Point2d::new(10.0, 0.0));
    let hole = s.add_circle_centered(center, 2.0).expect("hole");
    s.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(10.0),
        vec![EntityRef::Point(center)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(center)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::Radius(2.0),
        vec![EntityRef::Circle(hole)],
    ));

    let step = std::f64::consts::FRAC_PI_2;
    let outcome = sketch_ops::circular_pattern(&s, &[EntityRef::Circle(hole)], &hub, 4, step)
        .expect("pattern");

    let instance_circles: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Circle(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(instance_circles.len(), 3, "count 4 = source + 3 instances");

    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "circular pattern must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // Instances at 90, 180, 270 degrees on the R10 bolt circle.
    let mut found = [false; 3];
    for id in &instance_circles {
        let c = s.circle_center_position(id).expect("center");
        for (i, (ex, ey)) in [(0.0, 10.0), (-10.0, 0.0), (0.0, -10.0)].iter().enumerate() {
            if (c.x - ex).abs() < 1e-6 && (c.y - ey).abs() < 1e-6 {
                found[i] = true;
            }
        }
    }
    assert_eq!(found, [true; 3], "bolt-circle placement");

    // Spokes are construction guides -- Equal length + Angle chained.
    let angles = count_constraints_of(&s, |t| {
        matches!(
            t,
            ConstraintType::Dimensional(DimensionalConstraint::Angle(_))
        )
    });
    assert_eq!(angles, 3, "one Angle(step) per instance spoke");
}

#[test]
fn pattern_refuses_unsupported_sources_typed() {
    let s = fresh("slice6_pattern_refuse");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(5.0, 0.0));
    let line = s.add_line(a, b).expect("line");
    let err = sketch_ops::linear_pattern(&s, &[EntityRef::Line(line)], 3, 10.0, 0.0)
        .expect_err("must refuse");
    assert!(
        matches!(err, SketchOpError::Unsupported { .. }),
        "v1 patterns maintain point/circle instances only -- typed refuse, got {err:?}"
    );
    assert_eq!(s.lines().len(), 1, "no partial mutation on refusal");
    assert_eq!(s.points().len(), 2);
}

#[test]
fn provenance_is_cleaned_up_on_delete() {
    let s = fresh("slice6_provenance_cleanup");
    let src = s.add_point(Point2d::new(5.0, 5.0));
    fix_point(&s, &src);
    let outcome =
        sketch_ops::linear_pattern(&s, &[EntityRef::Point(src)], 2, 3.0, 0.0).expect("pattern");
    let instance = outcome
        .created
        .iter()
        .find_map(|e| match e {
            EntityRef::Point(id) => Some(*id),
            _ => None,
        })
        .expect("instance point");
    let iref = EntityRef::Point(instance);
    assert!(s.provenance_of(&iref).is_some());
    s.delete_point(&instance).expect("delete");
    assert!(
        s.provenance_of(&iref).is_none(),
        "provenance must not dangle after entity deletion"
    );
}
