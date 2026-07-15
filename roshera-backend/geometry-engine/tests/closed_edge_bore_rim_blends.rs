// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Bore-rim (inner-hole) fillet + chamfer regression (#26).
//!
//! `cylinder_rim_fillet` / `create_closed_edge_chamfer` originally only
//! handled the OUTER rim of a cap: they searched the cap face's *outer*
//! loop for the rim edge and assumed the cap circle shrinks to R−r. A
//! tube / washer / flange has an ANNULAR cap whose bore rim lives in an
//! *inner* loop, where the hole instead grows to R+r and the blend sits
//! on the torus inner equator (fillet) / a cone that opens the other way
//! (chamfer). The bore case therefore failed with
//! `InvalidGeometry("Rim edge not found in cap loop")`.
//!
//! These tests pin the fix: filleting and chamfering the bore rim of a
//! revolved tube succeeds, adds exactly one analytic blend face of the
//! right kind (Torus / Cone), and leaves the solid B-Rep-valid AND mesh-
//! watertight. Outer-rim coverage stays in `fillet_closed_edge.rs`.
//!
//! Cone-walled rims (Plane–Cone) are now supported too (task #89,
//! `cone_rim_fillet`): `cone_walled_rim_fillet_succeeds` pins that filleting a
//! frustum-tube's outer-top rim yields a sound, watertight torus blend.

use std::f64::consts::TAU;

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::{
    boolean_operation, fillet_edges, BooleanOp, BooleanOptions, FilletOptions, OperationError,
};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

/// Revolve a closed (r, z) profile a full turn about +Z.
fn revolve_tube(m: &mut BRepModel, pts: &[(f64, f64)]) -> SolidId {
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: TAU,
        segments: 64,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).expect("tube revolve")
}

/// Closed rim edge whose seam vertex sits at radius `r_want`, height `z_want`.
fn rim_at(m: &BRepModel, r_want: f64, z_want: f64) -> Option<EdgeId> {
    m.edges.iter().find_map(|(id, e)| {
        if !e.is_loop() {
            return None;
        }
        let p = m.vertices.get_position(e.start_vertex)?;
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        if (r - r_want).abs() < 0.5 && (p[2] - z_want).abs() < 0.5 {
            Some(id)
        } else {
            None
        }
    })
}

fn assert_valid_watertight(m: &mut BRepModel, s: SolidId, what: &str) {
    let v = validate_solid_scoped(m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "{what}: B-Rep invalid: {:?}", v.errors);
    assert!(
        geometry_engine::harness::watertight::is_watertight(m, s, 0.25, 1e-3),
        "{what}: mesh not watertight"
    );
}

/// Stricter companion to [`assert_valid_watertight`]: in addition to B-Rep
/// validity + volume-agreement watertightness, require the welded DISPLAY mesh
/// to be a closed, 2-manifold, CONSISTENTLY-WOUND boundary.
///
/// `is_watertight` compares the `.abs()` of the divergence-theorem volume, so a
/// CONSISTENTLY-FLIPPED or T-junction-leaking blend mesh can still pass it — the
/// exact blind spot that let the #89 cone-rim mis-weld (mesh `oriented == false`
/// + open edges on the narrow rim) pass "fixed". This gate closes it so a
/// mis-oriented cone-rim blend can never regress unnoticed.
fn assert_valid_watertight_oriented(m: &mut BRepModel, s: SolidId, what: &str) {
    assert_valid_watertight(m, s, what);
    let report = geometry_engine::harness::watertight::manifold_report(m, s, 0.1, 1e-6)
        .unwrap_or_else(|| panic!("{what}: solid did not tessellate"));
    assert_eq!(
        report.boundary_edges, 0,
        "{what}: mesh not closed ({} open edges)",
        report.boundary_edges
    );
    assert_eq!(
        report.nonmanifold_edges, 0,
        "{what}: mesh not 2-manifold ({} non-manifold edges)",
        report.nonmanifold_edges
    );
    assert!(
        report.oriented,
        "{what}: mesh not consistently oriented ({} inconsistent directed edges)",
        report.inconsistent_directed_edges
    );
}

fn count_surface(m: &BRepModel, s: SolidId, want: SurfaceType) -> usize {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut n = 0;
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            for &fid in &shell.faces {
                if let Some(f) = m.faces.get(fid) {
                    if let Some(surf) = m.surfaces.get(f.surface_id) {
                        if surf.surface_type() == want {
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    n
}

// Tube: outer wall R10, bore R6, z 0..20. Outer wall + bore are
// cylinders; top + bottom caps are annular planes. The bore-top rim
// (r≈6, z≈20) is the inner loop of the top cap.
const TUBE: &[(f64, f64)] = &[(10.0, 0.0), (10.0, 20.0), (6.0, 20.0), (6.0, 0.0)];

#[test]
fn bore_rim_fillet_succeeds_watertight_with_torus() {
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 6.0, 20.0).expect("bore-top rim is a closed edge");

    let tori_before = count_surface(&m, s, SurfaceType::Torus);
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut m, s, vec![rim], opts).expect("bore rim fillet must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Torus),
        tori_before + 1,
        "bore fillet must add exactly one torus blend face"
    );
    assert!(
        m.edges.get(rim).is_none(),
        "original bore rim edge must be retired"
    );
    assert_valid_watertight(&mut m, s, "bore-rim fillet");
}

/// Block (40×40×10) DIFFERENCE an r6 through cylinder — the exact live-dogfood
/// fixture. The boolean leaves the bore's TOP (z=10) and BOTTOM (z=0) rims
/// sharing ONE seamed wall cylinder. Filleting BOTH rims must stay sound
/// regardless of which rim is filleted first.
///
/// The first rim's surgery rebuilds the shared wall surface via
/// `Cylinder::new_finite` (fillet.rs step 7), which resets the cylinder's
/// `ref_dir` to `axis.perpendicular()` — a different angular anchor than the
/// bore's original seam. If the second rim seated its seam vertex from that
/// rebuilt `ref_dir` instead of from its own rim-vertex geometry, the two seam
/// vertices would land at different angles and the straight lateral seam LINE
/// between them would bow `R(1 − cos Δθ/2)` inside the wall (here
/// 6(1 − 1/√2) = 1.757), failing validation with "edge lies off Cylinder
/// surface". Both fillet orders must be sound, so we assert it both ways.
fn holed_block(m: &mut BRepModel) -> SolidId {
    let block = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 10.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    // create_box_3d is centred on the origin (z ∈ [-5, 5]); lift it to z ∈ [0, 10]
    // so the through cylinder (base z=-1, height 12) pierces it cleanly.
    geometry_engine::operations::transform::translate(
        m,
        vec![block],
        Vector3::Z,
        5.0,
        geometry_engine::operations::transform::TransformOptions::default(),
    )
    .expect("lift block");
    let hole = match TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -1.0), Vector3::Z, 6.0, 12.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    boolean_operation(
        m,
        block,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-hole difference")
}

/// The over-split rim ARCS of BOTH bore rims (radius ≈ `r_want`, both endpoints
/// at the same height — i.e. horizontal). Excludes the shared vertical seam
/// edge (different endpoint heights), which is a lateral seam, not a fillable
/// rim. Passing these to `fillet_edges` reproduces the live path: coalescing
/// rebuilds each rim's closed edge, so both bore rims are filleted on the ONE
/// shared wall cylinder (the seam-invariance case) while the block's outer
/// corners are left untouched.
fn bore_rim_arc_edges(m: &BRepModel, s: SolidId, r_want: f64) -> Vec<EdgeId> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let radial = |vid| -> Option<[f64; 3]> { m.vertices.get_position(vid) };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sh in shells {
        let Some(shell) = m.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = m.faces.get(fid) else {
                continue;
            };
            for lid in face.all_loops() {
                let Some(lp) = m.loops.get(lid) else { continue };
                for &eid in &lp.edges {
                    let Some(e) = m.edges.get(eid) else { continue };
                    let (Some(a), Some(b)) = (radial(e.start_vertex), radial(e.end_vertex)) else {
                        continue;
                    };
                    let ra = (a[0] * a[0] + a[1] * a[1]).sqrt();
                    let rb = (b[0] * b[0] + b[1] * b[1]).sqrt();
                    let horizontal = (a[2] - b[2]).abs() < 1e-6;
                    if (ra - r_want).abs() < 0.25
                        && (rb - r_want).abs() < 0.25
                        && horizontal
                        && seen.insert(eid)
                    {
                        out.push(eid);
                    }
                }
            }
        }
    }
    out
}

#[test]
fn through_bore_both_rims_fillet_is_sound() {
    let mut m = BRepModel::new();
    let s = holed_block(&mut m);

    let opts = FilletOptions {
        fillet_type: FilletType::Constant(2.0),
        radius: 2.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };

    let rim_arcs = bore_rim_arc_edges(&m, s, 6.0);
    assert!(
        rim_arcs.len() >= 2,
        "expected the over-split bore-rim arcs of both rims, found {}",
        rim_arcs.len()
    );
    fillet_edges(&mut m, s, rim_arcs, opts)
        .expect("filleting both bore rims (shared wall) must succeed");

    // Both rims blended on ONE shared wall cylinder: the pre-fix chord-sag would
    // have surfaced here as an "edge lies off Cylinder surface" validation error.
    assert_valid_watertight_oriented(&mut m, s, "through-bore both rims");
}

#[test]
fn bore_rim_chamfer_succeeds_watertight_with_cone() {
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 6.0, 20.0).expect("bore-top rim is a closed edge");

    let cones_before = count_surface(&m, s, SurfaceType::Cone);
    let opts = ChamferOptions::default(); // symmetric 1.0
    chamfer_edges(&mut m, s, vec![rim], opts).expect("bore rim chamfer must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Cone),
        cones_before + 1,
        "bore chamfer must add exactly one cone blend face"
    );
    assert_valid_watertight_oriented(&mut m, s, "bore-rim chamfer");
}

#[test]
fn outer_rim_of_annular_cap_still_works() {
    // Regression guard: the OUTER rim of the same annular cap must keep
    // working after the bore-rim generalization (radial_out = +1 path).
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 10.0, 20.0).expect("outer-top rim");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut m, s, vec![rim], opts).expect("outer rim fillet still works");
    assert_valid_watertight(&mut m, s, "outer-rim fillet (annular cap)");
}

#[test]
fn bore_rim_fillet_radius_too_large_rejected_cleanly() {
    // The rounded bore (R+r) must not reach the cap's outer edge (R_outer
    // = 10). r = 3.5 → 6 + 3.5 = 9.5 < 10 is fine; r = 4.5 → 10.5 > 10
    // must be rejected with an actionable message, not a panic.
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 6.0, 20.0).expect("bore-top rim");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(4.5),
        radius: 4.5,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let err =
        fillet_edges(&mut m, s, vec![rim], opts).expect_err("over-large bore fillet rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("bore") || msg.contains("outer edge"),
        "expected a bore-width rejection, got: {msg}"
    );
}

#[test]
fn cone_walled_rim_fillet_succeeds() {
    // A cone-frustum tube: the outer-top rim is Plane+Cone. Closed-edge fillet
    // now supports this (#89 — `cone_rim_fillet`, an analytic torus carrier), so
    // it must produce a SOUND, watertight solid with the torus blend — never a
    // corrupt solid and never NotImplemented.
    let cone: &[(f64, f64)] = &[(10.0, 0.0), (6.0, 20.0), (4.0, 20.0), (8.0, 0.0)];
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, cone);
    let rim = rim_at(&m, 6.0, 20.0).expect("cone outer-top rim");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let blend = fillet_edges(&mut m, s, vec![rim], opts).expect("cone-walled rim fillet (#89)");
    assert_eq!(
        blend.len(),
        1,
        "expected one torus blend face, got {}",
        blend.len()
    );
    // #89 cone-rim mis-weld regression: require a consistently-oriented mesh
    // (not just volume-agreement watertight), so a flipped/torn cone-rim blend
    // can never pass "fixed" again.
    assert_valid_watertight_oriented(&mut m, s, "cone-walled outer-top rim fillet");
}

// ---------------------------------------------------------------------------
// #82 Slice 2 — BOOLEAN-CUT blind bore (box − cylinder), not a revolved tube.
//
// Unlike `revolve_tube`, a boolean Difference (a) over-splits each bore rim
// into co-circular arcs (healed by `coalesce_smooth_cocurve_chains`) and, more
// importantly, (b) leaves the bore Cylinder's `height_limits` describing the
// UN-clipped extent of the cutter, not the surviving bore-wall face. Filleting
// the bore-TOP opening rim (the convex hole mouth where the bore breaks through
// the box top) previously failed validation with "edge lies 3.0 off cylinder"
// because `cylinder_rim_fillet` read the cap height from those stale
// `height_limits` (placing the cap ring one fillet-radius above the shortened
// cylinder) instead of from the rim's real height. The bore-BOTTOM (blind
// floor) rim was immune only because its cap height came from the un-clipped
// end. These gates pin both rims on a boolean-cut solid.
// ---------------------------------------------------------------------------

/// Box 40³ centred on the origin (z ∈ [−20, 20]) with a blind bore of radius
/// `bore_r` cut from the top: cylinder axis +Z, blind floor at `floor_z`,
/// opening through the top face at z = 20. Returns the solid plus the bore
/// radius and the two rim heights (top opening, blind floor).
fn boxed_blind_bore(bore_r: f64, floor_z: f64) -> (BRepModel, SolidId, f64, f64, f64) {
    let mut m = BRepModel::new();
    TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("box");
    let boxed = m.solids.iter().map(|(id, _)| id).last().expect("box solid");
    // Cutter runs from the blind floor up past the top face so the bore opens.
    let cutter_h = 20.0 - floor_z + 5.0;
    let cyl = match TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, floor_z), Vector3::Z, bore_r, cutter_h)
        .expect("cutter cylinder")
    {
        GeometryId::Solid(s) => s,
        other => panic!("expected a solid cutter, got {other:?}"),
    };
    let s = boolean_operation(
        &mut m,
        boxed,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("blind-bore Difference");
    (m, s, bore_r, 20.0, floor_z)
}

/// Every edge of `s` whose BOTH endpoints lie on the circle (radius `r_want`
/// about +Z, height `z_want`) — i.e. the arcs of one bore rim.
fn rim_arc_edges(m: &BRepModel, s: SolidId, r_want: f64, z_want: f64) -> Vec<EdgeId> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut ids = std::collections::BTreeSet::new();
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            for &fid in &shell.faces {
                if let Some(f) = m.faces.get(fid) {
                    let mut loops = vec![f.outer_loop];
                    loops.extend_from_slice(&f.inner_loops);
                    for lid in loops {
                        if let Some(l) = m.loops.get(lid) {
                            for &e in &l.edges {
                                ids.insert(e);
                            }
                        }
                    }
                }
            }
        }
    }
    let on_circle = |p: [f64; 3]| -> bool {
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        (r - r_want).abs() < 0.25 && (p[2] - z_want).abs() < 0.25
    };
    ids.into_iter()
        .filter(|&e| {
            let ed = match m.edges.get(e) {
                Some(ed) => ed,
                None => return false,
            };
            let sp = m.vertices.get_position(ed.start_vertex);
            let ep = m.vertices.get_position(ed.end_vertex);
            matches!((sp, ep), (Some(a), Some(b)) if on_circle(a) && on_circle(b))
        })
        .collect()
}

#[test]
fn boxed_blind_bore_top_opening_rim_fillet_watertight() {
    let (mut m, s, bore_r, top_z, _floor_z) = boxed_blind_bore(8.0, -5.0);
    let rim = rim_arc_edges(&m, s, bore_r, top_z);
    assert!(
        !rim.is_empty(),
        "expected to find the bore-top opening rim arcs"
    );

    let tori_before = count_surface(&m, s, SurfaceType::Torus);
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(3.0),
        radius: 3.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut m, s, rim, opts)
        .expect("boolean-cut blind-bore TOP opening rim fillet must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Torus),
        tori_before + 1,
        "top opening rim fillet must add exactly one torus blend face"
    );
    assert_valid_watertight_oriented(&mut m, s, "boxed blind-bore top opening rim fillet");
}

#[test]
fn boxed_blind_bore_top_opening_rim_chamfer_watertight() {
    // Fillet/chamfer PARITY (#82 successor): the SAME boolean-cut bore rim that
    // `boxed_blind_bore_top_opening_rim_fillet_watertight` rounds must also
    // CHAMFER. The Difference over-splits the rim into co-circular arcs joined
    // at 2-valent smooth vertices; before the shared
    // `coalesce_smooth_cocurve_chains` healing was wired into `chamfer_edges`,
    // those joints read as shared CORNER vertices and the F2-δ corner pre-flight
    // refused the whole op with a `NotImplemented` "share corner vertex … same-
    // kind corner-patch synthesis … not yet implemented". Healing merges the
    // arcs into one closed rim edge, which `create_edge_chamfer` routes to
    // `create_closed_edge_chamfer` (cone-frustum rim blend). Assert the rim now
    // chamfers SOUND: exactly one cone blend face, watertight + manifold +
    // consistently oriented.
    let (mut m, s, bore_r, top_z, _floor_z) = boxed_blind_bore(8.0, -5.0);
    let rim = rim_arc_edges(&m, s, bore_r, top_z);
    assert!(
        !rim.is_empty(),
        "expected to find the bore-top opening rim arcs"
    );
    assert!(
        rim.len() >= 2,
        "boolean Difference must over-split the rim into arcs, found {}",
        rim.len()
    );

    let cones_before = count_surface(&m, s, SurfaceType::Cone);
    let opts = ChamferOptions::default(); // symmetric 1.0
    chamfer_edges(&mut m, s, rim, opts)
        .expect("boolean-cut blind-bore TOP opening rim chamfer must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Cone),
        cones_before + 1,
        "top opening rim chamfer must add exactly one cone blend face"
    );
    assert_valid_watertight_oriented(&mut m, s, "boxed blind-bore top opening rim chamfer");
}

#[test]
fn through_bore_both_rims_chamfer_is_sound() {
    // Chamfer parity for the through-hole shared-wall case (the fillet analogue
    // is `through_bore_both_rims_fillet_is_sound`). Both boolean-split bore rims
    // share ONE seamed wall cylinder; healing rebuilds each rim's closed edge so
    // both rims chamfer on the shared wall. Before the shared coalescing was
    // wired into chamfer, this refused at the corner pre-flight.
    let mut m = BRepModel::new();
    let s = holed_block(&mut m);

    let rim_arcs = bore_rim_arc_edges(&m, s, 6.0);
    assert!(
        rim_arcs.len() >= 2,
        "expected the over-split bore-rim arcs of both rims, found {}",
        rim_arcs.len()
    );

    let cones_before = count_surface(&m, s, SurfaceType::Cone);
    let opts = ChamferOptions::default(); // symmetric 1.0
    chamfer_edges(&mut m, s, rim_arcs, opts)
        .expect("chamfering both bore rims (shared wall) must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Cone),
        cones_before + 2,
        "both bore rims must each add one cone blend face"
    );
    assert_valid_watertight_oriented(&mut m, s, "through-bore both rims chamfer");
}

#[test]
fn boxed_blind_bore_bottom_floor_rim_fillet_watertight() {
    // The genuinely-concave blind-floor rim already worked pre-fix; this pins
    // that the cap-height-from-rim change does not regress it.
    let (mut m, s, bore_r, _top_z, floor_z) = boxed_blind_bore(8.0, -5.0);
    let rim = rim_arc_edges(&m, s, bore_r, floor_z);
    assert!(
        !rim.is_empty(),
        "expected to find the bore-bottom floor rim arcs"
    );

    let tori_before = count_surface(&m, s, SurfaceType::Torus);
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(3.0),
        radius: 3.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut m, s, rim, opts)
        .expect("boolean-cut blind-bore BOTTOM floor rim fillet must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Torus),
        tori_before + 1,
        "bottom floor rim fillet must add exactly one torus blend face"
    );
    assert_valid_watertight_oriented(&mut m, s, "boxed blind-bore bottom floor rim fillet");
}
