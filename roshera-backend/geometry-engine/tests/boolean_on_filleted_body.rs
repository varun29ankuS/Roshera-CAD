//! Regression: boolean operations against a FILLETED body must not crash.
//!
//! ## The bug (dogfood 2026-07-10)
//!
//! Subtracting a cylinder from a filleted box 500'd with
//! `Internal error: "Failed to downcast first cylinder"`. A fillet adds
//! `CylindricalFillet` faces, and `analytical_surface_kind` correctly reports
//! them as `Cylinder` (a fillet on a straight edge IS a cylindrical patch) —
//! but the analytic intersection handlers
//! (`cylinder_cylinder_intersection`, `plane_cylinder_intersection`) downcast
//! the operand to the CONCRETE `Cylinder` struct, which a `CylindricalFillet`
//! is not, and errored. So booleans against any filleted body were impossible
//! (blocking fillet-then-cut / feature reordering).
//!
//! ## The fix
//!
//! `as_cylinder(surface)` extracts an equivalent `Cylinder` from either a real
//! `Cylinder` or a straight-spine `CylindricalFillet` (axis = spine tangent,
//! radius = fillet radius). The intersection math runs on the equivalent
//! cylinder; the fillet face is still split by the resulting 3D curve.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn all_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let mut shells = vec![s.outer_shell];
    shells.extend_from_slice(&s.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            for lid in face.all_loops() {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &e in &lp.edges {
                    if seen.insert(e) {
                        out.push(e);
                    }
                }
            }
        }
    }
    out
}

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        ..Default::default()
    }
}

/// Fillet all edges of a box (adds `CylindricalFillet` + `Sphere` faces), then
/// subtract a vertical bore through it. The bore's bbox overlaps the fillet
/// faces, so bore-lateral ∩ fillet-face pairs route into
/// `cylinder_cylinder_intersection`. Pre-fix this crashed with
/// "Failed to downcast first cylinder"; it must now complete and stay watertight.
#[test]
fn difference_bore_from_filleted_box_is_sound() {
    let mut m = BRepModel::new();
    let boxs = match TopologyBuilder::new(&mut m)
        .create_box_3d(100.0, 100.0, 100.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let edges = all_edges(&m, boxs);
    fillet_edges(&mut m, boxs, edges, fillet_opts(10.0)).expect("fillet all edges");

    // Vertical bore straight through the filleted box, on the axis.
    let bore = match TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -60.0), Vector3::Z, 20.0, 120.0)
        .expect("cyl")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };

    let result = boolean_operation(
        &mut m,
        boxs,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference on a filleted body must not crash (as_cylinder handles fillet faces)");

    let cert = m.certify_solid(result);
    assert!(
        cert.watertight,
        "difference on a filleted body must be watertight; cert = {cert:?}"
    );
}
