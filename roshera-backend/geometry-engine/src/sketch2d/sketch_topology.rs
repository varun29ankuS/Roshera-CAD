//! Sketch topology analysis and management
//!
//! This module provides tools for analyzing the topological structure of 2D sketches,
//! including loop detection, connectivity analysis, and region identification.
//!
//! # Key Features
//!
//! - Loop detection and classification (inner/outer)
//! - Connectivity graph construction
//! - Region identification and nesting
//! - Profile extraction for 3D operations
//! - Topological validation
//!
//! Indexed access into edge-list and vertex-sample arrays for sketch
//! connectivity analysis is the canonical idiom — all `arr[i]` sites use
//! indices bounded by graph node count. Matches the numerical-kernel
//! pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::constraints::EntityRef;
use super::line2d::LineGeometry;
use super::{Point2d, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d, Vector2d};
use dashmap::DashMap;
use std::sync::Arc;

/// Type of sketch entity edge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeType {
    Line,
    Arc,
    Circle,
    Spline,
    Ellipse,
    PolylineSegment(usize), // Index of segment in polyline
}

/// A directed edge in the topology graph
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyEdge {
    /// Entity reference
    pub entity: EntityRef,
    /// Edge type
    pub edge_type: EdgeType,
    /// Start point
    pub start: Point2d,
    /// End point
    pub end: Point2d,
    /// Direction (forward or reverse relative to entity)
    pub forward: bool,
    /// Parameter range on entity [t_start, t_end]
    pub param_range: Option<(f64, f64)>,
    /// Entity-extent bounding box, for edge types whose true extent is
    /// not spanned by their endpoints. A full circle's start and end
    /// are the SAME point (degenerate bounds, zero area, broken
    /// containment); an arc bulges past its chord by the sagitta.
    /// `None` for line/polyline edges, whose endpoints are exact.
    pub bounds_hint: Option<(Point2d, Point2d)>,
    /// A point strictly INTERIOR to the edge (arc midpoint, spline
    /// mid-parameter sample) for curved edge types. Winding
    /// classification (`loop_is_ccw`) threads it into the loop's
    /// vertex polygon so all-curved loops — whose chord polygons
    /// collapse (a two-arc lens has two cancelling chords) — still
    /// classify exactly. `None` for straight edges, whose endpoints
    /// already carry the geometry (SKETCH-DCM #45 follow-ups A).
    pub interior_hint: Option<Point2d>,
}

/// A vertex in the topology graph
#[derive(Debug, Clone)]
pub struct TopologyVertex {
    /// Position
    pub position: Point2d,
    /// Connected edges (edge index, outgoing direction)
    pub edges: Vec<(usize, bool)>,
}

/// A closed loop in the sketch
#[derive(Debug, Clone)]
pub struct SketchLoop {
    /// Edges forming the loop (in order)
    pub edges: Vec<usize>,
    /// Per-edge traversal direction along the walk: `true` when the
    /// loop traverses `edges[i]` start→end, `false` end→start. A walk
    /// can legitimately consume edges either way (two lines meeting at
    /// their shared START, a polyline drawn clockwise, …), so loop
    /// order alone is not enough to materialise the boundary —
    /// consumers MUST orient each edge's samples by this flag.
    pub orientations: Vec<bool>,
    /// Whether the walk is oriented GEOMETRICALLY counter-clockwise.
    /// This is true winding (SKETCH-DCM #45 follow-ups A fixed the
    /// legacy inverted trapezoid-sign convention at the root): the
    /// classification is exact-predicate-based over the walk's vertex
    /// polygon threaded with the curved edges' interior witnesses, so
    /// all-arc loops classify correctly too. Single-edge closed loops
    /// (full circle / ellipse / closed spline) have no walk direction
    /// of their own and report `true` — kernel curves are
    /// parameterised CCW.
    pub is_ccw: bool,
    /// ABSOLUTE area enclosed by the loop (chord-polygon measure with
    /// a bounds fallback for single-edge loops; winding-independent —
    /// use `is_ccw` for orientation).
    pub area: f64,
    /// Bounding box of the loop
    pub bounds: (Point2d, Point2d),
}

/// A region bounded by loops
#[derive(Debug, Clone)]
pub struct SketchRegion {
    /// Outer boundary loop
    pub outer_loop: usize,
    /// Inner holes
    pub inner_loops: Vec<usize>,
    /// Total area (outer - holes)
    pub area: f64,
    /// Nesting level (0 for outermost)
    pub depth: usize,
}

/// Classification of a sketch profile
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileType {
    /// Single closed region
    Simple,
    /// Multiple disjoint regions
    Disjoint,
    /// Nested regions (with holes)
    Nested,
    /// Open curves (cannot form region)
    Open,
    /// Mixed open and closed
    Mixed,
}

/// Sketch topology analysis result
pub struct SketchTopology {
    /// All edges in the topology
    edges: Vec<TopologyEdge>,
    /// All vertices in the topology
    vertices: Arc<DashMap<u64, TopologyVertex>>,
    /// Detected loops
    loops: Vec<SketchLoop>,
    /// Detected regions
    regions: Vec<SketchRegion>,
    /// Profile classification
    profile_type: ProfileType,
    /// Connectivity issues
    issues: Vec<TopologyIssue>,
}

/// Topology analysis issues
#[derive(Debug, Clone)]
pub enum TopologyIssue {
    /// Disconnected component
    DisconnectedComponent { entities: Vec<EntityRef> },
    /// Self-intersection detected
    SelfIntersection { entity: EntityRef, point: Point2d },
    /// T-junction (edge ends in middle of another)
    TJunction {
        edge1: EntityRef,
        edge2: EntityRef,
        point: Point2d,
    },
    /// Gap between entities
    Gap {
        entity1: EntityRef,
        entity2: EntityRef,
        distance: f64,
    },
    /// Overlapping edges
    Overlap {
        entity1: EntityRef,
        entity2: EntityRef,
    },
}

impl SketchTopology {
    /// Analyze sketch topology
    pub fn analyze(sketch: &super::Sketch, tolerance: &Tolerance2d) -> Sketch2dResult<Self> {
        let mut topology = Self {
            edges: Vec::new(),
            vertices: Arc::new(DashMap::new()),
            loops: Vec::new(),
            regions: Vec::new(),
            profile_type: ProfileType::Open,
            issues: Vec::new(),
        };

        // Build edge list from sketch entities
        topology.build_edges(sketch)?;

        // Build vertex connectivity
        topology.build_vertices(tolerance)?;

        // Find loops
        topology.find_loops()?;

        // Find regions
        topology.find_regions()?;

        // Classify profile
        topology.classify_profile();

        // Check for issues
        topology.check_issues(tolerance);

        Ok(topology)
    }

    /// Build edges from sketch entities.
    ///
    /// Construction (guide) entities are SKIPPED entirely (SKETCH-DCM
    /// #45 Slice 6): they are solver-real but profile-invisible — a
    /// construction circle must never nest as a bore, a construction
    /// guide line must never close or break a loop, and the extrude
    /// bridge (which consumes these loops) therefore never sees them.
    fn build_edges(&mut self, sketch: &super::Sketch) -> Sketch2dResult<()> {
        // Add line edges
        for entry in sketch.lines().iter() {
            let line = entry.value();
            if line.is_construction {
                continue;
            }
            match &line.geometry {
                LineGeometry::Infinite(_) => {
                    // Skip infinite lines in topology
                }
                LineGeometry::Ray(_) => {
                    // Skip rays in topology
                }
                LineGeometry::Segment(seg) => {
                    self.edges.push(TopologyEdge {
                        entity: EntityRef::Line(line.id),
                        edge_type: EdgeType::Line,
                        start: seg.start,
                        end: seg.end,
                        forward: true,
                        param_range: Some((0.0, 1.0)),
                        bounds_hint: None,
                        interior_hint: None,
                    });
                }
            }
        }

        // Add arc edges
        for entry in sketch.arcs().iter() {
            let arc = entry.value();
            if arc.is_construction {
                continue;
            }
            let start = arc.arc.start_point();
            let end = arc.arc.end_point();

            self.edges.push(TopologyEdge {
                entity: EntityRef::Arc(arc.id),
                edge_type: EdgeType::Arc,
                start,
                end,
                forward: true,
                param_range: Some((arc.arc.start_angle, arc.arc.end_angle)),
                bounds_hint: Some(arc.bounding_box()),
                interior_hint: Some(arc.arc.midpoint()),
            });
        }

        // Add circle edges (closed loops)
        for entry in sketch.circles().iter() {
            let circle = entry.value();
            if circle.is_construction {
                continue;
            }
            let start =
                circle
                    .circle
                    .point_at(0.0)
                    .map_err(|e| Sketch2dError::InvalidParameter {
                        parameter: "circle_start".to_string(),
                        value: "0.0".to_string(),
                        constraint: format!("Failed to get circle start point: {}", e),
                    })?;

            self.edges.push(TopologyEdge {
                entity: EntityRef::Circle(circle.id),
                edge_type: EdgeType::Circle,
                start,
                end: start, // Closed
                forward: true,
                param_range: Some((0.0, 2.0 * std::f64::consts::PI)),
                bounds_hint: Some(circle.bounding_box()),
                // Single-edge closed loop: winding is set by kernel
                // convention in `find_loops`, no witness needed.
                interior_hint: None,
            });
        }

        // Add rectangle edges (4 segments)
        for entry in sketch.rectangles().iter() {
            let rect = entry.value();
            if rect.is_construction {
                continue;
            }
            let corners = rect.rectangle.corners();

            for i in 0..4 {
                let next = (i + 1) % 4;
                self.edges.push(TopologyEdge {
                    entity: EntityRef::Rectangle(rect.id),
                    edge_type: EdgeType::Line,
                    start: corners[i],
                    end: corners[next],
                    forward: true,
                    param_range: Some((i as f64, next as f64)),
                    bounds_hint: None,
                    interior_hint: None,
                });
            }
        }

        // Add polyline edges
        for entry in sketch.polylines().iter() {
            let polyline = entry.value();
            if polyline.is_construction {
                continue;
            }
            let vertices = &polyline.polyline.vertices;

            let n = if polyline.polyline.is_closed {
                vertices.len()
            } else {
                vertices.len() - 1
            };

            for i in 0..n {
                let next = if i == vertices.len() - 1 { 0 } else { i + 1 };

                self.edges.push(TopologyEdge {
                    entity: EntityRef::Polyline(polyline.id),
                    edge_type: EdgeType::PolylineSegment(i),
                    start: vertices[i],
                    end: vertices[next],
                    forward: true,
                    param_range: Some((i as f64, next as f64)),
                    bounds_hint: None,
                    interior_hint: None,
                });
            }
        }

        // Add spline edges. The oriented loop walker chains edges by
        // endpoint coincidence, and the profile materialiser samples
        // `Spline2d::evaluate` over the recorded param range — so
        // splines participate in closed profiles like any other curve.
        // A closed spline (start ≈ end) forms a single-edge loop
        // exactly like a circle. The bounds hint covers the
        // control-polygon box (the curve lies inside its control
        // hull), keeping loop bounds and containment honest for curves
        // that bulge past their chord.
        for entry in sketch.splines().iter() {
            let spline = entry.value();
            if spline.is_construction {
                continue;
            }
            let (start, end) = match (spline.spline.evaluate(0.0), spline.spline.evaluate(1.0)) {
                (Ok(s), Ok(e)) => (s, e),
                _ => continue, // unevaluable spline cannot bound a profile
            };
            self.edges.push(TopologyEdge {
                entity: EntityRef::Spline(spline.id),
                edge_type: EdgeType::Spline,
                start,
                end,
                forward: true,
                param_range: Some((0.0, 1.0)),
                bounds_hint: Some(spline.bounding_box()),
                interior_hint: spline.spline.evaluate(0.5).ok(),
            });
        }

        // Add ellipse edges (closed single-edge loops, like circles).
        // Param range is the angle convention of `Ellipse2d::evaluate`
        // (0..2pi); the bounds hint carries the rotated extents.
        for entry in sketch.ellipses().iter() {
            let ellipse = entry.value();
            if ellipse.is_construction {
                continue;
            }
            let start = ellipse.ellipse.evaluate(0.0);
            self.edges.push(TopologyEdge {
                entity: EntityRef::Ellipse(ellipse.id),
                edge_type: EdgeType::Ellipse,
                start,
                end: start, // closed
                forward: true,
                param_range: Some((0.0, 2.0 * std::f64::consts::PI)),
                bounds_hint: Some(ellipse.bounding_box()),
                // Single-edge closed loop: CCW by kernel convention.
                interior_hint: None,
            });
        }

        Ok(())
    }

    /// Build vertex connectivity graph
    fn build_vertices(&mut self, tolerance: &Tolerance2d) -> Sketch2dResult<()> {
        // Hash function for vertex position
        let hash_point = |p: &Point2d| -> u64 {
            let x = (p.x / tolerance.distance).round() as i32;
            let y = (p.y / tolerance.distance).round() as i32;
            ((x as u64) << 32) | (y as u32 as u64)
        };

        // Add vertices and build connectivity
        for (edge_idx, edge) in self.edges.iter().enumerate() {
            // Start vertex
            let start_hash = hash_point(&edge.start);
            self.vertices
                .entry(start_hash)
                .or_insert_with(|| TopologyVertex {
                    position: edge.start,
                    edges: Vec::new(),
                })
                .edges
                .push((edge_idx, true)); // Outgoing

            // End vertex (if different from start)
            if edge.start.distance_to(&edge.end) > tolerance.distance {
                let end_hash = hash_point(&edge.end);
                self.vertices
                    .entry(end_hash)
                    .or_insert_with(|| TopologyVertex {
                        position: edge.end,
                        edges: Vec::new(),
                    })
                    .edges
                    .push((edge_idx, false)); // Incoming
            }
        }

        Ok(())
    }

    /// Find all closed loops by oriented boundary walking.
    ///
    /// History: the original implementation alternated between an
    /// edge's `end` and `start` by PATH-LENGTH PARITY rather than by
    /// the direction the walk actually traversed each edge. On the
    /// most ordinary input imaginable — a rectangle of four
    /// head-to-tail edges — the cursor reversed after the first hop,
    /// dead-ended, and found nothing; a lone circle (start == end,
    /// degree-2 self-vertex) couldn't seed a loop either. The module
    /// compiled, looked finished, and had never extracted a loop from
    /// a normal profile — discovered 2026-06-12 when the csketch →
    /// solid bridge became its first production caller.
    fn find_loops(&mut self) -> Sketch2dResult<()> {
        let mut visited_edges = vec![false; self.edges.len()];
        let mut loops = Vec::new();

        for start_edge in 0..self.edges.len() {
            if visited_edges[start_edge] {
                continue;
            }

            // Single-edge closed loop: a full circle (or closed spline
            // edge) closes onto itself and needs no walk. It has no
            // walk direction of its own; the kernel parameterises
            // closed curves CCW, so the loop reports that convention
            // (pinned by `lone_circle_loop_is_ccw_by_kernel_convention`).
            let e = &self.edges[start_edge];
            if e.start.distance_to(&e.end) < 1e-6 {
                visited_edges[start_edge] = true;
                let edges = vec![start_edge];
                let orientations = vec![true];
                let area = self.calculate_loop_area(&edges, &orientations);
                let bounds = self.calculate_loop_bounds(&edges);
                loops.push(SketchLoop {
                    edges,
                    orientations,
                    is_ccw: true,
                    area: area.abs(),
                    bounds,
                });
                continue;
            }

            if let Some((loop_edges, orientations)) =
                self.find_loop_from_edge(start_edge, &mut visited_edges)
            {
                let area = self.calculate_loop_area(&loop_edges, &orientations);
                let bounds = self.calculate_loop_bounds(&loop_edges);
                let is_ccw = self.loop_is_ccw(&loop_edges, &orientations, area);
                loops.push(SketchLoop {
                    edges: loop_edges,
                    orientations,
                    is_ccw,
                    area: area.abs(),
                    bounds,
                });
            }
        }

        self.loops = loops;
        Ok(())
    }

    /// Walk from `start_edge` (traversed start→end) following oriented
    /// endpoint connectivity until the cursor returns to the walk's
    /// origin. Returns the edge sequence and the per-edge traversal
    /// direction, or `None` (with `visited` rolled back) on a dead end.
    fn find_loop_from_edge(
        &self,
        start_edge: usize,
        visited: &mut [bool],
    ) -> Option<(Vec<usize>, Vec<bool>)> {
        const TOL: f64 = 1e-6;
        let origin = self.edges[start_edge].start;
        let mut cursor = self.edges[start_edge].end;
        let mut path = vec![start_edge];
        let mut orientations = vec![true];
        visited[start_edge] = true;

        loop {
            if cursor.distance_to(&origin) < TOL {
                return Some((path, orientations));
            }

            // Next unvisited edge incident to the cursor, traversed
            // away from it. Direction is decided by WHICH endpoint
            // touches the cursor — not by parity, not by the edge's
            // stored entity-relative `forward` flag.
            let mut next: Option<(usize, bool)> = None;
            for (edge_idx, edge) in self.edges.iter().enumerate() {
                if visited[edge_idx] {
                    continue;
                }
                if edge.start.distance_to(&cursor) < TOL {
                    next = Some((edge_idx, true));
                    break;
                }
                if edge.end.distance_to(&cursor) < TOL {
                    next = Some((edge_idx, false));
                    break;
                }
            }

            match next {
                Some((edge_idx, forward)) => {
                    visited[edge_idx] = true;
                    cursor = if forward {
                        self.edges[edge_idx].end
                    } else {
                        self.edges[edge_idx].start
                    };
                    path.push(edge_idx);
                    orientations.push(forward);
                }
                None => {
                    for &edge in &path {
                        visited[edge] = false;
                    }
                    return None;
                }
            }

            if path.len() > self.edges.len() {
                for &edge in &path {
                    visited[edge] = false;
                }
                return None;
            }
        }
    }

    /// Calculate signed area of a loop from its ORIENTED traversal.
    ///
    /// Every edge contributes its chord to the shoelace sum in the
    /// direction the walk traversed it (`orientations[i]`); summing the
    /// stored start→end direction regardless of traversal produced a
    /// meaningless sign for any loop containing a reversed edge. Arc /
    /// spline chords incur a bounded sagitta error which is acceptable
    /// for the CCW classification this area drives; it is NOT a
    /// substitute for analytic area. A single-edge closed loop (full
    /// circle) has a zero chord sum — fall back to its bounding box to
    /// get a usable positive magnitude (sign irrelevant: region
    /// nesting is decided by containment, not winding).
    fn calculate_loop_area(&self, loop_edges: &[usize], orientations: &[bool]) -> f64 {
        let mut area = 0.0;
        for (k, &edge_idx) in loop_edges.iter().enumerate() {
            let edge = &self.edges[edge_idx];
            let forward = orientations.get(k).copied().unwrap_or(true);
            let (a, b) = if forward {
                (edge.start, edge.end)
            } else {
                (edge.end, edge.start)
            };
            area += (b.x - a.x) * (b.y + a.y) / 2.0;
        }
        if area.abs() < 1e-12 && loop_edges.len() == 1 {
            // Full circle: π·rx·ry from the bounds (= πr² for a circle).
            let (min, max) = self.calculate_loop_bounds(loop_edges);
            let rx = (max.x - min.x) / 2.0;
            let ry = (max.y - min.y) / 2.0;
            return std::f64::consts::PI * rx * ry;
        }
        area
    }

    /// Exact GEOMETRIC loop winding (`is_ccw`), tolerance-free.
    ///
    /// The polygon fed to the exact predicate is the walk's vertex
    /// sequence with each curved edge's `interior_hint` (arc midpoint /
    /// spline mid-parameter sample) threaded in between its endpoints —
    /// without the witnesses, an all-curved loop like a two-arc lens
    /// has a degenerate chord polygon (the chords cancel) and CANNOT be
    /// classified from vertices alone.
    ///
    /// History (SKETCH-DCM #45 follow-ups A): this method used to
    /// preserve a legacy trapezoid-sign convention — `calculate_loop_area`
    /// computes `Σ(b.x−a.x)(b.y+a.y)/2 = −shoelace`, whose POSITIVE sign
    /// is CLOCKWISE, and the old mapping (`Clockwise => true`) kept that
    /// inverted `area > 0.0` decision alive in a field NAMED `is_ccw`.
    /// The convention is now geometric truth; the trapezoid fallback for
    /// exactly-degenerate polygons is sign-corrected to match
    /// (`area < 0.0` == CCW under the trapezoid form).
    fn loop_is_ccw(&self, loop_edges: &[usize], orientations: &[bool], area: f64) -> bool {
        use crate::math::vector2::Vector2;
        use crate::math::{signed_area_2d, Orientation};
        let mut poly: Vec<Vector2> = Vec::with_capacity(loop_edges.len() * 2);
        for (k, &edge_idx) in loop_edges.iter().enumerate() {
            let edge = &self.edges[edge_idx];
            let forward = orientations.get(k).copied().unwrap_or(true);
            let p = if forward { edge.start } else { edge.end };
            poly.push(Vector2::new(p.x, p.y));
            // The witness lies ON the edge's point set, so it sits
            // between the edge's endpoints in either traversal
            // direction.
            if let Some(w) = edge.interior_hint {
                poly.push(Vector2::new(w.x, w.y));
            }
        }
        match signed_area_2d(&poly) {
            Orientation::CounterClockwise => true,
            Orientation::Clockwise => false,
            Orientation::Collinear => area < 0.0,
        }
    }

    /// Calculate bounding box of a loop
    fn calculate_loop_bounds(&self, loop_edges: &[usize]) -> (Point2d, Point2d) {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for &edge_idx in loop_edges {
            let edge = &self.edges[edge_idx];

            min_x = min_x.min(edge.start.x).min(edge.end.x);
            min_y = min_y.min(edge.start.y).min(edge.end.y);
            max_x = max_x.max(edge.start.x).max(edge.end.x);
            max_y = max_y.max(edge.start.y).max(edge.end.y);
            if let Some((hmin, hmax)) = edge.bounds_hint {
                min_x = min_x.min(hmin.x);
                min_y = min_y.min(hmin.y);
                max_x = max_x.max(hmax.x);
                max_y = max_y.max(hmax.y);
            }
        }

        (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
    }

    /// Find regions from loops
    fn find_regions(&mut self) -> Sketch2dResult<()> {
        if self.loops.is_empty() {
            return Ok(());
        }

        // Sort loops by area (largest first)
        let mut loop_indices: Vec<usize> = (0..self.loops.len()).collect();
        // NaN-safe: if either loop area is NaN treat them as equal so the
        // sort remains total and deterministic.
        loop_indices.sort_by(|&a, &b| {
            self.loops[b]
                .area
                .partial_cmp(&self.loops[a].area)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Containment-depth hierarchy. Winding (`is_ccw`) is NOT a
        // valid outer-vs-hole signal here: walk direction depends on
        // which edge seeded the walk, and a circle's single edge is
        // always parameterised CCW — so the previous winding-gated
        // builder silently dropped circle holes (and any outer loop a
        // walk happened to traverse clockwise). Mainstream-CAD model
        // instead: a loop contained by an EVEN number of other loops
        // is an outer boundary; its holes are the loops directly
        // inside it (depth exactly one greater). Mirrors the
        // api-server click-draft `detect_regions`, which is the
        // production-proven implementation of the same rule.
        let n = self.loops.len();
        let mut depth = vec![0usize; n];
        for i in 0..n {
            for j in 0..n {
                if i != j && self.loop_contains_loop(j, i) {
                    depth[i] += 1;
                }
            }
        }

        let mut regions = Vec::new();
        for &i in &loop_indices {
            if depth[i] % 2 != 0 {
                continue; // odd depth = hole; attached to its parent below
            }
            let mut region = SketchRegion {
                outer_loop: i,
                inner_loops: Vec::new(),
                area: self.loops[i].area,
                depth: depth[i],
            };
            for &j in &loop_indices {
                if i != j && depth[j] == depth[i] + 1 && self.loop_contains_loop(i, j) {
                    region.inner_loops.push(j);
                    region.area -= self.loops[j].area;
                }
            }
            regions.push(region);
        }

        self.regions = regions;
        Ok(())
    }

    /// Check if loop i contains loop j.
    ///
    /// Uses an axis-aligned bounds prune followed by an even-odd ray-cast
    /// point-in-polygon test against loop_i's edge polyline. Since the
    /// detected loops are non-self-intersecting and pairwise disjoint at
    /// this stage, testing a single sample point on loop_j (the start of
    /// its first edge) is sufficient to decide containment.
    ///
    /// The ray is cast in the +X direction; horizontal edges (y_a == y_b)
    /// are skipped because they cannot produce a transverse crossing,
    /// and the half-open interval `[y_min, y_max)` convention prevents
    /// double-counting at vertex grazes.
    fn loop_contains_loop(&self, i: usize, j: usize) -> bool {
        let loop_i = &self.loops[i];
        let loop_j = &self.loops[j];

        // Axis-aligned bounds prune.
        let (min_i, max_i) = loop_i.bounds;
        let (min_j, max_j) = loop_j.bounds;

        if min_j.x < min_i.x || max_j.x > max_i.x || min_j.y < min_i.y || max_j.y > max_i.y {
            return false;
        }

        // Sample point: start vertex of loop_j's first edge.
        let Some(&first_edge_idx) = loop_j.edges.first() else {
            return false;
        };
        let Some(first_edge) = self.edges.get(first_edge_idx) else {
            return false;
        };
        let test = first_edge.start;

        // A single-edge closed loop (full circle / ellipse) has a
        // degenerate chord polyline — the even-odd ray cast below
        // would see zero crossings and report "contains nothing".
        // Decide analytically from its bounds instead: an ellipse
        // inscribed in the box, (dx/rx)^2 + (dy/ry)^2 < 1. Exact for
        // circles and axis-aligned ellipses; for ROTATED ellipses the
        // AABB-inscribed ellipse over-approximates the true curve, so
        // containment of points near the rotated corners can
        // misclassify — acceptable until topology edges carry sampled
        // boundary polylines.
        if loop_i.edges.len() == 1 {
            let (min_i, max_i) = loop_i.bounds;
            let cx = (min_i.x + max_i.x) / 2.0;
            let cy = (min_i.y + max_i.y) / 2.0;
            let rx = (max_i.x - min_i.x) / 2.0;
            let ry = (max_i.y - min_i.y) / 2.0;
            if rx <= 0.0 || ry <= 0.0 {
                return false;
            }
            let dx = (test.x - cx) / rx;
            let dy = (test.y - cy) / ry;
            return dx * dx + dy * dy < 1.0;
        }

        // Even-odd ray cast in +X against loop_i's edge polyline.
        let mut crossings = 0usize;
        for &edge_idx in &loop_i.edges {
            let Some(edge) = self.edges.get(edge_idx) else {
                continue;
            };
            let (a, b) = (edge.start, edge.end);
            let (y_min, y_max, x_at_min, x_at_max) = if a.y <= b.y {
                (a.y, b.y, a.x, b.x)
            } else {
                (b.y, a.y, b.x, a.x)
            };
            // Skip horizontal segments and edges entirely above or below test.y.
            if y_min == y_max || test.y < y_min || test.y >= y_max {
                continue;
            }
            // Linear-interpolate x at the ray's y level.
            let t = (test.y - y_min) / (y_max - y_min);
            let x_cross = x_at_min + t * (x_at_max - x_at_min);
            if x_cross > test.x {
                crossings += 1;
            }
        }

        crossings % 2 == 1
    }

    /// Classify the profile type
    fn classify_profile(&mut self) {
        let closed_count = self.loops.len();
        let open_count = self.edges.len() - self.loops.iter().map(|l| l.edges.len()).sum::<usize>();

        self.profile_type = if closed_count == 0 && open_count == 0 {
            // Empty sketch: no edges, no loops. Treat as trivially valid
            // (no profile to classify); callers that need a region can
            // short-circuit on an empty sketch.
            ProfileType::Simple
        } else if closed_count == 0 && open_count > 0 {
            ProfileType::Open
        } else if closed_count == 1 && open_count == 0 {
            ProfileType::Simple
        } else if closed_count > 1 && open_count == 0 {
            if self.regions.iter().any(|r| !r.inner_loops.is_empty()) {
                ProfileType::Nested
            } else {
                ProfileType::Disjoint
            }
        } else {
            ProfileType::Mixed
        };
    }

    /// Check for topology issues
    fn check_issues(&mut self, tolerance: &Tolerance2d) {
        // Check for gaps
        for i in 0..self.edges.len() {
            for j in i + 1..self.edges.len() {
                let edge_i = &self.edges[i];
                let edge_j = &self.edges[j];

                // Check end-to-end distances
                let distances = [
                    edge_i.start.distance_to(&edge_j.start),
                    edge_i.start.distance_to(&edge_j.end),
                    edge_i.end.distance_to(&edge_j.start),
                    edge_i.end.distance_to(&edge_j.end),
                ];

                for &dist in &distances {
                    if dist > tolerance.distance && dist < 10.0 * tolerance.distance {
                        self.issues.push(TopologyIssue::Gap {
                            entity1: edge_i.entity,
                            entity2: edge_j.entity,
                            distance: dist,
                        });
                    }
                }
            }
        }

        // Check for T-junctions
        // This would require checking if edge endpoints lie on other edges

        // Check for self-intersections
        // This would require intersection testing between edges
    }

    /// Get all loops
    pub fn loops(&self) -> &[SketchLoop] {
        &self.loops
    }

    /// All directed edges in the topology graph. `SketchLoop::edges`
    /// holds indices into this slice; consumers that materialise loop
    /// geometry (e.g. the api-server's csketch→solid bridge) walk the
    /// loop and resolve each index here to reach the entity reference,
    /// orientation, and parameter range.
    pub fn edges(&self) -> &[TopologyEdge] {
        &self.edges
    }

    /// Get all regions
    pub fn regions(&self) -> &[SketchRegion] {
        &self.regions
    }

    /// Get profile type
    pub fn profile_type(&self) -> ProfileType {
        self.profile_type
    }

    /// Get topology issues
    pub fn issues(&self) -> &[TopologyIssue] {
        &self.issues
    }

    /// Return the positions of every dangling vertex (degree-1 node).
    ///
    /// In an open profile each connected component terminates at a
    /// vertex incident to exactly one edge. These are the points the
    /// validator surfaces to the user as "fix the gap here". Vertices
    /// with degree ≥ 2 either lie on a closed loop or at an interior
    /// junction and are excluded.
    pub fn open_endpoints(&self) -> Vec<Point2d> {
        self.vertices
            .iter()
            .filter_map(|entry| {
                let v = entry.value();
                if v.edges.len() == 1 {
                    Some(v.position)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if topology forms valid profiles for extrusion
    pub fn is_valid_for_extrusion(&self) -> bool {
        matches!(
            self.profile_type,
            ProfileType::Simple | ProfileType::Nested | ProfileType::Disjoint
        ) && self.issues.is_empty()
    }

    /// Get outer profiles (for extrusion/revolution)
    pub fn get_outer_profiles(&self) -> Vec<&SketchLoop> {
        self.regions
            .iter()
            .map(|r| &self.loops[r.outer_loop])
            .collect()
    }
}

/// Profile extraction for 3D operations
pub struct ProfileExtractor;

impl ProfileExtractor {
    /// Extract profiles suitable for extrusion
    pub fn extract_for_extrusion(
        topology: &SketchTopology,
    ) -> Sketch2dResult<Vec<ExtrusionProfile>> {
        if !topology.is_valid_for_extrusion() {
            return Err(Sketch2dError::InvalidTopology {
                reason: "Sketch topology is not valid for extrusion".to_string(),
            });
        }

        let mut profiles = Vec::new();

        for region in &topology.regions {
            let outer_loop = &topology.loops[region.outer_loop];
            let inner_loops: Vec<_> = region
                .inner_loops
                .iter()
                .map(|&i| &topology.loops[i])
                .collect();

            profiles.push(ExtrusionProfile {
                outer_boundary: outer_loop.clone(),
                holes: inner_loops.into_iter().cloned().collect(),
                area: region.area,
            });
        }

        Ok(profiles)
    }

    /// Extract profiles suitable for revolution
    pub fn extract_for_revolution(
        topology: &SketchTopology,
        axis_origin: &Point2d,
        axis_direction: &Vector2d,
    ) -> Sketch2dResult<Vec<RevolutionProfile>> {
        let profiles = Self::extract_for_extrusion(topology)?;

        // Check that profiles don't cross the axis
        for profile in &profiles {
            for edge_idx in &profile.outer_boundary.edges {
                let edge = &topology.edges[*edge_idx];

                // Check if edge crosses axis
                if Self::segment_crosses_line(&edge.start, &edge.end, axis_origin, axis_direction) {
                    return Err(Sketch2dError::InvalidTopology {
                        reason: "Profile crosses revolution axis".to_string(),
                    });
                }
            }
        }

        // Convert to revolution profiles
        Ok(profiles
            .into_iter()
            .map(|p| RevolutionProfile {
                profile: p,
                axis_origin: *axis_origin,
                axis_direction: *axis_direction,
            })
            .collect())
    }

    /// Check if line segment crosses a line
    fn segment_crosses_line(
        p1: &Point2d,
        p2: &Point2d,
        line_origin: &Point2d,
        line_dir: &Vector2d,
    ) -> bool {
        // Calculate which side of the line each point is on
        let v1 = Vector2d::new(p1.x - line_origin.x, p1.y - line_origin.y);
        let v2 = Vector2d::new(p2.x - line_origin.x, p2.y - line_origin.y);

        let cross1 = line_dir.cross(&v1);
        let cross2 = line_dir.cross(&v2);

        // If signs differ, segment crosses line
        cross1 * cross2 < 0.0
    }
}

/// One typed edge of an analytic profile loop, in plane-local (u, v)
/// coordinates, oriented in LOOP-WALK order (traversal is baked in:
/// a reversed walk swaps a line's endpoints and flips an arc's angles
/// and winding, so consumers never re-consult the walk flags).
///
/// This is the SKETCH-DCM #45 Slice 5 (spec §3.3 "Phase D") boundary
/// type: it keeps the entity-level geometry the topology walker used
/// to discard at the chord-sampling boundary, so downstream face
/// construction can carry TRUE circular edges instead of a 64-gon.
/// The serde shape (`{"kind": "line" | "arc" | "circle", …}`) is the
/// wire format the `sketch_extrude` timeline event records — replay
/// rebuilds the identical analytic solid from it.
///
/// Angles follow the sketch convention: radians from the plane's +u
/// axis toward +v. `ccw: true` sweeps `start_angle → end_angle`
/// counter-clockwise (about +normal = u × v); `false` sweeps
/// clockwise. A full circle is its own single-edge closed loop.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileEdge {
    Line {
        start: [f64; 2],
        end: [f64; 2],
    },
    Arc {
        center: [f64; 2],
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        ccw: bool,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    /// Exact NURBS/B-spline boundary edge (SKETCH-DCM #45 Slice 7 —
    /// spline profiles stop chord-sampling). Loop-walk orientation is
    /// baked in like every other variant: a reversed walk reverses the
    /// control points (and weights) and mirrors the knot vector
    /// (`k'ᵢ = k_first + k_last − k_{n−1−i}` — the standard reversal,
    /// Piegl & Tiller §5.2). `weights: None` = non-rational B-spline.
    Nurbs {
        degree: usize,
        control_points: Vec<[f64; 2]>,
        weights: Option<Vec<f64>>,
        knots: Vec<f64>,
    },
}

/// Verdict of [`ProfileExtractor::analytic_loop_edges`]: either the
/// loop's full typed-edge list, or an HONEST refusal naming the first
/// entity whose exact geometry is not analytically lifted yet
/// (splines and ellipses this slice). Callers use the refusal as the
/// fallback signal to keep chord-sampling that loop — a sampled
/// polygon is never silently labeled analytic.
#[derive(Debug, Clone, PartialEq)]
pub enum AnalyticLoop {
    /// Every edge of the loop expressed exactly.
    Edges(Vec<ProfileEdge>),
    /// The loop contains an entity kind with no analytic lift.
    Unsupported {
        entity: EntityRef,
        edge_type: EdgeType,
    },
}

impl ProfileExtractor {
    /// Express a topology loop as typed analytic edges (SKETCH-DCM #45
    /// Slice 5, spec §3.3).
    ///
    /// Mirrors the walk semantics of the api-server's polygon
    /// materialiser (`sample_topology_loop`): edges are emitted in loop
    /// order with the per-edge traversal direction applied, so
    /// concatenating them yields the closed boundary. Geometry comes
    /// from the SKETCH ENTITIES (exact stored centers/radii/angles),
    /// not from re-fitting samples.
    ///
    /// Coverage audit (every entity kind the walker can put in a loop):
    /// line segments, rectangle sides and polyline segments → `Line`;
    /// arcs → `Arc`; circles → `Circle`; splines → `Nurbs` (exact
    /// stored control points / weights / knots — SKETCH-DCM #45
    /// Slice 7); ellipses have no exact analytic lift wired into face
    /// construction yet and return [`AnalyticLoop::Unsupported`]
    /// (never a silent approximation). Hard errors are reserved for
    /// structural corruption (a loop referencing a missing edge or
    /// entity).
    pub fn analytic_loop_edges(
        sketch: &super::Sketch,
        topology: &SketchTopology,
        sketch_loop: &SketchLoop,
    ) -> Sketch2dResult<AnalyticLoop> {
        let edges = topology.edges();
        let mut out = Vec::with_capacity(sketch_loop.edges.len());
        for (k, &edge_idx) in sketch_loop.edges.iter().enumerate() {
            // Walk orientation from the topology loop (NOT the edge's
            // entity-relative `forward` flag): the loop walker may
            // traverse any edge end→start, and emitting it un-reversed
            // would fold the boundary back on itself.
            let walk_forward = sketch_loop.orientations.get(k).copied().unwrap_or(true);
            let edge = edges
                .get(edge_idx)
                .ok_or_else(|| Sketch2dError::InvalidTopology {
                    reason: format!("loop references missing topology edge {edge_idx}"),
                })?;
            match (&edge.edge_type, &edge.entity) {
                (EdgeType::Line, _) | (EdgeType::PolylineSegment(_), _) => {
                    let (s, e) = if walk_forward {
                        (edge.start, edge.end)
                    } else {
                        (edge.end, edge.start)
                    };
                    out.push(ProfileEdge::Line {
                        start: [s.x, s.y],
                        end: [e.x, e.y],
                    });
                }
                (EdgeType::Arc, EntityRef::Arc(arc_id)) => {
                    let entry =
                        sketch
                            .arcs()
                            .get(arc_id)
                            .ok_or_else(|| Sketch2dError::EntityNotFound {
                                entity_type: "Arc".to_string(),
                                id: arc_id.to_string(),
                            })?;
                    let arc = entry.value().arc;
                    // Reversed traversal swaps the angles AND flips the
                    // winding — the geometric point set is identical,
                    // only the walk direction changes.
                    let (start_angle, end_angle, ccw) = if walk_forward {
                        (arc.start_angle, arc.end_angle, arc.ccw)
                    } else {
                        (arc.end_angle, arc.start_angle, !arc.ccw)
                    };
                    out.push(ProfileEdge::Arc {
                        center: [arc.center.x, arc.center.y],
                        radius: arc.radius,
                        start_angle,
                        end_angle,
                        ccw,
                    });
                }
                (EdgeType::Circle, EntityRef::Circle(circle_id)) => {
                    let entry = sketch.circles().get(circle_id).ok_or_else(|| {
                        Sketch2dError::EntityNotFound {
                            entity_type: "Circle".to_string(),
                            id: circle_id.to_string(),
                        }
                    })?;
                    let circle = entry.value().circle;
                    // A circle is a closed point set — traversal
                    // direction does not change it. The kernel curve is
                    // parameterised CCW about +normal, matching the
                    // proven click-draft analytic-circle path.
                    out.push(ProfileEdge::Circle {
                        center: [circle.center.x, circle.center.y],
                        radius: circle.radius,
                    });
                }
                (EdgeType::Spline, EntityRef::Spline(spline_id)) => {
                    // Exact NURBS lift (SKETCH-DCM #45 Slice 7): the
                    // stored control points / weights / knots ARE the
                    // edge — no chord fit. Reversed traversal reverses
                    // control points (and weights) and mirrors the
                    // knot vector, which parameterises the identical
                    // point set in the opposite direction.
                    let entry = sketch.splines().get(spline_id).ok_or_else(|| {
                        Sketch2dError::EntityNotFound {
                            entity_type: "Spline".to_string(),
                            id: spline_id.to_string(),
                        }
                    })?;
                    let (degree, mut cps, mut weights, mut knots) = match &entry.value().spline {
                        crate::sketch2d::Spline2d::BSpline(bs) => (
                            bs.degree,
                            bs.control_points
                                .iter()
                                .map(|p| [p.x, p.y])
                                .collect::<Vec<_>>(),
                            None,
                            bs.knots.clone(),
                        ),
                        crate::sketch2d::Spline2d::Nurbs(nurbs) => (
                            nurbs.degree,
                            nurbs
                                .control_points
                                .iter()
                                .map(|p| [p.x, p.y])
                                .collect::<Vec<_>>(),
                            Some(nurbs.weights.clone()),
                            nurbs.knots.clone(),
                        ),
                    };
                    if !walk_forward {
                        cps.reverse();
                        if let Some(w) = weights.as_mut() {
                            w.reverse();
                        }
                        let (first, last) = match (knots.first(), knots.last()) {
                            (Some(f), Some(l)) => (*f, *l),
                            _ => {
                                return Err(Sketch2dError::InvalidTopology {
                                    reason: format!("spline {spline_id} has an empty knot vector"),
                                })
                            }
                        };
                        let mirrored: Vec<f64> =
                            knots.iter().rev().map(|k| first + last - k).collect();
                        knots = mirrored;
                    }
                    out.push(ProfileEdge::Nurbs {
                        degree,
                        control_points: cps,
                        weights,
                        knots,
                    });
                }
                (edge_type, entity) => {
                    return Ok(AnalyticLoop::Unsupported {
                        entity: *entity,
                        edge_type: *edge_type,
                    });
                }
            }
        }
        Ok(AnalyticLoop::Edges(out))
    }
}

/// Profile ready for extrusion
#[derive(Debug, Clone)]
pub struct ExtrusionProfile {
    /// Outer boundary loop
    pub outer_boundary: SketchLoop,
    /// Interior holes
    pub holes: Vec<SketchLoop>,
    /// Total area
    pub area: f64,
}

/// Profile ready for revolution
#[derive(Debug, Clone)]
pub struct RevolutionProfile {
    /// The profile to revolve
    pub profile: ExtrusionProfile,
    /// Revolution axis origin
    pub axis_origin: Point2d,
    /// Revolution axis direction
    pub axis_direction: Vector2d,
}

#[cfg(test)]
mod tests {
    //! First-ever tests for the topology walker. The module shipped
    //! untested and its loop walk was broken on every normal profile
    //! (parity-based cursor flipping — see `find_loops` docs); these
    //! pin the rewritten oriented walk + containment-depth regions.
    use super::*;
    use crate::sketch2d::{Sketch, SketchAnchor, Tolerance2d};

    fn analyze(sketch: &Sketch) -> SketchTopology {
        SketchTopology::analyze(sketch, &Tolerance2d::default())
            .expect("topology analysis must succeed on valid sketches")
    }

    fn sketch() -> Sketch {
        Sketch::new("t".to_string(), SketchAnchor::xy())
    }

    #[test]
    fn closed_polyline_forms_one_loop_and_region() {
        let s = sketch();
        s.add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(80.0, 0.0),
                Point2d::new(80.0, 50.0),
                Point2d::new(0.0, 50.0),
            ],
            true,
        )
        .expect("closed polyline must construct");
        let topo = analyze(&s);
        assert_eq!(topo.loops().len(), 1, "one closed loop expected");
        assert_eq!(topo.regions().len(), 1, "one region expected");
        assert!(
            (topo.loops()[0].area - 4000.0).abs() < 1e-9,
            "80x50 loop area, got {}",
            topo.loops()[0].area
        );
        assert_eq!(topo.profile_type(), ProfileType::Simple);
        assert!(topo.is_valid_for_extrusion());
    }

    #[test]
    fn polyline_with_circle_hole_is_one_region_one_hole() {
        let s = sketch();
        s.add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(80.0, 0.0),
                Point2d::new(80.0, 50.0),
                Point2d::new(0.0, 50.0),
            ],
            true,
        )
        .expect("closed polyline must construct");
        s.add_circle(Point2d::new(25.0, 25.0), 6.0)
            .expect("circle must construct");
        let topo = analyze(&s);
        assert_eq!(topo.loops().len(), 2, "outer + circle loops");
        assert_eq!(topo.regions().len(), 1, "circle nests as a hole");
        assert_eq!(topo.regions()[0].inner_loops.len(), 1);
        assert!(topo.is_valid_for_extrusion());
    }

    /// Regression #41: a sparse 4-corner keyway notch used as a HOLE inside a
    /// round disc must nest as an inner loop and its area must be SUBTRACTED
    /// from the region — the eval saw the region collapse to the bare outer
    /// area (π·10²) as if the notch were dropped. Densifying the notch walls
    /// must not change the answer. Both configurations are asserted here so the
    /// containment/nesting path stays honest for sparse re-entrant holes.
    #[test]
    fn sparse_keyway_notch_nests_as_a_hole_and_subtracts_its_area() {
        // keyway rectangle: x∈[-1.5,1.5], y∈[4.77,6.4] ⟹ area = 3·1.63 = 4.89.
        let keyway_area = 3.0 * (6.4 - 4.77);
        let expect = std::f64::consts::PI * 100.0 - keyway_area;
        for (label, extra) in [("sparse", 0usize), ("dense", 4usize)] {
            let s = sketch();
            s.add_circle(Point2d::new(0.0, 0.0), 10.0).expect("gear");
            let base = [
                Point2d::new(1.5, 4.77),
                Point2d::new(1.5, 6.4),
                Point2d::new(-1.5, 6.4),
                Point2d::new(-1.5, 4.77),
            ];
            let mut pts = Vec::new();
            for i in 0..4 {
                let a = base[i];
                let b = base[(i + 1) % 4];
                for j in 0..=extra {
                    let t = j as f64 / (extra as f64 + 1.0);
                    pts.push(Point2d::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
                }
            }
            s.add_polyline(pts, true).expect("keyway polyline");
            let topo = analyze(&s);
            assert_eq!(topo.regions().len(), 1, "{label}: one region (disc)");
            let region = &topo.regions()[0];
            assert_eq!(
                region.inner_loops.len(),
                1,
                "{label}: keyway must nest as a hole, not become a second region"
            );
            // Circle area uses the bounds fallback (exact π r²), so the region
            // area is exactly π·100 − keyway; tolerate only f64 noise.
            assert!(
                (region.area - expect).abs() < 1e-6,
                "{label}: region area {} != {expect} (keyway hole not subtracted)",
                region.area
            );
        }
    }

    #[test]
    fn four_head_to_tail_lines_form_a_loop() {
        let s = sketch();
        let p = [
            Point2d::new(0.0, 0.0),
            Point2d::new(10.0, 0.0),
            Point2d::new(10.0, 10.0),
            Point2d::new(0.0, 10.0),
        ];
        let ids: Vec<_> = p.iter().map(|q| s.add_point(*q)).collect();
        for i in 0..4 {
            s.add_line(ids[i], ids[(i + 1) % 4])
                .expect("line between distinct points must construct");
        }
        let topo = analyze(&s);
        assert_eq!(topo.loops().len(), 1);
        assert!(topo.is_valid_for_extrusion());
    }

    #[test]
    fn lone_circle_is_a_closed_simple_profile() {
        let s = sketch();
        s.add_circle(Point2d::new(0.0, 0.0), 5.0)
            .expect("circle must construct");
        let topo = analyze(&s);
        assert_eq!(topo.loops().len(), 1, "circle is a single-edge loop");
        assert_eq!(topo.regions().len(), 1);
        assert!(topo.is_valid_for_extrusion());
        let area = topo.loops()[0].area;
        assert!(
            (area - std::f64::consts::PI * 25.0).abs() < 1e-6,
            "pi r^2 from bounds fallback, got {area}"
        );
    }

    #[test]
    fn open_chain_is_not_extrudable_and_names_its_endpoints() {
        let s = sketch();
        let a = s.add_point(Point2d::new(0.0, 0.0));
        let b = s.add_point(Point2d::new(10.0, 0.0));
        let c = s.add_point(Point2d::new(10.0, 10.0));
        s.add_line(a, b).expect("line ab");
        s.add_line(b, c).expect("line bc");
        let topo = analyze(&s);
        assert_eq!(topo.loops().len(), 0);
        assert_eq!(topo.profile_type(), ProfileType::Open);
        assert!(!topo.is_valid_for_extrusion());
        assert_eq!(topo.open_endpoints().len(), 2, "two dangling endpoints");
    }

    #[test]
    fn reversed_edge_direction_still_walks_the_loop() {
        // Two of the four lines are added "backwards" (end->start
        // relative to the walk) — the oriented walker must consume
        // them with orientation=false rather than dead-ending, and the
        // orientations vector must say so.
        let s = sketch();
        let p = [
            Point2d::new(0.0, 0.0),
            Point2d::new(10.0, 0.0),
            Point2d::new(10.0, 10.0),
            Point2d::new(0.0, 10.0),
        ];
        let ids: Vec<_> = p.iter().map(|q| s.add_point(*q)).collect();
        s.add_line(ids[0], ids[1]).expect("ab");
        s.add_line(ids[2], ids[1]).expect("cb reversed");
        s.add_line(ids[2], ids[3]).expect("cd");
        s.add_line(ids[0], ids[3]).expect("ad reversed");
        let topo = analyze(&s);
        assert_eq!(topo.loops().len(), 1);
        let lp = &topo.loops()[0];
        assert_eq!(lp.edges.len(), 4);
        assert!(
            lp.orientations.iter().any(|o| !o),
            "at least one edge must be traversed reversed"
        );
        assert!(topo.is_valid_for_extrusion());
    }

    #[test]
    fn disjoint_rectangles_are_two_regions() {
        let s = sketch();
        s.add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 0.0),
                Point2d::new(10.0, 10.0),
                Point2d::new(0.0, 10.0),
            ],
            true,
        )
        .expect("rect 1");
        s.add_polyline(
            vec![
                Point2d::new(100.0, 0.0),
                Point2d::new(110.0, 0.0),
                Point2d::new(110.0, 10.0),
                Point2d::new(100.0, 10.0),
            ],
            true,
        )
        .expect("rect 2");
        let topo = analyze(&s);
        assert_eq!(topo.loops().len(), 2);
        assert_eq!(topo.regions().len(), 2);
        assert_eq!(topo.profile_type(), ProfileType::Disjoint);
        assert!(topo.is_valid_for_extrusion());
    }
}

#[cfg(test)]
mod spline_profile_tests {
    //! Splines as profile boundaries (SKETCH-DCM showpiece slice,
    //! 2026-06-13): a line+spline closed chain and a closed spline
    //! must both form extrudable regions.
    use super::*;
    use crate::sketch2d::{Sketch, SketchAnchor, Spline2d, Tolerance2d};

    #[test]
    fn line_plus_spline_chain_forms_a_region() {
        let s = Sketch::new("t".to_string(), SketchAnchor::xy());
        // Open cubic B-Spline from (0,0) to (40,0) bulging upward;
        // clamped knot vector so evaluate(0)/evaluate(1) hit the
        // endpoint control points exactly.
        s.add_bspline(
            3,
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 25.0),
                Point2d::new(30.0, 25.0),
                Point2d::new(40.0, 0.0),
            ],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .expect("bspline must construct");
        // Straight return edge closing the chain.
        let a = s.add_point(Point2d::new(40.0, 0.0));
        let b = s.add_point(Point2d::new(0.0, 0.0));
        s.add_line(a, b).expect("closing line");

        let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("analysis succeeds");
        assert_eq!(topo.loops().len(), 1, "spline+line loop expected");
        assert_eq!(topo.regions().len(), 1);
        assert!(topo.is_valid_for_extrusion());
        // Bounds must include the spline's bulge (control hull up to
        // y=25), not just the chord at y=0.
        let (_min, max) = topo.loops()[0].bounds;
        assert!(
            max.y > 10.0,
            "loop bounds must cover the spline bulge, got max.y={}",
            max.y
        );
    }
}

#[cfg(test)]
mod ellipse_profile_tests {
    use super::*;
    use crate::sketch2d::{Sketch, SketchAnchor, Tolerance2d};

    #[test]
    fn lone_ellipse_is_a_closed_simple_profile() {
        let s = Sketch::new("t".to_string(), SketchAnchor::xy());
        s.add_ellipse(Point2d::new(0.0, 0.0), 20.0, 10.0, 0.0)
            .expect("ellipse must construct");
        let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("analysis succeeds");
        assert_eq!(topo.loops().len(), 1, "single-edge ellipse loop");
        assert_eq!(topo.regions().len(), 1);
        assert!(topo.is_valid_for_extrusion());
        let area = topo.loops()[0].area;
        let expected = std::f64::consts::PI * 20.0 * 10.0;
        assert!(
            (area - expected).abs() < 1e-6,
            "pi*a*b from bounds fallback, got {area} want {expected}"
        );
    }

    #[test]
    fn ellipse_inside_rectangle_is_a_hole() {
        let s = Sketch::new("t".to_string(), SketchAnchor::xy());
        s.add_polyline(
            vec![
                Point2d::new(-50.0, -30.0),
                Point2d::new(50.0, -30.0),
                Point2d::new(50.0, 30.0),
                Point2d::new(-50.0, 30.0),
            ],
            true,
        )
        .expect("rect");
        s.add_ellipse(Point2d::new(0.0, 0.0), 20.0, 10.0, 0.0)
            .expect("ellipse");
        let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("analysis succeeds");
        assert_eq!(topo.regions().len(), 1, "ellipse nests as hole");
        assert_eq!(topo.regions()[0].inner_loops.len(), 1);
    }
}
