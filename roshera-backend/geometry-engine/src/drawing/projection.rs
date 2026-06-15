//! Project a solid's edge network into 2D view space.
//!
//! The projection pipeline is intentionally simple:
//!
//! 1. Compute a world→view rotation matrix from the [`ProjectionType`].
//! 2. Walk every distinct edge of the solid's outer shell.
//! 3. Sample each underlying 3D curve at a fixed cadence (line ⇒ 2 pts,
//!    smooth curve ⇒ `samples_per_curve` pts).
//! 4. Multiply every sampled 3D point by the rotation, drop the
//!    third (view-Z) component, and emit a [`Polyline2d`].
//!
//! The output is a wireframe view: every topological edge appears in
//! the result regardless of visibility. Hidden-line / silhouette
//! removal is a deliberately deferred sub-slice (DRA-α follow-up):
//! good drawings require BRep ray-casting against face surfaces, which
//! is its own algorithmic block. For F0 the wireframe is sufficient
//! for inspection and dimensioning workflows.

use std::collections::HashSet;

use thiserror::Error;

use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

use super::types::{Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, ViewExtent};

/// Failure modes specific to drawing projection.
#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("solid {0:?} not found in model")]
    SolidNotFound(SolidId),
    #[error("solid {0:?} has no outer shell")]
    MissingShell(SolidId),
    #[error("curve {0:?} not found while sampling edge {1:?}")]
    MissingCurve(crate::primitives::curve::CurveId, EdgeId),
    #[error("curve evaluation failed at t={t}: {reason}")]
    CurveEvalFailed { t: f64, reason: String },
}

/// How many samples to draw along a non-linear curve segment. The
/// constant is conservative: drawing fidelity is bounded by SVG path
/// resolution, not by the kernel. 24 samples is enough to make a
/// 100mm circle look round on an A3 sheet without flooding the output.
pub const DEFAULT_CURVE_SAMPLES: usize = 24;

/// Construct the world→view rotation matrix for a given projection.
///
/// The matrix is a pure 3×3 rotation embedded in a 4×4: rows of the
/// 3×3 sub-matrix are the view-space basis vectors expressed in world
/// coordinates. Translation column is zero; the caller positions the
/// resulting 2D polylines on the sheet via
/// [`ProjectedView::position_mm`](super::types::ProjectedView::position_mm).
pub fn view_matrix_for_projection(projection: ProjectionType) -> Matrix4 {
    // For each projection we define:
    //   u (view-space X) = first row of the rotation
    //   v (view-space Y) = second row
    //   w (view-space Z, looking at the model from this direction) = third row
    //
    // Sign convention matches standard engineering drawings (Y up on
    // the page for Front/Right/Left, Z down for Top).
    let (u, v, w) = match projection {
        ProjectionType::Front => (
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, -1.0, 0.0),
        ),
        ProjectionType::Top => (
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, -1.0),
        ),
        ProjectionType::Right => (
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(-1.0, 0.0, 0.0),
        ),
        ProjectionType::Bottom => (
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ),
        ProjectionType::Left => (
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        ),
        ProjectionType::Isometric => {
            // Standard ISO axonometric: camera at (1,1,1)/√3, looking
            // toward the origin. The view-space u axis projects world
            // +X-and-+Y components, view-space v lifts +Z and tilts.
            let s = 1.0_f64 / 2.0_f64.sqrt();
            let t = 1.0_f64 / 6.0_f64.sqrt();
            let r = 1.0_f64 / 3.0_f64.sqrt();
            (
                Vector3::new(s, -s, 0.0),
                Vector3::new(t, t, 2.0 * t),
                Vector3::new(r, r, -r),
            )
        }
        ProjectionType::Custom { rotation } => {
            return Matrix4::new(
                rotation[0],
                rotation[1],
                rotation[2],
                0.0,
                rotation[3],
                rotation[4],
                rotation[5],
                0.0,
                rotation[6],
                rotation[7],
                rotation[8],
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
            );
        }
    };
    // Pack (u, v, w) as the three rows of the 4×4 rotation. The
    // resulting `transform_point` performs world→view directly.
    Matrix4::new(
        u.x, u.y, u.z, 0.0, v.x, v.y, v.z, 0.0, w.x, w.y, w.z, 0.0, 0.0, 0.0, 0.0, 1.0,
    )
}

/// Project every distinct edge of a solid into 2D view space.
///
/// Walks the solid's outer shell → faces → loops → edges, deduplicates
/// by `EdgeId`, samples each underlying curve, and returns one
/// [`Polyline2d`] per edge. Edges whose curve fails to evaluate at any
/// sample point are *skipped* (logged via the kernel's diagnostics
/// layer) so a single pathological curve does not poison the whole
/// view.
pub fn project_solid_edges(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    samples_per_curve: usize,
) -> Result<Vec<Polyline2d>, ProjectionError> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or(ProjectionError::SolidNotFound(solid_id))?;
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or(ProjectionError::MissingShell(solid_id))?;

    let view_matrix = view_matrix_for_projection(projection);

    let mut visited: HashSet<EdgeId> = HashSet::new();
    let mut polylines: Vec<Polyline2d> = Vec::new();

    for face_id in &shell.faces {
        let face = match model.faces.get(*face_id) {
            Some(f) => f,
            None => continue,
        };
        let loop_ids = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
        for loop_id in loop_ids {
            let topo_loop = match model.loops.get(loop_id) {
                Some(l) => l,
                None => continue,
            };
            for edge_id in &topo_loop.edges {
                if !visited.insert(*edge_id) {
                    continue;
                }
                let edge = match model.edges.get(*edge_id) {
                    Some(e) => e,
                    None => continue,
                };
                let curve = match model.curves.get(edge.curve_id) {
                    Some(c) => c,
                    None => continue,
                };

                // Sample count: 2 for genuinely linear edges, the full
                // budget for everything else. We treat a curve as
                // linear via the trait predicate so non-trivial Lines
                // with offset/rotation are still recognised.
                let is_linear = curve.is_linear(crate::math::Tolerance::default());
                let n_samples = if is_linear {
                    2
                } else {
                    samples_per_curve.max(2)
                };

                let t0 = edge.param_range.start;
                let t1 = edge.param_range.end;
                let mut pts: Vec<[f64; 2]> = Vec::with_capacity(n_samples);
                let mut sample_ok = true;
                for i in 0..n_samples {
                    let frac = i as f64 / (n_samples - 1) as f64;
                    let t = t0 + (t1 - t0) * frac;
                    match curve.point_at(t) {
                        Ok(p) => {
                            let v = view_matrix.transform_point(&p);
                            // Drop view-space Z. View-space X & Y are
                            // the page coordinates.
                            pts.push([v.x, v.y]);
                        }
                        Err(_) => {
                            sample_ok = false;
                            break;
                        }
                    }
                }
                if !sample_ok {
                    continue;
                }
                let polyline = Polyline2d::from_points(pts);
                // Drop edges that collapse to a single point in the
                // chosen projection (e.g. a vertical box edge in the
                // top view). They carry no information for the
                // wireframe.
                if polyline.points.len() < 2 {
                    continue;
                }
                polylines.push(polyline);
            }
        }
    }

    Ok(polylines)
}

/// One-shot: project a solid and build a complete [`ProjectedView`]
/// ready to drop into a [`Drawing`](super::types::Drawing).
///
/// `source` is the durable reference that will be stored on the view
/// — the caller resolves the source's part_id to the `BRepModel`
/// passed in `model`, and the projector uses the source's `solid_id`
/// internally to pick the right solid out of that model.
///
/// `name` should be a short human label (e.g. "Front", "Detail A").
/// `position_mm` is the sheet-space placement of the view's local
/// origin; `scale` is the view-to-sheet scale factor.
pub fn project_solid_view(
    model: &BRepModel,
    source: super::types::ViewSource,
    projection: ProjectionType,
    name: impl Into<String>,
    position_mm: [f64; 2],
    scale: f64,
) -> Result<ProjectedView, ProjectionError> {
    let solid_id = match source {
        super::types::ViewSource::Part { solid_id, .. } => solid_id,
    };
    let polylines = project_solid_edges(model, solid_id, projection, DEFAULT_CURVE_SAMPLES)?;
    let mut extent = ViewExtent::empty();
    for pl in &polylines {
        for p in &pl.points {
            extent.include(*p);
        }
    }
    Ok(ProjectedView {
        id: ProjectedViewId::new(),
        name: name.into(),
        projection,
        source,
        position_mm,
        scale,
        polylines,
        extent,
        dimensions: Vec::new(),
    })
}

/// Convenience accessor used by some callers (project a single 3D
/// point through a projection). Exposed so REST/test code can spot-
/// check the projection axes without re-deriving the matrix.
pub fn project_point(projection: ProjectionType, point: Point3) -> [f64; 2] {
    let m = view_matrix_for_projection(projection);
    let v = m.transform_point(&point);
    [v.x, v.y]
}
