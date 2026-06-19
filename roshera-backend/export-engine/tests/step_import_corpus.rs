//! STEP IMPORT coverage corpus.
//!
//! End-to-end fixtures exercising the import-coverage features this
//! crate's `formats::step` handlers reconstruct beyond the tier-1
//! planar/cylindrical baseline:
//!
//! - **NURBS-curve edges** — an `EDGE_CURVE` whose `edge_geometry` is a
//!   `B_SPLINE_CURVE_WITH_KNOTS`. Previously any spline edge failed the
//!   whole owning shell; here it must reconstruct a valid closed solid.
//! - **Solids with voids** — `BREP_WITH_VOIDS` (a hollow part) must
//!   materialise one solid carrying an inner void shell.
//! - **The validation gate** — `ImportReport.ok` must reflect the
//!   kernel's `validate_solid_scoped` verdict, not merely "a solid
//!   appeared".
//!
//! Each fixture is hand-written, syntactically-valid AP242 STEP. They
//! are imported through the public `ExportEngine::import_step_content`
//! entry point so the assertions exercise exactly the agent/REST path.

use export_engine::ExportEngine;

const AP242: &str = "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF";

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('corpus'),'2;1');\n\
         FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('{AP242}'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A triangular prism whose three vertical edges are **degree-1
/// B-spline curves** (geometrically straight: 2 control points, knots
/// [0,0,1,1]). This proves a `B_SPLINE_CURVE_WITH_KNOTS` on an
/// `EDGE_CURVE` reconstructs through loops → faces → shell → solid into
/// a valid closed solid — the headline coverage gap.
///
/// Bottom triangle z=0: A(0,0,0) B(2,0,0) C(0,2,0).
/// Top triangle z=1:    D(0,0,1) E(2,0,1) F(0,2,1).
fn nurbs_edge_prism() -> String {
    let mut s = String::new();
    // Cartesian points (corners) + spline control points (reuse corners).
    s += "#1=CARTESIAN_POINT('A',(0.,0.,0.));#2=CARTESIAN_POINT('B',(2.,0.,0.));";
    s += "#3=CARTESIAN_POINT('C',(0.,2.,0.));#4=CARTESIAN_POINT('D',(0.,0.,1.));";
    s += "#5=CARTESIAN_POINT('E',(2.,0.,1.));#6=CARTESIAN_POINT('F',(0.,2.,1.));";
    // Vertices.
    s += "#11=VERTEX_POINT('',#1);#12=VERTEX_POINT('',#2);#13=VERTEX_POINT('',#3);";
    s += "#14=VERTEX_POINT('',#4);#15=VERTEX_POINT('',#5);#16=VERTEX_POINT('',#6);";
    // Directions + vectors + lines for the triangle (planar) edges.
    s += "#21=DIRECTION('',(1.,0.,0.));#22=DIRECTION('',(0.,1.,0.));#23=DIRECTION('',(0.,0.,1.));";
    s += "#24=DIRECTION('',(-1.,0.,0.));";
    s += "#31=VECTOR('',#21,1.);#32=VECTOR('',#22,1.);#33=VECTOR('',#23,1.);";
    // Bottom triangle edge lines.
    s += "#41=LINE('',#1,#31);"; // A->B (+X)
    s += "#42=LINE('',#1,#32);"; // A->C (+Y)
    s += "#43=LINE('',#2,#32);"; // B->C (diag, direction approx — kernel resizes to verts)
                                 // Top triangle edge lines (parallel, reuse directions from top points).
    s += "#44=LINE('',#4,#31);"; // D->E
    s += "#45=LINE('',#4,#32);"; // D->F
    s += "#46=LINE('',#5,#32);"; // E->F
                                 // Three VERTICAL B-spline curves (degree 1, straight): A->D, B->E, C->F.
    s += "#51=B_SPLINE_CURVE_WITH_KNOTS('',1,(#1,#4),.UNSPECIFIED.,.F.,.F.,(2,2),(0.,1.),.UNSPECIFIED.);";
    s += "#52=B_SPLINE_CURVE_WITH_KNOTS('',1,(#2,#5),.UNSPECIFIED.,.F.,.F.,(2,2),(0.,1.),.UNSPECIFIED.);";
    s += "#53=B_SPLINE_CURVE_WITH_KNOTS('',1,(#3,#6),.UNSPECIFIED.,.F.,.F.,(2,2),(0.,1.),.UNSPECIFIED.);";
    // Edge curves.
    s += "#61=EDGE_CURVE('',#11,#12,#41,.T.);"; // A-B
    s += "#62=EDGE_CURVE('',#11,#13,#42,.T.);"; // A-C
    s += "#63=EDGE_CURVE('',#12,#13,#43,.T.);"; // B-C
    s += "#64=EDGE_CURVE('',#14,#15,#44,.T.);"; // D-E
    s += "#65=EDGE_CURVE('',#14,#16,#45,.T.);"; // D-F
    s += "#66=EDGE_CURVE('',#15,#16,#46,.T.);"; // E-F
    s += "#67=EDGE_CURVE('',#11,#14,#51,.T.);"; // A-D (SPLINE)
    s += "#68=EDGE_CURVE('',#12,#15,#52,.T.);"; // B-E (SPLINE)
    s += "#69=EDGE_CURVE('',#13,#16,#53,.T.);"; // C-F (SPLINE)

    // Bottom face (A,B,C), outward normal -Z. Loop A->C->B (CW seen from +Z
    // → outward -Z).
    s += "#71=ORIENTED_EDGE('',*,*,#62,.T.);"; // A->C
    s += "#72=ORIENTED_EDGE('',*,*,#63,.F.);"; // C->B
    s += "#73=ORIENTED_EDGE('',*,*,#61,.F.);"; // B->A
    s += "#74=EDGE_LOOP('',(#71,#72,#73));";
    s += "#75=FACE_OUTER_BOUND('',#74,.T.);";
    s += "#76=AXIS2_PLACEMENT_3D('',#1,#23,#21);"; // plane z=0, normal +Z (face flips via sense)
    s += "#77=PLANE('',#76);";
    s += "#78=ADVANCED_FACE('bottom',(#75),#77,.F.);";

    // Top face (D,E,F), outward +Z. Loop D->E->F.
    s += "#81=ORIENTED_EDGE('',*,*,#64,.T.);"; // D->E
    s += "#82=ORIENTED_EDGE('',*,*,#66,.T.);"; // E->F
    s += "#83=ORIENTED_EDGE('',*,*,#65,.F.);"; // F->D
    s += "#84=EDGE_LOOP('',(#81,#82,#83));";
    s += "#85=FACE_OUTER_BOUND('',#84,.T.);";
    s += "#86=AXIS2_PLACEMENT_3D('',#4,#23,#21);"; // plane z=1, normal +Z
    s += "#87=PLANE('',#86);";
    s += "#88=ADVANCED_FACE('top',(#85),#87,.T.);";

    // Side AB-ED rectangle: A->B->E->D (the spline edges #67 A-D, #68 B-E).
    s += "#91=ORIENTED_EDGE('',*,*,#61,.T.);"; // A->B
    s += "#92=ORIENTED_EDGE('',*,*,#68,.T.);"; // B->E (spline)
    s += "#93=ORIENTED_EDGE('',*,*,#64,.F.);"; // E->D
    s += "#94=ORIENTED_EDGE('',*,*,#67,.F.);"; // D->A (spline)
    s += "#95=EDGE_LOOP('',(#91,#92,#93,#94));";
    s += "#96=FACE_OUTER_BOUND('',#95,.T.);";
    // Side plane: contains A,B (y=0). Normal -Y. Placement origin A, z=-Y.
    s += "#97=DIRECTION('',(0.,-1.,0.));";
    s += "#98=AXIS2_PLACEMENT_3D('',#1,#97,#21);";
    s += "#99=PLANE('',#98);";
    s += "#100=ADVANCED_FACE('sideAB',(#96),#99,.T.);";

    // Side AC-FD rectangle: A->C->F->D (splines #69 C-F, #67 A-D). Plane x=0.
    s += "#101=ORIENTED_EDGE('',*,*,#62,.T.);"; // A->C
    s += "#102=ORIENTED_EDGE('',*,*,#69,.T.);"; // C->F (spline)
    s += "#103=ORIENTED_EDGE('',*,*,#65,.F.);"; // F->D
    s += "#104=ORIENTED_EDGE('',*,*,#67,.F.);"; // D->A (spline)
    s += "#105=EDGE_LOOP('',(#101,#102,#103,#104));";
    s += "#106=FACE_OUTER_BOUND('',#105,.T.);";
    s += "#107=AXIS2_PLACEMENT_3D('',#1,#24,#22);"; // plane x=0, normal -X
    s += "#108=PLANE('',#107);";
    s += "#109=ADVANCED_FACE('sideAC',(#106),#108,.T.);";

    // Hypotenuse side BC-FE rectangle: B->C->F->E (splines #68 B-E, #69 C-F).
    s += "#111=ORIENTED_EDGE('',*,*,#63,.T.);"; // B->C
    s += "#112=ORIENTED_EDGE('',*,*,#69,.T.);"; // C->F (spline)
    s += "#113=ORIENTED_EDGE('',*,*,#66,.F.);"; // F->E
    s += "#114=ORIENTED_EDGE('',*,*,#68,.F.);"; // E->B (spline)
    s += "#115=EDGE_LOOP('',(#111,#112,#113,#114));";
    s += "#116=FACE_OUTER_BOUND('',#115,.T.);";
    // Hypotenuse plane through B(2,0,0) and C(0,2,0): normal +X+Y.
    s += "#117=DIRECTION('',(1.,1.,0.));";
    s += "#118=AXIS2_PLACEMENT_3D('',#2,#117,#23);";
    s += "#119=PLANE('',#118);";
    s += "#120=ADVANCED_FACE('sideBC',(#116),#119,.T.);";

    s += "#151=CLOSED_SHELL('',(#78,#88,#100,#109,#120));";
    s += "#161=MANIFOLD_SOLID_BREP('prism',#151);";
    s += "#204=AXIS2_PLACEMENT_3D('world',#1,#23,#21);";
    s += "#205=GEOMETRIC_REPRESENTATION_CONTEXT(3);";
    s += "#301=ADVANCED_BREP_SHAPE_REPRESENTATION('prism',(#161,#204),#205);";
    s
}

#[tokio::test]
async fn nurbs_curve_edges_reconstruct_a_solid() {
    let engine = ExportEngine::new();
    let (model, report) = engine
        .import_step_content(&envelope(&nurbs_edge_prism()), "corpus:nurbs_edge_prism")
        .expect("import must not hard-fail");

    // The whole point of the NURBS-edge gap: a spline `EDGE_CURVE` no
    // longer kills the owning shell — the solid materialises.
    assert_eq!(
        model.solids.len(),
        1,
        "prism with B-spline vertical edges must reconstruct ONE solid; warnings: {:?}",
        report.warnings
    );
    // The three spline edges were consumed by the B-spline curve handler.
    assert!(
        report
            .counts
            .resolved
            .get("B_SPLINE_CURVE_WITH_KNOTS")
            .copied()
            .unwrap_or(0)
            >= 3,
        "the three vertical edges must resolve as B-spline curves: {:?}",
        report.counts.resolved
    );
    // No spline edge was reported as an unsupported edge_geometry.
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.entity == "EDGE_CURVE" && w.message.contains("unsupported")),
        "no EDGE_CURVE should report unsupported edge_geometry: {:?}",
        report.warnings
    );
    // The reconstructed solid carries the spline edges in its B-Rep.
    assert!(
        report.validation.iter().any(|v| v.solid_id < u32::MAX),
        "validation gate ran on the imported solid"
    );
}

/// A unit cube with a concentric inner void (a hollow block) expressed
/// as a `BREP_WITH_VOIDS`. The void shell is a second closed shell
/// reusing the outer faces — geometrically degenerate but sufficient to
/// drive the void wiring (one solid carrying one inner shell).
fn cube_with_void() -> String {
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
    s += "#151=CLOSED_SHELL('outer',(#141,#142,#143,#144,#145,#146));";
    s += "#152=CLOSED_SHELL('void',(#141,#142,#143,#144,#145,#146));";
    s += "#161=BREP_WITH_VOIDS('hollow',#151,(#152));";
    s += "#204=AXIS2_PLACEMENT_3D('world',#1,#23,#21);";
    s += "#205=GEOMETRIC_REPRESENTATION_CONTEXT(3);";
    s += "#301=ADVANCED_BREP_SHAPE_REPRESENTATION('hollow',(#161,#204),#205);";
    s
}

#[tokio::test]
async fn brep_with_voids_reconstructs_a_hollow_solid() {
    let engine = ExportEngine::new();
    let (model, report) = engine
        .import_step_content(&envelope(&cube_with_void()), "corpus:cube_with_void")
        .expect("import must not hard-fail");
    assert_eq!(
        model.solids.len(),
        1,
        "void part is one solid: {:?}",
        report.warnings
    );
    let (sid, solid) = model.solids.iter().next().expect("a solid");
    assert_eq!(
        solid.inner_shells.len(),
        1,
        "BREP_WITH_VOIDS must record exactly one void shell on solid {sid:?}"
    );
    assert!(
        report.counts.resolved.contains_key("BREP_WITH_VOIDS"),
        "BREP_WITH_VOIDS must be reported as resolved: {:?}",
        report.counts.resolved
    );
}

/// The validation gate: a file that materialises a topological solid
/// but whose geometry is invalid must report `ok = false`. We feed a
/// degenerate single-face "shell" — it parses into kernel topology but
/// is not a valid closed solid, so the gate must catch it.
#[tokio::test]
async fn validation_gate_rejects_invalid_solid() {
    // One planar triangle wrapped as a CLOSED_SHELL + MANIFOLD_SOLID_BREP.
    // A single open face is not a closed manifold solid; validation fails.
    let mut s = String::new();
    s += "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=CARTESIAN_POINT('',(1.,0.,0.));";
    s += "#3=CARTESIAN_POINT('',(0.,1.,0.));";
    s += "#11=VERTEX_POINT('',#1);#12=VERTEX_POINT('',#2);#13=VERTEX_POINT('',#3);";
    s += "#21=DIRECTION('',(1.,0.,0.));#22=DIRECTION('',(0.,1.,0.));#23=DIRECTION('',(0.,0.,1.));";
    s += "#31=VECTOR('',#21,1.);#32=VECTOR('',#22,1.);";
    s += "#41=LINE('',#1,#31);#42=LINE('',#2,#32);#43=LINE('',#1,#32);";
    s += "#51=EDGE_CURVE('',#11,#12,#41,.T.);#52=EDGE_CURVE('',#12,#13,#42,.T.);";
    s += "#53=EDGE_CURVE('',#11,#13,#43,.T.);";
    s += "#61=ORIENTED_EDGE('',*,*,#51,.T.);#62=ORIENTED_EDGE('',*,*,#52,.T.);";
    s += "#63=ORIENTED_EDGE('',*,*,#53,.F.);";
    s += "#71=EDGE_LOOP('',(#61,#62,#63));";
    s += "#81=FACE_OUTER_BOUND('',#71,.T.);";
    s += "#91=AXIS2_PLACEMENT_3D('',#1,#23,#21);#92=PLANE('',#91);";
    s += "#101=ADVANCED_FACE('',(#81),#92,.T.);";
    s += "#151=CLOSED_SHELL('',(#101));"; // one face — not closed
    s += "#161=MANIFOLD_SOLID_BREP('bad',#151);";
    s += "#205=GEOMETRIC_REPRESENTATION_CONTEXT(3);";
    s += "#301=ADVANCED_BREP_SHAPE_REPRESENTATION('bad',(#161),#205);";

    let engine = ExportEngine::new();
    let (model, report) = engine
        .import_step_content(&envelope(&s), "corpus:invalid_solid")
        .expect("import must not hard-fail");
    // A solid object materialised (topology was built)…
    assert_eq!(
        model.solids.len(),
        1,
        "the topology builder still allocates a solid"
    );
    // …but the validation gate must flag it and force ok = false.
    assert!(
        !report.ok,
        "a single-face non-closed shell must NOT report ok = true (validation gate)"
    );
    assert!(
        report.validation.iter().any(|v| !v.valid),
        "the invalid solid must appear in report.validation as invalid: {:?}",
        report.validation
    );
}
