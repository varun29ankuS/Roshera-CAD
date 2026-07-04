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
    /// "diameter" | "radius" | "length" | "angle" | "extent".
    pub kind: String,
    pub value: f64,
    /// "mm" for lengths, "deg" for angles.
    pub unit: String,
    /// Human label, e.g. "Ø20.00", "L 40.00", "X 110.00", "∠ 30.0°".
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

/// Assemble the full analytic dimension table for `solid_id`.
pub fn extract_dimensions(model: &BRepModel, solid_id: SolidId) -> Vec<DimensionRecord> {
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
                unit: "mm".into(),
                label: format!("{} {:.2}", names[axis], value),
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
        None => return out,
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
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
                    unit: "mm".into(),
                    label: format!("Ø{:.2}", cyl.radius * 2.0),
                    entities: vec![fid],
                    anchor,
                    direction: [rd.x, rd.y, rd.z],
                    axis: Some(axis_arr),
                    pid: feature_dim_pid(model, &[fid], "diameter"),
                    datum: None,
                });
                let length = (hi - lo).abs();
                if length > 1e-9 {
                    out.push(DimensionRecord {
                        id: id(),
                        kind: "length".into(),
                        value: length,
                        unit: "mm".into(),
                        label: format!("L {length:.2}"),
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

                            let label = format!("{} {:.2}", axis_names[perp_idx], offset);

                            out.push(DimensionRecord {
                                id: id(),
                                kind: "position".into(),
                                value: offset,
                                unit: "mm".into(),
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
                    unit: "mm".into(),
                    label: format!("SØ{:.2}", sph.radius * 2.0),
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
                        unit: "mm".into(),
                        label: format!("Ø{:.2}", base_r * 2.0),
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

    out
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
}
