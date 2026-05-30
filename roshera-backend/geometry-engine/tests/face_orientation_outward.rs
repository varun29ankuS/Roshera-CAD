//! Kernel-wide outward-normal regression suite.
//!
//! Slices 2 & 3 of the comprehensive face-orientation fix establish a
//! global invariant: **every face on a closed solid carries a
//! `FaceOrientation` such that its oriented surface normal at every
//! point on the face points away from the solid material**.
//!
//! This file pins that invariant for every primitive (Slice 3) and
//! for the boolean ops that build split / inherited faces (Slice 2's
//! `build_shells_from_faces`). If the invariant regresses, downstream
//! consumers (fillet rolling ball, chamfer bisector, shell solid-angle,
//! mass-properties divergence integrals, tessellation winding) silently
//! produce wrong results — pinning the invariant here turns regressions
//! into a single visible test failure.
//!
//! ## Sampling strategy
//!
//! Earlier drafts of this suite sampled the surface at the **surface's**
//! parametric midpoint. That is unreliable in two ways:
//!
//! - For planes the parameter range is unbounded so the midpoint is
//!   `±∞` / NaN.
//! - For trimmed surfaces (e.g. a cone face that lives on the kernel's
//!   *infinite* cone surface) the surface midpoint can lie outside the
//!   face's UV trim region — sampling there reports a point on the
//!   reflected/extrapolated surface beyond the apex.
//!
//! Instead we run the production tessellator on the face. That gives
//! us a mesh whose vertices are *on* the face (positions inside the
//! UV trim region) with normals computed via `Face::normal_at`, which
//! is exactly `surface.normal_at(u, v) * face.orientation.sign()` —
//! the very quantity downstream consumers consume. We then check that
//! this oriented normal points away from a known-interior reference
//! point for the solid.
//!
//! ## Per-face interior reference
//!
//! For convex primitives a single fixed centroid is "inside" relative
//! to every face. For a torus the solid material is non-convex (the
//! bbox centre sits in the hole, *outside* the material), so faces on
//! the inside of the tube would fail a fixed-centre check. The
//! checker therefore takes a closure `interior_fn: Fn(Point3) -> Point3`
//! that maps a face sample point to the appropriate interior reference
//! — for a torus we project onto the major circle so every face's
//! reference lies inside the tube directly opposite that face.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::edge_cache::EdgeSampleCache;
use geometry_engine::tessellation::{tessellate_face, TessellationParams, TriangleMesh};

// ---------------------------------------------------------------------
// Invariant checker.
// ---------------------------------------------------------------------

/// Assert that every face on the solid carries an orientation whose
/// oriented surface normal points away from the per-face interior
/// reference returned by `interior_fn`.
///
/// `interior_fn(sample_position) -> interior_reference` lets each test
/// specify the geometry-appropriate "inside" point for the sample.
/// Convex solids pass `|_| fixed_centre`; non-convex solids (torus)
/// pass a closure that picks a per-face reference.
///
/// `skip_if(position) -> bool` skips degenerate sample points where
/// the surface normal is mathematically undefined (e.g. the apex of
/// a cone, where the tangent plane is multi-valued). Tests with no
/// degenerate points pass `|_| false`.
fn assert_every_face_oriented_outward<F, S>(
    model: &BRepModel,
    solid_id: SolidId,
    interior_fn: F,
    skip_if: S,
    label: &str,
) where
    F: Fn(Point3) -> Point3,
    S: Fn(Point3) -> bool,
{
    // Collect every face id from every shell on the solid (outer + voids).
    let face_ids: Vec<_> = {
        let solid = model
            .solids
            .get(solid_id)
            .unwrap_or_else(|| panic!("[{label}] solid {solid_id:?} missing"));
        let mut ids = Vec::new();
        let shell_ids =
            std::iter::once(solid.outer_shell).chain(solid.inner_shells.iter().copied());
        for shell_id in shell_ids {
            if let Some(shell) = model.shells.get(shell_id) {
                ids.extend(shell.face_ids().iter().copied());
            }
        }
        ids
    };
    assert!(
        !face_ids.is_empty(),
        "[{label}] solid {solid_id:?} has no faces — degenerate shell"
    );

    let params = TessellationParams::coarse();
    let cache = EdgeSampleCache::new(&params);
    for face_id in face_ids {
        let face = model
            .faces
            .get(face_id)
            .unwrap_or_else(|| panic!("[{label}] face {face_id:?} missing"));

        let mut mesh = TriangleMesh::new();
        tessellate_face(face, model, &params, &cache, &mut mesh);

        assert!(
            !mesh.vertices.is_empty(),
            "[{label}] face {face_id:?} tessellated to zero vertices — \
             cannot verify orientation",
        );

        // Per-vertex: the tessellator computes normals via
        // `face.normal_at(u, v) = surface.normal_at(u, v) * orientation.sign()`.
        // If `face.orientation` is wrong, the per-vertex normal is
        // inverted and the dot below becomes negative.
        for v in &mesh.vertices {
            if !v.position.x.is_finite()
                || !v.position.y.is_finite()
                || !v.position.z.is_finite()
                || !v.normal.x.is_finite()
                || !v.normal.y.is_finite()
                || !v.normal.z.is_finite()
            {
                continue;
            }
            if skip_if(v.position) {
                continue;
            }
            let interior = interior_fn(v.position);
            let outward_dir = v.position - interior;
            // Skip samples that lie essentially at the interior
            // reference (degenerate direction).
            if outward_dir.magnitude_squared() < 1e-12 {
                continue;
            }
            let dot = v.normal.dot(&outward_dir);
            assert!(
                dot >= -1e-6,
                "[{label}] face {face_id:?}: oriented surface normal points INWARD. \
                 pos=({px:.4},{py:.4},{pz:.4}) interior=({ix:.4},{iy:.4},{iz:.4}) \
                 normal=({nx:.4},{ny:.4},{nz:.4}) orientation={orient:?} dot={dot:.6}",
                px = v.position.x,
                py = v.position.y,
                pz = v.position.z,
                ix = interior.x,
                iy = interior.y,
                iz = interior.z,
                nx = v.normal.x,
                ny = v.normal.y,
                nz = v.normal.z,
                orient = face.orientation,
            );
        }
    }
}

/// Helper: build the major-circle projection closure for a torus
/// centred at `centre` with axis `axis` (unit) and major radius `R`.
/// For any face sample point `P` the closest point on the major
/// circle is `centre + R * radial_dir`, where `radial_dir` is the
/// in-plane component of `P - centre` normalised.
fn torus_major_circle_projector(
    centre: Point3,
    axis: Vector3,
    major_radius: f64,
) -> impl Fn(Point3) -> Point3 {
    move |p| {
        let to_p = p - centre;
        let axial = axis * to_p.dot(&axis);
        let radial = to_p - axial;
        let radial_len = radial.magnitude();
        if radial_len < 1e-9 {
            // Sample sits on the torus axis — pick an arbitrary
            // representative point on the major circle. This branch
            // is unreachable for any well-formed torus face but we
            // guard against degenerate samples rather than panicking.
            return centre + Vector3::X * major_radius;
        }
        centre + radial * (major_radius / radial_len)
    }
}

// ---------------------------------------------------------------------
// Primitive factories.
// ---------------------------------------------------------------------

fn solid_of(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid geometry, got {other:?}"),
    }
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    solid_of(builder.create_box_3d(w, h, d).expect("create_box_3d"))
}

fn make_cylinder(model: &mut BRepModel, centre: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    solid_of(
        builder
            .create_cylinder_3d(centre, axis, r, h)
            .expect("create_cylinder_3d"),
    )
}

fn make_cone(
    model: &mut BRepModel,
    base: Point3,
    axis: Vector3,
    base_r: f64,
    height: f64,
) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    solid_of(
        builder
            .create_cone_3d(base, axis, base_r, 0.0, height)
            .expect("create_cone_3d"),
    )
}

fn make_sphere(model: &mut BRepModel, centre: Point3, r: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    solid_of(
        builder
            .create_sphere_3d(centre, r)
            .expect("create_sphere_3d"),
    )
}

fn make_torus(
    model: &mut BRepModel,
    centre: Point3,
    axis: Vector3,
    major: f64,
    minor: f64,
) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    solid_of(
        builder
            .create_torus_3d(centre, axis, major, minor)
            .expect("create_torus_3d"),
    )
}

// ---------------------------------------------------------------------
// Slice 3 — primitives.
// ---------------------------------------------------------------------

#[test]
fn box_every_face_oriented_outward() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 2.0, 3.0, 4.0);
    let centre = Point3::new(1.0, 1.5, 2.0);
    assert_every_face_oriented_outward(&model, id, |_| centre, |_| false, "box(2,3,4)");
}

#[test]
fn cylinder_every_face_oriented_outward() {
    let mut model = BRepModel::new();
    let id = make_cylinder(&mut model, Point3::ZERO, Vector3::Z, 1.0, 2.0);
    let centre = Point3::new(0.0, 0.0, 1.0);
    assert_every_face_oriented_outward(&model, id, |_| centre, |_| false, "cylinder(r=1, h=2)");
}

#[test]
fn cone_every_face_oriented_outward() {
    let mut model = BRepModel::new();
    let id = make_cone(&mut model, Point3::ZERO, Vector3::Z, 1.0, 2.0);
    // Pick an interior point on the cone axis, inside the actual
    // physical span (z ∈ [0, 2]). Tessellation samples the face's
    // trimmed region so vertex z-values land in [0, 2]; the interior
    // ref at z=0.5 keeps `(pos - centre)` dominated by the radial
    // component on the lateral face and by ±axis on the cap.
    let centre = Point3::new(0.0, 0.0, 0.5);
    // Two classes of degenerate samples to skip on the cone:
    //
    // 1. The apex itself (0, 0, 2). The tangent plane is multi-valued
    //    in the limit, so the surface normal is mathematically
    //    undefined; the tessellator returns a degenerate fallback.
    //
    // 2. Samples lying *outside* the cone's physical extent
    //    (z ∉ [0, 2]). The kernel's cone surface is mathematically a
    //    double-cone (hourglass extending past the apex), and
    //    `tessellation::surface::get_face_parameter_bounds` falls
    //    back to the surface's full v-range when the face's outer
    //    loop projects to a single v-line (the apex-degenerate
    //    case). The tessellator therefore emits a small overshoot
    //    band beyond the apex. This is a separate kernel issue
    //    (tessellator should respect the face's trim region for
    //    apex-degenerate cones) and is orthogonal to the
    //    face-orientation invariant we are pinning here.
    let skip_cone_degenerate = move |p: Point3| {
        let on_apex = (p.x.abs() < 1e-6) && (p.y.abs() < 1e-6) && ((p.z - 2.0).abs() < 1e-6);
        let outside_physical_z = p.z < -1e-6 || p.z > 2.0 + 1e-6;
        on_apex || outside_physical_z
    };
    assert_every_face_oriented_outward(
        &model,
        id,
        |_| centre,
        skip_cone_degenerate,
        "cone(r=1, h=2)",
    );
}

#[test]
fn sphere_every_face_oriented_outward() {
    let mut model = BRepModel::new();
    let id = make_sphere(&mut model, Point3::ZERO, 1.5);
    let centre = Point3::ZERO;
    assert_every_face_oriented_outward(&model, id, |_| centre, |_| false, "sphere(r=1.5)");
}

#[test]
fn torus_every_face_oriented_outward() {
    let mut model = BRepModel::new();
    let id = make_torus(&mut model, Point3::ZERO, Vector3::Z, 2.0, 0.5);
    // A torus is non-convex: every face sample's interior reference is
    // the closest point on the major circle (radius R=2 in the XY
    // plane), so the outward direction is always radial-from-tube-axis.
    let projector = torus_major_circle_projector(Point3::ZERO, Vector3::Z, 2.0);
    assert_every_face_oriented_outward(&model, id, projector, |_| false, "torus(R=2, r=0.5)");
}

// ---------------------------------------------------------------------
// Slice 2 — boolean op that builds inherited / split faces.
// ---------------------------------------------------------------------

#[test]
fn boolean_union_box_sphere_every_face_oriented_outward() {
    // Use box + sphere instead of box + box. Sphere has no planar
    // faces, so the plane-plane coincidence path that triggers the
    // documented Greiner-Hormann shared-vertex degeneracy on
    // axis-aligned box-box unions cannot fire. Sphere also forces
    // the boolean to exercise both plane-on-curve and curve-on-curve
    // intersection — the same `build_shells_from_faces` path that
    // Slice 2 patches for split/inherited face orientation.
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 2.0, 2.0, 2.0);
    let b = make_sphere(&mut model, Point3::new(1.0, 1.0, 1.0), 1.2);
    let id = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union(box, sphere) succeeds");
    // Both inputs surround (1, 1, 1): the box centroid, also the
    // sphere centre. The union's interior contains this point by
    // construction.
    let centre = Point3::new(1.0, 1.0, 1.0);
    assert_every_face_oriented_outward(
        &model,
        id,
        |_| centre,
        |_| false,
        "boolean_union(box, sphere)",
    );
}
