//! F6-α: radius vs. curvature feasibility gate.
//!
//! ## What this is
//!
//! Pre-flight check for fillet operations. Given a set of edges and a
//! requested blend radius, scan each edge's adjacent faces, find the
//! tightest analytic curvature on those faces, and reject if the
//! requested radius would not fit a rolling ball of that size.
//!
//! On rejection the result is the structured Diagnostics-α variant
//! [`BlendFailure::RadiusExceedsCurvature`] carrying the offending
//! edge, the radius the caller asked for, and `r_max = 1 / |κ_max|`
//! at the binding face. The Phase-2 typed surface
//! [`OperationError::BlendFailed`] wraps this for callers that route
//! through the kernel's standard error type.
//!
//! ## Why this matters
//!
//! Without an upper bound on the rolling-ball radius the spine
//! solver in `operations::spine_solver` will run a marching iteration
//! that diverges (a ball larger than the local feature can't sit in
//! the dihedral pocket). The downstream failure mode is either a
//! topology-corruption error a thousand lines deeper, or a
//! `SpineSolverDiverged` after burning the iteration budget. The
//! pre-flight catches the case cheaply and tells the agent /
//! frontend the maximum radius that *will* succeed at this site, so
//! a "try `r_max * 0.95`" recovery loop is trivial.
//!
//! ## What this MVP covers (and what it doesn't)
//!
//! The check inspects analytic surfaces only:
//!
//! | Surface   | Max curvature `κ_max`                |
//! |-----------|--------------------------------------|
//! | `Plane`   | 0 (never binds)                      |
//! | `Cylinder`| `1 / radius`                         |
//! | `Sphere`  | `1 / radius`                         |
//! | `Torus`   | `1 / minor_radius` (worst case)      |
//! | other     | skipped (pass-through, no rejection) |
//!
//! Cones (variable-curvature along the axis), `RuledSurface`, and
//! `NurbsSurface` are deliberately *not* checked here — sampling-
//! based curvature evaluation on those surfaces is F6-β work. The
//! MVP catches the common "fillet too big for the cylinder it sits
//! on" failures the integration tests target and leaves the rest as
//! pass-through so the existing dispatch (spine solver, etc.) keeps
//! handling them.
//!
//! ## What this MVP does NOT determine
//!
//! The gate does **not** decide which side of each adjacent face the
//! rolling ball sits on. The conservative direction is to reject
//! whenever the analytic curvature is high enough, regardless of
//! ball-side. This means a fillet on the *convex-from-ball* side of
//! a cylinder (where the surface curves away from the ball and any
//! radius is geometrically valid) can be rejected. The trade-off is
//! deliberate: a wrong-reject is recoverable by the caller (try a
//! smaller radius); a wrong-accept silently corrupts the model
//! through the spine solver. Ball-side disambiguation is F6-β.
//!
//! ## Where this runs
//!
//! Called from `fillet_edges` (operations/fillet.rs) right after
//! `lifecycle::validate_can_apply` succeeds, before the
//! `with_rollback` snapshot is taken. Pure read-only — never mutates
//! the model.

use crate::operations::diagnostics::BlendFailure;
use crate::operations::edge_classification::find_adjacent_faces;
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Cylinder, Sphere, Torus};
use crate::primitives::topology_builder::BRepModel;

/// Safety factor applied to `r_max`. The rolling-ball spine solver
/// becomes ill-conditioned as `r / r_max → 1`; we reject at 99 % to
/// keep a small headroom and avoid borderline divergence. Tightening
/// this factor is a follow-up once F6-β surfaces empirical limits.
const RADIUS_SAFETY_FACTOR: f64 = 0.99;

/// Curvatures below this are treated as flat. Anything ~1e-12 is
/// numerically indistinguishable from zero on a `Plane` and would
/// produce a meaningless `r_max ≈ 1e12`.
const FLAT_CURVATURE_EPSILON: f64 = 1.0e-9;

/// Run the F6-α feasibility check.
///
/// Returns `Ok(())` if every selected edge can host a rolling ball
/// of `radius` against every adjacent analytic face. Returns
/// [`BlendFailure::RadiusExceedsCurvature`] for the **first**
/// offending (edge, face) pair found — the failure carries the
/// binding `r_max` so callers can retry below that bound.
///
/// `radius` ≤ 0 is treated as "no upper bound to check" and passes
/// through; the caller's parameter validation handles negative or
/// zero radii.
pub fn validate_radius_against_curvature(
    model: &BRepModel,
    edges: &[EdgeId],
    radius: f64,
) -> Result<(), BlendFailure> {
    if !(radius > 0.0) {
        return Ok(());
    }

    for &edge_id in edges {
        let faces = find_adjacent_faces(model, edge_id);
        for face_id in faces {
            let Some(kappa_max) = max_analytic_curvature(model, face_id) else {
                continue;
            };
            if kappa_max < FLAT_CURVATURE_EPSILON {
                continue;
            }
            let r_max = 1.0 / kappa_max;
            if radius >= r_max * RADIUS_SAFETY_FACTOR {
                return Err(BlendFailure::RadiusExceedsCurvature {
                    edge: edge_id,
                    // Analytic surfaces in this MVP have uniform
                    // curvature, so the "station" is informational
                    // only. F6-β will report the true arc-length
                    // parameter once we evaluate curve points.
                    station: 0.5,
                    r_requested: radius,
                    r_max,
                });
            }
        }
    }
    Ok(())
}

/// Maximum analytic principal curvature for a face's underlying
/// surface, or `None` if the surface type is not analytically
/// classified by this MVP. The value is `|κ_max|` — sign is not
/// preserved here because we use it only for `1 / |κ|` comparisons.
fn max_analytic_curvature(model: &BRepModel, face_id: FaceId) -> Option<f64> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let any = surface.as_any();

    if let Some(cyl) = any.downcast_ref::<Cylinder>() {
        return Some(1.0 / cyl.radius.max(FLAT_CURVATURE_EPSILON));
    }
    if let Some(sph) = any.downcast_ref::<Sphere>() {
        return Some(1.0 / sph.radius.max(FLAT_CURVATURE_EPSILON));
    }
    if let Some(tor) = any.downcast_ref::<Torus>() {
        // Worst case on a torus is `1 / minor_radius` (the tube
        // curvature, present at every point on the torus). The
        // alternate curvature along the major-direction is bounded
        // by the same value on the inner ring (`1 / (major - minor)`
        // can exceed `1 / minor` only when major < 2·minor, i.e. a
        // self-intersecting torus — the kernel rejects those at
        // construction).
        return Some(1.0 / tor.minor_radius.max(FLAT_CURVATURE_EPSILON));
    }
    // Plane / Cone / RuledSurface / NurbsSurface: MVP pass-through.
    // Plane has κ = 0 (no constraint); the others need surface
    // evaluation, which is F6-β.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    /// Construct a unit cylinder solid and return its solid id.
    fn unit_cylinder(model: &mut BRepModel, radius: f64) -> GeometryId {
        TopologyBuilder::new(model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                radius,
                1.0,
            )
            .expect("cylinder construction must succeed for positive dims")
    }

    /// Collect every edge id of the solid (outer + inner shells).
    fn all_edge_ids(model: &BRepModel, solid_id: crate::primitives::solid::SolidId) -> Vec<EdgeId> {
        let mut edges = Vec::new();
        let solid = model.solids.get(solid_id).expect("solid present");
        let shell_ids =
            std::iter::once(solid.outer_shell).chain(solid.inner_shells.iter().copied());
        for shell_id in shell_ids {
            let shell = model.shells.get(shell_id).expect("shell present");
            for &face_id in &shell.faces {
                let face = model.faces.get(face_id).expect("face present");
                let loop_data = model.loops.get(face.outer_loop).expect("loop present");
                for &eid in &loop_data.edges {
                    if !edges.contains(&eid) {
                        edges.push(eid);
                    }
                }
            }
        }
        edges
    }

    #[test]
    fn empty_edge_set_passes() {
        let model = BRepModel::new();
        assert!(validate_radius_against_curvature(&model, &[], 0.5).is_ok());
    }

    #[test]
    fn zero_or_negative_radius_passes_through() {
        // Negative / zero radius is the caller's parameter problem;
        // the feasibility gate is not the place to reject it.
        let model = BRepModel::new();
        assert!(validate_radius_against_curvature(&model, &[1, 2], 0.0).is_ok());
        assert!(validate_radius_against_curvature(&model, &[1, 2], -1.0).is_ok());
    }

    #[test]
    fn unresolved_edges_are_skipped() {
        // Bogus edge ids resolve to no adjacent faces. The gate is
        // pre-flight — `validate_can_apply` rejects unknown ids
        // earlier with InvalidInput. The feasibility gate must not
        // panic or wrong-reject in that case.
        let model = BRepModel::new();
        assert!(validate_radius_against_curvature(&model, &[9999, 8888], 1.0).is_ok());
    }

    #[test]
    fn small_radius_against_unit_cylinder_passes() {
        let mut model = BRepModel::new();
        let solid_id = match unit_cylinder(&mut model, 1.0) {
            GeometryId::Solid(s) => s,
            other => panic!("expected solid, got {:?}", other),
        };
        let edges = all_edge_ids(&model, solid_id);
        assert!(
            validate_radius_against_curvature(&model, &edges, 0.1).is_ok(),
            "r=0.1 ≪ cylinder radius 1.0 must pass"
        );
    }

    #[test]
    fn oversize_radius_against_unit_cylinder_rejects_with_typed_failure() {
        let mut model = BRepModel::new();
        let solid_id = match unit_cylinder(&mut model, 1.0) {
            GeometryId::Solid(s) => s,
            other => panic!("expected solid, got {:?}", other),
        };
        let edges = all_edge_ids(&model, solid_id);

        let result = validate_radius_against_curvature(&model, &edges, 2.0);
        match result {
            Err(BlendFailure::RadiusExceedsCurvature {
                r_requested, r_max, ..
            }) => {
                assert_eq!(r_requested, 2.0);
                assert!(
                    (r_max - 1.0).abs() < 1e-9,
                    "expected r_max = 1/(1/1.0) = 1.0, got {}",
                    r_max
                );
            }
            Ok(()) => panic!("r=2.0 against cylinder radius=1.0 must be rejected by F6-α"),
            Err(other) => panic!("expected RadiusExceedsCurvature, got {:?}", other),
        }
    }

    #[test]
    fn safety_factor_rejects_at_r_max() {
        // Exact-equality case: r = r_max should reject because of
        // the 99 % safety factor. Tightening this is F6-β work, but
        // pin the current behaviour so the change is observable.
        let mut model = BRepModel::new();
        let solid_id = match unit_cylinder(&mut model, 1.0) {
            GeometryId::Solid(s) => s,
            other => panic!("expected solid, got {:?}", other),
        };
        let edges = all_edge_ids(&model, solid_id);
        assert!(
            validate_radius_against_curvature(&model, &edges, 1.0).is_err(),
            "r=1.0 == r_max must reject under the 0.99 safety factor"
        );
        // And a hair below 0.99 must pass.
        assert!(
            validate_radius_against_curvature(&model, &edges, 0.98).is_ok(),
            "r=0.98 < 0.99·r_max must pass"
        );
    }
}
