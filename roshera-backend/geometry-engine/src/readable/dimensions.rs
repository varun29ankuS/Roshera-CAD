//! EYE / dimensioning: a complete, structured, ANALYTIC dimension table.
//!
//! Where `features.rs` reports per-face feature dims, this assembles the table
//! an agent (or a drawing) wants in one call: overall extents, every bore/boss
//! diameter AND axial length, sphere/cone sizes — each as a [`DimensionRecord`]
//! carrying a stable id, value, the face entities it spans, and a 3D anchor so
//! the callout is recoverable (placeable in any view, queryable, never read off
//! pixels).
//!
//! Honest by construction: every value is read off an analytic surface
//! (downcast) or exact curve geometry — never the tessellation. In particular
//! the overall extents are taken from edge-CURVE samples, not vertices: post-#24
//! a cylinder has a single seam vertex, so a vertex AABB would under-report its
//! true ±radius extent. Non-analytic faces contribute no fabricated size.

use crate::math::{Point3, Vector3};
use crate::primitives::face::Face;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cone, Cylinder, Sphere};
use crate::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An explicit reference datum attached to a position dimension.
///
/// Every position dimension names its reference so a downstream reader
/// (driller, agent, dimension layer) always knows what the number is
/// measured from without out-of-band context.
///
/// `kind` is one of:
/// - `"part_corner"` — the AABB min-corner of the solid projected onto the
///   plane perpendicular to the cylinder axis at the drilled-face elevation
///   (machinist edge convention — the reference any machinist would clamp to).
/// - `"datum"` — a designated kernel `Datum` explicitly bound to the solid.
///   (Reserved for Spec C; not yet emitted — Spec C must wire the
///   datum-to-part association path through `BRepModel::solid_location_descriptor_cached`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatumDescriptor {
    /// `"part_corner"` or `"datum"`.
    pub kind: String,
    /// Human name shown in the callout, e.g. `"part corner"` or the datum's
    /// authored name.
    pub name: String,
    /// World-space position of the reference point (the corner or datum origin).
    pub origin: [f64; 3],
}

/// Project-unique namespace for all Roshera dimension pids.
///
/// Encoded as `6e5a1f00-0000-4000-8000-526f73686572` — the trailing bytes
/// spell "Rosher" in ASCII hex (52=R, 6f=o, 73=s, 68=h, 65=e, 72=r).
///
/// **FROZEN FOREVER.** Changing this constant invalids every pid ever minted;
/// downstream systems that hold a pid will silently lose their references.
const ROSHERA_DIM_NS: Uuid = Uuid::from_u128(0x6e5a_1f00_0000_4000_8000_526f_7368_6572);

/// Derive a stable `pid` for a **feature** dimension (diameter/length/angle).
///
/// Input: the face ids the dimension spans (`entities`) and the dimension
/// `kind` string.  The face pids are looked up in `model.face_pids`; if any
/// entity has no recorded pid the function returns `None` — honest absence,
/// never a fabricated stand-in.
///
/// Derivation: UUIDv5 over `ROSHERA_DIM_NS` + sorted entity pids (as
/// big-endian u128 bytes) + kind bytes, length-prefixed to prevent aliasing.
fn feature_dim_pid(model: &BRepModel, entities: &[u32], kind: &str) -> Option<String> {
    if entities.is_empty() {
        return None;
    }
    // Collect entity pids; bail on any missing.
    let mut entity_pids: Vec<u128> = Vec::with_capacity(entities.len());
    for &fid in entities {
        let pid = model.face_pids.get(&fid)?;
        entity_pids.push(pid.as_u128());
    }
    // Sort for determinism (face-id order is insertion-dependent).
    entity_pids.sort_unstable();

    // Build the hash input: len-prefixed entity pids then len-prefixed kind.
    let kind_bytes = kind.as_bytes();
    let mut buf: Vec<u8> = Vec::with_capacity(4 + entity_pids.len() * 16 + 4 + kind_bytes.len());
    buf.extend_from_slice(&(entity_pids.len() as u32).to_be_bytes());
    for p in entity_pids {
        buf.extend_from_slice(&p.to_be_bytes());
    }
    buf.extend_from_slice(&(kind_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(kind_bytes);

    Some(Uuid::new_v5(&ROSHERA_DIM_NS, &buf).hyphenated().to_string())
}

/// Derive a stable `pid` for a **whole-part extent** dimension.
///
/// Input: the solid's pid (`model.solid_pids`) + `"extent"` + the dominant
/// axis tag (`"x"`, `"y"`, or `"z"` — largest absolute component of
/// `direction`).  Returns `None` when the solid has no recorded pid.
fn extent_dim_pid(model: &BRepModel, solid_id: SolidId, direction: &[f64; 3]) -> Option<String> {
    let solid_pid = model.solid_pids.get(&solid_id)?;

    // Dominant axis of the direction vector.
    let axis_tag =
        if direction[0].abs() >= direction[1].abs() && direction[0].abs() >= direction[2].abs() {
            b"x" as &[u8]
        } else if direction[1].abs() >= direction[2].abs() {
            b"y"
        } else {
            b"z"
        };

    let solid_bytes = solid_pid.as_u128().to_be_bytes();
    // Layout: solid_pid (16 bytes) + "extent" (6) + axis tag (1).
    let mut buf: Vec<u8> = Vec::with_capacity(16 + 6 + 1);
    buf.extend_from_slice(&solid_bytes);
    buf.extend_from_slice(b"extent");
    buf.extend_from_slice(axis_tag);

    Some(Uuid::new_v5(&ROSHERA_DIM_NS, &buf).hyphenated().to_string())
}

/// One recoverable dimension callout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionRecord {
    /// Stable within one extraction (`"d0"`, `"d1"`, …) — the handle a future
    /// `set_dimension` (mould) edits.
    pub id: String,
    /// "diameter" | "radius" | "length" | "angle" | "extent" | "position", or
    /// a blend: "fillet" (value = the radius; `anchor` on its profile arc,
    /// `direction` from the tube centre to it) | "chamfer" (an equal-leg
    /// chamfer; value = the leg; `anchor` on its profile line).
    pub kind: String,
    pub value: f64,
    /// "mm" for lengths, "deg" for angles.
    pub unit: String,
    /// Human label, e.g. "Ø20.00", "L 40.00", "X 110.00", "∠ 30.0°", "R3.00",
    /// "0.50 × 45°".
    pub label: String,
    /// Face ids the dimension spans (empty for the whole-part extents).
    pub entities: Vec<u32>,
    /// World point to anchor the callout / leader line.
    pub anchor: [f64; 3],
    /// World direction the dimension is measured along (unit-ish).
    pub direction: [f64; 3],
    /// Feature axis for axis-bearing features (cylinder/cone): the 3D
    /// direction a drawing view must look along to see the feature as a
    /// true circle. `None` for extents and spheres.
    #[serde(default)]
    pub axis: Option<[f64; 3]>,
    /// Durable, cross-session identity for this dimension, derived via UUIDv5
    /// from the entity PersistentIds it spans (feature dims) or from the solid
    /// PersistentId + axis tag (extents).  `None` when the underlying entities
    /// have no recorded PersistentId (pre-PID solids, or operations not yet
    /// wired in slice 40-B…G).  The per-call `id` (`d0…`) is preserved for
    /// backwards compatibility; `pid` is the durable handle.
    #[serde(default)]
    pub pid: Option<String>,
    /// Reference datum for position dimensions (X/Y offset records).
    ///
    /// `None` for every non-`"position"` kind (diameter, length, angle,
    /// extent) — serde-default guarantees additive backwards compat.
    /// Position records always carry `Some(DatumDescriptor)` so a reader
    /// never sees a position number without knowing its reference.
    #[serde(default)]
    pub datum: Option<DatumDescriptor>,
}

/// Derive a stable `pid` for a **position** dimension (X or Y offset of a
/// cylinder axis from its part-corner datum).
///
/// Input: the face id carrying the cylinder + a suffix tag (`"position_x"` or
/// `"position_y"`).  Returns `None` when the face has no recorded pid.
fn position_dim_pid(model: &BRepModel, face_id: u32, suffix: &str) -> Option<String> {
    let face_pid = model.face_pids.get(&face_id)?;
    let kind_bytes = suffix.as_bytes();
    let mut buf: Vec<u8> = Vec::with_capacity(4 + 16 + 4 + kind_bytes.len());
    // Encode as a length-prefixed single entity so the hash input is
    // structurally identical to `feature_dim_pid` with one entity — preventing
    // aliasing with any future multi-entity position kind.
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&face_pid.as_u128().to_be_bytes());
    buf.extend_from_slice(&(kind_bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(kind_bytes);
    Some(Uuid::new_v5(&ROSHERA_DIM_NS, &buf).hyphenated().to_string())
}

/// Determine the two world-axis indices perpendicular to a dominant cylinder
/// axis, and verify the axis is close enough to world-aligned to give
/// unambiguous position offsets.
///
/// ## Axis selection
///
/// A cylinder axis is "world-aligned" when one of its three world-axis dot
/// products is strictly greater than the other two by a margin of 0.5 (i.e.
/// the dominant component is |axis·e_dominant| ≥ 0.5 + 0.5·others).
///
/// The two perpendicular axes are the two with the *smallest* |axis·e_i|.
///
/// ## Degenerate case
///
/// When no single axis is dominant (e.g. a (1,1,1)/√3 diagonal), position
/// offsets would be measured along arbitrary projections of the AABB — the
/// number would be misleading. In that case the function returns `None` and
/// the caller emits **no** position records (honest absence is safer than a
/// fabricated measurement).
///
/// The dominant axis index (0=X, 1=Y, 2=Z) and the two perpendicular axes
/// (ordered by world-axis index) are returned as `(dominant, [perp0, perp1])`.
fn axis_perpendicular_pair(axis: [f64; 3]) -> Option<(usize, [usize; 2])> {
    let abs = [axis[0].abs(), axis[1].abs(), axis[2].abs()];
    // Find the dominant axis (largest absolute component).
    let dominant = if abs[0] >= abs[1] && abs[0] >= abs[2] {
        0
    } else if abs[1] >= abs[2] {
        1
    } else {
        2
    };
    // Degenerate: the dominant component must be ≥ 0.5 to qualify as
    // "world-aligned enough". For a true (1/√3, 1/√3, 1/√3) diagonal
    // axis each component is ≈ 0.577 — passes this guard. The distinction
    // is made by the SECOND guard below: the dominant component must
    // exceed the second-largest by at least 0.1, so truly diagonal axes
    // (where two or more components are nearly equal) are rejected.
    if abs[dominant] < 0.5 {
        return None;
    }
    // Reject axes where two components are nearly equal in magnitude:
    // for a (1,1,0)/√2 axis the two dominant components are each ≈0.707 —
    // no single perpendicular pair is unambiguous.
    let perps = match dominant {
        0 => [1usize, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let second_largest = abs[perps[0]].max(abs[perps[1]]);
    // Require the dominant component to be meaningfully larger than the
    // second. Threshold 0.1 passes axes up to ~6° off world-aligned and
    // rejects truly diagonal or bi-diagonal axes.
    if abs[dominant] - second_largest < 0.1 {
        return None;
    }
    Some((dominant, perps))
}

/// Min/max world AABB accumulated from sampled points.
struct Aabb {
    min: [f64; 3],
    max: [f64; 3],
    any: bool,
}

impl Aabb {
    fn new() -> Self {
        Aabb {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
            any: false,
        }
    }
    fn add(&mut self, p: [f64; 3]) {
        for i in 0..3 {
            if p[i] < self.min[i] {
                self.min[i] = p[i];
            }
            if p[i] > self.max[i] {
                self.max[i] = p[i];
            }
        }
        self.any = true;
    }
}

/// Every edge id referenced by a solid's faces (outer + inner loops).
fn solid_edges(model: &BRepModel, solid_id: SolidId) -> Vec<u32> {
    let mut edges = Vec::new();
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return edges,
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    for sh in shells {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            let face = match model.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                if let Some(lp) = model.loops.get(lid) {
                    edges.extend_from_slice(&lp.edges);
                }
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    edges
}

/// World AABB from edge-CURVE samples (exact curves, not the mesh / vertices).
fn world_aabb(model: &BRepModel, solid_id: SolidId) -> Option<Aabb> {
    let mut aabb = Aabb::new();
    for eid in solid_edges(model, solid_id) {
        let edge = match model.edges.get(eid) {
            Some(e) => e,
            None => continue,
        };
        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => continue,
        };
        let r = edge.param_range;
        // 48 samples captures a full circle's ±radius extent to <0.1% of r.
        for k in 0..=48 {
            let t = r.start + (r.end - r.start) * (k as f64 / 48.0);
            if let Ok(p) = curve.point_at(t) {
                aabb.add([p.x, p.y, p.z]);
            }
        }
    }

    // Edgeless closed analytic surfaces (a full sphere has NO seam edge, so the
    // edge loop above contributes nothing) — add their exact ±radius envelope
    // so extents stay sound. Cylinders/cones are bounded by cap/base edges and
    // are already covered.
    if let Some(solid) = model.solids.get(solid_id) {
        let mut shells = vec![solid.outer_shell];
        shells.extend_from_slice(&solid.inner_shells);
        shells.extend_from_slice(&solid.peer_shells);
        for sh in shells {
            let shell = match model.shells.get(sh) {
                Some(s) => s,
                None => continue,
            };
            for &fid in &shell.faces {
                let face = match model.faces.get(fid) {
                    Some(f) => f,
                    None => continue,
                };
                if let Some(surf) = model.surfaces.get(face.surface_id) {
                    if let Some(sph) = surf.as_any().downcast_ref::<Sphere>() {
                        let (c, r) = (sph.center, sph.radius);
                        aabb.add([c.x - r, c.y - r, c.z - r]);
                        aabb.add([c.x + r, c.y + r, c.z + r]);
                    }
                }
            }
        }
    }

    if aabb.any {
        Some(aabb)
    } else {
        None
    }
}

/// Face ids of the solid's BORE walls: concave cylindrical faces whose
/// material-out normal points TOWARD the axis.
///
/// # The material-side rule (reused, not invented)
///
/// [`Cylinder`]'s `normal_at` returns the +radial direction (away from the
/// axis) by construction — see `evaluate_full` (`normal = radial.normalize()`).
/// The face's true outward (material-out) normal applies the orientation sign
/// on top, exactly as `queries::raycast::oriented_normal` does:
/// `Forward → +1`, `Backward → −1`. Therefore:
///
/// - **Bore** (cavity wall): material surrounds the cylinder → outward normal
///   points toward the axis → sign −1 → `FaceOrientation::Backward`.
/// - **Boss / OD** (outer silhouette): material fills the cylinder → outward
///   normal points away from the axis → `FaceOrientation::Forward`.
///
/// This is the honest discriminator the hole table needs: `extract_dimensions`
/// emits diameter/length/position records for EVERY cylindrical lateral face,
/// and without this set a flange's own OD would table as a "THRU hole".
pub fn bore_face_ids(model: &BRepModel, solid_id: SolidId) -> std::collections::HashSet<u32> {
    use crate::primitives::face::FaceOrientation;

    let mut out = std::collections::HashSet::new();
    let Some(solid) = model.solids.get(solid_id) else {
        return out;
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let Some(surface) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            if surface.as_any().downcast_ref::<Cylinder>().is_none() {
                continue;
            }
            if face.orientation == FaceOrientation::Backward {
                out.insert(fid);
            }
        }
    }
    out
}

/// True axial extent of a face from its trim EDGES, as `(min, max)` projected
/// onto `axis` relative to `origin`. Used for a cylinder face's real length:
/// the surface's `height_limits` is the *uncut* bound and goes stale after a
/// boolean trims the face, but the rim edges always bound the live face.
fn face_axial_extent(
    model: &BRepModel,
    face: &Face,
    origin: Point3,
    axis: Vector3,
) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut loops = vec![face.outer_loop];
    loops.extend_from_slice(&face.inner_loops);
    for lid in loops {
        let lp = match model.loops.get(lid) {
            Some(l) => l,
            None => continue,
        };
        for &eid in &lp.edges {
            let edge = match model.edges.get(eid) {
                Some(e) => e,
                None => continue,
            };
            let curve = match model.curves.get(edge.curve_id) {
                Some(c) => c,
                None => continue,
            };
            let r = edge.param_range;
            for k in 0..=8 {
                let t = r.start + (r.end - r.start) * (k as f64 / 8.0);
                if let Ok(p) = curve.point_at(t) {
                    let proj = (p.x - origin.x) * axis.x
                        + (p.y - origin.y) * axis.y
                        + (p.z - origin.z) * axis.z;
                    lo = lo.min(proj);
                    hi = hi.max(proj);
                }
            }
        }
    }
    if hi >= lo {
        Some((lo, hi))
    } else {
        None
    }
}

/// The faces bordering each edge of `solid_id`.
fn edge_faces_of(model: &BRepModel, solid_id: SolidId) -> std::collections::HashMap<u32, Vec<u32>> {
    let mut map: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let Some(solid) = model.solids.get(solid_id) else {
        return map;
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                if let Some(lp) = model.loops.get(lid) {
                    for &e in &lp.edges {
                        let faces = map.entry(e).or_default();
                        if !faces.contains(&fid) {
                            faces.push(fid);
                        }
                    }
                }
            }
        }
    }
    map
}

/// The edges of `face`'s loops (each once).
fn face_edges(model: &BRepModel, face: &Face) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
        if let Some(lp) = model.loops.get(lid) {
            for &e in &lp.edges {
                if !out.contains(&e) {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// A cylinder face's axial extent `(lo, hi)` (from its trim edges) carried to
/// the design's SHARP corners: where an end rim meets a blend -- a fillet
/// (torus) or a chamfer (cone) -- the end moves across the blend to the plane
/// square to the axis beyond it. An end with no blend, or a blend with no
/// such plane beyond it, keeps its trimmed station.
fn sharp_axial_extent(
    model: &BRepModel,
    solid_id: SolidId,
    face_id: u32,
    face: &Face,
    origin: Point3,
    axis: Vector3,
    (lo, hi): (f64, f64),
) -> (f64, f64) {
    use crate::primitives::surface::{Plane, Torus};
    let edge_faces = edge_faces_of(model, solid_id);
    let station = |p: Point3| (p - origin).dot(&axis);
    let is_blend = |fid: u32| {
        model
            .faces
            .get(fid)
            .and_then(|f| model.surfaces.get(f.surface_id))
            .is_some_and(|s| {
                s.as_any().downcast_ref::<Torus>().is_some()
                    || s.as_any().downcast_ref::<Cone>().is_some()
            })
    };
    let square_plane_station = |fid: u32| -> Option<f64> {
        let surface = model.surfaces.get(model.faces.get(fid)?.surface_id)?;
        let plane = surface.as_any().downcast_ref::<Plane>()?;
        let n = plane.normal.normalize().ok()?;
        (n.dot(&axis).abs() > 1.0 - 1e-9).then(|| station(plane.origin))
    };
    let (mut slo, mut shi) = (lo, hi);
    for e in face_edges(model, face) {
        let Some(neighbours) = edge_faces.get(&e) else {
            continue;
        };
        // A seam borders only this face.
        let Some(&blend) = neighbours.iter().find(|&&f| f != face_id) else {
            continue;
        };
        if !is_blend(blend) {
            continue;
        }
        let Some(rim_station) = model
            .edges
            .get(e)
            .and_then(|ed| model.vertices.get(ed.start_vertex))
            .map(|v| station(Point3::new(v.position[0], v.position[1], v.position[2])))
        else {
            continue;
        };
        let Some(blend_face) = model.faces.get(blend) else {
            continue;
        };
        let beyond = face_edges(model, blend_face)
            .into_iter()
            .filter(|&e2| e2 != e)
            .filter_map(|e2| edge_faces.get(&e2))
            .flat_map(|fs| fs.iter().copied())
            .filter(|&f| f != blend && f != face_id)
            .find_map(square_plane_station);
        let Some(sharp) = beyond else {
            continue;
        };
        if (rim_station - lo).abs() <= (rim_station - hi).abs() {
            slo = slo.min(sharp);
        } else {
            shi = shi.max(sharp);
        }
    }
    (slo, shi)
}

/// Quantise a `f64` anchor/direction component for coincident-row dedupe.
///
/// `scale` determines the resolution: 0.01 for values (mm), 0.1 for anchors,
/// and 100.0 for direction components (unit vectors ×100 rounded to int).
#[inline]
fn quantise(v: f64, scale: f64) -> i64 {
    (v / scale).round() as i64
}

/// Key used to identify coincident (duplicate) dimension rows.
///
/// Two rows are considered coincident when they have the same:
/// - `kind` string,
/// - `value` quantised to 0.01 mm,
/// - 3D anchor quantised to 0.1 mm per component,
/// - direction quantised to integer components (×100 — unit vectors).
///
/// When coincident rows are found the first is kept; its `entities` list
/// is extended with the merged, sorted, deduped entity ids of all duplicates.
#[derive(Hash, PartialEq, Eq, Clone)]
struct DedupeKey {
    kind: String,
    qvalue: i64,
    qanchor: [i64; 3],
    qdir: [i64; 3],
}

impl DedupeKey {
    fn from_record(d: &DimensionRecord) -> Self {
        DedupeKey {
            kind: d.kind.clone(),
            qvalue: quantise(d.value, 0.01),
            qanchor: [
                quantise(d.anchor[0], 0.1),
                quantise(d.anchor[1], 0.1),
                quantise(d.anchor[2], 0.1),
            ],
            qdir: [
                quantise(d.direction[0] * 100.0, 1.0),
                quantise(d.direction[1] * 100.0, 1.0),
                quantise(d.direction[2] * 100.0, 1.0),
            ],
        }
    }
}

/// Deduplicate coincident dimension rows (in-place).
///
/// A bore whose lateral wall is seam-split into two faces by the boolean
/// emits identical diameter/length/position rows — one per face — with only
/// the `entities` list differing. Merge them: keep the first occurrence's row
/// and extend its `entities` with those from every coincident duplicate
/// (sorted, deduped). The `id` and `pid` of the FIRST occurrence win so that
/// downstream handles remain stable.
fn dedupe_coincident(rows: Vec<DimensionRecord>) -> Vec<DimensionRecord> {
    // Output order = first-occurrence insertion order (the `out` Vec itself);
    // the map only resolves duplicates to their surviving row's index.
    let mut map: std::collections::HashMap<DedupeKey, usize> = std::collections::HashMap::new();
    let mut out: Vec<DimensionRecord> = Vec::new();

    for row in rows {
        let key = DedupeKey::from_record(&row);
        if let Some(&idx) = map.get(&key) {
            // Merge entities into the existing row; sort+dedup once at the
            // end of the merge (O(k log k), not a per-element linear scan).
            out[idx].entities.extend(row.entities);
            out[idx].entities.sort_unstable();
            out[idx].entities.dedup();
        } else {
            let idx = out.len();
            map.insert(key, idx);
            out.push(row);
        }
    }

    out
}

/// Assemble the full analytic dimension table for `solid_id`.
///
/// Labels are formatted via `model.document_unit`: the numeric part is
/// converted from kernel-native mm to the document unit, formatted to the
/// unit's drafting precision, and the unit suffix is appended. Prefixes
/// (Ø, SØ, ∠, L, X, Y, Z) are preserved unchanged.
///
/// Angles always use degrees regardless of `document_unit`.
///
/// Coincident rows (same kind + quantized value + quantized anchor + quantized
/// direction) are merged: the first row is kept and its `entities` list is
/// extended with the entity ids of every duplicate — so a seam-split bore face
/// emits exactly ONE diameter record naming BOTH faces.
pub fn extract_dimensions(model: &BRepModel, solid_id: SolidId) -> Vec<DimensionRecord> {
    let unit = model.document_unit();
    let unit_suffix = unit.suffix();
    let mut out: Vec<DimensionRecord> = Vec::new();
    let mut next = 0usize;
    let mut id = || {
        let s = format!("d{next}");
        next += 1;
        s
    };

    // ── Overall extents (X / Y / Z) from exact edge-curve bounds ──────────────
    // Compute once and reuse for both extent records and position dimensions.
    let bb = world_aabb(model, solid_id);
    if let Some(ref bb) = bb {
        let names = ["X", "Y", "Z"];
        let dirs = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for axis in 0..3 {
            let value = bb.max[axis] - bb.min[axis];
            if value <= 1e-9 {
                continue;
            }
            // Anchor along the lower edge of that axis on the min-min corner.
            let mut anchor = bb.min;
            anchor[axis] = 0.5 * (bb.min[axis] + bb.max[axis]);
            out.push(DimensionRecord {
                id: id(),
                kind: "extent".into(),
                value,
                unit: unit_suffix.into(),
                label: format!("{} {}", names[axis], unit.format_len_bare(value)),
                entities: Vec::new(),
                anchor,
                direction: dirs[axis],
                axis: None,
                pid: extent_dim_pid(model, solid_id, &dirs[axis]),
                datum: None,
            });
        }
    }

    // ── Per analytic surface: bores/bosses, spheres, cones ────────────────────
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return dedupe_coincident(out),
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    for sh in shells {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            let face = match model.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            let any = surface.as_any();
            if let Some(cyl) = any.downcast_ref::<Cylinder>() {
                let axis = cyl.axis.normalize().unwrap_or(cyl.axis);
                // Real axial extent from the face's trim edges (height_limits is
                // the uncut surface bound and stale after a boolean).
                let (lo, hi) = face_axial_extent(model, face, cyl.origin, axis)
                    .or_else(|| cyl.height_limits.map(|[a, b]| (a, b)))
                    .unwrap_or((0.0, 0.0));
                let mid = 0.5 * (lo + hi);
                // Anchor on the lateral at mid-height in the seam direction.
                let rd = cyl.ref_dir.normalize().unwrap_or(cyl.ref_dir);
                let anchor = [
                    cyl.origin.x + axis.x * mid + rd.x * cyl.radius,
                    cyl.origin.y + axis.y * mid + rd.y * cyl.radius,
                    cyl.origin.z + axis.z * mid + rd.z * cyl.radius,
                ];
                let axis_arr = [axis.x, axis.y, axis.z];
                out.push(DimensionRecord {
                    id: id(),
                    kind: "diameter".into(),
                    value: cyl.radius * 2.0,
                    unit: unit_suffix.into(),
                    label: format!("Ø{}", unit.format_len_bare(cyl.radius * 2.0)),
                    entities: vec![fid],
                    anchor,
                    direction: [rd.x, rd.y, rd.z],
                    axis: Some(axis_arr),
                    pid: feature_dim_pid(model, &[fid], "diameter"),
                    datum: None,
                });
                // Length to the design's sharp corners: an end rounded by a
                // fillet or cut by a chamfer is carried across the blend to
                // the plane beyond it (a filleted, chamfered hub is 20 high,
                // not its trimmed face's 16.5).
                let (slo, shi) =
                    sharp_axial_extent(model, solid_id, fid, face, cyl.origin, axis, (lo, hi));
                let length = (shi - slo).abs();
                if length > 1e-9 {
                    out.push(DimensionRecord {
                        id: id(),
                        kind: "length".into(),
                        value: length,
                        unit: unit_suffix.into(),
                        label: format!("L {}", unit.format_len_bare(length)),
                        entities: vec![fid],
                        anchor,
                        direction: [axis.x, axis.y, axis.z],
                        axis: Some(axis_arr),
                        pid: feature_dim_pid(model, &[fid], "length"),
                        datum: None,
                    });
                }

                // ── Position dimensions (X and Y offsets from part-corner) ────
                //
                // Emitted for every cylindrical face (bore AND boss). The datum
                // is the AABB min corner projected onto the plane perpendicular
                // to the cylinder axis at the "drilled-face" elevation (the
                // axial station `lo` — the face's lower edge in the axis direction).
                //
                // Axis generality: the two offset axes are the WORLD axes whose
                // dot product with the cylinder axis is SMALLEST (perpendicular pair).
                // `axis_perpendicular_pair` returns None for diagonal axes — no
                // records are emitted in that case (honest absence over
                // misleading fabricated numbers).
                if let Some(ref bb) = bb {
                    if let Some((dominant_idx, perps)) =
                        axis_perpendicular_pair([axis.x, axis.y, axis.z])
                    {
                        // Cylinder axis in world space: decompose the axis origin
                        // projected to world. The axis passes through `cyl.origin`
                        // in direction `axis`. The world position of the axis at
                        // axial station `lo` is:
                        //   P_lo = cyl.origin + axis * lo
                        let p_lo = [
                            cyl.origin.x + axis.x * lo,
                            cyl.origin.y + axis.y * lo,
                            cyl.origin.z + axis.z * lo,
                        ];

                        // The part-corner datum origin: AABB min corner projected
                        // perpendicular to the axis (i.e. the two perpendicular
                        // axes take their min values; the dominant axis takes the
                        // drilled-face station value so the corner lives in the
                        // correct cross-section plane).
                        //
                        // This is the machinist's natural reference: clamp the
                        // part at its two perpendicular min edges.
                        // dominant_idx comes straight from axis_perpendicular_pair
                        // (review I-1: re-deriving it from `perps` was fragile
                        // against any future reordering of the returned pair).
                        let mut corner = bb.min;
                        corner[dominant_idx] = p_lo[dominant_idx];

                        let datum_desc = DatumDescriptor {
                            kind: "part_corner".into(),
                            name: "part corner".into(),
                            origin: corner,
                        };

                        let world_axes = [[1.0_f64, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
                        let axis_names = ["X", "Y", "Z"];
                        // Suffix indexed by WORLD axis of the offset (x/y/z): a Y-dominant
                        // bore legitimately offsets along X and Z, hence three entries even
                        // though the common Z-bore case uses only x/y.
                        let pid_suffixes = ["position_x", "position_y", "position_z"];

                        for &perp_idx in &perps {
                            // Offset = |axis_position - corner| along the world axis.
                            let offset = (p_lo[perp_idx] - corner[perp_idx]).abs();

                            // Anchor = midpoint between the corner and the axis, at
                            // the drilled-face plane, in the perpendicular direction.
                            let axis_pos_at_perp = p_lo[perp_idx];
                            let corner_at_perp = corner[perp_idx];
                            let mid_perp = 0.5 * (corner_at_perp + axis_pos_at_perp);
                            let mut pos_anchor = p_lo;
                            pos_anchor[perp_idx] = mid_perp;

                            // Direction = world unit axis pointing from corner
                            // toward the bore axis (sign follows geometry).
                            let sign = if axis_pos_at_perp >= corner_at_perp {
                                1.0
                            } else {
                                -1.0
                            };
                            let mut dir = world_axes[perp_idx];
                            dir[perp_idx] *= sign;

                            let label = format!(
                                "{} {}",
                                axis_names[perp_idx],
                                unit.format_len_bare(offset)
                            );

                            out.push(DimensionRecord {
                                id: id(),
                                kind: "position".into(),
                                value: offset,
                                unit: unit_suffix.into(),
                                label,
                                entities: vec![fid],
                                anchor: pos_anchor,
                                direction: dir,
                                axis: Some(axis_arr),
                                pid: position_dim_pid(model, fid, pid_suffixes[perp_idx]),
                                datum: Some(datum_desc.clone()),
                            });
                        }
                    }
                }
            } else if let Some(sph) = any.downcast_ref::<Sphere>() {
                out.push(DimensionRecord {
                    id: id(),
                    kind: "diameter".into(),
                    value: sph.radius * 2.0,
                    unit: unit_suffix.into(),
                    label: format!("SØ{}", unit.format_len_bare(sph.radius * 2.0)),
                    entities: vec![fid],
                    anchor: [sph.center.x + sph.radius, sph.center.y, sph.center.z],
                    direction: [1.0, 0.0, 0.0],
                    axis: None,
                    pid: feature_dim_pid(model, &[fid], "diameter"),
                    datum: None,
                });
            } else if let Some(cone) = any.downcast_ref::<Cone>() {
                let deg = cone.half_angle.to_degrees();
                let axis = cone.axis.normalize().unwrap_or(cone.axis);
                let cone_axis_arr = [axis.x, axis.y, axis.z];
                let h = match cone.height_limits {
                    Some([a, b]) => a.abs().max(b.abs()),
                    None => 0.0,
                };
                let base_r = h * cone.half_angle.tan();
                let anchor = [
                    cone.apex.x + axis.x * h,
                    cone.apex.y + axis.y * h,
                    cone.apex.z + axis.z * h,
                ];
                // Angles always in degrees regardless of document_unit.
                out.push(DimensionRecord {
                    id: id(),
                    kind: "angle".into(),
                    value: deg,
                    unit: "deg".into(),
                    label: format!("∠ {deg:.1}°"),
                    entities: vec![fid],
                    anchor,
                    direction: [axis.x, axis.y, axis.z],
                    axis: None,
                    pid: feature_dim_pid(model, &[fid], "angle"),
                    datum: None,
                });
                if base_r > 1e-9 {
                    out.push(DimensionRecord {
                        id: id(),
                        kind: "diameter".into(),
                        value: base_r * 2.0,
                        unit: unit_suffix.into(),
                        label: format!("Ø{}", unit.format_len_bare(base_r * 2.0)),
                        entities: vec![fid],
                        anchor,
                        direction: [1.0, 0.0, 0.0],
                        axis: Some(cone_axis_arr),
                        pid: feature_dim_pid(model, &[fid], "diameter"),
                        datum: None,
                    });
                }
            }
        }
    }

    // ── Blends: a fillet's radius, a chamfer's size ───────────────────────────
    // After every per-face record so a blended part's earlier ids keep their
    // numbers.
    for b in blend_records(model, solid_id) {
        let (kind, label) = match b.kind {
            BlendKind::Fillet => ("fillet", format!("R{}", unit.format_len_bare(b.value))),
            BlendKind::Chamfer => (
                "chamfer",
                format!("{} \u{00D7} 45\u{00B0}", unit.format_len_bare(b.value)),
            ),
        };
        out.push(DimensionRecord {
            id: id(),
            kind: kind.into(),
            value: b.value,
            unit: unit_suffix.into(),
            label,
            pid: feature_dim_pid(model, &b.faces, kind),
            entities: b.faces,
            anchor: b.anchor,
            direction: b.direction,
            axis: Some(b.axis),
            datum: None,
        });
    }

    merge_coaxial_walls(dedupe_coincident(out))
}

/// Geometric agreement required of a blend's rims with its own surface (and
/// with the faces it joins), in model millimetres. A rim that misses its
/// surface by more than this is not the blend this module describes, and the
/// face gets no callout rather than an approximate one.
const BLEND_GEOM_TOL: f64 = 1e-6;

/// Which blend a face is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlendKind {
    /// A torus band: its tube radius is the fillet radius.
    Fillet,
    /// A cone between a coaxial cylinder and a plane square to the axis, with
    /// equal legs: the leg is the chamfer size.
    Chamfer,
}

/// One blend callout: every face of one fillet (or chamfer), its size, and a
/// world point on its profile where a drawing's leader touches it.
#[derive(Debug, Clone)]
struct BlendCallout {
    kind: BlendKind,
    /// Fillet radius, or the chamfer's (equal) leg.
    value: f64,
    /// Every face of the blend, sorted.
    faces: Vec<u32>,
    /// A point ON the blend's profile, at an azimuth the blend covers, picked
    /// so the profile reads in a standard view: the profile plane holds the
    /// axis and the first of world X, Z, Y that is most square to it.
    anchor: [f64; 3],
    /// Unit direction in that profile plane: from the fillet's tube centre to
    /// the anchor, or square to the chamfer's line, away from the material
    /// edge. A view reads the profile when this and the axis are both square
    /// to its viewing direction.
    direction: [f64; 3],
    /// The blend's (unit) axis of revolution.
    axis: [f64; 3],
}

/// A blend face's profile about its axis of revolution: the two rims as
/// `(station along the axis, radius)`, by increasing station, and the
/// azimuth interval the face covers.
struct Profile {
    rims: [(f64, f64); 2],
    /// `None` when the face goes all the way round; else `(start, length)` in
    /// radians, counter-clockwise from `start`, in the frame of [`radial_frame`].
    covers: Option<(f64, f64)>,
}

/// `(u, w)`: the radial frame about `axis` in which azimuth 0 is `u`, the
/// first of world X, Z, Y that is most square to the axis.
fn radial_frame(axis: Vector3) -> Option<(Vector3, Vector3)> {
    let mut best: Option<(f64, Vector3)> = None;
    for e in [Vector3::X, Vector3::Z, Vector3::Y] {
        let perp = e - axis * e.dot(&axis);
        let n = perp.magnitude();
        // Strictly larger only: ties keep the earlier world axis.
        if best.map_or(true, |(bn, _)| n > bn + BLEND_GEOM_TOL) {
            best = Some((n, perp));
        }
    }
    let u = best?.1.normalize().ok()?;
    let w = axis.cross(&u).normalize().ok()?;
    Some((u, w))
}

/// Read `face`'s profile about the axis line `(origin, axis)` off its trim
/// edges: every sampled boundary point is a `(station, radius)` pair, and a
/// blend's profile runs between its lowest-station rim and its highest. Each
/// rim must be one circle (all its samples at one radius) or the face is not
/// a band of revolution and has no profile.
fn blend_profile(
    model: &BRepModel,
    face: &Face,
    origin: Point3,
    axis: Vector3,
    (u, w): (Vector3, Vector3),
) -> Option<Profile> {
    const SAMPLES: usize = 32;
    let mut pts: Vec<(f64, f64, f64)> = Vec::new(); // (station, radius, azimuth)
    for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
        let Some(lp) = model.loops.get(lid) else {
            continue;
        };
        for &eid in &lp.edges {
            let Some(edge) = model.edges.get(eid) else {
                continue;
            };
            let Some(curve) = model.curves.get(edge.curve_id) else {
                continue;
            };
            let r = edge.param_range;
            for k in 0..=SAMPLES {
                let t = r.start + (r.end - r.start) * (k as f64 / SAMPLES as f64);
                let Ok(p) = curve.point_at(t) else { continue };
                let d = p - origin;
                let s = d.dot(&axis);
                let radial = d - axis * s;
                pts.push((s, radial.magnitude(), radial.dot(&w).atan2(radial.dot(&u))));
            }
        }
    }
    if pts.is_empty() {
        return None;
    }
    let s_lo = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let s_hi = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    if s_hi - s_lo <= BLEND_GEOM_TOL {
        return None;
    }
    let rim = |s: f64| -> Option<f64> {
        let radii: Vec<f64> = pts
            .iter()
            .filter(|p| (p.0 - s).abs() <= BLEND_GEOM_TOL)
            .map(|p| p.1)
            .collect();
        let lo = radii.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = radii.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (hi - lo <= BLEND_GEOM_TOL).then_some(0.5 * (lo + hi))
    };
    let rims = [(s_lo, rim(s_lo)?), (s_hi, rim(s_hi)?)];

    // Azimuth coverage: the complement of the widest gap between samples.
    // A face that goes round leaves no gap wider than its own sampling.
    let mut az: Vec<f64> = pts
        .iter()
        .filter(|p| p.1 > BLEND_GEOM_TOL)
        .map(|p| p.2)
        .collect();
    az.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut covers = None;
    if let (Some(&first), Some(&last)) = (az.first(), az.last()) {
        let tau = std::f64::consts::TAU;
        let (mut gap, mut gap_end) = (first + tau - last, first);
        for pair in az.windows(2) {
            if pair[1] - pair[0] > gap {
                gap = pair[1] - pair[0];
                gap_end = pair[1];
            }
        }
        // Twice one edge's sampling step of a full turn: any wider gap is
        // azimuth the face does not reach.
        if gap > 2.0 * tau / SAMPLES as f64 {
            covers = Some((gap_end, tau - gap));
        }
    }
    Some(Profile { rims, covers })
}

/// Is azimuth 0 inside `covers`?
fn covers_zero(covers: Option<(f64, f64)>) -> bool {
    match covers {
        None => true,
        Some((start, len)) => {
            let tau = std::f64::consts::TAU;
            (0.0 - start).rem_euclid(tau) <= len
        }
    }
}

/// The fillets and equal-leg chamfers of `solid_id`, one callout per blend
/// (a blend split into several faces is one callout naming them all).
///
/// - A **fillet** is a torus face (tube radius below its centre-circle
///   radius) bordering at least two faces that are not part of the same
///   torus, whose two rims both lie on the tube -- its radius is the tube
///   radius.
/// - A **chamfer** is a cone face bordering only a coaxial cylinder, a plane
///   square to the axis and (when split) itself: one rim on the cylinder, the
///   other in the plane. Its legs are read off those rims and must agree with
///   the cone's half-angle; only an EQUAL-leg chamfer is called out, because
///   a sheet fact carries one number and an unequal chamfer states two. An
///   unequal chamfer keeps its cone's angle and diameter rows.
///
/// A face that matches neither exactly gets no callout -- never an
/// approximate one.
fn blend_records(model: &BRepModel, solid_id: SolidId) -> Vec<BlendCallout> {
    use crate::primitives::surface::{Plane, Torus};

    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let edge_faces = edge_faces_of(model, solid_id);
    let neighbours = |fid: u32, face: &Face| -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        for e in face_edges(model, face) {
            for &f in edge_faces.get(&e).map(Vec::as_slice).unwrap_or(&[]) {
                if f != fid && !out.contains(&f) {
                    out.push(f);
                }
            }
        }
        out
    };
    let surface_of = |fid: u32| {
        model
            .faces
            .get(fid)
            .and_then(|f| model.surfaces.get(f.surface_id))
    };
    let on_axis_line = |p: Point3, origin: Point3, axis: Vector3| {
        let d = p - origin;
        (d - axis * d.dot(&axis)).magnitude() <= BLEND_GEOM_TOL
    };
    let parallel = |a: Vector3, b: Vector3| a.dot(&b).abs() > 1.0 - 1e-9;

    // (kind, value, axis, origin, profile rims, faces with their coverage)
    struct Group {
        kind: BlendKind,
        value: f64,
        axis: Vector3,
        origin: Point3,
        frame: (Vector3, Vector3),
        rims: [(f64, f64); 2],
        /// The profile point (station, radius) the leader touches, and the
        /// profile direction stored as `direction`.
        point: (f64, f64),
        dir: (f64, f64),
        faces: Vec<(u32, Option<(f64, f64)>)>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let same_rims = |a: &[(f64, f64); 2], b: &[(f64, f64); 2]| {
        a.iter().zip(b.iter()).all(|(x, y)| {
            (x.0 - y.0).abs() <= BLEND_GEOM_TOL && (x.1 - y.1).abs() <= BLEND_GEOM_TOL
        })
    };

    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let Some(surface) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            let any = surface.as_any();
            let candidate = if let Some(t) = any.downcast_ref::<Torus>() {
                let Ok(axis) = t.axis.normalize() else {
                    continue;
                };
                if !(t.minor_radius > 0.0 && t.minor_radius < t.major_radius) {
                    continue;
                }
                let sibling = |f: u32| {
                    surface_of(f)
                        .and_then(|s| s.as_any().downcast_ref::<Torus>())
                        .is_some_and(|o| {
                            parallel(o.axis, axis)
                                && (o.center - t.center).magnitude() <= BLEND_GEOM_TOL
                                && (o.major_radius - t.major_radius).abs() <= BLEND_GEOM_TOL
                                && (o.minor_radius - t.minor_radius).abs() <= BLEND_GEOM_TOL
                        })
                };
                let joined = neighbours(fid, face)
                    .into_iter()
                    .filter(|&f| !sibling(f))
                    .count();
                if joined < 2 {
                    continue;
                }
                let Some(frame) = radial_frame(axis) else {
                    continue;
                };
                let Some(profile) = blend_profile(model, face, t.center, axis, frame) else {
                    continue;
                };
                // Both rims on the tube, whose centre is (0, major) in profile.
                let r = t.minor_radius;
                let tube = (0.0, t.major_radius);
                let on_tube = |p: (f64, f64)| {
                    ((p.0 - tube.0).hypot(p.1 - tube.1) - r).abs() <= BLEND_GEOM_TOL
                };
                if !profile.rims.iter().all(|&p| on_tube(p)) {
                    continue;
                }
                // The profile arc's midpoint: from the tube centre toward the
                // chord's midpoint, one radius out (a fillet is the minor arc).
                let m = (
                    0.5 * (profile.rims[0].0 + profile.rims[1].0) - tube.0,
                    0.5 * (profile.rims[0].1 + profile.rims[1].1) - tube.1,
                );
                let ml = m.0.hypot(m.1);
                if ml <= BLEND_GEOM_TOL {
                    continue;
                }
                let dir = (m.0 / ml, m.1 / ml);
                Some((
                    BlendKind::Fillet,
                    r,
                    axis,
                    t.center,
                    frame,
                    profile,
                    (tube.0 + r * dir.0, tube.1 + r * dir.1),
                    dir,
                ))
            } else if let Some(c) = any.downcast_ref::<Cone>() {
                let Ok(axis) = c.axis.normalize() else {
                    continue;
                };
                let mut cylinder: Option<f64> = None; // its radius
                let mut plane: Option<f64> = None; // its station
                let mut other = false;
                for f in neighbours(fid, face) {
                    let Some(s) = surface_of(f) else {
                        other = true;
                        continue;
                    };
                    let s = s.as_any();
                    if let Some(cy) = s.downcast_ref::<Cylinder>() {
                        if parallel(cy.axis, axis) && on_axis_line(cy.origin, c.apex, axis) {
                            cylinder = Some(cy.radius);
                            continue;
                        }
                    } else if let Some(pl) = s.downcast_ref::<Plane>() {
                        if parallel(pl.normal, axis) {
                            plane = Some((pl.origin - c.apex).dot(&axis));
                            continue;
                        }
                    } else if let Some(o) = s.downcast_ref::<Cone>() {
                        let same = parallel(o.axis, axis)
                            && (o.apex - c.apex).magnitude() <= BLEND_GEOM_TOL
                            && (o.half_angle - c.half_angle).abs() <= 1e-9;
                        if same {
                            continue;
                        }
                    }
                    other = true;
                }
                let (Some(cyl_r), Some(plane_s), false) = (cylinder, plane, other) else {
                    continue;
                };
                let Some(frame) = radial_frame(axis) else {
                    continue;
                };
                let Some(profile) = blend_profile(model, face, c.apex, axis, frame) else {
                    continue;
                };
                // One rim on the cylinder, the other in the plane.
                let [p0, p1] = profile.rims;
                let (on_cyl, in_plane) = if (p0.1 - cyl_r).abs() <= BLEND_GEOM_TOL
                    && (p1.0 - plane_s).abs() <= BLEND_GEOM_TOL
                {
                    (p0, p1)
                } else if (p1.1 - cyl_r).abs() <= BLEND_GEOM_TOL
                    && (p0.0 - plane_s).abs() <= BLEND_GEOM_TOL
                {
                    (p1, p0)
                } else {
                    continue;
                };
                let axial = (in_plane.0 - on_cyl.0).abs();
                let radial = (in_plane.1 - on_cyl.1).abs();
                // The legs are the cone's own: radial = axial · tan(half-angle).
                if (radial - axial * c.half_angle.tan()).abs() > BLEND_GEOM_TOL
                    || (axial - radial).abs() > BLEND_GEOM_TOL
                {
                    continue;
                }
                let mid = (0.5 * (on_cyl.0 + in_plane.0), 0.5 * (on_cyl.1 + in_plane.1));
                // Square to the chamfer's line, away from the sharp corner
                // it replaced (in_plane.0, on_cyl.1).
                let line = (in_plane.0 - on_cyl.0, in_plane.1 - on_cyl.1);
                let ll = line.0.hypot(line.1);
                let mut n = (-line.1 / ll, line.0 / ll);
                let corner = (in_plane.0 - mid.0, on_cyl.1 - mid.1);
                if n.0 * corner.0 + n.1 * corner.1 > 0.0 {
                    n = (-n.0, -n.1);
                }
                Some((
                    BlendKind::Chamfer,
                    axial,
                    axis,
                    c.apex,
                    frame,
                    profile,
                    mid,
                    n,
                ))
            } else {
                None
            };
            let Some((kind, value, axis, origin, frame, profile, point, dir)) = candidate else {
                continue;
            };
            // One blend split into faces: same kind, size, axis line, rims.
            let existing = groups.iter_mut().find(|g| {
                g.kind == kind
                    && (g.value - value).abs() <= BLEND_GEOM_TOL
                    && parallel(g.axis, axis)
                    && on_axis_line(origin, g.origin, g.axis)
                    && {
                        // Stations are relative to each surface's own origin
                        // and axis sense; restate them in the group's.
                        let sense = g.axis.dot(&axis).signum();
                        let shift = (origin - g.origin).dot(&g.axis);
                        let mut rims = profile.rims.map(|(s, r)| (s * sense + shift, r));
                        rims.sort_by(|a, b| {
                            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        same_rims(&rims, &g.rims)
                    }
            });
            match existing {
                Some(g) => g.faces.push((fid, profile.covers)),
                None => groups.push(Group {
                    kind,
                    value,
                    axis,
                    origin,
                    frame,
                    rims: profile.rims,
                    point,
                    dir,
                    faces: vec![(fid, profile.covers)],
                }),
            }
        }
    }

    groups
        .into_iter()
        .map(|g| {
            // Azimuth 0 when the blend reaches it, else the middle of the
            // first face's reach (the frame is shared by the group: its axis
            // and origin are the first face's).
            let theta = if g.faces.iter().any(|f| covers_zero(f.1)) {
                0.0
            } else {
                g.faces
                    .first()
                    .and_then(|f| f.1)
                    .map_or(0.0, |(start, len)| start + 0.5 * len)
            };
            let (u0, w0) = g.frame;
            let u = u0 * theta.cos() + w0 * theta.sin();
            let anchor = g.origin + g.axis * g.point.0 + u * g.point.1;
            let direction = g.axis * g.dir.0 + u * g.dir.1;
            let mut faces: Vec<u32> = g.faces.iter().map(|f| f.0).collect();
            faces.sort_unstable();
            BlendCallout {
                kind: g.kind,
                value: g.value,
                faces,
                anchor: [anchor.x, anchor.y, anchor.z],
                direction: [direction.x, direction.y, direction.z],
                axis: [g.axis.x, g.axis.y, g.axis.z],
            }
        })
        .collect()
}

/// Merge the walls of ONE hole that two separate cuts left as two faces.
///
/// A bore cut through a plate, and later through a boss added under it, is
/// one hole -- but its wall is two coaxial, equal-radius cylinder faces that
/// meet end to end, with different mid-height anchors, so
/// [`dedupe_coincident`] (which merges seam-split faces with IDENTICAL
/// anchors) leaves them apart. The sheet then read a Ø30 THRU bore as two
/// blind holes ("Ø30 ↧ 8" and "Ø30 ↧ 15") on the live mounting plate.
///
/// Walls are merged when their diameters agree, their axes coincide, and
/// their axial spans touch or overlap. The surviving diameter record names
/// every face and sits at the merged span's mid-height; the merged length is
/// the union span; position records of the absorbed faces fold into the
/// survivor's. Different diameters (a counterbore over its drill) never
/// merge.
fn merge_coaxial_walls(rows: Vec<DimensionRecord>) -> Vec<DimensionRecord> {
    const TOL: f64 = 1e-3;
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // (row index, axis point at station 0, unit axis, span lo, span hi)
    struct Wall {
        dia: usize,
        len: Option<usize>,
        axis: [f64; 3],
        foot: [f64; 3],
        lo: f64,
        hi: f64,
    }
    let mut walls: Vec<Wall> = Vec::new();
    for (i, d) in rows.iter().enumerate() {
        if d.kind != "diameter" || d.entities.is_empty() {
            continue;
        }
        let Some(axis) = d.axis else { continue };
        let n = dot(axis, axis).sqrt();
        if n < 1e-12 {
            continue;
        }
        let axis = [axis[0] / n, axis[1] / n, axis[2] / n];
        // The anchor sits on the wall at mid-height, `value / 2` along the
        // seam direction from the axis.
        let r = 0.5 * d.value;
        let on_axis = [
            d.anchor[0] - d.direction[0] * r,
            d.anchor[1] - d.direction[1] * r,
            d.anchor[2] - d.direction[2] * r,
        ];
        let mid = dot(on_axis, axis);
        let foot = [
            on_axis[0] - axis[0] * mid,
            on_axis[1] - axis[1] * mid,
            on_axis[2] - axis[2] * mid,
        ];
        let len = rows
            .iter()
            .position(|l| l.kind == "length" && l.entities == d.entities);
        let half = len.map(|j| 0.5 * rows[j].value).unwrap_or(0.0);
        walls.push(Wall {
            dia: i,
            len,
            axis,
            foot,
            lo: mid - half,
            hi: mid + half,
        });
    }

    // Union-find over walls that are the same hole.
    let mut parent: Vec<usize> = (0..walls.len()).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut j = i;
        while p[j] != r {
            let next = p[j];
            p[j] = r;
            j = next;
        }
        r
    }
    for a in 0..walls.len() {
        for b in (a + 1)..walls.len() {
            let (wa, wb) = (&walls[a], &walls[b]);
            let same_dia = (rows[wa.dia].value - rows[wb.dia].value).abs() < TOL;
            let parallel = dot(wa.axis, wb.axis).abs() > 1.0 - 1e-9;
            let feet = [
                wa.foot[0] - wb.foot[0],
                wa.foot[1] - wb.foot[1],
                wa.foot[2] - wb.foot[2],
            ];
            let same_line = dot(feet, feet).sqrt() < TOL;
            // Spans along the SHARED axis direction (b may point the other way).
            let s = dot(wa.axis, wb.axis).signum();
            let (blo, bhi) = if s > 0.0 {
                (wb.lo, wb.hi)
            } else {
                (-wb.hi, -wb.lo)
            };
            let touch = blo <= wa.hi + TOL && wa.lo <= bhi + TOL;
            if same_dia && parallel && same_line && touch && wa.len.is_some() && wb.len.is_some() {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[rb] = ra;
                }
            }
        }
    }

    let mut drop: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut rows = rows;
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for w in 0..walls.len() {
        let root = find(&mut parent, w);
        groups.entry(root).or_default().push(w);
    }
    for (_, members) in groups {
        if members.len() < 2 {
            continue;
        }
        let first = &walls[members[0]];
        let axis = first.axis;
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        let mut faces: Vec<u32> = Vec::new();
        for &w in &members {
            let wall = &walls[w];
            let s = dot(axis, wall.axis).signum();
            let (wlo, whi) = if s > 0.0 {
                (wall.lo, wall.hi)
            } else {
                (-wall.hi, -wall.lo)
            };
            lo = lo.min(wlo);
            hi = hi.max(whi);
            faces.extend(rows[wall.dia].entities.iter().copied());
        }
        faces.sort_unstable();
        faces.dedup();
        let mid = 0.5 * (lo + hi);
        let survivor = members[0];
        let (dia_i, len_i) = (walls[survivor].dia, walls[survivor].len);
        let shift = mid - 0.5 * (walls[survivor].lo + walls[survivor].hi);
        let moved = |p: [f64; 3]| {
            [
                p[0] + axis[0] * shift,
                p[1] + axis[1] * shift,
                p[2] + axis[2] * shift,
            ]
        };
        rows[dia_i].anchor = moved(rows[dia_i].anchor);
        rows[dia_i].entities = faces.clone();
        if let Some(j) = len_i {
            rows[j].anchor = moved(rows[j].anchor);
            // Relabel in the label's own unit and precision: the label's
            // number is the old length converted to the document unit.
            let old_value = rows[j].value;
            let new_value = hi - lo;
            if let Some(num) = rows[j].label.strip_prefix("L ") {
                if let Ok(shown) = num.trim().parse::<f64>() {
                    let decimals = num.trim().split('.').nth(1).map_or(0, str::len);
                    let per_mm = if old_value.abs() > 1e-12 {
                        shown / old_value
                    } else {
                        1.0
                    };
                    rows[j].label = format!("L {:.*}", decimals, new_value * per_mm);
                }
            }
            rows[j].value = new_value;
            rows[j].entities = faces.clone();
        }
        for &w in &members[1..] {
            drop.insert(walls[w].dia);
            if let Some(j) = walls[w].len {
                drop.insert(j);
            }
        }
        // Position records of absorbed faces: fold into the survivor's.
        let absorbed: std::collections::HashSet<u32> = members[1..]
            .iter()
            .flat_map(|&w| rows[walls[w].dia].entities.clone())
            .collect();
        for (k, r) in rows.iter().enumerate() {
            if r.kind == "position"
                && !r.entities.is_empty()
                && r.entities.iter().all(|e| absorbed.contains(e))
            {
                drop.insert(k);
            }
        }
    }
    rows.into_iter()
        .enumerate()
        .filter(|(i, _)| !drop.contains(i))
        .map(|(_, r)| r)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    fn find(dims: &[DimensionRecord], kind: &str, value: f64) -> bool {
        dims.iter()
            .any(|d| d.kind == kind && (d.value - value).abs() < 1e-4)
    }

    #[test]
    fn box_extents_are_exact() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box"));
        let dims = extract_dimensions(&m, b);
        // Three extents, exactly the box size.
        assert!(find(&dims, "extent", 40.0), "X extent missing: {dims:?}");
        assert!(find(&dims, "extent", 30.0), "Y extent missing");
        assert!(find(&dims, "extent", 20.0), "Z extent missing");
    }

    #[test]
    fn bored_plate_reports_diameter_length_and_extents() {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        let dims = extract_dimensions(&m, part);
        // The bore: Ø20 diameter, and its axial length = the 20-thick plate.
        assert!(find(&dims, "diameter", 20.0), "Ø20 bore missing: {dims:?}");
        assert!(
            find(&dims, "length", 20.0),
            "bore length 20 missing: {dims:?}"
        );
        // Overall extents still present (the bore doesn't change the 40×40×20).
        assert!(find(&dims, "extent", 40.0), "X extent missing");
        // Every record is recoverable: finite anchor + spanned entities for feats.
        for d in &dims {
            assert!(d.anchor.iter().all(|c| c.is_finite()), "bad anchor {d:?}");
            if d.kind == "diameter" {
                assert!(!d.entities.is_empty(), "diameter must name its face: {d:?}");
            }
        }
    }

    #[test]
    fn cylinder_extent_uses_curve_not_vertices() {
        // A bare analytic cylinder: post-#24 it has ONE seam vertex, so a
        // vertex AABB would give ~0 width. The curve-sampled extent must
        // recover the true diameter (Ø30) on X and Y.
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 15.0, 50.0)
            .expect("cyl"));
        let dims = extract_dimensions(&m, c);
        assert!(
            find(&dims, "extent", 30.0),
            "X/Y extent should be Ø30: {dims:?}"
        );
        assert!(find(&dims, "extent", 50.0), "Z extent should be height 50");
        assert!(find(&dims, "diameter", 30.0), "Ø30 missing");
        assert!(find(&dims, "length", 50.0), "length 50 missing");
    }

    // ── Coincident-row dedupe (Task 1, ui-units campaign) ─────────────────────

    /// `dedupe_coincident` merges two records that are geometrically identical
    /// (same kind + quantized value + quantized anchor + quantized direction)
    /// into ONE record with both face ids in `entities` — and leaves the first
    /// record's `id` and `pid` unchanged.
    #[test]
    fn dedupe_coincident_merges_identical_rows_and_keeps_first_id() {
        fn make_rec(id: &str, eid: u32) -> DimensionRecord {
            DimensionRecord {
                id: id.to_string(),
                kind: "diameter".to_string(),
                value: 20.0,
                unit: "mm".to_string(),
                label: "Ø20.00mm".to_string(),
                entities: vec![eid],
                anchor: [5.0, 10.0, 15.0],
                direction: [1.0, 0.0, 0.0],
                axis: None,
                pid: Some(id.to_string()),
                datum: None,
            }
        }

        // Two records with different face ids but identical geometry.
        let rows = vec![make_rec("d0", 7), make_rec("d1", 11)];
        let merged = dedupe_coincident(rows);

        assert_eq!(merged.len(), 1, "two coincident rows must merge into one");
        assert_eq!(merged[0].id, "d0", "first id wins");
        assert_eq!(merged[0].pid.as_deref(), Some("d0"), "first pid wins");
        assert!(
            merged[0].entities.contains(&7),
            "original face id must be present"
        );
        assert!(
            merged[0].entities.contains(&11),
            "merged face id must be present"
        );
        assert_eq!(
            merged[0].entities.len(),
            2,
            "exactly two entity ids after merge"
        );
    }

    /// Two records with different values (20mm vs 30mm) must NOT be merged.
    #[test]
    fn dedupe_coincident_keeps_distinct_values_separate() {
        fn make_rec(id: &str, val: f64, eid: u32) -> DimensionRecord {
            DimensionRecord {
                id: id.to_string(),
                kind: "diameter".to_string(),
                value: val,
                unit: "mm".to_string(),
                label: "label".to_string(),
                entities: vec![eid],
                anchor: [0.0, 0.0, 0.0],
                direction: [1.0, 0.0, 0.0],
                axis: None,
                pid: None,
                datum: None,
            }
        }
        let rows = vec![make_rec("d0", 20.0, 1), make_rec("d1", 30.0, 2)];
        let merged = dedupe_coincident(rows);
        assert_eq!(
            merged.len(),
            2,
            "records with different values must not merge: {merged:?}"
        );
    }
}
