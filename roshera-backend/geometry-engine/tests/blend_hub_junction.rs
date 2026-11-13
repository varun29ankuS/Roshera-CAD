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

/// The R3 junction fillet seen from the front is drawn as a drafter draws
/// it: a quarter arc on each side of the hub (the fillet's outline), not a
/// full circle. Found live: FRONT showed a small full circle at the hub's
/// base -- the torus's seam (a quarter arc facing the camera) drawn and
/// closed into a circle, with no outline synthesized for the torus at all.
#[test]
fn the_junction_fillet_draws_as_quarter_arcs_from_the_front() {
    use geometry_engine::drawing::standard_drawing_hlr;
    use geometry_engine::drawing::types::{ProjectionType, SheetSize};
    let (mut m, part) = hub();
    let junction = rim(&m, 22.5, 12.0);
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
    .expect("R3 junction fillet");
    let d = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let front = d
        .views
        .iter()
        .find(|v| matches!(v.projection, ProjectionType::Front))
        .expect("front view");
    assert!(
        front.circles.iter().all(|c| (c.r - 3.0).abs() > 1e-6),
        "no full circle of the fillet radius: {:?}",
        front.circles.iter().map(|c| c.r).collect::<Vec<_>>()
    );
    // A quarter arc of radius 3 about each tube centre (±25.5, 15).
    for cx in [-25.5_f64, 25.5] {
        let on_arc = |p: &[f64; 2]| ((p[0] - cx).hypot(p[1] - 15.0) - 3.0).abs() < 1e-6;
        let arc = front
            .polylines
            .iter()
            .find(|pl| pl.points.len() >= 3 && pl.points.iter().all(on_arc));
        let arc = arc.unwrap_or_else(|| panic!("no fillet outline arc about ({cx}, 15)"));
        let angle = |p: &[f64; 2]| (p[1] - 15.0).atan2(p[0] - cx);
        let (a0, a1) = (
            angle(&arc.points[0]),
            angle(arc.points.last().expect("end")),
        );
        let sweep = (a1 - a0).abs().min(std::f64::consts::TAU - (a1 - a0).abs());
        assert!(
            (sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "the outline about ({cx}, 15) is a quarter arc, sweep {sweep}"
        );
    }
}

/// A blended cylinder is dimensioned to the design's sharp corners, as a
/// drafter does: the hub is 20 high (flange top to hub top) whether or not
/// an R3 fillet and a 0.5 chamfer round its ends. The face's own trimmed
/// height (15 → 31.5 = 16.5) was printed instead.
#[test]
fn a_blended_hub_is_dimensioned_to_its_sharp_corners() {
    use geometry_engine::primitives::surface::Cylinder;
    use geometry_engine::readable::dimensions::extract_dimensions;
    let (mut m, part) = hub();
    // Chamfer first, then the fillet (the order the live hub was built in;
    // the other order is its own defect, see below).
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
    .expect("0.5 chamfer on the hub top");
    let junction = rim(&m, 22.5, 12.0);
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
    .expect("R3 junction fillet");
    let hub_face = m
        .faces
        .iter()
        .find(|(_, f)| {
            m.surfaces.get(f.surface_id).is_some_and(|s| {
                s.as_any()
                    .downcast_ref::<Cylinder>()
                    .is_some_and(|c| (c.radius - 22.5).abs() < 1e-9)
            })
        })
        .map(|(id, _)| id)
        .expect("the hub's cylinder face");
    let dims = extract_dimensions(&m, part);
    let length = dims
        .iter()
        .find(|d| d.kind == "length" && d.entities == vec![hub_face])
        .map(|d| d.value)
        .expect("the hub's length dimension");
    assert!(
        (length - 20.0).abs() < 1e-6,
        "hub height {length}, expected 20"
    );
}

/// The hub with a `chamfer` mm equal-distance chamfer on its top rim, then an
/// R`fillet` junction fillet (the order the live hub was built in).
fn blended_hub(chamfer: f64, fillet: f64) -> (BRepModel, SolidId) {
    let (mut m, part) = hub();
    let top_rim = rim(&m, 22.5, 32.0);
    chamfer_edges(
        &mut m,
        part,
        top_rim,
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(chamfer),
            distance1: chamfer,
            distance2: chamfer,
            ..Default::default()
        },
    )
    .expect("chamfer on the hub top");
    let junction = rim(&m, 22.5, 12.0);
    fillet_edges(
        &mut m,
        part,
        junction,
        FilletOptions {
            fillet_type: FilletType::Constant(fillet),
            radius: fillet,
            ..Default::default()
        },
    )
    .expect("junction fillet");
    (m, part)
}

/// Face ids of the solid whose surface satisfies `pred`, sorted.
fn faces_where(
    m: &BRepModel,
    pred: impl Fn(&dyn geometry_engine::primitives::surface::Surface) -> bool,
) -> Vec<u32> {
    let mut out: Vec<u32> = m
        .faces
        .iter()
        .filter(|(_, f)| m.surfaces.get(f.surface_id).is_some_and(|s| pred(s)))
        .map(|(id, _)| id)
        .collect();
    out.sort_unstable();
    out
}

fn torus_faces(m: &BRepModel) -> Vec<u32> {
    use geometry_engine::primitives::surface::Torus;
    faces_where(m, |s| s.as_any().downcast_ref::<Torus>().is_some())
}

fn cone_faces(m: &BRepModel) -> Vec<u32> {
    use geometry_engine::primitives::surface::Cone;
    faces_where(m, |s| s.as_any().downcast_ref::<Cone>().is_some())
}

/// A drafter calls each blend out where it shows in profile: "R3" on the
/// junction fillet and "0.5 × 45°" on the hub's top chamfer, each on a leader
/// whose arrow touches the blend, in FRONT (where the fillet is an arc and the
/// chamfer a line). Each callout names its blend face and a durable id, so
/// the sheet certificate re-measures it like any other dimension. Found live
/// 2026-11-12: the hub's sheet carried no callout for either blend.
#[test]
fn a_blended_hub_sheet_calls_out_its_fillet_and_its_chamfer() {
    use geometry_engine::drawing::layout::compute_layout;
    use geometry_engine::drawing::types::ProjectionType;
    use geometry_engine::drawing::{
        certify_drawing, standard_drawing_auto, standard_drawing_hlr, SheetFactKind, SheetVerdict,
    };
    let (m, part) = blended_hub(0.5, 3.0);
    let tori = torus_faces(&m);
    let cones = cone_faces(&m);
    assert!(
        !tori.is_empty() && !cones.is_empty(),
        "fixture: a fillet and a chamfer"
    );
    let auto = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("auto sheet");
    let hlr = standard_drawing_hlr(
        &m,
        part,
        uuid::Uuid::nil(),
        geometry_engine::drawing::types::SheetSize::A3,
        1.0,
    )
    .expect("hlr sheet");
    for (route, d) in [("auto", &auto), ("hlr", &hlr)] {
        let all: Vec<(String, String, String)> = d
            .views
            .iter()
            .flat_map(|v| {
                v.dimensions
                    .iter()
                    .map(move |x| (v.name.clone(), x.kind.clone(), x.label.clone()))
            })
            .collect();
        let (vi, front) = d
            .views
            .iter()
            .enumerate()
            .find(|(_, v)| matches!(v.projection, ProjectionType::Front))
            .expect("front view");
        let fillet = front
            .dimensions
            .iter()
            .find(|x| x.label == "R3.00")
            .unwrap_or_else(|| panic!("{route}: no R3 callout in FRONT; the sheet shows {all:?}"));
        assert_eq!(
            fillet.entities, tori,
            "{route}: R3 names the fillet face(s)"
        );
        assert!(fillet.pid.is_some(), "{route}: R3 carries a durable id");
        // Its point lies on the fillet's profile arc: radius 3 about the tube
        // centre (25.5, 15) beside the hub, inside the quarter between the hub
        // wall and the flange top.
        let p = fillet.a;
        assert!(
            ((p[0] - 25.5).hypot(p[1] - 15.0) - 3.0).abs() < 1e-6 && p[0] < 25.5 && p[1] < 15.0,
            "{route}: the R3 leader point {p:?} is not on the fillet's arc"
        );
        let chamfer = front
            .dimensions
            .iter()
            .find(|x| x.label == "0.50 × 45°")
            .unwrap_or_else(|| {
                panic!("{route}: no 0.50 × 45° callout in FRONT; the sheet shows {all:?}")
            });
        assert_eq!(
            chamfer.entities, cones,
            "{route}: the chamfer names its cone"
        );
        assert!(
            chamfer.pid.is_some(),
            "{route}: the chamfer carries a durable id"
        );
        // On the chamfer's line, from (22.5, 31.5) to (22, 32).
        let q = chamfer.a;
        assert!(
            ((q[0] - 22.5) + (q[1] - 31.5)).abs() < 1e-6 && q[0] < 22.5 && q[0] > 22.0,
            "{route}: the chamfer leader point {q:?} is not on the chamfer"
        );
        // The chamfer cone states the chamfer, not a 45° angle and a diameter.
        assert!(
            !d.views.iter().flat_map(|v| v.dimensions.iter()).any(|x| {
                matches!(x.kind.as_str(), "angle" | "diameter")
                    && x.entities.iter().any(|e| cones.contains(e))
            }),
            "{route}: the chamfer cone is also stated as an angle or diameter: {all:?}"
        );
        // Each callout is inked once, on a leader whose arrow touches it.
        let layout = compute_layout(d);
        for (callout, label) in [(fillet, "R3.00"), (chamfer, "0.50 × 45°")] {
            let inked: Vec<_> = layout
                .dimensions
                .iter()
                .filter(|pd| pd.label == label)
                .collect();
            assert_eq!(
                inked.len(),
                1,
                "{route}: {label} inked {} times",
                inked.len()
            );
            let tip = inked[0].arrows[0].tip;
            let want = [
                front.position_mm[0] + callout.a[0] * front.scale,
                (d.sheet_size.height() - front.position_mm[1]) - callout.a[1] * front.scale,
            ];
            assert!(
                (tip[0] - want[0]).hypot(tip[1] - want[1]) < 1e-6,
                "{route}: {label}'s arrow at {tip:?}, not on the blend at {want:?}"
            );
            let leg = inked[0].ext[0];
            assert!(
                (leg[1][0] - leg[0][0]).hypot(leg[1][1] - leg[0][1]) > 1.0,
                "{route}: {label} has no leader leg"
            );
            assert_eq!(inked[0].owner_view, vi, "{route}: {label} belongs to FRONT");
        }
        // Certified like any other dimension.
        let cert = certify_drawing(&m, d);
        for label in ["R3.00", "0.50 × 45°"] {
            let fact = cert
                .facts
                .iter()
                .find(|f| f.kind == SheetFactKind::Dimension && f.label == label)
                .unwrap_or_else(|| panic!("{route}: no certificate fact for {label}"));
            assert_eq!(
                fact.live.verdict,
                SheetVerdict::Consistent,
                "{route}: {label}: {fact:?}"
            );
        }
        assert!(
            cert.sound,
            "{route}: {:?}",
            cert.unsound_facts().collect::<Vec<_>>()
        );
    }
}

/// The callouts are re-measured, not parroted: the R3 / 0.5 sheet certified
/// against the same hub blended R4 / 0.8 reads both callouts STALE (the
/// blend faces keep their durable ids across the size change, so this is the
/// value check speaking, not a lost reference).
#[test]
fn a_blend_callout_goes_stale_when_the_blend_changes_size() {
    use geometry_engine::drawing::{
        certify_drawing, standard_drawing_hlr, types::SheetSize, SheetFactKind, SheetVerdict,
    };
    let (m3, part3) = blended_hub(0.5, 3.0);
    let (m4, part4) = blended_hub(0.8, 4.0);
    assert_eq!(part3, part4, "fixture: same solid id");
    let pids = |m: &BRepModel, faces: Vec<u32>| -> Vec<_> {
        faces.into_iter().map(|f| m.face_pid(f)).collect()
    };
    assert_eq!(
        pids(&m3, torus_faces(&m3)),
        pids(&m4, torus_faces(&m4)),
        "fixture: the fillet keeps its durable id across a radius change"
    );
    assert_eq!(
        pids(&m3, cone_faces(&m3)),
        pids(&m4, cone_faces(&m4)),
        "fixture: the chamfer keeps its durable id across a size change"
    );
    let sheet =
        standard_drawing_hlr(&m3, part3, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("R3 sheet");
    let cert = certify_drawing(&m4, &sheet);
    for (label, live) in [("R3.00", 4.0), ("0.50 × 45°", 0.8)] {
        let fact = cert
            .facts
            .iter()
            .find(|f| f.kind == SheetFactKind::Dimension && f.label == label)
            .unwrap_or_else(|| panic!("no certificate fact for {label}"));
        assert_eq!(fact.live.verdict, SheetVerdict::Stale, "{label}: {fact:?}");
        let measured = fact.live.measured.expect("re-measured");
        assert!(
            (measured - live).abs() < 1e-6,
            "{label}: re-measured {measured}, the model's blend is {live}"
        );
    }
    assert!(!cert.sound);
}

/// Rim rounds are called out one per rim: R1 on the flange's top and bottom
/// outer rims are two blends of one size, each with its own "R1.00" on the
/// arc it names -- not merged into one, and not dropped as a repeat of the
/// other's value.
#[test]
fn each_rim_round_gets_its_own_radius_callout() {
    use geometry_engine::drawing::types::{ProjectionType, SheetSize};
    use geometry_engine::drawing::{certify_drawing, standard_drawing_hlr, SheetVerdict};
    let (mut m, part) = blended_hub(0.5, 3.0);
    for z in [12.0, 0.0] {
        let outer = rim(&m, 40.0, z);
        fillet_edges(
            &mut m,
            part,
            outer,
            FilletOptions {
                fillet_type: FilletType::Constant(1.0),
                radius: 1.0,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("R1 on the flange rim at z = {z}: {e:?}"));
    }
    let d = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let front = d
        .views
        .iter()
        .find(|v| matches!(v.projection, ProjectionType::Front))
        .expect("front view");
    let r1: Vec<_> = front
        .dimensions
        .iter()
        .filter(|x| x.label == "R1.00")
        .collect();
    assert_eq!(
        r1.len(),
        2,
        "two R1 callouts in FRONT: {:?}",
        front
            .dimensions
            .iter()
            .map(|x| x.label.clone())
            .collect::<Vec<_>>()
    );
    // Each on its own rim's arc: tube centres (39, 11) and (39, 1).
    for (cz, x) in [(11.0, 0usize), (1.0, 1)] {
        let hit = r1
            .iter()
            .any(|c| ((c.a[0] - 39.0).hypot(c.a[1] - cz) - 1.0).abs() < 1e-6);
        assert!(
            hit,
            "callout {x}: no R1 on the arc about (39, {cz}): {r1:?}"
        );
    }
    assert_ne!(r1[0].entities, r1[1].entities, "each names its own rim");
    let cert = certify_drawing(&m, &d);
    assert!(
        cert.facts
            .iter()
            .filter(|f| f.label == "R1.00")
            .all(|f| f.live.verdict == SheetVerdict::Consistent),
        "{:?}",
        cert.facts
            .iter()
            .filter(|f| f.label == "R1.00")
            .collect::<Vec<_>>()
    );
}

/// The two blends on the hub's cylinder in either order: an R3 fillet at its
/// foot, then a 0.5 chamfer on its top rim. The chamfer was refused -- "edge
/// lies 6.59 off face's Cylinder surface" -- while chamfer-then-fillet
/// worked.
#[test]
fn a_chamfer_after_a_fillet_on_the_same_cylinder() {
    let (mut m, part) = hub();
    let junction = rim(&m, 22.5, 12.0);
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
    .expect("R3 junction fillet");
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
    .expect("0.5 chamfer on the hub top after the fillet");
    let cert = m.certify_solid(part);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "{cert:?}"
    );
    assert_eq!(broken_loops(&m), Vec::<String>::new());
}

/// The callouts' leaders keep the sheet readable: a fully blended hub (R3
/// junction, 0.5 chamfer, R1 on both flange rims) passes the layout quality
/// gate on both sheet routes, and the exported SVG inks every callout.
#[test]
fn a_blended_hub_sheet_passes_the_layout_gate_with_its_callouts() {
    use geometry_engine::drawing::types::SheetSize;
    use geometry_engine::drawing::{
        render_drawing_svg, standard_drawing_auto, standard_drawing_hlr, verify_drawing, Severity,
    };
    let (mut m, part) = blended_hub(0.5, 3.0);
    for z in [12.0, 0.0] {
        let outer = rim(&m, 40.0, z);
        fillet_edges(
            &mut m,
            part,
            outer,
            FilletOptions {
                fillet_type: FilletType::Constant(1.0),
                radius: 1.0,
                ..Default::default()
            },
        )
        .expect("R1 flange rim");
    }
    let auto = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("auto");
    let hlr = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    for (route, d) in [("auto", &auto), ("hlr", &hlr)] {
        let report = verify_drawing(d);
        let errors: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{route}: {errors:#?}");
        let svg = render_drawing_svg(d);
        for label in ["R3.00", "R1.00", "0.50 × 45°"] {
            assert!(svg.contains(label), "{route}: the SVG has no {label}");
        }
    }
}
