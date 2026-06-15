//! Field spatial queries (#14 slice 2) — signed distance sampled over a grid.
//!
//! The composable spatial-query core's field primitive. `signed_distance`
//! answers "how far, and which side" for a single point (negative inside,
//! positive outside), built on the analytic `nearest_on_solid` for the
//! magnitude and exact ray-parity for the sign — never a tessellation lookup.
//! `sample_field` evaluates it over an axis-aligned grid, producing a
//! [`ScalarField`] whose every sample is recoverable to a world-xyz grid node
//! (`world_position`) and to the face that gave its nearest distance.
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

/// A scalar field sampled on a regular axis-aligned grid. Row-major in
/// `(i, j, k)` with `i` fastest (x), then `j` (y), then `k` (z). Node `(i,j,k)`
/// sits at `world_position(i,j,k)`; `values[index]` is the signed distance
/// there and `faces[index]` the nearest face that produced it.
#[derive(Debug, Clone)]
pub struct ScalarField {
    /// Min corner of the sampled box (node `(0,0,0)`).
    pub min: Point3,
    /// Max corner of the sampled box (node `(nx-1, ny-1, nz-1)`).
    pub max: Point3,
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Signed distance at each node, row-major (`i` fastest).
    pub values: Vec<f64>,
    /// Nearest face id at each node, parallel to `values`.
    pub faces: Vec<FaceId>,
}

impl ScalarField {
    /// Row-major index of node `(i, j, k)`.
    #[inline]
    pub fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.ny + j) * self.nx + i
    }

    /// World position of grid node `(i, j, k)`. Linear interpolation of the box
    /// corners; for a single node along an axis the min coordinate is used.
    pub fn world_position(&self, i: usize, j: usize, k: usize) -> Point3 {
        let f = |a: f64, b: f64, t: usize, n: usize| {
            if n <= 1 {
                a
            } else {
                a + (b - a) * (t as f64 / (n - 1) as f64)
            }
        };
        Point3::new(
            f(self.min.x, self.max.x, i, self.nx),
            f(self.min.y, self.max.y, j, self.ny),
            f(self.min.z, self.max.z, k, self.nz),
        )
    }

    /// Signed distance at node `(i, j, k)`.
    #[inline]
    pub fn value(&self, i: usize, j: usize, k: usize) -> f64 {
        self.values[self.idx(i, j, k)]
    }

    /// Count of nodes strictly inside the material (negative signed distance).
    pub fn inside_count(&self) -> usize {
        self.values.iter().filter(|&&v| v < 0.0).count()
    }
}

/// Sample [`signed_distance`] over a regular `nx × ny × nz` grid spanning the
/// axis-aligned box `[min, max]` (inclusive of both corners). Nodes whose
/// nearest face cannot be found (degenerate solid) get `+inf` / face `0`.
pub fn sample_field(
    model: &BRepModel,
    solid_id: SolidId,
    min: Point3,
    max: Point3,
    nx: usize,
    ny: usize,
    nz: usize,
) -> ScalarField {
    let nx = nx.max(1);
    let ny = ny.max(1);
    let nz = nz.max(1);
    let mut field = ScalarField {
        min,
        max,
        nx,
        ny,
        nz,
        values: vec![f64::INFINITY; nx * ny * nz],
        faces: vec![0; nx * ny * nz],
    };
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let p = field.world_position(i, j, k);
                if let Some((sd, fid)) = signed_distance(model, solid_id, p) {
                    let idx = field.idx(i, j, k);
                    field.values[idx] = sd;
                    field.faces[idx] = fid;
                }
            }
        }
    }
    field
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
    fn sample_field_grid_recoverable_and_signed() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        // 5³ grid spanning [-20,20]³: corners well outside, centre inside.
        let f = sample_field(
            &m,
            b,
            Point3::new(-20.0, -20.0, -20.0),
            Point3::new(20.0, 20.0, 20.0),
            5,
            5,
            5,
        );
        // Centre node (2,2,2) is the origin → inside.
        assert!(f.value(2, 2, 2) < 0.0, "centre node inside");
        // Corner node (0,0,0) at (-20,-20,-20) → outside, positive.
        assert!(f.value(0, 0, 0) > 0.0, "corner node outside");
        // Recoverable: the centre node's world position is the origin, and its
        // nearest face is a real face of the solid.
        let p = f.world_position(2, 2, 2);
        assert!(p.magnitude() < 1e-9, "centre node at origin: {p:?}");
        let fid = f.faces[f.idx(2, 2, 2)];
        let real: std::collections::HashSet<u32> = {
            let solid = m.solids.get(b).unwrap();
            let shell = m.shells.get(solid.outer_shell).unwrap();
            shell.faces.iter().copied().collect()
        };
        assert!(real.contains(&fid), "nearest face {fid} is a real face");
        // The inside region is the 27-node core minus the boundary: exactly the
        // nodes within ±10, i.e. the single centre node here (others at ±10/±20).
        assert!(f.inside_count() >= 1, "at least the centre is inside");
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
