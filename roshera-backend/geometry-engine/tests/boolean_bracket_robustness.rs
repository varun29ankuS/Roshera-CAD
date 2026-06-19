//! BOOLEAN-ROBUSTNESS BRACKET CAMPAIGN, iteration 1 — boolean failures pinned
//! from a live bracket build. Each operand is individually watertight/sound; the
//! kernel CORRECTLY flagged the broken results (it did not ship them — the
//! ValidityCertificate caught every one). These tests pin the CURRENT state so
//! the defects are documented + reproducible, and flip to SOUND as each is fixed.
//!
//! Run: `cargo test -p geometry-engine --test boolean_bracket_robustness`.
//! Trace a case: `ROSHERA_BOOL_TRACE=1 cargo test ... -- --nocapture <name>`.
//!
//! ── FINDINGS (iteration 1, this worktree) ──────────────────────────────────
//!   F1 holed-operand union   — BROKEN: manifold=false, nm≈298, euler=-4.
//!                               Root: union over a corner overlap that contains
//!                               a bore hole — coplanar imprint + an inner loop
//!                               in the same overlap region. Deep.
//!   F2 curved union          — BROKEN: watertight=false, bnd≈269, euler=1.
//!                               Root: a cylinder LATERAL crossing two planar
//!                               walls — plane↔cylinder SSI/corefinement leaves
//!                               cut edges unshared (the deep #17 family).
//!   F3 partial-embed imprint  — NO LONGER REPRODUCES with box primitives. The
//!                               box∪box rectangular-imprint-on-a-face path is
//!                               robust in the current kernel (the #35 chord-
//!                               polygon fixes + coplanar imprint-merge cover the
//!                               protruding/sunk/straddling/corner pad configs).
//!                               Pinned as a SOUND guard, not a broken pin.
//!   F4 through-wall bore     — BROKEN: brep_valid=false, euler=4 (two shells).
//!                               Root (isolated): NOT the union seam and NOT the
//!                               axis orientation. The bore RADIUS (7) exceeds
//!                               the wall HALF-thickness (4), so the bore circle
//!                               pokes through the entry face's top/bottom edges
//!                               — a "circle crosses a boundary edge" BITE. The
//!                               entry face splits into BANDS, not disc+annulus,
//!                               so merge_same_origin_fragments forms NO inner
//!                               loop → entry/exit rims unwelded → 2 shells. A
//!                               bore that FITS the wall (probe_x_bore_fits_wall)
//!                               is SOUND, in either +X or +Z. The #54 bite family.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ───────────────────────── operand builders ─────────────────────────

fn box_at(m: &mut BRepModel, w: f64, h: f64, d: f64, tx: f64, ty: f64, tz: f64) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    if tx != 0.0 {
        translate(m, vec![s], Vector3::X, tx, TransformOptions::default()).expect("tx");
    }
    if ty != 0.0 {
        translate(m, vec![s], Vector3::Y, ty, TransformOptions::default()).expect("ty");
    }
    if tz != 0.0 {
        translate(m, vec![s], Vector3::Z, tz, TransformOptions::default()).expect("tz");
    }
    s
}

fn cylinder(m: &mut BRepModel, base: Point3, axis: Vector3, radius: f64, height: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, axis, radius, height)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn diff(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default())
        .expect("difference must complete")
}

fn union(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Union, BooleanOptions::default())
        .expect("union must complete")
}

/// (sound, boundary_edges, nonmanifold_edges, euler) snapshot of a result.
fn metrics(m: &mut BRepModel, s: SolidId, label: &str) -> (bool, usize, usize, i64) {
    let gt = m
        .ground_truth(s)
        .unwrap_or_else(|| panic!("{label}: no ground truth"));
    let mr = manifold_report(m, s, 0.05, 1.0e-5)
        .unwrap_or_else(|| panic!("{label}: no manifold report"));
    eprintln!(
        "[{label}] sound={} bnd={} nm={} euler={} | {}",
        gt.certificate.is_sound(),
        mr.boundary_edges,
        mr.nonmanifold_edges,
        mr.euler_characteristic,
        gt.summary()
    );
    (
        gt.certificate.is_sound(),
        mr.boundary_edges,
        mr.nonmanifold_edges,
        mr.euler_characteristic,
    )
}

fn assert_operand_sound(m: &mut BRepModel, s: SolidId, label: &str) {
    let gt = m
        .ground_truth(s)
        .unwrap_or_else(|| panic!("{label}: no ground truth"));
    assert!(
        gt.certificate.is_sound(),
        "{label}: operand must be individually sound — {}",
        gt.summary()
    );
}

fn l_bracket(m: &mut BRepModel) -> SolidId {
    let base = box_at(m, 40.0, 12.0, 8.0, 0.0, 0.0, 0.0);
    let upright = box_at(m, 12.0, 40.0, 8.0, -14.0, 14.0, 0.0);
    union(m, base, upright)
}

// ───────── #1 holed-operand union: plate_with_4_holes ∪ box ─────────
//
// A 40×40×4 plate with four Ø8 through-holes (bored on +Z), then UNION a solid
// box overlapping a corner. The holed operand A is sound; the box is sound; the
// union is non-manifold (nm≈298) — the overlap region contains a bore hole, so
// the coplanar imprint-merge has to deal with a face carrying an inner loop.
fn plate_with_4_holes(m: &mut BRepModel) -> SolidId {
    let plate = box_at(m, 40.0, 40.0, 4.0, 0.0, 0.0, 0.0);
    let mut a = plate;
    for (hx, hy) in [(-12.0, -12.0), (12.0, -12.0), (-12.0, 12.0), (12.0, 12.0)] {
        let bore = cylinder(m, Point3::new(hx, hy, -3.0), Vector3::Z, 4.0, 10.0);
        a = diff(m, a, bore);
    }
    a
}

#[test]
fn f1_holed_operand_union_is_broken() {
    let mut m = BRepModel::new();
    let plate = plate_with_4_holes(&mut m);
    assert_operand_sound(&mut m, plate, "f1 plate operand");
    // box overlapping a corner of the plate (interpenetrating, over a bore hole).
    let pad = box_at(&mut m, 10.0, 10.0, 10.0, 15.0, 15.0, 3.0);
    assert_operand_sound(&mut m, pad, "f1 pad operand");
    let r = union(&mut m, plate, pad);
    let (sound, _bnd, nm, _euler) = metrics(&mut m, r, "f1 holed-union");
    // FIXME(#F1): holed-operand union is non-manifold. The corner overlap region
    // contains a Ø8 bore hole; the coplanar imprint-merge over a face that
    // already carries an inner loop (the hole) fails to weld the rims.
    // (Ranked: deep — coplanar + inner loops + corner overlap combined.)
    assert!(!sound, "f1: expected BROKEN (kernel flagged it)");
    assert!(nm > 0, "f1: expected non-manifold edges");
}

// ───────── #2 curved union: cylinder lateral ∪ L-bracket ─────────
//
// A horizontal (axis +X) cylinder whose CURVED LATERAL crosses the L-bracket's
// base top (z=4) and the upright's inner wall — the curved↔planar SSI must be
// corefined into both planar faces. The deep #17 family.
#[test]
fn f2_curved_union_is_broken() {
    let mut m = BRepModel::new();
    let bracket = l_bracket(&mut m);
    assert_operand_sound(&mut m, bracket, "f2 bracket operand");
    let boss = cylinder(&mut m, Point3::new(-25.0, 8.0, 4.0), Vector3::X, 6.0, 30.0);
    assert_operand_sound(&mut m, boss, "f2 boss operand");
    let r = union(&mut m, bracket, boss);
    let (sound, bnd, _nm, _euler) = metrics(&mut m, r, "f2 curved-union");
    // FIXME(#F2 / #17): a cylinder lateral crossing two planar walls — the
    // plane↔cylinder SSI/corefinement leaves the cut edges unshared between
    // operands (boundary edges, watertight=false). The deep curved-corefinement
    // family — see memory `nurbs-corefinement-17`. (Ranked: deepest; needs a
    // focused human-guided session, NOT a blind fix.)
    assert!(!sound, "f2: expected BROKEN (kernel flagged it)");
    assert!(bnd > 0, "f2: expected mesh boundary edges (not watertight)");
}

// ───────── #3 partial-embed face-imprint: protruding pad ─────────
//
// A target box, UNION a smaller box that PARTIALLY embeds in the target's +Z
// face (sunk lower half + protruding upper half, footprint inside the face).
// In the current kernel this WELDS CLEANLY — the box∪box rectangular-imprint
// path is robust (the #35 chord-polygon fixes + coplanar imprint-merge). This
// pin is a SOUND guard: if it ever flips to broken, the #35 family regressed.
#[test]
fn f3_partial_embed_face_imprint_is_sound() {
    let mut m = BRepModel::new();
    let target = box_at(&mut m, 40.0, 40.0, 10.0, 0.0, 0.0, 0.0); // z∈[-5,5]
    assert_operand_sound(&mut m, target, "f3 target operand");
    // pad z∈[0,8] pierces the z=5 top face; footprint 12×12 fully inside.
    let pad = box_at(&mut m, 12.0, 12.0, 8.0, 0.0, 0.0, 4.0);
    assert_operand_sound(&mut m, pad, "f3 pad operand");
    let r = union(&mut m, target, pad);
    let (sound, _bnd, _nm, _euler) = metrics(&mut m, r, "f3 face-imprint");
    // GUARD: the prompt's #3 (4 NM imprint-not-healed) NO LONGER reproduces with
    // box primitives — the imprint-on-a-face path was fixed since the live build.
    assert!(sound, "f3: box∪box partial-embed imprint must stay sound");
}

// ───────── #4 through-wall bore (radius exceeds wall thickness) ─────────
//
// Bore an axis-+X Ø14 cylinder through the UPRIGHT wall of an L-bracket. The
// bore radius (7) exceeds the wall half-thickness (4), so the bore circle pokes
// through the entry face's top/bottom edges — a "circle crosses a boundary edge"
// BITE. The entry face splits into bands (not disc+annulus), no inner loop is
// formed, the entry/exit rims stay unwelded → two shells (euler=4).
#[test]
fn f4_through_wall_bore_is_broken() {
    let mut m = BRepModel::new();
    let bracket = l_bracket(&mut m);
    assert_operand_sound(&mut m, bracket, "f4 bracket operand");
    // upright wall: x∈[-20,-8], z∈[-4,4] (8 thick). bore r=7 > half-thickness 4.
    let bore = cylinder(&mut m, Point3::new(-25.0, 14.0, 0.0), Vector3::X, 7.0, 30.0);
    assert_operand_sound(&mut m, bore, "f4 bore operand");
    let r = diff(&mut m, bracket, bore);
    let (sound, _bnd, _nm, euler) = metrics(&mut m, r, "f4 through-bore");
    // FIXME(#F4 / #54): the bore circle exceeds the wall thickness → the cut
    // circle crosses the entry face's boundary edges → the face splits into
    // bands, not disc+annulus → no inner loop → unwelded rims → 2 shells. A
    // bore that FITS the wall is sound (probe_x_bore_fits_wall, either axis).
    // The "circle crosses a boundary edge" bite family — core arrangement.
    // (Ranked: moderate, but touches the shared face-arrangement path.)
    assert!(!sound, "f4: expected BROKEN (kernel flagged it)");
    assert_eq!(euler, 4, "f4: expected two-shell mesh (euler=4)");
}

// ───────── isolation controls (prove the F4 root cause + F3 robustness) ─────────

/// CONTROL: a +X bore that FITS within the wall thickness (r=3 in an 8-thick
/// wall) is a clean fully-enclosed through-hole → SOUND. This proves F4 is the
/// radius-exceeds-thickness BITE, not the +X orientation and not the union seam.
#[test]
fn probe_x_bore_fits_wall_is_sound() {
    let mut m = BRepModel::new();
    let slab = box_at(&mut m, 12.0, 40.0, 8.0, -14.0, 14.0, 0.0); // z∈[-4,4]
    let bore = cylinder(&mut m, Point3::new(-25.0, 14.0, 0.0), Vector3::X, 3.0, 30.0);
    let r = diff(&mut m, slab, bore);
    let (sound, _b, _n, euler) = metrics(&mut m, r, "probe x-bore r3 fits wall");
    assert!(sound, "x-bore that fits the wall must be sound");
    assert_eq!(euler, 0, "a clean through-hole is genus-1 (euler=0)");
}

/// CONTROL: the SAME oversized bore (r=7) through a thick-enough wall (16 tall)
/// is SOUND — confirming F4 is purely the radius-vs-thickness relationship.
#[test]
fn probe_x_bore_thick_wall_is_sound() {
    let mut m = BRepModel::new();
    let slab = box_at(&mut m, 12.0, 40.0, 16.0, -14.0, 14.0, 0.0); // z∈[-8,8]
    let bore = cylinder(&mut m, Point3::new(-25.0, 14.0, 0.0), Vector3::X, 7.0, 30.0);
    let r = diff(&mut m, slab, bore);
    let (sound, _b, _n, euler) = metrics(&mut m, r, "probe x-bore r7 thick wall");
    assert!(
        sound,
        "oversized bore through a thick-enough wall must be sound"
    );
    assert_eq!(euler, 0, "a clean through-hole is genus-1 (euler=0)");
}
