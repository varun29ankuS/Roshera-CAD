//! Dogfood finding F1 — filleting the bottom (−Z-facing) outer rim of a
//! cylindrical disk fails validation while the top (+Z) rim of the SAME
//! cylinder fillets cleanly.
//!
//! Reproduced live against the running kernel via MCP: a Ø120 (r = 60),
//! height-16 disk. Filleting the +Z rim (radius 3) certifies SOUND; the
//! −Z rim surfaces
//!   `filleted solid failed validation: edge N lies 1.757e1 off face M's
//!    Cylinder surface`.
//!
//! The two rims are the same local geometry (a plane meeting the cylinder
//! at r = 60); the only difference is the adjacent cap's normal sign
//! (+Z top vs −Z bottom). A correct constant-radius rim fillet must place
//! its lateral trim edge ON the host cylinder for either orientation.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Mirror the MCP `create_cylinder` kernel path exactly (verified against
/// `POST /api/geometry/cylinder` → `create_cylinder_3d`): an analytic
/// cylinder primitive, base at `base`, axis `axis`.
fn make_disk_axis(
    model: &mut BRepModel,
    base: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_cylinder_3d(base, axis, radius, height)
        .expect("cylinder creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

/// Every closed (rim) edge: start_vertex == end_vertex.
fn closed_rims(model: &BRepModel) -> Vec<EdgeId> {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| if edge.is_loop() { Some(id) } else { None })
        .collect()
}

/// Closed (rim) edges that belong to a specific solid's boundary — walk
/// the solid's shells → faces → loops and keep the loop edges that are
/// closed. Distinguishes a solid's own rims from a coincident neighbour's.
fn disk_boundary_rims(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    for shell_id in s.shell_ids() {
        let Some(shell) = model.shells.get(shell_id) else {
            continue;
        };
        for &face_id in shell.face_ids() {
            let Some(face) = model.faces.get(face_id) else {
                continue;
            };
            let mut loops = vec![face.outer_loop];
            loops.extend(&face.inner_loops);
            for lid in loops {
                if let Some(l) = model.loops.get(lid) {
                    for &e in &l.edges {
                        if let Some(edge) = model.edges.get(e) {
                            if edge.is_loop() && !out.contains(&e) {
                                out.push(e);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Signed coordinate of a rim edge's vertex along `axis` from `base`.
fn rim_axis_coord(model: &BRepModel, rim: EdgeId, base: Point3, axis: Vector3) -> f64 {
    let e = model.edges.get(rim).expect("rim edge exists");
    let p = model
        .vertices
        .get_position(e.start_vertex)
        .expect("rim vertex has a position");
    (Point3::new(p[0], p[1], p[2]) - base).dot(&axis)
}

fn fillet_opts(radius: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// Fillet the rim whose axial coordinate is closest to `target_coord`
/// on a FRESH disk, returning the operation result.
fn fillet_rim_at(
    base: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
    fillet_r: f64,
    target_coord: f64,
) -> Result<(), String> {
    let mut model = BRepModel::new();
    let solid = make_disk_axis(&mut model, base, axis, radius, height);
    let rims = closed_rims(&model);
    let rim = *rims
        .iter()
        .min_by(|&&a, &&b| {
            let da = (rim_axis_coord(&model, a, base, axis) - target_coord).abs();
            let db = (rim_axis_coord(&model, b, base, axis) - target_coord).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("disk has rim edges");
    fillet_edges(&mut model, solid, vec![rim], fillet_opts(fillet_r))
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// The RED for F1: the +Z rim fillets SOUND but the −Z rim fails
/// validation on the SAME disk. `validate_result` defaults true, so a
/// successful `fillet_edges` return IS the "sound" verdict.
#[test]
fn disk_bottom_rim_fillet_matches_top_rim() {
    let (radius, height, fillet_r) = (60.0_f64, 16.0_f64, 3.0_f64);

    // TOP rim (+Z, z = height): the known-good direction.
    let top = fillet_rim_at(Point3::ORIGIN, Vector3::Z, radius, height, fillet_r, height);
    assert!(
        top.is_ok(),
        "top (+Z) rim fillet must succeed and validate; got {:?}",
        top.err()
    );

    // BOTTOM rim (−Z, z = 0): must be just as sound. Under F1 this
    // returns the `... lies 1.7e1 off ... Cylinder surface` TopologyError.
    let bottom = fillet_rim_at(Point3::ORIGIN, Vector3::Z, radius, height, fillet_r, 0.0);
    assert!(
        bottom.is_ok(),
        "F1: bottom (−Z) rim fillet must be as sound as the top rim, \
         but validation failed: {:?}",
        bottom.err()
    );
}

/// F1 TRUE ROOT CAUSE (diagnostic, not a fillet bug). The live failure
/// only appears when a *second, spatially-coincident* solid shares the
/// disk's rim. `create_cylinder_topology` builds its seam vertices with
/// `VertexStore::add_or_find`, which MERGES a coincident vertex from ANY
/// pre-existing solid in the shared model. A neighbour flange sharing the
/// z=0 rim footprint therefore shares the disk's bottom-rim seam vertex;
/// filleting that rim moves the shared vertex and corrupts the neighbour
/// (`edge N lies <radius> off face M's Plane surface`). The top rim, being
/// interior to the neighbour, shares no vertex and fillets clean — which
/// is the *entire* source of the reported top/bottom asymmetry.
///
/// This test pins the CURRENT (buggy) behaviour so the finding is
/// reproducible in-kernel. It is NOT the orientation sign bug F1
/// hypothesised — the isolated disk above proves both rims are sound.
#[test]
fn coincident_neighbor_shared_rim_fillet_corrupts_neighbor() {
    let (radius, height, fillet_r) = (60.0_f64, 16.0_f64, 3.0_f64);

    // Neighbour solid spanning z ∈ [0, 54] with the SAME Ø120 footprint,
    // built FIRST so its z=0 rim vertex is already in the store.
    let mut model = BRepModel::new();
    let _neighbor = make_disk_axis(&mut model, Point3::ORIGIN, Vector3::Z, radius, 54.0);
    // Disk built SECOND — its bottom-rim seam vertex (60,0,0) coincides
    // with the neighbour's and is merged by add_or_find.
    let disk = make_disk_axis(&mut model, Point3::ORIGIN, Vector3::Z, radius, height);

    // Collect the DISK's own boundary rim edges (its two caps' loops).
    let disk_rims: Vec<EdgeId> = disk_boundary_rims(&model, disk);
    let disk_bottom = *disk_rims
        .iter()
        .min_by(|&&a, &&b| {
            let za = rim_axis_coord(&model, a, Point3::ORIGIN, Vector3::Z).abs();
            let zb = rim_axis_coord(&model, b, Point3::ORIGIN, Vector3::Z).abs();
            za.partial_cmp(&zb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("disk has rim edges");

    // The disk's bottom-rim seam vertex is SHARED with the neighbour
    // (add_or_find merge). Filleting it moves the shared vertex and breaks
    // the neighbour → the live F1 failure ("... off face M's Plane surface").
    let res = fillet_edges(&mut model, disk, vec![disk_bottom], fillet_opts(fillet_r));
    assert!(
        res.is_err(),
        "REGRESSION MARKER: filleting a disk rim whose seam vertex is merged \
         with a coincident neighbour must currently corrupt the neighbour and \
         fail validation. If this now passes, the cross-solid vertex merge \
         (add_or_find) has been isolated — update the F1 report."
    );
    let msg = format!("{:?}", res.err());
    assert!(
        msg.contains("off face") && msg.contains("surface"),
        "expected an off-surface validation error from the corrupted \
         neighbour; got: {msg}"
    );
}

/// Generality: the bug is orientation-derived, not "z == 0" specific.
/// A disk lying on its side (axis = +X) must fillet BOTH rims — the one
/// facing −X (adjacent cap normal opposite the build axis) is the
/// analogue of the −Z bottom rim.
#[test]
fn disk_on_side_both_rims_fillet_sound() {
    let (radius, height, fillet_r) = (60.0_f64, 16.0_f64, 3.0_f64);
    let base = Point3::ORIGIN;
    let axis = Vector3::X;

    let far = fillet_rim_at(base, axis, radius, height, fillet_r, height);
    assert!(
        far.is_ok(),
        "+axis rim of a side-lying disk must fillet sound; got {:?}",
        far.err()
    );

    let near = fillet_rim_at(base, axis, radius, height, fillet_r, 0.0);
    assert!(
        near.is_ok(),
        "−axis rim of a side-lying disk must fillet sound (same class as \
         the −Z bottom rim); got {:?}",
        near.err()
    );
}
