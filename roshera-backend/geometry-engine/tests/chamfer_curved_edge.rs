// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **A chamfer on a CURVED open edge built its face out of a straight chord.**
//!
//! `create_ruled_chamfer_surface` took the 11 offset points
//! `compute_chamfer_offsets` samples along the edge and kept exactly two of
//! them: `Line::new(first, last)`. The face BOUNDARY, meanwhile, is built by
//! `create_offset_curve`, which threads a curve through ALL 11. On a straight
//! edge the two agree exactly. On a curved one they agree nowhere but the
//! endpoints — the boundary loop bows away from its own supporting surface by
//! the sagitta of the offset trail.
//!
//! A face whose loop does not lie on its surface is not a face. Every consumer
//! that evaluates the surface inside that loop — tessellation, area, normals,
//! ray casting, export — is reading a surface the boundary never touched.
//!
//! **The fixture** is a 90° pie slice (r = 10) extruded 20 mm: five faces, a
//! top cap bounded by `arc + chord + chord`, and a cylindrical wall. The
//! chamfer target is the top arc, where the cap meets the wall. Three
//! properties are load-bearing, each established by measuring the pre-fix
//! kernel against the alternatives:
//!
//! * **The arc is its own curve over `[0, 1]`.** `compute_chamfer_offsets`
//!   samples `t = i/10` on the EDGE'S CURVE without consulting
//!   `Edge::param_range`, so a boolean-trimmed rim arc (a sub-range of a full
//!   `Circle`) would have its offsets computed all the way round the circle —
//!   a separate, deeper defect that would mask this one. An extruded profile
//!   arc carries its own trimmed curve, so the sampling is honest here.
//! * **The arc meets its neighbours transversally** (at 90°, the radii). A
//!   fillet-derived arc joins its neighbour TANGENTIALLY, and the neighbour
//!   retrim then pushes a shared vertex off the third face's plane — again a
//!   separate pre-existing limitation.
//! * **Every loop in the neighbourhood has ≥ 3 edges**, which
//!   `splice_face_along_edge` requires ("blend surgery needs ≥3").
//!
//! Tests:
//!
//! * [`chamfer_of_open_arc_edge_face_lies_on_its_surface`] — the headline. Every
//!   sample of every boundary curve of the chamfer face must lie on that face's
//!   surface. Pre-fix worst residence error: **2.888 mm** against a 1e-6
//!   tolerance.
//! * [`chamfer_of_open_arc_edge_yields_a_sound_solid`] — the same chamfer under
//!   the production defaults (`validate_result: true`), then the kernel's own
//!   certificate, plus the Task-32 domain measurement on the minted bevel face.
//!   Pre-fix the operation refuses outright: the fitted trim edge lies 4.1e-2
//!   off the cylinder it is supposed to trim.
//! * [`box_edge_chamfer_leaves_the_straight_path_untouched`] — the control. A
//!   straight edge must come out of the fix with the same geometry it had
//!   before: Line rails, an eleven-point degree-3 boundary fit, and the
//!   analytic volume to the bit.

use std::f64::consts::FRAC_PI_2;

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode,
};
use geometry_engine::operations::{extrude_profile, CommonOptions, ExtrudeOptions};
use geometry_engine::primitives::curve::{Arc, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

const SLICE_R: f64 = 10.0;
const SLICE_H: f64 = 20.0;
const SETBACK: f64 = 1.0;

// ─── fixtures ────────────────────────────────────────────────────────────────

/// A quarter-disc (r = `SLICE_R`, 90°) profile extruded `SLICE_H` in `+Z`.
///
/// Faces: bottom cap, top cap (`arc + radius + radius`), the cylindrical wall,
/// and the two radial planes.
fn pie_slice(model: &mut BRepModel) -> SolidId {
    let centre = model.vertices.add(0.0, 0.0, 0.0);
    let start = model.vertices.add(SLICE_R, 0.0, 0.0);
    let end = model
        .vertices
        .add(SLICE_R * FRAC_PI_2.cos(), SLICE_R * FRAC_PI_2.sin(), 0.0);

    let arc = Arc::new(Point3::ORIGIN, Vector3::Z, SLICE_R, 0.0, FRAC_PI_2).expect("quarter arc");
    let arc_curve = model.curves.add(Box::new(arc));
    let e_arc = model.edges.add(Edge::new(
        0,
        start,
        end,
        arc_curve,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));

    let end_p = Point3::new(SLICE_R * FRAC_PI_2.cos(), SLICE_R * FRAC_PI_2.sin(), 0.0);
    let r1 = model.curves.add(Box::new(Line::new(end_p, Point3::ORIGIN)));
    let e_r1 = model.edges.add(Edge::new(
        0,
        end,
        centre,
        r1,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));

    let r2 = model.curves.add(Box::new(Line::new(
        Point3::ORIGIN,
        Point3::new(SLICE_R, 0.0, 0.0),
    )));
    let e_r2 = model.edges.add(Edge::new(
        0,
        centre,
        start,
        r2,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));

    extrude_profile(
        model,
        vec![e_arc, e_r1, e_r2],
        ExtrudeOptions {
            direction: Vector3::Z,
            distance: SLICE_H,
            ..Default::default()
        },
    )
    .expect("extrude the pie slice")
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("create_box_3d")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// Production defaults — `CommonOptions::default()` validates the result.
fn chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        symmetric: true,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

// ─── topology helpers ────────────────────────────────────────────────────────

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

/// Every edge referenced by any loop of any face of `solid`, deduplicated and
/// id-sorted.
fn solid_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut out: Vec<EdgeId> = Vec::new();
    for fid in face_ids(model, solid) {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let loop_ids: Vec<u32> = [face.outer_loop]
            .into_iter()
            .chain(face.inner_loops.iter().copied())
            .collect();
        for lid in loop_ids {
            if let Some(lp) = model.loops.get(lid) {
                for &eid in &lp.edges {
                    if !out.contains(&eid) {
                        out.push(eid);
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// Sample an edge's 3D curve over its own parameter range.
fn sample_edge(model: &BRepModel, edge_id: EdgeId, n: usize) -> Vec<Point3> {
    let edge = model.edges.get(edge_id).expect("edge");
    let curve = model.curves.get(edge.curve_id).expect("edge curve");
    let (t0, t1) = (edge.param_range.start, edge.param_range.end);
    (0..=n)
        .map(|i| {
            let t = t0 + (t1 - t0) * (i as f64 / n as f64);
            curve.point_at(t).expect("curve evaluates on its own range")
        })
        .collect()
}

/// Distance from `p` to the nearest point of a ruled `surface`, converged.
///
/// `Surface::closest_point` on a `RuledSurface` is a 64-sample scan with a
/// ternary refinement — fast, and accurate to ~3e-6 on a 15 mm rail, which is
/// coarser than the 1e-6 residence tolerance this file asserts. Measuring the
/// geometry with the kernel's own fast projector would therefore report the
/// PROJECTOR's error as the face's error (it reads 2.9e-6 whether the rail is
/// right or wrong). This is the independent instrument: the same closed-form
/// projection onto each ruling, over a 401-point scan plus 100 ternary
/// narrowings of the winning bracket (each cuts it by a third, so the bracket
/// closes far below the floating-point floor).
fn distance_to_surface(
    surface: &dyn geometry_engine::primitives::surface::Surface,
    p: Point3,
) -> f64 {
    let on_ruling = |u: f64| -> f64 {
        let (Ok(a), Ok(b)) = (surface.point_at(u, 0.0), surface.point_at(u, 1.0)) else {
            return f64::MAX;
        };
        let d = b - a;
        let dd = d.dot(&d);
        let v = if dd > 0.0 {
            ((p - a).dot(&d) / dd).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (p - (a + d * v)).magnitude()
    };

    const SCAN: usize = 400;
    let mut best_u = 0.0;
    let mut best = f64::MAX;
    for i in 0..=SCAN {
        let u = i as f64 / SCAN as f64;
        let d = on_ruling(u);
        if d < best {
            best = d;
            best_u = u;
        }
    }
    let step = 1.0 / SCAN as f64;
    let (mut lo, mut hi) = ((best_u - step).max(0.0), (best_u + step).min(1.0));
    for _ in 0..100 {
        let m1 = lo + (hi - lo) / 3.0;
        let m2 = hi - (hi - lo) / 3.0;
        if on_ruling(m1) < on_ruling(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    best.min(on_ruling(0.5 * (lo + hi)))
}

/// Maximum distance of an edge's own samples from the straight chord between
/// its endpoints — the witness that an edge is genuinely CURVED.
fn chord_deviation(model: &BRepModel, edge_id: EdgeId) -> f64 {
    let pts = sample_edge(model, edge_id, 32);
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let d = b - a;
    let dd = d.dot(&d);
    pts.iter()
        .map(|p| {
            let t = if dd > 0.0 {
                ((*p - a).dot(&d) / dd).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (*p - (a + d * t)).magnitude()
        })
        .fold(0.0, f64::max)
}

/// The pie slice's top arc: the one open edge with both vertices at `z = H`
/// that is not straight. Named by geometry, not by the id the extrude happened
/// to hand out.
fn top_arc(model: &BRepModel, solid: SolidId) -> EdgeId {
    let mut found: Vec<(EdgeId, f64)> = Vec::new();
    for eid in solid_edges(model, solid) {
        let Some(edge) = model.edges.get(eid) else {
            continue;
        };
        if edge.is_loop() {
            continue;
        }
        let Some(v0) = model.vertices.get(edge.start_vertex) else {
            continue;
        };
        let Some(v1) = model.vertices.get(edge.end_vertex) else {
            continue;
        };
        if (v0.position[2] - SLICE_H).abs() > 1e-9 || (v1.position[2] - SLICE_H).abs() > 1e-9 {
            continue;
        }
        let dev = chord_deviation(model, eid);
        if dev > 1.0 {
            found.push((eid, dev));
        }
    }
    assert_eq!(
        found.len(),
        1,
        "fixture must expose exactly one curved top edge; found {found:?}"
    );
    // A 90° arc of r=10 stands r·(1−cos45°) = 2.929 mm off its own chord: the
    // scale of the error a chord rail commits.
    let sagitta = SLICE_R * (1.0 - (FRAC_PI_2 / 2.0).cos());
    assert!(
        (found[0].1 - sagitta).abs() < 1e-9,
        "the top arc's sagitta must be r·(1−cos45°) = {sagitta}; measured {}",
        found[0].1
    );
    found[0].0
}

// ─── the defect ──────────────────────────────────────────────────────────────

#[test]
fn chamfer_of_open_arc_edge_face_lies_on_its_surface() {
    let mut model = BRepModel::new();
    let solid = pie_slice(&mut model);
    let arc = top_arc(&model, solid);

    // Result validation is switched OFF here on purpose: this test measures the
    // minted face itself rather than stopping at the kernel's own validator, so
    // the failure it reports is the residence error in millimetres. The sibling
    // test below keeps the production default on.
    let opts = ChamferOptions {
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..chamfer_opts(SETBACK)
    };
    let faces = chamfer_edges(&mut model, solid, vec![arc], opts)
        .expect("chamfering an open curved edge must succeed");
    assert_eq!(faces.len(), 1, "one selected edge mints one bevel face");
    let bevel = faces[0];

    let face = model.faces.get(bevel).expect("bevel face").clone();
    let surface = model
        .surfaces
        .get(face.surface_id)
        .expect("bevel surface")
        .clone_box();

    let loop_ids: Vec<u32> = [face.outer_loop]
        .into_iter()
        .chain(face.inner_loops.iter().copied())
        .collect();

    // The bevel must actually be the curved one — a fixture that silently
    // degenerated to a straight edge would satisfy the residence assertion for
    // the wrong reason.
    let boundary_curvature = loop_ids
        .iter()
        .filter_map(|&lid| model.loops.get(lid).cloned())
        .flat_map(|lp| lp.edges.clone())
        .map(|eid| chord_deviation(&model, eid))
        .fold(0.0, f64::max);
    assert!(
        boundary_curvature > 2.0,
        "precondition: the minted bevel must be bounded by a curved trim, not a \
         chord; worst boundary chord deviation {boundary_curvature}"
    );

    let tol = Tolerance::default();
    let mut worst = 0.0_f64;
    let mut worst_where = String::new();
    for lid in loop_ids {
        let lp = model.loops.get(lid).expect("bevel loop").clone();
        for &eid in &lp.edges {
            for (i, p) in sample_edge(&model, eid, 20).into_iter().enumerate() {
                let d = distance_to_surface(surface.as_ref(), p);
                if d > worst {
                    worst = d;
                    worst_where = format!("loop {lid} edge {eid} sample {i} at {p:?}");
                }
            }
        }
    }

    assert!(
        worst <= tol.distance(),
        "the chamfer face's boundary must lie ON the chamfer face's surface. \
         Worst residence error {worst} (tolerance {}) at {worst_where}. \
         A straight-chord ruled surface under a curve-fitted boundary leaves \
         the loop floating off its own face by the offset trail's sagitta.",
        tol.distance()
    );
}

#[test]
fn chamfer_of_open_arc_edge_yields_a_sound_solid() {
    let mut model = BRepModel::new();
    let solid = pie_slice(&mut model);
    let arc = top_arc(&model, solid);

    let faces = chamfer_edges(&mut model, solid, vec![arc], chamfer_opts(SETBACK))
        .expect("chamfering an open curved edge must succeed under production defaults");
    let bevel = faces[0];

    // Task 32: every minted face measures its own parametric domain. The bevel
    // is minted inside `create_chamfer_face`; rebuilding its rails must not
    // strand that measurement.
    assert!(
        model
            .faces
            .get(bevel)
            .expect("bevel face")
            .uv_bounds_are_measured(),
        "the minted bevel face must carry a MEASURED uv domain, not the \
         `Face::new` [0,1]² placeholder"
    );

    let cert = model.certify_solid(solid);
    assert!(
        cert.is_sound(),
        "chamfered pie slice must certify sound: watertight={} euler={} \
         boundary_edges={} nonmanifold={} selfint_free={} brep_valid={} errors={:?}",
        cert.watertight,
        cert.euler_characteristic,
        cert.boundary_edges,
        cert.nonmanifold_edges,
        cert.self_intersection_free,
        cert.brep_valid,
        cert.errors,
    );
}

// ─── the control ─────────────────────────────────────────────────────────────

/// A straight edge must be untouched by the curved-edge fix.
///
/// The witness is STRUCTURAL, not the volume. A prism's volume does not depend
/// on where its edges are subdivided, and 995.0 is exactly representable, so a
/// bit-compare of the volume cannot see straight-path drift — measured: raising
/// the offset sample count from 10 to 40 intervals globally (41 control points
/// in every boundary curve instead of 11, a real change to the straight path)
/// left `volume.to_bits()` at `0x408f180000000000`. The volume assertion is
/// kept for what it is — the analytic result, 10³ − (1²/2)·10 — and the
/// straight path is pinned by the geometry it actually produces:
///
/// * both rails of the bevel's `RuledSurface` are `Line`s, not interpolated
///   curves (`create_ruled_chamfer_surface`'s straight branch);
/// * the two trim curves are degree-3 NURBS over exactly 11 control points —
///   the `fit_to_points` fit over `BASE_SAMPLES + 1` samples
///   (`create_offset_curve`'s straight branch), which fails if a straight edge
///   is ever re-sampled or routed through the interpolating fit;
/// * the two cap curves are `Line`s.
#[test]
fn box_edge_chamfer_leaves_the_straight_path_untouched() {
    use geometry_engine::primitives::curve::NurbsCurve;
    use geometry_engine::primitives::surface::RuledSurface;

    const EXPECTED_BITS: u64 = 0x408f_1800_0000_0000;
    /// `BASE_SAMPLES + 1` in `compute_chamfer_offsets`: eleven sampled offset
    /// points, used directly as the boundary fit's control points.
    const STRAIGHT_RAIL_SAMPLES: usize = 11;

    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 10.0, 10.0, 10.0);
    let edge = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box must have edges");
    let faces = chamfer_edges(&mut model, solid, vec![edge], chamfer_opts(1.0))
        .expect("box edge chamfer must succeed");
    let bevel = faces[0];

    // --- the carrier surface: two Line rails, as before the fix ---
    let face = model.faces.get(bevel).expect("bevel face").clone();
    let surface = model
        .surfaces
        .get(face.surface_id)
        .expect("bevel surface")
        .clone_box();
    let ruled = surface
        .as_any()
        .downcast_ref::<RuledSurface>()
        .expect("a box-edge bevel is a RuledSurface");
    for (which, rail) in [(1u8, &ruled.curve1), (2u8, &ruled.curve2)] {
        assert!(
            rail.as_any().downcast_ref::<Line>().is_some(),
            "rail {which} of a STRAIGHT-edge chamfer must stay a Line; it is a \
             {}. The curved-rail fix must not reach collinear samples.",
            rail.type_name()
        );
    }

    // --- the boundary: two 11-point degree-3 fits and two straight caps ---
    let lp = model
        .loops
        .get(face.outer_loop)
        .expect("bevel outer loop")
        .clone();
    assert_eq!(lp.edges.len(), 4, "a box-edge bevel is a four-sided face");
    let mut nurbs_trims = 0;
    let mut line_caps = 0;
    for &eid in &lp.edges {
        let curve = model
            .curves
            .get(model.edges.get(eid).expect("bevel edge").curve_id)
            .expect("bevel edge curve");
        if let Some(n) = curve.as_any().downcast_ref::<NurbsCurve>() {
            assert_eq!(n.degree, 3, "boundary fit degree");
            assert_eq!(
                n.control_points.len(),
                STRAIGHT_RAIL_SAMPLES,
                "the straight-edge boundary must still be the fit over the \
                 eleven sampled offset points — a different count means the \
                 straight path was re-sampled or re-fitted"
            );
            nurbs_trims += 1;
        } else if curve.as_any().downcast_ref::<Line>().is_some() {
            line_caps += 1;
        } else {
            panic!("unexpected bevel boundary curve {}", curve.type_name());
        }
    }
    assert_eq!(nurbs_trims, 2, "two trim curves");
    assert_eq!(line_caps, 2, "two cap curves");

    // --- and the analytic volume, unchanged ---
    let volume = model
        .mass_properties_for(solid)
        .expect("chamfered box mass properties")
        .volume;
    assert_eq!(
        volume.to_bits(),
        EXPECTED_BITS,
        "straight-edge chamfer volume moved: got {volume} (bits {:#018x}), \
         expected bits {EXPECTED_BITS:#018x}",
        volume.to_bits()
    );
}
