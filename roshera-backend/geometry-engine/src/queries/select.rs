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

// ───────────────────── edge selection ──────────────────────────────

/// Curve-kind filter (matches `Curve::type_name()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveKind {
    Any,
    Line,
    Arc,
    Circle,
    Nurbs,
}

impl CurveKind {
    fn matches(self, type_name: &str) -> bool {
        match self {
            CurveKind::Any => true,
            CurveKind::Line => type_name == "Line",
            CurveKind::Arc => type_name == "Arc",
            CurveKind::Circle => type_name == "Circle",
            CurveKind::Nurbs => type_name.starts_with("Nurbs"),
        }
    }
}

/// Blend-state filter — addresses "the fillet edge" / "the unblended edges".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendFilter {
    Any,
    Filleted,
    Chamfered,
    Unblended,
}

impl BlendFilter {
    fn matches(self, k: Option<crate::primitives::solid::BlendKind>) -> bool {
        use crate::primitives::solid::BlendKind;
        match self {
            BlendFilter::Any => true,
            BlendFilter::Filleted => matches!(k, Some(BlendKind::Fillet)),
            BlendFilter::Chamfered => matches!(k, Some(BlendKind::Chamfer)),
            BlendFilter::Unblended => k.is_none(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum EdgeExtremal {
    None,
    Longest,
    Shortest,
    /// The edge whose midpoint is farthest along `dir`.
    MostAlong(Vector3),
}

/// A descriptive edge reference.
#[derive(Debug, Clone)]
pub struct EdgeQuery {
    pub kind: CurveKind,
    pub blend: BlendFilter,
    /// Require the edge's chord direction to align with this (sign-insensitive).
    pub direction: Option<Vector3>,
    pub angle_tol_deg: f64,
    pub extremal: EdgeExtremal,
}

impl EdgeQuery {
    pub fn new(kind: CurveKind) -> Self {
        Self {
            kind,
            blend: BlendFilter::Any,
            direction: None,
            angle_tol_deg: 12.0,
            extremal: EdgeExtremal::None,
        }
    }
    pub fn blend(mut self, b: BlendFilter) -> Self {
        self.blend = b;
        self
    }
    pub fn along(mut self, dir: Vector3) -> Self {
        self.direction = Some(dir);
        self
    }
    pub fn extremal(mut self, e: EdgeExtremal) -> Self {
        self.extremal = e;
        self
    }
}

fn solid_edge_ids(model: &BRepModel, solid: SolidId) -> Vec<crate::primitives::edge::EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    if let Some(s) = model.solids.get(solid) {
        for sh in s.shell_ids() {
            if let Some(shell) = model.shells.get(sh) {
                for &fid in &shell.faces {
                    if let Some(face) = model.faces.get(fid) {
                        let mut lids = vec![face.outer_loop];
                        lids.extend_from_slice(&face.inner_loops);
                        for lid in lids {
                            if let Some(lp) = model.loops.get(lid) {
                                for &e in &lp.edges {
                                    if seen.insert(e) {
                                        out.push(e);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Length (cached) or midpoint·dir for the extremal pick — split field borrows.
fn edge_metric(
    model: &mut BRepModel,
    eid: crate::primitives::edge::EdgeId,
    e: EdgeExtremal,
) -> Option<f64> {
    match e {
        EdgeExtremal::Longest | EdgeExtremal::Shortest => {
            let BRepModel { edges, curves, .. } = model;
            let edge = edges.get_mut(eid)?;
            edge.length(curves, crate::math::Tolerance::default()).ok()
        }
        EdgeExtremal::MostAlong(d) => {
            let edge = model.edges.get(eid)?;
            let a = model.vertices.get(edge.start_vertex)?.point();
            let b = model.vertices.get(edge.end_vertex)?.point();
            Some(0.5 * ((a.x + b.x) * d.x + (a.y + b.y) * d.y + (a.z + b.z) * d.z))
        }
        EdgeExtremal::None => None,
    }
}

/// Resolve a descriptive edge reference to a single `EdgeId`, or refuse.
pub fn resolve_edge(
    model: &mut BRepModel,
    solid: SolidId,
    q: &EdgeQuery,
) -> Result<crate::primitives::edge::EdgeId, SelectError> {
    let cos_tol = q.angle_tol_deg.to_radians().cos();
    let want_dir = q.direction.and_then(|d| d.normalize().ok());

    let mut matches: Vec<crate::primitives::edge::EdgeId> = Vec::new();
    for eid in solid_edge_ids(model, solid) {
        let edge = match model.edges.get(eid) {
            Some(e) => e,
            None => continue,
        };
        let cn = model
            .curves
            .get(edge.curve_id)
            .map(|c| c.type_name())
            .unwrap_or("");
        if !q.kind.matches(cn) {
            continue;
        }
        let blend = model
            .solids
            .get(solid)
            .and_then(|s| s.blend_kind_at_edge(eid));
        if !q.blend.matches(blend) {
            continue;
        }
        if let Some(d) = want_dir {
            let a = model.vertices.get(edge.start_vertex).map(|v| v.point());
            let b = model.vertices.get(edge.end_vertex).map(|v| v.point());
            match (a, b) {
                (Some(a), Some(b)) => match (b - a).normalize() {
                    Ok(span) if span.dot(&d).abs() >= cos_tol => {}
                    _ => continue,
                },
                _ => continue,
            }
        }
        matches.push(eid);
    }

    if matches.is_empty() {
        return Err(SelectError::NotFound);
    }
    match q.extremal {
        EdgeExtremal::None => {
            if matches.len() == 1 {
                Ok(matches[0])
            } else {
                Err(SelectError::Ambiguous(matches))
            }
        }
        e => {
            let mut scored: Vec<(crate::primitives::edge::EdgeId, f64)> = Vec::new();
            for &eid in &matches {
                if let Some(s) = edge_metric(model, eid, e) {
                    scored.push((eid, s));
                }
            }
            if scored.is_empty() {
                return Err(SelectError::NotFound);
            }
            let want_max = !matches!(e, EdgeExtremal::Shortest);
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
            let band = best.abs().max(1.0) * 0.01;
            let near: Vec<crate::primitives::edge::EdgeId> = scored
                .iter()
                .filter(|(_, s)| (s - best).abs() <= band)
                .map(|(x, _)| *x)
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
