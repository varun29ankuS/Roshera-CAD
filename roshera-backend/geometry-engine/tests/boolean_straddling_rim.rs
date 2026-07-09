//! #32 Phase A — STRADDLING-RIM duplicate cuts (the D-2 banked residual).
//!
//! The D-2 pair-level void-curve filter (`drop_pair_curves_in_preexisting_holes`,
//! `operations/boolean.rs`) drops a bore-rim meet curve only when it lies WHOLLY
//! inside a pre-existing hole of one of the two trimmed faces (the COAXIAL case,
//! `f6` / `f7_coaxial_control_is_sound` — now sound). When the bore axis is
//! OFFSET so its rim CROSSES the ring/disk coplanar seam of an overlapping-boss
//! union's fragmented bottom, neither bottom-face pair's hole wholly contains the
//! r8 circle → BOTH the ring-face pair (face 9 × bore wall) and the disk-face
//! pair (face 10 × bore wall) emit the FULL r8 circle onto the bore's lateral
//! face → the cutter receives DUPLICATE coincident z=0 cuts (curves 36 ≡ 37),
//! the same 3-face-fan / χ<0 corruption class as the coaxial variant, silent.
//! This is same-domain-unify territory (#32 / boolean-arch campaign).
//!
//! Run:   `cargo test -p geometry-engine --test boolean_straddling_rim`
//! Trace: `ROSHERA_BOOL_TRACE=1 cargo test ... -- --ignored --nocapture f7_trace_offset_10`
//!
//! DIAGNOSIS-ONLY (no production source touched). The straddling cases are
//! `#[ignore]` pins asserting the HONEST target (certified sound); they flip
//! green when #32 Phase B lands. See
//! `.superpowers/sdd/dogfood-diag-straddling-32.md`.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ───────────────────────── operand builders (house style) ─────────────────────

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

/// plate 60×60×10 (z∈[0,10]) ∪ OVERLAPPING boss cyl r15 h20 base z=0 — the
/// 2C-fragmented-bottom input (square-with-ring + coincident r15 disk on z=0).
/// Identical to the `f6` builder in `boolean_bracket_robustness.rs`.
fn overlapping_boss_union(m: &mut BRepModel) -> SolidId {
    let plate = box_at(m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0); // z∈[0,10]
    let boss = cylinder(m, Point3::new(0.0, 0.0, 0.0), Vector3::Z, 15.0, 20.0); // z∈[0,20]
    assert_operand_sound(m, plate, "straddling plate operand");
    assert_operand_sound(m, boss, "straddling boss operand");
    let u = union(m, plate, boss);
    assert_operand_sound(m, u, "straddling union (coplanar-fragmented bottom)");
    u
}

/// Subtract a bore r8 h26 (z∈[−3,23], fully through) whose axis is at
/// `(offset,0)`: its rim on z=0 CROSSES the r15 seam when `15−8 < offset < 15+8`
/// (i.e. `7 < offset < 23`), and the bore stays inside the 60×60 plate when
/// `offset+8 < 30`. At offset=0 the rim is COAXIAL (D-2 territory, sound).
fn straddling_bore_result(m: &mut BRepModel, offset: f64) -> SolidId {
    let u = overlapping_boss_union(m);
    let bore = cylinder(m, Point3::new(offset, 0.0, -3.0), Vector3::Z, 8.0, 26.0);
    assert_operand_sound(m, bore, "straddling bore operand");
    diff(m, u, bore)
}

// ───────────────────────── green control (D-2 fix holds) ──────────────────────

/// COAXIAL control (offset=0): the bore rim is concentric with the r15 seam, so
/// its meet curve lies WHOLLY inside the ring face's hole and the D-2 pair-void
/// filter drops it → sound. Proves the builder + harness are correct and that
/// the straddling breakage below is the OFFSET, not the overlapping-boss input.
#[test]
fn f7_coaxial_control_is_sound() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 0.0);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f7 coaxial control");
    assert!(sound, "f7: coaxial bore (D-2 fixed) must be sound");
    assert_eq!(nm, 0, "f7 coaxial: no non-manifold edges");
    assert_eq!(bnd, 0, "f7 coaxial: watertight");
    assert_eq!(euler, 0, "f7 coaxial: clean through-bore is genus-1");
}

// ───────────────────────── straddling pins (#32 Phase A) ──────────────────────
//
// Every straddling offset in (7, 22) breaks: the z=0 bore rim is emitted TWICE
// onto the cutter wall (curves 36 ≡ 37, from the two coincident coplanar bottom
// faces 9 and 10), plus single-copy z=10/z=20 straddle-phantom arcs whose in-
// boss halves imprint where no face exists. Signature (captured 2026-07-08):
//
//   offset  sound  bnd  nm   euler
//     0     true    0    0    0     ← coaxial control (green above)
//     9     false  838  614  -2
//    10     false  840  586  -2
//    11     false  824  544  -2
//    12     false  800  496  -2
//    14     false  762  420  -2
//
// bnd/nm shrink as the offset grows (less of the r8 rim sits inside r15 → fewer
// phantom edges); euler is a stable −2 across the band. The HONEST target is a
// sound genus-1 through-bore, asserted below; the pins flip green when Phase B
// lands. Two offsets pinned (mid-band + near-edge) so a partial fix can't sneak
// through on one geometry.

// #32 Phase B lands the per-target-face coincident-curve DEDUP: the straddling
// z=0 rim circle, routed twice onto the shared cutter wall (curves 36 ≡ 37 from
// the two coincident coplanar bottom fragments), collapses to one edge. That
// removes the DOMINANT corruption — the duplicate-coincident-ring / 3-face-fan
// class — flipping the Euler characteristic from the χ=−2 straddle signature
// back to χ=0 (genus correct). This is pinned, GREEN, by the
// `f7_straddling_offset_*_no_duplicate_fans` witnesses below (mutation-proof:
// disabling the dedup regresses euler to −2).
//
// Phase B is NECESSARY but NOT SUFFICIENT for full soundness. Two SINGLE-COPY
// straddle-phantom arcs survive dedup (each emitted once): curve 38 (z=10 rim,
// its −x arc imprints inside the r15 boss where no plate-top face exists) and
// curve 41 (z=20 rim, its +x arc imprints above the plate). They leave the
// result manifold-broken (nm/bnd > 0) though genus-correct. Killing them needs
// clipping each transverse rim to its SOURCE face's MATERIAL extent (outer
// boundary minus pre-existing holes) — which requires reworking the shared
// analytic circle-clip core (`clip_circle_to_planar_face` clips only against a
// face's LINE-bounded OUTER loop, never subtracting arc-bounded holes) and
// relies on DCEL welding of the resulting OPEN arcs against the SSI verticals.
// That is Phase C (Same-Domain unify) territory, escalated to Varun; see
// `.superpowers/sdd/dogfood-task-32b-report.md`. These two pins assert the
// honest END target (fully certified sound) and stay ignored until Phase C.

#[test]
fn f7_straddling_offset_10_is_sound() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f7 straddling offset=10");
    assert!(
        sound,
        "f7: straddling bore (offset=10) must be sound — #32 Phase C material-extent arc-clip"
    );
    assert_eq!(nm, 0, "f7 straddling@10: no non-manifold edges");
    assert_eq!(bnd, 0, "f7 straddling@10: watertight");
    assert_eq!(euler, 0, "f7 straddling@10: clean through-bore is genus-1");
}

#[test]
fn f7_straddling_offset_12_is_sound() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 12.0);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f7 straddling offset=12");
    assert!(
        sound,
        "f7: straddling bore (offset=12) must be sound — #32 Phase C material-extent arc-clip"
    );
    assert_eq!(nm, 0, "f7 straddling@12: no non-manifold edges");
    assert_eq!(bnd, 0, "f7 straddling@12: watertight");
    assert_eq!(euler, 0, "f7 straddling@12: clean through-bore is genus-1");
}

// ─────────────── Phase B witnesses: duplicate-fan class is closed ──────────────
//
// The DEDUP's durable, mutation-proof teeth. Before Phase B, a straddling bore
// receives the z=0 rim circle TWICE on the cutter wall (curves 36 ≡ 37), whose
// two coincident cut rings build a 3-face fan carrying spurious genus: χ = −2
// (captured pre-fix: offset 10 → bnd=840 nm=586 euler=−2; offset 12 → bnd=800
// nm=496 euler=−2). Phase B collapses the coincident pair to one edge, removing
// the fan and restoring χ = 0. These pins assert exactly that Euler flip — the
// class Phase B provably closes — independent of the residual single-copy
// phantom arcs (which keep the result not-yet-watertight, tracked by the
// ignored `_is_sound` pins above). Disabling the dedup regresses euler to −2 and
// fails these (mutation evidence in the 32b report).

#[test]
fn f7_straddling_offset_10_no_duplicate_fans() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let (_sound, _bnd, _nm, euler) = metrics(&mut m, r, "f7 no-dup-fans offset=10");
    assert_eq!(
        euler, 0,
        "f7 straddling@10: #32 Phase B dedup must remove the duplicate-coincident-ring \
         fan (euler −2 → 0); a non-zero euler means the coincident z=0 cut was not culled"
    );
}

#[test]
fn f7_straddling_offset_12_no_duplicate_fans() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 12.0);
    let (_sound, _bnd, _nm, euler) = metrics(&mut m, r, "f7 no-dup-fans offset=12");
    assert_eq!(
        euler, 0,
        "f7 straddling@12: #32 Phase B dedup must remove the duplicate-coincident-ring \
         fan (euler −2 → 0); a non-zero euler means the coincident z=0 cut was not culled"
    );
}

// ───────────────────────── diagnostics (on-demand) ────────────────────────────

/// Regenerates the offset→signature table. `#[ignore]` (6 booleans, ~35 s) —
/// run with `--ignored --nocapture` when the mechanism or fix moves.
#[test]
#[ignore = "diagnostic: regenerates the offset→cert-signature table"]
fn f7_straddling_sweep_signatures() {
    for offset in [0.0_f64, 9.0, 10.0, 11.0, 12.0, 14.0] {
        let mut m = BRepModel::new();
        let r = straddling_bore_result(&mut m, offset);
        let (sound, bnd, nm, euler) = metrics(&mut m, r, &format!("f7 offset={offset}"));
        eprintln!(">>> offset={offset} sound={sound} bnd={bnd} nm={nm} euler={euler}");
    }
}

/// Single-offset target for `ROSHERA_BOOL_TRACE=1` mechanism capture (the
/// duplicate z=0 circle onto the bore lateral). `#[ignore]` — run with
/// `--ignored --nocapture` under the env var.
#[test]
#[ignore = "diagnostic: ROSHERA_BOOL_TRACE mechanism capture (curves 36 ≡ 37)"]
fn f7_trace_offset_10() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let _ = metrics(&mut m, r, "f7 trace offset=10");
}
