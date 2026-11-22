//! A DETAIL view (ISO 128-3): a small region of a view -- circled and
//! lettered on its parent -- drawn again enlarged, where features too close
//! together to read at the sheet's scale are dimensioned.
//!
//! [`attach_detail_views`] makes them on the automatic sheet: features whose
//! locations the laid-out sheet prints on each other's lines are circled on
//! their view, drawn again at the ISO scale that parts them, and located
//! there by chains from the nearest one (which stays on the parent, from the
//! datum) -- dimensions the certificate re-measures and completeness credits.
//!
//! The geometry of the cut: everything of the parent view inside the
//! detail's circle, nothing outside it. A polyline is clipped to
//! the disc (one that leaves and comes back in is two pieces); a circle or an
//! ellipse wholly inside is kept as it is, one that crosses the boundary
//! becomes the arcs of it inside, one wholly outside is dropped; a centre
//! line's segments are clipped like polylines.

use super::types::{Polyline2d, ProjectedCircle, ProjectedEllipse};

/// Points sampled round a full circle or ellipse that the boundary cuts.
const ARC_SAMPLES: usize = 96;

/// The part of the segment `p -> q` inside the disc `(c, r)`, as the
/// parameters `(t0, t1)` along it, or `None` when it does not enter.
fn segment_in_disc(p: [f64; 2], q: [f64; 2], c: [f64; 2], r: f64) -> Option<(f64, f64)> {
    let d = [q[0] - p[0], q[1] - p[1]];
    let f = [p[0] - c[0], p[1] - c[1]];
    let a = d[0] * d[0] + d[1] * d[1];
    let fr = f[0] * f[0] + f[1] * f[1] - r * r;
    if a < 1e-24 {
        return (fr <= 0.0).then_some((0.0, 1.0));
    }
    let b = 2.0 * (f[0] * d[0] + f[1] * d[1]);
    let disc = b * b - 4.0 * a * fr;
    if disc <= 0.0 {
        return None;
    }
    let s = disc.sqrt();
    let t0 = ((-b - s) / (2.0 * a)).max(0.0);
    let t1 = ((-b + s) / (2.0 * a)).min(1.0);
    (t1 > t0).then_some((t0, t1))
}

fn lerp(p: [f64; 2], q: [f64; 2], t: f64) -> [f64; 2] {
    [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]
}

/// The pieces of the polyline `points` inside the disc `(c, r)`, in order.
pub(crate) fn clip_polyline(points: &[[f64; 2]], c: [f64; 2], r: f64) -> Vec<Vec<[f64; 2]>> {
    let mut pieces: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    for w in points.windows(2) {
        let (p, q) = (w[0], w[1]);
        match segment_in_disc(p, q, c, r) {
            Some((t0, t1)) => {
                let a = lerp(p, q, t0);
                let b = lerp(p, q, t1);
                // A gap since the last point kept: a new piece.
                let joins = current
                    .last()
                    .is_some_and(|l| (l[0] - a[0]).abs() < 1e-9 && (l[1] - a[1]).abs() < 1e-9);
                if !joins && !current.is_empty() {
                    pieces.push(std::mem::take(&mut current));
                }
                if current.is_empty() {
                    current.push(a);
                }
                current.push(b);
            }
            None => {
                if current.len() >= 2 {
                    pieces.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
        }
    }
    if current.len() >= 2 {
        pieces.push(current);
    }
    pieces.retain(|p| p.len() >= 2);
    pieces
}

/// What is left of a closed curve inside the disc: kept whole, its inside
/// arcs as polylines, or nothing.
pub(crate) enum ClippedCurve<T> {
    Whole(T),
    Arcs(Vec<Vec<[f64; 2]>>),
    Gone,
}

/// A circle clipped to the disc `(c, r)`.
pub(crate) fn clip_circle(
    circle: &ProjectedCircle,
    c: [f64; 2],
    r: f64,
) -> ClippedCurve<ProjectedCircle> {
    let dist = (circle.cx - c[0]).hypot(circle.cy - c[1]);
    if dist + circle.r <= r {
        return ClippedCurve::Whole(circle.clone());
    }
    if dist >= r + circle.r {
        return ClippedCurve::Gone;
    }
    let ring: Vec<[f64; 2]> = (0..=ARC_SAMPLES)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / ARC_SAMPLES as f64;
            [
                circle.cx + circle.r * a.cos(),
                circle.cy + circle.r * a.sin(),
            ]
        })
        .collect();
    let arcs = clip_polyline(&ring, c, r);
    if arcs.is_empty() {
        ClippedCurve::Gone
    } else {
        ClippedCurve::Arcs(arcs)
    }
}

/// An ellipse clipped to the disc `(c, r)`.
pub(crate) fn clip_ellipse(
    ellipse: &ProjectedEllipse,
    c: [f64; 2],
    r: f64,
) -> ClippedCurve<ProjectedEllipse> {
    let dist = (ellipse.cx - c[0]).hypot(ellipse.cy - c[1]);
    let reach = ellipse.rx.max(ellipse.ry);
    if dist + reach <= r {
        return ClippedCurve::Whole(ellipse.clone());
    }
    if dist >= r + reach {
        return ClippedCurve::Gone;
    }
    let (s, co) = ellipse.rotation.sin_cos();
    let ring: Vec<[f64; 2]> = (0..=ARC_SAMPLES)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / ARC_SAMPLES as f64;
            let (x, y) = (ellipse.rx * a.cos(), ellipse.ry * a.sin());
            [ellipse.cx + x * co - y * s, ellipse.cy + x * s + y * co]
        })
        .collect();
    let arcs = clip_polyline(&ring, c, r);
    if arcs.is_empty() {
        ClippedCurve::Gone
    } else {
        ClippedCurve::Arcs(arcs)
    }
}

/// Polylines of the pieces, each at least two points apart.
pub(crate) fn as_polylines(pieces: Vec<Vec<[f64; 2]>>) -> Vec<Polyline2d> {
    pieces
        .into_iter()
        .map(Polyline2d::from_points)
        .filter(|p| p.points.len() >= 2)
        .collect()
}

/// Paper a detail puts between its two closest features, mm: three times
/// the 0.5 mm under which two extension lines read as one
/// ([`super::layout::EXT_LINES_APART_MM`]).
const DETAIL_FEATURE_GAP_MM: f64 = 1.5;

/// Largest detail circle on paper (its diameter), mm.
const DETAIL_MAX_PAPER_MM: f64 = 150.0;

/// The smallest circle about a cluster of features, mm on the part: room
/// round them to read the geometry they sit in.
const DETAIL_MIN_RADIUS_MM: f64 = 4.0;

/// Paper kept round a placed detail for its dimensions and its title, mm.
const DETAIL_PAD_MM: f64 = super::verify::DIM_MARGIN_MM;

/// A location from the datum this module can restate as a chain: a
/// `"position"` with provenance and a datum, not already a chain.
fn chainable(d: &super::dimensioning::Dimension2d) -> bool {
    d.kind == "position" && d.pid.is_some() && d.datum.is_some() && d.chain_from.is_none()
}

/// A length's label: the unit's figures, trailing zeros dropped ("0.99",
/// "1", never "1.00"), as the sheet's other locations print.
fn length_label(unit: crate::units::LengthUnit, v: f64) -> String {
    let s = unit.format_len_bare(v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// `parent`'s geometry inside the disc `(c, r)` (view mm), as a view of its
/// own: polylines and centre lines clipped, circles and ellipses kept whole
/// or cut to their arcs, no dimensions yet. Its extent is the disc's box.
/// Its polylines carry no per-edge provenance (a piece of an edge is not the
/// edge): a readback of the detail refuses rather than guesses.
fn clipped_view(
    parent: &super::types::ProjectedView,
    c: [f64; 2],
    r: f64,
) -> super::types::ProjectedView {
    let mut v = parent.clone();
    let clip_all = |lines: &[Polyline2d]| -> Vec<Polyline2d> {
        lines
            .iter()
            .flat_map(|pl| as_polylines(clip_polyline(&pl.points, c, r)))
            .collect()
    };
    v.polylines = clip_all(&parent.polylines);
    v.hidden_polylines = clip_all(&parent.hidden_polylines);
    v.hatch_polylines = clip_all(&parent.hatch_polylines);
    v.polyline_sources = Vec::new();
    v.circles.clear();
    for circle in &parent.circles {
        match clip_circle(circle, c, r) {
            ClippedCurve::Whole(k) => v.circles.push(k),
            ClippedCurve::Arcs(arcs) => v.polylines.extend(as_polylines(arcs)),
            ClippedCurve::Gone => {}
        }
    }
    v.hidden_circles.clear();
    for circle in &parent.hidden_circles {
        match clip_circle(circle, c, r) {
            ClippedCurve::Whole(k) => v.hidden_circles.push(k),
            ClippedCurve::Arcs(arcs) => v.hidden_polylines.extend(as_polylines(arcs)),
            ClippedCurve::Gone => {}
        }
    }
    v.ellipses.clear();
    for e in &parent.ellipses {
        match clip_ellipse(e, c, r) {
            ClippedCurve::Whole(k) => v.ellipses.push(k),
            ClippedCurve::Arcs(arcs) => v.polylines.extend(as_polylines(arcs)),
            ClippedCurve::Gone => {}
        }
    }
    v.hidden_ellipses.clear();
    for e in &parent.hidden_ellipses {
        match clip_ellipse(e, c, r) {
            ClippedCurve::Whole(k) => v.hidden_ellipses.push(k),
            ClippedCurve::Arcs(arcs) => v.hidden_polylines.extend(as_polylines(arcs)),
            ClippedCurve::Gone => {}
        }
    }
    v.centerlines = parent
        .centerlines
        .iter()
        .filter_map(|cl| {
            let segments: Vec<[f64; 4]> = cl
                .segments
                .iter()
                .flat_map(|s| clip_polyline(&[[s[0], s[1]], [s[2], s[3]]], c, r))
                .filter_map(|piece| {
                    let (a, b) = (piece.first()?, piece.last()?);
                    Some([a[0], a[1], b[0], b[1]])
                })
                .collect();
            (!segments.is_empty()).then(|| super::centerlines::Centerline {
                segments,
                ..cl.clone()
            })
        })
        .collect();
    v.dimensions = Vec::new();
    v.shaded_raster = None;
    v.extent = super::types::ViewExtent {
        min_x: c[0] - r,
        min_y: c[1] - r,
        max_x: c[0] + r,
        max_y: c[1] + r,
    };
    v
}

/// Where a detail of disc `(c, r)` drawn at `k` goes on `drawing`: the view's
/// position of the first free rect a deterministic scan finds, the detail's
/// geometry with [`DETAIL_PAD_MM`] of paper round it for its dimensions and
/// title, clear of every inked item (a dimensioned view with its dimension
/// footprint). `None` when the sheet has no such paper.
fn free_slot(drawing: &super::types::Drawing, c: [f64; 2], r: f64, k: f64) -> Option<[f64; 2]> {
    let w = drawing.sheet().width();
    let h = drawing.sheet().height();
    let (ml, mr, mt, mb) = super::svg::frame_margins(&drawing.sheet_size);
    let layout = super::layout::compute_layout(drawing);
    let obstacles: Vec<super::layout::Rect2> = layout
        .items
        .iter()
        .map(|it| {
            let mut b = it.bbox;
            let dimensioned = it.kind == super::layout::SheetItemKind::ViewGeometry
                && it
                    .owner_view
                    .and_then(|i| drawing.views.get(i))
                    .is_some_and(|v| !v.dimensions.is_empty());
            if dimensioned {
                b.x0 -= super::verify::DIM_MARGIN_MM;
                b.y0 -= super::verify::DIM_MARGIN_MM;
                b.x1 += super::verify::DIM_MARGIN_MM;
                b.y1 += super::verify::DIM_MARGIN_MM;
            }
            b
        })
        .collect();
    const STEP: f64 = 4.0;
    let side = 2.0 * r * k + 2.0 * DETAIL_PAD_MM;
    let (x_lo, x_hi) = (ml + 2.0, w - mr - 2.0 - side);
    let (y_lo, y_hi) = (mt + 2.0, h - mb - 2.0 - side);
    if x_hi <= x_lo || y_hi <= y_lo {
        return None;
    }
    let nx = ((x_hi - x_lo) / STEP).floor() as usize;
    let ny = ((y_hi - y_lo) / STEP).floor() as usize;
    for ix in (0..=nx).rev() {
        let x0 = x_lo + ix as f64 * STEP;
        for iy in 0..=ny {
            let y0 = y_lo + iy as f64 * STEP;
            let cand = super::layout::Rect2 {
                x0,
                y0,
                x1: x0 + side,
                y1: y0 + side,
            };
            if obstacles.iter().any(|o| cand.intersects(o, 0.0)) {
                continue;
            }
            // The geometry sits in the middle of the free rect: invert the
            // render transform (sheet_x = pos.x + x * k, sheet_y_top =
            // (h - pos.y) - y * k).
            let geo_x0 = x0 + DETAIL_PAD_MM;
            let geo_y_top = y0 + DETAIL_PAD_MM;
            return Some([geo_x0 - (c[0] - r) * k, h - geo_y_top - (c[1] + r) * k]);
        }
    }
    None
}

/// Union-find root of `k` in `parent_of`.
fn cluster_root(
    parent_of: &std::collections::HashMap<(usize, String), (usize, String)>,
    k: (usize, String),
) -> (usize, String) {
    let mut cur = k;
    while let Some(next) = parent_of.get(&cur) {
        if *next == cur {
            break;
        }
        cur = next.clone();
    }
    cur
}

/// Draw a DETAIL view for each cluster of locations the laid-out sheet
/// prints on each other's lines
/// ([`super::verify::find_dimensions_on_each_others_lines`]: the same test as
/// the quality check, so the two cannot disagree): features too close
/// together to read from the datum at the sheet's scale.
///
/// The cluster's nearest location stays on the parent view, from the datum;
/// each other one is restated in the detail as a CHAIN from it
/// ([`super::dimensioning::Dimension2d::chain_from`]) -- the certificate
/// re-measures both ends live, and completeness credits the far feature only
/// while the anchor is located. A radius callout whose arrow falls inside
/// the circle moves into the detail with them. The detail is drawn at the
/// smallest ISO scale that puts its closest features
/// [`DETAIL_FEATURE_GAP_MM`] apart on paper, on free paper of the sheet; a
/// cluster whose locations do not share one datum and one axis, that no
/// circle of at most [`DETAIL_MAX_PAPER_MM`] separates, or that finds no
/// paper, is left as it is -- and the quality check still reports it.
pub(crate) fn attach_detail_views(drawing: &mut super::types::Drawing) {
    use std::collections::HashMap;
    let layout = super::layout::compute_layout(drawing);
    let pairs = super::verify::find_dimensions_on_each_others_lines(&layout);
    if pairs.is_empty() {
        return;
    }
    let dim_key = |i: usize| -> Option<(usize, String)> {
        let pd = layout.dimensions.get(i)?;
        let id = pd.dim_id.strip_suffix("-note").unwrap_or(&pd.dim_id);
        Some((pd.owner_view, id.to_string()))
    };
    // Clusters per view: the dimensions joined by being on each other's lines.
    let mut parent_of: HashMap<(usize, String), (usize, String)> = HashMap::new();
    for &(i, j) in &pairs {
        let (Some(a), Some(b)) = (dim_key(i), dim_key(j)) else {
            continue;
        };
        parent_of.entry(a.clone()).or_insert_with(|| a.clone());
        parent_of.entry(b.clone()).or_insert_with(|| b.clone());
        let (ra, rb) = (cluster_root(&parent_of, a), cluster_root(&parent_of, b));
        if ra != rb {
            parent_of.insert(ra, rb);
        }
    }
    let mut grouped: HashMap<(usize, String), Vec<String>> = HashMap::new();
    for k in parent_of.keys() {
        let r = cluster_root(&parent_of, k.clone());
        grouped.entry(r).or_default().push(k.1.clone());
    }
    let mut clusters: Vec<(usize, Vec<String>)> = grouped
        .into_iter()
        .map(|((vi, _), mut ids)| {
            ids.sort();
            (vi, ids)
        })
        .collect();
    clusters.sort();

    let unit = drawing.document_unit;
    for (vi, ids) in clusters {
        let Some(parent) = drawing.views.get(vi) else {
            continue;
        };
        // Principal views only: not a section, a pictorial or a detail.
        if matches!(
            parent.projection,
            super::types::ProjectionType::Isometric | super::types::ProjectionType::Custom { .. }
        ) || drawing.details.iter().any(|d| d.detail_view_idx == vi)
        {
            continue;
        }
        let mut members: Vec<super::dimensioning::Dimension2d> = parent
            .dimensions
            .iter()
            .filter(|d| ids.contains(&d.id) && chainable(d))
            .cloned()
            .collect();
        if members.len() < 2 {
            continue;
        }
        // One datum, one axis, one sense: then the difference of two
        // locations is the distance between their features.
        let along = |d: &super::dimensioning::Dimension2d| [d.b[0] - d.a[0], d.b[1] - d.a[1]];
        let first = along(&members[0]);
        let axis = if first[0].abs() >= first[1].abs() {
            0
        } else {
            1
        };
        let one_line = members.iter().all(|d| {
            let s = along(d);
            let same_datum = match (&d.datum, &members[0].datum) {
                (Some(x), Some(y)) => (0..3).all(|i| (x.origin[i] - y.origin[i]).abs() <= 1e-6),
                _ => false,
            };
            same_datum && s[1 - axis].abs() < 1e-6 && s[axis] * first[axis] > 0.0
        });
        if !one_line {
            continue;
        }
        members.sort_by(|a, b| {
            a.value
                .partial_cmp(&b.value)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let gap = members
            .windows(2)
            .map(|w| w[1].value - w[0].value)
            .fold(f64::INFINITY, f64::min);
        if !(gap > 1e-6) {
            continue;
        }
        // The circle: round the features, with room to read what they sit in.
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for d in &members {
            for i in 0..2 {
                lo[i] = lo[i].min(d.b[i]);
                hi[i] = hi[i].max(d.b[i]);
            }
        }
        let centre = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
        let half_diag = 0.5 * (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
        let radius = (1.3 * half_diag + 2.0).max(DETAIL_MIN_RADIUS_MM);
        // The smallest ISO scale that separates the closest features, at
        // least twice the parent's.
        let need = (DETAIL_FEATURE_GAP_MM / gap).max(2.0 * parent.scale);
        let Some(k) = super::dimensioning::ISO_SCALES
            .iter()
            .rev()
            .copied()
            .find(|&s| s >= need - 1e-12)
            .filter(|&s| 2.0 * radius * s <= DETAIL_MAX_PAPER_MM)
        else {
            continue;
        };
        let Some(position) = free_slot(drawing, centre, radius, k) else {
            continue;
        };
        let letter = next_letter(drawing);
        let parent = &drawing.views[vi];
        let mut detail = clipped_view(parent, centre, radius);
        detail.id = super::types::ProjectedViewId::new();
        detail.name = format!("DETAIL {letter}");
        detail.scale = k;
        detail.position_mm = position;
        // The chains, from the nearest feature, at its across-axis place so
        // each spans only the axis it measures.
        let anchor = members[0].clone();
        let mut moved: Vec<String> = Vec::new();
        for far in &members[1..] {
            let value = (far.value - anchor.value).abs();
            let mut b = anchor.b;
            b[axis] = far.b[axis];
            detail.dimensions.push(super::dimensioning::Dimension2d {
                id: format!("{}-chain", far.id),
                value,
                label: length_label(unit, value),
                a: anchor.b,
                b,
                key: None,
                tolerance: None,
                members: Vec::new(),
                member_points: Vec::new(),
                chain_from: anchor.pid.clone(),
                ..far.clone()
            });
            moved.push(far.id.clone());
        }
        // Radius callouts whose arrow lands inside the circle.
        for d in &parent.dimensions {
            let point_callout =
                d.kind == "radius" || super::dimensioning::is_blend_callout(&d.kind);
            if point_callout && (d.a[0] - centre[0]).hypot(d.a[1] - centre[1]) <= radius {
                detail.dimensions.push(d.clone());
                moved.push(d.id.clone());
            }
        }
        let detail_idx = drawing.views.len();
        drawing.views[vi]
            .dimensions
            .retain(|d| !moved.contains(&d.id));
        drawing.views.push(detail);
        drawing.details.push(super::types::DetailSemantics {
            letter,
            parent_view_idx: vi,
            detail_view_idx: detail_idx,
            centre,
            radius,
        });
    }
}

/// The next unused view letter: SECTION A-A takes "A"; I, O and Q are not
/// used (they read as 1 and 0).
fn next_letter(drawing: &super::types::Drawing) -> String {
    let taken = |c: char| {
        (c == 'A' && drawing.section.is_some())
            || drawing.details.iter().any(|d| d.letter.starts_with(c))
    };
    ('A'..='Z')
        .filter(|&c| !matches!(c, 'I' | 'O' | 'Q'))
        .find(|&c| !taken(c))
        .map(String::from)
        .unwrap_or_else(|| "Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(p: [f64; 2], c: [f64; 2], r: f64) -> bool {
        (p[0] - c[0]).hypot(p[1] - c[1]) <= r + 1e-9
    }

    /// A line through the disc keeps its chord; one past it is dropped; one
    /// wholly inside is kept as it is.
    #[test]
    fn a_line_keeps_its_chord_inside_the_circle() {
        let c = [0.0, 0.0];
        let through = clip_polyline(&[[-10.0, 0.0], [10.0, 0.0]], c, 5.0);
        assert_eq!(through, vec![vec![[-5.0, 0.0], [5.0, 0.0]]]);
        assert!(clip_polyline(&[[-10.0, 6.0], [10.0, 6.0]], c, 5.0).is_empty());
        let within = clip_polyline(&[[-1.0, 1.0], [1.0, 1.0], [1.0, -1.0]], c, 5.0);
        assert_eq!(within, vec![vec![[-1.0, 1.0], [1.0, 1.0], [1.0, -1.0]]]);
    }

    /// A polyline that leaves the circle and comes back in is two pieces,
    /// not one line drawn across the outside.
    ///
    /// Mutation: treat every kept segment as joining the last (`joins`
    /// always true) -> one piece -> RED.
    #[test]
    fn a_polyline_leaving_and_reentering_is_two_pieces() {
        let c = [0.0, 0.0];
        // A V poking out of the circle and straight back in: no segment of
        // it lies wholly outside, so only the gap between the exit and the
        // re-entry splits it.
        let v = [[-3.0, 0.0], [0.0, 10.0], [3.0, 0.0]];
        let pieces = clip_polyline(&v, c, 5.0);
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        for piece in &pieces {
            for p in piece {
                assert!(inside(*p, c, 5.0), "{p:?} outside");
            }
        }
    }

    /// A circle wholly inside is kept as a circle; one crossing the boundary
    /// leaves only arcs inside; one outside is gone.
    #[test]
    fn a_circle_is_kept_cut_or_dropped() {
        let c = [0.0, 0.0];
        let circle = |cx: f64, r: f64| ProjectedCircle {
            cx,
            cy: 0.0,
            r,
            face_ids: Vec::new(),
        };
        assert!(matches!(
            clip_circle(&circle(1.0, 2.0), c, 5.0),
            ClippedCurve::Whole(_)
        ));
        assert!(matches!(
            clip_circle(&circle(20.0, 2.0), c, 5.0),
            ClippedCurve::Gone
        ));
        let cut = clip_circle(&circle(5.0, 2.0), c, 5.0);
        assert!(
            matches!(cut, ClippedCurve::Arcs(_)),
            "a crossing circle leaves arcs"
        );
        if let ClippedCurve::Arcs(arcs) = cut {
            assert!(!arcs.is_empty());
            for p in arcs.iter().flatten() {
                assert!(inside(*p, c, 5.0), "{p:?} outside");
            }
        }
    }
}

#[cfg(test)]
mod build_tests {
    use super::*;
    use crate::drawing::dimensioning::Dimension2d;
    use crate::drawing::types::{
        Drawing, ProjectedView, ProjectedViewId, ProjectionType, SheetSize, ViewExtent, ViewSource,
    };

    fn front(scale: f64) -> ProjectedView {
        let outline = Polyline2d::from_points(vec![
            [-40.0, -20.0],
            [40.0, -20.0],
            [40.0, 20.0],
            [-40.0, 20.0],
            [-40.0, -20.0],
        ]);
        ProjectedView {
            id: ProjectedViewId::new(),
            name: "FRONT".to_string(),
            projection: ProjectionType::Front,
            source: ViewSource::Part {
                part_id: uuid::Uuid::nil(),
                solid_id: 0,
            },
            position_mm: [120.0, 150.0],
            scale,
            polylines: vec![outline],
            extent: ViewExtent {
                min_x: -40.0,
                min_y: -20.0,
                max_x: 40.0,
                max_y: 20.0,
            },
            dimensions: Vec::new(),
            centerlines: Vec::new(),
            hidden_polylines: Vec::new(),
            circles: Vec::new(),
            hidden_circles: Vec::new(),
            ellipses: Vec::new(),
            hidden_ellipses: Vec::new(),
            show_hidden: true,
            shaded_raster: None,
            hatch_polylines: Vec::new(),
            polyline_sources: Vec::new(),
        }
    }

    /// A location along X from the part corner (-40, y) to a feature at x.
    fn location(id: &str, x: f64, y: f64) -> Dimension2d {
        Dimension2d {
            id: id.to_string(),
            kind: "position".to_string(),
            value: x + 40.0,
            unit: "mm".to_string(),
            label: format!("{}", x + 40.0),
            a: [-40.0, y],
            b: [x, y],
            entities: vec![1],
            axis3: None,
            dir3: None,
            pid: Some(format!("pid-{id}")),
            datum: Some(crate::readable::dimensions::DatumDescriptor {
                kind: "part_corner".to_string(),
                name: "part corner".to_string(),
                origin: [0.0; 3],
            }),
            tolerance: None,
            key: None,
            members: Vec::new(),
            member_points: Vec::new(),
            chain_from: None,
        }
    }

    /// The detail view is reached in production: the automatic sheet build
    /// attaches it, once, after every other item is placed (it searches the
    /// finished sheet for free paper).
    #[test]
    fn the_sheet_build_attaches_detail_views() {
        let source = include_str!("dimensioning.rs").replace("\r\n", "\n");
        // Everything before the test module (the file has test-only helpers
        // further up, each under its own `#[cfg(test)]`).
        let production = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("split always yields a first element");
        assert_eq!(
            production
                .matches("super::detail_view::attach_detail_views(&mut drawing);")
                .count(),
            1
        );
        let detail = production
            .find("attach_detail_views(&mut drawing)")
            .expect("called");
        let pictorial = production
            .find("attach_pictorial_iso(model, solid_id, source, &mut drawing")
            .expect("the pictorial is attached");
        assert!(
            pictorial < detail,
            "the detail is placed after the pictorial"
        );
    }

    /// Three features 0.3 mm apart, located from one datum, drawn 1:2: their
    /// extension lines print on each other's (the precondition, checked).
    /// The detail restates them so nothing prints on another's lines: the
    /// nearest stays on FRONT from the datum, the others are chains from it
    /// in DETAIL B, drawn large enough to part them.
    ///
    /// Mutation: make `attach_detail_views` return at once -> the pairs stay
    /// -> RED.
    #[test]
    fn features_too_close_to_read_get_a_detail_view_with_chains() {
        let mut v = front(0.5);
        v.dimensions = vec![
            location("near", 10.0, 5.0),
            location("mid", 10.3, 8.0),
            location("far", 10.6, 11.0),
        ];
        let mut d = Drawing::new("detail", SheetSize::A3);
        d.add_view(v);
        let before = crate::drawing::verify::find_dimensions_on_each_others_lines(
            &crate::drawing::layout::compute_layout(&d),
        );
        assert!(
            !before.is_empty(),
            "fixture: the locations print on each other's lines"
        );

        attach_detail_views(&mut d);

        let after = crate::drawing::verify::find_dimensions_on_each_others_lines(
            &crate::drawing::layout::compute_layout(&d),
        );
        assert!(after.is_empty(), "still on each other's lines: {after:?}");
        assert_eq!(d.details.len(), 1, "one detail");
        let detail = &d.details[0];
        let view = &d.views[detail.detail_view_idx];
        assert!(view.name.starts_with("DETAIL "), "{}", view.name);
        assert!(
            view.scale >= 5.0,
            "parts 0.3 mm by 1.5 mm on paper: {}",
            view.scale
        );
        let anchor = d.views[0]
            .dimensions
            .iter()
            .find(|x| x.id == "near")
            .expect("the anchor stays on FRONT");
        assert!(anchor.chain_from.is_none());
        assert!(!view.dimensions.is_empty());
        for chain in &view.dimensions {
            assert_eq!(chain.chain_from.as_deref(), Some("pid-near"), "{chain:?}");
        }
        let values: Vec<f64> = view.dimensions.iter().map(|x| x.value).collect();
        assert!(
            values
                .iter()
                .all(|v| (*v - 0.3).abs() < 1e-9 || (*v - 0.6).abs() < 1e-9),
            "{values:?}"
        );
    }
}
