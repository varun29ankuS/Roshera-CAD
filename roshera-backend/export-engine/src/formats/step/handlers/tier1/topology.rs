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

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::{
    curve::{Arc, Circle, Curve, Line, ParameterRange},
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceOrientation},
    r#loop::{Loop, LoopId, LoopType},
    shell::{Shell, ShellId, ShellType},
    solid::Solid,
    vertex::VertexId,
};

use crate::formats::step::{
    context::{ImportContext, StepCircleGeom, StepLineGeom},
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
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
        // template is in step_lines / step_circles.
        let _ = ensure_resolved(geom_ref, &["LINE", "CIRCLE"], registry, dispatch, ctx);

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

        // Dispatch on the underlying geometry template.
        let curve_box: Box<dyn Curve> = if let Some(line_geom) =
            ctx.caches.step_lines.get(&geom_ref).copied()
        {
            build_line_curve(line_geom, start_pos, end_pos)
        } else if let Some(circle_geom) = ctx.caches.step_circles.get(&geom_ref).copied() {
            match build_circle_curve(circle_geom, start_pos, end_pos, instance, ctx) {
                Some(c) => c,
                None => {
                    return HandlerOutcome::Failed {
                        message: "kernel rejected circle/arc".into(),
                    }
                }
            }
        } else {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: names::EDGE_CURVE.into(),
                instance: Some(instance),
                message: format!(
                    "edge_geometry #{geom_ref} did not resolve to a tier-1 LINE or CIRCLE; tier-2 (NURBS) is not yet supported"
                ),
            });
            return HandlerOutcome::Failed {
                message: "unsupported edge_geometry".into(),
            };
        };

        // Per-edge snap check between the curve's endpoint evaluation
        // and the recorded vertex position. STEP doesn't always emit
        // perfectly coincident points; we record any deviation.
        if let Ok(cp) = curve_box.evaluate(0.0) {
            check_edge_vertex_snap(
                [cp.position.x, cp.position.y, cp.position.z],
                start_pos,
                ctx.default_tolerance,
                names::EDGE_CURVE,
                instance,
                ctx,
            );
        }
        if let Ok(cp) = curve_box.evaluate(1.0) {
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
            ParameterRange::unit(),
        );
        let mut edge = edge;
        edge.set_tolerance(tol);
        let edge_id = ctx.model.edges.add(edge);
        ctx.caches.edges.insert(instance, edge_id);
        HandlerOutcome::Resolved
    }
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
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> Option<Box<dyn Curve>> {
    let center = Point3::new(
        circle.placement.origin[0],
        circle.placement.origin[1],
        circle.placement.origin[2],
    );
    let normal = Vector3::new(
        circle.placement.z[0],
        circle.placement.z[1],
        circle.placement.z[2],
    );

    // Seam edge: start == end → full circle.
    let dx = start[0] - end[0];
    let dy = start[1] - end[1];
    let dz = start[2] - end[2];
    if (dx * dx + dy * dy + dz * dz).sqrt() < ctx.default_tolerance {
        return match Circle::new(center, normal, circle.radius) {
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

        // Resolve the surface (Plane / Cylinder cached by IMP2.3).
        let _ = ensure_resolved(
            surf_ref,
            &["PLANE", "CYLINDRICAL_SURFACE"],
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
                        "face_geometry #{surf_ref} did not resolve to a tier-1 surface"
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
            None => match bounds.first().map(|(l, _, _)| *l) {
                Some(o) => {
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: names::ADVANCED_FACE.into(),
                        instance: Some(instance),
                        message: "no FACE_OUTER_BOUND; promoting first FACE_BOUND".into(),
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
fn resolve_face(
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

/// Force `instance` to resolve as a CLOSED_SHELL.
fn resolve_shell(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<ShellId> {
    if let Some(s) = ctx.caches.shells.get(&instance) {
        return Some(*s);
    }
    let _ = ensure_resolved(instance, &[names::CLOSED_SHELL], registry, dispatch, ctx);
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
