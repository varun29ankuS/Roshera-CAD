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
//!   F1 holed-operand union   — FIXED (was manifold=false, nm≈298, euler=-4).
//!                               Root: the union imprint outline (pad footprint)
//!                               CROSSED a pre-existing bore-hole inner loop on
//!                               the cap face; the DCEL walked the slice of the
//!                               bore's VOID that the imprint bounds into its own
//!                               fragment, which classified Outside and was kept,
//!                               re-using the bore-rim arcs a THIRD time (8 edges
//!                               shared by 3 faces). Fix: drop arrangement
//!                               fragments lying entirely inside a pre-existing
//!                               hole (the fragment-level analogue of the #27
//!                               void-cut filter), in `split_face_by_curves`.
//!                               Now SOUND. NOTE: the SEPARATE coincident-
//!                               coplanar-face overlap (#32) still bites when the
//!                               pad bottom is COINCIDENT with the cap AND crosses
//!                               a bore — pinned in `f1b_coincident_bottom_over_
//!                               bore_is_broken` as a distinct, deeper defect.
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
//!   F4 oversized bore        — NOT A BUG (retracted; traced+rendered+component-
//!                               counted 2026-06-20). The bore (r=7) cuts clean
//!                               through the 8-thick wall and SEVERS the upright
//!                               into TWO disconnected bodies. Geometry is CORRECT
//!                               (watertight, manifold, nm=0, 2 components, euler=4);
//!                               brep_valid=false is the kernel HONESTLY refusing to
//!                               emit two outer Solids from one difference (single-
//!                               SolidId output). Real follow-up = a MULTI-BODY
//!                               BOOLEAN OUTPUT feature (deliberate core campaign),
//!                               NOT an arrangement fix. Bore that doesn't sever =
//!                               sound (probe controls, either +X/+Z).

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
fn f1_holed_operand_union_is_sound() {
    let mut m = BRepModel::new();
    let plate = plate_with_4_holes(&mut m);
    assert_operand_sound(&mut m, plate, "f1 plate operand");
    // Box pad overlapping a corner of the plate, fully INTERPENETRATING (z∈[-5,8],
    // below the plate's z=-2 bottom so there is NO coincident bottom face — that
    // would be the separate #32 coincident-coplanar overlap, pinned below). The
    // pad's footprint (x,y ∈ [10,20]) CROSSES the Ø8 bore hole at (12,12): the
    // pad edges enter the bore at two points. This is the bug the fix targets —
    // the imprint outline crossing a pre-existing inner loop.
    let pad = box_at(&mut m, 10.0, 10.0, 13.0, 15.0, 15.0, 1.5); // z∈[-5,8]
    assert_operand_sound(&mut m, pad, "f1 pad operand");
    let r = union(&mut m, plate, pad);
    let (sound, bnd, nm, _euler) = metrics(&mut m, r, "f1 holed-union");
    // FIXED: the bore-void slice the pad imprint bounded is now dropped as a
    // void fragment in `split_face_by_curves`, so the bore-rim arcs are used by
    // exactly two faces (the cap and the bore wall) → manifold + watertight.
    assert!(
        sound,
        "f1: holed-operand union over a crossed bore must be sound"
    );
    assert_eq!(nm, 0, "f1: no non-manifold edges");
    assert_eq!(bnd, 0, "f1: watertight (no boundary edges)");
}

// ── #1b coincident-bottom-over-bore (SEPARATE #32 family, still broken) ──
//
// The SAME corner pad but with its bottom COINCIDENT with the plate bottom
// (pad z∈[-2,8]) AND crossing a bore. The void-fragment fix (F1) removes the
// bore-void slices, but the coincident-coplanar overlap of the pad bottom and
// the (notched) cap bottom is the #32 same-domain-cull family: the two
// coincident OnBoundary faces are NOT identical (the cap copy carries the bore
// notch, the pad copy is a full square), so the dedup leaves open rims
// (boundary edges, watertight=false). DISTINCT from F1's arrangement defect —
// it is the coincident-face overlap, a deeper core-weld problem. Pinned BROKEN.
#[test]
fn f1b_coincident_bottom_over_bore_is_broken() {
    let mut m = BRepModel::new();
    let plate = plate_with_4_holes(&mut m);
    assert_operand_sound(&mut m, plate, "f1b plate operand");
    // pad z∈[-2,8]: bottom coincident with the plate bottom AND footprint crosses
    // the (12,12) bore.
    let pad = box_at(&mut m, 10.0, 10.0, 10.0, 15.0, 15.0, 3.0);
    assert_operand_sound(&mut m, pad, "f1b pad operand");
    let r = union(&mut m, plate, pad);
    let (sound, bnd, _nm, _euler) = metrics(&mut m, r, "f1b coincident-bore-union");
    // The void-fragment fix already cleared the non-manifold edges (nm=0); the
    // RESIDUAL is the coincident-coplanar overlap (#32) leaving open boundary
    // edges. Honest pin: not watertight until the same-domain coincident-face
    // weld handles a notched-vs-full coincident pair.
    assert!(
        !sound,
        "f1b: coincident-bottom-over-bore still broken (#32)"
    );
    assert!(
        bnd > 0,
        "f1b: open boundary edges from the coincident overlap"
    );
}

// ───────── #2 curved union: cylinder lateral ∪ L-bracket ─────────
//
// A horizontal (axis +X) cylinder whose CURVED LATERAL crosses the L-bracket's
// base top (z=4) and the upright's inner wall — the curved↔planar SSI must be
// corefined into both planar faces.
//
// FIXED by #32 Phase B (per-face coincident-curve dedup, `split_faces_along_
// curves`). The cylinder lateral crossing two planar walls emitted the SAME cut
// curve TWICE onto each wall face (faces 27/28, curves 50 ≡ 55 dropped) — the
// duplicate-coincident-cut class. Those coincident cut edges were the
// "unshared" boundary edges that left the union open (baseline: sound=false,
// bnd=593). The dedup collapses each duplicate to one edge, and the weld now
// closes CLEANLY: sound=true, watertight, manifold, euler=2 (genus-0). This is
// a welcome generalisation of the straddling-rim fix into the #17 curved-
// corefinement family — NOT a fixture-shaped tweak. GUARD: if this ever flips
// back to broken, the #32 Phase B dedup regressed. (See
// `.superpowers/sdd/dogfood-task-32b-report.md`.)
#[test]
fn f2_curved_union_is_sound() {
    let mut m = BRepModel::new();
    let bracket = l_bracket(&mut m);
    assert_operand_sound(&mut m, bracket, "f2 bracket operand");
    let boss = cylinder(&mut m, Point3::new(-25.0, 8.0, 4.0), Vector3::X, 6.0, 30.0);
    assert_operand_sound(&mut m, boss, "f2 boss operand");
    let r = union(&mut m, bracket, boss);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f2 curved-union");
    assert!(
        sound,
        "f2: cylinder-lateral ∪ L-bracket must be sound (#32 Phase B duplicate-cut dedup)"
    );
    assert_eq!(
        bnd, 0,
        "f2: watertight — no unshared curved-corefinement edges"
    );
    assert_eq!(nm, 0, "f2: manifold — no duplicate coincident cut edges");
    assert_eq!(euler, 2, "f2: clean genus-0 union");
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

// ───────── #4 oversized bore SEVERS the wall into two bodies ─────────
//
// RETRACTED DIAGNOSIS (traced + rendered + connected-component-counted, 2026-06-20):
// this is NOT a "circle crosses a boundary edge" bite, and NOT a boolean defect.
// The bore (r=7) is so large it CUTS CLEAN THROUGH the 8-thick wall and severs the
// upright into TWO disconnected solid bodies (a chunk joined to the base + a floating
// bar). The difference geometry is CORRECT: result is watertight=true, manifold=true,
// nm=0, 2 connected components, euler=4 (= two genus-0 shells). The kernel flags
// brep_valid=false ONLY because reconstruct_topology emits ONE Solid (shells[0]) and
// files the rest as void/inner shells — it cannot represent "one difference produced
// two outer bodies". That is HONEST refusal of an unrepresentable result, not a bug.
// The real follow-up is a MULTI-BODY BOOLEAN OUTPUT feature (detect disjoint outer
// shells → emit multiple Solids) — a deliberate core-contract campaign (blast radius:
// boolean_operation's single SolidId return + timeline/export/render/validity), NOT a
// planar-arrangement tweak. A bore that does NOT sever (fits, or thicker wall) is sound
// — see the two controls below. This test documents the CORRECT 2-body outcome.
#[test]
fn f4_oversized_bore_severs_into_two_bodies() {
    let mut m = BRepModel::new();
    let bracket = l_bracket(&mut m);
    assert_operand_sound(&mut m, bracket, "f4 bracket operand");
    // upright wall: x∈[-20,-8], z∈[-4,4] (8 thick). bore r=7 severs it (cuts through).
    let bore = cylinder(&mut m, Point3::new(-25.0, 14.0, 0.0), Vector3::X, 7.0, 30.0);
    assert_operand_sound(&mut m, bore, "f4 bore operand");
    let r = diff(&mut m, bracket, bore);
    let (sound, _bnd, _nm, euler) = metrics(&mut m, r, "f4 severing bore");
    // The geometry is correct (2 watertight bodies); single-Solid output can't carry
    // them, so the certificate honestly reports !sound. NOT a defect to "fix" in the
    // arrangement — needs the multi-body-output feature. Pinned as the correct outcome.
    assert!(
        !sound,
        "f4: single-Solid output cannot represent a 2-body sever"
    );
    assert_eq!(euler, 4, "f4: two severed genus-0 bodies (euler=4)");
}

// ───────── isolation controls (prove the F4 root cause + F3 robustness) ─────────

/// CONTROL: a +X bore that FITS within the wall thickness (r=3 in an 8-thick
/// wall) is a clean fully-enclosed through-hole → SOUND. This proves F4 is the
/// bore SEVERING the wall (radius exceeds thickness → one body becomes two), not
/// the +X orientation and not the union seam.
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

// ───────── #5 taller-upstand coincident L-bracket (live dogfood #32) ─────────
//
// Live MCP dogfood (2026-06-27): the SIMPLEST functional L-bracket — a base
// plate with a TALLER upstand flush on its back edge — is non-manifold. Base
// [x:-12..12, y:-6..6, z:0..3] ∪ upstand [x:-12..12, y:3..6, z:0..15]. The
// upstand's back face (y=6) is COINCIDENT with the base's back (a SUBSET: base
// back z[0,3] ⊂ upstand back z[0,15]) and its bottom (z=0) coincident with the
// base bottom, while the upstand PIERCES the base top (z=3). Cert: nm=5
// (3-faces/edge) + 1 boundary gap, euler=3. The non-identical coincident-face
// overlap (#32) — distinct from the flat-L `l_bracket` (same z-extent) which is
// sound. Pinned to flip SOUND when the coincident-face weld is fixed.
#[test]
fn f5_taller_upstand_coincident_l_bracket_is_sound() {
    let mut m = BRepModel::new();
    let base = box_at(&mut m, 24.0, 12.0, 3.0, 0.0, 0.0, 1.5); // z∈[0,3]
    let upstand = box_at(&mut m, 24.0, 3.0, 15.0, 0.0, 4.5, 7.5); // y∈[3,6], z∈[0,15]
    assert_operand_sound(&mut m, base, "f5 base operand");
    assert_operand_sound(&mut m, upstand, "f5 upstand operand");
    let r = union(&mut m, base, upstand);
    let (sound, bnd, nm, _euler) = metrics(&mut m, r, "f5 taller-upstand");
    assert!(
        sound,
        "f5: taller-upstand coincident L-bracket must be sound"
    );
    assert_eq!(nm, 0, "f5: no non-manifold edges");
    assert_eq!(bnd, 0, "f5: watertight (no boundary gap)");
}

// ───────── #6 overlapping boss + coaxial bore (dogfood D-2, live parts 8–12) ─────────
//
// Plate 60×60×10 (z∈[0,10]) ∪ boss cylinder r15 h20 base z=0 — OVERLAPPING
// (interpenetrating z 0..10, not stacked). The union is SOUND, but its bottom
// z=0 is legally FRAGMENTED into two coplanar faces (square-with-ring + r15
// disk — the 2C coplanar-merge output). Then a coaxial bore r8 (z∈[-3,23])
// difference fails two ways on that input (diagnosis
// `.superpowers/sdd/dogfood-diag-api-blend.md` Bug 2):
//   (a) cutter SKIRT leak — the bore wall is split at z=10 (the plate-top
//       plane, which the bore never crosses: r8 < r15) but NOT at z=0
//       against the coplanar disk face it genuinely crosses, so the
//       out-of-solid piece z<0 is kept with its base-cap rim as boundary;
//   (b) chordized rim — the r8 imprint on the coplanar bottom disk face
//       comes out as straight polygon-clip CHORDS instead of the analytic
//       NOTE (D-2): defect (b) did NOT reproduce at HEAD — the rim already
//       imprinted as analytic Circle arcs even in the RED run; the live
//       chord observation was a stale-binary artifact. The pin below is a
//       permanent REGRESSION GUARD for the class, not a fix witness.
//       circle, so the planar rim can never pair with the cutter wall's
//       circular rim → unpaired rims, 3-face fans, χ=−34.
// The stacked-boss fixtures never see this (no overlap volume → no coplanar
// fragmented bottom → no cutter crossing a coincident face pair).

/// plate ∪ overlapping boss (both base z=0) — the 2C-fragmented-bottom input.
fn overlapping_boss_union(m: &mut BRepModel) -> SolidId {
    let plate = box_at(m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0); // z∈[0,10]
    let boss = cylinder(m, Point3::new(0.0, 0.0, 0.0), Vector3::Z, 15.0, 20.0); // z∈[0,20]
    assert_operand_sound(m, plate, "f6 plate operand");
    assert_operand_sound(m, boss, "f6 boss operand");
    let u = union(m, plate, boss);
    assert_operand_sound(m, u, "f6 union (coplanar-fragmented bottom)");
    u
}

/// Every edge of `sid` whose BOTH endpoints lie on the z≈0 plane at `radius`
/// from the z-axis (the bore-rim signature on the bottom face), as
/// `(edge_id, curve_kind, midpoint_radius)`. A true analytic rim edge is an
/// Arc/Circle whose midpoint also sits at `radius`; a polygon-clip CHORD is a
/// Line/Polyline whose endpoints touch the circle but whose midpoint sags
/// inside it.
fn bottom_rim_edges(m: &BRepModel, sid: SolidId, radius: f64) -> Vec<(u32, String, f64)> {
    use geometry_engine::primitives::curve::{Arc, Circle, Line, Polyline};
    let solid = m.solids.get(sid).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("outer shell");
    let mut edge_ids: Vec<u32> = Vec::new();
    for &fid in &shell.faces {
        let face = m.faces.get(fid).expect("face");
        for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let lp = m.loops.get(lid).expect("loop");
            for &eid in &lp.edges {
                if !edge_ids.contains(&eid) {
                    edge_ids.push(eid);
                }
            }
        }
    }
    let on_rim = |p: [f64; 3]| -> bool {
        p[2].abs() < 1e-6 && ((p[0] * p[0] + p[1] * p[1]).sqrt() - radius).abs() < 1e-6
    };
    let mut rim = Vec::new();
    for eid in edge_ids {
        let e = m.edges.get(eid).expect("edge");
        let sp = m
            .vertices
            .get_position(e.start_vertex)
            .expect("start vertex");
        let ep = m.vertices.get_position(e.end_vertex).expect("end vertex");
        if !(on_rim(sp) && on_rim(ep)) {
            continue;
        }
        let curve = m.curves.get(e.curve_id).expect("curve");
        let kind = if curve.as_any().downcast_ref::<Circle>().is_some() {
            "Circle"
        } else if curve.as_any().downcast_ref::<Arc>().is_some() {
            "Arc"
        } else if curve.as_any().downcast_ref::<Line>().is_some() {
            "Line"
        } else if curve.as_any().downcast_ref::<Polyline>().is_some() {
            "Polyline"
        } else {
            "Other"
        };
        let t_mid = 0.5 * (e.param_range.start + e.param_range.end);
        let mid = curve.point_at(t_mid).expect("curve midpoint");
        let mid_r = (mid.x * mid.x + mid.y * mid.y).sqrt();
        rim.push((eid, kind.to_string(), mid_r));
    }
    rim
}

#[test]
fn f6_overlapping_boss_coaxial_bore_is_sound() {
    let mut m = BRepModel::new();
    let u = overlapping_boss_union(&mut m);
    let bore = cylinder(&mut m, Point3::new(0.0, 0.0, -3.0), Vector3::Z, 8.0, 26.0); // z∈[-3,23]
    assert_operand_sound(&mut m, bore, "f6 bore operand");
    let r = diff(&mut m, u, bore);
    let (sound, bnd, nm, euler) = metrics(&mut m, r, "f6 overlapping-boss bore");
    assert!(
        sound,
        "f6: coaxial through-bore in an overlapping-boss union must be sound"
    );
    assert_eq!(nm, 0, "f6: no non-manifold edges");
    assert_eq!(bnd, 0, "f6: watertight (no boundary edges)");
    assert_eq!(euler, 0, "f6: one clean through-hole is genus-1 (euler=0)");
}

/// The no-chords pin: the bore rim imprinted on the (coplanar-fragmented)
/// bottom face must be ANALYTIC circular geometry — Arc/Circle edges whose
/// midpoints stay on the r8 circle — never straight polygon-clip chords.
#[test]
fn f6_bore_rim_on_bottom_face_is_analytic_circle() {
    let mut m = BRepModel::new();
    let u = overlapping_boss_union(&mut m);
    let bore = cylinder(&mut m, Point3::new(0.0, 0.0, -3.0), Vector3::Z, 8.0, 26.0);
    let r = diff(&mut m, u, bore);
    let rim = bottom_rim_edges(&m, r, 8.0);
    for (eid, kind, mid_r) in &rim {
        eprintln!("[f6 rim] edge={eid} kind={kind} mid_radius={mid_r:.4}");
    }
    assert!(
        !rim.is_empty(),
        "f6 rim: the bore must imprint a rim on the bottom z=0 face"
    );
    for (eid, kind, mid_r) in &rim {
        assert!(
            kind == "Arc" || kind == "Circle",
            "f6 rim: edge {eid} is a {kind} (chord fallback) — the r8 rim on the \
             bottom face must be an analytic circular edge"
        );
        assert!(
            (mid_r - 8.0).abs() < 1e-6,
            "f6 rim: edge {eid} midpoint radius {mid_r:.6} sags off the r8 circle \
             (chord, not arc)"
        );
    }
}

// Diagnostic: at what ε on the coincident back face does the union break? The
// MCP path anchors the upstand via a Matrix4; if that leaves its back face a
// hair off the base's (instead of bit-exact y=6), and the boolean is fragile to
// that, this prints the break threshold. eps=0 is the sound f5 case.
#[test]
fn f5_epsilon_sensitivity_of_coincident_face() {
    for eps in [0.0_f64, 1e-15, 1e-12, 1e-9, 1e-6, 1e-3] {
        let mut m = BRepModel::new();
        let base = box_at(&mut m, 24.0, 12.0, 3.0, 0.0, 0.0, 1.5);
        let upstand = box_at(&mut m, 24.0, 3.0, 15.0, 0.0, 4.5 + eps, 7.5);
        let _ = (base, upstand);
        let r = union(&mut m, base, upstand);
        let gt = m.ground_truth(r).expect("ground truth");
        let mr = manifold_report(&mut m, r, 0.05, 1.0e-5).expect("manifold report");
        eprintln!(
            "[eps={:>9.0e}] sound={} nm={} bnd={} euler={}",
            eps,
            gt.certificate.is_sound(),
            mr.nonmanifold_edges,
            mr.boundary_edges,
            mr.euler_characteristic,
        );
    }
}
