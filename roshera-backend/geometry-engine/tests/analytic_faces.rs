//! Analytic-face gate (#24 + campaign task #2).
//!
//! A sketched/extruded circle must become ONE analytic `Cylinder` lateral
//! face, not ~64 planar strips. A faceted B-Rep can tessellate watertight by
//! luck, yet it has no diameter to read and booleans badly — so "watertight"
//! is necessary but NOT sufficient. This gate asserts the stronger property:
//! the exact analytic face inventory (surface TYPES + counts) and that each
//! cylinder's radius is recoverable, alongside validity + watertightness.
//!
//! It exercises the #24 path directly: build closed `Circle` profile edges,
//! extrude them (with and without circular holes), and check that the kernel's
//! `create_ruled_surface` -> `try_build_cylinder_from_circles` collapse fired
//! and that `validate_inner_loops_inside_outer` accepts holes inside a round
//! boundary (the curve-sampling fix). If circles ever regress to N-gons these
//! counts explode (cylinder => 66, not 3) and every assertion here fails.
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{
    create_face_from_profile_with_plane, extrude_face, ExtrudeOptions,
};
use geometry_engine::primitives::curve::{Circle, Curve, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{Cylinder, SurfaceType};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

const TOL: f64 = 1e-6;

/// Build a closed analytic `Circle` profile edge (start == end seam vertex at
/// the curve's t=0), mirroring the api-server sketch path #24 produces.
fn circle_edge(m: &mut BRepModel, center: Point3, axis: Vector3, radius: f64) -> u32 {
    let circle =
        Circle::new(center, axis, radius).unwrap_or_else(|e| panic!("circle build: {e:?}"));
    let seam = circle
        .point_at(0.0)
        .unwrap_or_else(|e| panic!("circle seam point_at(0): {e:?}"));
    let v = m
        .vertices
        .add_or_find(seam.x, seam.y, seam.z, Tolerance::default().distance());
    let cid = m.curves.add(Box::new(circle));
    m.edges.add(Edge::new(
        0,
        v,
        v,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ))
}

/// Build a closed polygon profile loop from in-plane points (z=0 plane),
/// returning the edge ids (one `Line` per segment).
fn polygon_edges(m: &mut BRepModel, pts: &[(f64, f64)]) -> Vec<u32> {
    let verts: Vec<_> = pts
        .iter()
        .map(|(x, y)| m.vertices.add(*x, *y, 0.0))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, pts[i].1, 0.0),
            Point3::new(pts[j].0, pts[j].1, 0.0),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    edges
}

/// Extrude an outer loop (already-built edges) with optional circular holes,
/// on the z=0 plane along +Z by `height`.
fn extrude_with_holes(
    m: &mut BRepModel,
    outer_edges: Vec<u32>,
    holes: &[(Point3, f64)],
    height: f64,
    label: &str,
) -> SolidId {
    let face = create_face_from_profile_with_plane(m, outer_edges, Point3::ZERO, Vector3::Z)
        .unwrap_or_else(|e| panic!("{label}: outer face: {e:?}"));
    for &(hc, hr) in holes {
        let he = circle_edge(m, hc, Vector3::Z, hr);
        let mut il = Loop::new(0, LoopType::Inner);
        il.add_edge(he, true);
        let ilid = m.loops.add(il);
        m.faces
            .get_mut(face)
            .unwrap_or_else(|| panic!("{label}: face vanished"))
            .add_inner_loop(ilid);
    }
    let opts = ExtrudeOptions {
        direction: Vector3::Z,
        distance: height,
        ..ExtrudeOptions::default()
    };
    extrude_face(m, face, opts).unwrap_or_else(|e| panic!("{label}: extrude: {e:?}"))
}

/// Surface type of every face on the solid's outer shell.
fn face_kinds(m: &BRepModel, sid: SolidId) -> Vec<SurfaceType> {
    let solid = m.solids.get(sid).unwrap_or_else(|| panic!("solid"));
    let shell = m
        .shells
        .get(solid.outer_shell)
        .unwrap_or_else(|| panic!("shell"));
    shell
        .faces
        .iter()
        .map(|&fid| {
            let f = m.faces.get(fid).unwrap_or_else(|| panic!("face"));
            m.surfaces
                .get(f.surface_id)
                .unwrap_or_else(|| panic!("surface"))
                .surface_type()
        })
        .collect()
}

fn count_kind(kinds: &[SurfaceType], want: SurfaceType) -> usize {
    kinds.iter().filter(|&&k| k == want).count()
}

/// Every cylindrical face's radius on the solid's outer shell.
fn cylinder_radii(m: &BRepModel, sid: SolidId) -> Vec<f64> {
    let solid = m.solids.get(sid).unwrap_or_else(|| panic!("solid"));
    let shell = m
        .shells
        .get(solid.outer_shell)
        .unwrap_or_else(|| panic!("shell"));
    let mut out = Vec::new();
    for &fid in &shell.faces {
        let f = m.faces.get(fid).unwrap_or_else(|| panic!("face"));
        let s = m
            .surfaces
            .get(f.surface_id)
            .unwrap_or_else(|| panic!("surf"));
        if let Some(c) = s.as_any().downcast_ref::<Cylinder>() {
            out.push(c.radius);
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// `size` is a characteristic feature size; mesh deflections are taken as
/// fractions of it so tiny and huge parts are both tessellated sanely (an
/// absolute deflection coarser than the feature makes ANY solid's mesh
/// degenerate — that's a tessellation-density artifact, not a real defect).
fn assert_watertight(m: &BRepModel, sid: SolidId, size: f64, label: &str) {
    let v = validate_solid_scoped(m, sid, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "{label}: B-Rep invalid: {:?}", v.errors);
    for frac in [0.05_f64, 0.01, 0.002] {
        let defl = size * frac;
        let rep = manifold_report(m, sid, defl, 1e-6)
            .unwrap_or_else(|| panic!("{label}: manifold_report none @defl {defl}"));
        assert_eq!(
            (rep.boundary_edges, rep.nonmanifold_edges),
            (0, 0),
            "{label}: not watertight @defl {defl}",
        );
    }
}

fn approx_contains(radii: &[f64], want: f64) -> bool {
    radii.iter().any(|&r| (r - want).abs() < TOL)
}

#[test]
fn cylinder_is_three_analytic_faces_24() {
    // The headline: a plain extruded circle = 2 planar caps + 1 Cylinder, and
    // the Cylinder's radius is exactly recoverable. A 64-gon regression would
    // give 66 faces and zero Cylinder faces.
    for (radius, height) in [(1.0, 4.0), (5.0, 40.0), (100.0, 3.0), (0.5, 2.0)] {
        let mut m = BRepModel::new();
        let e = circle_edge(&mut m, Point3::ZERO, Vector3::Z, radius);
        let sid = extrude_with_holes(&mut m, vec![e], &[], height, "cylinder");
        let kinds = face_kinds(&m, sid);
        assert_eq!(
            kinds.len(),
            3,
            "cylinder r{radius}: face count (kinds={kinds:?})"
        );
        assert_eq!(
            count_kind(&kinds, SurfaceType::Cylinder),
            1,
            "cylinder r{radius}: 1 lateral"
        );
        assert_eq!(
            count_kind(&kinds, SurfaceType::Plane),
            2,
            "cylinder r{radius}: 2 caps"
        );
        let radii = cylinder_radii(&m, sid);
        assert!(
            approx_contains(&radii, radius),
            "cylinder r{radius}: radius not recoverable (got {radii:?})"
        );
        assert_watertight(&m, sid, radius, "cylinder");
    }
}

#[test]
#[ignore = "FINDING (task #11 blind-spot): extreme-aspect cylinder (r0.5 x h200, \
            400:1) is a VALID B-Rep (validate_solid_scoped ok) but tessellates \
            with 2 nonmanifold mesh edges at the seam — a tessellation blind \
            spot, not a B-Rep defect. Pinned for the blind-spot audit."]
fn extreme_aspect_cylinder_mesh_nonmanifold_finding() {
    let mut m = BRepModel::new();
    let e = circle_edge(&mut m, Point3::ZERO, Vector3::Z, 0.5);
    let sid = extrude_with_holes(&mut m, vec![e], &[], 200.0, "thin cylinder");
    // B-Rep IS valid — the analytic faces are correct.
    let v = validate_solid_scoped(&m, sid, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "B-Rep should be valid: {:?}", v.errors);
    assert_eq!(count_kind(&face_kinds(&m, sid), SurfaceType::Cylinder), 1);
    // The mesh, however, is non-manifold at fine deflection (the finding).
    assert_watertight(&m, sid, 0.5, "thin cylinder");
}

#[test]
fn cylinder_on_arbitrary_axis_24() {
    // Analytic faces must not depend on the extrude axis being +Z.
    for axis in [Vector3::X, Vector3::Y, Vector3::new(1.0, 1.0, 1.0)] {
        let mut m = BRepModel::new();
        let n = axis.normalize().unwrap_or(Vector3::Z);
        let circle = Circle::new(Point3::ZERO, n, 7.0).unwrap_or_else(|e| panic!("circle: {e:?}"));
        let seam = circle
            .point_at(0.0)
            .unwrap_or_else(|e| panic!("seam: {e:?}"));
        let v = m
            .vertices
            .add_or_find(seam.x, seam.y, seam.z, Tolerance::default().distance());
        let cid = m.curves.add(Box::new(circle));
        let e = m.edges.add(Edge::new(
            0,
            v,
            v,
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let face = create_face_from_profile_with_plane(&mut m, vec![e], Point3::ZERO, n)
            .unwrap_or_else(|e| panic!("face: {e:?}"));
        let opts = ExtrudeOptions {
            direction: n,
            distance: 12.0,
            ..ExtrudeOptions::default()
        };
        let sid = extrude_face(&mut m, face, opts).unwrap_or_else(|e| panic!("extrude: {e:?}"));
        let kinds = face_kinds(&m, sid);
        assert_eq!(
            count_kind(&kinds, SurfaceType::Cylinder),
            1,
            "axis {n:?}: 1 cylinder"
        );
        assert_eq!(kinds.len(), 3, "axis {n:?}: 3 faces");
        assert_watertight(&m, sid, 7.0, "axis cylinder");
    }
}

#[test]
fn tube_has_two_analytic_walls_24() {
    // Annulus (outer circle + circular hole) => 2 planar annular caps + 2
    // Cylinder walls (outer + inner), both radii recoverable.
    let mut m = BRepModel::new();
    let outer = circle_edge(&mut m, Point3::ZERO, Vector3::Z, 30.0);
    let sid = extrude_with_holes(&mut m, vec![outer], &[(Point3::ZERO, 18.0)], 70.0, "tube");
    let kinds = face_kinds(&m, sid);
    assert_eq!(
        count_kind(&kinds, SurfaceType::Cylinder),
        2,
        "tube: outer+inner walls (kinds={kinds:?})"
    );
    assert_eq!(
        count_kind(&kinds, SurfaceType::Plane),
        2,
        "tube: 2 annular caps"
    );
    let radii = cylinder_radii(&m, sid);
    assert!(
        approx_contains(&radii, 30.0) && approx_contains(&radii, 18.0),
        "tube radii: {radii:?}"
    );
    assert_watertight(&m, sid, 30.0, "tube");
}

#[test]
fn round_flange_with_bolt_circle_24() {
    // The part that failed all session before #24: a round flange with a
    // central bore + a 6-hole bolt circle. Every hole is an analytic Cylinder
    // wall, and holes inside a ROUND outer loop must pass the inner-loop
    // containment test (the curve-sampling validator fix). Inventory: 2 caps +
    // outer wall + bore wall + 6 bolt walls = 10 faces, all analytic.
    let mut m = BRepModel::new();
    let outer = circle_edge(&mut m, Point3::ZERO, Vector3::Z, 55.0);
    let mut holes = vec![(Point3::ZERO, 18.0)];
    for i in 0..6 {
        let a = std::f64::consts::TAU * i as f64 / 6.0;
        holes.push((Point3::new(44.0 * a.cos(), 44.0 * a.sin(), 0.0), 4.0));
    }
    let sid = extrude_with_holes(&mut m, vec![outer], &holes, 14.0, "flange");
    let kinds = face_kinds(&m, sid);
    assert_eq!(
        count_kind(&kinds, SurfaceType::Cylinder),
        8,
        "flange: 8 cylinder walls (kinds={kinds:?})"
    );
    assert_eq!(count_kind(&kinds, SurfaceType::Plane), 2, "flange: 2 caps");
    assert_eq!(kinds.len(), 10, "flange: 10 faces total");
    let radii = cylinder_radii(&m, sid);
    assert!(
        approx_contains(&radii, 55.0) && approx_contains(&radii, 18.0),
        "flange OD/bore: {radii:?}"
    );
    assert_eq!(
        radii.iter().filter(|&&r| (r - 4.0).abs() < TOL).count(),
        6,
        "flange: 6 bolt holes"
    );
    assert_watertight(&m, sid, 55.0, "flange");
}

#[test]
fn rectangle_with_round_hole_24() {
    // A circular hole inside a POLYGON outer loop: 6 planar faces (4 walls +
    // 2 caps) + 1 analytic Cylinder hole wall = 7 faces.
    let mut m = BRepModel::new();
    let outer = polygon_edges(
        &mut m,
        &[(-30.0, -30.0), (30.0, -30.0), (30.0, 30.0), (-30.0, 30.0)],
    );
    let sid = extrude_with_holes(&mut m, outer, &[(Point3::ZERO, 12.0)], 40.0, "plate");
    let kinds = face_kinds(&m, sid);
    // 1 analytic Cylinder hole wall is the #24 point; the other 6 faces are 2
    // planar caps + 4 ruled straight walls (geometrically planar). No 64-strip
    // explosion: exactly 7 faces total.
    assert_eq!(
        count_kind(&kinds, SurfaceType::Cylinder),
        1,
        "plate: 1 cylinder hole (kinds={kinds:?})"
    );
    assert_eq!(kinds.len(), 7, "plate: 7 faces total (kinds={kinds:?})");
    assert!(
        approx_contains(&cylinder_radii(&m, sid), 12.0),
        "plate hole radius"
    );
    assert_watertight(&m, sid, 30.0, "plate");
}
