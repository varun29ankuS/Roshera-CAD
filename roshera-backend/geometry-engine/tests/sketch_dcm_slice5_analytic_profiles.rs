// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 — Slice 5: analytic profiles (spec §3.3, "Phase D").
//!
//! Kills the 64-gon bore at the topology-walker boundary: profile
//! extraction gains a TYPED edge list (`ProfileEdge`), and
//! `extrude_profile_regions` builds kernel edges with true circular
//! geometry, so a sketch circle extrudes to a genuine analytic
//! `Cylinder` lateral face — the same face `create_cylinder` emits,
//! inheriting every cylinder-hardened boolean/fillet/STEP path.
//!
//! Gate (spec §3.5 Slice 5, verbatim): "csketch circle-in-rectangle
//! extrude produces a solid whose bore face count and STEP output
//! match the `create_cylinder`-drilled equivalent; volume matches
//! πr²·h to analytic tolerance, not 64-gon tolerance."
//! (The STEP half of the gate lives in
//! `export-engine/tests/step_sketch_bore.rs` — geometry-engine cannot
//! depend on export-engine.)
//!
//! RED provenance: this file was written before the implementation and
//! failed to compile on `62445fc` (unresolved `ProfileEdge`,
//! `AnalyticLoop`, `analytic_loop_edges`, `extrude_profile_regions`).
//! The pre-slice BEHAVIOUR (sampled 64-gon bore, zero cylinder faces,
//! volume off by the 64-gon area deficit ≈ 1.8 mm³·mm) is pinned at
//! assert level by the mutation proofs in the slice report: routing
//! the gate profile through the sampled fallback makes
//! `gate_bore_face_count_matches_drilled_equivalent` and
//! `gate_bore_volume_matches_analytic_tolerance` fail with exactly the
//! pre-slice signature.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::primitives::curve::Arc as Arc3;
use geometry_engine::primitives::surface::{Cylinder, Plane};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::sketch2d::sketch_topology::{
    AnalyticLoop, EdgeType, ProfileEdge, ProfileExtractor, SketchTopology,
};
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use std::f64::consts::PI;

// ─── fixtures ────────────────────────────────────────────────────────

const RECT_W: f64 = 40.0;
const RECT_H: f64 = 30.0;
const BORE_R: f64 = 6.0;
const EXTRUDE_H: f64 = 10.0;

/// Rectangle (0,0)–(40,30) with a Ø12 circle at its centre — the
/// spec-gate "circle-in-rectangle" profile.
fn gate_sketch() -> Sketch {
    let sketch = Sketch::new("slice5-gate".to_string(), SketchAnchor::xy());
    sketch
        .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(RECT_W, RECT_H))
        .expect("rectangle");
    sketch
        .add_circle(Point2d::new(RECT_W / 2.0, RECT_H / 2.0), BORE_R)
        .expect("circle");
    sketch
}

const SLOT_L: f64 = 10.0; // arc centers at x = ±SLOT_L
const SLOT_R: f64 = 5.0;
const SLOT_H: f64 = 8.0;

/// Stadium/slot profile: two horizontal lines y = ±r for x ∈ [−L, L]
/// plus two semicircular end arcs of radius r centred at (±L, 0).
fn slot_sketch() -> Sketch {
    let sketch = Sketch::new("slice5-slot".to_string(), SketchAnchor::xy());
    let bl = sketch.add_point(Point2d::new(-SLOT_L, -SLOT_R));
    let br = sketch.add_point(Point2d::new(SLOT_L, -SLOT_R));
    let tr = sketch.add_point(Point2d::new(SLOT_L, SLOT_R));
    let tl = sketch.add_point(Point2d::new(-SLOT_L, SLOT_R));
    sketch.add_line(bl, br).expect("bottom line");
    sketch.add_line(tr, tl).expect("top line");
    // Right end cap: CCW from −π/2 (at (L,−r)) to +π/2 (at (L,r)).
    sketch
        .add_arc_center_angles(Point2d::new(SLOT_L, 0.0), SLOT_R, -PI / 2.0, PI / 2.0)
        .expect("right arc");
    // Left end cap: CCW from π/2 (at (−L,r)) to 3π/2 (at (−L,−r)).
    sketch
        .add_arc_center_angles(Point2d::new(-SLOT_L, 0.0), SLOT_R, PI / 2.0, 3.0 * PI / 2.0)
        .expect("left arc");
    sketch
}

/// Extract every region of `sketch` as analytic typed-edge loops,
/// panicking if any loop refuses (the fixtures here are all
/// line/arc/circle profiles, which MUST extract analytically).
fn analytic_regions(sketch: &Sketch) -> Vec<ProfileRegion> {
    let topo = SketchTopology::analyze(sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert!(!profiles.is_empty(), "fixture must produce >= 1 region");
    profiles
        .iter()
        .map(|profile| {
            let outer =
                match ProfileExtractor::analytic_loop_edges(sketch, &topo, &profile.outer_boundary)
                    .expect("outer loop extraction")
                {
                    AnalyticLoop::Edges(edges) => edges,
                    AnalyticLoop::Unsupported { entity, edge_type } => panic!(
                        "outer loop of a line/arc/circle profile refused analytic \
                     extraction: {entity} ({edge_type:?})"
                    ),
                };
            let holes = profile
                .holes
                .iter()
                .map(|hole| {
                    match ProfileExtractor::analytic_loop_edges(sketch, &topo, hole)
                        .expect("hole loop extraction")
                    {
                        AnalyticLoop::Edges(edges) => ProfileLoop::Edges(edges),
                        AnalyticLoop::Unsupported { entity, edge_type } => panic!(
                            "hole loop refused analytic extraction: {entity} ({edge_type:?})"
                        ),
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

fn as_solid(g: GeometryId) -> u32 {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected a solid, got {other:?}"),
    }
}

/// The `create_cylinder`-drilled equivalent of the gate profile: a
/// 40×30×10 box (centred at the origin) minus a through cylinder of
/// radius 6 — built purely from primitives + boolean, no sketch.
fn drilled_equivalent(model: &mut BRepModel) -> u32 {
    let box_id = as_solid(
        TopologyBuilder::new(model)
            .create_box_3d(RECT_W, RECT_H, EXTRUDE_H)
            .expect("box"),
    );
    let cyl_id = as_solid(
        TopologyBuilder::new(model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -EXTRUDE_H / 2.0 - 1.0),
                Vector3::Z,
                BORE_R,
                EXTRUDE_H + 2.0,
            )
            .expect("cylinder"),
    );
    boolean_operation(
        model,
        box_id,
        cyl_id,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box minus cylinder")
}

/// The sketch-built gate solid, positioned to coincide with
/// [`drilled_equivalent`] in world space (frame origin at the box's
/// (−W/2, −H/2, −h/2) corner).
fn sketch_bore_solid(model: &mut BRepModel) -> u32 {
    let sketch = gate_sketch();
    let regions = analytic_regions(&sketch);
    assert_eq!(regions.len(), 1, "one region: rectangle with circle hole");
    extrude_profile_regions(
        model,
        Point3::new(-RECT_W / 2.0, -RECT_H / 2.0, -EXTRUDE_H / 2.0),
        Vector3::X,
        Vector3::Y,
        &regions,
        EXTRUDE_H,
        None,
        Tolerance::default(),
    )
    .expect("analytic extrude")
}

/// All outer-shell faces of `solid` whose carrier surface is an
/// analytic `Cylinder`, as (radius, unit axis) pairs.
fn cylinder_faces(model: &BRepModel, solid: u32) -> Vec<(f64, Vector3)> {
    let solid_ref = model.solids.get(solid).expect("solid");
    let shell = model.shells.get(solid_ref.outer_shell).expect("shell");
    let mut found = Vec::new();
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
            found.push((cyl.radius, cyl.axis));
        }
    }
    found
}

fn outer_face_count(model: &BRepModel, solid: u32) -> usize {
    model
        .solid_outer_face_count(solid)
        .expect("solid must have an outer shell")
}

/// Volume oracle for the gate. Deliberately the MESH path
/// (`calculate_solid_volume`), with the choice documented rather than
/// hidden: the analytic divergence-quadrature path
/// (`primitives::mass_properties::integrate_solid`) is NOT converged
/// on hole-trimmed planar faces — measured on this very fixture at
/// +1.8e-2 relative error for the sketch solid and +6.9e-2 for the
/// `create_cylinder`-drilled reference (pre-existing; the
/// exact-mass-properties campaign is paused exactly at trimming). The
/// mesh path tessellates the TRUE analytic cylinder adaptively
/// (measured ≈ 1.7e-5 relative error here), an order of magnitude
/// inside the 64-gon signature (+1.67e-4), so it cleanly separates
/// analytic-bore from sampled-bore geometry.
fn measured_volume(model: &mut BRepModel, solid: u32) -> f64 {
    model
        .calculate_solid_volume(solid)
        .expect("solid volume must be computable")
}

// ─── typed extraction ────────────────────────────────────────────────

/// The rectangle's outer loop extracts as four exact `Line` edges
/// chained head-to-tail through the exact corners, and the circle hole
/// as a single exact `Circle` edge — the entity identity the walker
/// used to discard at the sampling boundary.
#[test]
fn typed_extraction_rectangle_circle_is_exact() {
    let sketch = gate_sketch();
    let topo = SketchTopology::analyze(&sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];

    let outer = match ProfileExtractor::analytic_loop_edges(&sketch, &topo, &profile.outer_boundary)
        .expect("outer extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("rectangle loop must extract analytically, got {other:?}"),
    };
    assert_eq!(outer.len(), 4, "rectangle = 4 line edges");
    let mut corners_seen = Vec::new();
    for (i, edge) in outer.iter().enumerate() {
        let (start, end) = match edge {
            ProfileEdge::Line { start, end } => (*start, *end),
            other => panic!("rectangle edge {i} must be a Line, got {other:?}"),
        };
        // Chained head-to-tail: this edge starts EXACTLY where the
        // previous one ended (corners are stored floats, not samples).
        let (prev_start, prev_end) = match &outer[(i + 3) % 4] {
            ProfileEdge::Line { start, end } => (*start, *end),
            other => panic!("rectangle edge must be a Line, got {other:?}"),
        };
        let _ = prev_start;
        assert_eq!(
            start, prev_end,
            "edge {i} must start bitwise at the previous edge's end"
        );
        corners_seen.push(end);
    }
    let expected = [[0.0, 0.0], [RECT_W, 0.0], [RECT_W, RECT_H], [0.0, RECT_H]];
    for c in expected {
        assert!(
            corners_seen.contains(&c),
            "corner {c:?} missing from extracted loop (got {corners_seen:?})"
        );
    }

    assert_eq!(profile.holes.len(), 1);
    let hole = match ProfileExtractor::analytic_loop_edges(&sketch, &topo, &profile.holes[0])
        .expect("hole extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("circle loop must extract analytically, got {other:?}"),
    };
    assert_eq!(hole.len(), 1, "full circle = one closed edge");
    match &hole[0] {
        ProfileEdge::Circle { center, radius } => {
            assert_eq!(*center, [RECT_W / 2.0, RECT_H / 2.0], "exact centre");
            assert_eq!(*radius, BORE_R, "exact radius — not a chord fit");
        }
        other => panic!("hole edge must be a Circle, got {other:?}"),
    }
}

/// The slot chains lines and arcs; arcs carry the exact centre /
/// radius / angles / winding of the sketch entity, and consecutive
/// edges join within strict tolerance (trig round-off only).
#[test]
fn typed_extraction_slot_chains_lines_and_arcs() {
    let sketch = slot_sketch();
    let topo = SketchTopology::analyze(&sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1);
    let edges =
        match ProfileExtractor::analytic_loop_edges(&sketch, &topo, &profiles[0].outer_boundary)
            .expect("slot extraction")
        {
            AnalyticLoop::Edges(edges) => edges,
            other => panic!("slot loop must extract analytically, got {other:?}"),
        };
    assert_eq!(edges.len(), 4, "slot = 2 lines + 2 arcs");

    let endpoint = |edge: &ProfileEdge, at_start: bool| -> [f64; 2] {
        match edge {
            ProfileEdge::Line { start, end } => {
                if at_start {
                    *start
                } else {
                    *end
                }
            }
            ProfileEdge::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                ..
            } => {
                let a = if at_start { *start_angle } else { *end_angle };
                [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
            }
            ProfileEdge::Circle { .. } => panic!("no circle in a slot loop"),
            // Slice 7 added the NURBS variant; slot fixtures are
            // line/arc-only by construction.
            ProfileEdge::Nurbs { .. } => panic!("no spline in a slot loop"),
        }
    };

    let mut lines = 0;
    let mut arcs = 0;
    for (i, edge) in edges.iter().enumerate() {
        // Chain continuity: walk-ordered, each edge starts where the
        // previous ended (within trig round-off).
        let prev = &edges[(i + edges.len() - 1) % edges.len()];
        let joint_prev = endpoint(prev, false);
        let joint_this = endpoint(edge, true);
        let gap = ((joint_this[0] - joint_prev[0]).powi(2)
            + (joint_this[1] - joint_prev[1]).powi(2))
        .sqrt();
        assert!(
            gap < 1e-9,
            "edge {i} starts {gap:.3e} away from the previous edge's end"
        );

        match edge {
            ProfileEdge::Line { .. } => lines += 1,
            ProfileEdge::Arc { center, radius, .. } => {
                arcs += 1;
                assert_eq!(*radius, SLOT_R, "exact arc radius");
                assert!(
                    (center[0].abs() - SLOT_L).abs() < 1e-12 && center[1].abs() < 1e-12,
                    "arc centre must be (±{SLOT_L}, 0), got {center:?}"
                );
            }
            ProfileEdge::Circle { .. } => panic!("no circle in a slot loop"),
            ProfileEdge::Nurbs { .. } => panic!("no spline in a slot loop"),
        }
    }
    assert_eq!((lines, arcs), (2, 2), "slot = exactly 2 lines + 2 arcs");
}

/// Spline and ellipse loops REFUSE analytic extraction with a typed
/// verdict naming the entity — the honest fallback signal the csketch
/// route uses to keep sampling them (never a sampled polygon silently
/// labeled analytic).
#[test]
fn spline_and_ellipse_loops_refuse_analytic_extraction() {
    // Closed ellipse: one single-edge loop.
    let sketch = Sketch::new("slice5-ellipse".to_string(), SketchAnchor::xy());
    sketch
        .add_ellipse(Point2d::new(0.0, 0.0), 8.0, 5.0, 0.0)
        .expect("ellipse");
    let topo = SketchTopology::analyze(&sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1);
    match ProfileExtractor::analytic_loop_edges(&sketch, &topo, &profiles[0].outer_boundary)
        .expect("extraction must not hard-error")
    {
        AnalyticLoop::Unsupported { edge_type, .. } => {
            assert_eq!(edge_type, EdgeType::Ellipse, "verdict names the ellipse")
        }
        AnalyticLoop::Edges(edges) => panic!(
            "ellipse loop must refuse analytic extraction (no exact ellipse \
             lift is wired yet), got {} edges",
            edges.len()
        ),
    }

    // Closed B-spline (first CP == last CP under clamped knots).
    // BEHAVIOUR CHANGE (deliberate, SKETCH-DCM #45 Slice 7): splines
    // now LIFT to a typed `ProfileEdge::Nurbs` instead of refusing —
    // the Slice-5 residual this test used to pin. The honesty
    // boundary moved DOWN the stack: the closed-single-edge wall trap
    // is now refused typed by `extrude_profile_regions` (pinned in
    // `sketch_dcm_slice7_generative.rs`), and the csketch route
    // pre-samples such loops with an honest `sampled_loops` count.
    let sketch = Sketch::new("slice5-spline".to_string(), SketchAnchor::xy());
    sketch
        .add_bspline(
            2,
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 0.0),
                Point2d::new(10.0, 10.0),
                Point2d::new(0.0, 10.0),
                Point2d::new(0.0, 0.0),
            ],
            vec![0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0],
        )
        .expect("closed bspline");
    let topo = SketchTopology::analyze(&sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1);
    match ProfileExtractor::analytic_loop_edges(&sketch, &topo, &profiles[0].outer_boundary)
        .expect("extraction must not hard-error")
    {
        AnalyticLoop::Edges(edges) => {
            assert_eq!(edges.len(), 1, "one closed spline edge");
            assert!(
                matches!(edges[0], ProfileEdge::Nurbs { .. }),
                "splines lift to typed NURBS edges since Slice 7: {edges:?}"
            );
        }
        AnalyticLoop::Unsupported { entity, edge_type } => panic!(
            "splines must lift analytically since Slice 7, got refusal on \
             {entity} ({edge_type:?})"
        ),
    }
}

/// The typed edges serialize to the exact wire shape the
/// `sketch_extrude` timeline event records, and round-trip losslessly
/// — this is what makes live-vs-replay byte-equivalent.
#[test]
fn profile_edge_serde_round_trip_and_wire_shape() {
    let edges = vec![
        ProfileEdge::Line {
            start: [0.0, 0.0],
            end: [40.0, 0.0],
        },
        ProfileEdge::Arc {
            center: [10.0, 0.0],
            radius: 5.0,
            start_angle: -PI / 2.0,
            end_angle: PI / 2.0,
            ccw: true,
        },
        ProfileEdge::Circle {
            center: [20.0, 15.0],
            radius: 6.0,
        },
    ];
    let value = serde_json::to_value(&edges).expect("serialize");
    let arr = value.as_array().expect("array");
    assert_eq!(arr[0]["kind"], "line");
    assert_eq!(arr[1]["kind"], "arc");
    assert_eq!(arr[1]["ccw"], true);
    assert_eq!(arr[2]["kind"], "circle");
    assert_eq!(arr[2]["radius"], 6.0);
    let back: Vec<ProfileEdge> = serde_json::from_value(value).expect("deserialize");
    assert_eq!(back, edges, "lossless round-trip");
}

// ─── the gate ────────────────────────────────────────────────────────

/// GATE 1/2 (spec §3.5 Slice 5): the sketch-extruded bore is a TRUE
/// analytic cylinder face — same bore face count, radius and axis as
/// the `create_cylinder`-drilled equivalent. Pre-slice the bore was 64
/// planar facets and this asserted 0 == 1.
#[test]
fn gate_bore_face_count_matches_drilled_equivalent() {
    let mut model = BRepModel::new();
    let sketch_solid = sketch_bore_solid(&mut model);
    let drilled = drilled_equivalent(&mut model);

    let sketch_cyls = cylinder_faces(&model, sketch_solid);
    let drilled_cyls = cylinder_faces(&model, drilled);

    assert!(
        !drilled_cyls.is_empty(),
        "drilled equivalent must carry an analytic cylinder bore face"
    );
    assert_eq!(
        sketch_cyls.len(),
        drilled_cyls.len(),
        "bore face count must match the create_cylinder-drilled \
         equivalent (pre-slice: 0 cylinder faces, 64 planar facets)"
    );
    for (radius, axis) in &sketch_cyls {
        assert!(
            (radius - BORE_R).abs() < 1e-9,
            "bore radius must be the exact sketch dimension: got {radius}, want {BORE_R}"
        );
        let axis_alignment = axis.normalize().expect("axis").dot(&Vector3::Z).abs();
        assert!(
            axis_alignment > 1.0 - 1e-9,
            "bore axis must be the extrusion direction (|axis.Z| = {axis_alignment})"
        );
    }

    // Analytic face budget: bottom cap + top cap + 4 walls + 1 bore.
    assert_eq!(
        outer_face_count(&model, sketch_solid),
        7,
        "rectangle-with-bore extrude = exactly 7 faces (was 6 + 64 facets)"
    );

    // The solid must be watertight-sound, not just typed prettily.
    let gt = model.ground_truth(sketch_solid).expect("ground truth");
    assert!(
        gt.certificate.is_sound(),
        "analytic-profile extrude must be SOUND — {}",
        gt.summary()
    );
    let mesh = tessellate_solid(
        model.solids.get(sketch_solid).expect("solid"),
        &model,
        &TessellationParams::default(),
    );
    assert!(
        !mesh.triangles.is_empty(),
        "analytic bore solid must tessellate to a non-empty mesh"
    );
}

/// GATE 2/2 (spec §3.5 Slice 5): volume matches (W·H − πr²)·h to
/// analytic-bore tolerance, not 64-gon tolerance. The 64-gon bore's
/// area deficit is r²/2·(2π − 64·sin(2π/64)) ≈ 0.1816 mm² — a volume
/// error of ≈ +1.816 mm³ (+1.67e-4 relative), and the mesh oracle is
/// EXACT for a planar-facet prism, so the pre-slice geometry measures
/// at exactly that signature. The analytic bore measures ≈ 1.7e-5
/// (adaptive tessellation of the true cylinder); the 5e-5 bound sits
/// an order of magnitude below the 64-gon signature with ≈ 3× margin
/// over the measured analytic value. See `measured_volume` for why
/// the divergence-quadrature "exact" path is not the oracle here.
#[test]
fn gate_bore_volume_matches_analytic_tolerance() {
    let mut model = BRepModel::new();
    let sketch_solid = sketch_bore_solid(&mut model);
    let drilled = drilled_equivalent(&mut model);

    let analytic = (RECT_W * RECT_H - PI * BORE_R * BORE_R) * EXTRUDE_H;
    let v_sketch = measured_volume(&mut model, sketch_solid);
    let v_drilled = measured_volume(&mut model, drilled);
    eprintln!(
        "[slice5 gate] analytic={analytic:.9} v_sketch={v_sketch:.9} v_drilled={v_drilled:.9}"
    );

    let rel = |a: f64, b: f64| (a - b).abs() / b.abs();
    assert!(
        rel(v_sketch, analytic) < 5e-5,
        "sketch bore volume must match (W·H − πr²)·h to analytic-bore tolerance: \
         got {v_sketch:.9}, analytic {analytic:.9}, rel err {:.3e} \
         (64-gon signature = +1.67e-4)",
        rel(v_sketch, analytic)
    );
    assert!(
        rel(v_drilled, analytic) < 5e-5,
        "drilled equivalent must measure the same analytic volume: \
         got {v_drilled:.9}, analytic {analytic:.9}, rel err {:.3e}",
        rel(v_drilled, analytic)
    );
    assert!(
        rel(v_sketch, v_drilled) < 1e-5,
        "sketch-built and primitive-drilled solids must agree to \
         tessellation noise: {v_sketch:.9} vs {v_drilled:.9} (rel {:.3e})",
        rel(v_sketch, v_drilled)
    );
}

// ─── partial arcs (slot) ─────────────────────────────────────────────

/// Slot extrude: the two end-cap walls are exact swept-arc surfaces,
/// the caps carry true `Arc` boundary edges, the solid is sound, and
/// the volume is the exact stadium prism (2L·2r + πr²)·h.
///
/// UPDATED (SKETCH-DCM #45 follow-ups B, item 3 — Slice-6/7 flip
/// precedent): the arc walls were exactly-swept generic
/// `RuledSurface`s with `Arc` rails (Slice-5 residual 2); they are now
/// promoted to TRUE trimmed `Cylinder` faces (exact radius + axis,
/// seam-aligned trim), so the wall assertions here moved from
/// rail-exactness on the ruled carrier to carrier-exactness on the
/// typed cylinder. Everything else the test pinned (Arc cap edges,
/// soundness, exact stadium volume) is unchanged.
#[test]
fn slot_extrude_arc_walls_are_exact_and_solid_is_sound() {
    let sketch = slot_sketch();
    let regions = analytic_regions(&sketch);
    assert_eq!(regions.len(), 1);
    let mut model = BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &regions,
        SLOT_H,
        None,
        Tolerance::default(),
    )
    .expect("slot extrude");

    // 2 line walls + 2 arc walls + 2 caps.
    assert_eq!(outer_face_count(&model, solid), 6, "slot = 6 faces");

    let solid_ref = model.solids.get(solid).expect("solid").clone();
    let shell = model.shells.get(solid_ref.outer_shell).expect("shell");
    let mut arc_walls = 0;
    let mut planar_caps_with_arc_edges = 0;
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
            // Follow-ups B item 3: the end-cap wall is a TRUE trimmed
            // Cylinder — exact radius, extrusion axis, and the arc's
            // own angular span (seam-aligned, never straddling the
            // carrier's parameterisation seam).
            assert!(
                (cyl.radius - SLOT_R).abs() < 1e-9,
                "cylinder wall radius must be exact: got {}, want {SLOT_R}",
                cyl.radius
            );
            let limits = cyl
                .angle_limits
                .expect("partial-arc wall carries its trim span");
            assert!(
                limits[0].abs() < 1e-12 && (limits[1] - PI).abs() < 1e-9,
                "semicircle span [0, π], got {limits:?}"
            );
            arc_walls += 1;
        } else if surface.as_any().downcast_ref::<Plane>().is_some() {
            // Cap faces: their boundary must carry TRUE Arc edges, not
            // chord strings. (The two straight walls are planes with 0
            // arc edges; the caps have exactly 2 each.)
            let lp = model.loops.get(face.outer_loop).expect("loop");
            let arc_edges = lp
                .edges
                .iter()
                .filter(|&&eid| {
                    let edge = model.edges.get(eid).expect("edge");
                    let curve = model.curves.get(edge.curve_id).expect("curve");
                    curve.as_any().downcast_ref::<Arc3>().is_some()
                })
                .count();
            if arc_edges == 2 {
                planar_caps_with_arc_edges += 1;
            } else {
                assert_eq!(
                    arc_edges, 0,
                    "planar face must carry 0 (wall) or 2 (cap) arc edges"
                );
            }
        } else {
            panic!("unexpected surface kind on slot extrude face {fid}");
        }
    }
    assert_eq!(arc_walls, 2, "exactly two swept-arc end-cap walls");
    assert_eq!(
        planar_caps_with_arc_edges, 2,
        "bottom and top caps must each carry 2 true Arc boundary edges"
    );

    let gt = model.ground_truth(solid).expect("ground truth");
    assert!(
        gt.certificate.is_sound(),
        "slot extrude must be SOUND — {}",
        gt.summary()
    );
    let mesh = tessellate_solid(
        model.solids.get(solid).expect("solid"),
        &model,
        &TessellationParams::default(),
    );
    assert!(!mesh.triangles.is_empty(), "slot must tessellate");

    // Mesh-oracle volume (see `measured_volume`). Follow-ups B item 3
    // moved the arc walls onto the cylinder-hardened tessellation
    // path (the Slice-5 generic-ruled measurement was 1.39e-4
    // relative; the 64-seg/turn sampled-profile signature ≈ 3.3e-4 —
    // per-semicircle 64-gon deficit 0.0454 mm² ⇒ +0.73 mm³). The
    // volume check is a guard rail; the STRUCTURAL assertions above
    // (typed trimmed Cylinder walls / true Arc cap edges) are this
    // test's primary teeth.
    let analytic = (2.0 * SLOT_L * 2.0 * SLOT_R + PI * SLOT_R * SLOT_R) * SLOT_H;
    let v = measured_volume(&mut model, solid);
    let rel = (v - analytic).abs() / analytic;
    eprintln!("[slice5 slot] analytic={analytic:.9} v={v:.9} rel={rel:.3e}");
    assert!(
        rel < 2e-4,
        "slot volume must measure inside the sampled-profile signature: got {v:.9}, \
         analytic {analytic:.9}, rel err {rel:.3e} (64-gon signature ≈ 3.3e-4)"
    );
}

// ─── honest refusal at the kernel boundary ───────────────────────────

/// An analytic full-circle loop extruded OBLIQUELY cannot become a
/// coaxial cylinder — the kernel must refuse with a typed error rather
/// than emit the known-broken closed ruled wall. (The csketch route
/// pre-checks this and falls back to the sampled polygon, so agents
/// keep oblique extrudes; this pins the kernel-level contract.)
#[test]
fn oblique_circle_extrusion_refuses_instead_of_emitting_broken_wall() {
    let sketch = gate_sketch();
    let regions = analytic_regions(&sketch);
    let mut model = BRepModel::new();
    let result = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &regions,
        EXTRUDE_H,
        Some(Vector3::new(0.3, 0.0, 1.0)),
        Tolerance::default(),
    );
    let err = match result {
        Err(e) => format!("{e:?}"),
        Ok(id) => panic!(
            "oblique analytic-circle extrusion must refuse (typed error), \
             but produced solid {id}"
        ),
    };
    assert!(
        err.contains("normal"),
        "refusal must explain the sketch-normal requirement, got: {err}"
    );
}
