//! `BREP_WITH_VOIDS` — a manifold solid with internal cavities
//! (ISO 10303-42 `brep_with_voids`, a subtype of `manifold_solid_brep`).
//!
//! `BREP_WITH_VOIDS('label', #outer_closed_shell, (#void_shell, …))`.
//! The outer shell bounds the material; each `void` is an
//! `ORIENTED_CLOSED_SHELL` (or bare `CLOSED_SHELL`) whose enclosed
//! region is *empty* (a hollow). We map this to a kernel `Solid` whose
//! outer shell is the bounding shell and whose `inner_shells` carry the
//! voids, via [`Solid::add_inner_shell`].

use ruststep::ast::{Parameter, Record};

use geometry_engine::primitives::solid::Solid;

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
};

use super::super::tier1::params;
use super::super::tier1::resolver::ensure_resolved;
use super::super::tier1::topology::resolve_shell;

const BREP_WITH_VOIDS: &str = "BREP_WITH_VOIDS";

/// `BREP_WITH_VOIDS('label', #outer, (#void_shell, …))`.
pub struct BrepWithVoidsHandler;
/// Static binding consumed by [`register`].
pub static BREP_WITH_VOIDS_HANDLER: BrepWithVoidsHandler = BrepWithVoidsHandler;

impl EntityHandler for BrepWithVoidsHandler {
    fn names(&self) -> &'static [&'static str] {
        &[BREP_WITH_VOIDS]
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
        let fields = match params::record_fields(&record.parameter, BREP_WITH_VOIDS, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 3 {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: BREP_WITH_VOIDS.into(),
                instance: Some(instance),
                message: "expected (label, outer, voids)".into(),
            });
            return HandlerOutcome::Failed {
                message: "too few fields".into(),
            };
        }

        // Outer bounding shell.
        let outer_ref = match params::as_entity_ref(&fields[1], BREP_WITH_VOIDS, instance, "outer")
        {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad outer ref".into(),
                };
            }
        };
        let outer_shell = match resolve_shell(outer_ref, registry, dispatch, ctx) {
            Some(s) => s,
            None => {
                return HandlerOutcome::Failed {
                    message: "outer shell unresolved".into(),
                };
            }
        };

        // Void shells. Each entry may be a bare CLOSED_SHELL or an
        // ORIENTED_CLOSED_SHELL wrapping one.
        let void_refs =
            match params::as_entity_ref_list(&fields[2], BREP_WITH_VOIDS, instance, "voids") {
                Ok(v) => v,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad voids list".into(),
                    };
                }
            };

        let mut solid = Solid::new(0, outer_shell);
        for void_ref in void_refs.iter().copied() {
            match resolve_void_shell(void_ref, registry, dispatch, ctx) {
                Some(sid) => solid.add_inner_shell(sid),
                None => {
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: BREP_WITH_VOIDS.into(),
                        instance: Some(instance),
                        message: format!("void shell #{void_ref} unresolved; dropped"),
                    });
                }
            }
        }

        let solid_id = ctx.model.solids.add(solid);
        ctx.caches.solids.insert(instance, solid_id);
        HandlerOutcome::Resolved
    }
}

/// Resolve a void-shell reference. STEP voids are `ORIENTED_CLOSED_SHELL`
/// instances (a wrapper that flips a `CLOSED_SHELL`'s orientation) or a
/// bare `CLOSED_SHELL`. We don't model per-shell orientation in the
/// kernel inner-shell list, so an `ORIENTED_CLOSED_SHELL` is unwrapped
/// to its underlying shell.
fn resolve_void_shell(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<geometry_engine::primitives::shell::ShellId> {
    // Already a directly-resolved shell?
    if let Some(s) = ctx.caches.shells.get(&instance).copied() {
        return Some(s);
    }
    // Peek the record: ORIENTED_CLOSED_SHELL forwards to a closed shell.
    if let Some(entity) = registry.get(instance) {
        if let crate::formats::step::registry::EntityKind::Simple(rec) = &entity.kind {
            if rec.name.eq_ignore_ascii_case("ORIENTED_CLOSED_SHELL") {
                // ORIENTED_CLOSED_SHELL('label', #closed_shell, orientation)
                if let Parameter::List(items) = &rec.parameter {
                    if let Some(inner) = items.get(1) {
                        if let Ok(inner_ref) = params::as_entity_ref(
                            inner,
                            "ORIENTED_CLOSED_SHELL",
                            instance,
                            "closed_shell_element",
                        ) {
                            return resolve_shell(inner_ref, registry, dispatch, ctx);
                        }
                    }
                }
            }
        }
    }
    // Otherwise force a CLOSED_SHELL / OPEN_SHELL resolution.
    let _ = ensure_resolved(
        instance,
        &["CLOSED_SHELL", "OPEN_SHELL", "ORIENTED_CLOSED_SHELL"],
        registry,
        dispatch,
        ctx,
    );
    ctx.caches.shells.get(&instance).copied()
}

/// Register the void-solid handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&BREP_WITH_VOIDS_HANDLER);
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
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));\n\
             ENDSEC;\nDATA;\n{body}\nENDSEC;\nEND-ISO-10303-21;\n"
        )
    }

    fn run(body: &str) -> (BRepModel, ImportReport, ResolutionCaches) {
        let src = wrap(body);
        let ex = parse_step(&src, "test").expect("parse");
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        super::super::super::register_all(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        ctx.unit = UnitScale::default();
        let _ = dispatch.run_all(&reg, &mut ctx);
        let caches = std::mem::take(&mut ctx.caches);
        (model, report, caches)
    }

    /// Minimal unit-cube geometry → outer `CLOSED_SHELL` #151. Inlined
    /// (the topology test helper is private to its module).
    fn unit_cube_shell() -> String {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        s += "#2=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#3=CARTESIAN_POINT('',(1.,1.,0.));";
        s += "#4=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#5=CARTESIAN_POINT('',(0.,0.,1.));";
        s += "#6=CARTESIAN_POINT('',(1.,0.,1.));";
        s += "#7=CARTESIAN_POINT('',(1.,1.,1.));";
        s += "#8=CARTESIAN_POINT('',(0.,1.,1.));";
        s += "#11=VERTEX_POINT('',#1);#12=VERTEX_POINT('',#2);#13=VERTEX_POINT('',#3);";
        s += "#14=VERTEX_POINT('',#4);#15=VERTEX_POINT('',#5);#16=VERTEX_POINT('',#6);";
        s += "#17=VERTEX_POINT('',#7);#18=VERTEX_POINT('',#8);";
        s += "#21=DIRECTION('',(1.,0.,0.));#22=DIRECTION('',(0.,1.,0.));#23=DIRECTION('',(0.,0.,1.));";
        s += "#24=DIRECTION('',(-1.,0.,0.));#25=DIRECTION('',(0.,-1.,0.));#26=DIRECTION('',(0.,0.,-1.));";
        s += "#31=VECTOR('',#21,1.);#32=VECTOR('',#22,1.);#33=VECTOR('',#23,1.);";
        s += "#41=LINE('',#1,#31);#42=LINE('',#1,#32);#43=LINE('',#1,#33);";
        s += "#51=EDGE_CURVE('',#11,#12,#41,.T.);#52=EDGE_CURVE('',#12,#13,#42,.T.);";
        s += "#53=EDGE_CURVE('',#14,#13,#41,.T.);#54=EDGE_CURVE('',#11,#14,#42,.T.);";
        s += "#55=EDGE_CURVE('',#15,#16,#41,.T.);#56=EDGE_CURVE('',#16,#17,#42,.T.);";
        s += "#57=EDGE_CURVE('',#18,#17,#41,.T.);#58=EDGE_CURVE('',#15,#18,#42,.T.);";
        s += "#59=EDGE_CURVE('',#11,#15,#43,.T.);#60=EDGE_CURVE('',#12,#16,#43,.T.);";
        s += "#61=EDGE_CURVE('',#13,#17,#43,.T.);#62=EDGE_CURVE('',#14,#18,#43,.T.);";
        s += "#71=ORIENTED_EDGE('',*,*,#51,.T.);#72=ORIENTED_EDGE('',*,*,#52,.T.);";
        s += "#73=ORIENTED_EDGE('',*,*,#53,.F.);#74=ORIENTED_EDGE('',*,*,#54,.F.);";
        s += "#75=ORIENTED_EDGE('',*,*,#55,.T.);#76=ORIENTED_EDGE('',*,*,#56,.T.);";
        s += "#77=ORIENTED_EDGE('',*,*,#57,.F.);#78=ORIENTED_EDGE('',*,*,#58,.F.);";
        s += "#79=ORIENTED_EDGE('',*,*,#51,.T.);#80=ORIENTED_EDGE('',*,*,#60,.T.);";
        s += "#81=ORIENTED_EDGE('',*,*,#55,.F.);#82=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#83=ORIENTED_EDGE('',*,*,#52,.T.);#84=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#85=ORIENTED_EDGE('',*,*,#56,.F.);#86=ORIENTED_EDGE('',*,*,#60,.F.);";
        s += "#87=ORIENTED_EDGE('',*,*,#53,.T.);#88=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#89=ORIENTED_EDGE('',*,*,#57,.F.);#90=ORIENTED_EDGE('',*,*,#62,.F.);";
        s += "#91=ORIENTED_EDGE('',*,*,#54,.T.);#92=ORIENTED_EDGE('',*,*,#62,.T.);";
        s += "#93=ORIENTED_EDGE('',*,*,#58,.F.);#94=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#101=EDGE_LOOP('',(#71,#72,#73,#74));#102=EDGE_LOOP('',(#75,#76,#77,#78));";
        s += "#103=EDGE_LOOP('',(#79,#80,#81,#82));#104=EDGE_LOOP('',(#83,#84,#85,#86));";
        s += "#105=EDGE_LOOP('',(#87,#88,#89,#90));#106=EDGE_LOOP('',(#91,#92,#93,#94));";
        s += "#111=FACE_OUTER_BOUND('',#101,.T.);#112=FACE_OUTER_BOUND('',#102,.T.);";
        s += "#113=FACE_OUTER_BOUND('',#103,.T.);#114=FACE_OUTER_BOUND('',#104,.T.);";
        s += "#115=FACE_OUTER_BOUND('',#105,.T.);#116=FACE_OUTER_BOUND('',#106,.T.);";
        s += "#121=AXIS2_PLACEMENT_3D('',#1,#26,#21);#122=AXIS2_PLACEMENT_3D('',#5,#23,#21);";
        s += "#123=AXIS2_PLACEMENT_3D('',#1,#25,#21);#124=AXIS2_PLACEMENT_3D('',#2,#21,#22);";
        s += "#125=AXIS2_PLACEMENT_3D('',#4,#22,#21);#126=AXIS2_PLACEMENT_3D('',#1,#24,#22);";
        s += "#131=PLANE('',#121);#132=PLANE('',#122);#133=PLANE('',#123);";
        s += "#134=PLANE('',#124);#135=PLANE('',#125);#136=PLANE('',#126);";
        s += "#141=ADVANCED_FACE('',(#111),#131,.T.);#142=ADVANCED_FACE('',(#112),#132,.T.);";
        s += "#143=ADVANCED_FACE('',(#113),#133,.T.);#144=ADVANCED_FACE('',(#114),#134,.T.);";
        s += "#145=ADVANCED_FACE('',(#115),#135,.T.);#146=ADVANCED_FACE('',(#116),#136,.T.);";
        s += "#151=CLOSED_SHELL('outer',(#141,#142,#143,#144,#145,#146));";
        s
    }

    /// An outer unit-cube shell with one inner void shell (the inner
    /// shell reuses the outer faces — sufficient to confirm the void
    /// solid wires up; geometric soundness is exercised by the
    /// integration corpus).
    #[test]
    fn brep_with_voids_resolves_outer_plus_void() {
        let mut s = unit_cube_shell();
        // #152 = a second (void) CLOSED_SHELL over the same faces,
        // #160 = BREP_WITH_VOIDS.
        s += "#152=CLOSED_SHELL('void',(#141,#142,#143,#144,#145,#146));";
        s += "#160=BREP_WITH_VOIDS('hollow',#151,(#152));";
        let (model, report, caches) = run(&s);
        let solid_id = caches
            .solids
            .get(&160)
            .copied()
            .unwrap_or_else(|| panic!("BREP_WITH_VOIDS must resolve: {:?}", report.warnings));
        let solid = model.solids.get(solid_id).expect("solid present");
        assert_eq!(solid.inner_shells.len(), 1, "one void shell recorded");
    }
}
