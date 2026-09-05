// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **A window cut through a cylinder wall ACROSS the parametric seam exists in
//! the B-Rep and not in the mesh.**

use geometry_engine::harness::watertight::{manifold_report, mesh_quality};
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

const CYL_R: f64 = 10.0;
const CYL_H: f64 = 20.0;
const WIN_HALF: f64 = 3.0;
const WIN_Z_LO: f64 = 6.0;
const WIN_Z_HI: f64 = 14.0;

fn cylinder(model: &mut BRepModel, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model).create_cylinder_3d(Point3::ORIGIN, Vector3::Z, r, h) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// A box that pierces the wall at `u = 0` (+X, the seam) and reaches neither cap.
fn seam_window(model: &mut BRepModel) -> SolidId {
    let id = match TopologyBuilder::new(model).create_box_3d(
        30.0,
        2.0 * WIN_HALF,
        WIN_Z_HI - WIN_Z_LO,
    ) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid, got {other:?}"),
    };
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&Vector3::new(15.0, 0.0, 0.5 * (WIN_Z_LO + WIN_Z_HI))),
        TransformOptions::default(),
    )
    .expect("translate window");
    id
}

/// The same window rotated a quarter turn: it pierces the wall at `u = pi/2`
/// (+Y), nowhere near the seam. The control.
fn off_seam_window(model: &mut BRepModel) -> SolidId {
    let id = match TopologyBuilder::new(model).create_box_3d(
        2.0 * WIN_HALF,
        30.0,
        WIN_Z_HI - WIN_Z_LO,
    ) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid, got {other:?}"),
    };
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&Vector3::new(0.0, 15.0, 0.5 * (WIN_Z_LO + WIN_Z_HI))),
        TransformOptions::default(),
    )
    .expect("translate window");
    id
}

fn face_ids(model: &BRepModel, solid: SolidId) -> Vec<u32> {
    let s = model.solids.get(solid).expect("solid");
    let mut out = Vec::new();
    for sh in [s.outer_shell]
        .into_iter()
        .chain(s.inner_shells.iter().copied())
    {
        if let Some(shell) = model.shells.get(sh) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

fn faces_of_kind(model: &BRepModel, solid: SolidId, kind: &str) -> Vec<u32> {
    face_ids(model, solid)
        .into_iter()
        .filter(|&fid| {
            model
                .faces
                .get(fid)
                .and_then(|f| model.surfaces.get(f.surface_id))
                .map(|s| s.type_name() == kind)
                .unwrap_or(false)
        })
        .collect()
}

fn wall_triangles(model: &BRepModel, solid: SolidId, fid: u32) -> Vec<[Point3; 3]> {
    let s = model.solids.get(solid).expect("solid");
    let mesh = tessellate_solid(s, model, &TessellationParams::default());
    assert_eq!(mesh.face_map.len(), mesh.triangles.len());
    let mut out = Vec::new();
    for (i, tri) in mesh.triangles.iter().enumerate() {
        if mesh.face_map[i] != fid {
            continue;
        }
        out.push([
            mesh.vertices[tri[0] as usize].position,
            mesh.vertices[tri[1] as usize].position,
            mesh.vertices[tri[2] as usize].position,
        ]);
    }
    out
}

fn area_of(tris: &[[Point3; 3]]) -> f64 {
    tris.iter()
        .map(|t| 0.5 * (t[1] - t[0]).cross(&(t[2] - t[0])).magnitude())
        .sum()
}

fn rel_err(a: f64, b: f64) -> f64 {
    ((a - b) / b).abs()
}

/// Analytic wall area minus the window's own patch.
fn expected_area() -> f64 {
    let half_u = (WIN_HALF / (CYL_R * CYL_R - WIN_HALF * WIN_HALF).sqrt()).atan();
    2.0 * std::f64::consts::PI * CYL_R * CYL_H - CYL_R * (2.0 * half_u) * (WIN_Z_HI - WIN_Z_LO)
}

/// Moller-Trumbore, returning the ray parameter of a hit.
fn ray_hits(tri: &[Point3; 3], origin: Point3, dir: Vector3) -> Option<f64> {
    let e1 = tri[1] - tri[0];
    let e2 = tri[2] - tri[0];
    let p = dir.cross(&e2);
    let det = e1.dot(&p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let t_vec = origin - tri[0];
    let u = t_vec.dot(&p) * inv;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let q = t_vec.cross(&e1);
    let v = dir.dot(&q) * inv;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    let t = e2.dot(&q) * inv;
    if t > 1e-9 {
        Some(t)
    } else {
        None
    }
}

fn cut_wall(model: &mut BRepModel, seam: bool) -> (SolidId, u32) {
    let cyl = cylinder(model, CYL_R, CYL_H);
    let win = if seam {
        seam_window(model)
    } else {
        off_seam_window(model)
    };
    let cut = boolean_operation(
        model,
        cyl,
        win,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    let walls = faces_of_kind(model, cut, "Cylinder");
    assert_eq!(walls.len(), 1, "one wall face expected, got {walls:?}");
    let wall = walls[0];
    assert_eq!(
        model.faces.get(wall).expect("wall").inner_loops.len(),
        1,
        "fixture precondition: the window must appear as ONE inner loop"
    );
    (cut, wall)
}

// --- the defect -------------------------------------------------------------

#[test]
fn seam_straddling_window_is_a_hole_in_the_mesh() {
    let mut model = BRepModel::new();
    let (cut, wall) = cut_wall(&mut model, true);
    let tris = wall_triangles(&model, cut, wall);
    let area = area_of(&tris);
    let expected = expected_area();
    let full = 2.0 * std::f64::consts::PI * CYL_R * CYL_H;
    assert!(
        rel_err(area, expected) <= 0.01,
        "seam window: mesh area {area:.3} mm2 != {expected:.3} mm2 \
         (full untouched wall is {full:.3}); off by {:.3}",
        area - expected
    );
}

#[test]
fn a_ray_through_the_seam_window_misses_the_wall() {
    let mut model = BRepModel::new();
    let (cut, wall) = cut_wall(&mut model, true);
    let tris = wall_triangles(&model, cut, wall);
    let origin = Point3::new(40.0, 0.0, 0.5 * (WIN_Z_LO + WIN_Z_HI));
    let dir = Vector3::new(-1.0, 0.0, 0.0);
    let near: Vec<f64> = tris
        .iter()
        .filter_map(|t| ray_hits(t, origin, dir))
        .filter(|&t| origin.x - t > 0.0)
        .collect();
    assert!(
        near.is_empty(),
        "a ray aimed at the axis through the window centre hit the near wall \
         {} time(s) at t = {near:?} -- the window is not in the mesh",
        near.len()
    );
}

// --- controls ---------------------------------------------------------------

#[test]
fn off_seam_window_is_a_hole_in_the_mesh() {
    let mut model = BRepModel::new();
    let (cut, wall) = cut_wall(&mut model, false);
    let tris = wall_triangles(&model, cut, wall);
    let area = area_of(&tris);
    let expected = expected_area();
    assert!(
        rel_err(area, expected) <= 0.01,
        "off-seam control: mesh area {area:.3} mm2 != {expected:.3} mm2; off by {:.3}",
        area - expected
    );
}

/// A face with no inner loop must mesh EXACTLY as it did before this task, and
/// the reason is structural, not incidental.
///
/// The branch re-cut is reachable only through an `Err` from
/// `align_inner_loops`, and that function's only failure sources are the
/// projection and validation of an INNER loop. A face with no inner loops
/// cannot produce one: the walk has nothing to walk, so it returns `Ok(vec![])`
/// and the new arm is never entered. This test pins both halves — the count the
/// pre-fix tessellator produced (792) and the precondition that makes it
/// unreachable — rather than an exact-bit hash, which would fingerprint this
/// machine's libm rather than the tessellator.
#[test]
fn a_hole_free_cylinder_meshes_identically_and_never_reaches_the_re_cut() {
    let mut model = BRepModel::new();
    let cyl = cylinder(&mut model, CYL_R, CYL_H);

    // The structural claim: no face of this solid carries an inner loop, so the
    // re-cut arm is unreachable for every one of them.
    let faces = face_ids(&model, cyl);
    assert!(!faces.is_empty(), "the cylinder must have faces");
    for fid in &faces {
        assert!(
            model.faces.get(*fid).expect("face").inner_loops.is_empty(),
            "face {fid} carries an inner loop; the re-cut arm is then reachable \
             and this test no longer proves byte-identity by construction"
        );
    }

    let s = model.solids.get(cyl).expect("solid");
    let mesh = tessellate_solid(s, &model, &TessellationParams::default());
    assert_eq!(
        mesh.triangles.len(),
        792,
        "hole-free cylinder triangle count moved"
    );
}

/// The re-cut chart closes: the two chart columns that replace the old seam
/// run between the SAME two cached samples, so the wall still welds shut.
///
/// Reported alongside the areas so the report carries the numbers the fix moved.
#[test]
fn the_re_cut_wall_welds_shut_and_reports_its_numbers() {
    for seam in [true, false] {
        let mut model = BRepModel::new();
        let (cut, wall) = cut_wall(&mut model, seam);
        let tris = wall_triangles(&model, cut, wall);
        println!(
            "seam={seam} wall_tris={} wall_area={:.4} expected={:.4} full={:.4}",
            tris.len(),
            area_of(&tris),
            expected_area(),
            2.0 * std::f64::consts::PI * CYL_R * CYL_H
        );
        let report = manifold_report(&model, cut, 0.02, 1e-6).expect("the cut solid must mesh");
        println!(
            "seam={seam} solid_tris={} boundary_edges={} nonmanifold_edges={}",
            report.triangles, report.boundary_edges, report.nonmanifold_edges
        );
        assert_eq!(
            report.boundary_edges, 0,
            "seam={seam}: the meshed solid must stay closed"
        );
        let cert = model.certify_solid(cut);
        println!(
            "seam={seam} cert sound={} watertight_be={} manifold_nm={}",
            cert.is_sound(),
            cert.boundary_edges,
            cert.nonmanifold_edges
        );
        assert!(
            cert.is_sound(),
            "seam={seam}: the re-cut solid must certify sound"
        );
    }
}

/// The re-cut wall is not merely present, it is the same QUALITY of mesh.
///
/// The polygon's two closing columns are 2-vertex constraint segments spanning
/// the full height. Ruppert encroachment can split such a segment, and a split
/// vertex is lifted through `surface.point_at` rather than taken from the edge
/// cache — so a bad re-cut could show up as slivers, off-surface facets, or
/// bore-bridging "wings" long before it showed up in an area sum. The off-seam
/// control is the same solid with the same window at a different angle, so its
/// numbers are the budget: aspect and min-angle within 2x, normal deviation
/// within 2 degrees, and zero boundary-crossing facets either way.
#[test]
fn the_re_cut_wall_meshes_as_cleanly_as_the_off_seam_control() {
    let mut m_seam = BRepModel::new();
    let (seam_solid, _) = cut_wall(&mut m_seam, true);
    let q_seam = mesh_quality(&m_seam, seam_solid).expect("the seam-cut solid must mesh");

    let mut m_ctrl = BRepModel::new();
    let (ctrl_solid, _) = cut_wall(&mut m_ctrl, false);
    let q_ctrl = mesh_quality(&m_ctrl, ctrl_solid).expect("the control solid must mesh");

    println!(
        "seam:    tris={} aspect={:.3} min_angle={:.4}deg dev={:.3}deg crossing={} clean={}",
        q_seam.triangles,
        q_seam.worst_aspect_ratio,
        q_seam.min_angle_deg,
        q_seam.max_normal_deviation_deg,
        q_seam.boundary_crossing_facets,
        q_seam.clean
    );
    println!(
        "control: tris={} aspect={:.3} min_angle={:.4}deg dev={:.3}deg crossing={} clean={}",
        q_ctrl.triangles,
        q_ctrl.worst_aspect_ratio,
        q_ctrl.min_angle_deg,
        q_ctrl.max_normal_deviation_deg,
        q_ctrl.boundary_crossing_facets,
        q_ctrl.clean
    );

    assert!(
        q_ctrl.clean,
        "the control must be clean or it is no budget at all"
    );
    assert!(
        q_seam.clean,
        "the re-cut wall's mesh is not clean: dev={:.3}deg crossing={} worst={:?}",
        q_seam.max_normal_deviation_deg, q_seam.boundary_crossing_facets, q_seam.worst_face
    );
    assert_eq!(
        q_seam.boundary_crossing_facets, 0,
        "the re-cut wall bridges its own chart: {} crossing facets against {} on the control",
        q_seam.boundary_crossing_facets, q_ctrl.boundary_crossing_facets
    );
    assert!(
        q_seam.max_normal_deviation_deg <= q_ctrl.max_normal_deviation_deg + 2.0,
        "re-cut normal deviation {:.3}deg exceeds the control's {:.3}deg by more than 2deg",
        q_seam.max_normal_deviation_deg,
        q_ctrl.max_normal_deviation_deg
    );
    assert!(
        q_seam.worst_aspect_ratio <= 2.0 * q_ctrl.worst_aspect_ratio,
        "re-cut worst aspect {:.3} is more than 2x the control's {:.3}",
        q_seam.worst_aspect_ratio,
        q_ctrl.worst_aspect_ratio
    );
    assert!(
        q_seam.min_angle_deg >= 0.5 * q_ctrl.min_angle_deg,
        "re-cut min angle {:.4}deg is below half the control's {:.4}deg",
        q_seam.min_angle_deg,
        q_ctrl.min_angle_deg
    );
}

// --- the piston-class case: two round breakouts, one on the seam ------------

/// Summed area of every mesh triangle lying on the OUTER WALL (centroid at
/// radius `CYL_R` about the Z axis), whichever faces the boolean split it into.
///
/// Face-id bookkeeping is not stable across the two arms below — the +X bore
/// leaves the wall as two faces and the +Y bore as one — so the wall is
/// identified geometrically instead. The bore's own lateral sits at radius 3
/// and the caps lie flat, so neither is counted.
fn outer_wall_area(model: &BRepModel, solid: SolidId) -> (f64, usize) {
    let s = model.solids.get(solid).expect("solid");
    let mesh = tessellate_solid(s, model, &TessellationParams::default());
    let mut area = 0.0;
    let mut n = 0usize;
    for tri in &mesh.triangles {
        let p = [
            mesh.vertices[tri[0] as usize].position,
            mesh.vertices[tri[1] as usize].position,
            mesh.vertices[tri[2] as usize].position,
        ];
        let cx = (p[0].x + p[1].x + p[2].x) / 3.0;
        let cy = (p[0].y + p[1].y + p[2].y) / 3.0;
        if ((cx * cx + cy * cy).sqrt() - CYL_R).abs() > 0.05 {
            continue;
        }
        area += 0.5 * (p[1] - p[0]).cross(&(p[2] - p[0])).magnitude();
        n += 1;
    }
    (area, n)
}

/// A cross bore drilled along the axis it breaks out ON.
///
/// `cross_bore_manifold` drills along +Y, so both breakouts land at `u = pi/2`
/// and `u = 3pi/2` and neither meets the seam. Rotating the SAME bore to +X puts
/// one breakout straddling `u = 0`.
///
/// **Measured, and it is the rest of the answer to "why did the piston never
/// show this?":** the +X arm has no straddling INNER LOOP to seat. Printed
/// below, the wall face (the one whose surface samples at `|xy| = 10`) comes
/// back as
///
/// ```text
///   bore+X: outer_edges=14  inner_loops=1     <- one breakout, at u = pi
///   bore+Y: outer_edges=8   inner_loops=2     <- both breakouts, neither on the seam
/// ```
///
/// Eight is the untouched wall's edge count. The +Y arm keeps it and carries
/// both breakouts as holes. The +X arm's seam-side breakout has instead been
/// spliced into the OUTER loop — 14 edges, one hole — because a round breakout
/// sitting on the seam consumes the seam edge outright. That is the "bite out of
/// the chart's edge" topology, and it needs no re-cut: `align_inner_loops` has
/// only the off-branch `u = pi` hole to seat and succeeds. The box window is
/// different precisely because it leaves the seam edge intact above and below
/// itself (12 edges, still walked twice), which is what pins the branch.
///
/// **The oracle is the +Y arm.** The two solids are the same geometry a quarter
/// turn apart, so their wall areas must agree; no analytic patch area for a
/// cylinder-cylinder breakout is needed, and a breakout the tessellator quietly
/// failed to cut would break the symmetry by exactly that patch.
fn cross_bored(model: &mut BRepModel, along_x: bool) -> SolidId {
    const BORE_R: f64 = 3.0;
    let blank = cylinder(model, CYL_R, CYL_H);
    let (base, axis) = if along_x {
        (Point3::new(-30.0, 0.0, 10.0), Vector3::new(1.0, 0.0, 0.0))
    } else {
        (Point3::new(0.0, -30.0, 10.0), Vector3::new(0.0, 1.0, 0.0))
    };
    let tool = match TopologyBuilder::new(model).create_cylinder_3d(base, axis, BORE_R, 60.0) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid, got {other:?}"),
    };
    boolean_operation(
        model,
        blank,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore difference")
}

#[test]
fn a_cross_bore_breaking_out_on_the_seam_cuts_both_its_holes() {
    let mut m_x = BRepModel::new();
    let solid_x = cross_bored(&mut m_x, true);
    let mut m_y = BRepModel::new();
    let solid_y = cross_bored(&mut m_y, false);

    // Fixture precondition: the breakouts really are inner loops on a wall face
    // in both arms, else the two arms are not the same experiment.
    for (label, model, solid) in [("X", &m_x, solid_x), ("Y", &m_y, solid_y)] {
        let holed: Vec<usize> = faces_of_kind(model, solid, "Cylinder")
            .into_iter()
            .filter_map(|fid| model.faces.get(fid))
            .map(|f| f.inner_loops.len())
            .filter(|n| *n > 0)
            .collect();
        for fid in faces_of_kind(model, solid, "Cylinder") {
            let f = model.faces.get(fid).expect("face");
            let r = model
                .surfaces
                .get(f.surface_id)
                .and_then(|s| s.point_at(0.0, 0.0).ok())
                .map(|p| (p.x * p.x + p.y * p.y).sqrt())
                .unwrap_or(-1.0);
            let ne = model
                .loops
                .get(f.outer_loop)
                .map(|l| l.edges.len())
                .unwrap_or(0);
            println!(
                "bore+{label}: face {fid} |xy| at (0,0)={r:.2} outer_edges={ne} \
                 inner_loops={}",
                f.inner_loops.len()
            );
        }
        println!("bore+{label}: holed cylinder faces = {holed:?}");
        assert!(
            !holed.is_empty(),
            "bore along {label}: the wall must carry at least one breakout loop"
        );
    }

    let (area_x, n_x) = outer_wall_area(&m_x, solid_x);
    let (area_y, n_y) = outer_wall_area(&m_y, solid_y);
    let full = 2.0 * std::f64::consts::PI * CYL_R * CYL_H;
    println!("bore+X (seam): wall_tris={n_x} area={area_x:.4}");
    println!("bore+Y (ctrl): wall_tris={n_y} area={area_y:.4}   full={full:.4}");

    assert!(
        area_y < full - 1.0,
        "the control arm must itself be holed: {area_y:.3} against a full wall of {full:.3}"
    );
    assert!(
        rel_err(area_x, area_y) <= 0.01,
        "the seam-breakout wall meshes to {area_x:.3} mm2 while the same bore a \
         quarter turn away meshes to {area_y:.3} mm2 (full wall {full:.3})"
    );

    // The bore is open: a ray down its axis must not meet the near wall.
    let s = m_x.solids.get(solid_x).expect("solid");
    let mesh = tessellate_solid(s, &m_x, &TessellationParams::default());
    let origin = Point3::new(40.0, 0.0, 10.0);
    let dir = Vector3::new(-1.0, 0.0, 0.0);
    let near: Vec<f64> = mesh
        .triangles
        .iter()
        .map(|t| {
            [
                mesh.vertices[t[0] as usize].position,
                mesh.vertices[t[1] as usize].position,
                mesh.vertices[t[2] as usize].position,
            ]
        })
        .filter(|t| {
            let cx = (t[0].x + t[1].x + t[2].x) / 3.0;
            let cy = (t[0].y + t[1].y + t[2].y) / 3.0;
            ((cx * cx + cy * cy).sqrt() - CYL_R).abs() <= 0.05
        })
        .filter_map(|t| ray_hits(&t, origin, dir))
        .filter(|&t| origin.x - t > 0.0)
        .collect();
    assert!(
        near.is_empty(),
        "a ray down the bore axis hit the near wall {} time(s) at t = {near:?}",
        near.len()
    );
}
