// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A flanged hub: Ø80 x 12 flange, Ø45 x 20 hub, Ø30 bore. Found live
//! 2026-11-12 finishing it with fillets and chamfers:
//!
//! - the union leaves the hub/flange junction as 16 arcs, each its own arc
//!   curve on the same circle; the pre-blend healer merged only arcs sharing
//!   one curve object, so an R3 fillet on the junction was refused (each
//!   joint read as a corner);
//! - a REFUSED blend still left the model changed (the healer merged other
//!   rims' arcs first), unrecorded -- the live model and its replay
//!   disagreed on edge ids.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn hub() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let f = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 40.0, 12.0)
        .expect("flange"));
    let h = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 12.0), Vector3::Z, 22.5, 20.0)
        .expect("hub"));
    let s = boolean_operation(&mut m, f, h, BooleanOp::Union, BooleanOptions::default())
        .expect("union");
    let b = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -4.0), Vector3::Z, 15.0, 40.0)
        .expect("bore"));
    let s = boolean_operation(
        &mut m,
        s,
        b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore");
    (m, s)
}

/// Non-straight edges whose start lies on the circle (radius `r`, height `z`).
fn rim(m: &BRepModel, r: f64, z: f64) -> Vec<EdgeId> {
    let mut out: Vec<EdgeId> = m
        .edges
        .iter()
        .filter(|(_, e)| {
            let on = m.vertices.get(e.start_vertex).is_some_and(|v| {
                let p = v.position;
                (p[2] - z).abs() < 1e-6 && ((p[0] * p[0] + p[1] * p[1]).sqrt() - r).abs() < 1e-6
            });
            on && m
                .curves
                .get(e.curve_id)
                .is_some_and(|c| !c.is_linear(geometry_engine::math::Tolerance::default()))
        })
        .map(|(id, _)| id)
        .collect();
    out.sort_unstable();
    out
}

/// Everything about the topology a refused op must leave alone.
fn fingerprint(m: &BRepModel) -> (Vec<EdgeId>, usize, usize, usize) {
    let mut edges: Vec<EdgeId> = m.edges.iter().map(|(id, _)| id).collect();
    edges.sort_unstable();
    (
        edges,
        m.vertices.iter().count(),
        m.loops.iter().count(),
        m.faces.iter().count(),
    )
}

#[test]
fn the_hub_flange_junction_takes_a_fillet() {
    let (mut m, part) = hub();
    let junction = rim(&m, 22.5, 12.0);
    assert!(
        junction.len() > 1,
        "fixture: the union splits the junction ({junction:?})"
    );
    fillet_edges(
        &mut m,
        part,
        junction,
        FilletOptions {
            fillet_type: FilletType::Constant(3.0),
            radius: 3.0,
            ..Default::default()
        },
    )
    .expect("an R3 fillet where the hub meets the flange");
    let cert = m.certify_solid(part);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "{cert:?}"
    );
    // The right fillet, not just a closed one: from the fillet's own centre
    // circle (radius 22.5 + 3, height 12 + 3 -- in the air beside the
    // junction) a ray toward the fillet's 45° point must hit the fillet
    // surface at exactly r = 3, all the way round.
    let r = 3.0_f64;
    let (c_rad, c_z) = (22.5 + r, 12.0 + r);
    for deg in [0.0_f64, 37.0, 90.0, 200.0, 315.0] {
        let (s, c) = deg.to_radians().sin_cos();
        let origin = Point3::new(c_rad * c, c_rad * s, c_z);
        let toward = Vector3::new(-c, -s, -1.0);
        let hit = geometry_engine::queries::raycast::raycast_solid(&m, part, origin, toward)
            .unwrap_or_else(|| panic!("at {deg}°: the ray misses the fillet"));
        assert!(
            (hit.distance - r).abs() < 1e-6,
            "at {deg}°: the fillet surface is {:.6} mm from its centre circle, not {r}",
            hit.distance
        );
    }
}

#[test]
fn a_refused_blend_leaves_the_model_as_it_was() {
    let (mut m, part) = hub();
    let before = fingerprint(&m);
    // R30 on the bore rim cannot fit a 12 mm flange: refused.
    let bore_rim = rim(&m, 15.0, 0.0);
    let refused = fillet_edges(
        &mut m,
        part,
        bore_rim.clone(),
        FilletOptions {
            fillet_type: FilletType::Constant(30.0),
            radius: 30.0,
            ..Default::default()
        },
    );
    assert!(refused.is_err(), "fixture: an R30 fillet here is refused");
    assert_eq!(
        fingerprint(&m),
        before,
        "a refused fillet changed the model"
    );

    let refused = chamfer_edges(
        &mut m,
        part,
        bore_rim,
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(30.0),
            distance1: 30.0,
            distance2: 30.0,
            ..Default::default()
        },
    );
    assert!(refused.is_err(), "fixture: a 30 mm chamfer here is refused");
    assert_eq!(
        fingerprint(&m),
        before,
        "a refused chamfer changed the model"
    );
}

/// Loops whose edges do not chain end to start (as oriented in the loop).
fn broken_loops(m: &BRepModel) -> Vec<String> {
    let mut out = Vec::new();
    for (lid, lp) in m.loops.iter() {
        let ends: Vec<(u32, u32)> = lp
            .edges
            .iter()
            .zip(&lp.orientations)
            .filter_map(|(&e, &fwd)| {
                let ed = m.edges.get(e)?;
                Some(if fwd {
                    (ed.start_vertex, ed.end_vertex)
                } else {
                    (ed.end_vertex, ed.start_vertex)
                })
            })
            .collect();
        for i in 0..ends.len() {
            let j = (i + 1) % ends.len();
            if ends[i].1 != ends[j].0 {
                out.push(format!(
                    "loop {lid}: edge {} ends at vertex {}, edge {} starts at {}",
                    lp.edges[i], ends[i].1, lp.edges[j], ends[j].0
                ));
            }
        }
    }
    out
}

/// A 0.5 mm chamfer on the hub's top rim hid everything below it in the TOP
/// view: the chamfer's cone carries explicit [0, 2π] angle limits, which read
/// as an open sector with no u period, so its (u, v) trim polygon folded to
/// zero area and the trim test accepted every point -- any ray from above
/// stopped on the infinite cone. Found live: the hub's sheet showed no bolt
/// holes and no Ø80 outline in TOP.
#[test]
fn a_chamfered_hub_top_hides_nothing_below_it() {
    use geometry_engine::drawing::standard_drawing_hlr;
    use geometry_engine::drawing::types::{ProjectionType, SheetSize};
    let (mut m, part) = hub();
    let top_rim = rim(&m, 22.5, 32.0);
    chamfer_edges(
        &mut m,
        part,
        top_rim,
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(0.5),
            distance1: 0.5,
            distance2: 0.5,
            ..Default::default()
        },
    )
    .expect("a 0.5 mm chamfer on the hub top");
    let d = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let top = d
        .views
        .iter()
        .find(|v| matches!(v.projection, ProjectionType::Top))
        .expect("top view");
    let shows = |r: f64| top.circles.iter().any(|c| (c.r - r).abs() < 1e-6);
    let hides = |r: f64| top.hidden_circles.iter().any(|c| (c.r - r).abs() < 1e-6);
    assert!(
        shows(40.0) && !hides(40.0),
        "the Ø80 rim is seen from above: {:?} / hidden {:?}",
        top.circles.iter().map(|c| c.r).collect::<Vec<_>>(),
        top.hidden_circles.iter().map(|c| c.r).collect::<Vec<_>>()
    );
    assert!(
        shows(22.0) && shows(22.5),
        "the chamfer's own rims are seen"
    );
}
