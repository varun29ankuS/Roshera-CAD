//! Agent tool definitions owned by the backend — stage 1 of the single-registry
//! design (`roshera-eval/mcp/SINGLE_REGISTRY_DESIGN.md`).
//!
//! Before this module a tool existed in up to four hand-kept places: the zod
//! schema in `roshera-mcp`, a transcription of it in `agent_registry.rs`, the
//! TypeScript handler that turned arguments into a REST call, and the gate
//! tables. The assembly bench is the first to have exactly ONE definition:
//! `agent_tools/assembly.json`, compiled into this binary.
//!
//! A definition carries everything a client needs to list and call the tool:
//! name, bench, stability, effects (read-only / mutates / destructive),
//! a one-line `summary` (what listings show), the long `description`, the
//! JSON Schema of its arguments, how a call maps onto this server's own
//! route (`call`), and worked `examples`. `GET /api/agent/tools` serves the
//! file; `roshera-mcp` registers every served tool generically from it (no
//! per-tool TypeScript), so the in-app agent (goose, which runs roshera-mcp)
//! and an external MCP client call the same definition. `agent_registry.rs`
//! takes its rows for these tools from here, so its served registry cannot
//! disagree with them either.
//!
//! ## `call`
//! Either one route, or `{switch: <arg>, cases: {<value>: route}}` when an
//! enum argument picks the route (`assembly_mate`'s `action`). A route is
//! `{method, path, query?, body?, requires?, result?}`:
//! - `path` placeholders `{arg}` are filled from string arguments,
//!   URL-encoded;
//! - `query` names arguments sent as query parameters when present;
//! - `body` is a JSON template: `{"$arg": "x"}` is argument `x` (omitted when
//!   absent unless `"default"` is given), `{"$pose": {matrix, position,
//!   angle_deg, axis}}` is a row-major 4×4 pose built from those arguments;
//! - `requires` lists arguments a switch case needs beyond the schema's
//!   `required`;
//! - `result` reshapes the answer: `{rename, echo}` or `{image, caption}`.
//!
//! The vocabulary is deliberately this small. A tool whose behaviour cannot be
//! said in it gets a route here that does the work (`POST
//! /api/assembly/{id}/joint` replaced a three-call composition), not a new
//! interpreter feature.

use crate::error_catalog::{ApiError, ErrorCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The registry wire format this server speaks. A client that speaks a
/// different MAJOR version must refuse to start rather than list half a
/// surface.
pub const REGISTRY_VERSION: &str = "2.0.0";

/// The definition files compiled into this binary, by bench.
const DEFINITION_FILES: &[(&str, &str)] =
    &[("assembly", include_str!("agent_tools/assembly.json"))];

/// The benches a definition may declare (the same set as `agent_registry`).
const BENCHES: &[&str] = &[
    "core", "sketch", "assembly", "drawing", "analysis", "simulate", "labels", "timeline",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefinitionFile {
    registry_version: String,
    #[allow(dead_code)]
    note: String,
    tools: Vec<ToolDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effects {
    ReadOnly,
    Mutates,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    Stable,
    Experimental,
}

impl Stability {
    pub fn as_str(self) -> &'static str {
        match self {
            Stability::Stable => "stable",
            Stability::Experimental => "experimental",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum Method {
    Get,
    Post,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteCall {
    #[allow(dead_code)]
    method: Method,
    path: String,
    #[serde(default)]
    query: Vec<String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    result: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchCall {
    switch: String,
    cases: BTreeMap<String, RouteCall>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Call {
    Switch(SwitchCall),
    Route(RouteCall),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Example {
    title: String,
    args: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDef {
    pub name: String,
    pub bench: String,
    pub stability: Stability,
    pub effects: Effects,
    /// Served to clients (MCP's idempotentHint); the server itself does not
    /// branch on it.
    #[allow(dead_code)]
    pub idempotent: bool,
    pub summary: String,
    pub description: String,
    pub input_schema: Value,
    call: Call,
    examples: Vec<Example>,
}

/// Everything parsed from the definition files: the typed defs (for checks
/// and for `agent_registry`) and the raw JSON (what is served, verbatim).
struct Loaded {
    defs: Vec<ToolDef>,
    raw: Vec<Value>,
}

fn load() -> Result<Loaded, String> {
    let mut defs = Vec::new();
    let mut raw = Vec::new();
    for (bench, text) in DEFINITION_FILES {
        let file: DefinitionFile = serde_json::from_str(text)
            .map_err(|e| format!("agent_tools/{bench}.json does not parse: {e}"))?;
        if file.registry_version != REGISTRY_VERSION {
            return Err(format!(
                "agent_tools/{bench}.json declares registry_version {} but this server speaks {REGISTRY_VERSION}",
                file.registry_version
            ));
        }
        let value: Value = serde_json::from_str(text)
            .map_err(|e| format!("agent_tools/{bench}.json does not parse: {e}"))?;
        let tools = value
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| format!("agent_tools/{bench}.json has no tools array"))?;
        defs.extend(file.tools);
        raw.extend(tools);
    }
    let problems = check(&defs);
    if !problems.is_empty() {
        return Err(format!(
            "agent tool definitions are inconsistent: {}",
            problems.join("; ")
        ));
    }
    Ok(Loaded { defs, raw })
}

fn loaded() -> &'static Result<Loaded, String> {
    static LOADED: OnceLock<Result<Loaded, String>> = OnceLock::new();
    LOADED.get_or_init(load)
}

/// The served definitions, or why they could not be loaded.
pub fn definitions() -> Result<&'static [ToolDef], &'static str> {
    match loaded() {
        Ok(l) => Ok(&l.defs),
        Err(e) => Err(e.as_str()),
    }
}

// ── Consistency checks (run at load AND by the tests) ──────────────────────

fn properties(schema: &Value) -> BTreeMap<String, Value> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn required(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `{name}` placeholders of a path template.
fn placeholders(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                out.push(after[..close].to_string());
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// Every argument name a body template reads.
fn body_args(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(name)) = m.get("$arg") {
                out.push(name.clone());
                return;
            }
            if let Some(Value::Object(pose)) = m.get("$pose") {
                for name in pose.values().filter_map(Value::as_str) {
                    out.push(name.to_string());
                }
                return;
            }
            for x in m.values() {
                body_args(x, out);
            }
        }
        Value::Array(a) => {
            for x in a {
                body_args(x, out);
            }
        }
        _ => {}
    }
}

fn check_route(
    tool: &str,
    case: &str,
    route: &RouteCall,
    props: &BTreeMap<String, Value>,
    always: &BTreeSet<String>,
    problems: &mut Vec<String>,
) {
    let at = if case.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} (case {case})")
    };
    if !route.path.starts_with("/api/") {
        problems.push(format!("{at}: path {} is not an /api/ route", route.path));
    }
    for p in placeholders(&route.path) {
        let is_string = props
            .get(&p)
            .and_then(|s| s.get("type"))
            .and_then(Value::as_str)
            == Some("string");
        if !is_string {
            problems.push(format!(
                "{at}: path placeholder {{{p}}} is not a string argument"
            ));
        }
        if !always.contains(&p) && !route.requires.contains(&p) {
            problems.push(format!(
                "{at}: path placeholder {{{p}}} is neither required nor in `requires`"
            ));
        }
    }
    let mut reads = route.query.clone();
    reads.extend(route.requires.iter().cloned());
    if let Some(body) = &route.body {
        body_args(body, &mut reads);
    }
    for name in reads {
        if !props.contains_key(&name) {
            problems.push(format!(
                "{at}: reads argument '{name}' the schema does not declare"
            ));
        }
    }
    if let Some(result) = &route.result {
        let keys: BTreeSet<&str> = result
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let allowed = [
            BTreeSet::from(["rename", "echo"]),
            BTreeSet::from(["rename"]),
            BTreeSet::from(["echo"]),
            BTreeSet::from(["image", "caption"]),
        ];
        if !allowed.contains(&keys) {
            problems.push(format!(
                "{at}: result spec {result} is not one this format defines"
            ));
        }
    }
}

/// Every inconsistency in a set of definitions. Empty = consistent.
fn check(defs: &[ToolDef]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut names = BTreeSet::new();
    for d in defs {
        let n = d.name.as_str();
        if n.is_empty()
            || !n
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            problems.push(format!("tool name {n:?} is not snake_case"));
        }
        if !names.insert(n) {
            problems.push(format!("tool {n} is defined twice"));
        }
        if !BENCHES.contains(&d.bench.as_str()) {
            problems.push(format!("{n}: bench {} is not one of {BENCHES:?}", d.bench));
        }
        if d.summary.trim().is_empty() || d.summary.contains('\n') {
            problems.push(format!("{n}: summary must be one non-empty line"));
        }
        if d.description.trim().is_empty() {
            problems.push(format!("{n}: description is empty"));
        }
        if d.input_schema.get("type").and_then(Value::as_str) != Some("object") {
            problems.push(format!("{n}: input_schema.type must be \"object\""));
        }
        if d.input_schema.get("additionalProperties") != Some(&Value::Bool(false)) {
            problems.push(format!(
                "{n}: input_schema must set additionalProperties:false (an unknown argument is refused, never dropped)"
            ));
        }
        let props = properties(&d.input_schema);
        let always = required(&d.input_schema);
        // A destructive tool must take an explicit confirmation (the design's
        // rule; roshera-mcp's clear_parts / clear_timeline / blackboard_clear).
        if d.effects == Effects::Destructive && !props.contains_key("confirm") {
            problems.push(format!(
                "{n}: a destructive tool must declare a `confirm` argument"
            ));
        }
        for r in &always {
            if !props.contains_key(r) {
                problems.push(format!("{n}: requires '{r}', which is not a property"));
            }
        }
        match &d.call {
            Call::Route(route) => check_route(n, "", route, &props, &always, &mut problems),
            Call::Switch(sw) => {
                let values: BTreeSet<String> = props
                    .get(&sw.switch)
                    .and_then(|s| s.get("enum"))
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let cases: BTreeSet<String> = sw.cases.keys().cloned().collect();
                if values.is_empty() || values != cases {
                    problems.push(format!(
                        "{n}: switch on '{}' must have exactly one case per enum value (enum {values:?}, cases {cases:?})",
                        sw.switch
                    ));
                }
                for (case, route) in &sw.cases {
                    check_route(n, case, route, &props, &always, &mut problems);
                }
            }
        }
        if d.examples.is_empty() {
            problems.push(format!("{n}: needs at least one example"));
        }
        for ex in &d.examples {
            let Some(args) = ex.args.as_object() else {
                problems.push(format!(
                    "{n}: example '{}' args are not an object",
                    ex.title
                ));
                continue;
            };
            for k in args.keys() {
                if !props.contains_key(k) {
                    problems.push(format!("{n}: example '{}' passes unknown '{k}'", ex.title));
                }
            }
            let mut needed: Vec<String> = always.iter().cloned().collect();
            if let Call::Switch(sw) = &d.call {
                let chosen = args
                    .get(&sw.switch)
                    .or_else(|| props.get(&sw.switch).and_then(|s| s.get("default")))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match sw.cases.get(chosen) {
                    Some(route) => needed.extend(route.requires.iter().cloned()),
                    None => problems.push(format!(
                        "{n}: example '{}' selects no case of '{}'",
                        ex.title, sw.switch
                    )),
                }
            }
            for r in needed {
                if !args.contains_key(&r) {
                    problems.push(format!("{n}: example '{}' omits required '{r}'", ex.title));
                }
            }
        }
    }
    problems
}

// ── Serving ────────────────────────────────────────────────────────────────

/// The registry rows `agent_registry` serves for these tools:
/// `(name, bench, stability, purpose, input_schema)`.
pub fn registry_rows() -> Result<Vec<(String, String, &'static str, String, Value)>, &'static str> {
    Ok(definitions()?
        .iter()
        .map(|d| {
            (
                d.name.clone(),
                d.bench.clone(),
                d.stability.as_str(),
                d.summary.clone(),
                d.input_schema.clone(),
            )
        })
        .collect())
}

/// `GET /api/agent/tools` — every backend-owned tool definition, verbatim.
pub async fn list_tools() -> Result<Json<Value>, ApiError> {
    match loaded() {
        Ok(l) => Ok(Json(json!({
            "registry_version": REGISTRY_VERSION,
            "tool_count": l.raw.len(),
            "tools": l.raw,
        }))),
        Err(e) => Err(ApiError::new(ErrorCode::Internal, e.clone()).with_hint(
            "the server's compiled tool definitions are inconsistent; this is a build defect, not a request error",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compiled definitions load and pass every consistency check.
    #[test]
    fn the_compiled_definitions_are_consistent() {
        match loaded() {
            Ok(l) => assert_eq!(l.defs.len(), l.raw.len()),
            Err(e) => assert!(false, "{e}"),
        }
    }

    /// The assembly bench is served whole: twelve tools, all `assembly`.
    #[test]
    fn the_assembly_bench_is_served_from_its_definition_file() {
        let Ok(defs) = definitions() else {
            unreachable!("definitions load: {:?}", definitions().err());
        };
        let names: BTreeSet<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        let expected: BTreeSet<&str> = [
            "assembly_verify",
            "assembly_create",
            "assembly_add_instance",
            "assembly_list_instances",
            "assembly_transform_instance",
            "assembly_view",
            "assembly_mate",
            "assembly_solve",
            "assembly_certify",
            "assembly_dof",
            "assembly_drag",
            "assembly_interference",
        ]
        .into_iter()
        .collect();
        assert_eq!(names, expected);
        assert!(defs.iter().all(|d| d.bench == "assembly"));
    }

    /// The checker is not vacuous: each class of defect it exists for is
    /// reported (mutation proof for `check`).
    #[test]
    fn the_checker_reports_each_defect_class() {
        let Ok(defs) = definitions() else {
            unreachable!("definitions load");
        };
        let mate = defs.iter().find(|d| d.name == "assembly_mate").cloned();
        let Some(mate) = mate else {
            unreachable!("assembly_mate is defined");
        };

        // A path placeholder that is not an argument.
        let mut d = mate.clone();
        if let Call::Switch(sw) = &mut d.call {
            if let Some(r) = sw.cases.get_mut("remove") {
                r.path = "/api/assembly/{assembly_id}/mate/{nope}".into();
            }
        }
        assert!(check(&[d]).iter().any(|p| p.contains("{nope}")));

        // A body that reads an undeclared argument.
        let mut d = mate.clone();
        if let Call::Switch(sw) = &mut d.call {
            if let Some(r) = sw.cases.get_mut("create") {
                r.body = Some(json!({ "kind": { "$arg": "knid" } }));
            }
        }
        assert!(check(&[d]).iter().any(|p| p.contains("'knid'")));

        // An example missing a case requirement.
        let mut d = mate.clone();
        d.examples = vec![Example {
            title: "remove without an id".into(),
            args: json!({ "assembly_id": "x", "action": "remove" }),
        }];
        assert!(check(&[d])
            .iter()
            .any(|p| p.contains("omits required 'mate_id'")));

        // A schema that would drop unknown arguments.
        let mut d = mate.clone();
        if let Some(o) = d.input_schema.as_object_mut() {
            o.remove("additionalProperties");
        }
        assert!(check(&[d])
            .iter()
            .any(|p| p.contains("additionalProperties")));

        // A duplicate name.
        assert!(check(&[mate.clone(), mate])
            .iter()
            .any(|p| p.contains("defined twice")));
    }

    /// Production call site: every route a definition calls is a route this
    /// server registers (read from `main.rs`, placeholders normalised), so a
    /// definition cannot name a path that 404s.
    #[test]
    fn every_defined_route_is_registered_in_the_router() {
        let main = include_str!("main.rs");
        let normalise = |p: &str| {
            let mut out = String::new();
            let mut depth = 0;
            for c in p.chars() {
                match c {
                    '{' => {
                        depth += 1;
                        out.push_str("{}");
                    }
                    '}' => depth -= 1,
                    _ if depth > 0 => {}
                    _ => out.push(c),
                }
            }
            out
        };
        // Each `.route(` call's first argument, the path literal.
        let mut registered = BTreeSet::new();
        for after in main.split(".route(").skip(1) {
            let lit = after.trim_start();
            if let Some(rest) = lit.strip_prefix('"') {
                if let Some(end) = rest.find('"') {
                    let path = &rest[..end];
                    if path.starts_with("/api/") {
                        registered.insert(normalise(path));
                    }
                }
            }
        }
        assert!(
            registered.len() > 100,
            "parsed only {} routes from main.rs; the scan is broken",
            registered.len()
        );
        let Ok(defs) = definitions() else {
            unreachable!("definitions load");
        };
        for d in defs {
            let routes: Vec<&RouteCall> = match &d.call {
                Call::Route(r) => vec![r],
                Call::Switch(sw) => sw.cases.values().collect(),
            };
            for r in routes {
                assert!(
                    registered.contains(&normalise(&r.path)),
                    "{} calls {}, which main.rs does not route",
                    d.name,
                    r.path
                );
            }
        }
    }
}
