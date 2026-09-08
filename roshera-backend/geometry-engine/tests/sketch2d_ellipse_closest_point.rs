// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::float_cmp)]

//! Task 49 -- `Ellipse2d::closest_point` must return the foot of the
//! perpendicular from the cursor, or refuse.
//!
//! Task 39 measured `SnapKind::OnEllipse` firing ZERO times across 2880
//! cursor positions around a 6x3 ellipse. This file's own grid measures
//! 16 of 2880 on the same code -- the four axis crossings, where the
//! `atan2` first guess is already the answer and the loop breaks before
//! it can do damage, times the four radial offsets. Both numbers say
//! the same thing: the on-boundary snap does not work.
//!
//! The cause is the Newton step in `Ellipse2d::closest_point`: it
//! iterates on
//! `f(t) = (P - E(t)) . E'(t)` while supplying
//! `+|E'|^2 + (P - E) . E''` as `f'(t)`, where the true derivative is
//! `-|E'|^2 + (P - E) . E''`. The dominant term carries the wrong sign,
//! so every step walks AWAY from the root; after ten iterations the
//! function hands back a point on the far side of the ellipse. It is
//! then further from the cursor than the snap radius, so
//! `find_snap_candidates` silently drops the candidate and the on-curve
//! snap never fires.
//!
//! # How the probes are built (do not "simplify" this)
//!
//! A cursor built by RADIAL scaling, `k * E(t)`, is not on the normal
//! at `E(t)`: for k = 1.01 at t = pi/4 on a 6x3 ellipse the true foot
//! sits near t = 0.7795, not 0.7854. Radial probes are therefore used
//! only for the COUNT -- they stay within 0.12 of the boundary, well
//! inside the 0.5 snap radius, so a correct solver must report every
//! one of them.
//!
//! Every probe that asserts WHICH point comes back is built as
//! `E(t) + d * n(t)` with `n` the outward unit normal, whose foot is
//! `E(t)` exactly -- provided `|d|` stays below the minimum radius of
//! curvature `b^2/a` (1.5 for 6x3). Past that the centre of curvature
//! is crossed and `E(t)` genuinely stops being the nearest point, so a
//! correct solver would fail an assertion written that way.

use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::snap::SnapKind;
use geometry_engine::sketch2d::{Circle2d, Ellipse2d, Point2d};
use std::f64::consts::TAU;

/// Recover the ellipse parameter of a point that lies ON the ellipse,
/// by inverting `evaluate`: undo the rotation and the axis scaling,
/// then read the angle off the resulting unit circle.
fn parameter_of(e: &Ellipse2d, p: &Point2d) -> f64 {
    let dx = p.x - e.center.x;
    let dy = p.y - e.center.y;
    let (cos_r, sin_r) = (e.rotation.cos(), e.rotation.sin());
    let x_local = dx * cos_r + dy * sin_r;
    let y_local = -dx * sin_r + dy * cos_r;
    (y_local / e.semi_minor).atan2(x_local / e.semi_major)
}

/// Signed difference between two angles, wrapped to (-pi, pi].
fn angle_delta(a: f64, b: f64) -> f64 {
    let mut d = (a - b) % TAU;
    if d > std::f64::consts::PI {
        d -= TAU;
    } else if d < -std::f64::consts::PI {
        d += TAU;
    }
    d
}

fn sketch_with(e: &Ellipse2d, tag: &str) -> Sketch {
    let sketch = Sketch::new(tag.to_string(), SketchAnchor::xy());
    sketch
        .add_ellipse(e.center, e.semi_major, e.semi_minor, e.rotation)
        .unwrap();
    sketch
}

/// The measurement Task 39 made, restated as an assertion: 720
/// parametric angles x 4 radial offsets around a 6x3 ellipse, every
/// cursor at most 0.12 from the boundary, snap radius 0.5. A solver
/// that converges reports an `OnEllipse` candidate for every one of
/// them; the Newton iteration on HEAD reports none.
#[test]
fn on_ellipse_snap_fires_at_every_probe_around_a_six_by_three_ellipse() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();
    let sketch = sketch_with(&e, "task49_probe_grid");

    let mut probes = 0usize;
    let mut fired = 0usize;
    let mut first_miss: Option<(f64, f64, f64)> = None;
    for i in 0..720 {
        let t = (i as f64) * TAU / 720.0;
        let on = e.evaluate(t);
        for k in [0.98_f64, 0.99, 1.01, 1.02] {
            let cursor = Point2d::new(on.x * k, on.y * k);
            probes += 1;
            match sketch
                .find_snap_candidates(cursor, 0.5)
                .into_iter()
                .find(|c| c.kind == SnapKind::OnEllipse)
            {
                Some(_) => fired += 1,
                None => {
                    if first_miss.is_none() {
                        first_miss = Some((t, k, cursor.distance_to(&on)));
                    }
                }
            }
        }
    }

    assert_eq!(probes, 2880, "the probe grid itself must be 2880 positions");
    assert_eq!(
        fired, 2880,
        concat!(
            "every cursor in the grid sits at most 0.12 from the ellipse, ",
            "inside the 0.5 snap radius, so OnEllipse must fire for all ",
            "2880; first miss at (t, k, distance-to-boundary) = {:?}"
        ),
        first_miss
    );
}

/// For a cursor on the outward normal at `t`, the nearest point on the
/// ellipse IS `E(t)`. The snap radius here is deliberately huge (100)
/// so the candidate is never filtered out by distance: this test is
/// about WHICH point comes back, not whether it is near.
fn assert_normal_foot(e: &Ellipse2d, tag: &str) {
    let sketch = sketch_with(e, tag);
    for i in 0..16 {
        let t = (i as f64) * TAU / 16.0;
        let on = e.evaluate(t);
        let n = e.normal(t).normalize().unwrap();
        for d in [-0.4_f64, -0.05, 0.05, 0.4] {
            let cursor = on.add_vector(&n.scale(d));
            let foot = sketch
                .find_snap_candidates(cursor, 100.0)
                .into_iter()
                .find(|c| c.kind == SnapKind::OnEllipse)
                .unwrap_or_else(|| panic!("{tag}: no OnEllipse candidate at t={t}, d={d}"));
            assert!(
                foot.point.distance_to(&on) < 1e-9,
                concat!(
                    "{}: cursor {:?} is {} along the outward normal at t={}, ",
                    "so the foot is {:?} at distance {}; got {:?} at distance {}"
                ),
                tag,
                cursor,
                d,
                t,
                on,
                d.abs(),
                foot.point,
                foot.distance
            );
            let got_t = parameter_of(e, &foot.point);
            assert!(
                angle_delta(got_t, t).abs() < 1e-9,
                "{}: expected parameter {}, recovered {}",
                tag,
                t,
                got_t
            );
        }
    }
}

#[test]
fn on_ellipse_foot_is_the_normal_foot_axis_aligned() {
    let e = Ellipse2d::axis_aligned(Point2d::new(2.0, -1.0), 6.0, 3.0).unwrap();
    assert_normal_foot(&e, "task49_axis_aligned");
}

#[test]
fn on_ellipse_foot_is_the_normal_foot_rotated() {
    let e = Ellipse2d::new(Point2d::new(-3.0, 4.0), 6.0, 3.0, 0.7).unwrap();
    assert_normal_foot(&e, "task49_rotated");
}

/// The mechanism, measured rather than argued. `f(t) = (P - E) . E'`
/// has derivative `-|E'|^2 + (P - E) . E''`; the iteration on HEAD
/// uses `+|E'|^2 + (P - E) . E''`. At the iteration's own starting
/// guess for a 6x3 ellipse and P = 1.01 * E(pi/4) the two differ by
/// `2|E'|^2 = 28.8` AND in sign, so the first step moves away from the
/// root instead of towards it.
#[test]
fn the_derivative_of_the_dot_product_carries_a_negative_tangent_norm() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();
    let on = e.evaluate(std::f64::consts::FRAC_PI_4);
    let p = Point2d::new(on.x * 1.01, on.y * 1.01);

    // The initial guess the iteration makes: atan2 of the cursor in
    // the ellipse's local frame (the rotation is zero here). At
    // t = pi/4 the local coordinates are (6c, 3c), so the guess is
    // exactly atan(1/2) -- 0.4636, a long way from the true foot.
    let t0 = p.y.atan2(p.x);
    assert!(
        (t0 - 0.5_f64.atan()).abs() < 1e-15,
        "the initial guess is atan(1/2), got {t0}"
    );

    let f = |t: f64| {
        let e_t = e.evaluate(t);
        let tangent = e.tangent(t);
        (p.x - e_t.x) * tangent.x + (p.y - e_t.y) * tangent.y
    };

    let h = 1e-6;
    let finite_difference = (f(t0 + h) - f(t0 - h)) / (2.0 * h);

    let tangent = e.tangent(t0);
    let e_t = e.evaluate(t0);
    let (ddx, ddy) = (-e.semi_major * t0.cos(), -e.semi_minor * t0.sin());
    let tangent_norm_sq = tangent.x * tangent.x + tangent.y * tangent.y;
    let curvature_term = (p.x - e_t.x) * ddx + (p.y - e_t.y) * ddy;

    let correct = -tangent_norm_sq + curvature_term;
    let as_coded = tangent_norm_sq + curvature_term;

    assert!(
        (finite_difference - correct).abs() < 1e-4,
        "measured f'(t0) = {finite_difference}, -|E'|^2 + (P-E).E'' = {correct}"
    );
    assert!(
        (finite_difference - as_coded).abs() > 28.0,
        "measured f'(t0) = {finite_difference}, +|E'|^2 + (P-E).E'' = {as_coded}"
    );
    assert!(
        f(t0) > 0.0 && correct < 0.0 && as_coded > 0.0,
        concat!(
            "f(t0) = {}, so a Newton step with the correct derivative {} ",
            "increases t towards the root while the coded {} decreases it"
        ),
        f(t0),
        correct,
        as_coded
    );
}

/// The claim `closest_point` makes is "no point of the ellipse is
/// nearer", so certify it against a dense sweep of the curve rather
/// than against a formula that could share the solver's own mistake.
/// Sampling can only under-state the true minimum, so this direction of
/// the inequality never fails spuriously.
fn assert_no_sample_is_nearer(e: &Ellipse2d, tag: &str) {
    const SAMPLES: usize = 4096;
    for i in 0..40 {
        let a = (i as f64) * TAU / 40.0;
        for reach in [0.0_f64, 0.3, 1.0, 4.0, 40.0] {
            let cursor = Point2d::new(
                e.center.x + reach * a.cos(),
                e.center.y + reach * a.sin() * 0.5,
            );
            let foot = e.closest_point(&cursor).unwrap();
            let got = cursor.distance_to(&foot);
            let mut best = f64::INFINITY;
            for k in 0..SAMPLES {
                let t = (k as f64) * TAU / (SAMPLES as f64);
                best = best.min(cursor.distance_to(&e.evaluate(t)));
            }
            assert!(
                got <= best + 1e-9,
                "{}: cursor {:?} -- foot {:?} is {} away, but a sample of the curve is {}",
                tag,
                cursor,
                foot,
                got,
                best
            );
            // The foot must also lie ON the ellipse, not merely near
            // the cursor: the implicit residual is the certificate.
            let dx = foot.x - e.center.x;
            let dy = foot.y - e.center.y;
            let (cos_r, sin_r) = (e.rotation.cos(), e.rotation.sin());
            let xl = (dx * cos_r + dy * sin_r) / e.semi_major;
            let yl = (-dx * sin_r + dy * cos_r) / e.semi_minor;
            assert!(
                (xl * xl + yl * yl - 1.0).abs() < 1e-12,
                "{}: foot {:?} is not on the ellipse (residual {})",
                tag,
                foot,
                xl * xl + yl * yl - 1.0
            );
        }
    }
}

#[test]
fn no_point_of_the_ellipse_is_nearer_than_the_foot() {
    assert_no_sample_is_nearer(
        &Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap(),
        "axis_aligned_6x3",
    );
    assert_no_sample_is_nearer(
        &Ellipse2d::new(Point2d::new(1.5, -2.5), 6.0, 3.0, 1.1).unwrap(),
        "rotated_6x3",
    );
    assert_no_sample_is_nearer(
        &Ellipse2d::axis_aligned(Point2d::new(-4.0, 0.5), 40.0, 0.25).unwrap(),
        "sliver_160to1",
    );
    assert_no_sample_is_nearer(
        &Ellipse2d::circle(Point2d::new(2.0, 2.0), 5.0).unwrap(),
        "circle_r5",
    );
}

/// A point already on the boundary is its own foot.
#[test]
fn a_point_on_the_ellipse_is_its_own_closest_point() {
    let e = Ellipse2d::new(Point2d::new(0.5, 0.25), 6.0, 3.0, 0.4).unwrap();
    for i in 0..64 {
        let t = (i as f64) * TAU / 64.0;
        let on = e.evaluate(t);
        let foot = e.closest_point(&on).unwrap();
        assert!(
            foot.distance_to(&on) < 1e-9,
            "t={t}: {on:?} moved to {foot:?}"
        );
    }
}

/// The centre of a PROPER ellipse is equidistant from both minor-axis
/// endpoints. Both are genuinely closest; the kernel returns the
/// `+minor` one, and the distance -- the part that is not a choice --
/// must be the semi-minor axis.
#[test]
fn the_centre_of_a_proper_ellipse_resolves_to_a_minor_axis_endpoint() {
    let e = Ellipse2d::new(Point2d::new(-2.0, 7.0), 6.0, 3.0, 0.9).unwrap();
    let foot = e.closest_point(&e.center).unwrap();
    assert!(
        (foot.distance_to(&e.center) - e.semi_minor).abs() < 1e-12,
        "the nearest boundary point to the centre is one semi-minor away, got {}",
        foot.distance_to(&e.center)
    );
    let expected = e.evaluate(std::f64::consts::FRAC_PI_2);
    assert!(
        foot.distance_to(&expected) < 1e-12,
        "documented choice is the +minor endpoint {expected:?}, got {foot:?}"
    );
}

/// The centre of a CIRCULAR ellipse is equidistant from the whole
/// boundary. The documented choice is parameter 0 -- the same choice
/// `Circle2d::closest_point` makes, so the two agree.
#[test]
fn the_centre_of_a_circular_ellipse_resolves_the_way_circle2d_does() {
    let centre = Point2d::new(3.0, -1.0);
    let e = Ellipse2d::circle(centre, 4.0).unwrap();
    let foot = e.closest_point(&centre).unwrap();
    let circle = Circle2d::new(centre, 4.0).unwrap();
    let sibling = circle.closest_point(&centre);
    assert!(
        foot.distance_to(&sibling) < 1e-12,
        "Ellipse2d gave {foot:?}, Circle2d gave {sibling:?}"
    );
    assert!(foot.distance_to(&e.evaluate(0.0)) < 1e-12);
}

/// The public fields let an ellipse exist with the LONGER axis in
/// `semi_minor` -- `ParametricEllipse2d::transform` produces exactly
/// that under a non-uniform scale. The reduction requires the longer
/// axis first, so the roles are sorted internally; the answer must
/// match the same curve built through the constructor, which swaps the
/// axes and adds a quarter turn instead.
#[test]
fn an_ellipse_whose_minor_field_holds_the_longer_axis_still_solves() {
    let centre = Point2d::new(1.0, 2.0);
    let inverted = Ellipse2d {
        center: centre,
        semi_major: 3.0,
        semi_minor: 6.0,
        rotation: 0.0,
    };
    let normalised = Ellipse2d::new(centre, 3.0, 6.0, 0.0).unwrap();
    assert_eq!(normalised.semi_major, 6.0, "the constructor sorts the axes");

    let mut cursors: Vec<Point2d> = Vec::new();
    for i in 0..32 {
        let a = (i as f64) * TAU / 32.0;
        cursors.push(Point2d::new(
            centre.x + 9.0 * a.cos(),
            centre.y + 9.0 * a.sin(),
        ));
    }
    // Hug the SHORT axis. This is where the ordering earns its keep:
    // solved with the axes the wrong way round, the ratio
    // `r0 = (e0/e1)^2` drops below 1 and the root function grows a pole
    // at `w = 1 - r0`, inside the interval the bracket has to search. A
    // cursor with a large offset never reaches down there; one a
    // whisker off the axis starts the bracket below it.
    for dy in [1e-9_f64, 1e-6, 1e-3, 0.05, 0.4] {
        for dx in [-20.0_f64, -5.0, -2.99, -0.5, 0.5, 2.99, 5.0, 20.0] {
            cursors.push(Point2d::new(centre.x + dx, centre.y + dy));
            cursors.push(Point2d::new(centre.x + dy, centre.y + dx));
        }
    }

    for cursor in cursors {
        let from_fields = inverted.closest_point(&cursor).unwrap();
        let from_constructor = normalised.closest_point(&cursor).unwrap();
        assert!(
            from_fields.distance_to(&from_constructor) < 1e-9,
            "{cursor:?}: inverted fields gave {from_fields:?}, constructor gave {from_constructor:?}"
        );
        // Both must be ON the curve and no farther than any sample of
        // it -- agreeing on a wrong answer is still wrong.
        let mut best = f64::INFINITY;
        for k in 0..4096 {
            let t = (k as f64) * TAU / 4096.0;
            best = best.min(cursor.distance_to(&normalised.evaluate(t)));
        }
        assert!(
            cursor.distance_to(&from_fields) <= best + 1e-9,
            "{cursor:?}: foot {from_fields:?} is {} away, a sample is {best}",
            cursor.distance_to(&from_fields)
        );
    }
}

/// A cursor on the major axis inside the evolute cusp
/// (`(a^2 - b^2)/a`, here 4.5) has its foot OFF the axis; outside it
/// the foot is the major vertex. Getting this arm wrong is invisible
/// to a normal-offset probe, because the cusp is where the normals
/// meet.
#[test]
fn a_cursor_on_the_major_axis_crosses_the_evolute_cusp() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();
    let cusp: f64 = (6.0 * 6.0 - 3.0 * 3.0) / 6.0;
    assert!((cusp - 4.5).abs() < 1e-15);

    let inside = e.closest_point(&Point2d::new(3.0, 0.0)).unwrap();
    assert!(
        inside.y.abs() > 1e-6,
        "inside the cusp the foot leaves the axis, got {inside:?}"
    );
    let on_curve = (inside.x / 6.0).powi(2) + (inside.y / 3.0).powi(2);
    assert!((on_curve - 1.0).abs() < 1e-12);

    let outside = e.closest_point(&Point2d::new(5.0, 0.0)).unwrap();
    assert!(
        outside.distance_to(&Point2d::new(6.0, 0.0)) < 1e-12,
        "outside the cusp the foot is the major vertex, got {outside:?}"
    );
}

/// A cursor exactly ON the minor axis takes its own arm of the
/// reduction -- the one the bisection never sees, because the root
/// function needs both local coordinates non-zero. The foot is the
/// nearer minor vertex, on both sides and from inside as well as out
/// (the minor axis' centre of curvature, at `(b^2 - a^2)/b = -9` here,
/// is off the curve entirely, so no cursor on this axis has a foot
/// anywhere else).
#[test]
fn a_cursor_on_the_minor_axis_takes_the_nearer_minor_vertex() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();
    for (y, expected) in [
        (8.0, 3.0),
        (3.0, 3.0),
        (0.5, 3.0),
        (-0.5, -3.0),
        (-3.0, -3.0),
        (-8.0, -3.0),
    ] {
        let foot = e.closest_point(&Point2d::new(0.0, y)).unwrap();
        assert!(
            foot.distance_to(&Point2d::new(0.0, expected)) < 1e-12,
            "cursor (0, {y}): expected (0, {expected}), got {foot:?}"
        );
    }
}

/// The refusals. A cursor that is not a position, and an ellipse the
/// public fields have left degenerate, get a typed error -- not a
/// point that would read as an answer.
#[test]
fn closest_point_refuses_rather_than_returning_a_point_it_cannot_justify() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();

    for bad in [
        Point2d::new(f64::NAN, 0.0),
        Point2d::new(0.0, f64::NAN),
        Point2d::new(f64::INFINITY, 1.0),
    ] {
        let err = e
            .closest_point(&bad)
            .expect_err("a non-finite cursor has no foot");
        let msg = err.to_string();
        assert!(msg.ends_with(", must be finite"), "{msg}");
        assert!(!msg.contains("must be must be"), "stuttered message: {msg}");
        assert!(!msg.contains("  "), "message has doubled spaces: {msg}");
    }

    let degenerate = Ellipse2d {
        center: Point2d::ORIGIN,
        semi_major: 6.0,
        semi_minor: 0.0,
        rotation: 0.0,
    };
    let err = degenerate
        .closest_point(&Point2d::new(1.0, 1.0))
        .expect_err("a zero semi-axis is not an ellipse");
    let msg = err.to_string();
    assert!(msg.contains("finite and positive"), "{msg}");
    assert!(!msg.contains("must be must be"), "stuttered message: {msg}");
    assert!(!msg.contains("  "), "message has doubled spaces: {msg}");

    let unplaced = Ellipse2d {
        center: Point2d::new(f64::NAN, 0.0),
        semi_major: 6.0,
        semi_minor: 3.0,
        rotation: 0.0,
    };
    let err = unplaced
        .closest_point(&Point2d::new(1.0, 1.0))
        .expect_err("an ellipse with no centre has no foot");
    let msg = err.to_string();
    assert!(msg.contains("must be finite"), "{msg}");
    assert!(!msg.contains("must be must be"), "stuttered message: {msg}");
    assert!(!msg.contains("  "), "message has doubled spaces: {msg}");
}

/// A refused ellipse withholds the on-boundary candidate and keeps the
/// discrete ones: the snap list never carries a foot the kernel could
/// not compute.
#[test]
fn a_non_finite_cursor_yields_no_on_ellipse_candidate() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();
    let sketch = sketch_with(&e, "task49_refusal");
    let candidates = sketch.find_snap_candidates(Point2d::new(f64::NAN, 0.0), 100.0);
    assert!(
        !candidates.iter().any(|c| c.kind == SnapKind::OnEllipse),
        "got {candidates:?}"
    );
}

/// The bracket certifies a SIGN CHANGE. It does not certify that the
/// arithmetic building it stayed inside the representable range, and
/// three inputs prove the difference -- each returned a confident,
/// wrong point before this task's second round.
///
/// Two of them are caught by the bracket's own finiteness check and one
/// by the on-curve residual on the finished foot, and the test asserts
/// WHICH, because the two guards fail differently and a future change
/// that loses one must not be covered by the other:
///
/// * (1e200, 1e200) on a 6x3 ellipse overflows `sqrt(n0^2 + z1^2)` to
///   `+inf`. `F(inf) = -1 <= 0` satisfies the upper-end sign test, and
///   the first bisection midpoint `0.5*(lower + inf)` IS `inf`, equal
///   to the endpoint, so the loop "converges" and the foot comes back
///   as the ellipse CENTRE -- implicit residual -1.0. Bracket
///   `[3.33e199, inf]`, refused.
/// * the same overflow at an aspect ratio of 1e140 gives bracket
///   `[1, inf]` and produced a NaN foot. Refused.
/// * subnormal coordinates fail the other way: the bracket is a finite
///   interval and the foot is finite and plausible, but it sits 0.0039
///   off the boundary. Only the residual check sees this one.
///
/// The boundary between "answer" and "refusal" is the residual, not a
/// magnitude, so the largest cursor that still WORKS is pinned too. A
/// future change may legitimately move that boundary; it may not make
/// either side silent.
#[test]
fn closest_point_refuses_a_foot_the_arithmetic_could_not_keep_on_the_curve() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();

    let ok = e
        .closest_point(&Point2d::new(1e154, 1e154))
        .expect("1e154 is inside the range the bracket can be built in");
    let residual = (ok.x / 6.0).powi(2) + (ok.y / 3.0).powi(2) - 1.0;
    assert!(
        residual.abs() < 1e-9,
        "the largest working cursor must still land ON the curve, residual {residual}"
    );

    let err = e
        .closest_point(&Point2d::new(1e200, 1e200))
        .expect_err("an overflowed bracket has no certified foot");
    let msg = err.to_string();
    assert!(msg.contains("is not usable after 0 widenings"), "{msg}");
    assert!(msg.contains(", inf]"), "the upper end overflowed: {msg}");
    assert!(!msg.contains("  "), "message has doubled spaces: {msg}");

    let extreme = Ellipse2d {
        center: Point2d::ORIGIN,
        semi_major: 1e140,
        semi_minor: 1.0,
        rotation: 0.0,
    };
    let err = extreme
        .closest_point(&Point2d::new(5e139, 1.0))
        .expect_err("a NaN foot is not an answer");
    let msg = err.to_string();
    assert!(msg.contains("is not usable after 0 widenings"), "{msg}");
    assert!(msg.contains("[1, inf]"), "{msg}");

    let err = e
        .closest_point(&Point2d::new(1e-320, 1e-320))
        .expect_err("a subnormal cursor foot lands off the curve");
    let msg = err.to_string();
    assert!(
        msg.contains("is not on the ellipse"),
        "the residual check, not the bracket check, must catch this: {msg}"
    );
    assert!(msg.contains("implicit residual -0.0039"), "{msg}");
    assert!(!msg.contains("  "), "message has doubled spaces: {msg}");
}

/// A refusal names the number of widenings it actually performed. The
/// exits that widen nothing must say zero -- printing the budget
/// constant there reports 2100 for a bracket rejected on sight, which
/// describes work the solver never did.
#[test]
fn a_bracket_refusal_reports_the_widenings_it_actually_did() {
    let e = Ellipse2d::axis_aligned(Point2d::ORIGIN, 6.0, 3.0).unwrap();
    let err = e
        .closest_point(&Point2d::new(1e200, 1e200))
        .expect_err("an overflowed bracket has no certified foot");
    let msg = err.to_string();
    assert!(
        msg.contains("after 0 widenings"),
        "the finiteness check runs before any widening: {msg}"
    );
    assert!(
        !msg.contains("after 2100 widenings"),
        "the budget constant is not a count: {msg}"
    );
}
