//! Drawing centerlines (#22) — the chain-line axes of circular features.
//!
//! A mechanical drawing marks the axis of every hole and shaft with a thin
//! chain (dash-dot) centerline, and the centre of a circular feature seen
//! end-on with a small cross (a "centre mark"). These are derived ANALYTICALLY
//! from each cylindrical face's exact axis + radius — never from the rasterised
//! view — and projected through the SAME view matrix as the edges, so each
//! centerline lands on the feature it belongs to and names the B-Rep face it
//! came from (recoverable, not decorative).
//!
//! Behaviour follows drafting convention:
//!   * axis points roughly AT the camera (feature reads as a circle) → a centre
//!     mark: a small cross at the projected axis point, sized to overshoot the
//!     rim;
//!   * otherwise (feature reads as a rectangle / side-on) → an axis centerline:
//!     the projected axis segment spanning the feature's axial extent, with a
//!     short overshoot past each end.

use super::projection::{project_point, view_matrix_for_projection};
use super::types::ProjectionType;
use crate::math::Vector3;
use crate::primitives::face::Face;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Cylinder;
use crate::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};

/// A centerline annotation in view-space (mm, pre-scale) — the same frame as the
/// projected polylines, so the renderer maps both uniformly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Centerline {
    /// "axis" (chain line through a side-on feature) | "center_mark" (cross at
    /// an end-on circular feature).
    pub kind: String,
    /// One or more line segments `[x1, y1, x2, y2]` in view-space mm. An axis is
    /// a single segment; a centre mark is two crossed segments.
    pub segments: Vec<[f64; 4]>,
    /// B-Rep face id(s) this centerline belongs to (recoverable).
    pub entities: Vec<u32>,
}

/// Signed axial extent `(lo, hi)` of a face along `axis` through `origin`,
/// taken from the face's own loop-edge vertices: `min/max of (V − O)·D`.
fn face_axial_extent(
    model: &BRepModel,
    face: &Face,
    origin: crate::math::Point3,
    axis: Vector3,
) -> Option<(f64, f64)> {
    let d = axis.normalize().ok()?;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut loops = vec![face.outer_loop];
    loops.extend_from_slice(&face.inner_loops);
    for lid in loops {
        let lp = match model.loops.get(lid) {
            Some(l) => l,
            None => continue,
        };
        for &eid in &lp.edges {
            let edge = match model.edges.get(eid) {
                Some(e) => e,
                None => continue,
            };
            for vid in [edge.start_vertex, edge.end_vertex] {
                if let Some(v) = model.vertices.get(vid) {
                    let p = crate::math::Point3::new(v.position[0], v.position[1], v.position[2]);
                    let s = (p - origin).dot(&d);
                    if s < lo {
                        lo = s;
                    }
                    if s > hi {
                        hi = s;
                    }
                }
            }
        }
    }
    if lo.is_finite() && hi.is_finite() && hi > lo {
        Some((lo, hi))
    } else {
        None
    }
}

/// Extend a 2D segment `a→b` outward by `frac` of its length plus a fixed
/// overshoot, returning `[x1, y1, x2, y2]`.
fn extend_segment(a: [f64; 2], b: [f64; 2], frac: f64, min_overshoot: f64) -> [f64; 4] {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return [a[0], a[1], b[0], b[1]];
    }
    let ux = dx / len;
    let uy = dy / len;
    let e = len * frac + min_overshoot;
    [a[0] - ux * e, a[1] - uy * e, b[0] + ux * e, b[1] + uy * e]
}

/// Derive the centerlines for `solid_id` in the given view. One entry per
/// cylindrical face (holes / shafts); planar and spherical faces have no axis.
pub fn centerlines(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
) -> Vec<Centerline> {
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let vm = view_matrix_for_projection(projection);
    let mut cands: Vec<Centerline> = Vec::new();

    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    for sh in shells {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            let face = match model.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let surf = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            let cyl = match surf.as_any().downcast_ref::<Cylinder>() {
                Some(c) => c,
                None => continue,
            };
            let axis = match cyl.axis.normalize() {
                Ok(a) => a,
                Err(_) => continue,
            };
            // Axial extent from this face's edges; fall back to height_limits.
            let (lo, hi) = face_axial_extent(model, face, cyl.origin, axis)
                .or_else(|| cyl.height_limits.map(|h| (h[0], h[1])))
                .unwrap_or((0.0, 0.0));
            if (hi - lo).abs() < 1e-9 {
                continue;
            }

            // How far the axis tilts out of the image plane: |view-space Z|.
            // 1 → straight at the camera (circle); 0 → in-plane (side-on).
            let out_of_plane = vm.transform_vector(&axis).z.abs();

            let cl = if out_of_plane > 0.966 {
                // End-on: centre mark cross, sized to overshoot the rim.
                let mid = cyl.origin + axis * (0.5 * (lo + hi));
                let c = project_point(projection, mid);
                let r = cyl.radius * 1.18;
                Centerline {
                    kind: "center_mark".to_string(),
                    segments: vec![
                        [c[0] - r, c[1], c[0] + r, c[1]],
                        [c[0], c[1] - r, c[0], c[1] + r],
                    ],
                    entities: vec![fid],
                }
            } else {
                // Side-on (or oblique): axis chain line spanning the extent.
                let p_lo = project_point(projection, cyl.origin + axis * lo);
                let p_hi = project_point(projection, cyl.origin + axis * hi);
                let seg = extend_segment(p_lo, p_hi, 0.10, 1.5);
                // If the axis projects to ~nothing (axis nearly straight at the
                // camera but below the circle threshold), skip — no useful line.
                let dx = seg[2] - seg[0];
                let dy = seg[3] - seg[1];
                if (dx * dx + dy * dy).sqrt() < 2.0 {
                    continue;
                }
                Centerline {
                    kind: "axis".to_string(),
                    segments: vec![seg],
                    entities: vec![fid],
                }
            };

            cands.push(cl);
        }
    }
    // Collapse coaxial features to one centerline per axis: coincident centre
    // marks (same projected centre) and collinear axis lines merge, keeping the
    // dominant geometry (largest mark / longest axis) and UNIONing the face
    // entities so the survivor still names every feature on that axis.
    dedup_centerlines(cands)
}

/// Grouping key + a "keep the bigger" metric for a centerline.
fn centerline_key(cl: &Centerline) -> (String, f64) {
    if cl.kind == "center_mark" {
        let h = &cl.segments[0];
        let cx = 0.5 * (h[0] + h[2]);
        let cy = h[1];
        let half = (h[2] - h[0]).abs() * 0.5;
        (format!("c|{cx:.2}|{cy:.2}"), half)
    } else {
        // Axis: key on the supporting line in normal form (direction mod π +
        // perpendicular offset from origin); metric is the segment length.
        let s = cl.segments[0];
        let dx = s[2] - s[0];
        let dy = s[3] - s[1];
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            return (format!("a|deg|{:.2}|{:.2}", s[0], s[1]), 0.0);
        }
        let ux = dx / len;
        let uy = dy / len;
        // Normal (−uy, ux); offset = n·p0. Fold direction to [0,π) so a line and
        // its reverse share a key.
        let mut theta = uy.atan2(ux);
        if theta < 0.0 {
            theta += std::f64::consts::PI;
        }
        let offset = -uy * s[0] + ux * s[1];
        (format!("a|{theta:.3}|{offset:.2}"), len)
    }
}

fn dedup_centerlines(cands: Vec<Centerline>) -> Vec<Centerline> {
    use std::collections::HashMap;
    let mut best: HashMap<String, (f64, Centerline)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for cl in cands {
        let (key, metric) = centerline_key(&cl);
        match best.get_mut(&key) {
            Some((m, keep)) => {
                // Union entities regardless of which geometry wins.
                for e in &cl.entities {
                    if !keep.entities.contains(e) {
                        keep.entities.push(*e);
                    }
                }
                if metric > *m {
                    let mut merged = cl;
                    for e in &keep.entities {
                        if !merged.entities.contains(e) {
                            merged.entities.push(*e);
                        }
                    }
                    *m = metric;
                    *keep = merged;
                }
            }
            None => {
                order.push(key.clone());
                best.insert(key, (metric, cl));
            }
        }
    }
    order
        .into_iter()
        .filter_map(|k| best.remove(&k).map(|(_, c)| c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn cylinder_end_on_gets_center_mark() {
        // Cylinder along Z. Top view (camera +Z, looking −Z) → axis straight at
        // the camera → a centre-mark cross, sized to overshoot the rim (r=10).
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
            .expect("cyl"));
        let cls = centerlines(&m, c, ProjectionType::Top);
        assert!(!cls.is_empty(), "cylinder has a centerline in Top");
        let mark = cls
            .iter()
            .find(|cl| cl.kind == "center_mark")
            .expect("centre mark in end-on view");
        assert_eq!(
            mark.segments.len(),
            2,
            "centre mark is a cross (2 segments)"
        );
        // Cross is centred on the axis (origin projects to (0,0) in Top) and
        // overshoots the rim (half-length > radius).
        let h = &mark.segments[0];
        let half = (h[2] - h[0]).abs() * 0.5;
        assert!(half > 10.0, "cross overshoots the Ø rim: half={half}");
        assert!(!mark.entities.is_empty(), "centre mark names its face");
    }

    #[test]
    fn cylinder_side_on_gets_axis_line() {
        // Same cylinder, Front view (camera +Y) → axis is vertical in-plane →
        // an axis chain line spanning the 30-tall extent with overshoot.
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
            .expect("cyl"));
        let cls = centerlines(&m, c, ProjectionType::Front);
        let axis = cls
            .iter()
            .find(|cl| cl.kind == "axis")
            .expect("axis line in side view");
        assert_eq!(axis.segments.len(), 1);
        let s = axis.segments[0];
        let len = ((s[2] - s[0]).powi(2) + (s[3] - s[1]).powi(2)).sqrt();
        // Spans the 30 extent plus overshoot both ends (>30, <30+2*5).
        assert!(
            len > 30.0 && len < 45.0,
            "axis spans extent+overshoot: {len}"
        );
        // The axis is vertical in Front (X≈const).
        assert!((s[2] - s[0]).abs() < 1e-6, "Front axis is vertical");
    }

    #[test]
    fn bored_plate_bore_has_center_mark_in_top() {
        use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 8.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        let cls = centerlines(&m, part, ProjectionType::Top);
        // The bore's cylindrical wall reads end-on in Top → a centre mark at the
        // origin sized to its Ø16 rim.
        let mark = cls
            .iter()
            .find(|cl| cl.kind == "center_mark")
            .expect("bore centre mark in Top");
        let h = &mark.segments[0];
        let half = (h[2] - h[0]).abs() * 0.5;
        assert!(
            half > 8.0 && half < 12.0,
            "cross sized to Ø16 bore: half={half}"
        );
    }
}
