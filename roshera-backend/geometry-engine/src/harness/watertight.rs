//! Universal watertightness oracle — the one correctness check every geometry
//! operation's output must pass.
//!
//! A solid is *watertight* when its boundary is a closed, consistently-oriented
//! surface enclosing a well-defined volume. The kernel can assert this cheaply
//! and universally: tessellate the solid and compare the mesh's enclosed volume
//! (the divergence-theorem sum over the triangles) against the analytic
//! mass-properties volume. A leak (open seam) or a flipped triangle makes the
//! divergence sum diverge wildly from the true volume, so agreement within the
//! faceting tolerance certifies the boundary is closed.
//!
//! Every op harness in this module — boolean, fillet, extrude, revolve, … — can
//! call [`is_watertight`] on its result; it is the shared, operation-agnostic
//! correctness primitive the whole geometry module is held to.

use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};
use std::collections::HashMap;

/// The analytic (mass-properties) volume of a solid, or `None` if it can't be
/// computed.
///
/// Uses the AUDIT-quality, non-caching volume — the coarse divergence-theorem
/// integration the verification path needs (a few-percent band), NOT the
/// export-grade `fine()` mass-properties the agent-facing `mass_properties_for`
/// returns. This is the change that makes the audit fast: the `fine()` volume on
/// a curved-Boolean fragment is the audit's dominant cost. It does not poison the
/// `fine()` mass-props cache (a later agent query still gets precise numbers),
/// and reuses that cache when a prior agent query already warmed it.
pub fn analytic_volume(model: &mut BRepModel, solid: SolidId) -> Option<f64> {
    model.audit_volume(solid)
}

/// The volume enclosed by the solid's tessellated mesh at chord tolerance
/// `chord`, via the divergence theorem `V = (1/6) Σ p0·(p1×p2)`. `None` if the
/// solid is missing or tessellates to nothing.
pub fn mesh_volume(model: &BRepModel, solid: SolidId, chord: f64) -> Option<f64> {
    let solid_ref = model.solids.get(solid)?;
    // The caller's `chord` is the deliberate quality knob here (fillet/chamfer
    // harnesses compare this against an EXTERNAL analytic truth at a tight chord,
    // so the segment ceiling stays at `default()`'s 100 — coarsening it would
    // shift a curved fillet's meshed volume relative to its closed-form truth).
    // The fan-budget non-termination guard scales with `max_segments`, so this
    // path is still bounded; the audit's heavy cost was the `fine()` ANALYTIC
    // volume, addressed by `analytic_volume`/`audit_volume`, not this mesh side.
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }
    let mut six_v = 0.0;
    for tri in &mesh.triangles {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        six_v += p0.dot(&p1.cross(&p2));
    }
    Some((six_v / 6.0).abs())
}

/// Is `solid` watertight? Its tessellated mesh must enclose the analytic volume
/// within the relative tolerance `rel_tol` (a few percent absorbs faceting; a
/// leak or flip produces a far larger discrepancy). `false` if either volume is
/// uncomputable (which is itself a failure).
pub fn is_watertight(model: &mut BRepModel, solid: SolidId, chord: f64, rel_tol: f64) -> bool {
    // AUDIT-quality analytic volume (coarse, non-caching) — both sides of the
    // comparison are now coarse, so they agree on the SHAPE'S volume and only a
    // real leak/flip (a topological defect coarse tessellation still exposes)
    // produces a discrepancy beyond `rel_tol`. See `analytic_volume`.
    let Some(analytic) = model.audit_volume(solid) else {
        return false;
    };
    let Some(mesh) = mesh_volume(model, solid, chord) else {
        return false;
    };
    let scale = analytic.abs().max(mesh.abs()).max(1.0);
    (analytic - mesh).abs() / scale <= rel_tol
}

/// Topological verdict for a tessellated solid — far stricter than the
/// volume-agreement [`is_watertight`] check.
///
/// `is_watertight` only asserts the divergence-theorem volume of the mesh
/// matches the analytic volume; a solid can pass that while being
/// topologically broken in ways the signed volume happens to cancel out
/// (the sphere-winding `.abs()` class: two inverted patches whose flipped
/// contributions net to the right number). This report inspects the mesh's
/// *connectivity* instead:
///
/// * **closed** — no boundary edge (every undirected edge borders exactly two
///   triangles). A leak/open seam shows up as `boundary_edges > 0`.
/// * **manifold** — no edge shared by three or more triangles
///   (`nonmanifold_edges == 0`).
/// * **oriented** — every *directed* edge appears at most once. A consistently
///   wound closed surface traverses each edge once per direction, so a repeated
///   directed edge means two triangles wind the same way across it — a flipped
///   normal or a duplicated facet. This is the check `is_watertight` cannot make.
///
/// The mesh is welded by quantised position first: per-face tessellation emits
/// independent vertex indices even where faces share a boundary edge, so raw
/// triangle indices never share vertices across faces. Welding restores the
/// shared topology (shared-edge samples are bit-exact by the `EdgeSampleCache`
/// contract, so a tight epsilon suffices).
#[derive(Debug, Clone)]
pub struct ManifoldReport {
    pub triangles: usize,
    pub degenerate_triangles: usize,
    pub welded_vertices: usize,
    pub undirected_edges: usize,
    /// Undirected edges bordering exactly one triangle — a leak.
    pub boundary_edges: usize,
    /// Undirected edges bordering three or more triangles — non-manifold.
    pub nonmanifold_edges: usize,
    /// Directed edges traversed by more than one triangle — orientation flip
    /// or duplicated facet.
    pub inconsistent_directed_edges: usize,
    /// Connected components of the welded mesh (disjoint solids/shells).
    pub components: usize,
    /// V − E + F over the welded mesh. For `c` disjoint genus-0 shells this is
    /// `2c`; a single closed genus-0 solid is `2`.
    pub euler_characteristic: i64,
    pub closed: bool,
    pub manifold: bool,
    pub oriented: bool,
}

impl ManifoldReport {
    /// The result is a valid closed, oriented 2-manifold solid boundary.
    pub fn is_valid_solid(&self) -> bool {
        self.closed && self.manifold && self.oriented && self.triangles > 0
    }
}

/// Quantise a position to an integer lattice at spacing `eps` for welding.
fn weld_key(p: &crate::math::vector3::Point3, eps: f64) -> (i64, i64, i64) {
    (
        (p.x / eps).round() as i64,
        (p.y / eps).round() as i64,
        (p.z / eps).round() as i64,
    )
}

/// Tessellate `solid` and analyse the mesh's topological connectivity. `None`
/// if the solid is missing or tessellates to nothing.
///
/// `weld_eps` is the absolute distance below which two mesh vertices are treated
/// as the same point. Choose well under the chord length but comfortably above
/// f64 noise — `1e-6` works for the unit-to-ten-unit solids the harness builds.
pub fn manifold_report(
    model: &BRepModel,
    solid: SolidId,
    chord: f64,
    weld_eps: f64,
) -> Option<ManifoldReport> {
    let solid_ref = model.solids.get(solid)?;
    // Caller-driven `chord` with `default()`'s segment ceiling (see `mesh_volume`
    // for why this side is not coarsened to `audit()`). Connectivity is a
    // topological verdict the `max_segments`-scaled fan-budget guard keeps
    // bounded; the audit's cost was the `fine()` analytic volume, not this.
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }

    // Weld vertices by quantised position.
    let mut weld_map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut welded_index: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let key = weld_key(&v.position, weld_eps);
        let next = weld_map.len() as u32;
        let id = *weld_map.entry(key).or_insert(next);
        welded_index.push(id);
    }
    let welded_vertices = weld_map.len();

    // Directed-edge multiset over welded indices; skip degenerate triangles.
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    let mut degenerate_triangles = 0usize;
    let mut live_triangles = 0usize;
    // Union-find over welded vertices for component counting.
    let mut parent: Vec<u32> = (0..welded_vertices as u32).collect();
    fn find(parent: &mut Vec<u32>, mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let union = |parent: &mut Vec<u32>, a: u32, b: u32| {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra as usize] = rb;
        }
    };

    for tri in &mesh.triangles {
        let a = welded_index[tri[0] as usize];
        let b = welded_index[tri[1] as usize];
        let c = welded_index[tri[2] as usize];
        if a == b || b == c || c == a {
            degenerate_triangles += 1;
            continue;
        }
        live_triangles += 1;
        for &(u, v) in &[(a, b), (b, c), (c, a)] {
            *directed.entry((u, v)).or_insert(0) += 1;
            union(&mut parent, u, v);
        }
    }

    // Aggregate undirected edges from the directed multiset.
    let mut undirected: HashMap<(u32, u32), u32> = HashMap::new();
    let mut inconsistent_directed_edges = 0usize;
    for (&(u, v), &count) in &directed {
        if count > 1 {
            inconsistent_directed_edges += 1;
        }
        let key = if u < v { (u, v) } else { (v, u) };
        *undirected.entry(key).or_insert(0) += count;
    }

    let mut boundary_edges = 0usize;
    let mut nonmanifold_edges = 0usize;
    for &incident in undirected.values() {
        if incident == 1 {
            boundary_edges += 1;
        } else if incident > 2 {
            nonmanifold_edges += 1;
        }
    }

    // Components over vertices actually referenced by a live triangle.
    let mut roots = std::collections::HashSet::new();
    for (&(u, v), _) in &directed {
        roots.insert(find(&mut parent, u));
        roots.insert(find(&mut parent, v));
    }
    let components = roots.len().max(1);

    let v_count = {
        let mut used = std::collections::HashSet::new();
        for (&(u, v), _) in &directed {
            used.insert(u);
            used.insert(v);
        }
        used.len() as i64
    };
    let e_count = undirected.len() as i64;
    let f_count = live_triangles as i64;
    let euler_characteristic = v_count - e_count + f_count;

    Some(ManifoldReport {
        triangles: mesh.triangles.len(),
        degenerate_triangles,
        welded_vertices,
        undirected_edges: undirected.len(),
        boundary_edges,
        nonmanifold_edges,
        inconsistent_directed_edges,
        components,
        euler_characteristic,
        closed: boundary_edges == 0,
        manifold: nonmanifold_edges == 0,
        oriented: inconsistent_directed_edges == 0,
    })
}

/// Diagnostic: the 3D segments of every BOUNDARY edge (an undirected mesh edge
/// incident to exactly one live triangle) in `solid`'s tessellation. Localizes
/// WHERE a non-watertight result leaks — clustered segments reveal a specific
/// seam (e.g. a cyl-cyl saddle ellipse), spread-out segments a classification
/// failure. Diagnostic-only; mirrors `manifold_report`'s welding.
pub fn boundary_edge_positions(
    model: &BRepModel,
    solid: SolidId,
    chord: f64,
    weld_eps: f64,
) -> Vec<[crate::math::vector3::Point3; 2]> {
    let Some(solid_ref) = model.solids.get(solid) else {
        return Vec::new();
    };
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let mesh = tessellate_solid(solid_ref, model, &params);
    let mut weld_map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut welded_index: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
    let mut pos_of: Vec<crate::math::vector3::Point3> = Vec::new();
    for v in &mesh.vertices {
        let key = weld_key(&v.position, weld_eps);
        let next = weld_map.len() as u32;
        let id = *weld_map.entry(key).or_insert(next);
        if id as usize == pos_of.len() {
            pos_of.push(v.position);
        }
        welded_index.push(id);
    }
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    for tri in &mesh.triangles {
        let a = welded_index[tri[0] as usize];
        let b = welded_index[tri[1] as usize];
        let c = welded_index[tri[2] as usize];
        if a == b || b == c || c == a {
            continue;
        }
        for &(u, v) in &[(a, b), (b, c), (c, a)] {
            *directed.entry((u, v)).or_insert(0) += 1;
        }
    }
    let mut undirected: HashMap<(u32, u32), u32> = HashMap::new();
    for (&(u, v), &count) in &directed {
        let key = if u < v { (u, v) } else { (v, u) };
        *undirected.entry(key).or_insert(0) += count;
    }
    let mut segs = Vec::new();
    for (&(u, v), &incident) in &undirected {
        if incident == 1 {
            segs.push([pos_of[u as usize], pos_of[v as usize]]);
        }
    }
    segs
}

/// Display-tessellation quality verdict for `solid` — the render-mesh analogue of
/// [`manifold_report`]'s topological check, measured at the **display density**
/// the viewport/exporter actually ships (`TessellationParams::default()`), not a
/// coarse audit chord, because a scribble can be density-dependent.
///
/// Per face (via the mesh `face_map`) it computes three things:
/// 1. **degenerate** zero-area facets;
/// 2. **stored-normal agreement** — winding `(p1−p0)×(p2−p0)` vs the stored vertex
///    normals (catches a facet shaded against its own geometry);
/// 3. **analytic-normal agreement** — winding vs the TRUE surface normal at the
///    facet centroid (`surface.closest_point` → `normal_at`, oriented by face
///    sense). This is the ground-truth check: an inner-bore scribble whose stored
///    normals are wrong-but-self-consistent passes (2) but fails (3), because the
///    off-radial facets disagree with the actual analytic surface.
///
/// Returns the aggregate plus the single **worst face** — a deterministic pointer
/// (lowest analytic agreement, then stored, then most-degenerate, then lowest id)
/// the agent can act on without rendering. `None` if the solid is missing or
/// tessellates to nothing.
pub fn tessellation_quality(
    model: &BRepModel,
    solid: SolidId,
) -> Option<crate::primitives::provenance::TessellationQuality> {
    use crate::primitives::provenance::{TessFaceDefect, TessellationQuality};
    use std::cmp::Ordering::Equal;
    let solid_ref = model.solids.get(solid)?;
    let params = TessellationParams::default();
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }
    let has_faces = mesh.face_map.len() == mesh.triangles.len();
    let tol = model.tolerance;

    // Per-face: (triangles, degenerate, stored_agree, analytic_agree, comparable).
    let mut per_face: HashMap<u32, (usize, usize, usize, usize, usize)> = HashMap::new();
    let mut degenerate_triangles = 0usize;
    let mut stored_agree = 0usize;
    let mut analytic_agree = 0usize;
    let mut comparable = 0usize;

    for (i, tri) in mesh.triangles.iter().enumerate() {
        let v0 = mesh.vertices[tri[0] as usize];
        let v1 = mesh.vertices[tri[1] as usize];
        let v2 = mesh.vertices[tri[2] as usize];
        let geo = (v1.position - v0.position).cross(&(v2.position - v0.position));
        let fid = if has_faces { mesh.face_map[i] } else { 0 };
        let entry = per_face.entry(fid).or_insert((0, 0, 0, 0, 0));
        entry.0 += 1;
        if geo.magnitude() <= 1e-12 {
            degenerate_triangles += 1;
            entry.1 += 1;
            continue;
        }
        comparable += 1;
        entry.4 += 1;
        // (1) stored-normal agreement.
        let stored = v0.normal + v1.normal + v2.normal;
        if stored.dot(&geo) > 0.0 {
            stored_agree += 1;
            entry.2 += 1;
        }
        // (2) analytic-normal agreement (ground truth). Unresolvable normal ⇒
        // treat as agreeing rather than manufacture a false defect.
        if analytic_facet_agrees(
            model,
            fid,
            &v0.position,
            &v1.position,
            &v2.position,
            &geo,
            tol,
        )
        .unwrap_or(true)
        {
            analytic_agree += 1;
            entry.3 += 1;
        }
    }

    let normal_agreement = if comparable == 0 {
        1.0
    } else {
        stored_agree as f64 / comparable as f64
    };
    let analytic_normal_agreement = if comparable == 0 {
        1.0
    } else {
        analytic_agree as f64 / comparable as f64
    };
    let inconsistent_facets = comparable.saturating_sub(stored_agree);
    let off_surface_facets = comparable.saturating_sub(analytic_agree);

    // Worst defective face, chosen deterministically.
    let mut defects: Vec<TessFaceDefect> = per_face
        .into_iter()
        .filter_map(|(fid, (tris, degen, st_a, an_a, cmp))| {
            let na = if cmp == 0 {
                1.0
            } else {
                st_a as f64 / cmp as f64
            };
            let aa = if cmp == 0 {
                1.0
            } else {
                an_a as f64 / cmp as f64
            };
            let clean_face = degen == 0
                && na >= TessellationQuality::MIN_NORMAL_AGREEMENT
                && aa >= TessellationQuality::MIN_ANALYTIC_AGREEMENT;
            if clean_face {
                return None;
            }
            Some(TessFaceDefect {
                face_id: fid as u64,
                triangles: tris,
                degenerate_triangles: degen,
                normal_agreement: na,
                analytic_normal_agreement: aa,
            })
        })
        .collect();
    defects.sort_by(|a, b| {
        a.analytic_normal_agreement
            .partial_cmp(&b.analytic_normal_agreement)
            .unwrap_or(Equal)
            .then(
                a.normal_agreement
                    .partial_cmp(&b.normal_agreement)
                    .unwrap_or(Equal),
            )
            .then(b.degenerate_triangles.cmp(&a.degenerate_triangles))
            .then(a.face_id.cmp(&b.face_id))
    });
    let worst_face = defects.into_iter().next();

    Some(TessellationQuality::evaluate(
        mesh.triangles.len(),
        degenerate_triangles,
        normal_agreement,
        analytic_normal_agreement,
        inconsistent_facets,
        off_surface_facets,
        worst_face,
    ))
}

/// `Some(true)` if the facet's winding normal `geo` sits in the same hemisphere as
/// the TRUE surface normal at the facet centroid (oriented by the owning face's
/// sense); `Some(false)` if it points the wrong way (off-surface / scribbled).
/// `None` when the face / surface / normal can't be resolved (caller treats as
/// agreeing).
fn analytic_facet_agrees(
    model: &BRepModel,
    face_id: u32,
    p0: &crate::math::Point3,
    p1: &crate::math::Point3,
    p2: &crate::math::Point3,
    geo: &crate::math::Vector3,
    tol: crate::math::Tolerance,
) -> Option<bool> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let centroid = crate::math::Point3::new(
        (p0.x + p1.x + p2.x) / 3.0,
        (p0.y + p1.y + p2.y) / 3.0,
        (p0.z + p1.z + p2.z) / 3.0,
    );
    let (u, v) = surface.closest_point(&centroid, tol).ok()?;
    let n = surface.normal_at(u, v).ok()?;
    let oriented = n * face.orientation.sign();
    Some(oriented.dot(geo) > 0.0)
}

/// Angle (degrees) between a facet's winding normal `geo` and the TRUE surface
/// normal at its centroid (oriented by face sense). `None` if unresolvable.
fn analytic_facet_deviation_deg(
    model: &BRepModel,
    face_id: u32,
    p0: &crate::math::Point3,
    p1: &crate::math::Point3,
    p2: &crate::math::Point3,
    geo: &crate::math::Vector3,
    tol: crate::math::Tolerance,
) -> Option<f64> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let centroid = crate::math::Point3::new(
        (p0.x + p1.x + p2.x) / 3.0,
        (p0.y + p1.y + p2.y) / 3.0,
        (p0.z + p1.z + p2.z) / 3.0,
    );
    let (u, v) = surface.closest_point(&centroid, tol).ok()?;
    let n = (surface.normal_at(u, v).ok()? * face.orientation.sign())
        .normalize()
        .ok()?;
    let g = geo.normalize().ok()?;
    let dot = g.dot(&n).clamp(-1.0, 1.0);
    Some(dot.acos().to_degrees())
}

/// Angular extent a triangle covers on a periodic direction of period `p` — the
/// circle minus the largest gap between its three (wrapped) parameter values. A
/// thin seam triangle covers ~0; a facet bridging across the interior (the bore
/// "wing") covers ~p/2 or more.
pub fn periodic_coverage(u0: f64, u1: f64, u2: f64, p: f64) -> f64 {
    if p <= 0.0 {
        return 0.0;
    }
    let mut us = [u0.rem_euclid(p), u1.rem_euclid(p), u2.rem_euclid(p)];
    us.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let g0 = us[1] - us[0];
    let g1 = us[2] - us[1];
    let g2 = p - us[2] + us[0]; // wrap gap
    let max_gap = g0.max(g1).max(g2);
    p - max_gap
}

/// **Mesh-quality** verdict — the render mesh against the CAD tessellation rules:
/// boundary conformance (no facet bridges across a periodic/closed lateral),
/// normal deviation from the true surface (smoothness / off-surface bridges), and
/// the agent-facing aspect-ratio / min-angle readout. A facet can be watertight,
/// non-degenerate and correctly oriented yet still bridge the bore (the "wing") —
/// this catches that. `None` if the solid is missing or tessellates to nothing.
pub fn mesh_quality(
    model: &BRepModel,
    solid: SolidId,
) -> Option<crate::primitives::provenance::MeshQuality> {
    use crate::primitives::provenance::{MeshFaceQualityDefect, MeshQuality};
    use std::cmp::Ordering::Equal;
    let solid_ref = model.solids.get(solid)?;
    let params = TessellationParams::default();
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }
    let has_faces = mesh.face_map.len() == mesh.triangles.len();
    let tol = model.tolerance;

    // Per face: (worst_aspect, min_angle_deg, max_normal_dev_deg, boundary_cross).
    let mut per_face: HashMap<u32, (f64, f64, f64, usize)> = HashMap::new();
    let mut worst_aspect = 1.0_f64;
    let mut min_angle = 180.0_f64;
    let mut max_dev = 0.0_f64;
    let mut boundary_crossing = 0usize;

    for (i, tri) in mesh.triangles.iter().enumerate() {
        let v0 = mesh.vertices[tri[0] as usize];
        let v1 = mesh.vertices[tri[1] as usize];
        let v2 = mesh.vertices[tri[2] as usize];
        let (p0, p1, p2) = (v0.position, v1.position, v2.position);
        let geo = (p1 - p0).cross(&(p2 - p0));
        if geo.magnitude() <= 1e-12 {
            continue; // zero-area handled by TessellationQuality
        }
        let fid = if has_faces { mesh.face_map[i] } else { 0 };
        let entry = per_face.entry(fid).or_insert((1.0, 180.0, 0.0, 0));

        // Aspect ratio = longest edge / shortest edge.
        let e0 = (p1 - p0).magnitude();
        let e1 = (p2 - p1).magnitude();
        let e2 = (p0 - p2).magnitude();
        let lo = e0.min(e1).min(e2).max(1e-12);
        let hi = e0.max(e1).max(e2);
        let aspect = hi / lo;
        worst_aspect = worst_aspect.max(aspect);
        entry.0 = entry.0.max(aspect);

        // Smallest interior angle (law of cosines).
        let ang = |a: f64, b: f64, c: f64| -> f64 {
            (((a * a + b * b - c * c) / (2.0 * a * b)).clamp(-1.0, 1.0))
                .acos()
                .to_degrees()
        };
        let tmin = ang(e0, e2, e1).min(ang(e0, e1, e2)).min(ang(e1, e2, e0));
        min_angle = min_angle.min(tmin);
        entry.1 = entry.1.min(tmin);

        // Normal deviation from the true surface.
        if let Some(dev) = analytic_facet_deviation_deg(model, fid, &p0, &p1, &p2, &geo, tol) {
            max_dev = max_dev.max(dev);
            entry.2 = entry.2.max(dev);
        }

        // Boundary conformance: a facet bridging across a periodic/closed lateral.
        if let (Some(face), Some(uv0), Some(uv1), Some(uv2)) =
            (model.faces.get(fid), v0.uv, v1.uv, v2.uv)
        {
            if let Some(surface) = model.surfaces.get(face.surface_id) {
                if let Some(p) = surface.period_u() {
                    if periodic_coverage(uv0.0, uv1.0, uv2.0, p) > p * 0.5 {
                        boundary_crossing += 1;
                        entry.3 += 1;
                    }
                }
            }
        }
    }

    let mut defects: Vec<MeshFaceQualityDefect> = per_face
        .into_iter()
        .filter_map(|(fid, (asp, ang, dev, bx))| {
            let clean = bx == 0
                && dev <= MeshQuality::MAX_NORMAL_DEVIATION_DEG
                && asp <= MeshQuality::MAX_ASPECT_RATIO
                && ang >= MeshQuality::MIN_ANGLE_DEG;
            if clean {
                return None;
            }
            Some(MeshFaceQualityDefect {
                face_id: fid as u64,
                worst_aspect_ratio: asp,
                min_angle_deg: ang,
                max_normal_deviation_deg: dev,
                boundary_crossing_facets: bx,
            })
        })
        .collect();
    defects.sort_by(|a, b| {
        b.boundary_crossing_facets
            .cmp(&a.boundary_crossing_facets)
            .then(
                a.min_angle_deg
                    .partial_cmp(&b.min_angle_deg)
                    .unwrap_or(Equal),
            )
            .then(
                b.worst_aspect_ratio
                    .partial_cmp(&a.worst_aspect_ratio)
                    .unwrap_or(Equal),
            )
            .then(
                b.max_normal_deviation_deg
                    .partial_cmp(&a.max_normal_deviation_deg)
                    .unwrap_or(Equal),
            )
            .then(a.face_id.cmp(&b.face_id))
    });
    let worst_face = defects.into_iter().next();

    Some(MeshQuality::evaluate(
        mesh.triangles.len(),
        worst_aspect,
        min_angle,
        max_dev,
        boundary_crossing,
        worst_face,
    ))
}

/// Pure per-mesh tessellation-quality scoring — the heart of
/// [`tessellation_quality`], split out so it can be unit-tested against
/// hand-built meshes (a deliberately inverted-normal facet, a zero-area sliver)
/// without driving a tessellator. Reads the mesh `face_map` for per-face
/// localisation; falls back to a single face id `0` when the map is absent.
pub fn score_mesh_tessellation(
    mesh: &crate::tessellation::mesh::TriangleMesh,
) -> crate::primitives::provenance::TessellationQuality {
    use crate::primitives::provenance::{TessFaceDefect, TessellationQuality};
    let has_faces = mesh.face_map.len() == mesh.triangles.len();

    // Per-face accumulators: (triangles, degenerate, agree, comparable).
    let mut per_face: HashMap<u32, (usize, usize, usize, usize)> = HashMap::new();
    let mut degenerate_triangles = 0usize;
    let mut agree = 0usize;
    let mut comparable = 0usize;

    for (i, tri) in mesh.triangles.iter().enumerate() {
        let v0 = mesh.vertices[tri[0] as usize];
        let v1 = mesh.vertices[tri[1] as usize];
        let v2 = mesh.vertices[tri[2] as usize];
        let geo = (v1.position - v0.position).cross(&(v2.position - v0.position));
        let fid = if has_faces { mesh.face_map[i] } else { 0 };
        let entry = per_face.entry(fid).or_insert((0, 0, 0, 0));
        entry.0 += 1;
        if geo.magnitude() <= 1e-12 {
            degenerate_triangles += 1;
            entry.1 += 1;
            continue;
        }
        comparable += 1;
        entry.3 += 1;
        // Stored vertex normals = the analytic intent; winding = the geometry.
        let stored = v0.normal + v1.normal + v2.normal;
        if stored.dot(&geo) > 0.0 {
            agree += 1;
            entry.2 += 1;
        }
    }

    let normal_agreement = if comparable == 0 {
        1.0
    } else {
        agree as f64 / comparable as f64
    };
    let inconsistent_facets = comparable.saturating_sub(agree);

    // Worst defective face, chosen deterministically (HashMap order is arbitrary):
    // lowest per-face agreement, then most degenerate, then lowest face id.
    let mut defects: Vec<TessFaceDefect> = per_face
        .into_iter()
        .filter_map(|(fid, (tris, degen, fa_agree, fa_cmp))| {
            let fa = if fa_cmp == 0 {
                1.0
            } else {
                fa_agree as f64 / fa_cmp as f64
            };
            if degen == 0 && fa >= TessellationQuality::MIN_NORMAL_AGREEMENT {
                return None; // this face is clean
            }
            Some(TessFaceDefect {
                face_id: fid as u64,
                triangles: tris,
                degenerate_triangles: degen,
                normal_agreement: fa,
                // Mesh-only scorer has no surface access; analytic agreement is
                // the full `tessellation_quality` path's job.
                analytic_normal_agreement: 1.0,
            })
        })
        .collect();
    defects.sort_by(|a, b| {
        a.normal_agreement
            .partial_cmp(&b.normal_agreement)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.degenerate_triangles.cmp(&a.degenerate_triangles))
            .then(a.face_id.cmp(&b.face_id))
    });
    let worst_face = defects.into_iter().next();

    // Mesh-only verdict: analytic agreement defaults to perfect (no surface
    // access here); off-surface facets = 0.
    TessellationQuality::evaluate(
        mesh.triangles.len(),
        degenerate_triangles,
        normal_agreement,
        1.0,
        inconsistent_facets,
        0,
        worst_face,
    )
}

/// Convenience: is `solid` a valid closed, oriented 2-manifold at the given
/// chord tolerance? Uses a `1e-6` weld epsilon.
pub fn is_manifold(model: &BRepModel, solid: SolidId, chord: f64) -> bool {
    manifold_report(model, solid, chord, 1e-6)
        .map(|r| r.is_valid_solid())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::transform::translate;
    use crate::primitives::topology_builder::TopologyBuilder;

    fn last_solid(model: &BRepModel) -> SolidId {
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    #[test]
    fn primitives_are_watertight() {
        // Box (exact), sphere and cylinder (curved, faceted) must all enclose
        // their analytic volume within the faceting tolerance.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let box_solid = last_solid(&model);
        assert!(
            is_watertight(&mut model, box_solid, 0.01, 1e-6),
            "box leaks"
        );

        let mut m2 = BRepModel::new();
        TopologyBuilder::new(&mut m2)
            .create_sphere_3d(Vector3::new(0.0, 0.0, 0.0), 3.0)
            .expect("sphere");
        let sphere = last_solid(&m2);
        assert!(is_watertight(&mut m2, sphere, 0.01, 0.03), "sphere leaks");

        let mut m3 = BRepModel::new();
        TopologyBuilder::new(&mut m3)
            .create_cylinder_3d(Vector3::new(0.0, 0.0, 0.0), Vector3::Z, 2.0, 5.0)
            .expect("cylinder");
        let cyl = last_solid(&m3);
        assert!(is_watertight(&mut m3, cyl, 0.01, 0.03), "cylinder leaks");
    }

    #[test]
    fn boolean_result_is_watertight() {
        // A union of two overlapping boxes must itself be a closed solid.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("a");
        let a = last_solid(&model);
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("b");
        let b = last_solid(&model);
        translate(&mut model, vec![b], Vector3::X, 2.0, Default::default()).expect("translate");

        let result = boolean_operation(
            &mut model,
            a,
            b,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("union");
        assert!(
            is_watertight(&mut model, result, 0.01, 1e-3),
            "boolean union result is not watertight"
        );
    }

    #[test]
    fn mesh_volume_matches_analytic_for_a_box() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 3.0, 4.0)
            .expect("box");
        let solid = last_solid(&model);
        let analytic = analytic_volume(&mut model, solid).expect("analytic");
        let mesh = mesh_volume(&model, solid, 0.01).expect("mesh");
        assert!((analytic - 24.0).abs() < 1e-6, "analytic {analytic}");
        assert!((mesh - 24.0).abs() < 1e-6, "mesh {mesh}");
    }

    // ── Manifold oracle ─────────────────────────────────────────────────

    #[test]
    #[ignore = "diagnostic: print manifold reports for all primitives"]
    fn diag_primitive_manifold_reports() {
        let cases: Vec<(&str, Box<dyn Fn(&mut BRepModel)>)> = vec![
            (
                "box",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_box_3d(2.0, 2.0, 2.0)
                        .unwrap();
                }),
            ),
            (
                "sphere",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_sphere_3d(Vector3::ZERO, 3.0)
                        .unwrap();
                }),
            ),
            (
                "cylinder",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_cylinder_3d(Vector3::ZERO, Vector3::Z, 2.0, 5.0)
                        .unwrap();
                }),
            ),
            (
                "cone",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_cone_3d(Vector3::ZERO, Vector3::Z, 2.0, 0.0, 5.0)
                        .unwrap();
                }),
            ),
            (
                "cone-frustum",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_cone_3d(Vector3::ZERO, Vector3::Z, 2.0, 1.0, 5.0)
                        .unwrap();
                }),
            ),
            (
                "torus",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_torus_3d(Vector3::ZERO, Vector3::Z, 3.0, 1.0)
                        .unwrap();
                }),
            ),
        ];
        for (name, build) in cases {
            let mut m = BRepModel::new();
            build(&mut m);
            let s = last_solid(&m);
            match manifold_report(&m, s, 0.05, 1e-6) {
                Some(r) => eprintln!(
                    "{name:>14}: valid={} closed={} manifold={} oriented={} \
                     bnd={} nonman={} inconsistent={} euler={} comp={} tris={}",
                    r.is_valid_solid(),
                    r.closed,
                    r.manifold,
                    r.oriented,
                    r.boundary_edges,
                    r.nonmanifold_edges,
                    r.inconsistent_directed_edges,
                    r.euler_characteristic,
                    r.components,
                    r.triangles,
                ),
                None => eprintln!("{name:>14}: NO MESH"),
            }
        }
    }

    #[test]
    fn primitives_are_valid_manifolds() {
        // Each primitive's tessellation must be a closed, oriented 2-manifold
        // with the genus-0 Euler characteristic 2.
        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let b = last_solid(&m);
        let r = manifold_report(&m, b, 0.05, 1e-6).expect("box mesh");
        assert!(r.is_valid_solid(), "box not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "box euler: {r:?}");
        assert_eq!(r.components, 1, "box components: {r:?}");

        let mut m2 = BRepModel::new();
        TopologyBuilder::new(&mut m2)
            .create_sphere_3d(Vector3::ZERO, 3.0)
            .expect("sphere");
        let s = last_solid(&m2);
        let r = manifold_report(&m2, s, 0.05, 1e-6).expect("sphere mesh");
        assert!(r.is_valid_solid(), "sphere not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "sphere euler: {r:?}");

        let mut m3 = BRepModel::new();
        TopologyBuilder::new(&mut m3)
            .create_cylinder_3d(Vector3::ZERO, Vector3::Z, 2.0, 5.0)
            .expect("cylinder");
        let c = last_solid(&m3);
        let r = manifold_report(&m3, c, 0.05, 1e-6).expect("cyl mesh");
        assert!(r.is_valid_solid(), "cylinder not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "cylinder euler: {r:?}");
    }

    #[test]
    fn box_union_is_a_valid_manifold() {
        // Overlapping-box union must close into a single valid genus-0 manifold.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("a");
        let a = last_solid(&model);
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("b");
        let b = last_solid(&model);
        translate(&mut model, vec![b], Vector3::X, 2.0, Default::default()).expect("translate");
        let result = boolean_operation(
            &mut model,
            a,
            b,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("union");
        let r = manifold_report(&model, result, 0.05, 1e-6).expect("union mesh");
        assert!(r.is_valid_solid(), "box union not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "union euler: {r:?}");
    }

    /// NON-VACUOUS AFTER THE AUDIT-PERF CHANGE: coarsening the audit's ANALYTIC
    /// volume to `audit_volume` (and scaling the spherical-fan budget with
    /// `max_segments`) must NOT make the leak detector blind. A box with one face
    /// removed is a genuine open shell — a hole the size of a whole face — and the
    /// connectivity oracle (`manifold_report`, the audit's topological leak check)
    /// must still flag it: `boundary_edges > 0`, not a valid solid. This is the
    /// regression gate proving the faster audit still SEES leaks.
    ///
    /// (Note: `is_watertight` — the volume-AGREEMENT check — is by construction
    /// blind to a removed face, as its own docstring states: both the analytic and
    /// mesh sides integrate the SAME open mesh, so they agree. That is exactly why
    /// `manifold_report` exists and why the audit runs BOTH. This test pins the
    /// detector that actually catches a torn-face leak.)
    #[test]
    fn open_box_leak_is_still_detected_after_audit_coarsening() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 8.0, 6.0)
            .expect("box");
        let solid = last_solid(&model);

        // Precondition: the intact box is watertight AND a valid manifold.
        assert!(
            is_watertight(&mut model, solid, 0.3, 0.06),
            "precondition: intact box must be watertight"
        );
        let intact = manifold_report(&model, solid, 0.3, 1e-6).expect("intact mesh");
        assert!(
            intact.is_valid_solid() && intact.boundary_edges == 0,
            "precondition: intact box must be a closed valid manifold: {intact:?}"
        );

        // Tear a face out → an open boundary the size of a whole face.
        let shell_id = model.solids.get(solid).expect("solid").outer_shell;
        let face_to_remove = *model
            .shells
            .get(shell_id)
            .expect("shell")
            .faces
            .first()
            .expect("box has faces");
        let removed = model
            .shells
            .get_mut(shell_id)
            .expect("shell")
            .remove_face(face_to_remove);
        assert!(removed, "face removal must succeed");

        // The connectivity oracle MUST still catch it after the perf change.
        let leaky = manifold_report(&model, solid, 0.3, 1e-6).expect("leaky mesh");
        assert!(
            leaky.boundary_edges > 0,
            "manifold_report must report boundary (open) edges for the torn box: {leaky:?}"
        );
        assert!(
            !leaky.is_valid_solid(),
            "manifold_report must reject the open box: {leaky:?}"
        );
    }

    /// `audit_volume` (the new coarse, non-caching analytic volume the audit uses)
    /// must agree with the exact analytic volume on the primitives — coarse is
    /// fine for a sanity band, but it must not be WRONG. A 2×3×4 box is exactly 24
    /// at any density; a sphere/cylinder must land within the coarse faceting
    /// band. This proves the audit's volume source is sound, not merely cheap.
    #[test]
    fn audit_volume_is_accurate_on_primitives() {
        let mut mb = BRepModel::new();
        TopologyBuilder::new(&mut mb)
            .create_box_3d(2.0, 3.0, 4.0)
            .expect("box");
        let b = last_solid(&mb);
        let vb = mb.audit_volume(b).expect("box audit volume");
        assert!((vb - 24.0).abs() < 1e-6, "box audit volume {vb} ≠ 24");

        let mut ms = BRepModel::new();
        TopologyBuilder::new(&mut ms)
            .create_sphere_3d(Vector3::ZERO, 3.0)
            .expect("sphere");
        let s = last_solid(&ms);
        let vs = ms.audit_volume(s).expect("sphere audit volume");
        let true_sphere = 4.0 / 3.0 * std::f64::consts::PI * 27.0;
        assert!(
            (vs - true_sphere).abs() / true_sphere < 0.05,
            "sphere audit volume {vs} vs {true_sphere} (>5% off — coarse audit too inaccurate)"
        );

        let mut mc = BRepModel::new();
        TopologyBuilder::new(&mut mc)
            .create_cylinder_3d(Vector3::ZERO, Vector3::Z, 2.0, 5.0)
            .expect("cyl");
        let c = last_solid(&mc);
        let vc = mc.audit_volume(c).expect("cyl audit volume");
        let true_cyl = std::f64::consts::PI * 4.0 * 5.0;
        assert!(
            (vc - true_cyl).abs() / true_cyl < 0.05,
            "cylinder audit volume {vc} vs {true_cyl} (>5% off)"
        );
    }

    /// The oracle is strict enough to FAIL on a known-broken result: the
    /// sphere-poke-through (sphere r=2.5 through a 4-box). Once mis-stitched the
    /// spherical face (tracked #53/#54); the curved-Boolean poke-matrix work
    /// (#53/#59/#60, 33/33) fixed the partition. This now asserts the intersection
    /// is a valid closed/manifold/oriented solid, so any regression of that work
    /// is caught by the fast lib-test gate (not only the heavy poke-matrix run).
    #[test]
    fn sphere_poke_through_is_manifold() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("box");
        let a = last_solid(&model);
        TopologyBuilder::new(&mut model)
            .create_sphere_3d(Vector3::ZERO, 2.5)
            .expect("sphere");
        let b = last_solid(&model);
        let result = boolean_operation(
            &mut model,
            a,
            b,
            BooleanOp::Intersection,
            BooleanOptions::default(),
        )
        .expect("intersection");
        let r = manifold_report(&model, result, 0.05, 1e-6).expect("mesh");
        assert!(
            r.is_valid_solid(),
            "sphere poke-through ∩ must be a valid closed/manifold/oriented solid: {r:?}"
        );
    }
}
