//! THROWAWAY PROBE (not product code): import one STEP file and explain its
//! first orientation errors -- for the edge each names, the two faces using
//! it: surface type, face sense, the loop (outer/inner) and the direction the
//! loop walks the edge.
//!
//! usage: face_probe <file.step>

use export_engine::ExportEngine;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
    let Ok((mut model, report)) = ExportEngine::new().import_step_content(&text, "probe") else {
        println!("import failed");
        return;
    };
    // PROBE_OPEN: the certificate mesh's (chord 0.1, weld 1e-6) open edges --
    // each with its face, the nearest B-Rep edge, and the nearest vertex the
    // OTHER face on that edge put there.
    if std::env::var_os("PROBE_OPEN").is_some() {
        use geometry_engine::math::Point3;
        use std::collections::HashMap;
        let params = geometry_engine::tessellation::TessellationParams {
            chord_tolerance: 0.1,
            ..geometry_engine::tessellation::TessellationParams::default()
        };
        let ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
        for sid in ids {
            let Some(solid) = model.solids.get(sid) else {
                continue;
            };
            let mesh = geometry_engine::tessellation::tessellate_solid(&solid, &model, &params);
            let key = |p: &Point3| {
                (
                    (p.x * 1e6).round() as i64,
                    (p.y * 1e6).round() as i64,
                    (p.z * 1e6).round() as i64,
                )
            };
            let mut canon: HashMap<(i64, i64, i64), u32> = HashMap::new();
            let w: Vec<u32> = mesh
                .vertices
                .iter()
                .map(|v| {
                    let n = canon.len() as u32;
                    *canon.entry(key(&v.position)).or_insert(n)
                })
                .collect();
            let mut uses: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
            for (ti, t) in mesh.triangles.iter().enumerate() {
                for k in 0..3 {
                    let (a, b) = (w[t[k] as usize], w[t[(k + 1) % 3] as usize]);
                    uses.entry((a.min(b), a.max(b))).or_default().push(ti);
                }
            }
            let mut pos_of: HashMap<u32, Point3> = HashMap::new();
            for (i, v) in mesh.vertices.iter().enumerate() {
                pos_of.insert(w[i], v.position);
            }
            // Per face, its mesh vertices (welded ids).
            let mut verts_of: HashMap<u32, Vec<u32>> = HashMap::new();
            for (ti, t) in mesh.triangles.iter().enumerate() {
                let f = mesh.face_map.get(ti).copied().unwrap_or(u32::MAX);
                for &i in t {
                    verts_of.entry(f).or_default().push(w[i as usize]);
                }
            }
            for v in verts_of.values_mut() {
                v.sort();
                v.dedup();
            }
            let mut open: Vec<((u32, u32), usize)> = uses
                .iter()
                .filter(|(_, t)| t.len() == 1)
                .map(|(e, t)| (*e, t[0]))
                .collect();
            open.sort();
            println!("solid {sid}: {} open mesh edges", open.len());
            // B-Rep edge nearest a point (sampled).
            let edges: Vec<(u32, Vec<Point3>)> = model
                .edges
                .iter()
                .filter_map(|(eid, e)| {
                    let c = model.curves.get(e.curve_id)?;
                    let r = e.param_range;
                    Some((
                        eid,
                        (0..=64)
                            .filter_map(|k| {
                                c.point_at(r.start + (r.end - r.start) * k as f64 / 64.0)
                                    .ok()
                            })
                            .collect(),
                    ))
                })
                .collect();
            for ((a, b), ti) in open.iter().take(40) {
                let (pa, pb) = (pos_of[a], pos_of[b]);
                let mid = Point3::new(
                    (pa.x + pb.x) / 2.0,
                    (pa.y + pb.y) / 2.0,
                    (pa.z + pb.z) / 2.0,
                );
                let f = mesh.face_map.get(*ti).copied().unwrap_or(u32::MAX);
                let (mut be, mut bd) = (u32::MAX, f64::INFINITY);
                for (eid, pts) in &edges {
                    for p in pts {
                        let d = (*p - mid).magnitude();
                        if d < bd {
                            bd = d;
                            be = *eid;
                        }
                    }
                }
                // Faces using that B-Rep edge.
                let users: Vec<u32> = model
                    .faces
                    .iter()
                    .filter(|(_, fc)| {
                        std::iter::once(fc.outer_loop)
                            .chain(fc.inner_loops.iter().copied())
                            .any(|l| model.loops.get(l).is_some_and(|lp| lp.edges.contains(&be)))
                    })
                    .map(|(fid, _)| fid)
                    .collect();
                let other: Vec<String> = users
                    .iter()
                    .filter(|&&u| u != f)
                    .map(|&u| {
                        let near = verts_of
                            .get(&u)
                            .map(|vs| {
                                vs.iter()
                                    .map(|v| {
                                        (pos_of[v] - pa)
                                            .magnitude()
                                            .min((pos_of[v] - pb).magnitude())
                                    })
                                    .fold(f64::INFINITY, f64::min)
                            })
                            .unwrap_or(f64::NAN);
                        let nv = verts_of.get(&u).map(|v| v.len()).unwrap_or(0);
                        format!("face {u} ({nv} verts) nearest {near:.2e}")
                    })
                    .collect();
                println!(
                    "  open ({:.4},{:.4},{:.4})-({:.4},{:.4},{:.4}) len {:.3e} face {f} | brep edge {be} at {bd:.2e} used by {users:?} | {}",
                    pa.x, pa.y, pa.z, pb.x, pb.y, pb.z, (pb - pa).magnitude(), other.join("; ")
                );
            }
        }
        return;
    }
    // PROBE_FACE=<id>: the face's loops, each edge sampled and placed on the
    // surface chart (u, v) by closest point.
    let want = std::env::var("PROBE_FACE").ok();
    let only_empty = std::env::var_os("PROBE_EMPTY").is_some();
    let meshed: std::collections::HashSet<u32> = model
        .solids
        .iter()
        .flat_map(|(_, solid)| {
            geometry_engine::tessellation::tessellate_solid(
                solid,
                &model,
                &geometry_engine::tessellation::TessellationParams::default(),
            )
            .face_map
        })
        .collect();
    let picked: Vec<u32> = model
        .faces
        .iter()
        .filter(|(fid, f)| match want.as_deref() {
            Some(w) if w.parse::<u32>().is_ok() => w.parse::<u32>().ok() == Some(*fid),
            Some(kind) => model
                .surfaces
                .get(f.surface_id)
                .is_some_and(|s| s.type_name() == kind),
            None => false,
        })
        .map(|(fid, _)| fid)
        .filter(|fid| !only_empty || !meshed.contains(fid))
        .collect();
    for fid in picked {
        if let Some(face) = model.faces.get(fid) {
            let surf = model.surfaces.get(face.surface_id);
            println!(
                "face {fid}: {:?}, orientation {:?}, {} inner loops",
                surf.map(|s| s.type_name()),
                face.orientation,
                face.inner_loops.len()
            );
            for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                let Some(lp) = model.loops.get(l) else {
                    continue;
                };
                println!(" loop {l}: {} edges", lp.edges.len());
                for (k, &e) in lp.edges.iter().enumerate() {
                    let Some(ed) = model.edges.get(e) else {
                        continue;
                    };
                    let Some(c) = model.curves.get(ed.curve_id) else {
                        continue;
                    };
                    let fw = lp.orientations.get(k).copied().unwrap_or(true);
                    let r = ed.param_range;
                    let mut uvs = Vec::new();
                    for s in 0..=4 {
                        let f = if fw {
                            s as f64 / 4.0
                        } else {
                            1.0 - s as f64 / 4.0
                        };
                        if let (Ok(pt), Some(sf)) =
                            (c.evaluate(r.start + f * (r.end - r.start)), surf)
                        {
                            if let Ok((u, v)) = sf.closest_point(
                                &pt.position,
                                geometry_engine::math::Tolerance::default(),
                            ) {
                                uvs.push(format!("({u:.3},{v:.3})"));
                            }
                        }
                    }
                    println!(
                        "  edge {e} {} {}: {}",
                        c.type_name(),
                        if fw { "fwd" } else { "rev" },
                        uvs.join(" ")
                    );
                    let gap = |t: f64, v: u32| -> f64 {
                        match (c.evaluate(t), model.vertices.get_position(v)) {
                            (Ok(p), Some(q)) => ((p.position.x - q[0]).powi(2)
                                + (p.position.y - q[1]).powi(2)
                                + (p.position.z - q[2]).powi(2))
                            .sqrt(),
                            _ => f64::NAN,
                        }
                    };
                    println!(
                        "      range [{:.6}, {:.6}] of curve domain {:?}; start gap {:.2e}, end gap {:.2e}",
                        r.start, r.end, c.parameter_range(), gap(r.start, ed.start_vertex), gap(r.end, ed.end_vertex)
                    );
                }
            }
        }
    }
    // Open mesh edges (used by one triangle), grouped by the face and
    // surface type they sit on, in the solid's display tessellation.
    for (_, solid) in model.solids.iter() {
        let mesh = geometry_engine::tessellation::tessellate_solid(
            solid,
            &model,
            &geometry_engine::tessellation::TessellationParams::default(),
        );
        let mut uses: std::collections::HashMap<(u32, u32), (usize, u32)> = Default::default();
        for (i, t) in mesh.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let e = uses
                    .entry((a.min(b), a.max(b)))
                    .or_insert((0, mesh.face_map[i]));
                e.0 += 1;
            }
        }
        let mut by_face: std::collections::BTreeMap<u32, usize> = Default::default();
        for (_, (n, f)) in uses.iter() {
            if *n == 1 {
                *by_face.entry(*f).or_default() += 1;
            }
        }
        let mut kinds: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
        for (f, n) in &by_face {
            let kind = model
                .faces
                .get(*f)
                .and_then(|face| model.surfaces.get(face.surface_id))
                .map(|s| s.type_name())
                .unwrap_or("?");
            let e = kinds.entry(kind).or_default();
            e.0 += 1;
            e.1 += n;
        }
        println!("open mesh edges by surface (faces, edges): {kinds:?}");
        let mut worst: Vec<_> = by_face.into_iter().collect();
        worst.sort_by(|a, b| b.1.cmp(&a.1));
        println!("worst faces: {:?}", &worst[..worst.len().min(8)]);
        for &(f, open) in worst.iter().take(3) {
            let tris: Vec<usize> = (0..mesh.triangles.len())
                .filter(|&i| mesh.face_map[i] == f)
                .collect();
            let verts: std::collections::HashSet<u32> =
                tris.iter().flat_map(|&i| mesh.triangles[i]).collect();
            let Some(face) = model.faces.get(f) else {
                continue;
            };
            let kind = model
                .surfaces
                .get(face.surface_id)
                .map(|s| s.type_name())
                .unwrap_or("?");
            let mut curves: std::collections::BTreeMap<&str, usize> = Default::default();
            let mut gaps = 0.0_f64;
            for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                let Some(lp) = model.loops.get(l) else {
                    continue;
                };
                for &e in &lp.edges {
                    let Some(ed) = model.edges.get(e) else {
                        continue;
                    };
                    if let Some(c) = model.curves.get(ed.curve_id) {
                        *curves.entry(c.type_name()).or_default() += 1;
                        let r = ed.param_range;
                        for (t, v) in [(r.start, ed.start_vertex), (r.end, ed.end_vertex)] {
                            if let (Ok(p), Some(q)) =
                                (c.evaluate(t), model.vertices.get_position(v))
                            {
                                let d = ((p.position.x - q[0]).powi(2)
                                    + (p.position.y - q[1]).powi(2)
                                    + (p.position.z - q[2]).powi(2))
                                .sqrt();
                                gaps = gaps.max(d);
                            }
                        }
                    }
                }
            }
            // How many of this face's mesh vertices coincide with a vertex of
            // ANOTHER face (i.e. would weld), and the nearest miss otherwise.
            let mine: Vec<geometry_engine::math::Point3> = verts
                .iter()
                .map(|&v| mesh.vertices[v as usize].position)
                .collect();
            let others: Vec<geometry_engine::math::Point3> = (0..mesh.triangles.len())
                .filter(|&i| mesh.face_map[i] != f)
                .flat_map(|i| mesh.triangles[i])
                .map(|v| mesh.vertices[v as usize].position)
                .collect();
            let q = 1e5;
            let key = |p: &geometry_engine::math::Point3| {
                (
                    (p.x * q).round() as i64,
                    (p.y * q).round() as i64,
                    (p.z * q).round() as i64,
                )
            };
            let other_keys: std::collections::HashSet<_> = others.iter().map(key).collect();
            let shared = mine.iter().filter(|p| other_keys.contains(&key(p))).count();
            println!(
                "    {shared} of {} vertices coincide (1e-5) with another face's",
                mine.len()
            );
            let mut buckets: std::collections::HashMap<_, Vec<geometry_engine::math::Point3>> =
                Default::default();
            for p in &others {
                buckets.entry(key(p)).or_default().push(*p);
            }
            let mut ds: Vec<f64> = mine
                .iter()
                .filter_map(|p| {
                    let (a, b, c) = key(p);
                    let mut best = f64::INFINITY;
                    for da in -1..=1 {
                        for db in -1..=1 {
                            for dc in -1..=1 {
                                if let Some(v) = buckets.get(&(a + da, b + db, c + dc)) {
                                    for o in v {
                                        best = best.min(o.distance(p));
                                    }
                                }
                            }
                        }
                    }
                    best.is_finite().then_some(best)
                })
                .collect();
            ds.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            if !ds.is_empty() {
                println!(
                    "    nearest other-face vertex: median {:.2e}, 90% {:.2e}, max {:.2e}",
                    ds[ds.len() / 2],
                    ds[ds.len() * 9 / 10],
                    ds[ds.len() - 1]
                );
            }
            println!(
                "  face {f} {kind} {:?}: {} triangles, {} vertices, {open} open edges, {} inner loops, edges {curves:?}, max curve-end-to-vertex gap {gaps:.2e}",
                face.orientation, tris.len(), verts.len(), face.inner_loops.len()
            );
        }
    }
    // The certificate's own manifold mesh: chord 0.1, welded by POSITION at
    // 1e-6. Open edges (one use) by the surface they sit on, and for each
    // whether its two end points exist on some OTHER face (a T-junction / a
    // different sampling of a shared edge) or not (a real gap).
    for (_, solid) in model.solids.iter() {
        let params = geometry_engine::tessellation::TessellationParams {
            chord_tolerance: 0.1,
            ..geometry_engine::tessellation::TessellationParams::default()
        };
        let mesh = geometry_engine::tessellation::tessellate_solid(solid, &model, &params);
        let key = |p: &geometry_engine::math::Point3| {
            (
                (p.x / 1e-6).round() as i64,
                (p.y / 1e-6).round() as i64,
                (p.z / 1e-6).round() as i64,
            )
        };
        let w: Vec<(i64, i64, i64)> = mesh.vertices.iter().map(|v| key(&v.position)).collect();
        let mut uses: std::collections::HashMap<((i64, i64, i64), (i64, i64, i64)), Vec<u32>> =
            Default::default();
        let mut on_face: std::collections::HashMap<
            (i64, i64, i64),
            std::collections::BTreeSet<u32>,
        > = Default::default();
        for (i, t) in mesh.triangles.iter().enumerate() {
            let f = mesh.face_map[i];
            for k in 0..3 {
                let (a, b) = (w[t[k] as usize], w[t[(k + 1) % 3] as usize]);
                if a == b {
                    continue;
                }
                uses.entry((a.min(b), a.max(b))).or_default().push(f);
                on_face.entry(a).or_default().insert(f);
            }
        }
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        for ((a, b), fs) in &uses {
            if fs.len() != 1 {
                continue;
            }
            let f = fs[0];
            let kind = model
                .faces
                .get(f)
                .and_then(|face| model.surfaces.get(face.surface_id))
                .map(|s| s.type_name())
                .unwrap_or("?");
            let shared = |p: &(i64, i64, i64)| {
                on_face
                    .get(p)
                    .is_some_and(|set| set.iter().any(|&g| g != f))
            };
            let class = match (shared(a), shared(b)) {
                (true, true) => "both ends on a neighbour",
                (true, false) | (false, true) => "one end on a neighbour",
                (false, false) => "no neighbour point",
            };
            *kinds.entry(format!("{kind}: {class}")).or_default() += 1;
        }
        let mut shown = 0;
        for ((a, b), fs) in &uses {
            if fs.len() != 1 || shown >= 10 {
                continue;
            }
            shown += 1;
            let p = |k: &(i64, i64, i64)| (k.0 as f64 * 1e-6, k.1 as f64 * 1e-6, k.2 as f64 * 1e-6);
            let (pa, pb) = (p(a), p(b));
            let len =
                ((pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2) + (pa.2 - pb.2).powi(2)).sqrt();
            let na: Vec<u32> = on_face
                .get(a)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();
            let nb: Vec<u32> = on_face
                .get(b)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();
            let f = fs[0];
            let (mut tris, mut flat) = (0usize, 0usize);
            for (i, t) in mesh.triangles.iter().enumerate() {
                if mesh.face_map[i] != f {
                    continue;
                }
                tris += 1;
                let (pa, pb, pc) = (
                    mesh.vertices[t[0] as usize].position,
                    mesh.vertices[t[1] as usize].position,
                    mesh.vertices[t[2] as usize].position,
                );
                if (pb - pa).cross(&(pc - pa)).magnitude() < 1e-9 {
                    flat += 1;
                }
            }
            let fk = model.faces.get(f).map(|face| {
                (
                    model
                        .surfaces
                        .get(face.surface_id)
                        .map(|s| s.type_name())
                        .unwrap_or("?"),
                    model
                        .loops
                        .get(face.outer_loop)
                        .map(|l| l.edges.len())
                        .unwrap_or(0),
                    face.inner_loops.len(),
                )
            });
            println!("    face {f}: {fk:?} (kind, outer edges, holes); {tris} triangles, {flat} of zero area");
            if tris <= 4 {
                for (i, t) in mesh.triangles.iter().enumerate() {
                    if mesh.face_map[i] == f {
                        let q: Vec<String> = t
                            .iter()
                            .map(|&k| {
                                let p = mesh.vertices[k as usize].position;
                                format!("({:.3},{:.3},{:.3})", p.x, p.y, p.z)
                            })
                            .collect();
                        println!("      tri {}", q.join(" "));
                    }
                }
                if let Some(face) = model.faces.get(f) {
                    if let Some(lp) = model.loops.get(face.outer_loop) {
                        for &e in &lp.edges {
                            if let Some(ed) = model.edges.get(e) {
                                let a = model
                                    .vertices
                                    .get_position(ed.start_vertex)
                                    .unwrap_or([0.0; 3]);
                                let b = model
                                    .vertices
                                    .get_position(ed.end_vertex)
                                    .unwrap_or([0.0; 3]);
                                println!("      loop edge {e}: ({:.3},{:.3},{:.3}) -> ({:.3},{:.3},{:.3})", a[0], a[1], a[2], b[0], b[1], b[2]);
                            }
                        }
                    }
                }
            }
            println!(
                "  open edge on face {}: ({:.4},{:.4},{:.4})-({:.4},{:.4},{:.4}) len {len:.4}; faces at ends {na:?} / {nb:?}",
                fs[0], pa.0, pa.1, pa.2, pb.0, pb.1, pb.2
            );
        }
        let total: usize = kinds.values().sum();
        println!("certificate mesh open edges: {total}");
        for (k, n) in &kinds {
            println!("  {n:>6}  {k}");
        }
    }
    // PROBE_WALK: for every "same orientation on edge E" error, recompute the
    // validator's walk test for both faces with (a) the vertex-only polygon it
    // uses and (b) a densely sampled boundary.
    if std::env::var_os("PROBE_WALK").is_some() {
        use geometry_engine::math::Vector3;
        let pip = |poly: &[Vector3], q: Vector3, n: Vector3| -> bool {
            // Project to the plane perpendicular to n, then even-odd ray cast.
            let ax = if n.x.abs() < 0.9 {
                Vector3::new(1.0, 0.0, 0.0)
            } else {
                Vector3::new(0.0, 1.0, 0.0)
            };
            let e1 = n.cross(&ax).normalize_or_zero();
            let e2 = n.cross(&e1);
            let to2 = |p: &Vector3| (p.dot(&e1), p.dot(&e2));
            let (qx, qy) = to2(&q);
            let mut inside = false;
            for i in 0..poly.len() {
                let (ax, ay) = to2(&poly[i]);
                let (bx, by) = to2(&poly[(i + 1) % poly.len()]);
                if (ay > qy) != (by > qy) && qx < ax + (qy - ay) * (bx - ax) / (by - ay) {
                    inside = !inside;
                }
            }
            inside
        };
        let errs: Vec<String> = report
            .validation
            .iter()
            .flat_map(|v| v.errors.iter().cloned())
            .collect();
        for e in errs
            .iter()
            .filter(|e| e.contains("have same orientation on edge"))
            .take(3)
        {
            let nums: Vec<u32> = e
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|t| t.parse().ok())
                .collect();
            let (f1, f2, eid) = (nums[0], nums[1], nums[2]);
            println!("walk check for edge {eid} (faces {f1}, {f2}):");
            let Some(edge) = model.edges.get(eid) else {
                continue;
            };
            let pa = model
                .vertices
                .get_position(edge.start_vertex)
                .unwrap_or([0.0; 3]);
            let pb = model
                .vertices
                .get_position(edge.end_vertex)
                .unwrap_or([0.0; 3]);
            let a = Vector3::new(pa[0], pa[1], pa[2]);
            let b = Vector3::new(pb[0], pb[1], pb[2]);
            let mid = (a + b) * 0.5;
            let tangent = (b - a).normalize_or_zero();
            for fid in [f1, f2] {
                let Some(face) = model.faces.get(fid) else {
                    continue;
                };
                let Some(surf) = model.surfaces.get(face.surface_id) else {
                    continue;
                };
                let raw = surf
                    .normal_at(0.5, 0.5)
                    .map(|n| n.normalize_or_zero())
                    .unwrap_or(Vector3::ZERO);
                let outward = raw * face.orientation.sign();
                let perp = outward.cross(&tangent).normalize_or_zero();
                for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                    let Some(lp) = model.loops.get(l) else {
                        continue;
                    };
                    if !lp.edges.contains(&eid) {
                        continue;
                    }
                    let is_inner = l != face.outer_loop;
                    let verts: Vec<Vector3> = lp
                        .vertices(&model.edges)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|&v| model.vertices.get_position(v))
                        .map(|p| Vector3::new(p[0], p[1], p[2]))
                        .collect();
                    let mut dense = Vec::new();
                    for (k, &ed) in lp.edges.iter().enumerate() {
                        let Some(edd) = model.edges.get(ed) else {
                            continue;
                        };
                        let Some(c) = model.curves.get(edd.curve_id) else {
                            continue;
                        };
                        let fw = lp.orientations.get(k).copied().unwrap_or(true);
                        let r = edd.param_range;
                        for s in 0..32 {
                            let f = s as f64 / 32.0;
                            let t = if fw {
                                r.start + f * (r.end - r.start)
                            } else {
                                r.end - f * (r.end - r.start)
                            };
                            if let Ok(pt) = c.evaluate(t) {
                                dense.push(pt.position.to_vec());
                            }
                        }
                    }
                    let walk_in_loop = lp
                        .edges
                        .iter()
                        .position(|&x| x == eid)
                        .and_then(|k| lp.orientations.get(k).copied())
                        .unwrap_or(true);
                    let q = mid + perp * 1e-6;
                    let decide = |inside_plus: bool| {
                        let mut dir = if inside_plus { perp } else { perp * -1.0 };
                        if is_inner {
                            dir = dir * -1.0;
                        }
                        outward.cross(&tangent).dot(&dir) > 0.0
                    };
                    println!(
                        "  face {fid} {:?} loop {l}{}: {} vertices, {} dense pts; loop walks it {}; geometric walk fwd: vertex-poly {}, dense {}",
                        face.orientation,
                        if is_inner { " (hole)" } else { "" },
                        verts.len(),
                        dense.len(),
                        if walk_in_loop { "fwd" } else { "rev" },
                        decide(pip(&verts, q, outward)),
                        decide(pip(&dense, q, outward)),
                    );
                }
            }
        }
    }
    // PROBE_HOLES: mesh every face ALONE at chord 0.1 and report faces whose
    // own mesh has an edge used once that is not a segment of the face's
    // boundary samples (a hole inside the face).
    if std::env::var_os("PROBE_HOLES").is_some() {
        let params = geometry_engine::tessellation::TessellationParams {
            chord_tolerance: 0.1,
            ..geometry_engine::tessellation::TessellationParams::default()
        };
        let cache = geometry_engine::tessellation::edge_cache::EdgeSampleCache::new(&params);
        let key = |p: geometry_engine::math::Point3| {
            (
                (p.x * 1e6).round() as i64,
                (p.y * 1e6).round() as i64,
                (p.z * 1e6).round() as i64,
            )
        };
        for (fid, face) in model.faces.iter() {
            let mut boundary = std::collections::HashSet::new();
            for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                let Some(lp) = model.loops.get(l) else {
                    continue;
                };
                for &e in &lp.edges {
                    let s = cache.get_or_compute(e, &model);
                    for w in s.windows(2) {
                        let (a, b) = (key(w[0]), key(w[1]));
                        boundary.insert((a.min(b), a.max(b)));
                    }
                }
            }
            eprintln!("=== probe face {fid}");
            let mut mesh = geometry_engine::tessellation::TriangleMesh::new();
            geometry_engine::tessellation::tessellate_face(
                face, &model, &params, &cache, &mut mesh,
            );
            let mut uses: std::collections::HashMap<_, usize> = Default::default();
            for t in &mesh.triangles {
                for k in 0..3 {
                    let a = key(mesh.vertices[t[k] as usize].position);
                    let b = key(mesh.vertices[t[(k + 1) % 3] as usize].position);
                    if a != b {
                        *uses.entry((a.min(b), a.max(b))).or_default() += 1;
                    }
                }
            }
            let inner_open = uses
                .iter()
                .filter(|(e, n)| **n == 1 && !boundary.contains(*e))
                .count();
            let missing = boundary.iter().filter(|e| !uses.contains_key(*e)).count();
            if inner_open > 0 || missing > 0 {
                let kind = model
                    .surfaces
                    .get(face.surface_id)
                    .map(|s| s.type_name())
                    .unwrap_or("?");
                let ne = model
                    .loops
                    .get(face.outer_loop)
                    .map(|l| l.edges.len())
                    .unwrap_or(0);
                let unused: Vec<String> = boundary
                    .iter()
                    .filter(|e| !uses.contains_key(*e))
                    .take(4)
                    .map(|(a, b)| {
                        format!(
                            "({:.3},{:.3},{:.3})-({:.3},{:.3},{:.3})",
                            a.0 as f64 * 1e-6,
                            a.1 as f64 * 1e-6,
                            a.2 as f64 * 1e-6,
                            b.0 as f64 * 1e-6,
                            b.1 as f64 * 1e-6,
                            b.2 as f64 * 1e-6
                        )
                    })
                    .collect();
                println!("  unused boundary segments: {}", unused.join(" "));
                if let Some(lp) = model.loops.get(face.outer_loop) {
                    for (k, &e) in lp.edges.iter().enumerate() {
                        let s = cache.get_or_compute(e, &model);
                        let c = model
                            .edges
                            .get(e)
                            .and_then(|ed| model.curves.get(ed.curve_id))
                            .map(|c| c.type_name())
                            .unwrap_or("?");
                        let f = s
                            .first()
                            .map(|p| format!("({:.3},{:.3},{:.3})", p.x, p.y, p.z))
                            .unwrap_or_default();
                        let l = s
                            .last()
                            .map(|p| format!("({:.3},{:.3},{:.3})", p.x, p.y, p.z))
                            .unwrap_or_default();
                        println!(
                            "  loop edge {e} {c} {}: {} samples {f} .. {l}",
                            if lp.orientations.get(k).copied().unwrap_or(true) {
                                "fwd"
                            } else {
                                "rev"
                            },
                            s.len()
                        );
                    }
                }
                println!(
                    "hole face {fid} {kind} ({ne} outer edges, {} holes): {} triangles, {inner_open} open edges off the boundary, {missing} boundary segments unused",
                    face.inner_loops.len(),
                    mesh.triangles.len()
                );
            }
        }
    }
    // PROBE_FLIP: faces whose triangles wind against the face's own outward
    // normal (surface normal x face sense) at the triangle.
    if std::env::var_os("PROBE_FLIP").is_some() {
        let params = geometry_engine::tessellation::TessellationParams {
            chord_tolerance: 0.1,
            ..geometry_engine::tessellation::TessellationParams::default()
        };
        let cache = geometry_engine::tessellation::edge_cache::EdgeSampleCache::new(&params);
        for (fid, face) in model.faces.iter() {
            let Some(surf) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            let mut mesh = geometry_engine::tessellation::TriangleMesh::new();
            geometry_engine::tessellation::tessellate_face(
                face, &model, &params, &cache, &mut mesh,
            );
            let (mut agree, mut against, mut off40) = (0usize, 0usize, 0usize);
            for t in &mesh.triangles {
                let (a, b, c) = (
                    mesh.vertices[t[0] as usize].position,
                    mesh.vertices[t[1] as usize].position,
                    mesh.vertices[t[2] as usize].position,
                );
                let tn = (b - a).cross(&(c - a));
                let m = geometry_engine::math::Point3::new(
                    (a.x + b.x + c.x) / 3.0,
                    (a.y + b.y + c.y) / 3.0,
                    (a.z + b.z + c.z) / 3.0,
                );
                let Ok((u, v)) =
                    surf.closest_point(&m, geometry_engine::math::Tolerance::default())
                else {
                    continue;
                };
                let Ok(n) = surf.normal_at(u, v) else {
                    continue;
                };
                if tn.dot(&(n * face.orientation.sign())) >= 0.0 {
                    agree += 1
                } else {
                    against += 1
                }
                let cosd = tn
                    .normalize()
                    .map(|t| t.dot(&(n.normalize().unwrap_or(n) * face.orientation.sign())))
                    .unwrap_or(1.0);
                if cosd < 40f64.to_radians().cos() {
                    off40 += 1;
                    if std::env::var_os("PROBE_OFF40_SHOW").is_some() && off40 <= 3 {
                        let bset: std::collections::HashSet<(i64, i64, i64)> =
                            std::iter::once(face.outer_loop)
                                .chain(face.inner_loops.iter().copied())
                                .filter_map(|l| model.loops.get(l))
                                .flat_map(|lp| lp.edges.clone())
                                .flat_map(|e| cache.get_or_compute(e, &model).as_ref().clone())
                                .map(|q| {
                                    (
                                        (q.x * 1e6).round() as i64,
                                        (q.y * 1e6).round() as i64,
                                        (q.z * 1e6).round() as i64,
                                    )
                                })
                                .collect();
                        let tag = |q: geometry_engine::math::Point3| {
                            if bset.contains(&(
                                (q.x * 1e6).round() as i64,
                                (q.y * 1e6).round() as i64,
                                (q.z * 1e6).round() as i64,
                            )) {
                                "B"
                            } else {
                                "i"
                            }
                        };
                        println!(
                            "    off tri {:.1} deg: ({:.3},{:.3},{:.3}){} ({:.3},{:.3},{:.3}){} ({:.3},{:.3},{:.3}){}",
                            cosd.clamp(-1.0, 1.0).acos().to_degrees(),
                            a.x, a.y, a.z, tag(a), b.x, b.y, b.z, tag(b), c.x, c.y, c.z, tag(c)
                        );
                    }
                }
            }
            if off40 > 0 && std::env::var_os("PROBE_OFF40").is_some() {
                let ne = model
                    .loops
                    .get(face.outer_loop)
                    .map(|l| l.edges.len())
                    .unwrap_or(0);
                println!(
                    "off40 face {fid} {} {:?} ({ne} outer edges, {} holes): {off40} of {} triangles more than 40 degrees off the surface",
                    surf.type_name(), face.orientation, face.inner_loops.len(), agree + against
                );
            }
            if against > 0 {
                let ne = model
                    .loops
                    .get(face.outer_loop)
                    .map(|l| l.edges.len())
                    .unwrap_or(0);
                println!(
                    "flipped face {fid} {} {:?} ({ne} outer edges, {} holes): {against} of {} triangles against its normal",
                    surf.type_name(), face.orientation, face.inner_loops.len(), agree + against
                );
            }
        }
    }
    // PROBE_TOPO: B-Rep edges walked the SAME way (start->end) by both of the
    // faces using them, after each loop's own orientation flags.
    if std::env::var_os("PROBE_TOPO").is_some() {
        let mut walks: std::collections::HashMap<u32, Vec<(u32, bool)>> = Default::default();
        for (fid, face) in model.faces.iter() {
            for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                let Some(lp) = model.loops.get(l) else {
                    continue;
                };
                let mut count: std::collections::HashMap<u32, usize> = Default::default();
                for &e in &lp.edges {
                    *count.entry(e).or_default() += 1;
                }
                for (k, &e) in lp.edges.iter().enumerate() {
                    if count[&e] > 1 {
                        continue;
                    } // seam
                    walks
                        .entry(e)
                        .or_default()
                        .push((fid, lp.orientations.get(k).copied().unwrap_or(true)));
                }
            }
        }
        let mut bad: std::collections::BTreeMap<u32, usize> = Default::default();
        let (mut ok, mut same) = (0, 0);
        for w in walks.values().filter(|w| w.len() == 2) {
            if w[0].1 == w[1].1 {
                same += 1;
                *bad.entry(w[0].0).or_default() += 1;
                *bad.entry(w[1].0).or_default() += 1;
            } else {
                ok += 1;
            }
        }
        println!("topology: {ok} shared edges walked oppositely, {same} walked the same way");
        let mut v: Vec<_> = bad.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (f, n) in v.iter().take(6) {
            let kind = model
                .faces
                .get(*f)
                .and_then(|x| model.surfaces.get(x.surface_id))
                .map(|s| s.type_name())
                .unwrap_or("?");
            let sense = model.faces.get(*f).map(|x| x.orientation);
            println!("  face {f} {kind} {sense:?}: on {n} same-way edges");
        }
    }
    // PROBE_DIRECTED: shared mesh edges walked the SAME way by both triangles
    // (the certificate's "inconsistently directed"), by the two faces' kinds.
    if std::env::var_os("PROBE_DIRECTED").is_some() {
        for (_, solid) in model.solids.iter() {
            let params = geometry_engine::tessellation::TessellationParams {
                chord_tolerance: 0.1,
                ..geometry_engine::tessellation::TessellationParams::default()
            };
            let mesh = geometry_engine::tessellation::tessellate_solid(solid, &model, &params);
            let key = |p: geometry_engine::math::Point3| {
                (
                    (p.x * 1e6).round() as i64,
                    (p.y * 1e6).round() as i64,
                    (p.z * 1e6).round() as i64,
                )
            };
            let mut dir: std::collections::HashMap<_, Vec<u32>> = Default::default();
            for (i, t) in mesh.triangles.iter().enumerate() {
                for k in 0..3 {
                    let a = key(mesh.vertices[t[k] as usize].position);
                    let b = key(mesh.vertices[t[(k + 1) % 3] as usize].position);
                    if a != b {
                        dir.entry((a, b)).or_default().push(mesh.face_map[i]);
                    }
                }
            }
            let kind = |f: u32| -> String {
                let Some(face) = model.faces.get(f) else {
                    return "?".into();
                };
                let Some(surf) = model.surfaces.get(face.surface_id) else {
                    return "?".into();
                };
                match surf
                    .as_any()
                    .downcast_ref::<geometry_engine::primitives::surface::Torus>()
                {
                    Some(t) => format!(
                        "Torus(R {}, r {}, {:?}, {} holes, outer {} edges)",
                        t.major_radius,
                        t.minor_radius,
                        face.orientation,
                        face.inner_loops.len(),
                        model
                            .loops
                            .get(face.outer_loop)
                            .map(|l| l.edges.len())
                            .unwrap_or(0)
                    ),
                    None => surf.type_name().to_string(),
                }
            };
            let mut pairs: std::collections::BTreeMap<String, usize> = Default::default();
            for ((_, _), fs) in dir.iter() {
                if fs.len() >= 2 {
                    let mut k = vec![
                        format!("{}#{}", kind(fs[0]), fs[0]),
                        format!("{}#{}", kind(fs[1]), fs[1]),
                    ];
                    k.sort();
                    *pairs.entry(k.join(" / ")).or_default() += 1;
                }
            }
            for (k, n) in &pairs {
                println!("same-direction edges {n:>5}  {k}");
            }
            // Ground truth: for each face on a same-direction edge, does its
            // triangle normal point OUT of the solid? Step a hair off a
            // triangle along its normal and ray-cast (parity) against the mesh.
            let inside = |q: geometry_engine::math::Point3| -> bool {
                let dir = geometry_engine::math::Vector3::new(0.5773, 0.5774, 0.5772);
                let mut hits = 0;
                for t in &mesh.triangles {
                    let (a, b, c) = (
                        mesh.vertices[t[0] as usize].position,
                        mesh.vertices[t[1] as usize].position,
                        mesh.vertices[t[2] as usize].position,
                    );
                    let (e1, e2) = (b - a, c - a);
                    let h = dir.cross(&e2);
                    let det = e1.dot(&h);
                    if det.abs() < 1e-14 {
                        continue;
                    }
                    let f = 1.0 / det;
                    let sv = q - a;
                    let u = f * sv.dot(&h);
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let qv = sv.cross(&e1);
                    let v = f * dir.dot(&qv);
                    if v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    if f * e2.dot(&qv) > 1e-9 {
                        hits += 1;
                    }
                }
                hits % 2 == 1
            };
            let mut seen = std::collections::BTreeSet::new();
            for fs in dir.values().filter(|fs| fs.len() >= 2) {
                for &f in &fs[..2] {
                    if !seen.insert(f) {
                        continue;
                    }
                    let (mut out, mut inn) = (0, 0);
                    for (i, t) in mesh
                        .triangles
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| mesh.face_map[*i] == f)
                        .take(40)
                    {
                        let _ = i;
                        let (a, b, c) = (
                            mesh.vertices[t[0] as usize].position,
                            mesh.vertices[t[1] as usize].position,
                            mesh.vertices[t[2] as usize].position,
                        );
                        let Ok(n) = (b - a).cross(&(c - a)).normalize() else {
                            continue;
                        };
                        let m = geometry_engine::math::Point3::new(
                            (a.x + b.x + c.x) / 3.0,
                            (a.y + b.y + c.y) / 3.0,
                            (a.z + b.z + c.z) / 3.0,
                        );
                        if inside(m + n * 1e-3) {
                            inn += 1
                        } else {
                            out += 1
                        }
                    }
                    println!(
                        "  face {} {}: {out} triangles face out of the solid, {inn} into it",
                        f,
                        kind(f)
                    );
                    if let Some(face) = model.faces.get(f) {
                        if let Some(surf) = model.surfaces.get(face.surface_id) {
                            if let Some(t) =
                                surf.as_any()
                                    .downcast_ref::<geometry_engine::primitives::surface::Torus>()
                            {
                                let axis = t
                                    .axis
                                    .normalize()
                                    .unwrap_or(geometry_engine::math::Vector3::Z);
                                let (mut tri_tube, mut tri_core, mut ker_ok, mut ker_bad) =
                                    (0, 0, 0, 0);
                                for tri in mesh
                                    .triangles
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, _)| mesh.face_map[*i] == f)
                                    .map(|(_, t)| t)
                                    .take(40)
                                {
                                    let (a, b, c) = (
                                        mesh.vertices[tri[0] as usize].position,
                                        mesh.vertices[tri[1] as usize].position,
                                        mesh.vertices[tri[2] as usize].position,
                                    );
                                    let m = geometry_engine::math::Point3::new(
                                        (a.x + b.x + c.x) / 3.0,
                                        (a.y + b.y + c.y) / 3.0,
                                        (a.z + b.z + c.z) / 3.0,
                                    );
                                    let rel = m - t.center;
                                    let radial = (rel - axis * rel.dot(&axis))
                                        .normalize()
                                        .unwrap_or(t.ref_dir);
                                    let truth = m - (t.center + radial * t.major_radius);
                                    let tn = (b - a).cross(&(c - a));
                                    if tn.dot(&truth) > 0.0 {
                                        tri_tube += 1
                                    } else {
                                        tri_core += 1
                                    }
                                    if let Ok((u, v)) = surf.closest_point(
                                        &m,
                                        geometry_engine::math::Tolerance::default(),
                                    ) {
                                        if let Ok(n) = surf.normal_at(u, v) {
                                            if n.dot(&truth) > 0.0 {
                                                ker_ok += 1
                                            } else {
                                                ker_bad += 1
                                            }
                                        }
                                    }
                                }
                                println!("    triangles face away from the tube core: {tri_tube}, toward it: {tri_core}; kernel normal_at agrees with tube-outward {ker_ok}, not {ker_bad}; sense {:?}", face.orientation);
                                // Tube angle covered by this face's mesh vertices.
                                let mut angs: Vec<f64> = mesh
                                    .triangles
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, _)| mesh.face_map[*i] == f)
                                    .flat_map(|(_, t)| t.iter().copied())
                                    .map(|k| {
                                        let p = mesh.vertices[k as usize].position;
                                        let rel = p - t.center;
                                        let h = rel.dot(&axis);
                                        let rad = (rel - axis * h).magnitude() - t.major_radius;
                                        h.atan2(rad)
                                    })
                                    .collect();
                                angs.sort_by(|a, b| {
                                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                angs.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
                                // Largest gap between sorted angles (circularly): covered = 2pi - gap.
                                let mut gap = 0.0_f64;
                                for w in angs.windows(2) {
                                    gap = gap.max(w[1] - w[0]);
                                }
                                if let (Some(f0), Some(l0)) = (angs.first(), angs.last()) {
                                    gap = gap.max(std::f64::consts::TAU - (l0 - f0));
                                }
                                println!(
                                    "    mesh covers {:.0} degrees of the tube",
                                    (std::f64::consts::TAU - gap).to_degrees()
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    // PROBE_DEV=<face>: that face's facets on the default mesh, worst tilt
    // from the true normal first, with their vertices' (u, v) and how far
    // the centroid sits off the surface.
    if let Ok(arg) = std::env::var("PROBE_DEV") {
        let want: u32 = arg.parse().unwrap_or(u32::MAX);
        let ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
        for sid in ids {
            let Some(solid) = model.solids.get(sid) else {
                continue;
            };
            let mesh = geometry_engine::tessellation::tessellate_solid(
                &solid,
                &model,
                &geometry_engine::tessellation::TessellationParams::default(),
            );
            drop(solid);
            let normals = geometry_engine::harness::watertight::facet_true_normals(&model, &mesh);
            let surf = model
                .faces
                .get(want)
                .and_then(|f| model.surfaces.get(f.surface_id));
            let Some(surf) = surf else { continue };
            let tol = geometry_engine::math::Tolerance::default();
            let mut rows: Vec<(f64, String)> = Vec::new();
            for (t, tri) in mesh.triangles.iter().enumerate() {
                if mesh.face_map.get(t) != Some(&want) {
                    continue;
                }
                let [a, b, c] = tri.map(|i| mesh.vertices[i as usize].position);
                let w = (b - a).cross(&(c - a));
                let Some(n) = normals.get(t).copied().flatten() else {
                    continue;
                };
                let m = w.magnitude();
                if m <= 1e-15 {
                    continue;
                }
                let dev = (w.dot(&n) / (m * n.magnitude()))
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                let cen = geometry_engine::math::Point3::new(
                    (a.x + b.x + c.x) / 3.0,
                    (a.y + b.y + c.y) / 3.0,
                    (a.z + b.z + c.z) / 3.0,
                );
                let off = surf
                    .closest_point(&cen, tol)
                    .and_then(|(u, v)| surf.point_at(u, v))
                    .map(|q| (q - cen).magnitude())
                    .unwrap_or(f64::NAN);
                let uvs: Vec<String> = [a, b, c]
                    .iter()
                    .map(|p| {
                        surf.closest_point(p, tol)
                            .map(|(u, v)| format!("({u:.4},{v:.4})"))
                            .unwrap_or_default()
                    })
                    .collect();
                let lens = [
                    (b - a).magnitude(),
                    (c - b).magnitude(),
                    (a - c).magnitude(),
                ];
                rows.push((
                    dev,
                    format!(
                        "facet {t} dev {dev:.1} off {off:.3e} edges {:.3} {:.3} {:.3} uv {}",
                        lens[0],
                        lens[1],
                        lens[2],
                        uvs.join(" ")
                    ),
                ));
            }
            rows.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
            println!("dev face {want}: {} facets", rows.len());
            for (_, r) in rows.iter().take(8) {
                println!("  {r}");
            }
        }
        return;
    }
    // PROBE_QUALITY: the certificate's own mesh-quality verdict, with the
    // worst face described (same run, so the face id is meaningful).
    if std::env::var_os("PROBE_QUALITY").is_some() {
        for (sid, _) in model.solids.iter() {
            if let Some(q) = geometry_engine::harness::watertight::mesh_quality(&model, sid) {
                println!(
                    "quality: clean {} max dev {:.1} crossings {}",
                    q.clean, q.max_normal_deviation_deg, q.boundary_crossing_facets
                );
                if let Some(solid) = model.solids.get(sid) {
                    let mesh = geometry_engine::tessellation::tessellate_solid(
                        solid,
                        &model,
                        &geometry_engine::tessellation::TessellationParams::default(),
                    );
                    let mut shown = 0;
                    for (i, t) in mesh.triangles.iter().enumerate() {
                        let f = mesh.face_map[i];
                        let Some(p) = model
                            .faces
                            .get(f)
                            .and_then(|x| model.surfaces.get(x.surface_id))
                            .and_then(|s| s.period_u())
                        else {
                            continue;
                        };
                        let uvs: Vec<_> = t.iter().map(|&k| mesh.vertices[k as usize].uv).collect();
                        let (Some(a), Some(b), Some(c)) = (uvs[0], uvs[1], uvs[2]) else {
                            continue;
                        };
                        let cov = geometry_engine::harness::watertight::periodic_coverage(
                            a.0, b.0, c.0, p,
                        );
                        if cov >= 0.5 * p * (1.0 - 1e-9) && shown < 4 {
                            shown += 1;
                            let pts: Vec<String> = t
                                .iter()
                                .map(|&k| {
                                    let q = mesh.vertices[k as usize].position;
                                    format!("({:.3},{:.3},{:.3})", q.x, q.y, q.z)
                                })
                                .collect();
                            println!(
                                "  crossing facet on face {f}: uv {a:?} {b:?} {c:?}; xyz {}",
                                pts.join(" ")
                            );
                        }
                    }
                }
                if let Some(w) = q.worst_face {
                    let f = w.face_id as u32;
                    if let Some(face) = model.faces.get(f) {
                        let surf = model.surfaces.get(face.surface_id);
                        let ne = model
                            .loops
                            .get(face.outer_loop)
                            .map(|l| l.edges.len())
                            .unwrap_or(0);
                        println!(
                            "  worst face {f}: {} {:?}, {ne} outer edges, {} holes; dev {:.1}, crossings {}, period_u {:?}, bounds {:?}",
                            surf.map(|s| s.type_name()).unwrap_or("?"),
                            face.orientation,
                            face.inner_loops.len(),
                            w.max_normal_deviation_deg,
                            w.boundary_crossing_facets,
                            surf.and_then(|s| s.period_u()),
                            surf.map(|s| s.parameter_bounds())
                        );
                    }
                }
            }
        }
    }
    let errors: Vec<&String> = report
        .validation
        .iter()
        .flat_map(|v| v.errors.iter())
        .collect();
    // PROBE_TESSFACE=<id>: on the certificate's own display mesh, every facet
    // of that face whose winding points away from the true surface normal.
    if let Some(want_arg) = std::env::var("PROBE_TESSFACE").ok() {
        let ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
        for sid in ids {
            let Some(solid) = model.solids.get(sid) else {
                continue;
            };
            let mesh = geometry_engine::tessellation::tessellate_solid(
                &solid,
                &model,
                &geometry_engine::tessellation::TessellationParams::default(),
            );
            drop(solid);
            let normals = geometry_engine::harness::watertight::facet_true_normals(&model, &mesh);
            // "auto": the first face the display-mesh check blames.
            let want: u32 = match want_arg.parse::<u32>() {
                Ok(n) => n,
                Err(_) => geometry_engine::harness::watertight::tessellation_quality_of_mesh(
                    &model, &mesh,
                )
                .and_then(|q| q.defects.first().map(|d| d.face_id as u32))
                .unwrap_or(u32::MAX),
            };
            let surf = model
                .faces
                .get(want)
                .and_then(|f| model.surfaces.get(f.surface_id));
            let mut on_face = 0usize;
            for (t, tri) in mesh.triangles.iter().enumerate() {
                if mesh.face_map.get(t) != Some(&want) {
                    continue;
                }
                on_face += 1;
                let [a, b, c] = tri.map(|i| mesh.vertices[i as usize].position);
                let w = (b - a).cross(&(c - a));
                if w.magnitude() <= 1e-12 {
                    println!(
                        "tessface {want}: facet {t} DEGENERATE (|cross| {:.3e}); vertex ids {:?}; verts {:?} {:?} {:?}",
                        w.magnitude(),
                        tri,
                        a,
                        b,
                        c
                    );
                    continue;
                }
                let Some(n) = normals.get(t).copied().flatten() else {
                    continue;
                };
                if w.dot(&n) >= 0.0 {
                    continue;
                }
                let uvs = tri.map(|i| {
                    surf.and_then(|s| {
                        s.closest_point(
                            &mesh.vertices[i as usize].position,
                            geometry_engine::math::Tolerance::default(),
                        )
                        .ok()
                    })
                });
                println!(
                    "tessface {want}: facet {t} against its normal; area {:.3e}; verts {:?} {:?} {:?}; projected uv {:?}; true n {:?}",
                    w.magnitude() / 2.0, a, b, c, uvs, n
                );
            }
            println!("tessface {want}: {on_face} facets on the face");
            if let Some(sf) = surf {
                let (mut worst, mut worst_at) = (0.0_f64, None);
                let mut failed = 0usize;
                for (t, tri) in mesh.triangles.iter().enumerate() {
                    if mesh.face_map.get(t) != Some(&want) {
                        continue;
                    }
                    for &i in tri {
                        let p = mesh.vertices[i as usize].position;
                        match sf
                            .closest_point(&p, geometry_engine::math::Tolerance::default())
                            .and_then(|(u, v)| sf.point_at(u, v).map(|q| (u, v, q)))
                        {
                            Ok((u, v, q)) => {
                                let d = (q - p).magnitude();
                                if d > worst {
                                    worst = d;
                                    worst_at = Some((p, u, v, q));
                                }
                            }
                            Err(_) => failed += 1,
                        }
                    }
                }
                println!(
                    "tessface {want}: closest_point residual worst {worst:.3e} at {worst_at:?}; {failed} failed"
                );
                let mut devs: Vec<(f64, usize)> = mesh
                    .triangles
                    .iter()
                    .enumerate()
                    .filter(|(t, _)| mesh.face_map.get(*t) == Some(&want))
                    .filter_map(|(t, tri)| {
                        let [a, b, c] = tri.map(|i| mesh.vertices[i as usize].position);
                        let w = (b - a).cross(&(c - a)).normalize().ok()?;
                        let n = normals.get(t).copied().flatten()?;
                        Some((w.dot(&n).clamp(-1.0, 1.0).acos().to_degrees(), t))
                    })
                    .collect();
                devs.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
                for &(d, t) in devs.iter().take(3) {
                    let tri = mesh.triangles[t];
                    let pts = tri.map(|i| mesh.vertices[i as usize].position);
                    println!("tessface {want}: facet {t} {d:.1} deg off; verts {pts:?}");
                }
            }
            if let Some(face) = model.faces.get(want) {
                if let Some(t) = model.surfaces.get(face.surface_id).and_then(|s| {
                    s.as_any()
                        .downcast_ref::<geometry_engine::primitives::surface::Torus>()
                        .cloned()
                }) {
                    println!(
                        "tessface torus: center {:?} axis {:?} R {} r {} limits {:?} orientation {:?}",
                        t.center, t.axis, t.major_radius, t.minor_radius, t.param_limits, face.orientation
                    );
                }
                let params = geometry_engine::tessellation::TessellationParams::default();
                let cache =
                    geometry_engine::tessellation::edge_cache::EdgeSampleCache::new(&params);
                for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                    if let Some(lp) = model.loops.get(l) {
                        let samples: Vec<usize> = lp
                            .edges
                            .iter()
                            .map(|&e| cache.get_or_compute(e, &model).len())
                            .collect();
                        let kinds: Vec<String> = lp
                            .edges
                            .iter()
                            .filter_map(|&e| model.edges.get(e))
                            .filter_map(|e| {
                                model
                                    .curves
                                    .get(e.curve_id)
                                    .map(|c| format!("{:?}", c.type_name()))
                            })
                            .collect();
                        let first = lp
                            .edges
                            .first()
                            .map(|&e| cache.get_or_compute(e, &model))
                            .and_then(|v| v.first().copied());
                        println!(
                            "tessface loop {l}: {} edges {kinds:?} orientations {:?} samples {samples:?} first point {first:?}",
                            lp.edges.len(), lp.orientations
                        );
                    }
                }
            }
            if let Some(q) =
                geometry_engine::harness::watertight::tessellation_quality_of_mesh(&model, &mesh)
            {
                println!(
                    "tessface: quality clean {} off {} tris {}; defects {:?}",
                    q.clean, q.off_surface_facets, q.triangles, q.defects
                );
            }
            let mut per: std::collections::BTreeMap<u32, usize> = Default::default();
            for &f in &mesh.face_map {
                *per.entry(f).or_default() += 1;
            }
            let top: Vec<_> = {
                let mut v: Vec<_> = per.into_iter().collect();
                v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
                v.into_iter().take(5).collect()
            };
            println!("tessface: largest faces by facets {top:?}");
        }
    }
    // PROBE_EDGEGAP=<edge>: the edge's distance from each face using it, and
    // whether alternating projection onto the two surfaces converges onto
    // their intersection (and how far that is from the edge's own curve).
    if let Some(eid) = std::env::var("PROBE_EDGEGAP")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        let users: Vec<u32> = model
            .faces
            .iter()
            .filter(|(_, f)| {
                std::iter::once(f.outer_loop)
                    .chain(f.inner_loops.iter().copied())
                    .any(|l| model.loops.get(l).is_some_and(|lp| lp.edges.contains(&eid)))
            })
            .map(|(id, _)| id)
            .collect();
        let surfs: Vec<_> = users
            .iter()
            .filter_map(|&f| {
                model
                    .faces
                    .get(f)
                    .and_then(|x| model.surfaces.get(x.surface_id))
            })
            .collect();
        let tol = geometry_engine::math::Tolerance::default();
        let dist = |s: &dyn geometry_engine::primitives::surface::Surface,
                    p: geometry_engine::math::Point3| {
            s.closest_point(&p, tol)
                .ok()
                .and_then(|(u, v)| s.point_at(u, v).ok())
                .map(|q| (q, (q - p).magnitude()))
        };
        if let Some(edge) = model.edges.get(eid) {
            println!(
                "edgegap {eid}: faces {users:?} surfaces {:?} curve {:?}",
                surfs.iter().map(|s| s.type_name()).collect::<Vec<_>>(),
                model.curves.get(edge.curve_id).map(|c| c.type_name())
            );
            let cache = geometry_engine::tessellation::edge_cache::EdgeSampleCache::new(
                &geometry_engine::tessellation::TessellationParams::default(),
            );
            let samples = cache.get_or_compute(eid, &model);
            let mut worst = vec![0.0_f64; surfs.len()];
            // Solver-free distance to a torus: | sqrt((rho - R)^2 + h^2) - r |.
            let mut analytic = vec![None::<f64>; surfs.len()];
            for &p in samples.iter() {
                for (k, s) in surfs.iter().enumerate() {
                    if let Some(t) = s
                        .as_any()
                        .downcast_ref::<geometry_engine::primitives::surface::Torus>()
                    {
                        if let Ok(ax) = t.axis.normalize() {
                            let w = p - t.center;
                            let h = w.dot(&ax);
                            let rho = (w - ax * h).magnitude();
                            let d = (((rho - t.major_radius).powi(2) + h * h).sqrt()
                                - t.minor_radius)
                                .abs();
                            analytic[k] = Some(analytic[k].unwrap_or(0.0).max(d));
                        }
                    }
                }
            }
            println!("edgegap {eid}: closed-form torus distance (worst) {analytic:?}");
            let (mut moved, mut residual) = (0.0_f64, 0.0_f64);
            for &p in samples.iter() {
                for (k, s) in surfs.iter().enumerate() {
                    if let Some((_, d)) = dist(*s, p) {
                        worst[k] = worst[k].max(d);
                    }
                }
                if surfs.len() == 2 {
                    let mut q = p;
                    for _ in 0..200 {
                        let Some((a, _)) = dist(surfs[0], q) else {
                            break;
                        };
                        let Some((b, _)) = dist(surfs[1], a) else {
                            break;
                        };
                        q = b;
                    }
                    let ra = dist(surfs[0], q).map(|x| x.1).unwrap_or(f64::NAN);
                    let rb = dist(surfs[1], q).map(|x| x.1).unwrap_or(f64::NAN);
                    residual = residual.max(ra.max(rb));
                    moved = moved.max((q - p).magnitude());
                }
            }
            println!(
                "edgegap {eid}: {} samples; worst distance to each face {worst:?}; \
                 intersection: residual {residual:.3e}, farthest from the curve {moved:.3e}",
                samples.len()
            );
        }
    }
    // PROBE_FOLD=<face>: does the face's surface fold over its own patch?
    // Samples du x dv over the uv box of the face's projected boundary and
    // reports how much of it points against the majority direction.
    if let Some(fid) = std::env::var("PROBE_FOLD")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        if let Some(face) = model.faces.get(fid) {
            if let Some(surface) = model.surfaces.get(face.surface_id) {
                let cache = geometry_engine::tessellation::edge_cache::EdgeSampleCache::new(
                    &geometry_engine::tessellation::TessellationParams::default(),
                );
                let mut uvs = Vec::new();
                if let Some(lp) = model.loops.get(face.outer_loop) {
                    for &e in &lp.edges {
                        for &p in cache.get_or_compute(e, &model).iter() {
                            if let Ok(uv) = surface
                                .closest_point(&p, geometry_engine::math::Tolerance::default())
                            {
                                uvs.push(uv);
                            }
                        }
                    }
                }
                let (u0, u1) = uvs
                    .iter()
                    .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
                let (v0, v1) = uvs
                    .iter()
                    .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
                {
                    let vm = 0.5 * (v0 + v1);
                    println!(
                        "fold {fid}: wrap check at v {vm:.3}: point_at(-0.01) {:?}  point_at(0) {:?}  point_at(0.99) {:?}  period_u {:?}",
                        surface.point_at(-0.01, vm).ok(),
                        surface.point_at(0.0, vm).ok(),
                        surface.point_at(0.99, vm).ok(),
                        surface.period_u()
                    );
                }
                println!("fold {fid}: {} type, boundary uv box u {u0:.3}..{u1:.3} v {v0:.3}..{v1:.3}, bounds {:?}", surface.type_name(), surface.parameter_bounds());
                let n = 60;
                let mut normals = Vec::new();
                let (mut du_len, mut dv_len) = (Vec::new(), Vec::new());
                for i in 0..=n {
                    for j in 0..=n {
                        let (u, v) = (
                            u0 + (u1 - u0) * i as f64 / n as f64,
                            v0 + (v1 - v0) * j as f64 / n as f64,
                        );
                        if let Ok(sp) = surface.evaluate_full(u, v) {
                            du_len.push(sp.du.magnitude());
                            dv_len.push(sp.dv.magnitude());
                            let c = sp.du.cross(&sp.dv);
                            if c.magnitude() > 0.0 {
                                normals.push(((u, v), c * (1.0 / c.magnitude()), c.magnitude()));
                            }
                        }
                    }
                }
                let stat = |v: &[f64]| {
                    let lo = v.iter().copied().fold(f64::MAX, f64::min);
                    let hi = v.iter().copied().fold(0.0_f64, f64::max);
                    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
                    (lo, mean, hi)
                };
                println!(
                    "fold {fid}: |du| (min, mean, max) {:?}  |dv| {:?}",
                    stat(&du_len),
                    stat(&dv_len)
                );
                let mean = normals.iter().fold(
                    geometry_engine::math::Vector3::new(0.0, 0.0, 0.0),
                    |a, n| a + n.1,
                );
                let against: Vec<_> = normals.iter().filter(|n| n.1.dot(&mean) < 0.0).collect();
                let jac_min = normals.iter().map(|n| n.2).fold(f64::MAX, f64::min);
                let jac_max = normals.iter().map(|n| n.2).fold(0.0_f64, f64::max);
                println!(
                    "fold {fid}: {} samples, {} against the mean normal; |du x dv| {jac_min:.3e}..{jac_max:.3e}; first against: {:?}",
                    normals.len(),
                    against.len(),
                    against.first().map(|n| n.0)
                );
            }
        }
    }
    // PROBE_SAMEWAY: face pairs whose meshes walk a shared mesh edge the same
    // way, with a count and one midpoint each.
    if std::env::var_os("PROBE_SAMEWAY").is_some() {
        let ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
        for sid in ids {
            let Some(solid) = model.solids.get(sid) else {
                continue;
            };
            let mesh = geometry_engine::tessellation::tessellate_solid(
                &solid,
                &model,
                &geometry_engine::tessellation::TessellationParams::default(),
            );
            drop(solid);
            let key = |p: geometry_engine::math::Point3| {
                (
                    (p.x * 1e5).round() as i64,
                    (p.y * 1e5).round() as i64,
                    (p.z * 1e5).round() as i64,
                )
            };
            // directed edge (from, to) -> faces walking it that way.
            let mut directed: std::collections::HashMap<_, Vec<u32>> =
                std::collections::HashMap::new();
            for (t, tri) in mesh.triangles.iter().enumerate() {
                let f = mesh.face_map.get(t).copied().unwrap_or(u32::MAX);
                for k in 0..3 {
                    let a = key(mesh.vertices[tri[k] as usize].position);
                    let b = key(mesh.vertices[tri[(k + 1) % 3] as usize].position);
                    directed.entry((a, b)).or_default().push(f);
                }
            }
            let mut pairs: std::collections::BTreeMap<(u32, u32), (usize, (i64, i64, i64))> =
                std::collections::BTreeMap::new();
            for ((a, b), fs) in &directed {
                if fs.len() < 2 {
                    continue;
                }
                let mid = ((a.0 + b.0) / 2, (a.1 + b.1) / 2, (a.2 + b.2) / 2);
                for i in 0..fs.len() {
                    for j in i + 1..fs.len() {
                        let k = (fs[i].min(fs[j]), fs[i].max(fs[j]));
                        let e = pairs.entry(k).or_insert((0, mid));
                        e.0 += 1;
                    }
                }
            }
            let kind = |f: u32| {
                model
                    .faces
                    .get(f)
                    .map(|x| {
                        (
                            model
                                .surfaces
                                .get(x.surface_id)
                                .map(|s| s.type_name())
                                .unwrap_or("?"),
                            format!("{:?}", x.orientation),
                        )
                    })
                    .unwrap_or(("?", String::new()))
            };
            for ((fa, fb), (n, mid)) in pairs {
                println!(
                    "sameway solid {sid}: faces {fa} {:?} / {fb} {:?}: {n} edge(s), e.g. near ({:.3}, {:.3}, {:.3})",
                    kind(fa),
                    kind(fb),
                    mid.0 as f64 * 1e-5,
                    mid.1 as f64 * 1e-5,
                    mid.2 as f64 * 1e-5
                );
            }
        }
    }
    // PROBE_SELFX: the face pairs whose meshes cross.
    if std::env::var_os("PROBE_SELFX").is_some() {
        let ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
        for sid in ids {
            let pairs = geometry_engine::harness::self_intersection::self_intersecting_faces(
                &model, sid, 0.5,
            );
            let kind = |f: u32| {
                model
                    .faces
                    .get(f)
                    .and_then(|x| model.surfaces.get(x.surface_id))
                    .map(|s| s.type_name())
                    .unwrap_or("?")
            };
            println!(
                "selfx solid {sid}: {} face pair(s): {:?}",
                pairs.len(),
                pairs
                    .iter()
                    .take(20)
                    .map(|&(a, b)| (a, kind(a), b, kind(b)))
                    .collect::<Vec<_>>()
            );
        }
    }
    // PROBE_BAND=<id,id>: each face's outer-loop edges, in loop order: sample
    // count, walk direction, first and last sample (as walked).
    if let Ok(list) = std::env::var("PROBE_BAND") {
        let cache = geometry_engine::tessellation::edge_cache::EdgeSampleCache::new(
            &geometry_engine::tessellation::TessellationParams::default(),
        );
        for fid in list.split(',').filter_map(|s| s.trim().parse::<u32>().ok()) {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let surf = model.surfaces.get(face.surface_id).map(|s| s.type_name());
            println!(
                "band face {fid}: {surf:?} sense {:?} inner {}",
                face.orientation,
                face.inner_loops.len()
            );
            let Some(lp) = model.loops.get(face.outer_loop) else {
                continue;
            };
            let axis = model.surfaces.get(face.surface_id).and_then(|s| {
                s.as_any()
                    .downcast_ref::<geometry_engine::primitives::surface::SurfaceOfRevolution>()
                    .map(|r| (r.axis_origin, r.axis_direction))
            });
            if let Some((o, d)) = axis {
                println!("  revolution axis origin {o:?} direction {d:?}");
            }
            for (k, &eid) in lp.edges.iter().enumerate() {
                let fwd = lp.orientations.get(k).copied().unwrap_or(true);
                let s = cache.get_or_compute(eid, &model);
                if let Some((o, d)) = axis {
                    if let Ok(d) = d.normalize() {
                        let (mut alo, mut ahi, mut rlo, mut rhi) =
                            (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
                        for &p in s.iter() {
                            let w = p - o;
                            let a = w.dot(&d);
                            let r = (w - d * a).magnitude();
                            alo = alo.min(a);
                            ahi = ahi.max(a);
                            rlo = rlo.min(r);
                            rhi = rhi.max(r);
                        }
                        println!(
                            "    edge {eid}: axial {alo:.6}..{ahi:.6} radius {rlo:.6}..{rhi:.6}"
                        );
                    }
                }
                let (a, b) = if fwd {
                    (s.first().copied(), s.last().copied())
                } else {
                    (s.last().copied(), s.first().copied())
                };
                let len = match (a, b) {
                    (Some(a), Some(b)) => (b - a).magnitude(),
                    _ => 0.0,
                };
                println!(
                    "  edge {eid} fwd {fwd} samples {} from {a:?} to {b:?} chord {len:.4}",
                    s.len()
                );
                if let Some(e) = model.edges.get(eid) {
                    let vp = |v: u32| model.vertices.get(v).map(|x| x.position);
                    println!(
                        "    vertices {} {:?} -> {} {:?}",
                        e.start_vertex,
                        vp(e.start_vertex),
                        e.end_vertex,
                        vp(e.end_vertex)
                    );
                }
            }
        }
    }
    // PROBE_LENS: the importer's decisions, on face ids.
    if std::env::var_os("PROBE_LENS").is_some() {
        let by_pid: std::collections::HashMap<u128, u32> = model
            .faces
            .iter()
            .filter_map(|(f, _)| model.face_pid(f).map(|p| (p.0, f)))
            .collect();
        for e in report.lens.events() {
            let face = match e.entity {
                geometry_engine::lens::EntityRef::Face(p) => by_pid.get(&p.0).copied(),
                geometry_engine::lens::EntityRef::Solid(_) => None,
            };
            println!(
                "lens: face {face:?} {} #{:?} {:?} -- {}",
                e.kind.name(),
                e.step_instance,
                e.measured,
                e.why
            );
        }
    }
    // Kernel Lens: what blocks each solid's certificate, on which face.
    {
        let m = &mut model;
        let ids: Vec<u32> = m.solids.iter().map(|(id, _)| id).collect();
        for sid in ids {
            let cert = m.certify_solid(sid);
            for b in &cert.blocking {
                let surf = b
                    .face
                    .and_then(|f| m.faces.get(f))
                    .and_then(|f| m.surfaces.get(f.surface_id))
                    .map(|s| format!("{:?}", s.surface_type()));
                println!(
                    "blocking: solid {sid} {} face {:?} {:?} {:?}",
                    b.conjunct, b.face, surf, b.measured
                );
            }
        }
    }
    println!("{} errors; first 4:", errors.len());
    for e in errors.iter().take(4) {
        println!("  {e}");
        let Some(edge) = e
            .split("on edge ")
            .nth(1)
            .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        for (fid, face) in model.faces.iter() {
            let loops = std::iter::once((face.outer_loop, "outer"))
                .chain(face.inner_loops.iter().map(|&l| (l, "inner")));
            for (lid, role) in loops {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for (k, &eid) in lp.edges.iter().enumerate() {
                    if eid == edge {
                        let fwd = lp.orientations.get(k).copied().unwrap_or(true);
                        let kind = model
                            .surfaces
                            .get(face.surface_id)
                            .map(|s| s.type_name())
                            .unwrap_or("?");
                        // Every loop of this face with its area projected along
                        // the face's normal at the loop's first vertex (planes).
                        let n = model
                            .surfaces
                            .get(face.surface_id)
                            .and_then(|s| s.normal_at(0.0, 0.0).ok());
                        let mut areas = Vec::new();
                        for l2 in
                            std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied())
                        {
                            let Some(lp2) = model.loops.get(l2) else {
                                continue;
                            };
                            let mut pts = Vec::new();
                            for (j, &e2) in lp2.edges.iter().enumerate() {
                                let Some(ed) = model.edges.get(e2) else {
                                    continue;
                                };
                                let Some(c) = model.curves.get(ed.curve_id) else {
                                    continue;
                                };
                                let fw = lp2.orientations.get(j).copied().unwrap_or(true);
                                for s in 0..8 {
                                    let f = if fw {
                                        s as f64 / 8.0
                                    } else {
                                        1.0 - s as f64 / 8.0
                                    };
                                    let r = ed.param_range;
                                    if let Ok(pt) = c.evaluate(r.start + f * (r.end - r.start)) {
                                        pts.push(pt.position);
                                    }
                                }
                            }
                            let mut area = geometry_engine::math::Vector3::ZERO;
                            for k2 in 0..pts.len() {
                                let (pa, pb) = (pts[k2], pts[(k2 + 1) % pts.len()]);
                                area = area + pa.to_vec().cross(&pb.to_vec());
                            }
                            let signed = n.map(|n| 0.5 * area.dot(&n)).unwrap_or(f64::NAN);
                            areas.push(format!(
                                "loop {l2}{}: {signed:.1}",
                                if l2 == face.outer_loop {
                                    " (outer)"
                                } else {
                                    ""
                                }
                            ));
                        }
                        println!("      loops: {}", areas.join(", "));
                        println!(
                            "    face {fid}: {kind}, sense {:?}, {role} loop {lid} ({} edges), walks edge {edge} {}",
                            face.orientation,
                            lp.edges.len(),
                            if fwd { "forward" } else { "reversed" }
                        );
                    }
                }
            }
        }
    }
}
