// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! CAPABILITY PROBE — LOFT, SWEEP, LINEAR PATTERN, driven directly against the
//! kernel API (`geometry_engine::operations::*`, `TopologyBuilder`), bypassing
//! the REST layer entirely. These three capabilities were never fairly
//! measured before: an earlier probe went through the REST layer and a bug
//! there poisoned the results. This file is a MEASUREMENT, not a fix — every
//! case reports what is actually true of the kernel today. A case that fails
//! its stated tolerance is pinned `#[ignore]` with the precise measured
//! deviation and root cause, assertions left INTACT (the `boolean_multibody.rs`
//! precedent), so the suite stays green and the red flips live the day the
//! defect is fixed. No production code is touched by this file.
//!
//! Construction idioms are lifted directly from existing GREEN tests so this
//! probe measures the kernel, not payload mistakes:
//!   * `circle()` / `line_edge()` — `tests/loft_validity_invariants.rs`,
//!     `tests/blend_weld_stress.rs` (self-closing `Circle` edge profile).
//!   * `mesh_volume()` / `rel_close()` — `tests/sweep_volume_invariants.rs`
//!     (divergence-theorem tessellated volume vs. an analytic closed form).
//!   * `make_box` / `make_cylinder` / `describe`-style certificate probing —
//!     `tests/boolean_multibody.rs`.
//!   * box-face pattern harness — `tests/persistent_id_pattern.rs`.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::loft::LoftType;
use geometry_engine::operations::{
    boolean_operation, create_pattern, loft_profiles, sweep_profile, BooleanOp, BooleanOptions,
    CommonOptions, LoftOptions, PatternOptions, PatternType, SweepOptions,
};
use geometry_engine::primitives::curve::{Circle, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::shell::{Shell, ShellType};
use geometry_engine::primitives::solid::{Solid, SolidId};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

// ---------------------------------------------------------------------------
// Shared helpers (lifted from the green tests named above)
// ---------------------------------------------------------------------------

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < 1e-9 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

/// Divergence-theorem volume of a (watertight) tessellated solid:
/// `Σ (a · (b × c)) / 6` over triangles. Lifted verbatim from
/// `sweep_volume_invariants.rs`.
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

fn line_edge(m: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let pa = m.vertices.get(a).expect("start vertex").position;
    let pb = m.vertices.get(b).expect("end vertex").position;
    let cid = m
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    m.edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
}

/// A closed circle of `radius` in the plane `z = center.z`, expressed as ONE
/// self-closing `Circle` edge — the idiom `loft_validity_invariants.rs` and
/// `blend_weld_stress.rs` both use for a circular profile. This is exactly
/// what an agent would get from a circular sketch fed to loft/sweep.
fn circle(m: &mut BRepModel, center: Point3, radius: f64) -> Vec<EdgeId> {
    let seam = m
        .vertices
        .add_or_find(center.x + radius, center.y, center.z, 1e-6);
    let cid = m.curves.add(Box::new(
        Circle::new(center, Vector3::new(0.0, 0.0, 1.0), radius).expect("circle"),
    ));
    vec![m.edges.add(Edge::new(
        0,
        seam,
        seam,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ))]
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model).create_box_3d(w, h, d) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

fn make_cylinder(model: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model).create_cylinder_3d(base, axis, r, h) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

/// Every face reachable from the solid (outer shell + inner shells). Lifted
/// from `boolean_multibody.rs`.
fn all_faces(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut out = Vec::new();
    if let Some(solid) = model.solids.get(solid_id) {
        for sh in solid.all_shells() {
            if let Some(shell) = model.shells.get(sh) {
                out.extend(shell.faces.iter().copied());
            }
        }
    }
    out
}

// ===========================================================================
// CASE 1 — LOFT: two parallel circular sections -> closed conical frustum.
//
// Entry point: `operations::loft_profiles` (`geometry-engine/src/operations
// /loft.rs`), the only `loft` re-exported from `operations::mod` (`nurbs_loft`
// is a separate, non-re-exported specialisation). LoftType defaults to
// `Linear`: ruled surfaces between corresponding profile vertices.
// ===========================================================================

/// Loft two circles (r0 at z=0, r1 at z=h) into a closed solid via the
/// default `LoftType::Linear`. Result is validated at construction.
fn loft_two_circles(r0: f64, r1: f64, h: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let p0 = circle(&mut m, Point3::new(0.0, 0.0, 0.0), r0);
    let p1 = circle(&mut m, Point3::new(0.0, 0.0, h), r1);
    let opts = LoftOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        loft_type: LoftType::Linear,
        create_solid: true,
        ..Default::default()
    };
    let sid = loft_profiles(&mut m, vec![p0, p1], opts)
        .expect("loft of two well-formed circular profiles must succeed");
    (m, sid)
}

/// MEASURED RED. `loft_profiles` densifies a profile's vertex correspondence
/// to `max(profile_vertex_counts).max(8)` (`loft.rs::densify_correspondence`).
/// A circular profile expressed as a single self-closing edge starts with
/// exactly ONE correspondence vertex (the seam), so BOTH rings densify to the
/// hard floor of 8 — an octagon inscribed in the circle — REGARDLESS of the
/// circle's radius. The two octagons are similar (same 8 angular positions,
/// scaled by r1/r0), so the lofted body is an exact octagonal-pyramid frustum,
/// not an approximation of the circular one: its volume is the circular
/// closed form scaled by the exact area ratio of a regular inscribed octagon
/// to its circle, `(8/2)·sin(2π/8) / π = 2√2/π ≈ 0.900316` — a ~10% shortfall
/// that no amount of loft-side smoothing (`LoftType::Cubic`/`Guided` reuse the
/// same 8-point correspondence) fixes. `operations::nurbs_loft::nurbs_loft`
/// (not re-exported from `operations::mod`, so not "the" loft entry point)
/// takes explicit point-sampled rings and does not hit this floor — see
/// `tests/nurbs_loft.rs`.
///
/// The measured numbers below are printed via `eprintln!` so a future re-run
/// after a fix documents the new deviation without editing this comment.
#[test]
#[ignore = "MEASURED: loft_profiles circular-profile correspondence floors at 8 \
            vertices (densify_correspondence's `.max(8)`), so two circles loft \
            into an OCTAGONAL frustum, not a circular one. Actual run: \
            tess_vol=2469.6883 mass_vol=2469.6883 (mesh and mass-props AGREE, \
            watertight) closed_form=2743.1340 (r0=7, r1=4.5, h=26) -> \
            deviation=9.9684%, matching the exact regular-octagon/circle area \
            ratio 2√2/π=0.900316 to 4 decimals. cert_sound=true -- the solid IS \
            a valid closed manifold, this is a fidelity gap, not a topology \
            defect. Pinned per boolean_multibody.rs precedent; assertions intact."]
fn loft_frustum_r7_to_r45_h26_matches_closed_form_within_2pct() {
    let (mut m, sid) = loft_two_circles(7.0, 4.5, 26.0);

    let cert = m.certify_solid(sid);
    assert!(
        cert.is_sound(),
        "loft of two circles must certify sound: {cert:?}"
    );

    let solid = m.solids.get(sid).expect("lofted solid");
    let mesh = tessellate_solid(solid, &m, &TessellationParams::default());
    let tess_vol = mesh_volume(&mesh);
    let mp = m
        .mass_properties_for(sid)
        .expect("lofted solid mass properties");

    let (r0, r1, h) = (7.0_f64, 4.5_f64, 26.0_f64);
    let expected = std::f64::consts::PI * h / 3.0 * (r0 * r0 + r0 * r1 + r1 * r1);
    let deviation = (tess_vol - expected).abs() / expected;

    eprintln!(
        "[loft r7->r4.5 h26] tess_vol={tess_vol:.4} mass_vol={:.4} \
         closed_form={expected:.4} deviation={:.4}% cert_sound={}",
        mp.volume,
        deviation * 100.0,
        cert.is_sound(),
    );

    assert!(
        rel_close(tess_vol, mp.volume, 0.03),
        "loft mesh volume {tess_vol} vs mass-props {} disagree (non-watertight?)",
        mp.volume,
    );
    assert!(
        deviation <= 0.02,
        "loft frustum volume {tess_vol} vs conical closed form {expected}: \
         deviation {:.4}% exceeds the 2% tolerance",
        deviation * 100.0,
    );
}

// ===========================================================================
// CASE 2 — SWEEP: a circle swept along a straight line -> cylinder.
//
// Entry point: `operations::sweep_profile` (`geometry-engine/src/operations
// /sweep.rs`), `SweepType::Path` (the default), degenerate-simple straight
// path.
// ===========================================================================

fn swept_cylinder(r: f64, length: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let profile = circle(&mut m, Point3::new(0.0, 0.0, 0.0), r);
    let va = m.vertices.add(0.0, 0.0, 0.0);
    let vb = m.vertices.add(0.0, 0.0, length);
    let path = line_edge(&mut m, va, vb);
    let opts = SweepOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let sid = sweep_profile(&mut m, profile, path, opts)
        .expect("sweep of a circular profile along a straight path must succeed");
    (m, sid)
}

/// MEASURED RED (on soundness, not volume). `sweep_profile`'s cap face is the
/// ANALYTIC transformed profile face (its outer loop is still the ONE closed
/// `Circle` edge -- see `pattern::transform_face`/`transform_loop`, which
/// transforms a loop's EXISTING edges rather than resampling them). The first
/// lateral ring, however, comes from `get_section_vertex_ring`, which
/// explicitly discretizes a closed-curve edge into `samples_per_closed_edge`
/// (= 32, hard-coded in `create_sweep_section`) BRAND NEW vertices
/// (`model.vertices.add`, not `add_or_find`) that never coincide by `VertexId`
/// with the cap loop's seam vertex -- an unstitched cap/lateral seam.
///
/// Actual run: volume is FINE -- tess_vol=375.2863 mass_vol=375.3487
/// closed_form=376.9911 (r=2, length=30) -> deviation=0.4522%, well inside the
/// 2% budget. But the certificate is NOT sound: brep_valid=false,
/// watertight=false, manifold_report=(264 boundary edges, 0 non-manifold,
/// closed=false, euler=-6) at coarse-chord tessellation. The seam gap is
/// invisible to a volume oracle (the sampled ring sits exactly ON the circle,
/// so it contributes ~0 leaked/missing volume) and only shows up as an open
/// boundary in the certificate/mesh topology -- exactly the kind of silent-
/// looking defect a REST-layer volume-only probe would miss entirely.
#[test]
#[ignore = "MEASURED: sweep_profile leaves an unstitched cap/lateral seam for \
            CLOSED-CURVE (e.g. circular) profiles -- get_section_vertex_ring \
            mints brand-new vertices for the first/last ring instead of \
            reusing the cap loop's seam vertex (root: pattern::transform_face \
            path). Actual run (r=2, length=30 straight sweep): volume is fine \
            (tess=375.2863 vs closed-form=376.9911, 0.4522% dev, well inside \
            2%) but cert_sound=false: brep_valid=false, watertight=false, \
            manifold_report boundary_edges=264 nonmanifold=0 closed=false \
            euler=-6. A volume-only probe would have called this GREEN; the \
            certificate does not. Pinned per boolean_multibody.rs precedent; \
            assertions intact."]
fn sweep_cylinder_r2_len30_matches_closed_form_within_2pct() {
    let (mut m, sid) = swept_cylinder(2.0, 30.0);

    let cert = m.certify_solid(sid);
    let solid = m.solids.get(sid).expect("swept solid");
    let mesh = tessellate_solid(solid, &m, &TessellationParams::default());
    let tess_vol = mesh_volume(&mesh);
    let mp = m.mass_properties_for(sid);
    let mr = geometry_engine::harness::watertight::manifold_report(&m, sid, 0.02, 1e-6);

    let expected = std::f64::consts::PI * 2.0 * 2.0 * 30.0;
    let deviation = (tess_vol - expected).abs() / expected;

    eprintln!(
        "[sweep r2 len30] tess_vol={tess_vol:.4} mass_vol={:?} closed_form={expected:.4} \
         deviation={:.4}% cert_sound={} brep_valid={} watertight={} manifold_report={:?}",
        mp.as_ref().map(|m| m.volume),
        deviation * 100.0,
        cert.is_sound(),
        cert.brep_valid,
        cert.watertight,
        mr.as_ref()
            .map(|r| (r.boundary_edges, r.nonmanifold_edges, r.closed, r.manifold)),
    );

    assert!(
        cert.is_sound(),
        "sweep of a circular profile along a straight line must certify sound: {cert:?}",
    );
    assert!(
        deviation <= 0.02,
        "swept cylinder volume {tess_vol} vs closed form {expected}: deviation \
         {:.4}% exceeds the 2% tolerance",
        deviation * 100.0,
    );
}

// ===========================================================================
// CASE 3 — LINEAR PATTERN: a small box patterned x4, non-overlapping spacing.
//
// Entry point: `operations::create_pattern` (`geometry-engine/src/operations
// /pattern.rs`). READ CONTRACT: `create_pattern(model, source_features:
// Vec<FaceId>, pattern_type, options) -> OperationResult<Vec<Vec<FaceId>>>`.
// It patterns a FACE SET, not a `SolidId`, and returns `N` groups of
// transformed FACE copies -- it never constructs new `SolidId`s and never
// booleans the copies together. `PatternOptions::merge_results` is declared
// but dead: `grep -rn merge_results src/` shows it is written in every
// constructor and read NOWHERE in `create_pattern_body`. So "4 bodies-worth
// of volume" is not something the API hands back directly; each instance's
// face GROUP has to be wrapped into a `Shell`/`Solid` by the caller (exactly
// as `boolean_multibody.rs`'s `make_box` wraps `TopologyBuilder` output) to
// even ask the question. That wrapping is done here, explicitly, as
// measurement scaffolding -- not something `create_pattern` provides.
// ===========================================================================

/// Wrap a face group (assumed to already form a closed 2-manifold, e.g. every
/// face of a patterned box instance) into a fresh `Solid` so its volume /
/// certificate can be queried. This is test-only scaffolding: `create_pattern`
/// itself never does this.
fn solid_from_face_group(m: &mut BRepModel, faces: &[FaceId]) -> SolidId {
    let mut shell = Shell::new(0, ShellType::Closed);
    for &f in faces {
        shell.add_face(f);
    }
    let shell_id = m.shells.add(shell);
    m.solids.add(Solid::new(0, shell_id))
}

/// MEASURED RED. `create_pattern_instance` calls `transform_face` once PER
/// FACE, independently; `transform_face` -> `transform_loop` builds a fresh
/// `vmap: HashMap<VertexId, VertexId>` LOCAL to that one call, so it dedups
/// corners WITHIN a face's own loop but never ACROSS two different faces --
/// two box faces meeting at a physical edge each mint their OWN private copy
/// of that edge (a different `EdgeId`, coincidentally equal endpoints).
/// `merge_pattern_geometry` (run because `PatternOptions::default()` sets
/// `merge_geometry: true`) then does a vertex-coincidence pass, but its own
/// doc comment says it plainly: "Edges and faces themselves are not
/// deduplicated here." So after merge, the two faces' private edges share
/// canonical VERTEX ids but remain two distinct `EdgeId`s, each referenced by
/// only ONE face's loop -- exactly what a whole-model B-Rep connectivity
/// check reports as an open boundary.
///
/// Actual run: instance 0 (the untouched seed) certifies sound. Instance 1 (a
/// transformed copy, wrapped in a fresh `Shell`/`Solid` by this test since
/// `create_pattern` itself never does that -- see the CASE 3 header) does
/// NOT: `ValidityCertificate { brep_valid: false, watertight: true,
/// manifold: true, euler_characteristic: 2, boundary_edges: 0, ... errors:
/// [24 x ConnectivityError "Boundary edge N detected - potential gap in
/// topology"] }` -- 24 errors = the box's 12 physical edges, each flagged
/// from BOTH adjacent faces' private copy. Volume is exactly right (96.0,
/// matching the seed) and the MESH-level watertight/manifold flags are both
/// true -- only the strict B-Rep half-edge accounting catches it. This
/// confirms the CASE 3 header's contract reading: `create_pattern` hands back
/// face copies, not a welded solid, and a caller assembling one downstream
/// needs an edge-welding pass the API does not provide.
#[test]
#[ignore = "MEASURED: create_pattern's per-face transform_loop mints private, \
            un-shared edges at every face boundary (merge_pattern_geometry \
            merges VERTEX coincidence only -- its own doc comment says so). \
            Wrapping a patterned box instance's 6 copied faces in a fresh \
            Shell/Solid (test-only scaffolding; create_pattern never does \
            this) certifies UNSOUND: brep_valid=false with 24 \
            ConnectivityError 'Boundary edge N' entries (the box's 12 \
            physical edges, each flagged twice, once per adjacent face's \
            private copy), even though volume=96.0 (exact) and the mesh-level \
            watertight/manifold flags are both true. Instance 0 (untouched \
            seed) is unaffected and certifies sound. Pinned per \
            boolean_multibody.rs precedent; assertions intact."]
fn linear_pattern_box_x4_contract_is_four_disjoint_face_groups() {
    let (w, h, d) = (4.0_f64, 4.0_f64, 6.0_f64);
    let spacing = 10.0_f64; // > w: instances cannot touch along the pattern axis.
    let count = 4_u32;

    let mut m = BRepModel::new();
    let seed_solid = make_box(&mut m, w, h, d);
    let seed_faces = all_faces(&m, seed_solid);
    assert_eq!(seed_faces.len(), 6, "box has 6 faces");

    let instances = create_pattern(
        &mut m,
        seed_faces.clone(),
        PatternType::Linear {
            direction: Vector3::X,
            spacing,
            count,
        },
        PatternOptions::default(),
    )
    .expect("linear pattern of a box's 6 faces must succeed");

    // CONTRACT: N groups, including the untouched seed as group 0.
    assert_eq!(
        instances.len(),
        count as usize,
        "create_pattern returns one group per requested count (incl. seed)",
    );
    assert_eq!(
        instances[0], seed_faces,
        "instance 0 IS the seed's own faces, byte-for-byte -- no copy made",
    );

    let box_volume = w * h * d;
    let mut total_volume = 0.0_f64;

    for (i, group) in instances.iter().enumerate() {
        assert_eq!(
            group.len(),
            6,
            "instance {i}: every copy carries the full 6-face box, not a subset",
        );
        let sid = if i == 0 {
            seed_solid
        } else {
            solid_from_face_group(&mut m, group)
        };
        let cert = m.certify_solid(sid);
        let vol = m
            .calculate_solid_volume(sid)
            .unwrap_or_else(|| panic!("instance {i}: no volume computed"));
        eprintln!(
            "[pattern instance {i}] volume={vol:.6} cert_sound={} brep_valid={} \
             watertight={}",
            cert.is_sound(),
            cert.brep_valid,
            cert.watertight,
        );
        assert!(
            cert.is_sound(),
            "instance {i}: patterned face group does not certify as a sound solid: {cert:?}",
        );
        assert!(
            rel_close(vol, box_volume, 1e-6),
            "instance {i}: volume {vol} should equal the seed box volume {box_volume} \
             (pure translation, no deformation)",
        );
        total_volume += vol;
    }

    let expected_total = count as f64 * box_volume;
    assert!(
        rel_close(total_volume, expected_total, 1e-6),
        "4 non-overlapping instances should sum to 4x the seed volume: \
         {total_volume} vs {expected_total}",
    );
}

// ===========================================================================
// CASE 4 — CHAINED: loft -> single through-bore at a distinct position ->
// certificate still sound.
// ===========================================================================

/// The loft body from CASE 1 (an octagonal-frustum solid, r0=7 at z=0, r1=4.5
/// at z=26 -- see that case's note on why it is octagonal, not circular) minus
/// a Ø2 through-bore offset from the lateral axis, entirely inside the
/// footprint at every z (so the cutter never exits sideways through the
/// tapering wall). The volume fidelity gap measured in CASE 1 is orthogonal
/// to this case: this only asks whether a boolean chained onto a loft result
/// keeps the certificate sound.
#[test]
fn loft_then_offset_bore_stays_sound() {
    let (mut m, loft_id) = loft_two_circles(7.0, 4.5, 26.0);
    let pre_cert = m.certify_solid(loft_id);
    assert!(
        pre_cert.is_sound(),
        "precondition: the loft body itself must be sound before boring: {pre_cert:?}",
    );
    // Captured BEFORE the boolean op: `loft_id` may not remain a valid solid
    // reference afterward (Difference can retire/replace the LHS solid id).
    let loft_v = m
        .mass_properties_for(loft_id)
        .map(|mp| mp.volume)
        .unwrap_or(f64::NAN);

    // Bore at x=2, y=0 (well inside the r=4.5 top radius, the tightest
    // cross-section), running the full Z extent plus generous overshoot on
    // both ends so it is a genuine THROUGH bore, not a blind pocket.
    let bore = make_cylinder(
        &mut m,
        Point3::new(2.0, 0.0, -5.0),
        Vector3::new(0.0, 0.0, 1.0),
        1.0,
        36.0,
    );

    let bored = boolean_operation(
        &mut m,
        loft_id,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-bore of a lofted frustum must succeed");

    let cert = m.certify_solid(bored);
    let mr = geometry_engine::harness::watertight::manifold_report(&m, bored, 0.02, 1e-6);
    eprintln!(
        "[loft+bore] cert_sound={} brep_valid={} watertight={} manifold={} \
         manifold_report={:?}",
        cert.is_sound(),
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        mr.as_ref()
            .map(|r| (r.boundary_edges, r.nonmanifold_edges, r.closed, r.manifold)),
    );

    assert!(
        cert.is_sound(),
        "loft -> through-bore chain must certify sound: {cert:?}",
    );

    let v = m
        .calculate_solid_volume(bored)
        .expect("bored loft body has a volume");
    assert!(
        v < loft_v || loft_v.is_nan(),
        "boring must not raise the volume: bored={v} loft={loft_v}",
    );
}
