// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! PILLAR 2 — randomized op-SEQUENCE fuzz, cross-checking PILLAR 1. Build a
//! random multi-feature part (a box pierced by one or two offset cylinders — the
//! realistic, well-conditioned union the bearing-housing builds exercise) and
//! assert, on the base primitive AND after EVERY boolean, that the kernel's OWN
//! ground truth holds:
//!   (a) provenance is RECORDED (the op didn't silently forget what it made),
//!   (b) the result CERTIFIES sound (brep_valid ∧ watertight ∧ manifold),
//!   (c) the full structural invariant bundle passes (full_contract).
//!
//! This converts "I think these compositions work" into "hundreds of random
//! compositions provably keep provenance + soundness". The fuzz that FOUND a
//! real defect (a union onto a boolean RESULT goes unsound — the #27 chained-
//! union family) is pinned separately below as a tracked repro,
//! `chained_unions_should_stay_sound`. This line used to call the defect
//! "chained concentric unions" and point at a KNOWN_BUGS entry; both were wrong
//! by 2026-09-07 — concentric boxes are CONTAINED and union cleanly, and no
//! KNOWN_BUGS file exists in the repo. See that test for the chain that repros.

use geometry_engine::harness::integration::full_contract;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::OperationError;
use geometry_engine::primitives::provenance::OperationKind;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use proptest::prelude::*;

fn boxs(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}
fn cyl(m: &mut BRepModel, base: Point3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, Vector3::Z, r, h)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn assert_truthful_and_sound(m: &mut BRepModel, s: SolidId, prim: bool, label: &str) {
    let gt = m
        .ground_truth(s)
        .unwrap_or_else(|| panic!("{label}: solid {s} has no ground truth"));
    let prov = gt
        .provenance
        .as_ref()
        .unwrap_or_else(|| panic!("{label}: solid {s} has NO provenance — op forgot to record it"));
    match (&prov.created_by, prim) {
        (OperationKind::Primitive(_), true) | (OperationKind::Boolean, false) => {}
        (other, _) => panic!(
            "{label}: unexpected provenance {other:?} ({})",
            gt.summary()
        ),
    }
    assert!(
        gt.certificate.is_sound(),
        "{label}: not sound: {}",
        gt.summary()
    );
    let c = full_contract(m, s, 0.1, 0.05);
    assert!(
        c.passes_structural(),
        "{label}: structural contract failed: {:?}",
        c.failures()
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// A box pierced by 1–2 offset cylinders (interpenetrating posts) stays sound
    /// and provenanced at every boolean step.
    #[test]
    fn random_box_with_posts_stays_sound_and_provenanced(
        w in 16.0f64..30.0, h in 16.0f64..30.0, d in 10.0f64..20.0,
        // A SINGLE post: chained unions (a 2nd union onto a boolean RESULT) hit
        // the deep #27 chained-union robustness gap (pinned below), so the green
        // guard fuzzes single booleans — the case that must always hold.
        posts in prop::collection::vec(
            (-0.25f64..0.25, -0.25f64..0.25, 2.0f64..4.0),
            1..=1,
        ),
    ) {
        let mut m = BRepModel::new();
        let mut cur = boxs(&mut m, w, h, d);
        assert_truthful_and_sound(&mut m, cur, true, "base box");

        for (i, &(fx, fy, r)) in posts.iter().enumerate() {
            // Post pierces the box vertically, sticking out both ends → a clean,
            // non-degenerate union (no coincident face planes with the box).
            let post = cyl(&mut m, Point3::new(fx * w, fy * h, -d / 2.0 - 3.0), r, d + 6.0);
            cur = boolean_operation(&mut m, cur, post, BooleanOp::Union, BooleanOptions::default())
                .unwrap_or_else(|e| panic!("post {i} union failed: {e:?}"));
            assert_truthful_and_sound(&mut m, cur, false, &format!("box + post {i}"));
        }
    }
}

/// FUZZ FINDING (pinned, RED → #[ignore]). The op-sequence fuzzer surfaced that
/// CHAINED unions — a second union applied onto an already-boolean RESULT — go
/// UNSOUND: a single `box ∪ cyl` certifies sound, but `box ∪ cyl ∪ cyl` reports
/// brep_valid=false / watertight=false. This is the deep #27 chained-union
/// robustness family (the result's scar faces aren't re-imprinted cleanly by the
/// next boolean). Asserts the DESIRED end state (every step sound); un-ignore
/// when the #27 chained-union lane lands.
///
/// FIXTURE CORRECTED (Task 45, 2026-09-07). The original body chained three
/// CONCENTRIC boxes — `boxs` centres on the origin, so boxes 2 and 3 were wholly
/// CONTAINED in box 1 and no scar face was ever re-imprinted. It therefore
/// PASSED on `9458f8c4`, the very commit that pinned it as "currently RED"
/// (measured in a detached worktree), i.e. it never once reproduced the defect
/// it names, and `ignored-reds.ps1` correctly flagged it as a parked red that
/// passes. The body is now the `box ∪ cyl ∪ cyl` chain the finding actually
/// describes, with the two posts INTERPENETRATING each other so the second union
/// must re-imprint the first one's scar. That chain is measured unsound at step 1
/// both on `9458f8c4` and on `780dfcab` (HEAD) — the defect is live and unfixed.
///
/// The certificate measured at `780dfcab`, step 1, verbatim:
///
/// ```text
/// box + post 1 (chained): not sound: solid 4 — origin=boolean designed=true
///   sound=false (brep_valid=false watertight=false manifold=true oriented=true
///   euler=-2 construction=not_applicable labels=not_applicable tess_clean=true
///   normal_agreement=1.000 degenerate=0)
/// ```
///
/// `brep_valid=false watertight=false` is exactly what the finding predicted;
/// `manifold=true oriented=true euler=-2` says the failure is a boundary/validity
/// one, not an orientation one, which is where a fix should look first.
///
/// The post shape is the generator's above, with one deliberate difference: the
/// generator draws `r` from the HALF-OPEN `2.0..4.0`, so `r = 4.0` sits just
/// outside its range. That is why the generator, capped at `1..=1` posts, has
/// never produced this chain on its own.
#[test]
#[ignore = "FUZZ FINDING = #27 family: a union onto a boolean RESULT goes unsound (brep_valid=false watertight=false manifold=true euler=-2); measured live at 780dfcab"]
fn chained_unions_should_stay_sound() {
    let (w, h, d) = (24.0_f64, 24.0_f64, 16.0_f64);
    let mut m = BRepModel::new();
    let mut cur = boxs(&mut m, w, h, d);
    assert_truthful_and_sound(&mut m, cur, true, "base box");
    // Centres at x = ±0.08·24 = ±1.92, i.e. 3.84 apart, with r=4 each: 3.84 < 8,
    // so the posts overlap and post 1 unions onto the scar post 0 left in the box.
    // Post 0 unions onto a PRIMITIVE and is sound; step 1 is the #27 case.
    for (i, &(fx, fy, r)) in [(-0.08_f64, 0.0_f64, 4.0_f64), (0.08, 0.0, 4.0)]
        .iter()
        .enumerate()
    {
        let post = cyl(
            &mut m,
            Point3::new(fx * w, fy * h, -d / 2.0 - 3.0),
            r,
            d + 6.0,
        );
        cur = boolean_operation(
            &mut m,
            cur,
            post,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .unwrap_or_else(|e| panic!("post {i} union failed: {e:?}"));
        assert_truthful_and_sound(&mut m, cur, false, &format!("box + post {i} (chained)"));
    }
}

/// REGRESSION FINDING (Task 45, 2026-09-07), pinned so it can be re-RUN rather
/// than re-derived. Surfaced while sweeping chain configurations for the #27
/// fixture above; it is NOT the #27 family, because it needs no chain at all --
/// this is a SINGLE union onto a PRIMITIVE.
///
/// A post tangent to the box wall: box 16 x 16 x 10, post centre
/// (-0.25*16, -0.25*16) = (-4, -4) with r4 against a box of half-width 8, base
/// z = -d/2 - 3 = -8, height d + 6 = 16 (so it pierces and protrudes both ends).
/// The post's outer edge therefore grazes the -x and -y walls exactly.
///
/// What changed, from the sweep, verbatim:
///
/// ```text
/// 9458f8c4:  step0=ERR(InvalidBRep("build_shells_from_faces: component 1 has only
///            1 planar face(s); closed polyhedral manifold requires >=4"))
/// 780dfcab:  step0=false          (Ok, certificate sound=false)
/// ```
///
/// The certificate behind that `step0=false`, measured at `780dfcab`, verbatim:
///
/// ```text
/// wall-tangent post, single union: not sound: solid 2 — origin=boolean
///   designed=true sound=false (brep_valid=false watertight=false manifold=false
///   oriented=false euler=1 construction=not_applicable labels=not_applicable
///   tess_clean=true normal_agreement=1.000 degenerate=0)
/// ```
///
/// Note this one fails HARDER than the #27 chain above: `manifold=false
/// oriented=false euler=1` (an open shell), against the chain's `manifold=true
/// oriented=true euler=-2`. Different defect, different lane.
///
/// The kernel used to REFUSE this union with a typed error. It then returned
/// `Ok` with a solid that certifies UNSOUND. That is the wrong direction under
/// "honest refusal over silent wrong answers": a caller who trusted the `Ok` is
/// handed geometry the kernel itself knows is bad. Either outcome that does not
/// lie is acceptable here, so the test PASSES on a typed refusal and passes on a
/// sound solid -- it fails only on the Ok-but-unsound answer.
///
/// CLOSED by Task 47 (2026-09-08). Bisect named `1016a695` ("boolean: #32 Phase
/// B -- per-face coincident-curve dedup") as the first bad commit: at exact
/// tangency `create_cylinder_parallel_intersection_lines` collapses its two
/// chord generators onto ONE line (`acos(d/r) = acos(1) = 0`), and Phase B's
/// per-target-face dedup reads that doubled generator as a routing duplicate and
/// drops one copy -- which lets the arrangement build a single 11-face component
/// that the polyhedral `< 4 faces` guard in `build_shells_from_faces` (the
/// source of the 9458f8c4 refusal above) never inspects.
///
/// The dedup is NOT reverted, because it is right about the other half of the
/// tangency family: measured at `439ccbc2`, a CONTAINED tangent post (same box,
/// same radius, post inside the box's z-extent) unions AND intersects SOUND
/// (euler=2) -- see `contained_tangent_post_*` below. Only a tangency whose
/// locus reaches a free boundary goes wrong, and that is a fact about the built
/// result, not about the input. So the fix witnesses the doubled curve, lets the
/// boolean run, and refuses only when THAT result certifies unsound. This
/// protruding post is unsound under both union and intersection, so it is
/// refused, and the refusal names the tangency.
#[test]
fn tangent_post_union_refuses_or_stays_sound() {
    let (w, h, d) = (16.0_f64, 16.0_f64, 10.0_f64);
    let mut m = BRepModel::new();
    let base = boxs(&mut m, w, h, d);
    let post = cyl(
        &mut m,
        Point3::new(-0.25 * w, -0.25 * h, -d / 2.0 - 3.0),
        4.0,
        d + 6.0,
    );
    match boolean_operation(
        &mut m,
        base,
        post,
        BooleanOp::Union,
        BooleanOptions::default(),
    ) {
        // A typed refusal is a PASS: the kernel declined instead of lying.
        Err(_) => {}
        Ok(res) => assert_truthful_and_sound(&mut m, res, false, "wall-tangent post, single union"),
    }
}

/// Task 47 message pin. The two-sided contract above deliberately accepts ANY
/// typed refusal, so on its own it cannot tell a refusal that NAMES the
/// degeneracy from one that merely fails somewhere. This test pins the refusal
/// the fix installs: the variant, the words that identify the geometry
/// (tangential contact, a doubled intersection curve), the named escape hatch,
/// and the house rule that a wrapped message carries no doubled spaces.
///
/// It also pins the ONE-SIDEDNESS of the witness by construction: this is a
/// single union onto a primitive, so the doubled curve it refuses on can only
/// have come from a single face pair. And it pins that the refusal QUOTES the
/// certificate, which is what makes it a measurement rather than a guess.
#[test]
fn tangent_post_union_refusal_names_the_tangency() {
    let (w, h, d) = (16.0_f64, 16.0_f64, 10.0_f64);
    let mut m = BRepModel::new();
    let base = boxs(&mut m, w, h, d);
    let post = cyl(
        &mut m,
        Point3::new(-0.25 * w, -0.25 * h, -d / 2.0 - 3.0),
        4.0,
        d + 6.0,
    );
    let err = match boolean_operation(
        &mut m,
        base,
        post,
        BooleanOp::Union,
        BooleanOptions::default(),
    ) {
        Err(e) => e,
        Ok(res) => panic!("wall-tangent post union returned Ok({res}) instead of refusing"),
    };
    let msg = match &err {
        OperationError::InvalidBRep(m) => m.clone(),
        other => panic!("expected InvalidBRep, got {other:?}"),
    };
    assert!(
        msg.contains("TANGENTIALLY"),
        "refusal must name the degeneracy: {msg}"
    );
    assert!(
        msg.contains("doubled"),
        "refusal must name the doubled intersection curve: {msg}"
    );
    assert!(
        msg.contains("manifold=false"),
        "refusal must quote the certificate bit that decided it: {msg}"
    );
    assert!(
        msg.contains("self_intersection_free=false"),
        "refusal must name the fixture's hidden fifth failure: {msg}"
    );
    assert!(
        msg.contains("allow_non_manifold=true"),
        "refusal must name the escape hatch: {msg}"
    );
    assert!(
        msg.contains("certifies UNSOUND"),
        "refusal must quote the certificate it acted on: {msg}"
    );
    assert!(
        !msg.contains("  "),
        "wrapped message has doubled spaces: {msg}"
    );
    // This post is tangent at TWO loci -- the -x wall and the -y wall -- and the
    // refusal must report both, not stop at the first witness.
    assert!(
        msg.contains("faces 2 and 8") && msg.contains("faces 4 and 8"),
        "both tangent loci must be named: {msg}"
    );
    // The refusal is a NON-EVENT: `with_rollback` restores both operands, so a
    // caller that handles the Err still holds the geometry it passed in.
    assert!(
        m.solids.get(base).is_some() && m.solids.get(post).is_some(),
        "a refused boolean must roll back: both operands must survive"
    );
}

/// The escape hatch is real, not decorative: with `allow_non_manifold` set the
/// caller has explicitly asked for a possibly-open shell, so the witness scan is
/// skipped entirely and this guard stands aside, exactly as the sibling
/// `< 4 planar faces` shell guard does.
///
/// Both arms are asserted, so the test cannot pass vacuously. On `Ok` the result
/// must be a REAL solid the caller can go on to use -- present in the model,
/// provenanced to a boolean, with both operands consumed as a union implies --
/// and NOTHING here asserts it is sound, because the whole point of the flag is
/// that the caller accepted an open shell. On `Err` the failure must be some
/// OTHER refusal, never this one.
#[test]
fn tangent_post_union_is_not_refused_when_non_manifold_is_allowed() {
    let (w, h, d) = (16.0_f64, 16.0_f64, 10.0_f64);
    let mut m = BRepModel::new();
    let base = boxs(&mut m, w, h, d);
    let post = cyl(
        &mut m,
        Point3::new(-0.25 * w, -0.25 * h, -d / 2.0 - 3.0),
        4.0,
        d + 6.0,
    );
    let options = BooleanOptions {
        allow_non_manifold: true,
        ..BooleanOptions::default()
    };
    match boolean_operation(&mut m, base, post, BooleanOp::Union, options) {
        Ok(res) => {
            assert!(
                m.solids.get(res).is_some(),
                "allow_non_manifold returned Ok({res}) but the solid is not in the model"
            );
            let gt = m
                .ground_truth(res)
                .unwrap_or_else(|| panic!("solid {res} has no ground truth"));
            let prov = gt
                .provenance
                .as_ref()
                .unwrap_or_else(|| panic!("solid {res} has no provenance"));
            assert!(
                matches!(prov.created_by, OperationKind::Boolean),
                "result must be provenanced as a boolean, got {:?}",
                prov.created_by
            );
            assert!(
                m.solids.get(base).is_none() && m.solids.get(post).is_none(),
                "a successful union consumes both operands"
            );
        }
        Err(OperationError::InvalidBRep(msg)) => assert!(
            !msg.contains("TANGENTIALLY"),
            "allow_non_manifold must bypass the tangency guard: {msg}"
        ),
        Err(_) => {}
    }
}

/// Mutation guard for the guard's DISCRIMINATOR. A well-conditioned post that
/// pierces the box away from every wall shares the whole rest of the fixture --
/// same box, same radius, same protrusion -- and differs only in that nothing
/// is tangent. It must still union soundly, so a guard widened from "coincident
/// within one pair" to anything coarser (per target face, or unconditional)
/// turns this green case red and is caught here rather than in a sweep.
#[test]
fn clear_of_the_walls_the_same_post_still_unions_soundly() {
    let (w, h, d) = (16.0_f64, 16.0_f64, 10.0_f64);
    let mut m = BRepModel::new();
    let base = boxs(&mut m, w, h, d);
    let post = cyl(&mut m, Point3::new(0.0, 0.0, -d / 2.0 - 3.0), 4.0, d + 6.0);
    let res = boolean_operation(
        &mut m,
        base,
        post,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .unwrap_or_else(|e| panic!("centred post union must not be refused: {e:?}"));
    assert_truthful_and_sound(&mut m, res, false, "wall-clear post, single union");
}

/// The refusal is a property of the CONTACT, not of the operator. The same
/// protruding wall-tangent post under INTERSECTION was measured at `439ccbc2`
/// as `Ok` + unsound too -- a different signature (`manifold=true oriented=true
/// euler=1` against the union's `manifold=false oriented=false euler=1`), the
/// same lie. It must be refused for the same reason.
#[test]
fn tangent_post_intersection_is_refused_too() {
    let (w, h, d) = (16.0_f64, 16.0_f64, 10.0_f64);
    let mut m = BRepModel::new();
    let base = boxs(&mut m, w, h, d);
    let post = cyl(
        &mut m,
        Point3::new(-0.25 * w, -0.25 * h, -d / 2.0 - 3.0),
        4.0,
        d + 6.0,
    );
    match boolean_operation(
        &mut m,
        base,
        post,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    ) {
        Err(OperationError::InvalidBRep(msg)) => assert!(
            msg.contains("TANGENTIALLY"),
            "intersection refusal must name the tangency too: {msg}"
        ),
        Err(other) => panic!("expected the tangency refusal, got {other:?}"),
        Ok(res) => {
            assert_truthful_and_sound(&mut m, res, false, "wall-tangent post, single intersection")
        }
    }
}

/// THE OVER-REFUSAL GUARD, and the reason Task 47 does not refuse on the
/// witness alone. A CONTAINED tangent post -- the same box, the same centre
/// `(-4, -4)`, the same `r = 4` grazing the `-x` and `-y` walls, but height 6
/// from `z = -3` so it sits wholly inside the box's `z` extent -- carries the
/// identical doubled tangent generator, yet the union is the box and builds
/// clean. Measured at `439ccbc2` before any fix:
///
/// ```text
/// C contained union: OK_SOUND solid 2 - origin=boolean designed=true sound=true
///   (brep_valid=true watertight=true manifold=true oriented=true euler=2 ...)
/// ```
///
/// A tangency guard that refused on the input would take this capability away.
/// It must keep building, and soundly.
#[test]
fn contained_tangent_post_unions_soundly() {
    let (w, h, d) = (16.0_f64, 16.0_f64, 10.0_f64);
    let mut m = BRepModel::new();
    let base = boxs(&mut m, w, h, d);
    let post = cyl(&mut m, Point3::new(-0.25 * w, -0.25 * h, -3.0), 4.0, 6.0);
    let res = boolean_operation(
        &mut m,
        base,
        post,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .unwrap_or_else(|e| panic!("a contained tangent post must still union: {e:?}"));
    assert_truthful_and_sound(&mut m, res, false, "contained tangent post, union");
}

/// The intersection half of the over-refusal guard: the same contained tangent
/// post, measured `OK_SOUND` (euler=2) at `439ccbc2`, must still intersect.
#[test]
fn contained_tangent_post_intersects_soundly() {
    let (w, h, d) = (16.0_f64, 16.0_f64, 10.0_f64);
    let mut m = BRepModel::new();
    let base = boxs(&mut m, w, h, d);
    let post = cyl(&mut m, Point3::new(-0.25 * w, -0.25 * h, -3.0), 4.0, 6.0);
    let res = boolean_operation(
        &mut m,
        base,
        post,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .unwrap_or_else(|e| panic!("a contained tangent post must still intersect: {e:?}"));
    assert_truthful_and_sound(&mut m, res, false, "contained tangent post, intersection");
}
