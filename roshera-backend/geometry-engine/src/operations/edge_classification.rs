//! Edge classification cache (F2-α).
//!
//! Every edge in a closed shell has a well-defined classification:
//! - **manifold kind** — boundary (1 face use), manifold (2), or
//!   non-manifold (≥3);
//! - **signed dihedral angle** at the midpoint — positive ⇒ convex,
//!   negative ⇒ concave, magnitude ≈ π ⇒ flat;
//! - **convexity** — sign of the dihedral, with a tolerance band
//!   around zero classified as straight (0);
//! - **smoothness** — sharp if the dihedral departs from π by more
//!   than the angular tolerance, smooth (G1) otherwise.
//!
//! Today every consumer that needs this data recomputes it from
//! scratch (`fillet::compute_face_angle`, `chamfer::get_adjacent_faces`,
//! the boolean shell-walking diagnostics, …). F2-α stamps the
//! classification onto [`EdgeAttributes`] at construction time so
//! downstream code (blend graph, sewing, healing) can read it
//! directly. Cache invalidation is explicit: any operation that
//! replaces an adjacent face calls [`invalidate_face_neighbours`].
//!
//! Why this module sits in `operations/` and not `primitives/`:
//! computing the signed dihedral requires `get_face_oriented_normal`
//! and `robust_face_angle` from `operations::fillet` and
//! `operations::fillet_robust`. Those helpers are themselves
//! production-grade (they bake in the outward-normal invariant from
//! Slices 2/3) and live in `operations/`. Moving them down to
//! `primitives/` would invert the dependency edge.

use crate::math::{Tolerance, Vector3};
use crate::operations::fillet::{edge_orientation_in_face, get_face_oriented_normal};
use crate::operations::fillet_robust::robust_face_angle;
use crate::operations::{OperationError, OperationResult};
use crate::primitives::edge::{EdgeId, ManifoldKind};
use crate::primitives::face::FaceId;
use crate::primitives::topology_builder::BRepModel;

/// Outcome of classifying an edge — a snapshot of the topology-derived
/// attributes that F2-α caches on [`crate::primitives::edge::EdgeAttributes`].
///
/// Returned by [`classify_edge`] and consumed by [`classify_and_cache`].
/// Holding it as a separate struct (rather than just mutating attributes
/// in place) lets callers inspect the classification *before* deciding
/// whether to stamp it — useful in validation and diagnostic paths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeClassification {
    /// Manifold classification based on face-usage count.
    pub manifold_kind: ManifoldKind,
    /// Signed dihedral at the edge midpoint, radians, in `(-π, π]`.
    /// `None` ⇒ no two-face dihedral exists (boundary or non-manifold),
    /// or the dihedral failed to compute robustly.
    pub dihedral_angle: Option<f64>,
    /// Convexity derived from the dihedral sign: `+1` convex, `-1`
    /// concave, `0` straight / flat / undefined.
    pub convexity: i8,
    /// `1.0` for sharp edges, `0.0` for G1-smooth (dihedral within
    /// `angle_tol` of ±π).
    pub sharpness: f32,
}

impl EdgeClassification {
    /// The unclassified result returned for edges that have no
    /// adjacent faces in the model (orphaned edges) or whose
    /// neighbourhood is otherwise undefined.
    pub const UNCLASSIFIED: Self = Self {
        manifold_kind: ManifoldKind::Unknown,
        dihedral_angle: None,
        convexity: 0,
        sharpness: 0.0,
    };

    /// `true` if this edge is interior to a closed manifold shell
    /// (the standard case blends, sewing and healing care about).
    #[inline(always)]
    pub fn is_manifold(&self) -> bool {
        matches!(self.manifold_kind, ManifoldKind::Manifold)
    }

    /// Typed convex / concave / G1 view of the cached signed dihedral.
    /// `None` when no two-face dihedral exists (boundary, non-manifold,
    /// or a robust-angle failure left `dihedral_angle` undefined).
    pub fn dihedral_class(&self) -> Option<DihedralClass> {
        self.dihedral_angle?;
        Some(match self.convexity {
            1 => DihedralClass::Convex,
            -1 => DihedralClass::Concave,
            _ => DihedralClass::G1Smooth,
        })
    }

    /// `true` iff this edge is a G1-smooth (tangent-continuous) join —
    /// the case that carries no LMD footpoint and is dropped from CD.
    #[inline]
    pub fn is_g1(&self) -> bool {
        matches!(self.dihedral_class(), Some(DihedralClass::G1Smooth))
    }
}

/// Convex / concave / G1 classification of a manifold edge's dihedral.
///
/// Sign convention matches thesis Eq 1.27 (`α = ⟨n₁ × n₂, t₁₂⟩`):
/// `Convex` for `0 < α < π`, `Concave` for `−π < α < 0`, `G1Smooth` at
/// `α ≈ 0` (the two faces share a tangent plane). G1 edges carry no LMD
/// footpoint and are dropped from collision detection; convex and concave
/// edges receive different normal-cone treatment downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DihedralClass {
    /// Tangent-continuous join (`α ≈ 0`).
    G1Smooth,
    /// Convex edge (`0 < α < π`) — material removed at the join.
    Convex,
    /// Concave edge (`−π < α < 0`) — material added at the join.
    Concave,
}

/// Find every face whose outer or inner loop references `edge_id`.
///
/// Walks all shells of all solids — F2-α is a model-wide pass, not a
/// per-solid one. The returned vector preserves discovery order so
/// callers that pick the first two for dihedral computation get
/// deterministic results across runs.
pub fn find_adjacent_faces(model: &BRepModel, edge_id: EdgeId) -> Vec<FaceId> {
    let mut faces = Vec::with_capacity(2);
    let mut visited = std::collections::HashSet::with_capacity(8);

    let loops = &model.loops;
    let face_store = &model.faces;

    for shell_entry in model.shells.iter() {
        let (_shell_id, shell) = shell_entry;
        for &face_id in &shell.faces {
            if visited.contains(&face_id) {
                continue;
            }
            let face = match face_store.get(face_id) {
                Some(f) => f,
                None => continue,
            };

            let mut touches = false;
            if let Some(loop_data) = loops.get(face.outer_loop) {
                if loop_data.edges.iter().any(|&e| e == edge_id) {
                    touches = true;
                }
            }
            if !touches {
                for &inner_loop in &face.inner_loops {
                    if let Some(loop_data) = loops.get(inner_loop) {
                        if loop_data.edges.iter().any(|&e| e == edge_id) {
                            touches = true;
                            break;
                        }
                    }
                }
            }

            if touches {
                visited.insert(face_id);
                faces.push(face_id);
            }
        }
    }

    faces
}

/// Classify a single edge by walking its face neighbourhood.
///
/// Read-only — does not mutate the model. Returns
/// [`EdgeClassification::UNCLASSIFIED`] when the edge is orphaned
/// (no adjacent faces). For the boundary / non-manifold cases the
/// `dihedral_angle` field is left `None` because no single signed
/// dihedral exists.
pub fn classify_edge(model: &BRepModel, edge_id: EdgeId) -> OperationResult<EdgeClassification> {
    if model.edges.get(edge_id).is_none() {
        return Ok(EdgeClassification::UNCLASSIFIED);
    }

    let faces = find_adjacent_faces(model, edge_id);

    match faces.len() {
        0 => Ok(EdgeClassification::UNCLASSIFIED),
        1 => Ok(EdgeClassification {
            manifold_kind: ManifoldKind::Boundary,
            dihedral_angle: None,
            convexity: 0,
            sharpness: 1.0,
        }),
        2 => classify_manifold_edge(model, edge_id, faces[0], faces[1]),
        _ => Ok(EdgeClassification {
            manifold_kind: ManifoldKind::NonManifold,
            dihedral_angle: None,
            convexity: 0,
            sharpness: 1.0,
        }),
    }
}

/// Classify `edge_id` and return its typed [`DihedralClass`], or `None`
/// for boundary / non-manifold / undefined edges. Read-only convenience
/// over [`classify_edge`] for the topology consumers (supermaximal
/// grouping, CD) that want the enum rather than the raw i8/f32.
pub fn classify_dihedral(
    model: &BRepModel,
    edge_id: EdgeId,
) -> OperationResult<Option<DihedralClass>> {
    Ok(classify_edge(model, edge_id)?.dihedral_class())
}

/// Two-face dihedral classification. The convexity-sign convention
/// matches `fillet::compute_face_angle` exactly — positive dihedrals
/// are convex, negative are concave — because we reuse the same
/// `(oriented_normal_1, oriented_normal_2, loop-aligned_tangent)`
/// triple that [`robust_face_angle`] consumes.
fn classify_manifold_edge(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<EdgeClassification> {
    let edge = match model.edges.get(edge_id) {
        Some(e) => e,
        None => return Ok(EdgeClassification::UNCLASSIFIED),
    };

    let edge_midpoint = edge
        .evaluate(0.5, &model.curves)
        .map_err(|e| OperationError::NumericalError(format!("midpoint eval failed: {:?}", e)))?;

    let face1_normal: Vector3 = get_face_oriented_normal(model, face1_id, &edge_midpoint)?;
    let face2_normal: Vector3 = get_face_oriented_normal(model, face2_id, &edge_midpoint)?;

    let face1_loop_sign = match edge_orientation_in_face(model, face1_id, edge_id) {
        Some(s) => s,
        None => {
            return Ok(EdgeClassification {
                manifold_kind: ManifoldKind::Manifold,
                dihedral_angle: None,
                convexity: 0,
                sharpness: 1.0,
            });
        }
    };

    let edge_tangent = edge
        .tangent_at(0.5, &model.curves)
        .map_err(|e| OperationError::NumericalError(format!("tangent eval failed: {:?}", e)))?
        * face1_loop_sign;

    let tolerance = Tolerance::default();
    let dihedral = match robust_face_angle(&face1_normal, &face2_normal, &edge_tangent, &tolerance)
    {
        Ok(a) => a,
        // Robust-angle failure (degenerate normals, zero-length tangent) ⇒
        // record manifold kind but leave the dihedral undefined.
        Err(_) => {
            return Ok(EdgeClassification {
                manifold_kind: ManifoldKind::Manifold,
                dihedral_angle: None,
                convexity: 0,
                sharpness: 1.0,
            });
        }
    };

    // `robust_face_angle` returns the angle between the two outward
    // normals projected onto the plane perpendicular to the edge.
    // For an exterior convex edge of a solid (e.g. a box corner) the
    // two outward normals diverge — the projected angle is positive
    // and in `(0, π)`. For a concave / interior edge the projected
    // angle is negative. A perfectly flat (smooth G1) pair has the
    // two normals aligned, so the projected angle is ≈ 0.
    let angle_tol = tolerance.angle();
    let convexity: i8 = if dihedral.abs() < angle_tol {
        0
    } else if dihedral > 0.0 {
        1
    } else {
        -1
    };
    let sharpness: f32 = if dihedral.abs() < angle_tol { 0.0 } else { 1.0 };

    Ok(EdgeClassification {
        manifold_kind: ManifoldKind::Manifold,
        dihedral_angle: Some(dihedral),
        convexity,
        sharpness,
    })
}

/// Classify `edge_id` and stamp the result onto its attributes. No-op
/// if the edge is already classified. Returns the classification that
/// was just installed (or the existing one on a cache hit) so callers
/// can branch on it without re-reading.
pub fn classify_and_cache(
    model: &mut BRepModel,
    edge_id: EdgeId,
) -> OperationResult<EdgeClassification> {
    if let Some(edge) = model.edges.get(edge_id) {
        if edge.attributes.is_classified() {
            return Ok(EdgeClassification {
                manifold_kind: edge.attributes.manifold_kind,
                dihedral_angle: edge.attributes.dihedral_angle,
                convexity: edge.attributes.convexity,
                sharpness: edge.attributes.sharpness,
            });
        }
    }

    let classification = classify_edge(model, edge_id)?;

    if let Some(edge) = model.edges.get_mut(edge_id) {
        edge.attributes.manifold_kind = classification.manifold_kind;
        edge.attributes.dihedral_angle = classification.dihedral_angle;
        edge.attributes.convexity = classification.convexity;
        edge.attributes.sharpness = classification.sharpness;
    }

    Ok(classification)
}

/// Drop the classification cache on every edge that currently belongs
/// to (any loop of) `face_id`. Called immediately *before* a face is
/// replaced or its surface is mutated; the next classify-and-cache
/// pass on these edges will re-compute against the new neighbourhood.
///
/// Returns the number of edges whose cache was invalidated — useful
/// for diagnostics, never load-bearing.
pub fn invalidate_face_neighbours(model: &mut BRepModel, face_id: FaceId) -> usize {
    let face = match model.faces.get(face_id) {
        Some(f) => f,
        None => return 0,
    };
    let outer = face.outer_loop;
    let inner_loops: Vec<_> = face.inner_loops.clone();

    let mut edge_ids: Vec<EdgeId> = Vec::new();
    if let Some(loop_data) = model.loops.get(outer) {
        edge_ids.extend(loop_data.edges.iter().copied());
    }
    for inner in inner_loops {
        if let Some(loop_data) = model.loops.get(inner) {
            edge_ids.extend(loop_data.edges.iter().copied());
        }
    }

    let mut invalidated = 0usize;
    for edge_id in edge_ids {
        if let Some(edge) = model.edges.get_mut(edge_id) {
            if edge.attributes.is_classified() {
                edge.attributes.invalidate();
                invalidated += 1;
            }
        }
    }
    invalidated
}

/// Sweep over every edge in the model and stamp any unclassified
/// edge with a fresh classification. Cheap idempotent — already-
/// classified edges are skipped without re-walking their face
/// neighbourhood.
///
/// Intended as the kernel-internal hook called at the tail of major
/// operations (extrude, revolve, fillet, …) so downstream consumers
/// always see classified edges. Wiring at op sites is incremental
/// and lives in subsequent sub-slices.
pub fn classify_all_unclassified_edges(model: &mut BRepModel) -> OperationResult<usize> {
    let edge_ids: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
    let mut stamped = 0usize;
    for edge_id in edge_ids {
        let already = model
            .edges
            .get(edge_id)
            .map(|e| e.attributes.is_classified())
            .unwrap_or(true);
        if already {
            continue;
        }
        classify_and_cache(model, edge_id)?;
        stamped += 1;
    }
    Ok(stamped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::edge::ManifoldKind;
    use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};

    /// Build a unit-box solid via the same `TopologyBuilder::create_box_3d`
    /// path used by `fillet_chamfer_dihedral_matrix.rs`. Keeps F2-α's
    /// fixture byte-for-byte compatible with the rest of the regression
    /// suite, so any future drift in primitive construction shows up in
    /// both test files at the same time.
    fn build_unit_box() -> BRepModel {
        let mut model = BRepModel::new();
        {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(1.0, 1.0, 1.0)
                .expect("create_box_3d should succeed for a unit cube");
        }
        model
    }

    #[test]
    fn unit_box_every_edge_classifies_as_convex_manifold() {
        let mut model = build_unit_box();
        let stamped = classify_all_unclassified_edges(&mut model)
            .expect("classification sweep should succeed on a unit box");

        // A unit cube has 12 edges; each is shared by exactly two
        // adjacent faces, and the dihedral at every edge is π/2
        // (90° exterior).
        assert_eq!(stamped, 12, "unit box has exactly 12 edges to classify");

        let mut classified = 0;
        for (eid, edge) in model.edges.iter() {
            assert_eq!(
                edge.attributes.manifold_kind,
                ManifoldKind::Manifold,
                "edge {} should be manifold (2-face) on a closed box",
                eid
            );
            let dihedral = edge.attributes.dihedral_angle.unwrap_or_else(|| {
                panic!(
                    "edge {} should have a defined dihedral on a manifold cube",
                    eid
                )
            });
            assert!(
                (dihedral.abs() - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
                "edge {} dihedral should be ±π/2, got {}",
                eid,
                dihedral
            );
            assert_eq!(
                edge.attributes.convexity, 1,
                "exterior cube edge {} should be convex (+1), got {}",
                eid, edge.attributes.convexity
            );
            assert!(
                (edge.attributes.sharpness - 1.0).abs() < f32::EPSILON,
                "cube edge {} should be sharp (1.0)",
                eid
            );
            classified += 1;
        }
        assert_eq!(classified, 12);
    }

    #[test]
    fn box_edges_classify_as_convex_dihedral() {
        let mut model = build_unit_box();
        classify_all_unclassified_edges(&mut model).expect("classify");
        for (eid, _) in model.edges.iter() {
            let class = classify_dihedral(&model, eid)
                .expect("classify_dihedral should succeed")
                .expect("a box edge has a defined dihedral");
            assert_eq!(class, DihedralClass::Convex, "box edge {} should be convex", eid);
        }
    }

    #[test]
    fn dihedral_class_maps_convexity_and_g1() {
        let convex = EdgeClassification {
            manifold_kind: ManifoldKind::Manifold,
            dihedral_angle: Some(0.5),
            convexity: 1,
            sharpness: 1.0,
        };
        assert_eq!(convex.dihedral_class(), Some(DihedralClass::Convex));
        assert!(!convex.is_g1());

        let concave = EdgeClassification {
            convexity: -1,
            dihedral_angle: Some(-0.5),
            ..convex
        };
        assert_eq!(concave.dihedral_class(), Some(DihedralClass::Concave));

        let g1 = EdgeClassification {
            convexity: 0,
            dihedral_angle: Some(0.0),
            sharpness: 0.0,
            ..convex
        };
        assert_eq!(g1.dihedral_class(), Some(DihedralClass::G1Smooth));
        assert!(g1.is_g1());

        // Boundary / undefined edges have no dihedral class.
        assert_eq!(EdgeClassification::UNCLASSIFIED.dihedral_class(), None);
        assert!(!EdgeClassification::UNCLASSIFIED.is_g1());
    }

    #[test]
    fn idempotent_classify_is_a_no_op() {
        let mut model = build_unit_box();
        let first = classify_all_unclassified_edges(&mut model).expect("first sweep");
        assert_eq!(first, 12);
        let second = classify_all_unclassified_edges(&mut model).expect("second sweep");
        assert_eq!(
            second, 0,
            "already-classified edges should be skipped on subsequent sweeps"
        );
    }

    #[test]
    fn invalidate_face_neighbours_clears_only_that_face() {
        let mut model = build_unit_box();
        classify_all_unclassified_edges(&mut model).expect("seed classification");

        // Pick the first face and count its loop edges.
        let face_id = model
            .faces
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("cube must have ≥ 1 face");
        let expected = {
            let face = model.faces.get(face_id).expect("face exists");
            let outer = model.loops.get(face.outer_loop).expect("loop exists");
            outer.edges.len()
                + face
                    .inner_loops
                    .iter()
                    .filter_map(|l| model.loops.get(*l).map(|loop_data| loop_data.edges.len()))
                    .sum::<usize>()
        };

        let invalidated = invalidate_face_neighbours(&mut model, face_id);
        assert_eq!(
            invalidated, expected,
            "every edge of the chosen face should have been invalidated"
        );

        let still_classified = model
            .edges
            .iter()
            .filter(|(_, e)| e.attributes.is_classified())
            .count();
        // A box has 12 edges total; the picked face's 4 edges are
        // invalidated. The other 8 remain classified.
        assert_eq!(
            still_classified,
            12 - expected,
            "non-neighbouring edges should keep their classification"
        );
    }

    #[test]
    fn re_classify_after_invalidation_restores_attributes() {
        let mut model = build_unit_box();
        classify_all_unclassified_edges(&mut model).expect("initial sweep");
        let face_id = model
            .faces
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("cube must have ≥ 1 face");
        invalidate_face_neighbours(&mut model, face_id);

        let stamped = classify_all_unclassified_edges(&mut model).expect("re-classify sweep");
        let face = model.faces.get(face_id).expect("face still present");
        let outer = model.loops.get(face.outer_loop).expect("loop present");
        assert_eq!(
            stamped,
            outer.edges.len(),
            "every invalidated edge should be re-stamped"
        );

        for &eid in &outer.edges {
            let edge = model.edges.get(eid).expect("edge present");
            assert_eq!(edge.attributes.manifold_kind, ManifoldKind::Manifold);
            assert_eq!(edge.attributes.convexity, 1);
        }
    }
}
