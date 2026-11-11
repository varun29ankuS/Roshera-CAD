//! Contacts between the curves of a sketch profile (SK-C3).
//!
//! A profile is the set of curves sketch topology builds loops from: every
//! non-construction line segment, arc, circle, rectangle edge, polyline
//! segment, spline and ellipse. Two of them may meet only where they JOIN —
//! at a point that is an endpoint of both. Any other contact is a defect the
//! certificate must report: a crossing, an endpoint resting on another
//! curve's interior (a T), or a stretch where two curves coincide.
//!
//! How each pair is decided:
//!
//! * **line / arc / circle** pairs are closed-form (the carriers are lines and
//!   circles), with collinear and co-circular overlap measured exactly.
//! * **anything against an ellipse, or a spline against a line, arc or
//!   circle**: one curve has an implicit equation, the other is cut into
//!   rational Bézier pieces (an ellipse is four rational quadratics, exactly;
//!   a spline is knot-inserted to Bézier form). Substituting the pieces into
//!   the implicit gives a polynomial in Bernstein form whose coefficient
//!   signs PROVE an interval free of contact (convex hull), and whose roots
//!   are the contacts. A known join at a piece end is a known root: it is
//!   divided out — twice for a tangent join — so the join itself never reads
//!   as a contact and a second, real contact near it still does.
//! * **spline against spline**, and a spline against itself: recursive
//!   subdivision with convex-hull boxes. Pieces that meet at a join are
//!   separated by the angular sectors their control points occupy around it.
//!
//! Nothing reads "free" without a proof. A pair the machinery cannot decide
//! within its work budget, or a curve it cannot convert, is reported
//! UNCHECKED, and the certificate treats unchecked as not free.

use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_1_SQRT_2, PI, TAU};

use super::line2d::LineGeometry;
use super::spline2d::Spline2d;
use super::{Ellipse2d, EntityRef, Sketch};

/// Two profile curves closer than this touch; two endpoints closer than this
/// are one join. The same distance sketch topology closes loops at.
pub const PROFILE_CONTACT_TOLERANCE: f64 = 1e-6;

/// Subdivision steps one spline pair (or one spline against itself) may take
/// before it is reported unchecked.
const SPLINE_PAIR_BUDGET: usize = 200_000;

/// Interval-halving depth for Bernstein root isolation and subdivision.
const MAX_DEPTH: u32 = 60;

/// Root-isolation steps one polynomial may take before it is unchecked.
const POLY_BUDGET: usize = 100_000;

/// What kind of contact two profile curves make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactKind {
    /// The curves meet at a point interior to both (transversal or tangent).
    Crossing,
    /// An endpoint of one curve rests on the interior of the other.
    EndOnInterior,
    /// The curves coincide over a stretch longer than the tolerance.
    Overlap,
}

/// One contact between two profile curves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileContact {
    /// The first curve (an entity, or an edge / segment of one).
    pub first: String,
    /// The second curve (may equal `first` for a spline crossing itself).
    pub second: String,
    /// Crossing, endpoint-on-interior, or overlap.
    pub kind: ContactKind,
    /// Where (a point of the overlap, for an overlap).
    pub point: [f64; 2],
}

/// A pair of curves the check could not decide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UncheckedPair {
    /// The first curve.
    pub first: String,
    /// The second curve.
    pub second: String,
    /// Why it could not be decided.
    pub reason: String,
}

/// Every contact between profile curves, and every pair left undecided.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileContactReport {
    /// Contacts other than joins.
    pub contacts: Vec<ProfileContact>,
    /// Pairs that could not be decided; never read as free.
    pub unchecked: Vec<UncheckedPair>,
}

impl ProfileContactReport {
    /// No contact found AND every pair decided.
    pub fn is_free(&self) -> bool {
        self.contacts.is_empty() && self.unchecked.is_empty()
    }
}

type P = [f64; 2];
/// Homogeneous control point `(w·x, w·y, w)`.
type H = [f64; 3];

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: P, b: P) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn norm(a: P) -> f64 {
    a[0].hypot(a[1])
}

/// Angle `a` wrapped into `(-π, π]`.
fn wrap_pi(a: f64) -> f64 {
    let w = (a + PI).rem_euclid(TAU) - PI;
    if w <= -PI {
        w + TAU
    } else {
        w
    }
}

fn binomial(n: usize, k: usize) -> f64 {
    if k > n {
        return 0.0;
    }
    let k = k.min(n - k);
    let mut r = 1.0;
    for i in 0..k {
        r = r * (n - i) as f64 / (i + 1) as f64;
    }
    r
}

// ── Rational Bézier pieces ───────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Bez {
    cp: Vec<H>,
}

impl Bez {
    fn proj(h: &H) -> P {
        [h[0] / h[2], h[1] / h[2]]
    }

    fn start(&self) -> P {
        Self::proj(&self.cp[0])
    }

    fn end(&self) -> P {
        Self::proj(&self.cp[self.cp.len() - 1])
    }

    fn points(&self) -> Vec<P> {
        self.cp.iter().map(Self::proj).collect()
    }

    /// Scale the weights so the largest is 1 (same curve).
    fn normalized(mut self) -> Self {
        let wmax = self.cp.iter().map(|h| h[2]).fold(0.0_f64, f64::max);
        if wmax > 0.0 {
            for h in &mut self.cp {
                for c in h.iter_mut() {
                    *c /= wmax;
                }
            }
        }
        self
    }

    fn eval(&self, t: f64) -> P {
        let mut v = self.cp.clone();
        let n = v.len();
        for r in 1..n {
            for i in 0..n - r {
                for c in 0..3 {
                    v[i][c] = (1.0 - t) * v[i][c] + t * v[i + 1][c];
                }
            }
        }
        Self::proj(&v[0])
    }

    fn split(&self, t: f64) -> (Bez, Bez) {
        let n = self.cp.len();
        let mut v = self.cp.clone();
        let mut left = Vec::with_capacity(n);
        let mut right = vec![[0.0; 3]; n];
        left.push(v[0]);
        right[n - 1] = v[n - 1];
        for r in 1..n {
            for i in 0..n - r {
                for c in 0..3 {
                    v[i][c] = (1.0 - t) * v[i][c] + t * v[i + 1][c];
                }
            }
            left.push(v[0]);
            right[n - 1 - r] = v[n - 1 - r];
        }
        (Bez { cp: left }, Bez { cp: right })
    }

    fn bbox(&self) -> (P, P) {
        bbox_of(&self.points())
    }
}

fn bbox_of(points: &[P]) -> (P, P) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in points {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

fn boxes_meet(a: &(P, P), b: &(P, P), pad: f64) -> bool {
    (0..2).all(|i| a.0[i] <= b.1[i] + pad && b.0[i] <= a.1[i] + pad)
}

fn diag(b: &(P, P)) -> f64 {
    dist(b.0, b.1)
}

/// An ellipse as four rational quadratic Bézier quarters (exact).
fn ellipse_pieces(e: &Ellipse2d) -> Vec<Bez> {
    let (s, c) = e.rotation.sin_cos();
    let u = [c * e.semi_major, s * e.semi_major];
    let v = [-s * e.semi_minor, c * e.semi_minor];
    let ctr = [e.center.x, e.center.y];
    let at = |a: f64, b: f64| [ctr[0] + a * u[0] + b * v[0], ctr[1] + a * u[1] + b * v[1]];
    let quarters = [
        ((1.0, 0.0), (1.0, 1.0), (0.0, 1.0)),
        ((0.0, 1.0), (-1.0, 1.0), (-1.0, 0.0)),
        ((-1.0, 0.0), (-1.0, -1.0), (0.0, -1.0)),
        ((0.0, -1.0), (1.0, -1.0), (1.0, 0.0)),
    ];
    quarters
        .iter()
        .map(|&(p0, p1, p2)| {
            let a = at(p0.0, p0.1);
            let m = at(p1.0, p1.1);
            let b = at(p2.0, p2.1);
            let w = FRAC_1_SQRT_2;
            Bez {
                cp: vec![
                    [a[0], a[1], 1.0],
                    [m[0] * w, m[1] * w, w],
                    [b[0], b[1], 1.0],
                ],
            }
        })
        .collect()
}

/// Insert knot `u` once (Boehm), on homogeneous control points.
fn insert_knot(
    knots: &mut Vec<f64>,
    pw: &mut Vec<H>,
    p: usize,
    u: f64,
    at_start: bool,
) -> Result<(), String> {
    let n = pw.len();
    // The span [U_k, U_k+1] holding u: the one to its right at the start of
    // the range, to its left elsewhere, so k stays within [p, n - 1].
    let k = (0..knots.len().saturating_sub(1))
        .rev()
        .find(|&i| {
            let below = if at_start {
                knots[i] <= u
            } else {
                knots[i] < u
            };
            below && knots[i] < knots[i + 1]
        })
        .ok_or_else(|| format!("knot {u} lies outside the knot vector"))?;
    if k < p || k >= n {
        return Err(format!("knot {u} falls in no valid span"));
    }
    let mut q = Vec::with_capacity(n + 1);
    for i in 0..=n {
        if i + p <= k {
            q.push(pw[i]);
        } else if i <= k {
            let den = knots[i + p] - knots[i];
            if den <= 0.0 {
                return Err("degenerate knot span".to_string());
            }
            let a = (u - knots[i]) / den;
            let (pi, pm) = (pw[i], pw[i - 1]);
            q.push([
                a * pi[0] + (1.0 - a) * pm[0],
                a * pi[1] + (1.0 - a) * pm[1],
                a * pi[2] + (1.0 - a) * pm[2],
            ]);
        } else {
            q.push(pw[i - 1]);
        }
    }
    knots.insert(k + 1, u);
    *pw = q;
    Ok(())
}

/// A B-spline / NURBS as consecutive rational Bézier pieces over its
/// parameter range (knot insertion to multiplicity = degree everywhere).
fn spline_pieces(spline: &Spline2d) -> Result<Vec<Bez>, String> {
    let (p, cps, weights, knots) = match spline {
        Spline2d::BSpline(b) => (
            b.degree,
            b.control_points.clone(),
            vec![1.0; b.control_points.len()],
            b.knots.clone(),
        ),
        Spline2d::Nurbs(n) => (
            n.degree,
            n.control_points.clone(),
            n.weights.clone(),
            n.knots.clone(),
        ),
    };
    let n = cps.len();
    if p == 0 || n <= p || knots.len() != n + p + 1 || weights.len() != n {
        return Err("malformed spline (degree / knots / weights)".to_string());
    }
    if weights.iter().any(|w| !(w.is_finite() && *w > 0.0)) {
        return Err("a non-positive weight voids the convex-hull bound".to_string());
    }
    if knots.windows(2).any(|w| !(w[0] <= w[1]))
        || cps.iter().any(|c| !(c.x.is_finite() && c.y.is_finite()))
    {
        return Err("non-monotone knots or non-finite control points".to_string());
    }
    let (umin, umax) = (knots[p], knots[n]);
    if !(umin < umax) {
        return Err("empty parameter range".to_string());
    }
    let mut pw: Vec<H> = cps
        .iter()
        .zip(&weights)
        .map(|(c, w)| [c.x * w, c.y * w, *w])
        .collect();
    let mut kv = knots;
    let mut distinct: Vec<f64> = kv
        .iter()
        .copied()
        .filter(|&u| u >= umin && u <= umax)
        .collect();
    distinct.dedup();
    for u in distinct {
        let mult = |kv: &Vec<f64>| kv.iter().filter(|&&k| k == u).count();
        while mult(&kv) < p {
            insert_knot(&mut kv, &mut pw, p, u, u == umin)?;
        }
    }
    let mut pieces = Vec::new();
    for k in p..kv.len().saturating_sub(p + 1) {
        if kv[k] < kv[k + 1] && kv[k] >= umin && kv[k + 1] <= umax && k < pw.len() {
            pieces.push(
                Bez {
                    cp: pw[k - p..=k].to_vec(),
                }
                .normalized(),
            );
        }
    }
    if pieces.is_empty() {
        return Err("no Bézier piece extracted".to_string());
    }
    Ok(pieces)
}

// ── Profile curves ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Geom {
    Seg(P, P),
    /// Circular arc: centre, radius, CCW start angle, CCW sweep.
    Arc {
        c: P,
        r: f64,
        start: f64,
        sweep: f64,
    },
    Circle {
        c: P,
        r: f64,
    },
    Ellipse(Ellipse2d),
    Spline {
        pieces: Vec<Bez>,
        closed: bool,
    },
}

#[derive(Debug, Clone)]
struct Curve {
    label: String,
    geom: Geom,
    /// Endpoints a join can be made at (`None` for closed curves).
    ends: Option<(P, P)>,
    bbox: (P, P),
}

fn arc_bbox(c: P, r: f64, start: f64, sweep: f64) -> (P, P) {
    let mut pts = vec![
        [c[0] + r * start.cos(), c[1] + r * start.sin()],
        [
            c[0] + r * (start + sweep).cos(),
            c[1] + r * (start + sweep).sin(),
        ],
    ];
    for k in 0..4 {
        let a = k as f64 * PI / 2.0;
        if (a - start).rem_euclid(TAU) <= sweep {
            pts.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
        }
    }
    bbox_of(&pts)
}

fn collect_curves(sketch: &Sketch, tol: f64) -> (Vec<Curve>, Vec<UncheckedPair>) {
    let mut curves = Vec::new();
    let mut unchecked = Vec::new();
    let seg = |label: String, a: P, b: P, curves: &mut Vec<Curve>| {
        if dist(a, b) > tol {
            curves.push(Curve {
                label,
                geom: Geom::Seg(a, b),
                ends: Some((a, b)),
                bbox: bbox_of(&[a, b]),
            });
        }
    };
    for e in sketch.lines().iter() {
        let l = e.value();
        if l.is_construction {
            continue;
        }
        if let LineGeometry::Segment(s) = &l.geometry {
            seg(
                EntityRef::Line(l.id).to_string(),
                [s.start.x, s.start.y],
                [s.end.x, s.end.y],
                &mut curves,
            );
        }
    }
    for e in sketch.rectangles().iter() {
        let r = e.value();
        if r.is_construction {
            continue;
        }
        let c = r.rectangle.corners();
        for i in 0..4 {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            seg(
                format!("{} edge {i}", EntityRef::Rectangle(r.id)),
                [a.x, a.y],
                [b.x, b.y],
                &mut curves,
            );
        }
    }
    for e in sketch.polylines().iter() {
        let pl = e.value();
        if pl.is_construction {
            continue;
        }
        let v = &pl.polyline.vertices;
        let count = if pl.polyline.is_closed {
            v.len()
        } else {
            v.len().saturating_sub(1)
        };
        for i in 0..count {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            seg(
                format!("{} segment {i}", EntityRef::Polyline(pl.id)),
                [a.x, a.y],
                [b.x, b.y],
                &mut curves,
            );
        }
    }
    for e in sketch.arcs().iter() {
        let a = e.value();
        if a.is_construction {
            continue;
        }
        let arc = &a.arc;
        let (start, sweep) = if arc.ccw {
            (
                arc.start_angle,
                (arc.end_angle - arc.start_angle).rem_euclid(TAU),
            )
        } else {
            (
                arc.end_angle,
                (arc.start_angle - arc.end_angle).rem_euclid(TAU),
            )
        };
        let sweep = if sweep <= 0.0 { TAU } else { sweep };
        let c = [arc.center.x, arc.center.y];
        let (s, en) = (arc.start_point(), arc.end_point());
        curves.push(Curve {
            label: EntityRef::Arc(a.id).to_string(),
            geom: Geom::Arc {
                c,
                r: arc.radius,
                start,
                sweep,
            },
            ends: Some(([s.x, s.y], [en.x, en.y])),
            bbox: arc_bbox(c, arc.radius, start, sweep),
        });
    }
    for e in sketch.circles().iter() {
        let ci = e.value();
        if ci.is_construction {
            continue;
        }
        let c = [ci.circle.center.x, ci.circle.center.y];
        let r = ci.circle.radius;
        curves.push(Curve {
            label: EntityRef::Circle(ci.id).to_string(),
            geom: Geom::Circle { c, r },
            ends: None,
            bbox: ([c[0] - r, c[1] - r], [c[0] + r, c[1] + r]),
        });
    }
    for e in sketch.ellipses().iter() {
        let el = e.value();
        if el.is_construction {
            continue;
        }
        let (lo, hi) = el.ellipse.bounding_box();
        curves.push(Curve {
            label: EntityRef::Ellipse(el.id).to_string(),
            geom: Geom::Ellipse(el.ellipse.clone()),
            ends: None,
            bbox: ([lo.x, lo.y], [hi.x, hi.y]),
        });
    }
    for e in sketch.splines().iter() {
        let sp = e.value();
        if sp.is_construction {
            continue;
        }
        let label = EntityRef::Spline(sp.id).to_string();
        match spline_pieces(&sp.spline) {
            Ok(pieces) => {
                let (s, en) = (pieces[0].start(), pieces[pieces.len() - 1].end());
                let closed = dist(s, en) <= tol;
                let all: Vec<P> = pieces.iter().flat_map(|b| b.points()).collect();
                curves.push(Curve {
                    label,
                    geom: Geom::Spline { pieces, closed },
                    ends: if closed { None } else { Some((s, en)) },
                    bbox: bbox_of(&all),
                });
            }
            Err(reason) => unchecked.push(UncheckedPair {
                first: label.clone(),
                second: label,
                reason,
            }),
        }
    }
    (curves, unchecked)
}

// ── Implicit forms of the conic curves ───────────────────────────────────

/// A conic curve's implicit equation on homogeneous `(x, y, w)`, scaled so
/// its gradient is at most about 1 near the curve: `|f| ≲ distance · w^k`.
enum Implicit {
    Linear([f64; 3]),
    Quadric([[f64; 3]; 3]),
}

fn implicit_of(g: &Geom) -> Option<Implicit> {
    match g {
        Geom::Seg(a, b) => {
            let d = sub(*b, *a);
            let l = norm(d);
            let n = [-d[1] / l, d[0] / l];
            Some(Implicit::Linear([n[0], n[1], -dot(n, *a)]))
        }
        Geom::Arc { c, r, .. } | Geom::Circle { c, r } => {
            let s = 1.0 / (2.0 * r);
            Some(Implicit::Quadric([
                [s, 0.0, -c[0] * s],
                [0.0, s, -c[1] * s],
                [
                    -c[0] * s,
                    -c[1] * s,
                    (c[0] * c[0] + c[1] * c[1] - r * r) * s,
                ],
            ]))
        }
        Geom::Ellipse(e) => {
            let (sn, cs) = e.rotation.sin_cos();
            let (cx, cy) = (e.center.x, e.center.y);
            let (a, b) = (e.semi_major, e.semi_minor);
            let l1 = [cs / a, sn / a, -(cx * cs + cy * sn) / a];
            let l2 = [-sn / b, cs / b, (cx * sn - cy * cs) / b];
            let k = a.min(b) / 2.0;
            let mut q = [[0.0; 3]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    q[i][j] = k * (l1[i] * l1[j] + l2[i] * l2[j]);
                }
            }
            q[2][2] -= k;
            Some(Implicit::Quadric(q))
        }
        Geom::Spline { .. } => None,
    }
}

/// The piece substituted into the implicit, as Bernstein coefficients.
fn substitute(imp: &Implicit, b: &Bez) -> Vec<f64> {
    match imp {
        Implicit::Linear(l) => {
            b.cp.iter()
                .map(|h| l[0] * h[0] + l[1] * h[1] + l[2] * h[2])
                .collect()
        }
        Implicit::Quadric(q) => {
            let n = b.cp.len() - 1;
            let big = 2 * n;
            let form = |u: &H, v: &H| -> f64 {
                let mut s = 0.0;
                for i in 0..3 {
                    for j in 0..3 {
                        s += u[i] * q[i][j] * v[j];
                    }
                }
                s
            };
            (0..=big)
                .map(|k| {
                    let lo = k.saturating_sub(n);
                    let hi = k.min(n);
                    let mut s = 0.0;
                    for i in lo..=hi {
                        let j = k - i;
                        s += binomial(n, i) * binomial(n, j) * form(&b.cp[i], &b.cp[j]);
                    }
                    s / binomial(big, k)
                })
                .collect()
        }
    }
}

/// Distance from `p` to a conic curve's CARRIER (line / full circle / ellipse).
fn carrier_distance(g: &Geom, p: P) -> Option<f64> {
    match g {
        Geom::Seg(a, b) => {
            let d = sub(*b, *a);
            Some(cross(d, sub(p, *a)).abs() / norm(d))
        }
        Geom::Arc { c, r, .. } | Geom::Circle { c, r } => Some((dist(p, *c) - r).abs()),
        Geom::Ellipse(e) => e
            .closest_point(&super::Point2d::new(p[0], p[1]))
            .ok()
            .map(|q| dist(p, [q.x, q.y])),
        Geom::Spline { .. } => None,
    }
}

/// `p` (already on the carrier within tolerance) lies on the curve's extent.
fn on_extent(g: &Geom, p: P, tol: f64) -> bool {
    match g {
        Geom::Seg(a, b) => {
            let d = sub(*b, *a);
            let l = norm(d);
            let s = dot(sub(p, *a), d) / l;
            s >= -tol && s <= l + tol
        }
        Geom::Arc { c, r, start, sweep } => {
            let ang = (p[1] - c[1]).atan2(p[0] - c[0]);
            let rel = (ang - start).rem_euclid(TAU);
            let margin = tol / r;
            rel <= sweep + margin || rel >= TAU - margin
        }
        _ => true,
    }
}

// ── Bernstein root isolation ─────────────────────────────────────────────

enum Hit {
    Point(f64),
    Span(f64, f64),
}

fn casteljau_split(b: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = b.len();
    let mut v = b.to_vec();
    let mut left = Vec::with_capacity(n);
    let mut right = vec![0.0; n];
    left.push(v[0]);
    right[n - 1] = v[n - 1];
    for r in 1..n {
        for i in 0..n - r {
            v[i] = 0.5 * (v[i] + v[i + 1]);
        }
        left.push(v[0]);
        right[n - 1 - r] = v[n - 1 - r];
    }
    (left, right)
}

fn bernstein_eval(b: &[f64], t: f64) -> f64 {
    let mut v = b.to_vec();
    let n = v.len();
    for r in 1..n {
        for i in 0..n - r {
            v[i] = (1.0 - t) * v[i] + t * v[i + 1];
        }
    }
    v[0]
}

/// Parameters in `[0, 1]` where `|f| ≤ eps`: isolated points and spans.
fn bernstein_hits(b: &[f64], eps: f64) -> Result<Vec<Hit>, String> {
    let mut out = Vec::new();
    let mut budget = POLY_BUDGET;
    hits_rec(b, 0.0, 1.0, eps, 0, &mut out, &mut budget)?;
    Ok(out)
}

fn hits_rec(
    b: &[f64],
    t0: f64,
    t1: f64,
    eps: f64,
    depth: u32,
    out: &mut Vec<Hit>,
    budget: &mut usize,
) -> Result<(), String> {
    if *budget == 0 {
        return Err("root isolation exceeded its work budget".to_string());
    }
    *budget -= 1;
    let mn = b.iter().copied().fold(f64::INFINITY, f64::min);
    let mx = b.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !(mn.is_finite() && mx.is_finite()) {
        return Err("non-finite polynomial coefficient".to_string());
    }
    if mn > eps || mx < -eps {
        return Ok(()); // convex hull: no contact on this interval
    }
    if mn >= -eps && mx <= eps {
        out.push(Hit::Span(t0, t1)); // within tolerance throughout
        return Ok(());
    }
    if depth >= MAX_DEPTH {
        out.push(Hit::Point(0.5 * (t0 + t1)));
        return Ok(());
    }
    let (first, last) = (b[0], b[b.len() - 1]);
    let changes = b
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    if changes == 1 && first.abs() > eps && last.abs() > eps {
        // Exactly one simple root: bisect it.
        let (mut lo, mut hi) = (0.0, 1.0);
        let neg_at_lo = first < 0.0;
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if (bernstein_eval(b, mid) < 0.0) == neg_at_lo {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        out.push(Hit::Point(t0 + (t1 - t0) * 0.5 * (lo + hi)));
        return Ok(());
    }
    let tm = 0.5 * (t0 + t1);
    let (l, r) = casteljau_split(b);
    hits_rec(&l, t0, tm, eps, depth + 1, out, budget)?;
    hits_rec(&r, tm, t1, eps, depth + 1, out, budget)
}

/// Divide the known root at `t = 0` out of `b` (forced exactly), then any
/// further root there the coefficients show (a tangent join is a double root).
fn deflate_start(mut b: Vec<f64>, eps: f64) -> Vec<f64> {
    b[0] = 0.0;
    while b.len() > 1 && b[0].abs() <= eps {
        let n = (b.len() - 1) as f64;
        b = (0..b.len() - 1)
            .map(|i| b[i + 1] * n / (i + 1) as f64)
            .collect();
    }
    b
}

/// [`deflate_start`] at `t = 1`.
fn deflate_end(mut b: Vec<f64>, eps: f64) -> Vec<f64> {
    let last = b.len() - 1;
    b[last] = 0.0;
    while b.len() > 1 && b[b.len() - 1].abs() <= eps {
        let n = (b.len() - 1) as f64;
        let m = b.len() - 1;
        b = (0..m).map(|i| b[i] * n / (m - i) as f64).collect();
    }
    b
}

// ── Pair decisions ───────────────────────────────────────────────────────

/// A contact point found on a pair, before classification.
enum Found {
    At(P),
    Overlap(P),
}

struct PairOutcome {
    found: Vec<Found>,
    /// Extra distance within which a found point is attributed to a join
    /// (a tangent contact is positionally ill-conditioned).
    join_radius: f64,
}

fn shared_join(a: &Curve, b: &Curve, tol: f64) -> Vec<P> {
    let mut joins = Vec::new();
    if let (Some((a0, a1)), Some((b0, b1))) = (a.ends, b.ends) {
        for ea in [a0, a1] {
            for eb in [b0, b1] {
                if dist(ea, eb) <= tol {
                    joins.push(ea);
                }
            }
        }
    }
    joins
}

fn circle_of(g: &Geom) -> Option<(P, f64)> {
    match g {
        Geom::Arc { c, r, .. } | Geom::Circle { c, r } => Some((*c, *r)),
        _ => None,
    }
}

/// CCW angular range of a circular curve: `(start, sweep)`.
fn angular_range(g: &Geom) -> Option<(f64, f64)> {
    match g {
        Geom::Arc { start, sweep, .. } => Some((*start, *sweep)),
        Geom::Circle { .. } => Some((0.0, TAU)),
        _ => None,
    }
}

/// Intersections of two CCW angular ranges, as `(start, length)` pieces.
fn range_overlaps(a: (f64, f64), b: (f64, f64)) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let bs = (b.0 - a.0).rem_euclid(TAU);
    for shift in [-TAU, 0.0, TAU] {
        let lo = (bs + shift).max(0.0);
        let hi = (bs + shift + b.1).min(a.1);
        if hi >= lo {
            out.push((a.0 + lo, hi - lo));
        }
    }
    out
}

fn analytic_pair(a: &Curve, b: &Curve, tol: f64) -> PairOutcome {
    let mut found = Vec::new();
    let mut join_radius = tol;
    match (&a.geom, &b.geom) {
        (Geom::Seg(a0, a1), Geom::Seg(b0, b1)) => {
            let d1 = sub(*a1, *a0);
            let d2 = sub(*b1, *b0);
            let (l1, l2) = (norm(d1), norm(d2));
            let off0 = cross(d1, sub(*b0, *a0)).abs() / l1;
            let off1 = cross(d1, sub(*b1, *a0)).abs() / l1;
            if off0 <= tol && off1 <= tol {
                // Collinear: overlap of the projections.
                let s0 = dot(sub(*b0, *a0), d1) / l1;
                let s1 = dot(sub(*b1, *a0), d1) / l1;
                let lo = s0.min(s1).max(0.0);
                let hi = s0.max(s1).min(l1);
                let at = |s: f64| [a0[0] + d1[0] * s / l1, a0[1] + d1[1] * s / l1];
                if hi - lo > tol {
                    found.push(Found::Overlap(at(0.5 * (lo + hi))));
                } else if hi - lo >= -tol {
                    found.push(Found::At(at(0.5 * (lo + hi))));
                }
            } else {
                let den = cross(d1, d2);
                if den.abs() > 1e-15 * l1 * l2 {
                    let w = sub(*b0, *a0);
                    let s = cross(w, d2) / den;
                    let t = cross(w, d1) / den;
                    if s * l1 >= -tol && s * l1 <= l1 + tol && t * l2 >= -tol && t * l2 <= l2 + tol
                    {
                        found.push(Found::At([a0[0] + s * d1[0], a0[1] + s * d1[1]]));
                    }
                }
            }
        }
        (Geom::Seg(s0, s1), other) | (other, Geom::Seg(s0, s1)) => {
            if let Some((c, r)) = circle_of(other) {
                let d = sub(*s1, *s0);
                let u = [d[0] / norm(d), d[1] / norm(d)];
                let along = dot(sub(c, *s0), u);
                let foot = [s0[0] + along * u[0], s0[1] + along * u[1]];
                let h = dist(c, foot);
                let seg = Geom::Seg(*s0, *s1);
                let mut push = |p: P| {
                    if on_extent(&seg, p, tol) && on_extent(other, p, tol) {
                        found.push(Found::At(p));
                    }
                };
                if (h - r).abs() <= tol {
                    join_radius = join_radius.max((2.0 * r * tol).sqrt() + tol);
                    push(foot);
                } else if h < r {
                    let dl = (r * r - h * h).sqrt();
                    push([foot[0] + dl * u[0], foot[1] + dl * u[1]]);
                    push([foot[0] - dl * u[0], foot[1] - dl * u[1]]);
                }
            }
        }
        (ga, gb) => {
            if let (Some((c1, r1)), Some((c2, r2))) = (circle_of(ga), circle_of(gb)) {
                let dd = dist(c1, c2);
                if dd <= tol && (r1 - r2).abs() <= tol {
                    // One circle: overlap of the angular ranges.
                    if let (Some(ra), Some(rb)) = (angular_range(ga), angular_range(gb)) {
                        for (s, len) in range_overlaps(ra, rb) {
                            let mid = s + 0.5 * len;
                            let p = [c1[0] + r1 * mid.cos(), c1[1] + r1 * mid.sin()];
                            if len * r1 > tol {
                                found.push(Found::Overlap(p));
                            } else {
                                found.push(Found::At(p));
                            }
                        }
                    }
                } else if dd > tol && dd <= r1 + r2 + tol && dd >= (r1 - r2).abs() - tol {
                    let u = [(c2[0] - c1[0]) / dd, (c2[1] - c1[1]) / dd];
                    let along = (dd * dd + r1 * r1 - r2 * r2) / (2.0 * dd);
                    let h2 = r1 * r1 - along * along;
                    let base = [c1[0] + along * u[0], c1[1] + along * u[1]];
                    let mut push = |p: P| {
                        if on_extent(ga, p, tol) && on_extent(gb, p, tol) {
                            found.push(Found::At(p));
                        }
                    };
                    let tangent =
                        (dd - (r1 + r2)).abs() <= tol || (dd - (r1 - r2).abs()).abs() <= tol;
                    if tangent || h2 <= 0.0 {
                        let rm = r1.min(r2);
                        join_radius = join_radius.max((2.0 * rm * tol).sqrt() + tol);
                        push(base);
                    } else {
                        let h = h2.sqrt();
                        let perp = [-u[1], u[0]];
                        push([base[0] + h * perp[0], base[1] + h * perp[1]]);
                        push([base[0] - h * perp[0], base[1] - h * perp[1]]);
                    }
                }
            }
        }
    }
    PairOutcome { found, join_radius }
}

/// `implicit_side` has an implicit equation; `param_side` is cut into
/// rational Bézier pieces and substituted into it.
fn polynomial_pair(
    implicit_side: &Curve,
    param_side: &Curve,
    tol: f64,
) -> Result<PairOutcome, String> {
    let imp = implicit_of(&implicit_side.geom).ok_or("no implicit form")?;
    let pieces = match &param_side.geom {
        Geom::Spline { pieces, .. } => pieces.clone(),
        Geom::Ellipse(e) => ellipse_pieces(e),
        _ => return Err("curve has no Bézier form".to_string()),
    };
    let eps = 2.0 * tol;
    let joins = shared_join(implicit_side, param_side, tol);
    let mut found = Vec::new();
    let last = pieces.len() - 1;
    for (i, piece) in pieces.iter().enumerate() {
        let mut coeffs = substitute(&imp, piece);
        if i == 0 && joins.iter().any(|j| dist(*j, piece.start()) <= tol) {
            coeffs = deflate_start(coeffs, eps);
        }
        if i == last && joins.iter().any(|j| dist(*j, piece.end()) <= tol) && coeffs.len() > 1 {
            coeffs = deflate_end(coeffs, eps);
        }
        for hit in bernstein_hits(&coeffs, eps)? {
            match hit {
                Hit::Point(t) => {
                    let p = piece.eval(t);
                    let d = carrier_distance(&implicit_side.geom, p)
                        .ok_or("carrier distance unavailable")?;
                    if d <= tol && on_extent(&implicit_side.geom, p, tol) {
                        found.push(Found::At(p));
                    }
                }
                Hit::Span(t0, t1) if dist(piece.eval(t0), piece.eval(t1)) <= 10.0 * tol => {
                    // A span no longer than the tolerance is a point contact.
                    let p = piece.eval(0.5 * (t0 + t1));
                    let d = carrier_distance(&implicit_side.geom, p)
                        .ok_or("carrier distance unavailable")?;
                    if d <= tol && on_extent(&implicit_side.geom, p, tol) {
                        found.push(Found::At(p));
                    }
                }
                Hit::Span(t0, t1) => {
                    for t in [0.5 * (t0 + t1), t0, t1] {
                        let p = piece.eval(t);
                        let d = carrier_distance(&implicit_side.geom, p)
                            .ok_or("carrier distance unavailable")?;
                        let near_join = joins.iter().any(|j| dist(*j, p) <= 10.0 * tol);
                        if d <= tol && on_extent(&implicit_side.geom, p, tol) && !near_join {
                            found.push(Found::Overlap(p));
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(PairOutcome {
        found,
        join_radius: 10.0 * tol,
    })
}

// ── Spline against spline ────────────────────────────────────────────────

struct SplineSearch {
    budget: usize,
    exhausted: bool,
    tol: f64,
    hits: Vec<P>,
}

/// The angular sector `(start, width)` the piece's control points occupy
/// around `j`, or `None` when it is not a pointed cone.
fn sector(piece: &Bez, j: P, tol: f64) -> Option<(f64, f64)> {
    let angles: Vec<f64> = piece
        .points()
        .into_iter()
        .filter(|p| dist(*p, j) > tol)
        .map(|p| (p[1] - j[1]).atan2(p[0] - j[0]))
        .collect();
    let reference = *angles.first()?;
    let rel: Vec<f64> = angles.iter().map(|a| wrap_pi(a - reference)).collect();
    let lo = rel.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = rel.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if hi - lo >= PI - 1e-9 {
        return None;
    }
    Some((reference + lo, hi - lo))
}

/// The two pieces' hulls meet only at the join `j`: their sectors around it
/// are disjoint.
fn separated_at(a: &Bez, b: &Bez, j: P, tol: f64) -> bool {
    let (Some((sa, wa)), Some((sb, wb))) = (sector(a, j, tol), sector(b, j, tol)) else {
        return false;
    };
    let gap = (sb - sa).rem_euclid(TAU);
    let margin = 1e-12;
    gap > wa + margin && gap + wb < TAU - margin
}

fn touches(piece: &Bez, j: P, tol: f64) -> bool {
    dist(piece.start(), j) <= tol || dist(piece.end(), j) <= tol
}

fn spline_pair_rec(a: &Bez, b: &Bez, joins: &[P], depth: u32, s: &mut SplineSearch) {
    if s.exhausted {
        return;
    }
    if s.budget == 0 {
        s.exhausted = true;
        return;
    }
    s.budget -= 1;
    let (ba, bb) = (a.bbox(), b.bbox());
    if !boxes_meet(&ba, &bb, s.tol) {
        return;
    }
    if joins.len() == 1 && separated_at(a, b, joins[0], s.tol) {
        return;
    }
    let (da, db) = (diag(&ba), diag(&bb));
    if da <= 0.25 * s.tol && db <= 0.25 * s.tol {
        let p = a.eval(0.5);
        if !joins.iter().any(|j| dist(*j, p) <= 10.0 * s.tol) {
            s.hits.push(p);
        }
        return;
    }
    if depth >= MAX_DEPTH {
        s.exhausted = true;
        return;
    }
    let keep = |piece: &Bez| -> Vec<P> {
        joins
            .iter()
            .copied()
            .filter(|j| touches(piece, *j, s.tol))
            .collect()
    };
    if da >= db {
        let (l, r) = a.split(0.5);
        let (jl, jr) = (keep(&l), keep(&r));
        spline_pair_rec(&l, b, &jl, depth + 1, s);
        spline_pair_rec(&r, b, &jr, depth + 1, s);
    } else {
        let (l, r) = b.split(0.5);
        let (jl, jr) = (keep(&l), keep(&r));
        spline_pair_rec(a, &l, &jl, depth + 1, s);
        spline_pair_rec(a, &r, &jr, depth + 1, s);
    }
}

/// The piece cannot cross itself: its control polygon advances strictly
/// along its chord, so by variation diminishing the curve does too.
fn monotone(piece: &Bez, tol: f64) -> bool {
    let pts = piece.points();
    let chord = sub(pts[pts.len() - 1], pts[0]);
    if norm(chord) <= tol {
        return false;
    }
    pts.windows(2).all(|w| dot(sub(w[1], w[0]), chord) > 0.0)
}

fn spline_self_rec(piece: &Bez, depth: u32, s: &mut SplineSearch) {
    if s.exhausted || monotone(piece, s.tol) {
        return;
    }
    if depth >= MAX_DEPTH {
        s.exhausted = true;
        return;
    }
    let (l, r) = piece.split(0.5);
    spline_self_rec(&l, depth + 1, s);
    spline_self_rec(&r, depth + 1, s);
    let mut joins = vec![l.end()];
    if dist(l.start(), r.end()) <= s.tol {
        joins.push(l.start());
    }
    spline_pair_rec(&l, &r, &joins, depth + 1, s);
}

fn spline_pair(a: &Curve, b: &Curve, tol: f64) -> Result<PairOutcome, String> {
    let (Geom::Spline { pieces: pa, .. }, Geom::Spline { pieces: pb, .. }) = (&a.geom, &b.geom)
    else {
        return Err("not a spline pair".to_string());
    };
    let joins = shared_join(a, b, tol);
    let mut s = SplineSearch {
        budget: SPLINE_PAIR_BUDGET,
        exhausted: false,
        tol,
        hits: Vec::new(),
    };
    for (i, x) in pa.iter().enumerate() {
        for (k, y) in pb.iter().enumerate() {
            let ends_a = (i == 0, i == pa.len() - 1);
            let ends_b = (k == 0, k == pb.len() - 1);
            let local: Vec<P> = joins
                .iter()
                .copied()
                .filter(|j| {
                    let on_a = (ends_a.0 && dist(x.start(), *j) <= tol)
                        || (ends_a.1 && dist(x.end(), *j) <= tol);
                    let on_b = (ends_b.0 && dist(y.start(), *j) <= tol)
                        || (ends_b.1 && dist(y.end(), *j) <= tol);
                    on_a && on_b
                })
                .collect();
            spline_pair_rec(x, y, &local, 0, &mut s);
        }
    }
    if s.exhausted {
        return Err("spline subdivision exceeded its work budget".to_string());
    }
    Ok(PairOutcome {
        found: s.hits.into_iter().map(Found::At).collect(),
        join_radius: 10.0 * tol,
    })
}

fn spline_self(c: &Curve, tol: f64) -> Result<Vec<P>, String> {
    let Geom::Spline { pieces, closed } = &c.geom else {
        return Ok(Vec::new());
    };
    let mut s = SplineSearch {
        budget: SPLINE_PAIR_BUDGET,
        exhausted: false,
        tol,
        hits: Vec::new(),
    };
    let n = pieces.len();
    for (i, x) in pieces.iter().enumerate() {
        spline_self_rec(x, 0, &mut s);
        for k in (i + 1)..n {
            let mut joins = Vec::new();
            if k == i + 1 {
                joins.push(x.end());
            }
            if *closed && i == 0 && k == n - 1 {
                joins.push(x.start());
            }
            spline_pair_rec(x, &pieces[k], &joins, 0, &mut s);
        }
    }
    if s.exhausted {
        return Err("spline subdivision exceeded its work budget".to_string());
    }
    Ok(s.hits)
}

// ── Driver ───────────────────────────────────────────────────────────────

fn classify(p: P, a: &Curve, b: &Curve, joins: &[P], join_radius: f64) -> Option<ContactKind> {
    if joins.iter().any(|j| dist(*j, p) <= join_radius) {
        return None;
    }
    let near_end = |c: &Curve| {
        c.ends
            .is_some_and(|(s, e)| dist(s, p) <= join_radius || dist(e, p) <= join_radius)
    };
    if near_end(a) || near_end(b) {
        Some(ContactKind::EndOnInterior)
    } else {
        Some(ContactKind::Crossing)
    }
}

fn push_contact(
    report: &mut ProfileContactReport,
    first: &str,
    second: &str,
    kind: ContactKind,
    p: P,
    tol: f64,
) {
    let dup = report.contacts.iter().any(|c| {
        c.first == first && c.second == second && c.kind == kind && dist(c.point, p) <= 100.0 * tol
    });
    if !dup {
        report.contacts.push(ProfileContact {
            first: first.to_string(),
            second: second.to_string(),
            kind,
            point: p,
        });
    }
}

/// Every contact between the curves of the sketch's profile, and every pair
/// it could not decide.
pub fn profile_contacts(sketch: &Sketch) -> ProfileContactReport {
    let tol = PROFILE_CONTACT_TOLERANCE;
    let (curves, unchecked) = collect_curves(sketch, tol);
    let mut report = ProfileContactReport {
        contacts: Vec::new(),
        unchecked,
    };
    for (i, a) in curves.iter().enumerate() {
        for b in curves.iter().skip(i + 1) {
            if !boxes_meet(&a.bbox, &b.bbox, tol) {
                continue;
            }
            let is_spline = |c: &Curve| matches!(c.geom, Geom::Spline { .. });
            let is_ellipse = |c: &Curve| matches!(c.geom, Geom::Ellipse(_));
            let outcome = if is_spline(a) && is_spline(b) {
                spline_pair(a, b, tol)
            } else if is_spline(b) {
                polynomial_pair(a, b, tol)
            } else if is_spline(a) {
                polynomial_pair(b, a, tol)
            } else if is_ellipse(a) {
                polynomial_pair(b, a, tol)
            } else if is_ellipse(b) {
                polynomial_pair(a, b, tol)
            } else {
                Ok(analytic_pair(a, b, tol))
            };
            match outcome {
                Ok(o) => {
                    let joins = shared_join(a, b, tol);
                    for f in o.found {
                        match f {
                            Found::At(p) => {
                                if let Some(kind) = classify(p, a, b, &joins, o.join_radius) {
                                    push_contact(&mut report, &a.label, &b.label, kind, p, tol);
                                }
                            }
                            Found::Overlap(p) => {
                                push_contact(
                                    &mut report,
                                    &a.label,
                                    &b.label,
                                    ContactKind::Overlap,
                                    p,
                                    tol,
                                );
                            }
                        }
                    }
                }
                Err(reason) => report.unchecked.push(UncheckedPair {
                    first: a.label.clone(),
                    second: b.label.clone(),
                    reason,
                }),
            }
        }
    }
    for c in &curves {
        match spline_self(c, tol) {
            Ok(hits) => {
                for p in hits {
                    push_contact(
                        &mut report,
                        &c.label,
                        &c.label,
                        ContactKind::Crossing,
                        p,
                        tol,
                    );
                }
            }
            Err(reason) => report.unchecked.push(UncheckedPair {
                first: c.label.clone(),
                second: c.label.clone(),
                reason,
            }),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cubic(cps: [(f64, f64); 4]) -> Bez {
        Bez {
            cp: cps.iter().map(|&(x, y)| [x, y, 1.0]).collect(),
        }
    }

    #[test]
    fn an_ellipse_quarter_lies_on_the_ellipse() {
        let e =
            Ellipse2d::new(super::super::Point2d::new(1.0, -2.0), 5.0, 2.0, 0.7).expect("ellipse");
        for piece in ellipse_pieces(&e) {
            for k in 0..=10 {
                let p = piece.eval(k as f64 / 10.0);
                let d = carrier_distance(&Geom::Ellipse(e.clone()), p).expect("distance");
                assert!(d < 1e-9, "quarter point {p:?} is {d} off the ellipse");
            }
        }
    }

    #[test]
    fn knot_insertion_reproduces_the_spline() {
        let cps = vec![
            super::super::Point2d::new(0.0, 0.0),
            super::super::Point2d::new(1.0, 3.0),
            super::super::Point2d::new(4.0, 3.0),
            super::super::Point2d::new(5.0, 0.0),
            super::super::Point2d::new(7.0, -2.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0];
        let sp = Spline2d::BSpline(
            super::super::spline2d::BSpline2d::new(3, cps, knots).expect("spline"),
        );
        let pieces = spline_pieces(&sp).expect("pieces");
        assert_eq!(pieces.len(), 2);
        let spans = [(0.0, 0.4), (0.4, 1.0)];
        for (piece, (u0, u1)) in pieces.iter().zip(spans) {
            for k in 0..=8 {
                let t = k as f64 / 8.0;
                let q = sp.evaluate(u0 + (u1 - u0) * t).expect("eval");
                let p = piece.eval(t);
                assert!(dist(p, [q.x, q.y]) < 1e-9, "piece {p:?} vs spline {q:?}");
            }
        }
    }

    #[test]
    fn a_tangent_join_divides_out_and_leaves_no_root() {
        // y·w of a cubic starting tangent to the x-axis at the origin.
        let b = substitute(
            &Implicit::Linear([0.0, 1.0, 0.0]),
            &cubic([(0.0, 0.0), (3.0, 0.0), (6.0, 4.0), (2.0, 8.0)]),
        );
        let q = deflate_start(b, 2e-6);
        assert_eq!(q.len(), 2, "a double root is divided out: {q:?}");
        assert!(bernstein_hits(&q, 2e-6).expect("hits").is_empty());
    }

    #[test]
    fn a_looping_cubic_is_not_monotone_but_a_gentle_one_is() {
        assert!(!monotone(
            &cubic([(0.0, 0.0), (15.0, 10.0), (-5.0, 10.0), (10.0, 0.0)]),
            1e-6
        ));
        assert!(monotone(
            &cubic([(0.0, 0.0), (3.0, 3.0), (6.0, 3.0), (9.0, 0.0)]),
            1e-6
        ));
    }
}
