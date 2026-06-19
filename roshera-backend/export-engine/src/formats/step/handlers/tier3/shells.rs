//! `OPEN_SHELL` — a connected set of faces that does **not** bound a
//! volume (ISO 10303-42 `open_shell`). Used by surface models and as
//! the non-solid sibling of `CLOSED_SHELL`.
//!
//! `OPEN_SHELL('label', (face_refs))` — identical field shape to
//! `CLOSED_SHELL`; the only difference is the kernel `ShellType::Open`
//! tag and that no manifold/closure validation is run (an open shell is
//! free-boundaried by definition).

use ruststep::ast::Record;

use geometry_engine::primitives::shell::{Shell, ShellType};

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::EntityRegistry,
};

use super::super::tier1::params;
use super::super::tier1::topology::resolve_face;

const OPEN_SHELL: &str = "OPEN_SHELL";

/// `OPEN_SHELL('label', (face_refs))`.
pub struct OpenShellHandler;
/// Static binding consumed by [`register`].
pub static OPEN_SHELL_HANDLER: OpenShellHandler = OpenShellHandler;

impl EntityHandler for OpenShellHandler {
    fn names(&self) -> &'static [&'static str] {
        &[OPEN_SHELL]
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
        let fields = match params::record_fields(&record.parameter, OPEN_SHELL, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 2 {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: OPEN_SHELL.into(),
                instance: Some(instance),
                message: "expected (label, (faces))".into(),
            });
            return HandlerOutcome::Failed {
                message: "too few fields".into(),
            };
        }
        let face_refs =
            match params::as_entity_ref_list(&fields[1], OPEN_SHELL, instance, "cfs_faces") {
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
                entity: OPEN_SHELL.into(),
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

        let mut shell = Shell::with_capacity(0, ShellType::Open, face_ids.len());
        shell.add_faces(&face_ids);
        let shell_id = ctx.model.shells.add(shell);
        ctx.caches.shells.insert(instance, shell_id);
        HandlerOutcome::Resolved
    }
}

/// Register the open-shell handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&OPEN_SHELL_HANDLER);
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

    /// One planar triangular face wrapped in an OPEN_SHELL.
    #[test]
    fn open_shell_with_one_face_resolves() {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        s += "#2=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#3=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#11=VERTEX_POINT('',#1);";
        s += "#12=VERTEX_POINT('',#2);";
        s += "#13=VERTEX_POINT('',#3);";
        s += "#21=DIRECTION('',(1.,0.,0.));";
        s += "#22=DIRECTION('',(0.,1.,0.));";
        s += "#23=DIRECTION('',(0.,0.,1.));";
        s += "#31=VECTOR('',#21,1.);";
        s += "#32=VECTOR('',#22,1.);";
        s += "#33=VECTOR('',#23,1.);";
        // Three edges of the triangle.
        s += "#41=LINE('',#1,#31);";
        s += "#42=LINE('',#2,#32);";
        s += "#43=LINE('',#1,#32);";
        s += "#51=EDGE_CURVE('',#11,#12,#41,.T.);";
        s += "#52=EDGE_CURVE('',#12,#13,#43,.T.);";
        s += "#53=EDGE_CURVE('',#11,#13,#43,.T.);";
        s += "#61=ORIENTED_EDGE('',*,*,#51,.T.);";
        s += "#62=ORIENTED_EDGE('',*,*,#52,.T.);";
        s += "#63=ORIENTED_EDGE('',*,*,#53,.F.);";
        s += "#71=EDGE_LOOP('',(#61,#62,#63));";
        s += "#81=FACE_OUTER_BOUND('',#71,.T.);";
        s += "#91=AXIS2_PLACEMENT_3D('',#1,#23,#21);";
        s += "#92=PLANE('',#91);";
        s += "#101=ADVANCED_FACE('',(#81),#92,.T.);";
        s += "#111=OPEN_SHELL('',(#101));";
        let (model, report, caches) = run(&s);
        let shell_id = caches
            .shells
            .get(&111)
            .copied()
            .unwrap_or_else(|| panic!("OPEN_SHELL must resolve: {:?}", report.warnings));
        assert!(model.shells.get(shell_id).is_some());
    }
}
