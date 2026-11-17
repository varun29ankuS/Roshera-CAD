//! The live user context: what the person at the app is pointing at,
//! selecting and looking at, kept per document so an agent can read it
//! while it works.
//!
//! - `POST /api/agent/user-context` -- the app reports its context (hover,
//!   selection, selected feature, history step, active tool, pick filter,
//!   camera). Latest wins: attention has no backlog. The answer says whether
//!   an agent has read it, and how long ago.
//! - `GET /api/agent/user-context` -- an agent reads it. Every entity is
//!   resolved against the live model at read time and carries its persistent
//!   id when the kernel has assigned one. An entity that no longer resolves
//!   is returned `resolved: false` with the reason; it is never re-targeted.
//!
//! What this is not: it is not history. Nothing here is recorded in the
//! timeline or written to disk; the server keeps one report per document and
//! forgets it when the next arrives. A report with `sharing: false` stores
//! the pause and nothing else, whatever else the body carried, so a paused
//! app cannot leak what the person is pointing at.
//!
//! A read that carries `X-Roshera-Agent` is an agent looking. It is noted
//! (the next report's answer carries it) and one `AgentLooked` frame goes to
//! every open app, which shows it next to what the agent saw.

use std::time::Instant;

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use dashmap::DashMap;
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent_show::Workspace;
use crate::error_catalog::{ApiError, ErrorCode};
use crate::part_mgr::ActiveModel;
use crate::AppState;

/// The most entities one report may select.
pub const MAX_SELECTION: usize = 256;
/// The longest free-text field (tool name, camera preset, object id).
pub const MAX_TEXT: usize = 128;
/// A report older than this is `stale`: the app has stopped reporting (it
/// reports at least every two seconds while it is open and in front).
pub const LIVE_WINDOW_MS: u64 = 6_000;
/// Documents whose context is kept; the one reported longest ago is dropped.
pub const MAX_DOCUMENTS: usize = 64;

/// What kind of thing an entity is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Part,
    Face,
    Edge,
    Vertex,
}

impl EntityKind {
    fn name(self) -> &'static str {
        match self {
            EntityKind::Part => "part",
            EntityKind::Face => "face",
            EntityKind::Edge => "edge",
            EntityKind::Vertex => "vertex",
        }
    }
}

/// One thing the person hovers or has selected, as the app knows it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntityReport {
    /// The scene object: a part's public UUID.
    pub object_id: String,
    pub kind: EntityKind,
    /// The kernel id of the face, edge or vertex. Absent for a part, and
    /// absent when the app has no kernel id for what is under the cursor
    /// (an edge or vertex being hovered is only a point until it is picked).
    #[serde(default)]
    pub id: Option<u32>,
    /// Where on the part the cursor is, in model space.
    #[serde(default)]
    pub point: Option<[f64; 3]>,
}

/// How the view is projected.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    Perspective,
    Orthographic,
}

/// Where the person is looking from.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CameraReport {
    pub position: [f64; 3],
    /// The direction the camera looks along.
    pub direction: [f64; 3],
    pub up: [f64; 3],
    pub projection: Projection,
    /// The named view the camera was last put in, when it still is in it.
    #[serde(default)]
    pub preset: Option<String>,
}

/// One report from the app.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserContextReport {
    /// False: the person paused sharing. Nothing else in the report is kept.
    pub sharing: bool,
    #[serde(default)]
    pub workspace: Option<Workspace>,
    /// The active tool, as the app names it (`select`, `translate`, ...).
    #[serde(default)]
    pub tool: Option<String>,
    /// What a click picks.
    #[serde(default)]
    pub pick_filter: Option<EntityKind>,
    #[serde(default)]
    pub hover: Option<EntityReport>,
    #[serde(default)]
    pub selection: Vec<EntityReport>,
    /// How many things are selected, when more than `selection` lists.
    #[serde(default)]
    pub selection_total: Option<u64>,
    /// The feature selected in the history (its anchor step).
    #[serde(default)]
    pub feature: Option<u64>,
    /// The past step the view shows; absent for the live part.
    #[serde(default)]
    pub history_step: Option<u64>,
    #[serde(default)]
    pub camera: Option<CameraReport>,
    /// The app's clock, ms since epoch. An ordering hint only.
    #[serde(default)]
    pub at_ms: Option<f64>,
}

impl UserContextReport {
    /// What is kept of a report made while sharing is paused: the pause.
    fn paused(at_ms: Option<f64>) -> Self {
        Self {
            sharing: false,
            workspace: None,
            tool: None,
            pick_filter: None,
            hover: None,
            selection: Vec::new(),
            selection_total: None,
            feature: None,
            history_step: None,
            camera: None,
            at_ms,
        }
    }
}

/// An agent's read of a document's context.
#[derive(Debug, Clone)]
struct Look {
    agent: String,
    at: Instant,
    /// The report the agent read.
    seq: u64,
}

/// When an agent last read the context, as the app is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentLook {
    pub agent: String,
    pub age_ms: u64,
    /// The report it read (compare with the report just acknowledged).
    pub seq: u64,
}

/// The answer to a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishAck {
    /// This report's number for the document, counted from 1.
    pub seq: u64,
    /// The last agent read of this document's context, if any.
    pub agent_look: Option<AgentLook>,
}

#[derive(Debug, Clone)]
struct Slot {
    seq: u64,
    received: Instant,
    report: UserContextReport,
    look: Option<Look>,
}

/// A document's context as last reported.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub seq: u64,
    pub age_ms: u64,
    pub report: UserContextReport,
}

fn millis_between(earlier: Instant, later: Instant) -> u64 {
    u64::try_from(later.saturating_duration_since(earlier).as_millis()).unwrap_or(u64::MAX)
}

/// The latest context of each document. Held on the viewport bridge.
#[derive(Default)]
pub struct UserContextStore {
    slots: DashMap<String, Slot>,
}

impl UserContextStore {
    /// Keep `report` as the document's context. A paused report keeps only
    /// the pause.
    pub fn publish(&self, document: &str, report: UserContextReport, now: Instant) -> PublishAck {
        let kept = if report.sharing {
            report
        } else {
            UserContextReport::paused(report.at_ms)
        };
        let ack = {
            let mut slot = self
                .slots
                .entry(document.to_string())
                .or_insert_with(|| Slot {
                    seq: 0,
                    received: now,
                    report: UserContextReport::paused(None),
                    look: None,
                });
            slot.seq += 1;
            slot.received = now;
            slot.report = kept;
            PublishAck {
                seq: slot.seq,
                agent_look: slot.look.as_ref().map(|l| AgentLook {
                    agent: l.agent.clone(),
                    age_ms: millis_between(l.at, now),
                    seq: l.seq,
                }),
            }
        };
        self.evict_beyond_capacity(document);
        ack
    }

    /// The document's context, if any was ever reported.
    pub fn read(&self, document: &str, now: Instant) -> Option<Snapshot> {
        self.slots.get(document).map(|s| Snapshot {
            seq: s.seq,
            age_ms: millis_between(s.received, now),
            report: s.report.clone(),
        })
    }

    /// Note that `agent` read the document's context. Returns the number of
    /// the report it read, or `None` when there is none to have read.
    pub fn note_look(&self, document: &str, agent: &str, now: Instant) -> Option<u64> {
        let mut slot = self.slots.get_mut(document)?;
        let seq = slot.seq;
        slot.look = Some(Look {
            agent: agent.to_string(),
            at: now,
            seq,
        });
        Some(seq)
    }

    /// Drop the documents reported longest ago until at most
    /// [`MAX_DOCUMENTS`] remain; `keep` is never dropped.
    fn evict_beyond_capacity(&self, keep: &str) {
        while self.slots.len() > MAX_DOCUMENTS {
            let oldest = self
                .slots
                .iter()
                .filter(|e| e.key() != keep)
                .min_by_key(|e| e.value().received)
                .map(|e| e.key().clone());
            match oldest {
                Some(key) => {
                    self.slots.remove(&key);
                }
                None => break,
            }
        }
    }
}

/// `live`, `stale` (the app stopped reporting) or `paused`.
fn state_name(snapshot: &Snapshot) -> &'static str {
    if !snapshot.report.sharing {
        "paused"
    } else if snapshot.age_ms > LIVE_WINDOW_MS {
        "stale"
    } else {
        "live"
    }
}

fn refuse(parameter: &str, message: String) -> ApiError {
    ApiError::new(ErrorCode::InvalidParameter, message)
        .with_details(json!({ "parameter": parameter }))
}

fn check_text(parameter: &str, text: &str) -> Result<(), ApiError> {
    if text.chars().count() > MAX_TEXT {
        return Err(refuse(
            parameter,
            format!("{parameter} is longer than {MAX_TEXT} characters"),
        ));
    }
    Ok(())
}

fn check_finite(parameter: &str, v: &[f64; 3]) -> Result<(), ApiError> {
    if v.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(refuse(
            parameter,
            format!("{parameter} must be three finite numbers"),
        ))
    }
}

fn check_entity(parameter: &str, e: &EntityReport) -> Result<(), ApiError> {
    check_text(parameter, &e.object_id)?;
    if e.object_id.trim().is_empty() {
        return Err(refuse(parameter, format!("{parameter}.object_id is empty")));
    }
    if let Some(p) = &e.point {
        check_finite(parameter, p)?;
    }
    Ok(())
}

/// Every check the types cannot make. A report that fails is refused whole.
pub fn validate(report: &UserContextReport) -> Result<(), ApiError> {
    if report.selection.len() > MAX_SELECTION {
        return Err(refuse(
            "selection",
            format!(
                "selection has {} entities; at most {MAX_SELECTION} are reported",
                report.selection.len()
            ),
        ));
    }
    if let Some(t) = &report.tool {
        check_text("tool", t)?;
    }
    if let Some(h) = &report.hover {
        check_entity("hover", h)?;
    }
    for e in &report.selection {
        check_entity("selection", e)?;
    }
    if let Some(c) = &report.camera {
        check_finite("camera.position", &c.position)?;
        check_finite("camera.direction", &c.direction)?;
        check_finite("camera.up", &c.up)?;
        if let Some(p) = &c.preset {
            check_text("camera.preset", p)?;
        }
    }
    if let Some(t) = report.at_ms {
        if !t.is_finite() {
            return Err(refuse("at_ms", "at_ms must be a finite number".to_string()));
        }
    }
    Ok(())
}

fn hex(pid: PersistentId) -> String {
    format!("{:032x}", pid.0)
}

/// Every face of `solid`, over all of its shells.
fn faces_of(model: &BRepModel, solid: u32) -> Vec<u32> {
    let shells = model
        .solids
        .get(solid)
        .map(|s| s.all_shells())
        .unwrap_or_default();
    let mut faces = Vec::new();
    for sh in shells {
        if let Some(shell) = model.shells.get(sh) {
            faces.extend(shell.faces.iter().copied());
        }
    }
    faces
}

/// Every edge on the boundary of a face of `solid`.
fn edges_of(model: &BRepModel, solid: u32) -> Vec<u32> {
    let mut edges = Vec::new();
    for f in faces_of(model, solid) {
        let Some(face) = model.faces.get(f) else {
            continue;
        };
        let loops = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
        for l in loops {
            if let Some(lp) = model.loops.get(l) {
                edges.extend(lp.edges.iter().copied());
            }
        }
    }
    edges
}

fn unresolved(mut base: serde_json::Map<String, Value>, reason: String) -> Value {
    base.insert("resolved".into(), json!(false));
    base.insert("reason".into(), json!(reason));
    Value::Object(base)
}

/// One reported entity against the live model. `live` is false when the
/// document the report was made in is not the one whose model is loaded.
fn resolve_entity(state: &AppState, model: &mut BRepModel, live: bool, e: &EntityReport) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("kind".into(), json!(e.kind.name()));
    out.insert("object_id".into(), json!(e.object_id));
    if let Some(id) = e.id {
        out.insert("id".into(), json!(id));
    }
    if let Some(p) = e.point {
        out.insert("point".into(), json!(p));
    }
    if !live {
        return unresolved(
            out,
            "this document is not the one open in the kernel now, so its entities cannot be checked"
                .to_string(),
        );
    }
    let Ok(uuid) = uuid::Uuid::parse_str(&e.object_id) else {
        return unresolved(
            out,
            "not a part of this document (an assembly instance or a display-only object)"
                .to_string(),
        );
    };
    let Some(solid) = state
        .get_local_id(&uuid)
        .filter(|id| model.solids.get(*id).is_some())
    else {
        return unresolved(
            out,
            "this part is not in the model: it was deleted or consumed by a later operation"
                .to_string(),
        );
    };
    out.insert("solid_id".into(), json!(solid));
    let name = model
        .solids
        .get(solid)
        .and_then(|s| s.name.clone())
        .unwrap_or_else(|| format!("solid_{solid}"));
    out.insert("part_name".into(), json!(name));
    out.insert(
        "part_persistent_id".into(),
        json!(model.solid_pid(solid).map(hex)),
    );

    let kind = e.kind;
    let id = match (kind, e.id) {
        (EntityKind::Part, _) => {
            out.insert("resolved".into(), json!(true));
            return Value::Object(out);
        }
        (_, Some(id)) => id,
        (_, None) => {
            return unresolved(
                out,
                format!(
                    "the app has no kernel id for this {}: only the part and the point are known",
                    kind.name()
                ),
            );
        }
    };
    match kind {
        EntityKind::Face => {
            let Some(report) = model.query_face(id) else {
                return unresolved(
                    out,
                    format!("face {id} is not in the model: a later operation replaced it"),
                );
            };
            if report.host_solid != Some(solid) {
                return unresolved(out, format!("face {id} is not a face of this part"));
            }
            out.insert("persistent_id".into(), json!(model.face_pid(id).map(hex)));
            out.insert("surface_type".into(), json!(report.surface_type));
            out.insert("area".into(), json!(report.area));
        }
        EntityKind::Edge => {
            if !edges_of(model, solid).contains(&id) {
                return unresolved(out, format!("edge {id} is not an edge of this part"));
            }
            let Some(report) = model.query_edge(id) else {
                return unresolved(
                    out,
                    format!("edge {id} is not in the model: a later operation replaced it"),
                );
            };
            out.insert("persistent_id".into(), json!(model.edge_pid(id).map(hex)));
            out.insert("curve_kind".into(), json!(report.curve_kind));
            out.insert("length".into(), json!(report.length));
        }
        EntityKind::Vertex => {
            let on_part = edges_of(model, solid).iter().any(|eid| {
                model
                    .edges
                    .get(*eid)
                    .is_some_and(|edge| edge.start_vertex == id || edge.end_vertex == id)
            });
            let position = model.vertices.get(id).map(|v| v.position);
            let (true, Some(position)) = (on_part, position) else {
                return unresolved(out, format!("vertex {id} is not a vertex of this part"));
            };
            // The kernel assigns no persistent id to a vertex: stated, not
            // invented.
            out.insert("persistent_id".into(), Value::Null);
            out.insert("position".into(), json!(position));
        }
        EntityKind::Part => {}
    }
    out.insert("resolved".into(), json!(true));
    Value::Object(out)
}

/// The agent named by `X-Roshera-Agent`, when the request carries one.
fn agent_name(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-roshera-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(MAX_TEXT).collect())
}

/// `POST /api/agent/user-context`: the app reports the person's context.
pub async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<PublishAck>, ApiError> {
    let Json(raw) = body.map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.body_text()))?;
    let report: UserContextReport = serde_json::from_value(raw).map_err(|e| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("not a user context report: {e}"),
        )
    })?;
    validate(&report)?;
    let document = crate::documents::resolve_document(&state, &headers).await?;
    let ack = state
        .viewport_bridge
        .user_context
        .publish(&document, report, Instant::now());
    Ok(Json(ack))
}

/// `GET /api/agent/user-context`: what the person is pointing at, selecting
/// and looking at in the caller's document, resolved against the live model.
///
/// Always `200`. `state` is `live`, `stale` (the app stopped reporting; the
/// content is the last it said, `age_ms` old), `paused` (the person paused
/// sharing: no entities) or `none` (no app has reported for this document).
pub async fn read(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let document = crate::documents::resolve_document(&state, &headers).await?;
    let store = &state.viewport_bridge.user_context;
    let now = Instant::now();
    let Some(snapshot) = store.read(&document, now) else {
        return Ok(Json(json!({
            "state": "none",
            "document_id": document,
            "note": "no app has reported a context for this document: nobody has it open, or sharing never started",
        })));
    };
    let state_now = state_name(&snapshot);
    if let Some(agent) = agent_name(&headers) {
        if let Some(seq) = store.note_look(&document, &agent, now) {
            let frame = json!({
                "type": "AgentLooked",
                "payload": {
                    "agent": agent,
                    "document_id": document,
                    "seq": seq,
                    "saw": state_now,
                },
            });
            // `send` errs only when no app is listening; the look is still
            // noted and the next report's answer carries it.
            let _ = crate::geometry_broadcaster().send(frame.to_string());
        }
    }
    let report = &snapshot.report;
    if !report.sharing {
        return Ok(Json(json!({
            "state": state_now,
            "document_id": document,
            "seq": snapshot.seq,
            "age_ms": snapshot.age_ms,
            "note": "the person paused sharing: what they point at and select is not available. Ask them.",
        })));
    }
    let live = state.active_document.read().await.as_str() == document;
    let (hover, selection) = {
        let mut model = model_handle.write().await;
        let hover = report
            .hover
            .as_ref()
            .map(|e| resolve_entity(&state, &mut model, live, e));
        let selection: Vec<Value> = report
            .selection
            .iter()
            .map(|e| resolve_entity(&state, &mut model, live, e))
            .collect();
        (hover, selection)
    };
    let mut out = json!({
        "state": state_now,
        "document_id": document,
        "seq": snapshot.seq,
        "age_ms": snapshot.age_ms,
        "workspace": report.workspace,
        "tool": report.tool,
        "pick_filter": report.pick_filter,
        "hover": hover,
        "selection": selection,
        "selection_total": report
            .selection_total
            .unwrap_or(report.selection.len() as u64),
        "feature_anchor": report.feature,
        "history_step": report.history_step,
        "camera": report.camera,
    });
    if state_now == "stale" {
        out["note"] = json!(
            "the app stopped reporting (closed, or in the background): this is the last it said"
        );
    }
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_router;
    use crate::router_integration_tests::make_test_state;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use std::time::Duration;
    use tower::ServiceExt;

    fn sharing_report() -> UserContextReport {
        UserContextReport {
            sharing: true,
            workspace: None,
            tool: Some("select".into()),
            pick_filter: Some(EntityKind::Face),
            hover: Some(EntityReport {
                object_id: "not-a-uuid".into(),
                kind: EntityKind::Face,
                id: Some(3),
                point: Some([1.0, 2.0, 3.0]),
            }),
            selection: vec![EntityReport {
                object_id: "not-a-uuid".into(),
                kind: EntityKind::Part,
                id: None,
                point: None,
            }],
            selection_total: None,
            feature: Some(7),
            history_step: None,
            camera: None,
            at_ms: Some(1.0),
        }
    }

    #[test]
    fn the_latest_report_wins_and_reports_are_numbered_per_document() {
        let store = UserContextStore::default();
        let t0 = Instant::now();
        assert_eq!(store.publish("a", sharing_report(), t0).seq, 1);
        let mut second = sharing_report();
        second.feature = Some(9);
        assert_eq!(store.publish("a", second, t0).seq, 2);
        assert_eq!(store.publish("b", sharing_report(), t0).seq, 1);
        let a = store.read("a", t0).expect("a was reported");
        assert_eq!(a.seq, 2);
        assert_eq!(
            a.report.feature,
            Some(9),
            "the later report replaced the first"
        );
        assert!(
            store.read("c", t0).is_none(),
            "an unreported document has none"
        );
    }

    #[test]
    fn a_paused_report_keeps_the_pause_and_nothing_else() {
        let store = UserContextStore::default();
        let t0 = Instant::now();
        let mut paused = sharing_report();
        paused.sharing = false;
        store.publish("a", paused, t0);
        let kept = store.read("a", t0).expect("the pause is kept").report;
        assert!(!kept.sharing);
        assert!(kept.hover.is_none(), "a paused app's hover is not kept");
        assert!(kept.selection.is_empty(), "nor its selection");
        assert!(kept.feature.is_none() && kept.tool.is_none() && kept.camera.is_none());
    }

    #[test]
    fn an_agents_look_is_told_to_the_next_report_with_its_age() {
        let store = UserContextStore::default();
        let t0 = Instant::now();
        assert!(
            store.note_look("a", "Claude", t0).is_none(),
            "nothing reported, nothing to have read"
        );
        assert_eq!(store.publish("a", sharing_report(), t0).agent_look, None);
        assert_eq!(store.note_look("a", "Claude", t0), Some(1));
        let ack = store.publish("a", sharing_report(), t0 + Duration::from_millis(1500));
        assert_eq!(
            ack.agent_look,
            Some(AgentLook {
                agent: "Claude".into(),
                age_ms: 1500,
                seq: 1
            })
        );
        assert_eq!(ack.seq, 2);
    }

    #[test]
    fn a_report_is_live_then_stale_and_paused_is_neither() {
        let store = UserContextStore::default();
        let t0 = Instant::now();
        store.publish("a", sharing_report(), t0);
        let fresh = store
            .read("a", t0 + Duration::from_millis(LIVE_WINDOW_MS))
            .expect("reported");
        assert_eq!(state_name(&fresh), "live");
        let old = store
            .read("a", t0 + Duration::from_millis(LIVE_WINDOW_MS + 1))
            .expect("reported");
        assert_eq!(state_name(&old), "stale");
        let mut paused = sharing_report();
        paused.sharing = false;
        store.publish("a", paused, t0);
        let p = store
            .read("a", t0 + Duration::from_millis(LIVE_WINDOW_MS + 1))
            .expect("reported");
        assert_eq!(state_name(&p), "paused");
    }

    #[test]
    fn only_the_most_recently_reported_documents_are_kept() {
        let store = UserContextStore::default();
        let t0 = Instant::now();
        for i in 0..=MAX_DOCUMENTS {
            let at = t0 + Duration::from_millis(i as u64);
            store.publish(&format!("doc-{i}"), sharing_report(), at);
        }
        assert!(store.read("doc-0", t0).is_none(), "the oldest was dropped");
        assert!(store.read("doc-1", t0).is_some());
        assert!(store.read(&format!("doc-{MAX_DOCUMENTS}"), t0).is_some());
    }

    #[test]
    fn an_oversized_or_non_finite_report_is_refused_by_name() {
        let mut many = sharing_report();
        many.selection = (0..=MAX_SELECTION)
            .map(|_| EntityReport {
                object_id: "x".into(),
                kind: EntityKind::Part,
                id: None,
                point: None,
            })
            .collect();
        let err = validate(&many).expect_err("257 entities are refused");
        assert_eq!(
            err.details.as_ref().map(|d| d["parameter"].clone()),
            Some(json!("selection"))
        );

        let mut nan = sharing_report();
        nan.hover = Some(EntityReport {
            object_id: "x".into(),
            kind: EntityKind::Face,
            id: Some(1),
            point: Some([0.0, f64::NAN, 0.0]),
        });
        let err = validate(&nan).expect_err("a NaN point is refused");
        assert_eq!(
            err.details.as_ref().map(|d| d["parameter"].clone()),
            Some(json!("hover"))
        );

        assert!(validate(&sharing_report()).is_ok());
    }

    // ── through the live router ──────────────────────────────────────

    async fn dispatch(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
        let response = build_router(state.clone())
            .oneshot(request)
            .await
            .expect("the router answers");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a finite body");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("a JSON body")
        };
        (status, body)
    }

    fn post(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request")
    }

    fn get(uri: &str, agent: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method(Method::GET).uri(uri);
        if let Some(a) = agent {
            b = b.header("X-Roshera-Agent", a);
        }
        b.body(Body::empty()).expect("a request")
    }

    /// A 10 mm box made through the geometry route: its object UUID and one
    /// of its kernel face ids.
    async fn a_box(state: &AppState) -> (String, u64) {
        let (status, body) = dispatch(
            state,
            post(
                "/api/geometry",
                json!({
                    "shape_type": "box",
                    "parameters": { "width": 10.0, "height": 10.0, "depth": 10.0 },
                    "fast": true,
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let uuid = body["object"]["id"]
            .as_str()
            .expect("object id")
            .to_string();
        let face = body["object"]["mesh"]["face_ids"][0]
            .as_u64()
            .expect("the mesh names the kernel face of each triangle");
        (uuid, face)
    }

    const ROUTE: &str = "/api/agent/user-context";

    #[tokio::test]
    async fn before_any_report_the_context_is_none_not_an_error() {
        let state = make_test_state().await;
        let (status, body) = dispatch(&state, get(ROUTE, Some("Claude"))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "none", "{body}");
        assert!(body.get("hover").is_none(), "{body}");
    }

    #[tokio::test]
    async fn a_hovered_face_reaches_the_agent_by_its_persistent_id() {
        let state = make_test_state().await;
        let (uuid, face) = a_box(&state).await;
        let (status, ack) = dispatch(
            &state,
            post(
                ROUTE,
                json!({
                    "sharing": true,
                    "workspace": "part",
                    "tool": "select",
                    "pick_filter": "face",
                    "hover": { "object_id": uuid, "kind": "face", "id": face, "point": [5.0, 5.0, 10.0] },
                    "selection": [{ "object_id": uuid, "kind": "part" }],
                    "feature": 0,
                    "camera": {
                        "position": [50.0, 50.0, 50.0],
                        "direction": [-0.577, -0.577, -0.577],
                        "up": [0.0, 0.0, 1.0],
                        "projection": "perspective",
                        "preset": "isometric"
                    },
                    "at_ms": 1700000000000.0
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{ack}");
        assert_eq!(ack["seq"], 1, "{ack}");
        assert!(
            ack["agent_look"].is_null(),
            "no agent has read it yet: {ack}"
        );

        let (status, body) = dispatch(&state, get(ROUTE, Some("Claude"))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "live", "{body}");
        assert_eq!(body["seq"], 1, "{body}");
        assert_eq!(body["tool"], "select", "{body}");
        assert_eq!(body["pick_filter"], "face", "{body}");
        assert_eq!(body["feature_anchor"], 0, "{body}");
        assert_eq!(body["camera"]["preset"], "isometric", "{body}");

        let hover = &body["hover"];
        assert_eq!(hover["resolved"], true, "{body}");
        assert_eq!(hover["kind"], "face", "{body}");
        assert_eq!(hover["id"], face, "{body}");
        assert_eq!(
            hover["surface_type"], "plane",
            "a box face is a plane: {body}"
        );
        let pid = hover["persistent_id"]
            .as_str()
            .expect("a box face carries a persistent id");
        assert_eq!(pid.len(), 32, "{body}");
        assert!(pid.chars().all(|c| c.is_ascii_hexdigit()), "{body}");
        let solid = hover["solid_id"].as_u64().expect("the host part");

        let selected = &body["selection"][0];
        assert_eq!(selected["kind"], "part", "{body}");
        assert_eq!(selected["resolved"], true, "{body}");
        assert_eq!(selected["solid_id"], solid, "{body}");
    }

    #[tokio::test]
    async fn an_entity_that_does_not_resolve_is_said_so_never_retargeted() {
        let state = make_test_state().await;
        let (uuid, _) = a_box(&state).await;
        let gone = uuid::Uuid::new_v4().to_string();
        let (status, _) = dispatch(
            &state,
            post(
                ROUTE,
                json!({
                    "sharing": true,
                    "hover": { "object_id": uuid, "kind": "face", "id": 987654 },
                    "selection": [
                        { "object_id": gone, "kind": "part" },
                        { "object_id": "asm-comp:7", "kind": "part" },
                        { "object_id": uuid, "kind": "edge", "point": [0.0, 0.0, 0.0] }
                    ]
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = dispatch(&state, get(ROUTE, None)).await;
        let hover = &body["hover"];
        assert_eq!(hover["resolved"], false, "{body}");
        assert_eq!(
            hover["id"], 987654,
            "the id the app sent is echoed, not replaced: {body}"
        );
        assert!(hover.get("persistent_id").is_none(), "{body}");
        assert!(
            hover["reason"]
                .as_str()
                .is_some_and(|r| r.contains("987654")),
            "{body}"
        );
        for (i, word) in [
            (0, "not in the model"),
            (1, "not a part"),
            (2, "no kernel id"),
        ] {
            let e = &body["selection"][i];
            assert_eq!(e["resolved"], false, "{body}");
            assert!(
                e["reason"].as_str().is_some_and(|r| r.contains(word)),
                "selection[{i}] says why ({word}): {body}"
            );
        }
        assert!(
            body["selection"][2]["solid_id"].as_u64().is_some(),
            "the part of an unidentified edge is still named: {body}"
        );
    }

    #[tokio::test]
    async fn a_face_of_another_part_is_not_accepted_as_this_parts() {
        let state = make_test_state().await;
        let (first, first_face) = a_box(&state).await;
        let (second, _) = a_box(&state).await;
        assert_ne!(first, second);
        dispatch(
            &state,
            post(
                ROUTE,
                json!({
                    "sharing": true,
                    "hover": { "object_id": second, "kind": "face", "id": first_face }
                }),
            ),
        )
        .await;
        let (_, body) = dispatch(&state, get(ROUTE, None)).await;
        assert_eq!(body["hover"]["resolved"], false, "{body}");
        assert!(
            body["hover"]["reason"]
                .as_str()
                .is_some_and(|r| r.contains("not a face of this part")),
            "{body}"
        );
    }

    #[tokio::test]
    async fn pausing_withholds_everything_from_the_agent() {
        let state = make_test_state().await;
        let (uuid, face) = a_box(&state).await;
        let (status, _) = dispatch(
            &state,
            post(
                ROUTE,
                json!({
                    "sharing": false,
                    "hover": { "object_id": uuid, "kind": "face", "id": face },
                    "selection": [{ "object_id": uuid, "kind": "part" }]
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = dispatch(&state, get(ROUTE, Some("Claude"))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "paused", "{body}");
        assert!(body.get("hover").is_none(), "{body}");
        assert!(body.get("selection").is_none(), "{body}");
        assert!(
            !body.to_string().contains(&uuid),
            "nothing of the report leaks: {body}"
        );
    }

    #[tokio::test]
    async fn only_an_agents_read_counts_as_the_agent_looking() {
        let state = make_test_state().await;
        let report = json!({ "sharing": true, "tool": "select" });
        dispatch(&state, post(ROUTE, report.clone())).await;
        // The app reading it back is not an agent looking.
        dispatch(&state, get(ROUTE, None)).await;
        let (_, ack) = dispatch(&state, post(ROUTE, report.clone())).await;
        assert!(ack["agent_look"].is_null(), "{ack}");

        let mut frames = crate::geometry_broadcaster().subscribe();
        dispatch(&state, get(ROUTE, Some("Claude"))).await;
        let (_, ack) = dispatch(&state, post(ROUTE, report)).await;
        assert_eq!(ack["agent_look"]["agent"], "Claude", "{ack}");
        assert_eq!(
            ack["agent_look"]["seq"], 2,
            "it read the second report: {ack}"
        );
        assert_eq!(ack["seq"], 3, "{ack}");

        // The open apps were told, on the live channel.
        let mut told = false;
        loop {
            let text = match frames.try_recv() {
                Ok(text) => text,
                // Other tests share the channel; frames dropped for being
                // too many are theirs to miss, not the end of the channel.
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let frame: Value = serde_json::from_str(&text).expect("frames are JSON");
            if frame["type"] == "AgentLooked" && frame["payload"]["seq"] == 2 {
                assert_eq!(frame["payload"]["agent"], "Claude", "{frame}");
                assert_eq!(frame["payload"]["saw"], "live", "{frame}");
                told = true;
            }
        }
        assert!(told, "an AgentLooked frame was broadcast");
    }

    #[tokio::test]
    async fn a_malformed_report_is_refused_with_a_typed_error() {
        let state = make_test_state().await;
        let (status, body) = dispatch(
            &state,
            post(ROUTE, json!({ "sharing": true, "mouse": [1, 2] })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error_code"], "invalid_parameter", "{body}");
        let (_, after) = dispatch(&state, get(ROUTE, None)).await;
        assert_eq!(
            after["state"], "none",
            "a refused report is not kept: {after}"
        );
    }

    /// The production call site: the agent's tool is in the registry the
    /// kernel serves, so an MCP client can find it.
    #[tokio::test]
    async fn the_user_context_tool_is_in_the_served_registry() {
        let state = make_test_state().await;
        let (status, body) = dispatch(&state, get("/api/agent/tool-registry", None)).await;
        assert_eq!(status, StatusCode::OK);
        let tools = body["tools"].as_array().expect("tools");
        assert!(
            tools.iter().any(|t| t["name"] == "user_context"),
            "user_context is registered"
        );
    }
}
