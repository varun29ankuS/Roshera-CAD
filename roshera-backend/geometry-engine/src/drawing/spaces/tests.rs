//! Synthetic sheets for the space map: one firing case per rule, a clean
//! case beside each, and the grade as a minimum.
//!
//! Mutations these catch: drop any one rule from `space_map` (its firing
//! test goes red); make `grade_of` a mean (`the_grade_is_the_worst_space`
//! goes red).

use super::*;
use crate::drawing::layout::{ArrowSpec, PlacedDimension, SheetItem, SheetItemKind, SheetLayout};
use crate::drawing::types::{
    Drawing, Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, SheetSize, ViewExtent,
    ViewSource,
};

const GEO: Rect2 = Rect2 {
    x0: 100.0,
    y0: 100.0,
    x1: 160.0,
    y1: 140.0,
};

fn view(name: &str, projection: ProjectionType) -> ProjectedView {
    ProjectedView {
        id: ProjectedViewId::new(),
        name: name.to_string(),
        projection,
        source: ViewSource::Part {
            part_id: uuid::Uuid::nil(),
            solid_id: 0,
        },
        position_mm: [0.0, 0.0],
        scale: 1.0,
        polylines: vec![Polyline2d::from_points(vec![[0.0, 0.0], [1.0, 1.0]])],
        extent: ViewExtent {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 1.0,
            max_y: 1.0,
        },
        dimensions: Vec::new(),
        centerlines: Vec::new(),
        hidden_polylines: Vec::new(),
        circles: Vec::new(),
        hidden_circles: Vec::new(),
        ellipses: Vec::new(),
        hidden_ellipses: Vec::new(),
        show_hidden: true,
        shaded_raster: None,
        hatch_polylines: Vec::new(),
        polyline_sources: Vec::new(),
    }
}

/// A horizontal dimension over `geo` from `x0` to `x1`, its line `d` mm
/// above the outline, its number TGAP above the line.
fn hdim(id: &str, label: &str, x0: f64, x1: f64, d: f64, geo: &Rect2) -> PlacedDimension {
    let y = geo.y0 - d;
    PlacedDimension {
        line: [[x0, y], [x1, y]],
        ext: [
            [[x0, geo.y0 - 1.5], [x0, y - 1.5]],
            [[x1, geo.y0 - 1.5], [x1, y - 1.5]],
        ],
        arrows: [
            ArrowSpec {
                tip: [x0, y],
                dir: [1.0, 0.0],
            },
            ArrowSpec {
                tip: [x1, y],
                dir: [-1.0, 0.0],
            },
        ],
        text_anchor: [0.5 * (x0 + x1), y - 1.4],
        text_rot_deg: 0.0,
        label: label.to_string(),
        dim_id: id.to_string(),
        owner_view: 0,
        stack: None,
        boxed: false,
        ext_breaks: Default::default(),
        arc: None,
    }
}

/// A leader from `tip` to `elbow`, shoulder `w` mm to the right.
fn leader(id: &str, label: &str, tip: [f64; 2], elbow: [f64; 2], w: f64) -> PlacedDimension {
    let end = [elbow[0] + w, elbow[1]];
    PlacedDimension {
        line: [elbow, end],
        ext: [[tip, elbow], [elbow, elbow]],
        arrows: [
            ArrowSpec {
                tip,
                dir: [1.0, 0.0],
            },
            ArrowSpec {
                tip: end,
                dir: [0.0, 0.0],
            },
        ],
        text_anchor: [elbow[0] + 0.5 * w, elbow[1] - 1.4],
        text_rot_deg: 0.0,
        label: label.to_string(),
        dim_id: id.to_string(),
        owner_view: 0,
        stack: None,
        boxed: false,
        ext_breaks: Default::default(),
        arc: None,
    }
}

fn item(kind: SheetItemKind, bbox: Rect2, owner: Option<usize>, text: Option<&str>) -> SheetItem {
    SheetItem {
        kind,
        bbox,
        owner_view: owner,
        text: text.map(str::to_string),
    }
}

/// A4 (no 3.5 mm requirement), one FRONT view at `GEO`, no title block.
fn sheet(size: SheetSize) -> Drawing {
    let mut d = Drawing::new("synthetic", size);
    d.views.push(view("FRONT", ProjectionType::Front));
    d
}

fn layout_of(d: &Drawing, geos: &[Rect2], dims: Vec<PlacedDimension>) -> SheetLayout {
    let mut items = Vec::new();
    for (vi, g) in geos.iter().enumerate() {
        items.push(item(SheetItemKind::ViewGeometry, *g, Some(vi), None));
    }
    for pd in &dims {
        items.push(item(
            SheetItemKind::DimensionText,
            placed_text_box(pd),
            Some(pd.owner_view),
            Some(&pd.label),
        ));
    }
    SheetLayout {
        sheet: Rect2 {
            x0: 0.0,
            y0: 0.0,
            x1: d.sheet_size.width(),
            y1: d.sheet_size.height(),
        },
        items,
        dimensions: dims,
        hole_tags: Vec::new(),
        gdt_leaders: Vec::new(),
    }
}

fn rules_in(map: &SpaceMap, space: &str) -> Vec<Rule> {
    map.space(space)
        .map(|s| s.findings.iter().map(|f| f.rule).collect())
        .unwrap_or_default()
}

fn has_rule(map: &SpaceMap, rule: Rule) -> bool {
    map.findings.iter().any(|f| f.rule == rule)
}

// ── LANE-PITCH ────────────────────────────────────────────────────────────

#[test]
fn mis_pitched_lanes_get_lane_pitch_on_their_band() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(
        &d,
        &[GEO],
        vec![
            hdim("a", "60", 100.0, 160.0, 10.0, &GEO),
            hdim("b", "30", 100.0, 130.0, 13.0, &GEO),
        ],
    );
    let map = space_map(&d, &l);
    assert!(
        rules_in(&map, "view:FRONT/band:top").contains(&Rule::LanePitch),
        "lanes 3 mm apart: {:#?}",
        map.findings
    );
    let f = map
        .findings
        .iter()
        .find(|f| f.rule == Rule::LanePitch)
        .map(|f| f.measured)
        .unwrap_or(0.0);
    assert!((f - 3.0).abs() < 1e-9, "measured pitch {f}");
    assert!(map.space("view:FRONT/band:top/lane:1").is_some());
    assert!(map.space("view:FRONT/band:top/lane:2").is_some());
}

#[test]
fn correctly_pitched_lanes_are_clean() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(
        &d,
        &[GEO],
        vec![
            hdim("a", "60", 100.0, 160.0, 10.0, &GEO),
            hdim("b", "30", 100.0, 130.0, 17.5, &GEO),
        ],
    );
    let map = space_map(&d, &l);
    assert!(!has_rule(&map, Rule::LanePitch), "{:#?}", map.findings);
}

#[test]
fn a_first_lane_too_close_to_the_outline_is_lane_pitch() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(&d, &[GEO], vec![hdim("a", "60", 100.0, 160.0, 4.0, &GEO)]);
    let map = space_map(&d, &l);
    assert!(rules_in(&map, "view:FRONT/band:top").contains(&Rule::LanePitch));
}

// ── ALIGN-ROW ─────────────────────────────────────────────────────────────

#[test]
fn a_misaligned_row_gets_align_row() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(
        &d,
        &[GEO],
        vec![
            hdim("a", "20", 100.0, 120.0, 10.0, &GEO),
            hdim("b", "40", 120.0, 160.0, 11.5, &GEO),
        ],
    );
    let map = space_map(&d, &l);
    let lane = map
        .space("view:FRONT/band:top/lane:1")
        .expect("one lane holds the row");
    assert_eq!(
        lane.contents.placements.len(),
        2,
        "1.5 mm off stays one row"
    );
    assert!(lane.findings.iter().any(|f| f.rule == Rule::AlignRow
        && f.severity == Severity::Error
        && (f.measured - 1.5).abs() < 1e-9));
    assert!(map.space("view:FRONT/band:top/lane:2").is_none());
}

#[test]
fn an_aligned_row_is_clean() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(
        &d,
        &[GEO],
        vec![
            hdim("a", "20", 100.0, 120.0, 10.0, &GEO),
            hdim("b", "40", 120.0, 160.0, 10.0, &GEO),
        ],
    );
    let map = space_map(&d, &l);
    assert!(!has_rule(&map, Rule::AlignRow), "{:#?}", map.findings);
}

// ── TEXT-HEIGHT ───────────────────────────────────────────────────────────

#[test]
fn title_block_labels_under_2_5_mm_get_text_height() {
    let d = sheet(SheetSize::A4);
    let mut l = layout_of(&d, &[GEO], vec![hdim("a", "60", 100.0, 160.0, 10.0, &GEO)]);
    let tb = crate::drawing::title_block::title_block_layout(&d).outer;
    l.items
        .push(item(SheetItemKind::TitleBlock, tb, None, None));
    let map = space_map(&d, &l);
    let f = map
        .space("title-block")
        .and_then(|s| s.findings.iter().find(|f| f.rule == Rule::TextHeight))
        .expect("the 2.4 mm field labels are 1.68 mm capitals");
    assert_eq!(f.severity, Severity::Error);
    assert!(f.measured < TEXT_HEIGHT_MIN_MM);
    // A4: 2.52 mm dimension capitals meet the minimum.
    assert!(!rules_in(&map, "view:FRONT/band:top/lane:1").contains(&Rule::TextHeight));
}

#[test]
fn dimension_text_under_3_5_mm_on_a3_gets_text_height() {
    let d = sheet(SheetSize::A3);
    let l = layout_of(&d, &[GEO], vec![hdim("a", "60", 100.0, 160.0, 10.0, &GEO)]);
    let map = space_map(&d, &l);
    let f = map
        .space("view:FRONT/band:top/lane:1")
        .and_then(|s| s.findings.iter().find(|f| f.rule == Rule::TextHeight))
        .expect("2.52 mm capitals on A3");
    assert_eq!(f.severity, Severity::Warning);
    assert_eq!(f.tolerance.min, Some(TEXT_HEIGHT_LARGE_SHEET_MM));
}

// ── TRAILING-ZERO ─────────────────────────────────────────────────────────

#[test]
fn trailing_zeros_are_counted_in_decimals_only() {
    assert_eq!(trailing_zero_count("45.00"), 1);
    assert_eq!(trailing_zero_count("\u{00D8}0.50"), 1);
    assert_eq!(trailing_zero_count("45"), 0);
    assert_eq!(trailing_zero_count("100"), 0);
    assert_eq!(trailing_zero_count("12.5"), 0);
    assert_eq!(trailing_zero_count("1.05"), 0);
    assert_eq!(trailing_zero_count("2\u{00D7} 10.0 \u{00D7} 45\u{00B0}"), 1);
}

#[test]
fn a_value_with_trailing_zeros_gets_trailing_zero() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(
        &d,
        &[GEO],
        vec![hdim("a", "45.00", 100.0, 160.0, 10.0, &GEO)],
    );
    let map = space_map(&d, &l);
    assert!(rules_in(&map, "view:FRONT/band:top/lane:1").contains(&Rule::TrailingZero));
    let clean = layout_of(&d, &[GEO], vec![hdim("a", "45", 100.0, 160.0, 10.0, &GEO)]);
    assert!(!has_rule(&space_map(&d, &clean), Rule::TrailingZero));
}

// ── TEXT-CLEAR ────────────────────────────────────────────────────────────

#[test]
fn overlapping_numbers_get_text_clear() {
    let d = sheet(SheetSize::A4);
    // Two leaders whose numbers print on top of each other.
    let l = layout_of(
        &d,
        &[GEO],
        vec![
            leader("a", "R5", [110.0, 110.0], [90.0, 80.0], 12.0),
            leader("b", "R6", [115.0, 112.0], [91.0, 80.5], 12.0),
        ],
    );
    let map = space_map(&d, &l);
    assert!(map
        .findings
        .iter()
        .any(|f| f.rule == Rule::TextClear && f.severity == Severity::Error));
}

#[test]
fn numbers_a_lane_apart_are_text_clear() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(
        &d,
        &[GEO],
        vec![
            hdim("a", "60", 100.0, 160.0, 10.0, &GEO),
            hdim("b", "20", 100.0, 120.0, 18.0, &GEO),
        ],
    );
    let map = space_map(&d, &l);
    assert!(!has_rule(&map, Rule::TextClear), "{:#?}", map.findings);
}

// ── CROWDING ──────────────────────────────────────────────────────────────

#[test]
fn a_band_packed_with_text_gets_crowding() {
    let d = sheet(SheetSize::A4);
    let small = Rect2 {
        x0: 100.0,
        y0: 100.0,
        x1: 110.0,
        y1: 110.0,
    };
    let l = layout_of(
        &d,
        &[small],
        vec![hdim("a", "1234567890123", 100.0, 110.0, 4.0, &small)],
    );
    let map = space_map(&d, &l);
    assert!(
        rules_in(&map, "view:FRONT/band:top").contains(&Rule::Crowding),
        "{:#?}",
        map.findings
    );
}

// ── EMPTY ─────────────────────────────────────────────────────────────────

#[test]
fn a_tiny_view_on_a_big_sheet_is_empty() {
    let d = sheet(SheetSize::A2);
    let tiny = Rect2 {
        x0: 100.0,
        y0: 100.0,
        x1: 120.0,
        y1: 110.0,
    };
    let l = layout_of(&d, &[tiny], Vec::new());
    let map = space_map(&d, &l);
    assert!(map.space("frame").is_some_and(|s| s
        .findings
        .iter()
        .any(|f| f.rule == Rule::Empty && f.severity == Severity::Error)));
    assert!(
        map.spaces.iter().any(|s| s.kind == SpaceKind::Free),
        "an empty sheet has free paper"
    );
}

#[test]
fn free_rectangles_avoid_ink_and_are_deterministic() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(&d, &[GEO], vec![hdim("a", "60", 100.0, 160.0, 10.0, &GEO)]);
    let a = space_map(&d, &l);
    let b = space_map(&d, &l);
    let frees: Vec<&Space> = a
        .spaces
        .iter()
        .filter(|s| s.kind == SpaceKind::Free)
        .collect();
    assert!(!frees.is_empty());
    for f in &frees {
        assert!(!f.rect.intersects(&GEO, 0.0), "{} covers the view", f.id);
        assert!(f.rect.width() >= FREE_MIN_SIDE_MM && f.rect.height() >= FREE_MIN_SIDE_MM);
    }
    let ids = |m: &SpaceMap| -> Vec<(String, [f64; 4])> {
        m.spaces
            .iter()
            .map(|s| (s.id.clone(), [s.rect.x0, s.rect.y0, s.rect.x1, s.rect.y1]))
            .collect()
    };
    assert_eq!(ids(&a), ids(&b));
    // Largest first.
    for w in frees.windows(2) {
        assert!(w[0].rect.area() >= w[1].rect.area());
    }
}

// ── TABLE-ANCHOR ──────────────────────────────────────────────────────────

#[test]
fn a_floating_table_gets_table_anchor_and_an_anchored_one_does_not() {
    let d = sheet(SheetSize::A4);
    let frame = frame_rect(&d.sheet_size);
    let floating = Rect2 {
        x0: 60.0,
        y0: 200.0,
        x1: 90.0,
        y1: 220.0,
    };
    let mut l = layout_of(&d, &[GEO], Vec::new());
    l.items
        .push(item(SheetItemKind::HoleTableBorder, floating, None, None));
    let map = space_map(&d, &l);
    assert!(rules_in(&map, "table:1").contains(&Rule::TableAnchor));
    assert!(map
        .space("tables")
        .is_some_and(|s| s.contents.tables == vec!["table:1"]));

    let anchored = Rect2 {
        x0: frame.x1 - 32.0,
        y0: frame.y0,
        x1: frame.x1,
        y1: frame.y0 + 20.0,
    };
    let mut l = layout_of(&d, &[GEO], Vec::new());
    l.items
        .push(item(SheetItemKind::HoleTableBorder, anchored, None, None));
    assert!(!has_rule(&space_map(&d, &l), Rule::TableAnchor));
}

// ── VIEW-ALIGN ────────────────────────────────────────────────────────────

#[test]
fn a_top_view_off_its_projection_line_gets_view_align() {
    let mut d = sheet(SheetSize::A3);
    d.views.push(view("TOP", ProjectionType::Top));
    let top_off = Rect2 {
        x0: 105.0,
        y0: 40.0,
        x1: 165.0,
        y1: 80.0,
    };
    let l = layout_of(&d, &[GEO, top_off], Vec::new());
    let map = space_map(&d, &l);
    let f = map
        .space("view:TOP")
        .and_then(|s| s.findings.iter().find(|f| f.rule == Rule::ViewAlign))
        .expect("5 mm off FRONT's column");
    assert_eq!(f.severity, Severity::Error);
    assert!((f.measured - 5.0).abs() < 1e-9);

    let top_on = Rect2 {
        x0: 100.0,
        ..top_off
    };
    let top_on = Rect2 {
        x1: 160.0,
        ..top_on
    };
    assert!(!has_rule(
        &space_map(&d, &layout_of(&d, &[GEO, top_on], Vec::new())),
        Rule::ViewAlign
    ));
}

// ── LEADER-ANGLE ──────────────────────────────────────────────────────────

#[test]
fn a_flat_leader_gets_leader_angle_and_a_45_degree_one_does_not() {
    let d = sheet(SheetSize::A4);
    // Leg from (100, 120) to (60, 115): about 7 degrees to the shoulder.
    let flat = leader("a", "R5", [100.0, 120.0], [60.0, 115.0], 10.0);
    let map = space_map(&d, &layout_of(&d, &[GEO], vec![flat]));
    assert!(has_rule(&map, Rule::LeaderAngle), "{:#?}", map.findings);
    // 45 degrees: leg (100,120) -> (80,100).
    let good = leader("a", "R5", [100.0, 120.0], [70.0, 90.0], 10.0);
    let map = space_map(&d, &layout_of(&d, &[GEO], vec![good]));
    assert!(!has_rule(&map, Rule::LeaderAngle), "{:#?}", map.findings);
}

// ── Grade ─────────────────────────────────────────────────────────────────

/// Many clean spaces and one bad one: the grade is the bad space's score,
/// below the mean. Mutation: `grade_of` as a mean -> RED.
#[test]
fn the_grade_is_the_worst_space() {
    let d = sheet(SheetSize::A4);
    // A view that fills the sheet, so the one bad space is the row.
    let big = Rect2 {
        x0: 20.0,
        y0: 100.0,
        x1: 195.0,
        y1: 280.0,
    };
    let l = layout_of(
        &d,
        &[big],
        vec![
            hdim("a", "40", 20.0, 60.0, 10.0, &big),
            hdim("b", "135", 60.0, 195.0, 11.5, &big),
        ],
    );
    let map = space_map(&d, &l);
    let worst = map
        .spaces
        .iter()
        .map(|s| s.score)
        .fold(f64::INFINITY, f64::min);
    let mean = map.spaces.iter().map(|s| s.score).sum::<f64>() / map.spaces.len() as f64;
    assert!(map.spaces.len() > 5, "a sheet of many spaces");
    assert!(
        (map.grade - worst).abs() < 1e-12,
        "grade {} worst {worst}",
        map.grade
    );
    assert!(
        map.grade < mean,
        "grade {} is not the mean {mean}",
        map.grade
    );
    assert!(map.grade <= ERROR_FACTOR, "one error caps the sheet");
    assert_eq!(
        map.below_bar,
        map.spaces.iter().filter(|s| s.score < SPACE_BAR).count()
    );
    assert!(map.below_bar >= 1);
    assert_eq!(map.worst.as_deref(), Some("view:FRONT/band:top/lane:1"));
    // Worst first.
    assert_eq!(map.findings[0].severity, Severity::Error);
}

#[test]
fn every_rule_has_a_stable_id_and_basis() {
    let ids: Vec<&str> = Rule::ALL.iter().map(|r| r.id()).collect();
    assert_eq!(
        ids,
        [
            "ALIGN-ROW",
            "LANE-PITCH",
            "TEXT-CLEAR",
            "CROWDING",
            "EMPTY",
            "TABLE-ANCHOR",
            "VIEW-ALIGN",
            "LEADER-ANGLE",
            "TEXT-HEIGHT",
            "TRAILING-ZERO",
            "DATUM-ATTACH"
        ]
    );
    for r in Rule::ALL {
        let json = serde_json::to_string(&r).unwrap_or_default();
        assert_eq!(json, format!("\"{}\"", r.id()));
        assert!(!r.basis().is_empty());
    }
}

#[test]
fn focus_keeps_a_space_and_its_descendants() {
    let d = sheet(SheetSize::A4);
    let l = layout_of(
        &d,
        &[GEO],
        vec![
            hdim("a", "20", 100.0, 120.0, 10.0, &GEO),
            hdim("b", "40", 120.0, 160.0, 11.5, &GEO),
        ],
    );
    let map = space_map(&d, &l);
    let f = map.focus("view:FRONT/band:top").expect("a band");
    let ids: Vec<&str> = f.spaces.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["view:FRONT/band:top", "view:FRONT/band:top/lane:1"]);
    assert!(f
        .findings
        .iter()
        .all(|x| x.space.starts_with("view:FRONT/band:top")));
    assert_eq!(f.grade, map.grade);
    assert!(map.focus("view:NOPE").is_none());
}

/// Two numbers side by side closer than a word space are a TEXT-CLEAR
/// warning; lines of text stacked at a line pitch are not (the notes strip).
#[test]
fn side_by_side_texts_need_a_word_space_and_stacked_lines_do_not() {
    let d = sheet(SheetSize::A4);
    let mut l = layout_of(&d, &[GEO], Vec::new());
    let a = Rect2 {
        x0: 30.0,
        y0: 50.0,
        x1: 40.0,
        y1: 53.6,
    };
    let b = Rect2 {
        x0: 40.5,
        x1: 50.0,
        ..a
    };
    l.items
        .push(item(SheetItemKind::NoteText, a, None, Some("NOTE A")));
    l.items
        .push(item(SheetItemKind::NoteText, b, None, Some("NOTE B")));
    let map = space_map(&d, &l);
    assert!(
        rules_in(&map, "notes").contains(&Rule::TextClear),
        "{:#?}",
        map.findings
    );

    let mut l = layout_of(&d, &[GEO], Vec::new());
    let below = Rect2 {
        y0: 55.0,
        y1: 58.6,
        ..a
    };
    l.items
        .push(item(SheetItemKind::NoteText, a, None, Some("LINE ONE")));
    l.items
        .push(item(SheetItemKind::NoteText, below, None, Some("LINE TWO")));
    assert!(!has_rule(&space_map(&d, &l), Rule::TextClear));
}

/// Two leaders whose legs cross are ONE defect: one finding, not one per
/// leader (their band would otherwise pay twice for it).
#[test]
fn crossing_leaders_are_one_finding() {
    let d = sheet(SheetSize::A4);
    // Two 45-degree legs crossing each other left of the part.
    let a = leader("a", "R5", [100.0, 120.0], [70.0, 90.0], 10.0);
    let b = leader("b", "R6", [100.0, 95.0], [75.0, 120.0], 10.0);
    let map = space_map(&d, &layout_of(&d, &[GEO], vec![a, b]));
    let crossings: Vec<&Finding> = map
        .findings
        .iter()
        .filter(|f| f.rule == Rule::LeaderAngle && f.unit == "count")
        .collect();
    assert_eq!(crossings.len(), 1, "{crossings:#?}");
}

// -- DATUM-ATTACH ------------------------------------------------------------

fn datum_sheet(hosted: bool) -> Drawing {
    let mut d = sheet(SheetSize::A4);
    d.datum_symbols
        .push(crate::drawing::types::PlacedDatumSymbol {
            label: "A".to_string(),
            anchor: [0.0, 0.0],
            owner_view: 0,
            feature_pid: Some("aa".to_string()),
        });
    if hosted {
        d.fcf_blocks.push(crate::drawing::types::PlacedFcfBlock {
            characteristic_glyph: "\u{23E5}".to_string(),
            tolerance_text: "0.05".to_string(),
            datum_labels: Vec::new(),
            anchor: [120.0, 150.0],
            leader_to: None,
            owner_view: 0,
            feature_pid: Some("aa".to_string()),
            annotation_index: Some(0),
            annotation_id: None,
        });
    }
    d
}

const FRAME_BOX: Rect2 = Rect2 {
    x0: 120.0,
    y0: 150.0,
    x1: 140.0,
    y1: 155.6,
};

/// A datum symbol of a feature that carries a frame stands on that frame:
/// on it, clean; anywhere else, an error naming the frame.
///
/// Mutation: drop the `rule_datum_attach` call -> the stray symbol is not
/// found, red.
#[test]
fn a_datum_symbol_stands_on_its_features_frame() {
    let d = datum_sheet(true);
    let mut on = layout_of(&d, &[GEO], Vec::new());
    on.items.push(item(
        SheetItemKind::FcfBlock,
        FRAME_BOX,
        Some(0),
        Some("\u{23E5} 0.05"),
    ));
    let stand = crate::drawing::layout::hosted_datum_box(&FRAME_BOX);
    on.items
        .push(item(SheetItemKind::DatumSymbol, stand, Some(0), Some("A")));
    assert!(
        !has_rule(&space_map(&d, &on), Rule::DatumAttach),
        "on its frame: clean"
    );

    let mut off = layout_of(&d, &[GEO], Vec::new());
    off.items.push(item(
        SheetItemKind::FcfBlock,
        FRAME_BOX,
        Some(0),
        Some("\u{23E5} 0.05"),
    ));
    let stray = Rect2 {
        x0: 60.0,
        y0: 60.0,
        x1: 65.6,
        y1: 65.6,
    };
    off.items
        .push(item(SheetItemKind::DatumSymbol, stray, Some(0), Some("A")));
    let map = space_map(&d, &off);
    let f = map
        .findings
        .iter()
        .find(|f| f.rule == Rule::DatumAttach)
        .expect("a stray datum symbol is found");
    assert_eq!(f.severity, Severity::Error);
    assert!(f.message.contains("fcf:0"), "{}", f.message);
}

/// A datum symbol with no frame to stand on stands on its feature or an
/// extension line; on a dimension line it designates the dimensioned
/// feature's axis, a different datum: an error. Floating: an error.
#[test]
fn an_unhosted_datum_symbol_never_stands_on_a_dimension_line() {
    let d = datum_sheet(false);
    let dim = hdim("d1", "40", 110.0, 150.0, 10.0, &GEO); // line at y 90
    let tip_box = |x: f64, y: f64| {
        let h = crate::drawing::layout::GDT_DATUM_TRI_H;
        Rect2 {
            x0: x - 2.8,
            y0: y - h - 5.6,
            x1: x + 2.8,
            y1: y - h,
        }
    };
    let mut on_line = layout_of(&d, &[GEO], vec![dim.clone()]);
    on_line.items.push(item(
        SheetItemKind::DatumSymbol,
        tip_box(130.0, 90.0),
        Some(0),
        Some("A"),
    ));
    let map = space_map(&d, &on_line);
    let f = map
        .findings
        .iter()
        .find(|f| f.rule == Rule::DatumAttach)
        .expect("on a dimension line");
    assert!(f.message.contains("dimension line"), "{}", f.message);

    let mut on_ext = layout_of(&d, &[GEO], vec![dim.clone()]);
    // The left extension line runs x 110 from y 98.5 up to 88.5.
    on_ext.items.push(item(
        SheetItemKind::DatumSymbol,
        tip_box(110.0, 95.0),
        Some(0),
        Some("A"),
    ));
    assert!(
        !has_rule(&space_map(&d, &on_ext), Rule::DatumAttach),
        "on an extension line: clean"
    );

    let mut floating = layout_of(&d, &[GEO], vec![dim]);
    floating.items.push(item(
        SheetItemKind::DatumSymbol,
        tip_box(30.0, 30.0),
        Some(0),
        Some("A"),
    ));
    let map = space_map(&d, &floating);
    let f = map
        .findings
        .iter()
        .find(|f| f.rule == Rule::DatumAttach)
        .expect("floating");
    assert!(f.message.contains("floats"), "{}", f.message);
}

/// A leader that runs ACROSS a drawn circle (in and out again) is flagged;
/// one that ends on the circle's outline is how it reaches its feature.
///
/// Mutation: drop `leader_across_circles` from the dimension-leader loop ->
/// red.
#[test]
fn a_leader_running_across_a_hole_is_flagged() {
    let mut d = sheet(SheetSize::A4);
    let h = d.sheet_size.height();
    // A hole of r 5 at sheet (130, 120) (view space y up from the sheet foot).
    d.views[0]
        .circles
        .push(crate::drawing::types::ProjectedCircle {
            cx: 130.0,
            cy: h - 120.0,
            r: 5.0,
            face_ids: Vec::new(),
        });
    let across = layout_of(
        &d,
        &[GEO],
        vec![leader("d1", "R5", [110.0, 121.0], [150.0, 119.0], 8.0)],
    );
    let map = space_map(&d, &across);
    assert!(
        map.findings.iter().any(
            |f| f.rule == Rule::LeaderAngle && f.message.contains("runs across 1 drawn circle")
        ),
        "{:#?}",
        map.findings
    );
    let onto = layout_of(
        &d,
        &[GEO],
        vec![leader("d1", "R5", [125.0, 120.0], [105.0, 100.0], 8.0)],
    );
    let map = space_map(&d, &onto);
    assert!(
        !map.findings
            .iter()
            .any(|f| f.message.contains("runs across")),
        "ending on the outline is not running across it"
    );
}
