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
/// Slice 1 stores datums at the world origin with no transform field —
/// that comes in Slice 2 when datums become anchor targets for primitives
/// and need to support translated / rotated user datums.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Datum {
    /// Stable id within the model's `DatumStore`.
    pub id: DatumId,
    /// User-visible name (e.g. "Origin", "FrontPlane", "X-Axis").
    pub name: String,
    /// Geometric kind.
    pub kind: DatumKind,
    /// World-space anchor point.
    pub origin: Point3,
    /// Whether this datum is rendered in the viewport.
    pub visible: bool,
    /// Whether this datum was seeded automatically by `seed_defaults`.
    /// Default datums cannot be deleted (they can be hidden via
    /// `visible = false`); user-authored datums (Slice 3) carry
    /// `is_default = false`.
    pub is_default: bool,
}

impl Datum {
    /// Local-to-world transform for this datum's reference frame.
    ///
    /// The frame's origin is the datum's `origin`. The frame's local +Z
    /// is the canonical "out" direction:
    ///
    /// - `Origin`              → identity rotation (axes match world).
    /// - `Plane(XY)` / Z-axis  → identity rotation (normal is world +Z).
    /// - `Plane(XZ)` / Y-axis  → rotate -π/2 around world X
    ///                           (local +Z maps to world +Y).
    /// - `Plane(YZ)` / X-axis  → rotate +π/2 around world Y
    ///                           (local +Z maps to world +X).
    /// - `Plane(Custom)`       → identity rotation (placeholder; real
    ///                           user-authored custom planes ship in
    ///                           Slice 3 and will carry their own
    ///                           orientation matrix on `Datum`).
    ///
    /// Used by primitive anchoring (Slice 2) to compose a world-space
    /// placement transform: `world = datum.frame() * local_transform`.
    pub fn frame(&self) -> Matrix4 {
        let translation = Matrix4::from_translation(&Vector3::new(
            self.origin.x,
            self.origin.y,
            self.origin.z,
        ));
        let rotation = match self.kind {
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
        let datum = Datum {
            id,
            name: name.to_string(),
            kind,
            origin: Point3::ORIGIN,
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
}
