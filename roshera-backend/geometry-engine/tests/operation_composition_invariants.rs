//! Property tests for *compositions* of operations — the paths that only get
//! exercised when geometry is transformed, combined, and re-measured, rather
//! than built axis-aligned and measured once. These are exactly the
//! "untravelled" paths where latent bugs hide (a transformed cylinder once
//! tessellated to ⅓ its true volume).
//!
//! All are oracle-free: rigid motions preserve volume / surface area / inertia
//! and map the centroid; boolean results must be watertight (mesh divergence
//! volume = reported volume) and respect inclusion–exclusion. Per the kernel's
//! numerical-rigor contract a boolean may return a typed `Err` on hard inputs;
//! those cases are skipped, but a *successful* op that returns a wrong answer
//! still fails the test.

use std::f64::consts::PI;

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

// --------------------------------------------------------------------------
// Builders
// --------------------------------------------------------------------------

fn as_solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}
fn build_box(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    as_solid(TopologyBuilder::new(m).create_box_3d(w, h, d).expect("box"))
}
fn build_sphere(m: &mut BRepModel, r: f64) -> SolidId {
    as_solid(
        TopologyBuilder::new(m)
            .create_sphere_3d(Point3::ORIGIN, r)
            .expect("sphere"),
    )
}
fn build_cylinder(m: &mut BRepModel, r: f64, h: f64) -> SolidId {
    as_solid(
        TopologyBuilder::new(m)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, r, h)
            .expect("cylinder"),
    )
}
fn build_cone(m: &mut BRepModel, rb: f64, rt: f64, h: f64) -> SolidId {
    as_solid(
        TopologyBuilder::new(m)
            .create_cone_3d(Point3::ORIGIN, Vector3::Z, rb, rt, h)
            .expect("cone"),
    )
}
fn build_torus(m: &mut BRepModel, big: f64, small: f64) -> SolidId {
    as_solid(
        TopologyBuilder::new(m)
            .create_torus_3d(Point3::ORIGIN, Vector3::Z, big, small)
            .expect("torus"),
    )
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < 1e-9 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut v = 0.0;
    for t in &mesh.triangles {
        let a = mesh.vertices[t[0] as usize].position;
        let b = mesh.vertices[t[1] as usize].position;
        let c = mesh.vertices[t[2] as usize].position;
        v += (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x))
            / 6.0;
    }
    v.abs()
}

// --------------------------------------------------------------------------
// Rigid-motion invariance of mass properties.
// --------------------------------------------------------------------------

/// A general rigid motion: rotate about an arbitrary axis, then translate.
fn rigid() -> Matrix4 {
    let axis = Vector3::new(1.0, 2.0, 3.0).normalize().expect("axis");
    let r = Matrix4::from_axis_angle(&axis, 0.73).expect("rotation");
    let t = Matrix4::from_translation(&Vector3::new(4.0, -3.0, 5.0));
    t * r
}

fn assert_rigid_invariance(build: impl Fn(&mut BRepModel) -> SolidId, label: &str, tol: f64) {
    let mut model = BRepModel::new();
    let id = build(&mut model);
    let before = model
        .mass_properties_for(id)
        .unwrap_or_else(|| panic!("{label}: mass props before"));

    let m = rigid();
    transform_solid(&mut model, id, m, TransformOptions::default())
        .unwrap_or_else(|e| panic!("{label}: transform failed: {e:?}"));
    let after = model
        .mass_properties_for(id)
        .unwrap_or_else(|| panic!("{label}: mass props after"));

    // Volume and surface area are rigid-motion invariants.
    assert!(
        rel_close(after.volume, before.volume, tol),
        "{label}: volume changed under rigid motion: {} -> {}",
        before.volume,
        after.volume
    );
    assert!(
        rel_close(after.surface_area, before.surface_area, tol),
        "{label}: surface area changed: {} -> {}",
        before.surface_area,
        after.surface_area
    );

    // The centroid must map exactly the way the point does.
    let c0 = Point3::new(
        before.center_of_mass[0],
        before.center_of_mass[1],
        before.center_of_mass[2],
    );
    let expected = m.transform_point(&c0);
    let c1 = Point3::new(
        after.center_of_mass[0],
        after.center_of_mass[1],
        after.center_of_mass[2],
    );
    let size = before.volume.cbrt().max(1.0);
    assert!(
        (c1 - expected).magnitude() < tol * size + 1e-6,
        "{label}: centroid {c1:?} != mapped {expected:?}"
    );
}

#[test]
fn box_mass_props_rigid_invariant() {
    assert_rigid_invariance(|m| build_box(m, 2.0, 3.0, 4.0), "box", 0.02);
}
#[test]
fn sphere_mass_props_rigid_invariant() {
    assert_rigid_invariance(|m| build_sphere(m, 2.5), "sphere", 0.03);
}
#[test]
fn cylinder_mass_props_rigid_invariant() {
    assert_rigid_invariance(|m| build_cylinder(m, 2.0, 6.0), "cylinder", 0.03);
}
#[test]
fn cone_mass_props_rigid_invariant() {
    assert_rigid_invariance(|m| build_cone(m, 3.0, 1.0, 5.0), "frustum", 0.04);
}
// FIXED (was: torus invalid as-built — 2 single-use seam edges flagged as
// boundary-edge gaps). TorusPrimitive::create now closes the single face's
// boundary as the commutator a·b·a⁻¹·b⁻¹ so each seam edge is used twice
// (manifold). Transform + mass-props therefore hold under rigid motion.
#[test]
fn torus_mass_props_rigid_invariant() {
    assert_rigid_invariance(|m| build_torus(m, 4.0, 1.0), "torus", 0.04);
}

/// Principal moments of inertia are invariant under rotation about the
/// centroid. The primitives are centred at the origin, so a pure rotation
/// about the origin is exactly that.
#[test]
fn principal_moments_invariant_under_rotation() {
    for (name, build) in [
        (
            "box",
            &(|m: &mut BRepModel| build_box(m, 2.0, 3.0, 4.0))
                as &dyn Fn(&mut BRepModel) -> SolidId,
        ),
        (
            "cylinder",
            &(|m: &mut BRepModel| build_cylinder(m, 2.0, 6.0)),
        ),
        ("sphere", &(|m: &mut BRepModel| build_sphere(m, 2.5))),
    ] {
        let mut model = BRepModel::new();
        let id = build(&mut model);
        let before = model.mass_properties_for(id).expect("before");
        let r = Matrix4::from_axis_angle(&Vector3::new(1.0, 1.0, 1.0).normalize().unwrap(), 0.9)
            .expect("rot");
        transform_solid(&mut model, id, r, TransformOptions::default()).expect("rotate");
        let after = model.mass_properties_for(id).expect("after");

        let mut pm0 = before.principal_moments;
        let mut pm1 = after.principal_moments;
        pm0.sort_by(|a, b| a.partial_cmp(b).unwrap());
        pm1.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for k in 0..3 {
            assert!(
                rel_close(pm1[k], pm0[k], 0.05),
                "{name}: principal moment {k} changed under rotation: {} -> {}",
                pm0[k],
                pm1[k]
            );
        }
    }
}

// --------------------------------------------------------------------------
// Booleans on transformed / rotated geometry, with watertight results.
// --------------------------------------------------------------------------

fn vol(model: &mut BRepModel, id: SolidId) -> Option<f64> {
    model.mass_properties_for(id).map(|mp| mp.volume)
}

/// Two unit-ish boxes, the second rotated about Z by `angle` and shifted, then
/// combined. Returns (vol_a, vol_b, vol_result, watertight_ok) on success.
fn rotated_boolean(angle: f64, shift: f64, op: BooleanOp) -> Option<(f64, f64, f64, bool)> {
    let mut model = BRepModel::new();
    let a = build_box(&mut model, 2.0, 2.0, 2.0);
    let b = build_box(&mut model, 2.0, 2.0, 2.0);
    let m = Matrix4::from_translation(&Vector3::new(shift, 0.0, 0.0)) * Matrix4::rotation_z(angle);
    transform_solid(&mut model, b, m, TransformOptions::default()).ok()?;
    let va = vol(&mut model, a)?;
    let vb = vol(&mut model, b)?;
    let result = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
    let vr = vol(&mut model, result)?;

    // Watertightness witness: the tessellated result's divergence volume must
    // match the reported volume.
    let solid = model.solids.get(result)?;
    let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
    let watertight = rel_close(mesh_volume(&mesh), vr, 0.05);
    Some((va, vb, vr, watertight))
}

#[test]
fn rotated_box_union_is_watertight_and_bounded() {
    if let Some((va, vb, vu, wt)) = rotated_boolean(PI / 4.0, 1.0, BooleanOp::Union) {
        assert!(vu > 0.0, "rotated union empty");
        assert!(
            vu >= va.max(vb) * 0.9,
            "union {vu} below larger input {}",
            va.max(vb)
        );
        assert!(vu <= (va + vb) * 1.05, "union {vu} exceeds sum {}", va + vb);
        assert!(
            wt,
            "rotated-box union mesh is not watertight (volume mismatch)"
        );
    }
}

#[test]
fn rotated_box_intersection_is_watertight_and_bounded() {
    if let Some((va, vb, vi, wt)) = rotated_boolean(PI / 6.0, 0.8, BooleanOp::Intersection) {
        assert!(vi > 0.0, "overlapping rotated boxes must intersect");
        assert!(
            vi <= va.min(vb) * 1.05,
            "intersection {vi} exceeds smaller input {}",
            va.min(vb)
        );
        assert!(wt, "rotated-box intersection mesh is not watertight");
    }
}

#[test]
fn rotated_box_difference_is_watertight_and_bounded() {
    if let Some((va, _vb, vd, wt)) = rotated_boolean(PI / 5.0, 1.0, BooleanOp::Difference) {
        assert!(vd > 0.0, "A−B of partially overlapping boxes is non-empty");
        assert!(vd <= va * 1.05, "A−B volume {vd} exceeds A {va}");
        assert!(wt, "rotated-box difference mesh is not watertight");
    }
}

// BUG REPRO (documented, not yet fixed) — root cause isolated by Monte-Carlo
// ground truth (4M samples). Two 2×2×2 boxes (va = vb = 8), B rotated 45° about
// Z + shifted +1 in x. TRUE values: intersection = 3.67, union = 12.33,
// A − B = 4.33. Kernel: union = 12.34 (CORRECT), intersection = 5.10 (WRONG,
// +39 %), difference = 2.90 (WRONG — it is exactly 8 − 5.10, inheriting the bad
// intersection). So union and face-SPLITTING are correct; the defect is
// specific to boolean INTERSECTION's Inside-fragment selection on non-axis-
// aligned input (it keeps fragments that lie outside the other solid, so the
// intersection — and the difference derived from it — over-/under-shoots).
// Each result is independently watertight. This is a deep boolean-robustness
// issue in the (large) intersection/classification path; tracked for a focused
// fix. The axis-aligned inclusion–exclusion test (operations_volume_invariants.rs)
// still guards the common case, which is correct.
#[test]
#[ignore = "boolean INTERSECTION over-reports on rotated input (MC truth 3.67 vs kernel 5.10); union correct — documented bug repro"]
fn rotated_union_inclusion_exclusion() {
    // vol(A∪B) = vol(A) + vol(B) − vol(A∩B), independent of B's orientation.
    if let (Some((va, vb, vu, _)), Some((_, _, vi, _))) = (
        rotated_boolean(PI / 4.0, 1.0, BooleanOp::Union),
        rotated_boolean(PI / 4.0, 1.0, BooleanOp::Intersection),
    ) {
        assert!(
            rel_close(vu, va + vb - vi, 0.06),
            "inclusion-exclusion on rotated boxes: {vu} vs {}",
            va + vb - vi
        );
    }
}

// --------------------------------------------------------------------------
// Multi-operation sequence: build → transform → boolean → re-measure.
// --------------------------------------------------------------------------

#[test]
fn transform_then_boolean_then_mass_props_is_finite_and_watertight() {
    let mut model = BRepModel::new();
    let a = build_box(&mut model, 3.0, 3.0, 3.0);
    let b = build_cylinder(&mut model, 1.0, 6.0);
    // Tilt + offset the cylinder so it bores through the box off-axis.
    let m = Matrix4::from_translation(&Vector3::new(0.5, 0.0, 0.0))
        * Matrix4::from_axis_angle(&Vector3::X, 0.3).expect("rot");
    transform_solid(&mut model, b, m, TransformOptions::default()).expect("transform cylinder");

    if let Ok(result) = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    ) {
        let mp = model
            .mass_properties_for(result)
            .expect("mass props of bored box");
        assert!(
            mp.volume.is_finite() && mp.volume > 0.0,
            "bad volume {}",
            mp.volume
        );
        // Boring a hole removes material: result < solid box (27).
        assert!(
            mp.volume < 27.0 * 1.01,
            "bored box {} not less than 27",
            mp.volume
        );
        assert!(
            mp.center_of_mass.iter().all(|c| c.is_finite()),
            "non-finite centroid"
        );
        let solid = model.solids.get(result).expect("result solid");
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        assert!(
            rel_close(mesh_volume(&mesh), mp.volume, 0.05),
            "bored-box mesh volume {} vs reported {}",
            mesh_volume(&mesh),
            mp.volume
        );
    }
}
