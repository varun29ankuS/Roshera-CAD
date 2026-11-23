//! A linear static solve through CalculiX: the input deck from a mesh and a
//! study, and the results read back.
//!
//! Deck: 10-node tetrahedra (C3D10; Gmsh's last two mid-side nodes swapped
//! into CalculiX's order), one isotropic elastic material, the support
//! faces' nodes held in all three directions (`*BOUNDARY`), each load as its
//! consistent nodal forces (`*CLOAD`, [`crate::loads::nodal_forces`]), and
//! the reaction total over the held nodes printed (`*NODE PRINT ... TOTALS=
//! ONLY`) so the report can check that the supports carry exactly the load.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::Duration;

use geometry_engine::primitives::face::FaceId;

use crate::engines::{run, EngineError, EngineInfo};
use crate::loads::{nodal_forces, LoadError, NodalLoads, Spread, Tri6};
use crate::mesh::VolumeMesh;

/// Gmsh 10-node tetrahedron -> CalculiX C3D10 (Gmsh: 9 = edge 2-3 (0-based),
/// 10 = edge 1-3; CalculiX: 9 = 1-3, 10 = 2-3).
const GMSH_TO_CCX_TET10: [usize; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 9, 8];

/// A load as the solver gets it: faces, total force, spread.
pub struct DeckLoad<'a> {
    pub faces: &'a [FaceId],
    pub force_n: [f64; 3],
    pub spread: Spread,
}

/// Results of one solve.
#[derive(Debug, Clone, Default)]
pub struct Solution {
    pub displacement: BTreeMap<u64, [f64; 3]>,
    /// Nodal stress (sxx, syy, szz, sxy, syz, szx), MPa.
    pub stress: BTreeMap<u64, [f64; 6]>,
    /// Sum of the reaction forces over the held nodes, N.
    pub reaction_total: [f64; 3],
    /// The applied load that lands on held nodes, N: it goes straight into
    /// the support, and the solver's reaction total does not count it, so a
    /// balance check adds it back (else a loaded face touching a held one
    /// reads as a fraction of a percent out of balance).
    pub load_on_held: [f64; 3],
    pub solver_seconds: f64,
}

/// Why a solve could not be done.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SolveError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error("no mesh node lies on the held faces")]
    NothingHeld,
    #[error("the solver deck could not be written: {0}")]
    Write(String),
    #[error("the solver's results could not be read: {0}")]
    Read(String),
}

/// Write the deck for `mesh`, solve it in `workdir`, and read the results.
pub fn solve_static(
    mesh: &VolumeMesh,
    held_faces: &BTreeSet<FaceId>,
    loads: &[DeckLoad<'_>],
    youngs_mpa: f64,
    poisson: f64,
    solver: &EngineInfo,
    workdir: &Path,
    limit: Duration,
) -> Result<Solution, SolveError> {
    let (deck, load_on_held) = deck(mesh, held_faces, loads, youngs_mpa, poisson)?;
    std::fs::write(workdir.join("job.inp"), deck).map_err(|e| SolveError::Write(e.to_string()))?;
    let took = run(
        solver,
        &["-i", "job"],
        workdir,
        &workdir.join("solver.log"),
        limit,
    )?;
    let frd = std::fs::read_to_string(workdir.join("job.frd"))
        .map_err(|e| SolveError::Read(e.to_string()))?;
    let dat = std::fs::read_to_string(workdir.join("job.dat"))
        .map_err(|e| SolveError::Read(e.to_string()))?;
    let (displacement, stress) = read_frd(&frd);
    if displacement.is_empty() || stress.is_empty() {
        return Err(SolveError::Read(
            "no displacement or stress block".to_string(),
        ));
    }
    Ok(Solution {
        displacement,
        stress,
        reaction_total: read_reaction_total(&dat)
            .ok_or_else(|| SolveError::Read("no reaction total".to_string()))?,
        load_on_held,
        solver_seconds: took.as_secs_f64(),
    })
}

/// The 6-node boundary triangles on `faces`, with outward normals (away from
/// the tetrahedron they bound).
pub(crate) fn tri6_on(mesh: &VolumeMesh, faces: &[FaceId]) -> Vec<Tri6> {
    // Each tet face (sorted corners) -> the tet's opposite corner.
    let mut opposite: HashMap<[u64; 3], u64> = HashMap::new();
    for t in &mesh.tets {
        for (skip, keep) in [
            (3, [0, 1, 2]),
            (2, [0, 1, 3]),
            (1, [0, 2, 3]),
            (0, [1, 2, 3]),
        ] {
            let mut key = [t[keep[0]], t[keep[1]], t[keep[2]]];
            key.sort_unstable();
            opposite.insert(key, t[skip]);
        }
    }
    let p = |i: u64| mesh.nodes.get(&i).copied().unwrap_or([0.0; 3]);
    mesh.boundary
        .iter()
        .filter(|b| faces.contains(&b.face))
        .map(|b| {
            let corners = [p(b.nodes[0]), p(b.nodes[1]), p(b.nodes[2])];
            let u = sub(corners[1], corners[0]);
            let v = sub(corners[2], corners[0]);
            let mut n = cross(u, v);
            let mut key = [b.nodes[0], b.nodes[1], b.nodes[2]];
            key.sort_unstable();
            if let Some(&inner) = opposite.get(&key) {
                if dot(n, sub(p(inner), corners[0])) > 0.0 {
                    n = [-n[0], -n[1], -n[2]];
                }
            }
            Tri6 {
                nodes: b.nodes,
                corners,
                normal: n,
            }
        })
        .collect()
}

fn deck(
    mesh: &VolumeMesh,
    held_faces: &BTreeSet<FaceId>,
    loads: &[DeckLoad<'_>],
    youngs_mpa: f64,
    poisson: f64,
) -> Result<(String, [f64; 3]), SolveError> {
    use std::fmt::Write as _;
    let held: BTreeSet<u64> = mesh
        .boundary
        .iter()
        .filter(|b| held_faces.contains(&b.face))
        .flat_map(|b| b.nodes)
        .collect();
    if held.is_empty() {
        return Err(SolveError::NothingHeld);
    }
    let mut cload = NodalLoads::default();
    for l in loads {
        let tris = tri6_on(mesh, l.faces);
        for (node, f) in nodal_forces(&tris, l.force_n, l.spread)?.0 {
            let e = cload.0.entry(node).or_insert([0.0; 3]);
            for i in 0..3 {
                e[i] += f[i];
            }
        }
    }
    let load_on_held = cload
        .0
        .iter()
        .filter(|(n, _)| held.contains(n))
        .fold([0.0; 3], |t, (_, f)| {
            [t[0] + f[0], t[1] + f[1], t[2] + f[2]]
        });
    let mut s = String::new();
    let w = |e: std::fmt::Error| SolveError::Write(e.to_string());
    writeln!(s, "*HEADING\nRoshera static study\n*NODE, NSET=NALL").map_err(w)?;
    for (id, p) in &mesh.nodes {
        writeln!(s, "{id}, {:.12e}, {:.12e}, {:.12e}", p[0], p[1], p[2]).map_err(w)?;
    }
    writeln!(s, "*ELEMENT, TYPE=C3D10, ELSET=EALL").map_err(w)?;
    for (k, t) in mesh.tets.iter().enumerate() {
        let ids: Vec<String> = GMSH_TO_CCX_TET10
            .iter()
            .map(|&j| t[j].to_string())
            .collect();
        writeln!(s, "{}, {}", k + 1, ids.join(", ")).map_err(w)?;
    }
    writeln!(s, "*NSET, NSET=HELD").map_err(w)?;
    let held: Vec<String> = held.iter().map(u64::to_string).collect();
    for chunk in held.chunks(12) {
        writeln!(s, "{}", chunk.join(", ")).map_err(w)?;
    }
    writeln!(
        s,
        "*MATERIAL, NAME=M\n*ELASTIC\n{youngs_mpa:.9e}, {poisson:.9e}\n*SOLID SECTION, ELSET=EALL, MATERIAL=M\n*STEP\n*STATIC\n*BOUNDARY\nHELD, 1, 3, 0.0\n*CLOAD"
    )
    .map_err(w)?;
    for (node, f) in &cload.0 {
        for (dof, v) in f.iter().enumerate() {
            if *v != 0.0 {
                writeln!(s, "{node}, {}, {v:.12e}", dof + 1).map_err(w)?;
            }
        }
    }
    writeln!(
        s,
        "*NODE FILE\nU\n*EL FILE\nS\n*NODE PRINT, NSET=HELD, TOTALS=ONLY\nRF\n*END STEP"
    )
    .map_err(w)?;
    Ok((s, load_on_held))
}

/// Nodal DISP and STRESS blocks of a CalculiX `.frd` (ASCII results).
pub(crate) fn read_frd(text: &str) -> (BTreeMap<u64, [f64; 3]>, BTreeMap<u64, [f64; 6]>) {
    let mut disp = BTreeMap::new();
    let mut stress = BTreeMap::new();
    #[derive(PartialEq)]
    enum Block {
        None,
        Disp,
        Stress,
    }
    let mut block = Block::None;
    for line in text.lines() {
        if line.starts_with(" -4") {
            let name = line.get(5..13).unwrap_or("").trim();
            block = match name {
                "DISP" => Block::Disp,
                "STRESS" => Block::Stress,
                _ => Block::None,
            };
        } else if line.starts_with(" -3") {
            block = Block::None;
        } else if block != Block::None && line.starts_with(" -1") {
            let Some(id) = line.get(3..13).and_then(|s| s.trim().parse::<u64>().ok()) else {
                continue;
            };
            let body = line.get(13..).unwrap_or("");
            let vals: Vec<f64> = (0..body.len() / 12)
                .filter_map(|k| body.get(12 * k..12 * k + 12))
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            match block {
                Block::Disp if vals.len() >= 3 => {
                    disp.insert(id, [vals[0], vals[1], vals[2]]);
                }
                Block::Stress if vals.len() >= 6 => {
                    stress.insert(id, [vals[0], vals[1], vals[2], vals[3], vals[4], vals[5]]);
                }
                _ => {}
            }
        }
    }
    (disp, stress)
}

/// The reaction total CalculiX prints for a set (`total force (fx,fy,fz)
/// for set HELD ...`, then a line of three numbers).
pub(crate) fn read_reaction_total(text: &str) -> Option<[f64; 3]> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.to_ascii_lowercase().contains("total force") {
            for next in lines.by_ref() {
                let vals: Vec<f64> = next
                    .split_whitespace()
                    .filter_map(|x| x.parse().ok())
                    .collect();
                if vals.len() >= 3 {
                    return Some([vals[0], vals[1], vals[2]]);
                }
            }
        }
    }
    None
}

/// Von Mises equivalent stress, MPa.
pub fn von_mises(s: &[f64; 6]) -> f64 {
    let [sxx, syy, szz, sxy, syz, szx] = *s;
    (0.5 * ((sxx - syy).powi(2) + (syy - szz).powi(2) + (szz - sxx).powi(2))
        + 3.0 * (sxy * sxy + syz * syz + szx * szx))
        .sqrt()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The `.frd` reader takes nodal DISP and STRESS rows (fixed-width:
    /// node id in columns 4-13, values 12 wide) and nothing else.
    #[test]
    fn frd_reader_takes_disp_and_stress_rows() {
        let text = "    1C\n -4  DISP        4    1\n -1         7 1.00000E-01-2.50000E-02 3.00000E+00\n -3\n -4  STRESS      6    1\n -1         7 1.00000E+01 2.00000E+00 0.00000E+00 1.00000E+00 0.00000E+00 0.00000E+00\n -3\n -4  OTHER       1    1\n -1         7 9.90000E+01\n -3\n";
        let (d, s) = read_frd(text);
        assert_eq!(d[&7], [0.1, -0.025, 3.0]);
        assert_eq!(s[&7], [10.0, 2.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(d.len(), 1);
    }

    /// The reaction total is the line of three numbers after CalculiX's
    /// "total force" heading.
    #[test]
    fn reaction_total_is_read_from_the_dat_file() {
        let text = "\n total force (fx,fy,fz) for set HELD and time  0.1000000E+01\n\n       -1.000000E-10 -2.000000E+03  4.500000E+01\n";
        assert_eq!(read_reaction_total(text), Some([-1.0e-10, -2000.0, 45.0]));
    }

    /// Von Mises: uniaxial stress is itself; pure shear tau is sqrt(3) tau.
    #[test]
    fn von_mises_of_simple_states() {
        assert!((von_mises(&[100.0, 0.0, 0.0, 0.0, 0.0, 0.0]) - 100.0).abs() < 1e-12);
        assert!((von_mises(&[0.0, 0.0, 0.0, 10.0, 0.0, 0.0]) - 10.0 * 3f64.sqrt()).abs() < 1e-12);
    }
}
