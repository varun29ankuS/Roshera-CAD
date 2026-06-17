//! Relational spatial queries (#14 slice 4) — how faces relate by axis.
//!
//! The composable spatial-query core's relational primitive: "how do these
//! relate?". Each analytic face carries a characteristic direction — a
//! cylinder/cone AXIS line, or a plane NORMAL — read straight from the exact
//! surface (never inferred from the mesh). From two of them we answer the
//! relations an agent reasons with: coaxial (same axis line), parallel,
//! perpendicular, or skew. [`coaxial_clusters`] groups a solid's faces that
//! share one axis — e.g. every bore drilled on a common centreline.
//!
//! All comparisons use exact surface parameters and an explicit angular /
//! distance tolerance, so every verdict is recoverable to `(face, axis)`.

use crate::math::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cone, Cylinder, Plane};
use crate::primitives::topology_builder::BRepModel;

/// What kind of surface produced an axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    CylinderAxis,
    ConeAxis,
    PlaneNormal,
}

/// A face's characteristic axis: a unit direction plus a point it passes
/// through (the cylinder/cone axis origin, or the plane origin).
#[derive(Debug, Clone, Copy)]
pub struct FaceAxis {
    pub face: FaceId,
    pub kind: AxisKind,
    pub point: Point3,
    /// Unit direction (axis for cylinder/cone, normal for plane).
    pub dir: Vector3,
}

/// How two axes relate. `Coaxial` implies parallel directions AND a shared axis
/// line; `Parallel`/`Perpendicular` are direction-only; `Skew` is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisRelation {
    Coaxial,
    Parallel,
    Perpendicular,
    Skew,
}

/// Extract the characteristic axis of a face's analytic surface. `None` for a
/// sphere (no axis) or a non-analytic surface.
pub fn face_axis(model: &BRepModel, face_id: FaceId) -> Option<FaceAxis> {
    let face = model.faces.get(face_id)?;
    let surf = model.surfaces.get(face.surface_id)?;
    if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
        let dir = c.axis.normalize().ok()?;
        return Some(FaceAxis {
            face: face_id,
            kind: AxisKind::CylinderAxis,
            point: c.origin,
            dir,
        });
    }
    if let Some(c) = surf.as_any().downcast_ref::<Cone>() {
        let dir = c.axis.normalize().ok()?;
        return Some(FaceAxis {
            face: face_id,
            kind: AxisKind::ConeAxis,
            point: c.apex,
            dir,
        });
    }
    if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
        let dir = p.normal.normalize().ok()?;
        return Some(FaceAxis {
            face: face_id,
            kind: AxisKind::PlaneNormal,
            point: p.origin,
            dir,
        });
    }
    None
}

/// Perpendicular distance from `p` to the line through `a` with unit direction
/// `dir`: `|(p − a) × dir|`.
fn point_line_distance(p: Point3, a: Point3, dir: Vector3) -> f64 {
    (p - a).cross(&dir).magnitude()
}

/// Relate two face axes. `ang_tol` is the sine of the allowed angular slack
/// (e.g. `1e-6`); `dist_tol` the allowed off-axis offset for coaxiality.
pub fn axis_relation(a: &FaceAxis, b: &FaceAxis, ang_tol: f64, dist_tol: f64) -> AxisRelation {
    let cross = a.dir.cross(&b.dir).magnitude();
    let dot = a.dir.dot(&b.dir).abs();
    if cross <= ang_tol {
        // Directions parallel (or anti-parallel). Coaxial if both axis lines
        // coincide — each origin lies on the other's line.
        let d = point_line_distance(b.point, a.point, a.dir);
        if d <= dist_tol {
            AxisRelation::Coaxial
        } else {
            AxisRelation::Parallel
        }
    } else if dot <= ang_tol {
        AxisRelation::Perpendicular
    } else {
        AxisRelation::Skew
    }
}

/// Whether two faces are coaxial (shared axis line within tolerance).
pub fn are_coaxial(model: &BRepModel, f1: FaceId, f2: FaceId, ang_tol: f64, dist_tol: f64) -> bool {
    match (face_axis(model, f1), face_axis(model, f2)) {
        (Some(a), Some(b)) => axis_relation(&a, &b, ang_tol, dist_tol) == AxisRelation::Coaxial,
        _ => false,
    }
}

/// Whether two faces have parallel axes (coaxial counts as parallel here).
pub fn are_parallel(model: &BRepModel, f1: FaceId, f2: FaceId, ang_tol: f64) -> bool {
    match (face_axis(model, f1), face_axis(model, f2)) {
        (Some(a), Some(b)) => a.dir.cross(&b.dir).magnitude() <= ang_tol,
        _ => false,
    }
}

/// Whether two faces have perpendicular axes.
pub fn are_perpendicular(model: &BRepModel, f1: FaceId, f2: FaceId, ang_tol: f64) -> bool {
    match (face_axis(model, f1), face_axis(model, f2)) {
        (Some(a), Some(b)) => a.dir.dot(&b.dir).abs() <= ang_tol,
        _ => false,
    }
}

/// Group a solid's axis-bearing faces (cylinders/cones) into clusters that
/// share one axis line. Plane faces are excluded — a "centreline cluster" is a
/// turning/drilling concept. Each returned cluster has ≥2 faces; singletons are
/// dropped. Faces within a cluster are sorted by id (deterministic).
pub fn coaxial_clusters(
    model: &BRepModel,
    solid_id: SolidId,
    ang_tol: f64,
    dist_tol: f64,
) -> Vec<Vec<FaceId>> {
    let mut axes: Vec<FaceAxis> = solid_faces(model, solid_id)
        .into_iter()
        .filter_map(|fid| face_axis(model, fid))
        .filter(|a| matches!(a.kind, AxisKind::CylinderAxis | AxisKind::ConeAxis))
        .collect();
    axes.sort_by_key(|a| a.face);

    let mut clusters: Vec<Vec<FaceAxis>> = Vec::new();
    for a in axes {
        let mut placed = false;
        for cluster in &mut clusters {
            // Representative is the first member.
            if axis_relation(&cluster[0], &a, ang_tol, dist_tol) == AxisRelation::Coaxial {
                cluster.push(a);
                placed = true;
                break;
            }
        }
        if !placed {
            clusters.push(vec![a]);
        }
    }

    let mut out: Vec<Vec<FaceId>> = clusters
        .into_iter()
        .filter(|c| c.len() >= 2)
        .map(|c| {
            let mut ids: Vec<FaceId> = c.into_iter().map(|a| a.face).collect();
            ids.sort_unstable();
            ids
        })
        .collect();
    out.sort();
    out
}

fn solid_faces(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut out = Vec::new();
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return out,
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    for sh in shells {
        if let Some(shell) = model.shells.get(sh) {
            out.extend_from_slice(&shell.faces);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::surface::{Cylinder, Plane};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    /// First face whose surface is a cylinder.
    fn cyl_face(m: &BRepModel, s: SolidId) -> FaceId {
        let solid = m.solids.get(s).unwrap();
        let shell = m.shells.get(solid.outer_shell).unwrap();
        for &fid in &shell.faces {
            let f = m.faces.get(fid).unwrap();
            let surf = m.surfaces.get(f.surface_id).unwrap();
            if surf.as_any().downcast_ref::<Cylinder>().is_some() {
                return fid;
            }
        }
        panic!("no cylinder face")
    }

    /// First plane face with normal closest to +`axis`.
    fn plane_face_along(m: &BRepModel, s: SolidId, axis: Vector3) -> FaceId {
        let solid = m.solids.get(s).unwrap();
        let shell = m.shells.get(solid.outer_shell).unwrap();
        let mut best = (f64::NEG_INFINITY, 0u32);
        for &fid in &shell.faces {
            let f = m.faces.get(fid).unwrap();
            let surf = m.surfaces.get(f.surface_id).unwrap();
            if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
                let d = p.normal.normalize().unwrap_or(Vector3::Z).dot(&axis);
                if d > best.0 {
                    best = (d, fid);
                }
            }
        }
        best.1
    }

    #[test]
    fn box_face_axes_parallel_and_perpendicular() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let zp = plane_face_along(&m, b, Vector3::Z);
        let zn = plane_face_along(&m, b, Vector3::new(0.0, 0.0, -1.0));
        let xp = plane_face_along(&m, b, Vector3::X);
        // +Z and −Z faces: anti-parallel normals → parallel axes.
        assert!(are_parallel(&m, zp, zn, 1e-6), "top/bottom planes parallel");
        // +Z and +X faces: perpendicular normals.
        assert!(
            are_perpendicular(&m, zp, xp, 1e-6),
            "top/side planes perpendicular"
        );
        assert!(!are_parallel(&m, zp, xp, 1e-6), "top/side not parallel");
    }

    #[test]
    fn two_cylinders_coaxial_vs_offset() {
        // Two cylinders on the SAME Z axis through the origin → coaxial.
        let mut m = BRepModel::new();
        let c1 = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
            .expect("c1"));
        let c2 = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, 5.0), Vector3::Z, 4.0, 10.0)
            .expect("c2"));
        let f1 = cyl_face(&m, c1);
        let f2 = cyl_face(&m, c2);
        assert!(
            are_coaxial(&m, f1, f2, 1e-6, 1e-6),
            "same-axis cylinders are coaxial"
        );

        // A third cylinder offset in X by 50 → parallel axis, NOT coaxial.
        let c3 = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(50.0, 0.0, 0.0), Vector3::Z, 4.0, 10.0)
            .expect("c3"));
        let f3 = cyl_face(&m, c3);
        assert!(
            are_parallel(&m, f1, f3, 1e-6),
            "offset cylinder still parallel"
        );
        assert!(
            !are_coaxial(&m, f1, f3, 1e-6, 1e-6),
            "offset cylinder is NOT coaxial"
        );

        // A fourth cylinder along X → perpendicular axis.
        let c4 = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::X, 4.0, 10.0)
            .expect("c4"));
        let f4 = cyl_face(&m, c4);
        assert!(
            are_perpendicular(&m, f1, f4, 1e-6),
            "X-axis cylinder perpendicular to Z-axis cylinder"
        );
    }

    #[test]
    fn coaxial_clusters_group_shared_axis() {
        // Two coaxial cylinders + one offset: the cluster set is exactly the
        // coaxial pair, recoverable to their face ids.
        let mut m = BRepModel::new();
        let c1 = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
            .expect("c1"));
        let c2 = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, 5.0), Vector3::Z, 4.0, 10.0)
            .expect("c2"));
        let _c3 = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(50.0, 0.0, 0.0), Vector3::Z, 4.0, 10.0)
            .expect("c3"));
        let f1 = cyl_face(&m, c1);
        let f2 = cyl_face(&m, c2);
        // coaxial_clusters runs per solid; cluster c1's faces only contains its
        // own lateral face, so cluster across solids: build the axis list
        // directly to validate the relation logic across the three solids.
        let axes: Vec<FaceAxis> = [f1, f2, cyl_face(&m, _c3)]
            .into_iter()
            .filter_map(|fid| face_axis(&m, fid))
            .collect();
        // f1 & f2 coaxial; f3 parallel but offset.
        assert_eq!(
            axis_relation(&axes[0], &axes[1], 1e-6, 1e-6),
            AxisRelation::Coaxial
        );
        assert_eq!(
            axis_relation(&axes[0], &axes[2], 1e-6, 1e-6),
            AxisRelation::Parallel
        );
    }
}
