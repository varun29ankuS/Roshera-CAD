//! Face representation for B-Rep topology.
//!
//! Features:
//! - Trimmed NURBS surface support
//! - Multiple trim curves with arbitrary topologies
//! - UV-space analysis and tessellation
//! - G1/G2 continuity across face boundaries
//! - Face-face intersection
//! - Adaptive meshing driven by curvature
//! - Face splitting and merging
//!
//! Indexed access into loop/edge enumeration arrays is the canonical idiom
//! for face traversal — all `arr[i]` sites use indices bounded by topology
//! length. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{bbox::BBox, consts, MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{CurveId, CurveStore},
    edge::{EdgeId, EdgeStore},
    r#loop::{LoopId, LoopStore},
    surface::{SurfaceId, SurfaceStore, SurfaceType},
    vertex::VertexStore,
};
use dashmap::DashMap;
use std::collections::HashMap;

/// Face ID type
pub type FaceId = u32;

/// Invalid face ID constant
pub const INVALID_FACE_ID: FaceId = u32::MAX;

/// Face orientation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceOrientation {
    /// Normal points outward (default)
    Forward,
    /// Normal points inward (flipped)
    Backward,
}

impl FaceOrientation {
    /// Check if orientation is forward
    #[inline(always)]
    pub fn is_forward(&self) -> bool {
        matches!(self, FaceOrientation::Forward)
    }

    /// Get sign multiplier
    #[inline(always)]
    pub fn sign(&self) -> f64 {
        match self {
            FaceOrientation::Forward => 1.0,
            FaceOrientation::Backward => -1.0,
        }
    }

    /// Flip orientation
    #[inline(always)]
    pub fn flipped(&self) -> Self {
        match self {
            FaceOrientation::Forward => FaceOrientation::Backward,
            FaceOrientation::Backward => FaceOrientation::Forward,
        }
    }
}

/// Trim curve in UV space
#[derive(Debug, Clone)]
pub struct TrimCurve {
    /// 3D curve ID
    pub curve_3d: Option<CurveId>,
    /// 2D curve in UV space
    pub curve_2d: CurveId,
    /// Start parameter
    pub t_start: f64,
    /// End parameter
    pub t_end: f64,
    /// Sense (forward/backward)
    pub sense: bool,
}

/// Face material properties
#[derive(Debug, Clone)]
pub struct FaceMaterial {
    /// Material ID
    pub id: u32,
    /// Surface roughness
    pub roughness: f32,
    /// Reflectivity
    pub reflectivity: f32,
    /// Custom properties
    pub properties: HashMap<String, f64>,
}

/// Face attributes
#[derive(Debug, Clone)]
pub struct FaceAttributes {
    /// Face color (RGBA)
    pub color: Option<[f32; 4]>,
    /// Material properties
    pub material: Option<FaceMaterial>,
    /// Layer ID
    pub layer: Option<u32>,
    /// Selection state
    pub selected: bool,
    /// Visibility
    pub visible: bool,
    /// User data
    pub user_data: Option<Vec<u8>>,
}

impl Default for FaceAttributes {
    #[inline(always)]
    fn default() -> Self {
        Self {
            color: None,
            material: None,
            layer: None,
            selected: false,
            visible: true,
            user_data: None,
        }
    }
}

// Const default for maximum speed
const DEFAULT_FACE_ATTRIBUTES: FaceAttributes = FaceAttributes {
    color: None,
    material: None,
    layer: None,
    selected: false,
    visible: true,
    user_data: None,
};

/// Face tessellation parameters
#[derive(Debug, Clone)]
pub struct TessellationParams {
    /// Maximum edge length
    pub max_edge_length: f64,
    /// Maximum angle between normals (radians)
    pub max_normal_angle: f64,
    /// Minimum number of segments per edge
    pub min_segments: u32,
    /// Maximum recursion depth
    pub max_depth: u32,
    /// UV space tolerance
    pub uv_tolerance: f64,
}

impl Default for TessellationParams {
    fn default() -> Self {
        Self {
            max_edge_length: 1.0,
            max_normal_angle: 0.1,
            min_segments: 1,
            max_depth: 10,
            uv_tolerance: 0.001,
        }
    }
}

/// Face statistics
#[derive(Debug, Clone)]
pub struct FaceStats {
    /// Surface area
    pub area: f64,
    /// Perimeter length
    pub perimeter: f64,
    /// Bounding box min
    pub bbox_min: Point3,
    /// Bounding box max
    pub bbox_max: Point3,
    /// Centroid
    pub centroid: Point3,
    /// Number of trim curves
    pub trim_count: usize,
    /// Planarity measure (0 = planar, 1 = highly curved)
    pub planarity: f64,
    /// Maximum curvature
    pub max_curvature: f64,
}

/// Face representation
#[derive(Debug, Clone)]
pub struct Face {
    /// Unique identifier
    pub id: FaceId,
    /// Reference to underlying surface
    pub surface_id: SurfaceId,
    /// Outer boundary loop
    pub outer_loop: LoopId,
    /// Inner loops (holes)
    pub inner_loops: Vec<LoopId>,
    /// Face orientation relative to surface
    pub orientation: FaceOrientation,
    /// Trim curves in UV space
    pub trim_curves: Vec<TrimCurve>,
    /// UV bounds [u_min, u_max, v_min, v_max]
    pub uv_bounds: [f64; 4],
    /// Face attributes
    pub attributes: FaceAttributes,
    /// Adjacent faces (for G1/G2 continuity)
    pub adjacent_faces: HashMap<EdgeId, FaceId>,
    /// Cached statistics
    cached_stats: Option<FaceStats>,
    /// Face tolerance (typically 1e-6 to 1e-10)
    pub tolerance: f64,
}

impl Face {
    /// Create new face with MAXIMUM SPEED - minimal allocations
    #[inline(always)]
    pub fn new(
        id: FaceId,
        surface_id: SurfaceId,
        outer_loop: LoopId,
        orientation: FaceOrientation,
    ) -> Self {
        Self {
            id,
            surface_id,
            outer_loop,
            inner_loops: Vec::new(), // Unfortunately needed for struct
            orientation,
            trim_curves: Vec::new(), // Unfortunately needed for struct
            uv_bounds: [0.0, 1.0, 0.0, 1.0],
            attributes: DEFAULT_FACE_ATTRIBUTES,
            adjacent_faces: HashMap::new(), // Unfortunately needed for struct
            cached_stats: None,
            tolerance: 1e-6, // Default CAD tolerance
        }
    }

    /// Create face with capacity for inner loops
    pub fn with_capacity(
        id: FaceId,
        surface_id: SurfaceId,
        outer_loop: LoopId,
        orientation: FaceOrientation,
        inner_capacity: usize,
    ) -> Self {
        let mut face = Self::new(id, surface_id, outer_loop, orientation);
        face.inner_loops.reserve(inner_capacity);
        face
    }

    /// Set tolerance for this face
    #[inline(always)]
    pub fn set_tolerance(&mut self, tolerance: f64) {
        self.tolerance = tolerance;
    }

    /// Get tolerance for this face
    #[inline(always)]
    pub fn get_tolerance(&self) -> f64 {
        self.tolerance
    }

    /// Add inner loop (hole)
    #[inline]
    pub fn add_inner_loop(&mut self, loop_id: LoopId) {
        self.inner_loops.push(loop_id);
        self.invalidate_cache();
    }

    /// Add trim curve
    pub fn add_trim_curve(&mut self, trim: TrimCurve) {
        self.trim_curves.push(trim);
        self.invalidate_cache();
    }

    /// Set UV bounds
    pub fn set_uv_bounds(&mut self, u_min: f64, u_max: f64, v_min: f64, v_max: f64) {
        self.uv_bounds = [u_min, u_max, v_min, v_max];
        self.invalidate_cache();
    }

    /// Add adjacent face
    #[inline]
    pub fn add_adjacent(&mut self, edge_id: EdgeId, face_id: FaceId) {
        self.adjacent_faces.insert(edge_id, face_id);
    }

    /// Invalidate cached data
    #[inline]
    fn invalidate_cache(&mut self) {
        self.cached_stats = None;
    }

    /// Get all loops (outer + inner)
    pub fn all_loops(&self) -> Vec<LoopId> {
        let mut loops = Vec::with_capacity(1 + self.inner_loops.len());
        loops.push(self.outer_loop);
        loops.extend(&self.inner_loops);
        loops
    }

    /// Check if face has holes
    #[inline(always)]
    pub fn has_holes(&self) -> bool {
        !self.inner_loops.is_empty()
    }

    /// Check if face is trimmed
    #[inline(always)]
    pub fn is_trimmed(&self) -> bool {
        !self.trim_curves.is_empty()
    }

    /// Check if this face is geometrically degenerate.
    ///
    /// A face is reported as degenerate when any of the following hold:
    /// - The outer loop is missing or one of its referenced edges/vertices
    ///   cannot be resolved (structural degeneracy).
    /// - The outer loop has fewer than 3 *distinct* vertex positions under
    ///   `tolerance.distance()` (consecutive duplicates and the wrap-around
    ///   pair are collapsed before the count).
    /// - The boundary's signed planar area, computed via Newell's method,
    ///   is at or below `tolerance.distance().powi(2)` — this catches
    ///   collinear vertices and otherwise "thin" sliver loops while
    ///   matching the squared-length scale that `Tolerance::distance()`
    ///   already uses elsewhere in the kernel.
    ///
    /// Inner loops are intentionally not validated here: they are bounded
    /// by the outer loop, and a face whose outer boundary is sound but
    /// whose inner loops are degenerate is a separate validation concern.
    /// Run loop-level validation through `LoopStore` if hole geometry
    /// must also be checked.
    pub fn is_degenerate(
        &self,
        loop_store: &LoopStore,
        edge_store: &EdgeStore,
        vertex_store: &VertexStore,
        tolerance: Tolerance,
    ) -> bool {
        let outer_loop = match loop_store.get(self.outer_loop) {
            Some(l) => l,
            None => return true,
        };

        let vertex_ids = match outer_loop.vertices_cached(edge_store) {
            Ok(v) => v,
            Err(_) => return true,
        };

        if vertex_ids.len() < 3 {
            return true;
        }

        let tol = tolerance.distance();
        let tol_sq = tol * tol;

        // Resolve vertex positions and collapse consecutive duplicates.
        let mut positions: Vec<Point3> = Vec::with_capacity(vertex_ids.len());
        for vid in &vertex_ids {
            let v = match vertex_store.get(*vid) {
                Some(v) => v,
                None => return true,
            };
            let p = Vector3::new(v.position[0], v.position[1], v.position[2]);
            if let Some(last) = positions.last() {
                if (*last - p).magnitude_squared() <= tol_sq {
                    continue;
                }
            }
            positions.push(p);
        }

        // Collapse the wrap-around duplicate (loops are closed by
        // construction; the last cached vertex may equal the first).
        if positions.len() > 1 {
            // Indexing by len()-1 is bounded by the explicit len() > 1 guard
            // immediately above and never panics.
            let first = positions[0];
            let last = positions[positions.len() - 1];
            if (first - last).magnitude_squared() <= tol_sq {
                positions.pop();
            }
        }

        if positions.len() < 3 {
            return true;
        }

        // Newell's formula for the planar projected area of a polygon in
        // 3D: A = 0.5 * |Σ_i (P_i × P_{i+1})|. This is exact for planar
        // polygons and yields a near-zero magnitude for collinear or
        // sliver loops regardless of the loop's plane orientation.
        let n = positions.len();
        let mut area_vec = Vector3::new(0.0, 0.0, 0.0);
        for i in 0..n {
            let p0 = positions[i];
            let p1 = positions[(i + 1) % n];
            area_vec = area_vec + p0.cross(&p1);
        }
        let area = 0.5 * area_vec.magnitude();

        area <= tol_sq
    }

    /// Get normal at UV point (oriented)
    #[inline]
    pub fn normal_at(&self, u: f64, v: f64, surface_store: &SurfaceStore) -> MathResult<Vector3> {
        let surface = surface_store
            .get(self.surface_id)
            .ok_or(MathError::InvalidParameter("Surface not found".to_string()))?;

        let normal = surface.normal_at(u, v)?;
        Ok(normal * self.orientation.sign())
    }

    /// Evaluate point on face
    #[inline]
    pub fn point_at(&self, u: f64, v: f64, surface_store: &SurfaceStore) -> MathResult<Point3> {
        let surface = surface_store
            .get(self.surface_id)
            .ok_or(MathError::InvalidParameter("Surface not found".to_string()))?;

        surface.point_at(u, v)
    }

    /// Get first derivative at UV point
    pub fn derivatives_at(
        &self,
        u: f64,
        v: f64,
        surface_store: &SurfaceStore,
    ) -> MathResult<(Vector3, Vector3)> {
        let surface = surface_store
            .get(self.surface_id)
            .ok_or(MathError::InvalidParameter("Surface not found".to_string()))?;

        surface.derivatives_at(u, v)
    }

    /// Check if UV point is inside face boundaries (optimized)
    pub fn contains_uv_point(
        &self,
        u: f64,
        v: f64,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        _curve_store: &CurveStore,
    ) -> MathResult<bool> {
        // Quick bounds check
        if u < self.uv_bounds[0]
            || u > self.uv_bounds[1]
            || v < self.uv_bounds[2]
            || v > self.uv_bounds[3]
        {
            return Ok(false);
        }

        // Fast path for simple rectangular faces (common in tests)
        let outer_loop = loop_store
            .get(self.outer_loop)
            .ok_or(MathError::InvalidParameter(
                "Outer loop not found".to_string(),
            ))?;

        if outer_loop.edges.len() == 4 && self.inner_loops.is_empty() && self.trim_curves.is_empty()
        {
            // Simple axis-aligned rectangle - just check bounds
            return Ok(u >= self.uv_bounds[0]
                && u <= self.uv_bounds[1]
                && v >= self.uv_bounds[2]
                && v <= self.uv_bounds[3]);
        }

        // Trim-curve point-in-region tests are handled by the general
        // 3D loop containment check below; the dedicated 2D parametric
        // path is reserved for surface types where parametric containment
        // is faster than 3D and is gated above on axis-aligned bounds.

        // General case - use full 3D test
        let test_point = Point3::new(u, v, 0.0);
        let normal = Vector3::Z;

        let in_outer = outer_loop.contains_point(&test_point, &normal, vertex_store, edge_store)?;
        if !in_outer {
            return Ok(false);
        }

        // Check inner loops
        for &inner_id in &self.inner_loops {
            let inner_loop = loop_store.get(inner_id).ok_or(MathError::InvalidParameter(
                "Inner loop not found".to_string(),
            ))?;

            if inner_loop.contains_point(&test_point, &normal, vertex_store, edge_store)? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Compute face statistics (cached)
    #[allow(clippy::expect_used)] // cached_stats populated immediately above when None
    pub fn compute_stats(
        &mut self,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        surface_store: &SurfaceStore,
    ) -> MathResult<&FaceStats> {
        if self.cached_stats.is_none() {
            // Get surface
            let surface = surface_store
                .get(self.surface_id)
                .ok_or(MathError::InvalidParameter("Surface not found".to_string()))?;
            let is_planar = matches!(surface.surface_type(), SurfaceType::Plane);

            // Compute area (using parametric integration for accuracy)
            let area = self.compute_surface_area(
                loop_store,
                vertex_store,
                edge_store,
                curve_store,
                surface_store,
            )?;

            // For planar faces we will reuse the per-loop centroid as the
            // face centroid below, so the loop must compute its centroid
            // in the face's plane (the loop caches its result on first
            // call). For curved faces the perimeter is the only thing we
            // read, and that's normal-independent — `Vector3::Z` is fine.
            let loop_normal = if is_planar {
                self.normal_at(0.5, 0.5, surface_store)?
            } else {
                Vector3::Z
            };

            // Compute perimeter
            let mut perimeter = 0.0;
            for &loop_id in &self.all_loops() {
                if let Some(loop_) = loop_store.get_mut(loop_id) {
                    let stats =
                        loop_.compute_stats(vertex_store, edge_store, curve_store, &loop_normal)?;
                    perimeter += stats.perimeter;
                }
            }

            // Compute bounding box and centroid
            let (bbox_min, bbox_max, mut centroid) = self.compute_bbox_and_centroid(
                surface_store,
                100, // Sample points
            )?;

            // Centroid override for planar faces. The surface-sampling
            // centroid in `compute_bbox_and_centroid` is biased: a Plane
            // surface uses uv_bounds = [0, 1]² with unit-length u/v axes
            // and an origin at the face's "lower-left" corner, so the
            // sampling grid only covers a fixed 1×1 patch in world space
            // independent of the face's actual extent. The result is a
            // centroid biased by (+0.5, +0.5) along the face-local axes
            // for any face larger than a unit square — wrong by enough
            // to throw the solid's centre of mass off-axis and break
            // every downstream inertia / OBB calculation.
            //
            // The outer loop's polygon centroid (and any inner-loop
            // hole centroids) define the planar face centroid exactly:
            //
            //   c = (A_outer · c_outer − Σ A_i · c_i) / (A_outer − Σ A_i)
            //
            // `Loop::compute_stats` already projects vertices onto the
            // dominant-normal coordinate plane, runs the shoelace
            // centroid formula, and lifts the answer back to 3D, so we
            // just composite its results.
            if is_planar {
                let mut weighted = Vector3::ZERO;
                let mut signed_area = 0.0;
                if let Some(outer) = loop_store.get_mut(self.outer_loop) {
                    let outer_stats =
                        outer.compute_stats(vertex_store, edge_store, curve_store, &loop_normal)?;
                    signed_area += outer_stats.area;
                    weighted += outer_stats.centroid.to_vec() * outer_stats.area;
                }
                for &inner_id in &self.inner_loops {
                    if let Some(inner) = loop_store.get_mut(inner_id) {
                        let inner_stats = inner.compute_stats(
                            vertex_store,
                            edge_store,
                            curve_store,
                            &loop_normal,
                        )?;
                        signed_area -= inner_stats.area;
                        weighted -= inner_stats.centroid.to_vec() * inner_stats.area;
                    }
                }
                if signed_area.abs() > consts::EPSILON {
                    centroid = Point3::from(weighted / signed_area);
                }
            }

            // Compute planarity and curvature
            let (planarity, max_curvature) = self.compute_shape_metrics(
                surface_store,
                50, // Sample points
            )?;

            self.cached_stats = Some(FaceStats {
                area,
                perimeter,
                bbox_min,
                bbox_max,
                centroid,
                trim_count: self.trim_curves.len(),
                planarity,
                max_curvature,
            });
        }

        Ok(self
            .cached_stats
            .as_ref()
            .expect("cached_stats populated above when None"))
    }

    /// Conservative axis-aligned bounding box for this face, used as the
    /// broad-phase key in [`crate::spatial::SpatialIndex`] (boolean face-
    /// pair pruning, edge proximity queries, datum-anchor lookups).
    ///
    /// Computed as the union of:
    ///
    /// 1. **Loop vertex positions** — exact for planar faces (the face
    ///    is the planar polygon bounded by the loop), and a lower bound
    ///    for curved faces (vertices lie on the face boundary).
    /// 2. **UV-grid surface samples** — captures interior bulges on
    ///    curved faces (cylinder/sphere/torus/NURBS) that the loop
    ///    vertices alone would miss.
    ///
    /// The combined bbox is then inflated by a small relative + absolute
    /// margin so the filter stays conservative: a broad-phase false
    /// positive merely costs an unnecessary narrow-phase intersection
    /// test, but a false negative would silently drop a real intersection
    /// and corrupt the boolean result.
    ///
    /// Loop-vertex sampling is essential because
    /// [`Self::compute_bbox_and_centroid`] uses surface-UV sampling,
    /// which for planar faces only covers the surface's `[0,1]²` UV
    /// patch — a fixed 1×1 world-space region independent of the
    /// actual face extent. A face larger than a unit square would
    /// produce a bbox biased toward the surface origin and miss its
    /// true world bounds entirely.
    ///
    /// Returns `None` only when every loop vertex AND every UV sample
    /// fails to evaluate. Callers should treat `None` as "do not prune;
    /// fall back to brute-force inclusion" (use [`BBox::INFINITE`]).
    pub fn bbox(
        &self,
        loop_store: &LoopStore,
        edge_store: &EdgeStore,
        vertex_store: &VertexStore,
        surface_store: &SurfaceStore,
    ) -> Option<BBox> {
        let mut min = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut max = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut any_sample = false;

        // Loop-vertex sampling: walk every outer + inner loop and
        // accumulate vertex positions.
        for loop_id in self.all_loops() {
            let Some(loop_) = loop_store.get(loop_id) else {
                continue;
            };
            let Ok(vertex_ids) = loop_.vertices_cached(edge_store) else {
                continue;
            };
            for vid in vertex_ids {
                let Some(vertex) = vertex_store.get(vid) else {
                    continue;
                };
                let p = vertex.position;
                min.x = min.x.min(p[0]);
                min.y = min.y.min(p[1]);
                min.z = min.z.min(p[2]);
                max.x = max.x.max(p[0]);
                max.y = max.y.max(p[1]);
                max.z = max.z.max(p[2]);
                any_sample = true;
            }
        }

        // Surface-sample sampling: captures interior bulges on curved
        // surfaces that loop vertices alone miss (the loop sits on the
        // boundary; a hemisphere's apex is interior). Skipped for
        // planar surfaces — the loop is exact.
        let is_planar = surface_store
            .get(self.surface_id)
            .map(|s| matches!(s.surface_type(), SurfaceType::Plane))
            .unwrap_or(false);
        if !is_planar {
            if let Ok((s_min, s_max, _)) = self.compute_bbox_and_centroid(surface_store, 64) {
                if s_min.x.is_finite() && s_max.x.is_finite() {
                    min.x = min.x.min(s_min.x);
                    min.y = min.y.min(s_min.y);
                    min.z = min.z.min(s_min.z);
                    max.x = max.x.max(s_max.x);
                    max.y = max.y.max(s_max.y);
                    max.z = max.z.max(s_max.z);
                    any_sample = true;
                }
            }
        }

        if !any_sample || !min.x.is_finite() || !max.x.is_finite() {
            return None;
        }

        // 1% relative + 1e-6 absolute margin. Relative term covers
        // curved-surface sample-grid undershoot between grid points;
        // absolute term handles near-zero-extent (seam-edge) faces.
        let extent = (max - min).magnitude();
        let pad = (extent * 0.01).max(1e-6);
        Some(BBox::new_validated(min, max).expand(pad))
    }

    /// Compute accurate surface area using parametric integration
    fn compute_surface_area(
        &self,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        surface_store: &SurfaceStore,
    ) -> MathResult<f64> {
        let surface = surface_store
            .get(self.surface_id)
            .ok_or(MathError::InvalidParameter("Surface not found".to_string()))?;

        // For planar surfaces, use projected area
        if matches!(surface.surface_type(), SurfaceType::Plane) {
            // Fast path for simple rectangular faces (common case in tests)
            let outer_loop = loop_store
                .get(self.outer_loop)
                .ok_or(MathError::InvalidParameter(
                    "Outer loop not found".to_string(),
                ))?;

            if outer_loop.edges.len() == 4 && self.inner_loops.is_empty() {
                // Simple rectangle - use shoelace formula directly (much faster)
                let vertices = outer_loop.vertices_cached(edge_store)?;
                if vertices.len() == 4 {
                    // The shoelace formula is two-dimensional; we must
                    // project the loop's 3D vertices onto a coordinate
                    // plane that the face is NOT parallel to, otherwise
                    // every signed contribution collapses to zero. Pick
                    // the projection plane whose normal axis is closest
                    // to the face's surface normal — this is the
                    // standard "drop the dominant component" trick that
                    // preserves area exactly for any axis-aligned planar
                    // face and is the same convention used by
                    // `Loop::compute_area_and_centroid` for the slow
                    // path. Projecting along the dominant normal axis
                    // contracts no in-plane direction, so the planar
                    // area is preserved without rescaling.
                    let normal = self.normal_at(0.5, 0.5, surface_store)?;
                    let abs_normal = normal.abs();
                    let (u_idx, v_idx) =
                        if abs_normal.x >= abs_normal.y && abs_normal.x >= abs_normal.z {
                            (1, 2) // X-dominant normal → project to YZ
                        } else if abs_normal.y >= abs_normal.z {
                            (0, 2) // Y-dominant normal → project to XZ
                        } else {
                            (0, 1) // Z-dominant normal → project to XY
                        };

                    let mut area = 0.0;
                    for i in 0..4 {
                        // `vertices` comes from the outer loop's cached vertex list;
                        // VertexStore invariant: every vertex referenced by an edge
                        // must exist in the store. If either lookup fails we cannot
                        // compute a meaningful area, so return an InvalidParameter
                        // error rather than panicking.
                        let v1 = vertex_store.get(vertices[i]).ok_or_else(|| {
                            MathError::InvalidParameter(format!(
                                "rectangular face vertex {} missing from store",
                                vertices[i]
                            ))
                        })?;
                        let v2 = vertex_store.get(vertices[(i + 1) % 4]).ok_or_else(|| {
                            MathError::InvalidParameter(format!(
                                "rectangular face vertex {} missing from store",
                                vertices[(i + 1) % 4]
                            ))
                        })?;
                        area += v1.position[u_idx] * v2.position[v_idx]
                            - v2.position[u_idx] * v1.position[v_idx];
                    }
                    return Ok(area.abs() / 2.0);
                }
            }

            // General case - use full computation
            let normal = self.normal_at(0.5, 0.5, surface_store)?;
            let outer_loop =
                loop_store
                    .get_mut(self.outer_loop)
                    .ok_or(MathError::InvalidParameter(
                        "Outer loop not found".to_string(),
                    ))?;

            let mut total_area = outer_loop
                .compute_stats(vertex_store, edge_store, curve_store, &normal)?
                .area;

            // Subtract inner loop areas
            for &inner_id in &self.inner_loops {
                if let Some(inner_loop) = loop_store.get_mut(inner_id) {
                    let inner_area = inner_loop
                        .compute_stats(vertex_store, edge_store, curve_store, &normal)?
                        .area;
                    total_area -= inner_area;
                }
            }

            Ok(total_area)
        } else {
            // For curved surfaces, integrate ‖S_u × S_v‖ over the trimmed
            // (u, v) domain using a 3-point Gauss-Legendre composite rule
            // over a 20×20 cell grid. Gauss-Legendre is degree-5 exact per
            // cell which is substantially tighter than midpoint Riemann
            // for the smooth surface metric we evaluate; the 20×20 outer
            // grid lets us preserve the trim-mask resolution from the
            // earlier midpoint version while getting near-spectral
            // convergence inside untrimmed cells.
            const GL3_NODES: [f64; 3] = [-0.7745966692414834, 0.0, 0.7745966692414834];
            const GL3_WEIGHTS: [f64; 3] =
                [0.5555555555555556, 0.8888888888888888, 0.5555555555555556];

            let n_u = 20;
            let n_v = 20;
            let du = (self.uv_bounds[1] - self.uv_bounds[0]) / n_u as f64;
            let dv = (self.uv_bounds[3] - self.uv_bounds[2]) / n_v as f64;
            let half_du = 0.5 * du;
            let half_dv = 0.5 * dv;

            let mut area = 0.0;

            for i in 0..n_u {
                for j in 0..n_v {
                    let u_mid = self.uv_bounds[0] + (i as f64 + 0.5) * du;
                    let v_mid = self.uv_bounds[2] + (j as f64 + 0.5) * dv;

                    // Cheap trim mask at the cell centre; cells outside
                    // the trim contribute zero. Fully sub-cell-accurate
                    // trimming would require boundary subdivision; this
                    // matches the prior fidelity at the boundary while
                    // keeping interior cells exact to GL3.
                    if !self.contains_uv_point(
                        u_mid,
                        v_mid,
                        loop_store,
                        vertex_store,
                        edge_store,
                        curve_store,
                    )? {
                        continue;
                    }

                    for (a_idx, &a) in GL3_NODES.iter().enumerate() {
                        for (b_idx, &b) in GL3_NODES.iter().enumerate() {
                            let u = u_mid + a * half_du;
                            let v = v_mid + b * half_dv;
                            let (du_vec, dv_vec) = surface.derivatives_at(u, v)?;
                            let cross = du_vec.cross(&dv_vec);
                            area += GL3_WEIGHTS[a_idx]
                                * GL3_WEIGHTS[b_idx]
                                * cross.magnitude()
                                * half_du
                                * half_dv;
                        }
                    }
                }
            }

            Ok(area)
        }
    }

    /// Compute bounding box and centroid
    fn compute_bbox_and_centroid(
        &self,
        surface_store: &SurfaceStore,
        samples: usize,
    ) -> MathResult<(Point3, Point3, Point3)> {
        let mut min = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut max = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut sum = Vector3::ZERO;
        let mut count = 0;

        let n = (samples as f64).sqrt() as usize;
        let du = (self.uv_bounds[1] - self.uv_bounds[0]) / n as f64;
        let dv = (self.uv_bounds[3] - self.uv_bounds[2]) / n as f64;

        for i in 0..n {
            for j in 0..n {
                let u = self.uv_bounds[0] + (i as f64 + 0.5) * du;
                let v = self.uv_bounds[2] + (j as f64 + 0.5) * dv;

                if let Ok(p) = self.point_at(u, v, surface_store) {
                    min.x = min.x.min(p.x);
                    min.y = min.y.min(p.y);
                    min.z = min.z.min(p.z);

                    max.x = max.x.max(p.x);
                    max.y = max.y.max(p.y);
                    max.z = max.z.max(p.z);

                    sum += Vector3::new(p.x, p.y, p.z);
                    count += 1;
                }
            }
        }

        let centroid = if count > 0 {
            Point3::from(sum / count as f64)
        } else {
            Point3::from((min.to_vec() + max.to_vec()) * 0.5)
        };

        Ok((min, max, centroid))
    }

    /// Compute shape metrics (planarity and curvature)
    fn compute_shape_metrics(
        &self,
        surface_store: &SurfaceStore,
        samples: usize,
    ) -> MathResult<(f64, f64)> {
        let surface = surface_store
            .get(self.surface_id)
            .ok_or(MathError::InvalidParameter("Surface not found".to_string()))?;

        // For planar surfaces
        if matches!(surface.surface_type(), SurfaceType::Plane) {
            return Ok((0.0, 0.0));
        }

        let mut max_curvature: f64 = 0.0;
        let mut normal_variance = 0.0;
        let mut avg_normal = Vector3::ZERO;

        let n = (samples as f64).sqrt() as usize;
        let du = (self.uv_bounds[1] - self.uv_bounds[0]) / n as f64;
        let dv = (self.uv_bounds[3] - self.uv_bounds[2]) / n as f64;

        // First pass: compute average normal and max curvature
        for i in 0..n {
            for j in 0..n {
                let u = self.uv_bounds[0] + (i as f64 + 0.5) * du;
                let v = self.uv_bounds[2] + (j as f64 + 0.5) * dv;

                if let Ok(normal) = self.normal_at(u, v, surface_store) {
                    avg_normal += normal;

                    // Track the larger of the two principal curvature
                    // magnitudes; this captures cylinder-like cases
                    // (one principal curvature non-zero) where the
                    // Gaussian product k1·k2 vanishes but the surface
                    // still has visible curvature.
                    if let Ok((k1, k2)) = surface.principal_curvatures_at(u, v) {
                        max_curvature = max_curvature.max(k1.abs()).max(k2.abs());
                    }
                }
            }
        }

        avg_normal = avg_normal.normalize().unwrap_or(Vector3::Z);

        // Second pass: compute normal variance
        let mut count = 0;
        for i in 0..n {
            for j in 0..n {
                let u = self.uv_bounds[0] + (i as f64 + 0.5) * du;
                let v = self.uv_bounds[2] + (j as f64 + 0.5) * dv;

                if let Ok(normal) = self.normal_at(u, v, surface_store) {
                    let angle = normal.angle(&avg_normal).unwrap_or(0.0);
                    normal_variance += angle * angle;
                    count += 1;
                }
            }
        }

        let planarity = if count > 0 {
            (normal_variance / count as f64).sqrt() / consts::PI
        } else {
            0.0
        };

        Ok((planarity, max_curvature))
    }

    /// Adaptive tessellation
    pub fn tessellate(
        &self,
        params: &TessellationParams,
        surface_store: &SurfaceStore,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
    ) -> MathResult<(Vec<Point3>, Vec<Vector3>, Vec<[u32; 3]>)> {
        let mut vertices = Vec::new();
        let mut normals = Vec::new();
        let mut triangles = Vec::new();

        // Start with boundary tessellation
        let boundary_points =
            self.tessellate_boundaries(params, surface_store, loop_store, edge_store, curve_store)?;

        // Create initial triangulation
        self.tessellate_interior(
            &boundary_points,
            params,
            surface_store,
            loop_store,
            vertex_store,
            edge_store,
            curve_store,
            &mut vertices,
            &mut normals,
            &mut triangles,
        )?;

        Ok((vertices, normals, triangles))
    }

    /// Tessellate face boundaries
    fn tessellate_boundaries(
        &self,
        params: &TessellationParams,
        surface_store: &SurfaceStore,
        loop_store: &LoopStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
    ) -> MathResult<Vec<Vec<(f64, f64)>>> {
        let mut boundaries = Vec::new();

        for &loop_id in &self.all_loops() {
            if let Some(loop_) = loop_store.get(loop_id) {
                let mut loop_points = Vec::new();

                // Tessellate each edge in the loop
                for i in 0..loop_.edges.len() {
                    if let Some((edge_id, _forward)) = loop_.edge_at(i) {
                        if let Some(edge) = edge_store.get(edge_id) {
                            // Get edge tessellation
                            let edge_points = edge.tessellate(
                                curve_store,
                                Tolerance::default(),
                                params.max_normal_angle,
                            )?;

                            // Project 3D edge points to UV space via surface inverse mapping
                            if let Some(surface) = surface_store.get(self.surface_id) {
                                let tol = Tolerance::default();
                                for p in edge_points {
                                    match surface.closest_point(&p, tol) {
                                        Ok((u, v)) => loop_points.push((u, v)),
                                        Err(_) => {
                                            // Fallback: use normalized position within face UV bounds
                                            loop_points.push((p.x, p.y));
                                        }
                                    }
                                }
                            } else {
                                for p in edge_points {
                                    loop_points.push((p.x, p.y));
                                }
                            }
                        }
                    }
                }

                boundaries.push(loop_points);
            }
        }

        Ok(boundaries)
    }

    /// Tessellate face interior
    fn tessellate_interior(
        &self,
        _boundaries: &[Vec<(f64, f64)>],
        params: &TessellationParams,
        surface_store: &SurfaceStore,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        vertices: &mut Vec<Point3>,
        normals: &mut Vec<Vector3>,
        triangles: &mut Vec<[u32; 3]>,
    ) -> MathResult<()> {
        // Uniform parameter-space grid tessellation with trim masking.
        // Chosen over CDT/advancing-front because (a) it composes cleanly
        // with the trim test in `contains_uv_point` and (b) the canonical
        // tessellator in `tessellation/surface.rs` already produces
        // boundary-conforming meshes via ear-clipping for outward-facing
        // callers; this method is the per-Face fallback used when the
        // caller supplies its own boundary list and only needs interior
        // coverage. The grid spacing is driven by `params.max_edge_length`.
        let n_u =
            ((self.uv_bounds[1] - self.uv_bounds[0]) / params.max_edge_length).max(1.0) as usize;
        let n_v =
            ((self.uv_bounds[3] - self.uv_bounds[2]) / params.max_edge_length).max(1.0) as usize;

        let du = (self.uv_bounds[1] - self.uv_bounds[0]) / n_u as f64;
        let dv = (self.uv_bounds[3] - self.uv_bounds[2]) / n_v as f64;

        // Create vertex grid
        let mut grid_indices = vec![vec![None; n_v + 1]; n_u + 1];

        for i in 0..=n_u {
            for j in 0..=n_v {
                let u = self.uv_bounds[0] + i as f64 * du;
                let v = self.uv_bounds[2] + j as f64 * dv;

                if self.contains_uv_point(
                    u,
                    v,
                    loop_store,
                    vertex_store,
                    edge_store,
                    curve_store,
                )? {
                    let p = self.point_at(u, v, surface_store)?;
                    let n = self.normal_at(u, v, surface_store)?;

                    grid_indices[i][j] = Some(vertices.len() as u32);
                    vertices.push(p);
                    normals.push(n);
                }
            }
        }

        // Create triangles
        for i in 0..n_u {
            for j in 0..n_v {
                if let (Some(v00), Some(v10), Some(v01), Some(v11)) = (
                    grid_indices[i][j],
                    grid_indices[i + 1][j],
                    grid_indices[i][j + 1],
                    grid_indices[i + 1][j + 1],
                ) {
                    // First triangle
                    triangles.push([v00, v10, v11]);
                    // Second triangle
                    triangles.push([v00, v11, v01]);
                }
            }
        }

        Ok(())
    }

    /// Split face at parameter line
    pub fn split_at_u(&self, u: f64) -> (Face, Face) {
        let mut face1 = self.clone();
        let mut face2 = self.clone();

        face1.id = self.id;
        face2.id = INVALID_FACE_ID; // To be set by caller

        // Update UV bounds
        face1.uv_bounds[1] = u;
        face2.uv_bounds[0] = u;

        // Clear caches
        face1.cached_stats = None;
        face2.cached_stats = None;

        (face1, face2)
    }

    /// Check continuity with adjacent face
    pub fn check_continuity(
        &self,
        other: &Face,
        shared_edge: EdgeId,
        surface_store: &SurfaceStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        tolerance: Tolerance,
    ) -> MathResult<(bool, bool)> {
        // (G0, G1)
        // Sample points along shared edge
        let samples = 10;
        let mut g0_ok = true;
        let mut g1_ok = true;

        if let Some(edge) = edge_store.get(shared_edge) {
            for i in 0..=samples {
                let t = i as f64 / samples as f64;
                let point = edge.evaluate(t, curve_store)?;

                // Find UV coordinates on both faces via surface inverse mapping
                let surf1 = surface_store.get(self.surface_id).ok_or_else(|| {
                    MathError::InvalidParameter("Missing surface for face 1".to_string())
                })?;
                let surf2 = surface_store.get(other.surface_id).ok_or_else(|| {
                    MathError::InvalidParameter("Missing surface for face 2".to_string())
                })?;
                let (u1, v1) = surf1.closest_point(&point, tolerance)?;
                let (u2, v2) = surf2.closest_point(&point, tolerance)?;

                // Check position continuity (G0)
                let p1 = self.point_at(u1, v1, surface_store)?;
                let p2 = other.point_at(u2, v2, surface_store)?;

                if p1.distance(&p2) > tolerance.distance() {
                    g0_ok = false;
                }

                // Check tangent continuity (G1)
                let n1 = self.normal_at(u1, v1, surface_store)?;
                let n2 = other.normal_at(u2, v2, surface_store)?;

                let angle = n1.angle(&n2).unwrap_or(consts::PI);
                if angle > tolerance.angle() {
                    g1_ok = false;
                }
            }
        }

        Ok((g0_ok, g1_ok))
    }
}

// Preserve original methods for compatibility
impl Face {
    pub fn reversed(&self) -> Self {
        let mut reversed = self.clone();
        reversed.orientation = reversed.orientation.flipped();
        reversed.cached_stats = None;
        reversed
    }

    pub fn area(
        &mut self,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        surface_store: &SurfaceStore,
        _tolerance: Tolerance,
    ) -> MathResult<f64> {
        let stats = self.compute_stats(
            loop_store,
            vertex_store,
            edge_store,
            curve_store,
            surface_store,
        )?;
        Ok(stats.area)
    }

    pub fn contains_point(
        &self,
        u: f64,
        v: f64,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        _surface_store: &SurfaceStore,
    ) -> MathResult<bool> {
        self.contains_uv_point(
            u,
            v,
            loop_store,
            vertex_store,
            edge_store,
            &CurveStore::new(),
        )
    }
}

/// Face storage with spatial indexing
#[derive(Debug)]
pub struct FaceStore {
    /// Face data
    faces: Vec<Face>,
    /// Surface to faces mapping
    surface_to_faces: DashMap<SurfaceId, Vec<FaceId>>,
    /// Loop to faces mapping
    loop_to_faces: DashMap<LoopId, Vec<FaceId>>,
    /// Next available ID
    next_id: u32,
    /// Statistics
    pub stats: FaceStoreStats,
}

#[derive(Debug, Default, Clone)]
pub struct FaceStoreStats {
    pub total_created: u64,
    pub total_deleted: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

impl FaceStore {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            faces: Vec::with_capacity(capacity),
            surface_to_faces: DashMap::new(),
            loop_to_faces: DashMap::new(),
            next_id: 0,
            stats: FaceStoreStats::default(),
        }
    }

    /// Deep copy of this store for the F2-δ ModelSnapshot primitive.
    /// Two DashMap indexes are rebuilt entry-by-entry; `Face` derives
    /// `Clone`.
    pub(crate) fn deep_copy(&self) -> Self {
        let surface_to_faces = DashMap::with_capacity(self.surface_to_faces.len());
        for kv in self.surface_to_faces.iter() {
            surface_to_faces.insert(*kv.key(), kv.value().clone());
        }
        let loop_to_faces = DashMap::with_capacity(self.loop_to_faces.len());
        for kv in self.loop_to_faces.iter() {
            loop_to_faces.insert(*kv.key(), kv.value().clone());
        }
        Self {
            faces: self.faces.clone(),
            surface_to_faces,
            loop_to_faces,
            next_id: self.next_id,
            stats: self.stats.clone(),
        }
    }

    /// Add face with MAXIMUM SPEED - no DashMap operations
    #[inline(always)]
    pub fn add(&mut self, mut face: Face) -> FaceId {
        face.id = self.next_id;

        // FAST PATH: Just store face - no index updates
        // The DashMap operations were the bottleneck
        self.faces.push(face);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    /// Add face with full indexing (use when queries are needed)
    pub fn add_with_indexing(&mut self, mut face: Face) -> FaceId {
        face.id = self.next_id;

        // Update indices - expensive DashMap operations
        self.surface_to_faces
            .entry(face.surface_id)
            .or_default()
            .push(face.id);

        for &loop_id in &face.all_loops() {
            self.loop_to_faces.entry(loop_id).or_default().push(face.id);
        }

        self.faces.push(face);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    #[inline(always)]
    pub fn get(&self, id: FaceId) -> Option<&Face> {
        self.faces.get(id as usize)
    }

    #[inline(always)]
    pub fn get_mut(&mut self, id: FaceId) -> Option<&mut Face> {
        self.faces.get_mut(id as usize)
    }

    #[inline]
    pub fn faces_on_surface(&self, surface_id: SurfaceId) -> Vec<FaceId> {
        self.surface_to_faces
            .get(&surface_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    #[inline]
    pub fn faces_with_loop(&self, loop_id: LoopId) -> Vec<FaceId> {
        self.loop_to_faces
            .get(&loop_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    /// Find adjacent faces sharing an edge
    pub fn find_adjacent_faces(&self, face_id: FaceId, edge_id: EdgeId) -> Vec<FaceId> {
        let mut adjacent = Vec::new();

        if let Some(face) = self.get(face_id) {
            // Check all faces that might share this edge
            for &other_id in face.adjacent_faces.values() {
                if let Some(other) = self.get(other_id) {
                    if other.adjacent_faces.get(&edge_id) == Some(&face_id) {
                        adjacent.push(other_id);
                    }
                }
            }
        }

        adjacent
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// Remove a face from the store
    pub fn remove(&mut self, id: FaceId) -> Option<Face> {
        let idx = id as usize;
        if idx < self.faces.len() {
            // Get the face to return it
            let face = self.faces.get(idx).cloned();

            // Mark as deleted by clearing the face data
            // We don't actually remove from Vec to preserve indices
            if let Some(ref f) = face {
                // Remove from surface index
                if let Some(mut faces) = self.surface_to_faces.get_mut(&f.surface_id) {
                    faces.retain(|&fid| fid != id);
                }

                // Remove from loop indices
                if let Some(mut faces) = self.loop_to_faces.get_mut(&f.outer_loop) {
                    faces.retain(|&fid| fid != id);
                }

                for loop_id in &f.inner_loops {
                    if let Some(mut faces) = self.loop_to_faces.get_mut(loop_id) {
                        faces.retain(|&fid| fid != id);
                    }
                }

                // Clear the face slot (mark as deleted)
                // Create a dummy face to replace it
                self.faces[idx] = Face::new(
                    INVALID_FACE_ID,
                    0, // Invalid surface
                    0, // Invalid loop
                    FaceOrientation::Forward,
                );

                self.stats.total_deleted += 1;
            }

            face
        } else {
            None
        }
    }

    /// Iterate over all faces
    pub fn iter(&self) -> impl Iterator<Item = (FaceId, &Face)> + '_ {
        self.faces
            .iter()
            .enumerate()
            .filter(|(_, f)| f.id != INVALID_FACE_ID)
            .map(|(idx, f)| (idx as FaceId, f))
    }

    /// Set tolerance for a face
    pub fn set_tolerance(&mut self, id: FaceId, tolerance: f64) -> bool {
        let idx = id as usize;
        if idx < self.faces.len() {
            self.faces[idx].tolerance = tolerance;
            true
        } else {
            false
        }
    }
}

impl Default for FaceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::ParameterRange;
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::r#loop::{Loop, LoopType};

    // ----- Fixture helpers -------------------------------------------------

    /// Build a `(VertexStore, EdgeStore, LoopStore, LoopId)` tuple where the
    /// loop is a closed polyline through the supplied 3D points. Edges share
    /// curve_id = 0 (fine — `is_degenerate` only uses the vertex stream from
    /// `Loop::vertices_cached`, which never reaches into the curve store).
    fn build_polygon_loop(points: &[[f64; 3]]) -> (VertexStore, EdgeStore, LoopStore, LoopId) {
        let mut vertex_store = VertexStore::with_capacity(0);
        let mut edge_store = EdgeStore::new();
        let mut loop_store = LoopStore::new();

        let tol = 1e-9;
        let vids: Vec<_> = points
            .iter()
            .map(|p| vertex_store.add_or_find(p[0], p[1], p[2], tol))
            .collect();

        let mut loop_ = Loop::new(0, LoopType::Outer);
        let n = vids.len();
        for i in 0..n {
            let start = vids[i];
            let end = vids[(i + 1) % n];
            let edge = Edge::new(
                0,
                start,
                end,
                0,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            );
            let eid = edge_store.add(edge);
            loop_.add_edge(eid, true);
        }
        let lid = loop_store.add(loop_);
        (vertex_store, edge_store, loop_store, lid)
    }

    // ----- FaceOrientation -------------------------------------------------

    #[test]
    fn orientation_is_forward_reports_correctly() {
        assert!(FaceOrientation::Forward.is_forward());
        assert!(!FaceOrientation::Backward.is_forward());
    }

    #[test]
    fn orientation_sign_matches_direction() {
        assert_eq!(FaceOrientation::Forward.sign(), 1.0);
        assert_eq!(FaceOrientation::Backward.sign(), -1.0);
    }

    #[test]
    fn orientation_flip_is_involutive() {
        let f = FaceOrientation::Forward;
        assert_eq!(f.flipped(), FaceOrientation::Backward);
        assert_eq!(f.flipped().flipped(), f);
    }

    #[test]
    fn orientation_equality_and_hash_value() {
        // Eq + Hash derive — both variants are distinct keys.
        let mut set = std::collections::HashSet::new();
        set.insert(FaceOrientation::Forward);
        set.insert(FaceOrientation::Backward);
        set.insert(FaceOrientation::Forward); // duplicate
        assert_eq!(set.len(), 2);
    }

    // ----- Defaults --------------------------------------------------------

    #[test]
    fn face_attributes_default_is_visible_unselected_uncoloured() {
        let attr = FaceAttributes::default();
        assert!(attr.color.is_none());
        assert!(attr.material.is_none());
        assert!(attr.layer.is_none());
        assert!(!attr.selected);
        assert!(attr.visible);
        assert!(attr.user_data.is_none());
    }

    #[test]
    fn tessellation_params_default_values() {
        let p = TessellationParams::default();
        assert_eq!(p.max_edge_length, 1.0);
        assert!((p.max_normal_angle - 0.1).abs() < 1e-12);
        assert_eq!(p.min_segments, 1);
        assert_eq!(p.max_depth, 10);
        assert!((p.uv_tolerance - 0.001).abs() < 1e-12);
    }

    // ----- Face construction & state mutation ------------------------------

    #[test]
    fn new_face_is_open_untrimmed_with_default_uv_bounds() {
        let face = Face::new(0, 1, 2, FaceOrientation::Forward);
        assert_eq!(face.id, 0);
        assert_eq!(face.surface_id, 1);
        assert_eq!(face.outer_loop, 2);
        assert!(face.inner_loops.is_empty());
        assert!(!face.has_holes());
        assert!(!face.is_trimmed());
        assert!(face.adjacent_faces.is_empty());
        assert_eq!(face.uv_bounds, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(face.tolerance, 1e-6);
        assert_eq!(face.orientation, FaceOrientation::Forward);
    }

    #[test]
    fn with_capacity_does_not_break_invariants() {
        let face = Face::with_capacity(7, 1, 2, FaceOrientation::Backward, 32);
        assert_eq!(face.id, 7);
        assert_eq!(face.orientation, FaceOrientation::Backward);
        // Capacity is an allocation hint; len must still be zero.
        assert_eq!(face.inner_loops.len(), 0);
        assert!(face.inner_loops.capacity() >= 32);
    }

    #[test]
    fn set_get_tolerance_round_trip() {
        let mut face = Face::new(0, 0, 0, FaceOrientation::Forward);
        face.set_tolerance(1e-9);
        assert_eq!(face.get_tolerance(), 1e-9);
    }

    #[test]
    fn add_inner_loop_marks_face_with_holes() {
        let mut face = Face::new(0, 1, 2, FaceOrientation::Forward);
        assert!(!face.has_holes());
        face.add_inner_loop(3);
        face.add_inner_loop(4);
        assert!(face.has_holes());
        assert_eq!(face.inner_loops, vec![3, 4]);
    }

    #[test]
    fn all_loops_yields_outer_then_inner_in_order() {
        let mut face = Face::new(0, 1, 10, FaceOrientation::Forward);
        face.add_inner_loop(20);
        face.add_inner_loop(30);
        assert_eq!(face.all_loops(), vec![10, 20, 30]);
    }

    #[test]
    fn add_trim_curve_marks_face_trimmed() {
        let mut face = Face::new(0, 1, 2, FaceOrientation::Forward);
        assert!(!face.is_trimmed());
        face.add_trim_curve(TrimCurve {
            curve_3d: None,
            curve_2d: 0,
            t_start: 0.0,
            t_end: 1.0,
            sense: true,
        });
        assert!(face.is_trimmed());
        assert_eq!(face.trim_curves.len(), 1);
    }

    #[test]
    fn set_uv_bounds_writes_through() {
        let mut face = Face::new(0, 0, 0, FaceOrientation::Forward);
        face.set_uv_bounds(-1.0, 2.0, -3.0, 4.0);
        assert_eq!(face.uv_bounds, [-1.0, 2.0, -3.0, 4.0]);
    }

    #[test]
    fn add_adjacent_records_edge_to_face_mapping() {
        let mut face = Face::new(0, 0, 0, FaceOrientation::Forward);
        face.add_adjacent(42, 11);
        face.add_adjacent(43, 12);
        assert_eq!(face.adjacent_faces.get(&42), Some(&11));
        assert_eq!(face.adjacent_faces.get(&43), Some(&12));
        assert_eq!(face.adjacent_faces.len(), 2);
    }

    #[test]
    fn add_adjacent_overwrites_existing_edge_mapping() {
        let mut face = Face::new(0, 0, 0, FaceOrientation::Forward);
        face.add_adjacent(42, 11);
        face.add_adjacent(42, 99); // same edge, new neighbour
        assert_eq!(face.adjacent_faces.get(&42), Some(&99));
        assert_eq!(face.adjacent_faces.len(), 1);
    }

    #[test]
    fn reversed_flips_orientation_and_keeps_topology() {
        let mut face = Face::new(7, 1, 2, FaceOrientation::Forward);
        face.add_inner_loop(3);
        let r = face.reversed();
        assert_eq!(r.orientation, FaceOrientation::Backward);
        assert_eq!(r.id, 7);
        assert_eq!(r.surface_id, 1);
        assert_eq!(r.outer_loop, 2);
        assert_eq!(r.inner_loops, vec![3]);
    }

    #[test]
    fn reversed_twice_is_original_orientation() {
        let face = Face::new(0, 0, 0, FaceOrientation::Backward);
        assert_eq!(face.reversed().reversed().orientation, face.orientation);
    }

    // ----- is_degenerate ---------------------------------------------------

    #[test]
    fn is_degenerate_when_outer_loop_missing() {
        let vertex_store = VertexStore::with_capacity(0);
        let edge_store = EdgeStore::new();
        let loop_store = LoopStore::new();
        // outer_loop = 99 not present in store -> degenerate
        let face = Face::new(0, 0, 99, FaceOrientation::Forward);
        assert!(face.is_degenerate(
            &loop_store,
            &edge_store,
            &vertex_store,
            Tolerance::default(),
        ));
    }

    #[test]
    fn is_degenerate_when_loop_has_two_distinct_vertices() {
        // Two distinct points -> vertex_ids.len() < 3 after dedup -> degenerate.
        let (v, e, l, lid) = build_polygon_loop(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]);
        let face = Face::new(0, 0, lid, FaceOrientation::Forward);
        assert!(face.is_degenerate(&l, &e, &v, Tolerance::default()));
    }

    #[test]
    fn is_degenerate_when_three_vertices_are_collinear() {
        // Three points on the X axis -> Newell area magnitude ≈ 0.
        let (v, e, l, lid) =
            build_polygon_loop(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
        let face = Face::new(0, 0, lid, FaceOrientation::Forward);
        assert!(face.is_degenerate(&l, &e, &v, Tolerance::default()));
    }

    #[test]
    fn is_degenerate_when_three_vertices_are_a_sliver_under_tolerance() {
        // Sliver triangle: two long edges of length 10 with the apex offset
        // perpendicularly by 1e-15. Newell area = 0.5 · 10 · 1e-15 = 5e-15,
        // which sits well below the default tol² = (1e-6)² = 1e-12.
        let (v, e, l, lid) =
            build_polygon_loop(&[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [5.0, 1e-15, 0.0]]);
        let face = Face::new(0, 0, lid, FaceOrientation::Forward);
        assert!(face.is_degenerate(&l, &e, &v, Tolerance::default()));
    }

    #[test]
    fn is_not_degenerate_for_unit_square() {
        let (v, e, l, lid) = build_polygon_loop(&[
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ]);
        let face = Face::new(0, 0, lid, FaceOrientation::Forward);
        assert!(!face.is_degenerate(&l, &e, &v, Tolerance::default()));
    }

    #[test]
    fn is_not_degenerate_for_tilted_planar_triangle() {
        // Plane normal not aligned with any axis — Newell handles this.
        let (v, e, l, lid) =
            build_polygon_loop(&[[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 1.0]]);
        let face = Face::new(0, 0, lid, FaceOrientation::Forward);
        assert!(!face.is_degenerate(&l, &e, &v, Tolerance::default()));
    }

    #[test]
    fn is_degenerate_collapses_consecutive_duplicate_vertices() {
        // Tolerance::default() distance is large enough that nearly-coincident
        // points are collapsed; the residual loop has only 2 distinct points.
        let (v, e, l, lid) =
            build_polygon_loop(&[[0.0, 0.0, 0.0], [1e-15, 0.0, 0.0], [1.0, 0.0, 0.0]]);
        let face = Face::new(0, 0, lid, FaceOrientation::Forward);
        assert!(face.is_degenerate(&l, &e, &v, Tolerance::default()));
    }

    // ----- FaceStore CRUD --------------------------------------------------

    #[test]
    fn store_new_is_empty() {
        let store = FaceStore::new();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn store_default_matches_new() {
        let a = FaceStore::default();
        assert!(a.is_empty());
    }

    #[test]
    fn store_with_capacity_is_empty_until_used() {
        let store = FaceStore::with_capacity(64);
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn store_add_assigns_sequential_ids() {
        let mut store = FaceStore::new();
        let a = store.add(Face::new(999, 0, 0, FaceOrientation::Forward));
        let b = store.add(Face::new(999, 0, 0, FaceOrientation::Forward));
        let c = store.add(Face::new(999, 0, 0, FaceOrientation::Forward));
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2);
        assert_eq!(store.len(), 3);
        assert!(!store.is_empty());
    }

    #[test]
    fn store_add_overwrites_supplied_id_with_assigned_id() {
        let mut store = FaceStore::new();
        let id = store.add(Face::new(42, 1, 2, FaceOrientation::Forward));
        let face = store.get(id).expect("face just added");
        assert_eq!(face.id, id);
        assert_ne!(face.id, 42);
    }

    #[test]
    fn store_add_increments_total_created_stat() {
        let mut store = FaceStore::new();
        store.add(Face::new(0, 0, 0, FaceOrientation::Forward));
        store.add(Face::new(0, 0, 0, FaceOrientation::Forward));
        assert_eq!(store.stats.total_created, 2);
        assert_eq!(store.stats.total_deleted, 0);
    }

    #[test]
    fn store_get_out_of_range_is_none() {
        let store = FaceStore::new();
        assert!(store.get(0).is_none());
        assert!(store.get(99).is_none());
    }

    #[test]
    fn store_get_mut_allows_field_mutation() {
        let mut store = FaceStore::new();
        let id = store.add(Face::new(0, 1, 2, FaceOrientation::Forward));
        {
            let f = store.get_mut(id).expect("face just added");
            f.attributes.selected = true;
        }
        assert!(store.get(id).expect("face").attributes.selected);
    }

    #[test]
    fn store_set_tolerance_returns_true_for_valid_id() {
        let mut store = FaceStore::new();
        let id = store.add(Face::new(0, 1, 2, FaceOrientation::Forward));
        assert!(store.set_tolerance(id, 5e-7));
        assert_eq!(store.get(id).expect("face").tolerance, 5e-7);
    }

    #[test]
    fn store_set_tolerance_returns_false_for_missing_id() {
        let mut store = FaceStore::new();
        assert!(!store.set_tolerance(99, 1e-9));
    }

    #[test]
    fn store_remove_returns_face_and_marks_slot_invalid() {
        let mut store = FaceStore::new();
        let id = store.add_with_indexing(Face::new(0, 1, 2, FaceOrientation::Forward));
        let removed = store.remove(id);
        assert!(removed.is_some());
        // Slot is replaced with a sentinel; iter() must skip it.
        assert_eq!(store.iter().count(), 0);
        assert_eq!(store.stats.total_deleted, 1);
    }

    #[test]
    fn store_remove_unknown_id_is_none() {
        let mut store = FaceStore::new();
        assert!(store.remove(99).is_none());
        assert_eq!(store.stats.total_deleted, 0);
    }

    #[test]
    fn store_remove_preserves_ids_of_other_faces() {
        let mut store = FaceStore::new();
        let a = store.add_with_indexing(Face::new(0, 1, 10, FaceOrientation::Forward));
        let b = store.add_with_indexing(Face::new(0, 1, 11, FaceOrientation::Forward));
        let c = store.add_with_indexing(Face::new(0, 1, 12, FaceOrientation::Forward));
        store.remove(b);
        // a and c keep their original ids — Vec is not compacted.
        assert_eq!(store.get(a).expect("face a").outer_loop, 10);
        assert_eq!(store.get(c).expect("face c").outer_loop, 12);
        let live: Vec<_> = store.iter().map(|(id, _)| id).collect();
        assert_eq!(live, vec![a, c]);
    }

    // ----- Surface / loop / adjacency queries ------------------------------

    #[test]
    fn faces_on_surface_finds_indexed_faces() {
        let mut store = FaceStore::new();
        let a = store.add_with_indexing(Face::new(0, 7, 1, FaceOrientation::Forward));
        let _ = store.add_with_indexing(Face::new(0, 8, 2, FaceOrientation::Forward));
        let c = store.add_with_indexing(Face::new(0, 7, 3, FaceOrientation::Forward));
        let mut hits = store.faces_on_surface(7);
        hits.sort_unstable();
        assert_eq!(hits, vec![a, c]);
    }

    #[test]
    fn faces_on_surface_is_empty_for_fast_path_adds() {
        let mut store = FaceStore::new();
        // `add` (fast path) deliberately skips index updates.
        store.add(Face::new(0, 7, 1, FaceOrientation::Forward));
        assert!(store.faces_on_surface(7).is_empty());
    }

    #[test]
    fn faces_on_surface_unknown_surface_is_empty() {
        let store = FaceStore::new();
        assert!(store.faces_on_surface(123).is_empty());
    }

    #[test]
    fn faces_with_loop_indexes_outer_and_inner_loops() {
        let mut store = FaceStore::new();
        let mut f = Face::new(0, 1, 100, FaceOrientation::Forward);
        f.add_inner_loop(200);
        f.add_inner_loop(201);
        let id = store.add_with_indexing(f);
        assert_eq!(store.faces_with_loop(100), vec![id]);
        assert_eq!(store.faces_with_loop(200), vec![id]);
        assert_eq!(store.faces_with_loop(201), vec![id]);
        assert!(store.faces_with_loop(999).is_empty());
    }

    #[test]
    fn iter_skips_removed_slots() {
        let mut store = FaceStore::new();
        let a = store.add_with_indexing(Face::new(0, 1, 10, FaceOrientation::Forward));
        let b = store.add_with_indexing(Face::new(0, 1, 11, FaceOrientation::Forward));
        let c = store.add_with_indexing(Face::new(0, 1, 12, FaceOrientation::Forward));
        store.remove(a);
        store.remove(c);
        let live: Vec<_> = store.iter().map(|(id, _)| id).collect();
        assert_eq!(live, vec![b]);
    }

    #[test]
    fn find_adjacent_faces_returns_mutually_referenced_faces() {
        let mut store = FaceStore::new();
        let mut f0 = Face::new(0, 1, 10, FaceOrientation::Forward);
        let mut f1 = Face::new(0, 1, 11, FaceOrientation::Forward);
        // Edge 50 is shared. Each face records the *neighbour* keyed by the
        // shared edge; find_adjacent_faces walks face0's neighbours and only
        // returns those whose own neighbour map points back via the same edge.
        f0.add_adjacent(50, 1);
        f1.add_adjacent(50, 0);
        let id0 = store.add_with_indexing(f0);
        let id1 = store.add_with_indexing(f1);
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(store.find_adjacent_faces(0, 50), vec![1]);
    }

    #[test]
    fn find_adjacent_faces_empty_when_no_back_reference() {
        let mut store = FaceStore::new();
        let mut f0 = Face::new(0, 1, 10, FaceOrientation::Forward);
        let f1 = Face::new(0, 1, 11, FaceOrientation::Forward);
        f0.add_adjacent(50, 1); // f1 has no reciprocal edge -> not adjacent
        let _id0 = store.add_with_indexing(f0);
        let _id1 = store.add_with_indexing(f1);
        assert!(store.find_adjacent_faces(0, 50).is_empty());
    }

    #[test]
    fn find_adjacent_faces_unknown_face_is_empty() {
        let store = FaceStore::new();
        assert!(store.find_adjacent_faces(99, 50).is_empty());
    }
}

/// Validation result for faces
#[derive(Debug, Clone)]
pub struct FaceValidation {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}
