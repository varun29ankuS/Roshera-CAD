//! Root-phase handlers: STEP root containers → kernel solid registry.
//!
//! Covered entities (tier-1):
//!
//! | STEP                                  | Effect on context                                                          |
//! |---------------------------------------|----------------------------------------------------------------------------|
//! | `SHAPE_REPRESENTATION`                | Walks `items`, routes resolved `MANIFOLD_SOLID_BREP` ids into `caches.roots`. |
//! | `ADVANCED_BREP_SHAPE_REPRESENTATION`  | Same shape and behaviour — the AP242 specialisation for B-Rep models.      |
//!
//! Both entities are tuples of `(name, items, context_of_items)` where
//! `items` is a `SET OF representation_item` and `context_of_items` is
//! a `representation_context` (e.g. `GEOMETRIC_REPRESENTATION_CONTEXT`).
//! Tier-1's job at the root is purely bookkeeping: every item is
//! either a `MANIFOLD_SOLID_BREP` (the geometry we care about), an
//! `AXIS2_PLACEMENT_3D` (the world origin — already resolved by the
//! geometry phase), or something tier-1 doesn't model
//! (`MAPPED_ITEM`, `STYLED_ITEM`, …). Non-solid items are tolerated
//! with an informational warning rather than rejected — the AP242
//! schema permits arbitrary representation_items, and rejecting an
//! unknown one would force the whole shape representation to fail
//! when the geometry actually imported correctly.
//!
//! ## ImportReport.ok semantics
//!
//! After dispatch, the importer surfaces `report.ok = true` when at
//! least one root entry in `caches.roots` is non-empty. That stronger
//! invariant — "at least one solid is reachable from a root" — is
//! enforced at the call site in [`super::super::super::mod`], not
//! here. The root handler's only contract is: populate
//! `caches.roots[instance]` with the ids of solids referenced by the
//! root's items list, in source order.

use ruststep::ast::Record;

use geometry_engine::primitives::solid::SolidId;

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
};

use super::params;
use super::resolver::ensure_resolved;

/// Common entity names. Centralised so the resolver's `expected`
/// strings and the dispatcher agree on capitalisation.
mod names {
    pub const SHAPE_REPRESENTATION: &str = "SHAPE_REPRESENTATION";
    pub const ADVANCED_BREP_SHAPE_REPRESENTATION: &str = "ADVANCED_BREP_SHAPE_REPRESENTATION";
    pub const MANIFOLD_SOLID_BREP: &str = "MANIFOLD_SOLID_BREP";
    pub const AXIS2_PLACEMENT_3D: &str = "AXIS2_PLACEMENT_3D";
}

// =========================================================================
// SHAPE_REPRESENTATION / ADVANCED_BREP_SHAPE_REPRESENTATION
// =========================================================================

/// `SHAPE_REPRESENTATION('label', (#i1, #i2, …), #context)` and the
/// AP242 specialisation `ADVANCED_BREP_SHAPE_REPRESENTATION` of the
/// same shape. Both populate `ctx.caches.roots[instance]` with the
/// kernel solid ids of every `MANIFOLD_SOLID_BREP` item.
pub struct ShapeRepresentationHandler;
/// Static binding consumed by [`register`].
pub static SHAPE_REPRESENTATION_HANDLER: ShapeRepresentationHandler = ShapeRepresentationHandler;

impl EntityHandler for ShapeRepresentationHandler {
    fn names(&self) -> &'static [&'static str] {
        &[
            names::SHAPE_REPRESENTATION,
            names::ADVANCED_BREP_SHAPE_REPRESENTATION,
        ]
    }
    fn phase(&self) -> Phase {
        Phase::Root
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let entity = record.name.as_str();

        // Field layout: (name, items, context_of_items).
        let fields = match params::record_fields(&record.parameter, entity, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "malformed record".to_string(),
                };
            }
        };
        if fields.len() != 3 {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: entity.to_string(),
                instance: Some(instance),
                message: format!(
                    "expected 3 fields (name, items, context), got {}",
                    fields.len()
                ),
            });
            return HandlerOutcome::Failed {
                message: "wrong arity".to_string(),
            };
        }

        // Items list.
        let items = match params::as_entity_ref_list(&fields[1], entity, instance, "items") {
            Ok(v) => v,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "items list malformed".to_string(),
                };
            }
        };
        if items.is_empty() {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: entity.to_string(),
                instance: Some(instance),
                message: "items list is empty — root has no geometry".to_string(),
            });
            ctx.caches.roots.insert(instance, Vec::new());
            return HandlerOutcome::Resolved;
        }

        // Walk items: every MANIFOLD_SOLID_BREP feeds the root's solid
        // list; AXIS2_PLACEMENT_3D and anything else tier-1 doesn't
        // model is tolerated with an informational warning.
        let mut root_solids: Vec<SolidId> = Vec::with_capacity(items.len());
        for item_ref in &items {
            collect_root_item(
                *item_ref,
                entity,
                instance,
                &mut root_solids,
                registry,
                dispatch,
                ctx,
            );
        }

        ctx.caches.roots.insert(instance, root_solids);

        // Resolve the context_of_items reference if it points at a
        // handler we know about. Tier-1 doesn't allocate anything
        // from a representation_context, but pulling it through
        // exercises the unit / geometry-context handlers wired in
        // IMP2.2 and surfaces unknown context kinds via the usual
        // unsupported path.
        if let Ok(ctx_ref) = params::as_entity_ref(&fields[2], entity, instance, "context") {
            // Empty `expected` list ⇒ accept any entity kind.
            let _ = ensure_resolved(ctx_ref, &[], registry, dispatch, ctx);
        }

        HandlerOutcome::Resolved
    }
}

/// Resolve one item reference inside a root representation's items
/// list. The function is forgiving on purpose: items can legally be
/// anything that implements `representation_item`, and tier-1 only
/// has a kernel home for `MANIFOLD_SOLID_BREP`.
fn collect_root_item(
    item_ref: u64,
    entity: &str,
    instance: u64,
    root_solids: &mut Vec<SolidId>,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) {
    // Fast path: already resolved by the Topology phase.
    if let Some(solid_id) = ctx.caches.solids.get(&item_ref).copied() {
        root_solids.push(solid_id);
        return;
    }
    if ctx.caches.placements.contains_key(&item_ref) {
        return; // origin placement — no kernel side-effect
    }

    // Slow path: force resolution.
    let _ = ensure_resolved(
        item_ref,
        &[names::MANIFOLD_SOLID_BREP, names::AXIS2_PLACEMENT_3D],
        registry,
        dispatch,
        ctx,
    );

    // Re-check: a successful MANIFOLD_SOLID_BREP resolve populates
    // caches.solids; a successful AXIS2_PLACEMENT_3D resolve
    // populates caches.placements. Anything else stayed off the
    // expected list and triggered a wrong-kind warning already.
    if let Some(solid_id) = ctx.caches.solids.get(&item_ref).copied() {
        root_solids.push(solid_id);
        return;
    }
    if ctx.caches.placements.contains_key(&item_ref) {
        return;
    }

    // The item exists in the file but didn't produce a known
    // resolution. Surface it as an Info-level skip so the caller
    // sees the dropped item without treating it as an error.
    ctx.report.push_warning(Warning {
        severity: Severity::Info,
        entity: entity.to_string(),
        instance: Some(instance),
        message: format!("item #{item_ref} is not a tier-1 representation_item; dropped from root"),
    });
}

/// Register every root-phase handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&SHAPE_REPRESENTATION_HANDLER);
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

    /// Drive the full tier-1 pipeline against `body` and return
    /// (model, report, caches).
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

    /// Full unit cube body (no SHAPE_REPRESENTATION) — copied from
    /// `topology::tests::unit_cube_body`. Reproduced inline to keep
    /// the test module self-contained (the topology tests are in a
    /// sibling submodule that can't be reached from here without
    /// `pub(crate)` scaffolding we don't need otherwise).
    fn unit_cube_body() -> String {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        s += "#2=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#3=CARTESIAN_POINT('',(1.,1.,0.));";
        s += "#4=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#5=CARTESIAN_POINT('',(0.,0.,1.));";
        s += "#6=CARTESIAN_POINT('',(1.,0.,1.));";
        s += "#7=CARTESIAN_POINT('',(1.,1.,1.));";
        s += "#8=CARTESIAN_POINT('',(0.,1.,1.));";
        s += "#11=VERTEX_POINT('',#1);";
        s += "#12=VERTEX_POINT('',#2);";
        s += "#13=VERTEX_POINT('',#3);";
        s += "#14=VERTEX_POINT('',#4);";
        s += "#15=VERTEX_POINT('',#5);";
        s += "#16=VERTEX_POINT('',#6);";
        s += "#17=VERTEX_POINT('',#7);";
        s += "#18=VERTEX_POINT('',#8);";
        s += "#21=DIRECTION('',(1.,0.,0.));";
        s += "#22=DIRECTION('',(0.,1.,0.));";
        s += "#23=DIRECTION('',(0.,0.,1.));";
        s += "#24=DIRECTION('',(-1.,0.,0.));";
        s += "#25=DIRECTION('',(0.,-1.,0.));";
        s += "#26=DIRECTION('',(0.,0.,-1.));";
        s += "#31=VECTOR('',#21,1.);";
        s += "#32=VECTOR('',#22,1.);";
        s += "#33=VECTOR('',#23,1.);";
        s += "#41=LINE('',#1,#31);";
        s += "#42=LINE('',#1,#32);";
        s += "#43=LINE('',#1,#33);";
        s += "#51=EDGE_CURVE('',#11,#12,#41,.T.);";
        s += "#52=EDGE_CURVE('',#12,#13,#42,.T.);";
        s += "#53=EDGE_CURVE('',#14,#13,#41,.T.);";
        s += "#54=EDGE_CURVE('',#11,#14,#42,.T.);";
        s += "#55=EDGE_CURVE('',#15,#16,#41,.T.);";
        s += "#56=EDGE_CURVE('',#16,#17,#42,.T.);";
        s += "#57=EDGE_CURVE('',#18,#17,#41,.T.);";
        s += "#58=EDGE_CURVE('',#15,#18,#42,.T.);";
        s += "#59=EDGE_CURVE('',#11,#15,#43,.T.);";
        s += "#60=EDGE_CURVE('',#12,#16,#43,.T.);";
        s += "#61=EDGE_CURVE('',#13,#17,#43,.T.);";
        s += "#62=EDGE_CURVE('',#14,#18,#43,.T.);";
        s += "#71=ORIENTED_EDGE('',*,*,#51,.T.);";
        s += "#72=ORIENTED_EDGE('',*,*,#52,.T.);";
        s += "#73=ORIENTED_EDGE('',*,*,#53,.F.);";
        s += "#74=ORIENTED_EDGE('',*,*,#54,.F.);";
        s += "#75=ORIENTED_EDGE('',*,*,#55,.T.);";
        s += "#76=ORIENTED_EDGE('',*,*,#56,.T.);";
        s += "#77=ORIENTED_EDGE('',*,*,#57,.F.);";
        s += "#78=ORIENTED_EDGE('',*,*,#58,.F.);";
        s += "#79=ORIENTED_EDGE('',*,*,#51,.T.);";
        s += "#80=ORIENTED_EDGE('',*,*,#60,.T.);";
        s += "#81=ORIENTED_EDGE('',*,*,#55,.F.);";
        s += "#82=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#83=ORIENTED_EDGE('',*,*,#52,.T.);";
        s += "#84=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#85=ORIENTED_EDGE('',*,*,#56,.F.);";
        s += "#86=ORIENTED_EDGE('',*,*,#60,.F.);";
        s += "#87=ORIENTED_EDGE('',*,*,#53,.T.);";
        s += "#88=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#89=ORIENTED_EDGE('',*,*,#57,.F.);";
        s += "#90=ORIENTED_EDGE('',*,*,#62,.F.);";
        s += "#91=ORIENTED_EDGE('',*,*,#54,.T.);";
        s += "#92=ORIENTED_EDGE('',*,*,#62,.T.);";
        s += "#93=ORIENTED_EDGE('',*,*,#58,.F.);";
        s += "#94=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#101=EDGE_LOOP('',(#71,#72,#73,#74));";
        s += "#102=EDGE_LOOP('',(#75,#76,#77,#78));";
        s += "#103=EDGE_LOOP('',(#79,#80,#81,#82));";
        s += "#104=EDGE_LOOP('',(#83,#84,#85,#86));";
        s += "#105=EDGE_LOOP('',(#87,#88,#89,#90));";
        s += "#106=EDGE_LOOP('',(#91,#92,#93,#94));";
        s += "#111=FACE_OUTER_BOUND('',#101,.T.);";
        s += "#112=FACE_OUTER_BOUND('',#102,.T.);";
        s += "#113=FACE_OUTER_BOUND('',#103,.T.);";
        s += "#114=FACE_OUTER_BOUND('',#104,.T.);";
        s += "#115=FACE_OUTER_BOUND('',#105,.T.);";
        s += "#116=FACE_OUTER_BOUND('',#106,.T.);";
        s += "#121=AXIS2_PLACEMENT_3D('',#1,#26,#21);";
        s += "#122=AXIS2_PLACEMENT_3D('',#5,#23,#21);";
        s += "#123=AXIS2_PLACEMENT_3D('',#1,#25,#21);";
        s += "#124=AXIS2_PLACEMENT_3D('',#2,#21,#22);";
        s += "#125=AXIS2_PLACEMENT_3D('',#4,#22,#21);";
        s += "#126=AXIS2_PLACEMENT_3D('',#1,#24,#22);";
        s += "#131=PLANE('',#121);";
        s += "#132=PLANE('',#122);";
        s += "#133=PLANE('',#123);";
        s += "#134=PLANE('',#124);";
        s += "#135=PLANE('',#125);";
        s += "#136=PLANE('',#126);";
        s += "#141=ADVANCED_FACE('',(#111),#131,.T.);";
        s += "#142=ADVANCED_FACE('',(#112),#132,.T.);";
        s += "#143=ADVANCED_FACE('',(#113),#133,.T.);";
        s += "#144=ADVANCED_FACE('',(#114),#134,.T.);";
        s += "#145=ADVANCED_FACE('',(#115),#135,.T.);";
        s += "#146=ADVANCED_FACE('',(#116),#136,.T.);";
        s += "#151=CLOSED_SHELL('',(#141,#142,#143,#144,#145,#146));";
        s += "#161=MANIFOLD_SOLID_BREP('',#151);";
        s
    }

    /// Origin placement + minimal geometric_representation_context
    /// used by every root-handler test that needs a believable
    /// (items, context) pair. The origin uses different `#N`s than
    /// the cube builder so the two bodies can be concatenated
    /// without collisions.
    fn root_scaffolding() -> String {
        // #201 origin, #202 +Z, #203 +X, #204 placement, #205 context.
        // The GEOMETRIC_REPRESENTATION_CONTEXT has no handler in tier-1
        // — it'll surface as Unsupported, which is fine: the root
        // handler tolerates unknown context entities.
        let mut s = String::new();
        s += "#201=CARTESIAN_POINT('origin',(0.,0.,0.));";
        s += "#202=DIRECTION('Z',(0.,0.,1.));";
        s += "#203=DIRECTION('X',(1.,0.,0.));";
        s += "#204=AXIS2_PLACEMENT_3D('world',#201,#202,#203);";
        s += "#205=GEOMETRIC_REPRESENTATION_CONTEXT(3);";
        s
    }

    // ------- SHAPE_REPRESENTATION -------

    #[test]
    fn shape_representation_resolves_unit_cube() {
        let mut body = unit_cube_body();
        body += &root_scaffolding();
        body += "#301=SHAPE_REPRESENTATION('cube',(#161,#204),#205);";
        let (model, report, caches) = run(&body);
        assert_eq!(model.solids.len(), 1, "kernel should hold one solid");
        let roots = caches.roots.get(&301).expect("root cached");
        assert_eq!(
            roots.len(),
            1,
            "exactly the MANIFOLD_SOLID_BREP becomes a root solid; placement does not"
        );
        // No structured error-severity warnings — only Info-level
        // skips for the unsupported context are acceptable.
        assert!(
            !report.warnings.iter().any(|w| matches!(
                w.severity,
                super::super::super::super::diagnostics::Severity::Error
            )),
            "no Error-severity warnings: {:?}",
            report.warnings
        );
    }

    #[test]
    fn advanced_brep_shape_representation_resolves_unit_cube() {
        let mut body = unit_cube_body();
        body += &root_scaffolding();
        body += "#301=ADVANCED_BREP_SHAPE_REPRESENTATION('cube',(#161,#204),#205);";
        let (model, _report, caches) = run(&body);
        assert_eq!(model.solids.len(), 1);
        let roots = caches.roots.get(&301).expect("root cached");
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn shape_representation_wrong_arity_warns() {
        let body = format!(
            "{cube}#301=SHAPE_REPRESENTATION('label',(#1));",
            cube = "#1=CARTESIAN_POINT('',(0.,0.,0.));"
        );
        let (_m, r, c) = run(&body);
        assert!(
            !c.roots.contains_key(&301),
            "wrong-arity root must not populate caches.roots"
        );
        assert!(
            r.warnings.iter().any(|w| w.entity == "SHAPE_REPRESENTATION"
                && matches!(
                    w.severity,
                    super::super::super::super::diagnostics::Severity::Warn
                )),
            "wrong-arity warning expected: {:?}",
            r.warnings
        );
    }

    #[test]
    fn shape_representation_empty_items_warns_but_resolves() {
        let body = format!(
            "{ctx}#301=SHAPE_REPRESENTATION('empty',(),#205);",
            ctx = root_scaffolding()
        );
        let (_m, r, c) = run(&body);
        let entry = c.roots.get(&301).expect("root cached even when empty");
        assert!(entry.is_empty());
        assert!(r
            .warnings
            .iter()
            .any(|w| w.message.contains("items list is empty")));
    }

    #[test]
    fn shape_representation_tolerates_unknown_items() {
        // PRESENTATION_LAYER_ASSIGNMENT has no tier-1 handler; it
        // should be logged as Info-skipped, not as an Error.
        let body = format!(
            "{cube}{ctx}\
             #299=PRESENTATION_LAYER_ASSIGNMENT('layer','',(#161));\
             #301=SHAPE_REPRESENTATION('label',(#161,#299,#204),#205);",
            cube = unit_cube_body(),
            ctx = root_scaffolding()
        );
        let (_m, r, c) = run(&body);
        let roots = c.roots.get(&301).expect("root cached");
        assert_eq!(roots.len(), 1, "only the cube solid counts");
        assert!(
            r.warnings
                .iter()
                .any(|w| w.message.contains("not a tier-1 representation_item")),
            "expected Info-skip for #299"
        );
    }

    #[test]
    fn shape_representation_multiple_solids_all_cached() {
        // Two disjoint unit cubes share geometry templates but get
        // separate MANIFOLD_SOLID_BREPs.
        let mut body = unit_cube_body();
        // Translate the cube vertices for a second solid: reuse the
        // same EDGE_CURVE/LOOP/FACE/SHELL graph trick is too
        // intricate — easier to just duplicate one MANIFOLD_SOLID_BREP
        // (the kernel won't care; both will bind to the same shell
        // for this acceptance test, which is enough to confirm the
        // root handler routes >1 solid into caches.roots).
        body += "#162=MANIFOLD_SOLID_BREP('cube2',#151);";
        body += &root_scaffolding();
        body += "#301=SHAPE_REPRESENTATION('two',(#161,#162),#205);";
        let (model, _r, c) = run(&body);
        assert_eq!(model.solids.len(), 2);
        let roots = c.roots.get(&301).expect("root cached");
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn shape_representation_drops_when_items_field_not_a_list() {
        // `items` is supposed to be a list; passing a scalar errors out.
        let body = "#1=CARTESIAN_POINT('',(0.,0.,0.));\
                    #205=GEOMETRIC_REPRESENTATION_CONTEXT(3);\
                    #301=SHAPE_REPRESENTATION('bad',#1,#205);"
            .to_string();
        let (_m, r, c) = run(&body);
        assert!(!c.roots.contains_key(&301));
        assert!(r
            .warnings
            .iter()
            .any(|w| w.entity == "SHAPE_REPRESENTATION"));
    }

    #[test]
    fn root_handler_runs_after_topology_phase() {
        // Even when SHAPE_REPRESENTATION is declared *before* the
        // MANIFOLD_SOLID_BREP in source order, phase ordering means
        // the root handler sees a populated caches.solids by the
        // time it runs.
        let mut body = String::new();
        body += &root_scaffolding();
        // Declare the root first.
        body += "#301=SHAPE_REPRESENTATION('out-of-order',(#161),#205);";
        // Then the cube.
        body += &unit_cube_body();
        let (model, _r, c) = run(&body);
        assert_eq!(model.solids.len(), 1);
        let roots = c.roots.get(&301).expect("root cached");
        assert_eq!(roots.len(), 1);
    }
}
