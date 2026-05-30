//! Geometry-phase handlers: STEP pure-geometry primitives →
//! kernel curves / surfaces / placements / vertex points.
//!
//! Covered entities (tier-1, planar + cylindrical):
//!
//! | STEP                  | Effect on context                                                         |
//! |-----------------------|---------------------------------------------------------------------------|
//! | `CARTESIAN_POINT`     | Length-scaled 3-D point cached at `caches.points`                         |
//! | `DIRECTION`           | Healed unit-length direction at `caches.directions`                       |
//! | `VECTOR`              | Direction × magnitude (length-scaled) at `caches.vectors`                 |
//! | `AXIS2_PLACEMENT_3D`  | Orthonormal frame at `caches.placements`                                  |
//! | `LINE`                | Origin + unit direction at `caches.step_lines` (geometry template)        |
//! | `CIRCLE`              | Placement + radius at `caches.step_circles` (geometry template)           |
//! | `PLANE`               | Kernel `Plane` allocated into `model.surfaces`, id at `caches.surfaces`   |
//! | `CYLINDRICAL_SURFACE` | Kernel `Cylinder` allocated into `model.surfaces`, id at `caches.surfaces`|
//! | `VERTEX_POINT`        | Kernel vertex allocated into `model.vertices`, id at `caches.vertices`    |
//!
//! ## Cross-phase resolution
//!
//! Handlers reference each other freely (a `PLANE` refers to an
//! `AXIS2_PLACEMENT_3D` which refers to three `DIRECTION` /
//! `CARTESIAN_POINT` entities). Within a single dispatcher phase the
//! walk order is HashMap-arbitrary, so each handler that follows a
//! `#N` checks whether the referent is already in the cache and, if
//! not, calls [`super::resolver::ensure_resolved`] to force it. The
//! resolver is cycle-guarded by `ctx.resolution_stack`.
//!
//! ## Unit scaling
//!
//! Every length-typed value is multiplied by `ctx.unit.length` (mm
//! per source unit). Directions are dimensionless and are not scaled.
//! The `VECTOR.magnitude` field is length-typed.

use ruststep::ast::Record;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::{
    curve::{Circle, Line},
    surface::{Cylinder, Plane},
};

use crate::formats::step::{
    context::{Axis2Placement, ImportContext, StepCircleGeom, StepLineGeom},
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
};

use super::healing::{build_axis_frame, heal_direction, normalize};
use super::params;
use super::resolver::ensure_resolved;

/// Common `entity_name` constants used in diagnostic strings and the
/// resolver's `expected` arguments. Centralised so the dispatcher and
/// the resolver agree on capitalisation.
mod names {
    pub const CARTESIAN_POINT: &str = "CARTESIAN_POINT";
    pub const DIRECTION: &str = "DIRECTION";
    pub const VECTOR: &str = "VECTOR";
    pub const AXIS2_PLACEMENT_3D: &str = "AXIS2_PLACEMENT_3D";
    pub const LINE: &str = "LINE";
    pub const CIRCLE: &str = "CIRCLE";
    pub const PLANE: &str = "PLANE";
    pub const CYLINDRICAL_SURFACE: &str = "CYLINDRICAL_SURFACE";
    pub const VERTEX_POINT: &str = "VERTEX_POINT";
}

// =========================================================================
// CARTESIAN_POINT
// =========================================================================

/// `CARTESIAN_POINT('label', (x, y, z))`. Length-scales and caches the
/// 3-D coordinates.
pub struct CartesianPointHandler;
/// Static binding consumed by [`register`].
pub static CARTESIAN_POINT_HANDLER: CartesianPointHandler = CartesianPointHandler;

impl EntityHandler for CartesianPointHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::CARTESIAN_POINT]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        _registry: &EntityRegistry,
        _dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields =
            match params::record_fields(&record.parameter, names::CARTESIAN_POINT, instance) {
                Ok(f) => f,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad record shape".into(),
                    };
                }
            };
        if fields.len() < 2 {
            return field_count_error(
                ctx,
                names::CARTESIAN_POINT,
                instance,
                "expected (label, coordinates)",
            );
        }
        let coords = match params::as_real_array::<3>(
            &fields[1],
            names::CARTESIAN_POINT,
            instance,
            "coordinates",
        ) {
            Ok(c) => c,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad coordinates".into(),
                };
            }
        };
        let s = ctx.unit.length;
        ctx.caches
            .points
            .insert(instance, [coords[0] * s, coords[1] * s, coords[2] * s]);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// DIRECTION
// =========================================================================

/// `DIRECTION('label', (dx, dy, dz))`. Heals (normalizes; zero-length
/// emits `ZeroLengthDirection` healing and falls back to `+Z`).
pub struct DirectionHandler;
/// Static binding consumed by [`register`].
pub static DIRECTION_HANDLER: DirectionHandler = DirectionHandler;

impl EntityHandler for DirectionHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::DIRECTION]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        _registry: &EntityRegistry,
        _dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::DIRECTION, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 2 {
            return field_count_error(ctx, names::DIRECTION, instance, "expected (label, ratios)");
        }
        let raw = match params::as_real_array::<3>(
            &fields[1],
            names::DIRECTION,
            instance,
            "direction_ratios",
        ) {
            Ok(d) => d,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad ratios".into(),
                };
            }
        };
        let healed = heal_direction(raw, names::DIRECTION, instance, ctx);
        ctx.caches.directions.insert(instance, healed);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// VECTOR
// =========================================================================

/// `VECTOR('label', #direction_ref, magnitude)`. Resolves the
/// direction reference (lazy if necessary), normalizes it, and stores
/// `direction × scaled_magnitude` in `caches.vectors`.
pub struct VectorHandler;
/// Static binding consumed by [`register`].
pub static VECTOR_HANDLER: VectorHandler = VectorHandler;

impl EntityHandler for VectorHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::VECTOR]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::VECTOR, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 3 {
            return field_count_error(
                ctx,
                names::VECTOR,
                instance,
                "expected (label, orientation, magnitude)",
            );
        }
        let dir_ref =
            match params::as_entity_ref(&fields[1], names::VECTOR, instance, "orientation") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad direction ref".into(),
                    };
                }
            };
        let mag = match params::as_real(&fields[2], names::VECTOR, instance, "magnitude") {
            Ok(m) => m,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad magnitude".into(),
                };
            }
        };
        let dir = match resolve_direction(dir_ref, registry, dispatch, ctx) {
            Some(d) => d,
            None => {
                return HandlerOutcome::Failed {
                    message: "direction unresolved".into(),
                }
            }
        };
        let scaled = mag * ctx.unit.length;
        ctx.caches.vectors.insert(
            instance,
            [dir[0] * scaled, dir[1] * scaled, dir[2] * scaled],
        );
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// AXIS2_PLACEMENT_3D
// =========================================================================

/// `AXIS2_PLACEMENT_3D('label', #location, #axis?, #ref_direction?)`.
///
/// Per ISO 10303-42: `axis` defaults to +Z, `ref_direction` defaults
/// to +X. We resolve every reference, project `ref_direction` into the
/// plane normal to `axis` via [`build_axis_frame`] (heals
/// `ref_direction` parallel to `axis`), and cache the resulting
/// orthonormal frame.
pub struct Axis2Placement3DHandler;
/// Static binding consumed by [`register`].
pub static AXIS2_PLACEMENT_3D_HANDLER: Axis2Placement3DHandler = Axis2Placement3DHandler;

impl EntityHandler for Axis2Placement3DHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::AXIS2_PLACEMENT_3D]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields =
            match params::record_fields(&record.parameter, names::AXIS2_PLACEMENT_3D, instance) {
                Ok(f) => f,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad record shape".into(),
                    };
                }
            };
        if fields.len() < 2 {
            return field_count_error(
                ctx,
                names::AXIS2_PLACEMENT_3D,
                instance,
                "expected (label, location, axis?, ref_direction?)",
            );
        }
        let loc_ref = match params::as_entity_ref(
            &fields[1],
            names::AXIS2_PLACEMENT_3D,
            instance,
            "location",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad location ref".into(),
                };
            }
        };
        let origin = match resolve_point(loc_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "location unresolved".into(),
                }
            }
        };
        let axis_dir = if fields.len() >= 3 {
            match params::as_optional_entity_ref(
                &fields[2],
                names::AXIS2_PLACEMENT_3D,
                instance,
                "axis",
            ) {
                Ok(Some(r)) => {
                    resolve_direction(r, registry, dispatch, ctx).unwrap_or([0.0, 0.0, 1.0])
                }
                Ok(None) => [0.0, 0.0, 1.0],
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    [0.0, 0.0, 1.0]
                }
            }
        } else {
            [0.0, 0.0, 1.0]
        };
        let ref_dir = if fields.len() >= 4 {
            match params::as_optional_entity_ref(
                &fields[3],
                names::AXIS2_PLACEMENT_3D,
                instance,
                "ref_direction",
            ) {
                Ok(Some(r)) => {
                    resolve_direction(r, registry, dispatch, ctx).unwrap_or([1.0, 0.0, 0.0])
                }
                Ok(None) => [1.0, 0.0, 0.0],
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    [1.0, 0.0, 0.0]
                }
            }
        } else {
            [1.0, 0.0, 0.0]
        };

        let (x, y, z) =
            build_axis_frame(axis_dir, ref_dir, names::AXIS2_PLACEMENT_3D, instance, ctx);
        ctx.caches
            .placements
            .insert(instance, Axis2Placement { origin, z, x, y });
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// LINE
// =========================================================================

/// `LINE('label', #pnt, #vec)`. Caches `(origin, unit_direction)` —
/// the kernel curve is allocated per-edge by IMP2.4's `EDGE_CURVE`
/// handler from these data plus the edge's vertex endpoints.
pub struct LineHandler;
/// Static binding consumed by [`register`].
pub static LINE_HANDLER: LineHandler = LineHandler;

impl EntityHandler for LineHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::LINE]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::LINE, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 3 {
            return field_count_error(ctx, names::LINE, instance, "expected (label, pnt, vec)");
        }
        let pnt_ref = match params::as_entity_ref(&fields[1], names::LINE, instance, "pnt") {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad pnt ref".into(),
                };
            }
        };
        let vec_ref = match params::as_entity_ref(&fields[2], names::LINE, instance, "vec") {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad vec ref".into(),
                };
            }
        };
        let origin = match resolve_point(pnt_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "pnt unresolved".into(),
                }
            }
        };
        let raw_dir = match resolve_vector(vec_ref, registry, dispatch, ctx) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "vec unresolved".into(),
                }
            }
        };
        let direction = match normalize(raw_dir) {
            Some(d) => d,
            None => {
                // Already healed by Vector handler (which calls
                // heal_direction on its source DIRECTION). A still-
                // zero magnitude here means the source magnitude was
                // zero; fall back to +X to keep downstream alive.
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: names::LINE.into(),
                    instance: Some(instance),
                    message: "zero-magnitude vec; defaulting line direction to +X".into(),
                });
                [1.0, 0.0, 0.0]
            }
        };
        ctx.caches
            .step_lines
            .insert(instance, StepLineGeom { origin, direction });
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// CIRCLE
// =========================================================================

/// `CIRCLE('label', #axis2_placement, radius)`. Caches placement +
/// length-scaled radius — kernel curve allocated per-edge in IMP2.4.
pub struct CircleHandler;
/// Static binding consumed by [`register`].
pub static CIRCLE_HANDLER: CircleHandler = CircleHandler;

impl EntityHandler for CircleHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::CIRCLE]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::CIRCLE, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 3 {
            return field_count_error(
                ctx,
                names::CIRCLE,
                instance,
                "expected (label, position, radius)",
            );
        }
        let placement_ref =
            match params::as_entity_ref(&fields[1], names::CIRCLE, instance, "position") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad placement ref".into(),
                    };
                }
            };
        let raw_radius = match params::as_real(&fields[2], names::CIRCLE, instance, "radius") {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad radius".into(),
                };
            }
        };
        if raw_radius <= 0.0 {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::CIRCLE.into(),
                instance: Some(instance),
                message: format!("non-positive radius {raw_radius}; CIRCLE skipped"),
            });
            return HandlerOutcome::Failed {
                message: "non-positive radius".into(),
            };
        }
        let placement = match resolve_placement(placement_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "placement unresolved".into(),
                }
            }
        };
        let radius = raw_radius * ctx.unit.length;
        ctx.caches
            .step_circles
            .insert(instance, StepCircleGeom { placement, radius });
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// PLANE
// =========================================================================

/// `PLANE('label', #axis2_placement)`. Allocates a kernel `Plane`
/// into `model.surfaces`.
pub struct PlaneHandler;
/// Static binding consumed by [`register`].
pub static PLANE_HANDLER: PlaneHandler = PlaneHandler;

impl EntityHandler for PlaneHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::PLANE]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::PLANE, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 2 {
            return field_count_error(ctx, names::PLANE, instance, "expected (label, position)");
        }
        let placement_ref =
            match params::as_entity_ref(&fields[1], names::PLANE, instance, "position") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad placement ref".into(),
                    };
                }
            };
        let p = match resolve_placement(placement_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "placement unresolved".into(),
                }
            }
        };
        let plane = match Plane::new(
            Point3::new(p.origin[0], p.origin[1], p.origin[2]),
            Vector3::new(p.z[0], p.z[1], p.z[2]),
            Vector3::new(p.x[0], p.x[1], p.x[2]),
        ) {
            Ok(s) => s,
            Err(e) => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: names::PLANE.into(),
                    instance: Some(instance),
                    message: format!("kernel rejected Plane: {e}"),
                });
                return HandlerOutcome::Failed {
                    message: "kernel rejected plane".into(),
                };
            }
        };
        let sid = ctx.model.surfaces.add(Box::new(plane));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// CYLINDRICAL_SURFACE
// =========================================================================

/// `CYLINDRICAL_SURFACE('label', #axis2_placement, radius)`. Allocates
/// a kernel `Cylinder` into `model.surfaces`.
pub struct CylindricalSurfaceHandler;
/// Static binding consumed by [`register`].
pub static CYLINDRICAL_SURFACE_HANDLER: CylindricalSurfaceHandler = CylindricalSurfaceHandler;

impl EntityHandler for CylindricalSurfaceHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::CYLINDRICAL_SURFACE]
    }
    fn phase(&self) -> Phase {
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields =
            match params::record_fields(&record.parameter, names::CYLINDRICAL_SURFACE, instance) {
                Ok(f) => f,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad record shape".into(),
                    };
                }
            };
        if fields.len() < 3 {
            return field_count_error(
                ctx,
                names::CYLINDRICAL_SURFACE,
                instance,
                "expected (label, position, radius)",
            );
        }
        let placement_ref = match params::as_entity_ref(
            &fields[1],
            names::CYLINDRICAL_SURFACE,
            instance,
            "position",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad placement ref".into(),
                };
            }
        };
        let raw_radius =
            match params::as_real(&fields[2], names::CYLINDRICAL_SURFACE, instance, "radius") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad radius".into(),
                    };
                }
            };
        if raw_radius <= 0.0 {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::CYLINDRICAL_SURFACE.into(),
                instance: Some(instance),
                message: format!("non-positive radius {raw_radius}; CYLINDRICAL_SURFACE skipped"),
            });
            return HandlerOutcome::Failed {
                message: "non-positive radius".into(),
            };
        }
        let p = match resolve_placement(placement_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "placement unresolved".into(),
                }
            }
        };
        let radius = raw_radius * ctx.unit.length;
        let cyl = match Cylinder::new(
            Point3::new(p.origin[0], p.origin[1], p.origin[2]),
            Vector3::new(p.z[0], p.z[1], p.z[2]),
            radius,
        ) {
            Ok(s) => s,
            Err(e) => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: names::CYLINDRICAL_SURFACE.into(),
                    instance: Some(instance),
                    message: format!("kernel rejected Cylinder: {e}"),
                });
                return HandlerOutcome::Failed {
                    message: "kernel rejected cylinder".into(),
                };
            }
        };
        let sid = ctx.model.surfaces.add(Box::new(cyl));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// VERTEX_POINT
// =========================================================================

/// `VERTEX_POINT('label', #cartesian_point)`. Allocates a kernel
/// vertex from the resolved point and caches its id.
pub struct VertexPointHandler;
/// Static binding consumed by [`register`].
pub static VERTEX_POINT_HANDLER: VertexPointHandler = VertexPointHandler;

impl EntityHandler for VertexPointHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::VERTEX_POINT]
    }
    fn phase(&self) -> Phase {
        // Vertex points reference geometry; we still run them in the
        // Geometry phase so that `EDGE_CURVE` (Topology) can rely on
        // every vertex being materialised by the time it runs.
        Phase::Geometry
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::VERTEX_POINT, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 2 {
            return field_count_error(
                ctx,
                names::VERTEX_POINT,
                instance,
                "expected (label, vertex_geometry)",
            );
        }
        let pnt_ref = match params::as_entity_ref(
            &fields[1],
            names::VERTEX_POINT,
            instance,
            "vertex_geometry",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad point ref".into(),
                };
            }
        };
        let p = match resolve_point(pnt_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "point unresolved".into(),
                }
            }
        };
        let vid = ctx.model.vertices.add(p[0], p[1], p[2]);
        ctx.caches.vertices.insert(instance, vid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// Shared resolver helpers
// =========================================================================

/// Push a `Severity::Warn` Warning recording a too-short field list.
fn field_count_error(
    ctx: &mut ImportContext<'_>,
    entity: &str,
    instance: u64,
    detail: &str,
) -> HandlerOutcome {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: entity.into(),
        instance: Some(instance),
        message: format!("too few fields on {entity}: {detail}"),
    });
    HandlerOutcome::Failed {
        message: "too few fields".into(),
    }
}

/// Force `instance` to resolve as a CARTESIAN_POINT, returning the
/// cached 3-D point on success. The first lookup hits the cache; on
/// miss we fall back to [`ensure_resolved`].
fn resolve_point(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<[f64; 3]> {
    if let Some(p) = ctx.caches.points.get(&instance) {
        return Some(*p);
    }
    let _ = ensure_resolved(instance, &[names::CARTESIAN_POINT], registry, dispatch, ctx);
    ctx.caches.points.get(&instance).copied()
}

/// Force `instance` to resolve as a DIRECTION, returning the healed
/// unit vector on success.
fn resolve_direction(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<[f64; 3]> {
    if let Some(d) = ctx.caches.directions.get(&instance) {
        return Some(*d);
    }
    let _ = ensure_resolved(instance, &[names::DIRECTION], registry, dispatch, ctx);
    ctx.caches.directions.get(&instance).copied()
}

/// Force `instance` to resolve as a VECTOR, returning
/// `direction × scaled_magnitude` on success.
fn resolve_vector(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<[f64; 3]> {
    if let Some(v) = ctx.caches.vectors.get(&instance) {
        return Some(*v);
    }
    let _ = ensure_resolved(instance, &[names::VECTOR], registry, dispatch, ctx);
    ctx.caches.vectors.get(&instance).copied()
}

/// Force `instance` to resolve as an AXIS2_PLACEMENT_3D, returning
/// the cached frame on success.
fn resolve_placement(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<Axis2Placement> {
    if let Some(p) = ctx.caches.placements.get(&instance) {
        return Some(*p);
    }
    let _ = ensure_resolved(
        instance,
        &[names::AXIS2_PLACEMENT_3D],
        registry,
        dispatch,
        ctx,
    );
    ctx.caches.placements.get(&instance).copied()
}

/// Register every geometry-phase handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&CARTESIAN_POINT_HANDLER);
    dispatch.register(&DIRECTION_HANDLER);
    dispatch.register(&VECTOR_HANDLER);
    dispatch.register(&AXIS2_PLACEMENT_3D_HANDLER);
    dispatch.register(&LINE_HANDLER);
    dispatch.register(&CIRCLE_HANDLER);
    dispatch.register(&PLANE_HANDLER);
    dispatch.register(&CYLINDRICAL_SURFACE_HANDLER);
    dispatch.register(&VERTEX_POINT_HANDLER);
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::{
        context::{ImportContext, ResolutionCaches, UnitScale},
        diagnostics::{HealingKind, ImportReport},
        dispatch::EntityDispatch,
        parser::parse_step,
        registry::EntityRegistry,
    };
    use geometry_engine::primitives::topology_builder::BRepModel;

    /// Wrap a DATA body in the minimal STEP envelope `parse_step`
    /// accepts.
    fn wrap(body: &str) -> String {
        format!(
            "ISO-10303-21;\n\
             HEADER;\n\
             FILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n\
             ENDSEC;\n\
             DATA;\n\
             {body}\n\
             ENDSEC;\n\
             END-ISO-10303-21;\n"
        )
    }

    /// Build (registry, dispatch, model, report) and run every
    /// geometry-phase handler against `body`. Returns the populated
    /// model + report and the import context's caches.
    fn run(body: &str) -> (BRepModel, ImportReport, ResolutionCaches, UnitScale) {
        run_with_unit(body, UnitScale::default())
    }

    fn run_with_unit(
        body: &str,
        unit: UnitScale,
    ) -> (BRepModel, ImportReport, ResolutionCaches, UnitScale) {
        let src = wrap(body);
        let ex = parse_step(&src, "test").expect("parse");
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        ctx.unit = unit;
        let _ = dispatch.run_all(&reg, &mut ctx);
        let final_unit = ctx.unit;
        let caches = std::mem::take(&mut ctx.caches);
        (model, report, caches, final_unit)
    }

    // ------- CARTESIAN_POINT -------

    #[test]
    fn cartesian_point_happy_path() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(1.,2.,3.));");
        assert_eq!(c.points.get(&1), Some(&[1.0, 2.0, 3.0]));
    }

    #[test]
    fn cartesian_point_origin() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));");
        assert_eq!(c.points.get(&1), Some(&[0.0, 0.0, 0.0]));
    }

    #[test]
    fn cartesian_point_negative_coords() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(-1.5,-2.,-3.));");
        assert_eq!(c.points.get(&1), Some(&[-1.5, -2.0, -3.0]));
    }

    #[test]
    fn cartesian_point_unit_scaled_to_mm() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_m, _r, c, _) = run_with_unit("#1=CARTESIAN_POINT('',(1.,0.,0.));", unit);
        let p = c.points.get(&1).unwrap();
        assert!((p[0] - 25.4).abs() < 1e-12);
    }

    #[test]
    fn cartesian_point_wrong_arity_warns() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(1.,2.));");
        assert!(!c.points.contains_key(&1));
        assert!(r.warnings.iter().any(|w| w.entity == "CARTESIAN_POINT"));
    }

    #[test]
    fn cartesian_point_extreme_magnitudes_preserve_precision() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(1.0E6,1.0E-6,0.));");
        let p = c.points.get(&1).unwrap();
        assert!((p[0] - 1.0e6).abs() < 1e-3);
        assert!((p[1] - 1.0e-6).abs() < 1e-15);
    }

    #[test]
    fn cartesian_point_integer_coordinate_coerces_to_real() {
        // STEP files sometimes emit `0` instead of `0.0` — params::as_real coerces.
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0,0,0));");
        assert_eq!(c.points.get(&1), Some(&[0.0, 0.0, 0.0]));
    }

    // ------- DIRECTION -------

    #[test]
    fn direction_happy_path_unit_vector() {
        let (_m, _r, c, _) = run("#1=DIRECTION('',(1.,0.,0.));");
        let d = c.directions.get(&1).unwrap();
        assert!((d[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn direction_non_unit_is_normalized() {
        let (_m, _r, c, _) = run("#1=DIRECTION('',(3.,0.,0.));");
        let d = c.directions.get(&1).unwrap();
        assert!((d[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn direction_zero_length_emits_healing_and_falls_back() {
        let (_m, r, c, _) = run("#1=DIRECTION('',(0.,0.,0.));");
        // Heals: fallback unit direction stored.
        assert!(c.directions.contains_key(&1));
        assert!(r
            .healings
            .iter()
            .any(|h| matches!(h.kind, HealingKind::ZeroLengthDirection)));
    }

    #[test]
    fn direction_is_dimensionless_no_unit_scaling() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_m, _r, c, _) = run_with_unit("#1=DIRECTION('',(1.,0.,0.));", unit);
        let d = c.directions.get(&1).unwrap();
        assert!(
            (d[0] - 1.0).abs() < 1e-12,
            "direction must not be unit-scaled"
        );
    }

    #[test]
    fn direction_negative_components_preserved_after_normalize() {
        let (_m, _r, c, _) = run("#1=DIRECTION('',(-2.,0.,0.));");
        let d = c.directions.get(&1).unwrap();
        assert!((d[0] - (-1.0)).abs() < 1e-12);
    }

    #[test]
    fn direction_off_axis_unit_after_normalize() {
        let (_m, _r, c, _) = run("#1=DIRECTION('',(1.,1.,0.));");
        let d = c.directions.get(&1).unwrap();
        let m = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!((m - 1.0).abs() < 1e-12);
    }

    #[test]
    fn direction_wrong_arity_warns() {
        let (_m, r, c, _) = run("#1=DIRECTION('',(1.,0.));");
        assert!(!c.directions.contains_key(&1));
        assert!(r.warnings.iter().any(|w| w.entity == "DIRECTION"));
    }

    // ------- VECTOR -------

    #[test]
    fn vector_happy_path() {
        let (_m, _r, c, _) = run("#1=DIRECTION('',(1.,0.,0.));\
             #2=VECTOR('',#1,5.);");
        let v = c.vectors.get(&2).unwrap();
        assert!((v[0] - 5.0).abs() < 1e-12);
    }

    #[test]
    fn vector_unit_scales_magnitude() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_m, _r, c, _) = run_with_unit(
            "#1=DIRECTION('',(1.,0.,0.));\
             #2=VECTOR('',#1,2.);",
            unit,
        );
        let v = c.vectors.get(&2).unwrap();
        assert!((v[0] - 50.8).abs() < 1e-9, "v[0] = {}", v[0]);
    }

    #[test]
    fn vector_zero_magnitude_yields_zero_vector() {
        let (_m, _r, c, _) = run("#1=DIRECTION('',(1.,0.,0.));\
             #2=VECTOR('',#1,0.);");
        let v = c.vectors.get(&2).unwrap();
        assert_eq!(v, &[0.0, 0.0, 0.0]);
    }

    #[test]
    fn vector_resolves_cross_phase_direction() {
        // VECTOR appears in source before DIRECTION; the lazy
        // resolver must materialise the direction first.
        let (_m, _r, c, _) = run("#2=VECTOR('',#1,1.);\
             #1=DIRECTION('',(0.,1.,0.));");
        let v = c.vectors.get(&2).unwrap();
        assert!((v[1] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn vector_missing_direction_ref_fails() {
        let (_m, _r, c, _) = run("#2=VECTOR('',#99,1.);");
        assert!(!c.vectors.contains_key(&2));
    }

    #[test]
    fn vector_wrong_arity_warns() {
        let (_m, r, c, _) = run("#1=DIRECTION('',(1.,0.,0.));\
             #2=VECTOR('',#1);");
        assert!(!c.vectors.contains_key(&2));
        assert!(r.warnings.iter().any(|w| w.entity == "VECTOR"));
    }

    // ------- AXIS2_PLACEMENT_3D -------

    #[test]
    fn axis2_placement_happy_path() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);");
        let p = c.placements.get(&4).unwrap();
        assert!((p.z[2] - 1.0).abs() < 1e-12);
        assert!((p.x[0] - 1.0).abs() < 1e-12);
        // y = z × x = +Y
        assert!((p.y[1] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn axis2_placement_origin_carries_unit_scaling() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_m, _r, c, _) = run_with_unit(
            "#1=CARTESIAN_POINT('',(1.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);",
            unit,
        );
        let p = c.placements.get(&4).unwrap();
        assert!((p.origin[0] - 25.4).abs() < 1e-9);
    }

    #[test]
    fn axis2_placement_ref_direction_projected_when_not_perpendicular() {
        // ref_direction = (1,0,1) is NOT perpendicular to axis (0,0,1).
        // build_axis_frame should project it onto the z=0 plane → (1,0,0).
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,1.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);");
        let p = c.placements.get(&4).unwrap();
        assert!((p.x[0] - 1.0).abs() < 1e-12);
        assert!(p.x[2].abs() < 1e-12);
    }

    #[test]
    fn axis2_placement_ref_parallel_to_axis_is_healed() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(0.,0.,1.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);");
        let p = c.placements.get(&4).unwrap();
        // x must be unit-length and perpendicular to z.
        let dot = p.x[0] * p.z[0] + p.x[1] * p.z[1] + p.x[2] * p.z[2];
        assert!(dot.abs() < 1e-9);
        assert!(r
            .healings
            .iter()
            .any(|h| matches!(h.kind, HealingKind::PlacementAxisDegenerate)));
    }

    #[test]
    fn axis2_placement_omitted_axis_defaults_to_z_up() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,$,$);");
        let p = c.placements.get(&4).unwrap();
        assert!((p.z[2] - 1.0).abs() < 1e-12);
        assert!((p.x[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn axis2_placement_forwards_resolution_to_dependencies() {
        // Out-of-order: AXIS2 first, then its DIRECTIONs / POINTs.
        let (_m, _r, c, _) = run("#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #3=DIRECTION('',(1.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #1=CARTESIAN_POINT('',(0.,0.,0.));");
        assert!(c.placements.contains_key(&4));
    }

    #[test]
    fn axis2_placement_wrong_arity_warns() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('');");
        assert!(!c.placements.contains_key(&4));
        assert!(r.warnings.iter().any(|w| w.entity == "AXIS2_PLACEMENT_3D"));
    }

    // ------- LINE -------

    #[test]
    fn line_happy_path() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(1.,0.,0.));\
             #3=VECTOR('',#2,1.);\
             #4=LINE('',#1,#3);");
        let l = c.step_lines.get(&4).unwrap();
        assert_eq!(l.origin, [0.0, 0.0, 0.0]);
        assert!((l.direction[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn line_unit_scales_origin() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_m, _r, c, _) = run_with_unit(
            "#1=CARTESIAN_POINT('',(1.,0.,0.));\
             #2=DIRECTION('',(1.,0.,0.));\
             #3=VECTOR('',#2,1.);\
             #4=LINE('',#1,#3);",
            unit,
        );
        let l = c.step_lines.get(&4).unwrap();
        assert!((l.origin[0] - 25.4).abs() < 1e-9);
    }

    #[test]
    fn line_normalizes_direction_regardless_of_vector_magnitude() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(1.,0.,0.));\
             #3=VECTOR('',#2,500.);\
             #4=LINE('',#1,#3);");
        let l = c.step_lines.get(&4).unwrap();
        assert!((l.direction[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn line_zero_vector_falls_back_to_x() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(1.,0.,0.));\
             #3=VECTOR('',#2,0.);\
             #4=LINE('',#1,#3);");
        let l = c.step_lines.get(&4).unwrap();
        assert!((l.direction[0] - 1.0).abs() < 1e-12);
        assert!(r
            .warnings
            .iter()
            .any(|w| w.entity == "LINE" && w.message.contains("zero-magnitude")));
    }

    #[test]
    fn line_wrong_arity_warns() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #4=LINE('',#1);");
        assert!(!c.step_lines.contains_key(&4));
        assert!(r.warnings.iter().any(|w| w.entity == "LINE"));
    }

    // ------- CIRCLE -------

    #[test]
    fn circle_happy_path() {
        let (_m, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CIRCLE('',#4,2.5);");
        let circ = c.step_circles.get(&5).unwrap();
        assert!((circ.radius - 2.5).abs() < 1e-12);
        assert!((circ.placement.z[2] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn circle_radius_unit_scaled() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_m, _r, c, _) = run_with_unit(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CIRCLE('',#4,1.);",
            unit,
        );
        let circ = c.step_circles.get(&5).unwrap();
        assert!((circ.radius - 25.4).abs() < 1e-9);
    }

    #[test]
    fn circle_zero_radius_rejected() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CIRCLE('',#4,0.);");
        assert!(!c.step_circles.contains_key(&5));
        assert!(r
            .warnings
            .iter()
            .any(|w| w.entity == "CIRCLE" && w.message.contains("non-positive")));
    }

    #[test]
    fn circle_negative_radius_rejected() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CIRCLE('',#4,-1.5);");
        assert!(!c.step_circles.contains_key(&5));
        assert!(r.warnings.iter().any(|w| w.entity == "CIRCLE"));
    }

    #[test]
    fn circle_missing_placement_fails() {
        let (_m, _r, c, _) = run("#5=CIRCLE('',#99,1.);");
        assert!(!c.step_circles.contains_key(&5));
    }

    #[test]
    fn circle_wrong_arity_warns() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CIRCLE('',#4);");
        assert!(!c.step_circles.contains_key(&5));
        assert!(r.warnings.iter().any(|w| w.entity == "CIRCLE"));
    }

    // ------- PLANE -------

    #[test]
    fn plane_happy_path_allocates_surface() {
        let (model, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=PLANE('',#4);");
        let sid = c.surfaces.get(&5).copied().unwrap();
        assert!(model.surfaces.get(sid).is_some());
    }

    #[test]
    fn plane_origin_uses_unit_scaling() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_m, _r, c, _) = run_with_unit(
            "#1=CARTESIAN_POINT('',(10.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=PLANE('',#4);",
            unit,
        );
        let p = c.placements.get(&4).unwrap();
        assert!((p.origin[0] - 254.0).abs() < 1e-9);
        assert!(c.surfaces.contains_key(&5));
    }

    #[test]
    fn plane_wrong_arity_warns() {
        let (_m, r, c, _) = run("#5=PLANE('');");
        assert!(!c.surfaces.contains_key(&5));
        assert!(r.warnings.iter().any(|w| w.entity == "PLANE"));
    }

    #[test]
    fn plane_resolves_cross_phase_dependencies() {
        // Forward-reference: PLANE first, deps later.
        let (_m, _r, c, _) = run("#5=PLANE('',#4);\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #3=DIRECTION('',(1.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #1=CARTESIAN_POINT('',(0.,0.,0.));");
        assert!(c.surfaces.contains_key(&5));
    }

    // ------- CYLINDRICAL_SURFACE -------

    #[test]
    fn cylindrical_surface_happy_path() {
        let (model, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CYLINDRICAL_SURFACE('',#4,1.);");
        let sid = c.surfaces.get(&5).copied().unwrap();
        assert!(model.surfaces.get(sid).is_some());
    }

    #[test]
    fn cylindrical_surface_radius_unit_scaled() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        // The surface is constructed with the unit-scaled radius; we
        // check that the surface allocation succeeded (a zero radius
        // would fail in Cylinder::new).
        let (_model, _r, c, _) = run_with_unit(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CYLINDRICAL_SURFACE('',#4,1.);",
            unit,
        );
        assert!(c.surfaces.contains_key(&5));
    }

    #[test]
    fn cylindrical_surface_zero_radius_rejected() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CYLINDRICAL_SURFACE('',#4,0.);");
        assert!(!c.surfaces.contains_key(&5));
        assert!(r
            .warnings
            .iter()
            .any(|w| w.entity == "CYLINDRICAL_SURFACE" && w.message.contains("non-positive")));
    }

    #[test]
    fn cylindrical_surface_negative_radius_rejected() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CYLINDRICAL_SURFACE('',#4,-2.);");
        assert!(!c.surfaces.contains_key(&5));
        assert!(r.warnings.iter().any(|w| w.entity == "CYLINDRICAL_SURFACE"));
    }

    #[test]
    fn cylindrical_surface_wrong_arity_warns() {
        let (_m, r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=DIRECTION('',(0.,0.,1.));\
             #3=DIRECTION('',(1.,0.,0.));\
             #4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\
             #5=CYLINDRICAL_SURFACE('',#4);");
        assert!(!c.surfaces.contains_key(&5));
        assert!(r.warnings.iter().any(|w| w.entity == "CYLINDRICAL_SURFACE"));
    }

    // ------- VERTEX_POINT -------

    #[test]
    fn vertex_point_happy_path() {
        let (model, _r, c, _) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=VERTEX_POINT('',#1);");
        let vid = c.vertices.get(&2).copied().unwrap();
        assert!(model.vertices.get(vid).is_some());
    }

    #[test]
    fn vertex_point_propagates_unit_scaling() {
        let unit = UnitScale {
            length: 25.4,
            angle_radians_per_source: 1.0,
        };
        let (_model, _r, c, _) = run_with_unit(
            "#1=CARTESIAN_POINT('',(1.,0.,0.));\
             #2=VERTEX_POINT('',#1);",
            unit,
        );
        // Vertex coordinates are reflected in the cached point.
        let p = c.points.get(&1).unwrap();
        assert!((p[0] - 25.4).abs() < 1e-9);
        assert!(c.vertices.contains_key(&2));
    }

    #[test]
    fn vertex_point_missing_point_ref_fails() {
        let (_m, _r, c, _) = run("#2=VERTEX_POINT('',#99);");
        assert!(!c.vertices.contains_key(&2));
    }

    #[test]
    fn vertex_point_wrong_arity_warns() {
        let (_m, r, c, _) = run("#2=VERTEX_POINT('');");
        assert!(!c.vertices.contains_key(&2));
        assert!(r.warnings.iter().any(|w| w.entity == "VERTEX_POINT"));
    }

    #[test]
    fn vertex_point_resolves_cross_phase() {
        let (_m, _r, c, _) = run("#2=VERTEX_POINT('',#1);\
             #1=CARTESIAN_POINT('',(7.,8.,9.));");
        assert!(c.vertices.contains_key(&2));
        assert_eq!(c.points.get(&1), Some(&[7.0, 8.0, 9.0]));
    }

    // ------- Integration: full unit-cube-corner geometry chain -------

    #[test]
    fn integration_full_chain_resolves_in_any_order() {
        // Pour everything in reverse source order to exercise the
        // lazy resolver maximally. The HashMap-backed registry has
        // no source-order guarantee, so every cross-reference must
        // be resolved on demand by `ensure_resolved`.
        let body = "\
             #10=PLANE('top',#9);\
             #9=AXIS2_PLACEMENT_3D('',#3,#5,#7);\
             #8=CYLINDRICAL_SURFACE('side',#9,3.);\
             #7=DIRECTION('x',(1.,0.,0.));\
             #6=VERTEX_POINT('v',#3);\
             #5=DIRECTION('z',(0.,0.,1.));\
             #4=LINE('edge',#3,#11);\
             #11=VECTOR('',#7,1.);\
             #3=CARTESIAN_POINT('p',(1.,2.,3.));\
             #2=CIRCLE('c',#9,2.5);\
             #1=CARTESIAN_POINT('o',(0.,0.,0.));";
        let (_m, _r, c, _) = run(body);
        assert!(c.points.contains_key(&1));
        assert!(c.points.contains_key(&3));
        assert!(c.directions.contains_key(&5));
        assert!(c.directions.contains_key(&7));
        assert!(c.vectors.contains_key(&11));
        assert!(c.placements.contains_key(&9));
        assert!(c.step_lines.contains_key(&4));
        assert!(c.step_circles.contains_key(&2));
        assert!(c.surfaces.contains_key(&8));
        assert!(c.surfaces.contains_key(&10));
        assert!(c.vertices.contains_key(&6));
    }
}
