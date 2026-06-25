//! FILLET / CHAMFER STRESS HARNESS — a bug HUNT, not a fixer.
//!
//! Surfaces NEW geometry defects the existing per-op harnesses miss by throwing
//! a battery of difficult blend cases at the kernel and judging each against
//! THREE independent oracles, none of which is weakened here:
//!
//!   1. B-Rep oracle  — `primitives::validation::validate_solid_scoped`
//!                       (Standard): mesh-independent topology verdict.
//!   2. Mesh oracle    — `harness::watertight::manifold_report`: the welded
//!                       display mesh must close (boundary_edges == 0), be
//!                       2-manifold (nonmanifold_edges == 0) and be consistently
//!                       wound (oriented == true).
//!   3. Self-cert      — `BRepModel::certify_solid(..).is_sound()`: the kernel's
//!                       own intrinsic certificate (ANDs in self-intersection-
//!                       free + tessellation/mesh-quality, which the first two
//!                       cannot see).
//!
//! A case that BUILDS but fails ANY oracle is a BUG. Each test reports the exact
//! defect (open-edge / non-manifold counts, which certificate dimension is
//! false) and never `panic!`s on a build failure — a failed op is itself a
//! reportable datum, recorded as DID-NOT-BUILD.
//!
//! Run: cargo test -p geometry-engine --test fillet_chamfer_stress -- --nocapture

use geometry_engine::harness::watertight::{manifold_report, ManifoldReport};
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::curve::Curve;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
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
            parts.join(" ")
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
#[derive(Default)]
struct Table {
    rows: Vec<(String, bool, String)>,
}

impl Table {
    fn add(&mut self, case: &str, v: &Verdict) {
        self.rows.push((case.to_string(), v.pass(), v.defect()));
    }

    /// Print the table and return the number of FAILs.
    fn render(&self, title: &str) -> usize {
        eprintln!("\n================ {title} ================");
        eprintln!("{:<48} {:<6} {}", "CASE", "RESULT", "DEFECT");
        eprintln!("{}", "-".repeat(110));
        let mut fails = 0usize;
        for (case, pass, defect) in &self.rows {
            let tag = if *pass { "PASS" } else { "FAIL" };
            if !*pass {
                fails += 1;
            }
            eprintln!("{case:<48} {tag:<6} {defect}");
        }
        eprintln!("{}", "-".repeat(110));
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

/// A `sx × sy × sz` box at the origin; returns its solid id.
fn make_box(model: &mut BRepModel, sx: f64, sy: f64, sz: f64) -> SolidId {
    TopologyBuilder::new(model)
        .create_box_3d(sx, sy, sz)
        .expect("box");
    last_solid(model)
}

/// All edges of a solid's outer shell, de-duplicated.
fn all_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let Some(shell) = model.shells.get(s.outer_shell) else {
        return out;
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
    out
}

/// The STRAIGHT (linear) edges of a solid — the cap rims of a cylinder are
/// circular; its lateral seam is straight. For a box every edge is straight.
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

/// The CIRCULAR (non-linear) edges of a solid — cylinder/cone cap rims.
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

/// Radius of a circular rim edge (its curve downcast to a `Circle`); 0.0 if the
/// edge is not a circle. Used to pick the base (largest-radius) cone rim.
fn rim_radius(model: &BRepModel, eid: EdgeId) -> f64 {
    use geometry_engine::primitives::curve::Circle;
    model
        .edges
        .get(eid)
        .and_then(|e| model.curves.get(e.curve_id))
        .and_then(|c| {
            c.as_any()
                .downcast_ref::<Circle>()
                .map(|circ| circ.radius())
        })
        .unwrap_or(0.0)
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

/// Apply a fillet then judge — sequences the two `&mut model` uses so they do
/// not overlap (the op runs to a `Result`, THEN the oracles run).
fn fillet_and_judge(model: &mut BRepModel, solid: SolidId, edges: Vec<EdgeId>, r: f64) -> Verdict {
    let res = do_fillet(model, solid, edges, r);
    judge(model, solid, res)
}

/// Apply a chamfer then judge (same sequencing as [`fillet_and_judge`]).
fn chamfer_and_judge(model: &mut BRepModel, solid: SolidId, edges: Vec<EdgeId>, d: f64) -> Verdict {
    let res = do_chamfer(model, solid, edges, d);
    judge(model, solid, res)
}

// ===========================================================================
// TEST 1 — Fillet ALL 12 edges of a box at varied radii.
// ===========================================================================

#[test]
fn fillet_all_12_edges_varied_radii() {
    let mut table = Table::new_default();
    // Box 10×10×10; shortest edge = 10. Radii from tiny to near-degenerate
    // (close to half the shortest edge, the geometric ceiling).
    for &r in &[0.05_f64, 0.5, 1.0, 2.5, 4.0, 4.5, 4.9] {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = all_edges(&model, solid);
        let n = edges.len();
        let v = fillet_and_judge(&mut model, solid, edges, r);
        table.add(&format!("fillet 12-edge box10 r={r} (edges={n})"), &v);
    }
    let fails = table.render("FILLET all-12-edges varied radii");
    eprintln!(
        "NOTE: large-r-relative-to-edge (r≥4.0 of a 10-box, where 2r approaches \
         the 10 face → adjacent round-overs collide) is the prime NEW-bug candidate."
    );
    let _ = fails; // hunt: report, do not gate.
}

// ===========================================================================
// TEST 2 — Chamfer ALL 12 edges of a box at varied distances.
// ===========================================================================

#[test]
fn chamfer_all_12_edges_varied_distances() {
    let mut table = Table::new_default();
    for &d in &[0.05_f64, 0.5, 1.0, 2.5, 4.0, 4.5, 4.9] {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = all_edges(&model, solid);
        let n = edges.len();
        let v = chamfer_and_judge(&mut model, solid, edges, d);
        table.add(&format!("chamfer 12-edge box10 d={d} (edges={n})"), &v);
    }
    table.render("CHAMFER all-12-edges varied distances");
}

// ===========================================================================
// TEST 3 — MIXED on one box: fillet some edges + chamfer others (1C2F class).
// ===========================================================================

#[test]
fn mixed_fillet_and_chamfer_one_box() {
    let mut table = Table::new_default();

    // 3a: fillet half the edges, then chamfer a disjoint half (corners where a
    // filleted and a chamfered edge meet = the 1C2F mixed-kind corner stress).
    {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = all_edges(&model, solid);
        let half = edges.len() / 2;
        let fillet_set: Vec<_> = edges.iter().take(half).copied().collect();
        let chamfer_set: Vec<_> = edges.iter().skip(half).copied().collect();
        let r1 = do_fillet(&mut model, solid, fillet_set, 1.0);
        let v1 = if r1.is_ok() {
            chamfer_and_judge(&mut model, solid, chamfer_set, 1.0)
        } else {
            judge(&mut model, solid, r1)
        };
        table.add("box10: fillet 6 edges THEN chamfer other 6 (r=d=1.0)", &v1);
    }

    // 3b: a single corner with one chamfered + two filleted incident edges —
    // the canonical 1C2F corner the chamfer/fillet-corner work (#70/#72) targets.
    {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = all_edges(&model, solid);
        // Take the first three distinct edges (they share corners on a box).
        let f_edges: Vec<_> = edges.iter().take(2).copied().collect();
        let c_edges: Vec<_> = edges.iter().skip(2).take(1).copied().collect();
        let r1 = do_fillet(&mut model, solid, f_edges, 1.0);
        let v = if r1.is_ok() {
            chamfer_and_judge(&mut model, solid, c_edges, 1.0)
        } else {
            judge(&mut model, solid, r1)
        };
        table.add("box10: 1C2F single corner (2 fillet + 1 chamfer)", &v);
    }

    table.render("MIXED fillet+chamfer on one box (1C2F class)");
}

// ===========================================================================
// TEST 4 — Fillet-over-fillet / chamfer crossing a fillet (the #70 class).
// ===========================================================================

#[test]
fn blend_crossing_blend() {
    let mut table = Table::new_default();

    // 4a: fillet one edge, then fillet a PERPENDICULAR edge that shares its
    // corner (fillet-over-fillet at the shared vertex).
    {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = all_edges(&model, solid);
        let e0 = vec![edges[0]];
        let e1 = vec![edges[1]];
        let r1 = do_fillet(&mut model, solid, e0, 1.0);
        let v = if r1.is_ok() {
            fillet_and_judge(&mut model, solid, e1, 1.0)
        } else {
            judge(&mut model, solid, r1)
        };
        table.add("box10: fillet edge0 THEN fillet adjacent edge1 (r=1.0)", &v);
    }

    // 4b: fillet an edge, then chamfer a crossing edge (#70 chamfer-crosses-
    // fillet — KNOWN pinned class).
    {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = all_edges(&model, solid);
        let e0 = vec![edges[0]];
        let e1 = vec![edges[1]];
        let r1 = do_fillet(&mut model, solid, e0, 1.5);
        let v = if r1.is_ok() {
            chamfer_and_judge(&mut model, solid, e1, 1.0)
        } else {
            judge(&mut model, solid, r1)
        };
        table.add("box10: fillet edge0 THEN chamfer adjacent edge1 (#70)", &v);
    }

    let fails = table.render("BLEND-CROSSING-BLEND (#70 class)");
    eprintln!("NOTE: any FAIL here is candidate KNOWN #70 (chamfer-crosses-fillet).");
    let _ = fails;
}

// ===========================================================================
// TEST 5 — Blends on NON-box solids.
// ===========================================================================

#[test]
fn blends_on_non_box_solids() {
    let mut table = Table::new_default();

    // 5a: fillet a cylinder's circular cap rims.
    {
        let mut model = BRepModel::new();
        let solid = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 5.0, 20.0)
            .expect("cyl"));
        let rims = circular_edges(&model, solid);
        let n = rims.len();
        let v = fillet_and_judge(&mut model, solid, rims, 1.0);
        table.add(&format!("cylinder r5 h20: fillet {n} cap rims (r=1.0)"), &v);
    }

    // 5b: chamfer a cylinder's cap rims.
    {
        let mut model = BRepModel::new();
        let solid = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 5.0, 20.0)
            .expect("cyl"));
        let rims = circular_edges(&model, solid);
        let n = rims.len();
        let v = chamfer_and_judge(&mut model, solid, rims, 1.0);
        table.add(
            &format!("cylinder r5 h20: chamfer {n} cap rims (d=1.0)"),
            &v,
        );
    }

    // 5c: fillet a cone's base rim.
    {
        let mut model = BRepModel::new();
        let solid = sid(TopologyBuilder::new(&mut model)
            .create_cone_3d(Point3::ORIGIN, Vector3::Z, 5.0, 2.0, 12.0)
            .expect("cone"));
        let rims = circular_edges(&model, solid);
        let n = rims.len();
        let v = fillet_and_judge(&mut model, solid, rims, 0.8);
        table.add(&format!("cone r5→2 h12: fillet {n} rims (r=0.8)"), &v);
    }

    // 5d: chamfer a cone's base rim.
    {
        let mut model = BRepModel::new();
        let solid = sid(TopologyBuilder::new(&mut model)
            .create_cone_3d(Point3::ORIGIN, Vector3::Z, 5.0, 2.0, 12.0)
            .expect("cone"));
        let rims = circular_edges(&model, solid);
        let n = rims.len();
        let v = chamfer_and_judge(&mut model, solid, rims, 0.8);
        table.add(&format!("cone r5→2 h12: chamfer {n} rims (d=0.8)"), &v);
    }

    // 5c': fillet ONE cone rim only (the base, largest-radius rim) — isolates a
    // multi-rim interaction from a single-rim defect against KNOWN #89 (single
    // Plane–Cone rim fillet documented FIXED).
    {
        let mut model = BRepModel::new();
        let solid = sid(TopologyBuilder::new(&mut model)
            .create_cone_3d(Point3::ORIGIN, Vector3::Z, 5.0, 2.0, 12.0)
            .expect("cone"));
        // Pick the single circular rim of largest radius (the base, r=5).
        let base_rim = circular_edges(&model, solid)
            .into_iter()
            .max_by(|&a, &b| {
                let ra = rim_radius(&model, a);
                let rb = rim_radius(&model, b);
                ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .into_iter()
            .collect::<Vec<_>>();
        let v = fillet_and_judge(&mut model, solid, base_rim, 0.8);
        table.add(
            "cone r5→2 h12: fillet ONLY base rim r5 (r=0.8) [#89 probe]",
            &v,
        );
    }

    // 5e: fillet the seam/weld edges of a box∪box union result.
    {
        let (mut model, solid) = box_union_box();
        // Fillet every straight edge of the union (the seam edges where the two
        // boxes weld are linear too).
        let edges = linear_edges(&model, solid);
        let n = edges.len();
        let v = fillet_and_judge(&mut model, solid, edges, 0.4);
        table.add(&format!("box∪box: fillet {n} linear edges (r=0.4)"), &v);
    }

    // 5f: fillet the through-bore rims of a box-with-a-cylindrical-hole.
    {
        let (mut model, solid) = box_with_cyl_hole();
        let rims = circular_edges(&model, solid);
        let n = rims.len();
        let v = fillet_and_judge(&mut model, solid, rims, 0.8);
        table.add(&format!("box−cyl (bore): fillet {n} bore rims (r=0.8)"), &v);
    }

    // 5g: chamfer the through-bore rims of a box-with-a-cylindrical-hole.
    {
        let (mut model, solid) = box_with_cyl_hole();
        let rims = circular_edges(&model, solid);
        let n = rims.len();
        let v = chamfer_and_judge(&mut model, solid, rims, 0.8);
        table.add(
            &format!("box−cyl (bore): chamfer {n} bore rims (d=0.8)"),
            &v,
        );
    }

    table.render("BLENDS on NON-BOX solids");
}

// ===========================================================================
// TEST 6 — Extreme convergence: 3+ edges meeting at a vertex, all blended.
// ===========================================================================

#[test]
fn extreme_corner_convergence() {
    let mut table = Table::new_default();

    // 6a: the three edges meeting at ONE box corner, all filleted (3-edge
    // convergence — the apex-sphere corner synthesis).
    {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = corner_edges(&model, solid);
        let n = edges.len();
        let v = fillet_and_judge(&mut model, solid, edges, 1.0);
        table.add(
            &format!("box10: fillet {n} edges at ONE corner (r=1.0)"),
            &v,
        );
    }

    // 6b: same three converging edges, all chamfered.
    {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = corner_edges(&model, solid);
        let n = edges.len();
        let v = chamfer_and_judge(&mut model, solid, edges, 1.0);
        table.add(
            &format!("box10: chamfer {n} edges at ONE corner (d=1.0)"),
            &v,
        );
    }

    // 6c: ALL 12 edges chamfered — every one of the 8 corners is a 3-edge
    // converging chamfer corner simultaneously (max convergence stress).
    {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model, 10.0, 10.0, 10.0);
        let edges = all_edges(&model, solid);
        let n = edges.len();
        let v = chamfer_and_judge(&mut model, solid, edges, 2.0);
        table.add(
            &format!("box10: chamfer ALL {n} edges d=2.0 (8 corners)"),
            &v,
        );
    }

    table.render("EXTREME corner convergence (3+ edges/vertex)");
}

// ---------------------------------------------------------------------------
// Composite builders for TEST 5 / TEST 6.
// ---------------------------------------------------------------------------

/// Two overlapping boxes welded by Union: a 10×10×10 box ∪ a 6×6×6 box offset
/// so it straddles the +Z face of the first (a boss-on-block — exposed seam
/// edges where the smaller box protrudes through the bigger one's top).
fn box_union_box() -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 10.0, 10.0, 10.0);
    let b = make_box(&mut model, 6.0, 6.0, 10.0);
    // Move B up by 8 so its lower half overlaps A's top and its upper half
    // protrudes. `translate` moves vertices + surfaces + curves coherently.
    translate(
        &mut model,
        vec![b],
        Vector3::Z,
        8.0,
        TransformOptions::default(),
    )
    .expect("translate box B");
    let res = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("box ∪ box");
    (model, res)
}

/// Box 20×20×20 minus a coaxial cylinder fully through it = a clean through-bore.
fn box_with_cyl_hole() -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    let b = make_box(&mut model, 20.0, 20.0, 20.0);
    let cyl = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 5.0, 30.0)
        .expect("cyl"));
    let res = boolean_operation(
        &mut model,
        b,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box − cyl");
    (model, res)
}

/// The (up to three) edges incident to one corner of a box — the corner with
/// the most negative coordinates. Returns the edges whose endpoints touch that
/// vertex.
fn corner_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let edges = all_edges(model, solid);
    // Find the vertex with minimum (x+y+z) — a unique box corner.
    let mut best_v = None;
    let mut best_key = f64::INFINITY;
    let mut vert_of = std::collections::HashSet::new();
    for &eid in &edges {
        if let Some(e) = model.edges.get(eid) {
            vert_of.insert(e.start_vertex);
            vert_of.insert(e.end_vertex);
        }
    }
    for vid in vert_of {
        if let Some(v) = model.vertices.get(vid) {
            let k = v.position[0] + v.position[1] + v.position[2];
            if k < best_key {
                best_key = k;
                best_v = Some(vid);
            }
        }
    }
    let Some(corner) = best_v else {
        return Vec::new();
    };
    edges
        .into_iter()
        .filter(|&eid| {
            model
                .edges
                .get(eid)
                .map(|e| e.start_vertex == corner || e.end_vertex == corner)
                .unwrap_or(false)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Table ctor (avoids a Default import clash with the harness ones).
// ---------------------------------------------------------------------------
impl Table {
    fn new_default() -> Self {
        Table { rows: Vec::new() }
    }
}
