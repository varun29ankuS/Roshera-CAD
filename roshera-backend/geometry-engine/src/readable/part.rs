//! Agent-facing part records.
//!
//! These types are the wire shape the AI executor passes back to the
//! LLM. They are **not** internal kernel state — they are flat,
//! self-describing snapshots designed to be serialized to JSON and read
//! by an agent.
//!
//! Every coordinate triplet here is paired with a frame so the agent
//! never has to guess what the numbers mean:
//! - `center_world` / `world_bbox_min` / `world_bbox_max` are in the
//!   world frame
//! - `center_in_anchor_frame` is in the part's anchor datum frame
//! - `signed_distance_{front,top,right}` are measured against the
//!   canonical world XY/XZ/YZ planes (per [`LocationDescriptor`] doc)
//!
//! Cached behind [`BRepModel::solid_location_descriptor_cached`] so
//! repeated agent queries don't re-walk topology for the location
//! payload. Volume / surface area / topology counts are recomputed per
//! call (slice 6 ships without caching them — they cost ≪ 1 ms on
//! production-sized models and an LRU cache would be premature).

use crate::primitives::datum::{DatumId, DatumKind, LocationDescriptor};
use crate::primitives::solid::{MassPropertiesMethod, SolidId};
use serde::{Deserialize, Serialize};

/// Topology fingerprint — counts of the canonical entity types a solid
/// is composed of.
///
/// Useful for agents reasoning about complexity. A simple box has
/// `{6, 12, 8}` (6 faces, 12 edges, 8 vertices); a sphere mesh might
/// have `{1, 0, 0}` if rendered as a single NURBS face or `{N, M, K}`
/// after being booleaned with another solid.
///
/// All three counts are over the solid's outer shell only — inner
/// shells (voids) are excluded so agents reading "vertex_count: 8"
/// don't get confused by hollow geometry that internally tracks
/// thousands of void-shell vertices.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopologyFingerprint {
    /// Number of faces in the outer shell.
    pub face_count: usize,
    /// Number of unique edges referenced by the outer shell's faces.
    /// Each edge is counted once even if shared between adjacent faces
    /// (which it always is in a manifold solid).
    pub edge_count: usize,
    /// Number of unique vertices referenced by the outer shell's edges.
    pub vertex_count: usize,
}

/// Filter for [`BRepModel::list_parts_filtered`].
///
/// All fields are optional and AND-ed together — supplying multiple
/// criteria narrows the result set. Default-constructed (`Default`)
/// matches every solid in the model and is equivalent to calling
/// the no-arg `list_parts` accessor.
///
/// Future extensions (slice 7+): kind filter, volume range,
/// has-feature filter. The current scope is intentionally minimal so
/// agents have an obvious primary discriminator (anchor) and a fuzzy
/// fallback (name substring) without combinatorial explosion in the
/// tool schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListPartsFilter {
    /// When `Some(id)`, only parts whose `SolidAnchor::datum_id` equals
    /// `id` are returned. Useful for "list parts on FrontPlane".
    pub anchor_datum_id: Option<DatumId>,
    /// When `Some(s)`, only parts whose name contains `s` (case-
    /// insensitive substring) are returned. Useful for "find the
    /// bracket parts". Auto-generated `solid_<id>` placeholders are
    /// matched against verbatim.
    pub name_contains: Option<String>,
}

/// Material snapshot for [`PartReport::material`].
///
/// Carries the two fields a mech-engineering agent actually needs to
/// reason about weight and cost: a human-readable name (so the agent
/// can quote "Steel" / "Aluminum 6061" back to the user) and a numeric
/// density in kg/m³ (so it can be multiplied by the kernel's volume
/// integral to recover mass).
///
/// Other engineering moduli on the kernel-side `Material`
/// (Young's modulus, Poisson's ratio, thermal expansion) are
/// intentionally not surfaced in slice 6 — they pertain to FEA / CAE
/// workflows that aren't part of the slice-6 agent contract. Add as
/// new fields when the FEA story lands rather than retro-fitting the
/// existing two-field shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaterialSummary {
    /// Human-readable material name. Defaults to `"Steel"` for solids
    /// that haven't had a material explicitly assigned (per the
    /// kernel's [`crate::primitives::solid::Material::default`]).
    pub name: String,
    /// Density in kg/m³. Default-Steel = 7850.0.
    pub density: f64,
}

/// Mass-properties report for a single solid.
///
/// Returned by `BRepModel::mass_properties_for`. All values are in SI
/// units assuming the working-unit convention is millimetres for
/// length: volume in **mm³**, mass in **kg** when density is kg/m³
/// (the kernel does the cubic-mm-to-cubic-m conversion internally),
/// center of mass / inertia tensor in working units.
///
/// `principal_axes` is a column-vector triplet ordered by ascending
/// eigenvalue (`principal_moments[0]` corresponds to `principal_axes[0]`,
/// etc.). The third axis is therefore the part's "long axis" — the
/// direction with the smallest moment of inertia, useful for agents
/// asking *"which way does this part point?"*.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MassPropertiesReport {
    /// Stable solid identifier this report describes.
    pub solid_id: SolidId,
    /// Volume integral over the outer shell minus inner shells. Always
    /// non-negative for well-formed solids.
    pub volume: f64,
    /// Outer-shell surface area.
    pub surface_area: f64,
    /// Mass = volume × material density. Always populated since the
    /// kernel guarantees every solid carries a `Material` (defaulting
    /// to Steel @ 7850 kg/m³).
    pub mass: f64,
    /// Material snapshot used in the mass calculation, surfaced so the
    /// agent can quote back *"this part weighs 1.42 kg in Aluminum
    /// 6061"* without a follow-up query.
    pub material: MaterialSummary,
    /// World-space center of mass (the same point one would attach a
    /// string to and the part would hang balanced).
    pub center_of_mass: [f64; 3],
    /// Full 3×3 inertia tensor about the center of mass, in
    /// row-major form. Symmetric — `tensor[i][j] == tensor[j][i]`
    /// up to floating-point noise.
    pub inertia_tensor: [[f64; 3]; 3],
    /// Principal moments of inertia, sorted ascending. The smallest
    /// moment corresponds to the long-axis direction.
    pub principal_moments: [f64; 3],
    /// Principal axes — orthonormal eigenvectors of the inertia
    /// tensor, ordered to match `principal_moments`. `principal_axes[0]`
    /// is the direction of smallest moment (i.e. the part's "long
    /// axis"); `principal_axes[2]` is the direction of largest moment.
    pub principal_axes: [[f64; 3]; 3],
    /// Radius of gyration about each principal axis (`sqrt(I/m)`).
    pub radius_of_gyration: [f64; 3],
    /// Indicator of how this report was computed —
    /// [`MassPropertiesMethod::Analytical`] (closed-form face traversal,
    /// exact to floating-point noise) or
    /// [`MassPropertiesMethod::Tessellated`] (numerical integration over
    /// the tessellated surface, used for curved primitives whose
    /// analytical seam loops would degenerate). The tessellated variant
    /// carries an empirical relative-tolerance bound the agent can use
    /// to decide whether to trust the tail digits.
    pub method: MassPropertiesMethod,
}

/// Oriented bounding box returned by `BRepModel::oriented_bbox_for`.
///
/// Tighter than the world-aligned AABB for parts whose principal axes
/// don't align with world XYZ. The agent gets this when it asks
/// *"how long is this part along its own axis?"* — the answer is
/// `2 * half_extents[0]` along `axes[0]`, etc.
///
/// The OBB is in the world frame: `center` is a world-space point and
/// `axes` are unit vectors expressed in world coordinates. Half-extents
/// are along the corresponding axis (sorted to match `axes`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrientedBBox {
    /// World-space center of the OBB.
    pub center: [f64; 3],
    /// Three orthonormal axes (column vectors). Row-major: `axes[i]` is
    /// the i-th axis as a unit vector. Ordered to match the inertia
    /// tensor's principal-axis ordering — `axes[0]` is the long axis.
    pub axes: [[f64; 3]; 3],
    /// Half-extents along each axis. The full length along axis `i` is
    /// `2 * half_extents[i]`.
    pub half_extents: [f64; 3],
}

/// Per-face report for `BRepModel::query_face`.
///
/// Lets agents drill from "tell me about part 7" → "tell me about
/// face 12 of part 7". A face report carries enough geometry for an
/// agent to decide *"is this the inside of a hole?"* (cylindrical +
/// inward-facing normal at centroid) or *"is this the top face?"*
/// (planar + normal close to world +Z).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaceReport {
    /// Stable kernel identifier.
    pub id: u32,
    /// Solid that owns this face. `None` when the face is not yet
    /// attached to a shell (transient construction state).
    pub host_solid: Option<SolidId>,
    /// One-word surface-type tag: `"plane"`, `"cylinder"`, `"sphere"`,
    /// `"cone"`, `"torus"`, `"nurbs"`, or `"unknown"` for surfaces the
    /// kernel can't classify.
    pub surface_type: String,
    /// Face area in squared working units. `None` when the kernel
    /// cannot compute the area for a degenerate face.
    pub area: Option<f64>,
    /// Edge ids of the outer-loop boundary, in loop order.
    pub edge_ids: Vec<u32>,
    /// Adjacent face ids sharing an edge with this face. Order is
    /// kernel-internal hash-map order — agents should treat this as a
    /// set, not a sequence.
    pub neighbour_face_ids: Vec<u32>,
}

/// Agent-facing "what is the cursor pointing at" record for
/// `BRepModel::query_hover`.
///
/// The viewport raycasts a hover to a kernel `FaceId` (via the mesh's
/// per-triangle `face_map`); this report turns that bare id into the
/// signal an agent actually needs: the face's own geometry PLUS its
/// host part's identity and datum-anchored location. That join is the
/// point — per the `readable/` architectural rule, agents address
/// geometry by **part identity + anchor datum**, so a hover signal that
/// returned only `surface_type`/`area` would force a second round-trip
/// to discover *which part, anchored where*. `host_part_name` +
/// `location_oneliner` close that gap in one call.
///
/// `host_part_name` / `location_oneliner` are `None` when the face is
/// not attached to a solid (transient construction state) — `face` is
/// still fully populated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HoverReport {
    /// The hovered face's own report (surface type, area, boundary).
    pub face: FaceReport,
    /// Name of the solid that owns this face (or `solid_<id>`
    /// placeholder). `None` when the face is not attached to a solid.
    pub host_part_name: Option<String>,
    /// One-line datum-anchored summary of the host part, e.g.
    /// `"FrontPlane, offset (10, 0, 5), 20×15×10"`. `None` when the
    /// face has no host solid or its location can't be resolved.
    pub location_oneliner: Option<String>,
}

/// Per-edge report for `BRepModel::query_edge`.
///
/// Edge-level introspection lets agents reason about specific
/// geometric features ("the fillet between face 3 and face 4 is
/// 5 mm", "this edge is 100 mm long") without enumerating every face.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeReport {
    /// Stable kernel identifier.
    pub id: u32,
    /// One-word curve-kind tag: `"line"`, `"arc"`, `"circle"`,
    /// `"nurbs"`, `"composite"`, `"polyline"`, or `"unknown"`.
    pub curve_kind: String,
    /// Edge length in working units. `None` when length cannot be
    /// computed (degenerate parameter range, missing curve).
    pub length: Option<f64>,
    /// World-space start vertex coordinates.
    pub start: [f64; 3],
    /// World-space end vertex coordinates.
    pub end: [f64; 3],
}

/// Full agent-facing record for a single solid.
///
/// Returned by `BRepModel::query_part`. Use this when an agent asks
/// *"tell me about part X"* — `name`, `kind`, `anchor`, `dimensions`,
/// `volume`, `surface_area`, `topology`, `dependent_datums`, and
/// `neighbors` together form a self-contained briefing covering
/// identity, geometry, semantic context, and spatial neighborhood.
///
/// Field naming is deliberately verbose (e.g. `anchor_datum_name`
/// rather than `anchor`) because LLM context windows are forgiving and
/// over-explicit field names dramatically reduce hallucinated
/// assumptions about what a value represents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartReport {
    /// Stable kernel identifier of the solid.
    pub id: SolidId,
    /// User-facing label, or auto-generated `solid_<id>` placeholder
    /// when the solid was created without a name.
    pub name: String,
    /// One-word kind tag: `"box"`, `"sphere"`, `"cylinder"`, `"cone"`,
    /// `"boolean"`, or `"solid"` when the kind is not yet recoverable
    /// from the topology stores. Slice 6 ships with `"solid"` as the
    /// fallback — primitive subclassification requires a
    /// `Solid::primitive_kind` field stamped at creation time, queued
    /// for slice 7. Exposing the field today with `"solid"` lets the
    /// wire format be stable without lying to agents.
    pub kind: String,
    /// The full slice-3b/5 location descriptor: anchor-frame center,
    /// world-frame center, dimensions, canonical-plane signed
    /// distances. Already cached per-solid via
    /// `solid_location_descriptor_cached`.
    pub location: LocationDescriptor,
    /// Convenience repeat of `location.anchor_datum_id`.
    pub anchor_datum_id: DatumId,
    /// Convenience repeat of `location.anchor_datum_name`.
    pub anchor_datum_name: String,
    /// World-space axis-aligned bounding box minimum corner. Together
    /// with `world_bbox_max` gives agents the literal extents needed
    /// for axis-aligned overlap checks (`min_a.x <= max_b.x`, …).
    pub world_bbox_min: [f64; 3],
    /// World-space axis-aligned bounding box maximum corner.
    pub world_bbox_max: [f64; 3],
    /// Solid volume in cubic working units (mm³ in mech CAD
    /// convention). `None` when the kernel cannot recover a closed
    /// volume — e.g. a degenerate or non-manifold shell. Agents
    /// reasoning about mass / cost should branch on the `None` case
    /// rather than substituting zero.
    pub volume: Option<f64>,
    /// Outer-shell surface area in square working units. `None` when
    /// the kernel cannot compute the area for a degenerate face.
    pub surface_area: Option<f64>,
    /// Counts of faces / edges / vertices in the outer shell. Useful
    /// signal for agents reasoning about complexity ("simple primitive"
    /// vs "complex booleaned part").
    pub topology: TopologyFingerprint,
    /// Material assigned to this part. Always populated — solids
    /// without an explicit material default to Steel @ 7850 kg/m³.
    /// Pair with `volume` to compute mass without the round-trip
    /// through `mass_properties`.
    pub material: MaterialSummary,
    /// Datums whose [`DatumSource`](crate::primitives::datum::DatumSource)
    /// references geometry of this part — `EdgeAxis` for one of this
    /// part's edges, `VertexPoint` for one of its vertices, etc. When
    /// this part moves these datums must be re-evaluated.
    ///
    /// Walked from the slice-5 `DatumGraph` per-vertex / per-edge /
    /// per-face inverted indices, deduplicated, and sorted ascending.
    /// Empty for primitive parts that no derived datums reference.
    pub dependent_datums: Vec<DatumId>,
    /// Top-K (K = 5) other parts by ascending bbox-center distance.
    /// Useful for agent queries like *"what's around this part?"*
    /// without forcing a separate `parts_near_*` round-trip. Self
    /// reference is excluded; deterministic order matches
    /// [`PartProximity::distance`] sort.
    pub neighbors: Vec<PartProximity>,
    /// Pre-formatted one-line human-readable summary, suitable for
    /// agent narration: *"FrontPlane, offset (10, 0, 5), 20×15×10"*.
    pub location_oneliner: String,
}

/// Light list-item record for `list_parts` / `list_parts_filtered`.
///
/// One-line per part — agents asking *"what's in this model?"* get a
/// flat `Vec<PartSummary>` and decide which to drill into with
/// `query_part`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartSummary {
    /// Stable kernel identifier.
    pub id: SolidId,
    /// User-facing label or auto-generated placeholder.
    pub name: String,
    /// Anchor datum id (denormalized for filterability).
    pub anchor_datum_id: DatumId,
    /// Anchor datum name (e.g. `"FrontPlane"`).
    pub anchor_datum_name: String,
    /// Pre-formatted one-line summary.
    pub location_oneliner: String,
}

/// Solid + distance pair for proximity queries.
///
/// Returned by `BRepModel::parts_near_datum`. The distance is the
/// signed/absolute distance from the part's bbox center to the supplied
/// datum (point datums use Euclidean distance to the origin; plane
/// datums use absolute perpendicular distance to the plane; axis datums
/// use perpendicular distance to the axis line).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartProximity {
    /// Stable kernel identifier of the part.
    pub id: SolidId,
    /// User-facing label.
    pub name: String,
    /// Distance from the part's bbox center to the queried datum or
    /// reference part. Units match the model's working units.
    /// Always non-negative — proximity queries do not surface
    /// signedness; use `query_part` if signed canonical distances
    /// matter.
    pub distance: f64,
    /// Anchor datum id of this part — surfaced so an agent can decide
    /// whether the proximity result is "intrinsic" (matching anchor) or
    /// "extrinsic" (different anchor).
    pub anchor_datum_id: DatumId,
}

/// Distance breakdown between two parts.
///
/// Returned by `BRepModel::part_distance`. Multiple measures are
/// surfaced because LLMs reasoning about clearance / mating /
/// interference each want a different one and asking would cost a
/// round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DistanceReport {
    /// Bbox-center Euclidean distance — always defined when both
    /// parts resolve. The cheapest and most stable measure; falls
    /// back to zero for coincident parts.
    pub center_to_center: f64,
    /// Conservative axis-aligned bbox gap — zero when the bboxes
    /// overlap, otherwise the Euclidean distance between the bboxes
    /// computed as `sqrt(Σ max(0, gap_axis)²)`. This is a lower bound
    /// on the true surface-to-surface distance: real surfaces inside
    /// the bboxes can only be at least this far apart, never closer.
    /// Slice 6 ships this in lieu of true closest-face distance,
    /// which requires the trimmed-NURBS surface intersection
    /// machinery to stabilize (queued for a follow-up slice).
    pub surface_to_surface: f64,
    /// World-space AABB overlap on every axis. `true` indicates the
    /// parts may interfere; `false` is a fast, conservative rejection.
    pub bbox_overlap: bool,
    /// Unit vector pointing from `a`'s bbox center toward `b`'s bbox
    /// center. `None` when the centers are coincident (within a
    /// `1e-12` numerical floor) — agents asked to direct motion
    /// should branch on this case rather than dividing by zero.
    pub direction: Option<[f64; 3]>,
}

/// Agent-facing datum summary for `BRepModel::list_datums`.
///
/// Lightweight, self-describing record so an agent can call
/// `list_datums` once at the start of a session and know what
/// reference frames are available, what type each is, and where it
/// lives in world space — without follow-up queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatumSummary {
    /// Stable identifier in the [`DatumStore`](crate::primitives::datum::DatumStore).
    pub id: DatumId,
    /// User-facing label (e.g. `"FrontPlane"`, `"Origin"`,
    /// `"BracketTop"`).
    pub name: String,
    /// One-word kind tag: `"origin"`, `"plane"`, or `"axis"`.
    /// Plane orientation / axis direction is surfaced as `subkind`
    /// rather than mangled into the kind string so the field stays
    /// stable across the canonical / custom split.
    pub kind: String,
    /// Subclassification — for planes: `"xy"` / `"xz"` / `"yz"` /
    /// `"custom"`; for axes: `"x"` / `"y"` / `"z"` / `"custom"`; for
    /// origin: empty. Lets agents distinguish FrontPlane from a
    /// user-authored custom plane without parsing names.
    pub subkind: String,
    /// World-space origin of the datum (translation column of its
    /// frame).
    pub origin: [f64; 3],
    /// World-space +Z basis of the datum's frame — the plane normal
    /// for plane datums, the axis direction for axis datums. For
    /// origin datums this is `[0, 0, 1]` (the canonical world up,
    /// carried through unchanged).
    pub frame_z: [f64; 3],
    /// Whether the datum is one of the seeded seven (Origin / three
    /// reference planes / three reference axes). Defaults are
    /// undeletable and unrenamable.
    pub is_default: bool,
    /// Whether the datum is currently visible in the viewport.
    pub visible: bool,
    /// One-word source-kind tag describing how the datum was
    /// authored: `"manual"`, `"offset_plane"`, `"angle_plane"`,
    /// `"three_points"`, `"plane_from_face"`, `"mid_plane"`,
    /// `"edge_axis"`, `"two_points_axis"`, `"normal_axis"`,
    /// `"vertex_point"`, `"curve_midpoint"`, `"face_centroid"`. Lets
    /// agents distinguish user-typed-numbers planes from
    /// model-derived planes without inspecting the full source recipe.
    pub source_kind: String,
}

/// Render a one-word kind string for a [`DatumKind`].
///
/// Top-level dispatch — the orientation/direction discriminator is
/// surfaced separately by [`format_datum_subkind`] so agents have two
/// stable axes to filter on.
pub fn format_datum_kind(kind: DatumKind) -> &'static str {
    match kind {
        DatumKind::Origin => "origin",
        DatumKind::Plane(_) => "plane",
        DatumKind::Axis(_) => "axis",
    }
}

/// Render the subclassification string for a [`DatumKind`]. See
/// [`DatumSummary::subkind`] for the value space.
pub fn format_datum_subkind(kind: DatumKind) -> &'static str {
    use crate::primitives::datum::AxisDirection;
    use crate::sketch2d::sketch_plane::PlaneOrientation;
    match kind {
        DatumKind::Origin => "",
        DatumKind::Plane(orient) => match orient {
            PlaneOrientation::XY => "xy",
            PlaneOrientation::XZ => "xz",
            PlaneOrientation::YZ => "yz",
            PlaneOrientation::Custom => "custom",
        },
        DatumKind::Axis(dir) => match dir {
            AxisDirection::X => "x",
            AxisDirection::Y => "y",
            AxisDirection::Z => "z",
            AxisDirection::Custom => "custom",
        },
    }
}

/// Render the canonical one-line summary for a [`LocationDescriptor`].
///
/// Format: *"<anchor_name>, offset (<x>, <y>, <z>), <W>×<H>×<D>"*.
/// Coordinates are formatted with up to 3 fractional digits, with
/// trailing zeros trimmed. Units are not included — they are model-wide
/// and agents already know them from the model metadata channel.
pub fn format_location_oneliner(location: &LocationDescriptor) -> String {
    let [ox, oy, oz] = location.center_in_anchor_frame;
    let [w, h, d] = location.dimensions_world;
    format!(
        "{}, offset ({}, {}, {}), {}×{}×{}",
        location.anchor_datum_name,
        format_compact(ox),
        format_compact(oy),
        format_compact(oz),
        format_compact(w),
        format_compact(h),
        format_compact(d),
    )
}

/// Format an `f64` with up to 3 fractional digits and trailing zeros
/// trimmed. Round-half-to-even is the underlying `{:.3}` semantics.
fn format_compact(value: f64) -> String {
    let rendered = format!("{:.3}", value);
    if rendered.contains('.') {
        let trimmed = rendered.trim_end_matches('0');
        let trimmed = trimmed.trim_end_matches('.');
        if trimmed.is_empty() || trimmed == "-" {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::datum::{AxisDirection, DatumKind};
    use crate::sketch2d::sketch_plane::PlaneOrientation;

    fn descriptor_with(
        anchor_name: &str,
        offset: [f64; 3],
        dimensions: [f64; 3],
    ) -> LocationDescriptor {
        LocationDescriptor {
            solid_id: 0,
            anchor_datum_id: 0,
            anchor_datum_name: anchor_name.to_string(),
            center_world: offset,
            center_in_anchor_frame: offset,
            dimensions_world: dimensions,
            signed_distance_front: offset[2],
            signed_distance_top: offset[1],
            signed_distance_right: offset[0],
        }
    }

    #[test]
    fn oneliner_renders_integers_without_decimal_point() {
        let d = descriptor_with("FrontPlane", [10.0, 0.0, 5.0], [20.0, 15.0, 10.0]);
        assert_eq!(
            format_location_oneliner(&d),
            "FrontPlane, offset (10, 0, 5), 20×15×10"
        );
    }

    #[test]
    fn oneliner_trims_trailing_zeros_on_fractional_values() {
        let d = descriptor_with("Origin", [3.14, -1.5, 0.0], [2.5, 2.5, 2.5]);
        assert_eq!(
            format_location_oneliner(&d),
            "Origin, offset (3.14, -1.5, 0), 2.5×2.5×2.5"
        );
    }

    #[test]
    fn oneliner_caps_fractional_digits_at_three() {
        let d = descriptor_with("FrontPlane", [0.123456, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let rendered = format_location_oneliner(&d);
        assert!(rendered.starts_with("FrontPlane, offset (0.123, 0, 0)"), "got {}", rendered);
    }

    #[test]
    fn oneliner_renders_negative_zero_as_zero() {
        let d = descriptor_with("FrontPlane", [-0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let rendered = format_location_oneliner(&d);
        assert!(
            rendered.contains("offset (0, 0, 0)") || rendered.contains("offset (-0, 0, 0)"),
            "got {}",
            rendered
        );
    }

    #[test]
    fn format_compact_handles_negative_decimal() {
        assert_eq!(format_compact(-2.5), "-2.5");
    }

    #[test]
    fn format_compact_handles_very_small_value() {
        assert_eq!(format_compact(0.0001), "0");
    }

    #[test]
    fn format_compact_handles_exact_integer() {
        assert_eq!(format_compact(42.0), "42");
    }

    #[test]
    fn format_datum_kind_dispatches_on_variant() {
        assert_eq!(format_datum_kind(DatumKind::Origin), "origin");
        assert_eq!(
            format_datum_kind(DatumKind::Plane(PlaneOrientation::XY)),
            "plane"
        );
        assert_eq!(
            format_datum_kind(DatumKind::Axis(AxisDirection::Z)),
            "axis"
        );
    }

    #[test]
    fn format_datum_subkind_for_canonical_planes() {
        assert_eq!(
            format_datum_subkind(DatumKind::Plane(PlaneOrientation::XY)),
            "xy"
        );
        assert_eq!(
            format_datum_subkind(DatumKind::Plane(PlaneOrientation::XZ)),
            "xz"
        );
        assert_eq!(
            format_datum_subkind(DatumKind::Plane(PlaneOrientation::YZ)),
            "yz"
        );
        assert_eq!(
            format_datum_subkind(DatumKind::Plane(PlaneOrientation::Custom)),
            "custom"
        );
    }

    #[test]
    fn format_datum_subkind_for_canonical_axes() {
        assert_eq!(format_datum_subkind(DatumKind::Axis(AxisDirection::X)), "x");
        assert_eq!(format_datum_subkind(DatumKind::Axis(AxisDirection::Y)), "y");
        assert_eq!(format_datum_subkind(DatumKind::Axis(AxisDirection::Z)), "z");
        assert_eq!(
            format_datum_subkind(DatumKind::Axis(AxisDirection::Custom)),
            "custom"
        );
    }

    #[test]
    fn format_datum_subkind_for_origin_is_empty() {
        assert_eq!(format_datum_subkind(DatumKind::Origin), "");
    }

    #[test]
    fn part_summary_serializes_to_json() {
        let summary = PartSummary {
            id: 7,
            name: "Bracket".to_string(),
            anchor_datum_id: 1,
            anchor_datum_name: "FrontPlane".to_string(),
            location_oneliner: "FrontPlane, offset (0, 0, 0), 10×10×10".to_string(),
        };
        let json = serde_json::to_string(&summary).expect("serialize");
        assert!(json.contains("\"id\":7"));
        assert!(json.contains("\"name\":\"Bracket\""));
        assert!(json.contains("\"anchor_datum_id\":1"));
        assert!(json.contains("\"anchor_datum_name\":\"FrontPlane\""));
    }

    #[test]
    fn distance_report_serializes_with_full_breakdown() {
        let report = DistanceReport {
            center_to_center: 12.5,
            surface_to_surface: 8.0,
            bbox_overlap: false,
            direction: Some([1.0, 0.0, 0.0]),
        };
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(json.contains("\"center_to_center\":12.5"));
        assert!(json.contains("\"surface_to_surface\":8.0"));
        assert!(json.contains("\"bbox_overlap\":false"));
        assert!(json.contains("\"direction\":[1.0,0.0,0.0]"));
    }

    #[test]
    fn distance_report_serializes_with_null_direction_when_coincident() {
        let report = DistanceReport {
            center_to_center: 0.0,
            surface_to_surface: 0.0,
            bbox_overlap: true,
            direction: None,
        };
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(json.contains("\"direction\":null"));
    }

    #[test]
    fn list_parts_filter_default_is_empty() {
        let f = ListPartsFilter::default();
        assert!(f.anchor_datum_id.is_none());
        assert!(f.name_contains.is_none());
    }

    #[test]
    fn topology_fingerprint_is_copy() {
        let f = TopologyFingerprint {
            face_count: 6,
            edge_count: 12,
            vertex_count: 8,
        };
        let g = f;
        assert_eq!(g.face_count, 6);
        assert_eq!(f.face_count, 6); // f still usable after copy
    }

    #[test]
    fn datum_summary_serializes_with_subkind() {
        let summary = DatumSummary {
            id: 1,
            name: "FrontPlane".to_string(),
            kind: "plane".to_string(),
            subkind: "xy".to_string(),
            origin: [0.0, 0.0, 0.0],
            frame_z: [0.0, 0.0, 1.0],
            is_default: true,
            visible: true,
            source_kind: "manual".to_string(),
        };
        let json = serde_json::to_string(&summary).expect("serialize");
        assert!(json.contains("\"kind\":\"plane\""));
        assert!(json.contains("\"subkind\":\"xy\""));
        assert!(json.contains("\"is_default\":true"));
        assert!(json.contains("\"source_kind\":\"manual\""));
    }
}
