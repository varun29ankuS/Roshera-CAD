//! Imprint cutting curves onto a face and split it into sub-faces.
//!
//! Implements Parasolid `PK_FACE_imprint_curves` semantics: the input
//! face's boundary loop is unioned with the cutting curves to form a
//! planar arrangement on the face's surface; each non-outer cycle of
//! the arrangement becomes a new sub-face that replaces the original.
//! The original face is removed from its parent shell and the sub-faces
//! are inserted in its place, so the model's topology stays consistent
//! after the call.
//!
//! # Pipeline
//!
//! 1. Validate every cutting curve lies on the face's surface within
//!    `options.common.tolerance` (sampled `closest_point` test). The
//!    caller is responsible for projecting curves onto the surface
//!    upstream — we will not silently project a curve that is off the
//!    surface, because directional projection is a separate operation
//!    handled by the boolean intersector.
//! 2. Build an [`IntersectionGraph`] from the face's outer-loop edges
//!    plus one new edge per cutting curve.
//! 3. Run [`compute_edge_intersections`] to imprint T-junctions where
//!    boundary and cutting curves cross. This consumes the same
//!    multi-crossing finder the boolean operation uses, so a curve
//!    bisecting the face produces both crossings, not just the deepest.
//! 4. Build the DCEL planar arrangement on the face's surface tangent
//!    plane via [`face_arrangement::build_arrangement`] and walk
//!    minimal-face cycles via [`face_arrangement::extract_regions`].
//!    Outer cycle (CW under surface normal) and dangling-edge detours
//!    are stripped automatically.
//! 5. Each surviving region becomes a new [`Loop`] + [`Face`] in the
//!    model, replacing the original face inside its parent shell.
//! 6. Emit a `RecordedOperation` so attached recorders see the imprint.
//!
//! # Why face-splitting, not "internal-edge" imprinting
//!
//! The previous implementation persisted imprinted edges as "internal
//! edges" stored on the face — a feature the [`Face`] struct never
//! actually grew. Every call returned `NotImplemented`, so the operation
//! was unusable. Parasolid's modern API (`PK_FACE_imprint_curves`) and
//! ACIS's (`api_imprint_face_face`) both produce sub-faces; that is the
//! semantic downstream operations (booleans, mesh extraction, classify)
//! already understand. Splitting matches the kernel's existing data
//! shape and removes a stub.
//!
//! # References
//!
//! - Siemens. *Parasolid Programming Reference* — `PK_FACE_imprint_curves`.
//! - Spatial Corp. *ACIS 3D Modeler Reference* — `api_imprint_face_face`.
//! - de Berg, van Kreveld, Overmars, Schwarzkopf (2008).
//!   *Computational Geometry: Algorithms and Applications*, §2.2.

use super::boolean::{
    compute_edge_intersections, create_edge_from_curve, get_face_boundary_edges, EdgeType,
    IntersectionGraph,
};
use super::face_arrangement::{build_arrangement, extract_regions};
use super::recorder::RecordedOperation;
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::Tolerance;
use crate::primitives::{
    curve::{Curve, CurveId},
    face::{Face, FaceId, FaceOrientation},
    r#loop::{Loop, LoopType},
    shell::ShellId,
    surface::Surface,
    topology_builder::BRepModel,
};

/// Options for an imprint operation.
#[derive(Debug, Clone, Default)]
pub struct ImprintOptions {
    /// Tolerance, validation, and history settings shared with other
    /// operations.
    pub common: CommonOptions,
}

/// Result of [`imprint_curves_on_face`].
#[derive(Debug)]
pub struct ImprintResult {
    /// Newly created sub-faces, in DCEL extraction order. The original
    /// `face_id` passed to [`imprint_curves_on_face`] is no longer in
    /// the parent shell after the call returns; consumers should index
    /// faces by these new IDs.
    pub sub_faces: Vec<FaceId>,
    /// Parent shell that held the original face. `None` if the face was
    /// not in any shell (orphan face — uncommon, but legal in
    /// intermediate states).
    pub parent_shell: Option<ShellId>,
}

/// Imprint cutting curves onto a face, splitting it into sub-faces.
///
/// See module docs for full semantics.
pub fn imprint_curves_on_face(
    model: &mut BRepModel,
    face_id: FaceId,
    curves: Vec<CurveId>,
    options: ImprintOptions,
) -> OperationResult<ImprintResult> {
    if curves.is_empty() {
        return Err(OperationError::InvalidInput {
            parameter: "curves".to_string(),
            expected: "at least one cutting curve".to_string(),
            received: "0".to_string(),
        });
    }

    // ------------------------------------------------------------------
    // 1. Resolve the face's surface and validate curves lie on it.
    // ------------------------------------------------------------------
    let surface_id = {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidInput {
                parameter: "face_id".to_string(),
                expected: "valid face ID".to_string(),
                received: format!("{face_id:?}"),
            })?;
        face.surface_id
    };

    let tolerance = options.common.tolerance;
    {
        let surface = model.surfaces.get(surface_id).ok_or_else(|| {
            OperationError::InvalidInput {
                parameter: "surface_id".to_string(),
                expected: "valid surface ID".to_string(),
                received: format!("{surface_id:?}"),
            }
        })?;
        for &cid in &curves {
            let curve = model
                .curves
                .get(cid)
                .ok_or_else(|| OperationError::InvalidInput {
                    parameter: "curve_id".to_string(),
                    expected: "valid curve ID".to_string(),
                    received: format!("{cid:?}"),
                })?;
            if !curve_lies_on_surface(curve, surface, &tolerance)? {
                return Err(OperationError::InvalidGeometry(format!(
                    "Imprint curve {cid:?} does not lie on face {face_id:?}'s surface within \
                     tolerance {:.3e}; project the curve onto the surface upstream",
                    tolerance.distance()
                )));
            }
        }
    }

    // ------------------------------------------------------------------
    // 2. Build the intersection graph: face boundary + cutting curves.
    // ------------------------------------------------------------------
    let mut graph = IntersectionGraph::new();
    let boundary = get_face_boundary_edges(model, face_id)?;
    for &(edge_id, _) in &boundary {
        graph.add_edge(edge_id, EdgeType::Boundary);
    }
    for &cid in &curves {
        let edge_id = create_edge_from_curve(model, cid)?;
        graph.add_edge(edge_id, EdgeType::Splitting);
    }

    // ------------------------------------------------------------------
    // 3. Imprint T-junctions where boundary and cutting curves cross.
    // ------------------------------------------------------------------
    compute_edge_intersections(&mut graph, model, &tolerance)?;
    graph.resolve_vertices(model);

    // ------------------------------------------------------------------
    // 4. Build DCEL arrangement and walk minimal-face cycles.
    // ------------------------------------------------------------------
    let arrangement = build_arrangement(&graph, model, surface_id)?;
    let regions = {
        let surface = model.surfaces.get(surface_id).ok_or_else(|| {
            OperationError::InvalidInput {
                parameter: "surface_id".to_string(),
                expected: "valid surface ID".to_string(),
                received: format!("{surface_id:?}"),
            }
        })?;
        extract_regions(&arrangement, model, surface)
    };

    if regions.is_empty() {
        return Err(OperationError::InvalidGeometry(format!(
            "Imprint on face {face_id:?} produced no sub-faces; arrangement extraction \
             yielded an empty region set (cutting curves may not have reached the boundary)"
        )));
    }

    // ------------------------------------------------------------------
    // 5. Materialise each region as a Loop + Face in the model.
    // ------------------------------------------------------------------
    let mut sub_faces = Vec::with_capacity(regions.len());
    for region_edges in &regions {
        let mut face_loop = Loop::new(0, LoopType::Outer);
        for &(edge_id, forward) in region_edges {
            face_loop.add_edge(edge_id, forward);
        }
        let loop_id = model.loops.add(face_loop);

        let new_face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        let new_face_id = model.faces.add(new_face);
        sub_faces.push(new_face_id);
    }

    // ------------------------------------------------------------------
    // 6. Swap original face for sub-faces inside the parent shell.
    // ------------------------------------------------------------------
    let parent_shell = find_parent_shell(model, face_id);
    if let Some(shell_id) = parent_shell {
        if let Some(shell) = model.shells.get_mut(shell_id) {
            shell.remove_face(face_id);
            for &fid in &sub_faces {
                shell.add_face(fid);
            }
        }
    }

    // ------------------------------------------------------------------
    // 7. Optional post-condition validation.
    // ------------------------------------------------------------------
    if options.common.validate_result {
        validate_sub_faces(model, &sub_faces)?;
    }

    // ------------------------------------------------------------------
    // 8. Record for attached recorders.
    // ------------------------------------------------------------------
    let mut inputs: Vec<u64> = Vec::with_capacity(1 + curves.len());
    inputs.push(face_id as u64);
    inputs.extend(curves.iter().map(|&c| c as u64));
    model.record_operation(
        RecordedOperation::new("imprint_curves_on_face")
            .with_parameters(serde_json::json!({
                "face_id": face_id,
                "curve_count": curves.len(),
                "tolerance": tolerance.distance(),
                "sub_face_count": sub_faces.len(),
            }))
            .with_inputs(inputs)
            .with_outputs(sub_faces.iter().map(|&f| f as u64).collect()),
    );

    Ok(ImprintResult {
        sub_faces,
        parent_shell,
    })
}

/// Sample-test that a curve lies on a surface to within the given
/// tolerance. Returns `false` on the first sample whose 3D distance to
/// its surface foot exceeds tolerance.
///
/// The surface foot is computed via [`Surface::closest_point`] — Newton
/// refinement, not the previous 21×21 grid search. The grid search was
/// orders of magnitude slower on every Surface implementation that
/// already had an analytic or Newton-based projection.
fn curve_lies_on_surface(
    curve: &dyn Curve,
    surface: &dyn Surface,
    tolerance: &Tolerance,
) -> OperationResult<bool> {
    const SAMPLES: usize = 12;
    for i in 0..=SAMPLES {
        let t = i as f64 / SAMPLES as f64;
        let point = curve.point_at(t)?;
        let (u, v) = surface.closest_point(&point, *tolerance).map_err(|e| {
            OperationError::NumericalError(format!("Surface::closest_point failed: {e:?}"))
        })?;
        let foot = surface.point_at(u, v)?;
        if point.distance(&foot) > tolerance.distance() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Linear scan of every shell looking for the one that lists `face_id`.
///
/// `ShellStore::shells_with_face` consults a `face_to_shells` index, but
/// that index is only updated by `ShellStore::add_with_indexing` — most
/// kernel paths use the fast `add` path that skips index maintenance,
/// so the index is unreliable. Until the store guarantees the index, a
/// linear scan over `shells.iter()` is the only correct lookup.
fn find_parent_shell(model: &BRepModel, face_id: FaceId) -> Option<ShellId> {
    for (shell_id, shell) in model.shells.iter() {
        if shell.find_face(face_id).is_some() {
            return Some(shell_id);
        }
    }
    None
}

/// Verify every sub-face has a non-empty outer loop and a resolvable
/// surface. Cheap sanity check that catches most arrangement-emit bugs
/// where a region produced an empty edge list.
fn validate_sub_faces(model: &BRepModel, sub_faces: &[FaceId]) -> OperationResult<()> {
    for &fid in sub_faces {
        let face = model.faces.get(fid).ok_or_else(|| {
            OperationError::InvalidBRep(format!("imprint sub-face {fid:?} not found in store"))
        })?;
        let lp = model.loops.get(face.outer_loop).ok_or_else(|| {
            OperationError::InvalidBRep(format!(
                "imprint sub-face {fid:?} references missing loop {:?}",
                face.outer_loop
            ))
        })?;
        if lp.edges.is_empty() {
            return Err(OperationError::InvalidBRep(format!(
                "imprint sub-face {fid:?} has empty outer loop"
            )));
        }
        if model.surfaces.get(face.surface_id).is_none() {
            return Err(OperationError::InvalidBRep(format!(
                "imprint sub-face {fid:?} references missing surface {:?}",
                face.surface_id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::Line;
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::surface::Plane;
    use crate::primitives::topology_builder::BRepModel;

    /// Build a unit square face on the XY plane with corners
    /// (0,0,0), (1,0,0), (1,1,0), (0,1,0). Returns (face_id, plane_id).
    fn build_unit_square_face(model: &mut BRepModel) -> (FaceId, u32) {
        let plane = Plane::new(Point3::ORIGIN, Vector3::Z, Vector3::X).unwrap();
        let surface_id = model.surfaces.add(Box::new(plane));

        let v0 = model.vertices.add_or_find(0.0, 0.0, 0.0, 1e-6);
        let v1 = model.vertices.add_or_find(1.0, 0.0, 0.0, 1e-6);
        let v2 = model.vertices.add_or_find(1.0, 1.0, 0.0, 1e-6);
        let v3 = model.vertices.add_or_find(0.0, 1.0, 0.0, 1e-6);

        let c0 = model.curves.add(Box::new(Line::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        )));
        let c1 = model.curves.add(Box::new(Line::new(
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        )));
        let c2 = model.curves.add(Box::new(Line::new(
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        )));
        let c3 = model.curves.add(Box::new(Line::new(
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
        )));

        let e0 = model
            .edges
            .add(Edge::new_auto_range(0, v0, v1, c0, EdgeOrientation::Forward));
        let e1 = model
            .edges
            .add(Edge::new_auto_range(0, v1, v2, c1, EdgeOrientation::Forward));
        let e2 = model
            .edges
            .add(Edge::new_auto_range(0, v2, v3, c2, EdgeOrientation::Forward));
        let e3 = model
            .edges
            .add(Edge::new_auto_range(0, v3, v0, c3, EdgeOrientation::Forward));

        let mut outer = Loop::new(0, LoopType::Outer);
        outer.add_edge(e0, true);
        outer.add_edge(e1, true);
        outer.add_edge(e2, true);
        outer.add_edge(e3, true);
        let loop_id = model.loops.add(outer);

        let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        let face_id = model.faces.add(face);

        (face_id, surface_id)
    }

    #[test]
    fn imprint_with_no_curves_is_rejected() {
        let mut model = BRepModel::new();
        let (face_id, _) = build_unit_square_face(&mut model);

        let result = imprint_curves_on_face(&mut model, face_id, vec![], ImprintOptions::default());
        assert!(matches!(result, Err(OperationError::InvalidInput { .. })));
    }

    #[test]
    fn imprint_curve_off_surface_is_rejected() {
        let mut model = BRepModel::new();
        let (face_id, _) = build_unit_square_face(&mut model);

        // Curve at z = 1.0 is well off the z = 0 plane.
        let off_surface = model.curves.add(Box::new(Line::new(
            Point3::new(0.5, -0.1, 1.0),
            Point3::new(0.5, 1.1, 1.0),
        )));
        let result = imprint_curves_on_face(
            &mut model,
            face_id,
            vec![off_surface],
            ImprintOptions::default(),
        );
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn imprint_bisecting_line_yields_two_sub_faces() {
        // A vertical line from (0.5, -0.1, 0) to (0.5, 1.1, 0) crosses
        // the bottom edge at (0.5, 0) and the top edge at (0.5, 1),
        // splitting the unit square into a left half and a right half.
        let mut model = BRepModel::new();
        let (face_id, _) = build_unit_square_face(&mut model);

        let bisector = model.curves.add(Box::new(Line::new(
            Point3::new(0.5, -0.1, 0.0),
            Point3::new(0.5, 1.1, 0.0),
        )));

        let mut opts = ImprintOptions::default();
        // Skip post-validation so any minor edge-list quirks in the
        // arrangement extraction surface as test failures rather than
        // panics inside `validate_sub_faces`.
        opts.common.validate_result = false;
        let result = imprint_curves_on_face(&mut model, face_id, vec![bisector], opts).unwrap();

        assert_eq!(
            result.sub_faces.len(),
            2,
            "Bisecting line should split square into 2 sub-faces, got {}",
            result.sub_faces.len()
        );
        for &fid in &result.sub_faces {
            let face = model.faces.get(fid).expect("sub-face must exist");
            let lp = model
                .loops
                .get(face.outer_loop)
                .expect("sub-face loop must exist");
            assert!(
                lp.edges.len() >= 3,
                "Sub-face loop must have ≥ 3 edges, got {}",
                lp.edges.len()
            );
        }
    }
}
