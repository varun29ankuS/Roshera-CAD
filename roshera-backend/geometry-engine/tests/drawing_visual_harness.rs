// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Visual quality harness for engineering drawings (2026-08-16
//! drawing-quality brief, Part 1).
//!
//! Renders fixture parts through the PRODUCTION path
//! (`standard_drawing_auto` -> `render_drawing_svg`), writes the SVG to disk
//! for a human to look at, and asserts MACHINE-CHECKABLE quality invariants
//! — the invariants are the deliverable, not the renders (rasterising to PNG
//! is a manual, optional step outside this harness; CI need not do it).
//!
//! Supersedes `tests/zz_drawing_probe.rs` (deleted, per the brief: "take it
//! as the seed... then delete the zz_ prototype — do not leave both"). That
//! was a throwaway human-in-the-loop probe with no assertions; this is the
//! permanent regression harness it was seeded from.
//!
//! ## Why these two fixtures, and no third
//!
//! - [`flange`] — Ø120×14 disc, Ø50 bore, 4×Ø12 bolt holes on a Ø90 PCD.
//!   THE fixture the 2026-08-16 findings (D1-D8) were taken from: it
//!   exercises HLR, hidden lines, centrelines, the hole table, section-view
//!   attachment and auto-dimensioning all at once. This is the load-bearing
//!   fixture for this harness.
//! - [`plain_box`] — an 80×50×20 box. The cheapest possible sanity check: no
//!   HLR ambiguity, no hole table, no section. It isolates a frame/ink/label
//!   defect from the HLR/section machinery, so a regression caught here
//!   points at the annotation/layout layer specifically, not at geometry.
//!
//! No third fixture was added. A synthetic "boss" part (a positive
//! cylindrical protrusion, NOT a bore) would be the natural specimen for
//! proving the D2 diameter-suppression fix is scoped to TABLED bores only —
//! but a small, un-tabled diameter callout is exactly the shape that can
//! trip invariant #3 (`find_span_overflows`) on its own, unrelated grounds
//! (a small feature's span is narrow at typical drawing scale). Rendering it
//! through this harness would conflate proving the D2 boundary with proving
//! invariant #3, on a fixture invented for the former. That boundary is
//! already proven by a PRE-EXISTING test:
//! `boss_and_od_faces_are_not_tabled_as_holes` in `drawing_quality_oracle.rs`
//! proves a welded boss never enters `drawing.hole_sites`; the D2 fix's
//! `is_tabled` predicate in `src/drawing/layout.rs::place_dimensions` keys
//! strictly off `tabled_face_ids` (built from `hole_sites`), so a diameter
//! dim whose entities never reach `hole_sites` is provably untouched by the
//! suppression — the boundary this harness would otherwise need a new
//! fixture to demonstrate is already load-bearing elsewhere in the suite.
//!
//! ## Invariants asserted (the deliverable)
//!
//! 1. **No annotation text collides.** Two layers: `verify_drawing().passed`
//!    (the crate's pre-existing wired gate — `DimensionLabelCollision`,
//!    `ViewLabelCollision`, `GdtSymbolCollision`; this ALREADY catches the
//!    D3 "A" label / "Ø12.00mm" collision, see
//!    [`flange_sheet_passes_the_wired_quality_gate`] for the historical RED
//!    evidence), plus [`geometry_engine::drawing::verify::find_text_collisions`]
//!    — a stricter, kind-unrestricted pairwise check that also covers
//!    pairings the wired gate does not (e.g. two `NoteText` lines). Each
//!    text-node bbox is approximated from its anchor, font size and glyph
//!    count (`GLYPH_ADVANCE_EM * font_mm` per character) — NOT exact font
//!    metrics. This is stated honestly as an approximation: it over-
//!    estimates width (0.62 em is conservative/wide for the faces this
//!    renders with), so it never UNDER-detects a real glyph-through-glyph
//!    overlap, which is the failure mode that matters.
//! 2. **All ink lies inside the frame.**
//!    [`geometry_engine::drawing::verify::find_ink_outside_frame`] — every
//!    layout item (not only view geometry) inside the sheet's drawing frame.
//! 3. **Every dimension's text is legible at its own scale.**
//!    [`geometry_engine::drawing::verify::find_span_overflows`] — no
//!    dimension's approximate label width exceeds the span it is centred on
//!    (place_dimensions never moves a label outside the extension lines
//!    onto a leader, so an oversized label there is unreadable).
//! 4. **A view carrying a cutting plane has outline geometry, not only
//!    hatch.** [`geometry_engine::drawing::verify::section_shows_only_hatch`]
//!    — `#[ignore]`d in [`section_a_a_shows_more_than_the_cut_faces`]: this
//!    is finding **D1**, deliberately out of scope for this change (the
//!    section-view repair is Part 3). Confirmed RED against the live flange
//!    fixture; un-ignore it once D1 lands.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::layout::compute_layout;
use geometry_engine::drawing::svg::render_drawing_svg;
use geometry_engine::drawing::verify::{
    find_ink_outside_frame, find_span_overflows, find_text_collisions, frame_rect,
    section_shows_only_hatch, verify_drawing,
};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Where fixture SVGs are written — `ROSHERA_PROBE_OUT` if set (kept for
/// continuity with the deleted probe's env var), else a `target/` scratch
/// directory (gitignored) rather than the crate root, so running this
/// harness never litters the working tree with `.svg` files.
fn out_dir() -> std::path::PathBuf {
    let p = std::path::PathBuf::from(
        std::env::var("ROSHERA_PROBE_OUT")
            .unwrap_or_else(|_| "target/drawing-visual-harness".to_string()),
    );
    std::fs::create_dir_all(&p).ok();
    p
}

/// Write `drawing`'s SVG to `<out_dir>/<name>.svg` and return the rendered
/// [`geometry_engine::drawing::types::Drawing`] path for the caller's
/// assertions. The write is the harness's human-review artifact; nothing
/// downstream depends on the file existing.
fn emit_svg(name: &str, drawing: &geometry_engine::drawing::types::Drawing) -> std::path::PathBuf {
    let svg = render_drawing_svg(drawing);
    let path = out_dir().join(format!("{name}.svg"));
    std::fs::write(&path, &svg).expect("write svg");
    path
}

// ── Fixtures ──────────────────────────────────────────────────────────────

/// Ø120×14 flange disc, Ø50 through bore, 4×Ø12 bolt holes on a Ø90 PCD —
/// the demo-catalog shape, and the exact fixture the 2026-08-16 findings
/// were taken from (probe_flange.svg/.png in the session that wrote this
/// harness).
fn flange() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let disc = match TopologyBuilder::new(&mut m)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            60.0,
            14.0,
        )
        .expect("disc")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let bore = match TopologyBuilder::new(&mut m)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, -5.0),
            Vector3::new(0.0, 0.0, 1.0),
            25.0,
            24.0,
        )
        .expect("bore")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let mut cur = boolean_operation(
        &mut m,
        disc,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore cut");
    for (i, (x, y)) in [(45.0, 0.0), (-45.0, 0.0), (0.0, 45.0), (0.0, -45.0)]
        .iter()
        .enumerate()
    {
        let hole = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(*x, *y, -5.0),
                Vector3::new(0.0, 0.0, 1.0),
                6.0,
                24.0,
            )
            .expect("hole")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        cur = boolean_operation(
            &mut m,
            cur,
            hole,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .unwrap_or_else(|e| panic!("bolt hole {i} cut: {e:?}"));
    }
    (m, cur)
}

/// 80×50×20 box — the simplest thing the sheet generator can be asked for.
fn plain_box() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let sid = match TopologyBuilder::new(&mut m)
        .create_box_3d(80.0, 50.0, 20.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    (m, sid)
}

// ── Invariant #1: no annotation text collides ───────────────────────────────

/// THE PAYOFF: the flange sheet — the exact geometry finding D3 was taken
/// from ("the section-arrow label 'A' is drawn straight through the
/// 'Ø12.00mm' text") — now passes the crate's existing wired quality gate.
///
/// RED evidence (pre-fix, captured live in this session before D2/D4 landed,
/// same fixture, same `verify_drawing` call — this check ALREADY existed,
/// nothing new was added to catch it):
///   `passed=false`
///   `issue Error DimensionLabelCollision view=None msg=callout 'Ø12.00mm' overlaps callout 'A'`
///
/// The actual harness gap was not a missing invariant — it was that no
/// PERMANENT test ever ran this exact fixture through `verify_drawing`. The
/// crate's other fixtures (`six_hole_plate`, `ring_plate` in
/// `drawing_quality_oracle.rs`) don't reproduce D3's geometry (smaller
/// bolt circle, different sheet size), so they stayed green throughout.
///
/// GREEN after D2 (table-delegation for tabled diameters) + D4 (drop the
/// redundant "mm" suffix): the offending "Ø12.00mm" callout no longer
/// renders at all (delegated to the hole table), so there is nothing left
/// for the "A" label to collide with.
#[test]
fn flange_sheet_passes_the_wired_quality_gate() {
    let (m, part) = flange();
    let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    emit_svg("harness_flange", &drawing);
    let report = verify_drawing(&drawing);
    assert!(
        report.passed,
        "flange sheet must pass the wired quality gate; issues={:?}",
        report.issues
    );
    assert!(
        !report.has(geometry_engine::drawing::DrawingIssueKind::DimensionLabelCollision),
        "D3 must not recur: no callout may collide with the section-arrow label"
    );
}

/// The box sheet — no HLR ambiguity, no hole table, no section — must also
/// pass. Isolates a regression in the frame/label/dimension layer from the
/// HLR/section machinery the flange also exercises.
#[test]
fn box_sheet_passes_the_wired_quality_gate() {
    let (m, part) = plain_box();
    let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    emit_svg("harness_box", &drawing);
    let report = verify_drawing(&drawing);
    assert!(
        report.passed,
        "box sheet must pass the wired quality gate; issues={:?}",
        report.issues
    );
}

/// The STRICTER, kind-unrestricted text-collision sweep — every pair of
/// text-carrying sheet items, not only the pairings `verify_drawing`'s wired
/// checks name. Must be empty on both fixtures.
#[test]
fn no_text_collisions_on_either_fixture() {
    for (name, (m, part)) in [("flange", flange()), ("box", plain_box())] {
        let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
        let layout = compute_layout(&drawing);
        let hits = find_text_collisions(&layout);
        assert!(
            hits.is_empty(),
            "{name}: no annotation text may collide; collisions: {:#?}",
            hits.iter()
                .map(|c| format!(
                    "{:?} '{}' @ ({:.1},{:.1})-({:.1},{:.1}) overlaps {:?} '{}' @ ({:.1},{:.1})-({:.1},{:.1})",
                    c.a_kind, c.a_text, c.a_bbox.x0, c.a_bbox.y0, c.a_bbox.x1, c.a_bbox.y1,
                    c.b_kind, c.b_text, c.b_bbox.x0, c.b_bbox.y0, c.b_bbox.x1, c.b_bbox.y1,
                ))
                .collect::<Vec<_>>()
        );
    }
}

// ── Invariant #2: all ink lies inside the frame ─────────────────────────────

#[test]
fn no_ink_outside_frame_on_either_fixture() {
    for (name, (m, part)) in [("flange", flange()), ("box", plain_box())] {
        let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
        let layout = compute_layout(&drawing);
        let frame = frame_rect(&drawing.sheet_size);
        let hits = find_ink_outside_frame(&layout, frame);
        assert!(
            hits.is_empty(),
            "{name}: all ink must lie inside the frame {:?}; offenders: {:#?}",
            frame,
            hits.iter()
                .map(|o| format!(
                    "{:?} '{}' @ ({:.1},{:.1})-({:.1},{:.1})",
                    o.kind, o.text, o.bbox.x0, o.bbox.y0, o.bbox.x1, o.bbox.y1
                ))
                .collect::<Vec<_>>()
        );
    }
}

// ── Invariant #3: every dimension's text is legible at its own scale ───────

#[test]
fn no_dimension_text_overflows_its_span_on_either_fixture() {
    for (name, (m, part)) in [("flange", flange()), ("box", plain_box())] {
        let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
        let layout = compute_layout(&drawing);
        let hits = find_span_overflows(&layout);
        assert!(
            hits.is_empty(),
            "{name}: no dimension label may be wider than its own span while \
             centred inline (not placed outside / on a leader); offenders: {:#?}",
            hits.iter()
                .map(|o| format!(
                    "'{}' text_w={:.2}mm > span={:.2}mm @ ({:.1},{:.1})",
                    o.label, o.text_width_mm, o.span_mm, o.text_anchor[0], o.text_anchor[1]
                ))
                .collect::<Vec<_>>()
        );
    }
}

// ── Invariant #4: a section carries outline geometry, not only hatch ───────

/// **D1 — NOT fixed by this change.** The section-view repair is a separate
/// pass (Part 3 of the 2026-08-16 brief): today `section_view.rs` renders
/// only the cut cross-section (hatched material) with no silhouette for
/// anything behind the cutting plane, so SECTION A-A reads as disconnected
/// hatched rectangles rather than a part.
///
/// `#[ignore]`d so the crate stays green; un-ignore this once D1 lands. It
/// is not vacuously green today — `section_shows_only_hatch` was confirmed
/// to fire on this exact fixture (see
/// `drawing::verify::harness_invariant_tests::flange_section_view_is_flagged_today`
/// in `src/drawing/verify.rs`, which asserts the RED state directly so a
/// mistaken "staged but already passing" invariant would be caught there,
/// not silently skipped here).
#[test]
#[ignore = "D1 (2026-08-16 brief): section view repair is Part 3, not this change"]
fn section_a_a_shows_more_than_the_cut_faces() {
    let (m, part) = flange();
    let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let section = drawing
        .views
        .iter()
        .find(|v| v.name == "SECTION A-A")
        .expect("flange sheet must carry a SECTION A-A view");
    assert!(
        !section_shows_only_hatch(section),
        "D1: SECTION A-A must show outline geometry beyond the hatched cut \
         faces (the bore wall, the far side of the bolt holes, the outer \
         profile) — today it shows only the cut"
    );
}

// ── D2 / D4 regression proofs, direct to the finding ────────────────────────

/// **D2**: tabled bore diameters (Ø12 ×4, Ø50) must not render as floating
/// linear dimensions parked below a view — they are represented in the hole
/// table instead (table-delegation, not a leader callout; see
/// `place_dimensions`'s doc comment in `src/drawing/layout.rs` for the
/// ruling and its justification).
#[test]
fn flange_diameter_dims_are_table_delegated_not_floating() {
    let (m, part) = flange();
    let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    assert!(
        !drawing.hole_sites.is_empty(),
        "fixture must have hole sites, else this proves nothing"
    );
    let layout = compute_layout(&drawing);
    let floating_diameters: Vec<&str> = layout
        .dimensions
        .iter()
        .filter(|pd| pd.label.starts_with('\u{00d8}') || pd.label.starts_with("S\u{00d8}"))
        .map(|pd| pd.label.as_str())
        .collect();
    assert!(
        floating_diameters.is_empty(),
        "D2: tabled bore diameters must not appear as floating dims; found {floating_diameters:?}"
    );
}

/// **D4**: no dimension label (on any view) and no hole-table Ø cell carries
/// the redundant "mm" suffix — the sheet already declares "ALL DIMENSIONS IN
/// MILLIMETRES UNLESS OTHERWISE STATED" once, and the hole table's own Ø
/// column header makes the per-cell suffix doubly redundant there.
#[test]
fn flange_labels_omit_the_redundant_unit_suffix() {
    let (m, part) = flange();
    let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let suffixed_dims: Vec<String> = drawing
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .filter(|d| d.label.ends_with("mm"))
        .map(|d| d.label.clone())
        .collect();
    assert!(
        suffixed_dims.is_empty(),
        "D4: dimension labels must omit the mm suffix; found {suffixed_dims:?}"
    );
    assert!(
        !drawing.hole_sites.is_empty(),
        "fixture must have hole sites, else the table assertion below proves nothing"
    );
    let suffixed_table: Vec<&str> = drawing
        .hole_sites
        .iter()
        .filter(|s| s.dia_label.ends_with("mm"))
        .map(|s| s.dia_label.as_str())
        .collect();
    assert!(
        suffixed_table.is_empty(),
        "D4: hole-table Ø cells must also omit the suffix; found {suffixed_table:?}"
    );
}
