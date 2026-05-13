//! Edge representation for B-Rep topology.
//!
//! Features:
//! - G1/G2 continuity tracking at vertices
//! - Edge-edge intersection
//! - Adaptive tessellation by curvature
//! - Split/merge operations
//! - Tolerance-based coincidence detection
//! - Thread-safe edge operations
//!
//! Indexed access into vertex / curve enumeration arrays is the canonical
//! idiom for edge topology walks — bounded by enumeration length. Matches the
//! pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{consts, ApproxEq, MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{CurveId, CurveStore, ParameterRange},
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

    /// Drop the cached arc-length so the next [`Edge::length`] call
    /// recomputes from the underlying curve.
    ///
    /// Call this whenever `param_range` is mutated in place (for example,
    /// when blend topology surgery re-trims an edge to terminate at a
    /// new vertex). The cached length is private and would otherwise go
    /// stale relative to the new range.
    #[inline]
    pub fn invalidate_length_cache(&mut self) {
        self.cached_length = f64::NAN;
    }

    /// Calculate edge length (cached)
    pub fn length(&mut self, curves: &CurveStore, tolerance: Tolerance) -> MathResult<f64> {
        if !self.cached_length.is_nan() {
            return Ok(self.cached_length);
        }

        let _curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        // Use adaptive integration for all curves
        self.cached_length = self.compute_arc_length(curves, tolerance)?;

        Ok(self.cached_length)
    }

    /// Compute arc length using adaptive integration (non-caching)
    ///
    /// Integrates `|dC/du|` over the curve parameter range
    /// `[param_range.start, param_range.end]`. The previous implementation
    /// used `Edge::tangent_at` which returns a *normalized* tangent (unit
    /// magnitude), so the integral collapsed to the length of the unit
    /// interval (≈ 1.0) regardless of edge geometry — every box edge then
    /// validated as 1 mm long, which broke fillet-radius validation
    /// (`radius > edge_length * 0.5`) for any non-trivial radius.
    pub fn compute_arc_length(&self, curves: &CurveStore, tolerance: Tolerance) -> MathResult<f64> {
        let curve = curves
            .get(self.curve_id)
            .ok_or(MathError::InvalidParameter("Invalid curve ID".to_string()))?;

        // Speed function s(u) = |dC/du| evaluated on the underlying curve's
        // parameter axis.
        let speed = |u: f64| -> MathResult<f64> {
            let cp = curve.evaluate(u)?;
            Ok(cp.derivative1.magnitude())
        };

        // Adaptive Simpson's rule over the curve parameter range. The
        // edge `param_range` is honoured directly so trimmed edges
        // integrate over the correct subinterval.
        let u_start = self.param_range.start;
        let u_end = self.param_range.end;
        if (u_end - u_start).abs() <= f64::EPSILON {
            return Ok(0.0);
        }
        let (lo, hi) = if u_start <= u_end {
            (u_start, u_end)
        } else {
            (u_end, u_start)
        };

        let mut stack = vec![(lo, hi)];
        let mut total_length = 0.0;

        while let Some((a, b)) = stack.pop() {
            let mid = 0.5 * (a + b);

            // Evaluate at 5 points for Simpson's rule
            let fa = speed(a)?;
            let fmid = speed(mid)?;
            let fb = speed(b)?;

            let h = b - a;
            let s1 = h * (fa + 4.0 * fmid + fb) / 6.0;

            // Subdivide for error estimation
            let mid1 = 0.5 * (a + mid);
            let mid2 = 0.5 * (mid + b);
            let f1 = speed(mid1)?;
            let f2 = speed(mid2)?;

            let s2 = h * (fa + 4.0 * f1 + 2.0 * fmid + 4.0 * f2 + fb) / 12.0;

            // Guard against pathological refinement on tiny intervals: stop
            // once the subinterval is at the tolerance scale.
            if (s2 - s1).abs() < tolerance.distance() * h.max(1.0)
                || h < tolerance.distance() * 1e-3
            {
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
    #[allow(clippy::expect_used)] // points non-empty: short-circuit guard !is_empty branch
    pub fn tessellate(
        &self,
        curves: &CurveStore,
        tolerance: Tolerance,
        max_angle: f64,
    ) -> MathResult<Vec<Point3>> {
        let _curve = curves
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

#[derive(Debug, Default, Clone)]
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

    /// Deep copy of this store for the F2-δ ModelSnapshot primitive.
    /// Three DashMap indexes are rebuilt entry-by-entry; `Edge` derives
    /// `Clone`, so the backing `Vec` clones cleanly.
    pub(crate) fn deep_copy(&self) -> Self {
        let vertex_to_edges = DashMap::with_capacity(self.vertex_to_edges.len());
        for kv in self.vertex_to_edges.iter() {
            vertex_to_edges.insert(*kv.key(), kv.value().clone());
        }
        let curve_to_edges = DashMap::with_capacity(self.curve_to_edges.len());
        for kv in self.curve_to_edges.iter() {
            curve_to_edges.insert(*kv.key(), kv.value().clone());
        }
        let edge_cache = DashMap::with_capacity(self.edge_cache.len());
        for kv in self.edge_cache.iter() {
            edge_cache.insert(*kv.key(), *kv.value());
        }
        Self {
            edges: self.edges.clone(),
            vertex_to_edges,
            curve_to_edges,
            edge_cache,
            next_id: AtomicU32::new(self.next_id.load(Ordering::Acquire)),
            stats: self.stats.clone(),
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
            .or_default()
            .push(id);
        self.vertex_to_edges
            .entry(edge.end_vertex)
            .or_default()
            .push(id);
        self.curve_to_edges
            .entry(edge.curve_id)
            .or_default()
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
        // Filter sentinels (edges removed via `remove`, which writes
        // INVALID_EDGE_ID into the slot rather than truly deleting it
        // — IDs are stable and slots are not reused). Without this
        // filter `iter()` and `get()` disagree: callers that walk
        // `iter` correctly skip removed edges, while callers that
        // `.get(id)` after a remove get back a sentinel `Edge` with
        // INVALID_VERTEX_ID/INVALID_EDGE_ID — silently corrupting any
        // downstream logic that proceeds on the assumption that
        // `Some(&Edge)` means "live edge". See Task #89: this caused
        // the cylinder-rim fillet to leave the retired rim edge
        // visible to topology queries even though `remove` had been
        // called.
        self.edges.get(id as usize).filter(|e| e.id != INVALID_EDGE_ID)
    }

    /// Get mutable edge by ID
    #[inline(always)]
    pub fn get_mut(&mut self, id: EdgeId) -> Option<&mut Edge> {
        // Sentinel-filter parity with `get`. See the comment there for
        // why removed slots must not surface through the lookup API.
        self.edges
            .get_mut(id as usize)
            .filter(|e| e.id != INVALID_EDGE_ID)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::{Arc, Line};
    use std::f64::consts::{FRAC_PI_2, PI};

    // ---- EdgeOrientation ----------------------------------------------------

    #[test]
    fn orientation_forward_is_forward_and_positive() {
        assert!(EdgeOrientation::Forward.is_forward());
        assert_eq!(EdgeOrientation::Forward.sign(), 1.0);
    }

    #[test]
    fn orientation_backward_is_not_forward_and_negative() {
        assert!(!EdgeOrientation::Backward.is_forward());
        assert_eq!(EdgeOrientation::Backward.sign(), -1.0);
    }

    #[test]
    fn orientation_flipped_swaps_direction() {
        assert_eq!(EdgeOrientation::Forward.flipped(), EdgeOrientation::Backward);
        assert_eq!(EdgeOrientation::Backward.flipped(), EdgeOrientation::Forward);
    }

    #[test]
    fn orientation_double_flip_is_identity() {
        assert_eq!(
            EdgeOrientation::Forward.flipped().flipped(),
            EdgeOrientation::Forward
        );
    }

    // ---- EdgeAttributes -----------------------------------------------------

    #[test]
    fn edge_attributes_default_is_unknown_and_smooth() {
        let attrs = EdgeAttributes::default();
        assert!(matches!(attrs.start_continuity, Continuity::Unknown));
        assert!(matches!(attrs.end_continuity, Continuity::Unknown));
        assert_eq!(attrs.convexity, 0);
        assert_eq!(attrs.sharpness, 0.0);
        assert_eq!(attrs.weight, 1.0);
        assert!(attrs.user_data.is_none());
    }

    // ---- Edge construction --------------------------------------------------

    fn make_edge(orientation: EdgeOrientation, range: ParameterRange) -> Edge {
        Edge::new(0, 1, 2, 0, orientation, range)
    }

    #[test]
    fn edge_new_stores_all_supplied_fields() {
        let e = Edge::new(
            10,
            5,
            7,
            3,
            EdgeOrientation::Backward,
            ParameterRange::new(0.25, 0.75),
        );
        assert_eq!(e.id, 10);
        assert_eq!(e.start_vertex, 5);
        assert_eq!(e.end_vertex, 7);
        assert_eq!(e.curve_id, 3);
        assert_eq!(e.orientation, EdgeOrientation::Backward);
        assert_eq!(e.param_range.start, 0.25);
        assert_eq!(e.param_range.end, 0.75);
        assert!(e.cached_length.is_nan());
        assert!((e.tolerance - 1e-6).abs() < 1e-15);
    }

    #[test]
    fn edge_new_auto_range_yields_unit_range() {
        let e = Edge::new_auto_range(0, 1, 2, 0, EdgeOrientation::Forward);
        assert_eq!(e.param_range.start, 0.0);
        assert_eq!(e.param_range.end, 1.0);
    }

    // ---- Predicates ---------------------------------------------------------

    #[test]
    fn edge_is_loop_when_start_equals_end() {
        let e = Edge::new(0, 4, 4, 0, EdgeOrientation::Forward, ParameterRange::unit());
        assert!(e.is_loop());
    }

    #[test]
    fn edge_is_not_loop_when_distinct_vertices() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::unit());
        assert!(!e.is_loop());
    }

    #[test]
    fn edge_is_degenerate_when_param_span_below_tolerance() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::new(0.5, 0.5));
        assert!(e.is_degenerate(1e-6));
    }

    #[test]
    fn edge_is_not_degenerate_for_unit_range() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::unit());
        assert!(!e.is_degenerate(1e-6));
    }

    #[test]
    fn edge_other_vertex_at_start_returns_end() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::unit());
        assert_eq!(e.other_vertex(1), Some(2));
    }

    #[test]
    fn edge_other_vertex_at_end_returns_start() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::unit());
        assert_eq!(e.other_vertex(2), Some(1));
    }

    #[test]
    fn edge_other_vertex_unknown_returns_none() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::unit());
        assert!(e.other_vertex(99).is_none());
    }

    // ---- Parameter mapping --------------------------------------------------

    #[test]
    fn edge_to_curve_parameter_forward_denormalizes() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::new(0.2, 0.8));
        // t=0 → 0.2, t=1 → 0.8, t=0.5 → 0.5
        assert!((e.edge_to_curve_parameter(0.0) - 0.2).abs() < 1e-15);
        assert!((e.edge_to_curve_parameter(1.0) - 0.8).abs() < 1e-15);
        assert!((e.edge_to_curve_parameter(0.5) - 0.5).abs() < 1e-15);
    }

    #[test]
    fn edge_to_curve_parameter_backward_reverses() {
        let e = make_edge(EdgeOrientation::Backward, ParameterRange::new(0.2, 0.8));
        assert!((e.edge_to_curve_parameter(0.0) - 0.8).abs() < 1e-15);
        assert!((e.edge_to_curve_parameter(1.0) - 0.2).abs() < 1e-15);
    }

    #[test]
    fn curve_to_edge_parameter_inverts_edge_to_curve_forward() {
        let e = make_edge(EdgeOrientation::Forward, ParameterRange::new(0.2, 0.8));
        for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let u = e.edge_to_curve_parameter(t);
            let round_trip = e.curve_to_edge_parameter(u);
            assert!((round_trip - t).abs() < 1e-12, "t={} round trip={}", t, round_trip);
        }
    }

    #[test]
    fn curve_to_edge_parameter_inverts_edge_to_curve_backward() {
        let e = make_edge(EdgeOrientation::Backward, ParameterRange::new(0.1, 0.9));
        for &t in &[0.0, 0.3, 0.6, 1.0] {
            let u = e.edge_to_curve_parameter(t);
            let round_trip = e.curve_to_edge_parameter(u);
            assert!((round_trip - t).abs() < 1e-12);
        }
    }

    // ---- Evaluation against real curves ------------------------------------

    fn line_store(start: Point3, end: Point3) -> (CurveStore, CurveId) {
        let mut s = CurveStore::new();
        let id = s.add(Box::new(Line::new(start, end)));
        (s, id)
    }

    #[test]
    fn edge_evaluate_forward_line_matches_lerp() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let p_start = e.evaluate(0.0, &cs).expect("eval");
        let p_mid = e.evaluate(0.5, &cs).expect("eval");
        let p_end = e.evaluate(1.0, &cs).expect("eval");
        assert!((p_start.x - 0.0).abs() < 1e-12);
        assert!((p_mid.x - 5.0).abs() < 1e-12);
        assert!((p_end.x - 10.0).abs() < 1e-12);
    }

    #[test]
    fn edge_evaluate_backward_line_reverses_endpoints() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Backward, ParameterRange::unit());
        let p0 = e.evaluate(0.0, &cs).expect("eval");
        let p1 = e.evaluate(1.0, &cs).expect("eval");
        assert!((p0.x - 10.0).abs() < 1e-12);
        assert!((p1.x - 0.0).abs() < 1e-12);
    }

    #[test]
    fn edge_evaluate_returns_err_for_invalid_curve_id() {
        let cs = CurveStore::new();
        let e = Edge::new(0, 1, 2, 99, EdgeOrientation::Forward, ParameterRange::unit());
        assert!(e.evaluate(0.5, &cs).is_err());
    }

    #[test]
    fn edge_tangent_at_forward_line_points_along_direction() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let tan = e.tangent_at(0.5, &cs).expect("tan");
        // Curve::tangent_at returns the unit tangent of the underlying curve.
        // Line is along +x, so unit tangent = (1, 0, 0).
        assert!((tan.x - 1.0).abs() < 1e-12, "got {}", tan.x);
        assert!(tan.y.abs() < 1e-12);
        assert!(tan.z.abs() < 1e-12);
    }

    #[test]
    fn edge_tangent_at_backward_line_reverses_sign() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Backward, ParameterRange::unit());
        let tan = e.tangent_at(0.5, &cs).expect("tan");
        assert!((tan.x + 1.0).abs() < 1e-12, "got {}", tan.x);
    }

    // ---- length / compute_arc_length ---------------------------------------

    #[test]
    fn edge_length_of_unit_line_segment_is_segment_length() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(3.0, 4.0, 0.0));
        let mut e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let len = e.length(&cs, Tolerance::from_distance(1e-6)).expect("len");
        assert!((len - 5.0).abs() < 1e-6, "got {}", len);
    }

    #[test]
    fn edge_length_caches_result() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0));
        let mut e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let _ = e.length(&cs, Tolerance::from_distance(1e-6)).expect("len");
        // After first call, cached_length should be non-NaN.
        assert!(!e.cached_length.is_nan());
        // Second call returns cached value (mutating the store would not
        // invalidate the cache — we don't; the cache is the contract).
        let len2 = e.length(&cs, Tolerance::from_distance(1e-6)).expect("len");
        assert!((len2 - 2.0).abs() < 1e-6);
    }

    #[test]
    fn edge_compute_arc_length_quarter_circle_is_pi_over_two() {
        let arc = Arc::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
            0.0,
            FRAC_PI_2,
        )
        .expect("arc");
        let mut cs = CurveStore::new();
        let cid = cs.add(Box::new(arc));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let arc_len = e
            .compute_arc_length(&cs, Tolerance::from_distance(1e-6))
            .expect("arc len");
        assert!((arc_len - FRAC_PI_2).abs() < 1e-3, "got {}", arc_len);
    }

    #[test]
    fn edge_compute_arc_length_full_circle_is_two_pi() {
        let circle = Arc::circle(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
        )
        .expect("circle");
        let mut cs = CurveStore::new();
        let cid = cs.add(Box::new(circle));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let len = e
            .compute_arc_length(&cs, Tolerance::from_distance(1e-6))
            .expect("len");
        assert!((len - 2.0 * PI).abs() < 1e-3, "got {}", len);
    }

    #[test]
    fn edge_compute_arc_length_zero_for_collapsed_range() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let e = Edge::new(
            0,
            1,
            2,
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.5, 0.5),
        );
        let len = e
            .compute_arc_length(&cs, Tolerance::from_distance(1e-6))
            .expect("len");
        assert_eq!(len, 0.0);
    }

    // ---- split_at -----------------------------------------------------------

    #[test]
    fn edge_split_at_partitions_param_range() {
        let e = Edge::new(
            5,
            1,
            2,
            7,
            EdgeOrientation::Forward,
            ParameterRange::new(0.2, 0.8),
        );
        let (left, right) = e.split_at(0.5);
        // Left preserves id and start vertex; loses end vertex (callers fill in).
        assert_eq!(left.id, 5);
        assert_eq!(left.start_vertex, 1);
        assert_eq!(left.end_vertex, INVALID_VERTEX_ID);
        assert_eq!(left.curve_id, 7);
        assert!((left.param_range.start - 0.2).abs() < 1e-15);
        assert!((left.param_range.end - 0.5).abs() < 1e-15);
        // Right gets fresh placeholder ids; preserves end vertex.
        assert_eq!(right.id, INVALID_EDGE_ID);
        assert_eq!(right.start_vertex, INVALID_VERTEX_ID);
        assert_eq!(right.end_vertex, 2);
        assert!((right.param_range.start - 0.5).abs() < 1e-15);
        assert!((right.param_range.end - 0.8).abs() < 1e-15);
    }

    #[test]
    fn edge_split_at_with_backward_orientation_partitions_in_curve_space() {
        let e = Edge::new(
            5,
            1,
            2,
            7,
            EdgeOrientation::Backward,
            ParameterRange::new(0.0, 1.0),
        );
        // edge_to_curve_parameter(0.5) for Backward range[0,1] = 1 - 0.5 = 0.5.
        let (left, right) = e.split_at(0.5);
        assert!((left.param_range.start - 0.0).abs() < 1e-15);
        assert!((left.param_range.end - 0.5).abs() < 1e-15);
        assert!((right.param_range.start - 0.5).abs() < 1e-15);
        assert!((right.param_range.end - 1.0).abs() < 1e-15);
    }

    // ---- tessellation -------------------------------------------------------

    #[test]
    fn edge_tessellate_line_returns_two_endpoints() {
        let (cs, cid) = line_store(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let pts = e
            .tessellate(&cs, Tolerance::from_distance(1e-3), FRAC_PI_2)
            .expect("tess");
        assert!(pts.len() >= 2, "tessellation must produce at least the endpoints");
        assert!((pts.first().expect("first").x - 0.0).abs() < 1e-6);
        assert!((pts.last().expect("last").x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn edge_tessellate_arc_subdivides_for_curvature() {
        let arc = Arc::circle(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
        )
        .expect("circle");
        let mut cs = CurveStore::new();
        let cid = cs.add(Box::new(arc));
        let e = Edge::new(0, 1, 2, cid, EdgeOrientation::Forward, ParameterRange::unit());
        let pts = e
            .tessellate(&cs, Tolerance::from_distance(1e-2), 0.1)
            .expect("tess");
        // Tight angle tolerance forces many subdivisions.
        assert!(pts.len() > 8, "expected multi-segment circle tessellation, got {}", pts.len());
    }

    // ---- compute_continuity ------------------------------------------------

    #[test]
    fn compute_continuity_returns_unknown_for_unknown_curve_ids() {
        let cs = CurveStore::new();
        let a = Edge::new(0, 1, 2, 99, EdgeOrientation::Forward, ParameterRange::unit());
        let b = Edge::new(1, 2, 3, 99, EdgeOrientation::Forward, ParameterRange::unit());
        let c = a.compute_continuity(&b, 2, &cs, Tolerance::from_distance(1e-6));
        assert!(matches!(c, Continuity::Unknown));
    }

    #[test]
    fn compute_continuity_g0_at_sharp_corner_between_two_lines() {
        // Two line edges meeting at the origin at right angles.
        let mut cs = CurveStore::new();
        let cid_a = cs.add(Box::new(Line::new(
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
        )));
        let cid_b = cs.add(Box::new(Line::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        )));
        let a = Edge::new(0, 1, 2, cid_a, EdgeOrientation::Forward, ParameterRange::unit());
        let b = Edge::new(1, 2, 3, cid_b, EdgeOrientation::Forward, ParameterRange::unit());
        // Shared vertex = vertex 2; edge a ends at it, edge b starts at it.
        let c = a.compute_continuity(&b, 2, &cs, Tolerance::new(1e-6, 1e-3));
        assert!(matches!(c, Continuity::G0), "got {:?}", c);
    }

    // ---- EdgeStore ---------------------------------------------------------

    #[test]
    fn edge_store_new_is_empty() {
        let s = EdgeStore::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn edge_store_add_assigns_sequential_ids() {
        let mut s = EdgeStore::new();
        let a = s.add(Edge::new(0, 1, 2, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        let b = s.add(Edge::new(0, 2, 3, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(s.len(), 2);
        assert_eq!(s.stats.total_created, 2);
    }

    #[test]
    fn edge_store_get_returns_added_edge() {
        let mut s = EdgeStore::new();
        let id = s.add(Edge::new(0, 5, 7, 11, EdgeOrientation::Backward, ParameterRange::unit()));
        let e = s.get(id).expect("edge");
        assert_eq!(e.start_vertex, 5);
        assert_eq!(e.end_vertex, 7);
        assert_eq!(e.curve_id, 11);
        assert_eq!(e.orientation, EdgeOrientation::Backward);
    }

    #[test]
    fn edge_store_get_returns_none_for_unknown_id() {
        let s = EdgeStore::new();
        assert!(s.get(0).is_none());
    }

    #[test]
    fn edge_store_get_mut_allows_mutation() {
        let mut s = EdgeStore::new();
        let id = s.add(Edge::new(0, 1, 2, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        if let Some(e) = s.get_mut(id) {
            e.tolerance = 5e-9;
        }
        assert!((s.get(id).expect("edge").tolerance - 5e-9).abs() < 1e-20);
    }

    #[test]
    fn edge_store_with_indexing_populates_vertex_to_edges() {
        let mut s = EdgeStore::new();
        let id = s.add_with_indexing(Edge::new(
            0,
            5,
            7,
            0,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let edges_at_5 = s.edges_at_vertex(5);
        let edges_at_7 = s.edges_at_vertex(7);
        assert!(edges_at_5.contains(&id));
        assert!(edges_at_7.contains(&id));
    }

    #[test]
    fn edge_store_find_edge_between_returns_id_when_indexed() {
        let mut s = EdgeStore::new();
        let id = s.add_with_indexing(Edge::new(
            0,
            5,
            7,
            0,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        assert_eq!(s.find_edge_between(5, 7), Some(id));
        // Symmetric — order independent.
        assert_eq!(s.find_edge_between(7, 5), Some(id));
    }

    #[test]
    fn edge_store_find_edge_between_none_for_unindexed() {
        let mut s = EdgeStore::new();
        // add() bypasses indexing, so cache is empty.
        s.add(Edge::new(0, 5, 7, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        assert_eq!(s.find_edge_between(5, 7), None);
    }

    #[test]
    fn edge_store_add_or_find_dedups_on_same_vertex_pair() {
        let mut s = EdgeStore::new();
        let a = s.add_or_find(Edge::new(0, 5, 7, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        // Reverse vertex order also matches.
        let b = s.add_or_find(Edge::new(0, 7, 5, 0, EdgeOrientation::Backward, ParameterRange::unit()));
        assert_eq!(a, b);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn edge_store_add_or_find_creates_new_for_distinct_pair() {
        let mut s = EdgeStore::new();
        let a = s.add_or_find(Edge::new(0, 5, 7, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        let b = s.add_or_find(Edge::new(0, 5, 9, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        assert_ne!(a, b);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn edge_store_remove_marks_invalid_and_returns_clone() {
        let mut s = EdgeStore::new();
        let id = s.add_with_indexing(Edge::new(
            0,
            5,
            7,
            0,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let removed = s.remove(id).expect("removed edge");
        assert_eq!(removed.start_vertex, 5);
        // After removal, `get` must return None — the lookup API is the
        // public-facing contract for "is this edge live". Internally the
        // slot is overwritten with an INVALID_EDGE_ID sentinel (IDs are
        // stable, not reused), but `get` filters those sentinels so
        // callers see consistent semantics with `iter` (which also
        // skips them). See Task #89 for the bug this consistency fixes.
        assert!(
            s.get(id).is_none(),
            "get(removed_id) must return None after remove"
        );
        // The underlying slot still exists with an INVALID_EDGE_ID
        // sentinel — that's an internal detail, exposed only via the
        // `total_deleted` counter, not via the lookup API.
        // Vertex index is cleared.
        assert!(!s.edges_at_vertex(5).contains(&id));
        // find_edge_between cache cleared.
        assert!(s.find_edge_between(5, 7).is_none());
        assert_eq!(s.stats.total_deleted, 1);
    }

    #[test]
    fn edge_store_remove_returns_none_for_unknown_id() {
        let mut s = EdgeStore::new();
        assert!(s.remove(99).is_none());
    }

    #[test]
    fn edge_store_iter_skips_removed_edges() {
        let mut s = EdgeStore::new();
        let a = s.add_with_indexing(Edge::new(0, 1, 2, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        let b = s.add_with_indexing(Edge::new(0, 2, 3, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        let _c = s.add_with_indexing(Edge::new(0, 3, 4, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        s.remove(b);
        let live: Vec<EdgeId> = s.iter().map(|(_, e)| e.id).collect();
        assert!(live.contains(&a));
        assert!(!live.contains(&b));
        assert_eq!(live.len(), 2);
    }

    #[test]
    fn edge_store_edges_on_curve_indexed() {
        let mut s = EdgeStore::new();
        let a = s.add_with_indexing(Edge::new(0, 1, 2, 7, EdgeOrientation::Forward, ParameterRange::unit()));
        let b = s.add_with_indexing(Edge::new(0, 2, 3, 7, EdgeOrientation::Forward, ParameterRange::unit()));
        let edges = s.edges_on_curve(7);
        assert!(edges.contains(&a));
        assert!(edges.contains(&b));
    }

    #[test]
    fn edge_store_edges_on_curve_empty_for_unknown() {
        let s = EdgeStore::new();
        assert!(s.edges_on_curve(99).is_empty());
    }

    #[test]
    fn edge_store_set_tolerance_persists() {
        let mut s = EdgeStore::new();
        let id = s.add(Edge::new(0, 1, 2, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        assert!(s.set_tolerance(id, 1e-10));
        assert!((s.get(id).expect("edge").tolerance - 1e-10).abs() < 1e-22);
    }

    #[test]
    fn edge_store_set_tolerance_returns_false_for_unknown() {
        let mut s = EdgeStore::new();
        assert!(!s.set_tolerance(99, 1e-10));
    }

    #[test]
    fn edge_store_default_constructs_empty_store() {
        let s = EdgeStore::default();
        assert!(s.is_empty());
    }

    #[test]
    fn edge_store_edges_at_vertex_falls_back_to_linear_scan() {
        let mut s = EdgeStore::new();
        // Use add() which skips indexing.
        let a = s.add(Edge::new(0, 5, 7, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        let _b = s.add(Edge::new(0, 8, 9, 0, EdgeOrientation::Forward, ParameterRange::unit()));
        let hits = s.edges_at_vertex(5);
        assert_eq!(hits, vec![a]);
    }
}
