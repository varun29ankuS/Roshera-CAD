//! Gate test for the LABELLER — human-readable names pinned to topological
//! entities and cross-sections, the shared vocabulary between agent and user.
//!
//! The point of the labeller is that a NAME and the ENTITY it pins are bound
//! HONESTLY: a name attached by Pillar-3 description resolves back to the SAME
//! entity the description found, an unknown name REFUSES (never guesses), the
//! binding survives a snapshot round-trip, and `clear()` drops it with the
//! geometry. This gate proves each of those, then renders the part with the
//! labels overlaid so the callouts can be confirmed to land on the right
//! features.
//!
//! The part is a tube/washer of revolution (a minimal nozzle): two coaxial
//! cylindrical walls (inner = throat, outer = chamber) capped by two planar
//! annular ends (one of which is the exit). It is the known-watertight
//! `revolve_profile` shape, so the gate exercises the labeller, not the kernel.

use geometry_engine::labels::{LabelError, LabelKind, LabelTarget};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::ParameterRange;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::queries::select::{resolve_face, Extremal, FaceQuery, SurfaceKind};
use geometry_engine::render::{render_solid_with_labels, CanonicalView, RenderOptions};

/// A line edge between two existing vertices, built from public store APIs.
fn line_edge(model: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    use geometry_engine::primitives::curve::Line;
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

/// Build the tube nozzle: an offset rectangle meridian (inner r=2, outer r=4,
/// height z=0..3) revolved a full turn about +Z. Yields inner + outer cylinders
/// and two planar annular caps.
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

#[test]
fn labeller_attach_resolve_refuse_snapshot_clear_and_overlay() {
    let mut model = BRepModel::new();
    let solid = build_nozzle(&mut model);

    // ── Selectors (Pillar 3): describe each feature by MEANING ───────────────
    // Throat = the smaller-area cylindrical wall (inner radius). Chamber = the
    // larger-area cylindrical wall (outer). Exit = the topmost planar cap.
    let throat_sel = FaceQuery::new(SurfaceKind::Cylindrical).extremal(Extremal::SmallestArea);
    let chamber_sel = FaceQuery::new(SurfaceKind::Cylindrical).extremal(Extremal::LargestArea);
    // Exit = the planar cap whose outward normal points downstream (+Z).
    let exit_sel = FaceQuery::new(SurfaceKind::Planar).facing(Vector3::new(0.0, 0.0, 1.0));

    let throat_fid = resolve_face(&mut model, solid, &throat_sel).expect("throat resolves");
    let chamber_fid = resolve_face(&mut model, solid, &chamber_sel).expect("chamber resolves");
    let exit_fid = resolve_face(&mut model, solid, &exit_sel).expect("exit resolves");
    assert_ne!(
        throat_fid, chamber_fid,
        "throat and chamber are distinct faces"
    );
    assert_ne!(throat_fid, exit_fid);

    // ── Attach labels BY the selector-found ids ──────────────────────────────
    model
        .label_face(throat_fid, "throat", Some("minimum-radius wall".into()))
        .expect("label throat");
    model
        .label_face(chamber_fid, "chamber", None)
        .expect("label chamber");
    model
        .label_face(exit_fid, "exit", Some("downstream planar cap".into()))
        .expect("label exit");
    // A named cross-section (a plane, not an entity).
    model
        .label_section(
            "midspan",
            Point3::new(0.0, 0.0, 1.5),
            Vector3::new(0.0, 0.0, 1.0),
            Some("cut halfway up".into()),
        )
        .expect("label section");

    // ── list() has them all ──────────────────────────────────────────────────
    let listed = model.list_labels();
    let names: Vec<&str> = listed.iter().map(|(n, _, _, _)| n.as_str()).collect();
    assert!(names.contains(&"throat"));
    assert!(names.contains(&"chamber"));
    assert!(names.contains(&"exit"));
    assert!(names.contains(&"midspan"));
    assert_eq!(model.labels.len(), 4);
    // Section label reports kind "section"; faces report "face".
    let midspan_kind = listed
        .iter()
        .find(|(n, _, _, _)| n == "midspan")
        .map(|(_, k, _, _)| *k);
    assert_eq!(midspan_kind, Some("section"));

    // ── resolve("throat") returns the SAME face the selector found ───────────
    let resolved_throat = model.resolve_label_face("throat").expect("resolve throat");
    assert_eq!(
        resolved_throat, throat_fid,
        "resolving the label must return the exact face the selector found"
    );
    let resolved_exit = model.resolve_label_face("exit").expect("resolve exit");
    assert_eq!(resolved_exit, exit_fid);

    // Resolving a section by name returns the stored plane.
    let plane = model
        .resolve_label_section("midspan")
        .expect("resolve section");
    assert_eq!(plane.origin, Point3::new(0.0, 0.0, 1.5));

    // The stored target kind is Face for "throat".
    match &model.label("throat").expect("label present").target {
        LabelTarget::Entity { kind, .. } => assert_eq!(*kind, LabelKind::Face),
        _ => panic!("throat should be an entity label"),
    }

    // ── REFUSE: an unknown name is NotFound, never a guess ───────────────────
    assert_eq!(
        model.resolve_label_face("nozzle_of_theseus"),
        Err(LabelError::NotFound)
    );
    // A section name resolved as a face refuses (wrong kind), and vice versa.
    assert_eq!(
        model.resolve_label_face("midspan"),
        Err(LabelError::NotFound)
    );
    assert_eq!(
        model.resolve_label_section("throat"),
        Err(LabelError::NotFound)
    );
    // Empty name is refused at attach time.
    assert_eq!(
        model.label_face(throat_fid, "   ", None),
        Err(LabelError::EmptyName)
    );

    // ── snapshot round-trip preserves labels ─────────────────────────────────
    use geometry_engine::primitives::snapshot::ModelSnapshot;
    let snap = ModelSnapshot::take(&model);
    // Mutate: drop a label, then restore must bring it back.
    model.labels.remove("throat");
    assert!(model.resolve_label_face("throat").is_err());
    snap.restore(&mut model);
    assert_eq!(
        model
            .resolve_label_face("throat")
            .expect("throat after restore"),
        throat_fid,
        "snapshot restore must preserve the label binding"
    );
    assert_eq!(model.labels.len(), 4, "all four labels restored");

    // ── EYE OVERLAY: render the part with the labels drawn as callouts ───────
    let callouts: Vec<(Point3, String)> = ["throat", "chamber", "exit", "midspan"]
        .iter()
        .filter_map(|name| {
            model
                .label_anchor(name)
                .map(|anchor| (anchor, name.to_uppercase()))
        })
        .collect();
    assert_eq!(callouts.len(), 4, "every label produced a world anchor");

    let frame = render_solid_with_labels(
        &model,
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
        .join("labels_gate_overlay.png");
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create output dir");
    }
    std::fs::write(&out, &png).expect("write png");
    eprintln!("LABELLER overlay render written to {}", out.display());

    // ── clear() drops every label with the geometry ─────────────────────────
    model.clear_geometry();
    assert!(model.labels.is_empty(), "clear_geometry empties labels");
    assert_eq!(
        model.resolve_label_face("throat"),
        Err(LabelError::NotFound)
    );
}
