//! Fix A regression anchor — a boolean-Difference re-entrant notch must not
//! be certified as convex.
//!
//! The three edges meeting at the re-entrant vertex of a notched box are
//! geometrically CONCAVE (270° of material) and MUST classify
//! `convexity == -1` with a negative signed dihedral. Before Fix A the
//! dihedral classifier derived the edge-tangent handedness from face1's
//! stored loop-winding flag, which boolean Difference leaves inconsistent
//! with the (proven-correct) outward normal on tool-derived faces — so all
//! three re-entrant edges were signed `+1` (convex), a self-certifying lie.
//!
//! The control (the outer box edge farthest from the notch) is a genuine
//! convex box edge and must stay `+1`, proving the fix flips the sign ONLY
//! where the loop flag disagreed with geometry.

use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::edge_classification::classify_edge;
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 40³ box centred at origin with a 20³ notch removed from the (+,+,+)
/// corner. The inner re-entrant vertex at (0,0,0) is a concave degree-3
/// corner: three concave edges (the notch's inner vertical/horizontal edges)
/// meet there. Fixture copied from `fillet_concave_three_edge_corner.rs`.
fn notched_box(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("base")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid geometry for base box, got {o:?}"),
    };
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

#[test]
fn boolean_notch_reentrant_edges_classify_concave() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);

    // The notch must be a sound, watertight solid before we assert anything
    // about its edge convexity — otherwise a misclassification could be an
    // artifact of a broken boolean rather than the tangent-handedness bug.
    let cert = m.certify_solid(s);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "notched_box must be a sound, watertight solid; got brep_valid={} watertight={} manifold={} errors={:?}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.errors,
    );

    // Locate the re-entrant vertex ROBUSTLY (nearest to the origin), never
    // by a magic id.
    let mut origin_vid = None;
    let mut best = f64::MAX;
    for (vid, v) in m.vertices.iter() {
        let d = v.point().magnitude();
        if d < best {
            best = d;
            origin_vid = Some(vid);
        }
    }
    let origin_vid = origin_vid.expect("notched box must have vertices");
    assert!(
        best < 1e-6,
        "re-entrant vertex must sit at the origin; nearest vertex was {best} from origin"
    );

    // Its incident edges are the three concave notch edges.
    let mut reentrant: Vec<u32> = Vec::new();
    for (eid, edge) in m.edges.iter() {
        if edge.start_vertex == origin_vid || edge.end_vertex == origin_vid {
            reentrant.push(eid);
        }
    }
    assert_eq!(
        reentrant.len(),
        3,
        "the origin re-entrant corner is degree-3; got edges {reentrant:?}"
    );

    for &e in &reentrant {
        let c = classify_edge(&m, e).expect("classify re-entrant edge");
        let dih = c
            .dihedral_angle
            .expect("a manifold notch edge has a defined dihedral");
        assert_eq!(
            c.convexity, -1,
            "re-entrant notch edge {e} must be concave (-1); got convexity {} (dihedral {dih})",
            c.convexity
        );
        assert!(
            dih < 0.0,
            "re-entrant notch edge {e} dihedral must be negative (concave); got {dih}"
        );
    }

    // Control: a genuine convex box edge in the (-,-,-) corner, diagonally
    // opposite the (+,+,+) notch and untouched by the boolean. Selecting the
    // edge whose midpoint has the most-negative coordinate sum lands deep in
    // that corner, well away from any coplanar seam edge the difference leaves
    // near the notch mouth. It must stay +1 — this guards against an
    // over-broad sign flip that would corrupt correct convex edges.
    let mut control = None;
    let mut min_sum = f64::MAX;
    for (eid, edge) in m.edges.iter() {
        if reentrant.contains(&eid) {
            continue;
        }
        let mid = edge.evaluate(0.5, &m.curves).expect("edge midpoint");
        let sum = mid.x + mid.y + mid.z;
        if sum < min_sum {
            min_sum = sum;
            control = Some(eid);
        }
    }
    let control = control.expect("a control edge must exist");
    let cc = classify_edge(&m, control).expect("classify control edge");
    assert_eq!(
        cc.convexity, 1,
        "far (-,-,-) box edge {control} must stay convex (+1); got {} (dihedral {:?})",
        cc.convexity, cc.dihedral_angle
    );
}
