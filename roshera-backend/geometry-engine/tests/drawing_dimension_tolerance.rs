// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A tolerance authored on a dimension is ON THE SHEET.
//!
//! The data model carried size tolerances and the history recorded them, and
//! no renderer ever drew one: the label stayed "Ø20.00". These pin that a
//! tolerance on a diameter AND on a linear dimension is inked in the SVG, the
//! DXF and the PDF, that the certificate states the dimension as inked and
//! checks that ink against the model, and that a dragged dimension moves.

use geometry_engine::drawing::dimension_edit::{DimTolerance, DimensionEdit};
use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::dxf::render_drawing_dxf;
use geometry_engine::drawing::layout::{compute_layout, Rect2, SheetItemKind};
use geometry_engine::drawing::pdf::render_drawing_pdf;
use geometry_engine::drawing::sheet_certificate::{certify_drawing, SheetFactKind, SheetVerdict};
use geometry_engine::drawing::svg::render_drawing_svg;
use geometry_engine::drawing::types::Drawing;
use geometry_engine::gdt::model::{Annotation, DimensionalTolerance};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::readable::bore_face_ids;

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn block() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("tol-block".to_string()));
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(120.0, 60.0, 20.0)
            .unwrap(),
    );
    m.set_event_key(None);
    (m, s)
}

/// A Ø25 x 40 pin: its diameter is a dimension line, not a hole-table row.
fn pin() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("tol-pin".to_string()));
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 12.5, 40.0)
            .unwrap(),
    );
    m.set_event_key(None);
    (m, s)
}

/// The id of the dimension printed `label`. Labels are as the sheet prints
/// them: no trailing zeros (ISO 129-1) -- `120`, `\u{00D8}25` -- while a
/// tolerance keeps the places it is stated to (`+0.10`).
fn dim_id(d: &Drawing, label: &str) -> String {
    d.views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .find(|x| x.label == label)
        .unwrap_or_else(|| {
            panic!(
                "no dimension labelled {label}; the sheet has {:?}",
                d.views
                    .iter()
                    .flat_map(|v| v.dimensions.iter().map(|x| x.label.clone()))
                    .collect::<Vec<_>>()
            )
        })
        .id
        .clone()
}

fn tolerance(t: DimTolerance) -> DimensionEdit {
    DimensionEdit {
        tolerance: Some(t),
        ..DimensionEdit::default()
    }
}

/// The DXF as text, with its `\U+XXXX` escapes (how a DXF writes a character
/// outside ASCII: the plus-minus and diameter signs) decoded.
fn dxf_text(d: &Drawing) -> String {
    let raw = String::from_utf8_lossy(&render_drawing_dxf(d).expect("dxf")).into_owned();
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_str();
    while let Some(i) = rest.find("\\U+") {
        out.push_str(&rest[..i]);
        let hex = rest.get(i + 3..i + 7).unwrap_or("");
        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
            Some(c) if hex.len() == 4 => {
                out.push(c);
                rest = &rest[i + 7..];
            }
            _ => {
                out.push_str("\\U+");
                rest = &rest[i + 3..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A LINEAR dimension takes a tolerance and every renderer inks it.
///
/// Mutation: place dimensions with the measured label instead of the inked
/// one (`inked.label = ink.label` in `place_dimensions_routed`) and every
/// assertion below goes red.
#[test]
fn a_linear_tolerance_is_inked_in_svg_dxf_and_pdf() {
    let (m, s) = block();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let plain_pdf = render_drawing_pdf(&d).expect("pdf");
    assert!(!render_drawing_svg(&d).contains("+0.10"));

    let id = dim_id(&d, "120");
    d.set_dimension_edit(
        &id,
        tolerance(DimTolerance::Bilateral {
            upper: 0.1,
            lower: 0.05,
        }),
        false,
    )
    .expect("a linear dimension takes a tolerance");

    // SVG: the value, then the two deviations stacked, each clickable as the
    // same dimension.
    let svg = render_drawing_svg(&d);
    assert!(svg.contains("dim-toleranced"), "the stacked form is drawn");
    for piece in [">120<", ">+0.10<", ">+0.05<"] {
        assert!(svg.contains(piece), "SVG lacks {piece}");
    }
    assert!(
        svg.matches(&format!("data-dim-id=\"{id}\"")).count() >= 3,
        "value and both deviations carry the dimension's id"
    );
    // DXF: one line of text, ISO 129-1 single-line form.
    assert!(
        dxf_text(&d).contains("120 +0.10/+0.05"),
        "DXF lacks the tolerance"
    );
    // PDF: transcoded from the same SVG; it renders and it changed.
    let pdf = render_drawing_pdf(&d).expect("pdf with a tolerance");
    assert!(pdf.starts_with(b"%PDF-"));
    assert_ne!(pdf, plain_pdf, "the tolerance reached the PDF");

    // Symmetric and limits, same dimension.
    d.set_dimension_edit(&id, tolerance(DimTolerance::Symmetric { pm: 0.2 }), false)
        .expect("symmetric");
    assert!(render_drawing_svg(&d).contains(">120 \u{00B1}0.2<"));
    assert!(dxf_text(&d).contains("120 \u{00B1}0.2"));
    d.set_dimension_edit(
        &id,
        tolerance(DimTolerance::Limits {
            upper: 120.15,
            lower: 119.85,
        }),
        false,
    )
    .expect("limits");
    assert!(dxf_text(&d).contains("120.15/119.85"));
    // Reference and basic.
    d.set_dimension_edit(&id, tolerance(DimTolerance::Reference), false)
        .expect("reference");
    assert!(render_drawing_svg(&d).contains(">(120)<"));
    d.set_dimension_edit(&id, tolerance(DimTolerance::Basic), false)
        .expect("basic");
    assert!(render_drawing_svg(&d).contains("class=\"dim-box\""));
}

/// A DIAMETER takes an ISO 286 class; its limits come from the tables and
/// the certificate states the dimension as inked, with the limits.
#[test]
fn a_diameter_fit_is_inked_with_its_iso_286_limits_and_certified() {
    let (m, s) = pin();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let id = dim_id(&d, "\u{00D8}25");
    d.set_dimension_edit(
        &id,
        DimensionEdit {
            tolerance: Some(DimTolerance::Fit {
                designation: "g6".to_string(),
            }),
            suffix: "TYP".to_string(),
            ..DimensionEdit::default()
        },
        false,
    )
    .expect("g6 on a pin");

    assert!(
        dxf_text(&d).contains("\u{00D8}25 g6 (-0.007/-0.020) TYP"),
        "DXF lacks the fit"
    );
    let svg = render_drawing_svg(&d);
    for piece in [">-0.007<", ">-0.020<", "g6", "TYP"] {
        assert!(svg.contains(piece), "SVG lacks {piece}");
    }

    let cert = certify_drawing(&m, &d);
    let fact = cert
        .facts
        .iter()
        .find(|f| f.kind == SheetFactKind::Dimension && f.label.contains("g6"))
        .expect("the certificate states the dimension as inked");
    assert_eq!(fact.label, "\u{00D8}25 g6 (-0.007/-0.020) TYP");
    assert_eq!(fact.live.verdict, SheetVerdict::Consistent);
    let t = fact.tolerance.as_ref().expect("tolerance in the readback");
    assert_eq!(t.kind, "fit");
    assert_eq!(t.designation.as_deref(), Some("g6"));
    let [lo, hi] = t.limits.expect("resolved limits");
    assert!((lo - 24.980).abs() < 1e-9 && (hi - 24.993).abs() < 1e-9);
    assert!(
        fact.live
            .detail
            .as_deref()
            .is_some_and(|t| t.contains("authored text, not verified") && t.contains("TYP")),
        "authored text is listed as not verified: {:?}",
        fact.live.detail
    );
    assert!(cert.sound);

    // A class the tables do not carry is refused with its reason; nothing
    // changes on the sheet.
    let refused = d.set_dimension_edit(
        &id,
        tolerance(DimTolerance::Fit {
            designation: "za6".to_string(),
        }),
        false,
    );
    assert!(refused.is_err());
    assert!(
        dxf_text(&d).contains("g6"),
        "a refused edit changes nothing"
    );
}

fn bored_plate() -> (BRepModel, SolidId, PersistentId) {
    let mut m = BRepModel::new();
    let plate = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .unwrap(),
    );
    let bore = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 5.0, 80.0)
            .unwrap(),
    );
    let part = boolean_operation(
        &mut m,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap();
    let face = *bore_face_ids(&m, part).iter().next().expect("a bore face");
    let pid = m.face_pids.get(&face).copied().unwrap_or_else(|| {
        let p = PersistentId::root(b"tolerance-bore-face");
        m.face_pids.insert(face, p);
        m.pid_to_face.insert(p, face);
        p
    });
    (m, part, pid)
}

/// A size tolerance on the bore FACE (the GD&T sidecar) is inked in the
/// bore's hole-table row, and the certificate checks that ink against the
/// model: change the tolerance on the model and the sheet is stale.
///
/// Mutation: drop the comparison in `dimension_ink_check` and the stale
/// assertions go red.
#[test]
fn a_feature_tolerance_is_inked_and_goes_stale_when_the_model_changes() {
    let (mut m, part, pid) = bored_plate();
    m.gdt.attach(
        pid,
        Annotation::Dimensional(DimensionalTolerance::fit(10.0, "H7")),
    );
    let d = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let svg = render_drawing_svg(&d);
    assert!(
        svg.contains("H7 (+0.015/0)"),
        "the bore's row states \u{00D8}10 H7 with its limits"
    );
    let fresh = certify_drawing(&m, &d);
    assert!(fresh.sound, "{:?}", fresh.counts);
    assert!(fresh
        .facts
        .iter()
        .any(|f| f.label.contains("H7 (+0.015/0)")));

    // The model's tolerance is re-authored; the sheet still inks H7.
    m.gdt.clear_feature(pid);
    m.gdt.attach(
        pid,
        Annotation::Dimensional(DimensionalTolerance::symmetric(10.0, 0.05)),
    );
    let after = certify_drawing(&m, &d);
    let stale: Vec<_> = after
        .facts
        .iter()
        .filter(|f| {
            f.live.verdict == SheetVerdict::Stale
                && f.live
                    .detail
                    .as_deref()
                    .is_some_and(|t| t.contains("tolerance"))
        })
        .collect();
    assert!(
        !stale.is_empty(),
        "the inked tolerance no longer matches the model: {:?}",
        after
            .facts
            .iter()
            .map(|f| (&f.label, f.live.verdict, &f.live.detail))
            .collect::<Vec<_>>()
    );
    assert!(!after.sound);
}

/// Dragging a dimension moves its line and text by the recorded shift, and
/// nothing else on the sheet.
///
/// Mutation: skip `shift_placed_dimension` and this goes red.
#[test]
fn a_dragged_dimension_moves_by_its_recorded_shift() {
    let (m, s) = block();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let id = dim_id(&d, "120");
    let before = compute_layout(&d);
    let was = before
        .dimensions
        .iter()
        .find(|p| p.dim_id == id)
        .expect("placed")
        .clone();
    let horizontal = (was.line[0][1] - was.line[1][1]).abs() < 1e-9;
    assert!(horizontal, "the 120 width is a horizontal dimension");

    d.set_dimension_edit(
        &id,
        DimensionEdit {
            text_shift_mm: Some([7.0, 5.0]),
            ..DimensionEdit::default()
        },
        false,
    )
    .expect("move");
    let after = compute_layout(&d);
    let now = after
        .dimensions
        .iter()
        .find(|p| p.dim_id == id)
        .expect("placed");
    // The line moves across itself (down 5), the text slides along it (7).
    assert!((now.line[0][1] - was.line[0][1] - 5.0).abs() < 1e-9);
    assert!((now.line[0][0] - was.line[0][0]).abs() < 1e-9);
    assert!((now.text_anchor[0] - was.text_anchor[0] - 7.0).abs() < 1e-9);
    assert!((now.text_anchor[1] - was.text_anchor[1] - 5.0).abs() < 1e-9);
    // The extension lines still start at the part.
    assert!((now.ext[0][0][1] - was.ext[0][0][1]).abs() < 1e-9);
    assert!((now.ext[0][1][1] - was.ext[0][1][1] - 5.0).abs() < 1e-9);
    // Every other dimension is where it was.
    for p in &before.dimensions {
        if p.dim_id == id {
            continue;
        }
        let q = after
            .dimensions
            .iter()
            .find(|q| q.dim_id == p.dim_id && q.owner_view == p.owner_view)
            .expect("still placed");
        assert_eq!(q.text_anchor, p.text_anchor, "{} moved", p.dim_id);
    }
    assert!(d
        .set_dimension_edit(
            &id,
            DimensionEdit {
                text_shift_mm: Some([f64::NAN, 0.0]),
                ..DimensionEdit::default()
            },
            false
        )
        .is_err());
}

/// An edit whose dimension is gone is reported `dangling`; it is not moved
/// to another dimension.
#[test]
fn an_edit_on_a_dimension_that_is_gone_is_dangling_not_retargeted() {
    let (m, s) = block();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    d.apply_dimension_edit(
        "a-key-no-dimension-on-this-sheet-has",
        tolerance(DimTolerance::Symmetric { pm: 0.1 }),
    );
    let cert = certify_drawing(&m, &d);
    let dangling: Vec<_> = cert
        .facts
        .iter()
        .filter(|f| f.live.verdict == SheetVerdict::Dangling)
        .collect();
    assert_eq!(dangling.len(), 1, "{:?}", cert.counts);
    assert!(dangling[0].label.contains("no longer on the sheet"));
    assert!(!cert.sound);
    assert!(
        !render_drawing_svg(&d).contains("\u{00B1}0.1"),
        "the orphaned tolerance is on no dimension"
    );
}

/// Where the sheets below are written for a person to look at: the visual
/// harness's own directory (`ROSHERA_PROBE_OUT`, else a gitignored `target/`
/// scratch directory).
fn emit_svg(name: &str, d: &Drawing) {
    let dir = std::path::PathBuf::from(
        std::env::var("ROSHERA_PROBE_OUT")
            .unwrap_or_else(|_| "target/drawing-visual-harness".to_string()),
    );
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(dir.join(format!("{name}.svg")), render_drawing_svg(d)).expect("write svg");
}

/// A tolerance makes a label longer. On the fixture sheets it must not cost
/// legibility: the layout check finds no more errors with the tolerances on
/// than without. The sheets are written out to be LOOKED at (stacked
/// deviations, a fit with its limits, a basic box, a reference).
#[test]
fn toleranced_sheets_stay_legible_and_are_written_for_looking() {
    use geometry_engine::drawing::verify::verify_drawing;

    let (m, s) = block();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let before = verify_drawing(&d).error_count();
    d.set_dimension_edit(
        &dim_id(&d, "120"),
        tolerance(DimTolerance::Bilateral {
            upper: 0.1,
            lower: -0.05,
        }),
        false,
    )
    .expect("bilateral");
    d.set_dimension_edit(
        &dim_id(&d, "60"),
        tolerance(DimTolerance::Symmetric { pm: 0.2 }),
        false,
    )
    .expect("symmetric");
    d.set_dimension_edit(
        &dim_id(&d, "20"),
        DimensionEdit {
            tolerance: Some(DimTolerance::Limits {
                upper: 20.1,
                lower: 19.9,
            }),
            suffix: "TYP".to_string(),
            ..DimensionEdit::default()
        },
        false,
    )
    .expect("limits");
    emit_svg("tolerance_block", &d);
    let after = verify_drawing(&d);
    assert!(
        after.error_count() <= before,
        "tolerances on the block cost legibility: {:?}",
        after.issues
    );

    let (m, s) = pin();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let before = verify_drawing(&d).error_count();
    d.set_dimension_edit(
        &dim_id(&d, "\u{00D8}25"),
        tolerance(DimTolerance::Fit {
            designation: "g6".to_string(),
        }),
        false,
    )
    .expect("fit");
    d.set_dimension_edit(&dim_id(&d, "40"), tolerance(DimTolerance::Basic), false)
        .expect("basic");
    emit_svg("tolerance_pin", &d);
    // Found by looking: the toleranced diameter is wider than the 25 mm it
    // spans, so its text goes outside -- and it went to the right, across
    // the isometric beside the view. It goes to the side that is free.
    let layout = compute_layout(&d);
    let texts: Vec<_> = layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::DimensionText)
        .collect();
    assert!(!texts.is_empty());
    for t in &texts {
        for g in layout
            .items
            .iter()
            .filter(|i| i.kind == SheetItemKind::ViewGeometry && i.owner_view != t.owner_view)
        {
            // With a millimetre of air: the box is an estimate of the
            // rendered text, and text that just clears a view reads as
            // touching it.
            let aired = Rect2 {
                x0: t.bbox.x0 - 1.0,
                y0: t.bbox.y0 - 1.0,
                x1: t.bbox.x1 + 1.0,
                y1: t.bbox.y1 + 1.0,
            };
            assert!(
                !aired.intersects(&g.bbox, 0.0),
                "dimension text {:?} is printed over (or against) another view",
                t.text
            );
        }
    }
    let after = verify_drawing(&d);
    assert!(
        after.error_count() <= before,
        "a fit on the pin costs legibility: {:?}",
        after.issues
    );
}
