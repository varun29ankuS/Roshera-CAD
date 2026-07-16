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
//!
//! RESOLVED (2026-07-16, #46): the difference-side residual (flat bnd=302 +
//! offset-dependent nm, all at z≈20/z≈15) was NOT a cutter-wall clip problem —
//! on post-cap-merge topology the cutter wall's excess fragments were already
//! culled correctly by classification. Three BASE-side roots, each fixed at its
//! own altitude in `operations/boolean.rs`:
//!   1. `get_face_interior_point`: the boundary-midpoint-centroid → cylinder
//!      projection landed the ~301° boss-wall complement fragment's probe at
//!      θ=0, inside its own excluded in-bore sector → whole boss wall culled
//!      (the 300 z≈20 segs). Now verified via `cylinder_fragment_contains_point`
//!      on the unrolled (θ, z) chart and repaired with
//!      `curved_fragment_interior_point` only when off-fragment.
//!   2. clip-to-face: cut sub-arcs lying inside a PRE-EXISTING hole of the face
//!      (`is_point_in_face` sees only the outer loop) walked phantom material
//!      inside the plate-top annulus's r15 hole (the nm class). Now dropped by
//!      the 5-sample in-hole criterion (#27's test at arc level).
//!   3. `fragment_is_origin_hole_void` (selection): the composite cell bounded
//!      by the hole rim's long arcs + the material rim arc survived selection
//!      whenever its interior fell outside the cutter (offset ≥ 13). A fragment
//!      whose interior lies in its origin face's pre-existing hole with a
//!      non-boundary two-sided material signature is void. Root enabler: the
//!      shared `planar_face_hole_polygons` ignored loop ORIENTATIONS, so a
//!      3-arc rim scrambled into a self-crossing polygon and even-odd
//!      containment misfired — the exact pitfall `clip_circle_to_planar_face`
//!      documents.
//! The `_is_sound` pins are LIVE (offsets 10 and 12); the whole straddling band
//! offsets 8..=21.9 measures bnd=0 nm=0 euler=0 sound. Honest residual: a
//! NEAR-TANGENT straddle (offset 7.5, rim grazing the r15 seam at y≈±3.9)
//! still leaks (bnd=682) — the #86 near-tangency class, out of #46 scope.
//! History: `.superpowers/sdd/dogfood-diag-straddling-32.md`,
//! `dogfood-task-32{b,c}-report.md`.

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

// ─────────────── straddling pins (#32 Phase A → CLOSED by #46) ────────────────
//
// Historic signature (2026-07-08, pre Phase B): every straddling offset in
// (7, 22) broke with χ=−2 (offset 10: bnd=840 nm=586). Phase B's dedup
// restored χ=0 leaving a flat bnd=302 + offset-dependent nm. The #46 fixes
// (2026-07-16) close the class; the measured post-#46 band:
//
//   offset  sound  bnd  nm  euler
//     0     true    0    0   0   ← coaxial control (green above)
//     8..=21.9      0    0   0   ← whole straddling band SOUND (swept 8, 9, 10,
//                                  11, 12, 13, 14, 15, 16, 18, 20, 21, 21.9)
//     7.5   false  682   —   —   ← near-tangent graze of the r15 seam (y≈±3.9):
//                                  #86 near-tangency class, out of #46 scope
//
// Two offsets pinned live (mid-band 10 + 12) so a partial regression can't
// sneak through on one geometry; regenerate the table with
// `f7_straddling_sweep_signatures` / `ROSHERA_F7_OFFSET` when the core moves.

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
// `f7_union_bottom_is_one_seamless_face`). The z-histogram evidence (all 302
// boundary segs at z≈20/z≈15, NONE at z=0) refuted the union-seam diagnosis and
// re-localised the residual to the DIFFERENCE side.
//
// #46 (2026-07-16): CLOSED — both `_is_sound` pins are LIVE. See the module doc
// §RESOLVED for the three base-side roots (mis-located cylinder-fragment
// interior point; unclipped in-hole cut arcs; origin-hole void cell surviving
// selection, enabled by the orientation-scrambled hole polygon). The
// mutation-proof teeth are these two pins plus the two #46 structural pins
// (`_keeps_boss_lateral_wall`, `_no_phantom_face_in_plate_top_hole`).

#[test]
fn f7_straddling_offset_10_is_sound() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f7 straddling offset=10");
    assert!(
        sound,
        "f7: straddling bore (offset=10) must be sound — #46 base-side fixes \
         (chart interior point + in-hole arc clip + origin-hole void filter)"
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
        "f7: straddling bore (offset=12) must be sound — #46 base-side fixes \
         (chart interior point + in-hole arc clip + origin-hole void filter)"
    );
    assert_eq!(nm, 0, "f7 straddling@12: no non-manifold edges");
    assert_eq!(bnd, 0, "f7 straddling@12: watertight");
    assert_eq!(euler, 0, "f7 straddling@12: clean through-bore is genus-1");
}

/// #46 pin C: offset 14 exercises the SELECTION-level origin-hole void filter
/// specifically. At offsets ≤ 12 the composite hole-rim cell's interior falls
/// inside the bore and classification culls it by luck; from offset ≈ 13 the
/// interior escapes the bore, classifies Outside, and only
/// `fragment_is_origin_hole_void` (backed by the orientation-faithful
/// `planar_face_hole_polygons`) removes it. Mutating either regresses THIS pin
/// while offsets 10/12 stay green.
#[test]
fn f7_straddling_offset_14_is_sound() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 14.0);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f7 straddling offset=14");
    assert!(
        sound,
        "f7: straddling bore (offset=14) must be sound — #46 origin-hole void filter"
    );
    assert_eq!(nm, 0, "f7 straddling@14: no non-manifold edges");
    assert_eq!(bnd, 0, "f7 straddling@14: watertight");
    assert_eq!(euler, 0, "f7 straddling@14: clean through-bore is genus-1");
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

// ───────────── #46 structural pins: the two DIFFERENCE-side roots ─────────────
//
// A ROSHERA_BOOL_TRACE re-characterization on the post-cap-merge topology
// (2026-07-16) localized the flat bnd=302 / offset-dependent nm residual to two
// INDEPENDENT difference-side defects, both pinned here on the REAL fixture:
//
//  A. BOSS-LATERAL VANISHES (the 300 bnd segs at z≈20 + 2 at z≈15): the union
//     now correctly trims the boss lateral to z∈[10,20]; the difference splits
//     it into the in-bore sector (two seam-halves, correctly culled) plus the
//     BIG ~301° complement fragment. That fragment's interior point is computed
//     as boundary-edge-midpoint centroid → `closest_point` projection; the two
//     SSI-vertical midpoints at x=13.05 dominate the centroid (≈(1.5,0,15)), so
//     the projection lands at θ=0 → (15,0,15) — INSIDE the culled in-bore
//     sector. The whole remaining boss wall classifies Inside and is dropped,
//     leaving the boss-top crescent's outer r15 rim (300 segs) and the two
//     z∈[10,20] SSI verticals (2 segs) unpaired.
//
//  B. PHANTOM MATERIAL INSIDE THE PLATE-TOP HOLE (the nm class): the z=10 bore
//     rim (curve 37) is imprinted on the plate-top annulus UNCLIPPED. Its −x
//     sub-arcs lie inside the annulus's r15 hole — not on face material — and
//     the clip-to-face pass keeps them because `is_point_in_face` tests only
//     the OUTER loop. The arrangement then walks overlapping cells: a kept
//     fragment wholly inside the hole (interior point (12,8.4,10), r≈14.65<15)
//     plus inner loops triple-sharing the in-hole arc edges (the nm=248 class).
//
// Both tests are derived from the traced real topology and stay on the real
// fixture (minimal repro ≠ real topology).

/// #46 pin A: the difference result must still CARRY the boss lateral — at
/// least one kept face lies on the r15 cylinder about the origin. Before the
/// interior-point repair the big complement fragment is mis-culled and the
/// count is ZERO (all three r15 fragments classify Inside).
#[test]
fn f7_straddling_offset_10_keeps_boss_lateral_wall() {
    use geometry_engine::primitives::surface::Cylinder;
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let solid = m.solids.get(r).expect("difference solid");
    let shell = m.shells.get(solid.outer_shell).expect("outer shell");
    let mut boss_wall_faces = 0usize;
    for &fid in &shell.faces {
        let face = m.faces.get(fid).expect("face");
        let Some(surf) = m.surfaces.get(face.surface_id) else {
            continue;
        };
        if let Some(cyl) = surf.as_any().downcast_ref::<Cylinder>() {
            // The boss wall: r=15 about the world origin, axis ‖ Z. (The bore
            // wall is r=8 about (10,0,−3) — excluded by the radius test.)
            if (cyl.radius - 15.0).abs() < 1e-6
                && cyl.origin.x.abs() < 1e-6
                && cyl.origin.y.abs() < 1e-6
                && cyl.axis.z.abs() > 0.99
            {
                boss_wall_faces += 1;
            }
        }
    }
    assert!(
        boss_wall_faces >= 1,
        "difference must keep the boss lateral (r15 wall outside the bore); found 0 — \
         the ~301° complement fragment was mis-classified Inside (interior-point probe \
         landed in the culled in-bore sector) and the whole boss wall was dropped"
    );
}

/// #46 pin B: no kept z=10 planar face may lie ENTIRELY within the r15 boss
/// footprint. At z=10 the region inside r15 is the boss INTERIOR (material
/// continues above) — the plate-top annulus has a hole there, so any face whose
/// every boundary vertex sits at radius ≤ 15+ε is phantom material manufactured
/// from the unclipped in-hole bore-rim arcs. The legitimate annulus fragment
/// always reaches the 60×60 outer square (r≈42) and never trips this.
#[test]
fn f7_straddling_offset_10_no_phantom_face_in_plate_top_hole() {
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, 10.0);
    let solid = m.solids.get(r).expect("difference solid");
    let shell = m.shells.get(solid.outer_shell).expect("outer shell");
    let mut phantom: Vec<u32> = Vec::new();
    for &fid in &shell.faces {
        let face = m.faces.get(fid).expect("face");
        let mut all_z10 = true;
        let mut all_within_r15 = true;
        let mut any_vertex = false;
        for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let l = m.loops.get(lid).expect("loop");
            for &eid in &l.edges {
                let e = m.edges.get(eid).expect("edge");
                for vid in [e.start_vertex, e.end_vertex] {
                    let p = m.vertices.get_position(vid).expect("vertex");
                    any_vertex = true;
                    if (p[2] - 10.0).abs() > 1e-6 {
                        all_z10 = false;
                    }
                    if (p[0] * p[0] + p[1] * p[1]).sqrt() > 15.0 + 1e-4 {
                        all_within_r15 = false;
                    }
                }
            }
        }
        if any_vertex && all_z10 && all_within_r15 {
            phantom.push(fid);
        }
    }
    assert!(
        phantom.is_empty(),
        "found {} kept z=10 face(s) entirely inside the r15 boss footprint {:?} — \
         phantom material walked from the unclipped in-hole bore-rim arcs (the \
         clip-to-face pass must drop cut arcs lying inside a pre-existing hole)",
        phantom.len(),
        phantom,
    );
}

/// Offset for the on-demand diagnostics below: `ROSHERA_F7_OFFSET` (default 10).
fn diag_offset() -> f64 {
    std::env::var("ROSHERA_F7_OFFSET")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(10.0)
}

#[test]
#[ignore = "diagnostic: z-histogram of the difference leak (ROSHERA_F7_OFFSET)"]
fn f7_leak_zhistogram_offset_10() {
    use geometry_engine::harness::watertight::boundary_edge_positions;
    let mut m = BRepModel::new();
    let r = straddling_bore_result(&mut m, diag_offset());
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
#[ignore = "diagnostic: ROSHERA_BOOL_TRACE mechanism capture (ROSHERA_F7_OFFSET)"]
fn f7_trace_offset_10() {
    let mut m = BRepModel::new();
    let off = diag_offset();
    let r = straddling_bore_result(&mut m, off);
    let _ = metrics(&mut m, r, &format!("f7 trace offset={off}"));
}
