//! Tessellation correctness oracle — the tough, multi-tolerance counterpart to
//! the [`tessellation`](crate::harness::tessellation) ablation study and the
//! [`watertight`](crate::harness::watertight) per-mesh manifold check.
//!
//! Where the ablation study measures the *density-vs-accuracy* tradeoff and the
//! watertight oracle answers "is this one mesh closed?", this module pins the
//! properties tessellation must hold **across the whole tolerance range and run
//! to run**, the classes that have actually bitten this kernel:
//!
//! * **Validity at every tolerance** — not just the finest. A coarse chord must
//!   still yield a closed, oriented, manifold solid (the TESS-FIX class: a coarse
//!   weld once collapsed a sphere to zero triangles; a coarse grid can also leave
//!   an open seam). Checked over the primitive zoo + a boolean result.
//! * **Convergence** — refining the chord tolerance must drive the mesh volume
//!   monotonically toward the analytic volume and never *increase* the triangle
//!   count's error; a finer mesh that is *less* accurate signals a broken
//!   adaptive criterion.
//! * **Determinism** — tessellating the same solid twice must produce a
//!   bit-identical mesh (positions, normals, indices, face-map). A difference is
//!   a HashMap-iteration-order nondeterminism bug, the exact class that makes a
//!   downstream watertight test flaky ("a flaky result is a determinism bug").
//! * **No degenerate facets** — zero-area triangles are never emitted; they break
//!   normal computation and downstream area/normal queries.
//! * **Normal orientation agreement** — each triangle's stored vertex normals
//!   agree with its winding normal (their dot is positive), so the shaded normal
//!   and the geometric facet face the same way — the rendering-correctness analogue
//!   of the mesh `oriented` topological check.

use crate::harness::watertight::manifold_report;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::mesh::TriangleMesh;
use crate::tessellation::{tessellate_solid, TessellationParams};

/// Tessellation params with the **chord tolerance as the sole density driver** —
/// the angle/edge-length limits are set non-binding so a chord sweep is not
/// masked by a fixed angular grid (mirrors the ablation study's isolation).
fn chord_only_params(chord: f64) -> TessellationParams {
    TessellationParams {
        chord_tolerance: chord,
        max_angle_deviation: std::f64::consts::PI,
        max_edge_length: 1.0e9,
        ..TessellationParams::default()
    }
}

/// Signed-then-absolute enclosed volume of a mesh by the divergence theorem
/// `V = (1/6) Σ p0·(p1×p2)`.
fn mesh_enclosed_volume(mesh: &TriangleMesh) -> f64 {
    let mut six_v = 0.0;
    for tri in &mesh.triangles {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        six_v += p0.dot(&p1.cross(&p2));
    }
    (six_v / 6.0).abs()
}

/// Count of zero-area (degenerate) triangles — two coincident vertices or three
/// collinear ones, judged by a tiny cross-product magnitude.
fn degenerate_count(mesh: &TriangleMesh) -> usize {
    let mut n = 0;
    for tri in &mesh.triangles {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        if (p1 - p0).cross(&(p2 - p0)).magnitude() <= 1e-12 {
            n += 1;
        }
    }
    n
}

/// Fraction of (non-degenerate) triangles whose stored vertex normals agree with
/// the triangle's winding normal — `(n0+n1+n2) · ((p1−p0)×(p2−p0)) > 0`. A value
/// of 1.0 means every facet's shaded normal faces the same way as its geometry;
/// anything less is an inward-facing / mis-set normal.
fn normal_agreement(mesh: &TriangleMesh) -> f64 {
    let mut agree = 0usize;
    let mut total = 0usize;
    for tri in &mesh.triangles {
        let v0 = mesh.vertices[tri[0] as usize];
        let v1 = mesh.vertices[tri[1] as usize];
        let v2 = mesh.vertices[tri[2] as usize];
        let geo = (v1.position - v0.position).cross(&(v2.position - v0.position));
        if geo.magnitude() <= 1e-12 {
            continue; // degenerate: no winding normal to compare
        }
        total += 1;
        let stored = v0.normal + v1.normal + v2.normal;
        if stored.dot(&geo) > 0.0 {
            agree += 1;
        }
    }
    if total == 0 {
        1.0
    } else {
        agree as f64 / total as f64
    }
}

/// Full quality record for one tessellation level.
#[derive(Debug, Clone)]
pub struct TessQuality {
    pub chord_tolerance: f64,
    pub triangles: usize,
    pub degenerate_triangles: usize,
    pub mesh_volume: f64,
    /// `|mesh − analytic| / max(|analytic|, 1)`.
    pub volume_rel_error: f64,
    pub closed: bool,
    pub manifold: bool,
    pub oriented: bool,
    pub euler_characteristic: i64,
    /// Fraction of facets whose stored normals agree with their winding normal.
    pub normal_agreement: f64,
}

impl TessQuality {
    /// A valid render mesh: a closed, oriented, manifold solid with triangles,
    /// no degenerate facets, and every normal facing outward.
    pub fn is_valid(&self) -> bool {
        self.closed
            && self.manifold
            && self.oriented
            && self.triangles > 0
            && self.degenerate_triangles == 0
            && self.normal_agreement >= 1.0
    }
}

/// Tessellate `solid` at chord tolerance `chord` and compute its full quality
/// record against `analytic_volume`. `None` if the solid is missing or
/// tessellates to nothing.
pub fn tess_quality(
    model: &BRepModel,
    solid: SolidId,
    analytic_volume: f64,
    chord: f64,
) -> Option<TessQuality> {
    let solid_ref = model.solids.get(solid)?;
    let params = chord_only_params(chord);
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }
    let mesh_volume = mesh_enclosed_volume(&mesh);
    let scale = analytic_volume.abs().max(1.0);
    // The connectivity verdict comes from the shared manifold oracle (welded by
    // quantised position). Determinism (asserted separately) guarantees its
    // re-tessellation matches the mesh measured here.
    let mr = manifold_report(model, solid, chord, 1e-6)?;
    Some(TessQuality {
        chord_tolerance: chord,
        triangles: mesh.triangles.len(),
        degenerate_triangles: degenerate_count(&mesh),
        mesh_volume,
        volume_rel_error: (mesh_volume - analytic_volume).abs() / scale,
        closed: mr.closed,
        manifold: mr.manifold,
        oriented: mr.oriented,
        euler_characteristic: mr.euler_characteristic,
        normal_agreement: normal_agreement(&mesh),
    })
}

/// Quality record at each chord tolerance.
pub fn tess_quality_sweep(
    model: &BRepModel,
    solid: SolidId,
    analytic_volume: f64,
    chords: &[f64],
) -> Vec<TessQuality> {
    chords
        .iter()
        .filter_map(|&c| tess_quality(model, solid, analytic_volume, c))
        .collect()
}

/// Two meshes are **bit-identical**: same vertex count, same triangle list, same
/// face-map, and every position/normal/uv equal to the last bit (`to_bits`). This
/// is the exact-reproducibility predicate determinism is checked with — no
/// tolerance, because a correct, deterministic tessellator re-runs to the same
/// floating-point values.
pub fn meshes_bit_identical(a: &TriangleMesh, b: &TriangleMesh) -> bool {
    if a.vertices.len() != b.vertices.len()
        || a.triangles.len() != b.triangles.len()
        || a.face_map.len() != b.face_map.len()
    {
        return false;
    }
    if a.triangles != b.triangles || a.face_map != b.face_map {
        return false;
    }
    let bits = |p: &crate::math::Point3| (p.x.to_bits(), p.y.to_bits(), p.z.to_bits());
    let nbits = |v: &crate::math::Vector3| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits());
    for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
        if bits(&va.position) != bits(&vb.position) || nbits(&va.normal) != nbits(&vb.normal) {
            return false;
        }
        match (va.uv, vb.uv) {
            (Some((u0, v0)), Some((u1, v1))) => {
                if u0.to_bits() != u1.to_bits() || v0.to_bits() != v1.to_bits() {
                    return false;
                }
            }
            (None, None) => {}
            _ => return false,
        }
    }
    true
}

/// Tessellate `solid` `runs` times at chord `chord` and report whether every run
/// is bit-identical to the first. `None` if the solid is missing or empty.
pub fn is_deterministic(
    model: &BRepModel,
    solid: SolidId,
    chord: f64,
    runs: usize,
) -> Option<bool> {
    let solid_ref = model.solids.get(solid)?;
    let params = chord_only_params(chord);
    let first = tessellate_solid(solid_ref, model, &params);
    if first.triangles.is_empty() {
        return None;
    }
    for _ in 1..runs.max(2) {
        let again = tessellate_solid(solid_ref, model, &params);
        if !meshes_bit_identical(&first, &again) {
            return Some(false);
        }
    }
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::transform::translate;
    use crate::primitives::topology_builder::TopologyBuilder;
    use proptest::prelude::*;

    fn last(m: &BRepModel) -> SolidId {
        m.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    /// The primitive zoo plus a boolean union, with their analytic volumes. Each
    /// builder returns the solid id; the volume is the watertightness oracle.
    fn zoo() -> Vec<(&'static str, BRepModel, SolidId, f64)> {
        let pi = std::f64::consts::PI;
        let mut out: Vec<(&'static str, BRepModel, SolidId, f64)> = Vec::new();

        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_box_3d(2.0, 3.0, 4.0)
            .expect("box");
        let s = last(&m);
        out.push(("box", m, s, 24.0));

        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_sphere_3d(Vector3::ZERO, 2.0)
            .expect("sphere");
        let s = last(&m);
        out.push(("sphere", m, s, 4.0 / 3.0 * pi * 8.0));

        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Vector3::ZERO, Vector3::Z, 2.0, 5.0)
            .expect("cyl");
        let s = last(&m);
        out.push(("cylinder", m, s, pi * 4.0 * 5.0));

        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_cone_3d(Vector3::ZERO, Vector3::Z, 2.0, 0.0, 5.0)
            .expect("cone");
        let s = last(&m);
        out.push(("cone", m, s, pi * 4.0 * 5.0 / 3.0));

        // Boolean union of two overlapping boxes: 4³ + half of a 4³ = 96.
        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("a");
        let a = last(&m);
        TopologyBuilder::new(&mut m)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("b");
        let b = last(&m);
        translate(&mut m, vec![b], Vector3::X, 2.0, Default::default()).expect("translate");
        let u = boolean_operation(&mut m, a, b, BooleanOp::Union, BooleanOptions::default())
            .expect("union");
        out.push(("box-union", m, u, 96.0));

        out
    }

    #[test]
    #[ignore = "diagnostic: dump per-triangle winding vs stored normal for a box"]
    fn diag_box_normals() {
        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let s = last(&m);
        let solid_ref = m.solids.get(s).unwrap();
        let mesh = tessellate_solid(solid_ref, &m, &chord_only_params(0.05));
        eprintln!(
            "tris={} verts={}",
            mesh.triangles.len(),
            mesh.vertices.len()
        );
        if let Some(solid) = m.solids.get(s) {
            if let Some(shell) = m.shells.get(solid.outer_shell) {
                for &fid in &shell.faces {
                    if let Some(face) = m.faces.get(fid) {
                        let st = m.surfaces.get(face.surface_id).map(|s| s.type_name());
                        eprintln!(
                            "  face {fid}: surface={:?} orientation={:?}",
                            st, face.orientation
                        );
                    }
                }
            }
        }
        for (i, tri) in mesh.triangles.iter().enumerate() {
            let v0 = mesh.vertices[tri[0] as usize];
            let v1 = mesh.vertices[tri[1] as usize];
            let v2 = mesh.vertices[tri[2] as usize];
            let geo = (v1.position - v0.position).cross(&(v2.position - v0.position));
            let c = crate::math::Point3::new(
                (v0.position.x + v1.position.x + v2.position.x) / 3.0,
                (v0.position.y + v1.position.y + v2.position.y) / 3.0,
                (v0.position.z + v1.position.z + v2.position.z) / 3.0,
            );
            let stored = v0.normal + v1.normal + v2.normal;
            eprintln!(
                "t{i:2} face={} centroid=({:+.1},{:+.1},{:+.1}) winding=({:+.2},{:+.2},{:+.2}) \
                 n0=({:+.2},{:+.2},{:+.2}) sum·wind={:+.3}",
                mesh.face_map[i],
                c.x,
                c.y,
                c.z,
                geo.x,
                geo.y,
                geo.z,
                v0.normal.x,
                v0.normal.y,
                v0.normal.z,
                stored.dot(&geo),
            );
        }
    }

    /// VALIDITY AT EVERY TOLERANCE — the whole zoo must tessellate to a closed,
    /// oriented, manifold solid with no degenerate facets and outward normals at
    /// *every* chord level from coarse to fine, not just the finest. This is the
    /// multi-tolerance guard the single-tolerance watertight tests do not give.
    #[test]
    fn every_primitive_is_valid_at_every_tolerance() {
        let chords = [0.2, 0.1, 0.05, 0.02, 0.005];
        let mut bad = Vec::new();
        for (name, model, solid, vol) in zoo() {
            let levels = tess_quality_sweep(&model, solid, vol, &chords);
            if levels.len() != chords.len() {
                bad.push(format!(
                    "{name}: only {} of {} levels meshed",
                    levels.len(),
                    chords.len()
                ));
                continue;
            }
            for q in &levels {
                if !q.is_valid() {
                    bad.push(format!(
                        "{name} @ chord {}: valid=false (closed={} manifold={} oriented={} \
                         degenerate={} normals={:.3} euler={} tris={})",
                        q.chord_tolerance,
                        q.closed,
                        q.manifold,
                        q.oriented,
                        q.degenerate_triangles,
                        q.normal_agreement,
                        q.euler_characteristic,
                        q.triangles
                    ));
                }
                if q.euler_characteristic != 2 {
                    bad.push(format!(
                        "{name} @ chord {}: euler {} ≠ 2 (genus-0 expected)",
                        q.chord_tolerance, q.euler_characteristic
                    ));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "tessellation validity failures:\n{}",
            bad.join("\n")
        );
    }

    /// CONVERGENCE — refining the chord tolerance drives the mesh volume toward
    /// the analytic volume. Asserted on the curved primitives (a box is exact at
    /// every level, so it has nothing to converge): the finest level's relative
    /// volume error is no worse than the coarsest, and triangle count is strictly
    /// monotone in tolerance.
    #[test]
    fn refining_tolerance_converges_volume_and_adds_triangles() {
        let chords = [0.2, 0.05, 0.01];
        for (name, model, solid, vol) in zoo() {
            if name == "box" || name == "box-union" {
                continue; // exact at every tolerance — convergence is trivial/flat
            }
            let levels = tess_quality_sweep(&model, solid, vol, &chords);
            assert_eq!(levels.len(), 3, "{name}: missing levels");
            assert!(
                levels[0].triangles < levels[1].triangles
                    && levels[1].triangles < levels[2].triangles,
                "{name}: triangle count not monotone in tolerance: {} {} {}",
                levels[0].triangles,
                levels[1].triangles,
                levels[2].triangles
            );
            assert!(
                levels[2].volume_rel_error <= levels[0].volume_rel_error + 1e-9,
                "{name}: finer mesh is LESS accurate ({:.4e} → {:.4e})",
                levels[0].volume_rel_error,
                levels[2].volume_rel_error
            );
            // The finest curved mesh sits within a few percent of analytic.
            assert!(
                levels[2].volume_rel_error < 0.03,
                "{name}: finest mesh volume error {:.4e} too large",
                levels[2].volume_rel_error
            );
        }
    }

    /// DETERMINISM — every solid in the zoo tessellates bit-identically across
    /// repeated runs. A single non-identical run is a HashMap-iteration-order
    /// nondeterminism bug (the class that makes downstream watertight tests
    /// flaky). Run at a mid and a fine tolerance.
    #[test]
    fn tessellation_is_bit_deterministic() {
        for (name, model, solid, _vol) in zoo() {
            for chord in [0.05, 0.01] {
                match is_deterministic(&model, solid, chord, 4) {
                    Some(true) => {}
                    Some(false) => {
                        panic!("{name} @ chord {chord}: tessellation is NON-deterministic")
                    }
                    None => panic!("{name} @ chord {chord}: no mesh"),
                }
            }
        }
    }

    /// The box mesh is exact at any tolerance — no degenerate facets, volume to
    /// machine precision, valid manifold, all normals outward.
    #[test]
    fn box_tessellation_is_exact_and_clean() {
        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_box_3d(2.0, 3.0, 4.0)
            .expect("box");
        let s = last(&m);
        let q = tess_quality(&m, s, 24.0, 0.05).expect("mesh");
        assert!(q.is_valid(), "box not valid: {q:?}");
        assert_eq!(q.degenerate_triangles, 0);
        assert!(
            q.volume_rel_error < 1e-9,
            "box volume error {:.3e}",
            q.volume_rel_error
        );
        assert!(
            (q.normal_agreement - 1.0).abs() < 1e-12,
            "box normals {}",
            q.normal_agreement
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

        /// A box of any (positive) dimensions tessellates to a valid, deterministic
        /// mesh whose volume is exact and whose normals all face outward, at a
        /// randomly chosen tolerance. This sweeps scale × tolerance jointly — the
        /// regime where a fixed weld epsilon or a scale-blind density heuristic
        /// would crack (the TESS-FIX class generalised).
        #[test]
        fn pp_box_any_scale_tessellates_valid_and_deterministic(
            w in 0.2f64..20.0, h in 0.2f64..20.0, d in 0.2f64..20.0,
            chord in 0.005f64..0.3,
        ) {
            let mut m = BRepModel::new();
            TopologyBuilder::new(&mut m).create_box_3d(w, h, d).expect("box");
            let s = last(&m);
            let vol = w * h * d;
            let q = tess_quality(&m, s, vol, chord).expect("mesh");
            prop_assert!(q.is_valid(), "invalid box mesh: {q:?}");
            prop_assert_eq!(q.euler_characteristic, 2, "box euler {}", q.euler_characteristic);
            // A planar-faced box is volume-exact regardless of chord tolerance.
            prop_assert!(q.volume_rel_error < 1e-9, "box volume error {:.3e}", q.volume_rel_error);
            prop_assert_eq!(is_deterministic(&m, s, chord, 3), Some(true));
        }

        /// A sphere of any radius is a valid closed manifold at any tolerance, and
        /// finer tolerance never makes the volume error worse — the curved-surface
        /// convergence property over random scale.
        #[test]
        fn pp_sphere_any_radius_is_valid_and_converges(
            r in 0.5f64..10.0,
        ) {
            let mut m = BRepModel::new();
            TopologyBuilder::new(&mut m).create_sphere_3d(Vector3::ZERO, r).expect("sphere");
            let s = last(&m);
            let vol = 4.0 / 3.0 * std::f64::consts::PI * r.powi(3);
            // Tolerances scaled to the radius so faceting is comparable across scale.
            let coarse = r * 0.1;
            let fine = r * 0.01;
            let qc = tess_quality(&m, s, vol, coarse).expect("coarse");
            let qf = tess_quality(&m, s, vol, fine).expect("fine");
            prop_assert!(qc.is_valid(), "coarse sphere invalid: {qc:?}");
            prop_assert!(qf.is_valid(), "fine sphere invalid: {qf:?}");
            prop_assert!(qf.triangles >= qc.triangles, "fine has fewer triangles");
            prop_assert!(
                qf.volume_rel_error <= qc.volume_rel_error + 1e-9,
                "fine sphere less accurate ({:.3e} → {:.3e})",
                qc.volume_rel_error, qf.volume_rel_error
            );
        }
    }
}
