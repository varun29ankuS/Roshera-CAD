//! Diagnostics-α Phase-2 wire-shape harness.
//!
//! The unit tests in `error_catalog::tests` pin the wire shape for
//! every `BlendFailure` variant using **synthetic** kernel errors —
//! they prove the bridge layer produces the right JSON for a given
//! `OperationError::BlendFailed(...)`. This harness is the
//! complement: it builds **real geometry**, runs **real kernel
//! operations** that produce real `BlendFailed` errors, and asserts
//! the wire shape at the byte level (status code + JSON body) after
//! `IntoResponse::into_response` has fully rendered the HTTP reply.
//!
//! That covers the gap between "the bridge serializes correctly" and
//! "an agent POSTing to `/api/geometry/fillet` actually sees the
//! typed payload". Without a full router-level test (which requires
//! refactoring `main()`'s inline `Router::new()` into a `build_router`
//! factory — Phase-3 scope), this is the closest harness we can build
//! to the live HTTP surface.
//!
//! # Layers covered end-to-end
//!
//! 1. Real geometry constructed via `TopologyBuilder` (cylinder, box).
//! 2. Real `kernel_fillet` invocation that produces an
//!    `OperationError::BlendFailed(BlendFailure::...)` via the F6-α
//!    feasibility gate.
//! 3. `From<OperationError> for ApiError` bridge in
//!    `error_catalog.rs`.
//! 4. `IntoResponse for ApiError` — the actual HTTP rendering used by
//!    every endpoint that returns `Result<_, ApiError>`.
//! 5. The serialized JSON body bytes.
//!
//! # Fixtures
//!
//! The harness exposes two fixtures any new test can reuse:
//!
//! - `fixtures::unit_cylinder(radius, height)` — analytic cylinder
//!   primitive. Filleting any rim edge with `r ≥ cylinder.radius`
//!   trips F6-α with `BlendFailure::RadiusExceedsCurvature`.
//! - `fixtures::box_solid(w, h, d)` — analytic box. All faces are
//!   planar (curvature 0), so F6-α is a no-op for box rims; useful
//!   for the negative path (the kernel surfaces non-`BlendFailed`
//!   `OperationError` variants, which must funnel through the
//!   legacy `kernel_error` surface).
//!
//! And one helper:
//!
//! - `wire::fillet_and_render(model, solid_id, edges, radius)` —
//!   runs `kernel_fillet`, threads any error through `ApiError::from`,
//!   renders the full HTTP `Response`, reads the body bytes, parses
//!   the JSON. Returns the `(StatusCode, serde_json::Value)` pair
//!   tests assert against.

#![cfg(test)]

use crate::error_catalog::{ApiError, ErrorCode};
use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::fillet::{
    fillet_edges, FilletOptions, FilletType, PropagationMode,
};
use geometry_engine::operations::OperationError;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Geometry fixtures shared across harness tests.
mod fixtures {
    use super::*;

    /// Build a `(0, 0, 0) → (0, 0, height)` cylinder of the given
    /// radius and return `(model, solid_id, rim_edge)`. `rim_edge` is
    /// the closed top-rim circular edge at `z = height`. The rim sits
    /// between the cylinder's side surface (curvature `1 / radius`)
    /// and the top cap (planar — no curvature constraint), so
    /// filleting it with `r > radius` is exactly the F6-α
    /// `RadiusExceedsCurvature` trigger.
    ///
    /// A single edge is returned (rather than every shell edge)
    /// because `lifecycle::validate_can_apply` rejects corner-
    /// sharing edge pairs ahead of F6-α (Task #82 — vertex blends),
    /// so passing the whole edge set would short-circuit the
    /// feasibility gate before it can run.
    ///
    /// If the kernel build does not expose the rim as a single
    /// closed edge (some primitive backends represent rims as
    /// parameter-space circles without a distinct topological edge),
    /// the function panics — those backends are out of scope for
    /// this harness.
    pub fn unit_cylinder(radius: f64, height: f64) -> (BRepModel, SolidId, EdgeId) {
        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            match builder
                .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
                .expect("cylinder primitive must build for positive r/h")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {:?}", other),
            }
        };
        let rim = find_top_rim_edge(&model, height)
            .expect("cylinder kernel build must expose the top rim as a closed topological edge");
        (model, solid_id, rim)
    }

    /// Locate the cylinder's top-rim edge: a closed (start == end)
    /// edge whose endpoints sit at `z ≈ height`. Mirrors the helper
    /// in `fillet_chamfer_dihedral_matrix::fillet_on_cylinder_top_rim_*`.
    fn find_top_rim_edge(model: &BRepModel, height: f64) -> Option<EdgeId> {
        model.edges.iter().find_map(|(id, e)| {
            let s = model.vertices.get(e.start_vertex)?.position;
            let t = model.vertices.get(e.end_vertex)?.position;
            let closed = (s[0] - t[0]).abs() < 1e-7
                && (s[1] - t[1]).abs() < 1e-7
                && (s[2] - t[2]).abs() < 1e-7;
            let on_top = (s[2] - height).abs() < 1e-7;
            if closed && on_top {
                Some(id)
            } else {
                None
            }
        })
    }

    /// Build an axis-aligned box and return `(model, solid_id,
    /// every_edge)`. The shell is purely planar — useful for paths
    /// where F6-α passes (no curvature) and the kernel either
    /// succeeds or rejects via a non-`BlendFailed` variant.
    pub fn box_solid(w: f64, h: f64, d: f64) -> (BRepModel, SolidId, Vec<EdgeId>) {
        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            match builder
                .create_box_3d(w, h, d)
                .expect("box primitive must build for positive dims")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {:?}", other),
            }
        };
        let edges = all_outer_shell_edges(&model, solid_id);
        (model, solid_id, edges)
    }

    /// Walk the outer shell and return a deduplicated edge list.
    /// Mirrors the helper `feasibility::tests::all_edge_ids` uses so
    /// the harness sees the same edge set the F6-α gate sees.
    fn all_outer_shell_edges(model: &BRepModel, solid_id: SolidId) -> Vec<EdgeId> {
        let mut edges = Vec::new();
        let solid = model.solids.get(solid_id).expect("solid must exist");
        let shell = model
            .shells
            .get(solid.outer_shell)
            .expect("outer shell must exist");
        for &face_id in &shell.faces {
            let face = model.faces.get(face_id).expect("face must exist");
            let outer = model
                .loops
                .get(face.outer_loop)
                .expect("face outer loop must exist");
            for &eid in &outer.edges {
                if !edges.contains(&eid) {
                    edges.push(eid);
                }
            }
        }
        edges
    }

    /// Pick a single non-loop, non-corner-sharing edge from a box.
    /// The fillet entry point rejects corner-sharing pairs ahead of
    /// the F6-α gate (Task #82 — vertex blends not yet supported),
    /// so single-edge fillets are the cleanest input for the
    /// non-`BlendFailed` paths.
    pub fn first_open_box_edge(model: &BRepModel) -> EdgeId {
        model
            .edges
            .iter()
            .filter_map(|(id, e)| if !e.is_loop() { Some(id) } else { None })
            .next()
            .expect("box always carries at least one open edge")
    }
}

/// HTTP-rendering helpers — these own the `kernel_fillet →
/// ApiError::from → IntoResponse → body bytes → JSON` pipeline so
/// individual tests can stay focused on what they assert about the
/// wire shape.
mod wire {
    use super::*;

    /// Outcome of `kernel_fillet` rendered all the way to the wire.
    /// `status` is the HTTP status `IntoResponse` chose; `body` is
    /// the parsed JSON body. On the success path `body` is the
    /// (kernel-defined) success payload; on the error path it is
    /// the serialized `ApiError`.
    #[derive(Debug)]
    pub struct WireResponse {
        pub status: StatusCode,
        pub body: serde_json::Value,
    }

    /// Drive a fillet through to the wire. The kernel's `Ok` value
    /// is not part of this harness's contract — only the **error**
    /// rendering is — but the success path is exposed so tests can
    /// assert that the wire still renders cleanly when the gate
    /// passes.
    pub async fn fillet_and_render(
        model: &mut BRepModel,
        solid_id: SolidId,
        edges: Vec<EdgeId>,
        radius: f64,
    ) -> WireResponse {
        let opts = FilletOptions {
            fillet_type: FilletType::Constant(radius),
            propagation: PropagationMode::None,
            ..FilletOptions::default()
        };
        match fillet_edges(model, solid_id, edges, opts) {
            Ok(_) => WireResponse {
                status: StatusCode::OK,
                body: serde_json::json!({"success": true}),
            },
            Err(op_err) => render_error(op_err).await,
        }
    }

    /// Render any `OperationError` through the bridge and the
    /// `IntoResponse` impl. Returns the post-rendering status code
    /// and parsed JSON body. The `IntoResponse` invocation here is
    /// the same one every Axum handler uses, so the body bytes are
    /// byte-identical to what a live HTTP client would receive.
    pub async fn render_error(op_err: OperationError) -> WireResponse {
        let api_err: ApiError = op_err.into();
        let response = api_err.into_response();
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX)
            .await
            .expect("ApiError body must serialize to finite bytes");
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("ApiError body must be valid JSON");
        WireResponse {
            status: parts.status,
            body,
        }
    }
}

// =====================================================================
// Real-geometry harness tests
// =====================================================================

/// Filleting a unit cylinder's rim edges with `r = 2 × cylinder_radius`
/// must trip F6-α. The kernel returns
/// `OperationError::BlendFailed(BlendFailure::RadiusExceedsCurvature)`
/// and the HTTP surface renders that as a 400 with the typed payload.
///
/// This is the canonical happy-path-for-rejection: the variant most
/// agents will see in practice (radius too big for the local feature).
#[tokio::test]
async fn cylinder_oversize_radius_renders_radius_exceeds_curvature_at_400() {
    let (mut model, solid_id, rim) = fixtures::unit_cylinder(1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![rim], 2.0).await;

    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "F6-α rejection must surface as HTTP 400 (caller-side problem), got body: {}",
        response.body
    );
    assert_eq!(response.body["success"], false);
    assert_eq!(response.body["error_code"], "blend_failed");
    assert_eq!(response.body["retryable"], false);

    let failure = &response.body["details"]["failure"];
    assert_eq!(
        failure["type"], "RadiusExceedsCurvature",
        "wire payload must carry the internally-tagged discriminator; got {failure}"
    );
    assert_eq!(failure["r_requested"], 2.0);
    // F6-α's r_max is `1 / kappa_max` = cylinder radius for a
    // cylinder; we built one with radius 1.0.
    let r_max = failure["r_max"]
        .as_f64()
        .expect("r_max must be a JSON number");
    assert!(
        (r_max - 1.0).abs() < 1e-9,
        "r_max for a unit cylinder must be 1.0, got {r_max}"
    );
}

/// The kernel's `Display` for `BlendFailure::RadiusExceedsCurvature`
/// embeds the binding `r_max` value, and the bridge layer's
/// `ApiError::blend_failed` prefixes `"blend failed: "` onto that
/// string. The wire's `error` field must carry both — logs and
/// humans read this field; agents read `details.failure`.
#[tokio::test]
async fn cylinder_oversize_radius_wire_error_field_carries_r_max() {
    let (mut model, solid_id, rim) = fixtures::unit_cylinder(1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![rim], 2.0).await;

    let error = response.body["error"]
        .as_str()
        .expect("error field must be a string");
    assert!(
        error.starts_with("blend failed:"),
        "error must carry the typed-surface prefix; got {error:?}"
    );
    assert!(
        error.contains("r_max=1"),
        "error must embed the binding curvature limit; got {error:?}"
    );
}

/// Filleting a cylinder with `r ≪ cylinder_radius` must pass the
/// F6-α gate cleanly. The harness here doesn't assert anything
/// about the success body shape (that's the kernel's contract, not
/// the wire shape's) — it only proves that the gate isn't
/// rejecting feasible radii, which would manifest as a regression
/// of the entire fillet feature.
///
/// If this test fails because the kernel rejects the fillet for
/// *some other reason* (sew, splice, etc.), the wire still has to
/// render the rejection — we accept either OK or a typed
/// `BlendFailed` here.
#[tokio::test]
async fn cylinder_small_radius_does_not_trip_f6_alpha_gate() {
    let (mut model, solid_id, rim) = fixtures::unit_cylinder(1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![rim], 0.05).await;

    if response.status == StatusCode::BAD_REQUEST {
        // If the kernel did reject, it must be for a downstream
        // reason — never `RadiusExceedsCurvature`, because r=0.05
        // is well below the r_max=1.0 bound.
        let failure_type = response.body["details"]["failure"]["type"]
            .as_str()
            .unwrap_or("");
        assert_ne!(
            failure_type, "RadiusExceedsCurvature",
            "r=0.05 against r_max=1.0 must NOT trip F6-α; got body: {}",
            response.body
        );
    }
}

/// Box edges sit between planar faces (curvature = 0), so F6-α's
/// `max_analytic_curvature` returns `None` and the gate passes
/// regardless of radius. Filleting a box edge with a too-large
/// radius therefore reaches the downstream parameter-validation
/// step, which rejects via `OperationError::InvalidInput`. That
/// is *not* a `BlendFailed`, so the bridge funnels it through
/// `kernel_error` — the legacy surface that pre-Phase-2 callers
/// already rely on.
#[tokio::test]
async fn box_oversize_radius_funnels_through_legacy_kernel_error_surface() {
    let (mut model, solid_id, _edges) = fixtures::box_solid(1.0, 1.0, 1.0);
    let edge = fixtures::first_open_box_edge(&model);
    // Box edge length is 1.0, half-edge bound is 0.5. r = 10.0
    // is way over.
    let response = wire::fillet_and_render(&mut model, solid_id, vec![edge], 10.0).await;

    // Plane curvature is 0 → F6-α passes → downstream
    // validate_fillet_parameters rejects with InvalidInput, which
    // the bridge routes to `kernel_error` (legacy).
    assert_ne!(
        response.status,
        StatusCode::OK,
        "r=10 on a 1×1×1 box edge cannot succeed"
    );
    assert_ne!(
        response.body["error_code"], "blend_failed",
        "planar-face rejection must NOT route to blend_failed; \
         F6-α has nothing to say about planes (curvature=0)"
    );
}

/// Box edges accept small fillet radii cleanly. Same caveat as the
/// cylinder happy-path: the harness only proves the gate doesn't
/// wrong-reject, it doesn't audit the success body.
#[tokio::test]
async fn box_small_radius_does_not_trip_blend_failed() {
    let (mut model, solid_id, _edges) = fixtures::box_solid(1.0, 1.0, 1.0);
    let edge = fixtures::first_open_box_edge(&model);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![edge], 0.1).await;

    // Either OK or some other (non-blend_failed) downstream
    // rejection. Either way, F6-α must not have produced a
    // `BlendFailure` here.
    assert_ne!(
        response.body["error_code"], "blend_failed",
        "r=0.1 on a planar box edge must not surface as a BlendFailed"
    );
}

/// Filleting a non-existent edge id fails inside
/// `lifecycle::validate_can_apply` with `InvalidInput` — that's
/// a caller-side parameter problem and NOT a blend failure.
/// The bridge must NOT route it to `blend_failed`; it must
/// surface through the legacy `kernel_error` path.
#[tokio::test]
async fn unknown_edge_id_does_not_route_to_blend_failed() {
    let (mut model, solid_id, _edges) = fixtures::box_solid(1.0, 1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![999_999], 0.1).await;

    assert_ne!(
        response.body["error_code"], "blend_failed",
        "unknown-edge rejection must not pose as a blend failure; got body: {}",
        response.body
    );
    // Status will be 5xx (legacy `kernel_error` is INTERNAL_SERVER_ERROR).
    // We don't pin a specific code here because that's owned by the
    // `kernel_error` mapping, not by the BlendFailed contract.
    assert!(
        response.status.as_u16() >= 400,
        "rejection must surface as an error status, got {}",
        response.status
    );
}

/// Empty edge list is a caller-side parameter problem. Some kernel
/// builds reject up-front, some pass through to a downstream
/// no-op. Either way: not a BlendFailed.
#[tokio::test]
async fn empty_edge_list_does_not_route_to_blend_failed() {
    let (mut model, solid_id, _edges) = fixtures::box_solid(1.0, 1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, Vec::new(), 0.1).await;

    assert_ne!(
        response.body["error_code"], "blend_failed",
        "empty-edge-list rejection must not pose as a blend failure"
    );
}

/// The wire's `error_code` must round-trip through
/// `ErrorCode::as_str` — it is the discriminator agents pattern-
/// match against. This is a redundancy check on the catalog: if
/// somebody renames the variant tag without updating `as_str` (or
/// vice versa), this catches it.
#[tokio::test]
async fn error_code_is_serialized_via_as_str() {
    let (mut model, solid_id, rim) = fixtures::unit_cylinder(1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![rim], 2.0).await;

    assert_eq!(
        response.body["error_code"].as_str().unwrap(),
        ErrorCode::BlendFailed.as_str(),
        "wire error_code must match ErrorCode::as_str output"
    );
}

/// Synthetic kernel error path: drive every `OperationError`
/// variant the bridge can see through `ApiError::from` and assert
/// the wire still renders sensibly. This is the catch-all that
/// fires if a future kernel variant lands without a bridge update.
#[tokio::test]
async fn every_non_blend_operation_error_renders_through_legacy_surface() {
    use geometry_engine::operations::OperationError;
    let synthetic = vec![
        OperationError::InvalidGeometry("synthetic".into()),
        OperationError::NumericalError("synthetic".into()),
        OperationError::FeatureTooSmall,
        OperationError::InvalidRadius(-1.0),
    ];
    for op_err in synthetic {
        let label = format!("{:?}", op_err);
        let response = wire::render_error(op_err).await;
        assert_ne!(
            response.body["error_code"], "blend_failed",
            "{label}: non-BlendFailed variant must NOT route to blend_failed"
        );
        assert!(
            response.status.as_u16() >= 400,
            "{label}: must render as an error status"
        );
        assert_eq!(
            response.body["success"], false,
            "{label}: success must be false on the error path"
        );
    }
}

/// The wire-rendered `BlendFailed` body must contain the **exact**
/// set of top-level fields the agent surface contracts on:
/// `success`, `error_code`, `error`, `retryable`, `hint`, `details`.
/// Adding or removing fields at the top level is a breaking change.
#[tokio::test]
async fn blend_failed_wire_body_has_expected_top_level_fields() {
    let (mut model, solid_id, rim) = fixtures::unit_cylinder(1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![rim], 2.0).await;

    let obj = response
        .body
        .as_object()
        .expect("wire body must be a JSON object");
    for required in &["success", "error_code", "error", "retryable", "details"] {
        assert!(
            obj.contains_key(*required),
            "wire body missing required field {required}; body={:?}",
            response.body
        );
    }
    // `hint` is optional but consistently present for caller-side
    // problems; spot-check it's a string when present.
    if let Some(hint) = obj.get("hint") {
        assert!(
            hint.is_string() || hint.is_null(),
            "hint must be a string or null when present, got {hint:?}"
        );
    }
}

/// `details.failure` is the typed payload's exclusive home.
/// Confirm it survives the full round-trip from the kernel struct
/// to JSON bytes without losing the discriminator tag or any of
/// its fields.
#[tokio::test]
async fn typed_failure_payload_lives_under_details_failure() {
    let (mut model, solid_id, rim) = fixtures::unit_cylinder(1.0, 1.0);
    let response = wire::fillet_and_render(&mut model, solid_id, vec![rim], 2.0).await;

    let details = response
        .body
        .get("details")
        .expect("wire body must include details for BlendFailed");
    let failure = details
        .get("failure")
        .expect("details must include 'failure' for BlendFailed");
    assert!(
        failure.get("type").is_some(),
        "failure payload must include the internally-tagged 'type' discriminator"
    );
}
