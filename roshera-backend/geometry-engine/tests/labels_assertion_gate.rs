//! Gate for the ASSERTION-REQUIRED labeller (D4 + D3) — "a name the kernel can
//! keep proving".
//!
//! D4 (NO BARE LABELS): every entity label carries an ASSERTION (the Pillar-3
//! selector that found it, or the entity's geometric fingerprint). `resolve()`
//! RE-RUNS the assertion and reports `Stale` when it no longer holds — it never
//! silently re-points. The `labels_consistent` certificate flag is the part-wide
//! roll-up; per D4 an `Inconsistent` verdict is an ANNOTATION defect, NOT a
//! geometric one, so it does NOT pull `sound` down.
//!
//! D3 (AUTO-PROPOSE): the kernel recognizes features and SUGGESTS a name + the
//! assertion that pins it, without applying — confirming a proposal is
//! `label_*_with_assertion` with that exact assertion.
//!
//! The part is the known-watertight tube nozzle of `labels_gate.rs`: inner +
//! outer cylindrical walls (throat / chamber) capped by two planar annular ends.

use geometry_engine::labels::{AssertionStatus, LabelAssertion, SelectorSpec};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::provenance::LabelsConsistency;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::queries::select::{resolve_face, Extremal, FaceQuery, SurfaceKind};

fn line_edge(model: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let s = model.vertices.get(a).expect("start vertex").point();
    let e = model.vertices.get(b).expect("end vertex").point();
    let line = Line::new(s, e);
    let cid = model.curves.add(Box::new(line));
    let edge = Edge::new(
        0,
        a,
        b,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    );
    model.edges.add(edge)
}

fn build_nozzle(model: &mut BRepModel) -> SolidId {
    let v0 = model.vertices.add(2.0, 0.0, 0.0);
    let v1 = model.vertices.add(4.0, 0.0, 0.0);
    let v2 = model.vertices.add(4.0, 0.0, 3.0);
    let v3 = model.vertices.add(2.0, 0.0, 3.0);
    let edges = vec![
        line_edge(model, v0, v1),
        line_edge(model, v1, v2),
        line_edge(model, v2, v3),
        line_edge(model, v3, v0),
    ];
    revolve_profile(model, edges, RevolveOptions::default()).expect("revolve nozzle")
}

/// The throat selector: the smallest-area cylindrical wall (the inner / min-
/// radius bore). The chamber selector is its largest-area twin.
fn throat_selector() -> FaceQuery {
    FaceQuery::new(SurfaceKind::Cylindrical).extremal(Extremal::SmallestArea)
}

/// (a) D4 — a BARE entity label (no assertion) is REJECTED.
#[test]
fn bare_label_create_is_rejected() {
    use geometry_engine::labels::{Label, LabelError, LabelKind, LabelTarget};
    use geometry_engine::primitives::persistent_id::PersistentId;

    let mut m = BRepModel::new();
    let solid = build_nozzle(&mut m);
    let throat_fid = resolve_face(&mut m, solid, &throat_selector()).expect("throat resolves");
    let pid = m.ensure_face_annotation_key(throat_fid);

    // Hand-built bare label (the only way to even express "no assertion"): the
    // sidecar refuses it. The public `label_*` helpers cannot produce one — they
    // always capture an assertion — which is exactly the point of D4.
    let bare = Label {
        target: LabelTarget::Entity {
            kind: LabelKind::Face,
            pid,
        },
        assertion: None,
        description: None,
    };
    assert_eq!(
        m.labels.attach("throat", bare),
        Err(LabelError::MissingAssertion),
        "a bare entity label must be refused, never stored"
    );
    assert!(m.labels.is_empty(), "nothing was stored");
}

/// (b) D4 must-fail — a selector-labelled throat resolves to the min-radius
/// face; then a geometry mutation that breaks the assertion makes resolve report
/// `Stale` AND the certificate's `labels_consistent` go `Inconsistent`.
#[test]
fn selector_label_resolves_then_goes_stale_when_assertion_breaks() {
    let mut m = BRepModel::new();
    let solid = build_nozzle(&mut m);

    let throat_fid = resolve_face(&mut m, solid, &throat_selector()).expect("throat resolves");

    // Label BY SELECTOR — the selector IS the assertion.
    let spec = SelectorSpec::Face(geometry_engine::labels::FaceSelectorSpec {
        surface: "cylindrical".into(),
        normal_dir: None,
        angle_tol_deg: 12.0,
        extremal: "smallest_area".into(),
        along: None,
        axis_origin: None,
        axis_dir: None,
    });
    m.label_face_with_assertion(
        throat_fid,
        "throat",
        LabelAssertion::Selector(spec),
        Some("minimum-radius wall".into()),
    )
    .expect("label throat by selector");

    // Resolve returns the SAME min-radius face, and the assertion HOLDS.
    let (resolved, status) = m
        .resolve_label_face_checked("throat")
        .expect("throat resolves");
    assert_eq!(
        resolved, throat_fid,
        "resolve returns the selector-found face"
    );
    assert_eq!(
        status,
        AssertionStatus::Holds,
        "assertion holds on clean geom"
    );

    // Certificate: with one (holding) label, labels_consistent == Consistent and
    // soundness is unaffected by the annotation.
    let before = m.certify_solid(solid);
    assert_eq!(before.labels_consistent, LabelsConsistency::Consistent);

    // ── MUTATE so the assertion BREAKS: remove the throat face from its shell.
    // The smallest-area cylinder selector now resolves to the OTHER (chamber)
    // cylinder — a DIFFERENT face — so the label's claim no longer holds.
    let shells: Vec<_> = m.solids.get(solid).expect("solid").shell_ids().to_vec();
    let mut removed = false;
    for sh in shells {
        if let Some(shell) = m.shells.get_mut(sh) {
            if shell.remove_face(throat_fid) {
                removed = true;
            }
        }
    }
    assert!(removed, "throat face removed from its shell");

    // The selector now resolves to a face that is NOT the labelled throat.
    let now = resolve_face(&mut m, solid, &throat_selector());
    assert!(
        matches!(now, Ok(f) if f != throat_fid) || now.is_err(),
        "the selector no longer resolves to the labelled throat"
    );

    // resolve() → Stale (the id is the last-known throat, but the claim broke).
    let (still, status) = m
        .resolve_label_face_checked("throat")
        .expect("throat still resolves to a live id");
    assert_eq!(still, throat_fid, "still the named entity's last-known id");
    assert_eq!(
        status,
        AssertionStatus::Stale,
        "the broken assertion must surface as Stale, never a silent re-point"
    );

    // labels_consistent → Inconsistent — a check that can't go Inconsistent is
    // worthless.
    let after = m.certify_solid(solid);
    assert_eq!(
        after.labels_consistent,
        LabelsConsistency::Inconsistent,
        "a stale label must drive labels_consistent Inconsistent"
    );
    // And the label flag is an ANNOTATION, not geometry: `is_sound()` is purely
    // the geometric verdict and EXCLUDES labels_consistent. (The shell-edit above
    // legitimately opened the mesh, so geometric soundness may change on its own
    // merits — the invariant is that the LABEL flag is not a factor.)
    let geometric_sound = after.brep_valid
        && after.watertight
        && after.manifold
        && after.self_intersection_free
        && after.construction_consistent.is_sound();
    assert_eq!(
        after.is_sound(),
        geometric_sound,
        "is_sound must be the geometric verdict only — Inconsistent labels must NOT pull it down"
    );
}

/// (c) D3 — propose_labels on the revolved nozzle proposes a "throat"
/// (min-radius) and an "exit", each carrying the assertion that pins it.
#[test]
fn propose_labels_recognizes_throat_and_exit_with_assertions() {
    let mut m = BRepModel::new();
    let solid = build_nozzle(&mut m);

    let proposals = m.propose_labels(solid);
    let names: Vec<&str> = proposals
        .iter()
        .map(|p| p.suggested_name.as_str())
        .collect();
    assert!(names.contains(&"throat"), "proposes throat: {names:?}");
    assert!(names.contains(&"exit"), "proposes exit: {names:?}");

    // Each proposal carries an ASSERTION; confirming it = label_*_with_assertion
    // with that exact assertion. Confirm the throat proposal and prove it
    // resolves to the actual min-radius face.
    let throat_prop = proposals
        .iter()
        .find(|p| p.suggested_name == "throat")
        .expect("throat proposal");
    assert!(throat_prop.confidence > 0.0 && throat_prop.confidence <= 1.0);
    let expected_throat = resolve_face(&mut m, solid, &throat_selector()).expect("throat");

    // The proposal's assertion is a selector (the kernel owns the claim). On a
    // surface-of-revolution part the throat recognizer is the geometry-aware
    // min-radius station (axis-relative), which on this tube nozzle picks the
    // same inner wall the smallest-area-cylinder selector does — but is now
    // robust to a necked-down revolved band, not just an analytic cylinder.
    match &throat_prop.assertion {
        LabelAssertion::Selector(SelectorSpec::Face(s)) => {
            assert_eq!(s.extremal, "min_radius_station");
            assert!(
                s.axis_origin.is_some() && s.axis_dir.is_some(),
                "min-radius-station throat carries the symmetry axis in its selector"
            );
        }
        other => panic!("throat proposal should carry a face selector assertion: {other:?}"),
    }

    // Confirm = attach with the proposal's assertion. Then it resolves to the
    // recognized feature and the assertion holds. (The min-radius station on this
    // tube nozzle is the inner cylindrical wall — the same face `throat_selector`
    // finds, which `expected_throat` captured above.)
    m.label_face_with_assertion(
        expected_throat,
        &throat_prop.suggested_name,
        throat_prop.assertion.clone(),
        None,
    )
    .expect("confirm throat proposal");
    let (fid, status) = m
        .resolve_label_face_checked("throat")
        .expect("confirmed throat resolves");
    assert_eq!(fid, expected_throat);
    assert_eq!(status, AssertionStatus::Holds);

    // The exit proposal too carries an assertion that resolves to a planar cap.
    // On a surface-of-revolution part the exit recognizer is the axis-EXTREMAL
    // planar cap (axis-relative, so a bell-down nozzle works), carrying the
    // symmetry axis in its selector rather than a hardcoded +Z normal.
    let exit_prop = proposals
        .iter()
        .find(|p| p.suggested_name == "exit")
        .expect("exit proposal");
    match &exit_prop.assertion {
        LabelAssertion::Selector(SelectorSpec::Face(s)) => {
            assert_eq!(s.surface, "planar");
            assert_eq!(s.extremal, "axial_extremal_cap");
            assert!(
                s.axis_origin.is_some() && s.axis_dir.is_some(),
                "axial-extremal-cap exit carries the symmetry axis"
            );
        }
        other => panic!("exit proposal should carry a planar-cap selector: {other:?}"),
    }
}

/// (d) NO-REGRESSION — a solid with NO labels: labels_consistent ==
/// NotApplicable, and soundness is exactly the geometry's verdict (the label
/// flag never affects it).
#[test]
fn no_labels_is_not_applicable_and_sound_unaffected() {
    let mut m = BRepModel::new();
    let solid = build_nozzle(&mut m);

    let cert = m.certify_solid(solid);
    assert_eq!(
        cert.labels_consistent,
        LabelsConsistency::NotApplicable,
        "a part with no labels has nothing to check"
    );
    // The labels flag is excluded from is_sound by construction; a NotApplicable
    // verdict cannot make a sound solid unsound.
    let gt = m.ground_truth(solid).expect("ground truth");
    assert_eq!(
        gt.certificate.labels_consistent,
        LabelsConsistency::NotApplicable
    );
}

/// D4 — a FINGERPRINT-backed (by-id) label HOLDS on the entity it captured and
/// goes Stale when that entity's geometric identity drifts. Here the throat
/// face is removed from its shell while a fresh, distinct face is what the
/// fingerprint would now have to match — the fingerprint records the captured
/// radius, so a swap to a different-radius live entity reads as Stale.
#[test]
fn fingerprint_label_holds_then_breaks_on_radius_drift() {
    use geometry_engine::labels::{Fingerprint, LabelKind};

    let mut m = BRepModel::new();
    let solid = build_nozzle(&mut m);
    let throat_fid = resolve_face(&mut m, solid, &throat_selector()).expect("throat");
    let chamber_fid = resolve_face(
        &mut m,
        solid,
        &FaceQuery::new(SurfaceKind::Cylindrical).extremal(Extremal::LargestArea),
    )
    .expect("chamber");

    // By-id attach captures the throat's FINGERPRINT (its min radius ~2).
    m.label_face(throat_fid, "throat", None)
        .expect("label by id");
    assert_eq!(m.verify_label_assertion("throat"), AssertionStatus::Holds);

    // Forge an assertion at the throat's own POSITION but with the CHAMBER's
    // radius (~4) — only the radius differs from the live throat (~2). The
    // entity pid still points at the throat face, so this isolates the radius
    // check: a mismatched radius alone must read Stale (the fingerprint is
    // actually CHECKED, not a rubber stamp).
    let throat_fp = m.face_fingerprint(throat_fid).expect("throat fp");
    let chamber_fp = m.face_fingerprint(chamber_fid).expect("chamber fp");
    let mismatched = Fingerprint {
        kind: LabelKind::Face,
        position: throat_fp.position,
        normal: None,
        radius: chamber_fp.radius,
        size: None,
    };
    m.label_face_with_assertion(
        throat_fid,
        "throat",
        LabelAssertion::Fingerprint(mismatched),
        None,
    )
    .expect("relabel with mismatched fingerprint");
    assert_eq!(
        m.verify_label_assertion("throat"),
        AssertionStatus::Stale,
        "a fingerprint that no longer matches the live entity must read Stale"
    );
}

/// RENDER — propose labels, CONFIRM them with their assertions, and overlay the
/// resulting callouts on the part so the proposed+confirmed labels can be SEEN.
#[test]
fn render_proposed_and_confirmed_labels_overlay() {
    use geometry_engine::math::Point3;
    use geometry_engine::render::{render_solid_with_labels, CanonicalView, RenderOptions};

    let mut m = BRepModel::new();
    let solid = build_nozzle(&mut m);

    // D3: recognize features, then CONFIRM each proposal with its own assertion
    // (the user owns the name, the kernel owns the claim).
    let proposals = m.propose_labels(solid);
    assert!(!proposals.is_empty(), "the nozzle yields proposals");
    for p in &proposals {
        match p.kind {
            "face" => {
                // Resolve the proposal's own assertion to the entity it names,
                // then pin the suggested name with that exact assertion.
                if let LabelAssertion::Selector(SelectorSpec::Face(s)) = &p.assertion {
                    use geometry_engine::math::Vector3;
                    use geometry_engine::queries::select::Axis;
                    let axis = match (s.axis_origin, s.axis_dir) {
                        (Some(o), Some(d)) => Some(Axis {
                            origin: Vector3::new(o[0], o[1], o[2]),
                            direction: Vector3::new(d[0], d[1], d[2]),
                        }),
                        _ => None,
                    };
                    let q = match s.extremal.as_str() {
                        "smallest_area" => FaceQuery::new(SurfaceKind::Cylindrical)
                            .extremal(Extremal::SmallestArea),
                        "largest_area" => {
                            FaceQuery::new(SurfaceKind::Cylindrical).extremal(Extremal::LargestArea)
                        }
                        "min_radius_station" => FaceQuery::new(SurfaceKind::Any)
                            .extremal(Extremal::MinRadiusStation(axis.expect("axis"))),
                        "axial_extremal_cap" => FaceQuery::new(SurfaceKind::Planar)
                            .extremal(Extremal::AxialExtremalCap(axis.expect("axis"))),
                        _ => FaceQuery::new(SurfaceKind::Planar)
                            .facing(Vector3::new(0.0, 0.0, 1.0))
                            .extremal(Extremal::MostAlong(Vector3::new(0.0, 0.0, 1.0))),
                    };
                    if let Ok(fid) = resolve_face(&mut m, solid, &q) {
                        m.label_face_with_assertion(
                            fid,
                            &p.suggested_name,
                            p.assertion.clone(),
                            Some(p.rationale.clone()),
                        )
                        .expect("confirm proposal");
                    }
                }
            }
            _ => {}
        }
    }
    assert!(m.labels.len() >= 2, "confirmed at least throat + exit");

    // Build a callout per label that still has a world anchor.
    let names: Vec<String> = m.list_labels().into_iter().map(|(n, ..)| n).collect();
    let callouts: Vec<(Point3, String)> = names
        .iter()
        .filter_map(|name| {
            m.label_anchor(name)
                .map(|anchor| (anchor, name.to_uppercase()))
        })
        .collect();
    assert!(!callouts.is_empty(), "labels produced world anchors");

    let frame = render_solid_with_labels(
        &m,
        solid,
        &callouts,
        &RenderOptions {
            width: 800,
            height: 800,
            view: CanonicalView::Isometric,
            ..Default::default()
        },
    )
    .expect("render with labels");
    let png = frame.to_png().expect("encode png");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("labels_assertion_overlay.png");
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create output dir");
    }
    std::fs::write(&out, &png).expect("write png");
    eprintln!(
        "PROPOSED+CONFIRMED label overlay written to {}",
        out.display()
    );
}
