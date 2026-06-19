//! Rational B-spline curves and surfaces delivered as STEP **complex
//! entities** (and the rarer simple-form `RATIONAL_B_SPLINE_*`).
//!
//! ## Why a separate path
//!
//! ISO 10303-42 has no single `RATIONAL_B_SPLINE_CURVE` entity that
//! carries everything: a rational curve is the AND-combination of
//! several partial types. Real exporters (OpenCASCADE, NX, SolidWorks,
//! and the Roshera writer once spec-conformant) emit it as a STEP
//! *complex* instance — `ruststep` hands us [`EntityKind::Complex`], a
//! `Vec<Record>` of the constituent partial records:
//!
//! ```text
//! #42 = ( BOUNDED_CURVE()
//!         B_SPLINE_CURVE(3,(#1,#2,#3,#4),.UNSPECIFIED.,.F.,.F.)
//!         B_SPLINE_CURVE_WITH_KNOTS((4,4),(0.,1.),.UNSPECIFIED.)
//!         CURVE()
//!         GEOMETRIC_REPRESENTATION_ITEM()
//!         RATIONAL_B_SPLINE_CURVE((1.,0.7,0.7,1.))
//!         REPRESENTATION_ITEM('') );
//! ```
//!
//! The fields are scattered across the partials:
//! - `degree` + `control_points` live on `B_SPLINE_CURVE`,
//! - `knot_multiplicities` + `knots` live on `B_SPLINE_CURVE_WITH_KNOTS`,
//! - `weights` live on `RATIONAL_B_SPLINE_CURVE`.
//!
//! We gather them across the constituents, expand the knot vector, and
//! call the rational kernel constructor
//! [`NurbsCurve::new`]`(degree, control_points, weights, knots)` /
//! [`NurbsSurface::new`]. The non-rational complex form (same record
//! list minus `RATIONAL_B_SPLINE_*`, weights defaulting to 1) is also
//! accepted — some exporters wrap even a non-rational spline in a
//! complex record for `BOUNDED_*` supertyping.
//!
//! ## Dispatch entry
//!
//! The dispatcher and the lazy resolver call [`try_build_complex`] for
//! every `EntityKind::Complex` instance during the Geometry phase. It
//! returns `true` only when it recognised and materialised a curve or
//! surface; anything else returns `false` so the caller logs the entity
//! as Unsupported exactly as before.

use ruststep::ast::{Parameter, Record};

use geometry_engine::math::{nurbs::NurbsSurface as MathNurbsSurface, Point3};
use geometry_engine::primitives::{curve::NurbsCurve, surface::GeneralNurbsSurface};

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::EntityDispatch,
    registry::EntityRegistry,
};

use super::super::tier1::params;
use super::bspline::{expand_knot_vector, parse_integer_list, parse_real_list, resolve_point};

/// Try to materialise a complex (or simple rational) B-spline curve or
/// surface from the constituent records of instance `instance`.
///
/// `records` is the constituent list for an `EntityKind::Complex`
/// instance (or a single-element slice wrapping a simple
/// `RATIONAL_B_SPLINE_*` record). Returns `true` if a kernel
/// curve/surface was registered into `ctx.caches`.
pub fn try_build_complex(
    instance: u64,
    records: &[Record],
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> bool {
    let has = |name: &str| records.iter().any(|r| r.name.eq_ignore_ascii_case(name));

    if has("B_SPLINE_SURFACE") || has("B_SPLINE_SURFACE_WITH_KNOTS") {
        return build_surface(instance, records, registry, dispatch, ctx);
    }
    if has("B_SPLINE_CURVE") || has("B_SPLINE_CURVE_WITH_KNOTS") {
        return build_curve(instance, records, registry, dispatch, ctx);
    }
    false
}

/// Find the first constituent whose name matches `name` and return its
/// field slice.
fn fields_of<'a>(records: &'a [Record], name: &str) -> Option<&'a [Parameter]> {
    let rec = records.iter().find(|r| r.name.eq_ignore_ascii_case(name))?;
    match &rec.parameter {
        Parameter::List(items) => Some(items.as_slice()),
        _ => None,
    }
}

/// Extract the `(weights)` list from a `RATIONAL_B_SPLINE_CURVE`
/// constituent's single field.
fn rational_curve_weights(
    records: &[Record],
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> Option<Vec<f64>> {
    let f = fields_of(records, "RATIONAL_B_SPLINE_CURVE")?;
    // The partial type carries exactly one field: the weights list.
    let weights_param = f.first()?;
    parse_real_list(
        weights_param,
        "RATIONAL_B_SPLINE_CURVE",
        instance,
        "weights_data",
        ctx,
    )
}

fn build_curve(
    instance: u64,
    records: &[Record],
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> bool {
    const ENTITY: &str = "RATIONAL_B_SPLINE_CURVE";

    // degree + control_points come from B_SPLINE_CURVE:
    //   B_SPLINE_CURVE(degree, (cps), form, closed, self_intersect)
    let bsc = match fields_of(records, "B_SPLINE_CURVE") {
        Some(f) if f.len() >= 2 => f,
        _ => {
            warn(ctx, ENTITY, instance, "missing B_SPLINE_CURVE constituent");
            return false;
        }
    };
    let degree = match params::as_integer(&bsc[0], "B_SPLINE_CURVE", instance, "degree") {
        Ok(d) if d > 0 => d as usize,
        _ => {
            warn(ctx, ENTITY, instance, "non-positive / missing degree");
            return false;
        }
    };
    let cp_refs = match params::as_entity_ref_list(
        &bsc[1],
        "B_SPLINE_CURVE",
        instance,
        "control_points_list",
    ) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            warn(ctx, ENTITY, instance, "empty / malformed control_points");
            return false;
        }
    };

    // knot_multiplicities + knots come from B_SPLINE_CURVE_WITH_KNOTS:
    //   B_SPLINE_CURVE_WITH_KNOTS((mults), (knots), knot_spec)
    let bsck = match fields_of(records, "B_SPLINE_CURVE_WITH_KNOTS") {
        Some(f) if f.len() >= 2 => f,
        _ => {
            warn(
                ctx,
                ENTITY,
                instance,
                "missing B_SPLINE_CURVE_WITH_KNOTS constituent",
            );
            return false;
        }
    };
    let mults = match parse_integer_list(
        &bsck[0],
        "B_SPLINE_CURVE_WITH_KNOTS",
        instance,
        "knot_multiplicities",
        ctx,
    ) {
        Some(v) => v,
        None => return false,
    };
    let distinct = match parse_real_list(
        &bsck[1],
        "B_SPLINE_CURVE_WITH_KNOTS",
        instance,
        "knots",
        ctx,
    ) {
        Some(v) => v,
        None => return false,
    };

    // Control points (length-scaled).
    let mut control_points: Vec<Point3> = Vec::with_capacity(cp_refs.len());
    for cp in &cp_refs {
        match resolve_point(*cp, registry, dispatch, ctx) {
            Some(p) => control_points.push(Point3::new(p[0], p[1], p[2])),
            None => {
                warn(
                    ctx,
                    ENTITY,
                    instance,
                    &format!("control point #{cp} missing"),
                );
                return false;
            }
        }
    }

    // Weights: present on the rational partial; default to 1 for a
    // non-rational complex spline.
    let weights = match rational_curve_weights(records, instance, ctx) {
        Some(w) => w,
        None => vec![1.0; control_points.len()],
    };
    if weights.len() != control_points.len() {
        warn(
            ctx,
            ENTITY,
            instance,
            &format!(
                "weight count {} ≠ control-point count {}",
                weights.len(),
                control_points.len()
            ),
        );
        return false;
    }

    let knots = match expand_knot_vector(&distinct, &mults) {
        Ok(k) => k,
        Err(e) => {
            warn(ctx, ENTITY, instance, &format!("knot expansion: {e}"));
            return false;
        }
    };
    let expected = control_points.len() + degree + 1;
    if knots.len() != expected {
        warn(
            ctx,
            ENTITY,
            instance,
            &format!("|knots|={} ≠ n+p+1={expected}", knots.len()),
        );
        return false;
    }

    let curve = match NurbsCurve::new(degree, control_points, weights, knots) {
        Ok(c) => c,
        Err(e) => {
            warn(
                ctx,
                ENTITY,
                instance,
                &format!("kernel rejected NurbsCurve: {e}"),
            );
            return false;
        }
    };
    let cid = ctx.model.curves.add(Box::new(curve));
    ctx.caches.curves.insert(instance, cid);
    true
}

fn build_surface(
    instance: u64,
    records: &[Record],
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> bool {
    const ENTITY: &str = "RATIONAL_B_SPLINE_SURFACE";

    // B_SPLINE_SURFACE(u_degree, v_degree, ((cp grid)), form, u_closed,
    //                  v_closed, self_intersect)
    let bss = match fields_of(records, "B_SPLINE_SURFACE") {
        Some(f) if f.len() >= 3 => f,
        _ => {
            warn(
                ctx,
                ENTITY,
                instance,
                "missing B_SPLINE_SURFACE constituent",
            );
            return false;
        }
    };
    let u_degree = match params::as_integer(&bss[0], "B_SPLINE_SURFACE", instance, "u_degree") {
        Ok(d) if d > 0 => d as usize,
        _ => {
            warn(ctx, ENTITY, instance, "bad u_degree");
            return false;
        }
    };
    let v_degree = match params::as_integer(&bss[1], "B_SPLINE_SURFACE", instance, "v_degree") {
        Ok(d) if d > 0 => d as usize,
        _ => {
            warn(ctx, ENTITY, instance, "bad v_degree");
            return false;
        }
    };
    let outer = match params::as_list(&bss[2], "B_SPLINE_SURFACE", instance, "control_points_list")
    {
        Ok(v) if !v.is_empty() => v,
        _ => {
            warn(ctx, ENTITY, instance, "empty control-point grid");
            return false;
        }
    };

    // Resolve the control-point grid (row-major).
    let mut grid: Vec<Vec<Point3>> = Vec::with_capacity(outer.len());
    let mut row_len: Option<usize> = None;
    for (i, row_param) in outer.iter().enumerate() {
        let row_refs = match params::as_entity_ref_list(
            row_param,
            "B_SPLINE_SURFACE",
            instance,
            "control_points_list[..]",
        ) {
            Ok(v) => v,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return false;
            }
        };
        match row_len {
            Some(expected) if row_refs.len() != expected => {
                warn(
                    ctx,
                    ENTITY,
                    instance,
                    &format!("non-rectangular grid at row {i}"),
                );
                return false;
            }
            None => row_len = Some(row_refs.len()),
            _ => {}
        }
        let mut row_pts = Vec::with_capacity(row_refs.len());
        for cp in &row_refs {
            match resolve_point(*cp, registry, dispatch, ctx) {
                Some(p) => row_pts.push(Point3::new(p[0], p[1], p[2])),
                None => {
                    warn(
                        ctx,
                        ENTITY,
                        instance,
                        &format!("control point #{cp} missing"),
                    );
                    return false;
                }
            }
        }
        grid.push(row_pts);
    }
    let n_u = grid.len();
    let n_v = row_len.unwrap_or(0);
    if n_v == 0 {
        warn(ctx, ENTITY, instance, "control-point rows empty");
        return false;
    }

    // B_SPLINE_SURFACE_WITH_KNOTS((u_mults),(v_mults),(u_knots),(v_knots),spec)
    let bssk = match fields_of(records, "B_SPLINE_SURFACE_WITH_KNOTS") {
        Some(f) if f.len() >= 4 => f,
        _ => {
            warn(
                ctx,
                ENTITY,
                instance,
                "missing B_SPLINE_SURFACE_WITH_KNOTS constituent",
            );
            return false;
        }
    };
    let u_mults = match parse_integer_list(&bssk[0], ENTITY, instance, "u_mults", ctx) {
        Some(v) => v,
        None => return false,
    };
    let v_mults = match parse_integer_list(&bssk[1], ENTITY, instance, "v_mults", ctx) {
        Some(v) => v,
        None => return false,
    };
    let u_distinct = match parse_real_list(&bssk[2], ENTITY, instance, "u_knots", ctx) {
        Some(v) => v,
        None => return false,
    };
    let v_distinct = match parse_real_list(&bssk[3], ENTITY, instance, "v_knots", ctx) {
        Some(v) => v,
        None => return false,
    };

    let u_knots = match expand_knot_vector(&u_distinct, &u_mults) {
        Ok(k) => k,
        Err(e) => {
            warn(ctx, ENTITY, instance, &format!("u knot expansion: {e}"));
            return false;
        }
    };
    let v_knots = match expand_knot_vector(&v_distinct, &v_mults) {
        Ok(k) => k,
        Err(e) => {
            warn(ctx, ENTITY, instance, &format!("v knot expansion: {e}"));
            return false;
        }
    };
    if u_knots.len() != n_u + u_degree + 1 || v_knots.len() != n_v + v_degree + 1 {
        warn(ctx, ENTITY, instance, "knot/control-count mismatch");
        return false;
    }

    // Weights grid from RATIONAL_B_SPLINE_SURFACE((row),(row),…), or
    // uniform 1 for the non-rational complex form.
    let weights = match rational_surface_weights(records, instance, n_u, n_v, ctx) {
        Some(w) => w,
        None => vec![vec![1.0; n_v]; n_u],
    };

    let math_surface =
        match MathNurbsSurface::new(grid, weights, u_knots, v_knots, u_degree, v_degree) {
            Ok(s) => s,
            Err(e) => {
                warn(
                    ctx,
                    ENTITY,
                    instance,
                    &format!("kernel rejected NurbsSurface: {e}"),
                );
                return false;
            }
        };
    let sid = ctx.model.surfaces.add(Box::new(GeneralNurbsSurface {
        nurbs: math_surface,
    }));
    ctx.caches.surfaces.insert(instance, sid);
    true
}

/// Extract the `((w),(w),…)` weight grid from a
/// `RATIONAL_B_SPLINE_SURFACE` constituent and validate its shape.
fn rational_surface_weights(
    records: &[Record],
    instance: u64,
    n_u: usize,
    n_v: usize,
    ctx: &mut ImportContext<'_>,
) -> Option<Vec<Vec<f64>>> {
    let f = fields_of(records, "RATIONAL_B_SPLINE_SURFACE")?;
    let outer = match params::as_list(
        f.first()?,
        "RATIONAL_B_SPLINE_SURFACE",
        instance,
        "weights_data",
    ) {
        Ok(v) => v,
        Err(e) => {
            ctx.report.push_warning(e.into_warning());
            return None;
        }
    };
    if outer.len() != n_u {
        warn(
            ctx,
            "RATIONAL_B_SPLINE_SURFACE",
            instance,
            "weight-grid row count mismatch",
        );
        return None;
    }
    let mut grid = Vec::with_capacity(n_u);
    for (i, row) in outer.iter().enumerate() {
        let w = parse_real_list(
            row,
            "RATIONAL_B_SPLINE_SURFACE",
            instance,
            &format!("weights_data[{i}]"),
            ctx,
        )?;
        if w.len() != n_v {
            warn(
                ctx,
                "RATIONAL_B_SPLINE_SURFACE",
                instance,
                "weight-grid column count mismatch",
            );
            return None;
        }
        grid.push(w);
    }
    Some(grid)
}

fn warn(ctx: &mut ImportContext<'_>, entity: &str, instance: u64, message: &str) {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: entity.into(),
        instance: Some(instance),
        message: message.to_string(),
    });
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

    /// A rational cubic Bézier (quarter-circle-style weights) delivered
    /// as the standard STEP complex entity.
    fn rational_complex_curve_body() -> String {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#2=CARTESIAN_POINT('',(1.,1.,0.));";
        s += "#3=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#10=( BOUNDED_CURVE() \
                B_SPLINE_CURVE(2,(#1,#2,#3),.UNSPECIFIED.,.F.,.F.) \
                B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) \
                CURVE() GEOMETRIC_REPRESENTATION_ITEM() \
                RATIONAL_B_SPLINE_CURVE((1.,0.70710678,1.)) \
                REPRESENTATION_ITEM('') );";
        s
    }

    #[test]
    fn rational_complex_curve_resolves() {
        let (model, report, caches) = run(&rational_complex_curve_body());
        let cid = caches
            .curves
            .get(&10)
            .copied()
            .expect("rational complex B-spline curve must allocate a kernel curve");
        assert!(model.curves.get(cid).is_some());
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.entity.contains("RATIONAL")),
            "no warnings on the success path: {:?}",
            report.warnings
        );
    }

    /// A rational bicubic patch as a complex entity (uniform weights).
    fn rational_complex_surface_body() -> String {
        let mut s = String::new();
        let mut idx = 1u32;
        for i in 0..3 {
            for j in 0..3 {
                s += &format!("#{idx}=CARTESIAN_POINT('',({i}.,{j}.,0.));");
                idx += 1;
            }
        }
        s += "#100=( BOUNDED_SURFACE() \
                B_SPLINE_SURFACE(2,2,((#1,#2,#3),(#4,#5,#6),(#7,#8,#9)),.UNSPECIFIED.,.F.,.F.,.F.) \
                B_SPLINE_SURFACE_WITH_KNOTS((3,3),(3,3),(0.,1.),(0.,1.),.UNSPECIFIED.) \
                GEOMETRIC_REPRESENTATION_ITEM() REPRESENTATION_ITEM('') SURFACE() \
                RATIONAL_B_SPLINE_SURFACE(((1.,0.8,1.),(0.8,0.6,0.8),(1.,0.8,1.))) );";
        s
    }

    #[test]
    fn rational_complex_surface_resolves() {
        let (model, report, caches) = run(&rational_complex_surface_body());
        let sid = caches
            .surfaces
            .get(&100)
            .copied()
            .expect("rational complex B-spline surface must allocate a kernel surface");
        assert!(model.surfaces.get(sid).is_some());
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.entity.contains("RATIONAL")),
            "no warnings on the success path: {:?}",
            report.warnings
        );
    }

    #[test]
    fn non_rational_complex_curve_defaults_unit_weights() {
        // Same shape minus the RATIONAL partial — weights default to 1.
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        s += "#2=CARTESIAN_POINT('',(1.,1.,0.));";
        s += "#3=CARTESIAN_POINT('',(2.,0.,0.));";
        s += "#10=( BOUNDED_CURVE() \
                B_SPLINE_CURVE(2,(#1,#2,#3),.UNSPECIFIED.,.F.,.F.) \
                B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) \
                CURVE() GEOMETRIC_REPRESENTATION_ITEM() REPRESENTATION_ITEM('') );";
        let (_m, _r, caches) = run(&s);
        assert!(caches.curves.contains_key(&10));
    }
}
