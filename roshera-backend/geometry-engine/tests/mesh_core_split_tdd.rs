// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! TDD gate for the mesh-core split (Move 2, Task 1).
//!
//! Three tests — one per new public surface:
//!  • `manifold_report_mesh`   — mesh-level manifold analysis (no tessellation)
//!  • `mesh_self_intersects_mesh` — mesh-level self-intersection check
//!  • `render_mesh`            — rasterize a raw TriangleMesh → RenderFrame
//!
//! Write FIRST (RED: functions do not exist yet), implement → GREEN.

use geometry_engine::harness::self_intersection::{
    mesh_self_intersects, mesh_self_intersects_mesh,
};
use geometry_engine::harness::watertight::{manifold_report, manifold_report_mesh};
use geometry_engine::math::vector3::Vector3;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::render::{render_mesh, CanonicalView, RenderMode, RenderOptions};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

/// Build a 4×3×2 box in a fresh model; return its SolidId.
#[allow(clippy::expect_used, clippy::panic)]
fn build_box(model: &mut BRepModel) -> geometry_engine::primitives::solid::SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(4.0, 3.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid, got {other:?}"),
    }
}

/// RED → GREEN: `manifold_report_mesh` must agree with `manifold_report`
/// (the existing solid wrapper) when given the same tessellation of the same
/// solid. A box must be a valid, closed, oriented, manifold solid.
#[test]
#[allow(clippy::expect_used)]
fn manifold_report_mesh_agrees_with_solid_wrapper() {
    let mut model = BRepModel::new();
    let solid = build_box(&mut model);

    let chord = 0.05_f64;
    let weld_eps = 1e-6_f64;

    // Wrapper result (tessellates internally with TessellationParams::default()
    // at the given chord).
    let wrapper = manifold_report(&model, solid, chord, weld_eps).expect("wrapper");

    // Tessellate manually with the same params the wrapper uses.
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let solid_ref = model.solids.get(solid).expect("solid ref");
    let mesh = tessellate_solid(solid_ref, &model, &params);

    // Call the new mesh-core fn directly on the tessellated mesh.
    let core = manifold_report_mesh(&mesh, weld_eps).expect("mesh-core");

    // Every topological verdict must be identical.
    assert_eq!(core.closed, wrapper.closed, "closed");
    assert_eq!(core.manifold, wrapper.manifold, "manifold");
    assert_eq!(core.oriented, wrapper.oriented, "oriented");
    assert_eq!(
        core.boundary_edges, wrapper.boundary_edges,
        "boundary_edges"
    );
    assert_eq!(
        core.nonmanifold_edges, wrapper.nonmanifold_edges,
        "nonmanifold_edges"
    );
    assert_eq!(
        core.inconsistent_directed_edges, wrapper.inconsistent_directed_edges,
        "inconsistent_directed_edges"
    );
    assert_eq!(
        core.is_valid_solid(),
        wrapper.is_valid_solid(),
        "is_valid_solid"
    );

    // The box must be a valid solid according to both paths.
    assert!(core.is_valid_solid(), "box mesh must be a valid manifold");
}

/// RED → GREEN: `mesh_self_intersects_mesh` must agree with the existing
/// solid wrapper `mesh_self_intersects` when given the same tessellated mesh.
/// A clean sphere must report `false` from both.
#[test]
#[allow(clippy::expect_used, clippy::panic)]
fn mesh_self_intersects_mesh_agrees_with_solid_wrapper() {
    let mut model = BRepModel::new();
    let solid = {
        match TopologyBuilder::new(&mut model)
            .create_sphere_3d(Vector3::ZERO, 3.0)
            .expect("sphere")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid from create_sphere_3d, got {other:?}"),
        }
    };

    let chord = 0.5_f64;

    // Solid wrapper result (uses TessellationParams::audit() internally).
    let wrapper_result = mesh_self_intersects(&model, solid, chord);

    // Tessellate with the same audit() params the wrapper uses, then call the
    // mesh-core directly.
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::audit()
    };
    let solid_ref = model.solids.get(solid).expect("solid ref");
    let mesh = tessellate_solid(solid_ref, &model, &params);
    let core_result = mesh_self_intersects_mesh(&mesh);

    // Both must agree on the same mesh.
    assert_eq!(
        core_result, wrapper_result,
        "mesh-core and solid wrapper must agree"
    );

    // A clean sphere must not self-intersect.
    assert!(!core_result, "a clean sphere must not self-intersect");
}

/// RED → GREEN: `render_mesh` must rasterize a raw TriangleMesh to a
/// RenderFrame at the requested dimensions and emit a valid PNG byte stream.
#[test]
#[allow(clippy::expect_used)]
fn render_mesh_produces_valid_png_at_expected_dimensions() {
    let mut model = BRepModel::new();
    let solid = build_box(&mut model);

    let solid_ref = model.solids.get(solid).expect("solid ref");
    let mesh = tessellate_solid(solid_ref, &model, &TessellationParams::default());

    let opts = RenderOptions {
        width: 64,
        height: 64,
        view: CanonicalView::Isometric,
        mode: RenderMode::Shaded,
        tessellation: TessellationParams::default(),
    };

    let frame = render_mesh(&mesh, &opts).expect("render_mesh must return a frame");

    assert_eq!(frame.width, 64, "frame width");
    assert_eq!(frame.height, 64, "frame height");
    assert_eq!(frame.pixels.len(), 64 * 64 * 3, "pixel buffer byte count");

    let png = frame.to_png().expect("to_png");
    assert!(!png.is_empty(), "PNG must not be empty");
    // 8-byte PNG file signature (ISO/IEC 15948).
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");
}
