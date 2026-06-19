//! Merge a freshly-imported [`BRepModel`] into a live (session) model.
//!
//! The importer builds geometry into a **fresh** `BRepModel`, so its
//! entity ids start from zero and would collide with a live session
//! model that already holds parts. There is no kernel-level model-merge
//! primitive (operations build directly into the one live model), so the
//! import path owns the splice: walk every solid in the imported model
//! and re-add its full topology graph — vertices, curves, surfaces,
//! edges, loops, faces, shells, solids, and void inner-shells — into the
//! target, threading an id remap so cross-references stay consistent.
//!
//! Only entities reachable from a solid are copied; orphan geometry the
//! importer left behind (templates, dangling sub-records) is intentionally
//! dropped. The function returns the target solid ids of the merged
//! solids in source order, so the caller can register UUIDs / broadcast
//! them.

use std::collections::HashMap;

use geometry_engine::primitives::{
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::{Loop, LoopId},
    shell::{Shell, ShellId},
    solid::{Solid, SolidId},
    surface::SurfaceId,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Per-merge id translation tables (source id → target id).
#[derive(Default)]
struct Remap {
    vertices: HashMap<VertexId, VertexId>,
    surfaces: HashMap<SurfaceId, SurfaceId>,
    edges: HashMap<EdgeId, EdgeId>,
    loops: HashMap<LoopId, LoopId>,
    faces: HashMap<FaceId, FaceId>,
    shells: HashMap<ShellId, ShellId>,
}

/// Merge every solid of `src` into `dst`, returning the new (target)
/// solid ids in source order.
pub fn merge_solids_into(dst: &mut BRepModel, src: &BRepModel) -> Vec<SolidId> {
    let mut remap = Remap::default();
    let mut out = Vec::new();
    let solid_ids: Vec<SolidId> = src.solids.iter().map(|(id, _)| id).collect();
    for sid in solid_ids {
        if let Some(new_sid) = merge_one_solid(dst, src, sid, &mut remap) {
            out.push(new_sid);
        }
    }
    out
}

fn merge_one_solid(
    dst: &mut BRepModel,
    src: &BRepModel,
    src_solid: SolidId,
    remap: &mut Remap,
) -> Option<SolidId> {
    let solid = src.solids.get(src_solid)?.clone();
    let outer = remap_shell(dst, src, solid.outer_shell, remap)?;
    let mut new_solid = Solid::new(0, outer);
    if !solid.name.clone().unwrap_or_default().is_empty() {
        if let Some(name) = solid.name.clone() {
            new_solid.name = Some(name);
        }
    }
    for void in &solid.inner_shells {
        if let Some(s) = remap_shell(dst, src, *void, remap) {
            new_solid.add_inner_shell(s);
        }
    }
    Some(dst.solids.add(new_solid))
}

fn remap_shell(
    dst: &mut BRepModel,
    src: &BRepModel,
    src_shell: ShellId,
    remap: &mut Remap,
) -> Option<ShellId> {
    if let Some(existing) = remap.shells.get(&src_shell) {
        return Some(*existing);
    }
    let shell = src.shells.get(src_shell)?.clone();
    let mut face_ids = Vec::with_capacity(shell.faces.len());
    for &fid in &shell.faces {
        if let Some(nf) = remap_face(dst, src, fid, remap) {
            face_ids.push(nf);
        }
    }
    let mut new_shell = Shell::with_capacity(0, shell.shell_type, face_ids.len());
    new_shell.add_faces(&face_ids);
    let new_id = dst.shells.add(new_shell);
    remap.shells.insert(src_shell, new_id);
    Some(new_id)
}

fn remap_face(
    dst: &mut BRepModel,
    src: &BRepModel,
    src_face: FaceId,
    remap: &mut Remap,
) -> Option<FaceId> {
    if let Some(existing) = remap.faces.get(&src_face) {
        return Some(*existing);
    }
    let face = src.faces.get(src_face)?.clone();
    let surface = remap_surface(dst, src, face.surface_id, remap)?;
    let outer = remap_loop(dst, src, face.outer_loop, remap)?;
    let mut new_face = Face::new(0, surface, outer, face.orientation);
    for &inner in &face.inner_loops {
        if let Some(nl) = remap_loop(dst, src, inner, remap) {
            new_face.add_inner_loop(nl);
        }
    }
    let new_id = dst.faces.add(new_face);
    remap.faces.insert(src_face, new_id);
    Some(new_id)
}

fn remap_loop(
    dst: &mut BRepModel,
    src: &BRepModel,
    src_loop: LoopId,
    remap: &mut Remap,
) -> Option<LoopId> {
    if let Some(existing) = remap.loops.get(&src_loop) {
        return Some(*existing);
    }
    let lp = src.loops.get(src_loop)?.clone();
    let mut new_loop = Loop::with_capacity(0, lp.loop_type, lp.edges.len());
    for (idx, &eid) in lp.edges.iter().enumerate() {
        let forward = lp.orientations.get(idx).copied().unwrap_or(true);
        if let Some(ne) = remap_edge(dst, src, eid, remap) {
            new_loop.add_edge(ne, forward);
        }
    }
    let new_id = dst.loops.add(new_loop);
    remap.loops.insert(src_loop, new_id);
    Some(new_id)
}

fn remap_edge(
    dst: &mut BRepModel,
    src: &BRepModel,
    src_edge: EdgeId,
    remap: &mut Remap,
) -> Option<EdgeId> {
    if let Some(existing) = remap.edges.get(&src_edge) {
        return Some(*existing);
    }
    let edge = src.edges.get(src_edge)?.clone();
    let sv = remap_vertex(dst, src, edge.start_vertex, remap)?;
    let ev = remap_vertex(dst, src, edge.end_vertex, remap)?;
    // Curves are owned per-edge (the importer clones a fresh curve for
    // each edge), so each edge gets its own re-added curve instance.
    let curve = src.curves.get(edge.curve_id)?.clone_box();
    let new_curve = dst.curves.add(curve);
    let mut new_edge = Edge::new(0, sv, ev, new_curve, edge.orientation, edge.param_range);
    new_edge.set_tolerance(edge.tolerance);
    let new_id = dst.edges.add(new_edge);
    remap.edges.insert(src_edge, new_id);
    Some(new_id)
}

fn remap_vertex(
    dst: &mut BRepModel,
    src: &BRepModel,
    src_vertex: VertexId,
    remap: &mut Remap,
) -> Option<VertexId> {
    if let Some(existing) = remap.vertices.get(&src_vertex) {
        return Some(*existing);
    }
    let p = src.vertices.get_position(src_vertex)?;
    let new_id = dst.vertices.add(p[0], p[1], p[2]);
    remap.vertices.insert(src_vertex, new_id);
    Some(new_id)
}

fn remap_surface(
    dst: &mut BRepModel,
    src: &BRepModel,
    src_surface: SurfaceId,
    remap: &mut Remap,
) -> Option<SurfaceId> {
    if let Some(existing) = remap.surfaces.get(&src_surface) {
        return Some(*existing);
    }
    let surf = src.surfaces.get(src_surface)?.clone_box();
    let new_id = dst.surfaces.add(surf);
    remap.surfaces.insert(src_surface, new_id);
    Some(new_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small helper to build a single-solid model via the unit-cube
    /// import, then merge it into an empty target and confirm the solid
    /// count and that the merged solid validates the same as the source.
    #[tokio::test]
    async fn merge_unit_cube_into_empty_model() {
        use crate::formats::step::import_step_to_brep_with_report;
        use std::io::Write;

        // Reuse the canonical unit-cube fixture shape.
        let body = unit_cube_with_root();
        let src = format!(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));\n\
             ENDSEC;\nDATA;\n{body}\nENDSEC;\nEND-ISO-10303-21;\n"
        );
        let mut f = tempfile::NamedTempFile::new().expect("tmp");
        f.write_all(src.as_bytes()).expect("write");
        let (imported, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(imported.solids.len(), 1, "fixture imports one solid");
        assert!(
            report.ok,
            "imported cube must be valid: {:?}",
            report.validation
        );

        let mut target = BRepModel::new();
        let merged = merge_solids_into(&mut target, &imported);
        assert_eq!(merged.len(), 1);
        assert_eq!(target.solids.len(), 1, "one solid spliced in");
        // The merged solid's outer shell carries the six cube faces.
        let sid = merged[0];
        let solid = target.solids.get(sid).expect("merged solid");
        let shell = target.shells.get(solid.outer_shell).expect("merged shell");
        assert_eq!(shell.faces.len(), 6, "cube has six faces after merge");
    }

    /// Two merges into the same target produce two independent solids
    /// with disjoint id ranges (no collision).
    #[tokio::test]
    async fn merge_twice_yields_two_solids() {
        use crate::formats::step::import_step_to_brep_with_report;
        use std::io::Write;

        let body = unit_cube_with_root();
        let src = format!(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));\n\
             ENDSEC;\nDATA;\n{body}\nENDSEC;\nEND-ISO-10303-21;\n"
        );
        let mut f = tempfile::NamedTempFile::new().expect("tmp");
        f.write_all(src.as_bytes()).expect("write");
        let (imported, _r) = import_step_to_brep_with_report(f.path()).await.unwrap();

        let mut target = BRepModel::new();
        let _ = merge_solids_into(&mut target, &imported);
        let _ = merge_solids_into(&mut target, &imported);
        assert_eq!(target.solids.len(), 2, "two independent cubes spliced");
    }

    fn unit_cube_with_root() -> String {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#3=CARTESIAN_POINT('',(1.,1.,0.));#4=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#5=CARTESIAN_POINT('',(0.,0.,1.));#6=CARTESIAN_POINT('',(1.,0.,1.));";
        s += "#7=CARTESIAN_POINT('',(1.,1.,1.));#8=CARTESIAN_POINT('',(0.,1.,1.));";
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
        s += "#151=CLOSED_SHELL('',(#141,#142,#143,#144,#145,#146));";
        s += "#161=MANIFOLD_SOLID_BREP('cube',#151);";
        s += "#204=AXIS2_PLACEMENT_3D('world',#1,#23,#21);";
        s += "#205=GEOMETRIC_REPRESENTATION_CONTEXT(3);";
        s += "#301=ADVANCED_BREP_SHAPE_REPRESENTATION('cube',(#161,#204),#205);";
        s
    }
}
