//! Gate 6 — "check what you built before you close it" — enforced by the
//! server for every agent request (2026-11-04).
//!
//! The rule belongs to Roshera's agent policy (`roshera-agent-policy.md`).
//! Until now only `roshera-mcp/src/gates.ts` held it, so an agent reaching
//! REST directly could build and move on without ever looking at the result.
//!
//! # What counts
//!
//! - **Built**: a successful (2xx) agent request on any route guarded by the
//!   intent gate (`require_declared_intent` records it — the SAME route set,
//!   so the two rules can never disagree about what "changing geometry" is).
//! - **Looked**: a full certificate read (`GET /api/agent/parts/{id}/
//!   perception?full=1`, what `verify_part` calls) or a claim that measured
//!   geometry (`POST /api/agent/verify-claim` with at least one non-constant
//!   binding and a verdict that is not `refused`). The cheap ambient
//!   perception read skips the expensive checks, so it is not a look.
//! - **Closing**: `POST /api/timeline/checkpoint` — opening the next intent
//!   closes the current one. Refused while unchecked work stands, unless the
//!   body carries `skip_verification: true` (the explicit, recorded escape).
//! - **Settled without a look**: a successful checkpoint, `POST
//!   /api/timeline/clear` (the ledger the work lived in is gone) and `DELETE
//!   /api/agent/parts` (no part is left for a verification to measure) —
//!   the same three cases the MCP gate settles.
//!
//! # Whose work
//!
//! State is kept per agent session: `X-Roshera-Agent-Session` (one id per
//! `roshera-mcp` process), falling back to the agent's own name when a client
//! sends no session id. One agent's unchecked work never blocks another's
//! checkpoint. Requests without `X-Roshera-Agent` are people using the app
//! and are never asked.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;

use crate::error_catalog::ApiError;

/// Largest request/response body this layer will buffer to read a checkpoint
/// or claim. Both are small JSON documents; anything larger is passed through
/// unread and treated as "not a look" — never as one.
const MAX_INSPECTED_BODY: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
struct Unverified {
    count: usize,
    /// What was built, in first-seen order, each named once (`METHOD path`).
    built: Vec<String>,
}

/// Per-agent-session tally of geometry changes not yet verified.
#[derive(Debug, Default)]
pub struct VerificationLedger {
    by_session: DashMap<String, Unverified>,
}

impl VerificationLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// A successful geometry change by the agent session `key`.
    pub fn record_built(&self, key: &str, what: String) {
        let mut entry = self.by_session.entry(key.to_string()).or_default();
        entry.count += 1;
        if !entry.built.contains(&what) {
            entry.built.push(what);
        }
    }

    /// The session looked, closed, or wiped its work — nothing left to check.
    pub fn settle(&self, key: &str) {
        self.by_session.remove(key);
    }

    fn pending(&self, key: &str) -> Option<Unverified> {
        self.by_session
            .get(key)
            .map(|e| e.clone())
            .filter(|u| u.count > 0)
    }
}

/// The agent session a request belongs to, or `None` for a person.
pub fn agent_session_key(headers: &HeaderMap) -> Option<String> {
    let agent = headers
        .get("x-roshera-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let session = headers
        .get("x-roshera-agent-session")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Some(match session {
        Some(s) => format!("session:{s}"),
        None => format!("agent:{agent}"),
    })
}

/// What this layer needs to know about a request.
enum Kind {
    Checkpoint,
    FullCertificateRead,
    ClaimCheck,
    Wipe,
    Other,
}

fn classify(method: &Method, path: &str, query: Option<&str>) -> Kind {
    match (method, path) {
        (&Method::POST, "/api/timeline/checkpoint") => Kind::Checkpoint,
        (&Method::POST, "/api/agent/verify-claim") => Kind::ClaimCheck,
        (&Method::POST, "/api/timeline/clear") | (&Method::DELETE, "/api/agent/parts") => {
            Kind::Wipe
        }
        (&Method::GET, p) if is_perception_path(p) && is_full(query) => Kind::FullCertificateRead,
        _ => Kind::Other,
    }
}

/// `/api/agent/parts/{id}/perception`
fn is_perception_path(path: &str) -> bool {
    let mut parts = path.trim_end_matches('/').split('/');
    matches!(
        (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next()
        ),
        (Some(""), Some("api"), Some("agent"), Some("parts"), Some(id), Some("perception"), None)
            if !id.is_empty()
    )
}

fn is_full(query: Option<&str>) -> bool {
    query.is_some_and(|q| {
        q.split('&')
            .any(|pair| matches!(pair, "full=1" | "full=true" | "full"))
    })
}

async fn buffer(body: Body) -> Option<axum::body::Bytes> {
    axum::body::to_bytes(body, MAX_INSPECTED_BODY).await.ok()
}

/// The global layer. Recording what was BUILT happens in
/// `require_declared_intent` (it already sits on exactly the geometry-changing
/// routes); this layer handles looking, closing and wiping.
pub async fn verification_scope_layer(
    State(ledger): State<Arc<VerificationLedger>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(key) = agent_session_key(request.headers()) else {
        return next.run(request).await;
    };
    let kind = classify(
        request.method(),
        request.uri().path(),
        request.uri().query(),
    );
    match kind {
        Kind::Other => next.run(request).await,
        Kind::FullCertificateRead | Kind::Wipe => {
            let response = next.run(request).await;
            if response.status().is_success() {
                ledger.settle(&key);
            }
            response
        }
        Kind::Checkpoint => {
            let (parts, body) = request.into_parts();
            let Some(bytes) = buffer(body).await else {
                // Unreadable body: let the handler reject it as it would.
                return next.run(Request::from_parts(parts, Body::empty())).await;
            };
            let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            let skip = parsed["skip_verification"].as_bool() == Some(true);
            if let Some(pending) = ledger.pending(&key) {
                if !skip {
                    let next_name = parsed["name"].as_str().unwrap_or("");
                    return ApiError::verification_required(
                        &pending.built,
                        pending.count,
                        next_name,
                    )
                    .into_response();
                }
            }
            let response = next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
            if response.status().is_success() {
                ledger.settle(&key);
            }
            response
        }
        Kind::ClaimCheck => {
            let (parts, body) = request.into_parts();
            let Some(bytes) = buffer(body).await else {
                return next.run(Request::from_parts(parts, Body::empty())).await;
            };
            let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            let measures_geometry = parsed["bindings"].as_array().is_some_and(|bindings| {
                bindings
                    .iter()
                    .any(|b| b["measure"]["kind"].as_str() != Some("constant"))
            });
            let response = next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
            if !(measures_geometry && response.status().is_success()) {
                return response;
            }
            let (resp_parts, resp_body) = response.into_parts();
            let Some(resp_bytes) = buffer(resp_body).await else {
                return Response::from_parts(resp_parts, Body::empty());
            };
            let verdict: serde_json::Value =
                serde_json::from_slice(&resp_bytes).unwrap_or_default();
            if verdict.is_object() && verdict["refused"].as_bool() != Some(true) {
                ledger.settle(&key);
            }
            Response::from_parts(resp_parts, Body::from(resp_bytes))
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn perception_path_and_full_flag() {
        assert!(is_perception_path("/api/agent/parts/12/perception"));
        assert!(!is_perception_path("/api/agent/parts/12/perception/extra"));
        assert!(!is_perception_path("/api/agent/parts//perception"));
        assert!(!is_perception_path("/api/agent/parts/12/render"));
        assert!(is_full(Some("full=1")));
        assert!(is_full(Some("view=iso&full=true")));
        assert!(!is_full(Some("full=0")));
        assert!(!is_full(None));
    }

    #[test]
    fn session_key_prefers_the_session_header_and_ignores_people() {
        let mut h = HeaderMap::new();
        assert_eq!(agent_session_key(&h), None, "no agent header = a person");
        h.insert("x-roshera-agent", "model-a".parse().expect("header"));
        assert_eq!(agent_session_key(&h).as_deref(), Some("agent:model-a"));
        h.insert("x-roshera-agent-session", "abc".parse().expect("header"));
        assert_eq!(agent_session_key(&h).as_deref(), Some("session:abc"));
    }
}
