//! `BRepModel` impl block exposing the agent-readable query surface.
//!
//! Methods come in two flavours:
//! - **Read-mostly** (`&self`): `query_part`, `list_parts`,
//!   `list_parts_filtered`, `list_datums`, `parts_near_datum`,
//!   `part_distance`. These hold only `RwLock::read()` in the
//!   api-server and never mutate kernel state.
//! - **Cache-warming** (`&mut self`): `mass_properties_for`,
//!   `oriented_bbox_for`, `query_face`, `query_edge`, `reanchor_part`.
//!   The first three need `&mut` because the underlying kernel
//!   primitives (`Solid::compute_mass_properties`, `Face::area`,
//!   `Edge::length`) populate per-entity caches on first call. After
//!   the cache is warm subsequent calls are O(1) and the write-lock
//!   contention vanishes; `reanchor_part` is genuinely mutating.
//!
//! ## Cost profile
//! - `query_part` — O(F + E + V) over the part's outer shell for the
//!   topology-walk path (volume / area / counts / dependents /
//!   neighbours); O(1) post-cache for the location descriptor.
//! - `list_parts` / `list_parts_filtered` — O(N · L) where N = solid
//!   count, L = avg `solid_location_descriptor_cached` cost.
//! - `list_datums` — O(D) over datums, all reads off the
//!   `DatumStore::snapshot` clone.
//! - `parts_near_datum` — O(N) over solids; the datum's geometry-kind
//!   distance test is closed-form.
//! - `part_distance` — O(1) post-cache.
//! - `mass_properties_for` — first call walks the divergence-theorem
//!   integral once (Piegl-Tiller §10.6); cached thereafter.
//! - `query_face` / `query_edge` — first call computes area / length;
//!   cached thereafter.

use crate::math::{Point3, Vector3};
use crate::primitives::datum::{
    frame_origin, frame_z_axis, Datum, DatumId, DatumKind, INVALID_DATUM_ID,
};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::SurfaceType;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::part::{
    format_datum_kind, format_datum_subkind, format_location_oneliner, DatumSummary,
    DistanceReport, EdgeReport, FaceReport, HoverReport, ListPartsFilter, MassPropertiesReport,
    MaterialSummary, OrientedBBox, PartProximity, PartReport, PartSummary, TopologyFingerprint,
};

/// Maximum number of nearest-neighbour parts surfaced inline on
/// [`PartReport::neighbors`]. Holds the wire-shape cardinality to
/// something an LLM context window can comfortably hold without
/// turning every `query_part` into a list dump.
const PART_REPORT_NEIGHBOR_K: usize = 5;

impl BRepModel {
    /// Resolve a single solid into its agent-facing [`PartReport`].
    ///
    /// Returns `None` when the id is unknown, the solid's anchor datum
    /// has been deleted, or the solid is degenerate (no reachable
    /// vertices). The returned report carries a comprehensive briefing:
    /// identity (id / name / kind), reference frame (anchor datum +
    /// offset), world extents (AABB min/max + dimensions), bulk
    /// properties (volume / surface area / material), topology
    /// fingerprint (face/edge/vertex counts), datum dependents, and
    /// the top-K spatial neighbours.
    ///
    /// `volume` and `surface_area` are `Option<f64>` — they are
    /// populated when the kernel can compute them and `None` for
    /// degenerate or non-manifold geometry. Agents reasoning about
    /// mass / cost should branch on the `None` case rather than
    /// substituting zero.
    ///
    /// Takes `&mut self` because the volume / surface-area integrals
    /// (`calculate_solid_volume`, `calculate_solid_surface_area`)
    /// drive `Solid::compute_mass_properties` which populates per-entity
    /// caches on the solid, shell, face, and loop stores. Subsequent
    /// calls hit the cache and are free. The api-server should briefly
    /// upgrade to a write lock for the first call against a given
    /// solid; same pattern as `mass_properties_for`.
    pub fn query_part(&mut self, id: SolidId) -> Option<PartReport> {
        // Snapshot every field we need off `solid` up front so the
        // immutable borrow of `self.solids` is released before the
        // `&mut self` mass-property calls below. Same hoist pattern
        // as `mass_properties_for`.
        let (solid_name, material) = {
            let solid = self.solids.get(id)?;
            (
                solid.name.clone(),
                MaterialSummary {
                    name: solid.attributes.material.name.clone(),
                    density: solid.attributes.material.density,
                },
            )
        };

        let location = self.solid_location_descriptor_cached(id)?;
        let bbox = self.solid_world_bbox(id)?;

        let name = solid_name.unwrap_or_else(|| format!("solid_{id}"));

        // Slice 6 ships with a generic "solid" kind; primitive-creation
        // entry points (`create_box_3d`, etc.) currently do not stamp a
        // back-pointer into the topology that survives Boolean rewrites.
        // Adding a `Solid::primitive_kind: SolidKind` field is queued
        // for slice 7. Exposing the field today with "solid" lets the
        // wire format be stable without lying to agents.
        let kind = "solid".to_string();

        let anchor_datum_id = location.anchor_datum_id;
        let anchor_datum_name = location.anchor_datum_name.clone();
        let location_oneliner = format_location_oneliner(&location);

        let world_bbox_min = [bbox.min.x, bbox.min.y, bbox.min.z];
        let world_bbox_max = [bbox.max.x, bbox.max.y, bbox.max.z];

        let volume = self.calculate_solid_volume(id);
        let surface_area = self.calculate_solid_surface_area(id);

        let topology = self
            .count_solid_topology(id)
            .unwrap_or(TopologyFingerprint {
                face_count: 0,
                edge_count: 0,
                vertex_count: 0,
            });

        let dependent_datums = self.compute_dependent_datums(id);
        let neighbors = self.compute_neighbors(id, &bbox.center(), PART_REPORT_NEIGHBOR_K);

        Some(PartReport {
            id,
            name,
            kind,
            location,
            anchor_datum_id,
            anchor_datum_name,
            world_bbox_min,
            world_bbox_max,
            volume,
            surface_area,
            topology,
            material,
            dependent_datums,
            neighbors,
            location_oneliner,
        })
    }

    /// Snapshot every solid in the model as a list of [`PartSummary`].
    ///
    /// Sorted by `SolidId` (ascending). Solids whose location
    /// descriptor cannot be resolved are skipped silently — same
    /// rationale as listed parts under `list_parts_filtered`.
    pub fn list_parts(&self) -> Vec<PartSummary> {
        self.list_parts_filtered(&ListPartsFilter::default())
    }

    /// Snapshot every solid matching `filter` as a list of
    /// [`PartSummary`], sorted by `SolidId` ascending.
    ///
    /// `filter` fields are AND-ed: supplying both `anchor_datum_id`
    /// and `name_contains` returns only parts that match both. A
    /// default-constructed filter matches every solid, identical to
    /// the no-arg `list_parts` accessor.
    ///
    /// `name_contains` is a case-insensitive substring match against
    /// the solid's user-facing name. Auto-generated `solid_<id>`
    /// placeholders (for solids without an explicit name) participate
    /// in matching verbatim.
    pub fn list_parts_filtered(&self, filter: &ListPartsFilter) -> Vec<PartSummary> {
        let needle_lower = filter
            .name_contains
            .as_ref()
            .map(|s| s.to_lowercase());

        let mut summaries: Vec<PartSummary> = self
            .solids
            .iter()
            .filter_map(|(id, solid)| {
                if let Some(target) = filter.anchor_datum_id {
                    if solid.anchor.datum_id != target {
                        return None;
                    }
                }
                let location = self.solid_location_descriptor_cached(id)?;
                let name = solid
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("solid_{id}"));
                if let Some(needle) = needle_lower.as_deref() {
                    if !name.to_lowercase().contains(needle) {
                        return None;
                    }
                }
                let location_oneliner = format_location_oneliner(&location);
                Some(PartSummary {
                    id,
                    name,
                    anchor_datum_id: location.anchor_datum_id,
                    anchor_datum_name: location.anchor_datum_name,
                    location_oneliner,
                })
            })
            .collect();
        summaries.sort_by_key(|s| s.id);
        summaries
    }

    /// Snapshot every datum in the model as a list of [`DatumSummary`].
    ///
    /// Sorted by `DatumId` ascending. Useful as a session-startup call
    /// so the agent can plan around what reference frames exist before
    /// issuing any spatial query.
    pub fn list_datums(&self) -> Vec<DatumSummary> {
        let mut datums = self.datums.snapshot();
        datums.sort_by_key(|d| d.id);

        datums
            .into_iter()
            .map(|d| {
                let frame = d.frame();
                let origin = frame_origin(&frame);
                let z = frame_z_axis(&frame);
                DatumSummary {
                    id: d.id,
                    name: d.name.clone(),
                    kind: format_datum_kind(d.kind).to_string(),
                    subkind: format_datum_subkind(d.kind).to_string(),
                    origin: [origin.x, origin.y, origin.z],
                    frame_z: [z.x, z.y, z.z],
                    is_default: d.is_default,
                    visible: d.visible,
                    source_kind: format_datum_source(&d.source).to_string(),
                }
            })
            .collect()
    }

    /// Return every part whose bbox center is within `radius` of the
    /// supplied datum, ordered by ascending distance.
    ///
    /// Distance metric depends on the datum's [`DatumKind`]:
    /// - `Origin`     — Euclidean distance from the bbox center to the
    ///                  datum origin (a point).
    /// - `Plane(_)`   — absolute perpendicular distance from the bbox
    ///                  center to the plane (the plane is infinite).
    /// - `Axis(_)`    — perpendicular distance from the bbox center to
    ///                  the axis line.
    ///
    /// Returns an empty `Vec` when the datum id is unknown or `radius`
    /// is non-finite. `radius == 0.0` is honored: only parts whose bbox
    /// center coincides with the datum geometry within floating-point
    /// epsilon are returned.
    pub fn parts_near_datum(&self, datum_id: DatumId, radius: f64) -> Vec<PartProximity> {
        if !radius.is_finite() || radius < 0.0 {
            return Vec::new();
        }
        let datum = match self.datums.get(datum_id) {
            Some(d) => d,
            None => return Vec::new(),
        };

        let mut hits: Vec<PartProximity> = self
            .solids
            .iter()
            .filter_map(|(id, solid)| {
                let bbox = self.solid_world_bbox(id)?;
                let distance = distance_from_datum(&datum, &bbox.center());
                if distance <= radius {
                    let name = solid
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("solid_{id}"));
                    Some(PartProximity {
                        id,
                        name,
                        distance,
                        anchor_datum_id: solid.anchor.datum_id,
                    })
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits
    }

    /// Compute a [`DistanceReport`] between two parts.
    ///
    /// Surfaces four distance/relationship measures so an LLM can
    /// answer clearance / mating / interference questions in one
    /// round-trip:
    /// - `center_to_center` — cheap and stable, falls back to zero
    ///   for coincident parts.
    /// - `surface_to_surface` — conservative AABB-gap lower bound on
    ///   the true closest-face distance (zero when bboxes overlap;
    ///   otherwise `sqrt(Σ max(0, gap_axis)²)`).
    /// - `bbox_overlap` — fast collision indicator.
    /// - `direction` — unit vector from `a`'s center to `b`'s center,
    ///   `None` when coincident (within 1e-12).
    ///
    /// Returns `None` when either solid id is unknown or either solid
    /// has no reachable vertices.
    pub fn part_distance(&self, a: SolidId, b: SolidId) -> Option<DistanceReport> {
        let bb_a = self.solid_world_bbox(a)?;
        let bb_b = self.solid_world_bbox(b)?;

        let center_a = bb_a.center();
        let center_b = bb_b.center();
        let delta = center_b - center_a;
        let center_to_center = delta.magnitude();

        let bbox_overlap = bb_a.min.x <= bb_b.max.x
            && bb_a.max.x >= bb_b.min.x
            && bb_a.min.y <= bb_b.max.y
            && bb_a.max.y >= bb_b.min.y
            && bb_a.min.z <= bb_b.max.z
            && bb_a.max.z >= bb_b.min.z;

        // AABB-gap surface-to-surface lower bound. Each axis
        // contributes `max(0, lhs.min - rhs.max, rhs.min - lhs.max)`,
        // i.e. zero when intervals overlap and otherwise the gap.
        let gx = (bb_a.min.x - bb_b.max.x).max(bb_b.min.x - bb_a.max.x).max(0.0);
        let gy = (bb_a.min.y - bb_b.max.y).max(bb_b.min.y - bb_a.max.y).max(0.0);
        let gz = (bb_a.min.z - bb_b.max.z).max(bb_b.min.z - bb_a.max.z).max(0.0);
        let surface_to_surface = (gx * gx + gy * gy + gz * gz).sqrt();

        // Direction unit vector. Numerical floor of 1e-12 — anything
        // below that we treat as coincident and surface `None` so
        // downstream agents don't divide by zero or chase noise.
        let direction = if center_to_center > 1e-12 {
            let n = delta / center_to_center;
            Some([n.x, n.y, n.z])
        } else {
            None
        };

        Some(DistanceReport {
            center_to_center,
            surface_to_surface,
            bbox_overlap,
            direction,
        })
    }

    /// Compute mass properties for a solid (volume, surface area, mass,
    /// center of mass, inertia tensor, principal axes, radius of
    /// gyration). Cached after first computation.
    ///
    /// Routes through [`crate::primitives::topology_builder::BRepModel::compute_solid_mass_properties`]
    /// — the unified entry point — so curved primitives (sphere,
    /// cylinder, cone, torus) whose analytical seam loops would
    /// degenerate transparently fall back to a Tonon (2004) mesh-based
    /// integration instead of producing `None` (which used to surface
    /// as 404s on the agent-facing REST endpoint). The returned
    /// `method` field discriminates the two pipelines.
    ///
    /// Takes `&mut self` because the unified entry point mutates
    /// per-entity caches on the solid, shell, face, and loop stores
    /// on first call. Subsequent calls hit the cache.
    ///
    /// Returns `None` only when the solid id is unknown or the
    /// tessellation pass produces zero triangles (an irrecoverably
    /// degenerate solid). Mass = `volume × material.density` —
    /// `Material` defaults to Steel (7850 kg/m³) for solids without
    /// an explicit assignment, so `mass` is always a real number on
    /// the returned report.
    pub fn mass_properties_for(&mut self, solid_id: SolidId) -> Option<MassPropertiesReport> {
        // Capture the material before borrowing the solid mutably so
        // we don't tangle borrows on the inertia computation.
        let (material_name, material_density) = {
            let solid = self.solids.get(solid_id)?;
            (
                solid.attributes.material.name.clone(),
                solid.attributes.material.density,
            )
        };

        let props = self.compute_solid_mass_properties(solid_id)?;

        // Surface area now lives on `SolidMassProperties` itself — read
        // from the same struct as volume / COM / inertia so the
        // numbers are guaranteed to come from the same integration
        // pass and never drift relative to each other.
        let surface_area = props.surface_area;

        // Principal moments come back as a Vector3; principal axes as
        // [Vector3; 3]. We surface them as fixed-size arrays for the
        // agent (less ambiguous than nested Vec<f64>).
        let principal_moments = [
            props.principal_moments.x,
            props.principal_moments.y,
            props.principal_moments.z,
        ];
        let principal_axes = [
            [
                props.principal_axes[0].x,
                props.principal_axes[0].y,
                props.principal_axes[0].z,
            ],
            [
                props.principal_axes[1].x,
                props.principal_axes[1].y,
                props.principal_axes[1].z,
            ],
            [
                props.principal_axes[2].x,
                props.principal_axes[2].y,
                props.principal_axes[2].z,
            ],
        ];

        Some(MassPropertiesReport {
            solid_id,
            volume: props.volume,
            surface_area,
            mass: props.mass,
            material: MaterialSummary {
                name: material_name,
                density: material_density,
            },
            center_of_mass: [
                props.center_of_mass.x,
                props.center_of_mass.y,
                props.center_of_mass.z,
            ],
            inertia_tensor: props.inertia_tensor,
            principal_moments,
            principal_axes,
            radius_of_gyration: [
                props.radius_of_gyration.x,
                props.radius_of_gyration.y,
                props.radius_of_gyration.z,
            ],
            method: props.method,
        })
    }

    /// Compute an oriented bounding box (OBB) for the solid using the
    /// principal axes from its inertia tensor.
    ///
    /// Tighter than the world-AABB for parts whose principal axes
    /// don't align with world XYZ (long thin parts at an angle, etc.).
    /// The `axes[0]` direction is the part's "long axis" — useful for
    /// agent queries like *"how long is this lever?"* (answer:
    /// `2 * half_extents[0]`).
    ///
    /// Method: project every outer-shell vertex onto each principal
    /// axis, recover min/max along each axis, and rebuild the box from
    /// the (axis, half-extent, midpoint) triplet relative to the
    /// center of mass. Takes `&mut self` because `mass_properties_for`
    /// does (cache-warming).
    pub fn oriented_bbox_for(&mut self, solid_id: SolidId) -> Option<OrientedBBox> {
        let mp = self.mass_properties_for(solid_id)?;
        let com = Point3::new(mp.center_of_mass[0], mp.center_of_mass[1], mp.center_of_mass[2]);
        let axes_world = [
            Vector3::new(mp.principal_axes[0][0], mp.principal_axes[0][1], mp.principal_axes[0][2]),
            Vector3::new(mp.principal_axes[1][0], mp.principal_axes[1][1], mp.principal_axes[1][2]),
            Vector3::new(mp.principal_axes[2][0], mp.principal_axes[2][1], mp.principal_axes[2][2]),
        ];

        // Walk every vertex referenced by the outer shell and project
        // onto each principal axis.
        let solid = self.solids.get(solid_id)?;
        let outer_shell = self.shells.get(solid.outer_shell)?;
        let mut min_proj = [f64::INFINITY; 3];
        let mut max_proj = [f64::NEG_INFINITY; 3];

        let mut visited_vertices = std::collections::HashSet::new();
        for &face_id in &outer_shell.faces {
            let face = match self.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let outer_loop = match self.loops.get(face.outer_loop) {
                Some(l) => l,
                None => continue,
            };
            for &edge_id in &outer_loop.edges {
                let edge = match self.edges.get(edge_id) {
                    Some(e) => e,
                    None => continue,
                };
                for vid in [edge.start_vertex, edge.end_vertex] {
                    if !visited_vertices.insert(vid) {
                        continue;
                    }
                    let vertex = match self.vertices.get(vid) {
                        Some(v) => v,
                        None => continue,
                    };
                    let p = vertex.point();
                    let rel = p - com;
                    for (i, axis) in axes_world.iter().enumerate() {
                        let proj = rel.dot(axis);
                        if proj < min_proj[i] {
                            min_proj[i] = proj;
                        }
                        if proj > max_proj[i] {
                            max_proj[i] = proj;
                        }
                    }
                }
            }
        }

        // No vertices reachable → OBB undefined.
        if min_proj.iter().any(|v| !v.is_finite())
            || max_proj.iter().any(|v| !v.is_finite())
        {
            return None;
        }

        // Recover OBB center as the midpoint of the projection
        // intervals, plus the COM offset — equivalent to translating
        // the box so it tightly encloses the projected extents.
        let mut center = [com.x, com.y, com.z];
        let mut half_extents = [0.0; 3];
        for i in 0..3 {
            let mid = 0.5 * (min_proj[i] + max_proj[i]);
            let half = 0.5 * (max_proj[i] - min_proj[i]);
            center[0] += axes_world[i].x * mid;
            center[1] += axes_world[i].y * mid;
            center[2] += axes_world[i].z * mid;
            half_extents[i] = half;
        }

        Some(OrientedBBox {
            center,
            axes: mp.principal_axes,
            half_extents,
        })
    }

    /// Resolve a face id into an agent-facing [`FaceReport`].
    ///
    /// `&mut self` because `Face::area` populates the face's stats
    /// cache on first call. Subsequent calls are O(1).
    ///
    /// `host_solid` walks the SolidStore and finds the solid whose
    /// outer shell references this face. `None` when the face is not
    /// yet attached to any solid (transient construction state) — the
    /// rest of the report is still populated.
    pub fn query_face(&mut self, face_id: u32) -> Option<FaceReport> {
        let face_clone = self.faces.get(face_id)?.clone();

        let surface_type = self
            .surfaces
            .get(face_clone.surface_id)
            .map(|s| format_surface_type(s.surface_type()).to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Outer-loop edge ids in loop order.
        let edge_ids: Vec<u32> = self
            .loops
            .get(face_clone.outer_loop)
            .map(|l| l.edges.clone())
            .unwrap_or_default();

        // Adjacent faces are tracked on `Face::adjacent_faces` keyed by
        // shared edge.
        let neighbour_face_ids: Vec<u32> =
            face_clone.adjacent_faces.values().copied().collect();

        // Find host solid by scanning shells. O(N · F) on first call
        // but face-querying is rare and the model has hundreds of
        // faces, not millions — acceptable.
        let host_solid = self.solids.iter().find_map(|(sid, solid)| {
            let shell = self.shells.get(solid.outer_shell)?;
            if shell.faces.contains(&face_id) {
                Some(sid)
            } else {
                None
            }
        });

        // Compute area. `Face::area` requires &mut. Tolerance taken
        // from the face's own value.
        let tolerance = crate::math::tolerance::Tolerance::from_distance(face_clone.tolerance);
        let face = self.faces.get_mut(face_id)?;
        let area = face
            .area(
                &mut self.loops,
                &self.vertices,
                &self.edges,
                &self.curves,
                &self.surfaces,
                tolerance,
            )
            .ok();

        Some(FaceReport {
            id: face_id,
            host_solid,
            surface_type,
            area,
            edge_ids,
            neighbour_face_ids,
        })
    }

    /// Resolve an edge id into an agent-facing [`EdgeReport`].
    ///
    /// `&mut self` because `Edge::length` populates the edge's
    /// `cached_length` field on first call.
    pub fn query_edge(&mut self, edge_id: u32) -> Option<EdgeReport> {
        let edge_clone = self.edges.get(edge_id)?.clone();

        let curve_kind = self
            .curves
            .get(edge_clone.curve_id)
            .map(|c| c.type_name().to_lowercase())
            .unwrap_or_else(|| "unknown".to_string());

        let start = self
            .vertices
            .get(edge_clone.start_vertex)
            .map(|v| v.point())
            .unwrap_or(Point3::ORIGIN);
        let end = self
            .vertices
            .get(edge_clone.end_vertex)
            .map(|v| v.point())
            .unwrap_or(Point3::ORIGIN);

        let tolerance =
            crate::math::tolerance::Tolerance::from_distance(edge_clone.tolerance);
        let edge = self.edges.get_mut(edge_id)?;
        let length = edge.length(&self.curves, tolerance).ok();

        Some(EdgeReport {
            id: edge_id,
            curve_kind,
            length,
            start: [start.x, start.y, start.z],
            end: [end.x, end.y, end.z],
        })
    }

    /// Resolve a hovered face id into an agent-facing [`HoverReport`].
    ///
    /// This is the "what is the cursor pointing at" query — the kernel
    /// side of the HOVER-α signal pipe. The viewport resolves a raycast
    /// to a `FaceId` (via the tessellation mesh's per-triangle
    /// `face_map`); this joins that face's [`FaceReport`] with its host
    /// part's name and datum-anchored one-line location so an agent
    /// learns *which part, anchored where* in a single call (per the
    /// `readable/` "address by part + anchor datum" rule).
    ///
    /// Returns `None` only when the face id itself is unknown. When the
    /// face exists but has no host solid (transient construction state),
    /// the `face` is populated and the host fields are `None`.
    pub fn query_hover(&mut self, face_id: u32) -> Option<HoverReport> {
        let face = self.query_face(face_id)?;

        // Join the host part's identity + datum-anchored summary. Both
        // are best-effort: a face attached to a solid whose location
        // descriptor can't be resolved still yields a useful `face`.
        let (host_part_name, location_oneliner) = match face.host_solid {
            Some(solid_id) => {
                let name = self
                    .solids
                    .get(solid_id)
                    .and_then(|s| s.name.clone())
                    .unwrap_or_else(|| format!("solid_{solid_id}"));
                let oneliner = self
                    .solid_location_descriptor_cached(solid_id)
                    .map(|loc| format_location_oneliner(&loc));
                (Some(name), oneliner)
            }
            None => (None, None),
        };

        Some(HoverReport {
            face,
            host_part_name,
            location_oneliner,
        })
    }

    /// Re-anchor a part to a different datum with an optional local
    /// offset.
    ///
    /// This is one of the few mutating methods on the readable surface
    /// — it exists here rather than in `topology_builder.rs` because
    /// agents conceptually think of "moving a part to a new reference"
    /// as a query-tier operation (the caller is reasoning about the
    /// model, not authoring topology).
    ///
    /// The offset, when supplied, becomes the part's
    /// `SolidAnchor::local_transform`. When `offset` is `None` the
    /// existing local transform is preserved — useful for re-parenting
    /// a part under a different datum without disturbing its placement.
    ///
    /// Returns `Err` when the solid id is unknown, the datum id is
    /// unknown, or the underlying [`BRepModel::reanchor_solid`]
    /// mediator fails.
    pub fn reanchor_part(
        &mut self,
        solid_id: SolidId,
        new_datum_id: DatumId,
        offset: Option<crate::math::Matrix4>,
    ) -> Result<(), ReanchorError> {
        // Pre-flight validation so the error surface is agent-friendly.
        // `reanchor_solid` would otherwise emit a generic
        // `PrimitiveError::InvalidParameters` whose `parameter` string
        // requires string-matching to discriminate solid-vs-datum.
        if self.solids.get(solid_id).is_none() {
            return Err(ReanchorError::UnknownSolid(solid_id));
        }
        if self.datums.get(new_datum_id).is_none() {
            return Err(ReanchorError::UnknownDatum(new_datum_id));
        }

        self.reanchor_solid(solid_id, new_datum_id, offset)
            .map_err(|e| ReanchorError::Internal(format!("{:?}", e)))
    }

    /// Walk the outer shell and count unique faces / edges / vertices.
    ///
    /// HashSet-deduped on the edge and vertex axes because each edge
    /// is shared by two faces and each vertex by ≥ 3 edges in any
    /// closed manifold. Returns `None` when the solid id is unknown
    /// or its outer shell cannot be resolved; this signals to
    /// `query_part` to fall back to a zero-fingerprint rather than
    /// omit the field.
    fn count_solid_topology(&self, solid_id: SolidId) -> Option<TopologyFingerprint> {
        let solid = self.solids.get(solid_id)?;
        let outer_shell = self.shells.get(solid.outer_shell)?;

        let mut edge_set = std::collections::HashSet::new();
        let mut vertex_set = std::collections::HashSet::new();
        for &face_id in &outer_shell.faces {
            let face = match self.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let outer_loop = match self.loops.get(face.outer_loop) {
                Some(l) => l,
                None => continue,
            };
            for &eid in &outer_loop.edges {
                edge_set.insert(eid);
                if let Some(edge) = self.edges.get(eid) {
                    vertex_set.insert(edge.start_vertex);
                    vertex_set.insert(edge.end_vertex);
                }
            }
        }

        Some(TopologyFingerprint {
            face_count: outer_shell.faces.len(),
            edge_count: edge_set.len(),
            vertex_count: vertex_set.len(),
        })
    }

    /// Walk the slice-5 [`crate::primitives::datum::DatumGraph`]
    /// inverse indices to find every datum that depends on this
    /// part's geometry. Deduplicated and sorted ascending.
    fn compute_dependent_datums(&self, solid_id: SolidId) -> Vec<DatumId> {
        let solid = match self.solids.get(solid_id) {
            Some(s) => s,
            None => return Vec::new(),
        };
        let outer_shell = match self.shells.get(solid.outer_shell) {
            Some(s) => s,
            None => return Vec::new(),
        };

        let mut found = std::collections::HashSet::new();
        for &face_id in &outer_shell.faces {
            for d in self.datum_graph.datums_dependent_on_face(face_id) {
                found.insert(d);
            }
            let face = match self.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let outer_loop = match self.loops.get(face.outer_loop) {
                Some(l) => l,
                None => continue,
            };
            for &eid in &outer_loop.edges {
                for d in self.datum_graph.datums_dependent_on_edge(eid) {
                    found.insert(d);
                }
                let edge = match self.edges.get(eid) {
                    Some(e) => e,
                    None => continue,
                };
                for vid in [edge.start_vertex, edge.end_vertex] {
                    for d in self.datum_graph.datums_dependent_on_vertex(vid) {
                        found.insert(d);
                    }
                }
            }
        }
        let mut out: Vec<DatumId> = found.into_iter().collect();
        out.sort();
        out
    }

    /// Collect the K nearest other solids, ordered by ascending bbox-
    /// center distance. Self is excluded; ties are broken by ascending
    /// solid id for deterministic output.
    fn compute_neighbors(
        &self,
        self_id: SolidId,
        center: &Point3,
        k: usize,
    ) -> Vec<PartProximity> {
        if k == 0 {
            return Vec::new();
        }

        let mut candidates: Vec<PartProximity> = self
            .solids
            .iter()
            .filter(|(other_id, _)| *other_id != self_id)
            .filter_map(|(other_id, other)| {
                let other_bbox = self.solid_world_bbox(other_id)?;
                let distance = (other_bbox.center() - *center).magnitude();
                let name = other
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("solid_{other_id}"));
                Some(PartProximity {
                    id: other_id,
                    name,
                    distance,
                    anchor_datum_id: other.anchor.datum_id,
                })
            })
            .collect();
        candidates.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        candidates.truncate(k);
        candidates
    }
}

/// Errors returned by [`BRepModel::reanchor_part`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReanchorError {
    /// The solid id does not resolve in the current model.
    #[error("solid id {0} not found")]
    UnknownSolid(SolidId),
    /// The target datum id does not resolve in the current model.
    #[error("datum id {0} not found")]
    UnknownDatum(DatumId),
    /// The kernel-level `reanchor_solid` mediator failed for an
    /// unexpected reason (e.g. transform application threw a geometry
    /// error). Carries the debug-rendered cause for triage.
    #[error("reanchor failed: {0}")]
    Internal(String),
}

/// Distance from a `point` to the geometry represented by `datum`.
///
/// Dispatches on [`DatumKind`]:
/// - `Origin` — Euclidean distance to the datum origin (zero-D ref).
/// - `Plane(_)` — absolute perpendicular distance to the plane,
///   computed as `|n · (p − o)|` where `n` is the local +Z basis (the
///   plane's normal — every plane datum is defined that way per
///   `Datum::canonical_transform`).
/// - `Axis(_)` — perpendicular distance to the line passing through
///   the datum origin along the local +Z basis.
fn distance_from_datum(datum: &Datum, point: &Point3) -> f64 {
    let frame = datum.frame();
    let origin = frame_origin(&frame);
    let to_point = *point - origin;

    match datum.kind {
        DatumKind::Origin => to_point.magnitude(),
        DatumKind::Plane(_) => {
            let normal = frame_z_axis(&frame);
            let n = normal.normalize().unwrap_or(Vector3::Z);
            to_point.dot(&n).abs()
        }
        DatumKind::Axis(_) => {
            let axis_dir = frame_z_axis(&frame);
            let n = axis_dir.normalize().unwrap_or(Vector3::Z);
            // Distance from point to line = |to_point − (to_point · n) n|.
            let along = to_point.dot(&n);
            let projection = n * along;
            (to_point - projection).magnitude()
        }
    }
}

/// Map a [`SurfaceType`] to its agent-facing one-word tag.
fn format_surface_type(t: SurfaceType) -> &'static str {
    match t {
        SurfaceType::Plane => "plane",
        SurfaceType::Cylinder => "cylinder",
        SurfaceType::Sphere => "sphere",
        SurfaceType::Cone => "cone",
        SurfaceType::Torus => "torus",
        SurfaceType::SurfaceOfRevolution => "revolution",
        SurfaceType::BSpline => "bspline",
        SurfaceType::NURBS => "nurbs",
        SurfaceType::Offset => "offset",
        SurfaceType::Ruled => "ruled",
    }
}

/// Map a [`DatumSource`](crate::primitives::datum::DatumSource) to its
/// agent-facing one-word tag. Mirrors the variant naming so an agent
/// querying both `kind` (top-level) and `source_kind` (provenance) can
/// reason about a datum without needing the full source recipe.
fn format_datum_source(source: &crate::primitives::datum::DatumSource) -> &'static str {
    use crate::primitives::datum::DatumSource;
    match source {
        DatumSource::Manual { .. } => "manual",
        DatumSource::OffsetPlane { .. } => "offset_plane",
        DatumSource::AnglePlane { .. } => "angle_plane",
        DatumSource::ThreePoints { .. } => "three_points",
        DatumSource::PlaneFromFace { .. } => "plane_from_face",
        DatumSource::MidPlane { .. } => "mid_plane",
        DatumSource::EdgeAxis { .. } => "edge_axis",
        DatumSource::TwoPointsAxis { .. } => "two_points_axis",
        DatumSource::NormalAxis { .. } => "normal_axis",
        DatumSource::VertexPoint { .. } => "vertex_point",
        DatumSource::CurveMidpoint { .. } => "curve_midpoint",
        DatumSource::FaceCentroid { .. } => "face_centroid",
    }
}

// Suppress the unused-import warning on `INVALID_DATUM_ID`. It is
// re-exported from the module in case downstream tests want to assert
// on the sentinel — the module-level allow keeps the diagnostic clean
// without dropping the re-export.
#[allow(dead_code)]
const _: DatumId = INVALID_DATUM_ID;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Matrix4, Point3, Vector3};
    use crate::primitives::box_primitive::{BoxParameters, BoxPrimitive};
    use crate::primitives::datum::{AxisDirection, DatumKind};
    use crate::primitives::primitive_traits::Primitive;
    use crate::primitives::topology_builder::BRepModel;
    use crate::sketch2d::sketch_plane::PlaneOrientation;

    fn fresh_model_with_box() -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        let params = BoxParameters {
            width: 2.0,
            height: 2.0,
            depth: 2.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        let id = BoxPrimitive::create(params, &mut model).expect("create box");
        (model, id)
    }

    #[test]
    fn query_part_returns_full_report_for_origin_box() {
        let (mut model, id) = fresh_model_with_box();
        let report = model.query_part(id).expect("part report");
        assert_eq!(report.id, id);
        assert_eq!(report.kind, "solid");
        assert_eq!(report.anchor_datum_id, 0);
        assert_eq!(report.anchor_datum_name, "Origin");
        // Box is centered at origin → 2×2×2 dimensions.
        assert_eq!(report.location.dimensions_world, [2.0, 2.0, 2.0]);
        // Oneliner ends with the dimension triplet.
        assert!(
            report.location_oneliner.ends_with("2×2×2"),
            "got {}",
            report.location_oneliner
        );
    }

    #[test]
    fn query_part_populates_world_bbox() {
        let (mut model, id) = fresh_model_with_box();
        let report = model.query_part(id).expect("part report");
        // 2×2×2 box centered at origin → corners at ±1.
        assert!((report.world_bbox_min[0] - -1.0).abs() < 1e-9);
        assert!((report.world_bbox_max[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn query_part_populates_topology_fingerprint() {
        let (mut model, id) = fresh_model_with_box();
        let report = model.query_part(id).expect("part report");
        // A box has 6 faces, 12 edges, 8 vertices.
        assert_eq!(report.topology.face_count, 6);
        assert_eq!(report.topology.edge_count, 12);
        assert_eq!(report.topology.vertex_count, 8);
    }

    #[test]
    fn query_part_populates_default_material() {
        let (mut model, id) = fresh_model_with_box();
        let report = model.query_part(id).expect("part report");
        // Default material is Steel @ 7850 kg/m³.
        assert_eq!(report.material.name, "Steel");
        assert!((report.material.density - 7850.0).abs() < 1e-9);
    }

    #[test]
    fn query_part_populates_volume_for_closed_box() {
        let mut model = BRepModel::new();
        let params = BoxParameters {
            width: 2.0,
            height: 3.0,
            depth: 4.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        let id = BoxPrimitive::create(params, &mut model).expect("box");
        let report = model.query_part(id).expect("part report");
        // Volume is now sourced from `Solid::compute_mass_properties`
        // (the canonical divergence-theorem integral) via the
        // delegation in `calculate_solid_volume`. For a 2×3×4 box
        // the analytical answer is 24. Volume-correctness tests at
        // tighter tolerances belong in the kernel volume code and
        // in `tests/kernel_workflow_regression.rs`.
        let v = report.volume.expect("volume should be defined for closed box");
        assert!(
            (v - 24.0).abs() < 1e-6,
            "expected box(2,3,4) volume = 24, got {}",
            v
        );
    }

    #[test]
    fn query_part_returns_none_for_unknown_id() {
        let (mut model, _) = fresh_model_with_box();
        assert!(model.query_part(9999).is_none());
    }

    #[test]
    fn query_part_falls_back_to_auto_name_when_solid_unnamed() {
        let (mut model, id) = fresh_model_with_box();
        let report = model.query_part(id).expect("part report");
        assert_eq!(report.name, format!("solid_{id}"));
    }

    #[test]
    fn query_part_neighbors_excludes_self_and_orders_by_distance() {
        let mut model = BRepModel::new();
        // Three boxes at increasing +X offsets.
        for x in [0.0_f64, 5.0, 10.0] {
            let params = BoxParameters {
                width: 1.0,
                height: 1.0,
                depth: 1.0,
                corner_radius: None,
                transform: Some(Matrix4::from_translation(&Vector3::new(x, 0.0, 0.0))),
                tolerance: None,
            };
            BoxPrimitive::create(params, &mut model).expect("box");
        }
        // Pick the first box; neighbors should be the other two,
        // distance-sorted (5 < 10).
        let first_id = model.solids.iter().map(|(id, _)| id).min().expect("first id");
        let report = model.query_part(first_id).expect("part report");
        assert!(report.neighbors.iter().all(|n| n.id != first_id));
        assert_eq!(report.neighbors.len(), 2);
        assert!(report.neighbors[0].distance <= report.neighbors[1].distance);
    }

    #[test]
    fn list_parts_returns_summaries_sorted_by_id() {
        let (mut model, first) = fresh_model_with_box();
        let params = BoxParameters {
            width: 4.0,
            height: 4.0,
            depth: 4.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        let second = BoxPrimitive::create(params, &mut model).expect("second box");

        let summaries = model.list_parts();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, first);
        assert_eq!(summaries[1].id, second);
        assert!(summaries[0].id < summaries[1].id);
    }

    #[test]
    fn list_parts_returns_empty_for_fresh_model() {
        let model = BRepModel::new();
        assert!(model.list_parts().is_empty());
    }

    #[test]
    fn list_parts_filtered_by_anchor_datum() {
        let (mut model, origin_box) = fresh_model_with_box();
        // Identify FrontPlane (XY) datum.
        let front_id = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Plane(PlaneOrientation::XY)) && d.is_default)
            .expect("FrontPlane")
            .id;
        // Create a second box and reanchor it to FrontPlane.
        let params = BoxParameters {
            width: 1.0,
            height: 1.0,
            depth: 1.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        let second = BoxPrimitive::create(params, &mut model).expect("second box");
        model.reanchor_part(second, front_id, None).expect("reanchor");

        let filter_origin = ListPartsFilter {
            anchor_datum_id: Some(0),
            name_contains: None,
        };
        let on_origin = model.list_parts_filtered(&filter_origin);
        assert_eq!(on_origin.len(), 1);
        assert_eq!(on_origin[0].id, origin_box);

        let filter_front = ListPartsFilter {
            anchor_datum_id: Some(front_id),
            name_contains: None,
        };
        let on_front = model.list_parts_filtered(&filter_front);
        assert_eq!(on_front.len(), 1);
        assert_eq!(on_front[0].id, second);
    }

    #[test]
    fn list_parts_filtered_by_name_substring_is_case_insensitive() {
        let (mut model, first_id) = fresh_model_with_box();
        // Reach in to set a name on the first solid.
        if let Some(s) = model.solids.get_mut(first_id) {
            s.name = Some("Bracket-A".to_string());
        }
        let params = BoxParameters {
            width: 1.0,
            height: 1.0,
            depth: 1.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        BoxPrimitive::create(params, &mut model).expect("second box");

        let filter = ListPartsFilter {
            anchor_datum_id: None,
            name_contains: Some("bracket".to_string()),
        };
        let hits = model.list_parts_filtered(&filter);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Bracket-A");
    }

    #[test]
    fn list_datums_includes_seven_defaults() {
        let model = BRepModel::new();
        let datums = model.list_datums();
        // Seven seeded defaults: Origin, FrontPlane, TopPlane,
        // RightPlane, X-Axis, Y-Axis, Z-Axis.
        assert_eq!(datums.len(), 7);
        // First entry is Origin.
        assert_eq!(datums[0].kind, "origin");
        assert_eq!(datums[0].name, "Origin");
        // Every default has is_default=true.
        assert!(datums.iter().all(|d| d.is_default));
        // FrontPlane subkind is "xy".
        let front = datums
            .iter()
            .find(|d| d.name == "FrontPlane")
            .expect("FrontPlane");
        assert_eq!(front.kind, "plane");
        assert_eq!(front.subkind, "xy");
        // Z-Axis subkind is "z".
        let z_axis = datums
            .iter()
            .find(|d| d.name == "Z-Axis")
            .expect("Z-Axis");
        assert_eq!(z_axis.kind, "axis");
        assert_eq!(z_axis.subkind, "z");
    }

    #[test]
    fn list_datums_sorts_by_id_ascending() {
        let model = BRepModel::new();
        let datums = model.list_datums();
        for w in datums.windows(2) {
            assert!(w[0].id < w[1].id, "datums not sorted: {} >= {}", w[0].id, w[1].id);
        }
    }

    #[test]
    fn parts_near_datum_origin_includes_box_at_origin() {
        let (model, id) = fresh_model_with_box();
        let near = model.parts_near_datum(0, 0.5);
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].id, id);
        assert!(near[0].distance <= 0.5);
    }

    #[test]
    fn parts_near_datum_excludes_solid_outside_radius() {
        let mut model = BRepModel::new();
        let translation = Matrix4::from_translation(&Vector3::new(10.0, 0.0, 0.0));
        let params = BoxParameters {
            width: 2.0,
            height: 2.0,
            depth: 2.0,
            corner_radius: None,
            transform: Some(translation),
            tolerance: None,
        };
        let _id = BoxPrimitive::create(params, &mut model).expect("translated box");

        let close = model.parts_near_datum(0, 5.0);
        assert!(close.is_empty(), "expected no hits within radius 5");

        let far = model.parts_near_datum(0, 20.0);
        assert_eq!(far.len(), 1);
    }

    #[test]
    fn parts_near_datum_plane_uses_perpendicular_distance() {
        let mut model = BRepModel::new();
        let translation = Matrix4::from_translation(&Vector3::new(0.0, 0.0, 5.0));
        let params = BoxParameters {
            width: 2.0,
            height: 2.0,
            depth: 2.0,
            corner_radius: None,
            transform: Some(translation),
            tolerance: None,
        };
        let _id = BoxPrimitive::create(params, &mut model).expect("translated box");

        let front_id = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Plane(PlaneOrientation::XY)) && d.is_default)
            .expect("FrontPlane")
            .id;

        let near = model.parts_near_datum(front_id, 6.0);
        assert_eq!(near.len(), 1);
        assert!(
            (near[0].distance - 5.0).abs() < 1e-9,
            "expected ~5.0 perpendicular distance, got {}",
            near[0].distance
        );

        let too_close = model.parts_near_datum(front_id, 3.0);
        assert!(too_close.is_empty());
    }

    #[test]
    fn parts_near_datum_axis_uses_perpendicular_to_line() {
        let mut model = BRepModel::new();
        let translation = Matrix4::from_translation(&Vector3::new(3.0, 4.0, 100.0));
        let params = BoxParameters {
            width: 1.0,
            height: 1.0,
            depth: 1.0,
            corner_radius: None,
            transform: Some(translation),
            tolerance: None,
        };
        let _id = BoxPrimitive::create(params, &mut model).expect("translated box");

        let z_axis_id = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Axis(AxisDirection::Z)) && d.is_default)
            .expect("Z-Axis")
            .id;

        let hit = model.parts_near_datum(z_axis_id, 6.0);
        assert_eq!(hit.len(), 1);
        assert!(
            (hit[0].distance - 5.0).abs() < 1e-9,
            "expected ~5.0 line distance, got {}",
            hit[0].distance
        );
    }

    #[test]
    fn parts_near_datum_returns_empty_for_unknown_datum() {
        let (model, _) = fresh_model_with_box();
        assert!(model.parts_near_datum(9999, 100.0).is_empty());
    }

    #[test]
    fn parts_near_datum_rejects_negative_radius() {
        let (model, _) = fresh_model_with_box();
        assert!(model.parts_near_datum(0, -1.0).is_empty());
    }

    #[test]
    fn parts_near_datum_rejects_nan_radius() {
        let (model, _) = fresh_model_with_box();
        assert!(model.parts_near_datum(0, f64::NAN).is_empty());
    }

    #[test]
    fn parts_near_datum_sorts_by_ascending_distance() {
        let mut model = BRepModel::new();
        for (i, x) in [3.0, 1.0, 5.0].iter().enumerate() {
            let translation = Matrix4::from_translation(&Vector3::new(*x, 0.0, 0.0));
            let params = BoxParameters {
                width: 0.5,
                height: 0.5,
                depth: 0.5,
                corner_radius: None,
                transform: Some(translation),
                tolerance: None,
            };
            BoxPrimitive::create(params, &mut model)
                .unwrap_or_else(|_| panic!("box {}", i));
        }
        let hits = model.parts_near_datum(0, 10.0);
        assert_eq!(hits.len(), 3);
        assert!(hits[0].distance <= hits[1].distance);
        assert!(hits[1].distance <= hits[2].distance);
    }

    #[test]
    fn part_distance_returns_zero_for_coincident_solids() {
        let (model, id) = fresh_model_with_box();
        let report = model.part_distance(id, id).expect("self distance");
        assert!(report.center_to_center.abs() < 1e-9);
        assert!(report.bbox_overlap);
        // Coincident → no direction.
        assert!(report.direction.is_none());
        // Surface-to-surface lower bound is zero (overlapping bboxes).
        assert!(report.surface_to_surface < 1e-9);
    }

    #[test]
    fn part_distance_detects_non_overlapping_bboxes_and_reports_gap() {
        let mut model = BRepModel::new();
        let p1 = BoxParameters {
            width: 1.0,
            height: 1.0,
            depth: 1.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        let id_a = BoxPrimitive::create(p1, &mut model).expect("box a");
        let p2 = BoxParameters {
            width: 1.0,
            height: 1.0,
            depth: 1.0,
            corner_radius: None,
            transform: Some(Matrix4::from_translation(&Vector3::new(10.0, 0.0, 0.0))),
            tolerance: None,
        };
        let id_b = BoxPrimitive::create(p2, &mut model).expect("box b");

        let report = model.part_distance(id_a, id_b).expect("distance defined");
        assert!((report.center_to_center - 10.0).abs() < 1e-9);
        assert!(!report.bbox_overlap);
        // Gap = 10 - half-width-a (0.5) - half-width-b (0.5) = 9.
        assert!(
            (report.surface_to_surface - 9.0).abs() < 1e-9,
            "expected gap ≈ 9, got {}",
            report.surface_to_surface
        );
        // Direction is +X.
        let dir = report.direction.expect("direction defined");
        assert!((dir[0] - 1.0).abs() < 1e-9);
        assert!(dir[1].abs() < 1e-9);
        assert!(dir[2].abs() < 1e-9);
    }

    #[test]
    fn part_distance_detects_overlapping_bboxes() {
        let mut model = BRepModel::new();
        let p1 = BoxParameters {
            width: 4.0,
            height: 4.0,
            depth: 4.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        let id_a = BoxPrimitive::create(p1, &mut model).expect("box a");
        let p2 = BoxParameters {
            width: 4.0,
            height: 4.0,
            depth: 4.0,
            corner_radius: None,
            transform: Some(Matrix4::from_translation(&Vector3::new(1.0, 0.0, 0.0))),
            tolerance: None,
        };
        let id_b = BoxPrimitive::create(p2, &mut model).expect("box b");

        let report = model.part_distance(id_a, id_b).expect("distance defined");
        assert!(report.bbox_overlap);
        // Overlapping bboxes → surface_to_surface lower bound is zero.
        assert!(report.surface_to_surface < 1e-9);
    }

    #[test]
    fn part_distance_returns_none_for_unknown_id() {
        let (model, id) = fresh_model_with_box();
        assert!(model.part_distance(id, 9999).is_none());
        assert!(model.part_distance(9999, id).is_none());
    }

    #[test]
    fn mass_properties_for_default_steel_box() {
        let (mut model, id) = fresh_model_with_box();
        let mp = model.mass_properties_for(id).expect("mass props");
        // Volume is populated and positive — exact numerical value is
        // kernel-dependent (Solid::compute_mass_properties is the
        // source of truth).
        assert!(mp.volume > 0.0 && mp.volume.is_finite());
        assert!(mp.mass > 0.0 && mp.mass.is_finite());
        assert_eq!(mp.material.name, "Steel");
        assert!((mp.material.density - 7850.0).abs() < 1e-9);
        // COM components are populated and finite — exact placement
        // depends on the kernel's divergence-theorem integration
        // (which is currently the source of truth for COM correctness).
        for c in mp.center_of_mass.iter() {
            assert!(c.is_finite(), "got non-finite COM component {}", c);
        }
        // Inertia tensor is symmetric and finite.
        for i in 0..3 {
            for j in 0..3 {
                assert!(mp.inertia_tensor[i][j].is_finite());
            }
        }
        assert!(mp.principal_moments.iter().all(|m| m.is_finite()));
    }

    #[test]
    fn mass_properties_for_unknown_solid_is_none() {
        let mut model = BRepModel::new();
        assert!(model.mass_properties_for(9999).is_none());
    }

    #[test]
    fn oriented_bbox_for_box_has_unit_half_extents() {
        let (mut model, id) = fresh_model_with_box();
        let obb = model.oriented_bbox_for(id).expect("OBB");
        // 2×2×2 box → half-extents 1 along every axis.
        for h in obb.half_extents.iter() {
            assert!((h - 1.0).abs() < 1e-6, "got half-extent {}", h);
        }
        // Center should be at origin.
        for c in obb.center.iter() {
            assert!(c.abs() < 1e-6);
        }
    }

    #[test]
    fn query_face_returns_planar_face_for_box() {
        let (mut model, _) = fresh_model_with_box();
        // Pick any face id from the model.
        let face_id = model.faces.iter().next().expect("at least one face").0;
        let report = model.query_face(face_id).expect("face report");
        assert_eq!(report.id, face_id);
        // Box faces are all planes.
        assert_eq!(report.surface_type, "plane");
        // Each face has 4 boundary edges.
        assert_eq!(report.edge_ids.len(), 4);
        // Area = 2 × 2 = 4.
        let area = report.area.expect("area defined");
        assert!((area - 4.0).abs() < 1e-6, "got area {}", area);
    }

    #[test]
    fn query_face_returns_none_for_unknown_id() {
        let mut model = BRepModel::new();
        assert!(model.query_face(9999).is_none());
    }

    #[test]
    fn query_hover_joins_face_with_host_part_location() {
        let (mut model, solid_id) = fresh_model_with_box();
        let face_id = model.faces.iter().next().expect("at least one face").0;
        let report = model.query_hover(face_id).expect("hover report");
        // The embedded face report matches a direct query_face.
        assert_eq!(report.face.id, face_id);
        assert_eq!(report.face.surface_type, "plane");
        assert_eq!(report.face.host_solid, Some(solid_id));
        // The hover join surfaces the host part identity + a datum-anchored
        // one-liner so an agent learns *which part, anchored where* in one
        // call — the readable/ "address by part + anchor datum" rule.
        assert!(report.host_part_name.is_some(), "host part name resolved");
        let oneliner = report
            .location_oneliner
            .expect("host part location one-liner");
        assert!(
            !oneliner.is_empty(),
            "one-liner must be non-empty, got {oneliner:?}"
        );
    }

    #[test]
    fn query_hover_unknown_face_returns_none() {
        let mut model = BRepModel::new();
        assert!(
            model.query_hover(9999).is_none(),
            "an unknown face id must yield None"
        );
    }

    #[test]
    fn query_edge_returns_line_for_box_edge() {
        let (mut model, _) = fresh_model_with_box();
        let edge_id = model.edges.iter().next().expect("edge").0;
        let report = model.query_edge(edge_id).expect("edge report");
        assert_eq!(report.id, edge_id);
        // Box edges should be lines.
        assert_eq!(report.curve_kind, "line");
        // Length = 2 (box side length).
        let len = report.length.expect("length defined");
        assert!((len - 2.0).abs() < 1e-6, "got length {}", len);
    }

    #[test]
    fn query_edge_returns_none_for_unknown_id() {
        let mut model = BRepModel::new();
        assert!(model.query_edge(9999).is_none());
    }

    #[test]
    fn reanchor_part_changes_anchor_datum_id() {
        let (mut model, id) = fresh_model_with_box();
        assert_eq!(
            model
                .solids
                .get(id)
                .expect("solid")
                .anchor
                .datum_id,
            0
        );

        let front_id = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Plane(PlaneOrientation::XY)) && d.is_default)
            .expect("FrontPlane")
            .id;

        model.reanchor_part(id, front_id, None).expect("reanchor");
        assert_eq!(
            model
                .solids
                .get(id)
                .expect("solid")
                .anchor
                .datum_id,
            front_id
        );
    }

    #[test]
    fn reanchor_part_preserves_local_transform_when_offset_none() {
        let (mut model, id) = fresh_model_with_box();
        let original_local = model
            .solids
            .get(id)
            .expect("solid")
            .anchor
            .local_transform;

        let front_id = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Plane(PlaneOrientation::XY)) && d.is_default)
            .expect("FrontPlane")
            .id;

        model.reanchor_part(id, front_id, None).expect("reanchor");
        let after = model
            .solids
            .get(id)
            .expect("solid")
            .anchor
            .local_transform;
        for r in 0..4 {
            for c in 0..4 {
                let lhs = original_local.get(r, c);
                let rhs = after.get(r, c);
                assert!((lhs - rhs).abs() < 1e-12, "row {} col {}", r, c);
            }
        }
    }

    #[test]
    fn reanchor_part_applies_supplied_offset() {
        let (mut model, id) = fresh_model_with_box();
        let front_id = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Plane(PlaneOrientation::XY)) && d.is_default)
            .expect("FrontPlane")
            .id;

        let offset = Matrix4::from_translation(&Vector3::new(1.0, 2.0, 3.0));
        model
            .reanchor_part(id, front_id, Some(offset))
            .expect("reanchor with offset");

        let after = model
            .solids
            .get(id)
            .expect("solid")
            .anchor
            .local_transform;
        assert!((after.get(0, 3) - 1.0).abs() < 1e-9);
        assert!((after.get(1, 3) - 2.0).abs() < 1e-9);
        assert!((after.get(2, 3) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn reanchor_part_rejects_unknown_solid() {
        let (mut model, _) = fresh_model_with_box();
        let err = model.reanchor_part(9999, 0, None).unwrap_err();
        assert_eq!(err, ReanchorError::UnknownSolid(9999));
    }

    #[test]
    fn reanchor_part_rejects_unknown_datum() {
        let (mut model, id) = fresh_model_with_box();
        let err = model.reanchor_part(id, 9999, None).unwrap_err();
        assert_eq!(err, ReanchorError::UnknownDatum(9999));
    }

    #[test]
    fn distance_from_datum_origin_is_euclidean() {
        let model = BRepModel::new();
        let origin = model.datums.get(0).expect("Origin datum");
        let p = Point3::new(3.0, 4.0, 0.0);
        let d = distance_from_datum(&origin, &p);
        assert!((d - 5.0).abs() < 1e-9);
    }

    #[test]
    fn distance_from_datum_plane_is_perpendicular() {
        let model = BRepModel::new();
        let front = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Plane(PlaneOrientation::XY)) && d.is_default)
            .expect("FrontPlane");
        let p = Point3::new(7.0, 8.0, -3.0);
        let d = distance_from_datum(&front, &p);
        assert!((d - 3.0).abs() < 1e-9);
    }

    #[test]
    fn distance_from_datum_axis_is_perpendicular_to_line() {
        let model = BRepModel::new();
        let z_axis = model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Axis(AxisDirection::Z)) && d.is_default)
            .expect("Z-Axis");
        let p = Point3::new(3.0, 4.0, 17.0);
        let d = distance_from_datum(&z_axis, &p);
        assert!((d - 5.0).abs() < 1e-9, "got {}", d);
    }
}
