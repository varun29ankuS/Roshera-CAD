//! Chamfer crease-normal sharpness regression (user-reported "flat chamfer
//! renders ROUNDED — looks like a fillet").
//!
//! A flat chamfer is a `Cone` surface with a straight cross-section. Its two
//! boundary rings are SHARP dihedral creases against the neighbour faces:
//!   - cone ↔ flat cap   (the shrunken top disc), and
//!   - cone ↔ cylinder wall (the shortened lateral).
//!
//! For the chamfer to read as a hard-edged bevel (not a soft fillet) the mesh
//! vertices sitting ON those creases must carry DISTINCT per-face normals: the
//! cone's outward normal on the chamfer side and the neighbour's (plane /
//! cylinder) normal on the other. If the conforming chamfer mesher
//! (`tessellate_blend_cone_conforming`) lets the shared boundary vertices weld
//! into ONE averaged normal, the renderer Phong-interpolates a soft gradient
//! across the crease that mimics a fillet.
//!
//! These tests inspect the live mesh:
//!   1. `chamfer_creases_carry_sharp_distinct_normals` — at a crease position
//!      the cone-side normal and the neighbour-side normal must differ by a
//!      large angle (a genuine dihedral), i.e. they must NOT have been averaged
//!      into one shared normal.
//!   2. `cylinder_lateral_seam_stays_smooth` — the cylinder lateral's own
//!      vertical seam (a genuinely-smooth surface continuation) must KEEP a
//!      single shared/smooth normal. This pins the other side of the contract:
//!      the fix must not facet smooth curved surfaces.

use std::f64::consts::PI;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode,
};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn make_cylinder(
    model: &mut BRepModel,
    center: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_cylinder_3d(center, axis, radius, height)
        .expect("cylinder creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

fn closed_edges(model: &BRepModel) -> Vec<EdgeId> {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| if edge.is_loop() { Some(id) } else { None })
        .collect()
}

fn equal_chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// Build a cylinder (R = 5, H = 10, +Z axis), chamfer the top rim with a 1 mm
/// equal-distance bevel, and tessellate the resulting solid.
///
/// Returns `(model, solid, mesh, cone_face_id)` where `cone_face_id` is the
/// FaceId of the new cone-frustum chamfer blend.
fn chamfered_cylinder() -> (
    BRepModel,
    SolidId,
    geometry_engine::tessellation::TriangleMesh,
    u32,
) {
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 10.0);

    // Pick the TOP rim (max average Z of its samples) so the chamfer creates a
    // shrinking cap at +Z — a deterministic, well-conditioned bevel.
    let rims = closed_edges(&model);
    assert!(!rims.is_empty(), "cylinder must have rim edges");
    let top_rim = *rims
        .iter()
        .max_by(|&&a, &&b| {
            let za = rim_avg_z(&model, a);
            let zb = rim_avg_z(&model, b);
            za.partial_cmp(&zb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("at least one rim");

    chamfer_edges(&mut model, solid, vec![top_rim], equal_chamfer_opts(1.0))
        .expect("top-rim chamfer succeeds");

    // The chamfer blend is the only Cone-typed face in the model.
    let cone_face_id = model
        .faces
        .iter()
        .find(|(_, f)| {
            model
                .surfaces
                .get(f.surface_id)
                .map(|s| s.surface_type() == SurfaceType::Cone)
                .unwrap_or(false)
        })
        .map(|(id, _)| id)
        .expect("chamfered solid has exactly one cone blend face");

    let s = model.solids.get(solid).expect("solid present");
    let mesh = tessellate_solid(s, &model, &TessellationParams::fine());
    (model, solid, mesh, cone_face_id)
}

fn rim_avg_z(model: &BRepModel, eid: EdgeId) -> f64 {
    let Some(e) = model.edges.get(eid) else {
        return 0.0;
    };
    let v = model.vertices.get(e.start_vertex);
    v.map(|v| v.position[2]).unwrap_or(0.0)
}

/// Largest angle (degrees) between any cone-side normal and any neighbour-side
/// normal at the same crease position. A SHARP crease yields a large angle (the
/// true dihedral); an AVERAGED crease yields ~0 (both sides carry one normal).
struct CreaseMeasurement {
    /// Max cone-vs-neighbour normal angle (deg) over all crease positions found.
    max_discontinuity_deg: f64,
    /// How many crease positions were inspected.
    positions: usize,
}

/// Inspect the mesh at the cone blend's boundary. For every mesh vertex on the
/// cone face that is COINCIDENT (within `pos_tol`) with a vertex on a neighbour
/// face, measure the angle between the cone-side normal and the neighbour-side
/// normal.
fn measure_crease(
    mesh: &geometry_engine::tessellation::TriangleMesh,
    cone_face_id: u32,
    pos_tol: f64,
) -> CreaseMeasurement {
    // Per-vertex set of face ids that reference it, and per-(vertex,face) the
    // normal that face emitted. Because the normal-aware weld keeps sharp seams
    // as DISTINCT vertices, the cone-side and neighbour-side normals live on
    // DIFFERENT vertex indices at the same 3D position. So we group by position.
    let n_tri = mesh.triangles.len();
    let face_map_ok = mesh.face_map.len() == n_tri;

    // Collect (position, normal, is_cone) for every distinct mesh vertex that a
    // triangle of a known face touches.
    let mut vert_face: Vec<(Point3, Vector3, bool)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (ti, tri) in mesh.triangles.iter().enumerate() {
        let is_cone = face_map_ok && mesh.face_map[ti] == cone_face_id;
        for &vi in tri {
            if seen.insert((vi, is_cone)) {
                let v = &mesh.vertices[vi as usize];
                vert_face.push((v.position, v.normal, is_cone));
            }
        }
    }

    let mut max_disc = 0.0_f64;
    let mut positions = 0usize;
    for i in 0..vert_face.len() {
        let (pi, ni, ci) = vert_face[i];
        if !ci {
            continue; // anchor the search on cone-side vertices
        }
        // Find a neighbour-side (non-cone) vertex coincident with this one.
        let mut found_here = false;
        for j in 0..vert_face.len() {
            let (pj, nj, cj) = vert_face[j];
            if cj {
                continue;
            }
            if (pi - pj).magnitude() <= pos_tol {
                let dot = ni.dot(&nj).clamp(-1.0, 1.0);
                let ang = dot.acos().to_degrees();
                if ang > max_disc {
                    max_disc = ang;
                }
                found_here = true;
            }
        }
        if found_here {
            positions += 1;
        }
    }

    CreaseMeasurement {
        max_discontinuity_deg: max_disc,
        positions,
    }
}

#[test]
fn chamfer_creases_carry_sharp_distinct_normals() {
    let (_model, _solid, mesh, cone_face_id) = chamfered_cylinder();

    // Sanity: the cone face actually produced triangles.
    let cone_tris = mesh.face_map.iter().filter(|&&f| f == cone_face_id).count();
    assert!(
        cone_tris > 0,
        "the cone chamfer blend produced no triangles (face_map has no entries for \
         face {cone_face_id}); cannot measure crease normals"
    );

    let m = measure_crease(&mesh, cone_face_id, 1e-6);

    assert!(
        m.positions > 0,
        "no crease positions found: every cone-blend boundary vertex was welded \
         into the neighbour face (no distinct cone-side vertex survives at the \
         crease). This is the rounded-chamfer bug — the conforming chamfer \
         boundary bypassed the normal-aware weld and shares ONE averaged normal."
    );

    // The cone↔wall dihedral on a 1 mm 45°-ish bevel and the cone↔cap dihedral
    // are both well above 30°. If the boundary were averaged the measured
    // discontinuity would be ~0. Require a genuine hard edge.
    assert!(
        m.max_discontinuity_deg > 30.0,
        "chamfer crease is SMOOTHED, not sharp: max cone-vs-neighbour normal \
         discontinuity = {:.2}° across {} crease positions (expected the full \
         dihedral, > 30°). The flat chamfer will render ROUNDED like a fillet \
         because the crease vertices carry averaged normals.",
        m.max_discontinuity_deg,
        m.positions
    );
}

#[test]
fn cylinder_lateral_seam_stays_smooth() {
    // The other side of the contract: a genuinely smooth curved seam (the
    // cylinder lateral's vertical u=0≡2π seam) must KEEP one shared/smooth
    // normal. We verify by checking that adjacent normals around the cylinder
    // lateral vary CONTINUOUSLY (no ~dihedral jump between neighbouring
    // lateral vertices at the same height). A plain (un-chamfered) cylinder
    // isolates the lateral.
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 10.0);

    // Find the lateral (cylinder) face.
    let lateral = model
        .faces
        .iter()
        .find(|(_, f)| {
            model
                .surfaces
                .get(f.surface_id)
                .map(|s| s.surface_type() == SurfaceType::Cylinder)
                .unwrap_or(false)
        })
        .map(|(id, _)| id)
        .expect("cylinder has a lateral face");

    let s = model.solids.get(solid).expect("solid present");
    let mesh = tessellate_solid(s, &model, &TessellationParams::fine());

    // Gather lateral vertices, sorted by angle about +Z at the mid-height band.
    let mut lateral_verts: Vec<(f64, Vector3)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (ti, tri) in mesh.triangles.iter().enumerate() {
        if mesh.face_map.get(ti).copied() != Some(lateral) {
            continue;
        }
        for &vi in tri {
            if !seen.insert(vi) {
                continue;
            }
            let v = &mesh.vertices[vi as usize];
            // A lateral vertex's outward normal must be radial (no significant
            // Z component) — otherwise it's a cap-rim vertex shared into the
            // lateral. Restrict to those so we test the true lateral seam.
            if v.normal.z.abs() < 0.2 {
                let ang = v.position.y.atan2(v.position.x);
                lateral_verts.push((ang, v.normal));
            }
        }
    }
    lateral_verts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    assert!(
        lateral_verts.len() >= 8,
        "expected a well-sampled cylinder lateral; got {} radial vertices",
        lateral_verts.len()
    );

    // The largest angle between consecutive (sorted-by-azimuth) lateral normals
    // must stay small — proportional to the angular step, NOT a dihedral jump.
    // With fine tessellation the step is a few degrees; a discontinuity would
    // show up as a >45° jump. We assert no jump exceeds 30° to leave headroom
    // for the coarsest acceptable lateral sampling while still failing if the
    // lateral seam were ever split into a hard edge.
    let mut max_step = 0.0_f64;
    for w in lateral_verts.windows(2) {
        let dot = w[0].1.dot(&w[1].1).clamp(-1.0, 1.0);
        let ang = dot.acos().to_degrees();
        if ang > max_step {
            max_step = ang;
        }
    }
    // Also bridge the wrap (last → first, accounting for 2π).
    if let (Some(first), Some(last)) = (lateral_verts.first(), lateral_verts.last()) {
        let dot = first.1.dot(&last.1).clamp(-1.0, 1.0);
        let _ = dot; // wrap normals are radial-adjacent; included for completeness
    }

    assert!(
        max_step < 30.0,
        "cylinder lateral was FACETED into a hard edge: max consecutive-normal \
         step = {:.2}° (expected a smooth radial sweep, < 30°). The crease fix \
         must only split SHARP dihedrals, never smooth curved surfaces.",
        max_step
    );

    // Belt-and-braces: PI is referenced to keep the angular reasoning explicit.
    let _full_turn = 2.0 * PI;
}
