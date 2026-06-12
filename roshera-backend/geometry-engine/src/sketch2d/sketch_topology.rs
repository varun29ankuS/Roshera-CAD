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
    /// Whether the loop is oriented counter-clockwise
    pub is_ccw: bool,
    /// Area enclosed by the loop (positive for CCW)
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

    /// Build edges from sketch entities
    fn build_edges(&mut self, sketch: &super::Sketch) -> Sketch2dResult<()> {
        // Add line edges
        for entry in sketch.lines().iter() {
            let line = entry.value();
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
                    });
                }
            }
        }

        // Add arc edges
        for entry in sketch.arcs().iter() {
            let arc = entry.value();
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
            });
        }

        // Add circle edges (closed loops)
        for entry in sketch.circles().iter() {
            let circle = entry.value();
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
            });
        }

        // Add rectangle edges (4 segments)
        for entry in sketch.rectangles().iter() {
            let rect = entry.value();
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
                });
            }
        }

        // Add polyline edges
        for entry in sketch.polylines().iter() {
            let polyline = entry.value();
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
                });
            }
        }

        // Ellipses and splines are deliberately not added to the topology
        // edge graph: they can introduce non-circular curvature that the
        // current loop-extraction passes (line/arc/polyline only) cannot
        // walk without sampling. Sketches containing them produce open
        // profiles and surface as such in validation.
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
            // edge) closes onto itself and needs no walk.
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
                    is_ccw: area > 0.0,
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
                let is_ccw = area > 0.0;
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

        // A single-edge closed loop (full circle) has a degenerate
        // chord polyline — the even-odd ray cast below would see zero
        // crossings and report "contains nothing". Decide analytically
        // from its bounds instead (centre + radius are exact for a
        // circle's axis-aligned box).
        if loop_i.edges.len() == 1 {
            let (min_i, max_i) = loop_i.bounds;
            let cx = (min_i.x + max_i.x) / 2.0;
            let cy = (min_i.y + max_i.y) / 2.0;
            let r = (max_i.x - min_i.x) / 2.0;
            let dx = test.x - cx;
            let dy = test.y - cy;
            return dx * dx + dy * dy < r * r;
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
