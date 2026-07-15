// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! AGENT-RENDER-α.6 — eyes on the diagnostic cells.
//!
//! Renders the boolean results of the still-broken cone radial cells
//! (and one conquered control) to PNGs under `render_out/`, in both
//! Shaded and FaceIds modes, iso + front views. A vision-capable agent
//! (or a human) can then LOOK at the failure instead of inferring shape
//! from volume numbers — the perception loop applied to our own kernel
//! debugging, which is its first and most demanding customer.
//!
//! Run: `cargo test -p geometry-engine --test diag_render -- --ignored --nocapture`

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::render::{render_solid, CanonicalView, RenderMode, RenderOptions};

#[allow(clippy::expect_used, clippy::panic)] // diagnostic-only fixture
fn the_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("box: {o:?}"),
    }
}

#[allow(clippy::expect_used, clippy::panic)] // diagnostic-only fixture
fn cone(model: &mut BRepModel, bc: [f64; 3], rb: f64, rt: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cone_3d(Point3::new(bc[0], bc[1], bc[2]), Vector3::Z, rb, rt, h)
        .expect("cone")
    {
        GeometryId::Solid(id) => id,
        o => panic!("cone: {o:?}"),
    }
}

#[test]
#[ignore = "diagnostic — renders failing cone cells to render_out/ (run with --ignored --nocapture)"]
#[allow(clippy::expect_used)]
fn render_cone_radial_cells() {
    let out_dir = std::path::Path::new("render_out");
    std::fs::create_dir_all(out_dir).expect("mkdir render_out");

    // (cone config, label, ops to render)
    let cells: Vec<([f64; 3], f64, f64, f64, &str)> = vec![
        // Broken: ∪/∖ carry open+nonman edges; ∩ errors pre-guard.
        ([1.0, 0.0, -0.5], 0.5, 0.3, 1.0, "radial-face-x"),
        // Broken: ∪ fragment blowup.
        ([0.0, 0.0, -1.5], 1.5, 0.5, 3.0, "wider-than-box"),
        // Conquered control: should look clean.
        ([1.4, 0.0, -0.5], 0.6, 0.4, 1.0, "poke-past-CONTROL"),
    ];
    let ops = [
        (BooleanOp::Union, "U"),
        (BooleanOp::Difference, "D"),
        (BooleanOp::Intersection, "I"),
    ];
    let views = [
        (CanonicalView::Isometric, "iso"),
        (CanonicalView::Front, "front"),
    ];

    for (bc, rb, rt, h, label) in &cells {
        for (op, opname) in &ops {
            let mut model = BRepModel::new();
            let bx = the_box(&mut model);
            let cn = cone(&mut model, *bc, *rb, *rt, *h);
            let res = match boolean_operation(&mut model, bx, cn, *op, BooleanOptions::default()) {
                Ok(r) => r,
                Err(e) => {
                    println!("{label} [{opname}] ERR {e:?} — nothing to render");
                    continue;
                }
            };
            for (view, viewname) in &views {
                for (mode, modename) in
                    [(RenderMode::Shaded, "shaded"), (RenderMode::FaceIds, "ids")]
                {
                    let Some(frame) = render_solid(
                        &model,
                        res,
                        &RenderOptions {
                            view: *view,
                            mode,
                            ..Default::default()
                        },
                    ) else {
                        println!("{label} [{opname}] {viewname}/{modename}: render none");
                        continue;
                    };
                    let png = frame.to_png().expect("png");
                    let path = out_dir.join(format!("{label}_{opname}_{viewname}_{modename}.png"));
                    std::fs::write(&path, &png).expect("write png");
                    println!(
                        "wrote {} ({} faces in legend)",
                        path.display(),
                        frame.face_legend.len()
                    );
                }
            }
        }
    }
}
