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

/// A revolution / symmetry axis: a point on the axis and its unit direction.
/// Carried by the geometry-aware extremals (`MinRadiusStation`,
/// `AxialExtremalCap`) so the score is measured RELATIVE to the part's own axis
/// rather than a hardcoded world direction. Built by the recognizer that detects
/// the part's symmetry axis (see `BRepModel::symmetry_axis`).
#[derive(Debug, Clone, Copy)]
pub struct Axis {
    pub origin: Vector3,
    pub direction: Vector3,
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
    /// THROAT recognizer (geometry-aware): the face whose globally-MINIMUM
    /// radial distance to the symmetry `Axis` is smallest. Works on any
    /// surface-of-revolution band — the necked-down station of a bell nozzle —
    /// not just an analytic cylinder, because the score samples the face surface
    /// for its closest approach to the axis. Smaller is "more throat".
    MinRadiusStation(Axis),
    /// EXIT recognizer (geometry-aware): the face whose centroid is most EXTREME
    /// along the symmetry `Axis` — the cap at EITHER end (max |signed distance|
    /// from the axis origin), axis-relative rather than hardcoded +Z. Larger
    /// |projection| is "more of an end cap".
    AxialExtremalCap(Axis),
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
    /// One or more candidate faces could not be TESTED against the query, so
    /// the resolution is undecidable rather than merely contested.
    ///
    /// A face whose parametric domain was never measured has no representative
    /// outward normal and no trustworthy area or centroid (see
    /// [`crate::primitives::face::Face::domain_is_known`]). Such a face can be
    /// neither accepted nor rejected by a normal-direction filter or an
    /// extremal ranking, and dropping it silently is the specific failure this
    /// variant exists to prevent: with one match and one untestable candidate,
    /// excluding the latter turns "I cannot tell" into a confident
    /// `Ok(single)`.
    ///
    /// `matched` are the faces that passed every test; `unmeasurable` are the
    /// ones no test could be applied to. Both are reported so a caller can name
    /// the obstacle rather than retry blind.
    Unmeasurable {
        matched: Vec<FaceId>,
        unmeasurable: Vec<FaceId>,
    },
}

/// What asking a face for its outward normal produced.
///
/// Three outcomes, deliberately not two: "the face does not resolve" and "the
/// face resolves but cannot be evaluated" are different facts, and only the
/// second one makes a query undecidable.
enum FaceNormal {
    /// Measured at the face's own parameter-domain centre, normalized.
    Known(Vector3),
    /// The face exists, but its parametric domain was never measured, so no
    /// point on it is known to sample — see `Face::probe_uv`.
    Unmeasurable,
    /// No such face, or its surface is missing: not a candidate at all.
    Missing,
}

/// What asking a face for its extremal SCORE produced.
///
/// The same three outcomes as [`FaceNormal`], for the same reason: a face the
/// kernel declines to RANK and a face whose statistics failed to compute are
/// different facts, and only the first makes the query undecidable. Folding
/// both into `None` is what let a dropped candidate become a confident answer.
enum FaceScore {
    /// A real number, computed over a domain the kernel measured.
    Known(f64),
    /// The face exists and matched every filter, but its parametric domain was
    /// never measured, so the area and centroid `FaceStats` would report
    /// describe a different patch of the surface. It cannot be ranked, and it
    /// cannot be excluded either — it is exactly the face that might have won.
    Unmeasurable,
    /// No such face, its statistics could not be computed, or this extremal has
    /// no score for it. Not a candidate at all, and not an obstacle.
    Missing,
}

/// Outward normal of a face at its parametric midpoint (constant for planes;
/// representative for the kinds we filter on).
fn face_outward_normal(model: &BRepModel, fid: FaceId) -> FaceNormal {
    let Some(face) = model.faces.get(fid) else {
        return FaceNormal::Missing;
    };
    // `None` on a curved face with an unmeasured domain — see `Face::probe_uv`.
    let Some((u, v)) = face.probe_uv(&model.surfaces) else {
        return FaceNormal::Unmeasurable;
    };
    match face
        .normal_at(u, v, &model.surfaces)
        .ok()
        .and_then(|n| n.normalize().ok())
    {
        Some(n) => FaceNormal::Known(n),
        // The domain is known but the surface has no normal there (a degenerate
        // parameterisation). Still untestable, not excludable.
        None => FaceNormal::Unmeasurable,
    }
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
    //
    // A candidate the normal filter cannot TEST is carried in `unmeasurable`,
    // never folded into `continue`. Excluding it would silently convert an
    // undecidable query into a confident answer — the face that was dropped is
    // exactly the one that might have matched.
    let mut matches: Vec<FaceId> = Vec::new();
    let mut unmeasurable: Vec<FaceId> = Vec::new();
    for fid in solid_face_ids(model, solid) {
        let kind_ok = face_kind_name(model, fid)
            .map(|tn| q.kind.matches(tn))
            .unwrap_or(false);
        if !kind_ok {
            continue;
        }
        if let Some(d) = dir {
            match face_outward_normal(model, fid) {
                FaceNormal::Known(n) if n.dot(&d) >= cos_tol => {}
                // Tested, and genuinely does not face that way.
                FaceNormal::Known(_) => continue,
                FaceNormal::Unmeasurable => {
                    unmeasurable.push(fid);
                    continue;
                }
                FaceNormal::Missing => continue,
            }
        }
        matches.push(fid);
    }

    if matches.is_empty() && unmeasurable.is_empty() {
        return Err(SelectError::NotFound);
    }
    if !unmeasurable.is_empty() {
        return Err(SelectError::Unmeasurable {
            matched: matches,
            unmeasurable,
        });
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
            //
            // A candidate the ranking cannot SCORE is carried in `unscorable`,
            // never folded into the drop — the same rule the normal filter
            // above follows, and for the same reason: the face that was
            // dropped is exactly the one that might have won. An unmeasured
            // face reaches here whenever the query names no direction (the
            // normal filter is then skipped entirely) or names one a plane
            // satisfies, and the kernel mints unmeasured faces BY DESIGN — an
            // apex cone's lateral, an obliquely cut spherical cap. Ranking the
            // rest and returning the runner-up as "the largest face" would be
            // a confident answer over a candidate set that was never finished.
            //
            // `Missing` still drops, and that is not the same concession: it
            // means the statistics errored or this extremal has no score for
            // the face, neither of which is a claim about the geometry.
            let mut scored: Vec<(FaceId, f64)> = Vec::with_capacity(matches.len());
            let mut unscorable: Vec<FaceId> = Vec::new();
            for &fid in &matches {
                match face_score(model, fid, e) {
                    FaceScore::Known(s) => scored.push((fid, s)),
                    FaceScore::Unmeasurable => unscorable.push(fid),
                    FaceScore::Missing => {}
                }
            }
            if !unscorable.is_empty() {
                return Err(SelectError::Unmeasurable {
                    matched: scored.iter().map(|(f, _)| *f).collect(),
                    unmeasurable: unscorable,
                });
            }
            if scored.is_empty() {
                return Err(SelectError::NotFound);
            }
            let want_max = !matches!(e, Extremal::SmallestArea | Extremal::MinRadiusStation(_));
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

/// Convexity filter — addresses "the concave edges" / "the convex edges". The
/// sign convention is the kernel's edge classification (`classify_edge` /
/// `edge.attributes.convexity`): `+1` convex, `-1` concave, `0` straight / G1 /
/// undefined. `Smooth` and undefined edges match NEITHER `Convex` nor `Concave`
/// — a concave-corner blend must target genuinely re-entrant edges, not a
/// tangent-continuous join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convexity {
    Any,
    Convex,
    Concave,
}

impl Convexity {
    /// Match a classified convexity sign (`+1` convex, `-1` concave, `0`
    /// smooth/undefined).
    fn matches(self, convexity: i8) -> bool {
        match self {
            Convexity::Any => true,
            Convexity::Convex => convexity == 1,
            Convexity::Concave => convexity == -1,
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
    /// Require the edge's classified convexity (convex / concave / any).
    pub convexity: Convexity,
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
            convexity: Convexity::Any,
            direction: None,
            angle_tol_deg: 12.0,
            extremal: EdgeExtremal::None,
        }
    }
    pub fn blend(mut self, b: BlendFilter) -> Self {
        self.blend = b;
        self
    }
    pub fn convexity(mut self, c: Convexity) -> Self {
        self.convexity = c;
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

/// Classified convexity sign of an edge (`+1` convex, `-1` concave, `0`
/// smooth/undefined). Prefers the F2-α cache when the edge is already
/// classified; otherwise classifies it READ-ONLY from geometry via
/// [`crate::operations::edge_classification::classify_edge`] (the same
/// dihedral-sign classifier the blend graph uses). Read-only by design so it
/// runs under the immutable-borrow candidate loop — resolving an edge never
/// mutates the model, and the classifier's own result is deterministic whether
/// or not it was cached. A classification failure yields `0` (matches neither
/// Convex nor Concave), so an unclassifiable edge is honestly EXCLUDED rather
/// than silently returned.
fn edge_convexity_sign(model: &BRepModel, eid: crate::primitives::edge::EdgeId) -> i8 {
    if let Some(edge) = model.edges.get(eid) {
        if edge.attributes.is_classified() {
            return edge.attributes.convexity;
        }
    }
    crate::operations::edge_classification::classify_edge(model, eid)
        .map(|c| c.convexity)
        .unwrap_or(0)
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
        // Convexity is only classified when the filter asks for it — the
        // default (Any) path stays borrow-for-borrow identical to before and
        // pays no classification cost.
        if q.convexity != Convexity::Any && !q.convexity.matches(edge_convexity_sign(model, eid)) {
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
/// `compute_stats` needs. Yields area, centroid·dir, the face's minimum radial
/// distance to an axis, or the magnitude of its centroid's axial projection.
///
/// Three-valued on purpose — see [`FaceScore`]. The caller must not fold
/// `Unmeasurable` into a drop.
fn face_score(model: &mut BRepModel, fid: FaceId, e: Extremal) -> FaceScore {
    // The two geometry-aware extremals SAMPLE the face surface, which needs an
    // immutable borrow of the model alongside the surface store — keep them off
    // the `compute_stats` split-borrow path.
    //
    // Neither is gated on the measured domain, and that is not an oversight:
    // both work from the BOUNDARY samples (loop vertices + edge midpoints),
    // never from a `uv_bounds` parametric grid, so an unmeasured domain does
    // not reach their answer. See `face_min_radius_to_axis`.
    match e {
        Extremal::MinRadiusStation(axis) => {
            return match face_min_radius_to_axis(model, fid, axis) {
                Some(s) => FaceScore::Known(s),
                None => FaceScore::Missing,
            }
        }
        Extremal::AxialExtremalCap(axis) => {
            return match face_axial_extent_magnitude(model, fid, axis) {
                Some(s) => FaceScore::Known(s),
                None => FaceScore::Missing,
            }
        }
        _ => {}
    }
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
    let Some(face) = faces.get_mut(fid) else {
        return FaceScore::Missing;
    };
    // `FaceStats` is computed over `Face::uv_bounds`, so on a face whose
    // domain was never measured both the curved-surface area and the sampled
    // centroid describe a different patch of the surface - one radian by one
    // millimetre of a cylinder that spans 2*pi by its height. `LargestArea` /
    // `SmallestArea` / `MostAlong` would then rank on a placeholder.
    //
    // Task 12 wrote this gate, ran it, and REVERTED it, because every curved
    // face minted outside a boolean was unmeasured then: gating turned 5
    // passing tests across `labels_gate` and `labels_assertion_gate` into
    // refusals, since their nozzle fixture resolves throat and chamber by
    // `SmallestArea` / `LargestArea` over two REVOLVED cylinder walls. It
    // recorded the precondition for landing it: MEASURE those faces at their
    // mint, after which the gate is a no-op and disables nothing.
    //
    // Task 32 measured them - `revolve.rs` included - so the gate lands here.
    // What it is worth: those rankings were right only by ACCIDENT. A
    // cylinder wall's placeholder area integrates to `r * 1 * 1 = r` while
    // the true area is `2*pi*r*h`, so the two orders agree exactly while the
    // compared walls share a height (the nozzle's inner r=2 and outer r=4
    // both span z in [0, 3]). Give them different heights and the ranking
    // inverts, silently. An unmeasured face is now a face the kernel declines
    // to RANK — and `Unmeasurable`, not `Missing`, because the caller must
    // REFUSE the whole query over it rather than rank the others and hand back
    // the runner-up. The kernel mints unmeasured faces by design (an apex
    // cone's lateral, an obliquely cut spherical cap), so this is reachable,
    // not hypothetical.
    if !face.domain_is_known(surfaces) {
        return FaceScore::Unmeasurable;
    }
    let Ok(stats) = face.compute_stats(loops, vertices, edges, curves, surfaces) else {
        return FaceScore::Missing;
    };
    match e {
        Extremal::LargestArea | Extremal::SmallestArea => FaceScore::Known(stats.area),
        Extremal::MostAlong(d) => {
            let c = stats.centroid;
            FaceScore::Known(c.x * d.x + c.y * d.y + c.z * d.z)
        }
        Extremal::None | Extremal::MinRadiusStation(_) | Extremal::AxialExtremalCap(_) => {
            FaceScore::Missing
        }
    }
}

/// Perpendicular distance from a point to the axis `(origin, dir)`. `dir` need
/// not be unit; it is normalized here (a zero axis yields 0).
fn point_to_axis_distance(p: crate::math::Point3, axis: Axis) -> f64 {
    let d = match axis.direction.normalize() {
        Ok(d) => d,
        Err(_) => return 0.0,
    };
    let o = crate::math::Point3::new(axis.origin.x, axis.origin.y, axis.origin.z);
    let w = p - o;
    let along = w.dot(&d);
    let proj = crate::math::Point3::new(o.x + d.x * along, o.y + d.y * along, o.z + d.z * along);
    p.distance(&proj)
}

/// The REPRESENTATIVE radial distance of a face to `axis` — the MEAN radius of
/// the face's boundary samples (loop vertices + edge midpoints). This is the
/// throat score: the face whose body SITS closest to the axis. A min-radius
/// cylinder/band scores its own radius; an adjacent contraction cone, which
/// meets the throat at one rim but flares to a wide rim at the other, scores its
/// rim-mean — well above the throat. Using the body MEAN rather than the
/// touching minimum avoids a degenerate tie at the shared throat rim, where a
/// cone and the throat band meet at the same radius but only the band STATIONS
/// there.
///
/// Boundary samples are used (not a `uv_bounds` parametric grid) because the
/// loop edges carry the trimmed extent directly, with no dependence on whether
/// the face's parametric domain was ever measured. That independence is why
/// `face_score` does NOT gate these two extremals on `domain_is_known`.
/// `None` if the face has no resolvable boundary point.
fn face_min_radius_to_axis(model: &BRepModel, fid: FaceId, axis: Axis) -> Option<f64> {
    let face = model.faces.get(fid)?;
    let mut sum = 0.0_f64;
    let mut count = 0u32;
    let mut add = |p: crate::math::Point3| {
        sum += point_to_axis_distance(p, axis);
        count += 1;
    };
    let mut lids = vec![face.outer_loop];
    lids.extend_from_slice(&face.inner_loops);
    for lid in lids {
        if let Some(lp) = model.loops.get(lid) {
            for &eid in &lp.edges {
                if let Some(edge) = model.edges.get(eid) {
                    let a = model.vertices.get(edge.start_vertex).map(|v| v.point());
                    let b = model.vertices.get(edge.end_vertex).map(|v| v.point());
                    if let Some(a) = a {
                        add(a);
                    }
                    // Edge midpoint — densifies a curved-band boundary so the
                    // mean tracks the band, not just its corner vertices.
                    if let (Some(a), Some(b)) = (a, b) {
                        add(crate::math::Point3::new(
                            0.5 * (a.x + b.x),
                            0.5 * (a.y + b.y),
                            0.5 * (a.z + b.z),
                        ));
                    }
                }
            }
        }
    }
    if count == 0 {
        return None;
    }
    Some(sum / count as f64)
}

/// The magnitude of a face's centroid projection onto `axis` (signed distance
/// from the axis origin, absolute) — how far toward EITHER end of the part the
/// face sits. The end caps maximize this. `None` if the centroid is unavailable.
fn face_axial_extent_magnitude(model: &mut BRepModel, fid: FaceId, axis: Axis) -> Option<f64> {
    let d = axis.direction.normalize().ok()?;
    let centroid = {
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
        match face.compute_stats(loops, vertices, edges, curves, surfaces) {
            Ok(stats) => stats.centroid,
            // Fall back to the boundary mean for a face whose centroid integral
            // declines (annular caps) — the axial position is still well-defined.
            Err(_) => model.face_boundary_mean(fid)?,
        }
    };
    let o = crate::math::Point3::new(axis.origin.x, axis.origin.y, axis.origin.z);
    let along = (centroid - o).dot(&d);
    Some(along.abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    /// Two coaxial cylinders in one model, one lateral measured and one not —
    /// the state the resolver must not paper over.
    ///
    /// **Neither half can be taken from a mint any more.** When this fixture
    /// was written the primitive builder did not measure its lateral, so the
    /// unmeasured half came free; task 32 wired every production mint, and a
    /// plain cylinder's wall now carries a real domain. Relying on that was the
    /// fixture encoding the defect it was written beside.
    ///
    /// So BOTH halves are declared here, each the way its own producer would:
    /// the measured one through `set_uv_bounds` (what a measuring mint does),
    /// the unmeasured one by replacing the stored face with a fresh
    /// `Face::new` over the same surface and loops — a constructor handed an
    /// id, a surface and a loop, which by construction cannot know the domain.
    /// That is the state under test, and it is now independent of what any
    /// mint happens to do. The point under test is the RESOLVER's contract,
    /// not the geometry.
    fn two_cylinders_one_measured() -> (BRepModel, SolidId, FaceId, FaceId) {
        let mut model = BRepModel::new();
        let mk = |model: &mut BRepModel, z: f64| -> SolidId {
            let mut b = TopologyBuilder::new(model);
            match b
                .create_cylinder_3d(Point3::new(0.0, 0.0, z), Vector3::Z, 5.0, 4.0)
                .expect("cylinder")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {other:?}"),
            }
        };
        let a = mk(&mut model, 0.0);
        let b = mk(&mut model, 20.0);

        let lateral_of = |model: &BRepModel, sid: SolidId| -> FaceId {
            solid_face_ids(model, sid)
                .into_iter()
                .find(|&f| face_kind_name(model, f) == Some("Cylinder"))
                .expect("each cylinder has a lateral")
        };
        let measured = lateral_of(&model, a);
        let unmeasured = lateral_of(&model, b);

        // Give solid `a`'s lateral its true domain, the way a measuring mint
        // would.
        if let Some(f) = model.faces.get_mut(measured) {
            f.set_uv_bounds(0.0, std::f64::consts::TAU, 0.0, 4.0);
        }

        // Put solid `b`'s lateral back to what a constructor alone produces:
        // same surface, same loops, no measurement. `Face::new` is the only
        // thing that can express "nobody measured this", because
        // `set_uv_bounds` is the only thing that flips the flag and there is
        // deliberately no way to unset it.
        if let Some(f) = model.faces.get_mut(unmeasured) {
            let fresh =
                crate::primitives::face::Face::new(f.id, f.surface_id, f.outer_loop, f.orientation);
            let inners = f.inner_loops.clone();
            *f = fresh;
            for inner in inners {
                f.add_inner_loop(inner);
            }
        }

        // One solid holding both laterals, so a single query sees them together.
        let shell_id = {
            let mut shell =
                crate::primitives::shell::Shell::new(0, crate::primitives::shell::ShellType::Open);
            shell.add_face(measured);
            shell.add_face(unmeasured);
            model.shells.add(shell)
        };
        let solid = model
            .solids
            .add(crate::primitives::solid::Solid::new(0, shell_id));
        (model, solid, measured, unmeasured)
    }

    /// THE HAZARD: one testable match plus one untestable candidate must not
    /// resolve. Folding the untestable one into `continue` leaves exactly one
    /// match and answers `Ok(measured)` — a confident pick over a candidate set
    /// the kernel never finished evaluating.
    #[test]
    fn one_untestable_candidate_makes_the_resolution_refuse_not_resolve() {
        let (mut model, solid, measured, unmeasured) = two_cylinders_one_measured();

        // Precondition: exactly one of the two is testable, so a swallow would
        // leave a single match and look like a clean answer.
        assert!(
            matches!(face_outward_normal(&model, measured), FaceNormal::Known(_)),
            "the measured lateral must be testable"
        );
        assert!(
            matches!(
                face_outward_normal(&model, unmeasured),
                FaceNormal::Unmeasurable
            ),
            "the unmeasured lateral must be untestable"
        );

        // `facing(+X)`: the measured lateral's domain centre is u = pi, i.e. it
        // faces -X, so it is legitimately excluded... but with a 180 degree
        // tolerance every testable face matches, which is what puts a real
        // match and an untestable candidate in the same query.
        let mut q = FaceQuery::new(SurfaceKind::Cylindrical).facing(Vector3::X);
        q.angle_tol_deg = 180.0;

        match resolve_face(&mut model, solid, &q) {
            Err(SelectError::Unmeasurable {
                matched,
                unmeasurable,
            }) => {
                assert_eq!(matched, vec![measured], "the testable match is reported");
                assert_eq!(
                    unmeasurable,
                    vec![unmeasured],
                    "the untestable candidate is named, not dropped"
                );
            }
            other => {
                panic!("one testable match + one untestable candidate must refuse, got {other:?}")
            }
        }
    }

    /// The same solid with the untestable face REMOVED resolves cleanly — so
    /// the refusal above is caused by the untestable candidate and by nothing
    /// else about the fixture.
    #[test]
    fn the_testable_candidate_alone_resolves() {
        let (mut model, _solid, measured, _unmeasured) = two_cylinders_one_measured();
        let shell_id = {
            let mut shell =
                crate::primitives::shell::Shell::new(0, crate::primitives::shell::ShellType::Open);
            shell.add_face(measured);
            model.shells.add(shell)
        };
        let solid = model
            .solids
            .add(crate::primitives::solid::Solid::new(0, shell_id));

        let mut q = FaceQuery::new(SurfaceKind::Cylindrical).facing(Vector3::X);
        q.angle_tol_deg = 180.0;
        assert_eq!(
            resolve_face(&mut model, solid, &q),
            Ok(measured),
            "a solid whose every candidate is testable still resolves"
        );
    }
}
