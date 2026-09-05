// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **A boolean-minted face carried the `[0, 1]²` UV placeholder as if it were
//! its parametric domain, and every consumer that reads that field believed
//! it.**
//!
//! `Face::new` writes `uv_bounds: [0, 1, 0, 1]` because a constructor handed
//! an id, a surface and a loop cannot know the domain — the domain is a
//! property of the loop's geometry projected onto the surface. The boolean's
//! result-face mint (`operations/boolean.rs`) never told it, so a re-trimmed
//! cylinder wall spanning 2π radians by 20 mm of height reported a domain of
//! one radian by one millimetre.
//!
//! That is not a small error. The curved-surface area quadrature integrates
//! `‖S_u × S_v‖` over exactly that rectangle, so a wall of ~1135 mm² came back
//! as ~10 mm² — and it came back through `readable::query_face().area`,
//! `queries::measure`'s `FaceInfo.area`, and `readable::claim`'s claim
//! verifier, which is to say through the whole agent-facing surface, wearing
//! no mark that anything was estimated. **The kernel cannot lie**: either the
//! domain is measured and the number is real, or the domain is unknown and the
//! kernel refuses.
//!
//! The tests below pin both halves:
//!
//! * [`boolean_cut_cylinder_wall_reports_its_real_area`] and
//!   [`claim_verifier_stamps_a_correct_face_area_on_a_boolean_wall`] —
//!   measured bounds, real number. RED at ~10 mm² before the fix.
//! * [`boolean_cut_frustum_lateral_reports_analytic_cone_curvature`] — the
//!   probe point moves with the domain; before the fix it sat near the cone's
//!   apex and reported a curvature the face does not have anywhere.
//! * [`primitive_box_planar_faces_are_unchanged`] — the control. A planar
//!   face's area never went through `uv_bounds` and must not move.
//! * [`unmeasured_domain_refuses_rather_than_reporting_the_placeholder`] — the
//!   refusal. A primitive cylinder's own lateral face is *still* unmeasured
//!   (its builder never called `set_uv_bounds` either — a wider instance of
//!   this same defect, deliberately left in scope for its own change), so it
//!   is the honest fixture for "unknown domain": it must report `None`, not
//!   the ~10 mm² the placeholder would integrate to.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::face::Face;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::features::{feature_normal_cone, SupermaximalFeature};
use geometry_engine::queries::measure::{measure, MeasureResult, MeasureSubject};
use geometry_engine::queries::select::{
    resolve_face, Extremal, FaceQuery, SelectError, SurfaceKind,
};
use geometry_engine::readable::claim::{verify_claim, CheckableClaim, ClaimBinding, Measurement};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

// ─── fixtures ────────────────────────────────────────────────────────────────

const CYL_R: f64 = 10.0;
const CYL_H: f64 = 20.0;

/// Half-width in `y` of the slot cut through the wall. Chosen so both cut
/// edges land at a well-conditioned angle on the r=10 circle.
const SLOT_HALF_Y: f64 = 3.0;

fn cylinder(model: &mut BRepModel, r: f64, h: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, r, h)
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn frustum(model: &mut BRepModel, r_base: f64, r_top: f64, h: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_cone_3d(Point3::ORIGIN, Vector3::Z, r_base, r_top, h)
        .expect("frustum")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// A box translated so it covers `x ∈ [0, 30]`, `|y| ≤ SLOT_HALF_Y`, and the
/// full height of a solid based at `z = 0` and taller than nothing.
///
/// **Why it straddles the seam.** The cylinder's parametric seam sits at
/// `u = 0`, i.e. at `+X`. Putting the removed band on top of it consumes the
/// seam edge, so the surviving wall is ONE connected fragment spanning
/// `u ∈ [θ, 2π−θ]` with no seam in its boundary — a rectangle in `(u, v)`,
/// which is what makes the expected area exactly `r · Δu · h` and lets the
/// assertion measure the domain and nothing else. Measured: the boolean emits
/// that rectangle as an EIGHT-edge loop (it splits each arc), not the four-edge
/// loop the analytic primitive carries — which is precisely why the old
/// `edges.len() == 4` trim-mask fast path missed it.
///
/// A cut placed elsewhere leaves the seam edge in the loop and splits the wall
/// in two, so the fixture would stop testing one rectangle.
fn seam_slot(model: &mut BRepModel, height: f64) -> SolidId {
    let id = {
        let mut b = TopologyBuilder::new(model);
        match b
            .create_box_3d(30.0, 2.0 * SLOT_HALF_Y, height + 10.0)
            .expect("slot box")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&Vector3::new(15.0, 0.0, 0.5 * height)),
        TransformOptions::default(),
    )
    .expect("translate slot");
    id
}

fn face_ids(model: &BRepModel, solid: SolidId) -> Vec<u32> {
    let s = model.solids.get(solid).expect("solid");
    let mut out = Vec::new();
    for sh in [s.outer_shell]
        .into_iter()
        .chain(s.inner_shells.iter().copied())
    {
        if let Some(shell) = model.shells.get(sh) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

/// Every face of `solid` whose supporting surface reports `kind`.
fn faces_of_kind(model: &BRepModel, solid: SolidId, kind: &str) -> Vec<u32> {
    face_ids(model, solid)
        .into_iter()
        .filter(|&fid| {
            model
                .faces
                .get(fid)
                .and_then(|f| model.surfaces.get(f.surface_id))
                .map(|s| s.type_name() == kind)
                .unwrap_or(false)
        })
        .collect()
}

/// Summed triangle area of one B-Rep face in the solid's render mesh — the
/// independent witness. It is produced by the tessellator, which reads the
/// boundary loops and never reads `uv_bounds`, so it cannot agree with a
/// wrong domain by construction.
fn tessellated_face_area(model: &BRepModel, solid: SolidId, fid: u32) -> f64 {
    let s = model.solids.get(solid).expect("solid");
    let mesh = tessellate_solid(s, model, &TessellationParams::default());
    assert_eq!(
        mesh.face_map.len(),
        mesh.triangles.len(),
        "face_map must cover every triangle for a per-face area to exist"
    );
    let mut area = 0.0;
    for (i, tri) in mesh.triangles.iter().enumerate() {
        if mesh.face_map[i] != fid {
            continue;
        }
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        area += 0.5 * (p1 - p0).cross(&(p2 - p0)).magnitude();
    }
    area
}

fn rel_err(a: f64, b: f64) -> f64 {
    ((a - b) / b).abs()
}

/// Put a face back to what a CONSTRUCTOR alone produces: same surface, same
/// loops, `uv_bounds` the `Face::new` placeholder, nothing measured.
///
/// Every test below that needs "a face whose domain nobody measured" used to
/// take one for free from a primitive builder, because no primitive builder
/// measured. Task 32 wired every production mint, so that source is gone - and
/// leaning on it was these fixtures encoding the very defect they were written
/// beside, which is why each carried a precondition assertion predicting this
/// day out loud.
///
/// This is the honest replacement, and it depends on no mint at all:
/// `Face::new` is handed an id, a surface and a loop, which cannot determine a
/// domain, and `set_uv_bounds` - the only thing that flips the measured flag -
/// is deliberately never called on it. There is no way to UNSET the flag by
/// design, so re-minting the face is how the unmeasured state is expressed.
fn unmeasure(model: &mut BRepModel, fid: u32) {
    let face = model.faces.get(fid).expect("face to unmeasure").clone();
    let mut fresh = Face::new(face.id, face.surface_id, face.outer_loop, face.orientation);
    for inner in &face.inner_loops {
        fresh.add_inner_loop(*inner);
    }
    *model.faces.get_mut(fid).expect("face to unmeasure") = fresh;
    assert!(
        !model
            .faces
            .get(fid)
            .expect("face to unmeasure")
            .uv_bounds_are_measured(),
        "unmeasure() must leave the face carrying no measurement"
    );
}

// ─── the fix: a boolean-minted curved face knows its own domain ──────────────

/// A cylinder wall cut by a full-height slot reports the area it actually has.
///
/// The witness is the tessellated area of the same face. Chord error on the
/// default tessellation of an r=10 wall is ~0.04% (the facet subtends ~0.1 rad),
/// far inside the 1% budget, and it errs LOW — so a quadrature that agreed with
/// it by returning ~10 mm² would have to be wrong by two orders of magnitude in
/// the same direction, which is exactly the pre-fix reading this pins.
#[test]
fn boolean_cut_cylinder_wall_reports_its_real_area() {
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);
    let slot = seam_slot(&mut model, CYL_H);
    let cut = boolean_operation(
        &mut model,
        cyl,
        slot,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let walls = faces_of_kind(&model, cut, "Cylinder");
    assert_eq!(
        walls.len(),
        1,
        "the slot consumes the seam, leaving exactly one wall fragment; got {walls:?}"
    );
    let wall = walls[0];

    let mesh_area = tessellated_face_area(&model, cut, wall);
    assert!(
        mesh_area > 0.5 * CYL_R * CYL_H,
        "the mesh witness must itself be a real wall area, got {mesh_area}"
    );

    let report = model.query_face(wall).expect("wall face report");
    let area = report
        .area
        .expect("a measured wall must report an area, not refuse");

    assert!(
        rel_err(area, mesh_area) <= 0.01,
        "wall area {area} mm2 disagrees with its own tessellation {mesh_area} mm2 \
         by {:.2}% (budget 1%)",
        100.0 * rel_err(area, mesh_area)
    );
}

/// The claim verifier — the agent-facing "check my number against the kernel"
/// path — stamps a correct `FaceArea` claim VERIFIED and a wrong one FALSE.
///
/// Both directions matter: a verifier that refuses everything also never
/// stamps a wrong claim, and a verifier reading the placeholder would verify
/// the claim `area = 10` that no wall has.
#[test]
fn claim_verifier_stamps_a_correct_face_area_on_a_boolean_wall() {
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);
    let slot = seam_slot(&mut model, CYL_H);
    let cut = boolean_operation(
        &mut model,
        cyl,
        slot,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let wall = faces_of_kind(&model, cut, "Cylinder")[0];
    let truth = tessellated_face_area(&model, cut, wall);

    let claim = |expected: f64| CheckableClaim {
        expr: "a".to_string(),
        bindings: vec![ClaimBinding {
            var: "a".to_string(),
            measure: Measurement::FaceArea { face: wall },
        }],
        expected,
        // 1% of the true area: the same budget the area test uses, so this
        // asserts the verifier's plumbing, not a tighter quadrature claim.
        tolerance: Some(0.01 * truth),
    };

    let good = verify_claim(&claim(truth), &mut model);
    assert!(
        !good.refused,
        "a measured wall must not refuse a FaceArea claim: {good:?}"
    );
    assert!(
        good.verified,
        "claim area={truth} must verify against the same wall: {good:?}"
    );

    // The placeholder domain would have produced ~10 mm2. That must now be
    // stamped FALSE, not verified — otherwise this test passes on a kernel
    // that simply refuses nothing.
    let bad = verify_claim(&claim(10.0), &mut model);
    assert!(
        !bad.refused && !bad.verified,
        "claim area=10 (the placeholder reading) must be answered and REJECTED: {bad:?}"
    );
}

/// Principal curvature on a boolean-cut frustum's lateral face matches the
/// analytic cone value AT THE POINT THE KERNEL SAYS IT SAMPLED.
///
/// A cone's curvature varies along the axis (`|k1| = cos α / ρ`), so unlike a
/// cylinder this cannot pass by accident on a wrong probe point: with the
/// `[0, 1]²` placeholder the probe sits at `v = 0.5`, a hair from the apex,
/// where `ρ` is a fraction of a millimetre and `|k1|` is orders of magnitude
/// larger than anything on the face.
#[test]
fn boolean_cut_frustum_lateral_reports_analytic_cone_curvature() {
    const R_BASE: f64 = 10.0;
    const R_TOP: f64 = 6.0;
    const H: f64 = 20.0;

    let mut model = BRepModel::new();
    let cone = frustum(&mut model, R_BASE, R_TOP, H);
    let slot = seam_slot(&mut model, H);
    let cut = boolean_operation(
        &mut model,
        cone,
        slot,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let laterals = faces_of_kind(&model, cut, "Cone");
    assert!(
        !laterals.is_empty(),
        "the cut must leave lateral cone geometry behind"
    );

    let half_angle = ((R_BASE - R_TOP) / H).atan();
    let mut answered = 0usize;

    for lateral in laterals {
        let report = model.query_face(lateral).expect("lateral face report");
        // A fragment whose domain the kernel could not bound refuses, and that
        // is a legal answer — the assertion below is on what it DOES report.
        // The `answered` counter is what stops this test passing on a kernel
        // that refuses everything.
        let Some([k1, k2]) = report.principal_curvatures else {
            continue;
        };
        answered += 1;

        // The point the kernel says it sampled: the centre of the face's own
        // domain. Read from the same field the consumer reads.
        let face = model.faces.get(lateral).expect("lateral face");
        let [u0, u1, v0, v1] = face.uv_bounds;
        let probe = model
            .surfaces
            .get(face.surface_id)
            .expect("cone surface")
            .point_at(0.5 * (u0 + u1), 0.5 * (v0 + v1))
            .expect("probe point");

        // Independent analytic expectation, from the frustum's own dimensions
        // and the probe's distance to the axis — never from the surface's own
        // curvature code, which is the thing under test.
        let rho = (probe.x * probe.x + probe.y * probe.y).sqrt();
        assert!(
            rho > R_TOP - 1e-6 && rho < R_BASE + 1e-6,
            "face {lateral}: a reported curvature means the probe is on the \
             frustum band (rho in [{R_TOP}, {R_BASE}]), got {rho}"
        );
        let expected_k = half_angle.cos() / rho;

        assert!(
            rel_err(k1.abs(), expected_k) <= 1e-6,
            "face {lateral}: |k1| = {} but the cone at rho={rho} has \
             cos(a)/rho = {expected_k}",
            k1.abs()
        );
        assert!(
            k2.abs() <= 1e-9,
            "face {lateral}: a cone's second principal curvature is 0 along \
             the ruling, got {k2}"
        );
    }

    assert!(
        answered > 0,
        "at least one surviving lateral must have a measured domain — a kernel \
         that refuses every fragment would satisfy the loop above vacuously"
    );
}

// ─── control ─────────────────────────────────────────────────────────────────

/// A planar face's area never read `uv_bounds` — the planar branch of
/// `compute_surface_area` works from the boundary loop — so nothing about this
/// change may move it, and the refusal must not spread to it. Every face of a
/// 20 mm box is exactly 400 mm², and each reports it.
///
/// The fixture is a BOX, not the cylinder's own caps: a cylinder cap's outer
/// loop is a single closed circle edge, which the planar area path already
/// declines to measure today (a separate, pre-existing gap). A control has to
/// be a case that produces a number, or it cannot show the number is unmoved.
#[test]
fn primitive_box_planar_faces_are_unchanged() {
    const SIDE: f64 = 20.0;
    let mut model = BRepModel::new();
    let bx = {
        let mut b = TopologyBuilder::new(&mut model);
        match b.create_box_3d(SIDE, SIDE, SIDE).expect("box") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };

    let planes = faces_of_kind(&model, bx, "Plane");
    assert_eq!(
        planes.len(),
        6,
        "a box has six planar faces, got {planes:?}"
    );

    let expected = SIDE * SIDE;
    for fid in planes {
        let area = model
            .query_face(fid)
            .expect("box face report")
            .area
            .expect("a planar face's area does not depend on uv_bounds");
        assert!(
            rel_err(area, expected) <= 1e-9,
            "box face {fid} area {area} mm2 != {expected} mm2"
        );
    }
}

// ─── refusal ─────────────────────────────────────────────────────────────────

/// A curved face whose domain nobody measured refuses instead of reporting the
/// placeholder's integral.
///
/// The fixture is a *primitive* cylinder's lateral face: `create_cylinder_3d`
/// never calls `set_uv_bounds` either, so the face carries `[0, 1]²` with no
/// claim to it. Integrating over that rectangle yields ~10 mm² for a
/// 1257 mm² wall — a number with no relationship to the face, published
/// through the same `area` field an agent reads on a measured face. `None` is
/// the only honest answer available without measuring, and this asserts the
/// kernel gives it.
///
/// Deliberately NOT asserted here: that the area equals ~10. Encoding the
/// placeholder reading as expected behaviour is how the lie survives a
/// refactor.
#[test]
fn unmeasured_domain_refuses_rather_than_reporting_the_placeholder() {
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);

    let walls = faces_of_kind(&model, cyl, "Cylinder");
    assert_eq!(walls.len(), 1, "one lateral face expected, got {walls:?}");
    let wall = walls[0];

    unmeasure(&mut model, wall);
    assert_eq!(
        model.faces.get(wall).expect("wall face").uv_bounds,
        [0.0, 1.0, 0.0, 1.0],
        "fixture precondition: the face under test carries the `Face::new` \
         placeholder and no claim to it"
    );

    let report = model.query_face(wall).expect("wall face report");
    assert!(
        report.area.is_none(),
        "an unmeasured curved face must refuse its area, got {:?}",
        report.area
    );
    assert!(
        report.principal_curvatures.is_none(),
        "an unmeasured curved face has no locatable probe point, got {:?}",
        report.principal_curvatures
    );
}

// ─── seam-straddling hole: the branch the projection landed on ───────────────

/// A window cut through the wall ACROSS the parametric seam is subtracted in
/// full, not by half.
///
/// Every loop is projected by its own walk, so the wall's outer boundary lifts
/// to `[0, 2π]` while the window's loop lifts to roughly `[-0.30, +0.30]`.
/// Only the half with `u ≥ 0` ever meets the integration grid, so the other
/// half of the window is never subtracted and the face OVER-reports.
///
/// Measured on this exact fixture before the fix: **1231.5 mm²** against a
/// truth of **1207.9** — over by **25.1**, which is exactly half of the
/// **48.8 mm²** window. That the error is precisely half the hole is what
/// identifies the cause as the branch, not as quadrature coarseness.
///
/// **The oracle is analytic, not the mesh.** The tessellator does not cut this
/// hole at all (the same face meshes to 1256.4 mm², the full untouched wall),
/// so the mesh cannot witness a hole here — see the report.
#[test]
fn seam_straddling_hole_is_subtracted_on_both_sides() {
    const WIN_HALF_Y: f64 = 3.0;
    const WIN_Z_LO: f64 = 6.0;
    const WIN_Z_HI: f64 = 14.0;

    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);

    // A box spanning x ∈ [0, 30], |y| ≤ 3, z ∈ [6, 14]: it pierces the wall at
    // the seam (u = 0, i.e. +X) and reaches NEITHER cap, so the wall keeps its
    // full 2π sweep and the removed patch becomes an inner loop rather than
    // splitting the face.
    let win = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_box_3d(30.0, 2.0 * WIN_HALF_Y, WIN_Z_HI - WIN_Z_LO)
            .expect("window box")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    transform_solid(
        &mut model,
        win,
        Matrix4::from_translation(&Vector3::new(15.0, 0.0, 0.5 * (WIN_Z_LO + WIN_Z_HI))),
        TransformOptions::default(),
    )
    .expect("translate window");

    let cut = boolean_operation(
        &mut model,
        cyl,
        win,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let walls = faces_of_kind(&model, cut, "Cylinder");
    assert_eq!(walls.len(), 1, "one wall face expected, got {walls:?}");
    let wall = walls[0];

    // Fixture precondition: the window really is a HOLE in this face, not a
    // split. Without an inner loop this test exercises nothing.
    assert_eq!(
        model.faces.get(wall).expect("wall face").inner_loops.len(),
        1,
        "the window must appear as one inner loop on the wall"
    );

    // Analytic truth: full wall minus the window's own patch. The window's
    // u-extent on the r=10 circle is 2·atan2(3, √(100−9)); its v-extent is the
    // box's height. Computed from the cut's dimensions, not from the kernel.
    let half_u = (WIN_HALF_Y / (CYL_R * CYL_R - WIN_HALF_Y * WIN_HALF_Y).sqrt()).atan();
    let full = 2.0 * std::f64::consts::PI * CYL_R * CYL_H;
    let window = CYL_R * (2.0 * half_u) * (WIN_Z_HI - WIN_Z_LO);
    let expected = full - window;

    let area = model
        .query_face(wall)
        .expect("wall report")
        .area
        .expect("a measured wall must report an area");

    assert!(
        rel_err(area, expected) <= 0.01,
        "wall area {area} mm2 != {expected} mm2 (full {full} - window {window}); \
         off by {:.1} mm2, and HALF the window is {:.1} mm2",
        area - expected,
        0.5 * window
    );
}

// ─── refusals must not be swallowed by the consumers ────────────────────────

/// `resolve_face` reports "I cannot test this candidate", not "no face matches".
///
/// A primitive cylinder's lateral is unmeasured, so the normal-direction filter
/// cannot evaluate it. Before this change the filter folded that into
/// `continue`, the candidate set emptied, and the resolver answered
/// `Err(NotFound)` — **a positive claim about the geometry** ("no cylindrical
/// face of this solid points +X") which is false: the wall points +X somewhere
/// and the kernel simply could not locate the point to sample. `Unmeasurable`
/// is a claim about the kernel's own knowledge, which is the true one.
///
/// The sharper hazard — one testable match plus one untestable candidate
/// resolving to a confident `Ok(single)` — is pinned in `queries::select`'s own
/// unit tests, because no single solid this kernel builds today can express it:
/// every boolean output face is re-minted and measured, and every primitive
/// lateral is not, so the two states never meet on one surface kind of one
/// solid.
#[test]
fn select_reports_untestable_rather_than_claiming_no_match() {
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);

    let walls = faces_of_kind(&model, cyl, "Cylinder");
    assert_eq!(walls.len(), 1, "one lateral expected, got {walls:?}");
    // The lateral must be unmeasured, else there is nothing for the resolver
    // to refuse over.
    unmeasure(&mut model, walls[0]);

    let q = FaceQuery::new(SurfaceKind::Cylindrical)
        .facing(Vector3::X)
        .extremal(Extremal::None);
    match resolve_face(&mut model, cyl, &q) {
        Err(SelectError::Unmeasurable {
            matched,
            unmeasurable,
        }) => {
            assert!(
                matched.is_empty(),
                "nothing was testable, so nothing may be reported as matched: {matched:?}"
            );
            assert_eq!(
                unmeasurable, walls,
                "the refusal must name the face it could not test"
            );
        }
        other => panic!(
            "an untestable candidate must be reported as such, not resolved or \
             denied; got {other:?}"
        ),
    }
}

/// A feature holding one unmeasurable face has a normal cone that excludes
/// NOTHING.
///
/// The cone is an upper bound used to cull contact pairs: a direction outside
/// it is one the feature provably cannot face. Dropping an unmeasurable face
/// and hulling the rest yields a SUBSET presented as the whole, which prunes
/// pairs that can really touch. The sound answer is the full space — the cull
/// then never prunes this feature.
#[test]
fn normal_cone_widens_to_full_space_when_a_face_is_unmeasurable() {
    let mut model = BRepModel::new();
    let bx = {
        let mut b = TopologyBuilder::new(&mut model);
        match b.create_box_3d(10.0, 10.0, 10.0).expect("box") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    let cyl = cylinder(&mut model, CYL_R, CYL_H);
    let walls = faces_of_kind(&model, cyl, "Cylinder");
    assert_eq!(walls.len(), 1, "one lateral expected, got {walls:?}");
    unmeasure(&mut model, walls[0]);

    // A pair of planar box faces whose cone genuinely CONSTRAINS — the control
    // that proves the assertion below is not vacuous.
    //
    // Searched rather than assumed: an ANTIPODAL pair (+X and −X) generates a
    // cone the kernel reports as the full space, so "any two planar faces"
    // would silently give a vacuous control. Take the first pair that actually
    // excludes a probe direction, and fail loudly if the box has none.
    let planes = faces_of_kind(&model, bx, "Plane");
    assert!(planes.len() >= 2, "a box has six planar faces");
    let probes = [
        Vector3::X,
        -Vector3::X,
        Vector3::Y,
        -Vector3::Y,
        Vector3::Z,
        -Vector3::Z,
    ];
    let mut pair: Option<Vec<u32>> = None;
    'search: for i in 0..planes.len() {
        for j in (i + 1)..planes.len() {
            let candidate = vec![planes[i], planes[j]];
            let cone = feature_normal_cone(
                &model,
                &SupermaximalFeature {
                    faces: candidate.clone(),
                },
            );
            if probes.iter().any(|d| !cone.contains(d)) {
                pair = Some(candidate);
                break 'search;
            }
        }
    }
    let pair = pair.expect(
        "no pair of the box's planar faces produced a cone that excludes any \
         probe direction — the control cannot be established, so the assertion \
         below would prove nothing",
    );

    // Add the unmeasurable lateral: nothing can be excluded any more.
    let mut faces = pair;
    faces.extend(walls);
    let cone = feature_normal_cone(&model, &SupermaximalFeature { faces });
    for d in probes {
        assert!(
            cone.contains(&d),
            "one unmeasurable face means no direction can be excluded, but \
             {d:?} was"
        );
    }
}

// ─── the measurement's refusal branches ─────────────────────────────────────

/// At product level: a boolean-cut spherical cap refuses its area rather than
/// publishing one derived from its rim.
///
/// A cap's domain can ENCLOSE a pole its boundary never reaches, so the rim's
/// `(u, v)` bbox is strictly inside the domain and understates it — see
/// `tessellation::surface::uv_domain_measurement_tests`, where each refusal
/// branch is exercised in isolation with its own mutation.
///
/// This test deliberately does NOT claim which branch declines: measured
/// against this fixture, the cap is refused even with the pole guard and the
/// collapsed-bbox check both removed, so more than one branch covers it. What
/// it pins is the statement an agent sees — the kernel does not publish an area
/// for a face whose domain it could not establish.
#[test]
fn obliquely_cut_sphere_cap_refuses_rather_than_measuring_its_rim() {
    let mut model = BRepModel::new();
    let sph = {
        let mut b = TopologyBuilder::new(&mut model);
        match b.create_sphere_3d(Point3::ORIGIN, 10.0).expect("sphere") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    // A half-space whose face is oblique to the sphere's polar axis, so the rim
    // is a great circle at VARYING latitude and the cap encloses a pole.
    let knife = {
        let mut b = TopologyBuilder::new(&mut model);
        match b.create_box_3d(60.0, 60.0, 60.0).expect("knife") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    transform_solid(
        &mut model,
        knife,
        Matrix4::from_translation(&Vector3::new(0.0, 24.0, 24.0)),
        TransformOptions::default(),
    )
    .expect("translate knife");

    let cut = boolean_operation(
        &mut model,
        sph,
        knife,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let caps = faces_of_kind(&model, cut, "Sphere");
    assert!(!caps.is_empty(), "the cut must leave spherical geometry");
    for fid in caps {
        assert!(
            !model
                .faces
                .get(fid)
                .expect("cap face")
                .uv_bounds_are_measured(),
            "face {fid}: a spherical face carries a pole inside its own v \
             domain, so its domain cannot be bounded from its boundary loop \
             and must be left unmeasured"
        );
        assert!(
            model.query_face(fid).expect("cap report").area.is_none(),
            "face {fid}: an unmeasured spherical cap must refuse its area"
        );
    }
}

// ─── planar fragments, and the anchor that must lie on the face ─────────────

/// The mean of a face's own boundary vertices, and the bounding box those
/// vertices span. Computed from the loop, independently of anything the face
/// says about its parametric domain.
fn boundary_stats(model: &BRepModel, fid: u32) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let face = model.faces.get(fid).expect("face");
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let mut sum = [0.0_f64; 3];
    let mut n = 0.0_f64;
    let mut lids = vec![face.outer_loop];
    lids.extend_from_slice(&face.inner_loops);
    for lid in lids {
        let lp = model.loops.get(lid).expect("loop");
        for &eid in &lp.edges {
            let e = model.edges.get(eid).expect("edge");
            for vid in [e.start_vertex, e.end_vertex] {
                let p = model.vertices.get(vid).expect("vertex").position;
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                    sum[k] += p[k];
                }
                n += 1.0;
            }
        }
    }
    assert!(n > 0.0, "face {fid} has no boundary vertices");
    ([sum[0] / n, sum[1] / n, sum[2] / n], lo, hi)
}

/// A boolean's PLANAR fragments carry a measured domain too.
///
/// The pole guard used to compute its probe period with `?` before asking
/// whether there was anywhere to probe, and an unbounded plane declares no
/// finite `u` extent — so the measurement returned `None` at that line for a
/// surface that has no pole and nothing to guard against. Every planar fragment
/// a boolean minted was left unmeasured, and every consumer that needs a point
/// on such a face had nothing to work from.
#[test]
fn boolean_minted_planar_fragments_are_measured() {
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);
    let slot = seam_slot(&mut model, CYL_H);
    let cut = boolean_operation(
        &mut model,
        cyl,
        slot,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let planes = faces_of_kind(&model, cut, "Plane");
    assert!(!planes.is_empty(), "the cut mints planar faces");
    for fid in planes {
        assert!(
            model
                .faces
                .get(fid)
                .expect("planar face")
                .uv_bounds_are_measured(),
            "planar fragment {fid} must carry a measured domain — a plane has \
             no pole, so there is nothing for the guard to refuse over"
        );
    }
}

/// `FaceInfo.anchor` is a point ON the face, not a spot in the carrier surface's
/// own frame.
///
/// A plane's outward normal and its curvature are the same everywhere, so the
/// `[0, 1]²` placeholder cannot corrupt them — but a POINT is position-dependent
/// by definition, and `(0.5, 0.5)` in the carrier plane's frame need not lie
/// within a trimmed fragment at all. Measured on this fixture: the window's side
/// walls carry `[-4, 4] × [5.46, 15]`, so the placeholder's `v = 0.5` is outside
/// the fragment entirely, and the anchor served was a point in space the face
/// does not contain.
///
/// Asserted over EVERY planar face of the cut, not one: some fragments happen to
/// contain `(0.5, 0.5)` and would pass either way.
#[test]
fn face_info_anchor_lies_on_the_face_it_describes() {
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);
    let slot = seam_slot(&mut model, CYL_H);
    let cut = boolean_operation(
        &mut model,
        cyl,
        slot,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    let planes = faces_of_kind(&model, cut, "Plane");
    assert!(!planes.is_empty(), "the cut mints planar faces");
    for fid in planes {
        let (_, lo, hi) = boundary_stats(&model, fid);
        let subj = MeasureSubject::Face {
            solid: cut,
            face: fid,
        };
        let anchor = match measure(&mut model, subj, None).expect("single-face measure") {
            MeasureResult::FaceInfo { anchor, .. } => anchor,
            other => panic!("a planar face measures as FaceInfo, got {other:?}"),
        };
        for k in 0..3 {
            assert!(
                anchor[k] >= lo[k] - 1e-6 && anchor[k] <= hi[k] + 1e-6,
                "face {fid}: anchor {anchor:?} is outside the face's own \
                 boundary box [{lo:?}, {hi:?}] on axis {k}"
            );
        }
    }
}

/// The same contract on an UNMEASURED planar face: a primitive box side.
///
/// Its domain was never measured, so there is no parametric midpoint to serve —
/// and the honest anchor is the one measured from the boundary, which for a
/// square is its centre. Before this change the carrier plane's `(0.5, 0.5)`
/// was served instead: the box-face plane's origin is a CORNER, so the anchor
/// sat 0.5 mm in from it rather than at the centre of a 20 mm face.
#[test]
fn unmeasured_planar_face_anchors_on_its_boundary_not_its_carrier_frame() {
    const SIDE: f64 = 20.0;
    let mut model = BRepModel::new();
    let bx = {
        let mut b = TopologyBuilder::new(&mut model);
        match b.create_box_3d(SIDE, SIDE, SIDE).expect("box") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };

    let planes = faces_of_kind(&model, bx, "Plane");
    assert_eq!(planes.len(), 6, "a box has six planar faces");
    for &fid in &planes {
        unmeasure(&mut model, fid);
    }
    for fid in planes {
        let (mean, _, _) = boundary_stats(&model, fid);
        let subj = MeasureSubject::Face {
            solid: bx,
            face: fid,
        };
        let anchor = match measure(&mut model, subj, None).expect("single-face measure") {
            MeasureResult::FaceInfo { anchor, .. } => anchor,
            other => panic!("a planar face measures as FaceInfo, got {other:?}"),
        };
        for k in 0..3 {
            assert!(
                (anchor[k] - mean[k]).abs() <= 1e-9,
                "face {fid}: anchor {anchor:?} is not the boundary mean {mean:?}"
            );
        }
    }
}
