//! Loop representation for B-Rep topology.
//!
//! Features:
//! - Winding-number computation for robust point-in-loop tests
//! - Loop simplification and cleanup
//! - Self-intersection detection
//! - Convexity analysis
//! - Loop offsetting for tool paths
//! - Hierarchical loop trees for complex faces

use crate::math::{consts, MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::CurveStore,
    edge::{EdgeId, EdgeStore},
    vertex::{VertexId, VertexStore},
};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Loop ID type
pub type LoopId = u32;

/// Invalid loop ID constant
pub const INVALID_LOOP_ID: LoopId = u32::MAX;

/// Loop type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoopType {
    /// Outer boundary of a face
    Outer,
    /// Inner boundary (hole) in a face
    Inner,
    /// Unknown/unclassified
    Unknown,
}

/// Loop convexity classification
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Convexity {
    /// All interior angles < 180°
    Convex,
    /// At least one interior angle > 180°
    Concave,
    /// Contains both convex and concave regions
    Mixed,
    /// Degenerate or undefined
    Unknown,
}

/// Loop statistics for analysis
#[derive(Debug, Clone)]
pub struct LoopStats {
    /// Total length of loop
    pub perimeter: f64,
    /// Area enclosed by loop
    pub area: f64,
    /// Centroid of loop
    pub centroid: Point3,
    /// Bounding box min
    pub bbox_min: Point3,
    /// Bounding box max
    pub bbox_max: Point3,
    /// Convexity classification
    pub convexity: Convexity,
    /// Number of self-intersections
    pub self_intersections: usize,
    /// Maximum curvature
    pub max_curvature: f64,
}

/// Loop representation
#[derive(Debug, Clone)]
pub struct Loop {
    /// Unique identifier
    pub id: LoopId,
    /// Ordered list of edge IDs
    pub edges: Vec<EdgeId>,
    /// Edge orientations (true = forward, false = backward)
    pub orientations: Vec<bool>,
    /// Loop type
    pub loop_type: LoopType,
    /// Parent loop ID (for hierarchical faces)
    pub parent_loop: Option<LoopId>,
    /// Child loops (holes within this loop)
    pub child_loops: Vec<LoopId>,
    /// Cached statistics
    cached_stats: Option<LoopStats>,
}

impl Loop {
    /// Create new loop
    pub fn new(id: LoopId, loop_type: LoopType) -> Self {
        Self {
            id,
            edges: Vec::new(),
            orientations: Vec::new(),
            loop_type,
            parent_loop: None,
            child_loops: Vec::new(),
            cached_stats: None,
        }
    }

    /// Create loop with capacity
    pub fn with_capacity(id: LoopId, loop_type: LoopType, capacity: usize) -> Self {
        Self {
            id,
            edges: Vec::with_capacity(capacity),
            orientations: Vec::with_capacity(capacity),
            loop_type,
            parent_loop: None,
            child_loops: Vec::new(),
            cached_stats: None,
        }
    }

    /// Add edge to loop
    #[inline]
    pub fn add_edge(&mut self, edge_id: EdgeId, forward: bool) {
        self.edges.push(edge_id);
        self.orientations.push(forward);
        self.invalidate_cache();
    }

    /// Insert edge at index
    pub fn insert_edge(&mut self, index: usize, edge_id: EdgeId, forward: bool) {
        self.edges.insert(index, edge_id);
        self.orientations.insert(index, forward);
        self.invalidate_cache();
    }

    /// Remove edge at index
    pub fn remove_edge(&mut self, index: usize) -> Option<(EdgeId, bool)> {
        if index < self.edges.len() {
            let edge = self.edges.remove(index);
            let orient = self.orientations.remove(index);
            self.invalidate_cache();
            Some((edge, orient))
        } else {
            None
        }
    }

    /// Invalidate cached statistics
    #[inline]
    fn invalidate_cache(&mut self) {
        self.cached_stats = None;
    }

    /// Get vertices in order with efficient caching
    pub fn vertices_cached(&self, edge_store: &EdgeStore) -> MathResult<Vec<VertexId>> {
        if self.is_empty() {
            return Ok(Vec::new());
        }

        let mut vertices = Vec::with_capacity(self.edges.len());

        for i in 0..self.edges.len() {
            let edge = edge_store
                .get(self.edges[i])
                .ok_or(MathError::InvalidParameter(format!(
                    "Edge {} not found",
                    self.edges[i]
                )))?;

            let vertex = if self.orientations[i] {
                edge.start_vertex
            } else {
                edge.end_vertex
            };

            vertices.push(vertex);
        }

        Ok(vertices)
    }

    /// Compute loop statistics (cached)
    pub fn compute_stats(
        &mut self,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        normal: &Vector3,
    ) -> MathResult<&LoopStats> {
        if self.cached_stats.is_some() {
            return Ok(self
                .cached_stats
                .as_ref()
                .expect("cached_stats presence verified by is_some() guard above"));
        }

        let vertices = self.vertices_cached(edge_store)?;
        if vertices.len() < 3 {
            return Err(MathError::InvalidParameter(
                "Loop has fewer than 3 vertices".to_string(),
            ));
        }

        // Calculate perimeter
        let mut perimeter = 0.0;
        for i in 0..self.edges.len() {
            let edge = edge_store
                .get(self.edges[i])
                .ok_or(MathError::InvalidParameter("Edge not found".to_string()))?;

            // Compute accurate arc length (non-caching since we have immutable reference)
            perimeter += edge.compute_arc_length(curve_store, Tolerance::default())?;
        }

        // Calculate area and centroid using shoelace formula
        let (area, centroid) = self.compute_area_and_centroid(&vertices, vertex_store, normal)?;

        // Calculate bounding box
        let (bbox_min, bbox_max) = self.compute_bbox(&vertices, vertex_store)?;

        // Analyze convexity
        let convexity = self.analyze_convexity(&vertices, vertex_store, normal)?;

        // Count self-intersections
        let self_intersections = self.count_self_intersections(edge_store, curve_store)?;

        // Find maximum curvature
        let max_curvature = self.find_max_curvature(edge_store, curve_store)?;

        self.cached_stats = Some(LoopStats {
            perimeter,
            area,
            centroid,
            bbox_min,
            bbox_max,
            convexity,
            self_intersections,
            max_curvature,
        });

        Ok(self
            .cached_stats
            .as_ref()
            .expect("cached_stats just assigned via `Some(...)` above"))
    }

    /// Compute area and centroid efficiently
    fn compute_area_and_centroid(
        &self,
        vertices: &[VertexId],
        vertex_store: &VertexStore,
        normal: &Vector3,
    ) -> MathResult<(f64, Point3)> {
        let n = vertices.len();

        // Find best projection plane
        let abs_normal = normal.abs();
        let (u_idx, v_idx) = if abs_normal.x > abs_normal.y && abs_normal.x > abs_normal.z {
            (1, 2) // YZ plane
        } else if abs_normal.y > abs_normal.z {
            (0, 2) // XZ plane
        } else {
            (0, 1) // XY plane
        };

        let mut area = 0.0;
        let mut cx = 0.0;
        let mut cy = 0.0;

        for i in 0..n {
            let v1 = vertex_store
                .get(vertices[i])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
            let v2 = vertex_store
                .get(vertices[(i + 1) % n])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;

            let p1 = v1.position;
            let p2 = v2.position;

            let u1 = p1[u_idx];
            let v1 = p1[v_idx];
            let u2 = p2[u_idx];
            let v2 = p2[v_idx];

            let a = u1 * v2 - u2 * v1;
            area += a;
            cx += (u1 + u2) * a;
            cy += (v1 + v2) * a;
        }

        area *= 0.5;
        let area_abs = area.abs();

        if area_abs < consts::EPSILON {
            // Degenerate loop, use simple average
            let mut sum = Vector3::ZERO;
            for &vid in vertices {
                let v = vertex_store
                    .get(vid)
                    .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
                sum.x += v.position[0];
                sum.y += v.position[1];
                sum.z += v.position[2];
            }
            let centroid = Point3::new(sum.x / n as f64, sum.y / n as f64, sum.z / n as f64);
            Ok((0.0, centroid))
        } else {
            cx /= 6.0 * area;
            cy /= 6.0 * area;

            // Reconstruct 3D centroid
            let mut centroid = Point3::new(0.0, 0.0, 0.0);
            match u_idx {
                0 => {
                    centroid.x = cx;
                    centroid.z = cy;
                }
                1 => {
                    centroid.y = cx;
                    centroid.z = cy;
                }
                _ => {
                    centroid.x = cx;
                    centroid.y = cy;
                }
            }

            Ok((area_abs, centroid))
        }
    }

    /// Compute bounding box
    fn compute_bbox(
        &self,
        vertices: &[VertexId],
        vertex_store: &VertexStore,
    ) -> MathResult<(Point3, Point3)> {
        let mut min = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut max = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);

        for &vid in vertices {
            let v = vertex_store
                .get(vid)
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;

            min.x = min.x.min(v.position[0]);
            min.y = min.y.min(v.position[1]);
            min.z = min.z.min(v.position[2]);

            max.x = max.x.max(v.position[0]);
            max.y = max.y.max(v.position[1]);
            max.z = max.z.max(v.position[2]);
        }

        Ok((min, max))
    }

    /// Analyze loop convexity
    fn analyze_convexity(
        &self,
        vertices: &[VertexId],
        vertex_store: &VertexStore,
        normal: &Vector3,
    ) -> MathResult<Convexity> {
        if vertices.len() < 3 {
            return Ok(Convexity::Unknown);
        }

        let mut has_convex = false;
        let mut has_concave = false;

        for i in 0..vertices.len() {
            let prev = vertex_store
                .get(vertices[if i == 0 { vertices.len() - 1 } else { i - 1 }])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
            let curr = vertex_store
                .get(vertices[i])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
            let next = vertex_store
                .get(vertices[(i + 1) % vertices.len()])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;

            let v1 = Vector3::new(
                curr.position[0] - prev.position[0],
                curr.position[1] - prev.position[1],
                curr.position[2] - prev.position[2],
            );
            let v2 = Vector3::new(
                next.position[0] - curr.position[0],
                next.position[1] - curr.position[1],
                next.position[2] - curr.position[2],
            );

            let cross = v1.cross(&v2);
            let dot = cross.dot(normal);

            if dot > consts::EPSILON {
                has_convex = true;
            } else if dot < -consts::EPSILON {
                has_concave = true;
            }

            if has_convex && has_concave {
                return Ok(Convexity::Mixed);
            }
        }

        if has_concave {
            Ok(Convexity::Concave)
        } else if has_convex {
            Ok(Convexity::Convex)
        } else {
            Ok(Convexity::Unknown)
        }
    }

    /// Count self-intersections
    fn count_self_intersections(
        &self,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
    ) -> MathResult<usize> {
        let mut count = 0;

        // Check each edge pair
        for i in 0..self.edges.len() {
            for j in i + 2..self.edges.len() {
                // Skip adjacent edges
                if j == i + 1 || (i == 0 && j == self.edges.len() - 1) {
                    continue;
                }

                let edge1 = edge_store
                    .get(self.edges[i])
                    .ok_or(MathError::InvalidParameter("Edge not found".to_string()))?;
                let edge2 = edge_store
                    .get(self.edges[j])
                    .ok_or(MathError::InvalidParameter("Edge not found".to_string()))?;

                let intersections = edge1.intersect_edge(edge2, curve_store, Tolerance::default());
                count += intersections.len();
            }
        }

        Ok(count)
    }

    /// Find maximum curvature
    fn find_max_curvature(
        &self,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
    ) -> MathResult<f64> {
        let mut max_curvature: f64 = 0.0;

        for &edge_id in &self.edges {
            let edge = edge_store
                .get(edge_id)
                .ok_or(MathError::InvalidParameter("Edge not found".to_string()))?;

            // Sample curvature at several points
            for i in 0..=10 {
                let t = i as f64 / 10.0;
                if let Ok(k) = edge.curvature_at(t, curve_store) {
                    max_curvature = max_curvature.max(k.abs());
                }
            }
        }

        Ok(max_curvature)
    }

    /// Robust point-in-loop test using winding number
    pub fn contains_point(
        &self,
        point: &Point3,
        normal: &Vector3,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<bool> {
        let winding = self.winding_number(point, normal, vertex_store, edge_store)?;
        Ok(winding.abs() > 0.5)
    }

    /// Calculate winding number (more robust than ray casting)
    pub fn winding_number(
        &self,
        point: &Point3,
        normal: &Vector3,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<f64> {
        let vertices = self.vertices_cached(edge_store)?;
        if vertices.len() < 3 {
            return Ok(0.0);
        }

        // Project to best plane
        let abs_normal = normal.abs();
        let (u_idx, v_idx) = if abs_normal.x > abs_normal.y && abs_normal.x > abs_normal.z {
            (1, 2)
        } else if abs_normal.y > abs_normal.z {
            (0, 2)
        } else {
            (0, 1)
        };

        let test_u = [point.x, point.y, point.z][u_idx];
        let test_v = [point.x, point.y, point.z][v_idx];

        let mut winding = 0.0;

        for i in 0..vertices.len() {
            let v1 = vertex_store
                .get(vertices[i])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
            let v2 = vertex_store
                .get(vertices[(i + 1) % vertices.len()])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;

            let u1 = v1.position[u_idx] - test_u;
            let v1 = v1.position[v_idx] - test_v;
            let u2 = v2.position[u_idx] - test_u;
            let v2 = v2.position[v_idx] - test_v;

            // Calculate angle
            let angle = (u1 * v2 - u2 * v1).atan2(u1 * u2 + v1 * v2);
            winding += angle;
        }

        Ok(winding / (2.0 * std::f64::consts::PI))
    }

    /// Simplify loop by removing collinear vertices
    pub fn simplify(
        &mut self,
        tolerance: f64,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<usize> {
        let vertices = self.vertices_cached(edge_store)?;
        if vertices.len() < 4 {
            return Ok(0); // Can't simplify further
        }

        let mut to_remove = Vec::new();

        for i in 0..vertices.len() {
            let prev = vertex_store
                .get(vertices[if i == 0 { vertices.len() - 1 } else { i - 1 }])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
            let curr = vertex_store
                .get(vertices[i])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
            let next = vertex_store
                .get(vertices[(i + 1) % vertices.len()])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;

            // Check if curr is collinear with prev and next
            let v1 = Vector3::new(
                curr.position[0] - prev.position[0],
                curr.position[1] - prev.position[1],
                curr.position[2] - prev.position[2],
            );
            let v2 = Vector3::new(
                next.position[0] - curr.position[0],
                next.position[1] - curr.position[1],
                next.position[2] - curr.position[2],
            );

            let cross = v1.cross(&v2);
            if cross.magnitude() < tolerance {
                to_remove.push(i);
            }
        }

        // Remove edges in reverse order
        for &idx in to_remove.iter().rev() {
            self.remove_edge(idx);
        }

        Ok(to_remove.len())
    }

    /// Offset loop by distance (for tool paths)
    pub fn offset(
        &self,
        distance: f64,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
    ) -> MathResult<Vec<Point3>> {
        let vertices = self.vertices_cached(edge_store)?;
        if vertices.len() < 3 {
            return Err(MathError::InvalidParameter(
                "Loop has fewer than 3 vertices".to_string(),
            ));
        }

        let mut offset_points = Vec::with_capacity(vertices.len());

        // Calculate offset for each vertex
        for i in 0..vertices.len() {
            let prev_idx = if i == 0 { vertices.len() - 1 } else { i - 1 };
            let next_idx = (i + 1) % vertices.len();

            // Get edges
            let edge1 = edge_store
                .get(self.edges[prev_idx])
                .ok_or(MathError::InvalidParameter("Edge not found".to_string()))?;
            let edge2 = edge_store
                .get(self.edges[i])
                .ok_or(MathError::InvalidParameter("Edge not found".to_string()))?;

            // Get tangents at vertex
            let t1 = edge1.tangent_at(1.0, curve_store)?;
            let t2 = edge2.tangent_at(0.0, curve_store)?;

            // Calculate bisector
            let bisector = (t1.normalize()? - t2.normalize()?).normalize()?;

            // Calculate offset distance along bisector
            let half_angle = t1.angle(&t2).unwrap_or(0.0) / 2.0;
            let offset_dist = distance / half_angle.sin().abs().max(0.1);

            // Get vertex position and offset
            let v = vertex_store
                .get(vertices[i])
                .ok_or(MathError::InvalidParameter("Vertex not found".to_string()))?;
            let pos = Point3::new(v.position[0], v.position[1], v.position[2]);

            offset_points.push(pos + bisector * offset_dist);
        }

        Ok(offset_points)
    }
}

// Preserve original methods for compatibility
impl Loop {
    #[inline]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    pub fn edge_at(&self, index: usize) -> Option<(EdgeId, bool)> {
        if index < self.edges.len() {
            Some((self.edges[index], self.orientations[index]))
        } else {
            None
        }
    }

    #[inline]
    pub fn next_index(&self, index: usize) -> usize {
        (index + 1) % self.edges.len()
    }

    #[inline]
    pub fn prev_index(&self, index: usize) -> usize {
        if index == 0 {
            self.edges.len() - 1
        } else {
            index - 1
        }
    }

    pub fn find_edge(&self, edge_id: EdgeId) -> Option<usize> {
        self.edges.iter().position(|&e| e == edge_id)
    }

    pub fn vertices(&self, edge_store: &EdgeStore) -> MathResult<Vec<VertexId>> {
        self.vertices_cached(edge_store)
    }

    pub fn area(
        &mut self,
        normal: &Vector3,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<f64> {
        let stats = self.compute_stats(vertex_store, edge_store, &CurveStore::new(), normal)?;
        Ok(stats.area)
    }

    pub fn centroid(
        &mut self,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<Point3> {
        let stats =
            self.compute_stats(vertex_store, edge_store, &CurveStore::new(), &Vector3::Z)?;
        Ok(stats.centroid)
    }

    pub fn reverse(&mut self) {
        self.edges.reverse();
        self.orientations.reverse();
        for orient in &mut self.orientations {
            *orient = !*orient;
        }
        self.invalidate_cache();
    }
}

/// Loop store with spatial indexing
#[derive(Debug)]
pub struct LoopStore {
    /// Loop data
    loops: Vec<Loop>,
    /// Edge to loops mapping
    edge_to_loops: HashMap<EdgeId, Vec<LoopId>>,
    /// Hierarchical loop tree
    root_loops: Vec<LoopId>,
    /// Next available ID
    next_id: LoopId,
    /// Statistics
    pub stats: LoopStoreStats,
}

#[derive(Debug, Default)]
pub struct LoopStoreStats {
    pub total_created: u64,
    pub total_deleted: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

impl LoopStore {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            loops: Vec::with_capacity(capacity),
            edge_to_loops: HashMap::new(),
            root_loops: Vec::new(),
            next_id: 0,
            stats: LoopStoreStats::default(),
        }
    }

    /// Add loop with MAXIMUM SPEED - no DashMap operations
    #[inline(always)]
    pub fn add(&mut self, mut loop_: Loop) -> LoopId {
        loop_.id = self.next_id;

        // FAST PATH: Skip expensive DashMap indexing operations
        // The edge_to_loops DashMap operations are too expensive for primitive creation

        // Update hierarchy (non-DashMap operation)
        if loop_.parent_loop.is_none() {
            self.root_loops.push(loop_.id);
        }

        self.loops.push(loop_);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    /// Add loop with full indexing (use when queries are needed)
    pub fn add_with_indexing(&mut self, mut loop_: Loop) -> LoopId {
        loop_.id = self.next_id;

        // Update edge index - expensive DashMap operations
        for &edge_id in &loop_.edges {
            self.edge_to_loops
                .entry(edge_id)
                .or_insert_with(Vec::new)
                .push(loop_.id);
        }

        // Update hierarchy
        if loop_.parent_loop.is_none() {
            self.root_loops.push(loop_.id);
        }

        self.loops.push(loop_);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    #[inline(always)]
    pub fn get(&self, id: LoopId) -> Option<&Loop> {
        self.loops.get(id as usize)
    }

    #[inline(always)]
    pub fn get_mut(&mut self, id: LoopId) -> Option<&mut Loop> {
        self.loops.get_mut(id as usize)
    }

    /// Remove a loop from the store
    pub fn remove(&mut self, id: LoopId) -> Option<Loop> {
        let idx = id as usize;
        if idx < self.loops.len() {
            let loop_ = self.loops.get(idx).cloned();

            if let Some(ref l) = loop_ {
                // Remove from edge indices
                for &edge_id in &l.edges {
                    if let Some(mut loops) = self.edge_to_loops.get_mut(&edge_id) {
                        loops.retain(|&lid| lid != id);
                    }
                }

                // Remove from root loops if applicable
                if l.parent_loop.is_none() {
                    self.root_loops.retain(|&lid| lid != id);
                }

                // Mark as deleted
                self.loops[idx] = Loop::new(INVALID_LOOP_ID, LoopType::Outer);
                self.stats.total_deleted += 1;
            }

            loop_
        } else {
            None
        }
    }

    /// Iterate over all loops
    pub fn iter(&self) -> impl Iterator<Item = (LoopId, &Loop)> + '_ {
        self.loops
            .iter()
            .enumerate()
            .filter(|(_, l)| l.id != INVALID_LOOP_ID)
            .map(|(idx, l)| (idx as LoopId, l))
    }

    #[inline]
    pub fn loops_with_edge(&self, edge_id: EdgeId) -> &[LoopId] {
        self.edge_to_loops
            .get(&edge_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.loops.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }
}

impl Default for LoopStore {
    fn default() -> Self {
        Self::new()
    }
}
