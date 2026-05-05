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
use crate::sketch2d::sketch_plane::PlaneOrientation;
use std::f64::consts::FRAC_PI_2;
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
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
}

/// Stable identifier for a datum within a `DatumStore`.
pub type DatumId = u32;

/// Sentinel for an unset / not-yet-allocated `DatumId`.
pub const INVALID_DATUM_ID: DatumId = u32::MAX;

/// Direction of a reference axis datum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AxisDirection {
    /// World +X axis.
    X,
    /// World +Y axis.
    Y,
    /// World +Z axis.
    Z,
}

impl AxisDirection {
    /// Unit vector for this axis in world space.
    pub fn unit_vector(self) -> Vector3 {
        match self {
            AxisDirection::X => Vector3::X,
            AxisDirection::Y => Vector3::Y,
            AxisDirection::Z => Vector3::Z,
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
        let translation =
            Matrix4::from_translation(&Vector3::new(origin.x, origin.y, origin.z));
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let datum = Datum {
            id,
            name: name.to_string(),
            kind,
            origin,
            transform: Datum::canonical_transform(origin, kind),
            visible: true,
            is_default: true,
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
        let datum = Datum {
            id,
            name,
            kind,
            origin,
            transform: Datum::canonical_transform(origin, kind),
            visible: true,
            is_default: false,
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
        let datum = Datum {
            id,
            name,
            kind: DatumKind::Origin,
            origin: position,
            transform: Datum::canonical_transform(position, DatumKind::Origin),
            visible: true,
            is_default: false,
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
    pub fn set_transform(
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
        Ok(prev)
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
        let (_id, removed) = self
            .datums
            .remove(&id)
            .ok_or(DatumError::UnknownId(id))?;
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
        let translation =
            Matrix4::from_translation(&Vector3::new(10.0, 20.0, 30.0));
        let id = store
            .create_plane("CustomPlane".to_string(), translation)
            .expect("plane creation succeeds");
        assert_eq!(id, 7, "first user datum gets the next id after seven defaults");
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
        assert_eq!(
            store.rename(id, "".to_string()),
            Err(DatumError::EmptyName)
        );
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
