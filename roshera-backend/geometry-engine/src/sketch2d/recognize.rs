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

/// Classify a closed polygon from its ordered (non-repeating) corner vertices.
fn classify_polygon(verts: &[Point2d]) -> ShapeClass {
    let n = verts.len();
    if n < 3 {
        return ShapeClass::Polygon { sides: n };
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
