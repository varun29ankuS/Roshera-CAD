//! Lazy cross-phase reference resolution.
//!
//! The dispatcher walks the registry in phase order (Unit → Geometry →
//! Topology → Root), but **within** a phase the iteration order is
//! whatever ordering the registry's HashMap chooses, which is
//! deliberately not source order. That means a handler in the Geometry
//! phase can be invoked for `AXIS2_PLACEMENT_3D #117` before
//! `CARTESIAN_POINT #42` (its `location` reference) has been
//! dispatched, even though both are in the same phase.
//!
//! [`ensure_resolved`] handles that: when a handler discovers that
//! `ctx.caches.points` doesn't yet contain `#42`, it calls
//! `ensure_resolved(42, &["CARTESIAN_POINT"], registry, dispatch, ctx)`
//! which:
//!   1. Detects cycles via [`ImportContext::resolution_stack`].
//!   2. Looks up `#42` in the registry. If absent, returns an error —
//!      the file references a non-existent entity.
//!   3. Verifies the entity's name matches one of the `expected`
//!      list. If not, returns an error — the file references the
//!      wrong kind of entity (e.g. an `EDGE_CURVE` where a
//!      `CARTESIAN_POINT` was expected).
//!   4. Dispatches the entity's handler in-line, with the resolution
//!      stack guarding against recursion.
//!   5. Returns `Ok(())` regardless of whether the recursive dispatch
//!      actually populated a cache entry — the caller re-checks its
//!      cache and emits a structured warning if the resolve failed
//!      silently (a malformed sub-entity, etc.).
//!
//! The resolver does **not** populate any caches itself. The handlers
//! own that — they read `record`, materialise the value, and write
//! the cache. The resolver only ensures `handler.handle(...)` has been
//! invoked.

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, HandlerOutcome},
    registry::{EntityKind, EntityRegistry},
};

/// Outcome of a [`ensure_resolved`] call. Distinct from
/// `HandlerOutcome` because resolution can also fail before any
/// handler runs (cycle, missing entity, wrong kind, no handler).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The recursive dispatch ran and returned `Resolved` or
    /// `Skipped`. The caller should re-check its cache to discover
    /// whether the resolution populated it.
    Dispatched,
    /// The instance's handler had already run this import (idempotence
    /// guard); no dispatch occurred. The caller re-reads its caches
    /// exactly as for [`Self::Dispatched`].
    AlreadyResolved,
    /// The entity was already in the resolution stack — a cycle. A
    /// warning has been appended to the report; no recursion occurred.
    CycleDetected,
    /// The instance number was not found in the registry. The file
    /// references a `#N` that doesn't exist.
    NotFound,
    /// The entity exists but its name doesn't match `expected`. The
    /// caller is referencing the wrong kind of entity.
    WrongKind {
        found: String,
        expected: Vec<String>,
    },
    /// No handler is registered for the entity's name. Logged on the
    /// first encounter via the normal dispatch path; here we just
    /// short-circuit so the caller can emit its own contextual
    /// warning.
    NoHandler { entity: String },
    /// The handler ran but reported a non-`Resolved` outcome
    /// (`Skipped` or `Failed`). The caller can still proceed — its
    /// own cache lookup will determine whether enough state was
    /// produced.
    HandlerNonResolved(HandlerOutcome),
}

/// Dispatch `instance` *now* if it hasn't been dispatched already.
///
/// `expected` is a list of upper-cased entity names the caller will
/// accept. The function refuses to dispatch an entity whose name is
/// not in the list — typed safety against malformed reference graphs.
pub fn ensure_resolved(
    instance: u64,
    expected: &[&str],
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> ResolveOutcome {
    // Idempotence guard: if this instance's handler already ran (either
    // here on-demand or via the top-level `run_all` sweep), do not
    // dispatch again — a repeat run mints duplicate kernel entities.
    // Callers re-read the resolution caches (e.g. `caches.edges`) after
    // this returns, so reporting `AlreadyResolved` is sufficient.
    if ctx.is_resolved(instance) {
        return ResolveOutcome::AlreadyResolved;
    }

    // Cycle guard.
    if ctx.is_resolving(instance) {
        ctx.report.push_warning(Warning {
            severity: Severity::Error,
            entity: String::new(),
            instance: Some(instance),
            message: format!(
                "cycle detected resolving #{}; resolution stack = {:?}",
                instance, ctx.resolution_stack
            ),
        });
        return ResolveOutcome::CycleDetected;
    }

    // Look up the target entity.
    let entity = match registry.get(instance) {
        Some(e) => e,
        None => {
            ctx.report.push_warning(Warning {
                severity: Severity::Error,
                entity: String::new(),
                instance: Some(instance),
                message: format!("reference to #{instance} which is not defined in the file"),
            });
            return ResolveOutcome::NotFound;
        }
    };

    // Complex (sub-super) instance: the rational / bounded B-spline
    // curve and surface families arrive this way and carry no single
    // dispatch name. Attempt the complex geometry builder before the
    // name check (which would otherwise reject the arbitrary
    // first-constituent name against the caller's `expected` list). On
    // success the kernel curve/surface is now in `ctx.caches`, which is
    // exactly what the caller re-reads after we return.
    if let EntityKind::Complex(records) = &entity.kind {
        let records = records.clone();
        ctx.resolution_stack.push(instance);
        ctx.mark_resolved(instance);
        let built = crate::formats::step::handlers::tier2::complex::try_build_complex(
            instance, &records, registry, dispatch, ctx,
        );
        let popped = ctx.resolution_stack.pop();
        debug_assert_eq!(
            popped,
            Some(instance),
            "resolution stack imbalance (complex)"
        );
        if built {
            return ResolveOutcome::Dispatched;
        }
        // Not a recognised complex geometry entity — fall through to the
        // legacy first-constituent dispatch path (un-mark so it can run).
        ctx.resolved.remove(&instance);
    }

    // Name check.
    let name = entity.kind.primary_name().to_string();
    if !expected.is_empty() && !expected.iter().any(|e| e.eq_ignore_ascii_case(&name)) {
        ctx.report.push_warning(Warning {
            severity: Severity::Warn,
            entity: name.clone(),
            instance: Some(instance),
            message: format!(
                "expected one of {:?}, found {} when resolving #{instance}",
                expected, name
            ),
        });
        return ResolveOutcome::WrongKind {
            found: name,
            expected: expected.iter().map(|s| s.to_string()).collect(),
        };
    }

    // Look up handler.
    let handler = match dispatch.lookup(&name) {
        Some(h) => h,
        None => return ResolveOutcome::NoHandler { entity: name },
    };

    // Pull the Simple record. Complex sub-super dispatch is tier-3.
    let record = match &entity.kind {
        EntityKind::Simple(r) => r,
        EntityKind::Complex(records) => match records.first() {
            Some(r) => r,
            None => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Error,
                    entity: name.clone(),
                    instance: Some(instance),
                    message: "empty complex entity".to_string(),
                });
                return ResolveOutcome::HandlerNonResolved(HandlerOutcome::Failed {
                    message: "empty complex entity".to_string(),
                });
            }
        },
    };

    // Push, dispatch, pop. The pop is deliberately unconditional so
    // an early-return from `handle` (panic-free Rust gives us this
    // for free, but we still write it as a guarded block in case
    // future handlers use `?` internally on a result type that the
    // compiler turns into an early return).
    ctx.resolution_stack.push(instance);
    // Mark resolved BEFORE handing off so the later `run_all` sweep
    // skips this instance (it was dispatched here on-demand). Marking
    // before dispatch is safe: the `is_resolving` stack still guards
    // genuine cycles, and a re-entrant `ensure_resolved` on the same
    // instance now short-circuits at the `is_resolved` guard above.
    ctx.mark_resolved(instance);
    let outcome = handler.handle(instance, record, registry, dispatch, ctx);
    let popped = ctx.resolution_stack.pop();
    debug_assert_eq!(popped, Some(instance), "resolution stack imbalance");

    match outcome {
        HandlerOutcome::Resolved => ResolveOutcome::Dispatched,
        other => ResolveOutcome::HandlerNonResolved(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::{
        diagnostics::ImportReport,
        dispatch::{EntityDispatch, EntityHandler, Phase},
        parser::parse_step,
        registry::EntityRegistry,
    };
    use geometry_engine::primitives::topology_builder::BRepModel;
    use ruststep::ast::Record;

    /// Test handler: records every instance it's invoked on into a
    /// thread-local list (handler is `&'static dyn` so we can't carry
    /// state; the cache write proxy is an `unsafe` static for tests).
    struct CountingHandler;

    impl EntityHandler for CountingHandler {
        fn names(&self) -> &'static [&'static str] {
            &["CARTESIAN_POINT"]
        }
        fn phase(&self) -> Phase {
            Phase::Geometry
        }
        fn handle(
            &self,
            instance: u64,
            _record: &Record,
            _registry: &EntityRegistry,
            _dispatch: &EntityDispatch,
            ctx: &mut ImportContext<'_>,
        ) -> HandlerOutcome {
            ctx.caches.points.insert(instance, [0.0, 0.0, 0.0]);
            HandlerOutcome::Resolved
        }
    }

    static COUNTING_HANDLER: CountingHandler = CountingHandler;

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

    fn setup(src: &str) -> (EntityRegistry, EntityDispatch, BRepModel, ImportReport) {
        let ex = parse_step(src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        dispatch.register(&COUNTING_HANDLER);
        (reg, dispatch, BRepModel::new(), ImportReport::new())
    }

    #[test]
    fn ensure_resolved_dispatches_a_known_entity() {
        let src = wrap("#1=CARTESIAN_POINT('',(0.,0.,0.));");
        let (reg, dispatch, mut model, mut report) = setup(&src);
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let r = ensure_resolved(1, &["CARTESIAN_POINT"], &reg, &dispatch, &mut ctx);
        assert_eq!(r, ResolveOutcome::Dispatched);
        assert!(ctx.caches.points.contains_key(&1));
    }

    #[test]
    fn ensure_resolved_flags_missing_entity() {
        let src = wrap("#1=CARTESIAN_POINT('',(0.,0.,0.));");
        let (reg, dispatch, mut model, mut report) = setup(&src);
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let r = ensure_resolved(99, &["CARTESIAN_POINT"], &reg, &dispatch, &mut ctx);
        assert_eq!(r, ResolveOutcome::NotFound);
        assert_eq!(ctx.report.warnings.len(), 1);
        assert!(ctx.report.warnings[0].message.contains("#99"));
    }

    #[test]
    fn ensure_resolved_flags_wrong_kind() {
        let src = wrap("#1=CARTESIAN_POINT('',(0.,0.,0.));");
        let (reg, dispatch, mut model, mut report) = setup(&src);
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let r = ensure_resolved(1, &["DIRECTION"], &reg, &dispatch, &mut ctx);
        match r {
            ResolveOutcome::WrongKind { found, .. } => assert_eq!(found, "CARTESIAN_POINT"),
            other => panic!("expected WrongKind, got {other:?}"),
        }
    }

    #[test]
    fn ensure_resolved_short_circuits_on_cycle() {
        let src = wrap("#1=CARTESIAN_POINT('',(0.,0.,0.));");
        let (reg, dispatch, mut model, mut report) = setup(&src);
        let mut ctx = ImportContext::new(&mut model, &mut report);
        // Simulate that #1 is already mid-resolve.
        ctx.resolution_stack.push(1);
        let r = ensure_resolved(1, &["CARTESIAN_POINT"], &reg, &dispatch, &mut ctx);
        assert_eq!(r, ResolveOutcome::CycleDetected);
        // No second dispatch occurred.
        assert!(!ctx.caches.points.contains_key(&1));
        // Cycle warning was logged.
        assert!(ctx
            .report
            .warnings
            .iter()
            .any(|w| w.message.contains("cycle")));
    }

    #[test]
    fn ensure_resolved_no_handler() {
        let src = wrap("#1=DIRECTION('',(0.,0.,1.));");
        let (reg, dispatch, mut model, mut report) = setup(&src);
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let r = ensure_resolved(1, &["DIRECTION"], &reg, &dispatch, &mut ctx);
        match r {
            ResolveOutcome::NoHandler { entity } => assert_eq!(entity, "DIRECTION"),
            other => panic!("expected NoHandler, got {other:?}"),
        }
    }
}
