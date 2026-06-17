//! EYE-6: active-perception viewpoint selection (best-view + next-best-view).
//!
//! EYE-5 tells an agent *that* it hasn't seen some faces; this tells it *where
//! to look*. Two research-grade primitives, not a naive "orbit to an angle":
//!
//! 1. **Best view** by VIEWPOINT ENTROPY (Vázquez, Feixas, Sbert & Heidrich,
//!    "Viewpoint Selection using Viewpoint Entropy", VMV 2001): score a view by
//!    the Shannon entropy of its projected, occlusion-resolved face-area
//!    distribution `H = −Σ (A_i/A_t)·ln(A_i/A_t)` (background included as one
//!    region). Entropy peaks when many faces are visible with balanced area —
//!    the most informative single view.
//!
//! 2. **Next-best-view** by GREEDY SUBMODULAR COVERAGE MAXIMIZATION (Nemhauser–
//!    Wolsey–Fisher 1978; Krause & Guestrin, sensor placement): face-set
//!    coverage is monotone submodular, so repeatedly taking the view that adds
//!    the most NEW faces is a (1 − 1/e)-approximation of the minimum view set
//!    that sees the whole part — the classic active-vision NBV result
//!    (Connolly 1985; Scott et al. survey 2003).
//!
//! Candidate views are drawn from a **spherical Fibonacci lattice** (González,
//! "Measurement of areas on a sphere using Fibonacci…", 2010) for near-uniform
//! coverage with no pole clustering — unlike a naive lat/long grid.
//!
//! Visibility is occlusion-resolved: each candidate is rendered in `FaceIds`
//! mode (flat colors, z-buffered) and a face counts as seen only where it wins
//! a pixel. API-only: pure geometry/optimization, no learned model.

use super::{render_solid_dir, RenderMode, RenderOptions};
use crate::math::Vector3;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::TessellationParams;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Resolution of the off-screen visibility renders. Modest by design — many
/// candidates are scored, and face *coverage* needs far less resolution than a
/// presentation render.
const VIS_RES: usize = 220;

/// A scored candidate viewpoint. `dir` is camera→scene; `az`/`el` are the
/// camera-position spherical coords (world Z up) so an agent can re-request it.
#[derive(Debug, Clone, Serialize)]
pub struct ViewScore {
    pub az_deg: f64,
    pub el_deg: f64,
    pub dir: [f64; 3],
    pub entropy: f64,
    pub visible_faces: usize,
}

/// One step of the greedy next-best-view sequence.
#[derive(Debug, Clone, Serialize)]
pub struct NbvStep {
    pub az_deg: f64,
    pub el_deg: f64,
    pub dir: [f64; 3],
    /// Faces this view adds that no earlier view in the sequence saw.
    pub new_faces: usize,
    /// Faces covered by the sequence through this step (monotone non-decreasing).
    pub cumulative_faces: usize,
    pub cumulative_fraction: f64,
}

/// EYE-6 report: the single most-informative view plus a minimal ordered view
/// set that (greedily) covers every face.
#[derive(Debug, Clone, Serialize)]
pub struct ViewpointReport {
    pub total_faces: usize,
    pub candidates_evaluated: usize,
    /// Max-viewpoint-entropy view.
    pub best_view: ViewScore,
    /// Greedy submodular cover, ordered by marginal gain.
    pub nbv_sequence: Vec<NbvStep>,
    /// Whether the NBV sequence reaches 100% face coverage.
    pub nbv_covers_all: bool,
    pub method: &'static str,
}

/// Spherical Fibonacci lattice of `n` unit points (near-uniform, no poles).
fn fibonacci_sphere(n: usize) -> Vec<[f64; 3]> {
    // Golden angle.
    let ga = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / (n as f64);
            let r = (1.0 - y * y).max(0.0).sqrt();
            let theta = ga * i as f64;
            [r * theta.cos(), y, r * theta.sin()]
        })
        .collect()
}

/// Camera-position unit vector → (azimuth, elevation) in degrees, world Z up.
fn az_el(pos: [f64; 3]) -> (f64, f64) {
    let el = pos[2].clamp(-1.0, 1.0).asin().to_degrees();
    let az = pos[1].atan2(pos[0]).to_degrees();
    (az, el)
}

/// Up-hint orthogonal-ish to `dir`; switch axes near the poles to avoid a
/// degenerate (parallel) basis.
fn up_hint_for(dir: [f64; 3]) -> Vector3 {
    if dir[2].abs() > 0.999 {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    }
}

/// Render the part in `FaceIds` from `dir` and return (per-face visible pixel
/// counts, background pixel count, total face count from the legend).
fn visibility(
    model: &BRepModel,
    solid_id: SolidId,
    dir: [f64; 3],
    tess: &TessellationParams,
) -> Option<(HashMap<u32, usize>, usize, usize)> {
    let frame = render_solid_dir(
        model,
        solid_id,
        Vector3::new(dir[0], dir[1], dir[2]),
        up_hint_for(dir),
        &RenderOptions {
            width: VIS_RES,
            height: VIS_RES,
            view: super::CanonicalView::Isometric, // ignored by render_solid_dir
            mode: RenderMode::FaceIds,
            tessellation: tess.clone(),
        },
    )?;
    let by_color: HashMap<[u8; 3], u32> = frame.face_legend.iter().map(|&(f, c)| (c, f)).collect();
    let total_faces = frame.face_legend.len();
    let mut counts: HashMap<u32, usize> = HashMap::new();
    let mut bg = 0usize;
    for px in frame.pixels.chunks_exact(3) {
        if px == [255, 255, 255] {
            bg += 1;
            continue;
        }
        if let Some(&fid) = by_color.get(&[px[0], px[1], px[2]]) {
            *counts.entry(fid).or_insert(0) += 1;
        }
    }
    Some((counts, bg, total_faces))
}

/// Viewpoint entropy (Vázquez 2001) of an occlusion-resolved face histogram.
fn entropy(counts: &HashMap<u32, usize>, bg: usize) -> f64 {
    let total: usize = counts.values().sum::<usize>() + bg;
    if total == 0 {
        return 0.0;
    }
    let total = total as f64;
    let mut h = 0.0;
    for &c in counts.values() {
        if c > 0 {
            let p = c as f64 / total;
            h -= p * p.ln();
        }
    }
    if bg > 0 {
        let p = bg as f64 / total;
        h -= p * p.ln();
    }
    h
}

/// Public: viewpoint entropy for one direction (camera→scene). Used by callers
/// and pinned by tests (iso view ≫ axis-on view).
pub fn viewpoint_entropy(
    model: &BRepModel,
    solid_id: SolidId,
    dir: [f64; 3],
    tess: &TessellationParams,
) -> Option<f64> {
    let (counts, bg, _) = visibility(model, solid_id, dir, tess)?;
    Some(entropy(&counts, bg))
}

/// Full EYE-6 analysis: best view by entropy + greedy submodular NBV cover.
pub fn analyze_viewpoints(
    model: &BRepModel,
    solid_id: SolidId,
    n_candidates: usize,
    tess: &TessellationParams,
) -> Option<ViewpointReport> {
    let positions = fibonacci_sphere(n_candidates.max(8));

    // Score every candidate once: entropy + the set of faces it sees.
    struct Cand {
        pos: [f64; 3],
        dir: [f64; 3],
        entropy: f64,
        seen: BTreeSet<u32>,
    }
    let mut cands: Vec<Cand> = Vec::with_capacity(positions.len());
    let mut total_faces = 0usize;
    for pos in positions {
        let dir = [-pos[0], -pos[1], -pos[2]]; // camera at pos, looking at part
        let (counts, bg, tf) = match visibility(model, solid_id, dir, tess) {
            Some(v) => v,
            None => continue,
        };
        total_faces = total_faces.max(tf);
        let e = entropy(&counts, bg);
        let seen: BTreeSet<u32> = counts.keys().copied().collect();
        cands.push(Cand {
            pos,
            dir,
            entropy: e,
            seen,
        });
    }
    if cands.is_empty() || total_faces == 0 {
        return None;
    }

    // Best view = max entropy.
    let best = cands.iter().max_by(|a, b| {
        a.entropy
            .partial_cmp(&b.entropy)
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let (b_az, b_el) = az_el(best.pos);
    let best_view = ViewScore {
        az_deg: b_az,
        el_deg: b_el,
        dir: best.dir,
        entropy: best.entropy,
        visible_faces: best.seen.len(),
    };

    // Greedy submodular cover: repeatedly take the unused candidate adding the
    // most new faces. Monotone by construction; (1−1/e)-optimal.
    let mut covered: HashSet<u32> = HashSet::new();
    let mut used: HashSet<usize> = HashSet::new();
    let mut nbv_sequence: Vec<NbvStep> = Vec::new();
    loop {
        let mut best_idx: Option<usize> = None;
        let mut best_gain = 0usize;
        for (i, c) in cands.iter().enumerate() {
            if used.contains(&i) {
                continue;
            }
            let gain = c.seen.iter().filter(|f| !covered.contains(f)).count();
            if gain > best_gain {
                best_gain = gain;
                best_idx = Some(i);
            }
        }
        let idx = match best_idx {
            Some(i) if best_gain > 0 => i,
            _ => break,
        };
        used.insert(idx);
        let c = &cands[idx];
        for f in &c.seen {
            covered.insert(*f);
        }
        let (az, el) = az_el(c.pos);
        nbv_sequence.push(NbvStep {
            az_deg: az,
            el_deg: el,
            dir: c.dir,
            new_faces: best_gain,
            cumulative_faces: covered.len(),
            cumulative_fraction: covered.len() as f64 / total_faces as f64,
        });
        if covered.len() >= total_faces {
            break;
        }
    }

    Some(ViewpointReport {
        total_faces,
        candidates_evaluated: cands.len(),
        best_view,
        nbv_covers_all: covered.len() >= total_faces,
        nbv_sequence,
        method:
            "viewpoint-entropy (Vázquez 2001) + greedy submodular NBV (Nemhauser-Wolsey-Fisher)",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    fn box_solid(m: &mut BRepModel) -> SolidId {
        sid(TopologyBuilder::new(m)
            .create_box_3d(40.0, 40.0, 40.0)
            .expect("box"))
    }

    /// PROVABLE INVARIANT (a): greedy NBV coverage is monotone non-decreasing,
    /// and (b) it reaches 100% face coverage on a box — which the 4 fixed
    /// standard views (EYE-5) cannot. This is the active-vision payoff.
    #[test]
    fn nbv_is_monotone_and_covers_all_faces() {
        let mut m = BRepModel::new();
        let s = box_solid(&mut m);
        let report = analyze_viewpoints(&m, s, 64, &TessellationParams::default()).expect("report");

        assert_eq!(report.total_faces, 6, "box has 6 faces");
        assert!(!report.nbv_sequence.is_empty(), "NBV produced no views");

        // (a) monotone non-decreasing cumulative coverage; each step adds ≥1.
        let mut prev = 0usize;
        for step in &report.nbv_sequence {
            assert!(
                step.cumulative_faces >= prev,
                "coverage went backwards: {} < {prev}",
                step.cumulative_faces
            );
            assert!(step.new_faces >= 1, "a kept NBV step must add ≥1 face");
            prev = step.cumulative_faces;
        }

        // (b) full coverage achieved.
        assert!(
            report.nbv_covers_all && prev == 6,
            "NBV must cover all 6 box faces (got {prev}); covers_all={}",
            report.nbv_covers_all
        );
        // Submodular cover should be efficient: a box needs only a few views.
        assert!(
            report.nbv_sequence.len() <= 4,
            "expected ≤4 views to cover a box, got {}",
            report.nbv_sequence.len()
        );
    }

    /// Emit the entropy-best view for eyeballing (verify-by-looking).
    #[test]
    #[ignore = "writes a PNG for manual inspection"]
    fn emit_best_view_png() {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(60.0, 40.0, 20.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                crate::math::Point3::new(0.0, 0.0, -20.0),
                Vector3::new(0.0, 0.0, 1.0),
                8.0,
                80.0,
            )
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        let report =
            analyze_viewpoints(&m, part, 64, &TessellationParams::default()).expect("report");
        let d = report.best_view.dir;
        let frame = render_solid_dir(
            &m,
            part,
            Vector3::new(d[0], d[1], d[2]),
            up_hint_for(d),
            &RenderOptions {
                width: 512,
                height: 512,
                view: super::super::CanonicalView::Isometric,
                mode: RenderMode::Shaded,
                tessellation: TessellationParams::default(),
            },
        )
        .expect("render");
        std::fs::write("../_best_view.png", frame.to_png().expect("png")).expect("write");
        eprintln!(
            "best-view az={:.1} el={:.1} entropy={:.3} visible_faces={} | NBV steps={} covers_all={}",
            report.best_view.az_deg,
            report.best_view.el_deg,
            report.best_view.entropy,
            report.best_view.visible_faces,
            report.nbv_sequence.len(),
            report.nbv_covers_all
        );
    }

    /// PROVABLE INVARIANT (c): viewpoint entropy is higher for a balanced 3/4
    /// (isometric) view than a degenerate axis-on (front) view, where only one
    /// face dominates. This is the property that makes entropy a good
    /// best-view objective (Vázquez 2001).
    #[test]
    fn iso_view_has_higher_entropy_than_axis_on() {
        let mut m = BRepModel::new();
        let s = box_solid(&mut m);
        let tess = TessellationParams::default();

        let inv = 1.0 / 3.0_f64.sqrt();
        let h_iso = viewpoint_entropy(&m, s, [-inv, -inv, -inv], &tess).expect("iso");
        let h_front = viewpoint_entropy(&m, s, [0.0, -1.0, 0.0], &tess).expect("front");

        assert!(
            h_iso > h_front,
            "iso entropy {h_iso} must exceed axis-on entropy {h_front}"
        );
    }
}
