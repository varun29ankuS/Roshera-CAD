//! Drawing-pipeline performance regression harness (#22).
//!
//! A modest high-edge-count shop part (a 293-face / 771-edge involute spur gear,
//! live 2026-07-14) wedged `standard_drawing_auto` for 4.5+ minutes with the
//! whole backend unresponsive: HLR classified every sampled edge segment by
//! ray-casting against EVERY face, i.e. O(views · E · samples · F) — tens of
//! millions of curved-surface intersections, and the extent pass rebuilt every
//! view a second time.
//!
//! This harness reproduces the pathology with a comparable high-edge-count solid
//! — a bumpy solid of revolution: hundreds of analytic cone/cylinder bands, so
//! hundreds of circular rim edges each sampled 96× against hundreds of faces —
//! and asserts the full automatic sheet completes well under a generous CI
//! budget. It is RED without the projected-AABB occlusion grid + extent-only
//! pass-1 fix (minutes / `ROSHERA_DRAW_NOACCEL`) and GREEN with it (seconds).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use geometry_engine::drawing::standard_drawing_auto;
use geometry_engine::operations::revolve::revolve_meridian;
use geometry_engine::operations::revolve::RevolveOptions;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;

/// Build a bumpy solid of revolution with `bands` analytic bands → hundreds of
/// curved faces and circular rim edges, reproducing the gear-class HLR stress in
/// one operation. The `(r, z)` meridian climbs `z` monotonically (a stack of
/// frustums, never self-intersecting) with `r` oscillating to force a distinct
/// band per step.
fn bumpy_vase(model: &mut BRepModel, bands: usize) -> SolidId {
    let height = 100.0_f64;
    let mut profile: Vec<(f64, f64)> = Vec::with_capacity(bands + 2);
    profile.push((0.0, 0.0)); // bottom cap on the axis
    for k in 0..bands {
        let z = height * (k as f64 + 1.0) / bands as f64;
        let r = 20.0 + 6.0 * (k as f64 * 0.7).sin();
        profile.push((r, z));
    }
    profile.push((0.0, height)); // top cap back to the axis
    revolve_meridian(model, &profile, RevolveOptions::default()).expect("revolve bumpy vase")
}

/// `(faces, distinct_edges, distinct_vertices)` of a solid across all shells.
fn topo_counts(model: &BRepModel, solid_id: SolidId) -> (usize, usize, usize) {
    let solid = model.solids.get(solid_id).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut faces = 0usize;
    let mut edges: HashSet<u32> = HashSet::new();
    let mut verts: HashSet<u32> = HashSet::new();
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            faces += 1;
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &eid in &lp.edges {
                    edges.insert(eid);
                    if let Some(e) = model.edges.get(eid) {
                        verts.insert(e.start_vertex);
                        verts.insert(e.end_vertex);
                    }
                }
            }
        }
    }
    (faces, edges.len(), verts.len())
}

/// The auto sheet for a gear-class part (hundreds of curved faces + circular
/// edges) must build in seconds, not minutes. Budget is deliberately generous
/// (30 s) for slow/loaded CI; the fix lands it in low single-digit seconds while
/// the pre-fix brute-force path takes minutes (verified via `ROSHERA_DRAW_NOACCEL`).
#[test]
fn gear_class_auto_drawing_completes_in_seconds() {
    let mut m = BRepModel::new();
    let part = bumpy_vase(&mut m, 300);
    let (faces, edges, verts) = topo_counts(&m, part);
    // Confirm we actually built a high-edge-count part comparable to the gear.
    assert!(
        faces >= 250 && edges >= 250,
        "fixture must be high-edge-count (got {faces} faces / {edges} edges / {verts} verts)"
    );
    eprintln!("[drawing_perf] fixture: {faces} faces / {edges} edges / {verts} verts");

    let t = Instant::now();
    let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("auto drawing");
    let elapsed = t.elapsed();
    eprintln!(
        "[drawing_perf] standard_drawing_auto: {:.2} s",
        elapsed.as_secs_f64()
    );

    assert!(
        !drawing.views.is_empty(),
        "auto drawing produced the standard views"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "gear-class auto drawing must finish < 30 s (took {:.1} s) — HLR occlusion \
         must stay broad-phase accelerated, not O(E·samples·F)",
        elapsed.as_secs_f64()
    );
}
