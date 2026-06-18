//! PILLAR 3 — reference-by-description selection (the moat). The agent names a
//! face by MEANING ("the largest +Z planar face") and the kernel resolves it to
//! a concrete `FaceId` — or REFUSES (`NotFound` / `Ambiguous`) rather than
//! guessing. Refusing on ambiguity is the whole point: a parametric edit that
//! made two faces equally match must NOT silently resolve to the wrong one.
//!
//! Built on the existing face accessors (surface kind, outward normal, area,
//! centroid). No new geometry — just an honest, deterministic resolver.

use crate::math::Vector3;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Surface-kind filter (matches `Surface::type_name()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Any,
    Planar,
    Cylindrical,
    Spherical,
    Conical,
    Toroidal,
    Nurbs,
}

impl SurfaceKind {
    fn matches(self, type_name: &str) -> bool {
        match self {
            SurfaceKind::Any => true,
            SurfaceKind::Planar => type_name == "Plane",
            SurfaceKind::Cylindrical => type_name == "Cylinder",
            SurfaceKind::Spherical => type_name == "Sphere",
            SurfaceKind::Conical => type_name == "Cone",
            SurfaceKind::Toroidal => type_name == "Torus",
            SurfaceKind::Nurbs => type_name == "NurbsSurface",
        }
    }
}

/// How to pick among multiple matches. `None` means "there must be exactly one"
/// (else `Ambiguous`); the extremal variants rank the matches and pick the top —
/// but a near-tie at the top is itself `Ambiguous` (the kernel won't guess).
#[derive(Debug, Clone, Copy)]
pub enum Extremal {
    None,
    LargestArea,
    SmallestArea,
    /// The face whose centroid is farthest along `dir` (e.g. +Z = "topmost").
    MostAlong(Vector3),
}

/// A descriptive face reference.
#[derive(Debug, Clone)]
pub struct FaceQuery {
    pub kind: SurfaceKind,
    /// Require the face's outward normal to align with this direction.
    pub normal_dir: Option<Vector3>,
    /// Half-angle tolerance (degrees) for the normal-direction match.
    pub angle_tol_deg: f64,
    pub extremal: Extremal,
}

impl FaceQuery {
    pub fn new(kind: SurfaceKind) -> Self {
        Self {
            kind,
            normal_dir: None,
            angle_tol_deg: 12.0,
            extremal: Extremal::None,
        }
    }
    pub fn facing(mut self, dir: Vector3) -> Self {
        self.normal_dir = Some(dir);
        self
    }
    pub fn extremal(mut self, e: Extremal) -> Self {
        self.extremal = e;
        self
    }
}

/// Why a descriptive reference did not resolve to a single face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// No face matched the description.
    NotFound,
    /// Several faces matched equally well — the kernel refuses to guess which.
    Ambiguous(Vec<FaceId>),
}

/// Outward normal of a face at its parametric midpoint (constant for planes;
/// representative for the kinds we filter on).
fn face_outward_normal(model: &BRepModel, fid: FaceId) -> Option<Vector3> {
    let face = model.faces.get(fid)?;
    let b = face.uv_bounds;
    let (u, v) = (0.5 * (b[0] + b[1]), 0.5 * (b[2] + b[3]));
    face.normal_at(u, v, &model.surfaces).ok()
}

fn face_kind_name(model: &BRepModel, fid: FaceId) -> Option<&'static str> {
    let face = model.faces.get(fid)?;
    model.surfaces.get(face.surface_id).map(|s| s.type_name())
}

fn solid_face_ids(model: &BRepModel, solid: SolidId) -> Vec<FaceId> {
    let mut faces = Vec::new();
    if let Some(s) = model.solids.get(solid) {
        for sh in s.shell_ids() {
            if let Some(shell) = model.shells.get(sh) {
                faces.extend_from_slice(&shell.faces);
            }
        }
    }
    faces
}

/// Resolve a descriptive face reference to a single `FaceId`, or refuse.
pub fn resolve_face(
    model: &mut BRepModel,
    solid: SolidId,
    q: &FaceQuery,
) -> Result<FaceId, SelectError> {
    let cos_tol = q.angle_tol_deg.to_radians().cos();
    let dir = q.normal_dir.and_then(|d| d.normalize().ok());

    // Filter by surface kind + normal direction (both immutable).
    let mut matches: Vec<FaceId> = Vec::new();
    for fid in solid_face_ids(model, solid) {
        let kind_ok = face_kind_name(model, fid)
            .map(|tn| q.kind.matches(tn))
            .unwrap_or(false);
        if !kind_ok {
            continue;
        }
        if let Some(d) = dir {
            match face_outward_normal(model, fid).and_then(|n| n.normalize().ok()) {
                Some(n) if n.dot(&d) >= cos_tol => {}
                _ => continue,
            }
        }
        matches.push(fid);
    }

    if matches.is_empty() {
        return Err(SelectError::NotFound);
    }

    match q.extremal {
        Extremal::None => {
            if matches.len() == 1 {
                Ok(matches[0])
            } else {
                Err(SelectError::Ambiguous(matches))
            }
        }
        e => {
            // Compute each match's score with split field borrows.
            let mut scored: Vec<(FaceId, f64)> = Vec::with_capacity(matches.len());
            for &fid in &matches {
                let s = face_score(model, fid, e);
                if let Some(s) = s {
                    scored.push((fid, s));
                }
            }
            if scored.is_empty() {
                return Err(SelectError::NotFound);
            }
            let want_max = !matches!(e, Extremal::SmallestArea);
            let best = scored
                .iter()
                .cloned()
                .reduce(|a, b| {
                    if (want_max && b.1 > a.1) || (!want_max && b.1 < a.1) {
                        b
                    } else {
                        a
                    }
                })
                .map(|(_, s)| s)
                .unwrap_or(0.0);
            // Refuse a near-tie at the extreme (relative 1% band).
            let band = best.abs().max(1.0) * 0.01;
            let near: Vec<FaceId> = scored
                .iter()
                .filter(|(_, s)| (s - best).abs() <= band)
                .map(|(f, _)| *f)
                .collect();
            if near.len() == 1 {
                Ok(near[0])
            } else {
                Err(SelectError::Ambiguous(near))
            }
        }
    }
}

/// Score one face for the extremal pick, with the split field borrows
/// `compute_stats` needs. Returns area or centroid·dir.
fn face_score(model: &mut BRepModel, fid: FaceId, e: Extremal) -> Option<f64> {
    // Distinct BRepModel fields → simultaneous &mut faces + &mut loops + & others
    // is sound (the readable query path relies on the same field-disjoint borrow).
    let BRepModel {
        faces,
        loops,
        vertices,
        edges,
        curves,
        surfaces,
        ..
    } = model;
    let face = faces.get_mut(fid)?;
    let stats = face
        .compute_stats(loops, vertices, edges, curves, surfaces)
        .ok()?;
    match e {
        Extremal::LargestArea | Extremal::SmallestArea => Some(stats.area),
        Extremal::MostAlong(d) => {
            let c = stats.centroid;
            Some(c.x * d.x + c.y * d.y + c.z * d.z)
        }
        Extremal::None => None,
    }
}
