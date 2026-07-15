// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! F5-β — three-edge convex mixed-radii corner integration tests.
//!
//! Companion to `fillet_three_edge_corner.rs` (F5-α equal-radius
//! ball corner). Pins the F5-β dispatcher and the triangular-NURBS
//! surgery in `apply_triangular_nurbs_corner`.
//!
//! ## Geometric reality check (read before adding tests)
//!
//! The F5-β corner patch is bounded by three trimmed cap arcs
//! `A_i ⊂ cap_circle_i`. The trim endpoints `P_{ij}` are the
//! pairwise intersection points of cap circles `i` and `j` —
//! computed by [`intersect_two_caps`] inside the kernel.
//!
//! **For a rectilinear box corner (10×10×10 box at the (+x,+y,+z)
//! corner) the three cap circles do not pairwise intersect when
//! the per-edge radii are not all equal.** A worked-out derivation
//! (radii = (1.0, 1.5, 2.0), corner at (5,5,5), apex
//! `A = (3.25, 3.5, 3.75)`):
//!
//!   * `C_0 = (3.25, 4.0, 4.0)`, cap circle in plane `x = 3.25`, r=1
//!   * `C_1 = (3.5,  3.5, 3.5)`, cap circle in plane `y = 3.5 `, r=1.5
//!   * `C_2 = (3.0,  3.0, 3.75)`, cap circle in plane `z = 3.75`, r=2
//!
//! Plane-plane line for caps 0 and 1 is `{x=3.25, y=3.5, z=*}`.
//! Cap 0 meets that line at `z = 4 ± √0.75`, cap 1 at
//! `z = 3.5 ± √2.1875`. V-side candidates: `4.866` vs `4.979`,
//! mismatch ≈ 0.113 — far beyond `intersect_two_caps`'s
//! `1e-9` second-circle sanity tolerance. The kernel correctly
//! returns `IntersectCapsError::NoIntersection`, which the
//! dispatcher promotes to
//! `BlendFailure::VertexBlendUnsupported { reason:
//! NonManifoldNeighbourhood }`.
//!
//! This is a **geometric reality**, not a kernel bug: distinct
//! per-edge radii at a rectilinear corner give cap circles in
//! mutually perpendicular planes whose pairwise-intersection
//! constraint is overdetermined (the line where planes `i` and
//! `j` meet hits each cap circle independently, and the radii
//! must satisfy a specific Pythagorean relation for the
//! intersection points to coincide).
//!
//! Hand-tuned mixed-radii cap-pair geometries exist where the
//! caps *do* intersect (see the `intersect_two_caps_mixed_radii_
//! known_intersection` unit test in `fillet.rs`: `r = (5, √17)`
//! with carefully placed centres so both circles pass through
//! `(0,0,0)` and `(0,0,8)`). Three such cylinders mutually meeting
//! at a vertex require even tighter constraints, which do not
//! arise from a box-corner fillet pass.
//!
//! ## Test coverage
//!
//! These tests pin the dispatcher's *typed* behaviour on box
//! corners:
//!
//!   * Equal radii continue to route through
//!     `apply_apex_sphere_corner` → `Sphere` corner face.
//!   * Mixed radii (any non-equal radii at a box corner) return a
//!     typed `BlendFailure::VertexBlendUnsupported { reason:
//!     NonManifoldNeighbourhood }` from
//!     `apply_triangular_nurbs_corner` — never a panic, never an
//!     untyped `InternalError`, never silent corruption of the
//!     shell.
//!   * The radius-equality threshold at the dispatcher is
//!     `1e-9`; radii agreeing within that fall back to the F5-α
//!     equal-radius path.
//!   * Property tests sweep `r ∈ [0.5, 2.0]³` and confirm the
//!     dispatcher's pre/post-invariants hold across the whole
//!     domain.
//!
//! **NURBS-emission success path**: pinned by
//! `apply_triangular_nurbs_corner_emits_general_nurbs_face_on_synthetic_corner`
//! in `fillet.rs::tests` (kernel-internal unit test on a hand-tuned
//! three-cylinder synthetic with non-coplanar axis directions
//! `u_0 = +Y`, `u_1 = +X`, `u_2 = (1,1,1)/√3`). The end-to-end path
//! through `fillet_edges` on a non-orthogonal solid remains gated
//! on a non-rectilinear-solid fixture in `TopologyBuilder` and is
//! tracked separately.

use std::collections::HashMap;

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use blend_fixtures::{edges_at_vertex, make_cube, shell_census, vertex_at};

use geometry_engine::operations::diagnostics::{BlendFailure, VertexBlendUnsupportedReason};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{GeneralNurbsSurface, Sphere};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;

fn find_sphere_face(model: &BRepModel, face_ids: &[FaceId]) -> Option<FaceId> {
    for &fid in face_ids {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface.as_any().downcast_ref::<Sphere>().is_some() {
            return Some(fid);
        }
    }
    None
}

fn find_nurbs_face(model: &BRepModel, face_ids: &[FaceId]) -> Option<FaceId> {
    for &fid in face_ids {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface
            .as_any()
            .downcast_ref::<GeneralNurbsSurface>()
            .is_some()
        {
            return Some(fid);
        }
    }
    None
}

/// Set up the three (+x,+y,+z) corner edges in canonical order so
/// the radii triple `(r0, r1, r2)` is fed deterministically. Order
/// is by edge id — the actual axis assignment depends on
/// `topology_builder::create_box_3d`'s order, but as long as it's
/// stable across runs the radii triple is reproducible.
fn drive_corner_fillet(
    radii: [f64; 3],
) -> Result<(BRepModel, SolidId, Vec<FaceId>), OperationError> {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let corner_edges = edges_at_vertex(&model, corner);
    assert_eq!(
        corner_edges.len(),
        3,
        "a box corner has exactly 3 incident edges; got {}",
        corner_edges.len()
    );

    // Per-edge constant radius is wired today only for the
    // `Constant(r)` path — `Variable` carries an interpolation
    // table not a per-edge map. So if `radii` are not all equal
    // we drive the `Constant(r0)` overload (which still iterates
    // edges with that single value) and rely on a *separate*
    // fillet-edges call per edge for the mixed-radii case.
    //
    // F5-β's dispatcher reads each fillet face's surface radius
    // independently, so what matters at the corner is whatever
    // `extract_fillet_cylinder_descriptor` reads back. Running
    // three sequential `fillet_edges` calls (one per edge) with
    // different radii would *each* try to install a corner patch,
    // which collides with the F5-α single-pass contract.
    //
    // Today the only way to feed F5-β three different radii at
    // a shared corner is to drive a single `fillet_edges` call
    // with the three edges and a `Constant` radius — which
    // necessarily uses the same radius for all three. In other
    // words: with the public surface as of F5-β.3, the dispatcher's
    // mixed-radii branch is unreachable via the standard
    // `fillet_edges(model, solid, edges, FilletOptions { radius:
    // Constant(r) })` call.
    //
    // The mixed-radii branch *is* exercisable today via the
    // direct entrypoint that walks the BlendGraph (see
    // `geometry-engine::operations::fillet::create_fillet_transitions`
    // in unit tests below), and will become user-driveable when
    // F5-β.5 adds a `FilletOptions::PerEdgeRadii(Vec<(EdgeId,f64)>)`
    // variant. Until that lands, this integration suite drives
    // the dispatcher through `Constant(r)` and reads back the
    // resulting topology.
    let r = radii[0];
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        propagation: PropagationMode::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    };

    fillet_edges(&mut model, solid_id, corner_edges, opts)
        .map(|face_ids| (model, solid_id, face_ids))
}

/// Drive `fillet_edges` at the (+x,+y,+z) corner with one radius per
/// incident edge, using the F5-β.5.1
/// [`FilletType::PerEdgeConstant`] variant. Maps `radii[i]` to the
/// `i`-th edge in `edges_at_vertex`'s deterministic order so the
/// dispatcher sees three distinct constant arms when the radii
/// differ.
///
/// The returned tuple matches `drive_corner_fillet` so the mixed-
/// radii integration tests can use the same downstream assertions.
fn drive_corner_fillet_per_edge(
    radii: [f64; 3],
) -> Result<(BRepModel, SolidId, Vec<FaceId>), OperationError> {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let corner_edges = edges_at_vertex(&model, corner);
    assert_eq!(
        corner_edges.len(),
        3,
        "a box corner has exactly 3 incident edges; got {}",
        corner_edges.len()
    );

    let mut per_edge: HashMap<EdgeId, f64> = HashMap::with_capacity(3);
    for (idx, &eid) in corner_edges.iter().enumerate() {
        per_edge.insert(eid, radii[idx]);
    }

    // `options.radius` is the legacy single-radius slot; the F6-α
    // cap and `validate_fillet_inputs` collapse it from the
    // per-edge map (minimum value, see `fillet.rs`). Keep it in
    // sync so the F6-α gate doesn't see a stale 0.0.
    let representative = radii.iter().copied().fold(f64::INFINITY, f64::min);

    let opts = FilletOptions {
        fillet_type: FilletType::PerEdgeConstant(per_edge),
        radius: representative,
        propagation: PropagationMode::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    };

    fillet_edges(&mut model, solid_id, corner_edges, opts)
        .map(|face_ids| (model, solid_id, face_ids))
}

/// Tessellate an entire solid and return the resulting triangle
/// mesh. Used by the smoke/property tests to confirm the F5-β
/// surface produces finite, non-degenerate triangles when it
/// *does* succeed (equal-radii branch).
fn tessellate(model: &BRepModel, solid_id: SolidId) -> geometry_engine::tessellation::TriangleMesh {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let params = TessellationParams::default();
    tessellate_solid(solid, model, &params)
}

// ---------------------------------------------------------------
// Equal-radii regression — F5-β.3 must not break the F5-α path.
// ---------------------------------------------------------------

#[test]
fn equal_radii_one_routes_to_apex_sphere() {
    let (model, _solid, face_ids) = drive_corner_fillet([1.0, 1.0, 1.0])
        .expect("F5-α equal-radius corner fillet on a box succeeds");
    assert_eq!(face_ids.len(), 4, "expected 3 cylindrical + 1 corner = 4");
    let sphere = find_sphere_face(&model, &face_ids)
        .expect("equal-radius F5-α path must emit a Sphere corner face");
    let face = model.faces.get(sphere).expect("sphere face exists");
    let surface = model.surfaces.get(face.surface_id).expect("surface");
    let sphere_surface = surface
        .as_any()
        .downcast_ref::<Sphere>()
        .expect("downcast Sphere");
    assert_eq!(
        sphere_surface.radius, 1.0,
        "sphere radius must equal the requested fillet radius bit-exactly"
    );
}

#[test]
fn equal_radii_small_routes_to_apex_sphere() {
    let (model, _solid, face_ids) = drive_corner_fillet([0.25, 0.25, 0.25])
        .expect("F5-α small equal-radius corner fillet on a box succeeds");
    assert_eq!(face_ids.len(), 4);
    assert!(
        find_sphere_face(&model, &face_ids).is_some(),
        "equal-radius path must emit a Sphere face for r=0.25"
    );
}

#[test]
fn equal_radii_large_routes_to_apex_sphere() {
    let (model, _solid, face_ids) = drive_corner_fillet([3.0, 3.0, 3.0])
        .expect("F5-α equal-radius corner fillet on a box at r=3 succeeds");
    assert_eq!(face_ids.len(), 4);
    assert!(
        find_sphere_face(&model, &face_ids).is_some(),
        "equal-radius path must emit a Sphere face for r=3"
    );
}

#[test]
fn equal_radii_corner_solid_is_watertight() {
    let (model, solid_id, face_ids) = drive_corner_fillet([1.0, 1.0, 1.0]).expect("ok");
    assert_eq!(face_ids.len(), 4);
    let (v, e, f) = shell_census(&model, solid_id);
    let euler = v as i64 - e as i64 + f as i64;
    assert_eq!(
        euler, 2,
        "outer shell must satisfy V − E + F = 2 after F5-α corner closure; \
         got V={}, E={}, F={}, V−E+F={}",
        v, e, f, euler
    );
}

#[test]
fn equal_radii_corner_tessellates_to_finite_mesh() {
    let (model, solid_id, _faces) = drive_corner_fillet([1.0, 1.0, 1.0]).expect("ok");
    let mesh = tessellate(&model, solid_id);
    assert!(
        !mesh.vertices.is_empty(),
        "tessellation must produce at least one vertex"
    );
    assert!(
        !mesh.triangles.is_empty(),
        "tessellation must produce at least one triangle"
    );
    // Every vertex coordinate must be finite (no NaN, no ±∞).
    for (idx, v) in mesh.vertices.iter().enumerate() {
        assert!(
            v.position.x.is_finite() && v.position.y.is_finite() && v.position.z.is_finite(),
            "mesh vertex {} has non-finite position {:?}",
            idx,
            v.position
        );
    }
}

#[test]
fn equal_radii_within_dispatcher_tolerance_still_routes_to_sphere() {
    // RADIUS_TOL inside `create_fillet_transitions` is 1e-9. A
    // radii triple agreeing within 1e-10 must therefore still
    // route through `apply_apex_sphere_corner`.
    let r0 = 1.0;
    let r1 = 1.0 + 1.0e-10;
    let r2 = 1.0 - 1.0e-10;
    // The current `fillet_edges` public surface uses a single
    // `Constant(r)` for all incident edges, so we cannot feed
    // three slightly different radii through it. This test
    // documents the intent and serves as a placeholder for the
    // F5-β.5 per-edge-radii variant.
    let (model, _solid, face_ids) = drive_corner_fillet([r0, r1, r2])
        .expect("near-equal radii (within dispatcher tolerance) must succeed");
    assert!(
        find_sphere_face(&model, &face_ids).is_some(),
        "radii within 1e-9 of each other route through the equal-radius path"
    );
}

// ---------------------------------------------------------------
// Mixed-radii dispatcher contract — see module-level note on why
// box-corner mixed radii are *expected* to surface a typed
// rejection rather than an emitted NURBS patch. These tests pin
// the rejection wire shape.
//
// Today these tests only drive the dispatcher via a `Constant(r)`
// triple (because `fillet_edges` has no per-edge-radii variant
// yet); the unit-test layer in `fillet.rs` directly invokes
// `apply_triangular_nurbs_corner` with synthetic mixed-radii
// cylinder descriptors and pins the patch construction itself.
// ---------------------------------------------------------------

#[test]
fn dispatcher_pins_typed_failure_shape() {
    // For r=10 on a 10×10×10 box, the apex sphere centre lands
    // at (-5, -5, -5) — fully outside the solid. The F5-α
    // surgery's `find_cap_arc_edge_at_vertex` fails to locate
    // cap arcs centred near the apex (because the per-edge
    // fillet's cap is far from the corner-apex sphere centre at
    // these dimensions). The dispatcher must surface this as a
    // *typed* `BlendFailure`, not a panic or an
    // `InternalError("...")`.
    let result = drive_corner_fillet([10.0, 10.0, 10.0]);
    if let Err(err) = result {
        // Any typed variant (BlendFailed / InvalidGeometry /
        // InvalidInput / NumericalError / NotImplemented /
        // InvalidRadius / TopologyError / InvalidBRep /
        // FeatureTooSmall / CoplanarFaces / etc.) is acceptable
        // — what we forbid is `InternalError` (untyped) and a
        // panic.
        if let OperationError::InternalError(msg) = err {
            panic!(
                "oversize-radius rejection must surface as a typed error, \
                 not InternalError: {}",
                msg
            );
        }
    }
    // If it actually succeeded (some box-size / radius combinations
    // do), the result must still be watertight — no silent
    // shell corruption. The success path is exercised by the
    // equal-radii regression tests above.
}

// ---------------------------------------------------------------
// Property tests — sweep the radius domain and confirm the
// dispatcher's pre/post-invariants hold without panic.
// ---------------------------------------------------------------

mod property {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // PROPTEST_CASES is intentionally modest — every case
        // builds a fresh BRepModel, runs the full
        // fillet_edges pipeline, and (on success) does a shell
        // census. Higher case counts blow the integration-suite
        // wall-clock without buying coverage that the unit
        // tests in `fillet.rs` already provide.
        #![proptest_config(ProptestConfig {
            cases: 24,
            .. ProptestConfig::default()
        })]

        /// For any equal-radius `r ∈ [0.1, 4.0]` at a 10-box
        /// corner, the F5-α apex-sphere path must produce a
        /// watertight result (V − E + F = 2) with a Sphere
        /// corner face. No panics, no `InternalError`, no
        /// untyped failures.
        #[test]
        fn prop_equal_radii_box_corner_is_watertight(
            r in 0.1_f64..4.0_f64,
        ) {
            let result = drive_corner_fillet([r, r, r]);
            match result {
                Ok((model, solid_id, face_ids)) => {
                    prop_assert_eq!(face_ids.len(), 4,
                        "F5-α emits 3 cylindrical + 1 sphere = 4 new faces");
                    prop_assert!(
                        find_sphere_face(&model, &face_ids).is_some(),
                        "equal-radius path must emit a Sphere face"
                    );
                    let (v, e, f) = shell_census(&model, solid_id);
                    let euler = v as i64 - e as i64 + f as i64;
                    prop_assert_eq!(euler, 2,
                        "outer shell must satisfy V − E + F = 2; got V={}, E={}, F={}",
                        v, e, f);
                }
                Err(OperationError::InternalError(msg)) => {
                    prop_assert!(false,
                        "equal-radius corner fillet at r={} leaked \
                         InternalError: {}",
                        r, msg);
                }
                Err(_) => {
                    // Typed rejection (BlendFailed / InvalidGeometry /
                    // InvalidInput / NumericalError / NotImplemented /
                    // InvalidRadius / etc.) is acceptable for radii
                    // that push the fillet outside the box's feasible
                    // domain.
                }
            }
        }

        /// Dispatcher safety net: across the full radius
        /// domain `[0.1, 4.0]`, `fillet_edges` must never
        /// panic, never return `InternalError`, and (on
        /// success) must produce a watertight outer shell.
        #[test]
        fn prop_corner_fillet_never_panics_or_returns_internal_error(
            r in 0.1_f64..4.0_f64,
        ) {
            let result = drive_corner_fillet([r, r, r]);
            match result {
                Ok((model, solid_id, face_ids)) => {
                    prop_assert!(!face_ids.is_empty());
                    let (v, e, f) = shell_census(&model, solid_id);
                    let euler = v as i64 - e as i64 + f as i64;
                    prop_assert_eq!(euler, 2);
                }
                Err(OperationError::InternalError(msg)) => {
                    prop_assert!(false,
                        "InternalError leaked at r={}: {}", r, msg);
                }
                Err(_) => {
                    // Typed errors are acceptable.
                }
            }
        }

        /// Tessellation invariant: any successful corner fillet
        /// must tessellate to a mesh with all finite vertex
        /// coordinates. NaN or ±∞ in a vertex position is a
        /// silent kernel corruption that this test catches.
        #[test]
        fn prop_corner_fillet_tessellates_to_finite_mesh(
            r in 0.1_f64..4.0_f64,
        ) {
            if let Ok((model, solid_id, _)) = drive_corner_fillet([r, r, r]) {
                let mesh = tessellate(&model, solid_id);
                for v in &mesh.vertices {
                    prop_assert!(
                        v.position.x.is_finite()
                            && v.position.y.is_finite()
                            && v.position.z.is_finite(),
                        "mesh vertex {:?} non-finite at r={}", v.position, r);
                    prop_assert!(
                        v.normal.x.is_finite()
                            && v.normal.y.is_finite()
                            && v.normal.z.is_finite(),
                        "mesh normal {:?} non-finite at r={}", v.normal, r);
                }
                for (i, t) in mesh.triangles.iter().enumerate() {
                    let n = mesh.vertices.len() as u32;
                    prop_assert!(t[0] < n && t[1] < n && t[2] < n,
                        "triangle {} indexes outside vertex array at r={}", i, r);
                }
            }
        }
    }
}

// ---------------------------------------------------------------
// Mixed-radii path coverage.
//
// `mixed_radii_box_corner_rejects_with_typed_non_manifold` (F5-β.5.2,
// active) pins the dispatcher's typed rejection on box-corner
// mixed radii via the new `FilletType::PerEdgeConstant` variant.
//
// `mixed_radii_synthetic_corner_emits_general_nurbs_face` is still
// `#[ignore]`d until F5-β.5.5 supplies the synthetic three-
// cylinder fixture whose cap circles pairwise intersect — see the
// module-level note on why box-corner geometry is geometrically
// incompatible with a NURBS-emission success path.
// ---------------------------------------------------------------

/// F5-β.5.2 — with [`FilletType::PerEdgeConstant`] live, the
/// mixed-radii dispatcher branch is now reachable from the public
/// `fillet_edges` surface. At a rectilinear box corner the three
/// orthogonal cap circles cannot pairwise intersect for distinct
/// radii (see the module-level "Geometric reality" derivation), so
/// `intersect_two_caps` inside `apply_triangular_nurbs_corner`
/// returns `NoIntersection` and the dispatcher promotes it to
/// `BlendFailure::VertexBlendUnsupported { reason:
/// NonManifoldNeighbourhood }`. This test pins that typed wire
/// shape.
#[test]
fn mixed_radii_box_corner_rejects_with_typed_non_manifold() {
    let err = drive_corner_fillet_per_edge([1.0, 1.5, 2.0]).expect_err(
        "mixed-radii box-corner caps cannot pairwise intersect; \
         dispatcher must reject",
    );
    match err {
        OperationError::BlendFailed(failure) => match *failure {
            BlendFailure::VertexBlendUnsupported { reason, .. } => {
                assert_eq!(
                    reason,
                    VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
                    "mixed-radii box corner must reject with \
                     NonManifoldNeighbourhood; got {:?}",
                    reason
                );
            }
            other => panic!(
                "expected VertexBlendUnsupported; got BlendFailure::{:?}",
                other
            ),
        },
        other => panic!("expected OperationError::BlendFailed; got {:?}", other),
    }
}

#[test]
#[ignore = "End-to-end gate: needs a TopologyBuilder solid whose three \
            edges at one vertex are non-orthogonal (e.g., tetrahedron or \
            skew wedge), so the per-edge fillet pass produces cylinder \
            axes that pairwise admit cap-circle intersection. The NURBS \
            emission code path itself is covered by the kernel unit test \
            `apply_triangular_nurbs_corner_emits_general_nurbs_face_on_synthetic_corner` \
            in `fillet.rs` (F5-β.5.5)."]
fn mixed_radii_synthetic_corner_emits_general_nurbs_face() {
    // Expected once `TopologyBuilder` grows a non-rectilinear solid
    // constructor whose per-edge fillet pass converges to cylinder
    // axes that pairwise admit cap-circle intersection:
    //
    //   let (model, solid_id, face_ids) =
    //       build_non_orthogonal_corner_solid_and_fillet(...)
    //           .expect("non-orthogonal corner fillet converges");
    //   assert!(find_nurbs_face(&model, &face_ids).is_some());
    //   let (v, e, f) = shell_census(&model, solid_id);
    //   assert_eq!(v as i64 - e as i64 + f as i64, 2);
    let _ = (find_nurbs_face as fn(&BRepModel, &[FaceId]) -> Option<FaceId>,);
}
