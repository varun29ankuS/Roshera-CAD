//! The timeline Map's feature graph (Varun 2026-11-18: "one look at map
//! should tell everything ... how it was made ... can it be used to rebuild
//! the part").
//!
//! Three readings of the recorded history, at the level an engineer thinks
//! in -- features ([`Chapter`]s), not kernel steps:
//!
//! * [`feature_edges`]: which feature each feature was BUILT ON, read off the
//!   recorded solid inputs and outputs (a hole's subtract consumed the body
//!   the flange body produced), never guessed from order or names;
//! * [`feature_sizes`]: its sizes as a drawing writes them (`Ø80`, `12`,
//!   `4 × Ø5.5`, `PCD 62`), from the same editable values a mould changes, so
//!   an edited size reads as edited;
//! * [`recipe`]: the build sheet -- one line per feature, in order, with its
//!   sizes and what it was built on -- enough to rebuild the part by hand.
//!
//! No kernel id (`solid:N`, an event UUID) reaches any text produced here.

use std::collections::{BTreeSet, HashMap};

use crate::chapters::Chapter;
use crate::dimension_drive::EditableParameter;
use crate::replay::{recorded_solid_inputs, recorded_solid_outputs};
use crate::types::TimelineEvent;

/// Feature `to` was built on feature `from` (both chapter anchors); `via`
/// names the parent the way the Map's hover says it ("the Flange body").
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FeatureEdge {
    /// Anchor of the feature built on.
    pub from: u64,
    /// Anchor of the feature built on it.
    pub to: u64,
    /// The parent feature, in words.
    pub via: String,
}

/// The feature's name for a person: the recorded name, else its kind in
/// words ("Hole", "Bolt pattern").
pub fn display_name(chapter: &Chapter) -> String {
    if let Some(n) = chapter
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        return n.to_string();
    }
    let words = match chapter.kind.as_str() {
        "pattern" => "Bolt pattern".to_string(),
        "boss" => "Boss".to_string(),
        other => {
            let spaced = other.replace('_', " ");
            let mut chars = spaced.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "Feature".to_string(),
            }
        }
    };
    words
}

/// Every step sequence of a chapter and of its pattern instances.
fn steps_of(chapter: &Chapter) -> Vec<u64> {
    let mut all = chapter.events.clone();
    for i in &chapter.instances {
        all.extend(steps_of(i));
    }
    all
}

/// Which feature each feature was built on: for every step of feature B and
/// every recorded solid input `s`, the latest EARLIER step whose recorded
/// outputs contain `s` belongs to feature A; A != B gives the edge A -> B.
/// Pattern instances count as their pattern. Merged and sorted by
/// (from, to); a feature that makes and consumes no solid gets no edge.
pub fn feature_edges(events: &[TimelineEvent], chapters: &[Chapter]) -> Vec<FeatureEdge> {
    let mut owner: HashMap<u64, usize> = HashMap::new();
    for (i, c) in chapters.iter().enumerate() {
        for seq in steps_of(c) {
            owner.insert(seq, i);
        }
    }
    let mut ordered: Vec<&TimelineEvent> = events.iter().collect();
    ordered.sort_by_key(|e| e.sequence_number);

    // The latest producer of each recorded solid id so far.
    let mut producer: HashMap<u64, u64> = HashMap::new();
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for e in ordered {
        let Some(&to) = owner.get(&e.sequence_number) else {
            continue;
        };
        for s in recorded_solid_inputs(e) {
            if let Some(from) = producer.get(&s).and_then(|seq| owner.get(seq)) {
                if *from != to {
                    pairs.insert((*from, to));
                }
            }
        }
        for s in recorded_solid_outputs(e) {
            producer.insert(s, e.sequence_number);
        }
    }
    let mut edges: Vec<FeatureEdge> = pairs
        .into_iter()
        .map(|(a, b)| FeatureEdge {
            from: chapters[a].anchor,
            to: chapters[b].anchor,
            via: format!("the {}", display_name(&chapters[a])),
        })
        .collect();
    edges.sort_by_key(|e| (e.from, e.to));
    edges
}

/// A length as a drawing writes it: up to 3 decimals, trailing zeros trimmed.
fn mm(v: f64) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// The value of `key` among one step's parameters.
fn value(params: &[&EditableParameter], key: &str) -> Option<f64> {
    params.iter().find(|p| p.key == key).map(|p| p.value)
}

/// The sizes of one step, in the order a drawing reads them.
fn step_sizes(params: &[&EditableParameter]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(r) = value(params, "radius") {
        out.push(format!("Ø{}", mm(2.0 * r)));
    } else if let Some(d) = value(params, "diameter") {
        out.push(format!("Ø{}", mm(d)));
    }
    let lengths: Vec<f64> = [
        "width",
        "length",
        "depth",
        "height",
        "thickness",
        "distance",
    ]
    .iter()
    .filter_map(|k| value(params, k))
    .collect();
    let is_box = ["width", "height", "depth"]
        .iter()
        .all(|k| value(params, k).is_some())
        && value(params, "radius").is_none();
    if is_box {
        let w = value(params, "width").unwrap_or_default();
        let h = value(params, "height").unwrap_or_default();
        let d = value(params, "depth").unwrap_or_default();
        out.push(format!("{} × {} × {}", mm(w), mm(d), mm(h)));
    } else {
        out.extend(lengths.into_iter().map(mm));
    }
    if let Some(a) = value(params, "angle_deg") {
        if (a - 360.0).abs() > 1e-9 {
            out.push(format!("{}°", mm(a)));
        }
    }
    out
}

/// The editable values of the steps of `seqs`, grouped per step in order.
fn per_step<'a>(seqs: &[u64], params: &'a [EditableParameter]) -> Vec<Vec<&'a EditableParameter>> {
    let mut sorted = seqs.to_vec();
    sorted.sort_unstable();
    sorted
        .into_iter()
        .map(|s| {
            params
                .iter()
                .filter(|p| p.sequence == s)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .collect()
}

/// The pitch-circle diameter of `centres` when they all sit on one circle
/// about their mean (within 1e-6 mm), else `None`.
fn pitch_circle(centres: &[[f64; 2]]) -> Option<f64> {
    if centres.len() < 3 {
        return None;
    }
    let n = centres.len() as f64;
    let cx = centres.iter().map(|c| c[0]).sum::<f64>() / n;
    let cy = centres.iter().map(|c| c[1]).sum::<f64>() / n;
    let r0 = ((centres[0][0] - cx).powi(2) + (centres[0][1] - cy).powi(2)).sqrt();
    let on_circle = centres
        .iter()
        .all(|c| (((c[0] - cx).powi(2) + (c[1] - cy).powi(2)).sqrt() - r0).abs() < 1e-6);
    (on_circle && r0 > 1e-9).then_some(2.0 * r0)
}

/// A feature's sizes as an engineer writes them: `Ø80`, `12`; a box
/// `40 × 30 × 10`; a pattern `4 × Ø5.5` and `PCD 62` when its holes sit on
/// one circle (or the recorded `ring_r`). Values are the editable values
/// (an edited size shows its new value).
pub fn feature_sizes(chapter: &Chapter, params: &[EditableParameter]) -> Vec<String> {
    if !chapter.instances.is_empty() {
        let mut out = Vec::new();
        let n = chapter.instances.len();
        let first = per_step(&steps_of(&chapter.instances[0]), params);
        let dia = first.iter().find_map(|step| {
            value(step, "radius")
                .map(|r| 2.0 * r)
                .or_else(|| value(step, "diameter"))
        });
        match dia {
            Some(d) => out.push(format!("{n} × Ø{}", mm(d))),
            None => out.push(format!("{n} ×")),
        }
        let centres: Vec<[f64; 2]> = chapter
            .instances
            .iter()
            .filter_map(|i| {
                per_step(&steps_of(i), params)
                    .iter()
                    .find_map(|step| Some([value(step, "base_x")?, value(step, "base_y")?]))
            })
            .collect();
        let ring = per_step(&chapter.events, params)
            .iter()
            .find_map(|step| value(step, "ring_r"))
            .map(|r| 2.0 * r);
        if let Some(pcd) = ring.or_else(|| {
            (centres.len() == n)
                .then(|| pitch_circle(&centres))
                .flatten()
        }) {
            out.push(format!("PCD {}", mm(pcd)));
        }
        return out;
    }
    let steps = per_step(&chapter.events, params);
    // The feature's own size is on its first sized step (a hole's cutter, a
    // body's primitive); later steps in it are its placement or its cut.
    steps.first().map(|s| step_sizes(s)).unwrap_or_default()
}

/// The build sheet: one line per top-level feature in build order --
/// `"2. Bolt pattern: 4 × Ø5.5, PCD 62; built on the Flange body"`.
pub fn recipe(
    chapters: &[Chapter],
    edges: &[FeatureEdge],
    params: &[EditableParameter],
) -> Vec<String> {
    let mut ordered: Vec<&Chapter> = chapters.iter().collect();
    ordered.sort_by_key(|c| c.anchor);
    ordered
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let sizes = feature_sizes(c, params);
            let mut line = format!("{}. {}", i + 1, display_name(c));
            if !sizes.is_empty() {
                line.push_str(": ");
                line.push_str(&sizes.join(", "));
            }
            let vias: Vec<&str> = edges
                .iter()
                .filter(|e| e.to == c.anchor)
                .map(|e| e.via.as_str())
                .collect();
            if !vias.is_empty() {
                line.push_str("; built on ");
                line.push_str(&vias.join(" and "));
            }
            line
        })
        .collect()
}
