//! Section (cross-section) views for engineering drawings.
//!
//! A section view cuts the solid on a plane and draws the resulting filled
//! cross-section: the cut OUTLINE (boundary of the sectioned material) plus
//! 45° HATCHING over the solid region — the ISO 128 convention that
//! distinguishes "cut solid material" from voids/holes. Unlike the projected
//! orthographic views, a section reveals INTERNAL features (bores, counterbores,
//! webs) that hidden lines only hint at.
//!
//! The cut itself reuses the kernel's [`section_solid_by_plane`], which returns
//! the cross-section as triangulated [`SectionCap`]s lying on the plane. We
//! project those into the plane's own 2D frame, trace the boundary for the
//! outline, and scan-fill the triangles for the hatch.

use crate::math::{Point3, Tolerance, Vector3};
use crate::operations::section::{section_solid_by_plane, SectionCap};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

use super::types::{
    Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, ViewExtent, ViewSource,
};

/// Hatch line spacing in model units (pre-scale). Lands at a sensible on-sheet
/// pitch (~2–3 mm) for the small/medium parts the drawing module targets.
const HATCH_SPACING: f64 = 4.0;

/// Build a SECTION view: cut `solid_id` on the plane `(origin, normal)` and
/// return a [`ProjectedView`] whose polylines are the cut outline plus 45°
/// hatching, placed at `pos` (sheet mm) with the given `scale`.
///
/// The view's 2D frame is the plane's in-plane basis `(u, v)` with `u =
/// normal.perpendicular()`; world point `p` maps to `(p−origin)·u, (p−origin)·v`.
/// Returns `Ok(None)` when the plane misses the solid (no material to section).
pub fn section_view(
    model: &BRepModel,
    solid_id: SolidId,
    part_id: uuid::Uuid,
    plane_origin: Point3,
    plane_normal: Vector3,
    name: &str,
    pos: [f64; 2],
    scale: f64,
) -> Option<ProjectedView> {
    let caps = section_solid_by_plane(
        model,
        solid_id,
        plane_origin,
        plane_normal,
        Tolerance::default(),
    )
    .ok()?;
    if caps.is_empty() {
        return None;
    }

    let n = plane_normal.normalize().ok()?;
    let u = n.perpendicular().normalize().ok()?;
    let v = n.cross(&u);
    let to2d = |p: Point3| -> [f64; 2] {
        let d = p - plane_origin;
        [d.dot(&u), d.dot(&v)]
    };

    // Project every cap's triangles into the plane frame (2D), keeping triangle
    // connectivity for the hatch scan-fill and counting undirected edges for the
    // boundary outline.
    let mut tris2d: Vec<[[f64; 2]; 3]> = Vec::new();
    let mut edge_count: std::collections::HashMap<(i64, i64, i64, i64), ([f64; 2], [f64; 2], u32)> =
        std::collections::HashMap::new();
    let q = |a: f64| (a * 1e4).round() as i64;
    for cap in &caps {
        push_cap(cap, &to2d, &mut tris2d, &mut edge_count, &q);
    }
    if tris2d.is_empty() {
        return None;
    }

    let mut polylines: Vec<Polyline2d> = Vec::new();
    // Outline = edges used by exactly one triangle (the section boundary).
    for (a, b, c) in edge_count.values() {
        if *c == 1 {
            polylines.push(Polyline2d::from_points(vec![*a, *b]));
        }
    }

    // 2D extent of the section.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for t in &tris2d {
        for p in t {
            min_x = min_x.min(p[0]);
            min_y = min_y.min(p[1]);
            max_x = max_x.max(p[0]);
            max_y = max_y.max(p[1]);
        }
    }

    // 45° hatch (direction (1,1)): lines of constant c = y − x. For each line,
    // clip against every triangle and draw the covered intervals — using the
    // triangles (not even-odd) so holes/bores stay correctly UN-hatched.
    let c_lo = min_y - max_x;
    let c_hi = max_y - min_x;
    let mut c = (c_lo / HATCH_SPACING).ceil() * HATCH_SPACING;
    while c < c_hi {
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for t in &tris2d {
            if let Some((s0, s1)) = tri_hatch_span(t, c) {
                spans.push((s0.min(s1), s0.max(s1)));
            }
        }
        // Merge overlapping spans (parametrised by x along the line y = x + c).
        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut cur: Option<(f64, f64)> = None;
        for (s0, s1) in spans {
            match cur {
                Some((_, e1)) if s0 <= e1 + 1e-9 => {
                    cur = Some((cur.unwrap().0, e1.max(s1)));
                }
                Some((b0, b1)) => {
                    polylines.push(Polyline2d::from_points(vec![[b0, b0 + c], [b1, b1 + c]]));
                    cur = Some((s0, s1));
                }
                None => cur = Some((s0, s1)),
            }
        }
        if let Some((b0, b1)) = cur {
            polylines.push(Polyline2d::from_points(vec![[b0, b0 + c], [b1, b1 + c]]));
        }
        c += HATCH_SPACING;
    }

    Some(ProjectedView {
        id: ProjectedViewId::new(),
        name: name.to_string(),
        projection: ProjectionType::Custom {
            rotation: [u.x, u.y, u.z, v.x, v.y, v.z, n.x, n.y, n.z],
        },
        source: ViewSource::Part { part_id, solid_id },
        position_mm: pos,
        scale,
        polylines,
        extent: ViewExtent {
            min_x,
            min_y,
            max_x,
            max_y,
        },
        dimensions: Vec::new(),
        centerlines: Vec::new(),
        hidden_polylines: Vec::new(),
        circles: Vec::new(),
        hidden_circles: Vec::new(),
    })
}

#[allow(clippy::type_complexity)]
fn push_cap(
    cap: &SectionCap,
    to2d: &impl Fn(Point3) -> [f64; 2],
    tris2d: &mut Vec<[[f64; 2]; 3]>,
    edge_count: &mut std::collections::HashMap<(i64, i64, i64, i64), ([f64; 2], [f64; 2], u32)>,
    q: &impl Fn(f64) -> i64,
) {
    for idx in &cap.indices {
        let p: [[f64; 2]; 3] = [
            to2d(cap.vertices[idx[0] as usize]),
            to2d(cap.vertices[idx[1] as usize]),
            to2d(cap.vertices[idx[2] as usize]),
        ];
        tris2d.push(p);
        for k in 0..3 {
            let a = p[k];
            let b = p[(k + 1) % 3];
            let (ka, kb) = ((q(a[0]), q(a[1])), (q(b[0]), q(b[1])));
            let key = if ka <= kb {
                (ka.0, ka.1, kb.0, kb.1)
            } else {
                (kb.0, kb.1, ka.0, ka.1)
            };
            edge_count
                .entry(key)
                .and_modify(|e| e.2 += 1)
                .or_insert((a, b, 1));
        }
    }
}

/// Intersect the line `y = x + c` with triangle `t`; return the x-span
/// `[x0, x1]` of the covered segment, or `None` if the line misses.
fn tri_hatch_span(t: &[[f64; 2]; 3], c: f64) -> Option<(f64, f64)> {
    // f(p) = p.y − p.x − c; the line is f = 0. Collect crossings on the edges.
    let f = |p: &[f64; 2]| p[1] - p[0] - c;
    let fv = [f(&t[0]), f(&t[1]), f(&t[2])];
    let mut xs: Vec<f64> = Vec::with_capacity(2);
    for k in 0..3 {
        let (a, b) = (t[k], t[(k + 1) % 3]);
        let (fa, fb) = (fv[k], fv[(k + 1) % 3]);
        if (fa <= 0.0 && fb > 0.0) || (fa > 0.0 && fb <= 0.0) {
            let s = fa / (fa - fb);
            xs.push(a[0] + s * (b[0] - a[0]));
        }
    }
    if xs.len() == 2 {
        Some((xs[0], xs[1]))
    } else {
        None
    }
}
