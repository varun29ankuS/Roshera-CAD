//! Coverage for the six geometry-mutation routes that, until now, had zero
//! test coverage anywhere in the suite: `shell`, `mirror`, `extrude`, `cone`,
//! `nurbs_loft`, and `face/extrude`. Every test asserts the GEOMETRY the
//! route actually produced (volume / position / soundness / closure), never
//! a bare HTTP 200 — a status-only test "manufactures confidence while
//! catching nothing" (see the task brief this file answers).
//!
//! Fixtures: the cheap in-memory `router_integration_tests::make_test_state`
//! for the six geometry tests; the file-backed
//! `durability_boot_tests::{temp_db_path, open_db, build_state}` fixture
//! (plus its `dispatch`/`post` request helpers, reused directly rather than
//! re-derived) for the two quarantine-disclosure cases, whose recipe is
//! copied from
//! `durability_boot_tests.rs::unknown_event_quarantines_and_serves_clean_prefix`
//! verbatim: box create -> flush -> inject an unreplayable
//! `Operation::Generic` event -> reboot with replay.

#![cfg(test)]

use crate::durability_boot_tests::{build_state, dispatch, open_db, post, temp_db_path};
use crate::router_integration_tests::make_test_state;
use crate::{durability, AppState};

use axum::http::StatusCode;
use geometry_engine::math::{ApproxEq, Point3, Tolerance, Vector3, NORMAL_TOLERANCE};
use serde_json::json;
use session_manager::{DatabasePersistence, TimelineEventData};
use uuid::Uuid;

// =====================================================================
// Shared helpers
// =====================================================================

/// Relative-tolerance float comparison for mesh-based mass-properties
/// volumes (never bit-exact against a closed-form formula) -- the same idiom
/// `geometry-engine/tests/shell_volume_invariants.rs` and
/// `geometry-engine/tests/primitive_mass_invariants.rs` use; the tolerances
/// passed at each call site below match those files' precedent for the same
/// primitive shape (box/prism ~3%, cone ~6%).
fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < Tolerance::default().distance() {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

/// Create a box through the live `/api/geometry/box` route and return its
/// object UUID plus the parsed response body (so callers can read
/// `perception.volume` / `perception.face_count` off the SAME response the
/// route actually sent, rather than recomputing it).
async fn create_box(
    state: &AppState,
    center: [f64; 3],
    width: f64,
    depth: f64,
    height: f64,
) -> (Uuid, serde_json::Value) {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "center": center, "width": width, "depth": depth, "height": height }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    let uuid = Uuid::parse_str(body["object"]["id"].as_str().expect("box uuid string"))
        .expect("box uuid must parse");
    (uuid, body)
}

/// Locate the FaceId of a solid's +Z face by surface normal -- the same
/// idiom `geometry-engine/tests/shell_volume_invariants.rs::box_with_top_face`
/// uses to find a face without depending on face-creation order. Reaches
/// directly into `state.model` (a private `AppState` field visible to this
/// sibling module exactly as `router_integration_tests::seed_box` already
/// relies on) instead of parsing FaceIds back out of the flattened mesh.
async fn top_face_id(state: &AppState, solid_id: u32) -> u32 {
    let model = state.model.read().await;
    let solid = model.solids.get(solid_id).expect("solid must exist");
    let shell = model.shells.get(solid.outer_shell).expect("outer shell");
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        if let Ok(n) = surface.normal_at(0.5, 0.5) {
            if n.approx_eq(&Vector3::Z, NORMAL_TOLERANCE) {
                return face_id;
            }
        }
    }
    panic!("solid {solid_id} has no +Z face");
}

/// World-space bounding-box centre of a live solid, read directly off the
/// exact B-Rep (not the tessellated display mesh), so position assertions
/// are not muddied by tessellation error.
async fn bbox_center(state: &AppState, solid_id: u32) -> Point3 {
    let model = state.model.read().await;
    model
        .solid_world_bbox(solid_id)
        .expect("solid must have a world bbox")
        .center()
}

/// Boot a document, seed one box, flush, inject an event the current kernel
/// cannot replay, then reboot against the SAME file-backed database -- the
/// EXACT recipe
/// `durability_boot_tests.rs::unknown_event_quarantines_and_serves_clean_prefix`
/// uses, reusing its `temp_db_path`/`open_db`/`build_state`/`dispatch`/`post`
/// primitives rather than re-deriving the fixture. Returns the rebooted,
/// quarantined `AppState`.
async fn quarantined_state() -> AppState {
    let path = temp_db_path();

    {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;
        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/box",
                json!({ "width": 4.0, "depth": 4.0, "height": 4.0 }),
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "seed box create must succeed; body = {body}"
        );
        state
            .timeline_recorder
            .flush()
            .await
            .expect("flush must succeed");
    }

    {
        let db = open_db(&path).await;
        let mut events = db
            .load_all_timeline_events(durability::DURABILITY_SESSION_ID)
            .await
            .expect("load persisted events");
        let template = events.pop().expect("at least one persisted box event");
        let max_seq = template.sequence_number;

        let unknown_op = timeline_engine::Operation::Generic {
            command_type: "quarantine_probe_six_routes_gap".to_string(),
            parameters: json!({}),
        };
        let new_id = Uuid::new_v4().to_string();
        let mut blob = template.data.clone();
        blob["operation"] = serde_json::to_value(&unknown_op).expect("op serializes");
        blob["sequence_number"] = json!(max_seq + 1);
        blob["id"] = json!(new_id);

        let injected = TimelineEventData {
            id: new_id,
            session_id: template.session_id.clone(),
            event_type: "quarantine_probe_six_routes_gap".to_string(),
            user_id: template.user_id.clone(),
            timestamp: template.timestamp,
            data: blob,
            branch_id: template.branch_id.clone(),
            sequence_number: max_seq + 1,
        };
        db.save_timeline_event(durability::DURABILITY_SESSION_ID, &injected)
            .await
            .expect("inject unknown event");
    }

    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    let status = state2.durability_status.read().await.clone();
    assert!(
        matches!(status, durability::DurabilityStatus::Quarantined { .. }),
        "fixture must actually quarantine the rebooted document; got {status:?}"
    );

    state2
}

// =====================================================================
// shell
// =====================================================================

/// `POST /api/geometry/shell` -- hollowing an `a x b x c` box with the +Z cap
/// removed must leave EXACTLY the open-top-shell analytic volume
/// `a*b*c - (a-2t)(b-2t)(c-t)`, the same formula
/// `geometry-engine/tests/shell_volume_invariants.rs` proves directly
/// against the kernel. The volume must strictly drop from the source solid,
/// the face count must rise (wall + floor faces added), and the hollow must
/// still certify sound and watertight.
#[tokio::test]
async fn shell_hollows_box_to_analytic_volume() {
    let state = make_test_state().await;
    let (a, b, c, t) = (10.0, 10.0, 10.0, 1.0);
    let (uuid, box_body) = create_box(&state, [0.0, 0.0, 0.0], a, b, c).await;
    let volume_before = box_body["perception"]["volume"]
        .as_f64()
        .expect("box volume");
    let face_count_before = box_body["perception"]["face_count"]
        .as_u64()
        .expect("box face_count");

    let solid_id = state.get_local_id(&uuid).expect("box must be registered");
    let top = top_face_id(&state, solid_id).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/shell",
            json!({ "object": uuid.to_string(), "thickness": t, "faces_to_remove": [top] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "shell must 200; body = {body}");

    let volume_after = body["perception"]["volume"].as_f64().expect("shell volume");
    let expected = a * b * c - (a - 2.0 * t) * (b - 2.0 * t) * (c - t);
    assert!(
        rel_close(volume_after, expected, 0.05),
        "open-top shell volume must match a*b*c - (a-2t)(b-2t)(c-t) = \
         {expected:.3}; got {volume_after:.3} (before = {volume_before:.3})"
    );
    assert!(
        volume_after < volume_before,
        "shelling must strictly remove material: before = {volume_before:.3}, \
         after = {volume_after:.3}"
    );

    let face_count_after = body["perception"]["face_count"]
        .as_u64()
        .expect("shell face_count");
    assert!(
        face_count_after > face_count_before,
        "hollowing must add wall/floor faces: before = {face_count_before}, \
         after = {face_count_after}"
    );

    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "an open-top shell of a valid box must certify sound; body = {body}"
    );
    assert_eq!(
        body["perception"]["watertight"].as_bool(),
        Some(true),
        "an open-top shell (single connected shell) must be watertight; \
         body = {body}"
    );
}

// =====================================================================
// mirror
// =====================================================================

/// `POST /api/geometry/mirror` -- mirroring an off-centre box across the YZ
/// plane (`plane_normal = [1,0,0]`, `plane_origin` at the world origin) must
/// flip its X centroid to the negative side while leaving Y/Z untouched, and
/// must preserve the source solid's volume (a rigid reflection, not an
/// approximation).
#[tokio::test]
async fn mirror_reflects_position_and_preserves_volume() {
    let state = make_test_state().await;
    let (uuid, box_body) = create_box(&state, [10.0, 0.0, 0.0], 4.0, 4.0, 4.0).await;
    let volume_before = box_body["perception"]["volume"]
        .as_f64()
        .expect("box volume");
    let solid_id = state.get_local_id(&uuid).expect("box must be registered");
    let center_before = bbox_center(&state, solid_id).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/mirror",
            json!({
                "object": uuid.to_string(),
                "plane_origin": [0.0, 0.0, 0.0],
                "plane_normal": [1.0, 0.0, 0.0],
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "mirror must 200; body = {body}");

    let volume_after = body["perception"]["volume"]
        .as_f64()
        .expect("mirror volume");
    assert!(
        rel_close(volume_after, volume_before, 0.02),
        "mirroring is a rigid transform -- volume must be preserved: \
         before = {volume_before:.3}, after = {volume_after:.3}"
    );

    let center_after = bbox_center(&state, solid_id).await;
    let expected = Point3::new(-center_before.x, center_before.y, center_before.z);
    assert!(
        center_after.approx_eq(&expected, NORMAL_TOLERANCE),
        "mirroring across the YZ plane must negate X and leave Y/Z \
         unchanged: before = ({:.3},{:.3},{:.3}), expected \
         ({:.3},{:.3},{:.3}), got ({:.3},{:.3},{:.3})",
        center_before.x,
        center_before.y,
        center_before.z,
        expected.x,
        expected.y,
        expected.z,
        center_after.x,
        center_after.y,
        center_after.z
    );

    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "a mirrored valid box must still certify sound; body = {body}"
    );
}

// =====================================================================
// extrude
// =====================================================================

/// `POST /api/geometry/extrude` -- a 6x4 rectangle extruded 5 units along
/// the default +Z direction must produce exactly `6*4*5 = 120` volume (a
/// straight prism, computed from the request inputs, not pasted from a
/// run), with exactly the 6 planar faces (2 caps + 4 sides) a rectangular
/// prism has.
#[tokio::test]
async fn extrude_profile_volume_matches_area_times_distance() {
    let state = make_test_state().await;
    let (lx, ly, distance) = (6.0, 4.0, 5.0);

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/extrude",
            json!({
                "profile": [
                    [0.0, 0.0, 0.0], [lx, 0.0, 0.0], [lx, ly, 0.0], [0.0, ly, 0.0]
                ],
                "distance": distance,
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "extrude must 200; body = {body}");

    let volume = body["perception"]["volume"]
        .as_f64()
        .expect("extrude volume");
    let expected = lx * ly * distance;
    assert!(
        rel_close(volume, expected, 0.03),
        "extruding a {lx}x{ly} rectangle by {distance} must yield volume \
         {expected:.3}; got {volume:.3}"
    );

    assert_eq!(
        body["perception"]["face_count"].as_u64(),
        Some(6),
        "a rectangular prism extrusion must have exactly 6 planar faces \
         (2 caps + 4 sides); body = {body}"
    );
    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "a straight rectangular extrusion must certify sound; body = {body}"
    );
}

// =====================================================================
// cone
// =====================================================================

/// `POST /api/geometry/cone` -- a pointed cone (`top_radius` omitted -> 0)
/// of base radius 3 and height 5 must match the analytic frustum formula
/// `V = (1/3)*pi*r^2*h`, computed here from the request inputs, with the
/// same 6% mesh-based tolerance
/// `geometry-engine/tests/primitive_mass_invariants.rs` uses for cone
/// volume (curved-surface tessellation, not exact quadrature).
#[tokio::test]
async fn cone_volume_matches_analytic_formula() {
    let state = make_test_state().await;
    let (r, h) = (3.0, 5.0);

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/cone",
            json!({ "base_radius": r, "height": h }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cone must 200; body = {body}");

    let volume = body["perception"]["volume"].as_f64().expect("cone volume");
    let expected = std::f64::consts::PI * r * r * h / 3.0;
    assert!(
        rel_close(volume, expected, 0.06),
        "a pointed cone of base radius {r} and height {h} must have volume \
         (1/3)*pi*r^2*h = {expected:.3}; got {volume:.3}"
    );

    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "a valid cone primitive must certify sound; body = {body}"
    );
    assert_eq!(
        body["perception"]["brep_valid"].as_bool(),
        Some(true),
        "a valid cone primitive must be brep-valid; body = {body}"
    );
}

// =====================================================================
// nurbs_loft
// =====================================================================

/// `POST /api/geometry/nurbs_loft` -- a constant-radius "straight tube" loft
/// through 3 rings (the same regression shape
/// `geometry-engine/tests/nurbs_loft.rs::nurbs_loft_straight_tube_is_watertight`
/// pins directly against the kernel) must certify sound, watertight, AND
/// span exactly the requested Z height -- the caps sit at the first/last
/// section's Z coordinate by construction, so height IS a derivable
/// invariant even though the lateral is a genuine NURBS skin.
///
/// Volume is intentionally NOT asserted: the periodic-cubic lateral
/// interpolates through only `n` sampled points around a true circle, so
/// its enclosed volume legitimately deviates from `pi*r^2*h` by an amount
/// that depends on sample density (Runge-type overshoot between samples),
/// not a fixed, derivable percentage the way a box/cone/prism's volume is.
#[tokio::test]
async fn nurbs_loft_closes_sound_and_spans_requested_height() {
    let state = make_test_state().await;
    let (r, h, n) = (3.0, 5.0, 12usize);
    let ring = |z: f64| -> Vec<[f64; 3]> {
        (0..n)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / n as f64;
                [r * a.cos(), r * a.sin(), z]
            })
            .collect::<Vec<_>>()
    };

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/nurbs_loft",
            json!({
                "sections": [ring(0.0), ring(h / 2.0), ring(h)],
                "degree_u": 3,
                "degree_v": 3,
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "nurbs_loft must 200; body = {body}");

    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "a straight-tube nurbs_loft must certify sound; body = {body}"
    );
    assert_eq!(
        body["perception"]["watertight"].as_bool(),
        Some(true),
        "a straight-tube nurbs_loft must be watertight; body = {body}"
    );
    assert_eq!(
        body["perception"]["open_edges"].as_u64(),
        Some(0),
        "a sound loft must have zero open edges; body = {body}"
    );

    let dims = body["perception"]["dims"]
        .as_array()
        .expect("perception.dims");
    let sz = dims[2].as_f64().expect("dims.z");
    let z_tol = NORMAL_TOLERANCE.distance() * 1_000.0;
    assert!(
        (sz - h).abs() < z_tol,
        "the loft's end caps sit exactly at the first/last section's Z -- \
         the solid's bbox height must equal the requested {h} (tol \
         {z_tol}), got {sz}"
    );
}

// =====================================================================
// face/extrude
// =====================================================================

/// `POST /api/geometry/face/extrude` -- pulling an 8x8x8 box's +Z cap
/// outward by 3 (direction omitted -> defaults to the face's own outward
/// normal) must add exactly `footprint_area * distance = 8*8*3 = 192`
/// volume on top of the source box's volume, and must preserve the host's
/// UUID (identity-preserving modify, not a replacement).
///
/// KNOWN RED -- production defect, not a test defect: the volume, dims, and
/// identity assertions all pass (the mesh is a clean, closed 8x8x11 box:
/// `cert.watertight: true`, `cert.boundary_edges: 0`, exact volume
/// 704.0000000000003 = 512 + 192). But `perception.sound` comes back
/// `false` -- `validate_solid_scoped` finds 4 real boundary edges (one on
/// each of the 4 original side walls, faces 7/8/9/10) where the new upper
/// wall failed to weld to the original wall at the z=8 seam. This is the
/// "unified extrusion" merge path (`create_unified_extrusion`, the branch
/// `extrude_face` takes when the target face already belongs to a parent
/// solid) -- a path with ZERO other test coverage anywhere in the
/// geometry-engine suite (grep for `create_unified_extrusion` in `tests/`).
/// Do not "fix" this test to stop asserting `sound`; that would document a
/// real defect as correct. See the task report for the full finding.
#[tokio::test]
async fn face_extrude_adds_footprint_times_distance_volume() {
    let state = make_test_state().await;
    let (w, d, h, pull) = (8.0, 8.0, 8.0, 3.0);
    let (uuid, box_body) = create_box(&state, [0.0, 0.0, 0.0], w, d, h).await;
    let volume_before = box_body["perception"]["volume"]
        .as_f64()
        .expect("box volume");
    let solid_id = state.get_local_id(&uuid).expect("box must be registered");
    let top = top_face_id(&state, solid_id).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/face/extrude",
            json!({ "object_uuid": uuid.to_string(), "face_id": top, "distance": pull }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "face/extrude must 200; body = {body}"
    );

    let volume_after = body["perception"]["volume"]
        .as_f64()
        .expect("face/extrude volume");
    let expected = volume_before + w * d * pull;
    let added = w * d * pull;
    assert!(
        rel_close(volume_after, expected, 0.03),
        "pulling the {w}x{d} top face by {pull} must add {added} volume: \
         before = {volume_before:.3}, expected {expected:.3}, got \
         {volume_after:.3}"
    );

    assert_eq!(
        body["object"]["id"].as_str(),
        Some(uuid.to_string().as_str()),
        "face-extrude is identity-preserving -- the host UUID must survive; \
         body = {body}"
    );
    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "a face-pulled valid box must still certify sound; body = {body}"
    );
}

// =====================================================================
// Quarantine disclosure -- two of the six previously-untested call sites
// whose `perception.durability` disclosure (`5397d71b` + follow-up) shipped
// unverified. Recipe: `quarantined_state()` above, copied from
// `durability_boot_tests.rs::unknown_event_quarantines_and_serves_clean_prefix`.
// =====================================================================

/// GATE: on a quarantined document, `POST /api/geometry/extrude`'s own
/// response (default full-certificate path) discloses the document-level
/// durability state under `perception.durability` -- an agent that only
/// ever calls this route must still learn its document is incomplete.
#[tokio::test]
async fn extrude_response_discloses_quarantine() {
    let state = quarantined_state().await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/extrude",
            json!({
                "profile": [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0]],
                "distance": 1.0,
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "extrude on the clean prefix must still 200; body = {body}"
    );
    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "the freshly extruded solid is itself sound; body = {body}"
    );
    assert_eq!(
        body["perception"]["durability"]["state"].as_str(),
        Some("quarantined"),
        "extrude's own response on a quarantined document must disclose it \
         under perception.durability; body = {body}"
    );
    assert_eq!(
        body["perception"]["durability"]["first_break_kind"].as_str(),
        Some("quarantine_probe_six_routes_gap"),
        "the disclosure must name the offending injected event kind; \
         body = {body}"
    );
}

/// GATE: same as above, for `POST /api/geometry/cone` -- the second of the
/// two previously-untested routes exercised against the quarantine
/// disclosure.
#[tokio::test]
async fn cone_response_discloses_quarantine() {
    let state = quarantined_state().await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/cone",
            json!({ "base_radius": 2.0, "height": 3.0 }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "cone on the clean prefix must still 200; body = {body}"
    );
    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "the freshly created cone is itself sound; body = {body}"
    );
    assert_eq!(
        body["perception"]["durability"]["state"].as_str(),
        Some("quarantined"),
        "cone's own response on a quarantined document must disclose it \
         under perception.durability; body = {body}"
    );
    assert_eq!(
        body["perception"]["durability"]["first_break_kind"].as_str(),
        Some("quarantine_probe_six_routes_gap"),
        "the disclosure must name the offending injected event kind; \
         body = {body}"
    );
}
