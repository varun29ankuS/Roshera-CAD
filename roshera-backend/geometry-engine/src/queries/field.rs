//! Field spatial queries (#14 slice 2) — the ANALYTIC signed-distance field.
//!
//! The composable spatial-query core's field primitive. `signed_distance`
//! answers "how far, and which side" for a single point (negative inside,
//! positive outside), built on the analytic `nearest_on_solid` for the
//! magnitude and exact ray-parity for the sign — never a tessellation lookup.
//! `sample_field_adaptive` refines an octree over a box, subdividing ONLY the
//! cells the surface may pass through (the ADF bound), producing an
//! [`AdaptiveField`] whose every leaf is an exact per-point evaluation
//! recoverable to the face that gave its nearest distance. Cost scales with
//! surface area, not volume — there is deliberately NO dense voxel grid here:
//! resolution is a property of the query, never of the representation.
//!
//! This is the agent's continuous handle on the solid: an SDF it can march for
//! an isosurface, probe for clearance, or difference between timeline states to
//! see where material moved.

use super::point::nearest_on_solid;
use super::raycast::raycast_all;
use crate::math::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Generic ray direction for parity: irrational-ish so it never grazes an
/// axis-aligned face or seam (those are measure-zero), keeping crossings exact.
const PARITY_DIR: Vector3 = Vector3::new(0.5212, 0.3389, 0.7831);

/// Signed distance from `p` to the solid boundary: negative inside the
/// material, positive outside, ~0 on the surface. Returns the nearest face too,
/// so the value stays recoverable to `(face, world-xyz)`. `None` only when the
/// solid has no reachable boundary face.
pub fn signed_distance(model: &BRepModel, solid_id: SolidId, p: Point3) -> Option<(f64, FaceId)> {
    let (face_id, _, d) = nearest_on_solid(model, solid_id, p)?;
    let inside = raycast_all(model, solid_id, p, PARITY_DIR).len() % 2 == 1;
    Some((if inside { -d } else { d }, face_id))
}

/// One leaf of an adaptively-refined signed-distance sampling: an axis-aligned
/// cell carrying the EXACT analytic signed distance (and nearest face) at its
/// center. Cells subdivide only where the surface may pass through them, so a
/// sampling's cost scales with SURFACE AREA, not volume — the reason a 1 m part
/// at fine resolution does not cost `(size/ε)³` nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldCell {
    /// Cell center (where the signed distance was evaluated).
    pub center: Point3,
    /// Half-extent per axis (the cell spans `center ± half_extent`).
    pub half_extent: Vector3,
    /// Exact signed distance at `center` (negative inside the material).
    pub distance: f64,
    /// Nearest boundary face at `center` — keeps every sample recoverable to
    /// real topology, never a bare number.
    pub face: FaceId,
    /// Octree depth of this leaf (0 = the root cell).
    pub depth: u32,
}

impl FieldCell {
    /// Conservative surface test: the boundary may pass through this cell iff
    /// the center's distance does not exceed the circumscribed radius (half the
    /// cell diagonal). This is the classic ADF refinement bound.
    #[inline]
    pub fn may_contain_surface(&self) -> bool {
        self.distance.abs() <= self.half_extent.magnitude()
    }
}

/// An adaptively-sampled signed-distance field: the LEAVES of an octree over
/// `[min, max]`, refined only in the narrow band around the solid's surface.
/// Deterministic: same inputs → bit-identical leaves in DFS order.
#[derive(Debug, Clone, PartialEq)]
pub struct AdaptiveField {
    /// Min corner of the sampled box.
    pub min: Point3,
    /// Max corner of the sampled box.
    pub max: Point3,
    /// Maximum refinement depth (a leaf at this depth is `2^max_depth` times
    /// smaller than the root along each axis).
    pub max_depth: u32,
    /// Leaf cells, depth-first, children in fixed z-major order.
    pub cells: Vec<FieldCell>,
}

impl AdaptiveField {
    /// Number of leaves actually materialized.
    #[inline]
    pub fn leaf_count(&self) -> usize {
        self.cells.len()
    }

    /// Leaves in the surface band (refinement criterion still true at leaf
    /// depth) — the cells an isosurface extractor would visit.
    pub fn band_cell_count(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| c.may_contain_surface())
            .count()
    }

    /// What a DENSE grid at the same resolution would cost: `(2^max_depth)³`
    /// nodes. The honest baseline for the adaptive saving.
    #[inline]
    pub fn uniform_equivalent(&self) -> usize {
        let n = 1usize << self.max_depth;
        n * n * n
    }

    /// Count of leaves strictly inside the material (negative signed distance).
    pub fn inside_count(&self) -> usize {
        self.cells.iter().filter(|c| c.distance < 0.0).count()
    }
}

/// Adaptively sample [`signed_distance`] over the axis-aligned box
/// `[min, max]`: an octree that SUBDIVIDES ONLY where the surface may pass
/// through a cell (center distance ≤ half-diagonal — the conservative ADF
/// bound) and keeps interior/exterior space as coarse leaves. Every leaf's
/// value is an exact per-point analytic evaluation — nothing is interpolated
/// or invented. Cells whose nearest face cannot be found (degenerate solid)
/// become `+inf` / face `0` leaves. Deterministic DFS, z-major child order.
pub fn sample_field_adaptive(
    model: &BRepModel,
    solid_id: SolidId,
    min: Point3,
    max: Point3,
    max_depth: u32,
) -> AdaptiveField {
    let mut field = AdaptiveField {
        min,
        max,
        max_depth,
        cells: Vec::new(),
    };
    let center = Point3::new(
        0.5 * (min.x + max.x),
        0.5 * (min.y + max.y),
        0.5 * (min.z + max.z),
    );
    let half = Vector3::new(
        0.5 * (max.x - min.x).abs(),
        0.5 * (max.y - min.y).abs(),
        0.5 * (max.z - min.z).abs(),
    );
    refine_cell(
        model,
        solid_id,
        center,
        half,
        0,
        max_depth,
        &mut field.cells,
    );
    field
}

/// Depth-first refinement: evaluate the cell center; subdivide into 8 children
/// (fixed z-major order, so output is deterministic) while the surface may
/// cross the cell and depth remains; otherwise emit the leaf.
fn refine_cell(
    model: &BRepModel,
    solid_id: SolidId,
    center: Point3,
    half: Vector3,
    depth: u32,
    max_depth: u32,
    out: &mut Vec<FieldCell>,
) {
    let (distance, face) = match signed_distance(model, solid_id, center) {
        Some((sd, fid)) => (sd, fid),
        // Degenerate solid (no reachable boundary face at all): record an
        // honest infinite leaf rather than inventing a value, and never
        // subdivide toward a surface that cannot be evaluated. (Healthy solids
        // never hit this — nearest_on_solid covers faces, edges and vertices.)
        None => {
            out.push(FieldCell {
                center,
                half_extent: half,
                distance: f64::INFINITY,
                face: 0,
                depth,
            });
            return;
        }
    };
    let cell = FieldCell {
        center,
        half_extent: half,
        distance,
        face,
        depth,
    };
    if depth < max_depth && cell.may_contain_surface() {
        let h = Vector3::new(0.5 * half.x, 0.5 * half.y, 0.5 * half.z);
        for dz in [-1.0, 1.0] {
            for dy in [-1.0, 1.0] {
                for dx in [-1.0, 1.0] {
                    let child = Point3::new(
                        center.x + dx * h.x,
                        center.y + dy * h.y,
                        center.z + dz * h.z,
                    );
                    refine_cell(model, solid_id, child, h, depth + 1, max_depth, out);
                }
            }
        }
    } else {
        out.push(cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn signed_distance_box_sign_and_magnitude() {
        // Box 20³ centred at origin: faces at ±10 on each axis.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        // Centre: nearest face 10 away, inside → −10.
        let (sd, _) = signed_distance(&m, b, Point3::ZERO).expect("sd");
        assert!((sd + 10.0).abs() < 1e-6, "centre sd {sd} != -10");
        // Outside +X by 5: +5.
        let (sd, _) = signed_distance(&m, b, Point3::new(15.0, 0.0, 0.0)).expect("sd");
        assert!((sd - 5.0).abs() < 1e-6, "outside sd {sd} != 5");
        // Just inside the +Z face: small negative.
        let (sd, _) = signed_distance(&m, b, Point3::new(0.0, 0.0, 9.0)).expect("sd");
        assert!((sd + 1.0).abs() < 1e-6, "near-face sd {sd} != -1");
    }

    #[test]
    fn adaptive_field_recoverable_signed_and_banded() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        // Depth-4 adaptive sampling of [-20,20]³ around a 20³ box: the surface
        // band refines, the far corners and deep interior stay coarse. (Depth 3
        // is degenerate for this lattice-aligned pair — every cell touches the
        // conservative band, corner cells at exactly the half-diagonal bound —
        // one level deeper the band thins and the saving appears.)
        let f = sample_field_adaptive(
            &m,
            b,
            Point3::new(-20.0, -20.0, -20.0),
            Point3::new(20.0, 20.0, 20.0),
            4,
        );
        // Signed: at least one leaf inside the material and one outside.
        assert!(f.inside_count() >= 1, "some leaf is inside");
        assert!(
            f.cells.iter().any(|c| c.distance > 0.0),
            "some leaf is outside"
        );
        // Banded: the surface band refined to max depth, and the field beat the
        // dense grid it replaces.
        assert!(
            f.cells.iter().any(|c| c.depth == 4),
            "band reaches max depth"
        );
        assert!(f.band_cell_count() >= 1, "band cells exist");
        assert!(
            f.leaf_count() < f.uniform_equivalent(),
            "{} leaves must undercut the {}-node dense grid",
            f.leaf_count(),
            f.uniform_equivalent()
        );
        // Recoverable: every finite leaf's nearest face is a REAL face of the
        // solid and its distance is exactly the direct per-point answer. An
        // INFINITE leaf is permitted ONLY for the known on-boundary quirk: this
        // lattice-aligned sampling box puts some cell centers exactly on the
        // box's edges/corners, where `nearest_on_solid` is boundary-exclusive
        // and yields None — assert that is genuinely the case (the center
        // classifies On), so the escape hatch can never hide a real failure.
        let real: std::collections::HashSet<u32> = {
            let solid = m.solids.get(b).unwrap();
            let shell = m.shells.get(solid.outer_shell).unwrap();
            shell.faces.iter().copied().collect()
        };
        for c in &f.cells {
            // A healthy solid must yield a FINITE distance for every leaf —
            // including centers in corner Voronoi wedges and exactly on
            // edges/corners (the slice-1 face-only nearest returned None
            // there; the edge/vertex pass closed that hole).
            assert!(
                c.distance.is_finite(),
                "no infinite leaves on a healthy solid, center {:?}",
                c.center
            );
            assert!(real.contains(&c.face), "leaf face {} is real", c.face);
            let (sd, fid) = signed_distance(&m, b, c.center).expect("sd");
            assert_eq!(sd, c.distance, "leaf distance is the exact evaluation");
            assert_eq!(fid, c.face, "leaf face is the exact nearest face");
        }
    }

    #[test]
    fn field_sees_through_hole() {
        // Plate with a Ø20 through-bore: a field node on the bore axis reads
        // positive (outside material) even though it is deep "inside" the bbox.
        use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        // On the bore axis → in the hole → outside material → positive sd.
        let (sd, _) = signed_distance(&m, part, Point3::ZERO).expect("sd");
        assert!(sd > 0.0, "point in through-hole has positive sd, got {sd}");
        // In the material ring → negative.
        let (sd, _) = signed_distance(&m, part, Point3::new(20.0, 0.0, 0.0)).expect("sd");
        assert!(sd < 0.0, "point in material has negative sd, got {sd}");
    }
}
