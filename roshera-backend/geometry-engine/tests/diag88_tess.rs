//! #88 poke-through tessellation diagnostic: the interior-offset r=1.05 ∩/∖
//! main sphere face (lens-complement outer loop + two z-circle holes) must
//! route to `tessellate_spherical_holed_region`. This serial repro prints the
//! result face inventory (loops, curve types, hint presence) and the per-op
//! volumes so the dispatch gate that bails can be read from data.

use geometry_engine::math::Point3;
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

#[allow(clippy::expect_used, clippy::panic)] // diagnostic-only test fixture
fn the_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("box: {o:?}"),
    }
}

#[allow(clippy::expect_used, clippy::panic)] // diagnostic-only test fixture
fn sphere(model: &mut BRepModel, c: [f64; 3], r: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(c[0], c[1], c[2]), r)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        o => panic!("sphere: {o:?}"),
    }
}

#[test]
#[ignore = "diagnostic — #88 r=1.2 crossing-presplit routing (run with --ignored --nocapture)"]
fn diag88_r12_presplit() {
    for (op, sym) in [(BooleanOp::Intersection, "I"), (BooleanOp::Union, "U")] {
        let mut model = BRepModel::new();
        let bx = the_box(&mut model);
        let sp = sphere(&mut model, [0.5, 0.3, 0.0], 1.2);
        match boolean_operation(&mut model, bx, sp, op, BooleanOptions::default()) {
            Ok(res) => {
                let vol = model.calculate_solid_volume(res).unwrap_or(f64::NAN);
                println!("== r=1.2 {sym}: vol={vol:.4} ==");
            }
            Err(e) => println!("== r=1.2 {sym}: ERR {e:?} =="),
        }
    }
}

#[test]
#[ignore = "diagnostic — #88 holed-region tessellation routing (run with --ignored --nocapture)"]
fn diag88_tess_routing() {
    for (op, sym) in [(BooleanOp::Intersection, "I"), (BooleanOp::Difference, "D")] {
        let mut model = BRepModel::new();
        let bx = the_box(&mut model);
        let sp = sphere(&mut model, [0.5, 0.3, 0.0], 1.05);
        let res = match boolean_operation(&mut model, bx, sp, op, BooleanOptions::default()) {
            Ok(r) => r,
            Err(e) => {
                println!("{sym}: ERR {e:?}");
                continue;
            }
        };
        let vol = model.calculate_solid_volume(res).unwrap_or(f64::NAN);
        println!("== {sym}: vol={vol:.4} ==");
        // Inventory every face of the result: surface type, outer-loop edge
        // count + curve types, inner loop count, hint presence.
        let shell_ids = model
            .solids
            .get(res)
            .map(|s| s.all_shells())
            .unwrap_or_default();
        for sid in shell_ids {
            let face_ids = model
                .shells
                .get(sid)
                .map(|sh| sh.faces.clone())
                .unwrap_or_default();
            for fid in face_ids {
                let Some(face) = model.faces.get(fid) else {
                    continue;
                };
                let sty = model
                    .surfaces
                    .get(face.surface_id)
                    .map(|s| format!("{:?}", s.surface_type()))
                    .unwrap_or_else(|| "?".into());
                let outer = model.loops.get(face.outer_loop);
                let outer_n = outer.map(|l| l.edges.len()).unwrap_or(0);
                let curve_kinds: Vec<&'static str> = outer
                    .map(|l| {
                        l.edges
                            .iter()
                            .filter_map(|&eid| {
                                let e = model.edges.get(eid)?;
                                let c = model.curves.get(e.curve_id)?;
                                Some(c.type_name())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let hint = model.cap_apex_hint.get(&fid).map(|h| *h.value());
                println!(
                    "  face={fid:?} surf={sty} outer_edges={outer_n} kinds={curve_kinds:?} \
                     inner_loops={} hint={hint:?}",
                    face.inner_loops.len(),
                );
            }
        }
    }
}
