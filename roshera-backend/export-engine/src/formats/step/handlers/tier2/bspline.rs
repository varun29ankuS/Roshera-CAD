//! B-spline curve and surface handlers (non-rational, simple form).
//!
//! | STEP                              | Kernel call                                                  | Cache              |
//! |-----------------------------------|--------------------------------------------------------------|--------------------|
//! | `B_SPLINE_CURVE_WITH_KNOTS`       | [`NurbsCurve::from_bspline`]                                 | `caches.curves`    |
//! | `B_SPLINE_SURFACE_WITH_KNOTS`     | [`NurbsSurface::new`] wrapped in [`GeneralNurbsSurface`]     | `caches.surfaces`  |
//!
//! ## Knot vector expansion
//!
//! STEP stores B-spline knots as a pair of parallel lists:
//!
//! - `knots = [k0, k1, …, kp]` — distinct parameter values,
//! - `knot_multiplicities = [m0, m1, …, mp]` — how many times each
//!   `ki` is repeated in the canonical flat knot vector.
//!
//! The kernel constructors take the flat vector
//! `[k0×m0, k1×m1, …, kp×mp]` whose total length must equal
//! `n + degree + 1`, where `n` is the control-point count. We expand
//! by replication; multiplicities `≤ 0` are rejected.
//!
//! ## Rational variants
//!
//! `RATIONAL_B_SPLINE_CURVE` / `RATIONAL_B_SPLINE_SURFACE` arrive as
//! STEP complex entities (one `EntityKind::Complex` containing a
//! list of sub-records). They are not handled in this slice — the
//! simple, non-rational form covers the bulk of demo files. Rational
//! variants surface through the unsupported path until a follow-up
//! adds complex-entity dispatch.

use ruststep::ast::Record;

use geometry_engine::math::{nurbs::NurbsSurface as MathNurbsSurface, Point3};
use geometry_engine::primitives::{curve::NurbsCurve, surface::GeneralNurbsSurface};

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
};

use super::super::tier1::params;
use super::super::tier1::resolver::ensure_resolved;

mod names {
    pub const B_SPLINE_CURVE_WITH_KNOTS: &str = "B_SPLINE_CURVE_WITH_KNOTS";
    pub const B_SPLINE_SURFACE_WITH_KNOTS: &str = "B_SPLINE_SURFACE_WITH_KNOTS";
    pub const CARTESIAN_POINT: &str = "CARTESIAN_POINT";
}

// =========================================================================
// B_SPLINE_CURVE_WITH_KNOTS
// =========================================================================

/// `B_SPLINE_CURVE_WITH_KNOTS('label', degree, (#cp1,…), curve_form,
/// closed_curve, self_intersect, knot_multiplicities, knots,
/// knot_spec)` — 9 fields. Length-scaled control points feed a
/// `from_bspline` (uniform weights) call.
pub struct BSplineCurveHandler;
/// Static binding consumed by [`register`].
pub static B_SPLINE_CURVE_HANDLER: BSplineCurveHandler = BSplineCurveHandler;

impl EntityHandler for BSplineCurveHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::B_SPLINE_CURVE_WITH_KNOTS]
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
            names::B_SPLINE_CURVE_WITH_KNOTS,
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
        // 9 fields. Some writers append a `knot_spec` enum at the end;
        // tolerate >=9 by indexing only the first nine.
        if fields.len() < 9 {
            return field_count_error(
                ctx,
                names::B_SPLINE_CURVE_WITH_KNOTS,
                instance,
                "expected ≥ 9 fields (label, degree, control_points, curve_form, closed, self_intersect, knot_mults, knots, knot_spec)",
                fields.len(),
            );
        }

        let degree = match params::as_integer(
            &fields[1],
            names::B_SPLINE_CURVE_WITH_KNOTS,
            instance,
            "degree",
        ) {
            Ok(d) if d > 0 => d as usize,
            Ok(d) => {
                push_warn(ctx, instance, format!("non-positive degree {d}"));
                return HandlerOutcome::Failed {
                    message: "non-positive degree".into(),
                };
            }
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad degree".into(),
                };
            }
        };

        let cp_refs = match params::as_entity_ref_list(
            &fields[2],
            names::B_SPLINE_CURVE_WITH_KNOTS,
            instance,
            "control_points_list",
        ) {
            Ok(v) => v,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad control_points_list".into(),
                };
            }
        };
        if cp_refs.is_empty() {
            push_warn(ctx, instance, "control_points_list is empty".to_string());
            return HandlerOutcome::Failed {
                message: "empty control points".into(),
            };
        }

        // Resolve every control point reference into a Point3 (length-scaled).
        let mut control_points: Vec<Point3> = Vec::with_capacity(cp_refs.len());
        for cp_ref in &cp_refs {
            match resolve_point(*cp_ref, registry, dispatch, ctx) {
                Some(p) => control_points.push(Point3::new(p[0], p[1], p[2])),
                None => {
                    push_warn(
                        ctx,
                        instance,
                        format!("control point #{cp_ref} did not resolve"),
                    );
                    return HandlerOutcome::Failed {
                        message: "control point missing".into(),
                    };
                }
            }
        }

        // Knot multiplicities and parameter values.
        let mults = match parse_integer_list(
            &fields[6],
            names::B_SPLINE_CURVE_WITH_KNOTS,
            instance,
            "knot_multiplicities",
            ctx,
        ) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "bad mults".into(),
                }
            }
        };
        let distinct_knots = match parse_real_list(
            &fields[7],
            names::B_SPLINE_CURVE_WITH_KNOTS,
            instance,
            "knots",
            ctx,
        ) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "bad knots".into(),
                }
            }
        };

        let knots = match expand_knot_vector(&distinct_knots, &mults) {
            Ok(k) => k,
            Err(e) => {
                push_warn(ctx, instance, format!("knot vector expansion: {e}"));
                return HandlerOutcome::Failed {
                    message: "knot expansion failed".into(),
                };
            }
        };

        // Validate flat-knot length against control-point count.
        let expected_knot_len = control_points.len() + degree + 1;
        if knots.len() != expected_knot_len {
            push_warn(
                ctx,
                instance,
                format!(
                    "expanded knot vector length {} ≠ n + p + 1 = {}",
                    knots.len(),
                    expected_knot_len
                ),
            );
            return HandlerOutcome::Failed {
                message: "knot/control count mismatch".into(),
            };
        }

        let curve = match NurbsCurve::from_bspline(degree, control_points, knots) {
            Ok(c) => c,
            Err(e) => {
                push_warn(ctx, instance, format!("kernel rejected NurbsCurve: {e}"));
                return HandlerOutcome::Failed {
                    message: "kernel rejected curve".into(),
                };
            }
        };
        let cid = ctx.model.curves.add(Box::new(curve));
        ctx.caches.curves.insert(instance, cid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// B_SPLINE_SURFACE_WITH_KNOTS
// =========================================================================

/// `B_SPLINE_SURFACE_WITH_KNOTS('label', u_degree, v_degree,
/// ((#cp,…),(…)), surface_form, u_closed, v_closed, self_intersect,
/// u_knot_mults, v_knot_mults, u_knots, v_knots, knot_spec)` — 13
/// fields. Wraps `math::nurbs::NurbsSurface` in `GeneralNurbsSurface`
/// and registers under `caches.surfaces`.
pub struct BSplineSurfaceHandler;
/// Static binding consumed by [`register`].
pub static B_SPLINE_SURFACE_HANDLER: BSplineSurfaceHandler = BSplineSurfaceHandler;

impl EntityHandler for BSplineSurfaceHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::B_SPLINE_SURFACE_WITH_KNOTS]
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
            names::B_SPLINE_SURFACE_WITH_KNOTS,
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
        if fields.len() < 13 {
            return field_count_error(
                ctx,
                names::B_SPLINE_SURFACE_WITH_KNOTS,
                instance,
                "expected ≥ 13 fields",
                fields.len(),
            );
        }

        let u_degree = match params::as_integer(
            &fields[1],
            names::B_SPLINE_SURFACE_WITH_KNOTS,
            instance,
            "u_degree",
        ) {
            Ok(d) if d > 0 => d as usize,
            Ok(d) => {
                push_warn_surface(ctx, instance, format!("non-positive u_degree {d}"));
                return HandlerOutcome::Failed {
                    message: "bad u_degree".into(),
                };
            }
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad u_degree".into(),
                };
            }
        };
        let v_degree = match params::as_integer(
            &fields[2],
            names::B_SPLINE_SURFACE_WITH_KNOTS,
            instance,
            "v_degree",
        ) {
            Ok(d) if d > 0 => d as usize,
            Ok(d) => {
                push_warn_surface(ctx, instance, format!("non-positive v_degree {d}"));
                return HandlerOutcome::Failed {
                    message: "bad v_degree".into(),
                };
            }
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad v_degree".into(),
                };
            }
        };

        // Control-point grid: list-of-lists of #N.
        let outer = match params::as_list(
            &fields[3],
            names::B_SPLINE_SURFACE_WITH_KNOTS,
            instance,
            "control_points_list",
        ) {
            Ok(v) => v,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad cp grid".into(),
                };
            }
        };
        if outer.is_empty() {
            push_warn_surface(ctx, instance, "control_points_list is empty".to_string());
            return HandlerOutcome::Failed {
                message: "empty cp grid".into(),
            };
        }
        let mut grid: Vec<Vec<Point3>> = Vec::with_capacity(outer.len());
        let mut row_len: Option<usize> = None;
        for (i, row_param) in outer.iter().enumerate() {
            let row_refs = match params::as_entity_ref_list(
                row_param,
                names::B_SPLINE_SURFACE_WITH_KNOTS,
                instance,
                "control_points_list[..]",
            ) {
                Ok(v) => v,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad cp row".into(),
                    };
                }
            };
            if let Some(expected) = row_len {
                if row_refs.len() != expected {
                    push_warn_surface(
                        ctx,
                        instance,
                        format!(
                            "control_points_list row {i} has {} entries, expected {expected}",
                            row_refs.len()
                        ),
                    );
                    return HandlerOutcome::Failed {
                        message: "non-rectangular cp grid".into(),
                    };
                }
            } else {
                row_len = Some(row_refs.len());
            }
            let mut row_pts: Vec<Point3> = Vec::with_capacity(row_refs.len());
            for cp_ref in &row_refs {
                match resolve_point(*cp_ref, registry, dispatch, ctx) {
                    Some(p) => row_pts.push(Point3::new(p[0], p[1], p[2])),
                    None => {
                        push_warn_surface(
                            ctx,
                            instance,
                            format!("control point #{cp_ref} did not resolve"),
                        );
                        return HandlerOutcome::Failed {
                            message: "cp missing".into(),
                        };
                    }
                }
            }
            grid.push(row_pts);
        }
        let n_u = grid.len();
        let n_v = row_len.unwrap_or(0);
        if n_v == 0 {
            push_warn_surface(ctx, instance, "control point rows are empty".to_string());
            return HandlerOutcome::Failed {
                message: "empty cp row".into(),
            };
        }
        // Uniform weights for the non-rational simple form.
        let weights = vec![vec![1.0; n_v]; n_u];

        // Knot vectors.
        let u_mults = match parse_integer_list(
            &fields[8],
            names::B_SPLINE_SURFACE_WITH_KNOTS,
            instance,
            "u_knot_multiplicities",
            ctx,
        ) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "bad u mults".into(),
                }
            }
        };
        let v_mults = match parse_integer_list(
            &fields[9],
            names::B_SPLINE_SURFACE_WITH_KNOTS,
            instance,
            "v_knot_multiplicities",
            ctx,
        ) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "bad v mults".into(),
                }
            }
        };
        let u_knots_distinct = match parse_real_list(
            &fields[10],
            names::B_SPLINE_SURFACE_WITH_KNOTS,
            instance,
            "u_knots",
            ctx,
        ) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "bad u knots".into(),
                }
            }
        };
        let v_knots_distinct = match parse_real_list(
            &fields[11],
            names::B_SPLINE_SURFACE_WITH_KNOTS,
            instance,
            "v_knots",
            ctx,
        ) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "bad v knots".into(),
                }
            }
        };

        let u_knots = match expand_knot_vector(&u_knots_distinct, &u_mults) {
            Ok(k) => k,
            Err(e) => {
                push_warn_surface(ctx, instance, format!("u knot expansion: {e}"));
                return HandlerOutcome::Failed {
                    message: "u knot expansion".into(),
                };
            }
        };
        let v_knots = match expand_knot_vector(&v_knots_distinct, &v_mults) {
            Ok(k) => k,
            Err(e) => {
                push_warn_surface(ctx, instance, format!("v knot expansion: {e}"));
                return HandlerOutcome::Failed {
                    message: "v knot expansion".into(),
                };
            }
        };
        let expected_u = n_u + u_degree + 1;
        let expected_v = n_v + v_degree + 1;
        if u_knots.len() != expected_u {
            push_warn_surface(
                ctx,
                instance,
                format!("|u_knots|={} ≠ n_u+p_u+1={}", u_knots.len(), expected_u),
            );
            return HandlerOutcome::Failed {
                message: "u knot count".into(),
            };
        }
        if v_knots.len() != expected_v {
            push_warn_surface(
                ctx,
                instance,
                format!("|v_knots|={} ≠ n_v+p_v+1={}", v_knots.len(), expected_v),
            );
            return HandlerOutcome::Failed {
                message: "v knot count".into(),
            };
        }

        let math_surface =
            match MathNurbsSurface::new(grid, weights, u_knots, v_knots, u_degree, v_degree) {
                Ok(s) => s,
                Err(e) => {
                    push_warn_surface(ctx, instance, format!("kernel rejected NurbsSurface: {e}"));
                    return HandlerOutcome::Failed {
                        message: "kernel rejected surface".into(),
                    };
                }
            };
        let wrapper = GeneralNurbsSurface {
            nurbs: math_surface,
        };
        let sid = ctx.model.surfaces.add(Box::new(wrapper));
        ctx.caches.surfaces.insert(instance, sid);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// Shared helpers
// =========================================================================

/// Force `instance` to resolve as a `CARTESIAN_POINT`, returning the
/// cached length-scaled position on success. Mirrors the helper
/// inside `tier1::geometry` so tier-2 handlers don't need to expose
/// it pub-crate.
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

fn parse_integer_list(
    param: &ruststep::ast::Parameter,
    entity: &str,
    instance: u64,
    path: &str,
    ctx: &mut ImportContext<'_>,
) -> Option<Vec<usize>> {
    let items = match params::as_list(param, entity, instance, path) {
        Ok(v) => v,
        Err(e) => {
            ctx.report.push_warning(e.into_warning());
            return None;
        }
    };
    let mut out: Vec<usize> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let path_i = format!("{path}[{i}]");
        let v = match params::as_integer(item, entity, instance, &path_i) {
            Ok(v) => v,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return None;
            }
        };
        if v <= 0 {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: entity.into(),
                instance: Some(instance),
                message: format!("non-positive multiplicity {v} at {path_i}"),
            });
            return None;
        }
        out.push(v as usize);
    }
    Some(out)
}

fn parse_real_list(
    param: &ruststep::ast::Parameter,
    entity: &str,
    instance: u64,
    path: &str,
    ctx: &mut ImportContext<'_>,
) -> Option<Vec<f64>> {
    let items = match params::as_list(param, entity, instance, path) {
        Ok(v) => v,
        Err(e) => {
            ctx.report.push_warning(e.into_warning());
            return None;
        }
    };
    let mut out: Vec<f64> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let path_i = format!("{path}[{i}]");
        match params::as_real(item, entity, instance, &path_i) {
            Ok(v) => out.push(v),
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return None;
            }
        }
    }
    Some(out)
}

/// Expand the STEP `(knots, multiplicities)` pair into the flat
/// vector the kernel expects. Rejects mismatched lengths, zero
/// multiplicities, and non-monotone `knots`.
fn expand_knot_vector(knots: &[f64], mults: &[usize]) -> Result<Vec<f64>, String> {
    if knots.len() != mults.len() {
        return Err(format!(
            "knots/multiplicities length mismatch ({} vs {})",
            knots.len(),
            mults.len()
        ));
    }
    if knots.is_empty() {
        return Err("knot vector is empty".to_string());
    }
    for w in knots.windows(2) {
        if w[1] < w[0] {
            return Err(format!("non-monotone knot sequence: {} > {}", w[0], w[1]));
        }
    }
    let total: usize = mults.iter().sum();
    let mut flat: Vec<f64> = Vec::with_capacity(total);
    for (k, m) in knots.iter().zip(mults.iter()) {
        for _ in 0..*m {
            flat.push(*k);
        }
    }
    Ok(flat)
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

fn push_warn(ctx: &mut ImportContext<'_>, instance: u64, message: String) {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: names::B_SPLINE_CURVE_WITH_KNOTS.into(),
        instance: Some(instance),
        message,
    });
}

fn push_warn_surface(ctx: &mut ImportContext<'_>, instance: u64, message: String) {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: names::B_SPLINE_SURFACE_WITH_KNOTS.into(),
        instance: Some(instance),
        message,
    });
}

/// Register every tier-2 B-spline handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&B_SPLINE_CURVE_HANDLER);
    dispatch.register(&B_SPLINE_SURFACE_HANDLER);
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

    // ---------- expand_knot_vector unit tests ----------

    #[test]
    fn expand_clamped_cubic_uniform() {
        // Cubic clamped at [0,1] with 4 distinct knots; usual control
        // count would be 4 with mult vector [4,1,1,4] for example —
        // but the simplest cubic Bézier has knots [0,0,0,0,1,1,1,1].
        let knots = [0.0, 1.0];
        let mults = [4usize, 4];
        let flat = expand_knot_vector(&knots, &mults).expect("expansion");
        assert_eq!(flat, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn expand_rejects_length_mismatch() {
        let err = expand_knot_vector(&[0.0, 1.0], &[1usize]).unwrap_err();
        assert!(err.contains("length mismatch"));
    }

    #[test]
    fn expand_rejects_non_monotone() {
        let err = expand_knot_vector(&[1.0, 0.0], &[1, 1]).unwrap_err();
        assert!(err.contains("non-monotone"));
    }

    #[test]
    fn expand_rejects_empty() {
        let err = expand_knot_vector(&[], &[]).unwrap_err();
        assert!(err.contains("empty"));
    }

    // ---------- B_SPLINE_CURVE_WITH_KNOTS handler ----------

    /// 4 control points (a cubic Bézier) → degree 3 → flat knot
    /// vector [0,0,0,0,1,1,1,1] → distinct knots (0,1), mults (4,4).
    fn cubic_bezier_curve_body() -> String {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('p1',(0.,0.,0.));";
        s += "#2=CARTESIAN_POINT('p2',(1.,1.,0.));";
        s += "#3=CARTESIAN_POINT('p3',(2.,1.,0.));";
        s += "#4=CARTESIAN_POINT('p4',(3.,0.,0.));";
        s += "#10=B_SPLINE_CURVE_WITH_KNOTS('c',3,(#1,#2,#3,#4),.UNSPECIFIED.,.F.,.F.,(4,4),(0.,1.),.UNSPECIFIED.);";
        s
    }

    #[test]
    fn bspline_curve_cubic_bezier_resolves() {
        let (model, report, caches) = run(&cubic_bezier_curve_body());
        let cid = caches
            .curves
            .get(&10)
            .copied()
            .expect("B_SPLINE_CURVE_WITH_KNOTS must allocate a kernel curve");
        assert!(
            model.curves.get(cid).is_some(),
            "curve id must be valid in BRepModel"
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.entity == "B_SPLINE_CURVE_WITH_KNOTS"),
            "no warnings on the success path: {:?}",
            report.warnings
        );
    }

    #[test]
    fn bspline_curve_rejects_wrong_arity() {
        // Only 3 fields supplied instead of 9.
        let body = "#10=B_SPLINE_CURVE_WITH_KNOTS('c',3,(#1));";
        let (_m, report, caches) = run(body);
        assert!(!caches.curves.contains_key(&10));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.entity == "B_SPLINE_CURVE_WITH_KNOTS"));
    }

    #[test]
    fn bspline_curve_rejects_negative_degree() {
        let mut body = String::new();
        body += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        body += "#2=CARTESIAN_POINT('',(1.,0.,0.));";
        // degree = -1, otherwise valid arity.
        body += "#10=B_SPLINE_CURVE_WITH_KNOTS('c',-1,(#1,#2),.UNSPECIFIED.,.F.,.F.,(2,2),(0.,1.),.UNSPECIFIED.);";
        let (_m, report, caches) = run(&body);
        assert!(!caches.curves.contains_key(&10));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.message.contains("non-positive degree")));
    }

    #[test]
    fn bspline_curve_rejects_knot_count_mismatch() {
        // 4 control points + degree 3 ⇒ knot vector length must be 8.
        // (4,3) sums to 7 instead → flagged.
        let mut body = String::new();
        body += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        body += "#2=CARTESIAN_POINT('',(1.,0.,0.));";
        body += "#3=CARTESIAN_POINT('',(2.,0.,0.));";
        body += "#4=CARTESIAN_POINT('',(3.,0.,0.));";
        body += "#10=B_SPLINE_CURVE_WITH_KNOTS('c',3,(#1,#2,#3,#4),.UNSPECIFIED.,.F.,.F.,(4,3),(0.,1.),.UNSPECIFIED.);";
        let (_m, report, caches) = run(&body);
        assert!(!caches.curves.contains_key(&10));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.message.contains("n + p + 1")));
    }

    #[test]
    fn bspline_curve_rejects_missing_control_point() {
        // Reference a CARTESIAN_POINT that doesn't exist.
        let body =
            "#10=B_SPLINE_CURVE_WITH_KNOTS('c',1,(#999,#998),.UNSPECIFIED.,.F.,.F.,(2,2),(0.,1.),.UNSPECIFIED.);";
        let (_m, report, caches) = run(body);
        assert!(!caches.curves.contains_key(&10));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.message.contains("did not resolve")));
    }

    // ---------- B_SPLINE_SURFACE_WITH_KNOTS handler ----------

    /// 4×4 cubic-bicubic Bézier patch — simplest non-trivial NURBS
    /// surface. u_degree=v_degree=3, knots both = [0,0,0,0,1,1,1,1].
    fn bicubic_bezier_patch_body() -> String {
        let mut s = String::new();
        // 4×4 control points forming a flat z=0 grid.
        let mut idx = 1u32;
        for i in 0..4 {
            for j in 0..4 {
                s += &format!("#{idx}=CARTESIAN_POINT('',({i}.,{j}.,0.));");
                idx += 1;
            }
        }
        // Rows of refs (#1..#4, #5..#8, #9..#12, #13..#16).
        s += "#100=B_SPLINE_SURFACE_WITH_KNOTS('s',3,3,\
                ((#1,#2,#3,#4),(#5,#6,#7,#8),(#9,#10,#11,#12),(#13,#14,#15,#16)),\
                .UNSPECIFIED.,.F.,.F.,.F.,(4,4),(4,4),(0.,1.),(0.,1.),.UNSPECIFIED.);";
        s
    }

    #[test]
    fn bspline_surface_bicubic_bezier_resolves() {
        let (model, report, caches) = run(&bicubic_bezier_patch_body());
        let sid = caches
            .surfaces
            .get(&100)
            .copied()
            .expect("B_SPLINE_SURFACE_WITH_KNOTS must allocate a surface");
        assert!(model.surfaces.get(sid).is_some());
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.entity == "B_SPLINE_SURFACE_WITH_KNOTS"),
            "no warnings on success: {:?}",
            report.warnings
        );
    }

    #[test]
    fn bspline_surface_rejects_non_rectangular_grid() {
        let mut body = String::new();
        for i in 1..=6 {
            body += &format!("#{i}=CARTESIAN_POINT('',(0.,0.,0.));");
        }
        // 2 rows: first has 3 points, second has 3 — but the field
        // schema requires consistency. We use a row with 2 pts and a
        // row with 3 pts.
        body += "#100=B_SPLINE_SURFACE_WITH_KNOTS('s',1,1,\
                ((#1,#2),(#3,#4,#5)),\
                .UNSPECIFIED.,.F.,.F.,.F.,(2,2),(2,2),(0.,1.),(0.,1.),.UNSPECIFIED.);";
        let (_m, report, caches) = run(&body);
        assert!(!caches.surfaces.contains_key(&100));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.message.contains("non-rectangular") || w.message.contains("expected")));
    }

    #[test]
    fn bspline_surface_rejects_wrong_arity() {
        let body = "#100=B_SPLINE_SURFACE_WITH_KNOTS('s',1,1);";
        let (_m, report, caches) = run(body);
        assert!(!caches.surfaces.contains_key(&100));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.entity == "B_SPLINE_SURFACE_WITH_KNOTS"));
    }
}
