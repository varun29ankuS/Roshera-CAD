//! Topology-phase handlers: STEP topology entities → kernel
//! `Edge`/`Loop`/`Face`/`Shell`/`Solid` stores.
//!
//! Covered entities (tier-1, planar + cylindrical):
//!
//! | STEP                  | Effect on context                                                                |
//! |-----------------------|----------------------------------------------------------------------------------|
//! | `EDGE_CURVE`          | Allocates a kernel `Edge` (with backing `Line` or `Arc` curve), caches `EdgeId`. |
//! | `ORIENTED_EDGE`       | `(EdgeId, bool)` pair recording orientation against an existing edge.            |
//! | `EDGE_LOOP`           | Allocates a kernel `Loop`, populates ordered (edge, forward) pairs.              |
//! | `FACE_BOUND`          | `(LoopId, is_outer=false, forward)` triple at `caches.face_bounds`.              |
//! | `FACE_OUTER_BOUND`    | `(LoopId, is_outer=true, forward)` triple at `caches.face_bounds`.               |
//! | `ADVANCED_FACE`       | Allocates a kernel `Face`, threads outer/inner loops + surface + orientation.    |
//! | `CLOSED_SHELL`        | Allocates a kernel `Shell` (`ShellType::Closed`); runs manifold validation.      |
//! | `MANIFOLD_SOLID_BREP` | Allocates a kernel `Solid` bound to the closed shell.                            |
//!
//! ## Edge geometry construction
//!
//! STEP's `EDGE_CURVE` shares one `edge_geometry` reference (a `LINE`
//! or `CIRCLE` geometry template) across many edges with different
//! vertex endpoints. Tier-1 owns the per-edge instantiation:
//!
//! - `LINE`-backed edge: build a fresh `math::Line` segment from
//!   `start_vertex` position to `end_vertex` position; parameter
//!   range `[0, 1]`. `same_sense` becomes the edge's [`EdgeOrientation`].
//! - `CIRCLE`-backed edge:
//!   - If `start_vertex == end_vertex` (seam circle on a cylinder),
//!     allocate a full `math::Circle`; parameter range `[0, 1]`.
//!   - Else build a `math::Arc` with `start_angle` and `sweep_angle`
//!     derived from the two vertex positions, going counter-clockwise
//!     about the placement normal (STEP convention). The kernel's
//!     `Arc::new` re-derives its own canonical `x_axis`; we project
//!     both vertices onto that frame to compute angles, so the
//!     resulting arc is self-consistent regardless of how STEP's
//!     placement was originally oriented.
//!
//! ## Loop closure healing
//!
//! After every `EDGE_LOOP` materialises, we walk its ordered chain of
//! `(start_pos, end_pos)` vertex pairs and feed
//! [`super::healing::check_loop_closure`]. A gap exceeding
//! `ctx.default_tolerance` raises a [`HealingKind::LoopNotClosed`].
//!
//! ## Closed-shell manifold validation
//!
//! After every `CLOSED_SHELL` materialises, we call
//! [`super::manifold::validate_closed_shell`] and route any failure
//! buckets into [`super::manifold::emit_manifold_warnings`].
//!
//! ## Cross-phase resolution
//!
//! Topology handlers depend on the Geometry phase (vertices,
//! placements, surfaces, line/circle templates) being complete. The
//! dispatcher orders phases `Unit → Geometry → Topology → Root`,
//! so by the time we run every geometry referent is in the caches.
//! Where it's not (HashMap order is arbitrary within a phase, so
//! `EDGE_CURVE` may be visited before its `EDGE_LOOP`'s peer edges
//! are walked), [`super::resolver::ensure_resolved`] forces resolution
//! and the caches catch up.

use std::f64::consts::TAU;

use ruststep::ast::Record;

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::{
    curve::{Arc, Circle, Curve, Ellipse, Line, NurbsCurve, ParameterRange},
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceOrientation},
    r#loop::{Loop, LoopId, LoopType},
    shell::{Shell, ShellId, ShellType},
    solid::Solid,
    vertex::VertexId,
};

use crate::formats::step::{
    context::{ImportContext, StepCircleGeom, StepEllipseGeom, StepLineGeom, StepOpenConicGeom},
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::{EntityKind, EntityRegistry},
};

use super::healing::{check_edge_vertex_snap, check_loop_closure};
use super::manifold::{emit_manifold_warnings, validate_closed_shell};
use super::params;
use super::resolver::ensure_resolved;

/// Common `entity_name` constants used in diagnostic strings and the
/// resolver's `expected` arguments. Centralised so the dispatcher and
/// the resolver agree on capitalisation.
mod names {
    pub const EDGE_CURVE: &str = "EDGE_CURVE";
    pub const ORIENTED_EDGE: &str = "ORIENTED_EDGE";
    pub const EDGE_LOOP: &str = "EDGE_LOOP";
    pub const FACE_BOUND: &str = "FACE_BOUND";
    pub const FACE_OUTER_BOUND: &str = "FACE_OUTER_BOUND";
    pub const ADVANCED_FACE: &str = "ADVANCED_FACE";
    pub const CLOSED_SHELL: &str = "CLOSED_SHELL";
    pub const MANIFOLD_SOLID_BREP: &str = "MANIFOLD_SOLID_BREP";
    pub const VERTEX_POINT: &str = "VERTEX_POINT";
}

/// If `geom_ref` points to a `SURFACE_CURVE` or `SEAM_CURVE` (an
/// ISO 10303-42 wrapper that pairs a 3D `curve_3d` with its parameter-space
/// pcurve(s)), follow it to the underlying 3D curve reference and return that;
/// otherwise return `geom_ref` unchanged.
///
/// `(SURFACE|SEAM)_CURVE('', curve_3d, (pcurves), master)` — field index 1
/// (after the label) is the 3D curve. Roshera emits these for edges on
/// non-planar parametric faces to give readers an explicit seam trim; on
/// re-import the kernel only needs the 3D curve (pcurves carry no topology).
/// Unbounded recursion is impossible (a curve cannot wrap itself), but we cap
/// the follow depth defensively.
fn unwrap_surface_curve(geom_ref: u64, registry: &EntityRegistry) -> u64 {
    let mut current = geom_ref;
    for _ in 0..4 {
        let Some(entry) = registry.get(current) else {
            return current;
        };
        let record = match &entry.kind {
            EntityKind::Simple(rec) => rec,
            // A SURFACE_CURVE/SEAM_CURVE is always a simple entity.
            EntityKind::Complex(_) => return current,
        };
        if record.name != "SURFACE_CURVE" && record.name != "SEAM_CURVE" {
            return current;
        }
        let fields = match params::record_fields(&record.parameter, &record.name, current) {
            Ok(f) => f,
            Err(_) => return current,
        };
        // field[0] = label, field[1] = curve_3d.
        match fields.get(1) {
            Some(p) => match params::as_entity_ref(p, &record.name, current, "curve_3d") {
                Ok(next) => current = next,
                Err(_) => return current,
            },
            None => return current,
        }
    }
    current
}

// =========================================================================
// EDGE_CURVE
// =========================================================================

/// `EDGE_CURVE('label', #start_vertex, #end_vertex, #edge_geometry, same_sense)`.
///
/// Allocates a kernel `Edge` backed by either a fresh `Line` (when the
/// underlying STEP entity is a `LINE`) or `Arc`/`Circle` (`CIRCLE`).
/// The curve is sized to the edge's vertex endpoints — STEP shares
/// one geometry record across many edges, so each edge gets its own
/// kernel curve instance.
pub struct EdgeCurveHandler;
/// Static binding consumed by [`register`].
pub static EDGE_CURVE_HANDLER: EdgeCurveHandler = EdgeCurveHandler;

impl EntityHandler for EdgeCurveHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::EDGE_CURVE]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::EDGE_CURVE, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 5 {
            return field_count_error(
                ctx,
                names::EDGE_CURVE,
                instance,
                "expected (label, start_vertex, end_vertex, edge_geometry, same_sense)",
            );
        }
        let start_ref =
            match params::as_entity_ref(&fields[1], names::EDGE_CURVE, instance, "start_vertex") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad start_vertex ref".into(),
                    };
                }
            };
        let end_ref =
            match params::as_entity_ref(&fields[2], names::EDGE_CURVE, instance, "end_vertex") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad end_vertex ref".into(),
                    };
                }
            };
        let geom_ref =
            match params::as_entity_ref(&fields[3], names::EDGE_CURVE, instance, "edge_geometry") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad edge_geometry ref".into(),
                    };
                }
            };
        // The `edge_geometry` may be a `SURFACE_CURVE` / `SEAM_CURVE` that
        // wraps the actual 3D curve together with its parameter-space pcurve(s)
        // (the form Roshera now exports for edges on parametric faces, and a
        // standard ISO 10303-42 form other systems emit too). We import the 3D
        // curve only — pcurves are a trimming hint for downstream readers and
        // carry no topology the kernel needs. Follow the wrapper's first field
        // (`curve_3d`) to the underlying geometry.
        let geom_ref = unwrap_surface_curve(geom_ref, registry);

        let same_sense =
            match params::as_bool(&fields[4], names::EDGE_CURVE, instance, "same_sense") {
                Ok(Some(b)) => b,
                Ok(None) => {
                    // `.U.` — unknown sense, default to true and warn.
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: names::EDGE_CURVE.into(),
                        instance: Some(instance),
                        message: "same_sense was .U.; defaulting to .T.".into(),
                    });
                    true
                }
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad same_sense".into(),
                    };
                }
            };

        let start_vid = match resolve_vertex(start_ref, registry, dispatch, ctx) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "start_vertex unresolved".into(),
                }
            }
        };
        let end_vid = match resolve_vertex(end_ref, registry, dispatch, ctx) {
            Some(v) => v,
            None => {
                return HandlerOutcome::Failed {
                    message: "end_vertex unresolved".into(),
                }
            }
        };

        // Force the underlying geometry record to be dispatched so the
        // template is in step_lines / step_circles, or — for free-form
        // edges — the kernel curve is in `caches.curves`. The edge
        // geometry may be a LINE, CIRCLE, ELLIPSE, a (simple or rational)
        // B-spline curve, or a trimmed/composite curve. We pass the full
        // accepted set; the resolver only dispatches the one whose name
        // matches, and complex (rational) B-splines dispatch through the
        // dispatcher's `Complex` arm.
        let _ = ensure_resolved(
            geom_ref,
            &[
                "LINE",
                "CIRCLE",
                "ELLIPSE",
                "HYPERBOLA",
                "PARABOLA",
                "B_SPLINE_CURVE_WITH_KNOTS",
                "BOUNDED_CURVE",
                "B_SPLINE_CURVE",
                "RATIONAL_B_SPLINE_CURVE",
                "CURVE",
            ],
            registry,
            dispatch,
            ctx,
        );

        // Vertex positions for curve construction + snap-check.
        let start_pos = match ctx.model.vertices.get_position(start_vid) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "start vertex missing position".into(),
                }
            }
        };
        let end_pos = match ctx.model.vertices.get_position(end_vid) {
            Some(p) => p,
            None => {
                return HandlerOutcome::Failed {
                    message: "end vertex missing position".into(),
                }
            }
        };

        // Dispatch on the underlying geometry template. LINE and CIRCLE
        // are per-edge instantiated (one STEP template shared by many
        // edges → a fresh kernel curve sized to this edge's vertices).
        // A free-form edge instead references a fully-realised kernel
        // curve already in `caches.curves` (a B-spline/NURBS curve, simple
        // or rational, materialised by the tier-2 B-spline handler or the
        // complex-entity dispatch path). For those we clone the kernel
        // curve and adopt its native parameter range. `edge_param_range`
        // carries the per-curve domain so the `Edge` is parameterised
        // consistently with its backing curve.
        // The edge is the WHOLE of a closed curve only when it starts and
        // ends at one vertex: the same vertex entity, or two written at the
        // very same point. Two different vertices a hair apart bound a hair
        // of arc, however short -- never "the same point within the file's
        // accuracy" (CADGenBench parts 247, 248 and 250: arcs of 3 to 19
        // micrometres built as the full circles they lie on).
        let closed = start_vid == end_vid || start_pos == end_pos;
        let mut edge_param_range = ParameterRange::unit();
        let curve_box: Box<dyn Curve> =
            if let Some(line_geom) = ctx.caches.step_lines.get(&geom_ref).copied() {
                build_line_curve(line_geom, start_pos, end_pos)
            } else if let Some(circle_geom) = ctx.caches.step_circles.get(&geom_ref).copied() {
                match build_circle_curve(
                    circle_geom,
                    start_pos,
                    end_pos,
                    same_sense,
                    closed,
                    instance,
                    ctx,
                ) {
                    Some(c) => c,
                    None => {
                        return HandlerOutcome::Failed {
                            message: "kernel rejected circle/arc".into(),
                        }
                    }
                }
            } else if let Some(ellipse_geom) = ctx.caches.step_ellipses.get(&geom_ref).copied() {
                match build_ellipse_curve(
                    ellipse_geom,
                    start_pos,
                    end_pos,
                    same_sense,
                    closed,
                    instance,
                    ctx,
                ) {
                    Some((c, range)) => {
                        edge_param_range = range;
                        c
                    }
                    None => {
                        return HandlerOutcome::Failed {
                            message: "kernel rejected ellipse".into(),
                        }
                    }
                }
            } else if let Some(conic) = ctx.caches.step_open_conics.get(&geom_ref).copied() {
                match build_open_conic_curve(conic, start_pos, end_pos, instance, ctx) {
                    Some((c, range)) => {
                        edge_param_range = range;
                        c
                    }
                    None => {
                        return HandlerOutcome::Failed {
                            message: "kernel rejected the conic arc".into(),
                        }
                    }
                }
            } else if let Some(curve_id) = ctx.caches.curves.get(&geom_ref).copied() {
                // Free-form edge backed by an already-materialised kernel
                // curve (B-spline / NURBS). Clone it so this edge owns an
                // independent curve instance and preserve its native domain.
                match ctx.model.curves.get(curve_id) {
                    Some(c) => {
                        edge_param_range =
                            match span_between_vertices(c, start_pos, end_pos, same_sense) {
                                Ok(span) => {
                                    if span.off_curve > VERTEX_ON_CURVE {
                                        ctx.report.push_warning(Warning {
                                            severity: Severity::Warn,
                                            entity: names::EDGE_CURVE.into(),
                                            instance: Some(instance),
                                            message: format!(
                                            "a vertex lies {:.3e} off its curve #{geom_ref}; the \
                                         edge, {:.3e} long, is trimmed at the curve's nearest \
                                         point",
                                            span.off_curve, span.length
                                        ),
                                        });
                                    }
                                    span.range
                                }
                                Err(refusal) => {
                                    // Reported here: an edge is usually reached
                                    // through its loop, and that path drops the
                                    // `Failed` message.
                                    ctx.report.push_warning(Warning {
                                        severity: Severity::Warn,
                                        entity: names::EDGE_CURVE.into(),
                                        instance: Some(instance),
                                        message: format!(
                                        "a vertex lies {:.3e} off its curve #{geom_ref}, beyond \
                                         the {:.3e} allowed for an edge {:.3e} long; the edge is \
                                         refused",
                                        refusal.distance, refusal.allowed, refusal.edge_length
                                    ),
                                    });
                                    return HandlerOutcome::Failed {
                                        message: "edge vertex too far from the edge's curve".into(),
                                    };
                                }
                            };
                        c.clone_box()
                    }
                    None => {
                        ctx.report.push_warning(Warning {
                            severity: Severity::Warn,
                            entity: names::EDGE_CURVE.into(),
                            instance: Some(instance),
                            message: format!(
                                "edge_geometry #{geom_ref} cached a curve id absent from the model"
                            ),
                        });
                        return HandlerOutcome::Failed {
                            message: "edge curve id dangling".into(),
                        };
                    }
                }
            } else {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: names::EDGE_CURVE.into(),
                    instance: Some(instance),
                    message: format!(
                        "edge_geometry #{geom_ref} did not resolve to a supported curve \
                     (LINE / CIRCLE / ELLIPSE / B_SPLINE_CURVE_WITH_KNOTS / rational B-spline)"
                    ),
                });
                return HandlerOutcome::Failed {
                    message: "unsupported edge_geometry".into(),
                };
            };

        // Per-edge snap check between the curve's endpoint evaluation
        // and the recorded vertex position. STEP doesn't always emit
        // perfectly coincident points; we record any deviation. Evaluate
        // at the curve's actual domain endpoints (unit range for the
        // per-edge Line/Arc, native range for a cloned NURBS curve).
        if let Ok(cp) = curve_box.evaluate(edge_param_range.start) {
            check_edge_vertex_snap(
                [cp.position.x, cp.position.y, cp.position.z],
                start_pos,
                ctx.default_tolerance,
                names::EDGE_CURVE,
                instance,
                ctx,
            );
        }
        if let Ok(cp) = curve_box.evaluate(edge_param_range.end) {
            check_edge_vertex_snap(
                [cp.position.x, cp.position.y, cp.position.z],
                end_pos,
                ctx.default_tolerance,
                names::EDGE_CURVE,
                instance,
                ctx,
            );
        }

        let curve_id = ctx.model.curves.add(curve_box);
        let orientation = if same_sense {
            EdgeOrientation::Forward
        } else {
            EdgeOrientation::Backward
        };
        let tol = ctx.default_tolerance;
        let edge = Edge::new(
            0,
            start_vid,
            end_vid,
            curve_id,
            orientation,
            edge_param_range,
        );
        let mut edge = edge;
        edge.set_tolerance(tol);
        let edge_id = ctx.model.edges.add(edge);
        ctx.caches.edges.insert(instance, edge_id);
        HandlerOutcome::Resolved
    }
}

/// Construct a kernel `Ellipse` trimmed to the edge: it runs from `start` to
/// `end` in the edge's direction of travel, parameterised by angle (`t` = 0
/// on the major semi-axis), and the returned range is that trimmed span.
///
/// Same convention as [`build_circle_curve`]: an EDGE_CURVE with
/// `same_sense = .F.` traverses the basis ellipse backwards, so the frame's
/// second axis is flipped -- the counter-clockwise sweep in the flipped frame
/// is the clockwise one in the basis frame. `start == end` is a seam: the
/// whole ellipse.
fn build_ellipse_curve(
    ellipse: StepEllipseGeom,
    start: [f64; 3],
    end: [f64; 3],
    same_sense: bool,
    closed: bool,
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> Option<(Box<dyn Curve>, ParameterRange)> {
    let p = ellipse.placement;
    let center = Point3::new(p.origin[0], p.origin[1], p.origin[2]);
    let x = Vector3::new(p.x[0], p.x[1], p.x[2]);
    let z = Vector3::new(p.z[0], p.z[1], p.z[2]);
    let mut y = z.cross(&x);
    if !same_sense {
        y = Vector3::new(-y.x, -y.y, -y.z);
    }
    let (a, b) = (ellipse.semi_axis_1, ellipse.semi_axis_2);
    let angle = |q: [f64; 3]| -> f64 {
        let d = Vector3::new(q[0] - center.x, q[1] - center.y, q[2] - center.z);
        (d.dot(&y) / b).atan2(d.dot(&x) / a)
    };
    let t0 = angle(start);
    // The whole ellipse only for an edge closed on one vertex (see the
    // caller); otherwise the arc from `start` round to `end`.
    let mut t1 = if closed { t0 + TAU } else { angle(end) };
    while t1 <= t0 {
        t1 += TAU;
    }
    let mut curve = match Ellipse::new(center, x, y, a, b) {
        Ok(c) => c,
        Err(e) => {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::EDGE_CURVE.into(),
                instance: Some(instance),
                message: format!("kernel rejected Ellipse: {e}"),
            });
            return None;
        }
    };
    let range = ParameterRange::new(t0, t1);
    curve.range = range;
    Some((Box::new(curve), range))
}

/// The exact rational quadratic arc of a HYPERBOLA or PARABOLA between two
/// of its points (Piegl & Tiller, The NURBS Book, sec. 7.5: every conic arc
/// is a rational quadratic Bezier). With the arc's parameters t0, t1,
/// m = (t0 + t1) / 2 and h = (t1 - t0) / 2:
///   hyperbola  P1 = o + (a cosh m / cosh h) x + (b sinh m / cosh h) y,
///              middle weight cosh h;
///   parabola   P1 = o + f t0 t1 x + f (t0 + t1) y, middle weight 1.
/// An open conic is a single branch, so the arc between two of its points
/// is unique and the edge's sense needs no special handling: the arc runs
/// from the start vertex to the end vertex. Returns the curve and its
/// parameter range [0, 1].
fn build_open_conic_curve(
    conic: StepOpenConicGeom,
    start: [f64; 3],
    end: [f64; 3],
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> Option<(Box<dyn Curve>, ParameterRange)> {
    let placement = match conic {
        StepOpenConicGeom::Hyperbola { placement, .. } => placement,
        StepOpenConicGeom::Parabola { placement, .. } => placement,
    };
    let o = Point3::new(
        placement.origin[0],
        placement.origin[1],
        placement.origin[2],
    );
    let x = Vector3::new(placement.x[0], placement.x[1], placement.x[2]);
    let z = Vector3::new(placement.z[0], placement.z[1], placement.z[2]);
    let y = z.cross(&x);
    let local = |q: [f64; 3]| -> (f64, f64) {
        let d = Vector3::new(q[0] - o.x, q[1] - o.y, q[2] - o.z);
        (d.dot(&x), d.dot(&y))
    };
    let at = |u: f64, v: f64| o + x * u + y * v;
    let (p1, w1, t0, t1) = match conic {
        StepOpenConicGeom::Hyperbola {
            semi_axis: a,
            semi_imag_axis: b,
            ..
        } => {
            let t0 = (local(start).1 / b).asinh();
            let t1 = (local(end).1 / b).asinh();
            let (m, h) = (0.5 * (t0 + t1), 0.5 * (t1 - t0));
            let ch = h.cosh();
            (at(a * m.cosh() / ch, b * m.sinh() / ch), ch, t0, t1)
        }
        StepOpenConicGeom::Parabola { focal_dist: f, .. } => {
            let t0 = local(start).1 / (2.0 * f);
            let t1 = local(end).1 / (2.0 * f);
            (at(f * t0 * t1, f * (t0 + t1)), 1.0, t0, t1)
        }
    };
    if !(t1 - t0).is_finite() || (t1 - t0).abs() < f64::EPSILON {
        ctx.report.push_warning(Warning {
            severity: Severity::Warn,
            entity: names::EDGE_CURVE.into(),
            instance: Some(instance),
            message: "conic edge has no length between its vertices".into(),
        });
        return None;
    }
    let p0 = Point3::new(start[0], start[1], start[2]);
    let p2 = Point3::new(end[0], end[1], end[2]);
    match NurbsCurve::new(
        2,
        vec![p0, p1, p2],
        vec![1.0, w1, 1.0],
        vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ) {
        Ok(c) => Some((Box::new(c), ParameterRange::new(0.0, 1.0))),
        Err(e) => {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::EDGE_CURVE.into(),
                instance: Some(instance),
                message: format!("kernel rejected the conic arc: {e}"),
            });
            None
        }
    }
}

/// Which of a face's bounds is its OUTER loop, when the file does not say.
/// STEP marks it with FACE_OUTER_BOUND, but writers may not (OCCT never does)
/// and list bounds in any order, so the first bound is no evidence. A face's
/// outer loop encloses its holes, so it has the greatest extent: the loop
/// whose sampled edges span the largest box. `None` when no loop is clearly
/// the largest (a cylinder band's two equal rims) -- the caller then keeps
/// the file's order, as any choice is then as good as another.
fn enclosing_loop(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
    bounds: &[(LoopId, bool, bool)],
) -> Option<LoopId> {
    if bounds.len() < 2 {
        return None;
    }
    let extent = |lid: LoopId| -> Option<f64> {
        let lp = model.loops.get(lid)?;
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for &e in &lp.edges {
            let edge = model.edges.get(e)?;
            let curve = model.curves.get(edge.curve_id)?;
            let r = edge.param_range;
            for k in 0..=8 {
                let t = r.start + (r.end - r.start) * k as f64 / 8.0;
                let p = curve.evaluate(t).ok()?.position;
                for (i, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                    lo[i] = lo[i].min(c);
                    hi[i] = hi[i].max(c);
                }
            }
        }
        let d: f64 = (0..3).map(|i| (hi[i] - lo[i]).powi(2)).sum::<f64>().sqrt();
        d.is_finite().then_some(d)
    };
    let mut sized: Vec<(LoopId, f64)> = Vec::with_capacity(bounds.len());
    for &(lid, _, _) in bounds {
        sized.push((lid, extent(lid)?));
    }
    sized.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (largest, next) = (sized[0], sized[1]);
    // Clearly the largest: more than a part in a million beyond the next.
    (largest.1 > next.1 * (1.0 + 1e-6)).then_some(largest.0)
}

/// The part of a shared curve an edge uses: the parameter span between its
/// two vertices. STEP writers reuse one long curve (an intersection curve, a
/// spline through several faces) for several edges, each a piece of it, so
/// an edge is the curve BETWEEN its vertices, not the whole curve. A vertex
/// at an end of the curve keeps that end exactly; a vertex inside it is
/// projected onto the curve. A curve whose two ends meet (a closed spline)
/// with both vertices there is the whole curve.
///
/// A vertex need not lie exactly on the curve: writers keep a tolerance on
/// each vertex (on valid CADGenBench parts vertices sit up to 4.7e-2 off
/// their curves), so the edge is trimmed at the curve's NEAREST point to the
/// vertex and the distance is returned in [`EdgeSpan::off_curve`]. That
/// distance is bounded by [`vertex_off_curve_allowance`]; a vertex farther
/// off is not on this curve in any useful sense and the edge is refused. It
/// is never widened to the whole curve, which lays it over its neighbours.
fn span_between_vertices(
    curve: &dyn Curve,
    start: [f64; 3],
    end: [f64; 3],
    same_sense: bool,
) -> Result<EdgeSpan, VertexOffCurve> {
    let domain = curve.parameter_range();
    let at = |t: f64| curve.evaluate(t).ok().map(|p| p.position);
    let (Some(c0), Some(c1)) = (at(domain.start), at(domain.end)) else {
        return Ok(EdgeSpan {
            range: domain,
            off_curve: 0.0,
            length: 0.0,
        });
    };
    let to = |p: Point3, q: [f64; 3]| p.distance(&Point3::new(q[0], q[1], q[2]));
    let end_tol = VERTEX_ON_CURVE;
    let closed = c0.distance(&c1) <= end_tol;
    // The parameter of the curve's point nearest the vertex, and how far
    // that point is from the vertex.
    let param = |q: [f64; 3]| -> (f64, f64) {
        if to(c0, q) <= end_tol {
            return (domain.start, to(c0, q));
        }
        if to(c1, q) <= end_tol {
            return (domain.end, to(c1, q));
        }
        let target = Point3::new(q[0], q[1], q[2]);
        let solved = curve
            .closest_point(&target, Tolerance::from_distance(1e-10))
            .ok()
            .map(|(t, p)| (t, p.distance(&target)));
        match solved {
            Some(on) if on.1 <= VERTEX_ON_CURVE => on,
            // The curve's own solver starts from a few samples and can stop
            // at a far local minimum on a long spline. Before the vertex is
            // called off the curve, the whole curve is searched.
            _ => match (solved, nearest_on_curve(curve, target)) {
                (Some(a), Some(b)) => {
                    if a.1 <= b.1 {
                        a
                    } else {
                        b
                    }
                }
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => (domain.start, f64::INFINITY),
            },
        }
    };
    let (ts, start_off) = param(start);
    let (te, end_off) = param(end);
    let range =
        if closed && (ts - te).abs() <= f64::EPSILON * (domain.end - domain.start).abs().max(1.0) {
            // Both vertices at one point of the closed curve: the whole loop.
            domain
        } else if closed {
            // On a closed curve a vertex at the seam is at BOTH ends of the
            // domain. Along the curve's direction the edge runs from its start
            // vertex when `same_sense`, else from its end vertex: the leading
            // vertex takes the domain's start, the trailing one its end.
            let seam = |t: f64| t == domain.start || t == domain.end;
            let (lead, trail) = if same_sense { (ts, te) } else { (te, ts) };
            let lead = if seam(lead) { domain.start } else { lead };
            let trail = if seam(trail) { domain.end } else { trail };
            ParameterRange::new(lead.min(trail), lead.max(trail))
        } else {
            ParameterRange::new(ts.min(te), ts.max(te))
        };
    let off_curve = start_off.max(end_off);
    if off_curve <= VERTEX_ON_CURVE {
        return Ok(EdgeSpan {
            range,
            off_curve,
            length: 0.0,
        });
    }
    let length = span_length(curve, range);
    let allowed = vertex_off_curve_allowance(length);
    if off_curve <= allowed {
        Ok(EdgeSpan {
            range,
            off_curve,
            length,
        })
    } else {
        // Also where the distance is not a number.
        Err(VertexOffCurve {
            distance: off_curve,
            allowed,
            edge_length: length,
        })
    }
}

/// The span of its curve an edge uses, and how far the farther of its two
/// vertices is from the curve there. `length` is the span's length, measured
/// only when a vertex is more than [`VERTEX_ON_CURVE`] off (else zero).
#[derive(Debug, Clone, Copy)]
struct EdgeSpan {
    range: ParameterRange,
    off_curve: f64,
    length: f64,
}

/// Why an edge was refused: a vertex is `distance` from the edge's curve,
/// more than the `allowed` for an edge `edge_length` long.
#[derive(Debug, Clone, Copy)]
struct VertexOffCurve {
    distance: f64,
    allowed: f64,
    edge_length: f64,
}

/// How far a vertex may sit from the curve its edge runs along and still
/// be read as exactly ON it (model units after scaling): nothing is
/// reported, and a vertex this close to an end of the curve is that end.
const VERTEX_ON_CURVE: f64 = 1e-3;

/// The most a vertex may sit off its edge's curve, as a share of the edge's
/// own length: less than half. The distance is the vertex's tolerance, the
/// radius of the ball in which the edge's end may be anywhere; at half the
/// edge's length the balls of its two ends meet and nothing of the edge is
/// left between them, so there is no honest place to trim. (On the valid
/// CADGenBench parts the largest share is 0.35, a 3.1e-3 gap on an edge
/// 9.0e-3 long; the largest gap, 4.7e-2, is 0.16 of its edge.)
const VERTEX_OFF_CURVE_SHARE: f64 = 0.5;

/// The most a vertex may sit off its edge's curve whatever the edge's
/// length (model units, mm): about ten times a typical writer's stated
/// accuracy, just above the largest gap on the valid CADGenBench parts
/// (4.7e-2). Without it a long edge would accept any gap under half its
/// length.
const VERTEX_OFF_CURVE_CAP: f64 = 0.05;

/// How far a vertex may be from the curve of an edge `edge_length` long and
/// still end that edge at the curve's nearest point: under half the edge's
/// length, and never more than [`VERTEX_OFF_CURVE_CAP`].
fn vertex_off_curve_allowance(edge_length: f64) -> f64 {
    VERTEX_ON_CURVE
        .max(VERTEX_OFF_CURVE_SHARE * edge_length)
        .min(VERTEX_OFF_CURVE_CAP)
}

/// The curve's nearest point to `target` over its whole domain, as
/// (parameter, distance): the nearest of evenly spaced samples, narrowed
/// between that sample's two neighbours by golden-section search. `None`
/// when the curve cannot be evaluated.
fn nearest_on_curve(curve: &dyn Curve, target: Point3) -> Option<(f64, f64)> {
    const SAMPLES: usize = 512;
    const NARROWINGS: usize = 80;
    let domain = curve.parameter_range();
    let step = (domain.end - domain.start) / SAMPLES as f64;
    let distance = |t: f64| curve.evaluate(t).ok().map(|p| p.position.distance(&target));
    let mut nearest = (domain.start, distance(domain.start)?);
    for k in 1..=SAMPLES {
        let t = domain.start + step * k as f64;
        let d = distance(t)?;
        if d < nearest.1 {
            nearest = (t, d);
        }
    }
    let mut lo = (nearest.0 - step).max(domain.start);
    let mut hi = (nearest.0 + step).min(domain.end);
    let golden = 0.5 * (5.0_f64.sqrt() - 1.0);
    let mut a = hi - golden * (hi - lo);
    let mut b = lo + golden * (hi - lo);
    let (mut da, mut db) = (distance(a)?, distance(b)?);
    for _ in 0..NARROWINGS {
        if da <= db {
            hi = b;
            (b, db) = (a, da);
            a = hi - golden * (hi - lo);
            da = distance(a)?;
        } else {
            lo = a;
            (a, da) = (b, db);
            b = lo + golden * (hi - lo);
            db = distance(b)?;
        }
    }
    for candidate in [(a, da), (b, db)] {
        if candidate.1 < nearest.1 {
            nearest = candidate;
        }
    }
    Some(nearest)
}

/// The length of the curve over `range`, as a polyline through evenly
/// spaced points: the scale an edge's vertex tolerance is judged against.
/// Zero when the curve cannot be evaluated, so nothing more is allowed.
fn span_length(curve: &dyn Curve, range: ParameterRange) -> f64 {
    const CHORDS: usize = 32;
    let mut length = 0.0;
    let mut previous: Option<Point3> = None;
    for k in 0..=CHORDS {
        let t = range.start + (range.end - range.start) * k as f64 / CHORDS as f64;
        let Ok(point) = curve.evaluate(t) else {
            return 0.0;
        };
        if let Some(p) = previous {
            length += p.distance(&point.position);
        }
        previous = Some(point.position);
    }
    length
}

/// Construct a `Line` curve sized to the edge's two vertex positions.
fn build_line_curve(_line: StepLineGeom, start: [f64; 3], end: [f64; 3]) -> Box<dyn Curve> {
    let s = Point3::new(start[0], start[1], start[2]);
    let e = Point3::new(end[0], end[1], end[2]);
    Box::new(Line::new(s, e))
}

/// Construct a `Circle` or `Arc` curve sized to the edge's two
/// vertex positions. Returns `None` when the kernel rejects the
/// constructor (zero radius or zero normal — emits a warning).
fn build_circle_curve(
    circle: StepCircleGeom,
    start: [f64; 3],
    end: [f64; 3],
    same_sense: bool,
    closed: bool,
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> Option<Box<dyn Curve>> {
    let center = Point3::new(
        circle.placement.origin[0],
        circle.placement.origin[1],
        circle.placement.origin[2],
    );
    // A CIRCLE parameterises COUNTER-CLOCKWISE about its placement normal.
    // An EDGE_CURVE with `same_sense = .F.` traverses that basis curve in
    // REVERSE, so the trimmed arc from `start` to `end` is the CW portion, not
    // the CCW one. The arc reconstruction below always takes the CCW sweep
    // (`sweep = end − start`, forced positive), which for a reversed-sense arc
    // yields the COMPLEMENT — a 90° fillet-band end-arc re-imports as a 270°
    // arc, tearing the band and its adjacent cap faces (#42). Negating the
    // reconstruction normal for a reversed edge flips CCW↔CW, so the forced-
    // positive sweep lands on the correct (short) arc while `evaluate(0)` still
    // sits on `start`. Full-circle seam edges (`start == end`, handled below)
    // are unaffected — a circle is orientation-symmetric.
    let normal = {
        let n = Vector3::new(
            circle.placement.z[0],
            circle.placement.z[1],
            circle.placement.z[2],
        );
        if same_sense {
            n
        } else {
            Vector3::new(-n.x, -n.y, -n.z)
        }
    };

    // Seam edge: closed on one vertex (decided by the caller) → full circle.
    if closed {
        // The whole circle, starting and ending at the edge's vertex.
        let vertex = Point3::new(start[0], start[1], start[2]);
        return match Circle::new_starting_at(center, normal, circle.radius, vertex) {
            Ok(c) => Some(Box::new(c)),
            Err(e) => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: names::EDGE_CURVE.into(),
                    instance: Some(instance),
                    message: format!("kernel rejected Circle: {e}"),
                });
                None
            }
        };
    }

    // Arc edge: compute start/sweep angles in the *kernel's* canonical
    // x_axis frame. Build a full circle first to learn what x_axis
    // the kernel picked, then re-build the arc with explicit angles.
    let probe = match Arc::new(center, normal, circle.radius, 0.0, TAU) {
        Ok(a) => a,
        Err(e) => {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::EDGE_CURVE.into(),
                instance: Some(instance),
                message: format!("kernel rejected Arc probe: {e}"),
            });
            return None;
        }
    };
    let x_axis = probe.x_axis;
    let n_unit = probe.normal;
    let y_axis = n_unit.cross(&x_axis);

    let s_local = Vector3::new(
        start[0] - center.x,
        start[1] - center.y,
        start[2] - center.z,
    );
    let e_local = Vector3::new(end[0] - center.x, end[1] - center.y, end[2] - center.z);

    let start_angle = s_local.dot(&y_axis).atan2(s_local.dot(&x_axis));
    let end_angle = e_local.dot(&y_axis).atan2(e_local.dot(&x_axis));
    // STEP arcs run counter-clockwise about the placement normal.
    let mut sweep = end_angle - start_angle;
    while sweep <= 0.0 {
        sweep += TAU;
    }

    match Arc::new(center, normal, circle.radius, start_angle, sweep) {
        Ok(a) => Some(Box::new(a)),
        Err(e) => {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::EDGE_CURVE.into(),
                instance: Some(instance),
                message: format!("kernel rejected Arc: {e}"),
            });
            None
        }
    }
}

// =========================================================================
// ORIENTED_EDGE
// =========================================================================

/// `ORIENTED_EDGE('label', *, *, #edge_element, orientation)`.
///
/// STEP's `ORIENTED_EDGE` inherits `edge_start`/`edge_end` from its
/// supertype `EDGE` and overrides them via the `*` placeholders.
/// We only care about `edge_element` (an `EDGE_CURVE` ref) and
/// `orientation` (boolean — same sense as the underlying edge or
/// reversed).
///
/// No kernel allocation: the cache entry is `(EdgeId, forward)`.
pub struct OrientedEdgeHandler;
/// Static binding consumed by [`register`].
pub static ORIENTED_EDGE_HANDLER: OrientedEdgeHandler = OrientedEdgeHandler;

impl EntityHandler for OrientedEdgeHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::ORIENTED_EDGE]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::ORIENTED_EDGE, instance)
        {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 5 {
            return field_count_error(
                ctx,
                names::ORIENTED_EDGE,
                instance,
                "expected (label, *, *, edge_element, orientation)",
            );
        }
        let edge_ref =
            match params::as_entity_ref(&fields[3], names::ORIENTED_EDGE, instance, "edge_element")
            {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad edge_element ref".into(),
                    };
                }
            };
        let orientation =
            match params::as_bool(&fields[4], names::ORIENTED_EDGE, instance, "orientation") {
                Ok(Some(b)) => b,
                Ok(None) => {
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: names::ORIENTED_EDGE.into(),
                        instance: Some(instance),
                        message: "orientation was .U.; defaulting to .T.".into(),
                    });
                    true
                }
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad orientation".into(),
                    };
                }
            };

        let edge_id = match resolve_edge(edge_ref, registry, dispatch, ctx) {
            Some(e) => e,
            None => {
                return HandlerOutcome::Failed {
                    message: "edge_element unresolved".into(),
                }
            }
        };
        ctx.caches
            .oriented_edges
            .insert(instance, (edge_id, orientation));
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// EDGE_LOOP
// =========================================================================

/// `EDGE_LOOP('label', (oriented_edge_refs))`.
///
/// Allocates a kernel `Loop` (initially `LoopType::Unknown`; the
/// owning `ADVANCED_FACE` retags it as Outer/Inner). Each oriented
/// edge contributes one `(EdgeId, forward)` pair.
///
/// Walks the chain after population and emits a
/// [`HealingKind::LoopNotClosed`] if the closure gap exceeds
/// `ctx.default_tolerance`.
pub struct EdgeLoopHandler;
/// Static binding consumed by [`register`].
pub static EDGE_LOOP_HANDLER: EdgeLoopHandler = EdgeLoopHandler;

impl EntityHandler for EdgeLoopHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::EDGE_LOOP]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::EDGE_LOOP, instance) {
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
                names::EDGE_LOOP,
                instance,
                "expected (label, (oriented_edges))",
            );
        }
        let oe_refs =
            match params::as_entity_ref_list(&fields[1], names::EDGE_LOOP, instance, "edge_list") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad edge_list".into(),
                    };
                }
            };
        if oe_refs.is_empty() {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::EDGE_LOOP.into(),
                instance: Some(instance),
                message: "empty edge_list".into(),
            });
            return HandlerOutcome::Failed {
                message: "empty edge_list".into(),
            };
        }

        let mut entries: Vec<(EdgeId, bool)> = Vec::with_capacity(oe_refs.len());
        for oe_ref in oe_refs.iter().copied() {
            match resolve_oriented_edge(oe_ref, registry, dispatch, ctx) {
                Some(pair) => entries.push(pair),
                None => {
                    return HandlerOutcome::Failed {
                        message: format!("oriented_edge #{oe_ref} unresolved"),
                    };
                }
            }
        }

        // Closure healing — record the chain of (start_pos, end_pos)
        // pairs in the orientation each oriented_edge implies.
        let mut chain: Vec<([f64; 3], [f64; 3])> = Vec::with_capacity(entries.len());
        for (edge_id, forward) in entries.iter().copied() {
            let edge = match ctx.model.edges.get(edge_id) {
                Some(e) => e.clone(),
                None => continue,
            };
            let s = ctx.model.vertices.get_position(edge.start_vertex);
            let e = ctx.model.vertices.get_position(edge.end_vertex);
            let (Some(sp), Some(ep)) = (s, e) else {
                continue;
            };
            if forward {
                chain.push((sp, ep));
            } else {
                chain.push((ep, sp));
            }
        }
        check_loop_closure(
            &chain,
            ctx.default_tolerance,
            names::EDGE_LOOP,
            instance,
            ctx,
        );

        let mut lp = Loop::with_capacity(0, LoopType::Unknown, entries.len());
        for (edge_id, forward) in entries.iter().copied() {
            lp.add_edge(edge_id, forward);
        }
        let loop_id = ctx.model.loops.add(lp);
        ctx.caches.loops.insert(instance, loop_id);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// FACE_BOUND / FACE_OUTER_BOUND
// =========================================================================

/// `FACE_BOUND('label', #edge_loop, orientation)`.
///
/// Caches `(LoopId, is_outer=false, forward)`. The owning
/// `ADVANCED_FACE` consumes this to pick out outer vs. inner.
pub struct FaceBoundHandler;
/// Static binding consumed by [`register`].
pub static FACE_BOUND_HANDLER: FaceBoundHandler = FaceBoundHandler;

impl EntityHandler for FaceBoundHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::FACE_BOUND]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        handle_face_bound(false, instance, record, registry, dispatch, ctx)
    }
}

/// `FACE_OUTER_BOUND('label', #edge_loop, orientation)`. Same fields
/// as `FACE_BOUND`; differs only in flagging the bound as outer.
pub struct FaceOuterBoundHandler;
/// Static binding consumed by [`register`].
pub static FACE_OUTER_BOUND_HANDLER: FaceOuterBoundHandler = FaceOuterBoundHandler;

impl EntityHandler for FaceOuterBoundHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::FACE_OUTER_BOUND]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        handle_face_bound(true, instance, record, registry, dispatch, ctx)
    }
}

fn handle_face_bound(
    is_outer: bool,
    instance: u64,
    record: &Record,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> HandlerOutcome {
    let entity_name = if is_outer {
        names::FACE_OUTER_BOUND
    } else {
        names::FACE_BOUND
    };
    let fields = match params::record_fields(&record.parameter, entity_name, instance) {
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
            entity_name,
            instance,
            "expected (label, bound, orientation)",
        );
    }
    let loop_ref = match params::as_entity_ref(&fields[1], entity_name, instance, "bound") {
        Ok(r) => r,
        Err(e) => {
            ctx.report.push_warning(e.into_warning());
            return HandlerOutcome::Failed {
                message: "bad bound ref".into(),
            };
        }
    };
    let forward = match params::as_bool(&fields[2], entity_name, instance, "orientation") {
        Ok(Some(b)) => b,
        Ok(None) => {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: entity_name.into(),
                instance: Some(instance),
                message: "orientation was .U.; defaulting to .T.".into(),
            });
            true
        }
        Err(e) => {
            ctx.report.push_warning(e.into_warning());
            return HandlerOutcome::Failed {
                message: "bad orientation".into(),
            };
        }
    };

    let loop_id = match resolve_loop(loop_ref, registry, dispatch, ctx) {
        Some(l) => l,
        None => {
            return HandlerOutcome::Failed {
                message: "bound unresolved".into(),
            }
        }
    };
    ctx.caches
        .face_bounds
        .insert(instance, (loop_id, is_outer, forward));
    HandlerOutcome::Resolved
}

// =========================================================================
// ADVANCED_FACE
// =========================================================================

/// `ADVANCED_FACE('label', (bound_refs), #face_geometry, same_sense)`.
///
/// Each `bound_ref` resolves to a `(LoopId, is_outer, forward)`
/// triple. Exactly one bound should be `is_outer=true`. Inner bounds
/// become the face's `inner_loops` list and are retagged as
/// `LoopType::Inner`; the outer is retagged `LoopType::Outer`.
///
/// `same_sense` maps to [`FaceOrientation::Forward`] / `Backward`.
pub struct AdvancedFaceHandler;
/// Static binding consumed by [`register`].
pub static ADVANCED_FACE_HANDLER: AdvancedFaceHandler = AdvancedFaceHandler;

impl EntityHandler for AdvancedFaceHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::ADVANCED_FACE]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::ADVANCED_FACE, instance)
        {
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
                names::ADVANCED_FACE,
                instance,
                "expected (label, (bounds), face_geometry, same_sense)",
            );
        }
        let bound_refs = match params::as_entity_ref_list(
            &fields[1],
            names::ADVANCED_FACE,
            instance,
            "bounds",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad bounds list".into(),
                };
            }
        };
        let surf_ref = match params::as_entity_ref(
            &fields[2],
            names::ADVANCED_FACE,
            instance,
            "face_geometry",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad face_geometry ref".into(),
                };
            }
        };
        let same_sense =
            match params::as_bool(&fields[3], names::ADVANCED_FACE, instance, "same_sense") {
                Ok(Some(b)) => b,
                Ok(None) => {
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: names::ADVANCED_FACE.into(),
                        instance: Some(instance),
                        message: "same_sense was .U.; defaulting to .T.".into(),
                    });
                    true
                }
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad same_sense".into(),
                    };
                }
            };

        // Resolve the face surface. Tier-1 covers PLANE / CYLINDRICAL;
        // tier-2 adds the spherical / toroidal / conical analytic family,
        // (rational) NURBS surfaces, and the swept surfaces (revolution /
        // linear extrusion). Complex (rational) B-spline surfaces resolve
        // through the dispatcher's complex path inside `ensure_resolved`.
        let _ = ensure_resolved(
            surf_ref,
            &[
                "PLANE",
                "CYLINDRICAL_SURFACE",
                "SPHERICAL_SURFACE",
                "TOROIDAL_SURFACE",
                "CONICAL_SURFACE",
                "B_SPLINE_SURFACE_WITH_KNOTS",
                "BOUNDED_SURFACE",
                "B_SPLINE_SURFACE",
                "RATIONAL_B_SPLINE_SURFACE",
                "SURFACE",
                "SURFACE_OF_REVOLUTION",
                "SURFACE_OF_LINEAR_EXTRUSION",
            ],
            registry,
            dispatch,
            ctx,
        );
        let surface_id = match ctx.caches.surfaces.get(&surf_ref).copied() {
            Some(s) => s,
            None => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: names::ADVANCED_FACE.into(),
                    instance: Some(instance),
                    message: format!(
                        "face_geometry #{surf_ref} did not resolve to a supported surface"
                    ),
                });
                return HandlerOutcome::Failed {
                    message: "surface unresolved".into(),
                };
            }
        };

        let mut bounds: Vec<(LoopId, bool, bool)> = Vec::with_capacity(bound_refs.len());
        for fb_ref in bound_refs.iter().copied() {
            match resolve_face_bound(fb_ref, registry, dispatch, ctx) {
                Some(t) => bounds.push(t),
                None => {
                    return HandlerOutcome::Failed {
                        message: format!("face_bound #{fb_ref} unresolved"),
                    };
                }
            }
        }

        // A FACE_BOUND with orientation .F. is walked REVERSED (ISO 10303-42
        // face_bound). The kernel keeps every loop so the face lies to its
        // left seen from the face's OUTWARD side (surface normal x the face's
        // sense) -- the convention the meshers and the certificate read --
        // and that is ISO's: measured on 6,292 planar outer loops of OCCT
        // files, a loop taken reversed exactly when its bound is .F. always
        // winds counter-clockwise about the face's outward normal. Ignoring
        // the flag left 2,815 of them backwards.
        for (k, &(loop_id, _, forward)) in bounds.iter().enumerate() {
            if !forward {
                if let Some(lp) = ctx.model.loops.get_mut(loop_id) {
                    lp.reverse();
                }
                ctx.caches.record_lens(
                    instance,
                    geometry_engine::lens::Stage::Import,
                    geometry_engine::lens::EventKind::LoopReversed,
                    bound_refs.get(k).copied(),
                    "FACE_BOUND marked .F.: walked reversed (ISO 10303-42)",
                    &[],
                );
            }
        }
        if ctx.caches.rescaled_surfaces.contains(&surf_ref) {
            ctx.caches.record_lens(
                instance,
                geometry_engine::lens::Stage::Import,
                geometry_engine::lens::EventKind::ProfileRescaled,
                Some(surf_ref),
                "swept-surface profile put on [0, 1] (exact B-spline knot rescale)",
                &[],
            );
        }

        // Find outer + collect inners.
        let mut outer: Option<LoopId> = None;
        let mut inners: Vec<LoopId> = Vec::new();
        for (loop_id, is_outer, _forward) in bounds.iter().copied() {
            if is_outer {
                if outer.is_some() {
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: names::ADVANCED_FACE.into(),
                        instance: Some(instance),
                        message: "more than one FACE_OUTER_BOUND; keeping the first".into(),
                    });
                } else {
                    outer = Some(loop_id);
                }
            } else {
                inners.push(loop_id);
            }
        }
        // Fallback: if no FACE_OUTER_BOUND was supplied, treat the
        // first FACE_BOUND as outer. STEP doesn't forbid omitting the
        // outer flag, and downstream geometry assumes one outer loop.
        let outer = match outer {
            Some(o) => o,
            None => match enclosing_loop(ctx.model, &bounds)
                .or_else(|| bounds.first().map(|(l, _, _)| *l))
            {
                Some(o) => {
                    if let Some(k) = bounds
                        .iter()
                        .position(|(l, _, _)| *l == o)
                        .filter(|&k| k > 0)
                    {
                        ctx.caches.record_lens(
                            instance,
                            geometry_engine::lens::Stage::Import,
                            geometry_engine::lens::EventKind::OuterLoopChosen,
                            bound_refs.get(k).copied(),
                            "no FACE_OUTER_BOUND: the loop enclosing the others is the outer one",
                            &[("bound_position", k as f64)],
                        );
                    }
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: names::ADVANCED_FACE.into(),
                        instance: Some(instance),
                        message:
                            "no FACE_OUTER_BOUND; the loop enclosing the others is the outer one"
                                .into(),
                    });
                    // Re-classify: the promoted first must not also
                    // appear in inners.
                    inners.retain(|l| *l != o);
                    o
                }
                None => {
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: names::ADVANCED_FACE.into(),
                        instance: Some(instance),
                        message: "no bounds supplied".into(),
                    });
                    return HandlerOutcome::Failed {
                        message: "no bounds".into(),
                    };
                }
            },
        };

        // Retag loop types on the kernel side now that we know who's
        // outer and who's inner.
        if let Some(lp) = ctx.model.loops.get_mut(outer) {
            lp.loop_type = LoopType::Outer;
        }
        for &inner in &inners {
            if let Some(lp) = ctx.model.loops.get_mut(inner) {
                lp.loop_type = LoopType::Inner;
            }
        }

        let orientation = if same_sense {
            FaceOrientation::Forward
        } else {
            FaceOrientation::Backward
        };
        let mut face = Face::new(0, surface_id, outer, orientation);
        for inner in inners {
            face.add_inner_loop(inner);
        }
        let face_id = ctx.model.faces.add(face);
        ctx.caches.faces.insert(instance, face_id);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// CLOSED_SHELL
// =========================================================================

/// `CLOSED_SHELL('label', (face_refs))`.
///
/// Allocates a kernel `Shell` with `ShellType::Closed`, populates its
/// faces, then runs [`super::manifold::validate_closed_shell`]. Any
/// non-manifold pattern surfaces as a `ManifoldWarning` in the report.
pub struct ClosedShellHandler;
/// Static binding consumed by [`register`].
pub static CLOSED_SHELL_HANDLER: ClosedShellHandler = ClosedShellHandler;

impl EntityHandler for ClosedShellHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::CLOSED_SHELL]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, names::CLOSED_SHELL, instance) {
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
                names::CLOSED_SHELL,
                instance,
                "expected (label, (faces))",
            );
        }
        let face_refs = match params::as_entity_ref_list(
            &fields[1],
            names::CLOSED_SHELL,
            instance,
            "cfs_faces",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad face list".into(),
                };
            }
        };
        if face_refs.is_empty() {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::CLOSED_SHELL.into(),
                instance: Some(instance),
                message: "empty face list".into(),
            });
            return HandlerOutcome::Failed {
                message: "empty face list".into(),
            };
        }

        let mut face_ids = Vec::with_capacity(face_refs.len());
        for f_ref in face_refs.iter().copied() {
            match resolve_face(f_ref, registry, dispatch, ctx) {
                Some(fid) => face_ids.push(fid),
                None => {
                    return HandlerOutcome::Failed {
                        message: format!("face #{f_ref} unresolved"),
                    };
                }
            }
        }

        let mut shell = Shell::with_capacity(0, ShellType::Closed, face_ids.len());
        shell.add_faces(&face_ids);
        let shell_id = ctx.model.shells.add(shell);
        ctx.caches.shells.insert(instance, shell_id);

        // Manifold validation — non-fatal but surfaced as warnings.
        if let Some(report) = validate_closed_shell(ctx.model, shell_id) {
            emit_manifold_warnings(instance, &report, ctx);
        }

        HandlerOutcome::Resolved
    }
}

// =========================================================================
// MANIFOLD_SOLID_BREP
// =========================================================================

/// `MANIFOLD_SOLID_BREP('label', #closed_shell)`.
///
/// Allocates a kernel `Solid` whose outer shell is the resolved
/// `CLOSED_SHELL`. Inner shells (voids) are tier-3 (`BREP_WITH_VOIDS`)
/// and not handled here.
pub struct ManifoldSolidBrepHandler;
/// Static binding consumed by [`register`].
pub static MANIFOLD_SOLID_BREP_HANDLER: ManifoldSolidBrepHandler = ManifoldSolidBrepHandler;

impl EntityHandler for ManifoldSolidBrepHandler {
    fn names(&self) -> &'static [&'static str] {
        &[names::MANIFOLD_SOLID_BREP]
    }
    fn phase(&self) -> Phase {
        Phase::Topology
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
            match params::record_fields(&record.parameter, names::MANIFOLD_SOLID_BREP, instance) {
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
                names::MANIFOLD_SOLID_BREP,
                instance,
                "expected (label, outer)",
            );
        }
        let shell_ref = match params::as_entity_ref(
            &fields[1],
            names::MANIFOLD_SOLID_BREP,
            instance,
            "outer",
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad outer ref".into(),
                };
            }
        };
        let shell_id = match resolve_shell(shell_ref, registry, dispatch, ctx) {
            Some(s) => s,
            None => {
                return HandlerOutcome::Failed {
                    message: "outer shell unresolved".into(),
                }
            }
        };
        let solid = Solid::new(0, shell_id);
        let solid_id = ctx.model.solids.add(solid);
        ctx.caches.solids.insert(instance, solid_id);
        HandlerOutcome::Resolved
    }
}

// =========================================================================
// Shared helpers
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

/// Force `instance` to resolve as a VERTEX_POINT, returning the
/// cached `VertexId`.
fn resolve_vertex(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<VertexId> {
    if let Some(v) = ctx.caches.vertices.get(&instance) {
        return Some(*v);
    }
    let _ = ensure_resolved(instance, &[names::VERTEX_POINT], registry, dispatch, ctx);
    ctx.caches.vertices.get(&instance).copied()
}

/// Force `instance` to resolve as an EDGE_CURVE, returning the
/// cached `EdgeId`.
fn resolve_edge(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<EdgeId> {
    if let Some(e) = ctx.caches.edges.get(&instance) {
        return Some(*e);
    }
    let _ = ensure_resolved(instance, &[names::EDGE_CURVE], registry, dispatch, ctx);
    ctx.caches.edges.get(&instance).copied()
}

/// Force `instance` to resolve as an ORIENTED_EDGE, returning the
/// `(EdgeId, forward)` pair.
fn resolve_oriented_edge(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<(EdgeId, bool)> {
    if let Some(p) = ctx.caches.oriented_edges.get(&instance) {
        return Some(*p);
    }
    let _ = ensure_resolved(instance, &[names::ORIENTED_EDGE], registry, dispatch, ctx);
    ctx.caches.oriented_edges.get(&instance).copied()
}

/// Force `instance` to resolve as an EDGE_LOOP, returning the
/// cached `LoopId`.
fn resolve_loop(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<LoopId> {
    if let Some(l) = ctx.caches.loops.get(&instance) {
        return Some(*l);
    }
    let _ = ensure_resolved(instance, &[names::EDGE_LOOP], registry, dispatch, ctx);
    ctx.caches.loops.get(&instance).copied()
}

/// Force `instance` to resolve as either `FACE_BOUND` or
/// `FACE_OUTER_BOUND`, returning the `(LoopId, is_outer, forward)`
/// triple.
fn resolve_face_bound(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<(LoopId, bool, bool)> {
    if let Some(t) = ctx.caches.face_bounds.get(&instance) {
        return Some(*t);
    }
    let _ = ensure_resolved(
        instance,
        &[names::FACE_BOUND, names::FACE_OUTER_BOUND],
        registry,
        dispatch,
        ctx,
    );
    ctx.caches.face_bounds.get(&instance).copied()
}

/// Force `instance` to resolve as an ADVANCED_FACE.
pub(crate) fn resolve_face(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<geometry_engine::primitives::face::FaceId> {
    if let Some(f) = ctx.caches.faces.get(&instance) {
        return Some(*f);
    }
    let _ = ensure_resolved(instance, &[names::ADVANCED_FACE], registry, dispatch, ctx);
    ctx.caches.faces.get(&instance).copied()
}

/// Force `instance` to resolve as a CLOSED_SHELL or OPEN_SHELL.
pub(crate) fn resolve_shell(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<ShellId> {
    if let Some(s) = ctx.caches.shells.get(&instance) {
        return Some(*s);
    }
    let _ = ensure_resolved(
        instance,
        &[names::CLOSED_SHELL, "OPEN_SHELL"],
        registry,
        dispatch,
        ctx,
    );
    ctx.caches.shells.get(&instance).copied()
}

/// Register every topology-phase handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&EDGE_CURVE_HANDLER);
    dispatch.register(&ORIENTED_EDGE_HANDLER);
    dispatch.register(&EDGE_LOOP_HANDLER);
    dispatch.register(&FACE_BOUND_HANDLER);
    dispatch.register(&FACE_OUTER_BOUND_HANDLER);
    dispatch.register(&ADVANCED_FACE_HANDLER);
    dispatch.register(&CLOSED_SHELL_HANDLER);
    dispatch.register(&MANIFOLD_SOLID_BREP_HANDLER);
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::{
        context::{ImportContext, ResolutionCaches, UnitScale},
        diagnostics::{HealingKind, ImportReport, ManifoldKind},
        dispatch::EntityDispatch,
        parser::parse_step,
        registry::EntityRegistry,
    };
    use geometry_engine::primitives::topology_builder::BRepModel;

    /// A vertex's distance off its edge's curve is bounded twice: by half
    /// the edge's length (the two ends' tolerance balls must not meet) AND
    /// by a fixed cap in model units, so a long edge cannot absorb a large
    /// gap. The largest gap on the valid CADGenBench parts is 4.7e-2.
    #[test]
    fn a_vertex_off_its_curve_is_bounded_by_the_edge_and_by_a_fixed_cap() {
        // Short edge: the share rule binds (9e-3 long -> 4.5e-3).
        assert!((vertex_off_curve_allowance(9.0e-3) - 4.5e-3).abs() < 1e-12);
        // Very short edge: never below the on-curve tolerance.
        assert!((vertex_off_curve_allowance(1.0e-4) - VERTEX_ON_CURVE).abs() < 1e-12);
        // The corpus's largest real gap, on a long edge, is accepted ...
        assert!(vertex_off_curve_allowance(10.0) >= 4.7e-2);
        // ... but a long edge does not accept more than the fixed cap.
        assert!(
            vertex_off_curve_allowance(10.0) <= VERTEX_OFF_CURVE_CAP,
            "a 10 mm edge allowed {} off its curve",
            vertex_off_curve_allowance(10.0)
        );
        assert!(vertex_off_curve_allowance(1000.0) <= VERTEX_OFF_CURVE_CAP);
        assert!(0.06 > vertex_off_curve_allowance(1000.0));
    }

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

    /// Drive the full tier-1 pipeline (units → geometry → topology)
    /// against `body` and return (model, report, caches).
    fn run(body: &str) -> (BRepModel, ImportReport, ResolutionCaches) {
        let src = wrap(body);
        let ex = parse_step(&src, "test").expect("parse");
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        super::super::register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        ctx.unit = UnitScale::default();
        let _ = dispatch.run_all(&reg, &mut ctx);
        let caches = std::mem::take(&mut ctx.caches);
        (model, report, caches)
    }

    /// A complete unit cube — 8 corners, 12 edges, 6 planar faces,
    /// 1 closed shell, 1 solid. Many tests reuse this builder.
    fn unit_cube_body() -> String {
        // Points: corners labelled by (x,y,z) bits.
        // 1=000 2=100 3=110 4=010 5=001 6=101 7=111 8=011
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        s += "#2=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#3=CARTESIAN_POINT('',(1.,1.,0.));";
        s += "#4=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#5=CARTESIAN_POINT('',(0.,0.,1.));";
        s += "#6=CARTESIAN_POINT('',(1.,0.,1.));";
        s += "#7=CARTESIAN_POINT('',(1.,1.,1.));";
        s += "#8=CARTESIAN_POINT('',(0.,1.,1.));";
        // Vertices.
        s += "#11=VERTEX_POINT('',#1);";
        s += "#12=VERTEX_POINT('',#2);";
        s += "#13=VERTEX_POINT('',#3);";
        s += "#14=VERTEX_POINT('',#4);";
        s += "#15=VERTEX_POINT('',#5);";
        s += "#16=VERTEX_POINT('',#6);";
        s += "#17=VERTEX_POINT('',#7);";
        s += "#18=VERTEX_POINT('',#8);";
        // Directions for placements + lines.
        s += "#21=DIRECTION('',(1.,0.,0.));";
        s += "#22=DIRECTION('',(0.,1.,0.));";
        s += "#23=DIRECTION('',(0.,0.,1.));";
        s += "#24=DIRECTION('',(-1.,0.,0.));";
        s += "#25=DIRECTION('',(0.,-1.,0.));";
        s += "#26=DIRECTION('',(0.,0.,-1.));";
        // Vectors with unit magnitude — shared per direction.
        s += "#31=VECTOR('',#21,1.);";
        s += "#32=VECTOR('',#22,1.);";
        s += "#33=VECTOR('',#23,1.);";
        // STEP LINEs — one per direction (shared by parallel edges).
        s += "#41=LINE('',#1,#31);"; // along +X from origin (parametric form only)
        s += "#42=LINE('',#1,#32);"; // along +Y
        s += "#43=LINE('',#1,#33);"; // along +Z
                                     // EDGE_CURVEs — bottom square z=0.
        s += "#51=EDGE_CURVE('',#11,#12,#41,.T.);"; // (0,0,0)→(1,0,0)
        s += "#52=EDGE_CURVE('',#12,#13,#42,.T.);"; // (1,0,0)→(1,1,0)
        s += "#53=EDGE_CURVE('',#14,#13,#41,.T.);"; // (0,1,0)→(1,1,0)
        s += "#54=EDGE_CURVE('',#11,#14,#42,.T.);"; // (0,0,0)→(0,1,0)
                                                    // Top square z=1.
        s += "#55=EDGE_CURVE('',#15,#16,#41,.T.);";
        s += "#56=EDGE_CURVE('',#16,#17,#42,.T.);";
        s += "#57=EDGE_CURVE('',#18,#17,#41,.T.);";
        s += "#58=EDGE_CURVE('',#15,#18,#42,.T.);";
        // Vertical edges.
        s += "#59=EDGE_CURVE('',#11,#15,#43,.T.);";
        s += "#60=EDGE_CURVE('',#12,#16,#43,.T.);";
        s += "#61=EDGE_CURVE('',#13,#17,#43,.T.);";
        s += "#62=EDGE_CURVE('',#14,#18,#43,.T.);";
        // Oriented edges for the six face loops.
        //
        // MANIFOLD WINDING INVARIANT: every EDGE_CURVE is shared by exactly
        // two faces, and for a closed solid with consistent outward normals
        // the two faces must traverse that shared edge in OPPOSITE
        // directions (one .T., one .F.). The manifold checker
        // (`validate_closed_shell`) tallies fwd/bwd uses per edge and flags
        // any edge used twice the same way as `OrientationMismatch`. The
        // cycles below are all CCW viewed from OUTSIDE the cube, which makes
        // every shared edge once-forward / once-backward by construction.
        // (A previous version of this fixture wound several faces
        // inconsistently — 6 of 12 edges came out fwd-twice/bwd-twice — so
        // the cube tripped 6 orientation mismatches.)
        //
        // Bottom face (z=0, outward normal -Z), CCW from below: 11→14→13→12.
        s += "#71=ORIENTED_EDGE('',*,*,#54,.T.);"; // 11→14
        s += "#72=ORIENTED_EDGE('',*,*,#53,.T.);"; // 14→13
        s += "#73=ORIENTED_EDGE('',*,*,#52,.F.);"; // 13→12
        s += "#74=ORIENTED_EDGE('',*,*,#51,.F.);"; // 12→11
                                                   // Top face (z=1, +Z), CCW from above: 15→16→17→18.
        s += "#75=ORIENTED_EDGE('',*,*,#55,.T.);"; // 15→16
        s += "#76=ORIENTED_EDGE('',*,*,#56,.T.);"; // 16→17
        s += "#77=ORIENTED_EDGE('',*,*,#57,.F.);"; // 17→18
        s += "#78=ORIENTED_EDGE('',*,*,#58,.F.);"; // 18→15
                                                   // Front face (y=0, -Y), CCW from front: 11→12→16→15.
        s += "#79=ORIENTED_EDGE('',*,*,#51,.T.);"; // 11→12
        s += "#80=ORIENTED_EDGE('',*,*,#60,.T.);"; // 12→16
        s += "#81=ORIENTED_EDGE('',*,*,#55,.F.);"; // 16→15
        s += "#82=ORIENTED_EDGE('',*,*,#59,.F.);"; // 15→11
                                                   // Right face (x=1, +X), CCW from the right: 12→13→17→16.
        s += "#83=ORIENTED_EDGE('',*,*,#52,.T.);"; // 12→13
        s += "#84=ORIENTED_EDGE('',*,*,#61,.T.);"; // 13→17
        s += "#85=ORIENTED_EDGE('',*,*,#56,.F.);"; // 17→16
        s += "#86=ORIENTED_EDGE('',*,*,#60,.F.);"; // 16→12
                                                   // Back face (y=1, +Y), CCW from behind: 13→14→18→17.
        s += "#87=ORIENTED_EDGE('',*,*,#53,.F.);"; // 13→14
        s += "#88=ORIENTED_EDGE('',*,*,#62,.T.);"; // 14→18
        s += "#89=ORIENTED_EDGE('',*,*,#57,.T.);"; // 18→17
        s += "#90=ORIENTED_EDGE('',*,*,#61,.F.);"; // 17→13
                                                   // Left face (x=0, -X), CCW from the left: 14→11→15→18.
        s += "#91=ORIENTED_EDGE('',*,*,#54,.F.);"; // 14→11
        s += "#92=ORIENTED_EDGE('',*,*,#59,.T.);"; // 11→15
        s += "#93=ORIENTED_EDGE('',*,*,#58,.T.);"; // 15→18
        s += "#94=ORIENTED_EDGE('',*,*,#62,.F.);"; // 18→14
                                                   // Loops.
        s += "#101=EDGE_LOOP('',(#71,#72,#73,#74));";
        s += "#102=EDGE_LOOP('',(#75,#76,#77,#78));";
        s += "#103=EDGE_LOOP('',(#79,#80,#81,#82));";
        s += "#104=EDGE_LOOP('',(#83,#84,#85,#86));";
        s += "#105=EDGE_LOOP('',(#87,#88,#89,#90));";
        s += "#106=EDGE_LOOP('',(#91,#92,#93,#94));";
        // Outer bounds.
        s += "#111=FACE_OUTER_BOUND('',#101,.T.);";
        s += "#112=FACE_OUTER_BOUND('',#102,.T.);";
        s += "#113=FACE_OUTER_BOUND('',#103,.T.);";
        s += "#114=FACE_OUTER_BOUND('',#104,.T.);";
        s += "#115=FACE_OUTER_BOUND('',#105,.T.);";
        s += "#116=FACE_OUTER_BOUND('',#106,.T.);";
        // Face placements — origin + normal + ref_x.
        s += "#121=AXIS2_PLACEMENT_3D('',#1,#26,#21);"; // bottom: normal -Z
        s += "#122=AXIS2_PLACEMENT_3D('',#5,#23,#21);"; // top: normal +Z
        s += "#123=AXIS2_PLACEMENT_3D('',#1,#25,#21);"; // front: normal -Y
        s += "#124=AXIS2_PLACEMENT_3D('',#2,#21,#22);"; // right: normal +X
        s += "#125=AXIS2_PLACEMENT_3D('',#4,#22,#21);"; // back: normal +Y
        s += "#126=AXIS2_PLACEMENT_3D('',#1,#24,#22);"; // left: normal -X
                                                        // Planes.
        s += "#131=PLANE('',#121);";
        s += "#132=PLANE('',#122);";
        s += "#133=PLANE('',#123);";
        s += "#134=PLANE('',#124);";
        s += "#135=PLANE('',#125);";
        s += "#136=PLANE('',#126);";
        // Advanced faces.
        s += "#141=ADVANCED_FACE('',(#111),#131,.T.);";
        s += "#142=ADVANCED_FACE('',(#112),#132,.T.);";
        s += "#143=ADVANCED_FACE('',(#113),#133,.T.);";
        s += "#144=ADVANCED_FACE('',(#114),#134,.T.);";
        s += "#145=ADVANCED_FACE('',(#115),#135,.T.);";
        s += "#146=ADVANCED_FACE('',(#116),#136,.T.);";
        // Shell + solid.
        s += "#151=CLOSED_SHELL('',(#141,#142,#143,#144,#145,#146));";
        s += "#161=MANIFOLD_SOLID_BREP('',#151);";
        s
    }

    // ------- EDGE_CURVE -------

    #[test]
    fn edge_curve_line_happy_path() {
        let (model, _r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #21=DIRECTION('',(1.,0.,0.));\
             #31=VECTOR('',#21,1.);\
             #41=LINE('',#1,#31);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);");
        let eid = c.edges.get(&51).copied().expect("edge cached");
        let edge = model.edges.get(eid).expect("edge stored");
        assert_eq!(edge.orientation, EdgeOrientation::Forward);
    }

    #[test]
    fn edge_curve_reverse_sense_yields_backward_orientation() {
        let (model, _r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #21=DIRECTION('',(1.,0.,0.));\
             #31=VECTOR('',#21,1.);\
             #41=LINE('',#1,#31);\
             #51=EDGE_CURVE('',#11,#12,#41,.F.);");
        let eid = c.edges.get(&51).unwrap();
        let edge = model.edges.get(*eid).unwrap();
        assert_eq!(edge.orientation, EdgeOrientation::Backward);
    }

    #[test]
    fn edge_curve_wrong_arity_warns() {
        let (_m, r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #51=EDGE_CURVE('',#11);");
        assert!(!c.edges.contains_key(&51));
        assert!(r.warnings.iter().any(|w| w.entity == "EDGE_CURVE"));
    }

    #[test]
    fn edge_curve_unknown_geometry_warns() {
        let (_m, r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #41=CARTESIAN_POINT('',(0.,0.,0.));\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);");
        assert!(!c.edges.contains_key(&51));
        assert!(r.warnings.iter().any(|w| w.entity == "EDGE_CURVE"));
    }

    #[test]
    fn edge_curve_seam_circle_is_full_circle() {
        // Start == end → full circle.
        let (model, _r, c) = run("#1=CARTESIAN_POINT('',(1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #2=CARTESIAN_POINT('',(0.,0.,0.));\
             #21=DIRECTION('',(0.,0.,1.));\
             #22=DIRECTION('',(1.,0.,0.));\
             #31=AXIS2_PLACEMENT_3D('',#2,#21,#22);\
             #41=CIRCLE('',#31,1.);\
             #51=EDGE_CURVE('',#11,#11,#41,.T.);");
        let eid = c.edges.get(&51).expect("seam edge cached");
        let edge = model.edges.get(*eid).expect("seam edge stored");
        // The kernel curve should evaluate to vertex pos at t=0 *and* t=1.
        let curve_box = model.curves.get(edge.curve_id).expect("curve stored");
        let p0 = curve_box.evaluate(0.0).unwrap().position;
        let p1 = curve_box.evaluate(1.0).unwrap().position;
        assert!(((p0.x - 1.0).abs() < 1e-9) && ((p1.x - 1.0).abs() < 1e-9));
    }

    /// A full-circle seam edge starts AND ends at its vertex, wherever on the
    /// circle the vertex sits, in either sense. The circle used to start at
    /// the kernel's own x-axis: a rim whose seam vertex sat elsewhere was
    /// sampled from the wrong point, the face outline then ran back over
    /// itself and the face was refused (48 cylinder walls of one
    /// CADGenBench part fell back to an untrimmed grid and cracked the mesh).
    #[test]
    fn edge_curve_seam_circle_starts_at_its_vertex() {
        for sense in [".T.", ".F."] {
            let (model, _r, c) = run(&format!(
                "#1=CARTESIAN_POINT('',(-0.6,0.8,5.));\
                 #11=VERTEX_POINT('',#1);\
                 #2=CARTESIAN_POINT('',(0.,0.,5.));\
                 #21=DIRECTION('',(0.,0.,1.));\
                 #22=DIRECTION('',(1.,0.,0.));\
                 #31=AXIS2_PLACEMENT_3D('',#2,#21,#22);\
                 #41=CIRCLE('',#31,1.);\
                 #51=EDGE_CURVE('',#11,#11,#41,{sense});"
            ));
            let eid = *c.edges.get(&51).expect("seam edge cached");
            let v = [-0.6, 0.8, 5.0];
            assert!(
                near(edge_point(&model, eid, 0.0), v),
                "{sense}: starts at its vertex"
            );
            assert!(
                near(edge_point(&model, eid, 1.0), v),
                "{sense}: ends at its vertex"
            );
            // A quarter of the way round, in the edge's sense about +Z.
            let q = edge_point(&model, eid, 0.25);
            let turn = if sense == ".T." { 1.0 } else { -1.0 };
            let want = [-0.8 * turn, -0.6 * turn, 5.0];
            assert!(
                near(q, want),
                "{sense}: quarter turn at {q:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn edge_curve_half_arc_yields_pi_sweep() {
        // Start (1,0,0), end (-1,0,0) about (0,0,0) normal +Z → π sweep.
        let (model, _r, c) = run("#1=CARTESIAN_POINT('',(1.,0.,0.));\
             #2=CARTESIAN_POINT('',(-1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #3=CARTESIAN_POINT('',(0.,0.,0.));\
             #21=DIRECTION('',(0.,0.,1.));\
             #22=DIRECTION('',(1.,0.,0.));\
             #31=AXIS2_PLACEMENT_3D('',#3,#21,#22);\
             #41=CIRCLE('',#31,1.);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);");
        let eid = c.edges.get(&51).expect("arc edge cached");
        let edge = model.edges.get(*eid).unwrap();
        let curve_box = model.curves.get(edge.curve_id).unwrap();
        // Midpoint of half-arc should be (0, ±1, 0).
        let mid = curve_box.evaluate(0.5).unwrap().position;
        assert!((mid.x).abs() < 1e-9);
        assert!((mid.y.abs() - 1.0).abs() < 1e-9);
    }

    /// ELLIPSE edges (an oblique cut through a cylinder: angled holes,
    /// chamfered pins). The GrabCAD bench vice carries 77 of them; none were
    /// read, and the vice's main body never imported.
    const ELLIPSE_FRAME: &str = "#3=CARTESIAN_POINT('',(0.,0.,0.));\
         #21=DIRECTION('',(0.,0.,1.));\
         #22=DIRECTION('',(1.,0.,0.));\
         #31=AXIS2_PLACEMENT_3D('',#3,#21,#22);\
         #41=ELLIPSE('',#31,2.,1.);";

    /// Position on the edge at fraction `f` of its parameter range.
    fn edge_point(model: &BRepModel, eid: u32, f: f64) -> [f64; 3] {
        let edge = model.edges.get(eid).unwrap();
        let curve = model.curves.get(edge.curve_id).unwrap();
        let r = edge.param_range;
        let p = curve
            .evaluate(r.start + f * (r.end - r.start))
            .unwrap()
            .position;
        [p.x, p.y, p.z]
    }

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2]) < 1e-9
    }

    /// HYPERBOLA and PARABOLA edges: a plane cutting a cone parallel to its
    /// axis (or to a generator). Generated CAD is full of them (5 of the 81
    /// CADGenBench parts built no solid without them). The edge is an EXACT
    /// rational quadratic arc: every point satisfies the conic's equation.
    const CONIC_FRAME: &str = "#3=CARTESIAN_POINT('',(0.,0.,0.));\
         #21=DIRECTION('',(0.,0.,1.));\
         #22=DIRECTION('',(1.,0.,0.));\
         #31=AXIS2_PLACEMENT_3D('',#3,#21,#22);";

    fn conic_edge(curve: &str, a: [f64; 3], b: [f64; 3]) -> (BRepModel, u32) {
        let (model, _r, c) = run(&format!(
            "{CONIC_FRAME}{curve}\
             #1=CARTESIAN_POINT('',({:.15},{:.15},{:.15}));\
             #2=CARTESIAN_POINT('',({:.15},{:.15},{:.15}));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);",
            a[0], a[1], a[2], b[0], b[1], b[2]
        ));
        let eid = *c.edges.get(&51).expect("conic edge imported");
        (model, eid)
    }

    #[test]
    fn edge_curve_hyperbola_is_an_exact_arc() {
        // x = 2 cosh t, y = sinh t: x^2/4 - y^2 = 1, t from -1 to 1.5.
        let at = |t: f64| [2.0 * t.cosh(), t.sinh(), 0.0];
        let (model, eid) = conic_edge("#41=HYPERBOLA('',#31,2.,1.);", at(-1.0), at(1.5));
        assert!(
            near(edge_point(&model, eid, 0.0), at(-1.0)),
            "starts at its vertex"
        );
        assert!(
            near(edge_point(&model, eid, 1.0), at(1.5)),
            "ends at its vertex"
        );
        for k in 0..=20 {
            let p = edge_point(&model, eid, k as f64 / 20.0);
            let on = p[0] * p[0] / 4.0 - p[1] * p[1];
            assert!(
                (on - 1.0).abs() < 1e-9,
                "point {p:?} off the hyperbola ({on})"
            );
            assert!(
                p[0] > 0.0 && p[2].abs() < 1e-12,
                "on the right branch, in plane"
            );
        }
    }

    #[test]
    fn edge_curve_parabola_is_an_exact_arc() {
        // x = f t^2, y = 2 f t with f = 0.5: x = y^2 / (4 f), t from -1 to 2.
        let f = 0.5;
        let at = |t: f64| [f * t * t, 2.0 * f * t, 0.0];
        let (model, eid) = conic_edge("#41=PARABOLA('',#31,0.5);", at(-1.0), at(2.0));
        assert!(near(edge_point(&model, eid, 0.0), at(-1.0)));
        assert!(near(edge_point(&model, eid, 1.0), at(2.0)));
        for k in 0..=20 {
            let p = edge_point(&model, eid, k as f64 / 20.0);
            assert!(
                (p[0] - p[1] * p[1] / (4.0 * f)).abs() < 1e-9,
                "point {p:?} off the parabola"
            );
        }
    }

    #[test]
    fn edge_curve_ellipse_quarter_arc() {
        let (model, _r, c) = run(&format!(
            "{ELLIPSE_FRAME}\
             #1=CARTESIAN_POINT('',(2.,0.,0.));\
             #2=CARTESIAN_POINT('',(0.,1.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);"
        ));
        let eid = *c.edges.get(&51).expect("ellipse edge imported");
        assert!(near(edge_point(&model, eid, 0.0), [2.0, 0.0, 0.0]));
        assert!(near(edge_point(&model, eid, 1.0), [0.0, 1.0, 0.0]));
        // Half-way along the quarter arc: angle π/4 on x²/4 + y² = 1.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(near(edge_point(&model, eid, 0.5), [2.0 * h, h, 0.0]));
        let edge = model.edges.get(eid).unwrap();
        assert!(
            !model.curves.get(edge.curve_id).unwrap().is_closed(),
            "a quarter arc is open"
        );
    }

    #[test]
    fn edge_curve_ellipse_reversed_sense_goes_the_other_way() {
        // Same vertices, .F.: the edge runs the basis ellipse backwards from
        // (2,0,0) to (0,1,0) -- the 270° arc through (-√2, -√2/2).
        let (model, _r, c) = run(&format!(
            "{ELLIPSE_FRAME}\
             #1=CARTESIAN_POINT('',(2.,0.,0.));\
             #2=CARTESIAN_POINT('',(0.,1.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #51=EDGE_CURVE('',#11,#12,#41,.F.);"
        ));
        let eid = *c.edges.get(&51).expect("ellipse edge imported");
        assert!(near(edge_point(&model, eid, 0.0), [2.0, 0.0, 0.0]));
        assert!(near(edge_point(&model, eid, 1.0), [0.0, 1.0, 0.0]));
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(
            near(edge_point(&model, eid, 0.5), [-2.0 * h, -h, 0.0]),
            "mid {:?}",
            edge_point(&model, eid, 0.5)
        );
    }

    #[test]
    fn edge_curve_seam_ellipse_is_the_whole_ellipse() {
        let (model, _r, c) = run(&format!(
            "{ELLIPSE_FRAME}\
             #1=CARTESIAN_POINT('',(0.,1.,0.));\
             #11=VERTEX_POINT('',#1);\
             #51=EDGE_CURVE('',#11,#11,#41,.T.);"
        ));
        let eid = *c.edges.get(&51).expect("seam ellipse imported");
        assert!(near(edge_point(&model, eid, 0.0), [0.0, 1.0, 0.0]));
        assert!(near(edge_point(&model, eid, 1.0), [0.0, 1.0, 0.0]));
        assert!(near(edge_point(&model, eid, 0.5), [0.0, -1.0, 0.0]));
        let edge = model.edges.get(eid).unwrap();
        assert!(model.curves.get(edge.curve_id).unwrap().is_closed());
    }

    #[test]
    fn edge_curve_records_vertex_snap_when_curve_evals_off() {
        // Vertex at (0,0,0), but LINE template starts at (0.5,0,0) +X — the
        // curve we build still goes (0,0,0)→(1,0,0) (kernel curve is sized
        // from the *vertices*), so the snap deviation is zero. The handler
        // does not warn in the happy path.
        let (_m, r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #21=DIRECTION('',(1.,0.,0.));\
             #31=VECTOR('',#21,1.);\
             #41=LINE('',#1,#31);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);");
        assert!(c.edges.contains_key(&51));
        assert!(!r
            .healings
            .iter()
            .any(|h| matches!(h.kind, HealingKind::EdgeVertexSnap)));
    }

    // ------- ORIENTED_EDGE -------

    #[test]
    fn oriented_edge_happy_path() {
        let (_m, _r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #21=DIRECTION('',(1.,0.,0.));\
             #31=VECTOR('',#21,1.);\
             #41=LINE('',#1,#31);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);\
             #71=ORIENTED_EDGE('',*,*,#51,.T.);");
        let pair = c.oriented_edges.get(&71).expect("oriented edge cached");
        assert!(pair.1);
    }

    #[test]
    fn oriented_edge_reverse_caches_false() {
        let (_m, _r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #21=DIRECTION('',(1.,0.,0.));\
             #31=VECTOR('',#21,1.);\
             #41=LINE('',#1,#31);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);\
             #71=ORIENTED_EDGE('',*,*,#51,.F.);");
        let (_eid, forward) = c.oriented_edges.get(&71).unwrap();
        assert!(!*forward);
    }

    #[test]
    fn oriented_edge_wrong_arity_warns() {
        let (_m, r, c) = run("#71=ORIENTED_EDGE('',*,*);");
        assert!(!c.oriented_edges.contains_key(&71));
        assert!(r.warnings.iter().any(|w| w.entity == "ORIENTED_EDGE"));
    }

    // ------- EDGE_LOOP -------

    #[test]
    fn edge_loop_happy_path_closed_chain() {
        // Square loop in XY.
        let (model, _r, c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #3=CARTESIAN_POINT('',(1.,1.,0.));\
             #4=CARTESIAN_POINT('',(0.,1.,0.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #13=VERTEX_POINT('',#3);\
             #14=VERTEX_POINT('',#4);\
             #21=DIRECTION('',(1.,0.,0.));\
             #22=DIRECTION('',(0.,1.,0.));\
             #31=VECTOR('',#21,1.);\
             #32=VECTOR('',#22,1.);\
             #41=LINE('',#1,#31);\
             #42=LINE('',#1,#32);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);\
             #52=EDGE_CURVE('',#12,#13,#42,.T.);\
             #53=EDGE_CURVE('',#14,#13,#41,.T.);\
             #54=EDGE_CURVE('',#11,#14,#42,.T.);\
             #71=ORIENTED_EDGE('',*,*,#51,.T.);\
             #72=ORIENTED_EDGE('',*,*,#52,.T.);\
             #73=ORIENTED_EDGE('',*,*,#53,.F.);\
             #74=ORIENTED_EDGE('',*,*,#54,.F.);\
             #101=EDGE_LOOP('',(#71,#72,#73,#74));");
        let lid = c.loops.get(&101).expect("loop cached");
        let lp = model.loops.get(*lid).expect("loop stored");
        assert_eq!(lp.edges.len(), 4);
        assert_eq!(lp.orientations.len(), 4);
    }

    #[test]
    fn edge_loop_open_chain_emits_healing() {
        // 11→12 + 13→14: chain doesn't close.
        let (_m, r, _c) = run("#1=CARTESIAN_POINT('',(0.,0.,0.));\
             #2=CARTESIAN_POINT('',(1.,0.,0.));\
             #3=CARTESIAN_POINT('',(5.,5.,5.));\
             #4=CARTESIAN_POINT('',(6.,6.,6.));\
             #11=VERTEX_POINT('',#1);\
             #12=VERTEX_POINT('',#2);\
             #13=VERTEX_POINT('',#3);\
             #14=VERTEX_POINT('',#4);\
             #21=DIRECTION('',(1.,0.,0.));\
             #31=VECTOR('',#21,1.);\
             #41=LINE('',#1,#31);\
             #51=EDGE_CURVE('',#11,#12,#41,.T.);\
             #52=EDGE_CURVE('',#13,#14,#41,.T.);\
             #71=ORIENTED_EDGE('',*,*,#51,.T.);\
             #72=ORIENTED_EDGE('',*,*,#52,.T.);\
             #101=EDGE_LOOP('',(#71,#72));");
        assert!(r
            .healings
            .iter()
            .any(|h| matches!(h.kind, HealingKind::LoopNotClosed)));
    }

    #[test]
    fn edge_loop_empty_warns() {
        let (_m, r, c) = run("#101=EDGE_LOOP('',());");
        assert!(!c.loops.contains_key(&101));
        assert!(r.warnings.iter().any(|w| w.entity == "EDGE_LOOP"));
    }

    #[test]
    fn edge_loop_wrong_arity_warns() {
        let (_m, r, c) = run("#101=EDGE_LOOP('');");
        assert!(!c.loops.contains_key(&101));
        assert!(r.warnings.iter().any(|w| w.entity == "EDGE_LOOP"));
    }

    // ------- FACE_BOUND / FACE_OUTER_BOUND -------

    fn make_unit_square_loop_body() -> &'static str {
        "#1=CARTESIAN_POINT('',(0.,0.,0.));\
         #2=CARTESIAN_POINT('',(1.,0.,0.));\
         #3=CARTESIAN_POINT('',(1.,1.,0.));\
         #4=CARTESIAN_POINT('',(0.,1.,0.));\
         #11=VERTEX_POINT('',#1);\
         #12=VERTEX_POINT('',#2);\
         #13=VERTEX_POINT('',#3);\
         #14=VERTEX_POINT('',#4);\
         #21=DIRECTION('',(1.,0.,0.));\
         #22=DIRECTION('',(0.,1.,0.));\
         #31=VECTOR('',#21,1.);\
         #32=VECTOR('',#22,1.);\
         #41=LINE('',#1,#31);\
         #42=LINE('',#1,#32);\
         #51=EDGE_CURVE('',#11,#12,#41,.T.);\
         #52=EDGE_CURVE('',#12,#13,#42,.T.);\
         #53=EDGE_CURVE('',#14,#13,#41,.T.);\
         #54=EDGE_CURVE('',#11,#14,#42,.T.);\
         #71=ORIENTED_EDGE('',*,*,#51,.T.);\
         #72=ORIENTED_EDGE('',*,*,#52,.T.);\
         #73=ORIENTED_EDGE('',*,*,#53,.F.);\
         #74=ORIENTED_EDGE('',*,*,#54,.F.);\
         #101=EDGE_LOOP('',(#71,#72,#73,#74));"
    }

    #[test]
    fn face_bound_caches_inner_flag() {
        let body = format!(
            "{}#111=FACE_BOUND('',#101,.T.);",
            make_unit_square_loop_body()
        );
        let (_m, _r, c) = run(&body);
        let (_lid, is_outer, forward) = *c.face_bounds.get(&111).unwrap();
        assert!(!is_outer);
        assert!(forward);
    }

    #[test]
    fn face_outer_bound_caches_outer_flag() {
        let body = format!(
            "{}#111=FACE_OUTER_BOUND('',#101,.T.);",
            make_unit_square_loop_body()
        );
        let (_m, _r, c) = run(&body);
        let (_lid, is_outer, _f) = *c.face_bounds.get(&111).unwrap();
        assert!(is_outer);
    }

    #[test]
    fn face_outer_bound_reverse_orientation_caches_false() {
        let body = format!(
            "{}#111=FACE_OUTER_BOUND('',#101,.F.);",
            make_unit_square_loop_body()
        );
        let (_m, _r, c) = run(&body);
        let (_lid, _io, forward) = *c.face_bounds.get(&111).unwrap();
        assert!(!forward);
    }

    #[test]
    fn face_bound_wrong_arity_warns() {
        let (_m, r, c) = run("#111=FACE_BOUND('');");
        assert!(!c.face_bounds.contains_key(&111));
        assert!(r.warnings.iter().any(|w| w.entity == "FACE_BOUND"));
    }

    // ------- ADVANCED_FACE -------

    #[test]
    fn advanced_face_happy_path_single_bound() {
        let body = format!(
            "{}\
             #111=FACE_OUTER_BOUND('',#101,.T.);\
             #121=AXIS2_PLACEMENT_3D('',#1,#22,#21);\
             #131=PLANE('',#121);\
             #141=ADVANCED_FACE('',(#111),#131,.T.);",
            make_unit_square_loop_body()
        );
        let (model, _r, c) = run(&body);
        let fid = c.faces.get(&141).expect("face cached");
        let face = model.faces.get(*fid).expect("face stored");
        assert_eq!(face.inner_loops.len(), 0);
        assert_eq!(face.orientation, FaceOrientation::Forward);
    }

    #[test]
    fn advanced_face_missing_outer_bound_promotes_first_and_warns() {
        let body = format!(
            "{}\
             #111=FACE_BOUND('',#101,.T.);\
             #121=AXIS2_PLACEMENT_3D('',#1,#22,#21);\
             #131=PLANE('',#121);\
             #141=ADVANCED_FACE('',(#111),#131,.T.);",
            make_unit_square_loop_body()
        );
        let (model, r, c) = run(&body);
        let fid = c.faces.get(&141).unwrap();
        let face = model.faces.get(*fid).unwrap();
        assert_eq!(face.inner_loops.len(), 0);
        assert!(r
            .warnings
            .iter()
            .any(|w| w.entity == "ADVANCED_FACE" && w.message.contains("no FACE_OUTER_BOUND")));
    }

    /// With no FACE_OUTER_BOUND -- OCCT never writes one, and writes a face's
    /// bounds in no particular order -- the outer loop is the one that
    /// encloses the others, not whichever bound comes first. In one
    /// CADGenBench submission 227 faces in 49 parts list a hole first; taken
    /// as the outer loop it turned the face inside out (OCCT re-decides it
    /// geometrically, ShapeFix_Face::FixOrientation).
    #[test]
    fn advanced_face_without_an_outer_bound_takes_the_enclosing_loop() {
        let (model, r, c) = run("#1000=CARTESIAN_POINT('',(0.0,0.0,0.));#1010=VERTEX_POINT('',#1000);#1001=CARTESIAN_POINT('',(10.0,0.0,0.));#1011=VERTEX_POINT('',#1001);#1002=CARTESIAN_POINT('',(10.0,10.0,0.));#1012=VERTEX_POINT('',#1002);#1003=CARTESIAN_POINT('',(0.0,10.0,0.));#1013=VERTEX_POINT('',#1003);#1020=DIRECTION('',(1.0,0.0,0.));#1030=VECTOR('',#1020,10.0);#1040=LINE('',#1000,#1030);#1050=EDGE_CURVE('',#1010,#1011,#1040,.T.);#1060=ORIENTED_EDGE('',*,*,#1050,.T.);#1021=DIRECTION('',(0.0,1.0,0.));#1031=VECTOR('',#1021,10.0);#1041=LINE('',#1001,#1031);#1051=EDGE_CURVE('',#1011,#1012,#1041,.T.);#1061=ORIENTED_EDGE('',*,*,#1051,.T.);#1022=DIRECTION('',(-1.0,0.0,0.));#1032=VECTOR('',#1022,10.0);#1042=LINE('',#1002,#1032);#1052=EDGE_CURVE('',#1012,#1013,#1042,.T.);#1062=ORIENTED_EDGE('',*,*,#1052,.T.);#1023=DIRECTION('',(0.0,-1.0,0.));#1033=VECTOR('',#1023,10.0);#1043=LINE('',#1003,#1033);#1053=EDGE_CURVE('',#1013,#1010,#1043,.T.);#1063=ORIENTED_EDGE('',*,*,#1053,.T.);#1070=EDGE_LOOP('',(#1060,#1061,#1062,#1063));#2000=CARTESIAN_POINT('',(4.0,4.0,0.));#2010=VERTEX_POINT('',#2000);#2001=CARTESIAN_POINT('',(6.0,4.0,0.));#2011=VERTEX_POINT('',#2001);#2002=CARTESIAN_POINT('',(6.0,6.0,0.));#2012=VERTEX_POINT('',#2002);#2003=CARTESIAN_POINT('',(4.0,6.0,0.));#2013=VERTEX_POINT('',#2003);#2020=DIRECTION('',(1.0,0.0,0.));#2030=VECTOR('',#2020,2.0);#2040=LINE('',#2000,#2030);#2050=EDGE_CURVE('',#2010,#2011,#2040,.T.);#2060=ORIENTED_EDGE('',*,*,#2050,.T.);#2021=DIRECTION('',(0.0,1.0,0.));#2031=VECTOR('',#2021,2.0);#2041=LINE('',#2001,#2031);#2051=EDGE_CURVE('',#2011,#2012,#2041,.T.);#2061=ORIENTED_EDGE('',*,*,#2051,.T.);#2022=DIRECTION('',(-1.0,0.0,0.));#2032=VECTOR('',#2022,2.0);#2042=LINE('',#2002,#2032);#2052=EDGE_CURVE('',#2012,#2013,#2042,.T.);#2062=ORIENTED_EDGE('',*,*,#2052,.T.);#2023=DIRECTION('',(0.0,-1.0,0.));#2033=VECTOR('',#2023,2.0);#2043=LINE('',#2003,#2033);#2053=EDGE_CURVE('',#2013,#2010,#2043,.T.);#2063=ORIENTED_EDGE('',*,*,#2053,.T.);#2070=EDGE_LOOP('',(#2060,#2061,#2062,#2063));#3001=FACE_BOUND('',#2070,.F.);#3002=FACE_BOUND('',#1070,.T.);#3010=CARTESIAN_POINT('',(0.,0.,0.));#3011=DIRECTION('',(0.,0.,1.));#3012=DIRECTION('',(1.,0.,0.));#3013=AXIS2_PLACEMENT_3D('',#3010,#3011,#3012);#3014=PLANE('',#3013);#3020=ADVANCED_FACE('',(#3001,#3002),#3014,.T.);");
        let fid = *c
            .faces
            .get(&3020)
            .unwrap_or_else(|| panic!("face built: {:#?}", r.warnings));
        let face = model.faces.get(fid).expect("face");
        let outer = *c.loops.get(&1070).expect("plate loop cached");
        let hole = *c.loops.get(&2070).expect("hole loop cached");
        assert_eq!(
            face.outer_loop, outer,
            "the 10 x 10 plate's loop is the outer one"
        );
        assert_eq!(face.inner_loops, vec![hole], "the 2 x 2 loop is the hole");
    }

    /// A FACE_BOUND marked .F. is walked reversed, and only then: the kernel
    /// keeps a face's outer loop counter-clockwise seen from its OUTWARD side
    /// (surface normal x face sense), as ISO does. OCCT writes both cases
    /// below; measured on 6,292 planar outer loops of one CADGenBench
    /// submission, the loop reversed exactly when its bound is .F. always
    /// winds counter-clockwise about the face's outward normal. Ignoring the
    /// flag left 2,815 of those loops backwards.
    #[test]
    fn a_face_bound_marked_f_is_walked_reversed() {
        // `body` writes its square CLOCKWISE about +Z on a +Z plane.
        let body = "#4000=CARTESIAN_POINT('',(0.0,0.0,0.));#4010=VERTEX_POINT('',#4000);#4001=CARTESIAN_POINT('',(0.0,10.0,0.));#4011=VERTEX_POINT('',#4001);#4002=CARTESIAN_POINT('',(10.0,10.0,0.));#4012=VERTEX_POINT('',#4002);#4003=CARTESIAN_POINT('',(10.0,0.0,0.));#4013=VERTEX_POINT('',#4003);#4020=DIRECTION('',(0.0,1.0,0.));#4030=VECTOR('',#4020,10.0);#4040=LINE('',#4000,#4030);#4050=EDGE_CURVE('',#4010,#4011,#4040,.T.);#4060=ORIENTED_EDGE('',*,*,#4050,.T.);#4021=DIRECTION('',(1.0,0.0,0.));#4031=VECTOR('',#4021,10.0);#4041=LINE('',#4001,#4031);#4051=EDGE_CURVE('',#4011,#4012,#4041,.T.);#4061=ORIENTED_EDGE('',*,*,#4051,.T.);#4022=DIRECTION('',(0.0,-1.0,0.));#4032=VECTOR('',#4022,10.0);#4042=LINE('',#4002,#4032);#4052=EDGE_CURVE('',#4012,#4013,#4042,.T.);#4062=ORIENTED_EDGE('',*,*,#4052,.T.);#4023=DIRECTION('',(-1.0,0.0,0.));#4033=VECTOR('',#4023,10.0);#4043=LINE('',#4003,#4033);#4053=EDGE_CURVE('',#4013,#4010,#4043,.T.);#4063=ORIENTED_EDGE('',*,*,#4053,.T.);#4070=EDGE_LOOP('',(#4060,#4061,#4062,#4063));#5001=FACE_BOUND('',#4070,.T.);#5010=CARTESIAN_POINT('',(0.,0.,0.));#5011=DIRECTION('',(0.,0.,1.));#5012=DIRECTION('',(1.,0.,0.));#5013=AXIS2_PLACEMENT_3D('',#5010,#5011,#5012);#5014=PLANE('',#5013);#5020=ADVANCED_FACE('',(#5001),#5014,.F.);";
        let winding_about_face_normal = |bound: &str, sense: &str| -> f64 {
            let text = body
                .replace(
                    "#5001=FACE_BOUND('',#4070,.T.);",
                    &format!("#5001=FACE_BOUND('',#4070,{bound});"),
                )
                .replace(
                    "#5020=ADVANCED_FACE('',(#5001),#5014,.F.);",
                    &format!("#5020=ADVANCED_FACE('',(#5001),#5014,{sense});"),
                );
            let (model, r, c) = run(&text);
            let fid = *c
                .faces
                .get(&5020)
                .unwrap_or_else(|| panic!("face built: {:#?}", r.warnings));
            let face = model.faces.get(fid).expect("face");
            let lp = model.loops.get(face.outer_loop).expect("outer loop");
            let mut pts = Vec::new();
            for (k, &e) in lp.edges.iter().enumerate() {
                let ed = model.edges.get(e).expect("edge");
                let v = if lp.orientations[k] {
                    ed.start_vertex
                } else {
                    ed.end_vertex
                };
                pts.push(model.vertices.get_position(v).expect("vertex"));
            }
            let mut area_z = 0.0;
            for i in 0..pts.len() {
                let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
                area_z += p[0] * q[1] - p[1] * q[0];
            }
            // The face's outward normal is +Z for .T., -Z for .F.
            area_z * face.orientation.sign()
        };
        // Clockwise about +Z, face sense .F. (outward -Z), bound .T.: kept --
        // already counter-clockwise seen from outside (an Inventor cap screw).
        assert!(
            winding_about_face_normal(".T.", ".F.") > 0.0,
            "bound .T. must be kept as written"
        );
        // Clockwise about +Z, face sense .T. (outward +Z), bound .F.: reversed.
        assert!(
            winding_about_face_normal(".F.", ".T.") > 0.0,
            "bound .F. must be walked reversed"
        );
    }

    #[test]
    fn advanced_face_reverse_same_sense_yields_backward() {
        let body = format!(
            "{}\
             #111=FACE_OUTER_BOUND('',#101,.T.);\
             #121=AXIS2_PLACEMENT_3D('',#1,#22,#21);\
             #131=PLANE('',#121);\
             #141=ADVANCED_FACE('',(#111),#131,.F.);",
            make_unit_square_loop_body()
        );
        let (model, _r, c) = run(&body);
        let fid = c.faces.get(&141).unwrap();
        let face = model.faces.get(*fid).unwrap();
        assert_eq!(face.orientation, FaceOrientation::Backward);
    }

    #[test]
    fn advanced_face_unknown_surface_kind_warns() {
        let body = format!(
            "{}\
             #111=FACE_OUTER_BOUND('',#101,.T.);\
             #131=CARTESIAN_POINT('',(0.,0.,0.));\
             #141=ADVANCED_FACE('',(#111),#131,.T.);",
            make_unit_square_loop_body()
        );
        let (_m, r, c) = run(&body);
        assert!(!c.faces.contains_key(&141));
        assert!(r.warnings.iter().any(|w| w.entity == "ADVANCED_FACE"));
    }

    #[test]
    fn advanced_face_wrong_arity_warns() {
        let (_m, r, c) = run("#141=ADVANCED_FACE('');");
        assert!(!c.faces.contains_key(&141));
        assert!(r.warnings.iter().any(|w| w.entity == "ADVANCED_FACE"));
    }

    #[test]
    fn advanced_face_tags_outer_loop_type() {
        let body = format!(
            "{}\
             #111=FACE_OUTER_BOUND('',#101,.T.);\
             #121=AXIS2_PLACEMENT_3D('',#1,#22,#21);\
             #131=PLANE('',#121);\
             #141=ADVANCED_FACE('',(#111),#131,.T.);",
            make_unit_square_loop_body()
        );
        let (model, _r, c) = run(&body);
        let lid = *c.loops.get(&101).unwrap();
        let lp = model.loops.get(lid).unwrap();
        assert_eq!(lp.loop_type, LoopType::Outer);
    }

    // ------- CLOSED_SHELL + MANIFOLD_SOLID_BREP (unit cube) -------

    #[test]
    fn unit_cube_builds_six_faces_one_shell_one_solid() {
        let body = unit_cube_body();
        let (model, _r, c) = run(&body);
        assert_eq!(c.faces.len(), 6, "six advanced faces");
        assert!(c.shells.contains_key(&151));
        assert!(c.solids.contains_key(&161));
        let sid = *c.shells.get(&151).unwrap();
        let shell = model.shells.get(sid).expect("shell stored");
        assert_eq!(shell.faces.len(), 6);
        assert_eq!(shell.shell_type, ShellType::Closed);
        let solid_id = *c.solids.get(&161).unwrap();
        let solid = model.solids.get(solid_id).expect("solid stored");
        assert_eq!(solid.outer_shell, sid);
    }

    #[test]
    fn unit_cube_manifold_check_passes_with_no_warnings() {
        let body = unit_cube_body();
        let (_m, r, _c) = run(&body);
        let mf_warns: Vec<_> = r
            .manifold_warnings
            .iter()
            .filter(|w| w.shell_instance == 151)
            .collect();
        assert!(
            mf_warns.is_empty(),
            "unit cube must be manifold; got: {:?}",
            r.manifold_warnings
        );
    }

    #[test]
    fn closed_shell_with_dangling_edge_emits_manifold_warning() {
        // Just the bottom face — its four edges are each used by one face → dangling.
        let body = format!(
            "{}\
             #111=FACE_OUTER_BOUND('',#101,.T.);\
             #121=AXIS2_PLACEMENT_3D('',#1,#22,#21);\
             #131=PLANE('',#121);\
             #141=ADVANCED_FACE('',(#111),#131,.T.);\
             #151=CLOSED_SHELL('',(#141));",
            make_unit_square_loop_body()
        );
        let (_m, r, _c) = run(&body);
        assert!(r
            .manifold_warnings
            .iter()
            .any(|w| matches!(w.kind, ManifoldKind::DanglingEdge)));
    }

    #[test]
    fn closed_shell_empty_warns() {
        let (_m, r, c) = run("#151=CLOSED_SHELL('',());");
        assert!(!c.shells.contains_key(&151));
        assert!(r.warnings.iter().any(|w| w.entity == "CLOSED_SHELL"));
    }

    #[test]
    fn closed_shell_wrong_arity_warns() {
        let (_m, r, c) = run("#151=CLOSED_SHELL('');");
        assert!(!c.shells.contains_key(&151));
        assert!(r.warnings.iter().any(|w| w.entity == "CLOSED_SHELL"));
    }

    #[test]
    fn manifold_solid_brep_wrong_arity_warns() {
        let (_m, r, c) = run("#161=MANIFOLD_SOLID_BREP('');");
        assert!(!c.solids.contains_key(&161));
        assert!(r.warnings.iter().any(|w| w.entity == "MANIFOLD_SOLID_BREP"));
    }

    #[test]
    fn manifold_solid_brep_dangling_shell_ref_fails() {
        let (_m, _r, c) = run("#161=MANIFOLD_SOLID_BREP('',#999);");
        assert!(!c.solids.contains_key(&161));
    }

    // ------- Cross-phase / out-of-order -------

    #[test]
    fn topology_resolves_when_geometry_appears_after() {
        // Reverse source order: solid first, geometry last.
        let mut s = String::new();
        s += "#161=MANIFOLD_SOLID_BREP('',#151);";
        s += &unit_cube_body();
        let (_m, _r, c) = run(&s);
        assert!(c.solids.contains_key(&161));
    }

    #[test]
    fn unit_cube_records_no_loop_not_closed_healings() {
        let body = unit_cube_body();
        let (_m, r, _c) = run(&body);
        assert!(!r
            .healings
            .iter()
            .any(|h| matches!(h.kind, HealingKind::LoopNotClosed)));
    }

    #[test]
    fn unit_cube_records_no_vertex_snap_healings() {
        let body = unit_cube_body();
        let (_m, r, _c) = run(&body);
        assert!(!r
            .healings
            .iter()
            .any(|h| matches!(h.kind, HealingKind::EdgeVertexSnap)));
    }
}
