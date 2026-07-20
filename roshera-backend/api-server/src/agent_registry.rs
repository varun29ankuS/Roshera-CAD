//! Kernel-served agent tool registry — Slice 1 / Layer 0 of the MCP scale
//! architecture (spec `2026-07-20-mcp-scale-architecture-design.md`, §2 Layer 0
//! + §3 honesty contract).
//!
//! `GET /api/agent/tool-registry` is the single server-side source of truth for
//! the agent-facing operation inventory. Today the MCP server hand-maintains
//! 90+ tool definitions in TypeScript; the spec's worst-case axiom (300+ ops,
//! weakest client) makes duplication untenable. Serving the inventory means a
//! new operation scales by ONE registration here, not by editing every client.
//!
//! ## Source-of-truth layering (honest, per the spec)
//! - `SchemaSource::Kernel` — the operation is registered in geometry-engine's
//!   `ai_operations_registry`. Its one-line `purpose` is GENERATED from that
//!   kernel registration at serve time (with a compiled fallback). The kernel
//!   is the authority on the operation's semantics; the wire schema below still
//!   curates the transport framing the kernel op does not model (object UUIDs,
//!   the `all_edges` convenience, placement echoes).
//! - `SchemaSource::Curated` — transcribed verbatim from the live zod wire
//!   contract in `roshera-mcp/src/tools/*.ts`. Many tools are api-server-level,
//!   not kernel ops (`render_part`, `scene_view`, `drill_pattern`, the whole
//!   drawing / assembly / labels surface), so the wire contract IS the source.
//!   Schemas are never invented — every field traces to a zod definition.
//!
//! It is expected for Slice 1 that most entries are curated; the kernel-
//! generated share grows as the kernel op registry does.
//!
//! ## Determinism
//! The tool table is a compiled constant; `generated_at` is the only per-
//! request field. `registry_hash` is a pure function of the canonicalized
//! tools array (sorted keys, FNV-1a-64) so the MCP can detect drift against its
//! compiled fallback snapshot and refuse loudly on mismatch (spec §3.4).

use axum::Json;
use geometry_engine::operations::ai_operations_registry::OperationsRegistry;
use serde_json::{json, Value};

/// The five benches + core (spec §Layer 1). Open-Q1 resolution: GD&T lives
/// inside the `drawing` bench rather than its own. Progressive-disclosure
/// category tag; benches optimise attention, they never gate capability
/// (§3.1) — that contract is enforced in the MCP layer (Slices 2–3), the
/// registry only carries the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bench {
    Core,
    Sketch,
    Assembly,
    Drawing,
    Analysis,
    Labels,
}

impl Bench {
    pub fn as_str(self) -> &'static str {
        match self {
            Bench::Core => "core",
            Bench::Sketch => "sketch",
            Bench::Assembly => "assembly",
            Bench::Drawing => "drawing",
            Bench::Analysis => "analysis",
            Bench::Labels => "labels",
        }
    }
}

/// Maturity tag. `Experimental` marks recently-landed or known-fragile surface
/// (kinematic-assembly slice, #64 timeline mould, #55 drawing readback, the
/// solver-citizen sketch op family, composed/freeform geometry) so a client can
/// prefer the settled core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    Stable,
    Experimental,
}

impl Stability {
    fn as_str(self) -> &'static str {
        match self {
            Stability::Stable => "stable",
            Stability::Experimental => "experimental",
        }
    }
}

/// Provenance of a registry entry's schema/description (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaSource {
    Kernel,
    Curated,
}

impl SchemaSource {
    fn as_str(self) -> &'static str {
        match self {
            SchemaSource::Kernel => "kernel",
            SchemaSource::Curated => "curated",
        }
    }
    /// For `Kernel` entries, the operation name in geometry-engine's
    /// `ai_operations_registry` whose description is pulled as the `purpose`.
    fn kernel_op(self, tool_name: &str) -> Option<&'static str> {
        if self != SchemaSource::Kernel {
            return None;
        }
        match tool_name {
            "boolean" => Some("boolean"),
            "fillet_edges" => Some("fillet"),
            "chamfer_edges" => Some("chamfer"),
            "revolve" => Some("revolve"),
            _ => None,
        }
    }
}

/// One row of the single tool table. `bench` is a column here — the bench
/// assignment is NOT scattered across the schema declarations.
struct ToolSpec {
    name: &'static str,
    bench: Bench,
    stability: Stability,
    source: SchemaSource,
    /// Compiled one-line purpose. For `SchemaSource::Kernel` rows this is the
    /// fallback used when the kernel registry cannot be read.
    purpose: &'static str,
    schema: Value,
}

fn t(
    name: &'static str,
    bench: Bench,
    stability: Stability,
    source: SchemaSource,
    purpose: &'static str,
    schema: Value,
) -> ToolSpec {
    ToolSpec {
        name,
        bench,
        stability,
        source,
        purpose,
        schema,
    }
}

// ── Small schema helpers (keep the table readable, faithful to zod) ────────

/// A `z.tuple([number; n])` — a fixed-length numeric array.
fn ntuple(n: u64, desc: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "number"},
        "minItems": n,
        "maxItems": n,
        "description": desc,
    })
}

/// The `PlaneSchema` union: `'xy'|'xz'|'yz'` or `{origin,u_axis,v_axis}`.
fn plane_schema(default_xy: bool) -> Value {
    let mut v = json!({
        "oneOf": [
            {"type": "string", "enum": ["xy", "xz", "yz"]},
            {
                "type": "object",
                "properties": {
                    "origin": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3},
                    "u_axis": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3},
                    "v_axis": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3}
                },
                "required": ["origin", "u_axis", "v_axis"]
            }
        ],
        "description": "'xy' | 'xz' | 'yz' or {origin, u_axis, v_axis} (e.g. from plane_from_face)"
    });
    if default_xy {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("default".to_string(), json!("xy"));
        }
    }
    v
}

// ───────────────────────────────────────────────────────────────────────────
// THE SINGLE TOOL TABLE
//
// One row per tool, bench as a column. Schemas transcribed from the live zod
// contract in roshera-mcp/src/tools/*.ts (read 2026-07-20). Order here is by
// bench for reading; the served array is sorted by name for a stable hash.
// ───────────────────────────────────────────────────────────────────────────
fn raw_tools() -> Vec<ToolSpec> {
    use Bench::*;
    use SchemaSource::*;
    use Stability::*;

    vec![
        // ═══════════════════ CORE ═══════════════════
        t(
            "create_box",
            Core,
            Stable,
            Curated,
            "One-call analytic box: width×depth on a plane, extruded height along the plane normal.",
            json!({
                "type": "object",
                "properties": {
                    "plane": plane_schema(true),
                    "cx": {"type": "number", "default": 0, "description": "base-centre offset along plane u (mm)"},
                    "cy": {"type": "number", "default": 0, "description": "base-centre offset along plane v (mm)"},
                    "center": ntuple(3, "explicit world base-centre [x,y,z] mm; overrides plane+cx+cy"),
                    "width": {"type": "number", "exclusiveMinimum": 0, "description": "size along plane u (mm)"},
                    "depth": {"type": "number", "exclusiveMinimum": 0, "description": "size along plane v (mm)"},
                    "height": {"type": "number", "description": "extrusion along the normal (mm)"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["width", "depth", "height"]
            }),
        ),
        t(
            "create_cylinder",
            Core,
            Stable,
            Curated,
            "One-call analytic cylinder; base-face centre at center (or cx,cy on plane), extruded height along +axis.",
            json!({
                "type": "object",
                "properties": {
                    "plane": plane_schema(true),
                    "cx": {"type": "number", "default": 0, "description": "base-centre offset along plane u (mm)"},
                    "cy": {"type": "number", "default": 0, "description": "base-centre offset along plane v (mm)"},
                    "center": ntuple(3, "explicit world base-centre [x,y,z] mm; overrides plane+cx+cy"),
                    "axis": ntuple(3, "extrusion direction [x,y,z]; defaults to the plane normal"),
                    "radius": {"type": "number", "exclusiveMinimum": 0, "description": "cylinder radius (mm)"},
                    "height": {"type": "number", "description": "extrusion length along +axis (mm)"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["radius", "height"]
            }),
        ),
        t(
            "create_sphere",
            Core,
            Stable,
            Curated,
            "One-call analytic sphere of radius at center.",
            json!({
                "type": "object",
                "properties": {
                    "radius": {"type": "number", "exclusiveMinimum": 0, "description": "sphere radius (mm)"},
                    "center": ntuple(3, "world centre [x,y,z] mm; defaults to the origin"),
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["radius"]
            }),
        ),
        t(
            "create_cone",
            Core,
            Stable,
            Curated,
            "One-call analytic cone or frustum with a true smooth cone surface (top_radius 0 = sharp apex).",
            json!({
                "type": "object",
                "properties": {
                    "center": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "default": [0, 0, 0], "description": "world base-face centre [x,y,z] mm"},
                    "axis": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "default": [0, 0, 1], "description": "apex direction from the base [x,y,z]"},
                    "base_radius": {"type": "number", "minimum": 0, "description": "radius at the base (mm)"},
                    "top_radius": {"type": "number", "minimum": 0, "default": 0, "description": "radius at the top (mm); 0 = sharp apex"},
                    "height": {"type": "number", "exclusiveMinimum": 0, "description": "base-to-top distance along axis (mm)"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["base_radius", "height"]
            }),
        ),
        t(
            "boolean",
            Core,
            Stable,
            Kernel,
            "Combine two solids by object UUID (union/difference/intersection); both operands consumed, a new solid is born.",
            json!({
                "type": "object",
                "properties": {
                    "op": {"type": "string", "enum": ["union", "difference", "intersection"], "description": "difference cuts object_b out of object_a"},
                    "object_a": {"type": "string", "format": "uuid", "description": "object_uuid of the base solid"},
                    "object_b": {"type": "string", "format": "uuid", "description": "object_uuid of the tool solid"}
                },
                "required": ["op", "object_a", "object_b"]
            }),
        ),
        t(
            "boolean_many",
            Core,
            Stable,
            Curated,
            "Batch boolean: apply many tool solids against one base sequentially; certified per step, halts at the first unsound step.",
            json!({
                "type": "object",
                "properties": {
                    "op": {"type": "string", "enum": ["union", "difference"], "description": "operation applied at each step"},
                    "base": {"type": "string", "format": "uuid", "description": "object_uuid of the base solid (kept)"},
                    "tools": {"type": "array", "items": {"type": "string", "format": "uuid"}, "minItems": 1, "maxItems": 64, "description": "object_uuids applied in order; all consumed"}
                },
                "required": ["op", "base", "tools"]
            }),
        ),
        t(
            "revolve",
            Core,
            Stable,
            Kernel,
            "Solid of revolution from a closed [r,z] meridian swept about an axis; typed profile_segments revolve to exact surfaces.",
            json!({
                "type": "object",
                "properties": {
                    "profile": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 3, "description": "closed [r,z] meridian (mm); auto-closes last->first. Exclusive with profile_segments."},
                    "profile_segments": {
                        "type": "array",
                        "minItems": 1,
                        "description": "typed [r,z] meridian segments in loop order (line/arc/nurbs); full-360 only. Exclusive with profile/smooth/bore_radius/wall_thickness.",
                        "items": {
                            "oneOf": [
                                {"type": "object", "properties": {"type": {"const": "line"}, "start": ntuple(2, "[r,z] start (mm)"), "end": ntuple(2, "[r,z] end (mm)")}, "required": ["type", "start", "end"]},
                                {"type": "object", "properties": {"type": {"const": "arc"}, "center": ntuple(2, "arc centre [r,z] (mm)"), "radius": {"type": "number", "exclusiveMinimum": 0}, "start_angle": {"type": "number", "description": "radians"}, "end_angle": {"type": "number", "description": "radians"}, "ccw": {"type": "boolean"}}, "required": ["type", "center", "radius", "start_angle", "end_angle", "ccw"]},
                                {"type": "object", "properties": {"type": {"const": "nurbs"}, "degree": {"type": "integer", "minimum": 1, "maximum": 7}, "control_points": {"type": "array", "items": ntuple(2, "[r,z] (mm)"), "minItems": 2}, "weights": {"type": "array", "items": {"type": "number"}}, "knots": {"type": "array", "items": {"type": "number"}}}, "required": ["type", "degree", "control_points", "knots"]}
                            ]
                        }
                    },
                    "axis_origin": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "default": [0, 0, 0], "description": "point on the revolution axis [x,y,z] mm"},
                    "axis_direction": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "default": [0, 0, 1], "description": "revolution axis direction [x,y,z]"},
                    "angle_deg": {"type": "number", "default": 360, "description": "sweep angle in degrees (profile_segments must be 360)"},
                    "segments": {"type": "integer", "minimum": 3, "maximum": 512, "default": 96, "description": "angular tessellation count for the sampled path"},
                    "smooth": {"type": "boolean", "description": "sampled-profile mode: fit a smooth NURBS wall (needs bore_radius)"},
                    "bore_radius": {"type": "number", "description": "hollow bore radius (mm) for a smooth-walled tube (with smooth=true)"},
                    "wall_thickness": {"type": "number", "description": "contoured nozzle/vessel: profile is INNER contour, outer wall offset by this (mm)"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": []
            }),
        ),
        t(
            "nurbs_loft",
            Core,
            Experimental,
            Curated,
            "Watertight freeform solid: skin one NURBS surface through a stack of cross-section rings (first/last become planar caps).",
            json!({
                "type": "object",
                "properties": {
                    "sections": {"type": "array", "minItems": 2, "items": {"type": "array", "minItems": 3, "items": ntuple(3, "[x,y,z] point (mm)")}, "description": "ordered stack of OPEN rings, SAME count each (auto-closed); first/last must be planar"},
                    "degree_u": {"type": "integer", "minimum": 1, "maximum": 7, "default": 3, "description": "NURBS degree around each section"},
                    "degree_v": {"type": "integer", "minimum": 1, "maximum": 7, "default": 3, "description": "NURBS degree along the loft (3 = G2 continuity)"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["sections"]
            }),
        ),
        t(
            "shell",
            Core,
            Stable,
            Curated,
            "Hollow a solid to a constant wall thickness (grows inward), opening the listed cap faces. Always verify_part after.",
            json!({
                "type": "object",
                "properties": {
                    "object": {"type": "string", "format": "uuid", "description": "object_uuid of the solid to hollow"},
                    "thickness": {"type": "number", "description": "inward wall thickness (mm); must be non-zero"},
                    "faces_to_remove": {"type": "array", "items": {"type": "integer", "minimum": 0}, "default": [], "description": "cap face ids to open; [] = fully closed void"}
                },
                "required": ["object", "thickness"]
            }),
        ),
        t(
            "fillet_edges",
            Core,
            Stable,
            Kernel,
            "Round (fillet) edges with a constant radius; omit edge_ids to blend all edges. Identity-preserving.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "radius": {"type": "number", "exclusiveMinimum": 0, "description": "fillet radius (mm)"},
                    "edge_ids": {"type": "array", "items": {"type": "integer", "minimum": 0}, "description": "edges to round (from select_edge or a render 'ids' legend); omit for ALL edges"}
                },
                "required": ["part_id", "radius"]
            }),
        ),
        t(
            "chamfer_edges",
            Core,
            Stable,
            Kernel,
            "Bevel (chamfer) edges with an equal-distance flat setback on each adjacent face; omit edge_ids to chamfer all edges.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "distance": {"type": "number", "exclusiveMinimum": 0, "description": "setback distance on each face (mm)"},
                    "edge_ids": {"type": "array", "items": {"type": "integer", "minimum": 0}, "description": "edges to bevel; omit for ALL edges"}
                },
                "required": ["part_id", "distance"]
            }),
        ),
        t(
            "transform",
            Core,
            Stable,
            Curated,
            "Move and/or rotate a solid in place by object_uuid (identity preserved). Rotation applies first, then translation.",
            json!({
                "type": "object",
                "properties": {
                    "object": {"type": "string", "format": "uuid", "description": "object_uuid of the solid to move"},
                    "translation": ntuple(3, "[dx, dy, dz] world-space offset (mm)"),
                    "rotation": {
                        "type": "object",
                        "description": "optional rotation; applied before translation",
                        "properties": {
                            "axis": ntuple(3, "rotation axis direction [x,y,z]"),
                            "angle_deg": {"type": "number", "description": "rotation angle in degrees"},
                            "center": ntuple(3, "pivot point [x,y,z] mm; default origin")
                        },
                        "required": ["axis", "angle_deg"]
                    }
                },
                "required": ["object"]
            }),
        ),
        t(
            "drill_pattern",
            Core,
            Experimental,
            Curated,
            "One-call bolt-circle: create count bore cylinders on a ring and subtract them from a target; certified per hole.",
            json!({
                "type": "object",
                "properties": {
                    "object": {"type": "string", "format": "uuid", "description": "object_uuid of the solid to drill"},
                    "plane": plane_schema(true),
                    "center": ntuple(3, "world-space ring centre [x,y,z] mm; overrides the plane origin"),
                    "axis": ntuple(3, "world-space bore direction [x,y,z]; overrides the plane normal"),
                    "cx": {"type": "number", "default": 0, "description": "ring-centre offset along plane u (mm)"},
                    "cy": {"type": "number", "default": 0, "description": "ring-centre offset along plane v (mm)"},
                    "count": {"type": "integer", "minimum": 1, "maximum": 64, "description": "number of holes"},
                    "ring_r": {"type": "number", "exclusiveMinimum": 0, "description": "radius the hole centres sit on (mm)"},
                    "hole_r": {"type": "number", "exclusiveMinimum": 0, "description": "bore radius (mm)"},
                    "depth": {"type": "number", "exclusiveMinimum": 0, "description": "bore length (mm); overshoot the part"},
                    "z_offset": {"type": "number", "default": -1, "description": "bore start along the normal (mm); -1 = 1mm under the plane"},
                    "start_angle_deg": {"type": "number", "default": 0, "description": "angle of the first hole about the ring (degrees)"}
                },
                "required": ["object", "count", "ring_r", "hole_r", "depth"]
            }),
        ),
        t(
            "delete_part",
            Core,
            Stable,
            Curated,
            "Delete one part (timeline-recorded, undo-safe). Kernel part ids renumber after deletion.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "clear_parts",
            Core,
            Stable,
            Curated,
            "Delete every part (each deletion timeline-recorded, undo-safe).",
            json!({"type": "object", "properties": {}, "required": []}),
        ),
        t(
            "list_parts",
            Core,
            Stable,
            Curated,
            "List every part in the live model (id, name, kind).",
            json!({"type": "object", "properties": {}, "required": []}),
        ),
        t(
            "get_part",
            Core,
            Stable,
            Curated,
            "Full report for one part: world placement, topology fingerprint, name.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "render_part",
            Core,
            Stable,
            Curated,
            "See a part: deterministic offscreen render; mode 'ids' returns a colour->face_id legend, 'diagnostic' highlights defects.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "mode": {"type": "string", "enum": ["shaded", "ids", "depth", "normals", "diagnostic"], "default": "shaded", "description": "render channel"},
                    "view": {"type": "string", "enum": ["iso", "front", "top", "right"], "default": "iso", "description": "camera view"},
                    "size": {"type": "integer", "minimum": 64, "maximum": 2048, "default": 512, "description": "image size in px"}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "scene_view",
            Core,
            Stable,
            Curated,
            "See the whole scene: composite every part into one image from an orbit camera (azimuth/elevation, world-Z up).",
            json!({
                "type": "object",
                "properties": {
                    "az": {"type": "number", "default": 35, "description": "azimuth degrees around world Z"},
                    "el": {"type": "number", "default": 20, "description": "elevation degrees above the horizon"},
                    "mode": {"type": "string", "enum": ["shaded", "ids", "depth", "normals", "diagnostic"], "default": "shaded", "description": "render channel"},
                    "size": {"type": "integer", "minimum": 64, "maximum": 2048, "default": 720, "description": "image size in px"},
                    "quality": {"type": "string", "enum": ["coarse", "medium", "fine"], "default": "medium", "description": "coarse=fast, fine=resolve curved silhouettes"}
                },
                "required": []
            }),
        ),
        t(
            "section_view",
            Core,
            Stable,
            Curated,
            "Cutaway: slice a part with a plane (point p + normal) and return the cross-section image + section area.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "p": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "default": [0, 0, 0], "description": "a point on the cutting plane [x,y,z] mm"},
                    "normal": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "default": [1, 0, 0], "description": "the cutting-plane normal [x,y,z]"}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "verify_part",
            Core,
            Stable,
            Curated,
            "Explicit full certificate: brep_valid + watertight + manifold + self-intersection-free + tessellation/mesh-quality, with a diagnostic image.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "view": {"type": "string", "enum": ["iso", "front", "top", "right"], "default": "iso", "description": "camera view for the diagnostic image"}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "mass_properties",
            Core,
            Stable,
            Curated,
            "Exact mass properties: volume, mass, centre of mass, inertia tensor + principal moments/axes (accuracy-gated).",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "document_units",
            Core,
            Stable,
            Curated,
            "Display-only document unit (mm/cm/m/in/ft); omit to read, provide to set. Geometry stays mm-native.",
            json!({
                "type": "object",
                "properties": {"unit": {"type": "string", "enum": ["mm", "cm", "m", "in", "ft"], "description": "omit to read; provide to set"}},
                "required": []
            }),
        ),
        t(
            "set_part_color",
            Core,
            Stable,
            Curated,
            "Set a part's display RGB for scene_view renders. Registry-only — does not modify geometry.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "r": {"type": "integer", "minimum": 0, "maximum": 255},
                    "g": {"type": "integer", "minimum": 0, "maximum": 255},
                    "b": {"type": "integer", "minimum": 0, "maximum": 255}
                },
                "required": ["part_id", "r", "g", "b"]
            }),
        ),
        t(
            "select_face",
            Core,
            Stable,
            Curated,
            "Address a face by description (kind, normal_dir, extremal tie-breaker); the kernel resolves it or refuses.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "kind": {"type": "string", "enum": ["any", "planar", "cylindrical", "spherical", "conical", "toroidal", "nurbs"], "default": "any", "description": "surface-type filter"},
                    "normal_dir": ntuple(3, "keep faces whose outward normal aligns with this [x,y,z]"),
                    "extremal": {"type": "string", "enum": ["none", "largest_area", "smallest_area", "most_along"], "default": "none", "description": "tie-breaker among matches"},
                    "along": ntuple(3, "direction [x,y,z] for the most_along extremal"),
                    "angle_tol_deg": {"type": "number", "default": 12, "description": "angular tolerance (degrees) for the normal_dir match"}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "select_edge",
            Core,
            Stable,
            Curated,
            "Address an edge by description (curve_kind, blend filter, convexity, extremal); the kernel resolves it or refuses.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer"},
                    "curve_kind": {"type": "string", "enum": ["any", "line", "arc", "circle", "nurbs"], "default": "any"},
                    "blend": {"type": "string", "enum": ["any", "filleted", "chamfered", "unblended"], "default": "any"},
                    "convexity": {"type": "string", "enum": ["any", "convex", "concave"], "default": "any"},
                    "direction": ntuple(3, "direction [x,y,z]"),
                    "extremal": {"type": "string", "enum": ["none", "longest", "shortest", "most_along"], "default": "none"},
                    "along": ntuple(3, "direction [x,y,z] for most_along"),
                    "angle_tol_deg": {"type": "number", "default": 12}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "import_step",
            Core,
            Stable,
            Curated,
            "Import a STEP file (AP203/214/242) as real B-Rep solids; give a path OR inline content. Every solid gets the full certificate.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "filesystem path to a .step/.stp file (read locally by the server)"},
                    "content": {"type": "string", "description": "inline STEP file text"},
                    "name": {"type": "string", "description": "display-name prefix for imported parts"}
                },
                "required": []
            }),
        ),
        t(
            "export_part",
            Core,
            Stable,
            Curated,
            "Export parts to a real CAD file on disk (STEP AP242 / STL / OBJ) and return the absolute path.",
            json!({
                "type": "object",
                "properties": {
                    "format": {"type": "string", "enum": ["STEP", "STL", "OBJ"], "default": "STEP", "description": "output file format"},
                    "objects": {"type": "array", "items": {"type": "string", "format": "uuid"}, "default": [], "description": "object_uuids to export; empty = every solid"},
                    "file_name": {"type": "string", "pattern": "^[\\w.-]+$", "description": "file name without directory, e.g. flange_2in.step"},
                    "save_path": {"type": "string", "description": "absolute destination path; overrides file_name/Desktop"},
                    "quality": {"type": "string", "enum": ["Low", "Medium", "High"], "default": "High", "description": "tessellation quality for STL/OBJ meshes"}
                },
                "required": ["file_name"]
            }),
        ),
        t(
            "get_pointer",
            Core,
            Stable,
            Curated,
            "What the human is pointing at in the viewport: latest click (object, face_id, world position) + kernel hover report.",
            json!({"type": "object", "properties": {}, "required": []}),
        ),
        t(
            "timeline_mould",
            Core,
            Experimental,
            Curated,
            "Edit a recorded parameter and re-derive the model (#64 parametric DAG); the edit is appended as a param.mould override event.",
            json!({
                "type": "object",
                "properties": {
                    "value": {"type": "number", "description": "the new dimensional value"},
                    "target_event_id": {"type": "string", "description": "event UUID to edit (with `parameter`)"},
                    "parameter": {"type": "string", "description": "raw numeric parameter key on the target event, e.g. 'radius'"},
                    "name": {"type": "string", "description": "stable parameter name to target (see bind_parameter_name)"},
                    "branch": {"type": "string", "default": "main"}
                },
                "required": ["value"]
            }),
        ),
        t(
            "bind_parameter_name",
            Core,
            Experimental,
            Curated,
            "Bind a stable name to a recorded (event, parameter) so a mould can target it by name (#64 Slice 3).",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "the name to bind, e.g. 'bore_diameter'"},
                    "target_event_id": {"type": "string", "description": "event UUID whose parameter to name"},
                    "parameter": {"type": "string", "description": "raw numeric parameter key, e.g. 'radius'"},
                    "branch": {"type": "string", "default": "main"}
                },
                "required": ["name", "target_event_id", "parameter"]
            }),
        ),
        t(
            "rebuild_certificate",
            Core,
            Experimental,
            Curated,
            "The honest per-feature rebuild certificate for a branch's current state (#64 Slice 5): Rebuilt/Unaffected/Failed/Dangling/Blocked.",
            json!({
                "type": "object",
                "properties": {"branch": {"type": "string", "default": "main"}},
                "required": []
            }),
        ),
        t(
            "timeline_scrub",
            Core,
            Experimental,
            Curated,
            "Look at the scene as of a past event — non-destructive; returns object count + mesh stats at that moment.",
            json!({
                "type": "object",
                "properties": {
                    "branch": {"type": "string", "default": "main"},
                    "sequence": {"type": "integer"}
                },
                "required": ["sequence"]
            }),
        ),
        t(
            "clear_timeline",
            Core,
            Stable,
            Curated,
            "Reset a timeline branch to zero events and wipe the live model to match — destructive and irreversible.",
            json!({
                "type": "object",
                "properties": {"branch_id": {"type": "string", "default": "main", "description": "branch to clear; 'main' is the trunk"}},
                "required": []
            }),
        ),
        // ═══════════════════ SKETCH ═══════════════════
        t(
            "create_sketch",
            Sketch,
            Stable,
            Curated,
            "Start a click-draft sketch session on a plane; returns sketch_id for sketch_points/sketch_add_shape/sketch_extrude.",
            json!({
                "type": "object",
                "properties": {
                    "plane": plane_schema(false),
                    "tool": {"type": "string", "enum": ["rectangle", "circle", "polyline"], "description": "first shape's kind"}
                },
                "required": ["plane", "tool"]
            }),
        ),
        t(
            "sketch_add_shape",
            Sketch,
            Stable,
            Curated,
            "Add another shape to an existing sketch (e.g. hole circles inside an outer boundary). Returns the new shape_index.",
            json!({
                "type": "object",
                "properties": {
                    "sketch_id": {"type": "string", "description": "sketch_id from create_sketch"},
                    "tool": {"type": "string", "enum": ["rectangle", "circle", "polyline"], "description": "shape kind to add"}
                },
                "required": ["sketch_id", "tool"]
            }),
        ),
        t(
            "sketch_points",
            Sketch,
            Stable,
            Curated,
            "Batch-add plane-local points to a sketch shape in one call.",
            json!({
                "type": "object",
                "properties": {
                    "sketch_id": {"type": "string", "description": "sketch_id from create_sketch"},
                    "points": {"type": "array", "minItems": 1, "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "plane-local [u,v] points (mm)"},
                    "shape_index": {"type": "integer", "description": "target shape (from sketch_add_shape); omit = first shape"}
                },
                "required": ["sketch_id", "points"]
            }),
        ),
        t(
            "sketch_extrude",
            Sketch,
            Stable,
            Curated,
            "Extrude the sketch into a solid along the plane normal (multi-shape sketches get region detection).",
            json!({
                "type": "object",
                "properties": {
                    "sketch_id": {"type": "string", "description": "sketch_id from create_sketch"},
                    "distance": {"type": "number", "description": "extrusion length (mm) along the plane normal; sign sets direction"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["sketch_id", "distance"]
            }),
        ),
        t(
            "plane_from_face",
            Sketch,
            Stable,
            Curated,
            "Derive a sketch plane from an existing planar face; returns {origin, u_axis, v_axis} to pass as a plane.",
            json!({
                "type": "object",
                "properties": {
                    "object_id": {"type": "string", "format": "uuid", "description": "the part's public object UUID"},
                    "face_id": {"type": "integer", "description": "planar face id from get_pointer or a render 'ids' legend"}
                },
                "required": ["object_id", "face_id"]
            }),
        ),
        t(
            "psketch_begin",
            Sketch,
            Stable,
            Curated,
            "Start a new parametric sketch session (constraint-solver backed, XY plane) and return its csketch_id.",
            json!({"type": "object", "properties": {}, "required": []}),
        ),
        t(
            "psketch_add_entity",
            Sketch,
            Stable,
            Curated,
            "Add one entity (point/line/circle/arc/rectangle/polyline/spline) to a parametric sketch. Returns the entity id.",
            json!({
                "type": "object",
                "properties": {
                    "csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"},
                    "kind": {"type": "string", "enum": ["point", "line", "circle", "arc", "rectangle", "polyline", "spline"], "description": "entity type; see description for each type's params"},
                    "params": {"type": "object", "description": "entity params for `kind` (sketch-plane mm/radians)"}
                },
                "required": ["csketch_id", "kind", "params"]
            }),
        ),
        t(
            "psketch_constrain",
            Sketch,
            Stable,
            Curated,
            "Add a geometric/continuity/dimensional constraint to a parametric sketch (incl. G1/G2 continuity).",
            json!({
                "type": "object",
                "properties": {
                    "csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"},
                    "constraint_type": {"type": "object", "description": "the constraint, e.g. {Horizontal:{}} or {Distance:80.0} (mm) / {Angle:1.57} (radians)"},
                    "entities": {"type": "array", "items": {"type": "object"}, "description": "target entity refs, e.g. [{Line:uuid}] or [{Point:uuid},{Point:uuid}]"}
                },
                "required": ["csketch_id", "constraint_type", "entities"]
            }),
        ),
        t(
            "psketch_solve",
            Sketch,
            Stable,
            Curated,
            "Run the Newton-Raphson solver; converged = geometry satisfies every constraint exactly.",
            json!({
                "type": "object",
                "properties": {"csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"}},
                "required": ["csketch_id"]
            }),
        ),
        t(
            "psketch_certify",
            Sketch,
            Stable,
            Curated,
            "Full certified-sketch verdict: solver status, per-constraint residuals, per-entity constrainment, conflict witnesses, DOF summary.",
            json!({
                "type": "object",
                "properties": {"csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"}},
                "required": ["csketch_id"]
            }),
        ),
        t(
            "psketch_dof",
            Sketch,
            Stable,
            Curated,
            "DOF summary + per-entity constrainment: which entities are fully constrained, which still move, which are over-constrained.",
            json!({
                "type": "object",
                "properties": {"csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"}},
                "required": ["csketch_id"]
            }),
        ),
        t(
            "psketch_op",
            Sketch,
            Experimental,
            Curated,
            "Sketch operation maintained by minted constraints: trim/extend/offset/mirror/linear|circular|curve|phyllotaxis pattern/construction.",
            json!({
                "type": "object",
                "properties": {
                    "csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"},
                    "op": {"type": "string", "enum": ["trim", "extend", "offset", "mirror", "linear_pattern", "circular_pattern", "curve_pattern", "phyllotaxis_pattern", "construction"], "description": "operation; see description for each op's params"},
                    "params": {"type": "object", "description": "op params (lengths mm, angles radians)"}
                },
                "required": ["csketch_id", "op", "params"]
            }),
        ),
        t(
            "psketch_extrude",
            Sketch,
            Stable,
            Curated,
            "Extrude the parametric sketch's closed regions into a solid (hole-aware).",
            json!({
                "type": "object",
                "properties": {
                    "csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"},
                    "distance": {"type": "number", "description": "extrusion length (mm); sign sets direction"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["csketch_id", "distance"]
            }),
        ),
        t(
            "psketch_revolve",
            Sketch,
            Stable,
            Curated,
            "Revolve the parametric sketch's closed regions about an in-plane axis (typed-analytic where honest).",
            json!({
                "type": "object",
                "properties": {
                    "csketch_id": {"type": "string", "format": "uuid", "description": "csketch_id from psketch_begin"},
                    "axis_origin": ntuple(2, "point on the axis, sketch-plane coords [x,y] mm"),
                    "axis_direction": ntuple(2, "axis direction in sketch-plane coords [x,y]"),
                    "angle": {"type": "number", "description": "sweep angle in radians (default 2pi, full)"},
                    "segments": {"type": "integer", "minimum": 3, "maximum": 512, "description": "angular tessellation count for sampled loops"},
                    "name": {"type": "string", "description": "display name for the part"}
                },
                "required": ["csketch_id", "axis_origin", "axis_direction"]
            }),
        ),
        // ═══════════════════ ASSEMBLY ═══════════════════
        t(
            "assembly_verify",
            Assembly,
            Experimental,
            Curated,
            "One-shot kinematic assembly certificate from a self-contained spec (parts + mates + mechanisms inline); 5-dimension verdict.",
            json!({
                "type": "object",
                "properties": {
                    "ground": {"type": "integer", "description": "instance_id of the grounded (fixed reference) part"},
                    "parts": {
                        "type": "array",
                        "description": "every part in the assembly, as an instance",
                        "items": {
                            "type": "object",
                            "properties": {
                                "object": {"type": "string", "description": "the part's object_uuid"},
                                "instance_id": {"type": "integer", "description": "this occurrence's id, referenced by mates/mechanisms"},
                                "translation": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "world position [x,y,z] mm"},
                                "rotation": {"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4, "description": "unit quaternion [x,y,z,w]"}
                            },
                            "required": ["object", "instance_id"]
                        }
                    },
                    "mates": {"type": "array", "items": {"type": "object"}, "description": "mate constraints — see MATE format in the tool description"},
                    "mechanisms": {"type": "array", "items": {"type": "object"}, "description": "mechanisms for swept-clearance — see MECHANISM format"},
                    "epsilon": {"type": "number", "default": 0.0, "description": "tessellation deviation bound (mm)"}
                },
                "required": ["ground", "parts"]
            }),
        ),
        t(
            "assembly_create",
            Assembly,
            Stable,
            Curated,
            "Create a true assembly: a named scene of positioned part instances (not a boolean merge). Returns the assembly id.",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string", "minLength": 1, "description": "display name, e.g. 'gearbox'"}},
                "required": ["name"]
            }),
        ),
        t(
            "assembly_add_instance",
            Assembly,
            Stable,
            Curated,
            "Place an instance of an existing part into an assembly at a world pose. Returns the instance id + assembly perception.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "description": "assembly id from assembly_create"},
                    "object": {"type": "string", "description": "the part's object_uuid"},
                    "position": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "world translation [x,y,z] mm"},
                    "rotation_deg": {"type": "number", "description": "rotation angle about rotation_axis (degrees)"},
                    "rotation_axis": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "unit rotation axis, e.g. [0,0,1]"},
                    "transform": {"type": "array", "minItems": 4, "maxItems": 4, "items": {"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4}, "description": "raw row-major 4x4 pose (overrides position/rotation)"},
                    "name": {"type": "string", "description": "placement name, e.g. 'wheel-FL'"},
                    "color": {"type": "array", "items": {"type": "integer", "minimum": 0, "maximum": 255}, "minItems": 3, "maxItems": 3, "description": "per-instance display RGB (0-255)"}
                },
                "required": ["assembly_id", "object"]
            }),
        ),
        t(
            "assembly_list_instances",
            Assembly,
            Stable,
            Curated,
            "List an assembly's instances with perception: instance_count vs unique_part_count, per-instance soundness, combined bbox.",
            json!({
                "type": "object",
                "properties": {"assembly_id": {"type": "string", "description": "assembly id from assembly_create"}},
                "required": ["assembly_id"]
            }),
        ),
        t(
            "assembly_transform_instance",
            Assembly,
            Stable,
            Curated,
            "Re-pose one instance without touching the others or the referenced part. Returns the updated assembly perception.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "description": "assembly id from assembly_create"},
                    "instance_id": {"type": "string", "description": "instance id from assembly_list_instances"},
                    "position": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "world translation [x,y,z] mm"},
                    "rotation_deg": {"type": "number", "description": "rotation angle about rotation_axis (degrees)"},
                    "rotation_axis": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "unit rotation axis"},
                    "transform": {"type": "array", "minItems": 4, "maxItems": 4, "items": {"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4}, "description": "raw row-major 4x4 pose"}
                },
                "required": ["assembly_id", "instance_id"]
            }),
        ),
        t(
            "assembly_view",
            Assembly,
            Stable,
            Curated,
            "See a whole assembly: composite every instance at its transform into one image from an orbit camera.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "description": "assembly id from assembly_create"},
                    "az": {"type": "number", "default": 35, "description": "azimuth degrees around world Z"},
                    "el": {"type": "number", "default": 20, "description": "elevation degrees above horizon"},
                    "mode": {"type": "string", "enum": ["shaded", "ids", "depth", "normals", "diagnostic"], "default": "shaded", "description": "render channel"},
                    "size": {"type": "integer", "minimum": 64, "maximum": 2048, "default": 720, "description": "image size in px"},
                    "quality": {"type": "string", "enum": ["coarse", "medium", "fine"], "default": "medium", "description": "tessellation quality"}
                },
                "required": ["assembly_id"]
            }),
        ),
        t(
            "assembly_mate",
            Assembly,
            Experimental,
            Curated,
            "Mate two instances — connectors + mate in one call. A mate relates two coordinate frames and IS the joint.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "format": "uuid", "description": "assembly id from assembly_create"},
                    "action": {"type": "string", "enum": ["create", "edit", "remove"], "default": "create", "description": "create a mate (+ connectors), edit its kind, or remove it"},
                    "kind": {"description": "the mate kind, e.g. 'Fastened' or {Revolute:{limits:[-0.1,0.1]}} (required for create/edit)"},
                    "a": {"type": "object", "description": "side A connector (required for create)"},
                    "b": {"type": "object", "description": "side B connector (required for create)"},
                    "couples": {"type": "array", "items": {"type": "string", "format": "uuid"}, "description": "for coupling kinds: the related mate ids"},
                    "mate_id": {"type": "string", "format": "uuid", "description": "the mate to edit/remove"}
                },
                "required": ["assembly_id"]
            }),
        ),
        t(
            "assembly_solve",
            Assembly,
            Experimental,
            Curated,
            "Solve the mate system: parts are placed by their mates; returns solved poses, per-mate facts, and a compact verdict.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "format": "uuid", "description": "assembly id from assembly_create"},
                    "ground": {"type": "string", "format": "uuid", "description": "instance that never moves (defaults to the first)"}
                },
                "required": ["assembly_id"]
            }),
        ),
        t(
            "assembly_certify",
            Assembly,
            Experimental,
            Curated,
            "The full assembly certificate — does this go together AND move without collision? is_sound = AND of mates/grounding/clearance dims.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "format": "uuid", "description": "assembly id from assembly_create"},
                    "ground": {"type": "string", "format": "uuid", "description": "instance that never moves"},
                    "epsilon": {"type": "number", "description": "clearance margin (mm); honoured only above the kernel floor"}
                },
                "required": ["assembly_id"]
            }),
        ),
        t(
            "assembly_dof",
            Assembly,
            Experimental,
            Curated,
            "DOF + per-instance constrainment: which instances are fully located, which still move (and how), which are over-constrained.",
            json!({
                "type": "object",
                "properties": {"assembly_id": {"type": "string", "format": "uuid", "description": "assembly id from assembly_create"}},
                "required": ["assembly_id"]
            }),
        ),
        t(
            "assembly_drag",
            Assembly,
            Experimental,
            Curated,
            "Drive a joint — your kinematic hand. Set a joint parameter and the affected chain re-solves; poses are written back.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "format": "uuid", "description": "assembly id from assembly_create"},
                    "mate_id": {"type": "string", "format": "uuid", "description": "the joint to drive"},
                    "param": {"type": "string", "enum": ["rotation", "translation"], "description": "rotation about the connector z, or translation along it"},
                    "value": {"type": "number", "description": "target value (radians for rotation, mm for translation)"},
                    "ground": {"type": "string", "format": "uuid", "description": "instance that never moves"}
                },
                "required": ["assembly_id", "mate_id", "param", "value"]
            }),
        ),
        t(
            "assembly_interference",
            Assembly,
            Experimental,
            Curated,
            "What touches, and at what angle: static overlaps + continuous-TOI joint sweeps, first_contact, min certified clearance.",
            json!({
                "type": "object",
                "properties": {
                    "assembly_id": {"type": "string", "format": "uuid", "description": "assembly id from assembly_create"},
                    "ground": {"type": "string", "format": "uuid", "description": "instance that never moves"},
                    "epsilon": {"type": "number", "description": "clearance margin (mm) above the kernel floor"}
                },
                "required": ["assembly_id"]
            }),
        ),
        // ═══════════════════ DRAWING ═══════════════════
        t(
            "make_drawing",
            Drawing,
            Stable,
            Curated,
            "Generate a 2D engineering drawing: standard four-view sheet with hidden-line removal, centerlines, auto dimensions + a quality report.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part/solid id from list_parts"},
                    "name": {"type": "string", "description": "title-block name for the sheet"}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "drawing_read_semantics",
            Drawing,
            Experimental,
            Curated,
            "Read the semantic model of a sheet + a live certificate (queryable data, not a file): views, dimensions with PIDs, hole table, GD&T, section.",
            json!({
                "type": "object",
                "properties": {"drawing_id": {"type": "string", "format": "uuid", "description": "drawing_id from make_drawing"}},
                "required": ["drawing_id"]
            }),
        ),
        t(
            "drawing_query",
            Drawing,
            Experimental,
            Curated,
            "Ask one typed, scoped question of a sheet, answered certified against the live model with provenance + a live-check verdict.",
            json!({
                "type": "object",
                "properties": {
                    "drawing_id": {"type": "string", "format": "uuid", "description": "drawing_id from make_drawing"},
                    "kind": {"type": "string", "enum": ["toleranced_diameter", "fcf", "section_cuts", "dimension_of", "hole", "entity_at"], "description": "the question kind"},
                    "tag": {"type": "string", "description": "hole tag, e.g. 'A1' (toleranced_diameter | hole)"},
                    "face_id": {"type": "integer", "description": "kernel face id (toleranced_diameter | dimension_of)"},
                    "pid": {"type": "string", "description": "dimension PID (toleranced_diameter | dimension_of)"},
                    "index": {"type": "integer", "description": "FCF block index (fcf)"},
                    "feature_pid": {"type": "string", "description": "toleranced feature PID hex (fcf)"},
                    "datum": {"type": "string", "description": "datum letter the FCF references (fcf)"},
                    "label": {"type": "string", "description": "dimension label substring (dimension_of)"},
                    "view": {"type": "integer", "description": "view index (entity_at)"},
                    "xy_mm": ntuple(2, "view-space coordinate [x, y] in mm (entity_at)")
                },
                "required": ["drawing_id", "kind"]
            }),
        ),
        t(
            "drawing_export_sheet",
            Drawing,
            Stable,
            Curated,
            "Save the rendered sheet from make_drawing to disk as a PDF/DXF/SVG file and return the absolute path.",
            json!({
                "type": "object",
                "properties": {
                    "drawing_id": {"type": "string", "format": "uuid", "description": "drawing_id from make_drawing"},
                    "format": {"type": "string", "enum": ["pdf", "dxf", "svg"], "default": "pdf", "description": "output file format"},
                    "file_name": {"type": "string", "pattern": "^[\\w.-]+$", "description": "file name without directory, e.g. flange_drawing.pdf"},
                    "save_path": {"type": "string", "description": "absolute destination path; overrides file_name/Desktop"}
                },
                "required": ["drawing_id", "file_name"]
            }),
        ),
        t(
            "dimension_part",
            Drawing,
            Stable,
            Curated,
            "Dimension a part in one call: a 2x2 multi-view image with leader+label callouts AND the structured table (incl. position rows).",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "gdt_datum",
            Drawing,
            Stable,
            Curated,
            "Designate a face as a datum (label + target) or list current datums; pins to PersistentIds and dangles honestly.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "label": {"type": "string", "description": "datum letter e.g. 'A', 'B', 'C' — omit to list"},
                    "face_id": {"type": "integer", "description": "kernel face id; mutually exclusive with selector"},
                    "selector": {"type": "string", "description": "face by description: JSON string shaped like select_face body"}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "gdt_fcf",
            Drawing,
            Stable,
            Curated,
            "Author a Feature Control Frame; returns an immediate kernel-certified verdict (exact B-Rep measurement). Session-only.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "characteristic": {"type": "string", "enum": ["flatness", "perpendicularity", "parallelism", "position"], "description": "GD&T characteristic to evaluate"},
                    "tolerance_mm": {"type": "number", "exclusiveMinimum": 0, "description": "tolerance zone width in millimetres"},
                    "datum_refs": {"type": "array", "items": {"type": "string"}, "description": "ordered datum labels e.g. ['A'] or ['A','B']; empty/omit for flatness"},
                    "target_face": {"type": "integer", "description": "kernel face id of the toleranced feature"},
                    "target_selector": {"type": "string", "description": "feature by description: JSON string shaped like select_face"},
                    "basic": ntuple(2, "basic dims [x, y] mm relative to DRF origin — required for position")
                },
                "required": ["part_id", "characteristic", "tolerance_mm"]
            }),
        ),
        t(
            "gdt_report",
            Drawing,
            Stable,
            Curated,
            "All GD&T state for a part: datum list + one compact verdict line per FCF annotation, re-evaluated live against the current B-Rep.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        // ═══════════════════ ANALYSIS ═══════════════════
        t(
            "point_query",
            Analysis,
            Stable,
            Curated,
            "Probe a world point against a part: signed distance, inside/outside/on classification, nearest boundary face + closest point.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "point": ntuple(3, "world-space query point [x, y, z]")
                },
                "required": ["part_id", "point"]
            }),
        ),
        t(
            "ray_query",
            Analysis,
            Stable,
            Curated,
            "Cast a ray through a part: ordered face crossings (face id, exact hit point, oriented normal, distance), near->far.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "origin": ntuple(3, "ray origin [x,y,z]"),
                    "direction": ntuple(3, "ray direction [x,y,z]; need not be unit length")
                },
                "required": ["part_id", "origin", "direction"]
            }),
        ),
        t(
            "region_query",
            Analysis,
            Stable,
            Curated,
            "Ask 'what is in here?': a box (center + half_extents) or sphere (center + radius) region returns parts/faces met and whether empty.",
            json!({
                "type": "object",
                "properties": {
                    "center": ntuple(3, "region centre [x,y,z]"),
                    "half_extents": ntuple(3, "box half-extents — supply for a BOX region"),
                    "radius": {"type": "number", "exclusiveMinimum": 0, "description": "sphere radius — supply for a SPHERE region"},
                    "part_id": {"type": "integer", "description": "restrict to one part; omit to scan every part"}
                },
                "required": ["center"]
            }),
        ),
        t(
            "occupancy_view",
            Analysis,
            Stable,
            Curated,
            "Non-deceivable SDF X-ray: a slice-stack ('#'=solid, '.'=empty) sampled from the exact solid — reveals cavities/wall thickness.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "n": {"type": "integer", "default": 20, "description": "grid resolution per axis (clamped to 4..48)"}
                },
                "required": ["part_id"]
            }),
        ),
        t(
            "part_coverage",
            Analysis,
            Stable,
            Curated,
            "Coverage honesty: which faces the 4 standard views actually show vs leave unseen.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "part_distance",
            Analysis,
            Stable,
            Curated,
            "Measure two parts' spatial relationship from world AABBs: gap, overlap, center distance, unit direction a->b.",
            json!({
                "type": "object",
                "properties": {
                    "part_a": {"type": "integer", "description": "kernel part id from list_parts"},
                    "part_b": {"type": "integer", "description": "kernel part id from list_parts"}
                },
                "required": ["part_a", "part_b"]
            }),
        ),
        t(
            "part_features",
            Analysis,
            Stable,
            Curated,
            "Read analytic feature sizes off the B-Rep: cylinder diameters + axes for bores/bosses, plane normals, distinct-diameter summary.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "ground_truth",
            Analysis,
            Stable,
            Curated,
            "The kernel's own account of a part: provenance (designed vs primitive stand-in), validity certificate, display-mesh verdict.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "measure_faces",
            Analysis,
            Stable,
            Curated,
            "Measure the exact relation between two faces (or inspect one): distance, dihedral, axis distance, diameter, area — kernel-exact.",
            json!({
                "type": "object",
                "properties": {
                    "part_a": {"type": "integer", "description": "kernel part id of the first face's solid"},
                    "face_a": {"type": "integer", "description": "first face id"},
                    "part_b": {"type": "integer", "description": "second face's solid (omit with face_b for single-face info)"},
                    "face_b": {"type": "integer", "description": "second face id (omit for single-face info)"}
                },
                "required": ["part_a", "face_a"]
            }),
        ),
        t(
            "verify_claim",
            Analysis,
            Stable,
            Curated,
            "Verify a math claim against kernel ground truth: bind variables to exact measurements, assert expected; deterministic three-state verdict.",
            json!({
                "type": "object",
                "properties": {
                    "expr": {"type": "string", "description": "math expression over the binding variable names, e.g. 'a_exit / a_throat'"},
                    "bindings": {
                        "type": "array",
                        "description": "variable->measurement bindings",
                        "items": {
                            "type": "object",
                            "properties": {
                                "var": {"type": "string", "description": "variable name used in expr"},
                                "measure": {
                                    "type": "object",
                                    "properties": {
                                        "kind": {"type": "string", "enum": ["volume", "surface_area", "face_area", "edge_length", "constant"]},
                                        "part": {"type": "string", "description": "part object UUID — for volume / surface_area"},
                                        "face": {"type": "integer", "description": "face id — for face_area"},
                                        "edge": {"type": "integer", "description": "edge id — for edge_length"},
                                        "value": {"type": "number", "description": "the value — for constant"}
                                    },
                                    "required": ["kind"]
                                }
                            },
                            "required": ["var", "measure"]
                        }
                    },
                    "expected": {"type": "number", "description": "the value the expression should equal"},
                    "tolerance": {"type": "number", "description": "absolute tolerance; omit for auto (1e-6 relative)"}
                },
                "required": ["expr", "bindings", "expected"]
            }),
        ),
        t(
            "get_face",
            Analysis,
            Stable,
            Curated,
            "Per-face report: surface type, area, principal curvatures, boundary edges, neighbours.",
            json!({
                "type": "object",
                "properties": {"face_id": {"type": "integer", "description": "kernel face id (render 'ids' legend or get_pointer)"}},
                "required": ["face_id"]
            }),
        ),
        t(
            "get_revolve_profile",
            Analysis,
            Stable,
            Curated,
            "Recover the editable [r,z] meridian a revolved part was built from (the edit->regenerate loop). 404 if not built by a revolve.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer", "description": "kernel part id from list_parts"}},
                "required": ["part_id"]
            }),
        ),
        // ═══════════════════ LABELS ═══════════════════
        t(
            "label_create",
            Labels,
            Stable,
            Curated,
            "Pin a name to a feature (by id, by description selector, or as a section plane). Re-using a name replaces it.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer", "description": "kernel part id from list_parts"},
                    "name": {"type": "string", "minLength": 1, "description": "the label, e.g. 'throat' (unique per part)"},
                    "kind": {"type": "string", "enum": ["vertex", "edge", "face", "section"], "description": "entity kind to pin (or 'section' for a cutting plane)"},
                    "entity_id": {"type": "integer", "description": "attach by id (omit when using selector or section)"},
                    "selector": {"type": "string", "description": "attach by description: JSON string shaped like select_face/select_edge body"},
                    "origin": ntuple(3, "section only: a point on the cutting plane"),
                    "normal": ntuple(3, "section only: the plane normal"),
                    "description": {"type": "string", "description": "optional free-text note stored with the label"}
                },
                "required": ["part_id", "name", "kind"]
            }),
        ),
        t(
            "label_list",
            Labels,
            Stable,
            Curated,
            "List every label on a part: name, kind, world anchor, colour, measured key dimension, GD&T verdict, staleness.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "label_resolve",
            Labels,
            Stable,
            Curated,
            "Resolve a label name to the live entity id or section plane it pins; refuses with not_found / dangling.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer"},
                    "name": {"type": "string", "minLength": 1}
                },
                "required": ["part_id", "name"]
            }),
        ),
        t(
            "label_rename",
            Labels,
            Stable,
            Curated,
            "Rename a label, preserving its binding; 404 when old name unknown, 409 when the new name is taken by a different label.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer"},
                    "name": {"type": "string", "minLength": 1, "description": "the existing label name"},
                    "new_name": {"type": "string", "minLength": 1, "description": "the new name (unique per part)"}
                },
                "required": ["part_id", "name", "new_name"]
            }),
        ),
        t(
            "label_delete",
            Labels,
            Stable,
            Curated,
            "Remove a label by name; deleted:true when it existed, 404 when not — reported honestly.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "integer"},
                    "name": {"type": "string", "minLength": 1, "description": "the label to remove"}
                },
                "required": ["part_id", "name"]
            }),
        ),
        t(
            "propose_labels",
            Labels,
            Stable,
            Curated,
            "Auto-propose labels: the kernel recognizes features and suggests name + pinning assertion — it does NOT apply them.",
            json!({
                "type": "object",
                "properties": {"part_id": {"type": "integer"}},
                "required": ["part_id"]
            }),
        ),
        t(
            "blackboard_add_entry",
            Labels,
            Stable,
            Curated,
            "Write a line to a Blackboard notebook the human sees live (markdown + $math$). Returns the line id.",
            json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "markdown + $math$ source for the line"},
                    "author": {"type": "string", "enum": ["agent", "user"], "default": "agent"},
                    "part_id": {"type": "string", "description": "target THIS part's notebook — a part UUID or integer kernel id. Omit for document-wide."},
                    "scope": {"type": "string", "description": "'document', 'part:<uuid>', or 'assembly:<uuid>'. Wins over part_id."}
                },
                "required": ["text"]
            }),
        ),
        t(
            "blackboard_edit_entry",
            Labels,
            Stable,
            Curated,
            "Edit a Blackboard line in place by id; the change appears live.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "line id from blackboard_list"},
                    "text": {"type": "string", "description": "new markdown + $math$ source"},
                    "part_id": {"type": "string", "description": "the part scope it was listed under (UUID or integer id)"},
                    "scope": {"type": "string", "description": "'document', 'part:<uuid>', or 'assembly:<uuid>'. Wins over part_id."}
                },
                "required": ["id", "text"]
            }),
        ),
        t(
            "blackboard_list",
            Labels,
            Stable,
            Curated,
            "Read a Blackboard notebook: current lines (id, author, text) in document order.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "string", "description": "that part's notebook (UUID or integer id); omit for document-wide"},
                    "scope": {"type": "string", "description": "'document', 'part:<uuid>', or 'assembly:<uuid>'. Wins over part_id."}
                },
                "required": []
            }),
        ),
        t(
            "blackboard_clear",
            Labels,
            Stable,
            Curated,
            "Clear one Blackboard notebook — every line + its event log. Destructive; does not touch geometry.",
            json!({
                "type": "object",
                "properties": {
                    "part_id": {"type": "string", "description": "clears only that part's notebook (UUID or integer id)"},
                    "scope": {"type": "string", "description": "'document', 'part:<uuid>', or 'assembly:<uuid>'. Wins over part_id."}
                },
                "required": []
            }),
        ),
    ]
}

// ───────────────────────────────────────────────────────────────────────────
// Serving
// ───────────────────────────────────────────────────────────────────────────

/// `token_estimate` heuristic: a deterministic, client-reproducible proxy for
/// the context cost of a tool definition. Method = `ceil(len / 4)` over the
/// compact JSON of `{name, purpose, input_schema}` (≈4 chars/token, the common
/// English/JSON rule of thumb). Not a tokenizer — a stable budgeting signal, so
/// the MCP's `jig budget` and the drift hash agree across languages.
fn token_estimate(name: &str, purpose: &str, schema: &Value) -> u64 {
    let payload = json!({"name": name, "purpose": purpose, "input_schema": schema});
    let len = serde_json::to_string(&payload)
        .map(|s| s.len())
        .unwrap_or(0) as u64;
    len.div_ceil(4)
}

/// FNV-1a 64-bit over `bytes`. Chosen over SipHash (`DefaultHasher`, unstable
/// across Rust releases and unseeded-nondeterministic) and over adding a `sha2`
/// dependency: FNV-1a is a fixed, dependency-free algorithm trivially
/// reproducible in the MCP's TypeScript, so both sides hash the identical
/// canonical bytes and a mismatch is real drift, not an algorithm skew.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Build the served registry `Value`. Deterministic except for `generated_at`.
fn build_registry() -> Value {
    // Kernel-served descriptions: for `SchemaSource::Kernel` rows, pull the
    // one-line purpose from geometry-engine's operations registry (Layer 0's
    // "the registry can be SERVED, not hand-maintained"). Fall back to the
    // compiled purpose if the kernel op is absent — honest degrade, never a
    // fabricated line.
    let kernel_catalog = OperationsRegistry::get_operations_catalog();

    let mut tools: Vec<Value> = raw_tools()
        .into_iter()
        .map(|spec| {
            let purpose: String = match spec.source.kernel_op(spec.name) {
                Some(op) => kernel_catalog
                    .get("operations")
                    .and_then(|ops| ops.get(op))
                    .and_then(|o| o.get("description"))
                    .and_then(|d| d.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| spec.purpose.to_string()),
                None => spec.purpose.to_string(),
            };
            let token_estimate = token_estimate(spec.name, &purpose, &spec.schema);
            json!({
                "name": spec.name,
                "bench": spec.bench.as_str(),
                "purpose": purpose,
                "input_schema": spec.schema,
                "token_estimate": token_estimate,
                "stability": spec.stability.as_str(),
                "source": spec.source.as_str(),
            })
        })
        .collect();

    // Sort by name so the array order — and therefore the hash — is stable
    // regardless of the table's declaration order.
    tools.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
    });

    // Canonical bytes for the drift hash: serde_json's Map is a BTreeMap (no
    // `preserve_order` feature in this workspace), so `to_string` emits keys in
    // sorted order at every nesting level — the array is already canonical.
    let canonical = serde_json::to_string(&Value::Array(tools.clone())).unwrap_or_default();
    let registry_hash = format!("{:016x}", fnv1a_64(canonical.as_bytes()));

    // Per-bench counts — a cheap disclosure of the surface's shape.
    let mut bench_counts = serde_json::Map::new();
    for bench in [
        Bench::Core,
        Bench::Sketch,
        Bench::Assembly,
        Bench::Drawing,
        Bench::Analysis,
        Bench::Labels,
    ] {
        let n = tools
            .iter()
            .filter(|t| t.get("bench").and_then(Value::as_str) == Some(bench.as_str()))
            .count();
        bench_counts.insert(bench.as_str().to_string(), json!(n));
    }

    json!({
        "registry_hash": registry_hash,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "hash_algorithm": "fnv1a-64",
        "tool_count": tools.len(),
        "bench_counts": Value::Object(bench_counts),
        "tools": tools,
    })
}

/// `GET /api/agent/tool-registry` — the kernel-served agent tool registry.
///
/// Read-only, no state: the inventory is a compiled constant plus kernel-served
/// purposes for the `Kernel`-sourced rows. Registered with the other
/// `/api/agent` GET routes; the auth `route_layer` gates only kernel-mutation
/// routes, so this read follows the existing posture with no new requirement.
pub async fn tool_registry() -> Json<Value> {
    Json(build_registry())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bench table covers exactly the tools declared, one bench each, all
    /// in the allowed set. Guards against a tool landing benchless.
    #[test]
    fn every_tool_has_an_allowed_bench_and_unique_name() {
        let allowed = [
            "core", "sketch", "assembly", "drawing", "analysis", "labels",
        ];
        let tools = raw_tools();
        let mut seen = std::collections::HashSet::new();
        for spec in &tools {
            assert!(
                allowed.contains(&spec.bench.as_str()),
                "tool {} has bench {} outside the allowed set",
                spec.name,
                spec.bench.as_str()
            );
            assert!(
                seen.insert(spec.name),
                "duplicate tool name in the table: {}",
                spec.name
            );
        }
        assert_eq!(tools.len(), 90, "expected 90 tools, got {}", tools.len());
    }

    /// The kernel-sourced rows correspond to operations actually registered in
    /// geometry-engine's `ai_operations_registry` — the "kernel-generated"
    /// claim is verifiable, not decorative.
    #[test]
    fn kernel_sourced_tools_exist_in_the_kernel_registry() {
        let catalog = OperationsRegistry::get_operations_catalog();
        let ops = catalog
            .get("operations")
            .and_then(Value::as_object)
            .expect("kernel catalog must carry an operations map");
        for spec in raw_tools() {
            if let Some(op) = spec.source.kernel_op(spec.name) {
                assert!(
                    ops.contains_key(op),
                    "tool {} is marked Kernel but op `{op}` is not in the kernel registry",
                    spec.name
                );
            }
        }
    }

    /// Every input_schema is an object of type "object" (mirrors the router
    /// gate at the module level).
    #[test]
    fn every_schema_is_object_typed() {
        for spec in raw_tools() {
            assert_eq!(
                spec.schema.get("type").and_then(Value::as_str),
                Some("object"),
                "tool {} input_schema.type must be object",
                spec.name
            );
        }
    }

    /// The FNV-1a-64 constant and the hash are stable and 16 hex chars.
    #[test]
    fn registry_hash_is_stable_hex() {
        let a = build_registry();
        let b = build_registry();
        let ha = a.get("registry_hash").and_then(Value::as_str).unwrap_or("");
        let hb = b.get("registry_hash").and_then(Value::as_str).unwrap_or("");
        assert_eq!(ha, hb, "hash must be stable across builds");
        assert_eq!(ha.len(), 16, "fnv1a-64 hash renders as 16 hex chars");
    }
}
