//! Task #82 Slice 1 — filleting the re-entrant (concave degree-3) corner of a
//! notched box. RED: currently refuses (fillet_edges rejects the concave
//! corner). GREEN after apply_apex_sphere_corner is generalized to
//! ConcaveCorner{degree:3}.
use geometry_engine::math::Vector3;
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

/// RED: the three concave edges at the re-entrant corner currently refuse.
#[test]
fn concave_three_edge_corner_currently_refuses() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);

    // Confirm the notch itself is a sound, watertight solid BEFORE we ever
    // call fillet_edges — if the boolean difference produced an unsound
    // result, a fillet refusal downstream would prove nothing about concave
    // corner support.
    let cert = m.certify_solid(s);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "notched_box must be a sound, watertight solid before filleting; \
         got brep_valid={} watertight={} manifold={} errors={:?}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.errors,
    );

    let edges = all_edges(&m, s);
    let res = fillet_edges(&mut m, s, edges, fillet_opts(3.0));
    // Pre-fix: the three concave edges at the re-entrant corner cause
    // fillet_edges to refuse. This test is the RED anchor; a later task
    // replaces it with the GREEN watertight assertion once the concave
    // corner blend is generalized.
    assert!(
        res.is_err(),
        "pre-Slice-1 this concave corner must refuse; got Ok({:?})",
        res.map(|faces| faces.len())
    );
}
