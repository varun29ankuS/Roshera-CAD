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

use super::constraints::EntityRef;
use super::line2d::LineGeometry;
use super::{
    Point2d,
    Sketch2dError, Sketch2dResult, Tolerance2d, Vector2d,
};
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
                });
            }
        }

        // Note: Ellipses and splines would need special handling
        // For now, we skip them or approximate with polylines

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

    /// Find all closed loops using DFS
    fn find_loops(&mut self) -> Sketch2dResult<()> {
        let mut visited_edges = vec![false; self.edges.len()];
        let mut loops = Vec::new();

        // Try to find loops starting from each unvisited edge
        for start_edge in 0..self.edges.len() {
            if visited_edges[start_edge] {
                continue;
            }

            // Try to form a loop starting from this edge
            if let Some(loop_edges) = self.find_loop_from_edge(start_edge, &mut visited_edges) {
                // Calculate loop properties
                let area = self.calculate_loop_area(&loop_edges);
                let bounds = self.calculate_loop_bounds(&loop_edges);
                let is_ccw = area > 0.0;

                loops.push(SketchLoop {
                    edges: loop_edges,
                    is_ccw,
                    area: area.abs(),
                    bounds,
                });
            }
        }

        self.loops = loops;
        Ok(())
    }

    /// Try to find a loop starting from given edge
    fn find_loop_from_edge(&self, start_edge: usize, visited: &mut [bool]) -> Option<Vec<usize>> {
        let mut path = vec![start_edge];
        let mut current_edge = start_edge;
        visited[start_edge] = true;

        loop {
            // Get end vertex of current edge
            let current = &self.edges[current_edge];
            let end_pos = if path.len() % 2 == 1 {
                current.end
            } else {
                current.start
            };

            // Find next edge connected to this vertex
            let mut next_edge = None;

            for vertex in self.vertices.iter() {
                if vertex.position.distance_to(&end_pos) < 1e-6 {
                    for &(edge_idx, outgoing) in &vertex.edges {
                        if edge_idx != current_edge && !visited[edge_idx] {
                            let edge = &self.edges[edge_idx];
                            let connects = if outgoing {
                                edge.start.distance_to(&end_pos) < 1e-6
                            } else {
                                edge.end.distance_to(&end_pos) < 1e-6
                            };

                            if connects {
                                next_edge = Some(edge_idx);
                                break;
                            }
                        }
                    }
                }

                if next_edge.is_some() {
                    break;
                }
            }

            match next_edge {
                Some(edge_idx) => {
                    // Check if we've completed a loop
                    let next = &self.edges[edge_idx];
                    let next_end = if path.len() % 2 == 0 {
                        next.end
                    } else {
                        next.start
                    };

                    if next_end.distance_to(&self.edges[start_edge].start) < 1e-6 {
                        // Loop completed
                        path.push(edge_idx);
                        return Some(path);
                    }

                    // Continue path
                    visited[edge_idx] = true;
                    path.push(edge_idx);
                    current_edge = edge_idx;
                }
                None => {
                    // Dead end - not a loop
                    // Unmark visited edges
                    for &edge in &path {
                        visited[edge] = false;
                    }
                    return None;
                }
            }

            // Prevent infinite loops
            if path.len() > self.edges.len() {
                return None;
            }
        }
    }

    /// Calculate signed area of a loop
    fn calculate_loop_area(&self, loop_edges: &[usize]) -> f64 {
        let mut area = 0.0;

        for &edge_idx in loop_edges {
            let edge = &self.edges[edge_idx];

            match edge.edge_type {
                EdgeType::Line => {
                    // Shoelace formula contribution
                    area += (edge.end.x - edge.start.x) * (edge.end.y + edge.start.y) / 2.0;
                }
                EdgeType::Arc => {
                    // Arc area contribution
                    // This is more complex and would need the arc parameters
                    // For now, approximate with line segment
                    area += (edge.end.x - edge.start.x) * (edge.end.y + edge.start.y) / 2.0;
                }
                _ => {
                    // Other types approximated as line segments
                    area += (edge.end.x - edge.start.x) * (edge.end.y + edge.start.y) / 2.0;
                }
            }
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
        loop_indices.sort_by(|&a, &b| self.loops[b].area.partial_cmp(&self.loops[a].area).unwrap());

        // Build containment hierarchy
        let mut regions = Vec::new();
        let mut assigned = vec![false; self.loops.len()];

        for &i in &loop_indices {
            if assigned[i] {
                continue;
            }

            let loop_i = &self.loops[i];

            if loop_i.is_ccw {
                // This is an outer boundary
                let mut region = SketchRegion {
                    outer_loop: i,
                    inner_loops: Vec::new(),
                    area: loop_i.area,
                    depth: 0,
                };

                // Find holes inside this boundary
                for &j in &loop_indices {
                    if i != j && !assigned[j] {
                        let loop_j = &self.loops[j];

                        if !loop_j.is_ccw && self.loop_contains_loop(i, j) {
                            region.inner_loops.push(j);
                            region.area -= loop_j.area;
                            assigned[j] = true;
                        }
                    }
                }

                assigned[i] = true;
                regions.push(region);
            }
        }

        self.regions = regions;
        Ok(())
    }

    /// Check if loop i contains loop j
    fn loop_contains_loop(&self, i: usize, j: usize) -> bool {
        let loop_i = &self.loops[i];
        let loop_j = &self.loops[j];

        // Quick bounds check
        let (min_i, max_i) = loop_i.bounds;
        let (min_j, max_j) = loop_j.bounds;

        if min_j.x < min_i.x || max_j.x > max_i.x || min_j.y < min_i.y || max_j.y > max_i.y {
            return false;
        }

        // Would need proper point-in-polygon test here
        true
    }

    /// Classify the profile type
    fn classify_profile(&mut self) {
        let closed_count = self.loops.len();
        let open_count = self.edges.len() - self.loops.iter().map(|l| l.edges.len()).sum::<usize>();

        self.profile_type = if closed_count == 0 && open_count > 0 {
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
