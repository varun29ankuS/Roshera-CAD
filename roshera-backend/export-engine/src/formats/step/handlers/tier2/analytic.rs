//! Analytic-surface handlers for the tier-2 family:
//! `SPHERICAL_SURFACE`, `TOROIDAL_SURFACE`, `CONICAL_SURFACE`.
//!
//! | STEP                 | Kernel call                                               | Cache             |
//! |----------------------|-----------------------------------------------------------|-------------------|
//! | `SPHERICAL_SURFACE`  | [`Sphere::new`]`(center, radius)`                         | `caches.surfaces` |
//! | `TOROIDAL_SURFACE`   | [`Torus::new`]`(center, axis, major, minor)`              | `caches.surfaces` |
//! | `CONICAL_SURFACE`    | [`Cone::new`]`(apex, axis, half_angle)` (apex derived)    | `caches.surfaces` |
//!
//! All three follow the tier-1 `PLANE` / `CYLINDRICAL_SURFACE`
//! pattern: resolve `AXIS2_PLACEMENT_3D`, length-scale the radii,
//! call the kernel constructor, register the resulting `SurfaceId`.
//!
//! ## CONICAL_SURFACE apex derivation
//!
//! ISO 10303-42 places the reference frame at a cross-section circle
//! of radius `radius`, with `axis` (the placement's z-direction)
//! pointing from base toward apex. The apex therefore lies at
//!
//! ```text
//!     apex = origin − axis · (radius / tan(semi_angle))
//! ```
//!
//! When `radius == 0` the placement origin is already the apex, and
//! we skip the offset. Both `radius < 0` and `semi_angle` outside
//! `(0, π/2)` are rejected before reaching the kernel; the kernel
//! also enforces the bound, but rejecting early keeps the warning
//! attribution on the STEP entity instead of the kernel call.

use std::f64::consts;

use ruststep::ast::Record;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::surface::{Cone, Sphere, Torus};

use crate::formats::step::{
    context::{Axis2Placement, ImportContext},
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
};

use super::super::tier1::params;
use super::super::tier1::resolver::ensure_resolved;

mod names {
    pub const SPHERICAL_SURFACE: &str = "SPHERICAL_SURFACE";
    pub const TOROIDAL_SURFACE: &str = "TOROIDAL_SURFACE";
    pub const CONICAL_SURFACE: &str = "CONICAL_SURFACE";
    pub const AXIS2_PLACEMENT_3D: &str = "AXIS2_PLACEMENT_3D";
}

// =========================================================================
// SPHERICAL_SURFACE
// =========================================================================

/// `SPHERICAL_SURFACE('label', #axis2_placement, radius)`. Allocates a
/// kernel `Sphere` (centred at the placement origin) into
/// `model.surfaces`.
pub struct SphericalSurfaceHandler;
/// Static binding consumed by [`register`].
pub static SPHERICAL_SURFACE_HANDLER: SphericalSurfaceHandler = SphericalSurfaceHandler;

impl EntityHandler for SphericalSurfaceHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::SPHERICAL_SURFACE]
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
            match params::record_fields(&record.parameter, names::SPHERICAL_SURFACE, instance) {
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
                names::SPHERICAL_SURFACE,
                instance,
                "expected (label, position, radius)",
                fields.len(),
            );
        }
        let placement_ref =
            match params::as_entity_ref(&fields[1], names::SPHERICAL_SURFACE, instance, "position")
            {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad placement ref".into(),
                    };
                }
            };
        let raw_radius =
            match params::as_real(&fields[2], names::SPHERICAL_SURFACE, instance, "radius") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad radius".into(),
                    };
                }
            };
        if raw_radius <= 0.0 {
            push_warn(
                ctx,
                names::SPHERICAL_SURFACE,
                instance,
                format!("non-positive radius {raw_radius}; SPHERICAL_SURFACE skipped"),
            );
            return HandlerOutcome::Failed {
                message: "non-positive radius".into(),
            };
        }
        let placement = match resolve_placement(placement_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "placement unresolved".into(),
                };
            }
        };
        let radius = raw_radius * ctx.unit.length;
        let sphere = match Sphere::new(
            Point3::new(
                placement.origin[0],
                placement.origin[1],
                placement.origin[2],
            ),
            radius,
        ) {
            Ok(s) => s,
            Err(e) => {
                push_warn(
                    ctx,
                    names::SPHERICAL_SURFACE,
                    instance,
                    format!("kernel rejected Sphere: {e}"),
                );
                return HandlerOutcome::Failed {
                    message: "kernel rejected sphere".into(),
                };
            }
        };
        let sid = ctx.model.surfaces.add(Box::new(sphere));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// TOROIDAL_SURFACE
// =========================================================================

/// `TOROIDAL_SURFACE('label', #axis2_placement, major_radius,
/// minor_radius)`. Allocates a kernel `Torus` into `model.surfaces`.
pub struct ToroidalSurfaceHandler;
/// Static binding consumed by [`register`].
pub static TOROIDAL_SURFACE_HANDLER: ToroidalSurfaceHandler = ToroidalSurfaceHandler;

impl EntityHandler for ToroidalSurfaceHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::TOROIDAL_SURFACE]
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
            match params::record_fields(&record.parameter, names::TOROIDAL_SURFACE, instance) {
                Ok(f) => f,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad record shape".into(),
                    };
                }
            };
        if fields.len() < 4 {
            return field_count_error(
                ctx,
                names::TOROIDAL_SURFACE,
                instance,
                "expected (label, position, major_radius, minor_radius)",
                fields.len(),
            );
        }
        let placement_ref = match params::as_entity_ref(
            &fields[1],
            names::TOROIDAL_SURFACE,
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
        let raw_major = match params::as_real(
            &fields[2],
            names::TOROIDAL_SURFACE,
            instance,
            "major_radius",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad major_radius".into(),
                };
            }
        };
        let raw_minor = match params::as_real(
            &fields[3],
            names::TOROIDAL_SURFACE,
            instance,
            "minor_radius",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad minor_radius".into(),
                };
            }
        };
        if raw_major <= 0.0 || raw_minor <= 0.0 {
            push_warn(
                ctx,
                names::TOROIDAL_SURFACE,
                instance,
                format!(
                    "non-positive radii (major={raw_major}, minor={raw_minor}); TOROIDAL_SURFACE skipped"
                ),
            );
            return HandlerOutcome::Failed {
                message: "non-positive radius".into(),
            };
        }
        // STEP allows "spindle" tori where minor_radius > major_radius
        // (self-intersecting). The kernel rejects those; surface the
        // rejection up-front so the warning attribution is clean.
        if raw_minor >= raw_major {
            push_warn(
                ctx,
                names::TOROIDAL_SURFACE,
                instance,
                format!(
                    "minor_radius {raw_minor} ≥ major_radius {raw_major}; spindle/horn torus unsupported"
                ),
            );
            return HandlerOutcome::Failed {
                message: "spindle torus".into(),
            };
        }
        let placement = match resolve_placement(placement_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "placement unresolved".into(),
                };
            }
        };
        let major = raw_major * ctx.unit.length;
        let minor = raw_minor * ctx.unit.length;
        let torus = match Torus::new(
            Point3::new(
                placement.origin[0],
                placement.origin[1],
                placement.origin[2],
            ),
            Vector3::new(placement.z[0], placement.z[1], placement.z[2]),
            major,
            minor,
        ) {
            Ok(t) => t,
            Err(e) => {
                push_warn(
                    ctx,
                    names::TOROIDAL_SURFACE,
                    instance,
                    format!("kernel rejected Torus: {e}"),
                );
                return HandlerOutcome::Failed {
                    message: "kernel rejected torus".into(),
                };
            }
        };
        let sid = ctx.model.surfaces.add(Box::new(torus));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// CONICAL_SURFACE
// =========================================================================

/// `CONICAL_SURFACE('label', #axis2_placement, radius, semi_angle)`.
/// Derives apex = `origin − axis · (radius / tan(semi_angle))` and
/// allocates a kernel `Cone` into `model.surfaces`. `semi_angle` is
/// interpreted as radians (STEP `plane_angle_measure`); no unit-scale
/// applied because units don't carry the plane-angle conversion in
/// `UnitScale` today (defaults to radians per ISO).
pub struct ConicalSurfaceHandler;
/// Static binding consumed by [`register`].
pub static CONICAL_SURFACE_HANDLER: ConicalSurfaceHandler = ConicalSurfaceHandler;

impl EntityHandler for ConicalSurfaceHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::CONICAL_SURFACE]
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
            match params::record_fields(&record.parameter, names::CONICAL_SURFACE, instance) {
                Ok(f) => f,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad record shape".into(),
                    };
                }
            };
        if fields.len() < 4 {
            return field_count_error(
                ctx,
                names::CONICAL_SURFACE,
                instance,
                "expected (label, position, radius, semi_angle)",
                fields.len(),
            );
        }
        let placement_ref =
            match params::as_entity_ref(&fields[1], names::CONICAL_SURFACE, instance, "position") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad placement ref".into(),
                    };
                }
            };
        let raw_radius =
            match params::as_real(&fields[2], names::CONICAL_SURFACE, instance, "radius") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad radius".into(),
                    };
                }
            };
        let semi_angle =
            match params::as_real(&fields[3], names::CONICAL_SURFACE, instance, "semi_angle") {
                Ok(a) => a,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad semi_angle".into(),
                    };
                }
            };
        if raw_radius < 0.0 {
            push_warn(
                ctx,
                names::CONICAL_SURFACE,
                instance,
                format!("negative radius {raw_radius}; CONICAL_SURFACE skipped"),
            );
            return HandlerOutcome::Failed {
                message: "negative radius".into(),
            };
        }
        if semi_angle <= 0.0 || semi_angle >= consts::PI / 2.0 {
            push_warn(
                ctx,
                names::CONICAL_SURFACE,
                instance,
                format!("semi_angle {semi_angle} not in (0, π/2)"),
            );
            return HandlerOutcome::Failed {
                message: "semi_angle out of range".into(),
            };
        }
        let placement = match resolve_placement(placement_ref, registry, dispatch, ctx) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "placement unresolved".into(),
                };
            }
        };
        let radius = raw_radius * ctx.unit.length;
        // axis points from base circle toward apex (STEP convention).
        let axis = Vector3::new(placement.z[0], placement.z[1], placement.z[2]);
        // Apex = origin − axis · (radius / tan(semi_angle)). When
        // radius == 0 the placement origin IS the apex; tan(semi_angle)
        // can't underflow inside (0, π/2) so the division is safe.
        let offset = if radius == 0.0 {
            0.0
        } else {
            radius / semi_angle.tan()
        };
        let apex = Point3::new(
            placement.origin[0] - axis.x * offset,
            placement.origin[1] - axis.y * offset,
            placement.origin[2] - axis.z * offset,
        );
        let cone = match Cone::new(apex, axis, semi_angle) {
            Ok(c) => c,
            Err(e) => {
                push_warn(
                    ctx,
                    names::CONICAL_SURFACE,
                    instance,
                    format!("kernel rejected Cone: {e}"),
                );
                return HandlerOutcome::Failed {
                    message: "kernel rejected cone".into(),
                };
            }
        };
        let sid = ctx.model.surfaces.add(Box::new(cone));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// Shared helpers
// =========================================================================

/// Force `instance` to resolve as an `AXIS2_PLACEMENT_3D`, returning
/// the cached frame on success. Mirrors `tier1::geometry`'s private
/// helper so tier-2 doesn't depend on `pub(crate)` exposure.
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

fn field_count_error(
    ctx: &mut ImportContext<'_>,
    entity: &str,
    instance: u64,
    detail: &str,
    got: usize,
) -> HandlerOutcome {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: entity.into(),
        instance: Some(instance),
        message: format!("{entity} has {got} fields: {detail}"),
    });
    HandlerOutcome::Failed {
        message: "wrong arity".into(),
    }
}

fn push_warn(ctx: &mut ImportContext<'_>, entity: &'static str, instance: u64, message: String) {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: entity.into(),
        instance: Some(instance),
        message,
    });
}

/// Register every tier-2 analytic-surface handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&SPHERICAL_SURFACE_HANDLER);
    dispatch.register(&TOROIDAL_SURFACE_HANDLER);
    dispatch.register(&CONICAL_SURFACE_HANDLER);
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::{
        context::{ImportContext, ResolutionCaches, UnitScale},
        diagnostics::ImportReport,
        dispatch::EntityDispatch,
        parser::parse_step,
        registry::EntityRegistry,
    };
    use geometry_engine::primitives::topology_builder::BRepModel;

    fn wrap(body: &str) -> String {
        format!(
            "ISO-10303-21;\n\
             HEADER;\n\
             FILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));\n\
             ENDSEC;\n\
             DATA;\n\
             {body}\n\
             ENDSEC;\n\
             END-ISO-10303-21;\n"
        )
    }

    fn run(body: &str) -> (BRepModel, ImportReport, ResolutionCaches) {
        let src = wrap(body);
        let ex = parse_step(&src, "test").expect("parse");
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        super::super::super::tier1::register(&mut dispatch);
        super::super::register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        ctx.unit = UnitScale::default();
        let _ = dispatch.run_all(&reg, &mut ctx);
        let caches = std::mem::take(&mut ctx.caches);
        (model, report, caches)
    }

    /// Standard `AXIS2_PLACEMENT_3D` at the world origin with +Z up
    /// and +X as reference direction. Reusable across the three
    /// analytic-surface tests below.
    const WORLD_PLACEMENT: &str = "\
        #1 = CARTESIAN_POINT('',(0.0,0.0,0.0));\n\
        #2 = DIRECTION('',(0.0,0.0,1.0));\n\
        #3 = DIRECTION('',(1.0,0.0,0.0));\n\
        #4 = AXIS2_PLACEMENT_3D('',#1,#2,#3);\n";

    // ---------- SPHERICAL_SURFACE ----------

    #[test]
    fn spherical_surface_resolves_into_caches() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = SPHERICAL_SURFACE('s',#4,5.0);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(
            caches.surfaces.contains_key(&10),
            "SPHERICAL_SURFACE #10 should appear in caches.surfaces; report: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn spherical_surface_rejects_non_positive_radius() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = SPHERICAL_SURFACE('s',#4,0.0);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(!caches.surfaces.contains_key(&10));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.entity == "SPHERICAL_SURFACE"
                    && w.message.contains("non-positive radius")),
            "expected non-positive-radius warning; got: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn spherical_surface_rejects_wrong_arity() {
        let body = "#10 = SPHERICAL_SURFACE('s');\n";
        let (_model, report, caches) = run(body);
        assert!(!caches.surfaces.contains_key(&10));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.entity == "SPHERICAL_SURFACE" && w.message.contains("1 fields")),
            "expected arity warning; got: {:#?}",
            report.warnings
        );
    }

    // ---------- TOROIDAL_SURFACE ----------

    #[test]
    fn toroidal_surface_resolves_into_caches() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = TOROIDAL_SURFACE('t',#4,10.0,2.0);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(
            caches.surfaces.contains_key(&10),
            "TOROIDAL_SURFACE #10 should appear in caches.surfaces; report: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn toroidal_surface_rejects_spindle_minor_ge_major() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = TOROIDAL_SURFACE('t',#4,2.0,5.0);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(!caches.surfaces.contains_key(&10));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.entity == "TOROIDAL_SURFACE"
                    && w.message.contains("spindle/horn torus")),
            "expected spindle-rejection warning; got: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn toroidal_surface_rejects_non_positive_radii() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = TOROIDAL_SURFACE('t',#4,0.0,2.0);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(!caches.surfaces.contains_key(&10));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.entity == "TOROIDAL_SURFACE"
                    && w.message.contains("non-positive radii")),
            "expected non-positive-radii warning; got: {:#?}",
            report.warnings
        );
    }

    // ---------- CONICAL_SURFACE ----------

    #[test]
    fn conical_surface_resolves_with_derived_apex() {
        // radius=4, semi_angle=π/4 → apex offset = 4/tan(π/4) = 4
        // along −axis from origin → apex z = −4.
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = CONICAL_SURFACE('c',#4,4.0,0.7853981633974483);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(
            caches.surfaces.contains_key(&10),
            "CONICAL_SURFACE #10 should appear in caches.surfaces; report: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn conical_surface_resolves_with_zero_radius_apex_at_origin() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = CONICAL_SURFACE('c',#4,0.0,0.5235987755982988);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(
            caches.surfaces.contains_key(&10),
            "CONICAL_SURFACE with radius=0 should still resolve; report: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn conical_surface_rejects_semi_angle_out_of_range() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = CONICAL_SURFACE('c',#4,4.0,1.5707963267948966);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(!caches.surfaces.contains_key(&10));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.entity == "CONICAL_SURFACE" && w.message.contains("semi_angle")),
            "expected semi_angle warning; got: {:#?}",
            report.warnings
        );
    }

    #[test]
    fn conical_surface_rejects_negative_radius() {
        let body = format!(
            "{WORLD_PLACEMENT}\
             #10 = CONICAL_SURFACE('c',#4,-1.0,0.5);\n"
        );
        let (_model, report, caches) = run(&body);
        assert!(!caches.surfaces.contains_key(&10));
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.entity == "CONICAL_SURFACE" && w.message.contains("negative radius")),
            "expected negative-radius warning; got: {:#?}",
            report.warnings
        );
    }
}
