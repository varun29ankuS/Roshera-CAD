//! A quadratic tetrahedral mesh of one solid, made by the mesher engine from
//! the part's exact geometry (STEP), with every boundary triangle tied back
//! to the kernel face it lies on.
//!
//! The mesher sees the exact surfaces, so it can grade sizes by curvature
//! (fillets, holes); its surface numbering is its own, so each boundary
//! triangle is classified to the nearest kernel face by distance to the
//! part's certified tessellation -- per triangle, so a kernel face the
//! mesher merges or splits still gets exactly its own triangles. The
//! largest distance from any boundary node to the part's surface is
//! measured and reported: how far the mesh strays from the part.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde::{Deserialize, Serialize};

use crate::engines::{run, EngineError, EngineInfo};

/// A quadratic tetrahedral mesh.
#[derive(Debug, Clone, Default)]
pub struct VolumeMesh {
    pub nodes: BTreeMap<u64, [f64; 3]>,
    /// 10-node tetrahedra in the MESHER's node order (Gmsh: corners, then
    /// mid-sides of 0-1, 1-2, 0-2, 0-3, 2-3, 1-3).
    pub tets: Vec<[u64; 10]>,
    /// 6-node boundary triangles, each with the kernel face it lies on.
    pub boundary: Vec<BoundaryTri>,
}

/// One boundary triangle: its nodes (corners, then mid-sides of 0-1, 1-2,
/// 2-0) and its kernel face.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryTri {
    pub nodes: [u64; 6],
    pub face: FaceId,
}

/// What the mesh is, for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshStats {
    pub size_mm: f64,
    pub nodes: usize,
    pub elements: usize,
    /// The largest distance from a boundary node to the part's surface
    /// (measured against its certified tessellation), mm.
    pub boundary_deviation_mm: f64,
    pub mesher_seconds: f64,
    /// Element shape, normalised mean ratio (1 = a regular tetrahedron,
    /// toward 0 = a flat sliver): the worst element and the average.
    #[serde(default)]
    pub quality_min: f64,
    #[serde(default)]
    pub quality_mean: f64,
    /// Elements below 0.2 -- shapes that degrade the stress near them.
    #[serde(default)]
    pub poor_elements: usize,
}

/// Why a mesh could not be made.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MeshError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("the part could not be written for the mesher: {0}")]
    Export(String),
    #[error("the mesher's output could not be read: {0}")]
    Read(String),
    #[error("the mesher produced no tetrahedra for the part")]
    Empty,
    #[error("a boundary triangle lies on no face of the part (nearest is {0:.3} mm away)")]
    Unclassified(f64),
}

/// How a mesh is graded beyond its general size.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshOptions {
    /// Elements around a full circle on curved faces (Gmsh's
    /// curvature-based size); `None`: 12. `Some(0)`: off.
    pub curve_elements: Option<u32>,
    /// Finer elements near chosen faces: each a box around a face, `size`
    /// inside, growing back to the general size over `reach`.
    pub boxes: Vec<SizeBox>,
    /// Finer elements around a stress peak (refinement at the peak).
    pub ball: Option<Refine>,
}

/// A local size: elements of at most `size` within the box, back to the
/// general size over `reach` outside it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SizeBox {
    pub lo: [f64; 3],
    pub hi: [f64; 3],
    pub size: f64,
    pub reach: f64,
}

/// Finer elements inside a ball: where a stress peak did not settle, the
/// mesh is refined there (`size` inside `radius` of `center`), growing back
/// to the general size over another radius.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Refine {
    pub center: [f64; 3],
    pub radius: f64,
    pub size: f64,
}

/// Mesh `solid` with largest element `size_mm`, in `workdir`, through
/// `mesher`.
pub fn mesh_solid(
    model: &BRepModel,
    solid: SolidId,
    size_mm: f64,
    mesher: &EngineInfo,
    workdir: &Path,
    limit: Duration,
) -> Result<(VolumeMesh, MeshStats), MeshError> {
    mesh_solid_graded(
        model,
        solid,
        size_mm,
        &MeshOptions::default(),
        mesher,
        workdir,
        limit,
    )
}

/// [`mesh_solid`], graded by `options` (curvature, local sizes, a peak ball).
pub fn mesh_solid_graded(
    model: &BRepModel,
    solid: SolidId,
    size_mm: f64,
    options: &MeshOptions,
    mesher: &EngineInfo,
    workdir: &Path,
    limit: Duration,
) -> Result<(VolumeMesh, MeshStats), MeshError> {
    std::fs::create_dir_all(workdir).map_err(|e| MeshError::Export(e.to_string()))?;
    let step = workdir.join("part.step");
    let file = std::fs::File::create(&step).map_err(|e| MeshError::Export(e.to_string()))?;
    // This solid only: the mesher meshes every body in the file it is given,
    // and a document holds every part.
    export_engine::formats::step::writer::write_solid_step(
        model,
        solid,
        std::io::BufWriter::new(file),
    )
    .map_err(|e| MeshError::Export(e.to_string()))?;
    let (lo, hi) = solid_bounds(model, solid).ok_or(MeshError::Empty)?;
    let pad = 1e-3 * (0..3).map(|i| hi[i] - lo[i]).fold(1.0_f64, f64::max);
    std::fs::write(
        workdir.join("part.geo"),
        geo_script(size_mm, lo, hi, pad, options),
    )
    .map_err(|e| MeshError::Export(e.to_string()))?;
    let took = run(
        mesher,
        &["part.geo", "-3", "-format", "msh2", "-o", "part.msh"],
        workdir,
        &workdir.join("mesher.log"),
        limit,
    )?;
    let text = std::fs::read_to_string(workdir.join("part.msh"))
        .map_err(|e| MeshError::Read(e.to_string()))?;
    let (nodes, tets, tris) = read_msh2(&text)?;
    if tets.is_empty() {
        return Err(MeshError::Empty);
    }
    let (boundary, deviation) = classify(model, solid, &nodes, &tris)?;
    let (quality_min, quality_mean, poor_elements) = shape_quality(&nodes, &tets);
    let stats = MeshStats {
        size_mm,
        nodes: nodes.len(),
        elements: tets.len(),
        boundary_deviation_mm: deviation,
        mesher_seconds: took.as_secs_f64(),
        quality_min,
        quality_mean,
        poor_elements,
    };
    Ok((
        VolumeMesh {
            nodes,
            tets,
            boundary,
        },
        stats,
    ))
}

/// The Gmsh script: read the part, keep the volume inside the solid's box,
/// quadratic tets graded by curvature and by `options`' local sizes, every
/// boundary surface its own group.
fn geo_script(size: f64, lo: [f64; 3], hi: [f64; 3], pad: f64, options: &MeshOptions) -> String {
    use std::fmt::Write as _;
    let mut fields = String::new();
    let mut ids: Vec<usize> = Vec::new();
    let mut smallest = size;
    if let Some(r) = &options.ball {
        let id = 1;
        let _ = write!(
            fields,
            "Field[{id}] = Ball;\nField[{id}].Radius = {};\nField[{id}].Thickness = {};\nField[{id}].VIn = {};\nField[{id}].VOut = {size};\nField[{id}].XCenter = {};\nField[{id}].YCenter = {};\nField[{id}].ZCenter = {};\n",
            r.radius, r.radius, r.size, r.center[0], r.center[1], r.center[2]
        );
        ids.push(id);
        smallest = smallest.min(r.size);
    }
    for (k, b) in options.boxes.iter().enumerate() {
        let id = 10 + k;
        let _ = write!(
            fields,
            "Field[{id}] = Box;\nField[{id}].VIn = {};\nField[{id}].VOut = {size};\nField[{id}].XMin = {};\nField[{id}].YMin = {};\nField[{id}].ZMin = {};\nField[{id}].XMax = {};\nField[{id}].YMax = {};\nField[{id}].ZMax = {};\nField[{id}].Thickness = {};\n",
            b.size, b.lo[0], b.lo[1], b.lo[2], b.hi[0], b.hi[1], b.hi[2], b.reach
        );
        ids.push(id);
        smallest = smallest.min(b.size);
    }
    if !ids.is_empty() {
        let list: Vec<String> = ids.iter().map(usize::to_string).collect();
        let _ = write!(
            fields,
            "Field[100] = Min;\nField[100].FieldsList = {{{}}};\nBackground Field = 100;\n",
            list.join(", ")
        );
    }
    let curve = options.curve_elements.unwrap_or(12);
    let min = (size / 8.0).min(smallest / 2.0);
    format!(
        r#"SetFactory("OpenCASCADE");
Geometry.OCCTargetUnit = "MM";
Merge "part.step";
v[] = Volume In BoundingBox{{{}, {}, {}, {}, {}, {}}};
Mesh.ElementOrder = 2;
Mesh.MeshSizeMax = {size};
Mesh.MeshSizeMin = {min};
Mesh.MeshSizeFromCurvature = {curve};
{fields}Physical Volume(1) = {{v[]}};
s[] = Boundary{{ Volume{{v[]}}; }};
For i In {{0:#s[]-1}}
  Physical Surface(Abs(s[i])) = {{Abs(s[i])}};
EndFor
"#,
        lo[0] - pad,
        lo[1] - pad,
        lo[2] - pad,
        hi[0] + pad,
        hi[1] + pad,
        hi[2] + pad,
    )
}

/// Element shape over the mesh: the normalised mean ratio of each
/// tetrahedron's corners, `12 (3V)^(2/3) / sum of squared edge lengths`
/// (1 for a regular tetrahedron, toward 0 for a sliver). Returns the worst,
/// the average, and how many fall below 0.2.
pub(crate) fn shape_quality(
    nodes: &BTreeMap<u64, [f64; 3]>,
    tets: &[[u64; 10]],
) -> (f64, f64, usize) {
    let mut worst = 1.0_f64;
    let mut sum = 0.0;
    let mut poor = 0usize;
    let mut counted = 0usize;
    for t in tets {
        let p: Vec<[f64; 3]> = t[..4]
            .iter()
            .filter_map(|n| nodes.get(n).copied())
            .collect();
        if p.len() != 4 {
            continue;
        }
        let d = |a: [f64; 3], b: [f64; 3]| [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let (u, v, w) = (d(p[0], p[1]), d(p[0], p[2]), d(p[0], p[3]));
        let vol = (u[0] * (v[1] * w[2] - v[2] * w[1]) - u[1] * (v[0] * w[2] - v[2] * w[0])
            + u[2] * (v[0] * w[1] - v[1] * w[0]))
            .abs()
            / 6.0;
        let mut edges = 0.0;
        for (i, j) in [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
            let e = d(p[i], p[j]);
            edges += e[0] * e[0] + e[1] * e[1] + e[2] * e[2];
        }
        if !(edges > 0.0) {
            continue;
        }
        let q = (12.0 * (3.0 * vol).powf(2.0 / 3.0) / edges).clamp(0.0, 1.0);
        worst = worst.min(q);
        sum += q;
        counted += 1;
        if q < 0.2 {
            poor += 1;
        }
    }
    if counted == 0 {
        return (0.0, 0.0, 0);
    }
    (worst, sum / counted as f64, poor)
}
fn solid_bounds(model: &BRepModel, solid: SolidId) -> Option<([f64; 3], [f64; 3])> {
    let mesh = tessellate_solid(
        model.solids.get(solid)?,
        model,
        &TessellationParams::default(),
    );
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &mesh.vertices {
        let p = [v.position.x, v.position.y, v.position.z];
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    lo[0].is_finite().then_some((lo, hi))
}

type MshTables = (BTreeMap<u64, [f64; 3]>, Vec<[u64; 10]>, Vec<[u64; 6]>);

/// Nodes, 10-node tetrahedra (type 11) and 6-node triangles (type 9) of a
/// Gmsh msh 2.2 ASCII file.
pub(crate) fn read_msh2(text: &str) -> Result<MshTables, MeshError> {
    let bad = |what: &str| MeshError::Read(what.to_string());
    let mut nodes = BTreeMap::new();
    let mut tets = Vec::new();
    let mut tris = Vec::new();
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        match line {
            "$Nodes" => {
                let n: usize = lines
                    .next()
                    .and_then(|l| l.parse().ok())
                    .ok_or_else(|| bad("node count"))?;
                for _ in 0..n {
                    let row: Vec<f64> = lines
                        .next()
                        .ok_or_else(|| bad("node row"))?
                        .split_whitespace()
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    if row.len() < 4 {
                        return Err(bad("node row"));
                    }
                    nodes.insert(row[0] as u64, [row[1], row[2], row[3]]);
                }
            }
            "$Elements" => {
                let n: usize = lines
                    .next()
                    .and_then(|l| l.parse().ok())
                    .ok_or_else(|| bad("element count"))?;
                for _ in 0..n {
                    let row: Vec<u64> = lines
                        .next()
                        .ok_or_else(|| bad("element row"))?
                        .split_whitespace()
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    if row.len() < 3 {
                        return Err(bad("element row"));
                    }
                    let (etype, ntags) = (row[1], row[2] as usize);
                    let ids = &row[(3 + ntags).min(row.len())..];
                    match (etype, ids.len()) {
                        (11, 10) => {
                            let mut t = [0u64; 10];
                            t.copy_from_slice(ids);
                            tets.push(t);
                        }
                        (9, 6) => {
                            let mut t = [0u64; 6];
                            t.copy_from_slice(ids);
                            tris.push(t);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok((nodes, tets, tris))
}

/// Each boundary triangle's kernel face (the face nearest its centroid), and
/// the largest distance of any boundary node from the part's surface.
fn classify(
    model: &BRepModel,
    solid: SolidId,
    nodes: &BTreeMap<u64, [f64; 3]>,
    tris: &[[u64; 6]],
) -> Result<(Vec<BoundaryTri>, f64), MeshError> {
    let surf = tessellate_solid(
        model.solids.get(solid).ok_or(MeshError::Empty)?,
        model,
        &TessellationParams::default(),
    );
    let pos = |i: u32| {
        let p = surf.vertices[i as usize].position;
        [p.x, p.y, p.z]
    };
    let kernel: Vec<([[f64; 3]; 3], FaceId)> = surf
        .triangles
        .iter()
        .zip(&surf.face_map)
        .map(|(t, &f)| ([pos(t[0]), pos(t[1]), pos(t[2])], f))
        .collect();
    let nearest = |p: [f64; 3]| -> (f64, FaceId) {
        kernel
            .iter()
            .map(|(t, f)| (point_triangle_distance(p, t), *f))
            .fold((f64::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a })
    };
    let node = |i: u64| {
        nodes
            .get(&i)
            .copied()
            .ok_or_else(|| MeshError::Read(format!("node {i}")))
    };
    let mut out = Vec::with_capacity(tris.len());
    let mut deviation = 0.0_f64;
    let mut checked: HashMap<u64, ()> = HashMap::new();
    let extent = nodes
        .values()
        .fold([f64::INFINITY, f64::NEG_INFINITY], |acc, p| {
            [acc[0].min(p[0]), acc[1].max(p[0])]
        });
    let snap = 1e-2 * (extent[1] - extent[0]).abs().max(1.0);
    for t in tris {
        let (a, b, c) = (node(t[0])?, node(t[1])?, node(t[2])?);
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        let (d, face) = nearest(centroid);
        if d > snap {
            return Err(MeshError::Unclassified(d));
        }
        for &n in t {
            if checked.insert(n, ()).is_none() {
                deviation = deviation.max(nearest(node(n)?).0);
            }
        }
        out.push(BoundaryTri { nodes: *t, face });
    }
    Ok((out, deviation))
}

/// The distance from `p` to the triangle `t` (Ericson, Real-Time Collision
/// Detection, closest point on a triangle).
pub(crate) fn point_triangle_distance(p: [f64; 3], t: &[[f64; 3]; 3]) -> f64 {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let [a, b, c] = *t;
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    let dist = |q: [f64; 3]| dot(sub(p, q), sub(p, q)).sqrt();
    let along =
        |o: [f64; 3], v: [f64; 3], s: f64| [o[0] + v[0] * s, o[1] + v[1] * s, o[2] + v[2] * s];
    if d1 <= 0.0 && d2 <= 0.0 {
        return dist(a);
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return dist(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return dist(along(a, ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return dist(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return dist(along(a, ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let bc = sub(c, b);
        return dist(along(b, bc, (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    dist([
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// A point above a triangle's interior, beside an edge, and past a
    /// corner: the closest point is on the face, the edge, the corner.
    #[test]
    fn point_to_triangle_distance_by_region() {
        let t = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]];
        assert!((point_triangle_distance([2.0, 2.0, 3.0], &t) - 3.0).abs() < 1e-12);
        assert!((point_triangle_distance([5.0, -4.0, 0.0], &t) - 4.0).abs() < 1e-12);
        assert!((point_triangle_distance([-3.0, -4.0, 0.0], &t) - 5.0).abs() < 1e-12);
        let hyp = point_triangle_distance([10.0, 10.0, 0.0], &t);
        assert!((hyp - 50f64.sqrt()).abs() < 1e-12, "{hyp}");
    }

    /// The msh 2.2 reader keeps 10-node tets and 6-node triangles, with
    /// their tags skipped, and ignores other element kinds.
    #[test]
    fn msh2_reader_keeps_quadratic_tets_and_triangles() {
        let text = "$MeshFormat\n2.2 0 8\n$EndMeshFormat\n$Nodes\n2\n1 0 0 0\n2 1.5 2 3\n$EndNodes\n$Elements\n3\n1 15 2 0 1 1\n2 9 2 7 7 1 2 3 4 5 6\n3 11 2 1 1 1 2 3 4 5 6 7 8 9 10\n$EndElements\n";
        let (nodes, tets, tris) = read_msh2(text).unwrap();
        assert_eq!(nodes[&2], [1.5, 2.0, 3.0]);
        assert_eq!(tets, vec![[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]]);
        assert_eq!(tris, vec![[1, 2, 3, 4, 5, 6]]);
    }
}
