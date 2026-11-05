// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! STEP assembly placement — the product-structure path every mainstream
//! exporter (SolidWorks, Fusion, Creo, OCCT) uses:
//!
//! ```text
//! NEXT_ASSEMBLY_USAGE_OCCURRENCE(parent PD, child PD)
//!   ← PRODUCT_DEFINITION_SHAPE(nauo)
//!   ← CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(
//!        (REPRESENTATION_RELATIONSHIP(child SR, parent SR)
//!         REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(ITEM_DEFINED_TRANSFORMATION)
//!         SHAPE_REPRESENTATION_RELATIONSHIP()),
//!        that PDS)
//! ```
//!
//! Before this was handled, a part used twice landed ONCE at its own local
//! origin (the robot-arm probe: 634 placements → 97 solids, all unplaced).

use export_engine::ExportEngine;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('asm'),'2;1');\n\
         FILE_NAME('asm.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// Unit cube part: `ADVANCED_BREP_SHAPE_REPRESENTATION #301` holding
/// `MANIFOLD_SOLID_BREP #161` plus its origin placement `#204`.
fn unit_cube_part() -> String {
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
    s += "#204=AXIS2_PLACEMENT_3D('origin',#1,#23,#21);";
    s += "#205=GEOMETRIC_REPRESENTATION_CONTEXT(3);";
    s += "#301=ADVANCED_BREP_SHAPE_REPRESENTATION('cube',(#161,#204),#205);";
    s
}

/// Product structure: part `cube` (PD #405) used twice by assembly `asm`
/// (PD #415), placed at (10,0,0) and at (0,20,0) rotated 90° about Z.
fn two_instance_assembly() -> String {
    let mut s = unit_cube_part();
    s += "#400=APPLICATION_CONTEXT('mechanical design');";
    s += "#402=PRODUCT_CONTEXT('',#400,'mechanical');";
    s += "#404=PRODUCT_DEFINITION_CONTEXT('part definition',#400,'design');";
    // Part.
    s += "#401=PRODUCT('cube','cube','',(#402));";
    s += "#403=PRODUCT_DEFINITION_FORMATION('','',#401);";
    s += "#405=PRODUCT_DEFINITION('design','',#403,#404);";
    s += "#406=PRODUCT_DEFINITION_SHAPE('','',#405);";
    s += "#407=SHAPE_DEFINITION_REPRESENTATION(#406,#301);";
    // Assembly.
    s += "#411=PRODUCT('asm','asm','',(#402));";
    s += "#413=PRODUCT_DEFINITION_FORMATION('','',#411);";
    s += "#415=PRODUCT_DEFINITION('design','',#413,#404);";
    s += "#416=PRODUCT_DEFINITION_SHAPE('','',#415);";
    s += "#430=CARTESIAN_POINT('',(10.,0.,0.));#431=CARTESIAN_POINT('',(0.,20.,0.));";
    s += "#421=AXIS2_PLACEMENT_3D('i1',#430,#23,#21);";
    s += "#422=AXIS2_PLACEMENT_3D('i2',#431,#23,#22);";
    s += "#420=SHAPE_REPRESENTATION('asm',(#204,#421,#422),#205);";
    s += "#417=SHAPE_DEFINITION_REPRESENTATION(#416,#420);";
    // Instance 1.
    s += "#440=NEXT_ASSEMBLY_USAGE_OCCURRENCE('1','i1','',#415,#405,$);";
    s += "#441=PRODUCT_DEFINITION_SHAPE('','',#440);";
    s += "#442=ITEM_DEFINED_TRANSFORMATION('','',#204,#421);";
    s += "#443=(REPRESENTATION_RELATIONSHIP('','',#301,#420)\
          REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#442)\
          SHAPE_REPRESENTATION_RELATIONSHIP());";
    s += "#444=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#443,#441);";
    // Instance 2.
    s += "#450=NEXT_ASSEMBLY_USAGE_OCCURRENCE('2','i2','',#415,#405,$);";
    s += "#451=PRODUCT_DEFINITION_SHAPE('','',#450);";
    s += "#452=ITEM_DEFINED_TRANSFORMATION('','',#204,#422);";
    s += "#453=(REPRESENTATION_RELATIONSHIP('','',#301,#420)\
          REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#452)\
          SHAPE_REPRESENTATION_RELATIONSHIP());";
    s += "#454=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#453,#451);";
    s
}

fn solid_bbox(model: &BRepModel, sid: SolidId) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let solid = model.solids.get(sid).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face");
        let lp = model.loops.get(face.outer_loop).expect("loop");
        for &eid in &lp.edges {
            let e = model.edges.get(eid).expect("edge");
            for v in [e.start_vertex, e.end_vertex] {
                let p = model.vertices.get_position(v).expect("vertex");
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
    }
    (lo, hi)
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9)
}

#[test]
fn a_part_used_twice_imports_as_two_placed_solids() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&two_instance_assembly()), "asm")
        .unwrap();

    assert_eq!(
        model.solids.len(),
        2,
        "two placements → two solids; report: {:?}",
        report.counts.unsupported
    );
    let boxes: Vec<_> = model
        .solids
        .iter()
        .map(|(sid, _)| solid_bbox(&model, sid))
        .collect();
    // Instance 1: translated by (10,0,0).
    assert!(
        boxes
            .iter()
            .any(|(lo, hi)| close(*lo, [10.0, 0.0, 0.0]) && close(*hi, [11.0, 1.0, 1.0])),
        "instance at (10,0,0) missing: {boxes:?}"
    );
    // Instance 2: rotated +90° about Z (x→y) then translated to (0,20,0):
    // the unit cube spans x∈[-1,0], y∈[20,21].
    assert!(
        boxes
            .iter()
            .any(|(lo, hi)| close(*lo, [-1.0, 20.0, 0.0]) && close(*hi, [0.0, 21.0, 1.0])),
        "rotated instance at (0,20,0) missing: {boxes:?}"
    );
    assert!(
        report.ok,
        "both instances must be sound: {:?}",
        report.validation
    );
}

#[test]
fn placed_instances_carry_the_part_name() {
    let (model, _) = ExportEngine::new()
        .import_step_content(&envelope(&two_instance_assembly()), "asm")
        .unwrap();
    let names: Vec<_> = model
        .solids
        .iter()
        .map(|(_, s)| s.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(
        names.iter().all(|n| n.starts_with("cube")),
        "each instance is named after its PRODUCT: {names:?}"
    );
}

/// Product scaffolding for a part `cube` (PD #405, rep #301) — shared by the
/// nested and reversed fixtures below.
fn cube_product() -> String {
    let mut s = unit_cube_part();
    s += "#400=APPLICATION_CONTEXT('mechanical design');";
    s += "#402=PRODUCT_CONTEXT('',#400,'mechanical');";
    s += "#404=PRODUCT_DEFINITION_CONTEXT('part definition',#400,'design');";
    s += "#401=PRODUCT('cube','cube','',(#402));";
    s += "#403=PRODUCT_DEFINITION_FORMATION('','',#401);";
    s += "#405=PRODUCT_DEFINITION('design','',#403,#404);";
    s += "#406=PRODUCT_DEFINITION_SHAPE('','',#405);";
    s += "#407=SHAPE_DEFINITION_REPRESENTATION(#406,#301);";
    s
}

/// A product `name` with PD `#pd`, shape rep `#rep` holding `items`.
fn assembly_product(name: &str, base: u64, items: &str) -> String {
    let (prod, pdf, pd, pds, sdr, rep) = (base, base + 1, base + 2, base + 3, base + 4, base + 5);
    format!(
        "#{prod}=PRODUCT('{name}','{name}','',(#402));\
         #{pdf}=PRODUCT_DEFINITION_FORMATION('','',#{prod});\
         #{pd}=PRODUCT_DEFINITION('design','',#{pdf},#404);\
         #{pds}=PRODUCT_DEFINITION_SHAPE('','',#{pd});\
         #{rep}=SHAPE_REPRESENTATION('{name}',({items}),#205);\
         #{sdr}=SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep});"
    )
}

/// One use of child PD `child` (rep `child_rep`, frame `child_item`) inside
/// parent PD `parent` (rep `parent_rep`, frame `parent_item`). `reversed`
/// writes the relationship parent-first, as some exporters do.
#[allow(clippy::too_many_arguments)]
fn occurrence(
    base: u64,
    parent: u64,
    child: u64,
    parent_rep: u64,
    child_rep: u64,
    parent_item: u64,
    child_item: u64,
    reversed: bool,
) -> String {
    let (nauo, pds, idt, rr, cdsr) = (base, base + 1, base + 2, base + 3, base + 4);
    let (rep_1, rep_2, item_1, item_2) = if reversed {
        (parent_rep, child_rep, parent_item, child_item)
    } else {
        (child_rep, parent_rep, child_item, parent_item)
    };
    format!(
        "#{nauo}=NEXT_ASSEMBLY_USAGE_OCCURRENCE('{nauo}','u{nauo}','',#{parent},#{child},$);\
         #{pds}=PRODUCT_DEFINITION_SHAPE('','',#{nauo});\
         #{idt}=ITEM_DEFINED_TRANSFORMATION('','',#{item_1},#{item_2});\
         #{rr}=(REPRESENTATION_RELATIONSHIP('','',#{rep_1},#{rep_2})\
         REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{idt})\
         SHAPE_REPRESENTATION_RELATIONSHIP());\
         #{cdsr}=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr},#{pds});"
    )
}

/// `top` places sub-assembly `sub` at (100,0,0); `sub` places `cube` at
/// (0,5,0). The cube must land at (100,5,0): placements compose down the
/// tree, they do not just apply the last one.
#[test]
fn nested_sub_assembly_placements_compose() {
    let mut s = cube_product();
    s += "#600=CARTESIAN_POINT('',(100.,0.,0.));#601=CARTESIAN_POINT('',(0.,5.,0.));";
    s += "#602=AXIS2_PLACEMENT_3D('top->sub',#600,#23,#21);";
    s += "#603=AXIS2_PLACEMENT_3D('sub->cube',#601,#23,#21);";
    // sub (PD #612, rep #615) holds its origin + the cube's target frame.
    s += &assembly_product("sub", 610, "#204,#603");
    // top (PD #622, rep #625) holds its origin + the sub's target frame.
    s += &assembly_product("top", 620, "#204,#602");
    s += &occurrence(700, 622, 612, 625, 615, 602, 204, false);
    s += &occurrence(710, 612, 405, 615, 301, 603, 204, false);
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&s), "nested")
        .unwrap();
    assert_eq!(
        model.solids.len(),
        1,
        "one use → one solid: {:?}",
        report.warnings
    );
    let (sid, _) = model.solids.iter().next().unwrap();
    let (lo, hi) = solid_bbox(&model, sid);
    assert!(
        close(lo, [100.0, 5.0, 0.0]) && close(hi, [101.0, 6.0, 1.0]),
        "cube must sit at (100,5,0) after BOTH placements: lo={lo:?} hi={hi:?}"
    );
}

/// The same placement written parent-first (rep_1 = parent) must land the
/// part in the same place: which side is the child is read from the product
/// tree, not assumed from field order.
#[test]
fn reversed_representation_relationship_places_the_same() {
    let mut s = cube_product();
    s += "#600=CARTESIAN_POINT('',(7.,0.,0.));";
    s += "#602=AXIS2_PLACEMENT_3D('asm->cube',#600,#23,#21);";
    s += &assembly_product("asm", 620, "#204,#602");
    s += &occurrence(700, 622, 405, 625, 301, 602, 204, true);
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&s), "reversed")
        .unwrap();
    assert_eq!(model.solids.len(), 1, "{:?}", report.warnings);
    let (sid, _) = model.solids.iter().next().unwrap();
    let (lo, hi) = solid_bbox(&model, sid);
    assert!(
        close(lo, [7.0, 0.0, 0.0]) && close(hi, [8.0, 1.0, 1.0]),
        "parent-first relationship must still move the cube to x=7: lo={lo:?} hi={hi:?}"
    );
}

/// The SolidWorks layout (the robot-arm probe file): the part's product
/// points at a placement-only `SHAPE_REPRESENTATION` (#330), which a plain
/// `SHAPE_REPRESENTATION_RELATIONSHIP` links to the B-Rep representation
/// (#301) holding the solid; the occurrence's placement relates the two
/// placement-only reps. The solid must still be found through the link and
/// placed — twice.
#[test]
fn a_part_whose_brep_is_linked_by_a_shape_representation_relationship_is_placed() {
    let mut s = unit_cube_part();
    s += "#400=APPLICATION_CONTEXT('mechanical design');";
    s += "#402=PRODUCT_CONTEXT('',#400,'mechanical');";
    s += "#404=PRODUCT_DEFINITION_CONTEXT('part definition',#400,'design');";
    s += "#401=PRODUCT('cube','cube','',(#402));";
    s += "#403=PRODUCT_DEFINITION_FORMATION('','',#401);";
    s += "#405=PRODUCT_DEFINITION('design','',#403,#404);";
    s += "#406=PRODUCT_DEFINITION_SHAPE('','',#405);";
    // Placement-only part rep, linked to the B-Rep rep #301.
    s += "#330=SHAPE_REPRESENTATION('cube',(#204),#205);";
    s += "#407=SHAPE_DEFINITION_REPRESENTATION(#406,#330);";
    s += "#331=SHAPE_REPRESENTATION_RELATIONSHIP('','',#330,#301);";
    s += "#430=CARTESIAN_POINT('',(10.,0.,0.));#431=CARTESIAN_POINT('',(0.,20.,0.));";
    s += "#421=AXIS2_PLACEMENT_3D('i1',#430,#23,#21);";
    s += "#422=AXIS2_PLACEMENT_3D('i2',#431,#23,#21);";
    s += &assembly_product("asm", 620, "#204,#421,#422");
    s += &occurrence(700, 622, 405, 625, 330, 421, 204, false);
    s += &occurrence(710, 622, 405, 625, 330, 422, 204, false);
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&s), "linked")
        .unwrap();
    assert_eq!(
        model.solids.len(),
        2,
        "two uses of the linked part → two solids: {:?}",
        report.warnings
    );
    let boxes: Vec<_> = model
        .solids
        .iter()
        .map(|(sid, _)| solid_bbox(&model, sid))
        .collect();
    for origin in [[10.0, 0.0, 0.0], [0.0, 20.0, 0.0]] {
        assert!(
            boxes.iter().any(|(lo, _)| close(*lo, origin)),
            "a cube must sit at {origin:?}: {boxes:?}"
        );
    }
    assert!(
        !report
            .counts
            .unsupported
            .contains_key("SHAPE_REPRESENTATION_RELATIONSHIP"),
        "the link is read, not dropped: {:?}",
        report.counts.unsupported
    );
}

/// An occurrence whose placement is missing is not silently guessed: the
/// part is still imported (at its parent's origin) and the report says so,
/// naming the occurrence.
#[test]
fn an_occurrence_without_a_placement_is_reported() {
    let mut s = cube_product();
    s += &assembly_product("asm", 620, "#204");
    s += "#700=NEXT_ASSEMBLY_USAGE_OCCURRENCE('700','u700','',#622,#405,$);";
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&s), "unplaced")
        .unwrap();
    assert_eq!(model.solids.len(), 1);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.instance == Some(700) && w.message.contains("no usable placement")),
        "missing placement must be reported against #700: {:?}",
        report.warnings
    );
}

/// Two uses of one part are two bodies with DISJOINT face PIDs, and the
/// whole import is reproducible: same file → same PIDs.
#[test]
fn each_use_of_a_part_gets_its_own_stable_pids() {
    let text = envelope(&two_instance_assembly());
    let pids = |model: &BRepModel| -> Vec<std::collections::BTreeSet<u128>> {
        let mut per_solid: Vec<_> = model
            .solids
            .iter()
            .map(|(sid, s)| {
                let shell = model.shells.get(s.outer_shell).unwrap();
                let set: std::collections::BTreeSet<u128> = shell
                    .faces
                    .iter()
                    .map(|f| model.face_pid(*f).expect("face PID").as_u128())
                    .collect();
                assert_eq!(set.len(), 6, "solid {sid}: six distinct face PIDs");
                set
            })
            .collect();
        per_solid.sort();
        per_solid
    };
    let (a, _) = ExportEngine::new().import_step_content(&text, "a").unwrap();
    let (b, _) = ExportEngine::new().import_step_content(&text, "b").unwrap();
    let (pa, pb) = (pids(&a), pids(&b));
    assert_eq!(pa.len(), 2);
    assert!(
        pa[0].is_disjoint(&pa[1]),
        "two uses of the cube must not share face PIDs"
    );
    assert_eq!(pa, pb, "same file → same PIDs");
}

#[test]
fn placement_entities_are_no_longer_reported_unsupported() {
    let (_, report) = ExportEngine::new()
        .import_step_content(&envelope(&two_instance_assembly()), "asm")
        .unwrap();
    for e in [
        "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
        "ITEM_DEFINED_TRANSFORMATION",
    ] {
        assert!(
            !report.counts.unsupported.contains_key(e),
            "{e} still unsupported: {:?}",
            report.counts.unsupported
        );
    }
}
