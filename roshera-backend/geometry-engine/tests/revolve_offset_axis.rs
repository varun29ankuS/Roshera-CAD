//! REVOLVE OFFSET-AXIS gate.
//!
//! A `revolve` about a NON-ORIGIN axis (`axis_origin != [0,0,0]`) used to leak
//! the axis offset into the solid's radius/bbox: the `(r, z)` meridian (r =
//! radius FROM the axis, z = height ALONG it) was placed at the WORLD point
//! `(r, 0, z)` regardless of where the axis sat, so `r` was measured from the
//! world origin instead of the axis. A nozzle of max r=14.4 about
//! `axis_origin=[30,0,0]` reported a Ø88.68 bbox (= 2·(30+14.34)) instead of a
//! Ø28.8 solid centred at x=30.
//!
//! FIXED by lifting `(r, z)` into the AXIS FRAME — `axis_origin + z·â + r·ê1`
//! with `ê1` the canonical radial reference (matching `Arc::new`'s x_axis, so
//! the default `+Z`-through-origin axis is byte-identical to before). The axis
//! origin now translates the whole solid; `r` is a pure radius.
//!
//! These gates pin: (1) an offset-axis cylinder lands at the right place with
//! the right radius and is watertight; (2) the nozzle profile about an offset
//! axis has the correct Ø and centre; (3) the default-axis revolve is UNCHANGED.
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::revolve::{revolve_meridian, RevolveOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

/// Tessellated world bbox `(lo, hi)` of a solid.
fn bbox(m: &BRepModel, sid: SolidId) -> ([f64; 3], [f64; 3]) {
    let solid = m.solids.get(sid).expect("solid present");
    let mesh = tessellate_solid(solid, m, &TessellationParams::default());
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &mesh.vertices {
        let p = [v.position.x, v.position.y, v.position.z];
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn revolve(prof: &[(f64, f64)], axis_origin: [f64; 3], axis_dir: [f64; 3]) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let opts = RevolveOptions {
        axis_origin: Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
        axis_direction: Vector3::new(axis_dir[0], axis_dir[1], axis_dir[2]),
        angle: std::f64::consts::TAU,
        segments: 96,
        ..Default::default()
    };
    let sid = revolve_meridian(&mut m, prof, opts).expect("revolve_meridian");
    (m, sid)
}

fn assert_watertight(m: &BRepModel, sid: SolidId, label: &str) {
    let v = validate_solid_scoped(m, sid, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "{label}: B-Rep invalid: {:?}", v.errors);
    let rep = manifold_report(m, sid, 0.1, 1e-6)
        .unwrap_or_else(|| panic!("{label}: manifold_report none"));
    assert_eq!(
        (rep.boundary_edges, rep.nonmanifold_edges),
        (0, 0),
        "{label}: not watertight (open={}, nm={})",
        rep.boundary_edges,
        rep.nonmanifold_edges
    );
}

fn approx(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: expected {b}, got {a} (|Δ|={} > {tol})",
        (a - b).abs()
    );
}

/// (1) SIMPLE: cylinder r=5, h=10 about axis_origin=[20,0,0], dir +Z.
/// Geometry must sit at x=20±5 (NOT Ø50 about the origin), watertight.
#[test]
fn offset_axis_cylinder_x20_r5() {
    // Closed meridian: outer wall (r=5) + caps back to the axis (r=0).
    let prof = vec![(5.0, 0.0), (5.0, 10.0), (0.0, 10.0), (0.0, 0.0)];
    let (m, sid) = revolve(&prof, [20.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    assert_watertight(&m, sid, "offset cylinder");
    let (lo, hi) = bbox(&m, sid);
    // x ∈ [15, 25], y ∈ [-5, 5], z ∈ [0, 10] — radius 5 about (20,0), NOT 20.
    approx(lo[0], 15.0, 0.05, "cyl lo.x");
    approx(hi[0], 25.0, 0.05, "cyl hi.x");
    approx(lo[1], -5.0, 0.05, "cyl lo.y");
    approx(hi[1], 5.0, 0.05, "cyl hi.y");
    approx(lo[2], 0.0, 1e-6, "cyl lo.z");
    approx(hi[2], 10.0, 1e-6, "cyl hi.z");
    // Diameter is Ø10 in BOTH radial axes (sanity against the old Ø50).
    approx(hi[0] - lo[0], 10.0, 0.1, "cyl Øx");
    approx(hi[1] - lo[1], 10.0, 0.1, "cyl Øy");
}

/// (2) Nozzle-like profile (max r=14.4) about axis_origin=[30,0,0].
/// Must be a Ø~28.8 solid centred at x=30 (x ∈ [15.6, 44.4]), watertight —
/// NOT the old Ø88.68.
#[test]
fn offset_axis_nozzle_x30() {
    let rmax = 14.4_f64;
    // A simple closed nozzle-ish meridian: throat r=6, exit r=14.4, plus the
    // axis return for a solid body.
    let prof = vec![
        (6.0, 0.0),
        (rmax, 8.0),
        (rmax, 20.0),
        (0.0, 20.0),
        (0.0, 0.0),
    ];
    let (m, sid) = revolve(&prof, [30.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    assert_watertight(&m, sid, "offset nozzle");
    let (lo, hi) = bbox(&m, sid);
    // Radial extent = rmax about x=30 ⇒ x ∈ [30-14.4, 30+14.4], Ø ≈ 28.8.
    approx(lo[0], 30.0 - rmax, 0.1, "nozzle lo.x");
    approx(hi[0], 30.0 + rmax, 0.1, "nozzle hi.x");
    approx(hi[0] - lo[0], 2.0 * rmax, 0.2, "nozzle Øx");
    approx(hi[1] - lo[1], 2.0 * rmax, 0.2, "nozzle Øy");
    // Centre at x=30 (NOT shifted toward the origin).
    approx((lo[0] + hi[0]) * 0.5, 30.0, 0.1, "nozzle centre.x");
}

/// (3) DEFAULT-AXIS revolve UNCHANGED: the same cylinder about the origin is a
/// Ø10 solid centred at the origin.
#[test]
fn default_axis_cylinder_unchanged() {
    let prof = vec![(5.0, 0.0), (5.0, 10.0), (0.0, 10.0), (0.0, 0.0)];
    let (m, sid) = revolve(&prof, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    assert_watertight(&m, sid, "default cylinder");
    let (lo, hi) = bbox(&m, sid);
    approx(lo[0], -5.0, 0.05, "def lo.x");
    approx(hi[0], 5.0, 0.05, "def hi.x");
    approx(lo[1], -5.0, 0.05, "def lo.y");
    approx(hi[1], 5.0, 0.05, "def hi.y");
    approx(lo[2], 0.0, 1e-6, "def lo.z");
    approx(hi[2], 10.0, 1e-6, "def hi.z");
}

/// (4) Offset cylinder about a NON-Z axis (axis_origin=[0,10,0], dir +X): the
/// `(r,z)` meridian rides the X axis, so the cylinder runs ALONG +X at y=10.
#[test]
fn offset_axis_nonz_direction() {
    let prof = vec![(5.0, 0.0), (5.0, 10.0), (0.0, 10.0), (0.0, 0.0)];
    let (m, sid) = revolve(&prof, [0.0, 10.0, 0.0], [1.0, 0.0, 0.0]);
    assert_watertight(&m, sid, "non-z cylinder");
    let (lo, hi) = bbox(&m, sid);
    // Axis is +X through (0,10,0): height z∈[0,10] maps ALONG x ⇒ x∈[0,10];
    // radius 5 about the axis ⇒ y∈[5,15], z∈[-5,5].
    approx(lo[0], 0.0, 0.05, "nz lo.x");
    approx(hi[0], 10.0, 0.05, "nz hi.x");
    approx(lo[1], 5.0, 0.1, "nz lo.y");
    approx(hi[1], 15.0, 0.1, "nz hi.y");
    approx(lo[2], -5.0, 0.1, "nz lo.z");
    approx(hi[2], 5.0, 0.1, "nz hi.z");
}

/// (5) The editable meridian round-trips: a part built about an offset axis
/// reads back its ORIGINAL `(r, z)` profile (the #25 edit→regenerate loop).
#[test]
fn offset_axis_meridian_roundtrip() {
    use geometry_engine::operations::revolve::get_revolve_meridian;
    let prof = vec![(5.0, 0.0), (5.0, 10.0), (2.0, 10.0), (2.0, 0.0)];
    let (m, sid) = revolve(&prof, [30.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let back = get_revolve_meridian(&m, sid).expect("meridian recovered");
    assert_eq!(back.len(), prof.len(), "meridian point count");
    for (i, (&(r, z), &(rb, zb))) in prof.iter().zip(back.iter()).enumerate() {
        approx(rb, r, 1e-6, &format!("meridian[{i}].r"));
        approx(zb, z, 1e-6, &format!("meridian[{i}].z"));
    }
}

/// (6) The retained construction geometry of an offset-axis revolve is
/// CO-LOCATED with the solid, so the certificate stays sound (the lifted
/// profile points ride the axis instead of being stranded near the world
/// origin — the orphaned-sketch failure the consistency invariant catches).
#[test]
fn offset_axis_construction_consistent() {
    use geometry_engine::primitives::provenance::ConstructionConsistency;
    let prof = vec![(5.0, 0.0), (5.0, 10.0), (2.0, 10.0), (2.0, 0.0)];
    let (m, sid) = revolve(&prof, [30.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    assert_eq!(
        m.construction_consistency(sid),
        ConstructionConsistency::Consistent,
        "offset-axis revolve construction geometry must co-locate with the solid"
    );
}
