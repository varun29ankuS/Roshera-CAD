//! BOX∘BOX bbox over-inclusion (the #34/#80 robustness-ceiling class).
//!
//! Discovered by the tier-3 bbox-containment proptest
//! (`prop_tier3_difference_bbox_within_minuend`) during the BOOL #7 fire — a
//! PRE-EXISTING bug (it reproduces on code that predates that work). A box
//! difference `A ∖ B` must satisfy `bbox(A∖B) ⊆ bbox(A)` (subtracting cannot
//! grow the minuend), but for this near-degenerate config the result bbox
//! escapes A — the difference leaves slightly-oversized geometry, the classic
//! over-inclusion ceiling.
//!
//! Shrunk failing case (proptest), both boxes centred at the origin:
//!   A = 19.828064 × 19.814276 × 8.400629
//!   B =  2.000000 × 19.851825 × 8.402209   (B spans A in y,z; thin x-slab)
//! So A∖B should be two x-side pieces with bbox exactly A; instead it overflows.
//!
//! Pinned #[ignore] (fails today) — flip on when the box∘box over-inclusion
//! (#34/#80) is fixed. The randomized proptest remains the live discovery gate.

use geometry_engine::math::Tolerance;
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> geometry_engine::primitives::solid::SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("expected Solid, got {o:?}"),
    }
}

#[test]
#[ignore = "#34/#80 box∘box bbox over-inclusion — flip on when fixed"]
fn box_box_difference_bbox_within_minuend_3480() {
    let mut m = BRepModel::new();
    let a = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(
            19.828_064_281_809_663,
            19.814_275_748_722_6,
            8.400_628_664_805_039,
        )
        .expect("box A"));
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(2.0, 19.851_824_906_275_304, 8.402_209_037_835_2)
        .expect("box B"));
    let bbox_a = m.solid_world_bbox(a).expect("bbox A");
    let res = boolean_operation(
        &mut m,
        a,
        b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference must succeed");
    let bbox_r = m.solid_world_bbox(res).expect("bbox result");
    let eps = Tolerance::default().distance() * 10.0;
    let within = bbox_r.min.x >= bbox_a.min.x - eps
        && bbox_r.min.y >= bbox_a.min.y - eps
        && bbox_r.min.z >= bbox_a.min.z - eps
        && bbox_r.max.x <= bbox_a.max.x + eps
        && bbox_r.max.y <= bbox_a.max.y + eps
        && bbox_r.max.z <= bbox_a.max.z + eps;
    assert!(
        within,
        "bbox(A∖B) escapes bbox(A): result min={:?} max={:?} vs A min={:?} max={:?}",
        bbox_r.min, bbox_r.max, bbox_a.min, bbox_a.max
    );
}
