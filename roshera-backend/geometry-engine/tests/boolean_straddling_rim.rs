// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
//! CORRECTION (2026-07-15, #32 same-domain-unify slice): the union-side
//! coincident-coplanar cap MERGE landed, so the union no longer produces the
//! z=0 ring/disk seam described above — it emits ONE seamless 60×60 bottom
//! (`f7_union_bottom_is_one_seamless_face`, GREEN, pins the production merge).
//! The straddling `_is_sound` pins stay `#[ignore]`d: their residual is NOT the
//! z=0 seam (removing it moved bnd/nm by ZERO; the 302 boundary segs sit at
//! z≈20/z≈15 — the boss TOP exit + lateral scallop — see
//! `f7_leak_zhistogram_offset_10`), but a DIFFERENCE-side straddle of the boss's
//! own real boundaries. They flip green when the #32 difference-side cutter-wall
//! arc-clip lands. See `.superpowers/sdd/dogfood-diag-straddling-32.md` and the
//! `.superpowers/sdd/dogfood-task-32c-report.md`.

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

// ───────────── structural RED/GREEN: seamless coincident-coplanar bottom ───────
//
// The overlapping-boss union's z=0 bottom is ONE 60×60 face. Before the #32
// same-domain-unify coincident-coplanar cap merge, the union fragments it into a
// ring (`square − r15`, an `Outside` face carrying an r15 inner loop) PLUS a
// coincident r15 disk (`OnBoundary` from A) — two z=0 −z faces whose shared r15
// seam arcs are what a later straddling bore cannot cut cleanly. This pins the
// merged result directly (independent of the difference), so a partial fix that
// leaves the fragmentation in place cannot pass.

/// Count of the union result's bottom faces (every outer-loop vertex at z≈0) and
/// the edge ids on the r15 seam (both endpoints at radius 15 on z=0).
fn bottom_face_and_seam_report(m: &BRepModel, sid: SolidId) -> (usize, Vec<u32>) {
    let solid = m.solids.get(sid).expect("union solid");
    let shell = m.shells.get(solid.outer_shell).expect("outer shell");
    let mut bottom_faces = 0usize;
    let mut seam_edges: Vec<u32> = Vec::new();
    let on_seam = |p: [f64; 3]| -> bool {
        p[2].abs() < 1e-6 && ((p[0] * p[0] + p[1] * p[1]).sqrt() - 15.0).abs() < 1e-4
    };
    for &fid in &shell.faces {
        let face = m.faces.get(fid).expect("face");
        let lp = m.loops.get(face.outer_loop).expect("outer loop");
        let mut all_z0 = !lp.edges.is_empty();
        for &eid in &lp.edges {
            let e = m.edges.get(eid).expect("edge");
            let sp = m.vertices.get_position(e.start_vertex).expect("sv");
            let ep = m.vertices.get_position(e.end_vertex).expect("ev");
            if sp[2].abs() >= 1e-6 || ep[2].abs() >= 1e-6 {
                all_z0 = false;
                break;
            }
        }
        if all_z0 {
            bottom_faces += 1;
        }
        // Seam edges anywhere on the face (outer + inner loops).
        for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let l = m.loops.get(lid).expect("loop");
            for &eid in &l.edges {
                let e = m.edges.get(eid).expect("edge");
                let sp = m.vertices.get_position(e.start_vertex).expect("sv");
                let ep = m.vertices.get_position(e.end_vertex).expect("ev");
                if on_seam(sp) && on_seam(ep) && !seam_edges.contains(&eid) {
                    seam_edges.push(eid);
                }
            }
        }
    }
    (bottom_faces, seam_edges)
}

/// #32 RED A (structural): the overlapping-boss union's z=0 bottom is exactly
/// ONE face and carries NO r15 coplanar seam. Fails before the merge (ring +
/// disk survive, 16 r15 seam arcs present).
#[test]
fn f7_union_bottom_is_one_seamless_face() {
    let mut m = BRepModel::new();
    let u = overlapping_boss_union(&mut m);
    let (bottom_faces, seam_edges) = bottom_face_and_seam_report(&m, u);
    assert!(
        seam_edges.is_empty(),
        "union bottom must carry NO r15 coplanar seam — found {} seam edge(s) {:?} \
         (ring/disk fragmentation not merged)",
        seam_edges.len(),
        seam_edges,
    );
    assert_eq!(
        bottom_faces, 1,
        "union bottom must be exactly ONE 60x60 face, found {bottom_faces} (ring + disk)"
    );
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
// result manifold-broken (nm/bnd > 0) though genus-correct.
//
// SLICE-2 UPDATE (2026-07-15): the union-side coincident-coplanar cap MERGE
// (`coincident_coplanar_cap_merge`, `operations/boolean.rs`) LANDED — the union
// no longer fragments the coincident-coplanar z=0 bottom into ring + disk; it
// emits ONE seamless 60×60 bottom (pinned GREEN by
// `f7_union_bottom_is_one_seamless_face`). But the earlier claim that this z=0
// seam was the ROOT of the difference's unsoundness is REFUTED by live evidence:
// the difference residual is BYTE-IDENTICAL before and after the merge
// (offset 10: bnd=302 nm=248 euler=0; offset 12: bnd=302 nm=198 euler=0). A
// change that removes the entire z=0 seam yet moves nm/bnd by ZERO cannot be the
// cause of nm/bnd. A z-histogram of the 302 boundary segments
// (`f7_leak_zhistogram_offset_10`) localises them at **z≈20 (300 segs, the boss
// TOP)** + z≈15 (2), with **NONE at z=0**. The real residual is the bore rim
// STRADDLING the boss's own emergence/exit boundaries: at z=20 the bore exits the
// r15 boss-top cap (its +x arc past r15 is above void, no material to close
// against → open); at z=10 the plate-top annulus (r15 boss-emergence hole) is
// straddled, yielding self-overlapping inner loops (the nm class). Both are
// REAL feature boundaries of the union — not spurious coincident fragments — so
// this is a DIFFERENCE-side z-dependent cutter-wall material-extent problem
// (exactly the `clip_circle_to_planar_face`-can't-do-it boundary the 32c report
// §"Why it does NOT close" named), NOT anything the union cap-merge can reach.
// These two pins therefore STAY `#[ignore]`d; the merge is a correct, orthogonal
// union-side improvement. See the `.superpowers/sdd/dogfood-task-32c-report.md`.

#[test]
#[ignore = "#32 union coincident-coplanar cap merge LANDED (z=0 bottom now one \
            seamless face, pinned by f7_union_bottom_is_one_seamless_face) but it \
            is ORTHOGONAL to this pin: the difference residual is byte-identical \
            pre/post merge (bnd=302 nm=248 euler=0) and a z-histogram \
            (f7_leak_zhistogram_offset_10) puts all 302 boundary segs at z≈20 (boss \
            TOP) + z≈15, NONE at z=0. Real root = the bore straddling the boss's \
            REAL top-cap exit (z=20) and plate-top annulus (z=10) — a \
            DIFFERENCE-side z-dependent cutter-wall material-extent clip, not the \
            union seam the prior diagnosis blamed. Stays blocked on the #32 \
            difference-side arc-clip (32c report §Precise gap)"]
fn f7_straddling_offset_10_is_sound() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f7 straddling offset=10");
    assert!(
        sound,
        "f7: straddling bore (offset=10) must be sound — #32 union coincident-coplanar cap merge"
    );
    assert_eq!(nm, 0, "f7 straddling@10: no non-manifold edges");
    assert_eq!(bnd, 0, "f7 straddling@10: watertight");
    assert_eq!(euler, 0, "f7 straddling@10: clean through-bore is genus-1");
}

#[test]
#[ignore = "#32 union coincident-coplanar cap merge LANDED (z=0 bottom seamless) \
            but ORTHOGONAL: difference residual byte-identical pre/post merge \
            (bnd=302 nm=198 euler=0), leak localised at z≈20 (boss TOP) / z≈15, \
            NONE at z=0. Real root = bore straddling the boss's real top-cap exit \
            + plate-top annulus — DIFFERENCE-side z-dependent cutter-wall \
            arc-clip, not the union seam. See f7_leak_zhistogram_offset_10 + \
            `.superpowers/sdd/dogfood-task-32c-report.md`"]
fn f7_straddling_offset_12_is_sound() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 12.0);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f7 straddling offset=12");
    assert!(
        sound,
        "f7: straddling bore (offset=12) must be sound — #32 union coincident-coplanar cap merge"
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

#[test]
#[ignore = "diagnostic: z-histogram of the difference leak"]
fn f7_leak_zhistogram_offset_10() {
    use geometry_engine::harness::watertight::boundary_edge_positions;
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let segs = boundary_edge_positions(&m, r, 0.05, 1.0e-5);
    let mut buckets: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
    for s in &segs {
        let zmid = ((s[0].z + s[1].z) * 0.5 * 2.0).round() as i64; // 0.5mm buckets
        *buckets.entry(zmid).or_default() += 1;
    }
    eprintln!("total boundary segs={}", segs.len());
    for (zb, c) in &buckets {
        eprintln!("  z≈{:.1}  count={}", *zb as f64 / 2.0, c);
    }
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
