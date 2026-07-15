// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! AGENT-RENDER-α gate (NON-ignored): the software rasterizer produces
//! valid, deterministic, topology-labeled images.
//!
//! Invariants pinned:
//!  1. Shaded render of a boolean result is a valid non-trivial PNG.
//!  2. FaceIds render: the set of distinct foreground colors in the
//!     framebuffer is a subset of the legend, and the legend covers every
//!     face of the solid (set-of-marks completeness).
//!  3. Determinism: same solid + options → bit-identical framebuffer
//!     (renders are snapshot-diffable across kernel changes, like volumes).

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::render::{render_solid, CanonicalView, RenderMode, RenderOptions};

#[allow(clippy::expect_used, clippy::panic)] // test fixture
fn union_box_cylinder(model: &mut BRepModel) -> SolidId {
    let bx = {
        let mut b = TopologyBuilder::new(model);
        match b.create_box_3d(2.0, 2.0, 2.0).expect("box") {
            GeometryId::Solid(id) => id,
            o => panic!("box: {o:?}"),
        }
    };
    let cy = {
        let mut b = TopologyBuilder::new(model);
        match b
            .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 0.5, 1.0)
            .expect("cylinder")
        {
            GeometryId::Solid(id) => id,
            o => panic!("cyl: {o:?}"),
        }
    };
    boolean_operation(model, bx, cy, BooleanOp::Union, BooleanOptions::default()).expect("union")
}

#[test]
#[allow(clippy::expect_used)]
fn shaded_render_is_valid_png() {
    let mut model = BRepModel::new();
    let id = union_box_cylinder(&mut model);
    let frame = render_solid(
        &model,
        id,
        &RenderOptions {
            view: CanonicalView::Isometric,
            mode: RenderMode::Shaded,
            ..Default::default()
        },
    )
    .expect("render");
    let png = frame.to_png().expect("png encode");
    assert!(
        png.len() > 1024,
        "suspiciously small PNG ({} bytes)",
        png.len()
    );
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG magic");
    // Something was actually drawn: not all background.
    assert!(
        frame.pixels.chunks_exact(3).any(|p| p != [255, 255, 255]),
        "framebuffer is all background"
    );
}

#[test]
#[allow(clippy::expect_used)]
fn face_id_render_labels_topology() {
    let mut model = BRepModel::new();
    let id = union_box_cylinder(&mut model);
    let frame = render_solid(
        &model,
        id,
        &RenderOptions {
            view: CanonicalView::Isometric,
            mode: RenderMode::FaceIds,
            ..Default::default()
        },
    )
    .expect("render");

    // Legend covers every face of the solid.
    let solid = model.solids.get(id).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    assert_eq!(
        frame.face_legend.len(),
        shell.faces.len(),
        "legend must cover every face"
    );

    // Every foreground color in the framebuffer is a legend color
    // (exact match — flat mode means no shading corruption), and a
    // sensible number of faces are visible from the iso view.
    let legend: std::collections::BTreeSet<[u8; 3]> =
        frame.face_legend.iter().map(|&(_, c)| c).collect();
    let mut seen = std::collections::BTreeSet::new();
    for px in frame.pixels.chunks_exact(3) {
        if px != [255, 255, 255] {
            let c = [px[0], px[1], px[2]];
            assert!(legend.contains(&c), "non-legend foreground color {c:?}");
            seen.insert(c);
        }
    }
    assert!(
        seen.len() >= 3,
        "iso view of a box∪cylinder should show ≥3 faces, saw {}",
        seen.len()
    );
}

#[test]
#[allow(clippy::expect_used)]
fn depth_and_normal_channels_are_sane() {
    let mut model = BRepModel::new();
    let id = union_box_cylinder(&mut model);

    // Depth: grayscale, foreground within the documented 40..=220 band,
    // and an iso view of a 3D solid must show depth VARIATION.
    let depth = render_solid(
        &model,
        id,
        &RenderOptions {
            view: CanonicalView::Isometric,
            mode: RenderMode::Depth,
            ..Default::default()
        },
    )
    .expect("depth render");
    let mut depth_values = std::collections::BTreeSet::new();
    for px in depth.pixels.chunks_exact(3) {
        if px != [255, 255, 255] {
            assert!(
                px[0] == px[1] && px[1] == px[2],
                "depth pixel not grayscale: {px:?}"
            );
            assert!(
                (40..=220).contains(&px[0]),
                "depth value {} outside documented band",
                px[0]
            );
            depth_values.insert(px[0]);
        }
    }
    assert!(
        depth_values.len() >= 8,
        "iso depth map of a 3D solid should vary, saw {} levels",
        depth_values.len()
    );

    // Normals: iso view of a box∪cylinder shows ≥3 distinct surface
    // orientations as distinct RGB encodings.
    let normals = render_solid(
        &model,
        id,
        &RenderOptions {
            view: CanonicalView::Isometric,
            mode: RenderMode::Normals,
            ..Default::default()
        },
    )
    .expect("normals render");
    let mut normal_colors = std::collections::BTreeSet::new();
    for px in normals.pixels.chunks_exact(3) {
        if px != [255, 255, 255] {
            normal_colors.insert([px[0], px[1], px[2]]);
        }
    }
    assert!(
        normal_colors.len() >= 3,
        "iso normal map should show ≥3 orientations, saw {}",
        normal_colors.len()
    );
}

#[test]
#[allow(clippy::expect_used)]
fn render_is_deterministic() {
    let mk = || {
        let mut model = BRepModel::new();
        let id = union_box_cylinder(&mut model);
        render_solid(
            &model,
            id,
            &RenderOptions {
                view: CanonicalView::Front,
                mode: RenderMode::FaceIds,
                ..Default::default()
            },
        )
        .expect("render")
    };
    let a = mk();
    let b = mk();
    assert_eq!(a.pixels, b.pixels, "render must be bit-deterministic");
    assert_eq!(a.face_legend, b.face_legend, "legend must be deterministic");
}
