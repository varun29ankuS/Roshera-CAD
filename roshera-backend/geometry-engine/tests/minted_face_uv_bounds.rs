// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **Every curved face minted OUTSIDE a boolean carried the `[0, 1]²` UV
//! placeholder, so the kernel refused to say how big it was.**
//!
//! Task 12 made `Face::uv_bounds` honest: only [`Face::set_uv_bounds`] marks a
//! domain as measured, and the agent-facing layers refuse rather than integrate
//! over the `Face::new` placeholder. It then wired exactly ONE mint —
//! `operations::boolean.rs` — leaving 69 other production `Face::new` sites
//! minting curved faces whose domain nobody ever measured. The visible
//! consequence: a plain cylinder's wall answered `area: None`, while the SAME
//! cylinder pushed through a no-op boolean answered `1257 mm²`.
//!
//! These tests pin the fix at the mints, primitive and operation alike, and
//! they pin it TWICE over:
//!
//! * the **domain** — a primitive's measured `uv_bounds` must equal the domain
//!   its own construction geometry implies (a cylinder `u ∈ [0, 2π]`,
//!   `v ∈ [0, h]`). The domain is *measured from the boundary loop*, never
//!   hard-coded, so this assertion is the agreement proof: loop measurement
//!   versus analytic truth.
//! * the **area** — the quadrature over that domain against the closed-form
//!   area (`2πrh`, `π(r₁+r₂)ℓ`, `4πr²`, `4π²Rr`), within 0.1%. A domain with
//!   the right *span* but the wrong *position* passes the first assertion's
//!   span check and fails this one on a non-uniform surface; a domain with the
//!   right area and the wrong extent fails the first.
//!
//! [`deep_cloned_face_keeps_its_measured_domain`] is deliberately built on a
//! BOOLEAN result: that face is already measured at HEAD, so the test isolates
//! `deep_clone`'s own drop of the flag (`clone_faces` re-mints through
//! `Face::new`) rather than passing or failing on the mint fix.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::chamfer::{
    ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::deep_clone::deep_clone_solid;
use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::imprint::{imprint_curves_on_face, ImprintOptions};
use geometry_engine::operations::loft::LoftType;
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::{
    boolean_operation, chamfer_edges, create_pattern, fillet_edges, loft_profiles, offset_solid,
    sweep_profile, BooleanOp, BooleanOptions, FilletOptions, LoftOptions, OffsetOptions,
    PatternOptions, PatternType, SweepOptions,
};
use geometry_engine::primitives::curve::{Arc, Circle, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::select::{
    resolve_face, Extremal, FaceQuery, SelectError, SurfaceKind,
};

use std::f64::consts::{PI, TAU};

// ─── fixtures ────────────────────────────────────────────────────────────────

fn cylinder(model: &mut BRepModel, r: f64, h: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, r, h)
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn frustum(model: &mut BRepModel, r_base: f64, r_top: f64, h: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_cone_3d(Point3::ORIGIN, Vector3::Z, r_base, r_top, h)
        .expect("frustum")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn sphere(model: &mut BRepModel, r: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b.create_sphere_3d(Point3::ORIGIN, r).expect("sphere") {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn torus(model: &mut BRepModel, major: f64, minor: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_torus_3d(Point3::ORIGIN, Vector3::Z, major, minor)
        .expect("torus")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn face_ids(model: &BRepModel, solid: SolidId) -> Vec<u32> {
    let s = model.solids.get(solid).expect("solid");
    let mut out = Vec::new();
    for sh in [s.outer_shell]
        .into_iter()
        .chain(s.inner_shells.iter().copied())
    {
        if let Some(shell) = model.shells.get(sh) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

/// Every face of `solid` whose supporting surface reports `kind`.
fn faces_of_kind(model: &BRepModel, solid: SolidId, kind: &str) -> Vec<u32> {
    face_ids(model, solid)
        .into_iter()
        .filter(|&fid| {
            model
                .faces
                .get(fid)
                .and_then(|f| model.surfaces.get(f.surface_id))
                .map(|s| s.type_name() == kind)
                .unwrap_or(false)
        })
        .collect()
}

fn only_face_of_kind(model: &BRepModel, solid: SolidId, kind: &str) -> u32 {
    let f = faces_of_kind(model, solid, kind);
    assert_eq!(f.len(), 1, "expected exactly one {kind} face, got {f:?}");
    f[0]
}

fn measured(model: &BRepModel, fid: u32) -> [f64; 4] {
    model
        .faces
        .get(fid)
        .expect("face")
        .measured_uv_bounds()
        .unwrap_or_else(|| {
            panic!(
                "face {fid} carries no MEASURED parametric domain — its mint never \
                 measured one, so every quantity that reads the domain refuses"
            )
        })
}

fn area(model: &mut BRepModel, fid: u32) -> f64 {
    model
        .query_face(fid)
        .expect("face report")
        .area
        .unwrap_or_else(|| {
            panic!("face {fid} refused its area — the kernel does not know its domain")
        })
}

fn rel_err(a: f64, b: f64) -> f64 {
    ((a - b) / b).abs()
}

/// 0.1%, the budget the brief sets for an analytic-versus-quadrature check.
const AREA_BUDGET: f64 = 1e-3;

fn assert_area(model: &mut BRepModel, fid: u32, expected: f64, what: &str) {
    let got = area(model, fid);
    assert!(
        rel_err(got, expected) <= AREA_BUDGET,
        "{what}: area {got} mm2 disagrees with the closed form {expected} mm2 by {:.4}% \
         (budget {:.2}%)",
        100.0 * rel_err(got, expected),
        100.0 * AREA_BUDGET
    );
}

/// Domain agreement, in the only form that is parameterisation-independent
/// enough to assert: the measured SPAN in each direction.
fn assert_span(got: [f64; 4], want_u: f64, want_v: f64, what: &str) {
    let (u, v) = (got[1] - got[0], got[3] - got[2]);
    assert!(
        (u - want_u).abs() <= 1e-9 && (v - want_v).abs() <= 1e-9,
        "{what}: measured domain {got:?} spans ({u}, {v}); the construction geometry \
         implies ({want_u}, {want_v})"
    );
}

// ─── primitives ──────────────────────────────────────────────────────────────

/// A plain cylinder's wall: `u ∈ [0, 2π]`, `v ∈ [0, h]`, area `2πrh`.
///
/// The domain is asserted EXACTLY (not just its span) because
/// `create_cylinder_topology` states it in its own comment — the lateral loop
/// is "a CCW rectangle with corners at (0, 0), (2π, 0), (2π, h), (0, h)". The
/// fix must MEASURE that from the loop and arrive at what the comment claims;
/// hard-coding the comment's numbers would prove nothing.
#[test]
fn primitive_cylinder_lateral_measures_its_own_domain_and_area() {
    const R: f64 = 10.0;
    const H: f64 = 20.0;
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, R, H);
    let wall = only_face_of_kind(&model, cyl, "Cylinder");

    let b = measured(&model, wall);
    assert!(
        (b[0] - 0.0).abs() <= 1e-9
            && (b[1] - TAU).abs() <= 1e-9
            && (b[2] - 0.0).abs() <= 1e-9
            && (b[3] - H).abs() <= 1e-9,
        "cylinder wall domain measured {b:?}, construction implies [0, 2π, 0, {H}]"
    );
    assert_area(&mut model, wall, TAU * R * H, "cylinder wall");
}

/// A cone FRUSTUM's lateral: area `π(r₁ + r₂)ℓ` with `ℓ = √(h² + Δr²)`.
///
/// The area oracle is closed-form and parameterisation-free, so it is a real
/// check on the quadrature and not a restatement of whatever domain was
/// written. The span assertion adds that the domain is a full revolution.
#[test]
fn primitive_cone_frustum_lateral_measures_its_own_domain_and_area() {
    const R1: f64 = 10.0;
    const R2: f64 = 4.0;
    const H: f64 = 12.0;
    let mut model = BRepModel::new();
    let sol = frustum(&mut model, R1, R2, H);
    let lateral = only_face_of_kind(&model, sol, "Cone");

    let b = measured(&model, lateral);
    let slant = (H * H + (R1 - R2) * (R1 - R2)).sqrt();
    assert!(
        (b[1] - b[0] - TAU).abs() <= 1e-9,
        "a full frustum's lateral sweeps a whole revolution; measured {b:?}"
    );
    assert_area(
        &mut model,
        lateral,
        PI * (R1 + R2) * slant,
        "frustum lateral",
    );
}

/// A whole sphere keeps the domain its builder measured: `[0, 2π] × [0, π]`.
///
/// **This is the control, not a RED.** `create_sphere_3d` already calls
/// `set_uv_bounds` with the surface's declared domain — which a whole sphere
/// provably covers — so it passes at HEAD. It is here because the shared mint
/// helper now runs on this face too, and a sphere has a pole at each `v`
/// endpoint: the loop measurement must REFUSE there and leave the analytic
/// domain standing, never overwrite it.
///
/// **The brief asked for `4πr²` here and it is not assertable, for a reason
/// that predates this task.** `Face::area` goes through `Face::compute_stats`,
/// which sums a perimeter over `all_loops()` BEFORE the area is read, and
/// `Loop::compute_stats` refuses a loop with fewer than three vertices
/// (`"Loop has fewer than 3 vertices"`). A whole sphere's outer loop is EMPTY
/// by construction, so its area has always been `Err` — measured at HEAD in
/// this file's own RED run, before any change here. The domain is what this
/// task owns and what this test pins.
#[test]
fn primitive_sphere_keeps_the_domain_its_builder_measured() {
    const R: f64 = 7.0;
    let mut model = BRepModel::new();
    let sol = sphere(&mut model, R);
    let f = only_face_of_kind(&model, sol, "Sphere");
    let b = measured(&model, f);
    assert!(
        (b[0] - 0.0).abs() <= 1e-12
            && (b[1] - TAU).abs() <= 1e-12
            && (b[2] - 0.0).abs() <= 1e-12
            && (b[3] - PI).abs() <= 1e-12,
        "sphere domain {b:?} is not the surface's own [0, 2π] × [0, π]"
    );
}

/// A whole torus: area `4π²Rr`, domain a full revolution in both directions.
#[test]
fn primitive_torus_measures_its_own_domain_and_area() {
    const MAJOR: f64 = 20.0;
    const MINOR: f64 = 5.0;
    let mut model = BRepModel::new();
    let sol = torus(&mut model, MAJOR, MINOR);
    let faces = faces_of_kind(&model, sol, "Torus");
    assert_eq!(faces.len(), 1, "a whole torus is one face, got {faces:?}");
    let f = faces[0];

    assert_span(measured(&model, f), TAU, TAU, "torus");
    assert_area(&mut model, f, 4.0 * PI * PI * MAJOR * MINOR, "torus");
}

// ─── operations ──────────────────────────────────────────────────────────────

/// An extruded circle's wall is a cylinder of the same `2πrh`.
///
/// `extrude_profile` mints its side walls through its own `Face::new` sites, so
/// this is a different mint from the primitive builder's and needs its own
/// pin.
#[test]
fn extruded_circle_wall_measures_its_own_domain_and_area() {
    const R: f64 = 6.0;
    const H: f64 = 15.0;
    let mut model = BRepModel::new();

    // FOUR quarter arcs, not one closed circle: `extrude_profile` refuses a
    // profile of fewer than three edges ("Need at least 3 edges to define a
    // plane"), and a full circle is one. Four arcs are the smallest closed
    // circular profile it accepts, and they extrude to four cylindrical wall
    // faces whose areas must sum to the whole `2πrh`.
    let quarter = TAU / 4.0;
    let corners: Vec<u32> = (0..4)
        .map(|i| {
            let a = quarter * i as f64;
            model.vertices.add(R * a.cos(), R * a.sin(), 0.0)
        })
        .collect();
    let mut edges = Vec::new();
    for i in 0..4 {
        let arc = Arc::new(Point3::ORIGIN, Vector3::Z, R, quarter * i as f64, quarter)
            .expect("quarter arc");
        let cid = model.curves.add(Box::new(arc));
        edges.push(model.edges.add(Edge::new(
            0,
            corners[i],
            corners[(i + 1) % 4],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }

    let sol = extrude_profile(
        &mut model,
        edges,
        ExtrudeOptions {
            direction: Vector3::Z,
            distance: H,
            ..Default::default()
        },
    )
    .expect("extrude circle");

    let walls = faces_of_kind(&model, sol, "Cylinder");
    let kinds: Vec<String> = face_ids(&model, sol)
        .into_iter()
        .filter_map(|fid| {
            model
                .faces
                .get(fid)
                .and_then(|f| model.surfaces.get(f.surface_id))
                .map(|s| format!("{fid}:{}", s.type_name()))
        })
        .collect();
    assert_eq!(
        walls.len(),
        4,
        "four quarter arcs extrude to four cylindrical walls; the solid carries {kinds:?}"
    );
    let mut total = 0.0;
    let mut detail = Vec::new();
    for &w in &walls {
        let b = measured(&model, w);
        let a = area(&mut model, w);
        detail.push(format!("{w}: bounds {b:?} area {a}"));
        total += a;
    }
    assert!(
        rel_err(total, TAU * R * H) <= AREA_BUDGET,
        "extruded circle walls total {total} mm2 vs closed form {} mm2 ({:.4}%); per wall {detail:?}",
        TAU * R * H,
        100.0 * rel_err(total, TAU * R * H)
    );
}

/// A revolved rectangular meridian emits analytic cylinder bands; the outer
/// band is `2πrh`.
///
/// This is the mint the `face_score` ranking gate is blocked on: Task 12
/// measured that the nozzle label fixtures rank throat against chamber by
/// `SmallestArea`/`LargestArea` over exactly these revolved walls.
#[test]
fn revolved_profile_wall_measures_its_own_domain_and_area() {
    const R_OUT: f64 = 8.0;
    const R_IN: f64 = 5.0;
    const H: f64 = 10.0;
    let mut model = BRepModel::new();

    // A closed tube meridian in the (r, z) half-plane.
    let pts = [(R_IN, 0.0), (R_OUT, 0.0), (R_OUT, H), (R_IN, H)];
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| model.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = model.curves.add(Box::new(line));
        edges.push(model.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }

    let sol = revolve_profile(
        &mut model,
        edges,
        RevolveOptions {
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: TAU,
            segments: 48,
            ..Default::default()
        },
    )
    .expect("revolve tube");

    let walls = faces_of_kind(&model, sol, "Cylinder");
    assert_eq!(
        walls.len(),
        2,
        "a revolved tube is one outer and one inner cylinder band, got {walls:?}"
    );

    // Identify the bands by radius rather than by store order.
    let mut outer = None;
    for &w in &walls {
        let _ = measured(&model, w);
        let a = area(&mut model, w);
        if rel_err(a, TAU * R_OUT * H) <= AREA_BUDGET {
            outer = Some(w);
        }
    }
    assert!(
        outer.is_some(),
        "no revolved band reported the outer wall's closed-form area {}; \
         the bands reported {:?}",
        TAU * R_OUT * H,
        walls
            .iter()
            .map(|&w| area(&mut model, w))
            .collect::<Vec<_>>()
    );

    // And the inner band must be the other one, not the same face twice.
    let inner: Vec<f64> = walls
        .iter()
        .filter(|&&w| Some(w) != outer)
        .map(|&w| area(&mut model, w))
        .collect();
    assert_eq!(inner.len(), 1, "exactly one band is not the outer one");
    assert!(
        rel_err(inner[0], TAU * R_IN * H) <= AREA_BUDGET,
        "inner band area {} mm2 vs closed form {} mm2",
        inner[0],
        TAU * R_IN * H
    );
}

// ─── deep clone ──────────────────────────────────────────────────────────────

/// Cloning a solid must clone what the kernel KNOWS about it.
///
/// `deep_clone::clone_faces` re-mints every face through `Face::new`, which
/// writes the placeholder and leaves `uv_bounds_measured` false — so a clone of
/// a fully measured solid was a solid the kernel had forgotten how to measure.
/// The source here is a BOOLEAN result, measured at HEAD, so this test fails
/// for exactly one reason: the clone dropped the state.
#[test]
fn deep_cloned_face_keeps_its_measured_domain() {
    const R: f64 = 10.0;
    const H: f64 = 20.0;
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, R, H);
    let tool = {
        let mut b = TopologyBuilder::new(&mut model);
        match b.create_sphere_3d(Point3::new(R, 0.0, 0.5 * H), 4.0) {
            Ok(GeometryId::Solid(id)) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    let cut = boolean_operation(
        &mut model,
        cyl,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let src_walls = faces_of_kind(&model, cut, "Cylinder");
    let src_measured: Vec<[f64; 4]> = src_walls
        .iter()
        .filter_map(|&w| model.faces.get(w).and_then(|f| f.measured_uv_bounds()))
        .collect();
    assert!(
        !src_measured.is_empty(),
        "the fixture must start from a solid the kernel HAS measured, else the \
         clone assertion below is vacuous"
    );

    let clone = deep_clone_solid(&mut model, cut, None).expect("deep clone");
    let clone_measured: Vec<[f64; 4]> = faces_of_kind(&model, clone, "Cylinder")
        .iter()
        .filter_map(|&w| model.faces.get(w).and_then(|f| f.measured_uv_bounds()))
        .collect();

    assert_eq!(
        clone_measured.len(),
        src_measured.len(),
        "the clone kept {} of {} measured cylindrical domains — a clone of a \
         measured solid must be a measured solid",
        clone_measured.len(),
        src_measured.len()
    );
    for (c, s) in clone_measured.iter().zip(src_measured.iter()) {
        assert_eq!(
            c, s,
            "a cloned face's measured domain must equal its source's"
        );
    }
}

// ─── the refusal: an unmeasured candidate is not a candidate to drop ─────────

/// **A ranking must REFUSE over a candidate it could not score, never rank the
/// rest and hand back the runner-up.**
///
/// `face_score` gates on the measured domain, and its caller used to fold that
/// refusal into a silent drop. That turns an undecidable query into a confident
/// answer: the face that was dropped is exactly the one that might have won.
/// The normal filter has always got this right (`SelectError::Unmeasurable`
/// carries both the matches and the untestable ones); the extremal path did
/// not.
///
/// The fixture is an APEX cone, and the choice matters. Its lateral is
/// unmeasured *by the geometry*, not by an unwired mint: the apex is a pole, so
/// the boundary bbox would understate a domain that encloses it, and
/// `measure_face_uv_domain` refuses by design. Every mint in the kernel is
/// wired and this face is still unmeasured — which is why the hazard is
/// reachable today rather than hypothetical. A query with no `.facing()` skips
/// the normal filter entirely, so nothing upstream catches it either.
#[test]
fn largest_area_refuses_over_an_unscorable_candidate_rather_than_ranking_the_rest() {
    let mut model = BRepModel::new();

    // A plinth UNION an apex cone standing on it. The union is what gives the
    // query both halves in ONE solid: the plinth contributes planar faces with
    // polygonal boundaries, which score cleanly, and the cone contributes an
    // apex lateral that cannot be scored at all.
    //
    // (An apex cone alone will not do, and the reason is worth recording: its
    // only other face is a cap bounded by ONE closed circle edge, and
    // `Loop::compute_stats` refuses a loop with fewer than three vertices - so
    // that cap lands in `FaceScore::Missing` and the query has no genuine
    // match left to be robbed of. Pre-existing, unrelated to this task, and it
    // made the first version of this test weaker than it looked.)
    let plinth = box_solid(&mut model, 40.0, 40.0, 6.0);
    let cone = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_cone_3d(Point3::new(0.0, 0.0, 2.0), Vector3::Z, 10.0, 0.0, 14.0)
            .expect("apex cone")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    let part = boolean_operation(
        &mut model,
        plinth,
        cone,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union plinth and cone");

    // Preconditions, asserted rather than assumed.
    let laterals = faces_of_kind(&model, part, "Cone");
    assert!(
        !laterals.is_empty(),
        "the union must keep the cone's lateral"
    );
    let unmeasured: Vec<u32> = laterals
        .iter()
        .copied()
        .filter(|&f| {
            !model
                .faces
                .get(f)
                .expect("lateral")
                .uv_bounds_are_measured()
        })
        .collect();
    assert!(
        !unmeasured.is_empty(),
        "fixture precondition: the apex pole makes a conical face unmeasurable, \
         which is what puts an unscorable candidate in the query. Every mint is \
         wired, so this is the geometry refusing, not a mint forgetting."
    );

    let q = FaceQuery::new(SurfaceKind::Any).extremal(Extremal::LargestArea);
    match resolve_face(&mut model, part, &q) {
        Err(SelectError::Unmeasurable {
            matched,
            unmeasurable: reported,
        }) => {
            for f in &unmeasured {
                assert!(
                    reported.contains(f),
                    "the unscorable conical face {f} must be NAMED as the \
                     obstacle, not dropped; reported {reported:?}"
                );
            }
            assert!(
                !matched.is_empty(),
                "the faces that COULD be scored are reported alongside it, so a \
                 caller can name the obstacle rather than retry blind"
            );
        }
        other => panic!(
            "a candidate set holding an unscorable face must refuse, not rank \
             the remainder and answer; got {other:?}"
        ),
    }
}

// ─── the widened claim, asserted across every operation that mints ──────────

/// A closed circle as ONE self-closing edge — the idiom loft and sweep take as
/// a circular profile.
fn closed_circle(model: &mut BRepModel, centre: Point3, radius: f64) -> Vec<u32> {
    let seam = model
        .vertices
        .add_or_find(centre.x + radius, centre.y, centre.z, 1e-6);
    let cid = model.curves.add(Box::new(
        Circle::new(centre, Vector3::Z, radius).expect("circle"),
    ));
    vec![model.edges.add(Edge::new(
        0,
        seam,
        seam,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ))]
}

fn box_solid(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b.create_box_3d(w, h, d).expect("box") {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// The planar face of `solid` whose boundary sits highest in `z`.
///
/// Picked by geometry, not by store order: `faces_of_kind(..).first()` returns
/// whichever planar face the shell happens to list first, and offsetting a
/// SIDE wall away instead of the top leaves a shell the validator rejects
/// (measured: three `OrientationError`s from a boundary walk that no longer
/// closes). The fixture must name the face it means.
fn top_planar_face(model: &BRepModel, solid: SolidId) -> u32 {
    let mut best: Option<(u32, f64)> = None;
    for fid in faces_of_kind(model, solid, "Plane") {
        let (mean, _, _) = face_boundary_mean_z(model, fid);
        if best.map(|(_, z)| mean > z).unwrap_or(true) {
            best = Some((fid, mean));
        }
    }
    best.expect("solid has a planar face").0
}

/// Mean `z` of a face's outer-loop vertices, plus its vertex count.
fn face_boundary_mean_z(model: &BRepModel, fid: u32) -> (f64, usize, usize) {
    let mut sum = 0.0;
    let mut n = 0usize;
    if let Some(l) = model
        .faces
        .get(fid)
        .and_then(|f| model.loops.get(f.outer_loop))
    {
        for &e in &l.edges {
            if let Some(edge) = model.edges.get(e) {
                for v in [edge.start_vertex, edge.end_vertex] {
                    if let Some(vx) = model.vertices.get(v) {
                        sum += vx.position[2];
                        n += 1;
                    }
                }
            }
        }
    }
    (
        if n == 0 {
            f64::NEG_INFINITY
        } else {
            sum / n as f64
        },
        n,
        n,
    )
}

/// One edge of `solid`'s outer shell — enough for a single-edge blend.
fn any_edge(model: &BRepModel, solid: SolidId) -> u32 {
    for fid in face_ids(model, solid) {
        if let Some(l) = model
            .faces
            .get(fid)
            .and_then(|f| model.loops.get(f.outer_loop))
        {
            if let Some(&e) = l.edges.first() {
                return e;
            }
        }
    }
    panic!("solid has no edges");
}

/// A straight path edge from the origin along +Z.
fn z_path(model: &mut BRepModel, length: f64) -> u32 {
    let va = model.vertices.add(0.0, 0.0, 0.0);
    let vb = model.vertices.add(0.0, 0.0, length);
    let line = Line::new(Point3::ORIGIN, Point3::new(0.0, 0.0, length));
    let cid = model.curves.add(Box::new(line));
    model.edges.add(Edge::new(
        0,
        va,
        vb,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ))
}

/// Collect every face in `faces` that carries NO measured parametric domain,
/// tagged by `op`, minus the surface kinds `allowed` names.
///
/// **The predicate is strictly "measured". Planes are NOT exempt**, and that is
/// load-bearing rather than incidental: `loft.rs::build_loft_cap` — the mint
/// this whole test exists to have caught — produces a PLANAR cap, so a
/// `measured || is_plane` predicate would have passed straight over it. The
/// `loftcap` mutation demonstrates exactly that: removing the helper call there
/// fails this test with `["loft: face 2 (Plane)", "loft: face 66 (Plane)"]`.
///
/// (A plane's *consumers* are a different question. `Face::domain_is_known`
/// treats an unmeasured plane as answerable because normal, curvature and the
/// planar area branch are all position-independent on it. That is a statement
/// about what may be REPORTED, not about whether the mint did its job, and
/// this test is about the mint.)
///
/// The only escape is `allowed`, passed per op and empty everywhere but one
/// place. An entry there is a claim that the GEOMETRY refuses, not that a mint
/// forgot.
///
/// **Two known coverage gaps, named here so the next reader does not have to
/// rediscover them:**
///
/// 1. **`imprint` is not exercised.** `imprint_curves_on_face` needs a curve
///    provably lying on the target face, and the cheap fixtures tried here
///    produced typed rejections rather than an imprint; manufacturing one means
///    hand-building the B-Rep configuration. Its single mint is wired, but no
///    end-to-end path in this test walks it.
/// 2. **The boolean row exempts `Sphere`.** The surviving cap of a tool sphere
///    is bounded by a constant-latitude rim on a surface with a pole inside its
///    own `v` domain, so `measure_face_uv_domain` refuses it by design. That
///    exemption is correct for the case at hand and is also a hole: a change
///    that stopped the boolean measuring spherical faces ENTIRELY would not
///    fail here. `tests/boolean_face_uv_bounds.rs` covers that path.
fn collect_unmeasured(
    model: &BRepModel,
    faces: &[u32],
    op: &str,
    allowed: &[&str],
    out: &mut Vec<String>,
) {
    assert!(!faces.is_empty(), "{op}: produced no faces at all");
    for &fid in faces {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        if face.uv_bounds_are_measured() {
            continue;
        }
        let kind = model
            .surfaces
            .get(face.surface_id)
            .map(|s| s.type_name().to_string())
            .unwrap_or_else(|| "<no surface>".to_string());
        if allowed.contains(&kind.as_str()) {
            continue;
        }
        out.push(format!("{op}: face {fid} ({kind})"));
    }
}

/// **The tripwire for the next missed mint.**
///
/// The claim this task makes is not "the primitives measure" but *every curved
/// face minted outside a boolean carries a measured UV domain*. Nothing
/// asserted that whole claim, and the gap showed immediately:
/// `loft.rs::build_loft_cap` was minting through `Face::new` with no
/// measurement while the site table listed it as done, because the enumeration
/// matched `.faces.add(` on one line and that particular call is split across
/// three.
///
/// One test with per-op sections rather than ten tests, deliberately: the
/// failure names the op and the face, and an op added later that forgets the
/// helper fails here without anyone remembering to add a case.
#[test]
fn every_operation_that_mints_a_curved_face_measures_its_domain() {
    // Accumulated, not asserted per op, so ONE run names every op that fails
    // rather than stopping at the first.
    let mut bad: Vec<String> = Vec::new();
    // --- boolean: the one site that was already wired ---
    {
        let mut model = BRepModel::new();
        let cyl = cylinder(&mut model, 10.0, 20.0);
        let tool = {
            let mut b = TopologyBuilder::new(&mut model);
            match b.create_sphere_3d(Point3::new(10.0, 0.0, 10.0), 4.0) {
                Ok(GeometryId::Solid(id)) => id,
                other => panic!("expected solid, got {other:?}"),
            }
        };
        let cut = boolean_operation(
            &mut model,
            cyl,
            tool,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("difference");
        let faces = face_ids(&model, cut);
        assert!(!faces.is_empty(), "boolean produced no faces");
        // `Sphere` is the ONE allowed exception, and it is a property of the
        // GEOMETRY rather than of the mint: the surviving cap of the tool
        // sphere is bounded by a constant-latitude rim, and its surface has a
        // pole inside its own `v` domain, so `measure_face_uv_domain` refuses
        // by design - the boundary bbox would understate a domain that
        // encloses the pole. The face then refuses its area and curvature
        // downstream, which is the honest answer. Every other kind, on every
        // other op, must be measured; those empty allow-lists are what make
        // this a tripwire rather than a formality.
        collect_unmeasured(&model, &faces, "boolean", &["Sphere"], &mut bad);
    }

    // --- extrude: four quarter arcs -> a capped cylinder ---
    {
        let mut model = BRepModel::new();
        let quarter = TAU / 4.0;
        let corners: Vec<u32> = (0..4)
            .map(|i| {
                let a = quarter * i as f64;
                model.vertices.add(6.0 * a.cos(), 6.0 * a.sin(), 0.0)
            })
            .collect();
        let mut edges = Vec::new();
        for i in 0..4 {
            let arc = Arc::new(Point3::ORIGIN, Vector3::Z, 6.0, quarter * i as f64, quarter)
                .expect("quarter arc");
            let cid = model.curves.add(Box::new(arc));
            edges.push(model.edges.add(Edge::new(
                0,
                corners[i],
                corners[(i + 1) % 4],
                cid,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            )));
        }
        let sol = extrude_profile(
            &mut model,
            edges,
            ExtrudeOptions {
                direction: Vector3::Z,
                distance: 9.0,
                ..Default::default()
            },
        )
        .expect("extrude");
        collect_unmeasured(&model, &face_ids(&model, sol), "extrude", &[], &mut bad);
    }

    // --- revolve: a closed tube meridian -> two analytic cylinder bands ---
    {
        let mut model = BRepModel::new();
        let pts = [(5.0, 0.0), (8.0, 0.0), (8.0, 10.0), (5.0, 10.0)];
        let verts: Vec<_> = pts
            .iter()
            .map(|(r, z)| model.vertices.add(*r, 0.0, *z))
            .collect();
        let mut edges = Vec::new();
        for i in 0..pts.len() {
            let j = (i + 1) % pts.len();
            let line = Line::new(
                Point3::new(pts[i].0, 0.0, pts[i].1),
                Point3::new(pts[j].0, 0.0, pts[j].1),
            );
            let cid = model.curves.add(Box::new(line));
            edges.push(model.edges.add(Edge::new(
                0,
                verts[i],
                verts[j],
                cid,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            )));
        }
        let sol = revolve_profile(
            &mut model,
            edges,
            RevolveOptions {
                axis_origin: Point3::ZERO,
                axis_direction: Vector3::Z,
                angle: TAU,
                segments: 48,
                ..Default::default()
            },
        )
        .expect("revolve");
        collect_unmeasured(&model, &face_ids(&model, sol), "revolve", &[], &mut bad);
    }

    // --- loft: two circles -> a conical frustum. Its CAP is the site that was
    //     missed; the laterals were already wired. ---
    {
        let mut model = BRepModel::new();
        let p0 = closed_circle(&mut model, Point3::ORIGIN, 8.0);
        let p1 = closed_circle(&mut model, Point3::new(0.0, 0.0, 12.0), 4.0);
        let sol = loft_profiles(
            &mut model,
            vec![p0, p1],
            LoftOptions {
                loft_type: LoftType::Linear,
                create_solid: true,
                ..Default::default()
            },
        )
        .expect("loft two circles");
        collect_unmeasured(&model, &face_ids(&model, sol), "loft", &[], &mut bad);
    }

    // --- sweep: a circle along a straight path -> a cylinder ---
    {
        let mut model = BRepModel::new();
        let profile = closed_circle(&mut model, Point3::ORIGIN, 5.0);
        let path = z_path(&mut model, 14.0);
        let sol = sweep_profile(&mut model, profile, path, SweepOptions::default())
            .expect("sweep a circle along a line");
        collect_unmeasured(&model, &face_ids(&model, sol), "sweep", &[], &mut bad);
    }

    // --- fillet: one box edge blended -> a cylindrical blend face ---
    {
        let mut model = BRepModel::new();
        let bx = box_solid(&mut model, 20.0, 20.0, 20.0);
        let edge = any_edge(&model, bx);
        fillet_edges(
            &mut model,
            bx,
            vec![edge],
            FilletOptions {
                fillet_type: FilletType::Constant(2.0),
                radius: 2.0,
                propagation: FilletProp::None,
                ..Default::default()
            },
        )
        .expect("fillet one box edge");
        collect_unmeasured(&model, &face_ids(&model, bx), "fillet", &[], &mut bad);
    }

    // --- chamfer: one box edge cut back ---
    {
        let mut model = BRepModel::new();
        let bx = box_solid(&mut model, 20.0, 20.0, 20.0);
        let edge = any_edge(&model, bx);
        chamfer_edges(
            &mut model,
            bx,
            vec![edge],
            ChamferOptions {
                chamfer_type: ChamferType::EqualDistance(2.0),
                distance1: 2.0,
                distance2: 2.0,
                symmetric: true,
                propagation: ChamferProp::None,
                ..Default::default()
            },
        )
        .expect("chamfer one box edge");
        collect_unmeasured(&model, &face_ids(&model, bx), "chamfer", &[], &mut bad);
    }

    // --- offset: hollow a box by removing one face ---
    {
        let mut model = BRepModel::new();
        let bx = box_solid(&mut model, 20.0, 20.0, 20.0);
        let top = top_planar_face(&model, bx);
        let hollow = offset_solid(&mut model, bx, 2.0, vec![top], OffsetOptions::default())
            .expect("offset (shell) a box");
        collect_unmeasured(&model, &face_ids(&model, hollow), "offset", &[], &mut bad);
    }

    // --- pattern: a CYLINDER's faces repeated, so the copies are curved ---
    {
        let mut model = BRepModel::new();
        let cyl = cylinder(&mut model, 4.0, 10.0);
        let seed = face_ids(&model, cyl);
        let groups = create_pattern(
            &mut model,
            seed,
            PatternType::Linear {
                direction: Vector3::X,
                spacing: 20.0,
                count: 3,
            },
            PatternOptions::default(),
        )
        .expect("linear pattern of a cylinder's faces");
        let all: Vec<u32> = groups.into_iter().flatten().collect();
        assert!(!all.is_empty(), "pattern produced no faces");
        collect_unmeasured(&model, &all, "pattern", &[], &mut bad);
    }

    assert!(
        bad.is_empty(),
        "these mints left a face with NO measured parametric domain, so every \
         quantity that reads the domain refuses on it: {bad:?}"
    );
}
