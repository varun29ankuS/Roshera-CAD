//! Trim-domain handling for the free-form narrow phase (CD-φ.5.3).
//!
//! A footpoint found on a face's *parent surface* only counts if it lies inside
//! the face's *trimmed* domain (Crozet Sec 4.4.1). Two pieces:
//!
//! * [`closest_point_on_face`] — the trimmed closest-point: Newton-solve the
//!   closest point on the supporting surface ([`crate::queries::newton`]), then
//!   the even-odd trim-loop test decides whether it falls inside the face. This
//!   is what the narrow phase calls to reject footpoints that drift off a trimmed
//!   patch onto its (untrimmed) parent surface.
//! * [`subdivide_2d_bezier`] — de Casteljau split of a 2D `(u,v)` trim curve. The
//!   restriction-curve primitive for adaptively bounding a trim boundary inside
//!   the patch-subdivision loop; the curve halves share their split point exactly
//!   and each reproduces the original over its half.

use crate::math::vector2::Vector2;
use crate::math::vector3::Point3;
use crate::primitives::face::FaceId;
use crate::primitives::topology_builder::BRepModel;
use crate::queries::newton::newton_closest_point;

/// The closest point of a *trimmed face* to a target, with the in-domain verdict.
#[derive(Debug, Clone, Copy)]
pub struct FaceClosestPoint {
    pub u: f64,
    pub v: f64,
    pub point: Point3,
    pub distance: f64,
    /// `true` if the footpoint lies inside the face's trim loops; `false` if it
    /// projects onto the parent surface *outside* the actual face.
    pub inside: bool,
}

/// Closest point on `face_id`'s supporting surface to `q`, plus whether it lies
/// inside the trimmed face. Seeds Newton from the surface's own closest-point
/// (analytic for canonical surfaces, a coarse hint otherwise), then runs the
/// even-odd trim test on the converged parameters.
pub fn closest_point_on_face(
    model: &BRepModel,
    face_id: FaceId,
    q: Point3,
) -> Option<FaceClosestPoint> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let (seed_u, seed_v) = surface
        .closest_point(&q, model.tolerance())
        .unwrap_or_else(|_| {
            let [u0, u1, v0, v1] = face.uv_bounds;
            (0.5 * (u0 + u1), 0.5 * (v0 + v1))
        });
    let nr = newton_closest_point(surface, q, seed_u, seed_v, 1e-10, 32)?;
    let inside = crate::tessellation::surface::point_inside_face_uv(nr.u, nr.v, face, model);
    Some(FaceClosestPoint {
        u: nr.u,
        v: nr.v,
        point: nr.point,
        distance: nr.distance,
        inside,
    })
}

/// de Casteljau split of a 2D Bézier trim curve at parameter `t ∈ [0,1]`.
/// Returns `(left, right)` control polygons covering `[0, t]` and `[t, 1]`; both
/// have the original degree, share the split point (`left.last() == right.first()`),
/// and reproduce the original curve over their sub-interval. Empty input → two
/// empty polygons.
pub fn subdivide_2d_bezier(control: &[Vector2], t: f64) -> (Vec<Vector2>, Vec<Vector2>) {
    let n = control.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut work = control.to_vec();
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    left.push(work[0]);
    right.push(work[n - 1]);

    let mut size = n;
    while size > 1 {
        for i in 0..size - 1 {
            work[i] = lerp(work[i], work[i + 1], t);
        }
        size -= 1;
        // After the pass, `work[0..size]` hold this level's points; the leading
        // edge is `work[0]` (→ left) and the trailing edge is `work[size-1]`.
        left.push(work[0]);
        right.push(work[size - 1]);
    }
    right.reverse();
    (left, right)
}

fn lerp(a: Vector2, b: Vector2, t: f64) -> Vector2 {
    a * (1.0 - t) + b * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::primitives::solid::SolidId;
    use crate::primitives::surface::Plane;
    use crate::primitives::topology_builder::TopologyBuilder;

    const X: Vector3 = Vector3::X;

    fn unit_box() -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let solid = model.solids.iter().last().map(|(id, _)| id).expect("solid");
        (model, solid)
    }

    /// The +X face of the box (plane at x = +1).
    fn plus_x_face(model: &BRepModel) -> FaceId {
        model
            .faces
            .iter()
            .find(|(_, face)| {
                model
                    .surfaces
                    .get(face.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>())
                    .map(|p| p.normal.dot(&X).abs() > 0.99 && p.origin.dot(&X) > 0.5)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id)
            .expect("+X face")
    }

    // -- trimmed closest point ---------------------------------------------

    #[test]
    fn point_in_front_of_face_projects_inside() {
        let (model, _) = unit_box();
        let face = plus_x_face(&model);
        // Directly in front of the face centre: closest point is (1,0,0), inside.
        let r = closest_point_on_face(&model, face, Vector3::new(3.0, 0.0, 0.0)).expect("cp");
        assert!((r.point - Vector3::new(1.0, 0.0, 0.0)).magnitude() < 1e-6);
        assert!((r.distance - 2.0).abs() < 1e-6);
        assert!(r.inside, "footpoint at the face centre must be inside");
    }

    #[test]
    fn point_off_to_the_side_projects_outside_the_trim() {
        let (model, _) = unit_box();
        let face = plus_x_face(&model);
        // In the face's plane direction but far off in +y: the closest surface
        // point (1, 5, 0) lies on the parent plane but outside the face.
        let r = closest_point_on_face(&model, face, Vector3::new(3.0, 5.0, 0.0)).expect("cp");
        assert!(
            !r.inside,
            "footpoint beyond the face extent must be rejected"
        );
    }

    // -- 2D trim-curve subdivision -----------------------------------------

    fn eval_2d_bezier(control: &[Vector2], t: f64) -> Vector2 {
        let mut work = control.to_vec();
        let mut size = work.len();
        while size > 1 {
            for i in 0..size - 1 {
                work[i] = lerp(work[i], work[i + 1], t);
            }
            size -= 1;
        }
        work[0]
    }

    #[test]
    fn subdivision_shares_split_point_and_endpoints() {
        let c = vec![
            Vector2::new(0.0, 0.0),
            Vector2::new(1.0, 2.0),
            Vector2::new(2.0, 0.0),
        ];
        let (left, right) = subdivide_2d_bezier(&c, 0.5);
        assert_eq!(left.len(), 3);
        assert_eq!(right.len(), 3);
        // Endpoints preserved.
        assert!((left[0] - c[0]).magnitude() < 1e-12);
        assert!((right[2] - c[2]).magnitude() < 1e-12);
        // Shared split point = the curve at t = 0.5.
        let mid = eval_2d_bezier(&c, 0.5);
        assert!((left[2] - mid).magnitude() < 1e-12);
        assert!((right[0] - mid).magnitude() < 1e-12);
        assert!((mid - Vector2::new(1.0, 1.0)).magnitude() < 1e-12);
    }

    #[test]
    fn halves_reproduce_the_original_curve() {
        let c = vec![
            Vector2::new(-1.0, 0.5),
            Vector2::new(0.0, 3.0),
            Vector2::new(2.0, -1.0),
            Vector2::new(3.0, 2.0),
        ];
        let split = 0.4;
        let (left, right) = subdivide_2d_bezier(&c, split);
        // A point in the left half: original at s == left at s/split.
        for &s in &[0.0, 0.1, 0.3, split] {
            let orig = eval_2d_bezier(&c, s);
            let half = eval_2d_bezier(&left, s / split);
            assert!((orig - half).magnitude() < 1e-9, "left mismatch at {s}");
        }
        // A point in the right half: original at s == right at (s-split)/(1-split).
        for &s in &[split, 0.6, 0.8, 1.0] {
            let orig = eval_2d_bezier(&c, s);
            let half = eval_2d_bezier(&right, (s - split) / (1.0 - split));
            assert!((orig - half).magnitude() < 1e-9, "right mismatch at {s}");
        }
    }
}
