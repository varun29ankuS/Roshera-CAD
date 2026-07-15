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
//! This harness found and pins the fix for LOFT-RECT-VOLUME-LOSS (#48): lofting
//! rectangle profiles previously dropped ⅓ of the volume (40 vs 60 on 3×4×5)
//! and built a degenerate zero-area lateral face that panicked `cdt`. Root
//! cause was in `operations/loft.rs`: `create_ruled_face` / `build_loft_cap`
//! recorded every loop edge as forward, but `create_or_find_edge` reuses shared
//! rails/rungs in *either* direction, so a reversed reused edge made the face
//! loop chain head-to-head instead of head-to-tail. Fixed by recording each
//! edge's true orientation (`directed_loop_edge`). Both axis-aligned and
//! XY-sheared congruent sections are now exact.

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

// Reason: harness helper -- the vertex ids were returned by add_or_find on
// this same model moments earlier in the fixture builder; a miss is memory
// corruption-level breakage the harness must abort on.
#[allow(clippy::expect_used)]
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

    /// Regression for LOFT-RECT-VOLUME-LOSS (#48): a sheared rectangle loft is
    /// an oblique prism of volume w·h·dz. Before the `directed_loop_edge` fix
    /// this yielded volume 40 vs 60 with a degenerate face; now exact.
    #[test]
    fn loft_sheared_sections_is_an_oblique_prism_and_watertight() {
        // 3×4 rectangles separated by 5, top sheared by (1,1) → oblique-prism
        // volume w·h·dz = 60 (shear-independent, Cavalieri).
        let c = loft_prism_invariants(3.0, 4.0, 5.0, 1.0, 1.0, 2e-2);
        assert!(c.prism_ok, "{c:?}");
        assert!(c.watertight, "lofted prism not watertight: {c:?}");
        assert!((c.expected_volume - 60.0).abs() < 1e-9);
    }

    /// The identical, axis-aligned stacked-rectangle case (zero shear) — the
    /// original degeneracy — is now a clean right prism too.
    #[test]
    fn loft_aligned_sections_is_a_right_prism_and_watertight() {
        let c = loft_prism_invariants(3.0, 4.0, 5.0, 0.0, 0.0, 2e-2);
        assert!(c.prism_ok, "{c:?}");
        assert!(c.watertight, "lofted prism not watertight: {c:?}");
        assert!((c.expected_volume - 60.0).abs() < 1e-9);
    }

    use proptest::prelude::*;

    proptest! {
        // Two tessellations per case → keep the count modest for CI speed.
        #![proptest_config(ProptestConfig { cases: 12, ..ProptestConfig::default() })]

        /// Loft between congruent sections (any XY shear, including none) is a
        /// prism of volume w·h·dz, watertight, for any rectangle and separation.
        #[test]
        fn pp_loft_prism_volume(
            w in 1.0f64..8.0,
            h in 1.0f64..8.0,
            l in 1.0f64..10.0,
            dx in 0.0f64..3.0,
            dy in 0.0f64..3.0,
        ) {
            let c = loft_prism_invariants(w, h, l, dx, dy, 3e-2);
            prop_assert!(c.prism_ok, "w={w} h={h} l={l} dx={dx} dy={dy}: {c:?}");
            prop_assert!(c.watertight, "not watertight: {c:?}");
        }
    }
}
