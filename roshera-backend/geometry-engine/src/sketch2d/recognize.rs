//! Analytic sketch shape recognition — the certifiable IDENTITY layer (Track 3).
//!
//! The validity certificate says a sketch is WELL-FORMED (sound); recognition
//! says WHAT it is. Unlike a VLM judgement, this is analytic and *provable*: it
//! measures the sketch's closed boundary from EXACT geometry — entity
//! composition, vertex count, interior angles, edge-length ratios, regularity —
//! and classifies it. This is the first rung of the "is a gear a gear?" ladder:
//! primitive shapes here; gear / house / bracket detectors build on top.

#![allow(clippy::indexing_slicing)]

use super::{Point2d, Sketch};
use serde::{Deserialize, Serialize};

/// Right-angle / angle-equality tolerance, degrees.
const ANGLE_TOL_DEG: f64 = 3.0;
/// Relative edge-length equality tolerance (2%).
const EDGE_REL_TOL: f64 = 0.02;

/// The recognised shape class of a sketch's dominant boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShapeClass {
    /// No drawable geometry.
    Empty,
    /// A single circle entity.
    Circle,
    /// A closed three-sided polygon.
    Triangle,
    /// Four right angles, opposite sides equal.
    Rectangle,
    /// A rectangle with all four sides equal.
    Square,
    /// All edges equal AND all interior angles equal (≥5 sides).
    RegularPolygon { sides: usize },
    /// An N-tooth gear/cog: `2·teeth` vertices whose radii about the centroid
    /// alternate between two distinct tight clusters (tooth tips and valleys)
    /// at regular angular spacing.
    Gear { teeth: usize },
    /// A mounting bracket / plate: a rectangular boundary with `holes` interior
    /// circular holes (all fully inside the plate).
    Bracket { holes: usize },
    /// A generic closed polygon.
    Polygon { sides: usize },
    /// Multiple loops / mixed entities — no single boundary to classify.
    Compound,
}

/// The recognition verdict for a sketch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recognition {
    pub class: ShapeClass,
    /// Boundary vertex count (0 for a circle).
    pub vertices: usize,
    pub closed: bool,
    /// Human-readable basis for the verdict.
    pub evidence: String,
}

fn dist(a: Point2d, b: Point2d) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Max relative spread of a set of positive lengths: `(max-min)/max`.
fn max_rel_spread(vals: &[f64]) -> f64 {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &v in vals {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi <= 1e-12 {
        return 0.0;
    }
    (hi - lo) / hi
}

/// Max relative spread allowed within a gear's tip cluster (and valley cluster).
const GEAR_CLUSTER_TOL: f64 = 0.05;
/// Minimum relative tip-to-valley depth `(R_tip − R_valley)/R_tip` — below this
/// the two clusters are indistinct and the shape is a regular polygon, not a gear.
const GEAR_TOOTH_DEPTH_MIN: f64 = 0.08;
/// Allowed deviation of each angular gap from the regular `TAU/n` spacing.
const GEAR_ANGLE_REL_TOL: f64 = 0.15;

/// Detect an N-tooth gear/cog. Returns the tooth count `N` when `verts` is an
/// even number (≥ 6) of vertices whose radii about the centroid alternate
/// between two DISTINCT, tight clusters (tips and valleys) at regular angular
/// spacing. A regular polygon (one radius cluster) is rejected by the
/// tip-depth gate, so this is safe to test before the polygon classification.
fn detect_gear(verts: &[Point2d]) -> Option<usize> {
    use std::f64::consts::{PI, TAU};
    let n = verts.len();
    if n < 6 || n % 2 != 0 {
        return None;
    }
    let cx = verts.iter().map(|p| p.x).sum::<f64>() / n as f64;
    let cy = verts.iter().map(|p| p.y).sum::<f64>() / n as f64;

    let mut radii = Vec::with_capacity(n);
    let mut angles = Vec::with_capacity(n);
    for p in verts {
        let dx = p.x - cx;
        let dy = p.y - cy;
        radii.push((dx * dx + dy * dy).sqrt());
        angles.push(dy.atan2(dx));
    }

    // Even- and odd-indexed vertices form the two candidate clusters.
    let evens: Vec<f64> = (0..n).step_by(2).map(|i| radii[i]).collect();
    let odds: Vec<f64> = (1..n).step_by(2).map(|i| radii[i]).collect();
    if max_rel_spread(&evens) > GEAR_CLUSTER_TOL || max_rel_spread(&odds) > GEAR_CLUSTER_TOL {
        return None;
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (me, mo) = (mean(&evens), mean(&odds));
    let (r_big, r_small) = if me >= mo { (me, mo) } else { (mo, me) };
    if r_small <= 1e-9 || (r_big - r_small) / r_big < GEAR_TOOTH_DEPTH_MIN {
        return None; // indistinct clusters → regular polygon, not a gear
    }

    // Vertices must be regularly spaced around the centroid (CW or CCW): the
    // minimal angular gap between consecutive vertices ≈ TAU/n.
    let expected = TAU / n as f64;
    for i in 0..n {
        let mut d = (angles[(i + 1) % n] - angles[i]).rem_euclid(TAU);
        if d > PI {
            d = TAU - d;
        }
        if (d - expected).abs() > expected * GEAR_ANGLE_REL_TOL {
            return None;
        }
    }
    Some(n / 2)
}

/// Classify a closed polygon from its ordered (non-repeating) corner vertices.
fn classify_polygon(verts: &[Point2d]) -> ShapeClass {
    let n = verts.len();
    if n < 3 {
        return ShapeClass::Polygon { sides: n };
    }
    // A gear's alternating-radius pattern is checked first; a regular polygon
    // fails the tip-depth gate, so this never steals a RegularPolygon verdict.
    if let Some(teeth) = detect_gear(verts) {
        return ShapeClass::Gear { teeth };
    }
    let mut edges = Vec::with_capacity(n);
    let mut angles = Vec::with_capacity(n);
    for i in 0..n {
        let cur = verts[i];
        let next = verts[(i + 1) % n];
        let prev = verts[(i + n - 1) % n];
        edges.push(dist(cur, next));
        let e1 = (prev.x - cur.x, prev.y - cur.y);
        let e2 = (next.x - cur.x, next.y - cur.y);
        let dot = e1.0 * e2.0 + e1.1 * e2.1;
        let m1 = (e1.0 * e1.0 + e1.1 * e1.1).sqrt();
        let m2 = (e2.0 * e2.0 + e2.1 * e2.1).sqrt();
        let cos = if m1 > 1e-12 && m2 > 1e-12 {
            (dot / (m1 * m2)).clamp(-1.0, 1.0)
        } else {
            1.0
        };
        angles.push(cos.acos().to_degrees());
    }

    let edges_equal = max_rel_spread(&edges) < EDGE_REL_TOL;
    let angle_spread = {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &a in &angles {
            lo = lo.min(a);
            hi = hi.max(a);
        }
        hi - lo
    };
    let angles_equal = angle_spread < ANGLE_TOL_DEG;
    let all_right = angles.iter().all(|a| (a - 90.0).abs() < ANGLE_TOL_DEG);

    match n {
        3 => ShapeClass::Triangle,
        4 if all_right && edges_equal => ShapeClass::Square,
        4 if all_right => ShapeClass::Rectangle,
        _ if edges_equal && angles_equal => ShapeClass::RegularPolygon { sides: n },
        _ => ShapeClass::Polygon { sides: n },
    }
}

/// Recognise the dominant shape of a sketch from its exact geometry.
pub fn recognize_sketch(sketch: &Sketch) -> Recognition {
    let n_circles = sketch.circles().iter().count();
    let n_lines = sketch.lines().iter().count();
    let polys: Vec<(Vec<Point2d>, bool)> = sketch
        .polylines()
        .iter()
        .map(|e| {
            (
                e.value().polyline.vertices.clone(),
                e.value().polyline.is_closed,
            )
        })
        .collect();

    let n_points = sketch.points().iter().count();
    if n_circles == 0 && n_lines == 0 && polys.is_empty() {
        // Bare points (or nothing) — no boundary.
        return Recognition {
            class: ShapeClass::Empty,
            vertices: 0,
            closed: false,
            evidence: format!("no boundary geometry ({n_points} loose points)"),
        };
    }

    // A single lone circle.
    if n_circles == 1 && n_lines == 0 && polys.is_empty() {
        return Recognition {
            class: ShapeClass::Circle,
            vertices: 0,
            closed: true,
            evidence: "exactly one circle entity".to_string(),
        };
    }

    // A single closed polyline is the boundary to classify.
    if n_circles == 0 && n_lines == 0 && polys.len() == 1 {
        if let Some((verts, closed)) = polys.first() {
            if *closed {
                let class = classify_polygon(verts);
                return Recognition {
                    class,
                    vertices: verts.len(),
                    closed: true,
                    evidence: format!("single closed {}-vertex polyline", verts.len()),
                };
            }
            return Recognition {
                class: ShapeClass::Polygon { sides: verts.len() },
                vertices: verts.len(),
                closed: false,
                evidence: "single OPEN polyline".to_string(),
            };
        }
    }

    // A mounting bracket / plate: one rectangular closed boundary plus N
    // interior circular holes, each fully inside the plate. (AABB containment —
    // exact for an axis-aligned plate, the common case.)
    if n_lines == 0 && n_circles >= 1 && polys.len() == 1 {
        if let Some((verts, true)) = polys.first().map(|(v, c)| (v, *c)) {
            if matches!(
                classify_polygon(verts),
                ShapeClass::Rectangle | ShapeClass::Square
            ) {
                let (mut minx, mut miny, mut maxx, mut maxy) = (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                );
                for v in verts {
                    minx = minx.min(v.x);
                    miny = miny.min(v.y);
                    maxx = maxx.max(v.x);
                    maxy = maxy.max(v.y);
                }
                let all_inside = sketch.circles().iter().all(|e| {
                    let c = &e.value().circle;
                    c.center.x - c.radius >= minx
                        && c.center.x + c.radius <= maxx
                        && c.center.y - c.radius >= miny
                        && c.center.y + c.radius <= maxy
                });
                if all_inside {
                    return Recognition {
                        class: ShapeClass::Bracket { holes: n_circles },
                        vertices: verts.len(),
                        closed: true,
                        evidence: format!(
                            "{}-gon plate with {n_circles} interior hole(s)",
                            verts.len()
                        ),
                    };
                }
            }
        }
    }

    Recognition {
        class: ShapeClass::Compound,
        vertices: 0,
        closed: false,
        evidence: format!(
            "{n_circles} circles, {n_lines} lines, {} polylines — no single boundary",
            polys.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::{Point2d, Sketch, SketchAnchor};
    use std::f64::consts::TAU;

    fn poly(verts: Vec<Point2d>) -> Recognition {
        let s = Sketch::new("s".to_string(), SketchAnchor::xy());
        s.add_polyline(verts, true).expect("polyline");
        recognize_sketch(&s)
    }

    #[test]
    fn recognises_a_circle() {
        let s = Sketch::new("c".to_string(), SketchAnchor::xy());
        s.add_circle(Point2d::new(0.0, 0.0), 5.0).expect("circle");
        assert_eq!(recognize_sketch(&s).class, ShapeClass::Circle);
    }

    #[test]
    fn recognises_a_triangle() {
        let r = poly(vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(10.0, 0.0),
            Point2d::new(5.0, 8.0),
        ]);
        assert_eq!(r.class, ShapeClass::Triangle);
    }

    #[test]
    fn recognises_square_vs_rectangle() {
        let sq = poly(vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(10.0, 0.0),
            Point2d::new(10.0, 10.0),
            Point2d::new(0.0, 10.0),
        ]);
        assert_eq!(sq.class, ShapeClass::Square);

        let rect = poly(vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(20.0, 0.0),
            Point2d::new(20.0, 10.0),
            Point2d::new(0.0, 10.0),
        ]);
        assert_eq!(rect.class, ShapeClass::Rectangle);
    }

    #[test]
    fn recognises_a_regular_hexagon() {
        let mut verts = Vec::new();
        for k in 0..6 {
            let t = (k as f64) / 6.0 * TAU;
            verts.push(Point2d::new(10.0 * t.cos(), 10.0 * t.sin()));
        }
        assert_eq!(poly(verts).class, ShapeClass::RegularPolygon { sides: 6 });
    }

    #[test]
    fn recognises_a_six_tooth_gear() {
        // 12 vertices: tooth tips (r=10) and valleys (r=7) alternating, evenly
        // spaced — the canonical build-it-then-recognise gear harness.
        let n = 12;
        let verts: Vec<Point2d> = (0..n)
            .map(|i| {
                let t = (i as f64) / (n as f64) * TAU;
                let r = if i % 2 == 0 { 10.0 } else { 7.0 };
                Point2d::new(r * t.cos(), r * t.sin())
            })
            .collect();
        assert_eq!(poly(verts).class, ShapeClass::Gear { teeth: 6 });
    }

    #[test]
    fn recognises_a_twenty_tooth_gear() {
        let teeth = 20;
        let n = 2 * teeth;
        let verts: Vec<Point2d> = (0..n)
            .map(|i| {
                let t = (i as f64) / (n as f64) * TAU;
                let r = if i % 2 == 0 { 5.0 } else { 4.2 };
                Point2d::new(r * t.cos(), r * t.sin())
            })
            .collect();
        assert_eq!(poly(verts).class, ShapeClass::Gear { teeth: 20 });
    }

    #[test]
    fn an_even_regular_polygon_is_not_mistaken_for_a_gear() {
        // A regular octagon: 8 vertices ALL at the same radius. The tip-depth
        // gate must reject it (no distinct tip/valley clusters) → RegularPolygon.
        let n = 8;
        let verts: Vec<Point2d> = (0..n)
            .map(|i| {
                let t = (i as f64) / (n as f64) * TAU;
                Point2d::new(10.0 * t.cos(), 10.0 * t.sin())
            })
            .collect();
        assert_eq!(poly(verts).class, ShapeClass::RegularPolygon { sides: 8 });
    }

    #[test]
    fn a_shallow_tooth_profile_is_not_a_gear() {
        // tips/valleys differ by ~2% (< GEAR_TOOTH_DEPTH_MIN) — not a gear.
        let n = 12;
        let verts: Vec<Point2d> = (0..n)
            .map(|i| {
                let t = (i as f64) / (n as f64) * TAU;
                let r = if i % 2 == 0 { 10.0 } else { 9.8 };
                Point2d::new(r * t.cos(), r * t.sin())
            })
            .collect();
        assert!(!matches!(poly(verts).class, ShapeClass::Gear { .. }));
    }

    #[test]
    fn recognises_a_bracket_plate_with_four_holes() {
        let s = Sketch::new("br".to_string(), SketchAnchor::xy());
        s.add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(40.0, 0.0),
                Point2d::new(40.0, 20.0),
                Point2d::new(0.0, 20.0),
            ],
            true,
        )
        .expect("plate");
        for (cx, cy) in [(5.0, 5.0), (35.0, 5.0), (35.0, 15.0), (5.0, 15.0)] {
            s.add_circle(Point2d::new(cx, cy), 2.0).expect("hole");
        }
        assert_eq!(recognize_sketch(&s).class, ShapeClass::Bracket { holes: 4 });
    }

    #[test]
    fn a_plate_with_a_hole_escaping_the_boundary_is_not_a_bracket() {
        let s = Sketch::new("br".to_string(), SketchAnchor::xy());
        s.add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(40.0, 0.0),
                Point2d::new(40.0, 20.0),
                Point2d::new(0.0, 20.0),
            ],
            true,
        )
        .expect("plate");
        // A circle straddling the right edge — not fully contained.
        s.add_circle(Point2d::new(39.0, 10.0), 3.0).expect("hole");
        assert!(!matches!(
            recognize_sketch(&s).class,
            ShapeClass::Bracket { .. }
        ));
    }

    #[test]
    fn a_house_outline_is_an_irregular_pentagon() {
        // Square base + peaked roof: a closed 5-gon, NOT regular — the specialised
        // "house" detector (rectangle base + apex) is a Track-3 follow-up.
        let r = poly(vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(10.0, 0.0),
            Point2d::new(10.0, 8.0),
            Point2d::new(5.0, 13.0),
            Point2d::new(0.0, 8.0),
        ]);
        assert_eq!(r.class, ShapeClass::Polygon { sides: 5 });
    }
}
