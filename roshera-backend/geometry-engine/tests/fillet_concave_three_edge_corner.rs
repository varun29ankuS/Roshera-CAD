// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task #82 Slice 1 — filleting the re-entrant (concave degree-3) corner of a
//! notched box. The apex-sphere synthesizer (`apply_apex_sphere_corner`) is
//! generalized from the convex apex to `ConcaveCorner { degree: 3 }`: the
//! rolling-ball centre lands in the removed pocket, so the synthesized sphere
//! cap's outward normal points INTO the void. This gate asserts the filleted
//! solid is a sound, watertight, coherently-oriented B-Rep and that the cap
//! normal points into the pocket.
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
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
    // each axis, flush with three of the base's outer faces and protruding
    // past them by nothing (co-planar) while cutting a cube bite out of the
    // (+,+,+) corner. The re-entrant vertex left behind sits at the origin.
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

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        ..Default::default()
    }
}

/// The three re-entrant edges of the notch that meet at the origin. Each is
/// the intersection of two notch walls (x=0∩y=0, x=0∩z=0, y=0∩z=0), so all
/// three share the concave degree-3 vertex at (0,0,0); their far ends land on
/// the notch's three MIXED-convexity corners at (20,0,0)/(0,20,0)/(0,0,20).
/// Selecting only these three edges keeps the far ends degree-1 (a plain
/// cylinder-fillet termination) — the Mixed corners are F5-δ territory and
/// out of Task #82 Slice-1 scope; this test exercises exactly the concave
/// apex-sphere corner patch and nothing else.
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

/// GREEN (Task #82 Slice 1): filleting the three re-entrant concave edges that
/// meet at the origin must now SUCCEED and leave a sound, watertight solid,
/// with a radius-3 spherical corner cap at the re-entrant apex whose outward
/// normal points INTO the notch pocket (away from material).
#[test]
fn concave_three_edge_corner_fillets_watertight() {
    use geometry_engine::primitives::face::FaceOrientation;
    use geometry_engine::primitives::surface::Sphere;

    let mut m = BRepModel::new();
    let s = notched_box(&mut m);

    // The notch must be a sound, watertight solid BEFORE filleting — a
    // downstream failure otherwise would prove nothing about corner support.
    let pre = m.certify_solid(s);
    assert!(
        pre.brep_valid && pre.watertight && pre.manifold,
        "notched_box must be sound before filleting; brep_valid={} watertight={} \
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
    fillet_edges(&mut m, s, concave_edges, fillet_opts(3.0))
        .expect("concave three-edge corner fillet must succeed");

    // The filleted solid must remain structurally sound: watertight (closes,
    // zero boundary edges), manifold, coherently oriented (no flipped-normal
    // faces — this is what would catch a cap face pointing the wrong way at
    // the mesh level), and a valid B-Rep.
    let cert = m.certify_solid(s);
    assert!(
        cert.watertight,
        "concave-filleted notched box must be watertight; boundary_edges={} cert={:?}",
        cert.boundary_edges, cert,
    );
    assert!(
        cert.manifold && cert.oriented && cert.brep_valid,
        "concave-filleted notched box must be a sound oriented B-Rep; \
         manifold={} oriented={} brep_valid={} inconsistent_directed_edges={} cert={:?}",
        cert.manifold,
        cert.oriented,
        cert.brep_valid,
        cert.inconsistent_directed_edges,
        cert,
    );

    // Locate the re-entrant corner cap: a radius-3 Sphere face whose centre is
    // the rolling-ball apex in the void at (r, r, r) = (3, 3, 3) — inside the
    // removed pocket, which is what distinguishes a concave cap from a convex
    // apex sphere (whose centre sits in the material).
    let solid = m.solids.get(s).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("outer shell");
    let mut concave_cap: Option<(u32, Point3)> = None;
    for &fid in &shell.faces {
        let Some(face) = m.faces.get(fid) else {
            continue;
        };
        let Some(surf) = m.surfaces.get(face.surface_id) else {
            continue;
        };
        if let Some(sp) = surf.as_any().downcast_ref::<Sphere>() {
            if (sp.radius - 3.0).abs() < 1e-6
                && (sp.center.x - 3.0).abs() < 1e-3
                && (sp.center.y - 3.0).abs() < 1e-3
                && (sp.center.z - 3.0).abs() < 1e-3
            {
                concave_cap = Some((fid, sp.center));
            }
        }
    }
    let (cap_fid, cap_center) =
        concave_cap.expect("a radius-3 re-entrant sphere cap centred at (3,3,3) must exist");

    // The cap's oriented outward normal, sampled at the patch point nearest
    // the original sharp corner (the origin), must point INTO the pocket —
    // i.e. toward the removed cube's interior at (+,+,+). Radially outward at
    // that point is (cap_pt - centre) ≈ (-,-,-); the face must therefore be
    // oriented Backward so its outward normal is (+,+,+).
    let cap_face = m.faces.get(cap_fid).expect("cap face");
    let to_origin = (Point3::new(0.0, 0.0, 0.0) - cap_center)
        .normalize()
        .expect("cap centre distinct from origin");
    let cap_pt = Point3::new(
        cap_center.x + to_origin.x * 3.0,
        cap_center.y + to_origin.y * 3.0,
        cap_center.z + to_origin.z * 3.0,
    );
    // Radially-outward sphere normal at cap_pt.
    let radial = (cap_pt - cap_center)
        .normalize()
        .expect("cap point distinct from centre");
    let oriented = if cap_face.orientation == FaceOrientation::Backward {
        Vector3::new(-radial.x, -radial.y, -radial.z)
    } else {
        radial
    };
    let pocket_dir = Vector3::new(1.0, 1.0, 1.0);
    assert!(
        oriented.dot(&pocket_dir) > 0.0,
        "re-entrant sphere cap outward normal must point INTO the notch pocket \
         (+,+,+); got oriented={:?} (orientation={:?}) dot={}",
        oriented,
        cap_face.orientation,
        oriented.dot(&pocket_dir),
    );
}
