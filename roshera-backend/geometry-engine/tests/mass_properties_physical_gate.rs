// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! MUST-FAIL GATE: the mass-properties pipeline must never emit a NON-PHYSICAL
//! result (the kernel "lying" with numbers a real rigid body cannot have).
//!
//! Surfaced by the gap-finder (`tests/gap_finder_fuzz.rs`): at `GAP_FINDER_SEED=10`
//! the seeded op-chain (a sphere ∪/∖ cylinder curved boolean) builds a
//! brep-invalid, non-watertight HUSK. The mesh-based mass-properties integration
//! (`BRepModel::mesh_based_mass_properties`, Tonon 2004) integrates the divergence
//! theorem over that leaky/inconsistently-wound mesh and emitted a symmetric but
//! INDEFINITE inertia tensor — a large NEGATIVE principal moment (~ -1.74e8). A
//! physical inertia tensor is positive-semidefinite, so a negative principal
//! moment is physically impossible; emitting it is a form of lying.
//!
//! The fix (in `primitives/topology_builder.rs::mesh_based_mass_properties`)
//! validates the computed result against the physical contract BEFORE returning
//! it: positive-finite volume + area, symmetric tensor, finite non-negative
//! principal moments, and the rigid-body triangle inequality. If the result is
//! non-physical the pipeline REFUSES — it returns `None` and logs the reason via
//! `tracing::warn!` — rather than handing a caller bogus numbers (and it does NOT
//! clamp a negative moment to zero, which would be a different lie). The existing
//! public `Option<MassPropertiesReport>` API is preserved: clean solids still get
//! a full report.
//!
//! Two halves, BOTH required (a guard that also rejects clean solids is useless):
//!   (a) the seed-10 husk → mass-properties REFUSES (returns `None`); no negative
//!       principal moment escapes through the public report; and
//!   (b) NO-REGRESSION: clean box / cylinder / sphere → full valid report,
//!       values unchanged (volume + principal moments within tolerance of the
//!       documented closed-form).

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{rotate, translate};
use geometry_engine::operations::TransformOptions;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ── SplitMix64: the SAME deterministic generator the gap-finder uses, so a seed
//    reproduces the EXACT op-chain (and therefore the exact husk) here. ─────────

struct SplitMix64(u64);
impl SplitMix64 {
    fn new(seed: u64) -> Self {
        SplitMix64(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo)
    }
    fn rangef(&mut self, lo: f64, hi: f64) -> f64 {
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + u * (hi - lo)
    }
}

#[derive(Clone, Copy)]
enum Prim {
    Box,
    Cylinder,
    Sphere,
    Cone,
}

fn make_primitive(m: &mut BRepModel, rng: &mut SplitMix64, kind: Prim) -> Option<SolidId> {
    let id = match kind {
        Prim::Box => TopologyBuilder::new(m).create_box_3d(
            rng.rangef(6.0, 16.0),
            rng.rangef(6.0, 16.0),
            rng.rangef(6.0, 16.0),
        ),
        Prim::Cylinder => TopologyBuilder::new(m).create_cylinder_3d(
            Point3::ZERO,
            Vector3::Z,
            rng.rangef(3.0, 8.0),
            rng.rangef(8.0, 16.0),
        ),
        Prim::Sphere => {
            TopologyBuilder::new(m).create_sphere_3d(Vector3::ZERO, rng.rangef(4.0, 9.0))
        }
        Prim::Cone => TopologyBuilder::new(m).create_cone_3d(
            Point3::ZERO,
            Vector3::Z,
            rng.rangef(5.0, 9.0),
            rng.rangef(0.0, 3.0),
            rng.rangef(8.0, 14.0),
        ),
    };
    match id {
        Ok(GeometryId::Solid(s)) => Some(s),
        _ => None,
    }
}

fn first_straight_edge(m: &BRepModel, cur: SolidId) -> Option<EdgeId> {
    let s = m.solids.get(cur)?;
    let shell = m.shells.get(s.outer_shell)?;
    for &fid in &shell.faces {
        let Some(face) = m.faces.get(fid) else {
            continue;
        };
        let Some(lp) = m.loops.get(face.outer_loop) else {
            continue;
        };
        for &eid in &lp.edges {
            let is_line = m
                .edges
                .get(eid)
                .and_then(|e| m.curves.get(e.curve_id))
                .map(|c| c.type_name() == "Line")
                .unwrap_or(false);
            if is_line {
                return Some(eid);
            }
        }
    }
    None
}

fn boolean_step(m: &mut BRepModel, rng: &mut SplitMix64, cur: SolidId) -> Option<SolidId> {
    let op = rng.range(0, 3);
    let other = if rng.range(0, 2) == 0 {
        let r = rng.rangef(2.0, 4.0);
        let off = rng.rangef(-3.0, 3.0);
        match TopologyBuilder::new(m).create_cylinder_3d(
            Point3::new(off, off * 0.5, -14.0),
            Vector3::Z,
            r,
            34.0,
        ) {
            Ok(GeometryId::Solid(s)) => s,
            _ => return None,
        }
    } else {
        let other = make_primitive(m, rng, Prim::Box)?;
        let dx = rng.rangef(2.0, 6.0);
        if translate(m, vec![other], Vector3::X, dx, TransformOptions::default()).is_err() {
            return None;
        }
        other
    };
    let kind = match op {
        0 => BooleanOp::Union,
        1 => BooleanOp::Difference,
        _ => BooleanOp::Intersection,
    };
    boolean_operation(m, cur, other, kind, BooleanOptions::default()).ok()
}

fn apply_random_op(m: &mut BRepModel, rng: &mut SplitMix64, cur: SolidId) -> Option<SolidId> {
    let code = rng.range(0, 6);
    match code {
        0 => {
            let d = rng.rangef(1.0, 5.0);
            translate(m, vec![cur], Vector3::X, d, TransformOptions::default())
                .ok()
                .map(|_| cur)
        }
        1 => {
            let axis = match rng.range(0, 3) {
                0 => Vector3::X,
                1 => Vector3::Y,
                _ => Vector3::Z,
            };
            let angle = rng.rangef(0.1, std::f64::consts::FRAC_PI_2);
            rotate(
                m,
                vec![cur],
                Point3::ZERO,
                axis,
                angle,
                TransformOptions::default(),
            )
            .ok()
            .map(|_| cur)
        }
        2 => {
            let e = first_straight_edge(m, cur)?;
            let r = rng.rangef(0.2, 0.8);
            fillet_edges(
                m,
                cur,
                vec![e],
                FilletOptions {
                    fillet_type: FilletType::Constant(r),
                    radius: r,
                    ..Default::default()
                },
            )
            .ok()
            .map(|_| cur)
        }
        3 => {
            let e = first_straight_edge(m, cur)?;
            let d = rng.rangef(0.2, 0.7);
            chamfer_edges(
                m,
                cur,
                vec![e],
                ChamferOptions {
                    chamfer_type: ChamferType::EqualDistance(d),
                    distance1: d,
                    distance2: d,
                    symmetric: true,
                    ..Default::default()
                },
            )
            .ok()
            .map(|_| cur)
        }
        _ => boolean_step(m, rng, cur),
    }
}

/// Re-run the gap-finder's seeded chain (matching `gap_finder_fuzz::audit_seed`'s
/// generator exactly) and return EVERY solid the chain produced along the way.
/// The husk is one of these intermediates, so the gate scans them all rather
/// than relying on which one happens to be the final `cur`.
fn run_chain_collecting(seed: u64) -> Option<(BRepModel, Vec<SolidId>)> {
    let mut rng = SplitMix64::new(seed);
    let mut m = BRepModel::new();
    let base = match rng.range(0, 4) {
        0 => Prim::Box,
        1 => Prim::Cylinder,
        2 => Prim::Sphere,
        _ => Prim::Cone,
    };
    let mut cur = make_primitive(&mut m, &mut rng, base)?;
    let mut seen = vec![cur];
    let len = rng.range(3, 13);
    let mut consecutive_skips = 0;
    let mut applied = 0;
    while applied < len {
        match apply_random_op(&mut m, &mut rng, cur) {
            Some(ns) => {
                cur = ns;
                if !seen.contains(&cur) {
                    seen.push(cur);
                }
                applied += 1;
                consecutive_skips = 0;
            }
            None => {
                consecutive_skips += 1;
                applied += 1;
                if consecutive_skips > 6 {
                    break;
                }
            }
        }
    }
    Some((m, seen))
}

// ─────────────────────────────── the gate (a) ───────────────────────────────

/// (a) REFUSAL: the seed-10 chain builds a malformed husk whose mesh-based
/// integration previously emitted a negative principal moment (~ -1.74e8). The
/// pipeline must now REFUSE (`mass_properties_for` → `None`) on that husk so the
/// non-physical numbers never reach a caller. We scan every solid the chain
/// produced and assert that NONE of them returns a report carrying a physically
/// impossible (negative / non-finite) principal moment.
#[test]
fn seed10_husk_mass_properties_refuses_nonphysical() {
    let (mut m, solids) = run_chain_collecting(10).expect("seed-10 chain builds a base primitive");

    let mut saw_husk_refusal = false;
    for &sid in &solids {
        match m.mass_properties_for(sid) {
            None => {
                // Refusal is acceptable for a malformed solid. Record that at
                // least one solid in the chain was refused (the husk).
                saw_husk_refusal = true;
            }
            Some(mp) => {
                // A RETURNED report must be physical: no negative / non-finite
                // principal moment may escape. Scale the negativity tolerance to
                // the tensor magnitude (a tiny eigensolver epsilon on a
                // near-degenerate axis is not a lie).
                let scale = mp
                    .inertia_tensor
                    .iter()
                    .flat_map(|r| r.iter())
                    .fold(0.0f64, |acc, &x| acc.max(x.abs()))
                    .max(1.0);
                let neg_tol = -1e-6 * scale;
                for (k, &pm) in mp.principal_moments.iter().enumerate() {
                    assert!(
                        pm.is_finite() && pm >= neg_tol,
                        "solid {sid}: NON-PHYSICAL principal moment {k} escaped the pipeline: \
                         {pm} (tensor scale {scale:.3e}); the guard must REFUSE this husk, \
                         not return it"
                    );
                    assert!(
                        mp.volume.is_finite() && mp.volume > 0.0,
                        "solid {sid}: non-physical volume {} escaped",
                        mp.volume
                    );
                }
            }
        }
    }

    // The whole point of seed 10 is that it DOES build a husk the guard must
    // refuse. If nothing was refused, either the chain no longer reaches the
    // malformed regime (the repro drifted) or the guard is a silent no-op — both
    // make this gate vacuous, so fail loudly.
    assert!(
        saw_husk_refusal,
        "seed-10 chain produced no refusal — the malformed-husk repro drifted or the \
         physical-validity guard never fires; gate would be vacuous"
    );
}

// ─────────────────────────────── the gate (b) ───────────────────────────────
// NO-REGRESSION: clean primitives still get a full, correct report. A guard that
// rejects clean solids is useless. Closed-form references (centered → COM frame):
//   box w×h×d:    I/m = diag(h²+d², w²+d², w²+h²) / 12
//   sphere r:     I/m = 2 r² / 5
//   cylinder r,h: I/m = diag((3r²+h²)/12, (3r²+h²)/12, r²/2)  (axis = Z)

fn assert_principal_moments_about_com(
    label: &str,
    m: &mut BRepModel,
    id: SolidId,
    expected_diag_over_m_sorted: [f64; 3],
    rel_tol: f64,
) {
    let mp = m
        .mass_properties_for(id)
        .unwrap_or_else(|| panic!("[{label}] clean solid must return full mass properties"));
    assert!(
        mp.volume.is_finite() && mp.volume > 0.0,
        "[{label}] volume {}",
        mp.volume
    );
    assert!(mp.mass.is_finite() && mp.mass > 0.0, "[{label}] mass");
    // Compare the principal moments as a SORTED set against the sorted
    // closed-form I (= m · I/m): the report's principal_moments ordering is an
    // eigensolver detail, but the multiset of eigenvalues is invariant.
    let mut want = expected_diag_over_m_sorted;
    want.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut got = [
        mp.principal_moments[0] / mp.mass,
        mp.principal_moments[1] / mp.mass,
        mp.principal_moments[2] / mp.mass,
    ];
    got.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    for k in 0..3 {
        let rel = (got[k] - want[k]).abs() / want[k].max(1e-12);
        assert!(
            rel <= rel_tol,
            "[{label}] principal moment {k}/m = {:.6} vs analytic {:.6} (rel {rel:.4})",
            got[k],
            want[k]
        );
    }
}

#[test]
fn clean_box_full_mass_properties_unchanged() {
    let mut m = BRepModel::new();
    let id = match TopologyBuilder::new(&mut m).create_box_3d(2.0, 1.0, 0.5) {
        Ok(GeometryId::Solid(s)) => s,
        o => panic!("box build: {o:?}"),
    };
    // I/m = ((1+0.25), (4+0.25), (4+1)) / 12.
    assert_principal_moments_about_com(
        "box 2x1x0.5",
        &mut m,
        id,
        [1.25 / 12.0, 4.25 / 12.0, 5.0 / 12.0],
        0.03,
    );
}

#[test]
fn clean_cylinder_full_mass_properties_unchanged() {
    let mut m = BRepModel::new();
    let id = match TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::new(0.0, 0.0, -1.0),
        Vector3::Z,
        0.5,
        2.0,
    ) {
        Ok(GeometryId::Solid(s)) => s,
        o => panic!("cylinder build: {o:?}"),
    };
    // Ixx/m = Iyy/m = (3r²+h²)/12 = 4.75/12, Izz/m = r²/2 = 0.125.
    assert_principal_moments_about_com(
        "cylinder r=0.5 h=2",
        &mut m,
        id,
        [4.75 / 12.0, 4.75 / 12.0, 0.125],
        0.05,
    );
}

#[test]
fn clean_sphere_full_mass_properties_unchanged() {
    let mut m = BRepModel::new();
    let id = match TopologyBuilder::new(&mut m).create_sphere_3d(Point3::ORIGIN, 1.0) {
        Ok(GeometryId::Solid(s)) => s,
        o => panic!("sphere build: {o:?}"),
    };
    // I/m = 2/5 on every axis (isotropic).
    assert_principal_moments_about_com("sphere r=1", &mut m, id, [0.4; 3], 0.05);
}
