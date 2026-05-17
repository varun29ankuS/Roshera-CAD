//! F4-α.3 — pin the analytic kpart dispatcher in the live fillet path.
//!
//! `fillet_surfaces::kpart_tests` unit-tests the `from_analytic_kpart`
//! constructors in isolation. `fillet_analytic_surface_contract.rs`
//! pins the `BlendSurfaceCarrier::from_spine_rail` tag against the
//! solver kind. Neither closes the loop on whether the F4-α.2
//! dispatcher inside `fillet::create_blend_surface_for_carrier`
//! actually fires in the production `fillet_edges` entry point.
//!
//! This file fills that gap: build a box, fillet one plane/plane
//! edge, retrieve the resulting blend face's surface from the model,
//! downcast to [`CylindricalFillet`], and assert the kpart-signature
//! invariants that a legacy `::new` construction would NOT satisfy:
//!
//! * `axis_field.len() == 2` — the kpart constructor stores a single
//!   midpoint frame as a 2-element constant Vec; the legacy 20-sample
//!   loop in `CylindricalFillet::new` would produce 20.
//! * All four per-station fields (`axis_field`, `frame_x_field`,
//!   `frame_y_field`, `angle_span`) carry length 2 with identical
//!   element pairs (the constancy invariant).
//!
//! ## Scope: cylindrical only
//!
//! A symmetric toroidal pin against a plane/cylinder rim was
//! considered, but the cylinder cap-rim case is handled by a
//! specialised closed-edge path (`cylinder_rim_fillet` in
//! `fillet.rs`) that builds a raw [`Torus`] primitive directly and
//! short-circuits `create_blend_surface_for_carrier` entirely. The
//! kpart toroidal arm in the dispatcher only fires for *open*
//! plane/cylinder edges (e.g. a partial bore through a planar face),
//! which requires a boolean-cut setup that is out of scope for this
//! α.3 integration pin. The toroidal kpart constructor remains
//! exercised by `fillet_surfaces::kpart_tests` and by the F4-α.1
//! contract test against the carrier-enum dispatch.

use geometry_engine::operations::edge_classification::find_adjacent_faces;
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::fillet_surfaces::CylindricalFillet;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Build a 10×10×10 box, return its `SolidId`.
fn make_box(model: &mut BRepModel, size: f64) -> geometry_engine::primitives::solid::SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(size, size, size)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

/// First edge whose two adjacent faces are both planar (a box edge).
fn first_plane_plane_edge(model: &BRepModel) -> EdgeId {
    for (edge_id, _) in model.edges.iter() {
        let faces = find_adjacent_faces(model, edge_id);
        if faces.len() != 2 {
            continue;
        }
        let surface_type = |face_id: FaceId| -> SurfaceType {
            let face = model.faces.get(face_id).expect("face exists");
            let surface = model.surfaces.get(face.surface_id).expect("surface exists");
            surface.surface_type()
        };
        if surface_type(faces[0]) == SurfaceType::Plane
            && surface_type(faces[1]) == SurfaceType::Plane
        {
            return edge_id;
        }
    }
    panic!("no plane/plane edge on the box");
}

#[test]
fn cylindrical_kpart_fires_on_live_box_edge_fillet() {
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model, 10.0);
    let edge_id = first_plane_plane_edge(&model);

    let radius = 2.0_f64;
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let blend_faces = fillet_edges(&mut model, solid_id, vec![edge_id], opts)
        .expect("fillet on a plane/plane box edge succeeds");
    assert_eq!(
        blend_faces.len(),
        1,
        "single-edge fillet emits exactly one blend face"
    );

    let face = model.faces.get(blend_faces[0]).expect("blend face exists");
    let surface = model
        .surfaces
        .get(face.surface_id)
        .expect("blend face's surface exists");

    let cyl = surface
        .as_any()
        .downcast_ref::<CylindricalFillet>()
        .expect(
            "plane/plane constant-radius fillet must route through \
             BlendSurfaceCarrier::Cylindrical → from_analytic_kpart \
             (an unexpected surface type here means the F4-α.2 \
             dispatcher silently fell back to the legacy or NURBS arm)",
        );

    assert_eq!(
        cyl.axis_field.len(),
        2,
        "kpart-constructed CylindricalFillet carries a 2-element \
         constant frame (legacy ::new produces 20)"
    );
    assert_eq!(cyl.frame_x_field.len(), 2);
    assert_eq!(cyl.frame_y_field.len(), 2);
    assert_eq!(cyl.angle_span.len(), 2);

    // Constancy invariant: kpart writes the same midpoint frame into
    // both slots. Bit-exact equality is the right check here — the
    // constructor writes a single computed value into both vec
    // positions via `vec![v, v]`, so a regression that switches back
    // to per-sample derivation would diverge instantly.
    assert_eq!(cyl.axis_field[0], cyl.axis_field[1]);
    assert_eq!(cyl.frame_x_field[0], cyl.frame_x_field[1]);
    assert_eq!(cyl.frame_y_field[0], cyl.frame_y_field[1]);
    assert_eq!(cyl.angle_span[0], cyl.angle_span[1]);

    assert_eq!(
        cyl.radius, radius,
        "kpart stores the caller-supplied radius bit-exactly"
    );
}
