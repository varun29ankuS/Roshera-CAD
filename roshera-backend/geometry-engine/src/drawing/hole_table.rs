//! Hole table model for shopfloor drawings.
//!
//! Groups cylindrical bores on a part into a tabulated hole schedule,
//! suitable for a machinist reading the sheet without doing arithmetic.
//!
//! # Algorithm
//!
//! 1. Collect all `"diameter"` + `"position"` dimension records for the
//!    solid (from `extract_dimensions`).
//! 2. Group by `(quantised_diameter_0.01mm, through_or_blind)`.  Two bores
//!    with the same diameter AND the same through/blind status share a group
//!    letter (A, B, C …).
//! 3. Within each group, order instances by (X ascending, Y ascending) and
//!    number them A1, A2, … for positional clarity.
//! 4. Depth: compare the bore axial extent (from the `"length"` record with
//!    the same face entities) to the overall part extent along that axis.
//!    When the bore length equals the part extent within 0.01 mm the bore
//!    is THRU; otherwise it is blind and the depth string is "↧ {depth}".
//! 5. X / Y values come from the `"position"` records' labels (already
//!    unit-formatted by `extract_dimensions`) with the axis prefix stripped —
//!    the table's X/Y column headers already name the axis.
//!
//! # Bore qualification happens at the CALLER
//!
//! This function tables every diameter record it is given. `extract_dimensions`
//! emits diameter records for every cylindrical lateral face — bore, boss, or
//! the part's own OD — so the caller (`attach_hole_table_from_dims`) MUST
//! pre-filter to bores using the material-side rule in
//! [`crate::readable::bore_face_ids`] (concave = outward normal toward the
//! axis). Feeding unfiltered records here puts the part's silhouette in the
//! hole table.
//!
//! # Layout integration
//!
//! The caller (`compute_layout`) positions the table on the sheet and
//! produces `SheetItem` entries for every cell border and text label.
//! The SVG/DXF renderers ink those items directly; the quality verifier
//! checks them like any other layout item.

use crate::readable::{DatumDescriptor, DimensionRecord};
use serde::{Deserialize, Serialize};

// ── Group tag letters ─────────────────────────────────────────────────────────

/// Return the group tag letter for group index 0..25 (A..Z).
///
/// Groups are indexed in order of first appearance (smallest diameter first,
/// then blind before through, so the sort is stable and deterministic).
/// Indices beyond 25 wrap to AA, AB, … (26 = AA, 27 = AB, …) to handle
/// extremely busy parts.
pub fn tag_letter(index: usize) -> String {
    if index < 26 {
        let c = (b'A' + index as u8) as char;
        c.to_string()
    } else {
        // Two-letter tags: AA, AB, … AZ, BA, BB, …
        let hi = (index / 26) - 1;
        let lo = index % 26;
        let h = (b'A' + hi.min(25) as u8) as char;
        let l = (b'A' + lo as u8) as char;
        format!("{h}{l}")
    }
}

// ── HoleSite ──────────────────────────────────────────────────────────────────

/// One bore site: the analytic data needed to populate a hole-table row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoleSite {
    /// Instance tag, e.g. "A1", "A2", "B1".
    pub tag: String,
    /// Group letter shared by sites of the same (diameter, depth-class).
    pub group: String,
    /// Diameter in kernel mm.
    pub diameter_mm: f64,
    /// Formatted X position label (already unit-converted by extraction).
    pub x_label: String,
    /// Formatted Y position label.
    pub y_label: String,
    /// Raw X offset in mm (for sort order; not displayed).
    pub x_mm: f64,
    /// Raw Y offset in mm (for sort order; not displayed).
    pub y_mm: f64,
    /// Formatted diameter label ("Ø5.00", "Ø5.000in", …).
    pub dia_label: String,
    /// Depth label: "THRU" or "↧ {depth}" (unit-formatted depth).
    pub depth_label: String,
    /// Whether the bore passes all the way through the part.
    pub is_through: bool,
    /// View-space centre of the bore in the axial view (used for tag callout).
    pub axial_centre: Option<[f64; 2]>,
    /// B-Rep face entity ids for this bore (from the diameter record).
    ///
    /// Used by the dimension-placement filter to suppress `kind == "position"`
    /// dimensions whose entity set intersects this bore's faces — those positions
    /// are represented in the hole table's X/Y columns and must NOT also appear
    /// in the general dimension stack (`place_dimensions` tabled-position suppression).
    #[serde(default)]
    pub face_entities: Vec<u32>,
    /// Reference datum the X/Y columns are measured from (campaign #55 Slice 1),
    /// carried from the bore's `"position"` dimension records — pre-#55 the
    /// corner the columns measured from existed on the sheet only as a
    /// `DatumMarker` layout item, so a readback could not name it. `None` when
    /// no position record carried a datum (e.g. a diagonal-axis bore).
    #[serde(default)]
    pub datum: Option<DatumDescriptor>,
    /// Bound GD&T dimensional tolerance (campaign #55 Slice 4), joined from the
    /// `GdtSidecar` by feature PID / face-set at build time, so "the toleranced
    /// diameter of the bore pattern" is answerable with limits + provenance.
    /// `None` for an untoleranced bore (readback then answers with the general
    /// tolerance, explicitly labelled). `#[serde(default)]` keeps older
    /// serialized drawings parsing.
    #[serde(default)]
    pub tolerance: Option<crate::drawing::types::ToleranceRef>,
}

// ── Group key ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct GroupKey {
    /// Diameter quantised to 0.01 mm (×100, round to i64).
    q_dia: i64,
    /// Through = 0, Blind = 1 (through sorts first within the same diameter).
    depth_class: u8,
}

impl GroupKey {
    fn new(diameter_mm: f64, is_through: bool) -> Self {
        GroupKey {
            q_dia: (diameter_mm * 100.0).round() as i64,
            depth_class: if is_through { 0 } else { 1 },
        }
    }
}

// ── build_hole_table ──────────────────────────────────────────────────────────

/// Derive a complete hole table from a flat dimension record list.
///
/// `part_extent_along_axis` is the overall part length along the bore's
/// dominant axis (used to decide THRU vs blind). Pass it keyed by the
/// dominant axis index: `[x_extent, y_extent, z_extent]`.
///
/// Returns `HoleSite` entries in group order (A1, A2, …, B1, …).
pub fn build_hole_table(dims: &[DimensionRecord], part_extents: [f64; 3]) -> Vec<HoleSite> {
    use std::collections::HashMap;

    // ── Step 1: collect diameter records ─────────────────────────────────────
    // For each diameter record we also need:
    //   - the length record for the same face entities (bore depth)
    //   - two position records (X and Y offsets from the part corner)
    //
    // All four records share the same entities list (same face ids).

    // Index records by entity set (sorted face ids as a key).
    let entity_key = |d: &DimensionRecord| -> Vec<u32> {
        let mut v = d.entities.clone();
        v.sort_unstable();
        v
    };

    // Build per-entity-set maps for fast lookup.
    let mut dia_by_ent: HashMap<Vec<u32>, &DimensionRecord> = HashMap::new();
    let mut len_by_ent: HashMap<Vec<u32>, &DimensionRecord> = HashMap::new();
    let mut pos_by_ent: HashMap<Vec<u32>, Vec<&DimensionRecord>> = HashMap::new();

    for d in dims {
        if d.entities.is_empty() {
            continue;
        }
        let key = entity_key(d);
        match d.kind.as_str() {
            "diameter" => {
                dia_by_ent.entry(key).or_insert(d);
            }
            "length" => {
                len_by_ent.entry(key).or_insert(d);
            }
            "position" => {
                pos_by_ent.entry(key).or_default().push(d);
            }
            _ => {}
        }
    }

    // ── Step 2: build raw site list ───────────────────────────────────────────
    struct RawSite {
        key: GroupKey,
        diameter_mm: f64,
        dia_label: String,
        x_mm: f64,
        y_mm: f64,
        x_label: String,
        y_label: String,
        is_through: bool,
        depth_label: String,
        face_entities: Vec<u32>,
        datum: Option<DatumDescriptor>,
    }

    let mut raw: Vec<RawSite> = Vec::new();

    for (ents, dia_rec) in &dia_by_ent {
        let diameter_mm = dia_rec.value;
        let dia_label = dia_rec.label.clone();

        // Bore axis: from the "axis" field on the diameter record.
        let axis = match dia_rec.axis {
            Some(a) => a,
            None => continue, // spheres/cones — not holes
        };
        // Dominant axis index (0=X, 1=Y, 2=Z).
        let abs = [axis[0].abs(), axis[1].abs(), axis[2].abs()];
        let dominant = if abs[0] >= abs[1] && abs[0] >= abs[2] {
            0
        } else if abs[1] >= abs[2] {
            1
        } else {
            2
        };

        // Bore depth from length record.
        let depth_mm = len_by_ent.get(ents).map(|lr| lr.value).unwrap_or(0.0);

        // THRU: bore length == part extent along dominant axis (±0.01 mm).
        let part_ext = part_extents[dominant];
        let is_through = (depth_mm - part_ext).abs() <= 0.01;

        // Depth label.
        let depth_label = if is_through {
            "THRU".to_string()
        } else if depth_mm > 1e-9 {
            // Use the length record's label (already unit-formatted) minus the
            // "L " prefix, and prepend the blind-depth glyph.
            let raw_lbl = len_by_ent
                .get(ents)
                .map(|lr| lr.label.as_str())
                .unwrap_or("");
            let num_part = raw_lbl.strip_prefix("L ").unwrap_or(raw_lbl);
            format!("\u{21A7} {num_part}")
        } else {
            "THRU".to_string() // fallback for degenerate depth
        };

        // Position labels and raw mm values.
        let positions = pos_by_ent.get(ents).map(|v| v.as_slice()).unwrap_or(&[]);

        // The two perpendicular axes relative to the dominant axis.
        let perps = match dominant {
            0 => [1usize, 2],
            1 => [0, 2],
            _ => [0, 1],
        };

        // Find X (perps[0]) and Y (perps[1]) position records.
        let find_pos = |axis_idx: usize| -> Option<(&DimensionRecord, usize)> {
            positions.iter().find_map(|p| {
                // Direction component along this world axis is nonzero.
                let d = p.direction;
                if d[axis_idx].abs() > 0.5 {
                    Some((*p, axis_idx))
                } else {
                    None
                }
            })
        };

        // The extraction label carries an axis prefix ("X 12.00mm"). The
        // table's X/Y COLUMN HEADERS already name the axis, so the cell
        // shows just the value ("12.00mm") — repeating the prefix under the
        // header is redundant ink (same strip rule the viewport applies).
        let strip_axis = |label: &str| -> String {
            label
                .strip_prefix("X ")
                .or_else(|| label.strip_prefix("Y "))
                .or_else(|| label.strip_prefix("Z "))
                .unwrap_or(label)
                .to_string()
        };

        let (x_mm, x_label) = find_pos(perps[0])
            .map(|(r, _)| (r.value, strip_axis(&r.label)))
            .unwrap_or((0.0, "—".to_string()));

        let (y_mm, y_label) = find_pos(perps[1])
            .map(|(r, _)| (r.value, strip_axis(&r.label)))
            .unwrap_or((0.0, "—".to_string()));

        let key = GroupKey::new(diameter_mm, is_through);
        // Capture the bore's face entity ids from the diameter record.
        // These are propagated to HoleSite.face_entities so the dimension
        // placement filter can suppress tabled position dims.
        let face_entities = ents.clone();

        // Reference datum for the X/Y columns (campaign #55 Slice 1): carried
        // from any position record that names one. Position records emitted by
        // `extract_dimensions` always carry a `part_corner` descriptor.
        let datum = positions.iter().find_map(|p| p.datum.clone());

        raw.push(RawSite {
            key,
            diameter_mm,
            dia_label,
            x_mm,
            y_mm,
            x_label,
            y_label,
            is_through,
            depth_label,
            face_entities,
            datum,
        });
    }

    // ── Step 3: sort raw sites by group key, then by position ─────────────────
    raw.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then(
                a.x_mm
                    .partial_cmp(&b.x_mm)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(
                a.y_mm
                    .partial_cmp(&b.y_mm)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    // ── Step 4: assign group letters and instance numbers ─────────────────────
    let mut groups: Vec<GroupKey> = Vec::new();
    let group_idx = |key: &GroupKey, groups: &mut Vec<GroupKey>| -> usize {
        if let Some(i) = groups.iter().position(|g| g == key) {
            i
        } else {
            let i = groups.len();
            groups.push(key.clone());
            i
        }
    };

    // Pre-compute group indices in sorted order.
    let mut site_group_idx: Vec<usize> = Vec::new();
    for s in &raw {
        let gi = group_idx(&s.key, &mut groups);
        site_group_idx.push(gi);
    }

    // Per-group running instance counter.
    let mut counters: Vec<usize> = vec![0; groups.len()];

    let mut out: Vec<HoleSite> = Vec::new();
    for (i, s) in raw.iter().enumerate() {
        let gi = site_group_idx[i];
        counters[gi] += 1;
        let group_letter = tag_letter(gi);
        let tag = format!("{}{}", group_letter, counters[gi]);

        out.push(HoleSite {
            tag,
            group: group_letter,
            diameter_mm: s.diameter_mm,
            x_label: s.x_label.clone(),
            y_label: s.y_label.clone(),
            x_mm: s.x_mm,
            y_mm: s.y_mm,
            dia_label: s.dia_label.clone(),
            depth_label: s.depth_label.clone(),
            is_through: s.is_through,
            axial_centre: None, // filled by the drawing layer
            face_entities: s.face_entities.clone(),
            datum: s.datum.clone(),
            // Bound GD&T dimensional tolerance joined later by `attach_tolerances`.
            tolerance: None,
        });
    }

    out
}

// ── Baseline detection ────────────────────────────────────────────────────────

/// True when a view carries ≥3 position dimensions measured from one shared
/// datum EDGE, qualifying for ISO 129-1 baseline dimensioning.
///
/// # Datum-edge semantics (not datum-point)
///
/// Baseline dims share a datum EDGE of the part, not a single point: three
/// X offsets from the left edge start at the same x but each runs at its own
/// bore's y. So positions are grouped by orientation — horizontal (dx ≥ dy)
/// versus vertical — exactly as `place_dimensions` classifies them, and a
/// group qualifies when it has ≥3 members whose min endpoint coordinate
/// ALONG the measured axis coincides within 0.5 mm (the extraction measures
/// offsets from the AABB min corner, so the datum end is always the min end).
///
/// # Interplay with the hole table (Deliverable 3 rule)
///
/// Position dims reach the sheet through exactly two channels:
/// 1. **Tabled bores** — the hole table's X/Y columns. `place_dimensions`
///    drops their position dims from the general stack (keyed by
///    `HoleSite::face_entities`), so this oracle never sees them.
/// 2. **Untabled positions** — a BASELINE stack, rendered iff this oracle
///    returns `true` for the untabled remainder. With every bore tabled
///    (the common case) no baseline stack is drawn: the hole table IS the
///    baseline. With <3 qualifying positions nothing renders — honest
///    omission beats a nonstandard chained callout.
pub fn qualifies_for_baseline(view_dims: &[crate::drawing::dimensioning::Dimension2d]) -> bool {
    // Datum coordinate of each position span, keyed by orientation.
    let mut h_datums: Vec<f64> = Vec::new(); // x of horizontal spans' min end
    let mut v_datums: Vec<f64> = Vec::new(); // y of vertical spans' min end
    for d in view_dims.iter().filter(|d| d.kind == "position") {
        let dx = (d.a[0] - d.b[0]).abs();
        let dy = (d.a[1] - d.b[1]).abs();
        if dx < 1e-6 && dy < 1e-6 {
            continue; // degenerate span carries no datum information
        }
        if dx >= dy {
            h_datums.push(d.a[0].min(d.b[0]));
        } else {
            v_datums.push(d.a[1].min(d.b[1]));
        }
    }
    let shares_edge = |coords: &[f64]| -> bool {
        coords.len() >= 3 && {
            let r = coords[0];
            coords.iter().all(|&c| (c - r).abs() < 0.5)
        }
    };
    shares_edge(&h_datums) || shares_edge(&v_datums)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dia_rec(id: &str, dia: f64, axis: [f64; 3], fid: u32) -> DimensionRecord {
        DimensionRecord {
            id: id.to_string(),
            kind: "diameter".to_string(),
            value: dia,
            unit: "mm".to_string(),
            label: format!("Ø{dia:.2}"),
            entities: vec![fid],
            anchor: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0],
            axis: Some(axis),
            pid: None,
            datum: None,
        }
    }

    fn len_rec(id: &str, length: f64, fid: u32) -> DimensionRecord {
        DimensionRecord {
            id: id.to_string(),
            kind: "length".to_string(),
            value: length,
            unit: "mm".to_string(),
            label: format!("L {length:.2}"),
            entities: vec![fid],
            anchor: [0.0, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
            axis: Some([0.0, 0.0, 1.0]),
            pid: None,
            datum: None,
        }
    }

    fn pos_rec(id: &str, value: f64, axis_idx: usize, fid: u32) -> DimensionRecord {
        let label = match axis_idx {
            0 => format!("X {value:.2}"),
            1 => format!("Y {value:.2}"),
            _ => format!("Z {value:.2}"),
        };
        let mut dir = [0.0_f64; 3];
        dir[axis_idx] = 1.0;
        DimensionRecord {
            id: id.to_string(),
            kind: "position".to_string(),
            value,
            unit: "mm".to_string(),
            label,
            entities: vec![fid],
            anchor: [0.0, 0.0, 0.0],
            direction: dir,
            axis: Some([0.0, 0.0, 1.0]),
            pid: None,
            datum: None,
        }
    }

    /// Two holes with the same diameter → one group (A); two instances A1 and A2.
    /// One is THRU (depth == part Z extent), one is blind.
    #[test]
    fn two_diameters_same_group_thru_and_blind() {
        // Z-axis bores, part Z extent = 10.0 mm.
        // Hole 1 (fid=1): Ø5, depth=10.0 → THRU
        // Hole 2 (fid=2): Ø5, depth=6.0  → blind
        let part_extents = [40.0, 40.0, 10.0]; // X, Y, Z
        let dims = vec![
            dia_rec("d0", 5.0, [0.0, 0.0, 1.0], 1),
            len_rec("d1", 10.0, 1),
            pos_rec("d2", 5.0, 0, 1), // X=5 for fid=1
            pos_rec("d3", 5.0, 1, 1), // Y=5 for fid=1
            dia_rec("d4", 5.0, [0.0, 0.0, 1.0], 2),
            len_rec("d5", 6.0, 2),
            pos_rec("d6", 15.0, 0, 2), // X=15 for fid=2
            pos_rec("d7", 15.0, 1, 2), // Y=15 for fid=2
        ];
        let table = build_hole_table(&dims, part_extents);
        // Both are Ø5, but different depth_class → TWO groups.
        // THRU (depth_class=0) sorts first → group A
        // Blind (depth_class=1) sorts second → group B
        assert_eq!(table.len(), 2, "two holes");
        let thru = table.iter().find(|s| s.is_through).expect("THRU hole");
        let blind = table.iter().find(|s| !s.is_through).expect("blind hole");
        assert_eq!(thru.group, "A", "THRU hole is group A");
        assert_eq!(blind.group, "B", "blind hole is group B");
        assert_eq!(thru.tag, "A1");
        assert_eq!(blind.tag, "B1");
        assert_eq!(thru.depth_label, "THRU");
        assert!(
            blind.depth_label.starts_with('\u{21A7}'),
            "blind depth has arrow glyph: {}",
            blind.depth_label
        );
    }

    /// Two different diameters → two groups A and B.
    #[test]
    fn two_different_diameters_two_groups() {
        let part_extents = [50.0, 50.0, 10.0];
        let dims = vec![
            // Ø5 THRU (fid=1)
            dia_rec("d0", 5.0, [0.0, 0.0, 1.0], 1),
            len_rec("d1", 10.0, 1),
            pos_rec("d2", 5.0, 0, 1),
            pos_rec("d3", 5.0, 1, 1),
            // Ø8 THRU (fid=2)
            dia_rec("d4", 8.0, [0.0, 0.0, 1.0], 2),
            len_rec("d5", 10.0, 2),
            pos_rec("d6", 25.0, 0, 2),
            pos_rec("d7", 25.0, 1, 2),
        ];
        let table = build_hole_table(&dims, part_extents);
        assert_eq!(table.len(), 2);
        // Ø5 < Ø8 → A = Ø5, B = Ø8
        let a = table.iter().find(|s| s.group == "A").expect("group A");
        let b = table.iter().find(|s| s.group == "B").expect("group B");
        assert!((a.diameter_mm - 5.0).abs() < 0.01, "group A is Ø5");
        assert!((b.diameter_mm - 8.0).abs() < 0.01, "group B is Ø8");
        assert_eq!(a.depth_label, "THRU");
        assert_eq!(b.depth_label, "THRU");
    }

    /// Six holes of the same Ø5 THRU → one group A, six instances A1..A6.
    /// Ordered by X then Y ascending.
    #[test]
    fn six_same_holes_one_group_six_instances() {
        let part_extents = [40.0, 40.0, 10.0];
        let mut dims = Vec::new();
        // Place at (5,5), (5,15), (5,25), (15,5), (15,15), (15,25)
        // After sort by X then Y: (5,5)→A1, (5,15)→A2, (5,25)→A3, (15,5)→A4, ...
        let positions = [
            (5.0, 5.0),
            (15.0, 5.0),
            (5.0, 15.0),
            (15.0, 15.0),
            (5.0, 25.0),
            (15.0, 25.0),
        ];
        for (i, (x, y)) in positions.iter().enumerate() {
            let fid = (i + 1) as u32;
            dims.push(dia_rec(&format!("d{}", i * 4), 5.0, [0.0, 0.0, 1.0], fid));
            dims.push(len_rec(&format!("d{}", i * 4 + 1), 10.0, fid));
            dims.push(pos_rec(&format!("d{}", i * 4 + 2), *x, 0, fid));
            dims.push(pos_rec(&format!("d{}", i * 4 + 3), *y, 1, fid));
        }
        let table = build_hole_table(&dims, part_extents);
        assert_eq!(table.len(), 6, "six instances");
        assert!(table.iter().all(|s| s.group == "A"), "all in group A");
        let tags: Vec<&str> = table.iter().map(|s| s.tag.as_str()).collect();
        assert!(
            tags.contains(&"A1") && tags.contains(&"A6"),
            "A1..A6 present"
        );
    }

    /// `tag_letter` correctness: 0..25 = A..Z, 26 = AA, 27 = AB.
    #[test]
    fn tag_letter_alphabet() {
        assert_eq!(tag_letter(0), "A");
        assert_eq!(tag_letter(25), "Z");
        assert_eq!(tag_letter(26), "AA");
        assert_eq!(tag_letter(27), "AB");
    }

    fn pos2d(label: &str, a: [f64; 2], b: [f64; 2]) -> crate::drawing::dimensioning::Dimension2d {
        crate::drawing::dimensioning::Dimension2d {
            id: label.to_string(),
            kind: "position".to_string(),
            value: 0.0,
            unit: "mm".to_string(),
            label: label.to_string(),
            a,
            b,
            entities: vec![1],
            axis3: None,
            dir3: None,
            pid: None,
            datum: None,
            tolerance: None,
        }
    }

    /// ISO 129-1 baseline qualification is per datum EDGE, not per datum
    /// point: three horizontal position spans all starting at x = 0 but at
    /// DIFFERENT heights (each bore has its own cross-section row) share the
    /// part's left datum edge and must qualify. Two positions never qualify,
    /// and three spans with scattered start coordinates share no datum.
    #[test]
    fn baseline_requires_three_positions_sharing_a_datum_edge() {
        let three = vec![
            pos2d("8.00", [0.0, 2.0], [8.0, 2.0]),
            pos2d("14.00", [0.0, 6.0], [14.0, 6.0]),
            pos2d("20.00", [0.0, 10.0], [20.0, 10.0]),
        ];
        assert!(
            qualifies_for_baseline(&three),
            "3 positions from one datum edge (shared x=0, differing y) qualify"
        );
        assert!(
            !qualifies_for_baseline(&three[..2]),
            "2 positions never qualify for a baseline stack"
        );
        let scattered = vec![
            pos2d("8.00", [0.0, 2.0], [8.0, 2.0]),
            pos2d("14.00", [3.0, 6.0], [17.0, 6.0]),
            pos2d("20.00", [7.0, 10.0], [27.0, 10.0]),
        ];
        assert!(
            !qualifies_for_baseline(&scattered),
            "scattered datum coordinates do not qualify"
        );
    }
}
