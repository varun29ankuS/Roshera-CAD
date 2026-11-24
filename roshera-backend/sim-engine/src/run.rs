//! Running a static study end to end: check the set-up, mesh and solve it
//! twice (coarse and fine), and give a verdict only the evidence supports.
//!
//! The verdict is refused -- with its reason -- rather than guessed: when
//! the part can still move, when the supports do not carry the load, when
//! the two meshes disagree (a peak that keeps rising is a sharp corner, not
//! a real stress), or when no yield strength with a source was given.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde::{Deserialize, Serialize};

use crate::ccx::{solve_static, von_mises, DeckHold, DeckLoad, DeckSupport, SolveError};
use crate::engines::Engines;
use crate::field::{surface_field, SurfaceField};
use crate::mesh::{mesh_solid_graded, MeshError, MeshOptions, MeshStats, Refine, SizeBox};
use crate::rigid::{free_motions, FreeMotion, HeldPoint, Holds};
use crate::study::{MeshControl, Study, SupportKind, FINE_RATIO};

/// Time each engine run may take.
pub(crate) const ENGINE_LIMIT: Duration = Duration::from_secs(600);
/// Reactions must balance the applied load to this share.
const BALANCE_TOL: f64 = 0.01;
/// At most this many refinements at the peak after the two approved meshes.
const MAX_REFINEMENTS: usize = 3;
/// No refinement whose mesh would pass this many nodes (about twice the last):
/// a laptop solves it in seconds to a minute, without running out of memory.
const NODE_BUDGET: usize = 150_000;

/// How the refinements read in a check's detail.
fn refined_note(refined: &[RunResult]) -> String {
    match refined.len() {
        0 => String::new(),
        n => format!(
            " (after {n} refinement{} at the peak, {} nodes)",
            if n == 1 { "" } else { "s" },
            refined[n - 1].mesh.nodes
        ),
    }
}

/// One run's numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub mesh: MeshStats,
    pub max_displacement_mm: f64,
    pub peak_von_mises_mpa: f64,
    /// Where the peak is (model coordinates, mm).
    pub peak_at: [f64; 3],
    pub reaction_total_n: [f64; 3],
    /// Load applied on held nodes (straight into the support), N.
    pub load_on_held_n: [f64; 3],
    pub solver_seconds: f64,
}

/// A named check and whether it passed, in words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// The answer to the study's question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// It meets the goal.
    Holds { safety_factor: f64, required: f64 },
    /// It does not.
    Fails { safety_factor: f64, required: f64 },
    /// The evidence does not support an answer; why.
    Undecided { reason: String },
}

/// Everything a static study produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticReport {
    pub verdict: Verdict,
    pub checks: Vec<Check>,
    pub coarse: RunResult,
    pub fine: RunResult,
    pub engines: Engines,
    /// The persistent ids of the faces the study names (hex), for replay.
    pub face_pids: Vec<(u32, String)>,
    pub assumptions: Vec<String>,
    /// The last run's result on the part's surface, for the viewport.
    pub field: SurfaceField,
    /// Runs refined at the peak after the two approved meshes, in order
    /// (empty when the peak settled on them): the verdict reads the last.
    #[serde(default)]
    pub refined: Vec<RunResult>,
}

/// Why a study was not run at all.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StudyError {
    #[error("solid {0} is not in the model")]
    UnknownSolid(SolidId),
    #[error("face {0} is not a face of this part")]
    UnknownFace(u32),
    #[error("the study names no support")]
    NoSupport,
    #[error("the study names no load")]
    NoLoad,
    #[error("the material is not physical: {0}")]
    Material(String),
    #[error("the part is not held: it {}", .0.iter().map(FreeMotion::describe).collect::<Vec<_>>().join("; "))]
    CanMove(Vec<FreeMotion>),
    #[error("the mesh size must be above 0 mm, not {0}")]
    MeshSize(f64),
    #[error(transparent)]
    Mesh(#[from] MeshError),
    #[error(transparent)]
    Solve(#[from] SolveError),
}

/// The set-up checks a study must pass before anything is meshed: a
/// support and a load, a physical material, every named face on this part,
/// nothing left free to move, and only supports this solver can model. The
/// study run and the set-up stage both call this, so a set-up accepted on
/// screen can never be refused by the run for a reason the screen did not
/// show.
pub fn check_setup(model: &BRepModel, solid: SolidId, study: &Study) -> Result<(), StudyError> {
    if study.supports.is_empty() {
        return Err(StudyError::NoSupport);
    }
    if study.loads.is_empty() {
        return Err(StudyError::NoLoad);
    }
    let m = &study.material;
    if !(m.youngs_modulus_mpa > 0.0) || !(m.poisson_ratio > -1.0 && m.poisson_ratio < 0.5) {
        return Err(StudyError::Material(format!(
            "E = {} MPa, nu = {}",
            m.youngs_modulus_mpa, m.poisson_ratio
        )));
    }
    let surface = tessellate_solid(
        model
            .solids
            .get(solid)
            .ok_or(StudyError::UnknownSolid(solid))?,
        model,
        &TessellationParams::default(),
    );
    let part_faces: BTreeSet<FaceId> = surface.face_map.iter().copied().collect();
    let named: Vec<u32> = study
        .supports
        .iter()
        .flat_map(|s| s.faces.iter())
        .chain(study.loads.iter().flat_map(|l| l.faces.iter()))
        .copied()
        .collect();
    for f in &named {
        if !part_faces.contains(f) {
            return Err(StudyError::UnknownFace(*f));
        }
    }
    // Can the part still move? Every triangle of a held face holds its
    // centroid, fully or along its normal.
    let pos = |i: u32| {
        let p = surface.vertices[i as usize].position;
        [p.x, p.y, p.z]
    };
    let mut held_points = Vec::new();
    for s in &study.supports {
        let holds = match s.kind {
            SupportKind::Fixed | SupportKind::BoltClamp => Holds::Fully,
            SupportKind::Pin | SupportKind::Slider => Holds::Normal,
        };
        for (t, f) in surface.triangles.iter().zip(&surface.face_map) {
            if s.faces.contains(f) {
                let (a, b, c) = (pos(t[0]), pos(t[1]), pos(t[2]));
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let normal = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                // The corners AND the centroid: a face of two triangles has
                // only two centroids, on one diagonal, and holding just
                // those left the part free to tip about that diagonal.
                let centroid = [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ];
                for point in [a, b, c, centroid] {
                    held_points.push(HeldPoint {
                        point,
                        normal,
                        holds,
                    });
                }
            }
        }
    }
    let free = free_motions(&held_points);
    if !free.is_empty() {
        return Err(StudyError::CanMove(free));
    }
    Ok(())
}

/// The coarse mesh's largest element: the size asked, or a twentieth of the
/// part's largest extent. The mesh stage and the run both use this, so the
/// mesh a person approves is the mesh that is solved.
pub fn coarse_mesh_size(
    model: &BRepModel,
    solid: SolidId,
    asked: Option<f64>,
) -> Result<f64, StudyError> {
    if let Some(size) = asked {
        if !(size > 0.0) {
            return Err(StudyError::MeshSize(size));
        }
        return Ok(size);
    }
    let surface = tessellate_solid(
        model
            .solids
            .get(solid)
            .ok_or(StudyError::UnknownSolid(solid))?,
        model,
        &TessellationParams::default(),
    );
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &surface.vertices {
        let p = [v.position.x, v.position.y, v.position.z];
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let extent = (0..3).map(|i| hi[i] - lo[i]).fold(0.0_f64, f64::max);
    if !(extent > 0.0) {
        return Err(StudyError::UnknownSolid(solid));
    }
    Ok(extent / 20.0)
}

/// The mesher's grading for `control` on `solid`: curvature, and a size box
/// around each face given a local size (the face's own extent, reaching
/// back to the general `size` over one general element). Refused when a
/// local size names a face not on this part or is not above 0.
pub fn mesh_options(
    model: &BRepModel,
    solid: SolidId,
    control: &MeshControl,
    size: f64,
) -> Result<MeshOptions, StudyError> {
    let mut boxes = Vec::new();
    if !control.local.is_empty() {
        let surface = tessellate_solid(
            model
                .solids
                .get(solid)
                .ok_or(StudyError::UnknownSolid(solid))?,
            model,
            &TessellationParams::default(),
        );
        for local in &control.local {
            if !(local.size_mm > 0.0) {
                return Err(StudyError::MeshSize(local.size_mm));
            }
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for &f in &local.faces {
                if !surface.face_map.contains(&f) {
                    return Err(StudyError::UnknownFace(f));
                }
            }
            for (t, f) in surface.triangles.iter().zip(&surface.face_map) {
                if !local.faces.contains(f) {
                    continue;
                }
                for &i in t {
                    let p = surface.vertices[i as usize].position;
                    for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                        lo[k] = lo[k].min(c);
                        hi[k] = hi[k].max(c);
                    }
                }
            }
            if lo[0].is_finite() {
                // A flat face's box has no thickness: give it the local size.
                let pad = local.size_mm;
                boxes.push(SizeBox {
                    lo: [lo[0] - pad, lo[1] - pad, lo[2] - pad],
                    hi: [hi[0] + pad, hi[1] + pad, hi[2] + pad],
                    size: local.size_mm,
                    reach: size,
                });
            }
        }
    }
    Ok(MeshOptions {
        curve_elements: control.curve_elements,
        boxes,
        ball: None,
    })
}

/// Run `study` on `solid`, its engine files in `workdir`.
pub fn run_static(
    model: &BRepModel,
    solid: SolidId,
    study: &Study,
    engines: &Engines,
    workdir: &Path,
) -> Result<StaticReport, StudyError> {
    check_setup(model, solid, study)?;
    let m = &study.material;

    // ── Mesh and solve, coarse then fine ──────────────────────────────────
    let coarse_size = coarse_mesh_size(model, solid, study.mesh.size_mm)?;
    // Fixed and bolted faces are held in every direction; a slider's face
    // along its normal, a pin's hole wall radially (along its normal there).
    let deck_supports: Vec<DeckSupport<'_>> = study
        .supports
        .iter()
        .map(|s| DeckSupport {
            faces: &s.faces,
            hold: match s.kind {
                SupportKind::Fixed | SupportKind::BoltClamp => DeckHold::All,
                SupportKind::Pin | SupportKind::Slider => DeckHold::AlongNormal,
            },
        })
        .collect();
    let applied: [f64; 3] = study.loads.iter().fold([0.0; 3], |t, l| {
        [
            t[0] + l.force_n[0],
            t[1] + l.force_n[1],
            t[2] + l.force_n[2],
        ]
    });
    let graded = mesh_options(model, solid, &study.mesh, coarse_size)?;
    let solve_at = |size: f64,
                    refine: Option<&Refine>,
                    label: &str|
     -> Result<(RunResult, SurfaceField), StudyError> {
        let dir = workdir.join(label);
        let options = MeshOptions {
            ball: refine.copied(),
            ..graded.clone()
        };
        let (mesh, stats) = mesh_solid_graded(
            model,
            solid,
            size,
            &options,
            &engines.mesher,
            &dir,
            ENGINE_LIMIT,
        )?;
        let loads: Vec<DeckLoad<'_>> = study
            .loads
            .iter()
            .map(|l| DeckLoad {
                faces: &l.faces,
                force_n: l.force_n,
                spread: l.spread,
            })
            .collect();
        let sol = solve_static(
            &mesh,
            &deck_supports,
            &loads,
            m.youngs_modulus_mpa,
            m.poisson_ratio,
            &engines.solver,
            &dir,
            ENGINE_LIMIT,
        )?;
        let max_displacement_mm = sol
            .displacement
            .values()
            .map(|d| (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
            .fold(0.0_f64, f64::max);
        let (peak_node, peak) = sol
            .stress
            .iter()
            .map(|(n, s)| (*n, von_mises(s)))
            .fold((0, 0.0_f64), |a, b| if b.1 > a.1 { b } else { a });
        let field = surface_field(&mesh, &sol);
        Ok((
            RunResult {
                mesh: stats,
                max_displacement_mm,
                peak_von_mises_mpa: peak,
                peak_at: mesh.nodes.get(&peak_node).copied().unwrap_or([0.0; 3]),
                reaction_total_n: sol.reaction_total,
                load_on_held_n: sol.load_on_held,
                solver_seconds: sol.solver_seconds,
            },
            field,
        ))
    };
    let (coarse, _) = solve_at(coarse_size, None, "coarse")?;
    let (fine, mut field) = solve_at(coarse_size * FINE_RATIO, None, "fine")?;

    // ── Refine at the peak until it settles ───────────────────────────────
    // A peak that moves between the two meshes is either a mesh too coarse
    // where the stress gathers (a fillet, a hole edge) or a sharp corner,
    // where the stress has no limit. Refine in a ball around the peak and
    // solve again: a real peak settles; a sharp corner keeps climbing.
    let rel = |a: f64, b: f64| (a - b).abs() / b.abs().max(1e-300);
    let fine_size = coarse_size * FINE_RATIO;
    let mut refined: Vec<RunResult> = Vec::new();
    let mut inside = fine_size;
    let max_refinements = if study.mesh.refine_at_peak.unwrap_or(true) {
        study.mesh.max_refinements.unwrap_or(MAX_REFINEMENTS)
    } else {
        0
    };
    let node_budget = study.mesh.node_budget.unwrap_or(NODE_BUDGET);
    let (stress_tol, displacement_tol) = (study.accuracy.stress, study.accuracy.displacement);
    for level in 1..=max_refinements {
        let (prev, last) = match refined.len() {
            0 => (&coarse, &fine),
            1 => (&fine, &refined[0]),
            n => (&refined[n - 2], &refined[n - 1]),
        };
        if rel(prev.peak_von_mises_mpa, last.peak_von_mises_mpa) <= stress_tol {
            break;
        }
        if last.mesh.nodes.saturating_mul(2) > node_budget {
            break;
        }
        inside *= FINE_RATIO;
        let ball = Refine {
            center: last.peak_at,
            radius: 3.0 * fine_size,
            size: inside,
        };
        let (run, f) = solve_at(fine_size, Some(&ball), &format!("refine{level}"))?;
        refined.push(run);
        field = f;
    }
    // The pair the convergence is read from: the last two runs.
    let (before, last) = match refined.len() {
        0 => (&coarse, &fine),
        1 => (&fine, &refined[0]),
        n => (&refined[n - 2], &refined[n - 1]),
    };

    // ── Checks and verdict ────────────────────────────────────────────────
    let mut checks = vec![Check {
        name: "held".to_string(),
        passed: true,
        detail: "the supports hold all six motions".to_string(),
    }];
    let applied_mag = norm(applied);
    // The supports carry the load: the solver's reactions plus the load that
    // lands directly on held nodes balance what was applied.
    let carried = [
        fine.reaction_total_n[0] - fine.load_on_held_n[0],
        fine.reaction_total_n[1] - fine.load_on_held_n[1],
        fine.reaction_total_n[2] - fine.load_on_held_n[2],
    ];
    let imbalance = norm([
        carried[0] + applied[0],
        carried[1] + applied[1],
        carried[2] + applied[2],
    ]) / applied_mag.max(1e-300);
    let balanced = imbalance <= BALANCE_TOL;
    checks.push(Check {
        name: "forces balance".to_string(),
        passed: balanced,
        detail: format!(
            "reactions {:.1} N against loads {:.1} N ({:.2}% apart)",
            norm(carried),
            applied_mag,
            100.0 * imbalance
        ),
    });
    let d_agree = rel(coarse.max_displacement_mm, fine.max_displacement_mm) <= displacement_tol;
    checks.push(Check {
        name: "displacement converged".to_string(),
        passed: d_agree,
        detail: format!(
            "{:.5} mm vs {:.5} mm on the two meshes",
            coarse.max_displacement_mm, fine.max_displacement_mm
        ),
    });
    let s_agree = rel(before.peak_von_mises_mpa, last.peak_von_mises_mpa) <= stress_tol;
    checks.push(Check {
        name: "peak stress converged".to_string(),
        passed: s_agree,
        detail: if s_agree {
            format!(
                "{:.1} MPa vs {:.1} MPa on the last two meshes{}",
                before.peak_von_mises_mpa,
                last.peak_von_mises_mpa,
                refined_note(&refined)
            )
        } else {
            format!(
                "{:.1} MPa then {:.1} MPa: it still rises with the mesh{} -- a sharp corner, where the stress has no limit (round it), or more refinement than this machine's budget",
                before.peak_von_mises_mpa,
                last.peak_von_mises_mpa,
                refined_note(&refined)
            )
        },
    });
    checks.push(Check {
        name: "on the part's surface".to_string(),
        passed: true,
        detail: format!(
            "boundary within {:.4} mm of the part (fine mesh)",
            fine.mesh.boundary_deviation_mm
        ),
    });
    let verdict = if !balanced {
        Verdict::Undecided {
            reason: "the supports do not carry the load".to_string(),
        }
    } else if !s_agree {
        Verdict::Undecided {
            reason: "the peak stress did not converge between the two meshes".to_string(),
        }
    } else {
        match (m.yield_strength_mpa, m.source.as_deref()) {
            (Some(y), Some(src)) if y > 0.0 && !src.trim().is_empty() => {
                let sf = y / last.peak_von_mises_mpa.max(1e-300);
                if sf >= study.goal.min_safety_factor {
                    Verdict::Holds {
                        safety_factor: sf,
                        required: study.goal.min_safety_factor,
                    }
                } else {
                    Verdict::Fails {
                        safety_factor: sf,
                        required: study.goal.min_safety_factor,
                    }
                }
            }
            _ => Verdict::Undecided {
                reason: "no yield strength with a source was given".to_string(),
            },
        }
    };
    let face_pids = study
        .supports
        .iter()
        .flat_map(|s| s.faces.iter())
        .chain(study.loads.iter().flat_map(|l| l.faces.iter()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|&f| {
            (
                f,
                model
                    .face_pid(f)
                    .map(|p| format!("{:032x}", p.as_u128()))
                    .unwrap_or_default(),
            )
        })
        .collect();
    let mut assumptions = vec![
        "linear elastic, small deflections".to_string(),
        "fixed and bolted faces held in every direction; slider faces along their normal only; pin holes radially only (free to turn and slide on the pin)".to_string(),
        format!(
            "mesher {}{}",
            engines.mesher.name,
            if engines.mesher.interim {
                " (interim)"
            } else {
                ""
            }
        ),
    ];
    // A safety factor on a TYPICAL yield is indicative only: say so, every
    // time, beside the number it qualifies.
    if m.yield_basis == Some(crate::materials::YieldBasis::Typical) {
        assumptions.push(
            "the yield strength is a TYPICAL value, not a specified minimum: the safety factor is indicative, not a design check"
                .to_string(),
        );
    }
    Ok(StaticReport {
        verdict,
        checks,
        coarse,
        fine,
        engines: engines.clone(),
        face_pids,
        assumptions,
        field,
        refined,
    })
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
