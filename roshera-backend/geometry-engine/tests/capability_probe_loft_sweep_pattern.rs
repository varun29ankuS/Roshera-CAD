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
use geometry_engine::primitives::shell::{Shell, ShellId, ShellType};
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

/// WAS A MEASURED RED — NOW GREEN. `loft_profiles` used to densify a profile's
/// vertex correspondence to `max(profile_vertex_counts).max(8)`
/// (`loft.rs::densify_correspondence`). A circular profile expressed as a
/// single self-closing edge starts with exactly ONE correspondence vertex (the
/// seam), so BOTH rings densified to the hard floor of 8 — an octagon inscribed
/// in the circle — REGARDLESS of the circle's radius. The two octagons were
/// similar (same 8 angular positions, scaled by r1/r0), so the lofted body was
/// an exact octagonal-pyramid frustum, not an approximation of the circular
/// one: its volume was the circular closed form scaled by the exact area ratio
/// of a regular inscribed octagon to its circle,
/// `(8/2)·sin(2π/8) / π = 2√2/π ≈ 0.900316` — a ~10% shortfall that no amount
/// of loft-side smoothing (`LoftType::Cubic`/`Guided` reuse the same
/// correspondence) fixed. Measured then: tess_vol=2469.6883 vs
/// closed_form=2743.1340 → deviation=9.9684%, with cert_sound=true throughout:
/// a genuinely closed manifold of the wrong shape, which is exactly why the
/// certificate could not see it.
///
/// FIXED by making the ring density chord-sag driven instead of a constant:
/// `densify_correspondence` now takes the max over profiles of the per-edge
/// sagitta count (`cos(θ/2) = 1 − h/r`, the same rule as
/// `tessellation::surface::arc_steps_for_quality`) against the operation's
/// chord budget, floored at the historical 8 for straight-edged profiles and
/// capped at `TessellationParams::default().max_segments`. r=7 → 59 chords,
/// r=4.5 → 48, so this loft rings at 59: tess_vol=2737.9519
/// mass_vol=2737.9519 closed_form=2743.1340 → deviation=0.1889%, matching the
/// 59-gon/circle area ratio. Straight-edged profiles are byte-identical to
/// before (a line edge needs exactly one chord).
///
/// The measured numbers are printed via `eprintln!` so a future re-run
/// documents the then-current deviation without editing this comment.
#[test]
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

/// WAS RED, NOW GREEN (fixed in `operations/sweep.rs`; this case was pinned
/// `#[ignore]` and the ignore is now removed, assertions intact + strengthened).
///
/// The defect: `sweep_profile`'s cap face was the ANALYTIC transformed profile
/// face (its outer loop still the ONE closed `Circle` edge -- see
/// `pattern::transform_face`/`transform_loop`, which transform a loop's
/// EXISTING edges rather than resampling them), while the lateral rings came
/// from `get_section_vertex_ring`, which discretizes a closed-curve edge into
/// `samples_per_closed_edge` (= 32, hard-coded in `create_sweep_section`) BRAND
/// NEW vertices (`model.vertices.add`, not `add_or_find`). For a closed profile
/// the caps and the lateral panels therefore shared NO topology at all: an
/// unstitched cap/lateral seam all the way round both ends. It measured as
/// cert_sound=false (brep_valid=false, watertight=false, manifold_report 264
/// boundary edges, closed=false, euler=-6) while the VOLUME looked fine
/// (tess=375.2863 vs closed-form=376.9911, 0.4522% dev) -- the sampled ring
/// sits exactly ON the circle, so the gap leaks ~0 volume. A volume-only probe
/// would have called this GREEN; the certificate did not.
///
/// The fix (`sweep::weld_section_face_to_ring`) rebuilds the cap face's outer
/// loop as the polygon through that same 32-vertex ring, taking each segment
/// from `create_or_find_edge` so the cap's edges ARE the adjacent lateral
/// quads' edges; the scratch sections that never reach the shell are dropped,
/// and `require_closed_sweep_shell` now refuses (typed, with rollback) rather
/// than emitting an open shell. The cap is a 32-gon afterwards, matching the
/// bilinear lateral facets it has to weld to -- so the residual deviation is
/// faceting fidelity, not a topology defect, and it stays well inside the 2%
/// budget asserted below. The runtime numbers are printed by the `eprintln!`
/// so a re-run documents them without editing this comment; the run that
/// flipped this case green measured tess_vol=374.5586 mass_vol=374.5586 (mesh
/// and mass-props now agree to every printed digit — they disagreed before)
/// vs closed_form=376.9911 -> deviation=0.6452%, which is the exact inscribed-
/// 32-gon/circle area ratio 16·sin(π/16)/π = 0.993570 (predicting 0.6430%) to
/// within 2.2e-5 relative — i.e. the whole residual IS the 32-facet cap and
/// lateral discretization, nothing else is leaking. Certificate on that run:
/// cert_sound=true brep_valid=true watertight=true, manifold_report
/// (boundary_edges=0, nonmanifold=0, closed=true, manifold=true).
#[test]
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

    // The three numbers the old `#[ignore]` reason recorded as broken, pinned
    // directly so a regression cannot hide behind an aggregate `is_sound()`:
    // the cap/lateral seam either exists in the mesh or it does not.
    let mr = mr.expect("a swept solid must tessellate to a non-empty mesh");
    assert_eq!(
        mr.boundary_edges, 0,
        "welded sweep must leave NO open mesh boundary (was 264 before the \
         cap/lateral weld): {mr:?}",
    );
    assert!(
        mr.closed,
        "welded sweep mesh must be closed (was closed=false, euler=-6): {mr:?}",
    );
    assert!(mr.manifold, "welded sweep mesh must be manifold: {mr:?}",);
}

// ===========================================================================
// CASE 3 — LINEAR PATTERN: a small box patterned x4, non-overlapping spacing.
//
// Entry point: `operations::create_pattern` (`geometry-engine/src/operations
// /pattern.rs`). READ CONTRACT: `create_pattern(model, source_features:
// Vec<FaceId>, pattern_type, options) -> OperationResult<Vec<Vec<FaceId>>>`.
// It patterns a FACE SET, not a `SolidId`, and returns `N` groups of
// transformed FACE copies -- it never constructs new `SolidId`s and never
// booleans the copies together. (`PatternOptions::merge_results` used to
// suggest otherwise; it was declared, written by every constructor, and read
// NOWHERE, so it has been DELETED rather than left as a lie the option list
// tells. Honestly implementing it would mean `create_pattern` minting solids,
// which its `Vec<Vec<FaceId>>` return type cannot express.) So "4 bodies-worth
// of volume" is not something the API hands back directly; each instance's
// face GROUP has to be wrapped into a `Shell`/`Solid` by the caller (exactly
// as `boolean_multibody.rs`'s `make_box` wraps `TopologyBuilder` output) to
// even ask the question. That wrapping is done here, explicitly, as
// measurement scaffolding -- not something `create_pattern` provides. What
// `create_pattern` DOES now guarantee is that such a wrap succeeds: an
// instance's copied faces are internally edge-welded, so they close.
// ===========================================================================

/// Wrap a face group (e.g. every face of a patterned box instance) into a fresh
/// `Shell`/`Solid` so its volume / certificate / shell closure can be queried.
/// Returns both ids -- the `ShellId` is what `validate_shell_closure` takes.
/// This is test-only scaffolding: `create_pattern` itself never does this.
fn solid_from_face_group(m: &mut BRepModel, faces: &[FaceId]) -> (SolidId, ShellId) {
    let mut shell = Shell::new(0, ShellType::Closed);
    for &f in faces {
        shell.add_face(f);
    }
    let shell_id = m.shells.add(shell);
    (m.solids.add(Solid::new(0, shell_id)), shell_id)
}

/// The multiset of `EdgeId`s referenced by a face group's loops (outer +
/// inner), as `edge_id -> number of distinct faces using it`. This is the
/// direct, mesh-independent read of the defect: welded topology puts every
/// edge on exactly two faces.
fn edge_face_counts(m: &BRepModel, faces: &[FaceId]) -> std::collections::BTreeMap<EdgeId, usize> {
    let mut counts: std::collections::BTreeMap<EdgeId, usize> = std::collections::BTreeMap::new();
    for &f in faces {
        let face = m.faces.get(f).expect("group face exists");
        let mut seen: std::collections::BTreeSet<EdgeId> = std::collections::BTreeSet::new();
        for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            if let Some(lp) = m.loops.get(lid) {
                seen.extend(lp.edges.iter().copied());
            }
        }
        for e in seen {
            *counts.entry(e).or_insert(0) += 1;
        }
    }
    counts
}

/// WAS A MEASURED RED — NOW GREEN (fixed in `operations/pattern.rs`; the
/// `#[ignore]` is removed, every original assertion intact and three added).
///
/// The defect: `create_pattern_instance` called `transform_face` once PER FACE,
/// independently, and `transform_face` -> `transform_loop` built a fresh
/// `vmap: HashMap<VertexId, VertexId>` LOCAL to that one call. It therefore
/// deduped corners WITHIN a face's own loop but never ACROSS two faces, and it
/// deduped EDGES nowhere at all -- two box faces meeting at a physical edge each
/// minted their OWN private copy of it (a different `EdgeId` on coincidentally
/// equal endpoints). `merge_pattern_geometry` (run because
/// `PatternOptions::default()` sets `merge_geometry: true`) then did a
/// vertex-coincidence pass, but it said so plainly: "Edges and faces themselves
/// are not deduplicated here." So after the merge the two private edges shared
/// canonical VERTEX ids and remained two distinct `EdgeId`s, each referenced by
/// exactly ONE face's loop -- which is what strict B-Rep connectivity reports as
/// an open boundary.
///
/// The RED run, verbatim: instance 0 (the untouched seed) certified sound;
/// instance 1 (a transformed copy, wrapped in a fresh `Shell`/`Solid` by this
/// test since `create_pattern` never does that -- see the CASE 3 header) did
/// NOT: `ValidityCertificate { brep_valid: false, watertight: true, manifold:
/// true, euler_characteristic: 2, boundary_edges: 0, nonmanifold_edges: 0,
/// oriented: true, ... errors: [24 x ConnectivityError "Boundary edge N
/// detected - potential gap in topology"] }`. Those 24 errors carried edge ids
/// 12..=35 and face ids 6..=11 -- i.e. **24 DISTINCT edges, each flagged once**:
/// 6 faces x 4 private edges apiece, where a welded box has 12 edges shared
/// two-apiece. (An earlier reading of this failure as "12 edges flagged twice"
/// was wrong; the ids in the payload settle it.) Volume was exactly right (96.0,
/// matching the seed) and the MESH-level flags -- watertight, manifold,
/// euler=2, boundary_edges=0 -- were ALL clean, because coincident private edges
/// position-weld away in tessellation. Only strict B-Rep half-edge accounting
/// could see it.
///
/// The fix (`pattern::InstanceRemap`) gives one copied instance a single
/// identity-keyed remap, `source EdgeId -> copy EdgeId` (and likewise for
/// vertices), threaded through every face of that instance -- the
/// `deep_clone::CloneContext` idiom rather than sweep's coincidence search. Two
/// source faces sharing an edge resolve it under the same key, so the copy is
/// welded exactly where the seed was and nowhere else. The 24->12 edge collapse
/// is asserted below directly, so a regression that drops the edge map fails on
/// the count even if certification were to stop noticing.
#[test]
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
        // The B-Rep read of the weld, taken BEFORE any certificate so it stands
        // on its own: a welded 6-face box references 12 distinct edges, each
        // used by exactly 2 faces. Un-welded it was 24 edges x 1 face.
        let counts = edge_face_counts(&m, group);
        let per_face: Vec<usize> = counts.values().copied().collect();

        let (sid, shell_id) = if i == 0 {
            let mut shell = None;
            if let Some(solid) = m.solids.get(seed_solid) {
                shell = Some(solid.outer_shell);
            }
            (seed_solid, shell.expect("seed solid has an outer shell"))
        } else {
            solid_from_face_group(&mut m, group)
        };
        let closure = geometry_engine::primitives::validation::validate_shell_closure(&m, shell_id);
        let cert = m.certify_solid(sid);
        let vol = m
            .calculate_solid_volume(sid)
            .unwrap_or_else(|| panic!("instance {i}: no volume computed"));
        eprintln!(
            "[pattern instance {i}] volume={vol:.6} cert_sound={} brep_valid={} \
             watertight={} distinct_edges={} faces_per_edge={:?} cert_errors={} \
             shell_closure_errors={}",
            cert.is_sound(),
            cert.brep_valid,
            cert.watertight,
            counts.len(),
            per_face.iter().collect::<std::collections::BTreeSet<_>>(),
            cert.errors.len(),
            closure.len(),
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

        // --- the three numbers the old `#[ignore]` reason recorded as broken ---
        //
        // (a) `brep_valid` was the ONE false flag on the copy; `watertight`,
        //     `manifold`, `euler=2` and `boundary_edges=0` were all true and
        //     lying, so pin the B-Rep half, not the mesh half.
        assert!(
            cert.brep_valid,
            "instance {i}: B-Rep accounting must be valid (it was false on every \
             copy while the mesh flags all read clean): {:?}",
            cert.errors,
        );
        assert!(
            cert.errors.is_empty(),
            "instance {i}: certificate must carry NO errors (24 ConnectivityError \
             'Boundary edge N' entries before the weld): {:?}",
            cert.errors,
        );
        // (b) The named contract: one instance's faces form a CLOSED shell.
        assert!(
            closure.is_empty(),
            "instance {i}: validate_shell_closure must report nothing -- a copied \
             instance's faces have to close on their own: {closure:?}",
        );
        // (c) The mesh-independent witness. 12 vs 24 is the whole defect: if the
        //     identity edge map is ever dropped, this fails on the count alone.
        assert_eq!(
            counts.len(),
            12,
            "instance {i}: a box instance must reference exactly 12 distinct edges \
             (it referenced 24 private ones before the weld), got {:?}",
            counts.keys().collect::<Vec<_>>(),
        );
        assert!(
            per_face.iter().all(|&c| c == 2),
            "instance {i}: every edge must be used by exactly 2 faces (each was \
             used by exactly 1 before the weld), got {counts:?}",
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
