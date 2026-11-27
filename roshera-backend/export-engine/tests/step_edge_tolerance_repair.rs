// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An edge lying off its face by more than the file's declared accuracy is
//! REPAIRED the way every exchange reader repairs it -- its tolerance raised
//! to the measured gap (OCCT: ShapeFix_Edge::FixSameParameter) -- but never
//! silently: each raise is recorded on the import report with the gap and the
//! tolerance it exceeded, and a gap beyond the repair cap is refused (the
//! edge keeps the declared tolerance, so the part fails validation, and the
//! report says why). 448 edges in 30 CADGenBench parts sit 0.001 to 0.02 mm
//! off their cylinders, tori and planes against a declared 1e-5 mm.

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('gap'),'2;1');\n\
         FILE_NAME('gap.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A 10 x 10 square of line edges at z = 0 bounding a face on the plane
/// z = `lift` -- every edge `lift` off its face.
fn lifted_plate(lift: f64) -> String {
    let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let mut s = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        s += &format!("#{}=CARTESIAN_POINT('',({x:.1},{y:.1},0.));", 10 + i);
        s += &format!("#{}=VERTEX_POINT('',#{});", 20 + i, 10 + i);
    }
    for i in 0..4 {
        let (a, b) = (pts[i], pts[(i + 1) % 4]);
        let (dx, dy) = ((b.0 - a.0) / 10.0, (b.1 - a.1) / 10.0);
        s += &format!("#{}=DIRECTION('',({dx:.1},{dy:.1},0.));", 30 + i);
        s += &format!("#{}=VECTOR('',#{},10.);", 40 + i, 30 + i);
        s += &format!("#{}=LINE('',#{},#{});", 50 + i, 10 + i, 40 + i);
        s += &format!(
            "#{}=EDGE_CURVE('',#{},#{},#{},.T.);",
            60 + i,
            20 + i,
            20 + (i + 1) % 4,
            50 + i
        );
        s += &format!("#{}=ORIENTED_EDGE('',*,*,#{},.T.);", 70 + i, 60 + i);
    }
    s += "#80=EDGE_LOOP('',(#70,#71,#72,#73));";
    s += "#81=FACE_OUTER_BOUND('',#80,.T.);";
    s += &format!("#90=CARTESIAN_POINT('',(0.,0.,{lift:.6}));");
    s += "#91=DIRECTION('',(0.,0.,1.));#92=DIRECTION('',(1.,0.,0.));";
    s += "#93=AXIS2_PLACEMENT_3D('',#90,#91,#92);#94=PLANE('',#93);";
    s += "#95=ADVANCED_FACE('',(#81),#94,.T.);";
    s += "#96=OPEN_SHELL('',(#95));#97=SHELL_BASED_SURFACE_MODEL('',(#96));";
    s
}

#[test]
fn an_edge_within_the_cap_has_its_tolerance_raised_and_reported() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&lifted_plate(0.005)), "gap")
        .expect("imports");
    let raised: Vec<_> = report
        .healings
        .iter()
        .filter(|h| format!("{:?}", h.kind) == "EdgeToleranceRaised")
        .collect();
    assert_eq!(
        raised.len(),
        4,
        "every edge's raise is reported: {:#?}",
        report.healings
    );
    for h in &raised {
        assert!(
            (h.deviation - 0.005).abs() < 1e-6,
            "the measured gap is reported ({})",
            h.deviation
        );
    }
    for (_, e) in model.edges.iter() {
        assert!(
            e.tolerance >= 0.005,
            "edge tolerance raised to the gap ({})",
            e.tolerance
        );
    }
}

/// Between the repair cap (0.01 mm) and the acceptance ceiling (0.05 mm) an
/// edge whose faces truly do not meet is ACCEPTED as a tolerant edge -- what
/// every exchange kernel does silently -- but named: a healing of its own
/// kind carrying the measured gap and the accuracy the file states, and a
/// warning that says so in words with the face it is off.
#[test]
fn an_edge_between_the_cap_and_the_ceiling_is_accepted_and_named() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&lifted_plate(0.03)), "gap")
        .expect("imports");
    let accepted: Vec<_> = report
        .healings
        .iter()
        .filter(|h| format!("{:?}", h.kind) == "EdgeGapAccepted")
        .collect();
    assert_eq!(
        accepted.len(),
        4,
        "every edge's acceptance is reported: {:#?}",
        report.healings
    );
    for h in &accepted {
        assert!(
            (h.deviation - 0.03).abs() < 1e-6,
            "the measured gap is reported ({})",
            h.deviation
        );
        assert!(
            h.tolerance < 0.01,
            "with the accuracy the edge carried from the file ({})",
            h.tolerance
        );
    }
    assert!(
        report
            .healings
            .iter()
            .all(|h| format!("{:?}", h.kind) != "EdgeToleranceRaised"),
        "an acceptance is not passed off as an ordinary raise: {:#?}",
        report.healings
    );
    let said: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| w.message.contains("accepted as a tolerant edge"))
        .collect();
    assert_eq!(said.len(), 4, "{:#?}", report.warnings);
    assert!(
        said.iter()
            .all(|w| w.message.contains("3.000e-2") && w.message.contains("face")),
        "each says the gap and the face: {:#?}",
        said
    );
    for (_, e) in model.edges.iter() {
        assert!(
            e.tolerance >= 0.03 && e.tolerance < 0.0301,
            "the edge carries the measured gap as its tolerance, no more ({})",
            e.tolerance
        );
    }
}

#[test]
fn an_edge_beyond_the_ceiling_is_refused_and_reported() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&lifted_plate(0.06)), "gap")
        .expect("imports");
    assert!(
        report.healings.iter().all(|h| {
            let kind = format!("{:?}", h.kind);
            kind != "EdgeToleranceRaised" && kind != "EdgeGapAccepted"
        }),
        "no raise and no acceptance beyond the ceiling: {:#?}",
        report.healings
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.message.contains("beyond the")),
        "the refusal is reported: {:#?}",
        report.warnings
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.message.contains("6.000e-2") && w.message.contains("0.05")),
        "with the gap and the ceiling it exceeded: {:#?}",
        report.warnings
    );
    for (_, e) in model.edges.iter() {
        assert!(
            e.tolerance < 0.01,
            "edge keeps the declared tolerance ({})",
            e.tolerance
        );
    }
}
