//! Chapters: the timeline's rows, one per ENGINEERING FEATURE, the way a CAD
//! user's feature tree reads (Varun 2026-11-13).
//!
//! The kernel records modelling steps, not features: the weld-neck flange
//! demo records 14 steps -- a revolve, five names, and four times a cutter
//! cylinder plus the subtract that consumes it -- where an engineer's tree has
//! two features (the body; four bolt holes). Chapters group the steps by
//! rules read off the recorded inputs and outputs. Nothing is guessed:
//!
//! * **A tool merges into the operation that consumes it.** A solid created
//!   only to be the second operand of a boolean -- named or placed before
//!   that, used by nothing after -- is part of that boolean's chapter
//!   (cylinder + subtract = a hole).
//! * **Placement folds into what it places:** a transform of a newly created
//!   solid, before anything else uses it, belongs to the create.
//! * **Names attach to what they name:** a `set_name` joins the chapter that
//!   owns its solid -- also when the name was recorded before the solid.
//! * **Repeats become a pattern:** three or more consecutive identical
//!   tool-features cut into the same part at regularly spaced positions
//!   (equal steps on a line, or equal angles on a circle) are one pattern
//!   chapter whose instances are the features.
//! * **What no rule claims stays its own row.**
//!
//! A chapter is a VIEW: every recorded step is in exactly one chapter, and
//! grouping never changes history.
use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::replay::{recorded_solid_inputs, recorded_solid_outputs};
use crate::types::{Operation, TimelineEvent};

/// One row of the timeline: an engineering feature and the recorded steps
/// that make it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Chapter {
    /// What the feature is ("hole", "revolve", "pattern", ...): the icon key.
    pub kind: String,
    /// The engineer's name for it, when one was recorded.
    pub name: Option<String>,
    /// The sequence numbers of every recorded step in this chapter, ascending.
    pub events: Vec<u64>,
    /// The step that defines the chapter (the subtract of a hole, the create
    /// of a box; a pattern's last instance): the part "after this chapter" is
    /// the model at this step.
    pub anchor: u64,
    /// For a pattern: its instances, each a chapter. Empty otherwise.
    pub instances: Vec<Chapter>,
}

/// One recorded step, read once.
struct Step<'a> {
    seq: u64,
    command: &'a str,
    params: &'a Value,
    inputs: Vec<u64>,
    outputs: Vec<u64>,
}

impl Step<'_> {
    fn is_create(&self) -> bool {
        self.command.starts_with("create_") && self.inputs.is_empty() && self.outputs.len() == 1
    }
    fn is_boolean(&self) -> bool {
        self.command.starts_with("boolean_")
    }
    fn is_name(&self) -> bool {
        self.command == "set_name"
    }
    /// A transform that leaves the solid's identity (moves it in place).
    fn moves(&self, solid: u64) -> bool {
        self.command == "transform_solid" && self.inputs == [solid] && self.outputs == [solid]
    }
    fn solid_b(&self) -> Option<u64> {
        self.params.get("params")?.get("solid_b")?.as_u64()
    }
    fn solid_a(&self) -> Option<u64> {
        self.params.get("params")?.get("solid_a")?.as_u64()
    }
    fn name_text(&self) -> Option<String> {
        self.params
            .get("params")?
            .get("name")?
            .as_str()
            .map(str::to_string)
    }
    /// A create's parameters (`params.Create3D.parameters`).
    fn create_parameters(&self) -> Option<&serde_json::Map<String, Value>> {
        self.params
            .get("params")?
            .get("Create3D")?
            .get("parameters")?
            .as_object()
    }
}

/// Feature-level facts about a chapter, kept while grouping.
struct Draft {
    chapter: Chapter,
    /// The step that defines the chapter (orders the rows).
    anchor: u64,
    /// For a tool-feature: (tool size signature, tool base point, part
    /// solid before, part solid after).
    tool: Option<ToolFeature>,
}

struct ToolFeature {
    signature: String,
    base: Option<[f64; 3]>,
    part_in: Option<u64>,
    part_out: Option<u64>,
}

/// Group a history into chapters, in order. A DELETED feature (every step
/// of it named by a `param.suppress`) is left out -- it is no longer part of
/// the part; the delete itself stays, as a step of the history.
pub fn chapters(events: &[TimelineEvent]) -> Vec<Chapter> {
    let deleted = crate::mould::OverrideSet::collect(events);
    let gone =
        |c: &Chapter| !c.events.is_empty() && c.events.iter().all(|s| deleted.is_suppressed(*s));
    let mut out = fold_sketch_edits(events, all_chapters(events));
    for c in &mut out {
        c.instances.retain(|i| !gone(i));
    }
    out.retain(|c| !gone(c));
    out
}

/// The sketch a recorded step belongs to (`csketch_id`), if any.
fn sketch_of(e: &TimelineEvent) -> Option<&str> {
    let Operation::Generic { parameters, .. } = &e.operation else {
        return None;
    };
    parameters
        .get("params")
        .unwrap_or(parameters)
        .get("csketch_id")
        .and_then(Value::as_str)
}

/// Editing a sketch (trim, fillet, mirror, pattern ... -- `csketch_*`
/// steps) is part of making the feature the sketch becomes, not a feature
/// of the part (Varun 2026-11-14). Each such step folds into the later
/// sketch extrude / revolve of the SAME sketch; the edits of a sketch that
/// never became a feature are one `sketch` entry. Nothing is hidden: every
/// step stays in exactly one chapter.
fn fold_sketch_edits(events: &[TimelineEvent], chapters: Vec<Chapter>) -> Vec<Chapter> {
    let by_seq: std::collections::HashMap<u64, &TimelineEvent> =
        events.iter().map(|e| (e.sequence_number, e)).collect();
    let is_edit = |c: &Chapter| c.kind.starts_with("csketch_");
    let sketch_id = |c: &Chapter| -> Option<String> {
        c.events
            .iter()
            .find_map(|s| by_seq.get(s).and_then(|e| sketch_of(e)).map(str::to_string))
    };
    let mut out: Vec<Chapter> = Vec::with_capacity(chapters.len());
    let mut pending: Vec<(String, Chapter)> = Vec::new();
    if !chapters.iter().any(is_edit) {
        return chapters;
    }
    for c in chapters {
        if is_edit(&c) {
            if let Some(id) = sketch_id(&c) {
                pending.push((id, c));
                continue;
            }
        }
        // A sketch feature takes the edits of its own sketch.
        let mut c = c;
        if c.kind == "extrude" || c.kind == "revolve" || c.kind == "cut" || c.kind == "boss" {
            if let Some(id) = sketch_id(&c) {
                let (mine, rest): (Vec<_>, Vec<_>) =
                    pending.into_iter().partition(|(sid, _)| *sid == id);
                pending = rest;
                for (_, e) in mine {
                    c.events.extend(e.events);
                }
                c.events.sort_unstable();
            }
        }
        out.push(c);
    }
    // Edits of sketches that never became a feature: one entry per sketch.
    let mut sketches: Vec<(String, Chapter)> = Vec::new();
    for (id, c) in pending {
        if let Some((_, s)) = sketches.iter_mut().find(|(sid, _)| *sid == id) {
            s.events.extend(c.events);
            s.events.sort_unstable();
            s.anchor = s.anchor.max(c.anchor);
        } else {
            sketches.push((
                id,
                Chapter {
                    kind: "sketch".to_string(),
                    name: None,
                    ..c
                },
            ));
        }
    }
    out.extend(sketches.into_iter().map(|(_, c)| c));
    out.sort_by_key(|c| c.events.first().copied().unwrap_or(c.anchor));
    out
}

fn all_chapters(events: &[TimelineEvent]) -> Vec<Chapter> {
    let mut ordered: Vec<&TimelineEvent> = events.iter().collect();
    ordered.sort_by_key(|e| e.sequence_number);
    let null = Value::Null;
    let steps: Vec<Step> = ordered
        .iter()
        .map(|e| {
            let (command, params) = match &e.operation {
                Operation::Generic {
                    command_type,
                    parameters,
                } => (command_type.as_str(), parameters),
                _ => ("", &null),
            };
            Step {
                seq: e.sequence_number,
                command,
                params,
                inputs: recorded_solid_inputs(e),
                outputs: recorded_solid_outputs(e),
            }
        })
        .collect();

    // Index of the step each step is absorbed into (tools, placements).
    let mut absorbed_into: HashMap<usize, usize> = HashMap::new();
    // For a boolean: the create step of the tool it consumed.
    let mut tool_of: HashMap<usize, usize> = HashMap::new();

    for (i, s) in steps.iter().enumerate() {
        if !s.is_create() {
            continue;
        }
        let solid = s.outputs[0];
        let mut placements = Vec::new();
        let mut consumer = None;
        for (j, t) in steps.iter().enumerate().skip(i + 1) {
            if t.is_name() && t.inputs == [solid] {
                continue;
            }
            if t.moves(solid) && consumer.is_none() {
                placements.push(j);
                continue;
            }
            if t.inputs.contains(&solid) {
                consumer = Some(j);
                break;
            }
        }
        // Placement folds into the create when the first real use is not a
        // tool-consuming boolean (or there is no use at all).
        let used_after = |from: usize| {
            steps
                .iter()
                .skip(from + 1)
                .any(|t| t.inputs.contains(&solid) && !t.is_name())
        };
        match consumer {
            Some(j)
                if steps[j].is_boolean() && steps[j].solid_b() == Some(solid) && !used_after(j) =>
            {
                absorbed_into.insert(i, j);
                for p in placements {
                    absorbed_into.insert(p, j);
                }
                tool_of.insert(j, i);
            }
            _ => {
                for p in placements {
                    absorbed_into.insert(p, i);
                }
            }
        }
    }

    // Build the chapters, one per step that is neither absorbed nor a name.
    let mut drafts: Vec<Draft> = Vec::new();
    let mut draft_of_step: HashMap<usize, usize> = HashMap::new();
    for (i, s) in steps.iter().enumerate() {
        if absorbed_into.contains_key(&i) || s.is_name() {
            continue;
        }
        let tool = tool_of.get(&i).map(|&c| &steps[c]);
        let kind = kind_of(s, tool);
        let tool_feature = tool.map(|t| ToolFeature {
            signature: tool_signature(t),
            base: tool_base(t),
            part_in: s.solid_a(),
            part_out: s.outputs.first().copied(),
        });
        draft_of_step.insert(i, drafts.len());
        drafts.push(Draft {
            chapter: Chapter {
                kind,
                name: None,
                events: vec![s.seq],
                instances: Vec::new(),
                anchor: s.seq,
            },
            anchor: s.seq,
            tool: tool_feature,
        });
    }
    for (&step, &into) in &absorbed_into {
        // `into` may itself be absorbed (a placement of a tool): follow it.
        let mut target = into;
        while let Some(&next) = absorbed_into.get(&target) {
            target = next;
        }
        if let Some(&d) = draft_of_step.get(&target) {
            drafts[d].chapter.events.push(steps[step].seq);
        }
    }

    // Which chapter owns each solid: outputs of its steps, and tool solids.
    let mut owner: HashMap<u64, usize> = HashMap::new();
    let mut owned_from: BTreeMap<u64, Vec<(u64, usize)>> = BTreeMap::new(); // seq -> solids gained
    for (i, s) in steps.iter().enumerate() {
        let mut target = i;
        while let Some(&next) = absorbed_into.get(&target) {
            target = next;
        }
        if let Some(&d) = draft_of_step.get(&target) {
            for &o in &s.outputs {
                owned_from.entry(s.seq).or_default().push((o, d));
            }
        }
    }
    // Names: attach to the chapter owning the solid at (or after) the name.
    let mut pending: HashMap<u64, Vec<usize>> = HashMap::new(); // solid -> name steps
    let mut name_rows: Vec<usize> = Vec::new();
    let mut gained = owned_from.into_iter().peekable();
    for (i, s) in steps.iter().enumerate() {
        while let Some((seq, _)) = gained.peek() {
            if *seq > s.seq {
                break;
            }
            let (_, solids) = gained.next().unwrap_or_default();
            for (solid, d) in solids {
                owner.insert(solid, d);
                if let Some(names) = pending.remove(&solid) {
                    for n in names {
                        attach_name(&mut drafts[d], &steps[n]);
                    }
                }
            }
        }
        if !s.is_name() {
            continue;
        }
        match s.inputs.first() {
            Some(solid) => match owner.get(solid) {
                Some(&d) => attach_name(&mut drafts[d], s),
                None => pending.entry(*solid).or_default().push(i),
            },
            None => name_rows.push(i),
        }
    }
    // A name whose solid never appears is its own row.
    let mut leftovers: Vec<usize> = pending.into_values().flatten().collect();
    leftovers.extend(name_rows);
    for n in leftovers {
        let s = &steps[n];
        drafts.push(Draft {
            chapter: Chapter {
                kind: "set_name".into(),
                name: s.name_text(),
                events: vec![s.seq],
                instances: Vec::new(),
                anchor: s.seq,
            },
            anchor: s.seq,
            tool: None,
        });
    }

    drafts.sort_by_key(|d| d.anchor);
    for d in &mut drafts {
        d.chapter.events.sort_unstable();
    }
    group_patterns(drafts)
}

fn attach_name(d: &mut Draft, name: &Step) {
    d.chapter.events.push(name.seq);
    if d.chapter.name.is_none() {
        d.chapter.name = name.name_text();
    }
}

fn kind_of(s: &Step, tool: Option<&Step>) -> String {
    let base = |c: &str| -> String {
        match c {
            "create_box_3d" => "box",
            "create_cylinder_3d" => "cylinder",
            "create_sphere_3d" => "sphere",
            "create_cone_3d" => "cone",
            "create_torus_3d" => "torus",
            "transform_solid" => "transform",
            _ if c.starts_with("revolve") => "revolve",
            _ if c.contains("extrude") => "extrude",
            _ if c.starts_with("fillet") => "fillet",
            _ if c.starts_with("chamfer") => "chamfer",
            _ if c.starts_with("shell") || c.starts_with("offset") => "shell",
            _ if c.starts_with("loft") => "loft",
            _ if c.starts_with("sweep") => "sweep",
            other => other,
        }
        .to_string()
    };
    if !s.is_boolean() {
        return base(s.command);
    }
    let op = s.command.trim_start_matches("boolean_");
    match (op, tool.map(|t| t.command)) {
        ("difference", Some("create_cylinder_3d")) => "hole".into(),
        ("difference", Some(_)) => "cut".into(),
        ("union", Some(_)) => "boss".into(),
        ("difference", None) => "subtract".into(),
        ("union", None) => "union".into(),
        ("intersection", _) => "intersect".into(),
        (other, _) => other.into(),
    }
}

/// The tool's shape and size, without where it is.
fn tool_signature(tool: &Step) -> String {
    let mut parts: Vec<String> = tool
        .create_parameters()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| {
                    !k.starts_with("base_") && !k.starts_with("center_") && !k.starts_with("origin")
                })
                .map(|(k, v)| format!("{k}={v}"))
                .collect()
        })
        .unwrap_or_default();
    parts.sort();
    format!("{}:{}", tool.command, parts.join(","))
}

fn tool_base(tool: &Step) -> Option<[f64; 3]> {
    let m = tool.create_parameters()?;
    let g = |k: &str| m.get(k).and_then(Value::as_f64);
    Some([g("base_x")?, g("base_y")?, g("base_z")?])
}

const MIN_PATTERN: usize = 3;

fn group_patterns(drafts: Vec<Draft>) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = Vec::new();
    let mut i = 0;
    while i < drafts.len() {
        let mut j = i + 1;
        if let Some(first) = drafts[i].tool.as_ref() {
            while j < drafts.len() {
                let (prev, next) = (&drafts[j - 1], &drafts[j]);
                let chained = match (prev.tool.as_ref(), next.tool.as_ref()) {
                    (Some(p), Some(n)) => {
                        n.signature == first.signature
                            && next.chapter.kind == drafts[i].chapter.kind
                            && n.part_in.is_some()
                            && n.part_in == p.part_out
                    }
                    _ => false,
                };
                if !chained {
                    break;
                }
                j += 1;
            }
        }
        let run = &drafts[i..j];
        let points: Option<Vec<[f64; 3]>> = run
            .iter()
            .map(|d| d.tool.as_ref().and_then(|t| t.base))
            .collect();
        if run.len() >= MIN_PATTERN && points.as_deref().is_some_and(regularly_spaced) {
            let instances: Vec<Chapter> = run.iter().map(|d| d.chapter.clone()).collect();
            let mut events: Vec<u64> = instances
                .iter()
                .flat_map(|c| c.events.iter().copied())
                .collect();
            events.sort_unstable();
            out.push(Chapter {
                kind: "pattern".into(),
                name: None,
                events,
                anchor: instances.last().map_or(0, |c| c.anchor),
                instances,
            });
            i = j;
        } else {
            out.push(drafts[i].chapter.clone());
            i += 1;
        }
    }
    out
}

/// Equal steps along a line, or equal angles on a circle (in order).
fn regularly_spaced(p: &[[f64; 3]]) -> bool {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let scale = p.iter().map(|q| len(*q)).fold(1.0_f64, f64::max);
    let tol = 1e-6 * scale;
    // Linear: every consecutive step the same vector.
    let d0 = sub(p[1], p[0]);
    if len(d0) > tol && p.windows(2).all(|w| len(sub(sub(w[1], w[0]), d0)) <= tol) {
        return true;
    }
    // Circular: all at one distance from their centroid, in one plane, with
    // equal angles between consecutive points.
    let n = p.len() as f64;
    let c = [
        p.iter().map(|q| q[0]).sum::<f64>() / n,
        p.iter().map(|q| q[1]).sum::<f64>() / n,
        p.iter().map(|q| q[2]).sum::<f64>() / n,
    ];
    let r0 = len(sub(p[0], c));
    if r0 <= tol || p.iter().any(|q| (len(sub(*q, c)) - r0).abs() > tol) {
        return false;
    }
    let a = sub(p[0], c);
    let b = sub(p[1], c);
    let normal = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let nl = len(normal);
    if nl <= tol * r0 {
        return false;
    }
    let nrm = [normal[0] / nl, normal[1] / nl, normal[2] / nl];
    let dot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    if p.iter().any(|q| dot(sub(*q, c), nrm).abs() > tol) {
        return false;
    }
    let angle = |u: [f64; 3], v: [f64; 3]| {
        let cr = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        dot(cr, nrm).atan2(dot(u, v))
    };
    let step = angle(sub(p[0], c), sub(p[1], c));
    p.windows(2)
        .all(|w| (angle(sub(w[0], c), sub(w[1], c)) - step).abs() <= 1e-6)
}
