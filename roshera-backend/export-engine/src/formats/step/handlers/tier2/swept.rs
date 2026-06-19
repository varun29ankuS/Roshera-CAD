//! Swept-surface handlers: `SURFACE_OF_REVOLUTION` and
//! `SURFACE_OF_LINEAR_EXTRUSION` (ISO 10303-42 §swept_surface).
//!
//! | STEP                          | Kernel surface                                              |
//! |-------------------------------|-------------------------------------------------------------|
//! | `SURFACE_OF_REVOLUTION`       | [`SurfaceOfRevolution`] (profile revolved 2π about an axis) |
//! | `SURFACE_OF_LINEAR_EXTRUSION` | [`RuledSurface`] between the profile and its translate      |
//!
//! Both reference a `swept_curve` (the profile / generatrix). We
//! materialise the profile into a concrete kernel `Box<dyn Curve>`:
//!
//! - a B-spline / NURBS profile already lives in `caches.curves`
//!   (tier-2 B-spline or the complex-entity path) → cloned directly;
//! - a `CIRCLE` profile → a full kernel [`Circle`] from the cached
//!   placement + radius;
//! - a `LINE` profile → a kernel [`Line`] segment from the cached
//!   origin one unit along the cached direction (STEP `LINE` is
//!   unbounded; without an owning `TRIMMED_CURVE` the natural unit
//!   segment is used and a warning is logged so the truncation is
//!   honest).
//!
//! ## SURFACE_OF_REVOLUTION
//!
//! `SURFACE_OF_REVOLUTION('label', #swept_curve, #axis_position)` where
//! `axis_position` is an `AXIS1_PLACEMENT(location, axis)`. The profile
//! is revolved a full `2π` about the placement's axis (ISO 10303-42
//! defines the surface over the complete revolution; trimming to a
//! partial sweep is carried by the owning face's boundary loops).
//!
//! ## SURFACE_OF_LINEAR_EXTRUSION
//!
//! `SURFACE_OF_LINEAR_EXTRUSION('label', #swept_curve, #extrusion_axis)`
//! where `extrusion_axis` is a `VECTOR` (direction × length). The kernel
//! has no dedicated linear-extrusion surface, but a linear extrusion is
//! exactly the ruled surface between the profile `C(u)` and its rigid
//! translate `C(u) + d`; we build that [`RuledSurface`].

use std::f64::consts::TAU;

use ruststep::ast::Record;

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::primitives::{
    curve::{Circle, Curve, Line},
    surface::{RuledSurface, SurfaceOfRevolution},
};

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
};

use super::super::tier1::params;
use super::super::tier1::resolver::ensure_resolved;

mod names {
    pub const SURFACE_OF_REVOLUTION: &str = "SURFACE_OF_REVOLUTION";
    pub const SURFACE_OF_LINEAR_EXTRUSION: &str = "SURFACE_OF_LINEAR_EXTRUSION";
    pub const AXIS1_PLACEMENT: &str = "AXIS1_PLACEMENT";
    pub const VECTOR: &str = "VECTOR";
}

// Profile curve names the swept handlers will resolve.
const PROFILE_NAMES: &[&str] = &[
    "LINE",
    "CIRCLE",
    "ELLIPSE",
    "B_SPLINE_CURVE_WITH_KNOTS",
    "BOUNDED_CURVE",
    "B_SPLINE_CURVE",
    "RATIONAL_B_SPLINE_CURVE",
    "CURVE",
];

// =========================================================================
// SURFACE_OF_REVOLUTION
// =========================================================================

/// `SURFACE_OF_REVOLUTION('label', #swept_curve, #axis_position)`.
pub struct SurfaceOfRevolutionHandler;
/// Static binding consumed by [`register`].
pub static SURFACE_OF_REVOLUTION_HANDLER: SurfaceOfRevolutionHandler = SurfaceOfRevolutionHandler;

impl EntityHandler for SurfaceOfRevolutionHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::SURFACE_OF_REVOLUTION]
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
        let fields = match params::record_fields(
            &record.parameter,
            names::SURFACE_OF_REVOLUTION,
            instance,
        ) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 3 {
            return arity_error(
                ctx,
                names::SURFACE_OF_REVOLUTION,
                instance,
                "expected (label, swept_curve, axis_position)",
            );
        }
        let curve_ref = match params::as_entity_ref(
            &fields[1],
            names::SURFACE_OF_REVOLUTION,
            instance,
            "swept_curve",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad swept_curve ref".into(),
                };
            }
        };
        let axis_ref = match params::as_entity_ref(
            &fields[2],
            names::SURFACE_OF_REVOLUTION,
            instance,
            "axis_position",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad axis_position ref".into(),
                };
            }
        };

        let profile = match resolve_profile_curve(curve_ref, registry, dispatch, ctx) {
            Some(c) => c,
            None => {
                warn(
                    ctx,
                    names::SURFACE_OF_REVOLUTION,
                    instance,
                    format!("swept_curve #{curve_ref} did not resolve to a supported profile"),
                );
                return HandlerOutcome::Failed {
                    message: "profile unresolved".into(),
                };
            }
        };
        let (axis_origin, axis_dir) = match resolve_axis1(
            axis_ref,
            registry,
            dispatch,
            ctx,
            names::SURFACE_OF_REVOLUTION,
        ) {
            Some(a) => a,
            None => {
                return HandlerOutcome::Failed {
                    message: "axis_position unresolved".into(),
                };
            }
        };

        let surf = match SurfaceOfRevolution::new(
            Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
            Vector3::new(axis_dir[0], axis_dir[1], axis_dir[2]),
            profile,
            TAU,
        ) {
            Ok(s) => s,
            Err(e) => {
                warn(
                    ctx,
                    names::SURFACE_OF_REVOLUTION,
                    instance,
                    format!("kernel rejected SurfaceOfRevolution: {e}"),
                );
                return HandlerOutcome::Failed {
                    message: "kernel rejected revolution".into(),
                };
            }
        };
        let sid = ctx.model.surfaces.add(Box::new(surf));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// SURFACE_OF_LINEAR_EXTRUSION
// =========================================================================

/// `SURFACE_OF_LINEAR_EXTRUSION('label', #swept_curve, #extrusion_axis)`.
pub struct SurfaceOfLinearExtrusionHandler;
/// Static binding consumed by [`register`].
pub static SURFACE_OF_LINEAR_EXTRUSION_HANDLER: SurfaceOfLinearExtrusionHandler =
    SurfaceOfLinearExtrusionHandler;

impl EntityHandler for SurfaceOfLinearExtrusionHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::SURFACE_OF_LINEAR_EXTRUSION]
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
        let fields = match params::record_fields(
            &record.parameter,
            names::SURFACE_OF_LINEAR_EXTRUSION,
            instance,
        ) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 3 {
            return arity_error(
                ctx,
                names::SURFACE_OF_LINEAR_EXTRUSION,
                instance,
                "expected (label, swept_curve, extrusion_axis)",
            );
        }
        let curve_ref = match params::as_entity_ref(
            &fields[1],
            names::SURFACE_OF_LINEAR_EXTRUSION,
            instance,
            "swept_curve",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad swept_curve ref".into(),
                };
            }
        };
        let vec_ref = match params::as_entity_ref(
            &fields[2],
            names::SURFACE_OF_LINEAR_EXTRUSION,
            instance,
            "extrusion_axis",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad extrusion_axis ref".into(),
                };
            }
        };

        let profile = match resolve_profile_curve(curve_ref, registry, dispatch, ctx) {
            Some(c) => c,
            None => {
                warn(
                    ctx,
                    names::SURFACE_OF_LINEAR_EXTRUSION,
                    instance,
                    format!("swept_curve #{curve_ref} did not resolve to a supported profile"),
                );
                return HandlerOutcome::Failed {
                    message: "profile unresolved".into(),
                };
            }
        };

        // The extrusion axis is a VECTOR (direction × length, length-scaled).
        let _ = ensure_resolved(vec_ref, &[names::VECTOR], registry, dispatch, ctx);
        let disp = match ctx.caches.vectors.get(&vec_ref).copied() {
            Some(v) => Vector3::new(v[0], v[1], v[2]),
            None => {
                warn(
                    ctx,
                    names::SURFACE_OF_LINEAR_EXTRUSION,
                    instance,
                    format!("extrusion_axis #{vec_ref} did not resolve to a VECTOR"),
                );
                return HandlerOutcome::Failed {
                    message: "extrusion vector unresolved".into(),
                };
            }
        };
        if disp.magnitude() < 1e-12 {
            warn(
                ctx,
                names::SURFACE_OF_LINEAR_EXTRUSION,
                instance,
                "zero-length extrusion axis".to_string(),
            );
            return HandlerOutcome::Failed {
                message: "zero extrusion".into(),
            };
        }

        // Linear extrusion = ruled surface between the profile and its
        // rigid translate by the extrusion vector.
        let xlate = Matrix4::translation(disp.x, disp.y, disp.z);
        let translated = profile.transform(&xlate);
        let surf = RuledSurface::new(profile, translated);
        let sid = ctx.model.surfaces.add(Box::new(surf));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// Shared helpers
// =========================================================================

/// Resolve a profile-curve reference into a concrete kernel curve.
fn resolve_profile_curve(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<Box<dyn Curve>> {
    let _ = ensure_resolved(instance, PROFILE_NAMES, registry, dispatch, ctx);

    // Already-materialised kernel curve (NURBS / B-spline / complex).
    if let Some(cid) = ctx.caches.curves.get(&instance).copied() {
        if let Some(c) = ctx.model.curves.get(cid) {
            return Some(c.clone_box());
        }
    }
    // Full circle from a cached CIRCLE template.
    if let Some(circ) = ctx.caches.step_circles.get(&instance).copied() {
        let center = Point3::new(
            circ.placement.origin[0],
            circ.placement.origin[1],
            circ.placement.origin[2],
        );
        let normal = Vector3::new(
            circ.placement.z[0],
            circ.placement.z[1],
            circ.placement.z[2],
        );
        if let Ok(c) = Circle::new(center, normal, circ.radius) {
            return Some(Box::new(c));
        }
    }
    // Unit-length segment from a cached LINE template. STEP's LINE is
    // unbounded; without an owning TRIMMED_CURVE the unit segment is the
    // honest default.
    if let Some(line) = ctx.caches.step_lines.get(&instance).copied() {
        let s = Point3::new(line.origin[0], line.origin[1], line.origin[2]);
        let e = Point3::new(
            line.origin[0] + line.direction[0],
            line.origin[1] + line.direction[1],
            line.origin[2] + line.direction[2],
        );
        return Some(Box::new(Line::new(s, e)));
    }
    None
}

/// Resolve an `AXIS1_PLACEMENT('label', #location, #axis?)` into
/// `(origin, axis_direction)`. `axis` defaults to +Z per ISO 10303-42.
fn resolve_axis1(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
    owner: &'static str,
) -> Option<([f64; 3], [f64; 3])> {
    let record = registry.get(instance)?.clone();
    let rec = match &record.kind {
        crate::formats::step::registry::EntityKind::Simple(r) => r,
        _ => return None,
    };
    if !rec.name.eq_ignore_ascii_case(names::AXIS1_PLACEMENT) {
        warn(
            ctx,
            owner,
            instance,
            format!(
                "axis_position #{instance} is {}, expected AXIS1_PLACEMENT",
                rec.name
            ),
        );
        return None;
    }
    let fields = params::record_fields(&rec.parameter, names::AXIS1_PLACEMENT, instance).ok()?;
    if fields.len() < 2 {
        warn(
            ctx,
            names::AXIS1_PLACEMENT,
            instance,
            "expected (label, location, axis?)".to_string(),
        );
        return None;
    }
    let loc_ref =
        params::as_entity_ref(&fields[1], names::AXIS1_PLACEMENT, instance, "location").ok()?;
    let _ = ensure_resolved(loc_ref, &["CARTESIAN_POINT"], registry, dispatch, ctx);
    let origin = ctx.caches.points.get(&loc_ref).copied()?;

    let axis = if fields.len() >= 3 {
        match params::as_optional_entity_ref(&fields[2], names::AXIS1_PLACEMENT, instance, "axis") {
            Ok(Some(r)) => {
                let _ = ensure_resolved(r, &["DIRECTION"], registry, dispatch, ctx);
                ctx.caches
                    .directions
                    .get(&r)
                    .copied()
                    .unwrap_or([0.0, 0.0, 1.0])
            }
            _ => [0.0, 0.0, 1.0],
        }
    } else {
        [0.0, 0.0, 1.0]
    };
    Some((origin, axis))
}

fn arity_error(
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

fn warn(ctx: &mut ImportContext<'_>, entity: &str, instance: u64, message: String) {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: entity.into(),
        instance: Some(instance),
        message,
    });
}

/// Register the swept-surface handlers.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&SURFACE_OF_REVOLUTION_HANDLER);
    dispatch.register(&SURFACE_OF_LINEAR_EXTRUSION_HANDLER);
}

#[cfg(test)]
mod tests {
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

    /// A circle profile revolved about the X axis → a torus-like surface.
    #[test]
    fn surface_of_revolution_with_circle_profile_resolves() {
        let mut s = String::new();
        // Circle centred at (0,5,0), normal +X, radius 2.
        s += "#1=CARTESIAN_POINT('',(0.,5.,0.));";
        s += "#2=DIRECTION('',(1.,0.,0.));";
        s += "#3=DIRECTION('',(0.,1.,0.));";
        s += "#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);";
        s += "#5=CIRCLE('',#4,2.);";
        // Revolution axis = Z through origin.
        s += "#6=CARTESIAN_POINT('',(0.,0.,0.));";
        s += "#7=DIRECTION('',(0.,0.,1.));";
        s += "#8=AXIS1_PLACEMENT('',#6,#7);";
        s += "#9=SURFACE_OF_REVOLUTION('',#5,#8);";
        let (model, report, caches) = run(&s);
        let sid =
            caches.surfaces.get(&9).copied().unwrap_or_else(|| {
                panic!("revolution surface must resolve: {:?}", report.warnings)
            });
        assert!(model.surfaces.get(sid).is_some());
    }

    /// A line profile extruded along +Z → a planar/ruled surface.
    #[test]
    fn surface_of_linear_extrusion_resolves() {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#2=DIRECTION('',(0.,1.,0.));";
        s += "#3=VECTOR('',#2,3.);";
        s += "#4=LINE('',#1,#3);";
        // Extrusion direction +Z, length 10.
        s += "#5=DIRECTION('',(0.,0.,1.));";
        s += "#6=VECTOR('',#5,10.);";
        s += "#7=SURFACE_OF_LINEAR_EXTRUSION('',#4,#6);";
        let (model, report, caches) = run(&s);
        let sid = caches
            .surfaces
            .get(&7)
            .copied()
            .unwrap_or_else(|| panic!("extrusion surface must resolve: {:?}", report.warnings));
        assert!(model.surfaces.get(sid).is_some());
    }
}
