// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A mirror comes back the right way out, face by face.
//!
//! The kernel carries two orientation conventions (see
//! `coedge_orientation_invariant.rs`): a loop's STORED walk runs
//! counter-clockwise about its surface's own normal, and `FaceOrientation` maps
//! that surface normal to the outward normal. An orientation-reversing
//! transform (negative determinant) turns every loop's image the other way
//! round, and — depending on the surface kind — may or may not flip the
//! surface's own normal:
//!
//! * plane, cylinder, cone, sphere and torus store their normal and transform
//!   it as a vector, so the reflected surface still points OUT of the
//!   reflected solid;
//! * a NURBS surface's normal is the cross product of its parametric
//!   derivatives, which flips with the handedness.
//!
//! Measured at BASE (`e3cae4e8`): the kernel `mirror` flipped EVERY face flag
//! and reversed every loop, so a mirrored box, cylinder, sphere, cone or torus
//! came back inside-out (signed volume −240 for a 10×6×4 box), while a bare
//! reflection through `transform_solid` left every loop wound backwards and a
//! NURBS face pointing inward. The first three tests pin the per-face
//! restore on both entry points (`mirror`, and `transform_solid` with a
//! negative-determinant matrix, what a legacy mirror event replays through)
//! for planar multi-vertex loops and face normals. The round-2 tests below
//! read the invariant through the CURVES — circle loops, partial arcs, a
//! through-bore's inner rims, quartic saddle rims — and pin downstream
//! operations on a mirrored part.

use std::collections::HashSet;

use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::operations::{
    boolean_operation, mirror, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::face::FaceOrientation;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn solid_of(id: Result<GeometryId, impl std::fmt::Debug>) -> SolidId {
    match id {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("expected a solid; got {other:?}"),
    }
}

fn block_with_boss(m: &mut BRepModel) -> SolidId {
    let block = solid_of(TopologyBuilder::new(m).create_box_3d(10.0, 10.0, 10.0));
    let boss = solid_of(TopologyBuilder::new(m).create_box_3d(4.0, 4.0, 4.0));
    transform_solid(
        m,
        boss,
        Matrix4::from_translation(&Vector3::new(6.0, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translate boss");
    boolean_operation(m, block, boss, BooleanOp::Union, BooleanOptions::default())
        .expect("block ∪ boss")
}

/// Square-to-square NURBS loft: one NURBS lateral face and two planar caps.
fn lofted_frustum(m: &mut BRepModel) -> SolidId {
    let section = |z: f64, half: f64| -> Vec<Point3> {
        vec![
            Point3::new(-half, -half, z),
            Point3::new(half, -half, z),
            Point3::new(half, half, z),
            Point3::new(-half, half, z),
        ]
    };
    nurbs_loft(
        m,
        vec![section(0.0, 5.0), section(10.0, 3.0)],
        NurbsLoftOptions::default(),
    )
    .expect("nurbs loft")
}

type Build = fn(&mut BRepModel) -> SolidId;

fn fixtures() -> Vec<(&'static str, Build)> {
    vec![
        ("box", |m| {
            solid_of(TopologyBuilder::new(m).create_box_3d(10.0, 6.0, 4.0))
        }),
        ("block with boss", block_with_boss),
        ("cylinder", |m| {
            solid_of(TopologyBuilder::new(m).create_cylinder_3d(
                Point3::ORIGIN,
                Vector3::Z,
                3.0,
                8.0,
            ))
        }),
        ("sphere", |m| {
            solid_of(TopologyBuilder::new(m).create_sphere_3d(Point3::ORIGIN, 5.0))
        }),
        ("cone frustum", |m| {
            solid_of(TopologyBuilder::new(m).create_cone_3d(
                Point3::ORIGIN,
                Vector3::Z,
                4.0,
                1.0,
                6.0,
            ))
        }),
        ("torus", |m| {
            solid_of(TopologyBuilder::new(m).create_torus_3d(Point3::ORIGIN, Vector3::Z, 6.0, 1.5))
        }),
        ("nurbs loft", lofted_frustum),
    ]
}

fn faces_of(model: &BRepModel, id: SolidId) -> Vec<u32> {
    let solid = model.solids.get(id).expect("solid");
    let mut out = Vec::new();
    for sh in solid.all_shells() {
        out.extend(model.shells.get(sh).expect("shell").faces.iter().copied());
    }
    out
}

/// Signed volume the solid's tessellation encloses.
fn signed_volume(model: &BRepModel, id: SolidId) -> f64 {
    let solid = model.solids.get(id).expect("solid");
    let mesh = tessellate_solid(&solid, model, &TessellationParams::default());
    let mut six = 0.0;
    for tri in &mesh.triangles {
        let p0 = mesh.vertices[tri[0] as usize].position.to_vec();
        let p1 = mesh.vertices[tri[1] as usize].position.to_vec();
        let p2 = mesh.vertices[tri[2] as usize].position.to_vec();
        six += p0.dot(&p1.cross(&p2));
    }
    six / 6.0
}

/// One point per face on its surface (a tessellation triangle centroid,
/// projected), keyed by face id.
fn face_points(model: &BRepModel, id: SolidId) -> Vec<(u32, Point3)> {
    let solid = model.solids.get(id).expect("solid");
    let mesh = tessellate_solid(&solid, model, &TessellationParams::default());
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (t, tri) in mesh.triangles.iter().enumerate() {
        let f = mesh.face_map[t];
        if !seen.insert(f) {
            continue;
        }
        let a = mesh.vertices[tri[0] as usize].position;
        let b = mesh.vertices[tri[1] as usize].position;
        let c = mesh.vertices[tri[2] as usize].position;
        out.push((
            f,
            Point3::new(
                (a.x + b.x + c.x) / 3.0,
                (a.y + b.y + c.y) / 3.0,
                (a.z + b.z + c.z) / 3.0,
            ),
        ));
    }
    out
}

/// The face's OUTWARD normal (surface normal × face flag) at the surface point
/// nearest `p`.
fn outward_normal(model: &BRepModel, face: u32, p: &Point3) -> Vector3 {
    let f = model.faces.get(face).expect("face");
    let surface = model.surfaces.get(f.surface_id).expect("surface");
    let (u, v) = surface
        .closest_point(p, Tolerance::default())
        .expect("closest point");
    let n = surface.normal_at(u, v).expect("normal");
    let sign = if f.orientation == FaceOrientation::Forward {
        1.0
    } else {
        -1.0
    };
    n * sign
}

/// Planar faces whose outer loop's STORED walk is NOT counter-clockwise about
/// the plane's own normal (Newell normal of the walk · surface normal ≤ 0).
/// Single-edge (circular) loops have a degenerate Newell polygon and are
/// skipped.
fn planar_loops_wound_backwards(model: &BRepModel, id: SolidId) -> Vec<u32> {
    let mut bad = Vec::new();
    for face in faces_of(model, id) {
        let f = model.faces.get(face).expect("face");
        let surface = model.surfaces.get(f.surface_id).expect("surface");
        if surface.surface_type() != SurfaceType::Plane {
            continue;
        }
        let lp = model.loops.get(f.outer_loop).expect("loop");
        let mut pts = Vec::new();
        for (i, &e) in lp.edges.iter().enumerate() {
            let edge = model.edges.get(e).expect("edge");
            let v = if lp.orientations[i] {
                edge.start_vertex
            } else {
                edge.end_vertex
            };
            let p = model.vertices.get_position(v).expect("vertex");
            pts.push(Vector3::new(p[0], p[1], p[2]));
        }
        if pts.len() < 3 {
            continue;
        }
        let mut newell = Vector3::ZERO;
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            newell = newell
                + Vector3::new(
                    (a.y - b.y) * (a.z + b.z),
                    (a.z - b.z) * (a.x + b.x),
                    (a.x - b.x) * (a.y + b.y),
                );
        }
        let p0 = Point3::new(pts[0].x, pts[0].y, pts[0].z);
        let (u, v) = surface
            .closest_point(&p0, Tolerance::default())
            .expect("closest point");
        let n = surface.normal_at(u, v).expect("normal");
        if newell.dot(&n) <= 0.0 {
            bad.push(face);
        }
    }
    bad
}

/// Apply `reflect` to a fresh copy of every fixture and check the result
/// against the untouched original, face by face.
fn check_every_fixture(
    label: &str,
    matrix: Matrix4,
    reflect: fn(&mut BRepModel, SolidId) -> Result<(), String>,
) {
    for (name, build) in fixtures() {
        let mut original = BRepModel::new();
        let oid = build(&mut original);
        let v0 = signed_volume(&original, oid);
        let sound0 = original.certify_solid(oid).is_sound();
        let points = face_points(&original, oid);

        let mut model = BRepModel::new();
        let id = build(&mut model);
        assert_eq!(id, oid, "{name}: fixture builds deterministically");
        reflect(&mut model, id).unwrap_or_else(|e| panic!("{label} {name}: {e}"));

        let v = signed_volume(&model, id);
        assert!(
            v > 0.0 && (v - v0).abs() <= 1e-3 * v0.abs(),
            "{label} {name}: signed volume {v0:.3} -> {v:.3}; the image must enclose the same POSITIVE volume"
        );

        for (face, p) in &points {
            let before = outward_normal(&original, *face, p);
            let expected = matrix.transform_normal(&before).expect("normal transform");
            let after = outward_normal(&model, *face, &matrix.transform_point(p));
            assert!(
                after.dot(&expected) > 0.5,
                "{label} {name}: face {face} points inward after the reflection (cosine {:.3})",
                after.dot(&expected)
            );
        }

        let backwards = planar_loops_wound_backwards(&model, id);
        assert!(
            backwards.is_empty(),
            "{label} {name}: planar faces {backwards:?} have loops wound clockwise about their own normal"
        );

        let cert = model.certify_solid(id);
        assert!(
            !cert.errors.iter().any(|e| e.contains("shells_outward")),
            "{label} {name}: {:?}",
            cert.errors
        );
        assert_eq!(
            cert.is_sound(),
            sound0,
            "{label} {name}: the reflection must not change soundness: {:?}",
            cert.errors
        );
    }
}

fn mirror_plane() -> (Point3, Vector3) {
    (Point3::new(20.0, 3.0, -1.0), Vector3::new(1.0, 0.5, 0.25))
}

#[test]
fn mirror_restores_every_face_the_right_way_out() {
    let (o, n) = mirror_plane();
    let matrix = Matrix4::mirror(o, n).expect("mirror matrix");
    check_every_fixture("mirror", matrix, |m, id| {
        let (o, n) = mirror_plane();
        mirror(m, vec![id], o, n, TransformOptions::default())
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    });
}

/// What a legacy mirror event (a "transform_solid" carrying the reflection)
/// replays through: `transform_solid` with a negative-determinant matrix.
#[test]
fn a_reflection_through_transform_solid_restores_every_face() {
    let (o, n) = mirror_plane();
    let matrix = Matrix4::mirror(o, n).expect("mirror matrix");
    check_every_fixture("reflection transform", matrix, |m, id| {
        let (o, n) = mirror_plane();
        let matrix = Matrix4::mirror(o, n).expect("mirror matrix");
        transform_solid(m, id, matrix, TransformOptions::default())
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    });
}

fn improper_motion() -> Matrix4 {
    // A reflection composed with a rotation and a translation: determinant −1,
    // not itself a mirror about any plane.
    let axis = Vector3::new(1.0, 2.0, 3.0).normalize_or_zero();
    let rot = Matrix4::from_axis_angle(&axis, 0.7).expect("axis-angle");
    let refl = Matrix4::mirror(Point3::ORIGIN, Vector3::Z).expect("mirror matrix");
    Matrix4::from_translation(&Vector3::new(40.0, -15.0, 7.5)) * rot * refl
}

#[test]
fn an_improper_rigid_motion_restores_every_face() {
    check_every_fixture("improper motion", improper_motion(), |m, id| {
        transform_solid(m, id, improper_motion(), TransformOptions::default())
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    });
}

// ---------------------------------------------------------------------------
// Round 2: the invariant read THROUGH THE CURVES, not the vertices.
//
// The checks above build Newell polygons from loop vertices, so a loop with
// fewer than three vertices (every full circle) and every arc's curvature were
// invisible to them. Measured at the round-1 tree: `Arc`/`Circle::transform`
// kept `L·normal`, so a reflected circle traced `L·p(−θ)` — every circle loop
// came out wound backwards after the loop restore, and every partial arc
// traced the wrong side of its own vertices.
// ---------------------------------------------------------------------------

use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::sketch2d::sketch_topology::{AnalyticLoop, ProfileExtractor, SketchTopology};
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};

/// Samples per edge when walking a loop through its curves.
const WALK_SAMPLES: usize = 16;

/// Every loop of `id` as `(face, loop, sense)`: the sign of the Newell normal
/// of the loop WALKED THROUGH ITS CURVES (each edge sampled in its use-sense
/// via `Edge::evaluate`), dotted with the face surface's own normal. The walk
/// convention is read from the solid itself, so outer and inner loops are
/// both covered without assuming which way inner loops run.
fn loop_senses(model: &BRepModel, id: SolidId) -> Vec<(u32, u32, f64)> {
    let mut out = Vec::new();
    for face_id in faces_of(model, id) {
        let face = model.faces.get(face_id).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        // The Newell normal measures a loop's turning sense only where the
        // loop bounds a flat region; a lateral band's loop (circle, seam,
        // circle, seam) projects to zero area. Planar faces carry every
        // circle cap, bore rim and slot-end curve this check is about.
        if surface.surface_type() != SurfaceType::Plane {
            continue;
        }
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            let lp = model.loops.get(lid).expect("loop");
            let mut pts: Vec<Vector3> = Vec::new();
            for (i, &eid) in lp.edges.iter().enumerate() {
                let edge = model.edges.get(eid).expect("edge");
                for k in 0..WALK_SAMPLES {
                    let s = k as f64 / WALK_SAMPLES as f64;
                    let t = if lp.orientations[i] { s } else { 1.0 - s };
                    let p = edge.evaluate(t, &model.curves).expect("edge evaluates");
                    pts.push(p.to_vec());
                }
            }
            let mut newell = Vector3::ZERO;
            for i in 0..pts.len() {
                let a = pts[i];
                let b = pts[(i + 1) % pts.len()];
                newell = newell
                    + Vector3::new(
                        (a.y - b.y) * (a.z + b.z),
                        (a.z - b.z) * (a.x + b.x),
                        (a.x - b.x) * (a.y + b.y),
                    );
            }
            let p0 = Point3::new(pts[0].x, pts[0].y, pts[0].z);
            let (u, v) = surface
                .closest_point(&p0, Tolerance::default())
                .expect("closest point");
            let n = surface.normal_at(u, v).expect("normal");
            out.push((face_id, lid, newell.dot(&n)));
        }
    }
    out
}

/// Every edge's curve, evaluated at its own ends, lands on its own vertices.
fn edges_off_their_vertices(model: &BRepModel, id: SolidId) -> Vec<(u32, f64)> {
    let mut bad = Vec::new();
    let mut seen = HashSet::new();
    for face_id in faces_of(model, id) {
        let face = model.faces.get(face_id).expect("face");
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            for &eid in &model.loops.get(lid).expect("loop").edges {
                if !seen.insert(eid) {
                    continue;
                }
                let edge = model.edges.get(eid).expect("edge");
                let vs = model.vertices.get_position(edge.start_vertex).expect("v");
                let ve = model.vertices.get_position(edge.end_vertex).expect("v");
                let ps = edge.evaluate(0.0, &model.curves).expect("start");
                let pe = edge.evaluate(1.0, &model.curves).expect("end");
                let ds = ps.distance(&Point3::new(vs[0], vs[1], vs[2]));
                let de = pe.distance(&Point3::new(ve[0], ve[1], ve[2]));
                let worst = ds.max(de);
                if worst > 1e-9 {
                    bad.push((eid, worst));
                }
            }
        }
    }
    bad
}

/// The loop senses of the reflected solid match the original's, loop by loop
/// (face and loop ids are stable through a transform).
fn assert_loop_senses_carried(label: &str, before: &BRepModel, after: &BRepModel, id: SolidId) {
    let b = loop_senses(before, id);
    let a = loop_senses(after, id);
    assert_eq!(a.len(), b.len(), "{label}: loop count changed");
    let flipped: Vec<(u32, u32, f64, f64)> = b
        .iter()
        .zip(a.iter())
        .filter(|((_, _, sb), (_, _, sa))| sb.signum() != sa.signum() || sa.abs() < 1e-12)
        .map(|((f, l, sb), (_, _, sa))| (*f, *l, *sb, *sa))
        .collect();
    assert!(
        flipped.is_empty(),
        "{label}: loops walked (through their curves) the other way round about their surface normal after the reflection: (face, loop, before, after) {flipped:?}"
    );
}

fn slot_solid(model: &mut BRepModel) -> SolidId {
    let sketch = Sketch::new("mirror_slot".to_string(), SketchAnchor::xy());
    let (l, r) = (10.0, 5.0);
    let bl = sketch.add_point(Point2d::new(-l, -r));
    let br = sketch.add_point(Point2d::new(l, -r));
    let tr = sketch.add_point(Point2d::new(l, r));
    let tl = sketch.add_point(Point2d::new(-l, r));
    sketch.add_line(bl, br).expect("bottom line");
    sketch.add_line(tr, tl).expect("top line");
    sketch
        .add_arc_center_angles(Point2d::new(l, 0.0), r, -PI_2, PI_2)
        .expect("right arc");
    sketch
        .add_arc_center_angles(Point2d::new(-l, 0.0), r, PI_2, 3.0 * PI_2)
        .expect("left arc");
    let topo = SketchTopology::analyze(&sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    let outer =
        match ProfileExtractor::analytic_loop_edges(&sketch, &topo, &profiles[0].outer_boundary)
            .expect("extraction")
        {
            AnalyticLoop::Edges(edges) => edges,
            other => panic!("slot loop lifts analytically: {other:?}"),
        };
    extrude_profile_regions(
        model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(outer),
            holes: Vec::new(),
        }],
        8.0,
        None,
        Tolerance::default(),
    )
    .expect("slot extrude")
}

const PI_2: f64 = std::f64::consts::FRAC_PI_2;

/// A 40×30×10 block with a vertical r = 4 through-bore at (bx, by).
fn bored_block_at(model: &mut BRepModel, bx: f64, by: f64) -> SolidId {
    let block = solid_of(TopologyBuilder::new(model).create_box_3d(40.0, 30.0, 10.0));
    let bore = solid_of(TopologyBuilder::new(model).create_cylinder_3d(
        Point3::new(bx, by, -10.0),
        Vector3::Z,
        4.0,
        30.0,
    ));
    boolean_operation(
        model,
        block,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-bore")
}

/// A 30³ block with an r = 8 bore along Z crossed by an r = 5 bore along X:
/// the saddle rims are analytic quartic (QSIC) curves.
fn cross_drilled_block(model: &mut BRepModel) -> SolidId {
    let block = solid_of(TopologyBuilder::new(model).create_box_3d(30.0, 30.0, 30.0));
    let z_bore = solid_of(TopologyBuilder::new(model).create_cylinder_3d(
        Point3::new(0.0, 0.0, -20.0),
        Vector3::Z,
        8.0,
        40.0,
    ));
    let bored = boolean_operation(
        model,
        block,
        z_bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("z bore");
    let x_bore = solid_of(TopologyBuilder::new(model).create_cylinder_3d(
        Point3::new(-20.0, 0.0, 0.0),
        Vector3::X,
        5.0,
        40.0,
    ));
    boolean_operation(
        model,
        bored,
        x_bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("x bore")
}

fn bored_block(model: &mut BRepModel) -> SolidId {
    bored_block_at(model, 6.0, 3.0)
}

type Reflect = fn(&mut BRepModel, SolidId) -> Result<(), String>;

fn reflections() -> Vec<(&'static str, Reflect)> {
    vec![
        ("mirror z=20", |m, id| {
            mirror(
                m,
                vec![id],
                Point3::new(0.0, 0.0, 20.0),
                Vector3::Z,
                TransformOptions::default(),
            )
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
        }),
        ("mirror x=5", |m, id| {
            mirror(
                m,
                vec![id],
                Point3::new(5.0, 0.0, 0.0),
                Vector3::X,
                TransformOptions::default(),
            )
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
        }),
        ("mirror oblique", |m, id| {
            let (o, n) = mirror_plane();
            mirror(m, vec![id], o, n, TransformOptions::default())
                .map(|_| ())
                .map_err(|e| format!("{e:?}"))
        }),
        ("reflection transform_solid", |m, id| {
            let (o, n) = mirror_plane();
            let matrix = Matrix4::mirror(o, n).expect("mirror matrix");
            transform_solid(m, id, matrix, TransformOptions::default())
                .map(|_| ())
                .map_err(|e| format!("{e:?}"))
        }),
    ]
}

/// Probe 1 (+ the reviewer's bore fixture): circle loops — cylinder caps and
/// a through-bore's inner rims — keep their walk sense about the surface
/// normal through every reflection, read through the curves.
#[test]
fn circle_loops_keep_their_winding_through_a_reflection() {
    let circle_fixtures: Vec<(&str, Build)> = vec![
        ("cylinder", |m| {
            solid_of(TopologyBuilder::new(m).create_cylinder_3d(
                Point3::ORIGIN,
                Vector3::Z,
                3.0,
                8.0,
            ))
        }),
        ("block with a through-bore", bored_block),
        ("cross-drilled block", cross_drilled_block),
    ];
    for (name, build) in circle_fixtures {
        for (how, reflect) in reflections() {
            let label = format!("{how} {name}");
            let mut before = BRepModel::new();
            let bid = build(&mut before);
            let mut after = BRepModel::new();
            let id = build(&mut after);
            assert_eq!(id, bid, "{label}: deterministic build");
            reflect(&mut after, id).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_loop_senses_carried(&label, &before, &after, id);
            let off = edges_off_their_vertices(&after, id);
            assert!(off.is_empty(), "{label}: edges off their vertices {off:?}");
            assert!(
                after.certify_solid(id).is_sound(),
                "{label}: {:?}",
                after.certify_solid(id).errors
            );
        }
    }
}

/// Probe 2: an extruded slot (two lines + two semicircular arcs) mirrors
/// without refusal, every edge's curve still ends on its own vertices, and
/// every loop keeps its walk sense.
#[test]
fn a_mirrored_slot_keeps_its_arcs_on_their_vertices() {
    for (how, reflect) in reflections() {
        let label = format!("{how} slot");
        let mut before = BRepModel::new();
        let bid = slot_solid(&mut before);
        assert!(
            edges_off_their_vertices(&before, bid).is_empty(),
            "fixture: the unmirrored slot is consistent"
        );
        let mut after = BRepModel::new();
        let id = slot_solid(&mut after);
        reflect(&mut after, id).unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        let off = edges_off_their_vertices(&after, id);
        assert!(
            off.is_empty(),
            "{label}: edges whose curve no longer ends at their vertices: {off:?}"
        );
        assert_loop_senses_carried(&label, &before, &after, id);
        let cert = after.certify_solid(id);
        assert!(cert.is_sound(), "{label}: {:?}", cert.errors);
        let v0 = signed_volume(&before, bid);
        let v1 = signed_volume(&after, id);
        assert!(
            v1 > 0.0 && (v1 - v0).abs() <= 1e-3 * v0,
            "{label}: volume {v0} -> {v1}"
        );
    }
}

/// The top rim of the bore at x = bx: the edges of the solid whose points
/// sit at the block's top face (z = 5) on the r = 4 circle around (bx, 3). A
/// boolean may leave the rim split into several arcs.
fn top_bore_rim(model: &BRepModel, id: SolidId, bx: f64) -> Vec<u32> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for face_id in faces_of(model, id) {
        let face = model.faces.get(face_id).expect("face");
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            for &eid in &model.loops.get(lid).expect("loop").edges {
                if !seen.insert(eid) {
                    continue;
                }
                let edge = model.edges.get(eid).expect("edge");
                let pts: Vec<Point3> = (0..8)
                    .map(|k| edge.evaluate(k as f64 / 8.0, &model.curves).expect("eval"))
                    .collect();
                let on_top = pts.iter().all(|p| (p.z - 5.0).abs() < 1e-6);
                let on_rim = pts.iter().all(|p| {
                    let r = ((p.x - bx).powi(2) + (p.y - 3.0).powi(2)).sqrt();
                    (r - 4.0).abs() < 1e-6
                });
                if on_top && on_rim {
                    found.push(eid);
                }
            }
        }
    }
    assert!(!found.is_empty(), "a top rim at x = {bx}");
    found
}

fn rim_fillet(model: &mut BRepModel, id: SolidId, bx: f64) {
    let rim = top_bore_rim(model, id, bx);
    fillet_edges(
        model,
        id,
        rim,
        FilletOptions {
            fillet_type: FilletType::Constant(1.0),
            radius: 1.0,
            ..FilletOptions::default()
        },
    )
    .expect("fillet the top bore rim");
}

/// Probe 3: downstream operations on a mirrored part agree with the same
/// operations mirrored afterwards. Mirror about x = 5 maps the bore at x = 6
/// to x = 4; the top face stays at z = 5.
#[test]
fn mirror_then_operate_equals_operate_then_mirror() {
    let plane = (Point3::new(5.0, 0.0, 0.0), Vector3::X);

    // A: mirror the plain block, then bore at the mirrored place and fillet.
    let mut a = BRepModel::new();
    let block = solid_of(TopologyBuilder::new(&mut a).create_box_3d(40.0, 30.0, 10.0));
    mirror(
        &mut a,
        vec![block],
        plane.0,
        plane.1,
        TransformOptions::default(),
    )
    .expect("mirror the block");
    let bore = solid_of(TopologyBuilder::new(&mut a).create_cylinder_3d(
        Point3::new(4.0, 3.0, -10.0),
        Vector3::Z,
        4.0,
        30.0,
    ));
    let a_id = boolean_operation(
        &mut a,
        block,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore the mirrored block");
    rim_fillet(&mut a, a_id, 4.0);

    // B: bore and fillet the plain block, then mirror.
    let mut b = BRepModel::new();
    let b_id = bored_block_at(&mut b, 6.0, 3.0);
    rim_fillet(&mut b, b_id, 6.0);
    mirror(
        &mut b,
        vec![b_id],
        plane.0,
        plane.1,
        TransformOptions::default(),
    )
    .expect("mirror the bored, filleted block");

    let ca = a.certify_solid(a_id);
    let cb = b.certify_solid(b_id);
    assert!(ca.is_sound(), "mirror-then-operate: {:?}", ca.errors);
    assert!(cb.is_sound(), "operate-then-mirror: {:?}", cb.errors);
    let va = signed_volume(&a, a_id);
    let vb = signed_volume(&b, b_id);
    assert!(
        va > 0.0 && vb > 0.0 && (va - vb).abs() <= 1e-3 * va,
        "mirror-then-operate V = {va}, operate-then-mirror V = {vb}"
    );
    assert!(
        edges_off_their_vertices(&b, b_id).is_empty(),
        "operate-then-mirror: {:?}",
        edges_off_their_vertices(&b, b_id)
    );
}

// ---------------------------------------------------------------------------
// Round 3: after a reflection, the parameters `closest_point` reports on a
// mirrored analytic face lie INSIDE the face's stored window.
//
// Measured at the round-2 tree: Cylinder, Cone, Sphere and Torus stored their
// reflected u-window as `[−b, −a]` (a full turn `[0, 2π]` became `[−2π, 0]`),
// while every one of their `closest_point`s except a trimmed Cylinder's
// reports u in `[0, 2π)`. Consumers that compare the two — the line–patch
// limits in `operations::intersect`, `Face::contains_uv_point` behind
// `operations::project` — rejected points on the mirrored face.
// ---------------------------------------------------------------------------

use geometry_engine::operations::extrude::{extrude_face, ExtrudeOptions};
use geometry_engine::operations::intersect::{intersect_curve_surface, IntersectionResult};
use geometry_engine::operations::project::{project_point_on_face, ProjectionOptions};
use geometry_engine::operations::revolve::revolve_profile_regions;

/// Faces of `id` on Cylinder / Cone / Sphere / Torus whose boundary samples'
/// `closest_point` u falls outside the stored window (the surface's u-trim,
/// and the face's measured `uv_bounds`).
fn faces_with_u_outside_window(model: &BRepModel, id: SolidId) -> Vec<(u32, f64, (f64, f64))> {
    let mut bad = Vec::new();
    for face_id in faces_of(model, id) {
        let face = model.faces.get(face_id).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        if !matches!(
            surface.surface_type(),
            SurfaceType::Cylinder | SurfaceType::Cone | SurfaceType::Sphere | SurfaceType::Torus
        ) {
            continue;
        }
        // A full-period window contains every angle; only partial windows
        // are compared number for number (as the consumers do).
        let full = |lo: f64, hi: f64| hi - lo >= std::f64::consts::TAU - 1e-6;
        let ((a, b), _) = surface.parameter_bounds();
        let (a, b) = if full(a, b) {
            (f64::NEG_INFINITY, f64::INFINITY)
        } else {
            (a, b)
        };
        let (lo, hi) = match face.measured_uv_bounds() {
            Some([u0, u1, _, _]) if !full(u0, u1) => (a.max(u0), b.min(u1)),
            _ => (a, b),
        };
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            for &eid in &model.loops.get(lid).expect("loop").edges {
                let edge = model.edges.get(eid).expect("edge");
                for k in [0.25, 0.5, 0.75] {
                    let p = edge.evaluate(k, &model.curves).expect("edge evaluates");
                    let (u, _) = surface
                        .closest_point(&p, Tolerance::default())
                        .expect("closest point");
                    if u < lo - 1e-6 || u > hi + 1e-6 {
                        bad.push((face_id, u, (lo, hi)));
                    }
                }
            }
        }
    }
    bad
}

/// A solid hemisphere dome of radius 5, revolved from a quarter-disc sketch
/// about the sketch's y-axis: its curved face is a Sphere with
/// `param_limits = [0, 2π, lo, hi]` (the `revolve.rs` sphere-zone path).
fn revolved_dome(model: &mut BRepModel) -> SolidId {
    let s = Sketch::new("dome".to_string(), SketchAnchor::xy());
    let o = s.add_point(Point2d::new(0.0, 0.0));
    let r = s.add_point(Point2d::new(5.0, 0.0));
    let top = s.add_point(Point2d::new(0.0, 5.0));
    s.add_line(o, r).expect("base");
    s.add_line(top, o).expect("axis");
    s.add_arc_center_angles(Point2d::new(0.0, 0.0), 5.0, 0.0, PI_2)
        .expect("quarter arc");
    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    let outer = match ProfileExtractor::analytic_loop_edges(&s, &topo, &profiles[0].outer_boundary)
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("dome lifts analytically: {other:?}"),
    };
    revolve_profile_regions(
        model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(outer),
            holes: Vec::new(),
        }],
        [0.0, 0.0],
        [0.0, 1.0],
        std::f64::consts::TAU,
        48,
        Tolerance::default(),
    )
    .expect("dome revolve")
}

/// A 3³ box centred on `c`.
fn cutter_at(model: &mut BRepModel, c: Point3) -> SolidId {
    let b = solid_of(TopologyBuilder::new(model).create_box_3d(3.0, 3.0, 3.0));
    transform_solid(
        model,
        b,
        Matrix4::from_translation(&Vector3::new(c.x, c.y, c.z)),
        TransformOptions::default(),
    )
    .expect("place cutter");
    b
}

/// A vertical 0.2 × 20 × 0.2 rod centred on `(x, 5, z)`; returns one of its
/// long edges (parallel to y), which crosses a dome's spherical face.
fn rod_edge(model: &mut BRepModel, x: f64, z: f64) -> u32 {
    let rod = solid_of(TopologyBuilder::new(model).create_box_3d(0.2, 20.0, 0.2));
    transform_solid(
        model,
        rod,
        Matrix4::from_translation(&Vector3::new(x, 5.0, z)),
        TransformOptions::default(),
    )
    .expect("place the rod");
    let mut long = None;
    for face_id in faces_of(model, rod) {
        let face = model.faces.get(face_id).expect("face");
        for &eid in &model.loops.get(face.outer_loop).expect("loop").edges {
            let e = model.edges.get(eid).expect("edge");
            let a = e.evaluate(0.0, &model.curves).expect("start");
            let b = e.evaluate(1.0, &model.curves).expect("end");
            if (a.y - b.y).abs() > 19.0 {
                long = Some(eid);
            }
        }
    }
    long.expect("the rod has a long edge")
}

fn sphere_face(model: &BRepModel, id: SolidId) -> u32 {
    faces_of(model, id)
        .into_iter()
        .find(|&f| {
            let face = model.faces.get(f).expect("face");
            model
                .surfaces
                .get(face.surface_id)
                .expect("surface")
                .surface_type()
                == SurfaceType::Sphere
        })
        .expect("fixture: the dome carries a Sphere face")
}

fn crossing_count(model: &BRepModel, edge: u32, face: u32) -> usize {
    match intersect_curve_surface(model, edge, face, Tolerance::default())
        .expect("edge–face intersection runs")
    {
        IntersectionResult::Points(p) => p.len(),
        _ => 0,
    }
}

/// Probe R3-1: a mirrored revolved dome (a Sphere face with
/// `param_limits = [0, 2π, lo, hi]`) keeps closest-point u inside every
/// analytic face's stored window, an edge crossing its spherical face still
/// intersects it (`operations::intersect` compares the closest-point u with
/// the patch limits), and a box cut across it behaves exactly as the same cut
/// on the unmirrored dome.
#[test]
fn a_mirrored_dome_keeps_its_patch_window_and_its_crossings() {
    let plane = (Point3::new(1.0, 0.0, 0.0), Vector3::X);
    let mirror_m = Matrix4::mirror(plane.0, plane.1).expect("mirror matrix");
    let (rx, rz) = (2.0, 1.0);

    // Before the mirror: the rod crosses the dome once.
    let mut before = BRepModel::new();
    let bdome = revolved_dome(&mut before);
    let brod = rod_edge(&mut before, rx, rz);
    let n0 = crossing_count(&before, brod, sphere_face(&before, bdome));
    assert!(
        n0 >= 1,
        "fixture: the rod crosses the dome's spherical face"
    );

    // After: the rod at the image place crosses the mirrored dome as often.
    let mut after = BRepModel::new();
    let dome = revolved_dome(&mut after);
    mirror(
        &mut after,
        vec![dome],
        plane.0,
        plane.1,
        TransformOptions::default(),
    )
    .expect("mirror the dome");
    let outside = faces_with_u_outside_window(&after, dome);
    assert!(
        outside.is_empty(),
        "mirrored dome: closest-point u outside the stored window (face, u, window): {outside:?}"
    );
    let image = mirror_m.transform_point(&Point3::new(rx, 0.0, rz));
    let rod = rod_edge(&mut after, image.x, image.z);
    let n1 = crossing_count(&after, rod, sphere_face(&after, dome));
    assert_eq!(
        n1, n0,
        "the mirrored dome must be crossed exactly as the original ({n0}); a patch window outside closest_point's range drops the hits"
    );

    // A box cut across the spherical face: the mirrored dome answers exactly
    // as the unmirrored one (this box/dome cut is a pre-existing capability
    // gap — the plain result is not sound — so parity is what is pinned).
    let cut_at = Point3::new(0.0, 4.5, 0.0);
    let mut plain = BRepModel::new();
    let pd = revolved_dome(&mut plain);
    let pc = cutter_at(&mut plain, cut_at);
    let p_res = boolean_operation(
        &mut plain,
        pd,
        pc,
        BooleanOp::Difference,
        BooleanOptions::default(),
    );
    let mut mirrored = BRepModel::new();
    let md = revolved_dome(&mut mirrored);
    mirror(
        &mut mirrored,
        vec![md],
        plane.0,
        plane.1,
        TransformOptions::default(),
    )
    .expect("mirror the dome");
    let mc = cutter_at(&mut mirrored, mirror_m.transform_point(&cut_at));
    let m_res = boolean_operation(
        &mut mirrored,
        md,
        mc,
        BooleanOp::Difference,
        BooleanOptions::default(),
    );
    match (p_res, m_res) {
        (Ok(p), Ok(m)) => {
            assert_eq!(
                plain.certify_solid(p).is_sound(),
                mirrored.certify_solid(m).is_sound(),
                "the mirrored cut must certify as the plain cut does"
            );
        }
        (Err(pe), Err(me)) => assert_eq!(
            std::mem::discriminant(&pe),
            std::mem::discriminant(&me),
            "plain: {pe:?}, mirrored: {me:?}"
        ),
        (p, m) => {
            panic!("the mirrored cut must answer as the plain cut: plain {p:?}, mirrored {m:?}")
        }
    }
}

/// Probe R3-1b: the same window check on every analytic fixture and every
/// reflection (cylinder, cone, sphere, torus, bored block, slot).
#[test]
fn closest_point_u_stays_inside_every_mirrored_window() {
    let mut all: Vec<(&str, Build)> = fixtures()
        .into_iter()
        .filter(|(n, _)| *n != "nurbs loft")
        .collect();
    all.push(("block with a through-bore", bored_block));
    all.push(("slot", slot_solid));
    for (name, build) in all {
        // A face whose window already disagreed with closest_point before the
        // mirror (measured: the cone primitive's lateral stores a full-period
        // `uv_bounds` of [−3π/2, π/2]) is not this reflection's to report.
        let mut plain = BRepModel::new();
        let pid = build(&mut plain);
        let already: HashSet<u32> = faces_with_u_outside_window(&plain, pid)
            .into_iter()
            .map(|(f, _, _)| f)
            .collect();
        for (how, reflect) in reflections() {
            let mut m = BRepModel::new();
            let id = build(&mut m);
            reflect(&mut m, id).unwrap_or_else(|e| panic!("{how} {name}: {e}"));
            let outside: Vec<_> = faces_with_u_outside_window(&m, id)
                .into_iter()
                .filter(|(f, _, _)| !already.contains(f))
                .collect();
            assert!(outside.is_empty(), "{how} {name}: {outside:?}");
        }
    }
}

/// Probe R3-2: projecting onto a mirrored cylinder's lateral face through
/// `operations::project` (whose bounds test is `Face::contains_uv_point`,
/// comparing the projection's u with the face's measured `uv_bounds`) lands
/// inside the face, as it does before the mirror.
#[test]
fn projection_onto_a_mirrored_cylinder_lateral_lands_inside() {
    let build = |m: &mut BRepModel| {
        solid_of(TopologyBuilder::new(m).create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 3.0, 8.0))
    };
    let lateral = |m: &BRepModel, id: SolidId| {
        faces_of(m, id)
            .into_iter()
            .find(|&f| {
                let face = m.faces.get(f).expect("face");
                m.surfaces
                    .get(face.surface_id)
                    .expect("surface")
                    .surface_type()
                    == SurfaceType::Cylinder
            })
            .expect("a lateral face")
    };
    let plane = (Point3::new(5.0, 0.0, 0.0), Vector3::X);
    let mirror_m = Matrix4::mirror(plane.0, plane.1).expect("mirror matrix");
    // Points just outside the wall at several angles and heights.
    let probes: Vec<Point3> = [0.4_f64, 1.3, 2.2, 3.5, 4.9, 5.8]
        .iter()
        .map(|&t| Point3::new(3.5 * t.cos(), 3.5 * t.sin(), 1.0 + t))
        .collect();

    let mut before = BRepModel::new();
    let bid = build(&mut before);
    let bf = lateral(&before, bid);
    for p in &probes {
        project_point_on_face(&before, *p, bf, &ProjectionOptions::default()).unwrap_or_else(|e| {
            panic!("fixture: projects onto the lateral before the mirror: {e:?}")
        });
    }

    let mut after = BRepModel::new();
    let id = build(&mut after);
    mirror(
        &mut after,
        vec![id],
        plane.0,
        plane.1,
        TransformOptions::default(),
    )
    .expect("mirror");
    let face = lateral(&after, id);
    for p in &probes {
        let image = mirror_m.transform_point(p);
        let projected = project_point_on_face(&after, image, face, &ProjectionOptions::default())
            .unwrap_or_else(|e| {
                panic!("the image {image:?} must project inside the mirrored lateral: {e:?}")
            });
        assert!(
            (projected.distance - 0.5).abs() < 1e-6,
            "projection distance {} (expected 0.5)",
            projected.distance
        );
    }
}

/// Probe R3-4: a mirrored slot's top cap — a planar face bounded by arcs whose
/// stored normal now opposes the plane's — extrudes, and the result is sound.
#[test]
fn a_mirrored_arc_bounded_face_extrudes() {
    for (how, reflect) in reflections() {
        let mut m = BRepModel::new();
        let id = slot_solid(&mut m);
        reflect(&mut m, id).unwrap_or_else(|e| panic!("{how}: {e}"));
        // The cap whose outward normal is the image of +Z.
        let (o, n) = match how {
            "mirror z=20" => (Point3::new(0.0, 0.0, 20.0), Vector3::Z),
            "mirror x=5" => (Point3::new(5.0, 0.0, 0.0), Vector3::X),
            _ => mirror_plane(),
        };
        let reflect_m = Matrix4::mirror(o, n).expect("mirror matrix");
        let up = reflect_m.transform_vector(&Vector3::Z);
        let cap = faces_of(&m, id)
            .into_iter()
            .find(|&f| {
                let face = m.faces.get(f).expect("face");
                let s = m.surfaces.get(face.surface_id).expect("surface");
                if s.surface_type() != SurfaceType::Plane {
                    return false;
                }
                let lp = m.loops.get(face.outer_loop).expect("loop");
                let has_arc = lp.edges.iter().any(|&e| {
                    let edge = m.edges.get(e).expect("edge");
                    m.curves.get(edge.curve_id).expect("curve").type_name() == "Arc"
                });
                let n = s.normal_at(0.0, 0.0).expect("normal")
                    * if face.orientation == FaceOrientation::Forward {
                        1.0
                    } else {
                        -1.0
                    };
                has_arc && n.dot(&up) > 0.99
            })
            .unwrap_or_else(|| panic!("{how}: the mirrored top cap"));
        let new_solid = extrude_face(
            &mut m,
            cap,
            ExtrudeOptions {
                direction: up,
                distance: 3.0,
                ..ExtrudeOptions::default()
            },
        )
        .unwrap_or_else(|e| panic!("{how}: extruding the mirrored arc-bounded cap: {e:?}"));
        let cert = m.certify_solid(new_solid);
        assert!(cert.is_sound(), "{how}: {:?}", cert.errors);
    }
}

/// A solid r = 5 cylinder with a spherical dent: minus an r = 3 sphere centred
/// on its lateral surface. The dent's rim is an analytic cylinder–sphere
/// intersection (QSIC, SphereBite chart).
fn dented_cylinder(model: &mut BRepModel) -> SolidId {
    let cyl = solid_of(TopologyBuilder::new(model).create_cylinder_3d(
        Point3::ORIGIN,
        Vector3::Z,
        5.0,
        20.0,
    ));
    let ball =
        solid_of(TopologyBuilder::new(model).create_sphere_3d(Point3::new(5.0, 0.0, 10.0), 3.0));
    boolean_operation(
        model,
        cyl,
        ball,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("dent the cylinder")
}

/// Probe R3-5: a part whose rim is a SphereBite QSIC mirrors through the
/// guard (which samples every curve against the image) and keeps its
/// soundness verdict and volume.
///
/// Measured on the BASE kernel, every one of the mirrors below left the dent
/// unsound (watertight=false, 192 boundary mesh edges). They are pinned sound
/// here. Two poses stay broken and are NOT pinned, because the defect is not a
/// reflection's: a mirror about x = 5 (the plane through the sphere centre)
/// and a plain ROTATION by π about x (det +1, a path this task does not
/// change) both leave the dent tessellating the complementary patch of its
/// sphere — 192 same-winding mesh edges at the rim, refused honestly by the
/// certificate, and identical at BASE for the rotation. Reported separately.
#[test]
fn a_sphere_bite_rim_mirrors_through_the_guard() {
    use geometry_engine::primitives::qsic_curve::{Chart, QsicCurve};
    let mut before = BRepModel::new();
    let bid = dented_cylinder(&mut before);
    let bites = before
        .edges
        .iter()
        .filter(|(_, e)| {
            before
                .curves
                .get(e.curve_id)
                .and_then(|c| {
                    c.as_any()
                        .downcast_ref::<QsicCurve>()
                        .map(|q| matches!(q.chart, Chart::SphereBite { .. }))
                })
                .unwrap_or(false)
        })
        .count();
    assert!(bites > 0, "fixture: the dent rim is a SphereBite QSIC");
    let sound0 = before.certify_solid(bid).is_sound();
    let v0 = signed_volume(&before, bid);
    let axis_planes: Vec<(&str, Point3, Vector3)> = vec![
        ("mirror x=3", Point3::new(3.0, 0.0, 0.0), Vector3::X),
        ("mirror x=0", Point3::ORIGIN, Vector3::X),
        ("mirror y=2", Point3::new(0.0, 2.0, 0.0), Vector3::Y),
        ("mirror z=10", Point3::new(0.0, 0.0, 10.0), Vector3::Z),
    ];
    for (how, o, n) in axis_planes {
        let mut m = BRepModel::new();
        let id = dented_cylinder(&mut m);
        mirror(&mut m, vec![id], o, n, TransformOptions::default())
            .unwrap_or_else(|e| panic!("{how}: the guard refused: {e:?}"));
        let cert = m.certify_solid(id);
        assert_eq!(cert.is_sound(), sound0, "{how}: {:?}", cert.errors);
        let v = signed_volume(&m, id);
        assert!(
            v > 0.0 && (v - v0).abs() <= 1e-3 * v0,
            "{how}: volume {v0} -> {v}"
        );
        // The QSIC rim's vertices come out of the boolean to ~1e-7; the
        // mirror must not make that any worse than it was.
        let before_worst = edges_off_their_vertices(&before, bid)
            .into_iter()
            .map(|(_, d)| d)
            .fold(1e-9, f64::max);
        let off: Vec<_> = edges_off_their_vertices(&m, id)
            .into_iter()
            .filter(|&(_, d)| d > 10.0 * before_worst)
            .collect();
        assert!(off.is_empty(), "{how}: {off:?} (before: {before_worst:e})");
    }
}

/// Needs-run 2 of the round-3 review: a fillet on arc edges commutes with a
/// mirror. The mirror reverses every arc edge (its start becomes the image of
/// the old end vertex), so a direction-sensitive schedule would be the risk.
/// Measured: the kernel refuses a VARIABLE fillet on every arc-edge fixture it
/// was offered — a slot's arc (corner-vertex blend unsupported), a slot's or
/// D-profile's rim, a cylinder's closed rim ("requires a periodic radius
/// profile", #89) and a bore's split rim — so the schedule case is not yet
/// constructible. The constant-radius case is pinned: the sampled boundary of
/// fillet-then-mirror equals that of mirror-then-fillet.
#[test]
fn a_bore_rim_fillet_commutes_with_a_mirror() {
    let plane = (Point3::new(30.0, 0.0, 0.0), Vector3::X);
    let top_rim = |m: &BRepModel, id: SolidId| -> Vec<u32> {
        let mut out = Vec::new();
        for f in faces_of(m, id) {
            let face = m.faces.get(f).expect("face");
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            for l in loops {
                for &e in &m.loops.get(l).expect("loop").edges {
                    let edge = m.edges.get(e).expect("edge");
                    let curved = m.curves.get(edge.curve_id).expect("curve").type_name() != "Line";
                    let a = edge.evaluate(0.0, &m.curves).expect("eval");
                    let b = edge.evaluate(0.5, &m.curves).expect("eval");
                    if curved
                        && (a.z - 5.0).abs() < 1e-6
                        && (b.z - 5.0).abs() < 1e-6
                        && !out.contains(&e)
                    {
                        out.push(e);
                    }
                }
            }
        }
        out
    };
    let sampled = |m: &BRepModel, id: SolidId| -> Vec<[i64; 3]> {
        let mut v: Vec<[i64; 3]> = Vec::new();
        for f in faces_of(m, id) {
            let face = m.faces.get(f).expect("face");
            let mut ls = vec![face.outer_loop];
            ls.extend(face.inner_loops.iter().copied());
            for l in ls {
                for &e in &m.loops.get(l).expect("loop").edges {
                    let ed = m.edges.get(e).expect("edge");
                    for k in 0..9 {
                        let p = ed.evaluate(k as f64 / 8.0, &m.curves).expect("eval");
                        v.push([
                            (p.x * 1e3).round() as i64,
                            (p.y * 1e3).round() as i64,
                            (p.z * 1e3).round() as i64,
                        ]);
                    }
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    };
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        ..FilletOptions::default()
    };

    let mut b = BRepModel::new();
    let bid = bored_block(&mut b);
    let rim = top_rim(&b, bid);
    fillet_edges(&mut b, bid, rim, opts.clone()).expect("fillet the rim");
    mirror(
        &mut b,
        vec![bid],
        plane.0,
        plane.1,
        TransformOptions::default(),
    )
    .expect("mirror");

    let mut a = BRepModel::new();
    let aid = bored_block(&mut a);
    mirror(
        &mut a,
        vec![aid],
        plane.0,
        plane.1,
        TransformOptions::default(),
    )
    .expect("mirror");
    let rim = top_rim(&a, aid);
    fillet_edges(&mut a, aid, rim, opts).expect("fillet the mirrored rim");

    assert!(
        a.certify_solid(aid).is_sound(),
        "mirror-then-fillet is sound"
    );
    assert!(
        b.certify_solid(bid).is_sound(),
        "fillet-then-mirror is sound"
    );
    assert_eq!(
        sampled(&a, aid),
        sampled(&b, bid),
        "mirror-then-fillet and fillet-then-mirror must bound the same solid"
    );
}

/// R3-M1: `transform_edges` takes a caller's edge list, which may repeat an
/// id. An arc edge reflected through it is reversed with its curve exactly
/// once — a repeat must not swap its endpoints back while its curve stays
/// reversed.
#[test]
fn a_repeated_edge_id_is_reversed_once() {
    use geometry_engine::operations::transform::transform_edges;
    let mut m = BRepModel::new();
    let id = slot_solid(&mut m);
    let arc = faces_of(&m, id)
        .into_iter()
        .flat_map(|f| {
            let face = m.faces.get(f).expect("face");
            m.loops.get(face.outer_loop).expect("loop").edges.clone()
        })
        .find(|&e| {
            let edge = m.edges.get(e).expect("edge");
            m.curves.get(edge.curve_id).expect("curve").type_name() == "Arc"
        })
        .expect("the slot has an arc edge");
    let reflection =
        Matrix4::mirror(Point3::new(5.0, 0.0, 0.0), Vector3::X).expect("mirror matrix");
    transform_edges(
        &mut m,
        vec![arc, arc],
        reflection,
        TransformOptions::default(),
    )
    .expect("reflect the edge");
    let edge = m.edges.get(arc).expect("edge");
    let vs = m.vertices.get_position(edge.start_vertex).expect("start");
    let ve = m.vertices.get_position(edge.end_vertex).expect("end");
    let ps = edge.evaluate(0.0, &m.curves).expect("start point");
    let pe = edge.evaluate(1.0, &m.curves).expect("end point");
    let ds = ps.distance(&Point3::new(vs[0], vs[1], vs[2]));
    let de = pe.distance(&Point3::new(ve[0], ve[1], ve[2]));
    assert!(
        ds < 1e-9 && de < 1e-9,
        "the reflected arc must still run from its start vertex to its end vertex: off by {ds:e} / {de:e}"
    );
}

/// R3-M2, pinned for Task 97: two poses of the dented cylinder that are
/// broken by a PRE-EXISTING, pose-dependent SphereBite tessellation defect —
/// a mirror through the sphere centre (x = 5) and a plain rotation by π about
/// x (det +1, a path this task does not change; it fails identically at
/// BASE). The dent face tessellates the complementary patch of its sphere and
/// the rim meshes with 192 same-winding edges.
///
/// Pinned as an HONEST REFUSAL: the certificate must call the part unsound and
/// name a mesh-closure conjunct. Measured on this tree the named conjunct is
/// `oriented=false` (at BASE the mirrors read `watertight=false`); either is
/// accepted so the pin tracks the refusal, not one symptom. When Task 97 fixes
/// the defect this test goes RED on purpose — flip it to assert sound.
#[test]
fn task_97_sphere_bite_poses_are_refused_not_passed() {
    let poses: Vec<(&str, Box<dyn Fn(&mut BRepModel, SolidId)>)> = vec![
        (
            "mirror through the sphere centre (x = 5)",
            Box::new(|m, id| {
                mirror(
                    m,
                    vec![id],
                    Point3::new(5.0, 0.0, 0.0),
                    Vector3::X,
                    TransformOptions::default(),
                )
                .expect("the guard passes this mirror");
            }),
        ),
        (
            "rotation by π about x",
            Box::new(|m, id| {
                let r =
                    Matrix4::from_axis_angle(&Vector3::X, std::f64::consts::PI).expect("rotation");
                transform_solid(m, id, r, TransformOptions::default()).expect("rotate");
            }),
        ),
    ];
    for (pose, apply) in poses {
        let mut m = BRepModel::new();
        let id = dented_cylinder(&mut m);
        apply(&mut m, id);
        let cert = m.certify_solid(id);
        assert!(
            !cert.is_sound(),
            "Task 97 ({pose}): the dent is known to mesh wrongly here; a SOUND verdict means the defect was fixed (flip this pin) or the certificate went blind: {:?}",
            cert.errors
        );
        assert!(
            cert.errors
                .iter()
                .any(|e| e.contains("oriented=false") || e.contains("watertight=false")),
            "Task 97 ({pose}): the refusal must name the mesh conjunct that fails: {:?}",
            cert.errors
        );
    }
}
