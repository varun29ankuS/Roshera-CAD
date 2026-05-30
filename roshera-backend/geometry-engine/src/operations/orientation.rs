//! Face orientation helpers.
//!
//! Every B-Rep face stores a `Surface` (the geometric carrier) plus a
//! `FaceOrientation` flag (`Forward` or `Backward`) that the kernel
//! multiplies into the surface's intrinsic u × v normal to get the
//! face's **oriented outward normal**. Downstream code (fillet rolling-
//! ball direction, chamfer bisector, mass-properties divergence
//! integrals, tessellation winding) consumes this oriented normal and
//! relies on it pointing **away from the solid material**.
//!
//! The surface's intrinsic normal is not guaranteed to point outward
//! because it derives from the order in which the surface was
//! constructed — Newell's method on a CW polygon gives `-Z`, on a CCW
//! polygon gives `+Z`; surfaces of revolution flip sign with the side
//! of the axis the profile sits on; ruled surfaces flip with the
//! edge-pair order. Every face-construction site in `operations/` and
//! `primitives/` must therefore pick `FaceOrientation::Forward` or
//! `Backward` such that the *product* matches the geometric outward
//! direction.
//!
//! This module centralises that pick. Callers compute the geometric
//! outward target (e.g. perpendicular to the extrusion axis away from
//! the loop centroid, or radial outward from a revolve axis) and pass
//! it here; this module samples the surface normal at its parametric
//! midpoint and picks the orientation that makes the dot product
//! non-negative.
//!
//! Historical note: this logic lived as a private
//! `orientation_for_target` in `operations/extrude.rs` and was called
//! at two sites (extrude top-cap construction, base-cap mutation).
//! That patched the cap-orientation bug for axis-aligned extrusions
//! but did not extend to side walls, lateral revolve / sweep / loft
//! surfaces, or blend surfaces, which is why fillet / chamfer
//! continued to misbehave on polyline extrusions at non-90° dihedrals
//! and on curved-surface operands. Promoting the helper to a shared
//! module is Slice 1 of the comprehensive face-orientation fix.

use super::{OperationError, OperationResult};
use crate::math::Vector3;
use crate::primitives::face::FaceOrientation;
use crate::primitives::surface::Surface;

/// Pick the `FaceOrientation` that aligns the face's oriented outward
/// normal (`surface.normal_at(u_mid, v_mid) * orientation.sign()`)
/// with the supplied geometric `target` direction.
///
/// Samples the surface at its parametric midpoint, which is sufficient
/// for the planar / ruled / lateral surface families this is called on
/// (constant-normal planes, ruled / revolution / blend surfaces whose
/// normal varies smoothly along the surface). For surfaces whose
/// midpoint normal is degenerate or whose orientation flips across the
/// patch interior (rare; would indicate a malformed surface), callers
/// should reach for `orient_face_for_outward_at` and pass a known-good
/// `(u, v)` sample.
///
/// If the surface midpoint normal and the target are exactly
/// perpendicular (dot = 0, oblique edge case), this deterministically
/// returns `Forward` — the surface is geometrically a knife edge and
/// the oriented-normal direction is undefined.
pub(crate) fn orient_face_for_outward(
    surface: &dyn Surface,
    target: Vector3,
) -> OperationResult<FaceOrientation> {
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    let u_mid = 0.5 * (u_min + u_max);
    let v_mid = 0.5 * (v_min + v_max);
    orient_face_for_outward_at(surface, target, u_mid, v_mid)
}

/// Variant of [`orient_face_for_outward`] that samples the surface
/// normal at a caller-supplied `(u, v)` instead of the parametric
/// midpoint.
///
/// Use when the parametric midpoint is known to land on a degenerate
/// point of the surface (e.g. the apex of a cone, the poles of a
/// sphere, a seam edge of a closed surface), where the surface normal
/// is either undefined or numerically unstable. The caller chooses a
/// `(u, v)` away from the singularity and passes it explicitly.
pub(crate) fn orient_face_for_outward_at(
    surface: &dyn Surface,
    target: Vector3,
    u: f64,
    v: f64,
) -> OperationResult<FaceOrientation> {
    let n = surface
        .normal_at(u, v)
        .map_err(|e| OperationError::NumericalError(format!("Surface normal failed: {:?}", e)))?;
    if n.dot(&target) >= 0.0 {
        Ok(FaceOrientation::Forward)
    } else {
        Ok(FaceOrientation::Backward)
    }
}

/// Pick complementary orientations for a **pair of faces** that share
/// a dihedral edge and must have oriented outward normals pointing
/// into opposite half-spaces.
///
/// Used by chamfer's split-edge case: when a single edge is replaced
/// by two new triangular faces meeting at the chamfer bisector, the
/// two faces' oriented normals must point away from each other across
/// the new bisector edge. Each face's orientation is picked
/// independently against its own `target` — the wrapper exists to
/// make the semantic intent explicit at the call site (and to assert
/// the targets are at least *somewhat* opposite by dot product < 0,
/// catching caller bugs early).
pub(crate) fn orient_complementary_pair(
    surface_a: &dyn Surface,
    target_a: Vector3,
    surface_b: &dyn Surface,
    target_b: Vector3,
) -> OperationResult<(FaceOrientation, FaceOrientation)> {
    if target_a.dot(&target_b) > 0.0 {
        return Err(OperationError::InvalidGeometry(format!(
            "orient_complementary_pair: target_a and target_b must point into \
             opposite half-spaces (dot = {} > 0)",
            target_a.dot(&target_b)
        )));
    }
    let a = orient_face_for_outward(surface_a, target_a)?;
    let b = orient_face_for_outward(surface_b, target_b)?;
    Ok((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;
    use crate::primitives::surface::{Cylinder, Plane};

    #[test]
    fn planar_surface_picks_forward_when_normal_aligns_with_target() {
        // XY plane (normal +Z), target +Z → Forward.
        let plane = Plane::from_point_normal(Point3::ZERO, Vector3::Z)
            .expect("XY plane construction must succeed");
        let orientation = orient_face_for_outward(&plane, Vector3::Z)
            .expect("midpoint normal is well-defined for a plane");
        assert_eq!(orientation, FaceOrientation::Forward);
    }

    #[test]
    fn planar_surface_picks_backward_when_normal_opposes_target() {
        // XY plane (normal +Z), target -Z → Backward.
        let plane = Plane::from_point_normal(Point3::ZERO, Vector3::Z)
            .expect("XY plane construction must succeed");
        let orientation = orient_face_for_outward(&plane, -Vector3::Z)
            .expect("midpoint normal is well-defined for a plane");
        assert_eq!(orientation, FaceOrientation::Backward);
    }

    #[test]
    fn cylinder_lateral_radial_outward_picks_forward() {
        // Cylinder axis = +Z at origin, radius 1. The intrinsic surface
        // normal at the parametric midpoint points radially outward
        // (+X at θ = 0, +Y at θ = π/2, etc.). Picking ANY radially
        // outward target should resolve to Forward.
        let cyl = Cylinder::new(Point3::ZERO, Vector3::Z, 1.0)
            .expect("unit cylinder along +Z must construct");
        let ((u_min, u_max), _) = cyl.parameter_bounds();
        let u_mid = 0.5 * (u_min + u_max);
        // Radial direction at u_mid: same as the cylinder's surface
        // point projected onto the radial plane.
        let n = cyl
            .normal_at(u_mid, 0.0)
            .expect("cylinder normal at midpoint is well-defined");
        // Target = the very normal we just sampled → must pick Forward.
        let orientation =
            orient_face_for_outward(&cyl, n).expect("cylinder normal is well-defined");
        assert_eq!(orientation, FaceOrientation::Forward);
        // Reversed target → must pick Backward.
        let orientation_rev =
            orient_face_for_outward(&cyl, -n).expect("cylinder normal is well-defined");
        assert_eq!(orientation_rev, FaceOrientation::Backward);
    }

    #[test]
    fn complementary_pair_rejects_same_half_space_targets() {
        let plane_a = Plane::from_point_normal(Point3::ZERO, Vector3::Z)
            .expect("plane construction must succeed");
        let plane_b = Plane::from_point_normal(Point3::ZERO, Vector3::Z)
            .expect("plane construction must succeed");
        // Both targets point +Z — caller bug.
        let result = orient_complementary_pair(&plane_a, Vector3::Z, &plane_b, Vector3::Z);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn complementary_pair_accepts_opposite_targets() {
        let plane_a = Plane::from_point_normal(Point3::ZERO, Vector3::Z)
            .expect("plane construction must succeed");
        let plane_b = Plane::from_point_normal(Point3::ZERO, Vector3::Z)
            .expect("plane construction must succeed");
        let (a, b) = orient_complementary_pair(&plane_a, Vector3::Z, &plane_b, -Vector3::Z)
            .expect("opposite targets must be accepted");
        assert_eq!(a, FaceOrientation::Forward);
        assert_eq!(b, FaceOrientation::Backward);
    }
}
