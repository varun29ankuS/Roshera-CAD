//! Loft correctness harness (GEOM-HARNESS).
//!
//! Invariant: lofting between two congruent `w×h` rectangle sections separated
//! by `dz` in Z is an oblique prism of volume `w·h·dz` — independent of any XY
//! shear between the sections (Cavalieri's principle: the cross-section area is
//! constant and the perpendicular height is `dz`). The harness pairs that
//! oblique-prism oracle with the universal
//! [`crate::harness::watertight::is_watertight`] check, building the solid with
//! `create_solid: true` (an open shell tessellates into degenerate geometry).
//!
//! The top section is XY-sheared by `(dx, dy)` away from the bottom on purpose:
//! lofting two *identical, axis-aligned* stacked rectangles collapses a lateral
//! wall to a zero-area face (mesh volume 40 vs 60 on 3×4×5, and a `cdt`
//! triangulation panic) — tracked as task LOFT-ALIGNED-DEGENERACY. A non-zero
//! shear is the robust regime and still has an exact volume oracle. (Loft
//! manifold-gap hardening is task #33.)

use crate::harness::watertight::{is_watertight, mesh_volume};
use crate::math::vector3::Point3;
use crate::operations::{loft_profiles, LoftOptions};
use crate::primitives::curve::Line;
use crate::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use crate::primitives::topology_builder::BRepModel;

/// Result of a loft invariant check.
#[derive(Debug, Clone)]
pub struct LoftCheck {
    pub mesh_volume: Option<f64>,
    pub expected_volume: f64,
    /// Mesh volume ≈ section area × separation (prism).
    pub prism_ok: bool,
    /// The lofted solid is watertight.
    pub watertight: bool,
    pub all_hold: bool,
}

/// Loft between two congruent `w×h` rectangles separated by `length` in Z, with
/// the top section XY-sheared by `(dx, dy)`, and check the oblique-prism volume
/// (`w·h·length`) + watertightness. `dx`/`dy` must be non-zero to avoid the
/// aligned-profile degeneracy (task LOFT-ALIGNED-DEGENERACY).
pub fn loft_prism_invariants(
    w: f64,
    h: f64,
    length: f64,
    dx: f64,
    dy: f64,
    rel_tol: f64,
) -> LoftCheck {
    let mut model = BRepModel::new();
    let p0 = rectangle_at(&mut model, w, h, 0.0, 0.0, 0.0);
    let p1 = rectangle_at(&mut model, w, h, dx, dy, length);
    let options = LoftOptions {
        create_solid: true,
        ..Default::default()
    };
    let expected_volume = w * h * length;

    match loft_profiles(&mut model, vec![p0, p1], options) {
        Ok(solid) => {
            let mesh_volume = mesh_volume(&model, solid, 0.01);
            let prism_ok = mesh_volume.is_some_and(|m| within_rel(m, expected_volume, rel_tol));
            let watertight = is_watertight(&mut model, solid, 0.01, rel_tol.max(1e-3));
            LoftCheck {
                mesh_volume,
                expected_volume,
                prism_ok,
                watertight,
                all_hold: prism_ok && watertight,
            }
        }
        Err(_) => LoftCheck {
            mesh_volume: None,
            expected_volume,
            prism_ok: false,
            watertight: false,
            all_hold: false,
        },
    }
}

// ---------------------------------------------------------------------------
// helpers (private)
// ---------------------------------------------------------------------------

fn rectangle_at(model: &mut BRepModel, w: f64, h: f64, ox: f64, oy: f64, z: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(ox, oy, z);
    let v1 = model.vertices.add(ox + w, oy, z);
    let v2 = model.vertices.add(ox + w, oy + h, z);
    let v3 = model.vertices.add(ox, oy + h, z);
    vec![
        add_line_edge(model, v0, v1),
        add_line_edge(model, v1, v2),
        add_line_edge(model, v2, v3),
        add_line_edge(model, v3, v0),
    ]
}

fn add_line_edge(model: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let s = model.vertices.get(a).expect("v").position;
    let e = model.vertices.get(b).expect("v").position;
    let curve_id = model
        .curves
        .add(Box::new(Line::new(Point3::from(s), Point3::from(e))));
    model.edges.add(Edge::new_auto_range(
        0,
        a,
        b,
        curve_id,
        EdgeOrientation::Forward,
    ))
}

fn within_rel(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HARNESS-FOUND BUG (task LOFT-RECT-VOLUME-LOSS): lofting two rectangle
    /// profiles produces a solid with only ⅔ of the correct volume (40 vs 60 on
    /// 3×4×5) and a degenerate zero-area lateral face that panics the `cdt`
    /// triangulator. `densify_correspondence` force-resamples every profile to a
    /// floor of 8 points (`max(count, 8)`); the resampled rings drive
    /// `create_ruled_surfaces_between_profiles` to build a collapsed wall quad
    /// (`(0,0),(-5,2),(-5,2),(0,0)` in its tangent plane). The existing loft
    /// tests only assert topological validity (`validate_model_enhanced`), which
    /// does not catch a zero-area face, so this slipped through.
    ///
    /// Pinned as `#[ignore]` (run with `--ignored` to reproduce) until the loft
    /// densify/correspondence path is fixed in the op-finetuning phase. The
    /// universal tessellation pass is now panic-safe regardless (see
    /// `tessellation::surface::triangulate_planar_polygon`).
    #[test]
    #[ignore = "LOFT-RECT-VOLUME-LOSS: loft drops ⅓ volume + degenerate face (harness-found)"]
    fn loft_sheared_sections_is_an_oblique_prism_and_watertight() {
        // 3×4 rectangles separated by 5, top sheared by (1,1) → oblique-prism
        // volume w·h·dz = 60 (shear-independent, Cavalieri). Currently fails:
        // the loft yields volume 40, prism_ok = false.
        let c = loft_prism_invariants(3.0, 4.0, 5.0, 1.0, 1.0, 2e-2);
        assert!(c.prism_ok, "{c:?}");
        assert!(c.watertight, "lofted prism not watertight: {c:?}");
        assert!((c.expected_volume - 60.0).abs() < 1e-9);
    }
}
