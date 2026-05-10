//! Geometric correctness probe for fillet on a 10×10×10 box.
//!
//! Tessellates the post-fillet model and reports where the fillet face
//! actually sits in space. Used to catch the "fillet outside the box"
//! regression that visual smoke tests caught but the existing
//! topology-only regression suite does not.

use geometry_engine::math::Tolerance;
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

#[test]
fn print_fillet_geometry_for_every_box_edge() {
    let box_size = 10.0_f64;
    let radius = 2.0_f64;

    let edges_to_test: Vec<EdgeId> = {
        let mut model = BRepModel::new();
        let _solid = TopologyBuilder::new(&mut model)
            .create_box_3d(box_size, box_size, box_size)
            .expect("box");
        model.edges.iter().map(|(id, _)| id).collect()
    };

    println!(
        "Box [0,{}]^3, fillet radius {}. Testing {} edges.",
        box_size,
        radius,
        edges_to_test.len()
    );

    let mut bad = 0usize;
    for (i, edge_id) in edges_to_test.iter().enumerate() {
        let mut model = BRepModel::new();
        let solid_id = match TopologyBuilder::new(&mut model).create_box_3d(box_size, box_size, box_size) {
            Ok(GeometryId::Solid(id)) => id,
            _ => panic!("box"),
        };
        let edges_for_this_run: Vec<EdgeId> =
            model.edges.iter().map(|(id, _)| id).collect();
        let edge = edges_for_this_run[i];

        let opts = FilletOptions {
            fillet_type: FilletType::Constant(radius),
            radius,
            propagation: PropagationMode::None,
            ..Default::default()
        };
        let blend_faces = match fillet_edges(&mut model, solid_id, vec![edge], opts) {
            Ok(f) => f,
            Err(e) => {
                println!("  edge#{} fillet ERR: {:?}", i, e);
                bad += 1;
                continue;
            }
        };
        let bf = blend_faces[0];

        // Tessellate just the blend face's surface to get a bbox.
        let face = model.faces.get(bf).expect("blend face");
        let surface = model.surfaces.get(face.surface_id).expect("blend surf");
        // Sample 5×5 grid in u,v ∈ [0,1] and find min/max.
        let mut bb_min = [f64::INFINITY; 3];
        let mut bb_max = [f64::NEG_INFINITY; 3];
        for ui in 0..=8 {
            let u = ui as f64 / 8.0;
            for vi in 0..=8 {
                let v = vi as f64 / 8.0;
                if let Ok(p) = surface.point_at(u, v) {
                    let coords = [p.x, p.y, p.z];
                    for k in 0..3 {
                        if coords[k] < bb_min[k] {
                            bb_min[k] = coords[k];
                        }
                        if coords[k] > bb_max[k] {
                            bb_max[k] = coords[k];
                        }
                    }
                }
            }
        }

        // The fillet face's bbox must fit inside the original box
        // [0, box_size]^3 (with small numerical slack). If any
        // coordinate goes negative or exceeds box_size, the fillet is
        // on the wrong side.
        let slack = 1e-6;
        let outside = (0..3).any(|k| {
            bb_min[k] < -slack || bb_max[k] > box_size + slack
        });
        let tag = if outside { "OUTSIDE" } else { "inside " };
        println!(
            "  edge#{:2} ({:?}) → bbox [{:6.3},{:6.3},{:6.3}] - [{:6.3},{:6.3},{:6.3}]  {}",
            i,
            edge,
            bb_min[0],
            bb_min[1],
            bb_min[2],
            bb_max[0],
            bb_max[1],
            bb_max[2],
            tag,
        );
        if outside {
            bad += 1;
        }
    }

    println!(
        "\nResult: {}/{} edges placed the fillet OUTSIDE the box.",
        bad,
        edges_to_test.len()
    );
    let _ = Tolerance::default();
}
