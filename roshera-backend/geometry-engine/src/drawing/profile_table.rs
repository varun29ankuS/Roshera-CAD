//! The ordinates of a freeform outline, measured from the solid (2026-11-19).
//!
//! Varun, on the wing-rib sheet: "the drawing for the aerofoil has no overall
//! dimensions neither spline measurement". A chord and a depth do not let a
//! machinist make an aerofoil: the profile itself has to be stated, and the
//! way drawings state one is a table of ordinates -- the upper and lower
//! surface, in millimetres from the chord line, at standard stations along
//! the chord -- with the chord, the maximum thickness and where it occurs,
//! and the trailing-edge thickness.
//!
//! A freeform outline is a planar face whose outer loop is either a run of
//! many short, gently turning straight edges (a profile sampled into a
//! polygon, as an aerofoil generated from its NACA designation is) or
//! contains spline edges. Its ordinates are measured on the face's own
//! boundary edges -- never on a tessellation -- in a chord frame found from
//! the outline itself: the leading edge is the rounded end, the trailing
//! edge the thin one. The sheet certificate re-measures the live solid and
//! compares every ordinate.

use serde::{Deserialize, Serialize};

use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// The stations, in percent of chord, the table states: the conventional
/// aerofoil set, dense at the leading edge where the curvature is.
pub const STATIONS_PERCENT: [f64; 18] = [
    0.0, 1.25, 2.5, 5.0, 7.5, 10.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0,
    95.0, 100.0,
];

/// Fewest consecutive short, gently turning straight edges that make a
/// sampled (faceted) outline. A filleted bracket's face has a handful of
/// arcs and lines; a profile sampled into a polygon has dozens of facets.
const MIN_FACET_RUN: usize = 16;

/// A facet is short against the outline it belongs to.
const FACET_MAX_FRACTION: f64 = 0.06;

/// And turns gently from its neighbour (degrees).
const FACET_MAX_TURN_DEG: f64 = 25.0;

/// One station of the table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileStation {
    /// Percent of chord from the leading edge.
    pub percent: f64,
    /// Distance from the leading edge along the chord, mm.
    pub x_mm: f64,
    /// Upper surface, mm from the chord line (positive up).
    pub upper_mm: f64,
    /// Lower surface, mm from the chord line.
    pub lower_mm: f64,
}

/// The ordinate table of one freeform outline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileTable {
    /// The view the outline is seen true in (the table is read against it);
    /// set by the sheet builder. `None` when no view looks along the face.
    #[serde(default)]
    pub owner_view: Option<usize>,
    /// The planar face whose outline is tabled.
    pub face: u32,
    /// Leading edge to trailing edge, mm.
    pub chord_mm: f64,
    /// The largest upper-minus-lower ordinate, mm.
    pub max_thickness_mm: f64,
    /// Where it occurs, percent of chord.
    pub max_thickness_at_percent: f64,
    /// Upper minus lower at the trailing edge, mm (0 for a sharp edge).
    pub te_thickness_mm: f64,
    /// The ordinates, leading edge to trailing edge.
    pub stations: Vec<ProfileStation>,
}

/// Measure `solid_id`'s freeform outline and set it on `drawing`, tied to the
/// view that sees it true (looking along the face's normal). Leaves
/// `drawing.profile_table` `None` when the part has no freeform outline.
pub fn attach_profile_table(
    model: &BRepModel,
    solid_id: SolidId,
    drawing: &mut super::types::Drawing,
) {
    let Some(mut table) = measure_profile(model, solid_id) else {
        drawing.profile_table = None;
        return;
    };
    let normal = model
        .faces
        .get(table.face)
        .and_then(|f| model.surfaces.get(f.surface_id))
        .and_then(|s| {
            s.as_any()
                .downcast_ref::<crate::primitives::surface::Plane>()
                .map(|p| p.normal)
        });
    if let Some(n) = normal.and_then(|n| n.normalize().ok()) {
        table.owner_view = drawing.views.iter().position(|v| {
            super::dimensioning::view_dir(&v.projection)
                .is_some_and(|d| (d[0] * n.x + d[1] * n.y + d[2] * n.z).abs() > 0.999)
        });
    }
    drawing.profile_table = Some(table);
}

/// One outline segment in the chord frame `(u along the chord, v across)`.
type Seg = ([f64; 2], [f64; 2]);

/// The freeform outline of `solid_id`, measured; `None` when it has none.
pub fn measure_profile(model: &BRepModel, solid_id: SolidId) -> Option<ProfileTable> {
    let solid = model.solids.get(solid_id)?;
    let shell = model.shells.get(solid.outer_shell)?;
    let mut best: Option<(f64, u32, Vec<Point3>, Vector3)> = None;
    for &face_id in &shell.faces {
        let Some((points, normal)) = freeform_outline(model, face_id) else {
            continue;
        };
        let extent = spread(&points);
        if best.as_ref().is_none_or(|(e, ..)| extent > *e + 1e-9) {
            best = Some((extent, face_id, points, normal));
        }
    }
    let (_, face, points, normal) = best?;
    measure_outline(face, &points, normal)
}

/// The boundary of `face_id` as an ordered list of segment end points (each
/// pair `points[2k], points[2k+1]` one segment), with the face's plane
/// normal, when its outer loop is a freeform outline.
fn freeform_outline(model: &BRepModel, face_id: u32) -> Option<(Vec<Point3>, Vector3)> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let plane = surface
        .as_any()
        .downcast_ref::<crate::primitives::surface::Plane>()?;
    let lp = model.loops.get(face.outer_loop)?;
    if lp.edges.len() < MIN_FACET_RUN {
        return None;
    }
    let tol = Tolerance::default();
    // Every edge as segments, and per edge: (is a straight line, start, end).
    let mut segs: Vec<Point3> = Vec::new();
    let mut lines: Vec<Option<(Point3, Point3)>> = Vec::new();
    let mut spline_len = 0.0;
    let mut total_len = 0.0;
    for &eid in &lp.edges {
        let edge = model.edges.get(eid)?;
        let curve = model.curves.get(edge.curve_id)?;
        let (t0, t1) = (edge.param_range.start, edge.param_range.end);
        let linear = curve.is_linear(tol);
        let n = if linear { 1 } else { 32 };
        let mut prev = curve.point_at(t0).ok()?;
        let first = prev;
        let mut len = 0.0;
        for i in 1..=n {
            let p = curve.point_at(t0 + (t1 - t0) * i as f64 / n as f64).ok()?;
            segs.push(prev);
            segs.push(p);
            len += (p - prev).magnitude();
            prev = p;
        }
        total_len += len;
        let ty = curve.type_name().to_ascii_lowercase();
        if !linear && !ty.contains("arc") && !ty.contains("circle") {
            spline_len += len;
        }
        lines.push(linear.then_some((first, prev)));
    }
    let extent = spread(&segs);
    if extent <= 0.0 {
        return None;
    }
    // A run of short, gently turning straight edges (cyclic).
    let short_dir = |l: &Option<(Point3, Point3)>| -> Option<Vector3> {
        let (a, b) = (*l)?;
        let d = b - a;
        let len = d.magnitude();
        (len > 0.0 && len < FACET_MAX_FRACTION * extent).then(|| d * (1.0 / len))
    };
    let cos_max = FACET_MAX_TURN_DEG.to_radians().cos();
    let n = lines.len();
    let mut best_run = 0usize;
    let mut run = 0usize;
    for k in 0..2 * n {
        let here = short_dir(&lines[k % n]);
        let prev = short_dir(&lines[(k + n - 1) % n]);
        run = match (here, prev) {
            (Some(h), Some(p)) if h.dot(&p).abs() >= cos_max => run + 1,
            (Some(_), _) => 1,
            _ => 0,
        };
        best_run = best_run.max(run.min(n));
    }
    let faceted = best_run >= MIN_FACET_RUN;
    let splined = total_len > 0.0 && spline_len / total_len >= 0.4;
    if !(faceted || splined) {
        return None;
    }
    let mut normal = plane.normal.normalize().ok()?;
    // One sign for both caps of a prism, so their tables agree: the normal's
    // dominant component is positive.
    let dom = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() {
        normal.x
    } else if normal.y.abs() >= normal.z.abs() {
        normal.y
    } else {
        normal.z
    };
    if dom < 0.0 {
        normal = -normal;
    }
    Some((segs, normal))
}

/// Largest distance between two points (the outline's diameter).
fn spread(points: &[Point3]) -> f64 {
    let mut best = 0.0_f64;
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            best = best.max((*b - *a).magnitude());
        }
    }
    best
}

/// The ordinates of an outline given as segment pairs in a plane with
/// normal `normal`.
fn measure_outline(face: u32, segs3: &[Point3], normal: Vector3) -> Option<ProfileTable> {
    // The diameter's two ends: the leading and trailing edge, in some order.
    let (mut ia, mut ib, mut far) = (0usize, 0usize, 0.0_f64);
    for i in 0..segs3.len() {
        for j in i + 1..segs3.len() {
            let d = (segs3[j] - segs3[i]).magnitude();
            if d > far {
                (ia, ib, far) = (i, j, d);
            }
        }
    }
    if far <= 1e-9 {
        return None;
    }
    let (a, b) = (segs3[ia], segs3[ib]);
    // Thickness of the outline just inside each end: the leading edge is
    // the rounded (thick) end, the trailing edge the thin one.
    let span_near = |from: Point3, to: Point3| -> f64 {
        let u = (to - from) * (1.0 / (to - from).magnitude());
        let v = normal.cross(&u);
        let window = 0.02 * far;
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in segs3 {
            let d = *p - from;
            if d.dot(&u) <= window {
                let y = d.dot(&v);
                lo = lo.min(y);
                hi = hi.max(y);
            }
        }
        if hi >= lo {
            hi - lo
        } else {
            0.0
        }
    };
    let (le, te_far) = if span_near(a, b) >= span_near(b, a) {
        (a, b)
    } else {
        (b, a)
    };
    // The trailing edge: the mean of the outline's points at the far end (the
    // two corners of an open trailing edge, or the single point of a sharp
    // one), so the chord runs to the middle of the trailing edge.
    let u0 = (te_far - le) * (1.0 / (te_far - le).magnitude());
    let umax = segs3
        .iter()
        .map(|p| (*p - le).dot(&u0))
        .fold(f64::NEG_INFINITY, f64::max);
    let tail: Vec<Point3> = segs3
        .iter()
        .copied()
        .filter(|p| (*p - le).dot(&u0) >= umax - 1e-3 * far)
        .collect();
    let mut te = Point3::new(0.0, 0.0, 0.0);
    for p in &tail {
        te = Point3::new(te.x + p.x, te.y + p.y, te.z + p.z);
    }
    let k = 1.0 / tail.len().max(1) as f64;
    let te = Point3::new(te.x * k, te.y * k, te.z * k);
    let chord_vec = te - le;
    let chord = chord_vec.magnitude();
    if chord <= 1e-9 {
        return None;
    }
    let u = chord_vec * (1.0 / chord);
    let v = normal.cross(&u);

    let segs: Vec<Seg> = segs3
        .chunks_exact(2)
        .map(|s| {
            let p = s[0] - le;
            let q = s[1] - le;
            ([p.dot(&u), p.dot(&v)], [q.dot(&u), q.dot(&v)])
        })
        .collect();
    let cut = |x: f64| -> Option<(f64, f64)> {
        let mut ys: Vec<f64> = Vec::new();
        for (p, q) in &segs {
            let (lo, hi) = if p[0] <= q[0] { (p, q) } else { (q, p) };
            if x < lo[0] || x > hi[0] {
                continue;
            }
            if (hi[0] - lo[0]).abs() < 1e-12 {
                ys.push(lo[1]);
                ys.push(hi[1]);
            } else {
                let t = (x - lo[0]) / (hi[0] - lo[0]);
                ys.push(lo[1] + t * (hi[1] - lo[1]));
            }
        }
        let up = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let dn = ys.iter().copied().fold(f64::INFINITY, f64::min);
        (up >= dn).then_some((up, dn))
    };
    let edge_eps = 1e-6 * chord;
    let mut stations = Vec::new();
    for pct in STATIONS_PERCENT {
        let x = (pct / 100.0 * chord).clamp(edge_eps, chord - edge_eps);
        let (up, dn) = if pct == 0.0 { (0.0, 0.0) } else { cut(x)? };
        stations.push(ProfileStation {
            percent: pct,
            x_mm: pct / 100.0 * chord,
            upper_mm: up,
            lower_mm: dn,
        });
    }
    let (mut max_t, mut max_at) = (0.0_f64, 0.0_f64);
    for i in 1..200 {
        let pct = i as f64 * 0.5;
        if let Some((up, dn)) = cut(pct / 100.0 * chord) {
            if up - dn > max_t {
                max_t = up - dn;
                max_at = pct;
            }
        }
    }
    let te_thickness = cut(chord - edge_eps).map_or(0.0, |(up, dn)| up - dn);
    Some(ProfileTable {
        owner_view: None,
        face,
        chord_mm: chord,
        max_thickness_mm: max_t,
        max_thickness_at_percent: max_at,
        te_thickness_mm: te_thickness,
        stations,
    })
}
