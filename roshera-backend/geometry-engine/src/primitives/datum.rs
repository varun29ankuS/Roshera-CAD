//! Datum entities — Origin, reference planes, and reference axes.
//!
//! Datums are first-class kernel entities that provide canonical reference
//! frames for primitive placement, dimensions, and AI-driven design queries.
//! Unlike sketch planes (which are workplane-scoped 2D coordinate systems
//! for sketch authoring), datums are world-scoped, persistent across
//! operations, and addressable by stable id.
//!
//! Every model seeds seven default datums: the world origin, three
//! orthogonal reference planes (Front=XY / Top=XZ / Right=YZ), and three
//! reference axes (X / Y / Z). Default datums are flagged
//! `is_default: true` and are not user-deletable. User-authored datums
//! (named planes, axes, points) are introduced in Slice 3.

use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::vertex::VertexId;
use crate::sketch2d::sketch_plane::PlaneOrientation;
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::f64::consts::FRAC_PI_2;
use thiserror::Error;

/// Errors raised by `DatumStore` mutation methods.
///
/// `Create*` paths can return `EmptyName`. Mutation paths
/// (`rename`, `set_transform`, `delete`) can return `UnknownId` when the
/// datum has been removed since the caller looked it up, or
/// `DefaultDatumNotMutable` when the caller targets one of the seven
/// seeded defaults — defaults are an invariant of the baseline model
/// and cannot be renamed, transformed, or deleted (they can be hidden
/// via `set_visible`).
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DatumError {
    /// No datum with the supplied id exists in the store.
    #[error("datum id {0} not found")]
    UnknownId(DatumId),
    /// The targeted datum has `is_default = true` and cannot be mutated.
    #[error("datum id {0} is a seeded default and cannot be renamed, transformed, or deleted")]
    DefaultDatumNotMutable(DatumId),
    /// A user-supplied name was empty or whitespace-only.
    #[error("datum name must not be empty")]
    EmptyName,
    /// A `DatumSource` referenced a vertex / edge / face / datum that
    /// could not be resolved against the current model. Carries the
    /// reference category and id for diagnostics.
    #[error("datum source references unknown {kind} id {id}")]
    UnknownReference {
        /// Category of the missing reference (`"vertex"`, `"edge"`,
        /// `"face"`, `"datum"`).
        kind: &'static str,
        /// Numeric id of the missing reference.
        id: u64,
    },
    /// A `DatumSource` resolved its inputs but the resulting geometry
    /// is degenerate (e.g. three collinear points, two coincident
    /// points for an axis, zero-length edge tangent).
    #[error("datum source produced degenerate geometry: {0}")]
    DegenerateSource(&'static str),
    /// A geometry evaluation needed by a `DatumSource` (face normal,
    /// edge tangent, …) returned a numerical error from the kernel.
    #[error("datum source evaluation failed: {0}")]
    EvaluationFailed(String),
}

/// Stable identifier for a datum within a `DatumStore`.
pub type DatumId = u32;

/// Sentinel for an unset / not-yet-allocated `DatumId`.
pub const INVALID_DATUM_ID: DatumId = u32::MAX;

/// Direction of a reference axis datum.
///
/// `X / Y / Z` cover the three canonical world axes used by the seeded
/// defaults and slice 4a manual axis authoring. `Custom` is reserved for
/// slice 4b derived axes (`EdgeAxis`, `TwoPointsAxis`, `NormalAxis`) whose
/// direction is recovered from referenced geometry rather than declared
/// up-front; the canonical direction is then carried by the datum's
/// `transform` (local +Z) and the variant tag is `Custom` so the API
/// surface and UI know they are looking at a derived axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AxisDirection {
    /// World +X axis.
    X,
    /// World +Y axis.
    Y,
    /// World +Z axis.
    Z,
    /// Direction not aligned with a world axis. The actual direction is
    /// the local +Z column of the datum's `transform` field — kept off
    /// the enum so two derived axes with slightly different directions
    /// remain distinct datums without expanding the enum to carry
    /// floating-point payload.
    Custom,
}

impl AxisDirection {
    /// Unit vector for this axis in world space. `Custom` returns
    /// `Vector3::Z` as a safe placeholder — derived axes carry their
    /// actual direction in `Datum::transform`'s local +Z column, so
    /// this value is only ever used by the canonical seeding path
    /// (which never produces `Custom`).
    pub fn unit_vector(self) -> Vector3 {
        match self {
            AxisDirection::X => Vector3::X,
            AxisDirection::Y => Vector3::Y,
            AxisDirection::Z => Vector3::Z,
            AxisDirection::Custom => Vector3::Z,
        }
    }
}

/// What geometric reference a datum represents.
///
/// `Origin` is a point. `Plane(_)` is an infinite plane through the
/// datum origin. `Axis(_)` is an infinite line through the datum origin.
/// Anchoring (Slice 2) and dimensions (Slice 3) reference datums by id +
/// kind, never by raw world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatumKind {
    /// A point datum (zero-dimensional reference).
    Origin,
    /// A plane datum oriented along one of the standard axes pairs.
    Plane(PlaneOrientation),
    /// An axis datum aligned with X, Y, or Z.
    Axis(AxisDirection),
}

/// Recipe describing where a datum's `transform` came from.
///
/// Slice 4a (manual authoring) only ever produces `Manual` sources — the
/// transform is supplied directly by the user. Slice 4b adds derived
/// sources whose `transform` is recomputed from referenced model
/// geometry: when a referenced vertex moves, face splits, or edge gets
/// swallowed by a Boolean, the propagation graph in slice 5 walks the
/// dependents, calls `BRepModel::evaluate_datum_source`, and writes the
/// fresh transform back via `set_datum_transform`.
///
/// The variants are exhaustive over the recipes the UI can build by
/// inspecting a single user selection (vertex / edge / face / datum
/// combinations); arbitrary scripted constructions go through
/// `Manual` with a pre-computed `Matrix4`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatumSource {
    /// Transform supplied directly. The slice 4a authoring path
    /// (`create_plane / create_axis / create_point`) and the seven
    /// seeded defaults all carry this variant.
    Manual { transform: [[f64; 4]; 4] },
    /// Plane parallel to `base`, translated `distance` along `base`'s
    /// local +Z (the plane normal). Negative `distance` flips the
    /// offset side.
    OffsetPlane { base: DatumId, distance: f64 },
    /// Plane through `base`'s origin, rotated by `angle` radians around
    /// `axis`. `axis` must be an `Axis(_)` datum; `base` must be a
    /// `Plane(_)` datum.
    AnglePlane {
        base: DatumId,
        axis: DatumId,
        angle: f64,
    },
    /// Plane through three vertices. Origin is `p0`; local +X points
    /// toward `p1`; local +Z is the cross product of `(p1 - p0)` and
    /// `(p2 - p0)` (right-hand rule). Degenerate (collinear) triples
    /// raise [`DatumError::DegenerateSource`].
    ThreePoints {
        p0: VertexId,
        p1: VertexId,
        p2: VertexId,
    },
    /// Plane whose origin is the face's loop centroid and whose
    /// normal matches the face's surface normal at that centroid.
    PlaneFromFace { face: FaceId },
    /// Plane bisecting two parent planes. Origin is the midpoint of
    /// their origins; normal is the unit average of their normals.
    /// Antiparallel parent normals raise
    /// [`DatumError::DegenerateSource`].
    MidPlane { a: DatumId, b: DatumId },
    /// Axis along an edge. Origin is the edge midpoint
    /// (`edge.point_at(0.5)`); direction is the tangent at that
    /// parameter.
    EdgeAxis { edge: EdgeId },
    /// Axis through two vertices. Origin is the midpoint; direction
    /// is `(p1 - p0).normalize()`. Coincident vertices raise
    /// [`DatumError::DegenerateSource`].
    TwoPointsAxis { p0: VertexId, p1: VertexId },
    /// Axis perpendicular to a plane datum, passing through a vertex.
    /// Origin is the vertex position; direction is the plane's normal.
    NormalAxis { plane: DatumId, point: VertexId },
    /// Point datum at a vertex.
    VertexPoint { vertex: VertexId },
    /// Point datum at the midpoint of an edge.
    CurveMidpoint { edge: EdgeId },
    /// Point datum at the centroid of a face's outer loop.
    FaceCentroid { face: FaceId },
}

impl DatumSource {
    /// Pack a `Matrix4` into the `[[f64;4];4]` layout used in the
    /// serialized `Manual` variant. Row-major (matches
    /// `Matrix4::get(row, col)`).
    pub fn pack_matrix(m: Matrix4) -> [[f64; 4]; 4] {
        [
            [m.get(0, 0), m.get(0, 1), m.get(0, 2), m.get(0, 3)],
            [m.get(1, 0), m.get(1, 1), m.get(1, 2), m.get(1, 3)],
            [m.get(2, 0), m.get(2, 1), m.get(2, 2), m.get(2, 3)],
            [m.get(3, 0), m.get(3, 1), m.get(3, 2), m.get(3, 3)],
        ]
    }

    /// Inverse of [`DatumSource::pack_matrix`]: reconstruct a
    /// `Matrix4` from the row-major payload.
    pub fn unpack_matrix(rows: [[f64; 4]; 4]) -> Matrix4 {
        Matrix4::from_rows_array(rows)
    }

    /// Build a `Manual` source from a `Matrix4`. Convenience wrapper
    /// over `pack_matrix`.
    pub fn manual(transform: Matrix4) -> Self {
        DatumSource::Manual {
            transform: Self::pack_matrix(transform),
        }
    }

    /// Whether evaluation needs access to model geometry beyond the
    /// `DatumStore`. `false` for `Manual / OffsetPlane / AnglePlane /
    /// MidPlane`; `true` for the vertex / edge / face variants.
    pub fn requires_geometry(&self) -> bool {
        !matches!(
            self,
            DatumSource::Manual { .. }
                | DatumSource::OffsetPlane { .. }
                | DatumSource::AnglePlane { .. }
                | DatumSource::MidPlane { .. }
        )
    }

    /// Datum ids this source depends on (parent datums whose
    /// transforms feed into this datum's evaluation). Used by
    /// [`DatumGraph`] to register forward-edges so a later
    /// `set_datum_transform` on a parent can find this dependent.
    pub fn referenced_datums(&self) -> Vec<DatumId> {
        match *self {
            DatumSource::Manual { .. }
            | DatumSource::ThreePoints { .. }
            | DatumSource::PlaneFromFace { .. }
            | DatumSource::EdgeAxis { .. }
            | DatumSource::TwoPointsAxis { .. }
            | DatumSource::VertexPoint { .. }
            | DatumSource::CurveMidpoint { .. }
            | DatumSource::FaceCentroid { .. } => Vec::new(),
            DatumSource::OffsetPlane { base, .. } => vec![base],
            DatumSource::AnglePlane { base, axis, .. } => vec![base, axis],
            DatumSource::MidPlane { a, b } => vec![a, b],
            DatumSource::NormalAxis { plane, .. } => vec![plane],
        }
    }

    /// Vertex ids this source depends on. Slice 5 propagation invalidates
    /// derived datums whose listed vertex moves (`VertexStore::set_position`).
    pub fn referenced_vertices(&self) -> Vec<VertexId> {
        match *self {
            DatumSource::ThreePoints { p0, p1, p2 } => vec![p0, p1, p2],
            DatumSource::TwoPointsAxis { p0, p1 } => vec![p0, p1],
            DatumSource::NormalAxis { point, .. } => vec![point],
            DatumSource::VertexPoint { vertex } => vec![vertex],
            _ => Vec::new(),
        }
    }

    /// Edge ids this source depends on.
    pub fn referenced_edges(&self) -> Vec<EdgeId> {
        match *self {
            DatumSource::EdgeAxis { edge } | DatumSource::CurveMidpoint { edge } => vec![edge],
            _ => Vec::new(),
        }
    }

    /// Face ids this source depends on.
    pub fn referenced_faces(&self) -> Vec<FaceId> {
        match *self {
            DatumSource::PlaneFromFace { face } | DatumSource::FaceCentroid { face } => vec![face],
            _ => Vec::new(),
        }
    }

    /// Canonical [`DatumKind`] for each source variant — the kind a
    /// derived datum should be tagged with after evaluation.
    /// `Manual` collapses to `Plane(Custom)` because the kind cannot
    /// be inferred from a bare transform; manual authoring should
    /// instead go through `BRepModel::create_datum_{plane,axis,point}`
    /// which set the kind explicitly.
    pub fn default_kind(&self) -> DatumKind {
        match self {
            DatumSource::Manual { .. } => DatumKind::Plane(PlaneOrientation::Custom),
            DatumSource::OffsetPlane { .. }
            | DatumSource::AnglePlane { .. }
            | DatumSource::ThreePoints { .. }
            | DatumSource::PlaneFromFace { .. }
            | DatumSource::MidPlane { .. } => DatumKind::Plane(PlaneOrientation::Custom),
            DatumSource::EdgeAxis { .. }
            | DatumSource::TwoPointsAxis { .. }
            | DatumSource::NormalAxis { .. } => DatumKind::Axis(AxisDirection::Custom),
            DatumSource::VertexPoint { .. }
            | DatumSource::CurveMidpoint { .. }
            | DatumSource::FaceCentroid { .. } => DatumKind::Origin,
        }
    }
}

/// Build a local-to-world `Matrix4` from an origin and a desired
/// local +Z direction, using a deterministic Gram-Schmidt completion to
/// pick an orthonormal +X / +Y basis.
///
/// The +X column is the projection of either `Vector3::X` or
/// `Vector3::Y` (whichever is more orthogonal to `z_dir`) onto the
/// plane perpendicular to `z_dir`, then normalized; +Y is `z × x` so
/// the basis stays right-handed. This matches the convention used by
/// `Datum::canonical_transform` for the seeded defaults (their +X /
/// +Y columns are world basis vectors orthogonal to +Z).
///
/// Returns [`DatumError::DegenerateSource`] when `z_dir` is too small
/// to normalize, when the chosen reference vector is parallel to
/// `z_dir`, or when the resulting +X is degenerate.
pub fn frame_from_origin_and_z(origin: Point3, z_dir: Vector3) -> Result<Matrix4, DatumError> {
    let z = z_dir
        .normalize()
        .map_err(|_| DatumError::DegenerateSource("zero-length normal direction"))?;
    // Pick whichever world basis vector is least parallel to z to keep
    // the cross-product numerically stable.
    let world_ref = if z.x.abs() < 0.9 {
        Vector3::X
    } else {
        Vector3::Y
    };
    let x = world_ref
        .cross(&z)
        .normalize()
        .map_err(|_| DatumError::DegenerateSource("could not derive +X basis from +Z"))?;
    let y = z.cross(&x);
    // Row-major: column 0 = x, column 1 = y, column 2 = z, column 3 = origin.
    Ok(Matrix4::from_rows_array([
        [x.x, y.x, z.x, origin.x],
        [x.y, y.y, z.y, origin.y],
        [x.z, y.z, z.z, origin.z],
        [0.0, 0.0, 0.0, 1.0],
    ]))
}

/// Extract the local +Z basis vector (column 2) from a 4×4 transform.
/// Used by derived plane / axis evaluation to read a parent datum's
/// "out" direction without unpacking the full matrix.
#[inline]
pub fn frame_z_axis(transform: &Matrix4) -> Vector3 {
    Vector3::new(
        transform.get(0, 2),
        transform.get(1, 2),
        transform.get(2, 2),
    )
}

/// Extract the translation (column 3) of a 4×4 transform as a
/// `Point3`. Mirrors the denormalized `Datum::origin` field.
#[inline]
pub fn frame_origin(transform: &Matrix4) -> Point3 {
    Point3::new(
        transform.get(0, 3),
        transform.get(1, 3),
        transform.get(2, 3),
    )
}

/// A first-class reference entity in the kernel.
///
/// As of slice 3d the local-to-world reference frame is stored explicitly
/// in `transform`. `kind` is preserved as a semantic tag (so the frontend
/// and AI tools can distinguish a plane from an axis from a point) but is
/// no longer the source of frame geometry — `frame()` returns `transform`
/// directly. User-authored datums (slice 4) populate `transform` from
/// either a manual `Matrix4` or a `DatumSource` recipe; default datums
/// pre-compute their canonical orientation at seed time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Datum {
    /// Stable id within the model's `DatumStore`.
    pub id: DatumId,
    /// User-visible name (e.g. "Origin", "FrontPlane", "X-Axis").
    pub name: String,
    /// Geometric kind. Semantic tag for UI / agent dispatch — does not
    /// drive the frame.
    pub kind: DatumKind,
    /// World-space anchor point. Equal to the translation column of
    /// `transform`; kept as a denormalized field for readers that want
    /// the position without unpacking the matrix.
    pub origin: Point3,
    /// Local-to-world transform for this datum's reference frame.
    /// Local +Z is the canonical "out" direction (plane normal /
    /// axis direction). The composed world transform applied to an
    /// anchored primitive is `datum.transform * solid.anchor.local_transform`.
    pub transform: Matrix4,
    /// Whether this datum is rendered in the viewport.
    pub visible: bool,
    /// Whether this datum was seeded automatically by `seed_defaults`.
    /// Default datums cannot be deleted (they can be hidden via
    /// `visible = false`); user-authored datums carry `is_default = false`.
    pub is_default: bool,
    /// Recipe describing how `transform` was produced. `Manual` for
    /// the seven seeded defaults, slice 4a manual datums, and any
    /// scripted construction; the other variants describe geometry
    /// dependencies that the slice 5 propagation graph walks on
    /// re-evaluation.
    pub source: DatumSource,
}

impl Datum {
    /// Local-to-world transform for this datum's reference frame.
    /// Returns the cached `transform` field directly.
    ///
    /// Used by primitive anchoring to compose a world-space placement
    /// transform: `world = datum.frame() * local_transform`.
    pub fn frame(&self) -> Matrix4 {
        self.transform
    }

    /// Compose the canonical local-to-world transform for a given
    /// `(origin, kind)` pair. This is the formula used by
    /// `seed_defaults` to populate `transform` for the canonical seven —
    /// keeping it as a free function so user-authored datums can also
    /// derive a starting transform from their declared kind when no
    /// explicit `Matrix4` is supplied.
    ///
    /// Frame conventions (local +Z = canonical "out"):
    /// - `Origin`              → identity rotation.
    /// - `Plane(XY)` / Z-axis  → identity rotation (normal is world +Z).
    /// - `Plane(XZ)` / Y-axis  → rotate -π/2 around world X
    ///                           (local +Z maps to world +Y).
    /// - `Plane(YZ)` / X-axis  → rotate +π/2 around world Y
    ///                           (local +Z maps to world +X).
    /// - `Plane(Custom)`       → identity rotation (placeholder for
    ///                           user-authored custom planes whose
    ///                           orientation is supplied directly).
    pub fn canonical_transform(origin: Point3, kind: DatumKind) -> Matrix4 {
        let translation = Matrix4::from_translation(&Vector3::new(origin.x, origin.y, origin.z));
        let rotation = match kind {
            DatumKind::Origin => Matrix4::identity(),
            DatumKind::Plane(orient) => match orient {
                PlaneOrientation::XY => Matrix4::identity(),
                PlaneOrientation::XZ => Matrix4::rotation_x(-FRAC_PI_2),
                PlaneOrientation::YZ => Matrix4::rotation_y(FRAC_PI_2),
                PlaneOrientation::Custom => Matrix4::identity(),
            },
            DatumKind::Axis(dir) => match dir {
                AxisDirection::X => Matrix4::rotation_y(FRAC_PI_2),
                AxisDirection::Y => Matrix4::rotation_x(-FRAC_PI_2),
                AxisDirection::Z => Matrix4::identity(),
                // `Custom` is never produced by the seeded path; if a
                // caller pushes it through this helper the safe answer
                // is the +Z identity rotation — derived axes always
                // overwrite the resulting transform with a frame
                // computed from referenced geometry anyway.
                AxisDirection::Custom => Matrix4::identity(),
            },
        };
        translation * rotation
    }
}

/// Datum-relative descriptor of where a solid sits in the model.
///
/// This is the agent-facing semantic blob produced by
/// `BRepModel::solid_location_descriptor` — every entry is a numeric
/// fact about the solid's placement that an LLM can read without
/// touching raw vertex data. Distances are in the same units as the
/// kernel (millimeters by convention), angles in radians.
///
/// Conventions:
/// - `center_world`: bbox centroid in world coordinates.
/// - `center_in_anchor_frame`: bbox centroid expressed in the anchor
///   datum's local frame (i.e. world centroid pulled back through
///   `datum.transform.inverse()`).
/// - `dimensions_world`: axis-aligned extents `[dx, dy, dz]`.
/// - `signed_distance_{front,top,right}`: signed perpendicular distance
///   from the canonical reference planes at the world origin
///   (FrontPlane=XY/normal+Z, TopPlane=XZ/normal+Y, RightPlane=YZ/normal+X).
///   Independent of whether those default datums have been hidden or
///   renamed — the canonical mathematical planes are always available
///   as a stable reference frame for agents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocationDescriptor {
    pub solid_id: u32,
    pub anchor_datum_id: DatumId,
    pub anchor_datum_name: String,
    pub center_world: [f64; 3],
    pub center_in_anchor_frame: [f64; 3],
    pub dimensions_world: [f64; 3],
    pub signed_distance_front: f64,
    pub signed_distance_top: f64,
    pub signed_distance_right: f64,
}

/// Concurrent store of datum entities.
///
/// Backed by `DashMap` so reads from the api-server's per-request handlers
/// never block the kernel write path. Per CLAUDE.md hard rule #9
/// (DashMap, not HashMap, for shared state).
#[derive(Debug, Default)]
pub struct DatumStore {
    datums: DashMap<DatumId, Datum>,
    next_id: Mutex<DatumId>,
}

impl DatumStore {
    /// Create an empty store. No default datums are seeded — call
    /// [`DatumStore::seed_defaults`] to install the canonical seven.
    pub fn new() -> Self {
        Self {
            datums: DashMap::new(),
            next_id: Mutex::new(0),
        }
    }

    /// Deep copy of this store for the F2-δ ModelSnapshot primitive.
    /// Each `Datum` derives `Clone`. `next_id` is read under the
    /// existing mutex and reseeded into a fresh mutex so the
    /// snapshot owns its own allocation counter.
    pub(crate) fn deep_copy(&self) -> Self {
        let datums = DashMap::with_capacity(self.datums.len());
        for kv in self.datums.iter() {
            datums.insert(*kv.key(), kv.value().clone());
        }
        Self {
            datums,
            next_id: Mutex::new(*self.next_id.lock()),
        }
    }

    /// Seed the canonical default datums: world Origin, three reference
    /// planes (XY / XZ / YZ), and three reference axes (X / Y / Z).
    ///
    /// Idempotent: when the store is non-empty (e.g. a model that has
    /// already been seeded or has user datums) this is a no-op. Returns
    /// the current datum count.
    pub fn seed_defaults(&self) -> usize {
        if !self.datums.is_empty() {
            return self.datums.len();
        }

        self.insert_default("Origin", DatumKind::Origin);

        // Front / Top / Right naming follows the common mech-CAD
        // convention; SolidWorks, Onshape, and Fusion 360 all use this
        // mapping to the standard XY / XZ / YZ planes respectively.
        self.insert_default("FrontPlane", DatumKind::Plane(PlaneOrientation::XY));
        self.insert_default("TopPlane", DatumKind::Plane(PlaneOrientation::XZ));
        self.insert_default("RightPlane", DatumKind::Plane(PlaneOrientation::YZ));

        self.insert_default("X-Axis", DatumKind::Axis(AxisDirection::X));
        self.insert_default("Y-Axis", DatumKind::Axis(AxisDirection::Y));
        self.insert_default("Z-Axis", DatumKind::Axis(AxisDirection::Z));

        self.datums.len()
    }

    fn insert_default(&self, name: &str, kind: DatumKind) -> DatumId {
        let id = self.allocate_id();
        let origin = Point3::ORIGIN;
        let transform = Datum::canonical_transform(origin, kind);
        let datum = Datum {
            id,
            name: name.to_string(),
            kind,
            origin,
            transform,
            visible: true,
            is_default: true,
            source: DatumSource::manual(transform),
        };
        self.datums.insert(id, datum);
        id
    }

    fn allocate_id(&self) -> DatumId {
        let mut next = self.next_id.lock();
        let id = *next;
        // Saturating to keep the function total even at u32::MAX. In
        // practice a model never reaches this — datum counts are tens,
        // not billions — but production code should not panic on
        // overflow.
        *next = next.saturating_add(1);
        id
    }

    /// Look up a datum by id, returning a clone. Datum payloads are
    /// small (under 100 B); the clone is cheaper than leaking a
    /// `DashMap` ref guard across module boundaries.
    pub fn get(&self, id: DatumId) -> Option<Datum> {
        self.datums.get(&id).map(|entry| entry.value().clone())
    }

    /// Convenience: local-to-world frame of the datum with this id.
    /// Returns `None` when the id is unknown. Equivalent to
    /// `self.get(id).map(|d| d.frame())`.
    pub fn frame(&self, id: DatumId) -> Option<Matrix4> {
        self.datums.get(&id).map(|entry| entry.value().frame())
    }

    /// Set the visibility flag. Returns the previous value, or `None`
    /// when the datum does not exist.
    pub fn set_visible(&self, id: DatumId, visible: bool) -> Option<bool> {
        self.datums.get_mut(&id).map(|mut entry| {
            let prev = entry.visible;
            entry.visible = visible;
            prev
        })
    }

    // ─────────────────────── 4a: user-authored datums ─────────────────────────
    //
    // Create / rename / set_transform / delete are user-driven mutations
    // and route through `BRepModel` mediators (`create_datum_*`,
    // `rename_datum`, `set_datum_transform`, `delete_datum`) so each
    // change emits a `RecordedOperation` for replay / branching. The
    // raw `DatumStore` methods below are unrecorded — they should only
    // be called *via* the mediators in production code; tests can call
    // them directly to exercise edge cases without recorder wiring.

    /// Create a user-authored plane datum from an explicit world-space
    /// transform. The plane's normal is the local +Z basis of
    /// `transform`. The denormalized `origin` field is set from the
    /// translation column.
    ///
    /// Kind is `Plane(Custom)` because user planes are not constrained
    /// to the canonical XY/XZ/YZ orientations.
    pub fn create_plane(&self, name: String, transform: Matrix4) -> Result<DatumId, DatumError> {
        if name.trim().is_empty() {
            return Err(DatumError::EmptyName);
        }
        let id = self.allocate_id();
        let origin = Point3::new(
            transform.get(0, 3),
            transform.get(1, 3),
            transform.get(2, 3),
        );
        let datum = Datum {
            id,
            name,
            kind: DatumKind::Plane(PlaneOrientation::Custom),
            origin,
            transform,
            visible: true,
            is_default: false,
            source: DatumSource::manual(transform),
        };
        self.datums.insert(id, datum);
        Ok(id)
    }

    /// Create a user-authored axis datum aligned with one of the
    /// canonical world directions (X, Y, or Z).
    ///
    /// Slice 4a restricts user axes to the three canonical directions
    /// for two reasons: (1) the wire format and the `Datums.tsx`
    /// renderer already key on `AxisDirection::{X,Y,Z}`; (2) arbitrary
    /// directions are the natural domain of slice 4b's derived datums
    /// (`TwoPointsAxis`, `NormalAxis`), where the direction is recovered
    /// from referenced geometry rather than typed in.
    pub fn create_axis(
        &self,
        name: String,
        origin: Point3,
        direction: AxisDirection,
    ) -> Result<DatumId, DatumError> {
        if name.trim().is_empty() {
            return Err(DatumError::EmptyName);
        }
        let id = self.allocate_id();
        let kind = DatumKind::Axis(direction);
        let transform = Datum::canonical_transform(origin, kind);
        let datum = Datum {
            id,
            name,
            kind,
            origin,
            transform,
            visible: true,
            is_default: false,
            source: DatumSource::manual(transform),
        };
        self.datums.insert(id, datum);
        Ok(id)
    }

    /// Create a user-authored point datum at the given world position.
    /// Stored with `kind = Origin` (the generic point-datum tag — only
    /// the seeded `Origin` is `is_default = true`).
    pub fn create_point(&self, name: String, position: Point3) -> Result<DatumId, DatumError> {
        if name.trim().is_empty() {
            return Err(DatumError::EmptyName);
        }
        let id = self.allocate_id();
        let transform = Datum::canonical_transform(position, DatumKind::Origin);
        let datum = Datum {
            id,
            name,
            kind: DatumKind::Origin,
            origin: position,
            transform,
            visible: true,
            is_default: false,
            source: DatumSource::manual(transform),
        };
        self.datums.insert(id, datum);
        Ok(id)
    }

    /// Rename a user-authored datum. Returns the previous name on
    /// success. Refuses to rename `is_default` datums (the canonical
    /// seven names are part of the agent-facing reference frame
    /// vocabulary).
    pub fn rename(&self, id: DatumId, name: String) -> Result<String, DatumError> {
        if name.trim().is_empty() {
            return Err(DatumError::EmptyName);
        }
        let mut entry = self.datums.get_mut(&id).ok_or(DatumError::UnknownId(id))?;
        if entry.is_default {
            return Err(DatumError::DefaultDatumNotMutable(id));
        }
        Ok(std::mem::replace(&mut entry.name, name))
    }

    /// Replace a user-authored datum's transform. Returns the previous
    /// transform on success. Refuses defaults — moving the canonical
    /// FrontPlane would silently break every signed-distance answer
    /// the agent surface gives.
    ///
    /// The denormalized `origin` field is kept in sync with the
    /// translation column of the new transform.
    pub fn set_transform(&self, id: DatumId, transform: Matrix4) -> Result<Matrix4, DatumError> {
        let mut entry = self.datums.get_mut(&id).ok_or(DatumError::UnknownId(id))?;
        if entry.is_default {
            return Err(DatumError::DefaultDatumNotMutable(id));
        }
        let prev = entry.transform;
        entry.transform = transform;
        entry.origin = Point3::new(
            transform.get(0, 3),
            transform.get(1, 3),
            transform.get(2, 3),
        );
        // A manual `set_transform` overrides whatever recipe was
        // previously in place — this is the user explicitly committing
        // to a hand-set frame. Derived re-evaluation (slice 5) calls
        // a separate `refresh_derived_transform` that preserves the
        // existing source.
        entry.source = DatumSource::manual(transform);
        Ok(prev)
    }

    /// Refresh a datum's `transform` from a freshly-evaluated derived
    /// source, preserving the existing `source` recipe. Used by the
    /// slice 5 propagation graph; not exposed to user mediator paths.
    /// Returns the previous transform on success.
    ///
    /// Refuses defaults (their transform is an invariant) but does
    /// *not* refuse to refresh a `Manual` source — re-evaluating a
    /// manual source is a no-op semantically (the source already
    /// carries the transform), and forbidding it would force callers
    /// to special-case Manual at every refresh site.
    pub fn refresh_derived_transform(
        &self,
        id: DatumId,
        transform: Matrix4,
    ) -> Result<Matrix4, DatumError> {
        let mut entry = self.datums.get_mut(&id).ok_or(DatumError::UnknownId(id))?;
        if entry.is_default {
            return Err(DatumError::DefaultDatumNotMutable(id));
        }
        let prev = entry.transform;
        entry.transform = transform;
        entry.origin = Point3::new(
            transform.get(0, 3),
            transform.get(1, 3),
            transform.get(2, 3),
        );
        // Source recipe is intentionally unchanged.
        Ok(prev)
    }

    /// Insert a datum produced by a derived `DatumSource` recipe.
    /// `transform` is the freshly-evaluated frame (caller is
    /// responsible for evaluating the source); `source` is recorded so
    /// the propagation graph can re-evaluate later.
    ///
    /// Used by `BRepModel::create_derived_datum`; tests can call it
    /// directly to exercise edge cases without recorder wiring.
    pub fn create_derived(
        &self,
        name: String,
        kind: DatumKind,
        transform: Matrix4,
        source: DatumSource,
    ) -> Result<DatumId, DatumError> {
        if name.trim().is_empty() {
            return Err(DatumError::EmptyName);
        }
        let id = self.allocate_id();
        let origin = Point3::new(
            transform.get(0, 3),
            transform.get(1, 3),
            transform.get(2, 3),
        );
        let datum = Datum {
            id,
            name,
            kind,
            origin,
            transform,
            visible: true,
            is_default: false,
            source,
        };
        self.datums.insert(id, datum);
        Ok(id)
    }

    /// Remove a user-authored datum from the store, returning the
    /// removed entity on success. Refuses defaults.
    ///
    /// Slice 4a deletion is shallow — anchored solids that referenced
    /// this datum keep their `anchor.datum_id` pointing at a now-stale
    /// id. The api-server validates dependents at the request layer
    /// (409 unless `?cascade=detach`); tests below also verify the
    /// kernel's behaviour.
    pub fn delete(&self, id: DatumId) -> Result<Datum, DatumError> {
        // Inspect the entry before removing so we can refuse defaults
        // without side effects. `DashMap::remove_if` would also work
        // but does not give us the typed error path.
        {
            let entry = self.datums.get(&id).ok_or(DatumError::UnknownId(id))?;
            if entry.is_default {
                return Err(DatumError::DefaultDatumNotMutable(id));
            }
        }
        let (_id, removed) = self.datums.remove(&id).ok_or(DatumError::UnknownId(id))?;
        Ok(removed)
    }

    /// Snapshot every datum as an owned `Vec` ordered by id. Used by
    /// the api-server's `/api/datums` handler; cheap because typical
    /// models hold under sixteen datums.
    pub fn snapshot(&self) -> Vec<Datum> {
        let mut out: Vec<Datum> = self.datums.iter().map(|e| e.value().clone()).collect();
        out.sort_by_key(|d| d.id);
        out
    }

    /// Number of datums currently in the store.
    pub fn len(&self) -> usize {
        self.datums.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.datums.is_empty()
    }
}

// ────────────────────────────── Slice 5 ─────────────────────────────────────
//
// Forward dependency graph + per-solid LocationDescriptor cache. The
// graph tracks "if upstream X changes, which downstream entities need
// re-evaluation / cache invalidation"; the cache memoizes the
// agent-facing descriptor blob so repeated reads don't walk the outer
// shell every time.
//
// Both structures use `DashMap` for shared-state concurrency per
// CLAUDE.md hard rule #9. Mutation methods take `&self` so they
// compose with the rest of the kernel's `&self` mediator surface.

/// Forward dependency graph keyed by upstream entity id.
///
/// Edges describe "if X changes, the listed dependents need to be
/// re-evaluated / invalidated":
///
/// - `datum_to_datums[d]`: derived datums whose [`DatumSource`]
///   references datum `d` (parent-datum edges).
/// - `datum_to_solids[d]`: solids whose [`crate::primitives::solid::SolidAnchor::datum_id`]
///   is `d` (anchor edges; slice 5 invalidates the location cache for
///   these on a parent move; auto-transform of the solid geometry is
///   deferred to a later sub-slice that takes a `&mut self` lock).
/// - `vertex_to_datums[v]` / `edge_to_datums[e]` / `face_to_datums[f]`:
///   derived datums whose source recipe references geometry. Used by
///   the `vertex_moved` / `edge_changed` / `face_changed` notifiers
///   the topology mutators call after an in-place geometry edit.
///
/// All edge maps allow many-to-many relationships — a single derived
/// datum can list multiple parent datums (e.g. `AnglePlane`) and a
/// single parent can have many dependents.
#[derive(Debug, Default)]
pub struct DatumGraph {
    datum_to_datums: DashMap<DatumId, Vec<DatumId>>,
    datum_to_solids: DashMap<DatumId, Vec<u32>>,
    vertex_to_datums: DashMap<VertexId, Vec<DatumId>>,
    edge_to_datums: DashMap<EdgeId, Vec<DatumId>>,
    face_to_datums: DashMap<FaceId, Vec<DatumId>>,
}

impl DatumGraph {
    /// Empty graph. `BRepModel::new()` constructs a fresh graph next
    /// to its [`DatumStore`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Deep copy of this graph for the F2-δ ModelSnapshot primitive.
    /// Each of the five DashMap indexes is rebuilt entry-by-entry.
    pub(crate) fn deep_copy(&self) -> Self {
        fn copy_u32_map(src: &DashMap<u32, Vec<u32>>) -> DashMap<u32, Vec<u32>> {
            let dst = DashMap::with_capacity(src.len());
            for kv in src.iter() {
                dst.insert(*kv.key(), kv.value().clone());
            }
            dst
        }
        Self {
            datum_to_datums: copy_u32_map(&self.datum_to_datums),
            datum_to_solids: copy_u32_map(&self.datum_to_solids),
            vertex_to_datums: copy_u32_map(&self.vertex_to_datums),
            edge_to_datums: copy_u32_map(&self.edge_to_datums),
            face_to_datums: copy_u32_map(&self.face_to_datums),
        }
    }

    /// Register that `datum_id`'s evaluation depends on every entity
    /// referenced by `source`. Idempotent per (upstream, downstream)
    /// pair: re-registering the same edge does nothing.
    ///
    /// Called by `BRepModel::create_derived_datum` after a successful
    /// `DatumStore::create_derived` insertion.
    pub fn register_source(&self, datum_id: DatumId, source: &DatumSource) {
        for parent in source.referenced_datums() {
            push_unique(&self.datum_to_datums, parent, datum_id);
        }
        for v in source.referenced_vertices() {
            push_unique(&self.vertex_to_datums, v, datum_id);
        }
        for e in source.referenced_edges() {
            push_unique(&self.edge_to_datums, e, datum_id);
        }
        for f in source.referenced_faces() {
            push_unique(&self.face_to_datums, f, datum_id);
        }
    }

    /// Remove every edge that lists `datum_id` as a dependent — i.e.
    /// "this datum no longer depends on anything". Inverse of
    /// `register_source`. Used when a derived datum's source changes
    /// (caller must `register_source` afterwards) and on
    /// `BRepModel::delete_datum`.
    ///
    /// Cost: O(parents × siblings) per upstream, but the typical
    /// derived datum has 1–3 parents and each parent has handful of
    /// dependents — well below any concern for model sizes we target.
    pub fn unregister_dependent(&self, datum_id: DatumId) {
        remove_value_from_all(&self.datum_to_datums, datum_id);
        remove_value_from_all(&self.vertex_to_datums, datum_id);
        remove_value_from_all(&self.edge_to_datums, datum_id);
        remove_value_from_all(&self.face_to_datums, datum_id);
    }

    /// Remove every dependent edge whose upstream is `datum_id`
    /// (other datums or solids that depend on this datum). Used when
    /// a datum is deleted — the dependents are now stale references
    /// and the api-server's cascade-detach path will rebind them to
    /// the world Origin.
    pub fn unregister_upstream_datum(&self, datum_id: DatumId) {
        self.datum_to_datums.remove(&datum_id);
        self.datum_to_solids.remove(&datum_id);
    }

    /// Register that `solid_id` is anchored to `datum_id`. Edge is
    /// idempotent.
    pub fn register_solid_anchor(&self, solid_id: u32, datum_id: DatumId) {
        push_unique(&self.datum_to_solids, datum_id, solid_id);
    }

    /// Remove the (datum → solid) edge. Used when a solid's anchor
    /// is reassigned to a different datum or the solid is deleted.
    pub fn unregister_solid_anchor(&self, solid_id: u32, datum_id: DatumId) {
        if let Some(mut list) = self.datum_to_solids.get_mut(&datum_id) {
            list.retain(|&s| s != solid_id);
        }
    }

    /// Datums whose source recipe references the given parent datum.
    /// Returns an owned `Vec` — typical lengths are 0-4 so the clone
    /// is cheaper than holding a `DashMap` ref guard across the
    /// re-evaluation walk.
    pub fn datums_dependent_on_datum(&self, id: DatumId) -> Vec<DatumId> {
        self.datum_to_datums
            .get(&id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    /// Solids anchored to the given datum.
    pub fn solids_dependent_on_datum(&self, id: DatumId) -> Vec<u32> {
        self.datum_to_solids
            .get(&id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    /// Datums whose source recipe references the given vertex.
    pub fn datums_dependent_on_vertex(&self, id: VertexId) -> Vec<DatumId> {
        self.vertex_to_datums
            .get(&id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    /// Datums whose source recipe references the given edge.
    pub fn datums_dependent_on_edge(&self, id: EdgeId) -> Vec<DatumId> {
        self.edge_to_datums
            .get(&id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    /// Datums whose source recipe references the given face.
    pub fn datums_dependent_on_face(&self, id: FaceId) -> Vec<DatumId> {
        self.face_to_datums
            .get(&id)
            .map(|e| e.value().clone())
            .unwrap_or_default()
    }

    /// Number of (parent-datum, dependent-datum) edges. Test helper.
    pub fn datum_edge_count(&self) -> usize {
        self.datum_to_datums.iter().map(|e| e.value().len()).sum()
    }

    /// Number of (parent-datum, dependent-solid) edges. Test helper.
    pub fn solid_edge_count(&self) -> usize {
        self.datum_to_solids.iter().map(|e| e.value().len()).sum()
    }
}

fn push_unique<K: Eq + std::hash::Hash + Clone, V: Eq + Copy>(
    map: &DashMap<K, Vec<V>>,
    key: K,
    value: V,
) {
    let mut entry = map.entry(key).or_default();
    if !entry.contains(&value) {
        entry.push(value);
    }
}

fn remove_value_from_all<K: Eq + std::hash::Hash + Clone, V: Eq + Copy>(
    map: &DashMap<K, Vec<V>>,
    target: V,
) {
    map.iter_mut().for_each(|mut entry| {
        entry.value_mut().retain(|&v| v != target);
    });
    // Drop now-empty buckets to keep the map size proportional to
    // active dependencies. `retain` accepts FnMut(&K, &mut V) -> bool.
    map.retain(|_, v| !v.is_empty());
}

/// Per-solid memoization of [`LocationDescriptor`].
///
/// Entries are populated lazily by
/// `BRepModel::solid_location_descriptor_cached`: cache miss recomputes
/// via the existing `solid_location_descriptor` walker, cache hit
/// returns a clone (descriptors are ~96 B). Invalidation is explicit —
/// any kernel mutator that affects a solid's bbox or anchor (transforms,
/// boolean ops, anchor reassignment, datum moves on the anchor datum,
/// derived-datum chain that ends at the anchor) must call
/// `invalidate(solid_id)` so the next read reads fresh state.
///
/// Conservative policy: when in doubt, invalidate. The recompute is
/// O(faces × edges) over the outer shell, not catastrophic.
#[derive(Debug, Default)]
pub struct LocationDescriptorCache {
    entries: DashMap<u32, LocationDescriptor>,
}

impl LocationDescriptorCache {
    /// Empty cache. `BRepModel::new()` constructs a fresh cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Deep copy of this cache for the F2-δ ModelSnapshot primitive.
    /// `LocationDescriptor` derives `Clone`; entries are rebuilt one
    /// by one so the snapshot owns its own DashMap.
    pub(crate) fn deep_copy(&self) -> Self {
        let entries = DashMap::with_capacity(self.entries.len());
        for kv in self.entries.iter() {
            entries.insert(*kv.key(), kv.value().clone());
        }
        Self { entries }
    }

    /// Cached descriptor for `solid_id`, or `None` on miss.
    pub fn get(&self, solid_id: u32) -> Option<LocationDescriptor> {
        self.entries.get(&solid_id).map(|e| e.value().clone())
    }

    /// Insert or replace the descriptor for the solid id carried in
    /// `descriptor.solid_id`.
    pub fn insert(&self, descriptor: LocationDescriptor) {
        self.entries.insert(descriptor.solid_id, descriptor);
    }

    /// Drop the cached entry for `solid_id`. Safe to call on a miss.
    pub fn invalidate(&self, solid_id: u32) {
        self.entries.remove(&solid_id);
    }

    /// Drop every cached entry. Used after broad-impact ops (full
    /// model transforms, replay reset) where tracking individual
    /// solids would be more bookkeeping than the recompute is worth.
    pub fn invalidate_all(&self) {
        self.entries.clear();
    }

    /// Number of cached entries. Test helper.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_defaults_installs_seven() {
        let store = DatumStore::new();
        assert_eq!(store.seed_defaults(), 7);
        assert_eq!(store.len(), 7);

        let snapshot = store.snapshot();
        assert_eq!(snapshot.len(), 7);
        assert_eq!(snapshot[0].name, "Origin");
        assert!(matches!(snapshot[0].kind, DatumKind::Origin));
        assert!(snapshot.iter().all(|d| d.is_default));
        assert!(snapshot.iter().all(|d| d.visible));
        assert!(snapshot.iter().all(|d| d.origin == Point3::ORIGIN));
    }

    #[test]
    fn seed_defaults_is_idempotent() {
        let store = DatumStore::new();
        assert_eq!(store.seed_defaults(), 7);
        assert_eq!(store.seed_defaults(), 7);
        assert_eq!(store.len(), 7);
    }

    #[test]
    fn set_visible_toggles_and_returns_previous() {
        let store = DatumStore::new();
        store.seed_defaults();
        let prev = store
            .set_visible(0, false)
            .expect("default origin exists at id 0");
        assert!(prev, "origin starts visible");
        let datum = store.get(0).expect("origin still in store");
        assert!(!datum.visible);
    }

    #[test]
    fn set_visible_returns_none_for_missing_id() {
        let store = DatumStore::new();
        assert!(store.set_visible(99, false).is_none());
    }

    #[test]
    fn axis_direction_unit_vectors() {
        assert_eq!(AxisDirection::X.unit_vector(), Vector3::X);
        assert_eq!(AxisDirection::Y.unit_vector(), Vector3::Y);
        assert_eq!(AxisDirection::Z.unit_vector(), Vector3::Z);
    }

    /// Default datums all sit at the world origin, so each frame's local
    /// +Z basis vector — column 3 of the 4×4 — must point along the
    /// canonical "out" direction for that datum kind.
    #[test]
    fn default_datum_frames_have_expected_normals() {
        let store = DatumStore::new();
        store.seed_defaults();

        // Origin: identity → local +Z = world +Z.
        let origin_frame = store.frame(0).expect("origin id 0");
        assert!((origin_frame.get(0, 2) - 0.0).abs() < 1e-12);
        assert!((origin_frame.get(1, 2) - 0.0).abs() < 1e-12);
        assert!((origin_frame.get(2, 2) - 1.0).abs() < 1e-12);

        // FrontPlane (XY): normal +Z.
        let front = store.frame(1).expect("front plane id 1");
        assert!((front.get(2, 2) - 1.0).abs() < 1e-12);

        // TopPlane (XZ): normal +Y.
        let top = store.frame(2).expect("top plane id 2");
        assert!((top.get(0, 2)).abs() < 1e-12);
        assert!((top.get(1, 2) - 1.0).abs() < 1e-12);
        assert!((top.get(2, 2)).abs() < 1e-12);

        // RightPlane (YZ): normal +X.
        let right = store.frame(3).expect("right plane id 3");
        assert!((right.get(0, 2) - 1.0).abs() < 1e-12);
        assert!((right.get(1, 2)).abs() < 1e-12);
        assert!((right.get(2, 2)).abs() < 1e-12);
    }

    // ───────────────────── 4a: user-authored datum store ───────────────────────

    #[test]
    fn create_plane_assigns_id_and_keeps_origin_in_sync() {
        let store = DatumStore::new();
        store.seed_defaults();
        let translation = Matrix4::from_translation(&Vector3::new(10.0, 20.0, 30.0));
        let id = store
            .create_plane("CustomPlane".to_string(), translation)
            .expect("plane creation succeeds");
        assert_eq!(
            id, 7,
            "first user datum gets the next id after seven defaults"
        );
        let datum = store.get(id).expect("created datum present");
        assert_eq!(datum.name, "CustomPlane");
        assert!(matches!(
            datum.kind,
            DatumKind::Plane(PlaneOrientation::Custom)
        ));
        assert!(!datum.is_default);
        assert!(datum.visible);
        assert!((datum.origin.x - 10.0).abs() < 1e-12);
        assert!((datum.origin.y - 20.0).abs() < 1e-12);
        assert!((datum.origin.z - 30.0).abs() < 1e-12);
    }

    #[test]
    fn create_axis_uses_canonical_transform() {
        let store = DatumStore::new();
        store.seed_defaults();
        let id = store
            .create_axis(
                "MyXAxis".to_string(),
                Point3::new(5.0, 0.0, 0.0),
                AxisDirection::X,
            )
            .expect("axis creation succeeds");
        let datum = store.get(id).expect("created datum present");
        assert!(matches!(datum.kind, DatumKind::Axis(AxisDirection::X)));
        assert!(!datum.is_default);
        // Local +Z (column 2) maps to world +X for an X-axis datum.
        let frame = datum.frame();
        assert!((frame.get(0, 2) - 1.0).abs() < 1e-12);
        // Translation column carries the origin.
        assert!((frame.get(0, 3) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn create_point_stores_position_and_origin_kind() {
        let store = DatumStore::new();
        store.seed_defaults();
        let id = store
            .create_point("ProbePoint".to_string(), Point3::new(1.0, 2.0, 3.0))
            .expect("point creation succeeds");
        let datum = store.get(id).expect("created datum present");
        assert!(matches!(datum.kind, DatumKind::Origin));
        assert!(!datum.is_default);
        assert!((datum.origin.x - 1.0).abs() < 1e-12);
        assert!((datum.origin.y - 2.0).abs() < 1e-12);
        assert!((datum.origin.z - 3.0).abs() < 1e-12);
    }

    #[test]
    fn create_with_empty_name_is_rejected() {
        let store = DatumStore::new();
        assert_eq!(
            store.create_plane("".to_string(), Matrix4::identity()),
            Err(DatumError::EmptyName)
        );
        assert_eq!(
            store.create_plane("   ".to_string(), Matrix4::identity()),
            Err(DatumError::EmptyName)
        );
        assert_eq!(
            store.create_point("".to_string(), Point3::ORIGIN),
            Err(DatumError::EmptyName)
        );
    }

    #[test]
    fn rename_returns_previous_name_for_user_datum() {
        let store = DatumStore::new();
        store.seed_defaults();
        let id = store
            .create_point("Old".to_string(), Point3::ORIGIN)
            .expect("point created");
        let prev = store
            .rename(id, "New".to_string())
            .expect("rename succeeds");
        assert_eq!(prev, "Old");
        assert_eq!(store.get(id).expect("still present").name, "New");
    }

    #[test]
    fn rename_refuses_default_datums() {
        let store = DatumStore::new();
        store.seed_defaults();
        let result = store.rename(0, "NotOrigin".to_string());
        assert_eq!(result, Err(DatumError::DefaultDatumNotMutable(0)));
        // Name unchanged.
        assert_eq!(store.get(0).expect("origin present").name, "Origin");
    }

    #[test]
    fn rename_refuses_unknown_id_and_empty_name() {
        let store = DatumStore::new();
        assert_eq!(
            store.rename(999, "Whatever".to_string()),
            Err(DatumError::UnknownId(999))
        );
        store.seed_defaults();
        let id = store
            .create_point("P".to_string(), Point3::ORIGIN)
            .expect("created");
        assert_eq!(store.rename(id, "".to_string()), Err(DatumError::EmptyName));
    }

    #[test]
    fn set_transform_updates_origin_and_returns_previous() {
        let store = DatumStore::new();
        store.seed_defaults();
        let id = store
            .create_point("P".to_string(), Point3::ORIGIN)
            .expect("created");
        let new_t = Matrix4::from_translation(&Vector3::new(7.0, 8.0, 9.0));
        let prev = store.set_transform(id, new_t).expect("set succeeds");
        // Previous was canonical_transform(ORIGIN, Origin) = identity.
        for r in 0..4 {
            for c in 0..4 {
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!((prev.get(r, c) - expected).abs() < 1e-12);
            }
        }
        let updated = store.get(id).expect("present");
        assert!((updated.origin.x - 7.0).abs() < 1e-12);
        assert!((updated.origin.y - 8.0).abs() < 1e-12);
        assert!((updated.origin.z - 9.0).abs() < 1e-12);
    }

    #[test]
    fn set_transform_refuses_default_datums() {
        let store = DatumStore::new();
        store.seed_defaults();
        let new_t = Matrix4::from_translation(&Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(
            store.set_transform(0, new_t),
            Err(DatumError::DefaultDatumNotMutable(0))
        );
    }

    #[test]
    fn delete_removes_user_datum_and_refuses_defaults() {
        let store = DatumStore::new();
        store.seed_defaults();
        let id = store
            .create_point("Tmp".to_string(), Point3::ORIGIN)
            .expect("created");
        let removed = store.delete(id).expect("delete succeeds");
        assert_eq!(removed.id, id);
        assert!(store.get(id).is_none(), "datum gone after delete");
        assert_eq!(store.len(), 7, "back down to seven defaults");

        assert_eq!(
            store.delete(0).expect_err("default refused"),
            DatumError::DefaultDatumNotMutable(0)
        );
        assert_eq!(
            store.delete(999).expect_err("unknown id"),
            DatumError::UnknownId(999)
        );
    }
}
