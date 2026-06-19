//! Entity-name → handler dispatch.
//!
//! Each STEP entity type (`CARTESIAN_POINT`, `LINE`, `ADVANCED_FACE`,
//! …) is handled by an [`EntityHandler`] implementation registered
//! into the global [`EntityDispatch`] table at import time.
//!
//! ## Phases
//!
//! Real STEP files reference entities in a topological order:
//! `MANIFOLD_SOLID_BREP` refers to a `CLOSED_SHELL`, which refers to
//! `ADVANCED_FACE`s, which refer to `EDGE_LOOP`s, which refer to
//! `ORIENTED_EDGE`s, which refer to `EDGE_CURVE`s … all the way down
//! to `CARTESIAN_POINT`. To keep dispatch order-independent of the
//! source file's instance-number ordering, handlers declare which
//! [`Phase`] they belong to. The dispatcher walks them in phase
//! order:
//!
//! 1. **Unit** — `GLOBAL_UNIT_ASSIGNED_CONTEXT`, `SI_UNIT`,
//!    `CONVERSION_BASED_UNIT`. Sets [`ImportContext::unit`].
//! 2. **Geometry** — Pure-geometry primitives that have no topology
//!    dependencies: points, directions, axes, plain curves, plain
//!    surfaces.
//! 3. **Topology** — Vertices, edges, loops, faces, shells, solids
//!    — entities that *use* geometry.
//! 4. **Root** — `ADVANCED_BREP_SHAPE_REPRESENTATION` and friends —
//!    entities that *contain* topology.
//!
//! IMP1 ships with no registered handlers — every entity falls
//! through to `Unsupported`. IMP2 adds tier-1, IMP3 adds tier-2, etc.

use ruststep::ast::Record;
use std::collections::HashMap;

use crate::formats::step::{
    context::ImportContext,
    diagnostics::Unsupported,
    registry::{EntityKind, EntityRegistry, IndexedEntity},
};

/// Dispatcher phase. Handlers declare exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    Unit,
    Geometry,
    Topology,
    Root,
}

impl Phase {
    /// Iteration order used by [`EntityDispatch::run_all`].
    pub const ORDER: [Phase; 4] = [Phase::Unit, Phase::Geometry, Phase::Topology, Phase::Root];
}

/// Outcome of handling a single entity.
///
/// `Resolved` and `Skipped` are both success paths — the difference
/// is that `Skipped` flags an *intentional* drop (presentation
/// entities, kinematic chains, anything we don't model). `Failed`
/// records a handler that ran but couldn't produce useful output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerOutcome {
    /// Handler succeeded; downstream entities can reference this
    /// instance via [`ImportContext::caches`].
    Resolved,
    /// Handler ran and chose to drop the entity (e.g. `STYLED_ITEM`).
    /// Logged at Severity::Info.
    Skipped { reason: &'static str },
    /// Handler ran but failed. The dispatcher will surface this as a
    /// Warning at Severity::Warn or Error.
    Failed { message: String },
}

/// Entity handler trait. One impl per STEP entity name (or per family
/// of equivalent names).
///
/// Handlers receive references to the [`EntityRegistry`] and the
/// [`EntityDispatch`] (this table) so they can drive lazy resolution
/// of referenced entities — when an `ADVANCED_FACE` handler resolves
/// its `face_geometry` reference into a `PLANE` that hasn't been
/// dispatched yet (because the registry is HashMap-backed and walk
/// order isn't source order), it asks the lazy resolver in
/// `super::handlers::tier1::resolver` to dispatch that entity now.
/// The recursion is cycle-guarded via [`ImportContext::resolution_stack`].
pub trait EntityHandler: Send + Sync {
    /// STEP entity names this handler accepts, upper-cased.
    fn names(&self) -> &'static [&'static str];

    /// Phase this handler belongs to.
    fn phase(&self) -> Phase;

    /// Resolve one entity instance.
    ///
    /// `registry` and `dispatch` are passed so the handler can
    /// invoke the lazy resolver from `handlers::tier1::resolver` for
    /// any referenced entities not yet in `ctx.caches`.
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome;
}

/// Registry of entity handlers, looked up by upper-case entity name.
#[derive(Default)]
pub struct EntityDispatch {
    by_name: HashMap<&'static str, &'static dyn EntityHandler>,
}

impl EntityDispatch {
    /// Construct an empty dispatch table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Each name the handler declares is bound to
    /// it; a later registration for the same name overrides earlier
    /// ones (last-write-wins — used in tests to swap mock handlers).
    pub fn register(&mut self, handler: &'static dyn EntityHandler) {
        for name in handler.names() {
            self.by_name.insert(*name, handler);
        }
    }

    /// Lookup by upper-cased entity name.
    pub fn lookup(&self, name: &str) -> Option<&'static dyn EntityHandler> {
        self.by_name.get(name).copied()
    }

    /// Number of registered handler-name bindings.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// `true` when no handlers are registered.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Walk the registry in phase order, dispatching every simple
    /// entity to its handler. Complex (sub-super) records are not
    /// dispatched at this entry point; tier-3 will introduce
    /// per-name dispatch over their constituents.
    ///
    /// Returns the number of entities that produced
    /// [`HandlerOutcome::Resolved`], summed across all phases.
    pub fn run_all(&self, registry: &EntityRegistry, ctx: &mut ImportContext<'_>) -> usize {
        let mut resolved_count = 0usize;
        for &phase in &Phase::ORDER {
            for entity in registry.iter() {
                resolved_count += self.run_one(phase, entity, registry, ctx) as usize;
            }
        }
        resolved_count
    }

    /// Returns `true` when the handler emitted `Resolved`.
    fn run_one(
        &self,
        phase: Phase,
        entity: &IndexedEntity,
        registry: &EntityRegistry,
        ctx: &mut ImportContext<'_>,
    ) -> bool {
        // Complex (`(NAME1(...) NAME2(...) …)`) instances carry no single
        // dispatch name — the partial types are combined by AND. The
        // rational / bounded B-spline curve and surface families arrive
        // this way. We attempt to materialise them in the Geometry phase
        // (they are pure geometry, referenced by topology exactly like a
        // simple `B_SPLINE_CURVE_WITH_KNOTS`). Recognised ones resolve;
        // unrecognised complex instances fall through to the existing
        // first-constituent dispatch / Unsupported logging below.
        if let EntityKind::Complex(records) = &entity.kind {
            if matches!(phase, Phase::Geometry) && !ctx.is_resolved(entity.instance) {
                ctx.mark_resolved(entity.instance);
                if super::handlers::tier2::complex::try_build_complex(
                    entity.instance,
                    records,
                    registry,
                    self,
                    ctx,
                ) {
                    ctx.report.counts.add_resolved(entity.kind.primary_name());
                    return true;
                }
                // Not a recognised complex geometry entity. Un-mark so the
                // first-constituent dispatch path below still gets a
                // chance on this or a later phase (it re-checks
                // `is_resolved`).
                ctx.resolved.remove(&entity.instance);
            }
        }

        let name = entity.kind.primary_name();
        let handler = match self.lookup(name) {
            Some(h) => h,
            None => {
                // Only log Unsupported on the FIRST phase pass so we
                // don't multi-count the same missing entity.
                if matches!(phase, Phase::Unit) {
                    ctx.report.push_unsupported(Unsupported {
                        entity: name.to_string(),
                        instance: entity.instance,
                        reason: format!("no handler registered for {name}"),
                    });
                }
                return false;
            }
        };
        if handler.phase() != phase {
            return false;
        }
        // Idempotence: if this instance was already dispatched (e.g.
        // on-demand via `ensure_resolved` when a peer followed a `#N`
        // reference into it during an earlier entity's resolution),
        // do NOT run the handler again — a second run mints duplicate
        // kernel entities. See `ImportContext::resolved`.
        if ctx.is_resolved(entity.instance) {
            return false;
        }
        let record = match &entity.kind {
            EntityKind::Simple(r) => r,
            EntityKind::Complex(records) => {
                // Tier-1 handlers only consume the first constituent.
                // Tier-3 will widen this when complex entities matter.
                match records.first() {
                    Some(r) => r,
                    None => {
                        ctx.report
                            .push_warning(crate::formats::step::diagnostics::Warning {
                                severity: crate::formats::step::diagnostics::Severity::Warn,
                                entity: name.to_string(),
                                instance: Some(entity.instance),
                                message: "empty complex entity".to_string(),
                            });
                        return false;
                    }
                }
            }
        };
        // Mark before dispatch so a re-entrant `ensure_resolved` on the
        // same instance (cycle back-edge) also short-circuits; the
        // `is_resolving` stack guards true cycles, this guards repeats.
        ctx.mark_resolved(entity.instance);
        match handler.handle(entity.instance, record, registry, self, ctx) {
            HandlerOutcome::Resolved => {
                ctx.report.counts.add_resolved(name);
                true
            }
            HandlerOutcome::Skipped { reason } => {
                ctx.report.counts.add_skipped(name);
                ctx.report
                    .push_warning(crate::formats::step::diagnostics::Warning {
                        severity: crate::formats::step::diagnostics::Severity::Info,
                        entity: name.to_string(),
                        instance: Some(entity.instance),
                        message: reason.to_string(),
                    });
                false
            }
            HandlerOutcome::Failed { message } => {
                ctx.report
                    .push_warning(crate::formats::step::diagnostics::Warning {
                        severity: crate::formats::step::diagnostics::Severity::Warn,
                        entity: name.to_string(),
                        instance: Some(entity.instance),
                        message,
                    });
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::diagnostics::ImportReport;
    use crate::formats::step::parser::parse_step;
    use crate::formats::step::registry::EntityRegistry;
    use geometry_engine::primitives::topology_builder::BRepModel;

    /// Fixture STEP body with one CARTESIAN_POINT.
    fn one_point() -> String {
        "ISO-10303-21;\n\
         HEADER;\n\
         FILE_DESCRIPTION(('t'),'2;1');\n\
         FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n\
         ENDSEC;\n\
         DATA;\n\
         #1=CARTESIAN_POINT('',(0.,0.,0.));\n\
         ENDSEC;\n\
         END-ISO-10303-21;\n"
            .to_string()
    }

    #[test]
    fn empty_dispatch_logs_unsupported_for_every_entity() {
        let ex = parse_step(&one_point(), "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let dispatch = EntityDispatch::new();
        let resolved = dispatch.run_all(&reg, &mut ctx);
        assert_eq!(resolved, 0);
        assert_eq!(report.unsupported.len(), 1);
        assert_eq!(report.unsupported[0].entity, "CARTESIAN_POINT");
        assert_eq!(report.unsupported[0].instance, 1);
    }
}
