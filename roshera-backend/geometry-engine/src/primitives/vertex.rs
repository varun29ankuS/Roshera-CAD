//! Vertex representation for B-Rep topology.
//!
//! Features:
//! - Structure-of-Arrays layout for cache efficiency
//! - Spatial hashing for deduplication
//! - Vertex-attributes system for custom data
//! - Merge/split operations
//! - Thread-safe concurrent access
//!
//! Indexed access into the SoA xs/ys/zs vertex buffers is the canonical
//! idiom — all `arr[i]` sites use indices bounded by `xs.len()` (the three
//! arrays maintain identical length by construction). Matches the
//! numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{Point3, Vector3};
// Note: Using DashMap globally for timeline architecture compatibility
use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Vertex ID type - u32 supports ~4.2 billion vertices
pub type VertexId = u32;

/// Invalid vertex ID constant
pub const INVALID_VERTEX_ID: VertexId = u32::MAX;

/// Vertex attributes for extensibility
#[derive(Debug, Clone, PartialEq)]
pub enum VertexAttribute {
    /// Color information (RGBA)
    Color([f32; 4]),
    /// Texture coordinates
    TextureCoords([f32; 2]),
    /// Normal vector (for rendering hints)
    Normal(Vector3),
    /// Custom user data
    UserData(Vec<u8>),
    /// Material ID reference
    MaterialId(u32),
    /// Selection state
    Selected(bool),
}

/// Compact vertex representation: position, surface params, id, attribute set
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    /// 3D position coordinates
    pub position: [f64; 3],
    /// Surface parameters (u, v) - NaN if not on a surface
    pub params: [f64; 2],
    /// Unique identifier for topology references
    pub id: VertexId,
    /// Topology flags (8 bits available)
    pub flags: u8,
    /// Reserved for future use (maintains 48-byte alignment)
    _reserved: [u8; 3],
    /// Tolerance for this vertex (typically 1e-6 to 1e-10)
    pub tolerance: f64,
}

/// Vertex flags for efficient topology queries
#[allow(non_snake_case)] // Pascal-case module name used as bit-flag namespace
pub mod VertexFlags {
    pub const ON_EDGE: u8 = 0b00000001;
    pub const ON_FACE: u8 = 0b00000010;
    pub const BOUNDARY: u8 = 0b00000100;
    pub const MANIFOLD: u8 = 0b00001000;
    pub const LOCKED: u8 = 0b00010000;
    pub const DELETED: u8 = 0b00100000;
    pub const MODIFIED: u8 = 0b01000000;
    pub const SELECTED: u8 = 0b10000000;
}

impl Vertex {
    /// Create a new vertex with position only
    #[inline(always)]
    pub fn new(id: VertexId, x: f64, y: f64, z: f64) -> Self {
        Self {
            position: [x, y, z],
            params: [f64::NAN, f64::NAN],
            id,
            flags: 0,
            _reserved: [0; 3],
            tolerance: 1e-6, // Default CAD tolerance
        }
    }

    /// Create a new vertex with position and surface parameters
    #[inline(always)]
    pub fn new_with_params(id: VertexId, x: f64, y: f64, z: f64, u: f64, v: f64) -> Self {
        Self {
            position: [x, y, z],
            params: [u, v],
            id,
            flags: VertexFlags::ON_FACE,
            _reserved: [0; 3],
            tolerance: 1e-6, // Default CAD tolerance
        }
    }

    /// Get position as Point3
    #[inline(always)]
    pub fn point(&self) -> Point3 {
        Point3::new(self.position[0], self.position[1], self.position[2])
    }

    /// Set flag
    #[inline(always)]
    pub fn set_flag(&mut self, flag: u8, value: bool) {
        if value {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    /// Check flag
    #[inline(always)]
    pub fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    /// Check if vertex is on boundary
    #[inline(always)]
    pub fn is_boundary(&self) -> bool {
        self.has_flag(VertexFlags::BOUNDARY)
    }

    /// Check if vertex is manifold
    #[inline(always)]
    pub fn is_manifold(&self) -> bool {
        self.has_flag(VertexFlags::MANIFOLD)
    }

    /// Set tolerance for this vertex
    #[inline(always)]
    pub fn set_tolerance(&mut self, tolerance: f64) {
        self.tolerance = tolerance;
    }

    /// Get tolerance for this vertex
    #[inline(always)]
    pub fn get_tolerance(&self) -> f64 {
        self.tolerance
    }
}

/// Spatial hash key for vertex deduplication
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
struct SpatialHashKey {
    x: i64,
    y: i64,
    z: i64,
}

impl SpatialHashKey {
    /// Create hash key from position with given tolerance
    fn from_position(x: f64, y: f64, z: f64, grid_size: f64) -> Self {
        Self {
            x: (x / grid_size).round() as i64,
            y: (y / grid_size).round() as i64,
            z: (z / grid_size).round() as i64,
        }
    }
}

/// Vertex storage with advanced features
#[derive(Debug)]
pub struct VertexStore {
    /// X coordinates packed together (SIMD-friendly)
    x_coords: Vec<f64>,
    /// Y coordinates packed together (SIMD-friendly)
    y_coords: Vec<f64>,
    /// Z coordinates packed together (SIMD-friendly)
    z_coords: Vec<f64>,
    /// U parameters (NaN if not on surface)
    u_params: Vec<f64>,
    /// V parameters (NaN if not on surface)
    v_params: Vec<f64>,
    /// Topology flags
    flags: Vec<u8>,
    /// Tolerances for each vertex
    tolerances: Vec<f64>,
    /// Spatial hash for O(1) deduplication - DashMap for timeline compatibility
    spatial_hash: DashMap<SpatialHashKey, Vec<VertexId>>,
    /// Custom attributes - DashMap for timeline compatibility
    attributes: DashMap<VertexId, Vec<VertexAttribute>>,
    /// Next available ID (atomic for thread safety)
    next_id: AtomicU32,
    /// Grid size for spatial hashing
    grid_size: f64,
    /// Deduplication enabled flag - can be disabled for speed
    enable_deduplication: bool,
    /// Statistics
    pub stats: VertexStoreStats,
}

/// Performance statistics
#[derive(Debug, Default, Clone)]
pub struct VertexStoreStats {
    pub total_created: u64,
    pub duplicates_found: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

impl VertexStore {
    /// Create with capacity and tolerance for deduplication
    pub fn with_capacity_and_tolerance(capacity: usize, tolerance: f64) -> Self {
        Self {
            x_coords: Vec::with_capacity(capacity),
            y_coords: Vec::with_capacity(capacity),
            z_coords: Vec::with_capacity(capacity),
            u_params: Vec::with_capacity(capacity),
            v_params: Vec::with_capacity(capacity),
            flags: Vec::with_capacity(capacity),
            tolerances: Vec::with_capacity(capacity),
            spatial_hash: DashMap::with_capacity(capacity / 4), // Smaller spatial hash
            attributes: DashMap::new(),
            next_id: AtomicU32::new(0),
            grid_size: tolerance * 10.0, // Grid size based on tolerance
            enable_deduplication: tolerance > 1e-10, // Disable for very small tolerances
            stats: VertexStoreStats::default(),
        }
    }

    /// Create with deduplication disabled for maximum speed
    pub fn with_capacity_no_dedup(capacity: usize) -> Self {
        Self {
            x_coords: Vec::with_capacity(capacity),
            y_coords: Vec::with_capacity(capacity),
            z_coords: Vec::with_capacity(capacity),
            u_params: Vec::with_capacity(capacity),
            v_params: Vec::with_capacity(capacity),
            flags: Vec::with_capacity(capacity),
            tolerances: Vec::with_capacity(capacity),
            spatial_hash: DashMap::new(), // Empty spatial hash
            attributes: DashMap::new(),
            next_id: AtomicU32::new(0),
            grid_size: 0.0,
            enable_deduplication: false,
            stats: VertexStoreStats::default(),
        }
    }

    /// Deep copy of this store for the F2-δ ModelSnapshot primitive.
    ///
    /// All SoA buffers are cloned by value. The two DashMaps are
    /// rebuilt entry-by-entry so the new store owns its concurrent
    /// state independent of the original. `next_id` is read with
    /// `Acquire` ordering and reseeded into a fresh atomic — the
    /// snapshot must observe at least every id that has been handed
    /// out at the call site, and `Acquire` pairs with the `Release`
    /// stores used in `add_or_find`.
    pub(crate) fn deep_copy(&self) -> Self {
        let spatial_hash = DashMap::with_capacity(self.spatial_hash.len());
        for kv in self.spatial_hash.iter() {
            spatial_hash.insert(*kv.key(), kv.value().clone());
        }
        let attributes = DashMap::with_capacity(self.attributes.len());
        for kv in self.attributes.iter() {
            attributes.insert(*kv.key(), kv.value().clone());
        }
        Self {
            x_coords: self.x_coords.clone(),
            y_coords: self.y_coords.clone(),
            z_coords: self.z_coords.clone(),
            u_params: self.u_params.clone(),
            v_params: self.v_params.clone(),
            flags: self.flags.clone(),
            tolerances: self.tolerances.clone(),
            spatial_hash,
            attributes,
            next_id: AtomicU32::new(self.next_id.load(Ordering::Acquire)),
            grid_size: self.grid_size,
            enable_deduplication: self.enable_deduplication,
            stats: self.stats.clone(),
        }
    }

    /// Add or find existing vertex (with deduplication) - OPTIMIZED VERSION
    ///
    /// The coincidence ball is the *union* of the caller-supplied tolerance
    /// sphere and each candidate's stored tolerance sphere (Parasolid
    /// convention): two points with tolerances `t1, t2` are coincident iff
    /// `dist(p1, p2) ≤ max(t1, t2)`. This means snapping a tight new
    /// vertex onto an existing loose one does not require relaxing the
    /// loose vertex's tolerance, and matches the behaviour of
    /// `tolerance_propagation::merge_tolerance`.
    #[inline(always)]
    pub fn add_or_find(&mut self, x: f64, y: f64, z: f64, tolerance: f64) -> VertexId {
        // Fast linear search through vertices for deduplication
        // This is much faster than DashMap for small numbers of vertices (like primitive creation)
        for i in 0..self.x_coords.len() {
            // Skip deleted vertices
            if self.flags[i] & VertexFlags::DELETED != 0 {
                continue;
            }

            let dx = self.x_coords[i] - x;
            let dy = self.y_coords[i] - y;
            let dz = self.z_coords[i] - z;
            let dist_sq = dx * dx + dy * dy + dz * dz;

            let stored = self.tolerances.get(i).copied().unwrap_or(1e-6);
            let merged = stored.max(tolerance);
            let tolerance_sq = merged * merged;

            if dist_sq <= tolerance_sq {
                self.stats.duplicates_found += 1;
                self.stats.cache_hits += 1;
                return i as VertexId;
            }
        }

        // No match found, create new vertex stamped with caller's tolerance
        self.stats.cache_misses += 1;
        self.add_unchecked_with_tolerance(x, y, z, tolerance)
    }

    /// Add vertex with full deduplication (use sparingly - expensive)
    ///
    /// Coincidence ball is `max(caller, stored)` per vertex (see
    /// `add_or_find` for the rationale).
    pub fn add_or_find_with_dedup(&mut self, x: f64, y: f64, z: f64, tolerance: f64) -> VertexId {
        if !self.enable_deduplication || tolerance < 1e-10 {
            return self.add_unchecked_with_tolerance(x, y, z, tolerance);
        }

        let key = SpatialHashKey::from_position(x, y, z, self.grid_size);

        // Check for duplicates
        if let Some(candidates_ref) = self.spatial_hash.get(&key) {
            for &id in candidates_ref.value().iter() {
                let idx = id as usize;
                let dx = self.x_coords[idx] - x;
                let dy = self.y_coords[idx] - y;
                let dz = self.z_coords[idx] - z;

                let stored = self.tolerances.get(idx).copied().unwrap_or(1e-6);
                let merged = stored.max(tolerance);
                let tolerance_sq = merged * merged;

                if dx * dx + dy * dy + dz * dz <= tolerance_sq {
                    self.stats.duplicates_found += 1;
                    self.stats.cache_hits += 1;
                    return id;
                }
            }
        }

        // Create new vertex stamped with caller's tolerance
        self.stats.cache_misses += 1;
        let id = self.add_unchecked_with_tolerance(x, y, z, tolerance);

        // Update spatial hash
        if let Some(mut entry) = self.spatial_hash.get_mut(&key) {
            entry.push(id);
        } else {
            self.spatial_hash.insert(key, vec![id]);
        }

        id
    }

    /// PERFORMANCE: Batch add multiple vertices with optimized deduplication
    ///
    /// Coincidence ball is `max(caller, stored)` per vertex (see
    /// `add_or_find` for the rationale).
    pub fn add_or_find_batch(
        &mut self,
        positions: &[(f64, f64, f64)],
        tolerance: f64,
    ) -> Vec<VertexId> {
        let mut result = Vec::with_capacity(positions.len());

        for &(x, y, z) in positions {
            let key = SpatialHashKey::from_position(x, y, z, self.grid_size);

            let mut found = None;
            if let Some(candidates) = self.spatial_hash.get(&key) {
                for &id in candidates.iter() {
                    let idx = id as usize;
                    let dx = self.x_coords[idx] - x;
                    let dy = self.y_coords[idx] - y;
                    let dz = self.z_coords[idx] - z;

                    let stored = self.tolerances.get(idx).copied().unwrap_or(1e-6);
                    let merged = stored.max(tolerance);
                    let tolerance_sq = merged * merged;

                    if dx * dx + dy * dy + dz * dz <= tolerance_sq {
                        found = Some(id);
                        self.stats.duplicates_found += 1;
                        self.stats.cache_hits += 1;
                        break;
                    }
                }
            }

            let id = match found {
                Some(id) => id,
                None => {
                    self.stats.cache_misses += 1;
                    let id = self.add_unchecked_with_tolerance(x, y, z, tolerance);
                    self.spatial_hash
                        .entry(key)
                        .or_insert_with(|| Vec::with_capacity(4))
                        .push(id);
                    id
                }
            };

            result.push(id);
        }

        result
    }

    /// Add vertex without deduplication check (uses default 1e-6 tolerance)
    #[inline(always)]
    pub fn add_unchecked(&mut self, x: f64, y: f64, z: f64) -> VertexId {
        self.add_unchecked_with_tolerance(x, y, z, 1e-6)
    }

    /// Add vertex without deduplication check, stamping the supplied
    /// tolerance on the new entity.
    ///
    /// Use this from operation sites that know the working tolerance of
    /// the op producing the vertex — downstream coincidence queries
    /// (e.g. sewing, intersection) will respect that stamped value via
    /// the `max(caller, stored)` rule in `add_or_find`.
    #[inline(always)]
    pub fn add_unchecked_with_tolerance(
        &mut self,
        x: f64,
        y: f64,
        z: f64,
        tolerance: f64,
    ) -> VertexId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.x_coords.push(x);
        self.y_coords.push(y);
        self.z_coords.push(z);
        self.u_params.push(f64::NAN);
        self.v_params.push(f64::NAN);
        self.flags.push(0);
        self.tolerances.push(tolerance);
        self.stats.total_created += 1;
        id
    }

    /// Batch transform vertices
    #[inline]
    pub fn transform_batch(&mut self, ids: &[VertexId], transform: &crate::math::Matrix4) {
        // This could be SIMD optimized
        for &id in ids {
            if let Some(pos) = self.get_position(id) {
                let p = Point3::new(pos[0], pos[1], pos[2]);
                let transformed = transform.transform_point(&p);
                self.set_position(id, transformed.x, transformed.y, transformed.z);
            }
        }
    }

    /// Find vertices in bounding box
    pub fn find_in_box(&self, min: &Point3, max: &Point3) -> Vec<VertexId> {
        let mut results = Vec::new();

        for i in 0..self.x_coords.len() {
            let x = self.x_coords[i];
            let y = self.y_coords[i];
            let z = self.z_coords[i];

            if x >= min.x && x <= max.x && y >= min.y && y <= max.y && z >= min.z && z <= max.z {
                results.push(i as VertexId);
            }
        }

        results
    }

    /// Merge two vertices (topology operation)
    pub fn merge_vertices(&mut self, keep: VertexId, remove: VertexId) -> bool {
        let keep_idx = keep as usize;
        let remove_idx = remove as usize;

        if keep_idx >= self.x_coords.len() || remove_idx >= self.x_coords.len() {
            return false;
        }

        // Mark removed vertex as deleted
        self.flags[remove_idx] |= VertexFlags::DELETED;

        // Update spatial hash
        let key = SpatialHashKey::from_position(
            self.x_coords[remove_idx],
            self.y_coords[remove_idx],
            self.z_coords[remove_idx],
            self.grid_size,
        );

        if let Some(mut candidates) = self.spatial_hash.get_mut(&key) {
            candidates.retain(|&id| id != remove);
        }

        true
    }

    /// Get vertex attributes
    #[inline]
    pub fn get_attributes(&self, id: VertexId) -> Option<Vec<VertexAttribute>> {
        self.attributes.get(&id).map(|r| r.clone())
    }

    /// Set vertex attributes
    #[inline]
    pub fn set_attributes(&mut self, id: VertexId, attrs: Vec<VertexAttribute>) {
        self.attributes.insert(id, attrs);
    }

    /// Compact storage (remove deleted vertices)
    pub fn compact(&mut self) -> DashMap<VertexId, VertexId> {
        let remap = DashMap::new();
        let mut write_idx = 0;

        for read_idx in 0..self.x_coords.len() {
            if self.flags[read_idx] & VertexFlags::DELETED == 0 {
                if write_idx != read_idx {
                    self.x_coords[write_idx] = self.x_coords[read_idx];
                    self.y_coords[write_idx] = self.y_coords[read_idx];
                    self.z_coords[write_idx] = self.z_coords[read_idx];
                    self.u_params[write_idx] = self.u_params[read_idx];
                    self.v_params[write_idx] = self.v_params[read_idx];
                    self.flags[write_idx] = self.flags[read_idx];
                    let src_tol = self.tolerances.get(read_idx).copied();
                    if let (Some(src), Some(slot)) = (src_tol, self.tolerances.get_mut(write_idx)) {
                        *slot = src;
                    }
                }
                remap.insert(read_idx as VertexId, write_idx as VertexId);
                write_idx += 1;
            }
        }

        // Truncate arrays
        self.x_coords.truncate(write_idx);
        self.y_coords.truncate(write_idx);
        self.z_coords.truncate(write_idx);
        self.u_params.truncate(write_idx);
        self.v_params.truncate(write_idx);
        self.flags.truncate(write_idx);
        self.tolerances.truncate(write_idx);

        // Rebuild spatial hash
        self.rebuild_spatial_hash();

        remap
    }

    /// Rebuild spatial hash after modifications
    fn rebuild_spatial_hash(&mut self) {
        self.spatial_hash.clear();

        for i in 0..self.x_coords.len() {
            if self.flags[i] & VertexFlags::DELETED == 0 {
                let key = SpatialHashKey::from_position(
                    self.x_coords[i],
                    self.y_coords[i],
                    self.z_coords[i],
                    self.grid_size,
                );
                self.spatial_hash
                    .entry(key)
                    .or_default()
                    .push(i as VertexId);
            }
        }
    }

    /// Get vertex by ID
    #[inline(always)]
    pub fn get(&self, id: VertexId) -> Option<Vertex> {
        let idx = id as usize;
        if idx < self.x_coords.len() && self.flags[idx] & VertexFlags::DELETED == 0 {
            Some(Vertex {
                position: [self.x_coords[idx], self.y_coords[idx], self.z_coords[idx]],
                params: [self.u_params[idx], self.v_params[idx]],
                id,
                flags: self.flags[idx],
                _reserved: [0; 3],
                tolerance: self.tolerances.get(idx).copied().unwrap_or(1e-6),
            })
        } else {
            None
        }
    }

    /// Get position only (most efficient)
    #[inline(always)]
    pub fn get_position(&self, id: VertexId) -> Option<[f64; 3]> {
        let idx = id as usize;
        if idx < self.x_coords.len() {
            Some([self.x_coords[idx], self.y_coords[idx], self.z_coords[idx]])
        } else {
            None
        }
    }

    /// Get tolerance for a vertex
    #[inline(always)]
    pub fn get_tolerance(&self, id: VertexId) -> Option<f64> {
        let idx = id as usize;
        self.tolerances.get(idx).copied()
    }

    /// Set tolerance for a vertex
    #[inline(always)]
    pub fn set_tolerance(&mut self, id: VertexId, tolerance: f64) -> bool {
        let idx = id as usize;
        if idx < self.tolerances.len() {
            self.tolerances[idx] = tolerance;
            true
        } else {
            false
        }
    }

    /// Remove a vertex from the store (mark as deleted)
    pub fn remove(&mut self, id: VertexId) -> bool {
        let idx = id as usize;
        if idx < self.flags.len() {
            // Mark as deleted
            self.flags[idx] |= VertexFlags::DELETED;
            self.stats.total_created -= 1; // Decrement count
            true
        } else {
            false
        }
    }

    /// Iterate over all non-deleted vertices
    pub fn iter(&self) -> impl Iterator<Item = (VertexId, Vertex)> + '_ {
        (0..self.x_coords.len())
            .filter(|&idx| self.flags[idx] & VertexFlags::DELETED == 0)
            .map(move |idx| {
                let vertex = Vertex {
                    position: [self.x_coords[idx], self.y_coords[idx], self.z_coords[idx]],
                    params: [self.u_params[idx], self.v_params[idx]],
                    id: idx as VertexId,
                    flags: self.flags[idx],
                    _reserved: [0; 3],
                    tolerance: self.tolerances.get(idx).copied().unwrap_or(1e-6),
                };
                (idx as VertexId, vertex)
            })
    }

    /// Set position
    #[inline(always)]
    pub fn set_position(&mut self, id: VertexId, x: f64, y: f64, z: f64) -> bool {
        let idx = id as usize;
        if idx < self.x_coords.len() {
            // Update position
            self.x_coords[idx] = x;
            self.y_coords[idx] = y;
            self.z_coords[idx] = z;

            // Mark as modified
            self.flags[idx] |= VertexFlags::MODIFIED;

            true
        } else {
            false
        }
    }

    /// Number of active vertices
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.x_coords.len() - self.deleted_count()
    }

    /// Count deleted vertices
    fn deleted_count(&self) -> usize {
        self.flags
            .iter()
            .filter(|&&f| f & VertexFlags::DELETED != 0)
            .count()
    }
}

// Original methods preserved for compatibility
impl VertexStore {
    pub fn with_capacity(capacity: usize) -> Self {
        // Use fast no-dedup version for better performance
        Self::with_capacity_no_dedup(capacity)
    }

    pub fn add(&mut self, x: f64, y: f64, z: f64) -> VertexId {
        self.add_unchecked(x, y, z)
    }

    pub fn add_with_params(&mut self, x: f64, y: f64, z: f64, u: f64, v: f64) -> VertexId {
        let id = self.add_unchecked(x, y, z);
        let idx = id as usize;
        self.u_params[idx] = u;
        self.v_params[idx] = v;
        self.flags[idx] |= VertexFlags::ON_FACE;
        id
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn reserve(&mut self, additional: usize) {
        self.x_coords.reserve(additional);
        self.y_coords.reserve(additional);
        self.z_coords.reserve(additional);
        self.u_params.reserve(additional);
        self.v_params.reserve(additional);
        self.flags.reserve(additional);
        self.tolerances.reserve(additional);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Matrix4;

    // ---- Vertex unit tests --------------------------------------------------

    #[test]
    fn vertex_new_records_position_and_nan_uv_params() {
        let v = Vertex::new(7, 1.0, 2.0, 3.0);
        assert_eq!(v.id, 7);
        assert_eq!(v.position, [1.0, 2.0, 3.0]);
        assert!(v.params[0].is_nan() && v.params[1].is_nan());
        assert_eq!(v.flags, 0);
        assert!((v.tolerance - 1e-6).abs() < 1e-15);
    }

    #[test]
    fn vertex_new_with_params_sets_on_face_flag() {
        let v = Vertex::new_with_params(0, 0.0, 0.0, 0.0, 0.25, 0.75);
        assert_eq!(v.params, [0.25, 0.75]);
        assert!(v.has_flag(VertexFlags::ON_FACE));
    }

    #[test]
    fn vertex_point_round_trips_position() {
        let v = Vertex::new(0, 1.5, -2.5, 3.5);
        let p = v.point();
        assert!((p.x - 1.5).abs() < 1e-15);
        assert!((p.y + 2.5).abs() < 1e-15);
        assert!((p.z - 3.5).abs() < 1e-15);
    }

    #[test]
    fn vertex_set_flag_supports_set_and_clear() {
        let mut v = Vertex::new(0, 0.0, 0.0, 0.0);
        v.set_flag(VertexFlags::BOUNDARY, true);
        assert!(v.is_boundary());
        v.set_flag(VertexFlags::BOUNDARY, false);
        assert!(!v.is_boundary());
    }

    #[test]
    fn vertex_set_flag_does_not_disturb_other_bits() {
        let mut v = Vertex::new(0, 0.0, 0.0, 0.0);
        v.set_flag(VertexFlags::BOUNDARY, true);
        v.set_flag(VertexFlags::MANIFOLD, true);
        v.set_flag(VertexFlags::BOUNDARY, false);
        assert!(!v.is_boundary());
        assert!(v.is_manifold());
    }

    #[test]
    fn vertex_flag_helpers_match_underlying_bits() {
        let mut v = Vertex::new(0, 0.0, 0.0, 0.0);
        v.flags = VertexFlags::BOUNDARY | VertexFlags::MANIFOLD;
        assert!(v.is_boundary());
        assert!(v.is_manifold());
        assert!(!v.has_flag(VertexFlags::ON_EDGE));
    }

    #[test]
    fn vertex_tolerance_get_set_round_trip() {
        let mut v = Vertex::new(0, 0.0, 0.0, 0.0);
        assert!((v.get_tolerance() - 1e-6).abs() < 1e-15);
        v.set_tolerance(1e-9);
        assert!((v.get_tolerance() - 1e-9).abs() < 1e-15);
    }

    // ---- VertexStore add / get ---------------------------------------------

    #[test]
    fn store_add_unchecked_assigns_sequential_ids() {
        let mut s = VertexStore::with_capacity_no_dedup(8);
        let a = s.add_unchecked(0.0, 0.0, 0.0);
        let b = s.add_unchecked(1.0, 0.0, 0.0);
        let c = s.add_unchecked(2.0, 0.0, 0.0);
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2);
        assert_eq!(s.len(), 3);
        assert_eq!(s.stats.total_created, 3);
    }

    #[test]
    fn store_get_returns_inserted_position() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let id = s.add(2.5, -3.0, 4.25);
        let v = s.get(id).expect("vertex should be retrievable");
        assert_eq!(v.position, [2.5, -3.0, 4.25]);
        assert_eq!(v.id, id);
    }

    #[test]
    fn store_get_returns_none_for_out_of_range_id() {
        let s = VertexStore::with_capacity_no_dedup(4);
        assert!(s.get(0).is_none());
        assert!(s.get(99).is_none());
    }

    #[test]
    fn store_get_position_returns_inserted_coordinates() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        let id = s.add(1.0, 2.0, 3.0);
        assert_eq!(s.get_position(id), Some([1.0, 2.0, 3.0]));
    }

    #[test]
    fn store_get_position_returns_none_for_unknown_id() {
        let s = VertexStore::with_capacity_no_dedup(2);
        assert_eq!(s.get_position(42), None);
    }

    #[test]
    fn store_add_with_params_stores_uv_and_sets_on_face() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let id = s.add_with_params(0.0, 0.0, 0.0, 0.5, 0.25);
        let v = s.get(id).expect("vertex");
        assert_eq!(v.params, [0.5, 0.25]);
        assert!(v.has_flag(VertexFlags::ON_FACE));
    }

    #[test]
    fn store_default_add_has_nan_uv_params() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        let id = s.add(0.0, 0.0, 0.0);
        let v = s.get(id).expect("vertex");
        assert!(v.params[0].is_nan() && v.params[1].is_nan());
    }

    // ---- Deduplication ------------------------------------------------------

    #[test]
    fn store_add_or_find_returns_existing_within_tolerance() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-6);
        let a = s.add_or_find(1.0, 2.0, 3.0, 1e-6);
        let b = s.add_or_find(1.0 + 1e-9, 2.0, 3.0, 1e-6);
        assert_eq!(a, b, "near-duplicate within tolerance must reuse id");
        assert_eq!(s.stats.duplicates_found, 1);
    }

    #[test]
    fn store_add_or_find_creates_new_when_outside_tolerance() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-6);
        let a = s.add_or_find(0.0, 0.0, 0.0, 1e-6);
        let b = s.add_or_find(1.0, 0.0, 0.0, 1e-6);
        assert_ne!(a, b);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn store_add_or_find_uses_squared_distance_threshold() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-3);
        let a = s.add_or_find(0.0, 0.0, 0.0, 1e-3);
        // ~0.0007 distance — strictly inside the 1e-3 ball.
        let b = s.add_or_find(4e-4, 4e-4, 4e-4, 1e-3);
        assert_eq!(a, b);
    }

    #[test]
    fn store_add_or_find_with_dedup_disabled_when_tolerance_too_tight() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-12);
        let a = s.add_or_find_with_dedup(0.0, 0.0, 0.0, 1e-12);
        let b = s.add_or_find_with_dedup(0.0, 0.0, 0.0, 1e-12);
        assert_ne!(a, b, "tolerance < 1e-10 disables dedup path");
    }

    #[test]
    fn store_add_or_find_with_dedup_finds_via_spatial_hash() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-3);
        let a = s.add_or_find_with_dedup(0.0, 0.0, 0.0, 1e-3);
        let b = s.add_or_find_with_dedup(1e-7, -1e-7, 0.0, 1e-3);
        assert_eq!(a, b);
    }

    #[test]
    fn store_add_or_find_batch_dedups_consecutive_inserts() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-3);
        let positions = [(0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (1.0, 0.0, 0.0)];
        let ids = s.add_or_find_batch(&positions, 1e-3);
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], ids[1]);
        assert_ne!(ids[0], ids[2]);
    }

    #[test]
    fn store_add_or_find_skips_deleted_vertices() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-3);
        let a = s.add_or_find(0.0, 0.0, 0.0, 1e-3);
        assert!(s.remove(a));
        let b = s.add_or_find(0.0, 0.0, 0.0, 1e-3);
        assert_ne!(a, b, "after delete, the slot must not be reused as a hit");
    }

    // ---- Tolerance accessors ------------------------------------------------

    #[test]
    fn store_set_tolerance_persists_per_vertex() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let id = s.add(0.0, 0.0, 0.0);
        assert!(s.set_tolerance(id, 5e-9));
        assert!((s.get_tolerance(id).expect("tol") - 5e-9).abs() < 1e-20);
    }

    #[test]
    fn store_set_tolerance_returns_false_for_unknown_id() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        assert!(!s.set_tolerance(42, 1e-9));
    }

    #[test]
    fn store_get_tolerance_none_for_out_of_range_id() {
        let s = VertexStore::with_capacity_no_dedup(4);
        assert!(s.get_tolerance(0).is_none());
    }

    // ---- Spatial queries ----------------------------------------------------

    #[test]
    fn store_find_in_box_returns_only_contained_vertices() {
        let mut s = VertexStore::with_capacity_no_dedup(8);
        let inside = s.add(0.5, 0.5, 0.5);
        let _outside = s.add(2.0, 2.0, 2.0);
        let on_min_corner = s.add(0.0, 0.0, 0.0);
        let on_max_corner = s.add(1.0, 1.0, 1.0);
        let hits = s.find_in_box(&Point3::new(0.0, 0.0, 0.0), &Point3::new(1.0, 1.0, 1.0));
        assert!(hits.contains(&inside));
        assert!(hits.contains(&on_min_corner));
        assert!(hits.contains(&on_max_corner));
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn store_find_in_box_empty_for_disjoint_query() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        s.add(10.0, 10.0, 10.0);
        let hits = s.find_in_box(&Point3::new(0.0, 0.0, 0.0), &Point3::new(1.0, 1.0, 1.0));
        assert!(hits.is_empty());
    }

    // ---- transform_batch ----------------------------------------------------

    #[test]
    fn store_transform_batch_applies_translation() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let a = s.add(1.0, 2.0, 3.0);
        let b = s.add(0.0, 0.0, 0.0);
        let m = Matrix4::translation(10.0, 20.0, 30.0);
        s.transform_batch(&[a, b], &m);
        assert_eq!(s.get_position(a), Some([11.0, 22.0, 33.0]));
        assert_eq!(s.get_position(b), Some([10.0, 20.0, 30.0]));
    }

    #[test]
    fn store_set_position_marks_modified_flag() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        let id = s.add(0.0, 0.0, 0.0);
        assert!(s.set_position(id, 1.0, 2.0, 3.0));
        let v = s.get(id).expect("vertex");
        assert_eq!(v.position, [1.0, 2.0, 3.0]);
        assert!(v.has_flag(VertexFlags::MODIFIED));
    }

    #[test]
    fn store_set_position_returns_false_for_unknown_id() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        assert!(!s.set_position(99, 0.0, 0.0, 0.0));
    }

    // ---- merge / remove / compact ------------------------------------------

    #[test]
    fn store_merge_vertices_marks_remove_as_deleted() {
        let mut s = VertexStore::with_capacity_and_tolerance(4, 1e-3);
        let keep = s.add_or_find_with_dedup(0.0, 0.0, 0.0, 1e-3);
        let remove = s.add_or_find_with_dedup(1.0, 0.0, 0.0, 1e-3);
        assert!(s.merge_vertices(keep, remove));
        assert!(
            s.get(remove).is_none(),
            "removed vertex must not be retrievable"
        );
        assert!(s.get(keep).is_some());
    }

    #[test]
    fn store_merge_vertices_returns_false_for_invalid_ids() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        let v = s.add(0.0, 0.0, 0.0);
        assert!(!s.merge_vertices(v, 99));
        assert!(!s.merge_vertices(99, v));
    }

    #[test]
    fn store_remove_marks_deleted_and_excludes_from_iter() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let a = s.add(0.0, 0.0, 0.0);
        let b = s.add(1.0, 0.0, 0.0);
        let c = s.add(2.0, 0.0, 0.0);
        assert!(s.remove(b));
        let live: Vec<_> = s.iter().map(|(id, _)| id).collect();
        assert!(live.contains(&a));
        assert!(!live.contains(&b));
        assert!(live.contains(&c));
    }

    #[test]
    fn store_remove_returns_false_for_unknown_id() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        assert!(!s.remove(99));
    }

    #[test]
    fn store_compact_removes_deleted_and_returns_remap() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-3);
        let a = s.add(0.0, 0.0, 0.0);
        let b = s.add(1.0, 0.0, 0.0);
        let c = s.add(2.0, 0.0, 0.0);
        s.remove(b);
        let remap = s.compact();
        // Live vertices remap to dense indices.
        assert_eq!(remap.get(&a).map(|r| *r), Some(0));
        assert_eq!(remap.get(&c).map(|r| *r), Some(1));
        // Deleted vertex has no remap entry.
        assert!(remap.get(&b).is_none());
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn store_compact_preserves_position_under_remap() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let a = s.add(7.0, 8.0, 9.0);
        let b = s.add(1.0, 2.0, 3.0);
        s.remove(a);
        let remap = s.compact();
        let new_b = *remap.get(&b).expect("b must remap");
        assert_eq!(s.get_position(new_b), Some([1.0, 2.0, 3.0]));
    }

    // ---- iter / len / is_empty ---------------------------------------------

    #[test]
    fn store_len_excludes_deleted() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let a = s.add(0.0, 0.0, 0.0);
        let _b = s.add(1.0, 0.0, 0.0);
        s.remove(a);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn store_is_empty_initially_and_after_full_deletion() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        assert!(s.is_empty());
        let id = s.add(0.0, 0.0, 0.0);
        assert!(!s.is_empty());
        s.remove(id);
        assert!(s.is_empty());
    }

    #[test]
    fn store_iter_visits_each_live_vertex_once() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        s.add(0.0, 0.0, 0.0);
        s.add(1.0, 0.0, 0.0);
        s.add(2.0, 0.0, 0.0);
        assert_eq!(s.iter().count(), 3);
    }

    // ---- Attributes ---------------------------------------------------------

    #[test]
    fn store_attributes_round_trip() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        let id = s.add(0.0, 0.0, 0.0);
        let attrs = vec![
            VertexAttribute::Color([1.0, 0.5, 0.25, 1.0]),
            VertexAttribute::Selected(true),
        ];
        s.set_attributes(id, attrs.clone());
        let got = s.get_attributes(id).expect("attrs");
        assert_eq!(got, attrs);
    }

    #[test]
    fn store_attributes_none_for_unknown_vertex() {
        let s = VertexStore::with_capacity_no_dedup(2);
        assert!(s.get_attributes(99).is_none());
    }

    // ---- Spatial-hash key ---------------------------------------------------

    #[test]
    fn spatial_hash_key_collides_within_grid_cell() {
        let k1 = SpatialHashKey::from_position(0.1, 0.1, 0.1, 1.0);
        let k2 = SpatialHashKey::from_position(0.4, -0.4, 0.49, 1.0);
        assert_eq!(k1, k2);
    }

    #[test]
    fn spatial_hash_key_differs_across_cells() {
        let k1 = SpatialHashKey::from_position(0.0, 0.0, 0.0, 1.0);
        let k2 = SpatialHashKey::from_position(1.0, 0.0, 0.0, 1.0);
        assert_ne!(k1, k2);
    }

    // ---- Edge / numerical sanity -------------------------------------------

    #[test]
    fn store_handles_negative_zero_as_zero() {
        let mut s = VertexStore::with_capacity_and_tolerance(2, 1e-9);
        let a = s.add_or_find(0.0, 0.0, 0.0, 1e-9);
        let b = s.add_or_find(-0.0, -0.0, -0.0, 1e-9);
        assert_eq!(a, b);
    }

    #[test]
    fn store_handles_very_large_coordinates() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        let id = s.add(1e15, -1e15, 1e15);
        assert_eq!(s.get_position(id), Some([1e15, -1e15, 1e15]));
    }

    #[test]
    fn store_reserve_does_not_change_len() {
        let mut s = VertexStore::with_capacity_no_dedup(0);
        s.reserve(128);
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }

    // ---- Per-vertex tolerance (F1-α) ---------------------------------------

    #[test]
    fn add_unchecked_with_tolerance_stamps_supplied_value() {
        let mut s = VertexStore::with_capacity_no_dedup(2);
        let id = s.add_unchecked_with_tolerance(0.0, 0.0, 0.0, 5e-4);
        let tol = s.get_tolerance(id).expect("vertex must exist");
        assert!((tol - 5e-4).abs() < 1e-15);
    }

    #[test]
    fn add_or_find_stamps_caller_tolerance_on_new_vertex() {
        let mut s = VertexStore::with_capacity_and_tolerance(2, 1e-6);
        let id = s.add_or_find(1.0, 2.0, 3.0, 5e-8);
        let tol = s.get_tolerance(id).expect("vertex must exist");
        assert!((tol - 5e-8).abs() < 1e-18);
    }

    #[test]
    fn add_or_find_uses_max_of_stored_and_caller_for_coincidence() {
        // A loose vertex with stored tolerance 1e-3 is queried with a
        // tight caller tolerance 1e-9; per Parasolid's union-of-spheres
        // convention, the merged radius is 1e-3 so a point 1e-4 away
        // must snap to the existing vertex.
        let mut s = VertexStore::with_capacity_and_tolerance(2, 1e-3);
        let loose_id = s.add_unchecked_with_tolerance(0.0, 0.0, 0.0, 1e-3);
        let snap_id = s.add_or_find(1e-4, 0.0, 0.0, 1e-9);
        assert_eq!(loose_id, snap_id, "tight query must snap onto loose vertex");
    }

    #[test]
    fn add_or_find_creates_new_vertex_when_outside_merged_ball() {
        // Stored 1e-9 + caller 1e-9 → coincidence radius 1e-9; a point
        // 1e-6 away must not snap.
        let mut s = VertexStore::with_capacity_and_tolerance(2, 1e-9);
        let a = s.add_unchecked_with_tolerance(0.0, 0.0, 0.0, 1e-9);
        let b = s.add_or_find(1e-6, 0.0, 0.0, 1e-9);
        assert_ne!(a, b);
    }

    #[test]
    fn add_or_find_with_dedup_respects_per_vertex_tolerance() {
        let mut s = VertexStore::with_capacity_and_tolerance(8, 1e-3);
        let loose_id = s.add_unchecked_with_tolerance(0.0, 0.0, 0.0, 1e-3);
        // Re-insert spatial-hash key so the dedup path can find it.
        let key = SpatialHashKey::from_position(0.0, 0.0, 0.0, s.grid_size);
        s.spatial_hash.insert(key, vec![loose_id]);
        let snap_id = s.add_or_find_with_dedup(1e-4, 0.0, 0.0, 1e-9);
        assert_eq!(loose_id, snap_id);
    }

    #[test]
    fn add_or_find_batch_stamps_caller_tolerance() {
        let mut s = VertexStore::with_capacity_and_tolerance(4, 1e-6);
        let positions = [(1.0, 0.0, 0.0), (2.0, 0.0, 0.0)];
        let ids = s.add_or_find_batch(&positions, 5e-7);
        for id in ids {
            let tol = s.get_tolerance(id).expect("vertex must exist");
            assert!((tol - 5e-7).abs() < 1e-18);
        }
    }

    #[test]
    fn compact_preserves_tolerances() {
        let mut s = VertexStore::with_capacity_no_dedup(4);
        let _a = s.add_unchecked_with_tolerance(0.0, 0.0, 0.0, 1e-9);
        let b = s.add_unchecked_with_tolerance(1.0, 0.0, 0.0, 1e-3);
        let _c = s.add_unchecked_with_tolerance(2.0, 0.0, 0.0, 1e-6);
        // Remove the middle vertex; compact must keep the tolerances on
        // the surviving vertices intact and not leak the stale slot.
        assert!(s.remove(b));
        let _remap = s.compact();
        let surviving: Vec<f64> = s
            .iter()
            .map(|(id, _)| s.get_tolerance(id).unwrap_or(0.0))
            .collect();
        assert_eq!(surviving.len(), 2);
        assert!(surviving.iter().any(|&t| (t - 1e-9).abs() < 1e-18));
        assert!(surviving.iter().any(|&t| (t - 1e-6).abs() < 1e-18));
        // tolerances Vec is also truncated, not just the coordinate arrays.
        assert_eq!(s.tolerances.len(), 2);
    }
}
