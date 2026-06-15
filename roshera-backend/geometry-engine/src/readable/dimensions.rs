//! EYE / dimensioning: a complete, structured, ANALYTIC dimension table.
//!
//! Where `features.rs` reports per-face feature dims, this assembles the table
//! an agent (or a drawing) wants in one call: overall extents, every bore/boss
//! diameter AND axial length, sphere/cone sizes — each as a [`DimensionRecord`]
//! carrying a stable id, value, the face entities it spans, and a 3D anchor so
//! the callout is recoverable (placeable in any view, queryable, never read off
//! pixels).
//!
//! Honest by construction: every value is read off an analytic surface
//! (downcast) or exact curve geometry — never the tessellation. In particular
//! the overall extents are taken from edge-CURVE samples, not vertices: post-#24
//! a cylinder has a single seam vertex, so a vertex AABB would under-report its
//! true ±radius extent. Non-analytic faces contribute no fabricated size.

use crate::math::{Point3, Vector3};
use crate::primitives::face::Face;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cone, Cylinder, Sphere};
use crate::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};

/// One recoverable dimension callout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionRecord {
    /// Stable within one extraction (`"d0"`, `"d1"`, …) — the handle a future
    /// `set_dimension` (mould) edits.
    pub id: String,
    /// "diameter" | "radius" | "length" | "angle" | "extent".
    pub kind: String,
    pub value: f64,
    /// "mm" for lengths, "deg" for angles.
    pub unit: String,
    /// Human label, e.g. "Ø20.00", "L 40.00", "X 110.00", "∠ 30.0°".
    pub label: String,
    /// Face ids the dimension spans (empty for the whole-part extents).
    pub entities: Vec<u32>,
    /// World point to anchor the callout / leader line.
    pub anchor: [f64; 3],
    /// World direction the dimension is measured along (unit-ish).
    pub direction: [f64; 3],
}

/// Min/max world AABB accumulated from sampled points.
struct Aabb {
    min: [f64; 3],
    max: [f64; 3],
    any: bool,
}

impl Aabb {
    fn new() -> Self {
        Aabb {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
            any: false,
        }
    }
    fn add(&mut self, p: [f64; 3]) {
        for i in 0..3 {
            if p[i] < self.min[i] {
                self.min[i] = p[i];
            }
            if p[i] > self.max[i] {
                self.max[i] = p[i];
            }
        }
        self.any = true;
    }
}

/// Every edge id referenced by a solid's faces (outer + inner loops).
fn solid_edges(model: &BRepModel, solid_id: SolidId) -> Vec<u32> {
    let mut edges = Vec::new();
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return edges,
    };
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
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                if let Some(lp) = model.loops.get(lid) {
                    edges.extend_from_slice(&lp.edges);
                }
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    edges
}

/// World AABB from edge-CURVE samples (exact curves, not the mesh / vertices).
fn world_aabb(model: &BRepModel, solid_id: SolidId) -> Option<Aabb> {
    let mut aabb = Aabb::new();
    for eid in solid_edges(model, solid_id) {
        let edge = match model.edges.get(eid) {
            Some(e) => e,
            None => continue,
        };
        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => continue,
        };
        let r = edge.param_range;
        // 48 samples captures a full circle's ±radius extent to <0.1% of r.
        for k in 0..=48 {
            let t = r.start + (r.end - r.start) * (k as f64 / 48.0);
            if let Ok(p) = curve.point_at(t) {
                aabb.add([p.x, p.y, p.z]);
            }
        }
    }
    if aabb.any {
        Some(aabb)
    } else {
        None
    }
}

/// True axial extent of a face from its trim EDGES, as `(min, max)` projected
/// onto `axis` relative to `origin`. Used for a cylinder face's real length:
/// the surface's `height_limits` is the *uncut* bound and goes stale after a
/// boolean trims the face, but the rim edges always bound the live face.
fn face_axial_extent(
    model: &BRepModel,
    face: &Face,
    origin: Point3,
    axis: Vector3,
) -> Option<(f64, f64)> {
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
            let curve = match model.curves.get(edge.curve_id) {
                Some(c) => c,
                None => continue,
            };
            let r = edge.param_range;
            for k in 0..=8 {
                let t = r.start + (r.end - r.start) * (k as f64 / 8.0);
                if let Ok(p) = curve.point_at(t) {
                    let proj = (p.x - origin.x) * axis.x
                        + (p.y - origin.y) * axis.y
                        + (p.z - origin.z) * axis.z;
                    lo = lo.min(proj);
                    hi = hi.max(proj);
                }
            }
        }
    }
    if hi >= lo {
        Some((lo, hi))
    } else {
        None
    }
}

/// Assemble the full analytic dimension table for `solid_id`.
pub fn extract_dimensions(model: &BRepModel, solid_id: SolidId) -> Vec<DimensionRecord> {
    let mut out: Vec<DimensionRecord> = Vec::new();
    let mut next = 0usize;
    let mut id = || {
        let s = format!("d{next}");
        next += 1;
        s
    };

    // ── Overall extents (X / Y / Z) from exact edge-curve bounds ──────────────
    if let Some(bb) = world_aabb(model, solid_id) {
        let names = ["X", "Y", "Z"];
        let dirs = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for axis in 0..3 {
            let value = bb.max[axis] - bb.min[axis];
            if value <= 1e-9 {
                continue;
            }
            // Anchor along the lower edge of that axis on the min-min corner.
            let mut anchor = bb.min;
            anchor[axis] = 0.5 * (bb.min[axis] + bb.max[axis]);
            out.push(DimensionRecord {
                id: id(),
                kind: "extent".into(),
                value,
                unit: "mm".into(),
                label: format!("{} {:.2}", names[axis], value),
                entities: Vec::new(),
                anchor,
                direction: dirs[axis],
            });
        }
    }

    // ── Per analytic surface: bores/bosses, spheres, cones ────────────────────
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return out,
    };
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
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            let any = surface.as_any();
            if let Some(cyl) = any.downcast_ref::<Cylinder>() {
                let axis = cyl.axis.normalize().unwrap_or(cyl.axis);
                // Real axial extent from the face's trim edges (height_limits is
                // the uncut surface bound and stale after a boolean).
                let (lo, hi) = face_axial_extent(model, face, cyl.origin, axis)
                    .or_else(|| cyl.height_limits.map(|[a, b]| (a, b)))
                    .unwrap_or((0.0, 0.0));
                let mid = 0.5 * (lo + hi);
                // Anchor on the lateral at mid-height in the seam direction.
                let rd = cyl.ref_dir.normalize().unwrap_or(cyl.ref_dir);
                let anchor = [
                    cyl.origin.x + axis.x * mid + rd.x * cyl.radius,
                    cyl.origin.y + axis.y * mid + rd.y * cyl.radius,
                    cyl.origin.z + axis.z * mid + rd.z * cyl.radius,
                ];
                out.push(DimensionRecord {
                    id: id(),
                    kind: "diameter".into(),
                    value: cyl.radius * 2.0,
                    unit: "mm".into(),
                    label: format!("Ø{:.2}", cyl.radius * 2.0),
                    entities: vec![fid],
                    anchor,
                    direction: [rd.x, rd.y, rd.z],
                });
                let length = (hi - lo).abs();
                if length > 1e-9 {
                    out.push(DimensionRecord {
                        id: id(),
                        kind: "length".into(),
                        value: length,
                        unit: "mm".into(),
                        label: format!("L {length:.2}"),
                        entities: vec![fid],
                        anchor,
                        direction: [axis.x, axis.y, axis.z],
                    });
                }
            } else if let Some(sph) = any.downcast_ref::<Sphere>() {
                out.push(DimensionRecord {
                    id: id(),
                    kind: "diameter".into(),
                    value: sph.radius * 2.0,
                    unit: "mm".into(),
                    label: format!("SØ{:.2}", sph.radius * 2.0),
                    entities: vec![fid],
                    anchor: [sph.center.x + sph.radius, sph.center.y, sph.center.z],
                    direction: [1.0, 0.0, 0.0],
                });
            } else if let Some(cone) = any.downcast_ref::<Cone>() {
                let deg = cone.half_angle.to_degrees();
                let axis = cone.axis.normalize().unwrap_or(cone.axis);
                let h = match cone.height_limits {
                    Some([a, b]) => a.abs().max(b.abs()),
                    None => 0.0,
                };
                let base_r = h * cone.half_angle.tan();
                let anchor = [
                    cone.apex.x + axis.x * h,
                    cone.apex.y + axis.y * h,
                    cone.apex.z + axis.z * h,
                ];
                out.push(DimensionRecord {
                    id: id(),
                    kind: "angle".into(),
                    value: deg,
                    unit: "deg".into(),
                    label: format!("∠ {deg:.1}°"),
                    entities: vec![fid],
                    anchor,
                    direction: [axis.x, axis.y, axis.z],
                });
                if base_r > 1e-9 {
                    out.push(DimensionRecord {
                        id: id(),
                        kind: "diameter".into(),
                        value: base_r * 2.0,
                        unit: "mm".into(),
                        label: format!("Ø{:.2}", base_r * 2.0),
                        entities: vec![fid],
                        anchor,
                        direction: [1.0, 0.0, 0.0],
                    });
                }
            }
        }
    }

    out
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

    fn find(dims: &[DimensionRecord], kind: &str, value: f64) -> bool {
        dims.iter()
            .any(|d| d.kind == kind && (d.value - value).abs() < 1e-4)
    }

    #[test]
    fn box_extents_are_exact() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box"));
        let dims = extract_dimensions(&m, b);
        // Three extents, exactly the box size.
        assert!(find(&dims, "extent", 40.0), "X extent missing: {dims:?}");
        assert!(find(&dims, "extent", 30.0), "Y extent missing");
        assert!(find(&dims, "extent", 20.0), "Z extent missing");
    }

    #[test]
    fn bored_plate_reports_diameter_length_and_extents() {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
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
        let dims = extract_dimensions(&m, part);
        // The bore: Ø20 diameter, and its axial length = the 20-thick plate.
        assert!(find(&dims, "diameter", 20.0), "Ø20 bore missing: {dims:?}");
        assert!(
            find(&dims, "length", 20.0),
            "bore length 20 missing: {dims:?}"
        );
        // Overall extents still present (the bore doesn't change the 40×40×20).
        assert!(find(&dims, "extent", 40.0), "X extent missing");
        // Every record is recoverable: finite anchor + spanned entities for feats.
        for d in &dims {
            assert!(d.anchor.iter().all(|c| c.is_finite()), "bad anchor {d:?}");
            if d.kind == "diameter" {
                assert!(!d.entities.is_empty(), "diameter must name its face: {d:?}");
            }
        }
    }

    #[test]
    fn cylinder_extent_uses_curve_not_vertices() {
        // A bare analytic cylinder: post-#24 it has ONE seam vertex, so a
        // vertex AABB would give ~0 width. The curve-sampled extent must
        // recover the true diameter (Ø30) on X and Y.
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 15.0, 50.0)
            .expect("cyl"));
        let dims = extract_dimensions(&m, c);
        assert!(
            find(&dims, "extent", 30.0),
            "X/Y extent should be Ø30: {dims:?}"
        );
        assert!(find(&dims, "extent", 50.0), "Z extent should be height 50");
        assert!(find(&dims, "diameter", 30.0), "Ø30 missing");
        assert!(find(&dims, "length", 50.0), "length 50 missing");
    }
}
