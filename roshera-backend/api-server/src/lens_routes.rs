//! Kernel Lens routes: what the kernel decided about a part, per face.
//!
//! - `GET /api/lens/{part}` returns `{ solid_id, sound, overview, faces: [{ face_id,
//!   pid, status }], solid_events }`.
//! - `GET /api/lens/{part}/face/{face_id}` returns `{ face_id, pid, status, events }`.
//!
//! `{part}` is the object UUID the scene uses, or the kernel's numeric solid
//! id. Two sources feed a part's journal:
//!
//! - **What the importer decided** (a bound walked reversed, an edge gap
//!   repaired, ...). The importer records these as it decides. They are kept
//!   in [`LensStore`] by face persistent id, which survives the id remap when
//!   an import is merged into the document.
//! - **What blocks the certificate.** This is read from the part's current
//!   certificate on every request, so it is never stale.
//!
//! Import events live for the server session. A part re-opened after a
//! restart shows its certificate's blocking faces but not the import story.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use dashmap::DashMap;
use geometry_engine::lens::{EntityRef, EventKind, FaceStatus, LensEvent, LensJournal, Stage};
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::part_mgr::ActiveModel;
use crate::AppState;

/// Import-time Lens events by the persistent id of the entity they concern.
#[derive(Clone, Default)]
pub struct LensStore {
    by_pid: Arc<DashMap<u128, Vec<LensEvent>>>,
}

impl LensStore {
    /// Keep every event of an import's journal.
    pub fn record(&self, journal: &LensJournal) {
        for e in journal.events() {
            let pid = match e.entity {
                EntityRef::Face(p) | EntityRef::Solid(p) => p.0,
            };
            self.by_pid.entry(pid).or_default().push(e.clone());
        }
    }

    fn events_for(&self, pid: PersistentId) -> Vec<LensEvent> {
        self.by_pid
            .get(&pid.0)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }
}

/// The one-line rule a failing certificate check stands for.
fn blocking_why(conjunct: &str) -> &'static str {
    match conjunct {
        "watertight" => "the mesh of this face has open edges: the solid is not closed here",
        "manifold" => "a mesh edge here is shared by three or more facets",
        "oriented" => "facets here wind the same way as a neighbour across a shared edge",
        "shells_outward" => "a shell encloses volume of the wrong sign for its role",
        "mesh_quality" => {
            "a facet here crosses the face boundary or folds off the surface by more than its chord"
        }
        "tessellation" => "the display mesh here is degenerate or points away from the surface",
        "brep_valid" => "the B-Rep topology fails validation",
        "self_intersection_free" => "the solid's surface intersects itself",
        "construction_consistent" => "the solid no longer matches the sketch it was built from",
        "eyes_consistent" => "a recognised feature refers to a face the solid no longer has",
        _ => "a certificate check fails",
    }
}

fn hex(pid: PersistentId) -> String {
    format!("{:032x}", pid.0)
}

fn status_name(s: FaceStatus) -> &'static str {
    match s {
        FaceStatus::AsWritten => "as_written",
        FaceStatus::Repaired => "repaired",
        FaceStatus::Blocking => "blocking",
    }
}

fn event_json(e: &LensEvent) -> Value {
    let mut v = serde_json::to_value(e).unwrap_or(Value::Null);
    if let Value::Object(m) = &mut v {
        m.remove("entity");
    }
    v
}

/// A part's journal and its faces, under one write lock (the certificate is
/// cached after import; certifying needs `&mut`).
struct PartLens {
    solid_id: u32,
    sound: bool,
    journal: LensJournal,
    /// Every face of the part, with its persistent id when it has one.
    faces: Vec<(u32, Option<PersistentId>)>,
    /// Checks the certificate fails that no single face can be blamed for.
    solid_blocking: Vec<LensEvent>,
}

fn resolve_solid(state: &AppState, model: &BRepModel, part: &str) -> Option<u32> {
    let id = match uuid::Uuid::parse_str(part) {
        Ok(u) => state.get_local_id(&u)?,
        Err(_) => part.parse::<u32>().ok()?,
    };
    model.solids.get(id).map(|_| id)
}

fn part_lens(state: &AppState, model: &mut BRepModel, solid_id: u32) -> PartLens {
    let shells = model
        .solids
        .get(solid_id)
        .map(|s| s.all_shells())
        .unwrap_or_default();
    let mut faces: Vec<(u32, Option<PersistentId>)> = Vec::new();
    for sh in shells {
        if let Some(shell) = model.shells.get(sh) {
            for &f in &shell.faces {
                faces.push((f, None));
            }
        }
    }
    for (f, pid) in faces.iter_mut() {
        *pid = model.face_pid(*f);
    }
    let mut journal = LensJournal::default();
    for (_, pid) in &faces {
        if let Some(p) = pid {
            for e in state.lens.events_for(*p) {
                journal.push(e);
            }
        }
    }
    let solid_pid = model.solid_pid(solid_id);
    if let Some(p) = solid_pid {
        for e in state.lens.events_for(p) {
            journal.push(e);
        }
    }
    let cert = model.certify_solid(solid_id);
    let mut solid_blocking = Vec::new();
    for b in &cert.blocking {
        let pid = b
            .face
            .and_then(|f| faces.iter().find(|(g, _)| *g == f))
            .and_then(|(_, p)| *p);
        let event = |entity| LensEvent {
            entity,
            stage: Stage::Certify,
            kind: EventKind::Blocking {
                conjunct: b.conjunct.clone(),
            },
            step_instance: None,
            why: blocking_why(&b.conjunct).to_string(),
            measured: b.measured.clone(),
        };
        match pid {
            Some(p) => journal.push(event(EntityRef::Face(p))),
            // A solid-level check, or a face with no persistent id: kept on
            // the solid so nothing blocking goes unshown.
            None => {
                let mut e = event(EntityRef::Solid(solid_pid.unwrap_or(PersistentId(0))));
                if let Some(f) = b.face {
                    e.measured.push(("face_id".into(), f as f64));
                }
                solid_blocking.push(e);
            }
        }
    }
    PartLens {
        solid_id,
        sound: cert.is_sound(),
        journal,
        faces,
        solid_blocking,
    }
}

/// `GET /api/lens/{part}`: the part's overview, every face's status, and
/// the events on the solid itself.
pub async fn get_part_lens(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(part): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let mut model = model_handle.write().await;
    let solid_id = resolve_solid(&state, &model, &part).ok_or(StatusCode::NOT_FOUND)?;
    let lens = part_lens(&state, &mut model, solid_id);
    drop(model);
    let mut overview = lens.journal.overview();
    for e in &lens.solid_blocking {
        let name = e.kind.name();
        match overview.iter_mut().find(|(k, _)| k == name) {
            Some((_, n)) => *n += 1,
            None => overview.push((name.to_string(), 1)),
        }
    }
    let solid_events: Vec<Value> = lens
        .journal
        .events()
        .iter()
        .filter(|e| matches!(e.entity, EntityRef::Solid(_)))
        .chain(lens.solid_blocking.iter())
        .map(event_json)
        .collect();
    Ok(Json(json!({
        "solid_id": lens.solid_id,
        "sound": lens.sound,
        "overview": overview
            .iter()
            .map(|(k, n)| json!({ "kind": k, "count": n }))
            .collect::<Vec<_>>(),
        "faces": lens
            .faces
            .iter()
            .map(|(f, pid)| json!({
                "face_id": f,
                "pid": pid.map(hex),
                "status": status_name(
                    pid.map(|p| lens.journal.status(p)).unwrap_or(FaceStatus::AsWritten)
                ),
            }))
            .collect::<Vec<_>>(),
        "solid_events": solid_events,
    })))
}

/// `GET /api/lens/{part}/face/{face_id}`: one face's story.
pub async fn get_face_lens(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((part, face_id)): Path<(String, u32)>,
) -> Result<Json<Value>, StatusCode> {
    let mut model = model_handle.write().await;
    let solid_id = resolve_solid(&state, &model, &part).ok_or(StatusCode::NOT_FOUND)?;
    let lens = part_lens(&state, &mut model, solid_id);
    drop(model);
    let (_, pid) = lens
        .faces
        .iter()
        .find(|(f, _)| *f == face_id)
        .copied()
        .ok_or(StatusCode::NOT_FOUND)?;
    let events: Vec<Value> = match pid {
        Some(p) => lens.journal.for_face(p).map(event_json).collect(),
        None => lens
            .solid_blocking
            .iter()
            .filter(|e| {
                e.measured
                    .iter()
                    .any(|(k, v)| k == "face_id" && *v == face_id as f64)
            })
            .map(event_json)
            .collect(),
    };
    let status = match pid {
        Some(p) => lens.journal.status(p),
        None if !events.is_empty() => FaceStatus::Blocking,
        None => FaceStatus::AsWritten,
    };
    Ok(Json(json!({
        "solid_id": lens.solid_id,
        "face_id": face_id,
        "pid": pid.map(hex),
        "status": status_name(status),
        "events": events,
    })))
}
