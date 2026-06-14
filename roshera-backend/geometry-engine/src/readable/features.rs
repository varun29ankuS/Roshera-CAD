//! EYE-4: feature-dimension extraction + measurement.
//!
//! The dimensioned render (EYE-1) gives an agent the bounding box; this gives
//! it the *features* — "there is a Ø10 bore on this axis, a Ø24 boss here, this
//! face is the +Z top plane". Every analytic face the kernel carries already
//! knows its exact size; this surfaces it as a structured query so an agent can
//! ask "what are the holes and how big are they" instead of measuring pixels.
//!
//! Honest by construction: dimensions are read straight off the analytic
//! surface (downcast), never inferred from the mesh. Non-analytic faces report
//! their kind with no fabricated size.

use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cylinder, Plane};
use crate::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};

/// One face's analytic feature dimensions. Optional fields are populated only
/// for the surface kinds that define them (diameter/axis for cylinders, normal
/// for planes); everything else stays `None` rather than guessed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureDim {
    pub face_id: FaceId,
    /// Lower-case surface kind: "cylinder", "plane", "sphere", "cone", …
    pub surface_kind: String,
    pub diameter: Option<f64>,
    pub radius: Option<f64>,
    /// Unit axis (cylinder/cone) in world space.
    pub axis: Option<[f64; 3]>,
    /// A point on the feature: cylinder origin / plane origin.
    pub origin: Option<[f64; 3]>,
    /// Plane unit normal.
    pub normal: Option<[f64; 3]>,
}

/// Extract analytic feature dimensions for every face of `solid_id` (outer +
/// inner shells). Cylinder faces (bores, bosses) get diameter + axis; plane
/// faces get their normal. Returns `[]` for an unknown solid.
pub fn extract_features(model: &BRepModel, solid_id: SolidId) -> Vec<FeatureDim> {
    let mut out = Vec::new();
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return out,
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            let mut fd = FeatureDim {
                face_id,
                surface_kind: format!("{:?}", surface.surface_type()).to_lowercase(),
                diameter: None,
                radius: None,
                axis: None,
                origin: None,
                normal: None,
            };
            let any = surface.as_any();
            if let Some(cyl) = any.downcast_ref::<Cylinder>() {
                fd.radius = Some(cyl.radius);
                fd.diameter = Some(cyl.radius * 2.0);
                let a = cyl.axis.normalize().unwrap_or(cyl.axis);
                fd.axis = Some([a.x, a.y, a.z]);
                fd.origin = Some([cyl.origin.x, cyl.origin.y, cyl.origin.z]);
            } else if let Some(pl) = any.downcast_ref::<Plane>() {
                let n = pl.normal.normalize().unwrap_or(pl.normal);
                fd.normal = Some([n.x, n.y, n.z]);
                fd.origin = Some([pl.origin.x, pl.origin.y, pl.origin.z]);
            }
            out.push(fd);
        }
    }
    out
}

/// The distinct cylindrical hole/boss diameters present on a solid, each with a
/// count — the agent's "what bore sizes does this part have" answer. Diameters
/// are bucketed at 1e-6 so faceting/float noise doesn't split a size.
pub fn cylindrical_diameters(model: &BRepModel, solid_id: SolidId) -> Vec<(f64, usize)> {
    let mut buckets: Vec<(f64, usize)> = Vec::new();
    for f in extract_features(model, solid_id) {
        if let Some(d) = f.diameter {
            match buckets.iter_mut().find(|(bd, _)| (bd - &d).abs() < 1e-6) {
                Some((_, c)) => *c += 1,
                None => buckets.push((d, 1)),
            }
        }
    }
    buckets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    buckets
}

// ── Measurement ─────────────────────────────────────────────────────────────

/// Euclidean distance between two world points.
pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Signed distance from a point to a plane (positive on the normal side). The
/// normal is normalised internally; a zero-length normal yields 0.
pub fn point_to_plane_signed(
    point: [f64; 3],
    plane_origin: [f64; 3],
    plane_normal: [f64; 3],
) -> f64 {
    let n_len =
        (plane_normal[0].powi(2) + plane_normal[1].powi(2) + plane_normal[2].powi(2)).sqrt();
    if n_len < 1e-12 {
        return 0.0;
    }
    ((point[0] - plane_origin[0]) * plane_normal[0]
        + (point[1] - plane_origin[1]) * plane_normal[1]
        + (point[2] - plane_origin[2]) * plane_normal[2])
        / n_len
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    /// A 40×40×20 plate with a central Ø20 through-bore. The bore wall is a
    /// cylinder face → feature extraction must report a Ø20 cylinder on +Z, and
    /// the plate must have plane faces with ±Z / ±X / ±Y normals.
    #[test]
    fn extracts_bore_diameter_and_plane_normals() {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -20.0),
                Vector3::new(0.0, 0.0, 1.0),
                10.0,
                80.0,
            )
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("through bore");

        let feats = extract_features(&m, part);
        assert!(!feats.is_empty(), "no features extracted");

        // The bore: a cylinder face of diameter 20 on the Z axis.
        let bore_feat = feats
            .iter()
            .find(|f| f.surface_kind == "cylinder")
            .expect("no cylinder feature for the bore");
        let d = bore_feat.diameter.expect("cylinder has no diameter");
        assert!((d - 20.0).abs() < 1e-6, "bore diameter {d} != 20");
        let axis = bore_feat.axis.expect("cylinder has no axis");
        assert!(axis[2].abs() > 0.999, "bore axis {axis:?} not ~Z");

        // Diameter summary: exactly one distinct cylindrical size (Ø20).
        let dias = cylindrical_diameters(&m, part);
        assert_eq!(dias.len(), 1, "expected one bore size, got {dias:?}");
        assert!((dias[0].0 - 20.0).abs() < 1e-6);

        // Planes: the plate's 6 outer faces (top/bottom now annular, still
        // planes) — at least the ±Z faces must be present with Z normals.
        let z_planes = feats
            .iter()
            .filter(|f| f.surface_kind == "plane")
            .filter(|f| f.normal.map(|n| n[2].abs() > 0.999).unwrap_or(false))
            .count();
        assert!(z_planes >= 2, "expected ≥2 Z-normal planes, got {z_planes}");
    }

    #[test]
    fn measure_helpers_are_correct() {
        assert!((distance([0.0, 0.0, 0.0], [3.0, 4.0, 0.0]) - 5.0).abs() < 1e-12);
        // Point 7 above the z=0 plane → signed distance +7.
        let d = point_to_plane_signed([1.0, 2.0, 7.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert!((d - 7.0).abs() < 1e-12, "signed distance {d} != 7");
        // Below the plane → negative.
        let d2 = point_to_plane_signed([0.0, 0.0, -3.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert!((d2 + 3.0).abs() < 1e-12);
    }
}
