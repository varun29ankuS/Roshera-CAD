//! Shell (hollow) correctness harness (GEOM-HARNESS).
//!
//! Shelling an `a×b×c` box with wall thickness `t` and the +Z face removed leaves
//! a cup: a floor of thickness `t` plus four side walls. The remaining material
//! volume is `a·b·c − (a−2t)(b−2t)(c−t)` (outer box minus the open cavity), and
//! the cup is a watertight solid (closed outer + inner-cavity boundary). The
//! harness pairs that analytic shell oracle with watertightness. (Shell is
//! `offset_solid` with faces removed; this is the regime hardened in task #36.)

use crate::harness::watertight::{is_watertight, mesh_volume};
use crate::operations::{offset_solid, CommonOptions, OffsetOptions};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};

/// Result of a shell invariant check.
#[derive(Debug, Clone)]
pub struct ShellCheck {
    pub mesh_volume: Option<f64>,
    pub expected_volume: f64,
    /// Volume = outer box − open cavity.
    pub shell_ok: bool,
    /// The hollow cup is watertight.
    pub watertight: bool,
    pub all_hold: bool,
}

/// Shell an `a×b×c` box (top face removed) with wall thickness `t` and check the
/// material volume against `a·b·c − (a−2t)(b−2t)(c−t)`, plus watertightness.
pub fn shell_open_box_invariants(a: f64, b: f64, c: f64, t: f64, rel_tol: f64) -> ShellCheck {
    let mut model = BRepModel::new();
    let solid = match make_box(&mut model, a, b, c) {
        Some(s) => s,
        None => return failed(),
    };
    let Some(top_face) = top_z_face(&model, solid) else {
        return failed();
    };

    let options = OffsetOptions {
        // Shells have documented open work in the full B-Rep validator; the
        // hollow GEOMETRY (volume + watertightness) is asserted directly.
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let expected_volume = a * b * c - (a - 2.0 * t) * (b - 2.0 * t) * (c - t);

    match offset_solid(&mut model, solid, t, vec![top_face], options) {
        Ok(hollow) => {
            let mesh_volume = mesh_volume(&model, hollow, 0.01);
            let shell_ok = mesh_volume.is_some_and(|m| within_rel(m, expected_volume, rel_tol));
            let watertight = is_watertight(&mut model, hollow, 0.01, rel_tol.max(1e-3));
            ShellCheck {
                mesh_volume,
                expected_volume,
                shell_ok,
                watertight,
                all_hold: shell_ok && watertight,
            }
        }
        Err(_) => ShellCheck {
            mesh_volume: None,
            expected_volume,
            shell_ok: false,
            watertight: false,
            all_hold: false,
        },
    }
}

// ---------------------------------------------------------------------------
// helpers (private)
// ---------------------------------------------------------------------------

fn make_box(model: &mut BRepModel, a: f64, b: f64, c: f64) -> Option<SolidId> {
    TopologyBuilder::new(model).create_box_3d(a, b, c).ok()?;
    model.solids.iter().last().map(|(id, _)| id)
}

/// The box's +Z face, located by surface normal.
fn top_z_face(model: &BRepModel, solid: SolidId) -> Option<FaceId> {
    let s = model.solids.get(solid)?;
    let shell = model.shells.get(s.outer_shell)?;
    for &face_id in &shell.faces {
        if let Some(face) = model.faces.get(face_id) {
            if let Some(surface) = model.surfaces.get(face.surface_id) {
                if let Ok(n) = surface.normal_at(0.5, 0.5) {
                    if (n.z - 1.0).abs() < 1e-9 && n.x.abs() < 1e-9 && n.y.abs() < 1e-9 {
                        return Some(face_id);
                    }
                }
            }
        }
    }
    None
}

fn failed() -> ShellCheck {
    ShellCheck {
        mesh_volume: None,
        expected_volume: 0.0,
        shell_ok: false,
        watertight: false,
        all_hold: false,
    }
}

fn within_rel(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelled_box_material_volume_and_watertight() {
        // 10³ box, t=1 → 1000 − 8·8·9 = 424 (the task-#36 regression value).
        let c = shell_open_box_invariants(10.0, 10.0, 10.0, 1.0, 2e-2);
        assert!(c.shell_ok, "{c:?}");
        assert!(c.watertight, "shelled cup not watertight: {c:?}");
        assert!((c.expected_volume - 424.0).abs() < 1e-9);
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]

        /// Material volume = outer − open cavity, watertight, for a range of box
        /// sizes and wall thicknesses (thickness kept well below the box).
        #[test]
        fn pp_shell_material_volume(
            a in 6.0f64..14.0,
            b in 6.0f64..14.0,
            c in 6.0f64..14.0,
            t in 0.5f64..2.0,
        ) {
            let chk = shell_open_box_invariants(a, b, c, t, 3e-2);
            prop_assert!(chk.shell_ok, "a={a} b={b} c={c} t={t}: {chk:?}");
            prop_assert!(chk.watertight, "not watertight: {chk:?}");
        }
    }
}
