//! BLEND-WELD STRESS HARNESS — a bug HUNT, not a fixer.
//!
//! Targets a specific, recurring defect class: a blend / curved surface that is
//! routed to the GENERIC curved-CDT mesher samples its boundary INDEPENDENTLY of
//! the shared `EdgeSampleCache`, so its rims do not weld bit-for-bit with the
//! neighbour faces. The result is open edges (`boundary_edges > 0`) and/or a
//! flipped winding (`oriented == false`) on the welded display mesh, even though
//! the B-Rep is structurally valid. Two instances were just fixed by adding a
//! dedicated CONFORMING mesher that takes the boundary verbatim from the cache:
//!
//!   * cone-rim fillet  (`5d31f3d`, `tessellate_blend_torus_conforming`)
//!   * bore-rim chamfer (`1a03aff`, `tessellate_blend_cone_conforming`)
//!
//! Other blend / curved geometries likely share the defect. This battery throws
//! a range of blend faces — fillet AND chamfer on sphere∪cylinder seams, torus
//! edges, several cone configs, cylinder cap rims AND lateral edges, and annular
//! caps (washer / tube / flange) OUTER + BORE rims — at the kernel, plus a
//! broadened pass over loft / nurbs-loft / shell / sweep / revolve-with-blend.
//!
//! Each case is judged against THREE independent oracles, none weakened here:
//!   1. B-Rep   — `primitives::validation::validate_solid_scoped` (Standard).
//!   2. Mesh    — `harness::watertight::manifold_report`: the welded display
//!                mesh must close (`boundary_edges == 0`), be 2-manifold
//!                (`nonmanifold_edges == 0`) and be consistently wound
//!                (`oriented == true`).
//!   3. Self-cert — `BRepModel::certify_solid(..).is_sound()`: the kernel's own
//!                intrinsic certificate (self-intersection-free + tessellation /
//!                mesh-quality, which the first two cannot see).
//!
//! A case that BUILDS but fails ANY oracle is a BUG. Each test reports the exact
//! defect (open-edge / non-manifold counts, `oriented`, which certificate
//! dimension is false) and NEVER `panic!`s on a build failure — a failed op is
//! itself a reportable datum, recorded as DID-NOT-BUILD (an honest reject is not
//! a bug). This file SURFACES bugs; it does not gate them.
//!
//! Run: cargo test -p geometry-engine --test blend_weld_stress -- --nocapture

use std::f64::consts::TAU;

use geometry_engine::harness::watertight::{manifold_report, ManifoldReport};
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::{
    boolean_operation, loft_profiles, offset_solid, sweep_profile, BooleanOp, BooleanOptions,
    CommonOptions, LoftOptions, OffsetOptions, SweepOptions,
};
use geometry_engine::primitives::curve::{Circle, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

// ---------------------------------------------------------------------------
// Oracle plumbing — shared by every case. Thresholds are NOT relaxed.
// ---------------------------------------------------------------------------

/// Certification chord (matches the kernel's own certificate density) and the
/// weld epsilon used across the existing harnesses.
const CERT_CHORD: f64 = 0.1;
const WELD_EPS: f64 = 1e-6;

/// The full verdict for one case.
struct Verdict {
    /// Did the op return `Ok`? `false` ⇒ DID-NOT-BUILD (all else N/A).
    built: bool,
    /// `validate_solid_scoped(Standard).is_valid`.
    brep_valid: bool,
    /// The welded-mesh report (None when the solid does not tessellate).
    mesh: Option<ManifoldReport>,
    /// `certify_solid(..).is_sound()`.
    sound: bool,
    /// Per-certificate-dimension breakdown, for precise FAIL reporting.
    cert_line: String,
    /// Operation error string when `!built`.
    err: String,
}

impl Verdict {
    /// All three oracles green.
    fn pass(&self) -> bool {
        let mesh_ok = self
            .mesh
            .as_ref()
            .map(|m| {
                m.boundary_edges == 0 && m.nonmanifold_edges == 0 && m.oriented && m.triangles > 0
            })
            .unwrap_or(false);
        self.built && self.brep_valid && mesh_ok && self.sound
    }

    /// `true` when the failure carries the BLEND-WELD signature: the solid BUILT
    /// and its B-Rep is structurally VALID, but the welded mesh has open edges
    /// and/or is not consistently oriented. That is exactly what an unwelded /
    /// flipped blend band (boundary not taken from `EdgeSampleCache`) produces —
    /// versus a different root cause (failed build, B-Rep-invalid, or a
    /// self-intersection / mesh-quality cert miss with a clean mesh).
    fn blend_weld_signature(&self) -> bool {
        if !self.built || !self.brep_valid {
            return false;
        }
        match &self.mesh {
            Some(m) => m.boundary_edges > 0 || !m.oriented,
            None => false,
        }
    }

    /// One-line classification tag appended to the DEFECT column.
    fn class_tag(&self) -> &'static str {
        if !self.built {
            "[reject — honest, not a bug]"
        } else if self.pass() {
            ""
        } else if self.blend_weld_signature() {
            "[BLEND-WELD class — boundary not from EdgeSampleCache?]"
        } else if !self.brep_valid {
            "[B-Rep-invalid — different root cause]"
        } else {
            "[cert/other — different root cause]"
        }
    }

    fn defect(&self) -> String {
        if !self.built {
            return format!("DID-NOT-BUILD: {}", self.err);
        }
        let mut parts = Vec::new();
        if !self.brep_valid {
            parts.push("brep_INVALID".to_string());
        }
        match &self.mesh {
            None => parts.push("mesh=NONE(no-tessellation)".to_string()),
            Some(m) => {
                if m.boundary_edges != 0 {
                    parts.push(format!("open_edges={}", m.boundary_edges));
                }
                if m.nonmanifold_edges != 0 {
                    parts.push(format!("nonmanifold_edges={}", m.nonmanifold_edges));
                }
                if !m.oriented {
                    parts.push(format!(
                        "NOT_ORIENTED(inconsistent_directed={})",
                        m.inconsistent_directed_edges
                    ));
                }
                if m.triangles == 0 {
                    parts.push("zero_triangles".to_string());
                }
            }
        }
        if !self.sound {
            parts.push(format!("NOT_SOUND[{}]", self.cert_line));
        }
        if parts.is_empty() {
            "PASS".to_string()
        } else {
            let tag = self.class_tag();
            if tag.is_empty() {
                parts.join(" ")
            } else {
                format!("{} {}", parts.join(" "), tag)
            }
        }
    }
}

/// Run all three oracles on `solid` after an op `result`.
fn judge(model: &mut BRepModel, solid: SolidId, result: Result<(), String>) -> Verdict {
    if let Err(e) = result {
        return Verdict {
            built: false,
            brep_valid: false,
            mesh: None,
            sound: false,
            cert_line: String::new(),
            err: e,
        };
    }

    let brep = validate_solid_scoped(
        model,
        solid,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    let mesh = manifold_report(model, solid, CERT_CHORD, WELD_EPS);
    let cert = model.certify_solid(solid);
    let cert_line = format!(
        "brep={} wt={} manif={} orient={} self_int_free={} tess_clean={} mesh_q_clean={}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.self_intersection_free,
        cert.tessellation.clean,
        cert.mesh_quality.clean,
    );

    Verdict {
        built: true,
        brep_valid: brep.is_valid,
        mesh,
        sound: cert.is_sound(),
        cert_line,
        err: String::new(),
    }
}

/// Row accumulator so each test prints a clean PASS/FAIL table.
struct Table {
    rows: Vec<(String, bool, String)>,
}

impl Table {
    fn new() -> Self {
        Table { rows: Vec::new() }
    }

    fn add(&mut self, case: &str, v: &Verdict) {
        self.rows.push((case.to_string(), v.pass(), v.defect()));
    }

    /// Print the table and return the number of FAILs.
    fn render(&self, title: &str) -> usize {
        eprintln!("\n================ {title} ================");
        eprintln!("{:<54} {:<6} {}", "CASE", "RESULT", "DEFECT");
        eprintln!("{}", "-".repeat(130));
        let mut fails = 0usize;
        for (case, pass, defect) in &self.rows {
            let tag = if *pass { "PASS" } else { "FAIL" };
            if !*pass {
                fails += 1;
            }
            eprintln!("{case:<54} {tag:<6} {defect}");
        }
        eprintln!("{}", "-".repeat(130));
        eprintln!(
            "{title}: {} cases, {} PASS, {} FAIL",
            self.rows.len(),
            self.rows.len() - fails,
            fails
        );
        fails
    }
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("expected Solid, got {o:?}"),
    }
}

fn last_solid(model: &BRepModel) -> SolidId {
    model
        .solids
        .iter()
        .last()
        .map(|(id, _)| id)
        .expect("a solid exists")
}

fn make_cylinder(model: &mut BRepModel, r: f64, h: f64) -> SolidId {
    sid(TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, r, h)
        .expect("cylinder"))
}

fn make_sphere(model: &mut BRepModel, center: Point3, r: f64) -> SolidId {
    sid(TopologyBuilder::new(model)
        .create_sphere_3d(center, r)
        .expect("sphere"))
}

fn make_cone(model: &mut BRepModel, r_base: f64, r_top: f64, h: f64) -> SolidId {
    sid(TopologyBuilder::new(model)
        .create_cone_3d(Point3::ORIGIN, Vector3::Z, r_base, r_top, h)
        .expect("cone"))
}

fn make_torus(model: &mut BRepModel, major: f64, minor: f64) -> SolidId {
    sid(TopologyBuilder::new(model)
        .create_torus_3d(Point3::ORIGIN, Vector3::Z, major, minor)
        .expect("torus"))
}

/// All edges of a solid's outer shell (plus inner shells), de-duplicated.
fn all_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let mut shells = vec![s.outer_shell];
    shells.extend_from_slice(&s.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            for lid in face.all_loops() {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &eid in &lp.edges {
                    if seen.insert(eid) {
                        out.push(eid);
                    }
                }
            }
        }
    }
    out
}

/// The STRAIGHT (linear) edges of a solid.
fn linear_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    all_edges(model, solid)
        .into_iter()
        .filter(|&eid| {
            model
                .edges
                .get(eid)
                .and_then(|e| model.curves.get(e.curve_id))
                .map(|c| c.is_linear(Tolerance::default()))
                .unwrap_or(false)
        })
        .collect()
}

/// The CIRCULAR (non-linear) edges of a solid.
fn circular_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    all_edges(model, solid)
        .into_iter()
        .filter(|&eid| {
            model
                .edges
                .get(eid)
                .and_then(|e| model.curves.get(e.curve_id))
                .map(|c| !c.is_linear(Tolerance::default()))
                .unwrap_or(false)
        })
        .collect()
}

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        ..Default::default()
    }
}

fn chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        symmetric: true,
        ..Default::default()
    }
}

fn do_fillet(
    model: &mut BRepModel,
    solid: SolidId,
    edges: Vec<EdgeId>,
    r: f64,
) -> Result<(), String> {
    fillet_edges(model, solid, edges, fillet_opts(r))
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

fn do_chamfer(
    model: &mut BRepModel,
    solid: SolidId,
    edges: Vec<EdgeId>,
    d: f64,
) -> Result<(), String> {
    chamfer_edges(model, solid, edges, chamfer_opts(d))
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// Apply a fillet to all circular rims of `solid`, then judge.
fn fillet_and_judge(model: &mut BRepModel, solid: SolidId, edges: Vec<EdgeId>, r: f64) -> Verdict {
    let res = do_fillet(model, solid, edges, r);
    judge(model, solid, res)
}

fn chamfer_and_judge(model: &mut BRepModel, solid: SolidId, edges: Vec<EdgeId>, d: f64) -> Verdict {
    let res = do_chamfer(model, solid, edges, d);
    judge(model, solid, res)
}

/// Revolve a closed (r, z) profile a full turn about +Z (annular cap builder).
fn revolve_tube(m: &mut BRepModel, pts: &[(f64, f64)]) -> SolidId {
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: TAU,
        segments: 64,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).expect("tube revolve")
}

/// Closed rim edge whose seam vertex sits at radius `r_want`, height `z_want`.
fn rim_at(m: &BRepModel, r_want: f64, z_want: f64) -> Option<EdgeId> {
    m.edges.iter().find_map(|(id, e)| {
        if !e.is_loop() {
            return None;
        }
        let p = m.vertices.get_position(e.start_vertex)?;
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        if (r - r_want).abs() < 0.5 && (p[2] - z_want).abs() < 0.5 {
            Some(id)
        } else {
            None
        }
    })
}

/// A closed circle of `radius` in the plane z = `z`, centred on the axis.
fn circle_profile(m: &mut BRepModel, radius: f64, z: f64) -> Vec<EdgeId> {
    let center = Point3::new(0.0, 0.0, z);
    let seam = m
        .vertices
        .add_or_find(center.x + radius, center.y, center.z, 1e-6);
    let cid = m.curves.add(Box::new(
        Circle::new(center, Vector3::new(0.0, 0.0, 1.0), radius).expect("circle"),
    ));
    vec![m.edges.add(Edge::new(
        0,
        seam,
        seam,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ))]
}

fn line_edge(m: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let pa = m.vertices.get(a).expect("a").position;
    let pb = m.vertices.get(b).expect("b").position;
    let cid = m
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    m.edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
}

/// A closed `side`-square polygon in the plane z = `z`, centred on the axis.
fn square_profile(m: &mut BRepModel, side: f64, z: f64) -> Vec<EdgeId> {
    let h = side / 2.0;
    let v0 = m.vertices.add(-h, -h, z);
    let v1 = m.vertices.add(h, -h, z);
    let v2 = m.vertices.add(h, h, z);
    let v3 = m.vertices.add(-h, h, z);
    vec![
        line_edge(m, v0, v1),
        line_edge(m, v1, v2),
        line_edge(m, v2, v3),
        line_edge(m, v3, v0),
    ]
}

// Tube: outer wall R10, bore R6, z 0..20. Outer wall + bore are cylinders;
// caps are annular planes. The bore-top rim (r≈6, z≈20) is the inner loop.
const TUBE: &[(f64, f64)] = &[(10.0, 0.0), (10.0, 20.0), (6.0, 20.0), (6.0, 0.0)];

// Flange: a thin wide annular plate (R20 outer, R8 bore, z 0..4) — short walls,
// big caps, a regime where the bore/outer rims are close to the cap planes.
const FLANGE: &[(f64, f64)] = &[(20.0, 0.0), (20.0, 4.0), (8.0, 4.0), (8.0, 0.0)];

// Washer: small thin annulus (R6 outer, R3 bore, z 0..1.5).
const WASHER: &[(f64, f64)] = &[(6.0, 0.0), (6.0, 1.5), (3.0, 1.5), (3.0, 0.0)];

/// sphere ∪ cylinder: a sphere centred on the cylinder axis, partway up, so the
/// union has a curved circular intersection seam (sphere↔cylinder SSI circle) —
/// the prime curved-blend-boundary candidate.
fn sphere_union_cylinder(
    sphere_r: f64,
    cyl_r: f64,
    cyl_h: f64,
    sphere_z: f64,
) -> Option<(BRepModel, SolidId)> {
    let mut model = BRepModel::new();
    let cyl = make_cylinder(&mut model, cyl_r, cyl_h);
    let sph = make_sphere(&mut model, Point3::new(0.0, 0.0, sphere_z), sphere_r);
    let res = boolean_operation(
        &mut model,
        cyl,
        sph,
        BooleanOp::Union,
        BooleanOptions::default(),
    );
    match res {
        Ok(s) => Some((model, s)),
        Err(_) => None,
    }
}

// ===========================================================================
// TEST 1 — BLEND-WELD on sphere∪cylinder intersection seam edges.
// ===========================================================================

#[test]
fn blend_weld_sphere_union_cylinder() {
    let mut table = Table::new();

    // Several sphere/cylinder size ratios so the seam circle sits at small,
    // medium, large radius relative to the cylinder.
    let configs: &[(&str, f64, f64, f64, f64)] = &[
        // (label, sphere_r, cyl_r, cyl_h, sphere_z)
        ("sph5 ∪ cyl4 h20 z18", 5.0, 4.0, 20.0, 18.0),
        ("sph8 ∪ cyl5 h20 z16", 8.0, 5.0, 20.0, 16.0),
        ("sph3 ∪ cyl6 h20 z19", 3.0, 6.0, 20.0, 19.0),
    ];

    for &(label, sr, cr, ch, sz) in configs {
        // First: does the union even build + is it itself clean? (baseline)
        match sphere_union_cylinder(sr, cr, ch, sz) {
            Some((mut model, solid)) => {
                let base = judge(&mut model, solid, Ok(()));
                table.add(&format!("{label}: union (no blend)"), &base);

                // Fillet every circular edge of the union (the SSI seam + cap rims).
                if let Some((mut m2, s2)) = sphere_union_cylinder(sr, cr, ch, sz) {
                    let rims = circular_edges(&m2, s2);
                    let n = rims.len();
                    let v = fillet_and_judge(&mut m2, s2, rims, 0.5);
                    table.add(&format!("{label}: fillet {n} circ edges r=0.5"), &v);
                }
                if let Some((mut m3, s3)) = sphere_union_cylinder(sr, cr, ch, sz) {
                    let rims = circular_edges(&m3, s3);
                    let n = rims.len();
                    let v = chamfer_and_judge(&mut m3, s3, rims, 0.5);
                    table.add(&format!("{label}: chamfer {n} circ edges d=0.5"), &v);
                }
            }
            None => {
                let v = Verdict {
                    built: false,
                    brep_valid: false,
                    mesh: None,
                    sound: false,
                    cert_line: String::new(),
                    err: "sphere∪cylinder union returned Err".to_string(),
                };
                table.add(&format!("{label}: union (no blend)"), &v);
            }
        }
    }

    let fails = table.render("BLEND-WELD: sphere∪cylinder seam");
    eprintln!(
        "NOTE: a curved sphere↔cylinder SSI seam fillet/chamfer is a prime new-bug \
         candidate (general curved-CDT path; the cone-rim/bore-rim fixes were the same class)."
    );
    let _ = fails;
}

// ===========================================================================
// TEST 2 — BLEND-WELD on torus edges (fillet + chamfer at varied radii).
// ===========================================================================

#[test]
fn blend_weld_torus_edges() {
    let mut table = Table::new();

    // A full torus has no rim edges, but its seam edges exist; to get blendable
    // rims, build a tube (annular cap) whose bore/outer rims are circles and a
    // torus primitive whose seam circle we can blend after a boolean. Here we
    // exercise the torus PRIMITIVE directly: fillet/chamfer its meridian/equator
    // seam edges, then also a torus∪cylinder boss whose seam is curved.
    for &minor in &[1.0_f64, 2.5, 4.0] {
        // torus major 10, minor varies.
        let mut model = BRepModel::new();
        let solid = make_torus(&mut model, 10.0, minor);
        let edges = circular_edges(&model, solid);
        let n = edges.len();
        // If a torus exposes no circular rim edges (degenerate seam), this is a
        // no-op set; fillet of an empty set is reported as DID-NOT-BUILD.
        let v = if n == 0 {
            Verdict {
                built: false,
                brep_valid: false,
                mesh: None,
                sound: false,
                cert_line: String::new(),
                err: "torus exposes no circular rim edges to blend".to_string(),
            }
        } else {
            fillet_and_judge(&mut model, solid, edges, (minor * 0.3).min(0.8))
        };
        table.add(
            &format!("torus maj10 min{minor}: fillet {n} circ edges"),
            &v,
        );
    }

    // torus ∪ cylinder boss: the torus sits on a cylinder, union seam is curved.
    {
        let mut model = BRepModel::new();
        let tor = make_torus(&mut model, 8.0, 2.0);
        let cyl = make_cylinder(&mut model, 3.0, 20.0);
        match boolean_operation(
            &mut model,
            tor,
            cyl,
            BooleanOp::Union,
            BooleanOptions::default(),
        ) {
            Ok(solid) => {
                let edges = circular_edges(&model, solid);
                let n = edges.len();
                let v = fillet_and_judge(&mut model, solid, edges, 0.5);
                table.add(&format!("torus∪cyl: fillet {n} circ edges r=0.5"), &v);
            }
            Err(e) => {
                let v = Verdict {
                    built: false,
                    brep_valid: false,
                    mesh: None,
                    sound: false,
                    cert_line: String::new(),
                    err: format!("torus∪cyl union: {e:?}"),
                };
                table.add("torus∪cyl: fillet circ edges r=0.5", &v);
            }
        }
    }

    table.render("BLEND-WELD: torus edges");
}

// ===========================================================================
// TEST 3 — BLEND-WELD on cone edges (several configs, fillet + chamfer).
// ===========================================================================

#[test]
fn blend_weld_cone_edges() {
    let mut table = Table::new();

    // (label, r_base, r_top, h) — full cone (r_top=0), shallow frustum, steep
    // frustum, near-cylinder frustum.
    let cones: &[(&str, f64, f64, f64)] = &[
        ("cone r5→0 h12 (full)", 5.0, 0.0001, 12.0),
        ("frustum r5→2 h12", 5.0, 2.0, 12.0),
        ("frustum r8→1 h6 (steep)", 8.0, 1.0, 6.0),
        ("frustum r5→4.5 h12 (near-cyl)", 5.0, 4.5, 12.0),
    ];

    for &r in &[0.3_f64, 0.8, 1.5] {
        for &(label, rb, rt, h) in cones {
            // fillet all rims
            {
                let mut model = BRepModel::new();
                let solid = make_cone(&mut model, rb, rt, h);
                let rims = circular_edges(&model, solid);
                let n = rims.len();
                let v = fillet_and_judge(&mut model, solid, rims, r);
                table.add(&format!("{label}: fillet {n} rims r={r}"), &v);
            }
            // chamfer all rims
            {
                let mut model = BRepModel::new();
                let solid = make_cone(&mut model, rb, rt, h);
                let rims = circular_edges(&model, solid);
                let n = rims.len();
                let v = chamfer_and_judge(&mut model, solid, rims, r);
                table.add(&format!("{label}: chamfer {n} rims d={r}"), &v);
            }
        }
    }

    let fails = table.render("BLEND-WELD: cone edges (configs × radii)");
    eprintln!(
        "NOTE: cone-rim fillet was JUST FIXED (#89, 5d31f3d). Any FAIL on a cone-rim \
         fillet/chamfer with the BLEND-WELD signature is a candidate REGRESSION or a \
         new config the conforming mesher does not yet cover."
    );
    let _ = fails;
}

// ===========================================================================
// TEST 4 — BLEND-WELD on cylinder cap rims AND lateral edge, varied radii.
// ===========================================================================

#[test]
fn blend_weld_cylinder_rims_and_lateral() {
    let mut table = Table::new();

    // Small / medium / large cylinders × small/medium/large blend radius.
    let cyls: &[(&str, f64, f64)] = &[
        ("cyl r3 h10 (small)", 3.0, 10.0),
        ("cyl r10 h30 (medium)", 10.0, 30.0),
        ("cyl r25 h60 (large)", 25.0, 60.0),
    ];

    for &(label, r, h) in cyls {
        let blends = [0.2_f64, 1.0, (r * 0.4).min(8.0)];
        for &b in &blends {
            // fillet cap rims
            {
                let mut model = BRepModel::new();
                let solid = make_cylinder(&mut model, r, h);
                let rims = circular_edges(&model, solid);
                let n = rims.len();
                let v = fillet_and_judge(&mut model, solid, rims, b);
                table.add(&format!("{label}: fillet {n} cap rims r={b}"), &v);
            }
            // chamfer cap rims
            {
                let mut model = BRepModel::new();
                let solid = make_cylinder(&mut model, r, h);
                let rims = circular_edges(&model, solid);
                let n = rims.len();
                let v = chamfer_and_judge(&mut model, solid, rims, b);
                table.add(&format!("{label}: chamfer {n} cap rims d={b}"), &v);
            }
        }
    }

    // Lateral seam edge of a cylinder (the straight generator line where the
    // single cylindrical face seam-closes). Filleting a lateral edge is unusual
    // but a valid blend-weld probe.
    {
        let mut model = BRepModel::new();
        let solid = make_cylinder(&mut model, 10.0, 30.0);
        let lat = linear_edges(&model, solid);
        let n = lat.len();
        let v = if n == 0 {
            Verdict {
                built: false,
                brep_valid: false,
                mesh: None,
                sound: false,
                cert_line: String::new(),
                err: "cylinder exposes no linear lateral seam edge".to_string(),
            }
        } else {
            fillet_and_judge(&mut model, solid, lat, 1.0)
        };
        table.add(
            &format!("cyl r10 h30: fillet {n} lateral seam edges r=1.0"),
            &v,
        );
    }

    table.render("BLEND-WELD: cylinder cap rims + lateral");
}

// ===========================================================================
// TEST 5 — BLEND-WELD on annular-cap (washer / tube / flange) OUTER + BORE rims.
// ===========================================================================

#[test]
fn blend_weld_annular_cap_rims() {
    let mut table = Table::new();

    // (label, profile, outer_r, bore_r, top_z) — blend the outer-top + bore-top
    // rims of each annular cap, both fillet and chamfer, at small/large radii.
    let parts: &[(&str, &[(f64, f64)], f64, f64, f64)] = &[
        ("washer R6/r3 h1.5", WASHER, 6.0, 3.0, 1.5),
        ("tube R10/r6 h20", TUBE, 10.0, 6.0, 20.0),
        ("flange R20/r8 h4", FLANGE, 20.0, 8.0, 4.0),
    ];

    for &(label, profile, outer_r, bore_r, top_z) in parts {
        // Pick a blend radius that fits the annulus width and the wall height.
        let width = outer_r - bore_r;
        let r_small = (width * 0.1).min(top_z * 0.3).max(0.1);
        let r_large = (width * 0.35).min(top_z * 0.6).max(r_small + 0.05);

        for (rtag, rb) in [("r_small", r_small), ("r_large", r_large)] {
            // OUTER rim fillet
            {
                let mut m = BRepModel::new();
                let s = revolve_tube(&mut m, profile);
                match rim_at(&m, outer_r, top_z) {
                    Some(rim) => {
                        let v = fillet_and_judge(&mut m, s, vec![rim], rb);
                        table.add(&format!("{label}: OUTER-rim fillet {rtag}={rb:.2}"), &v);
                    }
                    None => no_rim(&mut table, &format!("{label}: OUTER-rim fillet {rtag}")),
                }
            }
            // OUTER rim chamfer
            {
                let mut m = BRepModel::new();
                let s = revolve_tube(&mut m, profile);
                match rim_at(&m, outer_r, top_z) {
                    Some(rim) => {
                        let v = chamfer_and_judge(&mut m, s, vec![rim], rb);
                        table.add(&format!("{label}: OUTER-rim chamfer {rtag}={rb:.2}"), &v);
                    }
                    None => no_rim(&mut table, &format!("{label}: OUTER-rim chamfer {rtag}")),
                }
            }
            // BORE rim fillet
            {
                let mut m = BRepModel::new();
                let s = revolve_tube(&mut m, profile);
                match rim_at(&m, bore_r, top_z) {
                    Some(rim) => {
                        let v = fillet_and_judge(&mut m, s, vec![rim], rb);
                        table.add(&format!("{label}: BORE-rim fillet {rtag}={rb:.2}"), &v);
                    }
                    None => no_rim(&mut table, &format!("{label}: BORE-rim fillet {rtag}")),
                }
            }
            // BORE rim chamfer
            {
                let mut m = BRepModel::new();
                let s = revolve_tube(&mut m, profile);
                match rim_at(&m, bore_r, top_z) {
                    Some(rim) => {
                        let v = chamfer_and_judge(&mut m, s, vec![rim], rb);
                        table.add(&format!("{label}: BORE-rim chamfer {rtag}={rb:.2}"), &v);
                    }
                    None => no_rim(&mut table, &format!("{label}: BORE-rim chamfer {rtag}")),
                }
            }
        }
    }

    let fails = table.render("BLEND-WELD: annular-cap OUTER + BORE rims");
    eprintln!(
        "NOTE: bore-rim chamfer was JUST FIXED (1a03aff, cone-blend conforming weld). \
         Outer-rim fillet of an annular cap is the documented-FIXED #26 path. Any FAIL \
         here with the BLEND-WELD signature is a regression or an uncovered config."
    );
    let _ = fails;
}

fn no_rim(table: &mut Table, label: &str) {
    let v = Verdict {
        built: false,
        brep_valid: false,
        mesh: None,
        sound: false,
        cert_line: String::new(),
        err: "target rim edge not located on the annular cap".to_string(),
    };
    table.add(label, &v);
}

// ===========================================================================
// TEST 6 — BROADEN: loft / nurbs-loft with varied cross-sections.
// ===========================================================================

#[test]
fn broaden_loft_varied_sections() {
    let mut table = Table::new();

    // circle → circle (cylinder-like), circle → square → circle,
    // square → circle, three circles (Cubic/smooth). `loft_profiles` returns the
    // result solid id, so a failed build is recorded as DID-NOT-BUILD without a
    // dangling `last_solid` lookup that would panic when no solid was created.
    {
        let mut m = BRepModel::new();
        let p0 = circle_profile(&mut m, 10.0, 0.0);
        let p1 = circle_profile(&mut m, 4.0, 30.0);
        let v = loft_and_judge(&mut m, vec![p0, p1], loft_solid_opts());
        table.add("loft circle10→circle4 (Linear)", &v);
    }
    {
        let mut m = BRepModel::new();
        let p0 = circle_profile(&mut m, 25.0, 0.0);
        let p1 = square_profile(&mut m, 40.0, 50.0);
        let p2 = circle_profile(&mut m, 15.0, 100.0);
        let v = loft_and_judge(&mut m, vec![p0, p1, p2], loft_solid_opts());
        table.add("loft circle25→square40→circle15", &v);
    }
    {
        let mut m = BRepModel::new();
        let p0 = square_profile(&mut m, 20.0, 0.0);
        let p1 = circle_profile(&mut m, 8.0, 25.0);
        let v = loft_and_judge(&mut m, vec![p0, p1], loft_solid_opts());
        table.add("loft square20→circle8", &v);
    }
    {
        // nurbs/smooth loft via Cubic interpolation across 3 circles.
        let mut m = BRepModel::new();
        let p0 = circle_profile(&mut m, 12.0, 0.0);
        let p1 = circle_profile(&mut m, 20.0, 20.0);
        let p2 = circle_profile(&mut m, 6.0, 40.0);
        let mut opts = loft_solid_opts();
        opts.loft_type = geometry_engine::operations::loft::LoftType::Cubic;
        let v = loft_and_judge(&mut m, vec![p0, p1, p2], opts);
        table.add("loft circle12→20→6 (Cubic/smooth)", &v);
    }

    table.render("BROADEN: loft varied cross-sections");
}

/// Loft `profiles`, judging the result solid only on success — a build failure
/// is recorded as DID-NOT-BUILD (no `last_solid` lookup, which would panic when
/// the loft produced no solid).
fn loft_and_judge(m: &mut BRepModel, profiles: Vec<Vec<EdgeId>>, opts: LoftOptions) -> Verdict {
    match loft_profiles(m, profiles, opts) {
        Ok(solid) => judge(m, solid, Ok(())),
        Err(e) => judge(m, 0u32, Err(format!("{e:?}"))),
    }
}

fn loft_solid_opts() -> LoftOptions {
    LoftOptions {
        create_solid: true,
        ..Default::default()
    }
}

// ===========================================================================
// TEST 7 — BROADEN: shell at varied thickness (incl. thin walls).
// ===========================================================================

#[test]
fn broaden_shell_varied_thickness() {
    let mut table = Table::new();

    // Shell a 20×20×20 box (top face removed) at thicknesses from thick to thin.
    // Thin walls (t≪side) are the self-intersection / mesh-quality risk regime.
    for &t in &[5.0_f64, 2.0, 1.0, 0.5, 0.2, 0.05] {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box");
        let solid = last_solid(&model);
        let Some(top) = top_z_face(&model, solid) else {
            no_rim(&mut table, &format!("shell box20 t={t} (no top face)"));
            continue;
        };
        let opts = OffsetOptions {
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let res = offset_solid(&mut model, solid, t, vec![top], opts)
            .map(|_| ())
            .map_err(|e| format!("{e:?}"));
        // offset_solid returns a NEW solid id on success; locate it.
        let target = last_solid(&model);
        let v = judge(&mut model, target, res);
        table.add(&format!("shell box20 (top open) t={t}"), &v);
    }

    let fails = table.render("BROADEN: shell varied thickness");
    eprintln!(
        "NOTE: very thin walls (t≤0.2 of a 20-box) are the self-intersection / \
         mesh-quality candidate — watch self_int_free / mesh_q_clean in the cert line."
    );
    let _ = fails;
}

/// The box's +Z face, located by surface normal.
fn top_z_face(model: &BRepModel, solid: SolidId) -> Option<FaceId> {
    let s = model.solids.get(solid)?;
    let shell = model.shells.get(s.outer_shell)?;
    for &face_id in &shell.faces {
        if let Some(face) = model.faces.get(face_id) {
            if let Some(surface) = model.surfaces.get(face.surface_id) {
                if let Ok(n) = surface.normal_at(0.5, 0.5) {
                    if (n.z - 1.0).abs() < 1e-3 {
                        return Some(face_id);
                    }
                }
            }
        }
    }
    None
}

// ===========================================================================
// TEST 8 — BROADEN: sweep + revolve-with-blend.
// ===========================================================================

#[test]
fn broaden_sweep_and_revolve_blend() {
    let mut table = Table::new();

    // 8a: sweep a rectangle along a straight path (baseline prism).
    {
        let mut model = BRepModel::new();
        let profile = rectangle_xy(&mut model, 6.0, 4.0);
        let va = model.vertices.add(0.0, 0.0, 0.0);
        let vb = model.vertices.add(0.0, 0.0, 30.0);
        let path = line_edge(&mut model, va, vb);
        let res = sweep_profile(&mut model, profile, path, SweepOptions::default())
            .map(|_| ())
            .map_err(|e| format!("{e:?}"));
        let s = last_solid(&model);
        let v = judge(&mut model, s, res);
        // GATE: a trivial straight rectangular prism sweep MUST be fully sound —
        // valid B-Rep AND a clean watertight/oriented mesh. This pins the
        // SWEEP-SCRATCH-PROFILE fix: the template profile face is dropped after
        // the solid is assembled, so its profile edges no longer read as
        // boundary-edge gaps to the whole-model validator.
        assert!(
            v.built && v.brep_valid,
            "straight sweep must have a valid B-Rep: {}",
            v.defect()
        );
        assert!(
            v.pass() && v.sound,
            "straight sweep must be sound (brep + clean mesh + cert): {}",
            v.defect()
        );
        table.add("sweep rect6×4 along +Z 30 (straight)", &v);
    }

    // 8b: sweep a rectangle, then fillet its (linear) edges = swept-solid blend.
    {
        let mut model = BRepModel::new();
        let profile = rectangle_xy(&mut model, 6.0, 4.0);
        let va = model.vertices.add(0.0, 0.0, 0.0);
        let vb = model.vertices.add(0.0, 0.0, 30.0);
        let path = line_edge(&mut model, va, vb);
        match sweep_profile(&mut model, profile, path, SweepOptions::default()) {
            Ok(s) => {
                let edges = linear_edges(&model, s);
                // Fillet a couple of the swept solid's long edges.
                let take: Vec<_> = edges.into_iter().take(2).collect();
                let v = fillet_and_judge(&mut model, s, take, 0.5);
                table.add("sweep rect6×4: fillet 2 lateral edges r=0.5", &v);
            }
            Err(e) => no_rim(&mut table, &format!("sweep+fillet ({e:?})")),
        }
    }

    // 8c: revolve a profile then fillet the outer rim = revolve-with-blend.
    {
        let mut m = BRepModel::new();
        let s = revolve_tube(&mut m, TUBE);
        match rim_at(&m, 10.0, 20.0) {
            Some(rim) => {
                let v = fillet_and_judge(&mut m, s, vec![rim], 1.0);
                table.add("revolve tube: fillet outer-top rim r=1.0", &v);
            }
            None => no_rim(&mut table, "revolve tube outer-rim fillet"),
        }
    }

    // 8d: revolve a SLOPED (cone-walled) tube then fillet the outer rim — the
    // Plane–Cone rim (#89) path on a revolve result.
    {
        let cone_tube: &[(f64, f64)] = &[(10.0, 0.0), (6.0, 20.0), (4.0, 20.0), (8.0, 0.0)];
        let mut m = BRepModel::new();
        let s = revolve_tube(&mut m, cone_tube);
        match rim_at(&m, 6.0, 20.0) {
            Some(rim) => {
                let v = fillet_and_judge(&mut m, s, vec![rim], 1.0);
                table.add("revolve cone-tube: fillet cone-walled outer rim r=1.0", &v);
            }
            None => no_rim(&mut table, "revolve cone-tube outer-rim fillet"),
        }
    }

    table.render("BROADEN: sweep + revolve-with-blend");
}

/// Closed CCW `w×h` rectangle in the XY plane (z = 0).
fn rectangle_xy(model: &mut BRepModel, w: f64, h: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(w, 0.0, 0.0);
    let v2 = model.vertices.add(w, h, 0.0);
    let v3 = model.vertices.add(0.0, h, 0.0);
    vec![
        line_edge(model, v0, v1),
        line_edge(model, v1, v2),
        line_edge(model, v2, v3),
        line_edge(model, v3, v0),
    ]
}

// ===========================================================================
// TEST 9 — KNOWN-bug reference probes: #35 intersecting bores.
// ===========================================================================

#[test]
fn known_bug_intersecting_bores_reference() {
    // Reference probe for KNOWN #35 (intersecting cylindrical bores → cyl-cyl
    // saddle, catastrophic open edges). NOT a blend-weld case; included so the
    // report can distinguish KNOWN from NEW. A box with TWO perpendicular
    // intersecting cylindrical bores.
    let mut table = Table::new();

    let mut model = BRepModel::new();
    TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box");
    let solid = last_solid(&model);

    // First bore along Z.
    let cyl_z = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 4.0, 30.0)
        .expect("cyl-z"));
    let r1 = boolean_operation(
        &mut model,
        solid,
        cyl_z,
        BooleanOp::Difference,
        BooleanOptions::default(),
    );
    match r1 {
        Ok(s1) => {
            // Second bore along X, intersecting the first inside the box.
            let cyl_x = sid(TopologyBuilder::new(&mut model)
                .create_cylinder_3d(Point3::new(-15.0, 0.0, 0.0), Vector3::X, 4.0, 30.0)
                .expect("cyl-x"));
            let res = boolean_operation(
                &mut model,
                s1,
                cyl_x,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )
            .map(|_| ())
            .map_err(|e| format!("{e:?}"));
            let target = last_solid(&model);
            let v = judge(&mut model, target, res);
            table.add("box − bore_Z − bore_X (intersecting) [KNOWN #35]", &v);
        }
        Err(e) => {
            let v = Verdict {
                built: false,
                brep_valid: false,
                mesh: None,
                sound: false,
                cert_line: String::new(),
                err: format!("first bore: {e:?}"),
            };
            table.add("box − bore_Z [setup]", &v);
        }
    }

    let fails = table.render("KNOWN-bug reference: #35 intersecting bores");
    eprintln!("NOTE: any FAIL here is the KNOWN #35 cyl-cyl saddle, NOT a blend-weld bug.");
    let _ = fails;
}

// Surface helper to keep imports honest (used by count-based assertions in
// future fix iterations; referenced here so the type import is not dead).
#[allow(dead_code)]
fn surface_kind(model: &BRepModel, fid: FaceId) -> Option<SurfaceType> {
    let f = model.faces.get(fid)?;
    let s = model.surfaces.get(f.surface_id)?;
    Some(s.surface_type())
}
