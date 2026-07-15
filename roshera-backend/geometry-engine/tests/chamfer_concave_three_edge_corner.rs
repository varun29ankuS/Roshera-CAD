// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task #82 Slice 1 — chamfering the re-entrant (concave degree-3) corner of a
//! notched box. The planar corner-cap synthesizer (`apply_planar_chamfer_cap`,
//! reached through `handle_chamfer_vertices` / `identify_chamfer_corners`) is
//! generalized from the convex apex to `ConcaveCorner { degree: 3 }`: the cap
//! plane's outward normal points INTO the removed pocket. This gate asserts the
//! chamfered solid is a sound, watertight, coherently-oriented B-Rep, that a
//! `Plane` cap face closes the re-entrant triangular hole, and that the cap's
//! outward normal points into the pocket.
use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::chamfer::{ChamferOptions, ChamferType, PropagationMode};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::operations::{chamfer_edges, CommonOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 40³ box centred at origin with a 20³ notch removed from the (+,+,+)
/// corner. The inner re-entrant vertex at (0,0,0) is a concave degree-3
/// corner: three concave edges (the notch's inner vertical/horizontal edges)
/// meet there.
fn notched_box(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("base")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid geometry for base box, got {o:?}"),
    };
    // 20^3 tool centred at origin, then shifted by (10,10,10) so it occupies
    // exactly the (+,+,+) octant corner of the 40^3 base (which spans
    // -20..+20 on each axis): the tool then spans -10..+10 + 10 = 0..20 on
    // each axis, cutting a cube bite out of the (+,+,+) corner. The re-entrant
    // vertex left behind sits at the origin.
    let tool = match TopologyBuilder::new(m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid geometry for tool box, got {o:?}"),
    };
    translate(m, vec![tool], Vector3::X, 10.0, TransformOptions::default()).expect("tx");
    translate(m, vec![tool], Vector3::Y, 10.0, TransformOptions::default()).expect("ty");
    translate(m, vec![tool], Vector3::Z, 10.0, TransformOptions::default()).expect("tz");
    boolean_operation(
        m,
        base,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("notch")
}

/// All non-loop edges of the solid (collected across outer + inner shells,
/// deduplicated).
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

/// The three re-entrant edges of the notch that meet at the origin. Each is
/// the intersection of two notch walls (x=0∩y=0, x=0∩z=0, y=0∩z=0), so all
/// three share the concave degree-3 vertex at (0,0,0); their far ends land on
/// the notch's three MIXED-convexity corners at (20,0,0)/(0,20,0)/(0,0,20).
/// Selecting only these three edges keeps the far ends degree-1 (a plain
/// chamfer termination) — the Mixed corners are Slice-3 territory and out of
/// Task #82 Slice-1 scope; this test exercises exactly the concave planar
/// corner cap and nothing else.
fn concave_reentrant_edges(model: &BRepModel, solid: SolidId) -> (Vec<EdgeId>, u32) {
    let origin_vid = model
        .vertices
        .iter()
        .find(|(_, v)| {
            v.position[0].abs() < 1e-6 && v.position[1].abs() < 1e-6 && v.position[2].abs() < 1e-6
        })
        .map(|(id, _)| id)
        .expect("origin (re-entrant) vertex");
    let mut out: Vec<EdgeId> = all_edges(model, solid)
        .into_iter()
        .filter(|&e| {
            model
                .edges
                .get(e)
                .map(|ed| ed.start_vertex == origin_vid || ed.end_vertex == origin_vid)
                .unwrap_or(false)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    (out, origin_vid)
}

fn chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        symmetric: true,
        propagation: PropagationMode::None,
        preserve_edges: false,
        partial_corner_vertices: Vec::new(),
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..ChamferOptions::default()
    }
}

/// GREEN (Task #82 Slice 1): chamfering the three re-entrant concave edges that
/// meet at the origin must now SUCCEED and leave a sound, watertight solid,
/// with a planar corner cap at the re-entrant apex whose outward normal points
/// INTO the notch pocket (away from material).
#[test]
fn concave_three_edge_corner_chamfers_watertight() {
    use geometry_engine::primitives::face::FaceOrientation;
    use geometry_engine::primitives::surface::Plane;

    let mut m = BRepModel::new();
    let s = notched_box(&mut m);

    // The notch must be a sound, watertight solid BEFORE chamfering — a
    // downstream failure otherwise would prove nothing about corner support.
    let pre = m.certify_solid(s);
    assert!(
        pre.brep_valid && pre.watertight && pre.manifold,
        "notched_box must be sound before chamfering; brep_valid={} watertight={} \
         manifold={} errors={:?}",
        pre.brep_valid,
        pre.watertight,
        pre.manifold,
        pre.errors,
    );

    let (concave_edges, _origin) = concave_reentrant_edges(&m, s);
    assert_eq!(
        concave_edges.len(),
        3,
        "the re-entrant corner must have exactly three incident concave edges; got {:?}",
        concave_edges
    );
    let produced = chamfer_edges(&mut m, s, concave_edges, chamfer_opts(3.0))
        .expect("concave three-edge corner chamfer must succeed");
    assert!(
        !produced.is_empty(),
        "chamfer must return the produced faces (three chamfer faces + one cap)"
    );

    // The chamfered solid must remain structurally sound: watertight (closes,
    // zero boundary edges), manifold, coherently oriented (no flipped-normal
    // faces — this is what would catch a cap face pointing the wrong way at
    // the mesh level), and a valid B-Rep.
    let cert = m.certify_solid(s);
    assert!(
        cert.watertight,
        "concave-chamfered notched box must be watertight; boundary_edges={} cert={:?}",
        cert.boundary_edges, cert,
    );
    assert!(
        cert.manifold && cert.oriented && cert.brep_valid,
        "concave-chamfered notched box must be a sound oriented B-Rep; \
         manifold={} oriented={} brep_valid={} inconsistent_directed_edges={} cert={:?}",
        cert.manifold,
        cert.oriented,
        cert.brep_valid,
        cert.inconsistent_directed_edges,
        cert,
    );

    // Locate the re-entrant corner cap: a triangular Plane face whose three
    // corner vertices sit near the origin in the (+,+,+) pocket region, one on
    // each notch wall at chamfer distance 3 from the origin along the two
    // in-wall axes — i.e. positions that are a permutation of (0, 3, 3). The
    // cap plane is x + y + z = 6 (the chord plane cutting the re-entrant
    // corner; each cap corner sums to 6), so its outward normal is ±(1,1,1)/√3.
    let solid = m.solids.get(s).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("outer shell");
    let mut concave_cap: Option<(u32, Vector3, FaceOrientation)> = None;
    for &fid in &shell.faces {
        let Some(face) = m.faces.get(fid) else {
            continue;
        };
        let Some(surf) = m.surfaces.get(face.surface_id) else {
            continue;
        };
        let Some(plane) = surf.as_any().downcast_ref::<Plane>() else {
            continue;
        };
        // The three cap corners are the chamfer offset points on the three
        // notch walls, a permutation of (3,0,3)/(0,3,3)/(3,3,0): they lie on
        // the plane x + y + z = 6, unit normal ±(1,1,1)/√3. Every other planar
        // face of the notched box is axis-aligned (a box outer face or a notch
        // wall, normal ±X/±Y/±Z), so the (1,1,1)-parallel plane uniquely
        // identifies the re-entrant corner cap.
        let unit = (1.0 / 3.0_f64.sqrt()) * Vector3::new(1.0, 1.0, 1.0);
        let aligned = plane.normal.normalize().expect("cap plane normal unit");
        let is_diag = aligned.dot(&unit).abs() > 1.0 - 1e-6;
        // Confirm it is the x+y+z=6 chord plane cutting the (+,+,+) corner
        // (|origin·unit_normal| = 6/√3 = 2√3), not some other diagonal plane.
        let offset =
            plane.origin.x * aligned.x + plane.origin.y * aligned.y + plane.origin.z * aligned.z;
        if is_diag && (offset.abs() - 2.0 * 3.0_f64.sqrt()).abs() < 1e-3 {
            concave_cap = Some((fid, plane.normal, face.orientation));
        }
    }
    let (_cap_fid, plane_normal, orientation) =
        concave_cap.expect("a planar re-entrant cap on x+y+z=6 must exist");

    // The cap's oriented outward normal must point INTO the pocket — i.e.
    // toward the removed cube's interior at (+,+,+). The stored plane normal is
    // ±(1,1,1); the face orientation flips it. Assert the oriented normal has a
    // strictly positive dot with the pocket direction (1,1,1).
    let oriented = if orientation == FaceOrientation::Backward {
        Vector3::new(-plane_normal.x, -plane_normal.y, -plane_normal.z)
    } else {
        plane_normal
    };
    let pocket_dir = Vector3::new(1.0, 1.0, 1.0);
    assert!(
        oriented.dot(&pocket_dir) > 0.0,
        "re-entrant planar cap outward normal must point INTO the notch pocket \
         (+,+,+); got oriented={:?} (orientation={:?}) dot={}",
        oriented,
        orientation,
        oriented.dot(&pocket_dir),
    );
}
