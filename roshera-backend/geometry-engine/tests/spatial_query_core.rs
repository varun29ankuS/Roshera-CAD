//! Spatial-query core harness (#15).
//!
//! The five composable primitives — ray, point, field, region, relational —
//! must agree with EACH OTHER and with closed-form analytic ground truth. This
//! gate asserts:
//!
//!   * exactness — `signed_distance` equals the analytic SDF of box / cylinder
//!     / sphere at probe points whose nearest feature is a face interior;
//!   * cross-primitive consistency — `classify_point`'s side matches the sign of
//!     `signed_distance`, and `|signed_distance|` matches `nearest_on_solid`;
//!   * field ↔ point — every grid node's field sign matches `classify_point`;
//!   * region soundness — a thin query slab selects the face on that side and
//!     not the opposite face, and a selected face genuinely overlaps the query;
//!   * relational exactness — a box's face axes are exactly the ±X/±Y/±Z basis,
//!     and the coaxial/parallel/perpendicular relations are symmetric;
//!   * determinism — sampling the field twice is bit-identical.
//!
//! Every check reads analytic surfaces, never the mesh.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::{
    are_coaxial, are_parallel, are_perpendicular, axis_relation, classify_point, face_axis,
    faces_in_box, nearest_on_solid, sample_field_adaptive, signed_distance, AxisRelation,
    PointClass, WorldBox,
};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Closed-form signed distance to an axis-aligned box centred at the origin with
/// the given half-extents. Negative inside.
fn box_sdf(p: Point3, half: Vector3) -> f64 {
    let qx = p.x.abs() - half.x;
    let qy = p.y.abs() - half.y;
    let qz = p.z.abs() - half.z;
    let outside = Vector3::new(qx.max(0.0), qy.max(0.0), qz.max(0.0)).magnitude();
    let inside = qx.max(qy).max(qz).min(0.0);
    outside + inside
}

/// Assert the kernel's signed distance matches an analytic value at `p`.
fn assert_sd(m: &BRepModel, s: SolidId, p: Point3, expect: f64, label: &str) {
    let (sd, fid) = signed_distance(m, s, p).unwrap_or_else(|| panic!("{label}: no sd at {p:?}"));
    assert!(
        (sd - expect).abs() < 1e-5,
        "{label}: signed_distance {sd} != analytic {expect} at {p:?}"
    );
    // The face named is real and the magnitude matches nearest_on_solid.
    let (nf, _, nd) =
        nearest_on_solid(m, s, p).unwrap_or_else(|| panic!("{label}: no nearest at {p:?}"));
    assert_eq!(nf, fid, "{label}: signed_distance face == nearest face");
    assert!(
        (nd - sd.abs()).abs() < 1e-6,
        "{label}: |sd| {} != nearest dist {nd}",
        sd.abs()
    );
    // Side agrees with classify_point.
    let cls = classify_point(m, s, p, 1e-6);
    let by_sign = if sd.abs() <= 1e-6 {
        PointClass::On
    } else if sd < 0.0 {
        PointClass::Inside
    } else {
        PointClass::Outside
    };
    assert_eq!(
        cls, by_sign,
        "{label}: classify disagrees with sd sign at {p:?}"
    );
}

#[test]
fn box_signed_distance_matches_analytic_sdf() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let half = Vector3::new(10.0, 10.0, 10.0);
    // Probe points whose nearest feature is a face interior (on an axis, so the
    // projection lands inside a face — avoids the corner/edge refinement case).
    let probes = [
        Point3::ZERO,                 // deep inside → -10
        Point3::new(0.0, 0.0, 9.0),   // inside near +Z → -1
        Point3::new(0.0, 0.0, 15.0),  // outside +Z → +5
        Point3::new(7.0, 0.0, 0.0),   // inside near +X → -3
        Point3::new(0.0, -25.0, 0.0), // outside -Y → +15
        Point3::new(0.0, 0.0, -10.0), // on -Z face → 0
    ];
    for p in probes {
        assert_sd(&m, b, p, box_sdf(p, half), "box");
    }
}

#[test]
fn sphere_signed_distance_matches_analytic_sdf() {
    let mut m = BRepModel::new();
    let r = 12.0;
    let s = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::ZERO, r)
        .expect("sphere"));
    let probes = [
        Point3::ZERO,
        Point3::new(0.0, 0.0, 5.0),
        Point3::new(0.0, 0.0, 12.0),
        Point3::new(20.0, 0.0, 0.0),
        Point3::new(8.0, 0.0, 0.0),
    ];
    for p in probes {
        let expect = p.magnitude() - r;
        assert_sd(&m, s, p, expect, "sphere");
    }
}

#[test]
fn cylinder_signed_distance_matches_analytic_wall() {
    // Cylinder r=10 along Z from origin, height 40. At mid-height the nearest
    // feature for a radial probe is the lateral wall: sd = ρ − r.
    let mut m = BRepModel::new();
    let r = 10.0;
    let c = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, r, 40.0)
        .expect("cyl"));
    // Probe along +Y, not +X: the cylinder seam sits at u=0 (+X), where the
    // wall projection is rejected by the trim check and nearest falls to a cap
    // (the documented seam caveat — see MISSING.md). ρ=0 (on the axis) is also
    // excluded as closest_point is degenerate there.
    for &rho in &[4.0_f64, 9.0, 14.0, 25.0] {
        let p = Point3::new(0.0, rho, 20.0);
        // Interior: nearest is whichever is closer — wall (r−ρ) or cap (20). For
        // these ρ the wall is nearer, so |sd| = |ρ − r|, sign by inside/outside.
        let expect = rho - r;
        assert_sd(&m, c, p, expect, "cyl wall");
    }
}

#[test]
fn adaptive_field_sign_matches_classify_everywhere() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 20.0, 16.0)
        .expect("box"));
    let f = sample_field_adaptive(
        &m,
        b,
        Point3::new(-20.0, -15.0, -12.0),
        Point3::new(20.0, 15.0, 12.0),
        3,
    );
    assert!(!f.cells.is_empty(), "adaptive field has leaves");
    for c in &f.cells {
        let cls = classify_point(&m, b, c.center, 1e-6);
        let sd = c.distance;
        match cls {
            PointClass::Inside => assert!(sd < 1e-6, "inside cell has sd {sd} at {:?}", c.center),
            PointClass::Outside => {
                assert!(sd > -1e-6, "outside cell has sd {sd} at {:?}", c.center)
            }
            PointClass::On => assert!(sd.abs() < 1e-3, "on cell has sd {sd} at {:?}", c.center),
        }
    }
}

#[test]
fn adaptive_field_is_deterministic() {
    let mut m = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::ZERO, 10.0)
        .expect("sphere"));
    let mk = || {
        sample_field_adaptive(
            &m,
            s,
            Point3::new(-12.0, -12.0, -12.0),
            Point3::new(12.0, 12.0, 12.0),
            4,
        )
    };
    let a = mk();
    let b = mk();
    assert_eq!(a.cells, b.cells, "adaptive leaves bit-identical");
}

#[test]
fn adaptive_field_scales_with_surface_not_volume() {
    // THE de-voxeling proof: a sphere sampled at depth 4 must spend far fewer
    // leaves than the (2^4)³ = 4096 nodes a dense grid needs, because only the
    // narrow band around the surface refines — interior and empty space stay
    // as coarse leaves. Cost ∝ surface area, not volume.
    let mut m = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::ZERO, 10.0)
        .expect("sphere"));
    let sample = |depth: u32| {
        sample_field_adaptive(
            &m,
            s,
            Point3::new(-12.0, -12.0, -12.0),
            Point3::new(12.0, 12.0, 12.0),
            depth,
        )
    };
    let f4 = sample(4);
    let f6 = sample(6);
    assert_eq!(f4.uniform_equivalent(), 4096, "depth-4 dense grid is 16³");
    assert_eq!(
        f6.uniform_equivalent(),
        262_144,
        "depth-6 dense grid is 64³"
    );
    // Any saving at all at depth 4 (the sphere nearly fills this box, so the
    // band dominates here — the LAW below is the real claim).
    assert!(
        f4.leaf_count() < f4.uniform_equivalent(),
        "depth 4: {} leaves vs {} dense",
        f4.leaf_count(),
        f4.uniform_equivalent()
    );
    // THE SCALING LAW: refining 4 → 6 multiplies a dense grid by 64×, but the
    // adaptive field only refines the surface band, which grows ~4× per level
    // (area, not volume). Allow generous slack over the ideal 16×.
    let growth = f6.leaf_count() as f64 / f4.leaf_count() as f64;
    assert!(
        growth < 24.0,
        "adaptive growth 4→6 must be area-like (~16x), got {growth:.1}x \
         ({} → {} leaves; dense grows 64x)",
        f4.leaf_count(),
        f6.leaf_count()
    );
    // And at depth 6 the absolute saving is unambiguous.
    assert!(
        f6.leaf_count() < f6.uniform_equivalent() / 4,
        "depth 6: {} leaves must be <1/4 of the {} dense nodes",
        f6.leaf_count(),
        f6.uniform_equivalent()
    );
    // The band genuinely refined to max depth somewhere on the surface…
    assert!(
        f6.cells.iter().any(|c| c.depth == 6),
        "surface band reaches max depth"
    );
    // …and every leaf's distance is EXACTLY the analytic per-point answer at
    // its center (the adaptive structure never invents values).
    for c in &f4.cells {
        let (sd, fid) = signed_distance(&m, s, c.center).expect("sd at leaf center");
        assert_eq!(sd, c.distance, "leaf distance == direct signed_distance");
        assert_eq!(fid, c.face, "leaf face == direct nearest face");
    }
}

#[test]
fn region_slab_selects_near_face_not_far_face() {
    // A thin slab hugging the +Z plane of a box selects the +Z face (the one a
    // top-down ray hits) and never the −Z face.
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    // Identify the +Z face by raycasting down onto the top.
    let top = geometry_engine::queries::raycast_solid(
        &m,
        b,
        Point3::new(0.0, 0.0, 40.0),
        Vector3::new(0.0, 0.0, -1.0),
    )
    .expect("hit top")
    .face_id;
    let bottom = geometry_engine::queries::raycast_solid(
        &m,
        b,
        Point3::new(0.0, 0.0, -40.0),
        Vector3::new(0.0, 0.0, 1.0),
    )
    .expect("hit bottom")
    .face_id;

    let slab = WorldBox {
        min: Point3::new(-11.0, -11.0, 9.5),
        max: Point3::new(11.0, 11.0, 11.0),
    };
    let sel = faces_in_box(&m, b, slab);
    assert!(sel.contains(&top), "+Z slab selects the top face");
    assert!(
        !sel.contains(&bottom),
        "+Z slab must NOT select the bottom face"
    );
    // Soundness: every selected face really overlaps the slab.
    for fid in &sel {
        let bb = geometry_engine::queries::face_world_box(&m, *fid).expect("box");
        assert!(
            bb.intersects_box(&slab),
            "selected face {fid} overlaps slab"
        );
    }
}

#[test]
fn relational_box_axes_are_exact_basis_and_symmetric() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let solid = m.solids.get(b).unwrap();
    let faces: Vec<u32> = m.shells.get(solid.outer_shell).unwrap().faces.clone();
    assert_eq!(faces.len(), 6);

    // Each plane normal is one of the six signed basis directions.
    for &fid in &faces {
        let ax = face_axis(&m, fid).expect("plane axis");
        let n = ax.dir;
        let on_axis = (n.x.abs() > 0.999 && n.y.abs() < 1e-6 && n.z.abs() < 1e-6)
            || (n.y.abs() > 0.999 && n.x.abs() < 1e-6 && n.z.abs() < 1e-6)
            || (n.z.abs() > 0.999 && n.x.abs() < 1e-6 && n.y.abs() < 1e-6);
        assert!(on_axis, "box face normal {n:?} is an exact basis direction");
    }

    // Relations are symmetric across every pair.
    for (ia, &fa) in faces.iter().enumerate() {
        for &fb in faces.iter().skip(ia + 1) {
            assert_eq!(
                are_parallel(&m, fa, fb, 1e-6),
                are_parallel(&m, fb, fa, 1e-6),
                "parallel symmetric"
            );
            assert_eq!(
                are_perpendicular(&m, fa, fb, 1e-6),
                are_perpendicular(&m, fb, fa, 1e-6),
                "perpendicular symmetric"
            );
            assert_eq!(
                are_coaxial(&m, fa, fb, 1e-6, 1e-6),
                are_coaxial(&m, fb, fa, 1e-6, 1e-6),
                "coaxial symmetric"
            );
            // Exactly one of parallel / perpendicular holds for axis-aligned box
            // faces (no skew among the basis directions).
            let par = are_parallel(&m, fa, fb, 1e-6);
            let perp = are_perpendicular(&m, fa, fb, 1e-6);
            assert!(par ^ perp, "box face pair is parallel XOR perpendicular");
        }
    }
}

#[test]
fn relational_axis_relation_classifies_cylinders() {
    let mut m = BRepModel::new();
    let zc = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
        .expect("zc"));
    let zc2 = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 40.0), Vector3::Z, 5.0, 10.0)
        .expect("zc2"));
    let off = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(40.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
        .expect("off"));
    let xc = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::X, 5.0, 10.0)
        .expect("xc"));
    let cyl_face = |m: &BRepModel, s: SolidId| -> u32 {
        let solid = m.solids.get(s).unwrap();
        for &fid in &m.shells.get(solid.outer_shell).unwrap().faces {
            let f = m.faces.get(fid).unwrap();
            if face_axis(m, fid)
                .map(|a| matches!(a.kind, geometry_engine::queries::AxisKind::CylinderAxis))
                .unwrap_or(false)
            {
                let _ = f;
                return fid;
            }
        }
        panic!("no cyl face")
    };
    let a = face_axis(&m, cyl_face(&m, zc)).unwrap();
    let a2 = face_axis(&m, cyl_face(&m, zc2)).unwrap();
    let ao = face_axis(&m, cyl_face(&m, off)).unwrap();
    let ax = face_axis(&m, cyl_face(&m, xc)).unwrap();
    assert_eq!(axis_relation(&a, &a2, 1e-6, 1e-6), AxisRelation::Coaxial);
    assert_eq!(axis_relation(&a, &ao, 1e-6, 1e-6), AxisRelation::Parallel);
    assert_eq!(
        axis_relation(&a, &ax, 1e-6, 1e-6),
        AxisRelation::Perpendicular
    );
}
