//! Live-dogfood regression (confirmed live 2026-07-12): filleting a THROUGH-hole
//! whose two rims share ONE wall cylinder must not hard-500 with
//!   `Invalid geometry: Fillet radius 1.5 too large for cylinder rim: exceeds
//!    available cylinder height (-1.5); the lateral surface would collapse.`
//!
//! ## The bug (self-cert LIE)
//! On a boolean-cut BLIND/POCKET floor the cap plane's stored surface normal
//! points INTO the material (−axis) even though the bore rim sits at the wall's
//! HIGH axial end. `cylinder_rim_fillet` read the retraction direction (`sign`)
//! from that raw normal, so the FIRST rim's surgery shortened the WRONG end and
//! wrote an INVERTED `height_limits` (`[h_high + r, h_high]` → `new_finite`
//! height −r) onto the shared wall cylinder. The SECOND rim then read that
//! inverted extent and the collapse guard refused a rim with 20 mm of wall — a
//! lie (the wall is plainly filletable). Single-rim fillets never tripped it
//! (nothing reads the inverted extent), so it only bit the through-hole pair
//! (and, via ALL-edges mode, the live pocket + 4-bore `drill_pattern` part).
//!
//! ## The fix
//! `cylinder_rim_fillet` now derives the cap height, the wall's available axial
//! extent, and `sign` from GEOMETRY (rim vertex + lateral-face vertices), not the
//! raw normal / stored `height_limits`, so `new_height_limits` is always a
//! sorted, non-degenerate interval. ALL-edges mode additionally PRE-SKIPS any rim
//! whose radius exceeds the geometry-derived available wall height (including a
//! degenerate / negative stored extent), rounding the rest.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{
    fillet_edges, FilletOptions, FilletType, PropagationMode,
};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{Cylinder, Surface};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// 60×40×30 box, a 40×20×20 top pocket (floor at z=5), and a centred r=4 bore
/// piercing the pocket floor: a through-hole whose two rims (pocket floor z=5,
/// box bottom z=−15) share ONE 20 mm wall cylinder. The pocket-floor rim is the
/// one whose raw cap normal (−Z) is decoupled from its HIGH axial end.
fn pocketed_bored_part(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(60.0, 40.0, 30.0)
        .expect("base box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for base box, got {o:?}"),
    };
    let pocket = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 20.0, 20.0)
        .expect("pocket tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for pocket tool, got {o:?}"),
    };
    translate(
        m,
        vec![pocket],
        Vector3::Z,
        15.0,
        TransformOptions::default(),
    )
    .expect("raise pocket");
    let part = boolean_operation(
        m,
        base,
        pocket,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cut pocket");
    let bore = match TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -25.0), Vector3::Z, 4.0, 45.0)
        .expect("bore")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for bore, got {o:?}"),
    };
    boolean_operation(
        m,
        part,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("drill bore")
}

/// The bore-rim edges of every cylinder face on the solid. A boolean cut leaves
/// each rim SPLIT into several perpendicular-to-axis arcs (not one closed loop);
/// `fillet_edges` coalesces those co-curve arcs back into a canonical rim before
/// blending, so passing the arcs selects the whole rim. Rim arcs are the cyl-face
/// edges whose two endpoints share an axial (z) coordinate; the axial seam edge
/// (which spans the wall) is excluded.
fn cylinder_rim_arcs(m: &BRepModel, s: SolidId) -> Vec<EdgeId> {
    let mut out: Vec<EdgeId> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Some(solid) = m.solids.get(s) else {
        return out;
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    for sh in shells {
        let Some(shell) = m.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = m.faces.get(fid) else {
                continue;
            };
            let is_cyl = m
                .surfaces
                .get(face.surface_id)
                .map(|surf| surf.as_any().downcast_ref::<Cylinder>().is_some())
                .unwrap_or(false);
            if !is_cyl {
                continue;
            }
            for lid in face.all_loops() {
                if let Some(lp) = m.loops.get(lid) {
                    for &e in &lp.edges {
                        let Some(ed) = m.edges.get(e) else { continue };
                        let (Some(a), Some(b)) = (
                            m.vertices.get_position(ed.start_vertex),
                            m.vertices.get_position(ed.end_vertex),
                        ) else {
                            continue;
                        };
                        // Perpendicular-to-axis (constant z) ⇒ rim arc, not seam.
                        if (a[2] - b[2]).abs() < 1e-6 && seen.insert(e) {
                            out.push(e);
                        }
                    }
                }
            }
        }
    }
    out
}

/// All non-loop + loop edges of the solid (outer + inner shells, deduplicated).
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
                if let Some(lp) = model.loops.get(lid) {
                    for &e in &lp.edges {
                        if seen.insert(e) {
                            out.push(e);
                        }
                    }
                }
            }
        }
    }
    out
}

fn opts(r: f64, graceful: bool) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        graceful_corner_skip: graceful,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// ROOT: filleting BOTH shared-wall rims of the through-hole in ONE explicit
/// call succeeds and leaves a sound solid. Pre-fix the second-processed rim read
/// the inverted `height_limits` the first wrote and the collapse guard refused
/// with "exceeds available cylinder height (-1.5); the lateral surface would
/// collapse." — the exact live 500.
#[test]
fn through_bore_shared_wall_both_rims_fillet_sound() {
    let mut m = BRepModel::new();
    let s = pocketed_bored_part(&mut m);

    let rims = cylinder_rim_arcs(&m, s);
    assert!(
        !rims.is_empty(),
        "the through-bore must expose rim arcs to fillet",
    );

    let faces = fillet_edges(&mut m, s, rims, opts(1.5, false)).expect(
        "filleting both shared-wall through-hole rims must succeed (was the −1.5 \
         inverted-height collapse 500)",
    );
    assert_eq!(
        faces.len(),
        2,
        "both bore rims must be rounded (one torus blend each), got {faces:?}",
    );

    let cert = m.certify_solid(s);
    assert!(
        cert.watertight && cert.manifold && cert.oriented && cert.brep_valid,
        "rounded through-bore must stay sound: watertight={} manifold={} oriented={} \
         brep_valid={} errors={:?}",
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.brep_valid,
        cert.errors,
    );
}

/// CONTRACT: ALL-edges graceful mode on the same part rounds what it can WITHOUT
/// the rim-collapse 500. The shared-wall rims are now filletable (root fix); any
/// genuinely un-filletable rim would be pre-skipped with a warn. Pre-fix this
/// raised the "-1.5 … lateral surface would collapse" error and rolled back.
#[test]
fn fillet_all_graceful_no_rim_collapse_on_through_bore() {
    let mut m = BRepModel::new();
    let s = pocketed_bored_part(&mut m);

    let edges = all_edges(&m, s);
    match fillet_edges(&mut m, s, edges, opts(1.5, true)) {
        Ok(faces) => {
            assert!(
                !faces.is_empty(),
                "graceful round must still produce blend faces"
            );
            let cert = m.certify_solid(s);
            assert!(
                cert.watertight && cert.manifold && cert.brep_valid,
                "graceful round must leave a sound solid: watertight={} manifold={} \
                 brep_valid={} errors={:?}",
                cert.watertight,
                cert.manifold,
                cert.brep_valid,
                cert.errors,
            );
        }
        Err(e) => {
            // The op may still refuse for a genuinely out-of-scope reason, but it
            // must NEVER fail with either signature of the shared-wall inversion
            // bug: the collapse guard, or the sibling "edge lies … off … Cylinder
            // surface" the inverted `height_limits` produced.
            let msg = format!("{e:?}");
            assert!(
                !msg.contains("lateral surface would collapse")
                    && !msg.contains("available cylinder height")
                    && !msg.contains("off face")
                    && !msg.contains("Cylinder surface"),
                "graceful ALL-edges must not fail with the shared-wall inversion signature; \
                 got {e:?}",
            );
            // Any residual (out-of-scope) failure must be transactional.
            let cert = m.certify_solid(s);
            assert!(
                cert.brep_valid && cert.watertight,
                "graceful failure must roll back to the intact part: {:?}",
                cert.errors,
            );
        }
    }
}
