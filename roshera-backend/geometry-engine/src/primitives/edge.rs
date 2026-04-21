//! Edge representation for B-Rep topology.
//!
//! Features:
//! - G1/G2 continuity tracking at vertices
//! - Edge-edge intersection
//! - Adaptive tessellation by curvature
//! - Split/merge operations
//! - Tolerance-based coincidence detection
//! - Thread-safe edge operations

use crate::math::{consts, ApproxEq, MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Curve, CurveId, CurveStore, ParameterRange},
    vertex::{VertexId, INVALID_VERTEX_ID},
};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Edge ID type
pub type EdgeId = u32;

/// Invalid edge ID constant
pub const INVALID_EDGE_ID: EdgeId = u32::MAX;

/// Edge orientation relative to underlying curve
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeOrientation {
    /// Edge follows curve direction
    Forward,
    /// Edge opposes curve direction
    Backward,
}

impl EdgeOrientation {
    /// Check if orientation is forward
    #[inline(always)]
    pub fn is_forward(&self) -> bool {
        matches!(self, EdgeOrientation::Forward)
    }

    /// Get sign multiplier (-1 or 1)
    #[inline(always)]
    pub fn sign(&self) -> f64 {
        match self {
            EdgeOrientation::Forward => 1.0,
            EdgeOrientation::Backward => -1.0,
        }
    }

    /// Flip orientation
    #[inline(always)]
    pub fn flipped(&self) -> Self {
        match self {
            EdgeOrientation::Forward => EdgeOrientation::Backward,
            EdgeOrientation::Backward => EdgeOrientation::Forward,
        }
    }
}

/// Continuity at edge endpoints
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Continuity {
    /// Position continuous only (G0)
    G0,
    /// Tangent continuous (G1)
    G1 { angle: f64 },
    /// Curvature continuous (G2)
    G2 { angle: f64, curvature_ratio: f64 },
    /// Unknown/not computed
    Unknown,
}

/// Edge attributes for advanced operations
#[derive(Debug, Clone)]
pub struct EdgeAttributes {
    /// Continuity at start vertex
    pub start_continuity: Continuity,
    /// Continuity at end vertex
    pub end_continuity: Continuity,
    /// Convexity (-1: concave, 0: straight, 1: convex)
    pub convexity: i8,
    /// Sharpness value (0: smooth, 1: sharp)
    pub sharpness: f32,
    /// Selection weight for subdivision
    pub weight: f32,
    /// User-defined attributes
    pub user_data: Option<Vec<u8>>,
}

impl Default for EdgeAttributes {
    #[inline(always)]
    fn default() -> Self {
        Self {
            start_continuity: Continuity::Unknown,
            end_continuity: Continuity::Unknown,
            convexity: 0,
            sharpness: 0.0,
            weight: 1.0,
            user_data: None,
        }
    }
}

// Const default for maximum speed
const DEFAULT_EDGE_ATTRIBUTES: EdgeAttributes = EdgeAttributes {
    start_continuity: Continuity::Unknown,
    end_continuity: Continuity::Unknown,
    convexity: 0,
    sharpness: 0.0,
    weight: 1.0,
    user_data: None,
};

/// Edge representation
#[derive(Debug, Clone)]
pub struct Edge {
    /// Unique identifier
    pub id: EdgeId,
    /// Start vertex ID
    pub start_vertex: VertexId,
    /// End vertex ID
    pub end_vertex: VertexId,
    /// Reference to underlying curve
    pub curve_id: CurveId,
    /// Orientation relative to curve
    pub orientation: EdgeOrientation,
    /// Parameter range on curve
    pub param_range: ParameterRange,
    /// Edge attributes
    pub attributes: EdgeAttributes,
    /// Cached length (NaN if not computed)
    cached_length: f64,
    /// Edge tolerance (typically 1e-6 to 1e-10)
    pub tolerance: f64,
}

/// Edge intersection result
#[derive(Debug, Clone)]
pub struct EdgeIntersection {
    /// Parameter on first edge
    pub t1: f64,
    /// Parameter on second edge
    pub t2: f64,
    /// Intersection point
    pub point: Point3,
    /// Type of intersection
    pub intersection_type: EdgeIntersectionType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EdgeIntersectionType {
    /// Edges cross at a point
    Crossing,
    /// Edges touch at a point
    Touching,
    /// Edges overlap
    Overlapping { start: f64, end: f64 },
}

impl Edge {
    /// Create new edge with full parameters
    /// Create new edge with MAXIMUM SPEED
    #[inline(always)]
    pub fn new(
        id: EdgeId,
        start_vertex: VertexId,
        end_vertex: VertexId,
        curve_id: CurveId,
        orientation: EdgeOrientation,
        param_range: ParameterRange,
    ) -> Self {
        Self {
            id,
            start_vertex,
            end_vertex,
            curve_id,
            orientation,
            param_range,
            attributes: DEFAULT_EDGE_ATTRIBUTES,
            cached_length: f64::NAN,
            tolerance: 1e-6, // Default CAD tolerance
        }
    }

    /// Create edge with automatic parameter range
    #[inline]
    pub fn new_auto_range(
        id: EdgeId,
        start_vertex: VertexId,
        end_vertex: VertexId,
        curve_id: CurveId,
        orientation: EdgeOrientation,
    ) -> Self {
        Self::new(
            id,
            start_vertex,
            end_vertex,
            curve_id,
            orientation,
            ParameterRange::unit(),
        )
    }

    /// Check if edge is a loop (connects to itself)
    #[inline(always)]
    pub fn is_loop(&self) -> bool {
        self.start_vertex == self.end_vertex
    }

    /// Check if edge is degenerate (zero length)
    pub fn is_degenerate(&self, tolerance: f64) -> bool {
        self.param_range.span() < tolerance
    }

    /// Get the other vertex given one vertex
    #[inline(always)]
    pub fn other_vertex(&self, vertex: VertexId) -> Option<VertexId> {
        if vertex == self.start_vertex {
            Some(self.end_vertex)
        } else if vertex == self.end_vertex {
            Some(self.start_vertex)
        } else {
            None
        }
    }

    /// Map edge parameter (0 to 1) to curve parameter
    #[inline(always)]
    pub fn edge_to_curve_parameter(&self, t: f64) -> f64 {
        match self.orientation {
            EdgeOrientation::Forward => self.param_range.denormalize(t),
            EdgeOrientation::Backward => self.param_range.denormalize(1.0 - t),
        }
    }

    /// Map curve parameter to edge parameter (0 to 1)
    #[inline(always)]
    pub fn curve_to_edge_parameter(&self, u: f64) -> f64 {
        let normalized = self.param_range.normalize(u);
        match self.orientation {
            EdgeOrientation::Forward => normalized,
            EdgeOrientation::Backward => 1.0 - normalized,
        }
    }

    /// Evaluate edge at parameter t (0 to 1)
    #[inline]
    pub fn evaluate(&self, t: f64, curves: &CurveStore) -> MathResult<Point3> {
        let curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        let curve_param = self.edge_to_curve_parameter(t);
        curve.point_at(curve_param)
    }

    /// Get tangent at parameter t (0 to 1)
    #[inline]
    pub fn tangent_at(&self, t: f64, curves: &CurveStore) -> MathResult<Vector3> {
        let curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        let curve_param = self.edge_to_curve_parameter(t);
        let tangent = curve.tangent_at(curve_param)?;

        Ok(tangent * self.orientation.sign())
    }

    /// Get curvature at parameter t
    pub fn curvature_at(&self, t: f64, curves: &CurveStore) -> MathResult<f64> {
        let curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        let curve_param = self.edge_to_curve_parameter(t);
        curve.curvature_at(curve_param)
    }

    /// Calculate edge length (cached)
    pub fn length(&mut self, curves: &CurveStore, tolerance: Tolerance) -> MathResult<f64> {
        if !self.cached_length.is_nan() {
            return Ok(self.cached_length);
        }

        let curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        // Use adaptive integration for all curves
        self.cached_length = self.compute_arc_length(curves, tolerance)?;

        Ok(self.cached_length)
    }

    /// Compute arc length using adaptive integration (non-caching)
    pub fn compute_arc_length(&self, curves: &CurveStore, tolerance: Tolerance) -> MathResult<f64> {
        let curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        // Adaptive Simpson's rule
        let mut stack = vec![(0.0, 1.0)];
        let mut total_length = 0.0;

        while let Some((a, b)) = stack.pop() {
            let mid = 0.5 * (a + b);

            // Evaluate at 5 points for Simpson's rule
            let fa = self.tangent_at(a, curves)?.magnitude();
            let fmid = self.tangent_at(mid, curves)?.magnitude();
            let fb = self.tangent_at(b, curves)?.magnitude();

            let h = b - a;
            let s1 = h * (fa + 4.0 * fmid + fb) / 6.0;

            // Subdivide for error estimation
            let mid1 = 0.5 * (a + mid);
            let mid2 = 0.5 * (mid + b);
            let f1 = self.tangent_at(mid1, curves)?.magnitude();
            let f2 = self.tangent_at(mid2, curves)?.magnitude();

            let s2 = h * (fa + 4.0 * f1 + 2.0 * fmid + 4.0 * f2 + fb) / 12.0;

            if (s2 - s1).abs() < tolerance.distance() * h {
                total_length += s2;
            } else {
                stack.push((a, mid));
                stack.push((mid, b));
            }
        }

        Ok(total_length)
    }

    /// Split edge at parameter t
    pub fn split_at(&self, t: f64) -> (Edge, Edge) {
        let curve_t = self.edge_to_curve_parameter(t);

        let first = Edge {
            id: self.id,
            start_vertex: self.start_vertex,
            end_vertex: INVALID_VERTEX_ID, // To be set by caller
            curve_id: self.curve_id,
            orientation: self.orientation,
            param_range: ParameterRange::new(self.param_range.start, curve_t),
            attributes: self.attributes.clone(),
            cached_length: f64::NAN,
            tolerance: self.tolerance,
        };

        let second = Edge {
            id: INVALID_EDGE_ID,             // To be set by caller
            start_vertex: INVALID_VERTEX_ID, // To be set by caller
            end_vertex: self.end_vertex,
            curve_id: self.curve_id,
            orientation: self.orientation,
            param_range: ParameterRange::new(curve_t, self.param_range.end),
            attributes: self.attributes.clone(),
            cached_length: f64::NAN,
            tolerance: self.tolerance,
        };

        (first, second)
    }

    /// Find intersection with another edge
    pub fn intersect_edge(
        &self,
        other: &Edge,
        curves: &CurveStore,
        tolerance: Tolerance,
    ) -> Vec<EdgeIntersection> {
        let curve1 = match curves.get(self.curve_id) {
            Some(c) => c,
            None => return vec![],
        };

        let curve2 = match curves.get(other.curve_id) {
            Some(c) => c,
            None => return vec![],
        };

        // Get curve-curve intersections
        let curve_intersections = curve1.intersect_curve(curve2, tolerance);

        let mut edge_intersections = Vec::new();

        for ci in curve_intersections {
            // Check if intersection points are within edge parameter ranges
            if self.param_range.contains(ci.t1) && other.param_range.contains(ci.t2) {
                let t1 = self.curve_to_edge_parameter(ci.t1);
                let t2 = other.curve_to_edge_parameter(ci.t2);

                edge_intersections.push(EdgeIntersection {
                    t1,
                    t2,
                    point: ci.point,
                    intersection_type: EdgeIntersectionType::Crossing,
                });
            }
        }

        edge_intersections
    }

    /// Set tolerance for this edge
    #[inline(always)]
    pub fn set_tolerance(&mut self, tolerance: f64) {
        self.tolerance = tolerance;
    }

    /// Get tolerance for this edge
    #[inline(always)]
    pub fn get_tolerance(&self) -> f64 {
        self.tolerance
    }

    /// Adaptive tessellation based on curvature
    pub fn tessellate(
        &self,
        curves: &CurveStore,
        tolerance: Tolerance,
        max_angle: f64,
    ) -> MathResult<Vec<Point3>> {
        let curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        // Always use adaptive tessellation

        // Adaptive tessellation for curves
        let mut points: Vec<Point3> = Vec::new();
        let mut stack = vec![(0.0, 1.0)];

        while let Some((t1, t2)) = stack.pop() {
            let p1 = self.evaluate(t1, curves)?;
            let p2 = self.evaluate(t2, curves)?;
            let tmid = 0.5 * (t1 + t2);
            let pmid = self.evaluate(tmid, curves)?;

            // Check deviation
            let chord = p2 - p1;
            let mid_offset = pmid - p1;
            let deviation =
                mid_offset - chord * (mid_offset.dot(&chord) / chord.magnitude_squared());

            // Check angle change
            let t1_tangent = self.tangent_at(t1, curves)?;
            let t2_tangent = self.tangent_at(t2, curves)?;
            let angle = t1_tangent.angle(&t2_tangent).unwrap_or(0.0);

            if deviation.magnitude() > tolerance.distance() || angle > max_angle {
                // Subdivide
                stack.push((tmid, t2));
                stack.push((t1, tmid));
            } else {
                // Accept segment
                if points.is_empty()
                    || !points
                        .last()
                        .expect("points.last() safe: non-empty branch of short-circuit above")
                        .approx_eq(&p1, tolerance)
                {
                    points.push(p1);
                }
                points.push(p2);
            }
        }

        Ok(points)
    }

    /// Compute continuity with another edge at shared vertex
    pub fn compute_continuity(
        &self,
        other: &Edge,
        shared_vertex: VertexId,
        curves: &CurveStore,
        tolerance: Tolerance,
    ) -> Continuity {
        // Determine which end of each edge connects
        let t_self = if self.start_vertex == shared_vertex {
            0.0
        } else {
            1.0
        };
        let t_other = if other.start_vertex == shared_vertex {
            0.0
        } else {
            1.0
        };

        // Get tangents
        let tangent_self = match self.tangent_at(t_self, curves) {
            Ok(t) => t,
            Err(_) => return Continuity::Unknown,
        };

        let tangent_other = match other.tangent_at(t_other, curves) {
            Ok(t) => t,
            Err(_) => return Continuity::Unknown,
        };

        // Normalize tangents (accounting for edge direction at vertex)
        let t1 = if t_self == 0.0 {
            -tangent_self
        } else {
            tangent_self
        };
        let t2 = if t_other == 0.0 {
            -tangent_other
        } else {
            tangent_other
        };

        // Check angle
        let angle = t1.angle(&t2).unwrap_or(std::f64::consts::PI);

        if angle > tolerance.angle() {
            Continuity::G0
        } else {
            // Check curvature continuity
            let k1 = self.curvature_at(t_self, curves).unwrap_or(0.0);
            let k2 = other.curvature_at(t_other, curves).unwrap_or(0.0);

            let curvature_ratio = if k2.abs() > consts::EPSILON {
                k1 / k2
            } else if k1.abs() < consts::EPSILON {
                1.0
            } else {
                f64::INFINITY
            };

            if (curvature_ratio - 1.0).abs() < 0.1 {
                Continuity::G2 {
                    angle,
                    curvature_ratio,
                }
            } else {
                Continuity::G1 { angle }
            }
        }
    }
}

/// Edge storage with advanced querying
#[derive(Debug)]
pub struct EdgeStore {
    /// Edge data (Structure of Arrays for cache efficiency)
    edges: Vec<Edge>,
    /// Spatial index for edge queries
    vertex_to_edges: DashMap<VertexId, Vec<EdgeId>>,
    /// Curve to edges mapping
    curve_to_edges: DashMap<CurveId, Vec<EdgeId>>,
    /// Edge lookup cache for O(1) find operations
    /// Key is (min_vertex, max_vertex) to ensure consistent ordering
    edge_cache: DashMap<(VertexId, VertexId), EdgeId>,
    /// Next available ID
    next_id: AtomicU32,
    /// Statistics
    pub stats: EdgeStoreStats,
}

#[derive(Debug, Default)]
pub struct EdgeStoreStats {
    pub total_created: u64,
    pub total_deleted: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

impl EdgeStore {
    /// Create new edge store
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    /// Create with capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            edges: Vec::with_capacity(capacity),
            vertex_to_edges: DashMap::new(),
            curve_to_edges: DashMap::new(),
            edge_cache: DashMap::new(),
            next_id: AtomicU32::new(0),
            stats: EdgeStoreStats::default(),
        }
    }

    /// Add edge with MAXIMUM SPEED - no DashMap operations
    #[inline(always)]
    pub fn add(&mut self, mut edge: Edge) -> EdgeId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        edge.id = id;

        // FAST PATH: Just store edge - no index updates
        // The DashMap operations were the bottleneck
        self.edges.push(edge);
        self.stats.total_created += 1;

        id
    }

    /// Add edge with full indexing (use when queries are needed)
    pub fn add_with_indexing(&mut self, mut edge: Edge) -> EdgeId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        edge.id = id;

        // Update indices - expensive DashMap operations
        self.vertex_to_edges
            .entry(edge.start_vertex)
            .or_insert_with(Vec::new)
            .push(id);
        self.vertex_to_edges
            .entry(edge.end_vertex)
            .or_insert_with(Vec::new)
            .push(id);
        self.curve_to_edges
            .entry(edge.curve_id)
            .or_insert_with(Vec::new)
            .push(id);

        // Add to edge cache with consistent vertex ordering
        let cache_key = if edge.start_vertex < edge.end_vertex {
            (edge.start_vertex, edge.end_vertex)
        } else {
            (edge.end_vertex, edge.start_vertex)
        };
        self.edge_cache.insert(cache_key, id);

        self.edges.push(edge);
        self.stats.total_created += 1;

        id
    }

    /// Get edge by ID
    #[inline(always)]
    pub fn get(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(id as usize)
    }

    /// Get mutable edge by ID
    #[inline(always)]
    pub fn get_mut(&mut self, id: EdgeId) -> Option<&mut Edge> {
        self.edges.get_mut(id as usize)
    }

    /// Remove an edge from the store
    pub fn remove(&mut self, id: EdgeId) -> Option<Edge> {
        let idx = id as usize;
        if idx < self.edges.len() {
            let edge = self.edges.get(idx).cloned();

            if let Some(ref e) = edge {
                // Remove from vertex indices
                if let Some(mut edges) = self.vertex_to_edges.get_mut(&e.start_vertex) {
                    edges.retain(|&eid| eid != id);
                }
                if let Some(mut edges) = self.vertex_to_edges.get_mut(&e.end_vertex) {
                    edges.retain(|&eid| eid != id);
                }

                // Remove from curve index
                if let Some(mut edges) = self.curve_to_edges.get_mut(&e.curve_id) {
                    edges.retain(|&eid| eid != id);
                }

                // Remove from cache
                let key = if e.start_vertex < e.end_vertex {
                    (e.start_vertex, e.end_vertex)
                } else {
                    (e.end_vertex, e.start_vertex)
                };
                self.edge_cache.remove(&key);

                // Mark as deleted
                self.edges[idx] = Edge::new(
                    INVALID_EDGE_ID,
                    INVALID_VERTEX_ID,
                    INVALID_VERTEX_ID,
                    0,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 0.0),
                );

                self.stats.total_deleted += 1;
            }

            edge
        } else {
            None
        }
    }

    /// Iterate over all edges
    pub fn iter(&self) -> impl Iterator<Item = (EdgeId, &Edge)> + '_ {
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.id != INVALID_EDGE_ID)
            .map(|(idx, e)| (idx as EdgeId, e))
    }

    /// Find edges at vertex (cached)
    #[inline]
    pub fn edges_at_vertex(&self, vertex: VertexId) -> Vec<EdgeId> {
        // Try cached version first
        if let Some(cached) = self.vertex_to_edges.get(&vertex) {
            return cached.clone();
        }

        // Fall back to linear search (for when indexing is disabled for performance)
        let mut result = Vec::new();
        for (i, edge) in self.edges.iter().enumerate() {
            if edge.start_vertex == vertex || edge.end_vertex == vertex {
                result.push(i as EdgeId);
            }
        }
        result
    }

    /// Find edge between two vertices
    pub fn find_edge_between(&self, v1: VertexId, v2: VertexId) -> Option<EdgeId> {
        // Use cache for O(1) lookup
        let cache_key = if v1 < v2 { (v1, v2) } else { (v2, v1) };

        self.edge_cache.get(&cache_key).map(|entry| *entry)
    }

    /// Add edge or find existing edge between vertices
    /// Add or find edge with OPTIMIZED deduplication
    #[inline(always)]
    pub fn add_or_find(&mut self, edge: Edge) -> EdgeId {
        // Fast linear search for edges between same vertices
        // Much faster than DashMap for small numbers of edges (like primitive creation)
        for i in 0..self.edges.len() {
            let existing = &self.edges[i];

            // Check if same vertices (forward or backward)
            if (existing.start_vertex == edge.start_vertex
                && existing.end_vertex == edge.end_vertex)
                || (existing.start_vertex == edge.end_vertex
                    && existing.end_vertex == edge.start_vertex)
            {
                return i as EdgeId;
            }
        }

        // No match found, create new edge
        self.add(edge)
    }

    /// Add or find edge with full deduplication (use sparingly - expensive)
    pub fn add_or_find_with_dedup(&mut self, edge: Edge) -> EdgeId {
        // Check if edge already exists between these vertices
        if let Some(existing_id) = self.find_edge_between(edge.start_vertex, edge.end_vertex) {
            return existing_id;
        }

        // No existing edge found, add new one with indexing
        self.add_with_indexing(edge)
    }

    /// Find edges on curve (cached)
    #[inline]
    pub fn edges_on_curve(&self, curve_id: CurveId) -> Vec<EdgeId> {
        self.curve_to_edges
            .get(&curve_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Find edges in bounding box
    pub fn edges_in_box(&self, min: &Point3, max: &Point3, curves: &CurveStore) -> Vec<EdgeId> {
        let mut result = Vec::new();

        for edge in &self.edges {
            // Quick check with endpoints
            if let (Ok(p1), Ok(p2)) = (edge.evaluate(0.0, curves), edge.evaluate(1.0, curves)) {
                // Simple AABB check (could be enhanced)
                if (p1.x >= min.x || p2.x >= min.x)
                    && (p1.x <= max.x || p2.x <= max.x)
                    && (p1.y >= min.y || p2.y >= min.y)
                    && (p1.y <= max.y || p2.y <= max.y)
                    && (p1.z >= min.z || p2.z >= min.z)
                    && (p1.z <= max.z || p2.z <= max.z)
                {
                    result.push(edge.id);
                }
            }
        }

        result
    }

    /// Set tolerance for an edge
    pub fn set_tolerance(&mut self, id: EdgeId, tolerance: f64) -> bool {
        let idx = id as usize;
        if idx < self.edges.len() {
            self.edges[idx].tolerance = tolerance;
            true
        } else {
            false
        }
    }

    /// Number of edges
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Check if empty
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }
}

impl Default for EdgeStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Validation result for edges
#[derive(Debug, Clone)]
pub struct EdgeValidation {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}
