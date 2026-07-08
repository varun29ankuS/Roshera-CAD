//! F6 (dogfood-findings-flange-2026-07-08) — static-interference false-positive
//! on a part seated inside a CONCAVE neighbour.
//!
//! `Assembly::interference_report` tests each instance's CONVEX HULL
//! (`interference.rs::instance_convex`), documented there as "a conservative
//! over-approximation for a concave part until convex decomposition lands". A
//! bored flange (or any concave part) has a convex hull that FILLS the
//! concavity, so anything seated inside it — a pipe in a bore, a peg in a
//! pocket — reads as interfering even with real clearance. This is the live
//! flanged-pipe-spool finding (flange bore Ø30, pipe OD Ø28 = 1mm clearance →
//! `no_static_interference: false`).
//!
//! Minimal, kernel-free reproduction: a "dumbbell" (two cube lobes as ONE
//! mesh, a genuine concavity in the gap between them) with a small peg seated
//! in the empty gap, clear of both lobes. Correct answer: NO interference.
//! At HEAD the convex hull of the dumbbell fills the gap, so the peg
//! false-positives — hence `#[ignore]` pending the convex-decomposition
//! (VHACD) fix. The teeth test (real overlap) is NOT ignored and must pass.

use assembly_engine::types::{Assembly, Instance, InstanceId, Mesh};

/// Axis-aligned cube, half-size `h`, centred at (`cx`, 0, 0). 8 corners; the
/// convex-hull interference path uses only these vertices.
fn cube_verts(cx: f64, h: f64) -> Vec<[f64; 3]> {
    vec![
        [cx - h, -h, -h],
        [cx + h, -h, -h],
        [cx + h, h, -h],
        [cx - h, h, -h],
        [cx - h, -h, h],
        [cx + h, -h, h],
        [cx + h, h, h],
        [cx - h, h, h],
    ]
}

/// Standard 12 outward triangles for the 8-corner cube at vertex `offset`.
fn cube_tris(offset: u32) -> Vec<[u32; 3]> {
    let o = offset;
    vec![
        [o, o + 2, o + 1],
        [o, o + 3, o + 2],
        [o + 4, o + 5, o + 6],
        [o + 4, o + 6, o + 7],
        [o, o + 1, o + 5],
        [o, o + 5, o + 4],
        [o + 2, o + 3, o + 7],
        [o + 2, o + 7, o + 6],
        [o + 1, o + 2, o + 6],
        [o + 1, o + 6, o + 5],
        [o + 3, o, o + 4],
        [o + 3, o + 4, o + 7],
    ]
}

fn single_cube(h: f64) -> Mesh {
    Mesh {
        vertices: cube_verts(0.0, h),
        triangles: cube_tris(0),
    }
}

/// Two cube lobes at x = ±`d` (half-size `h`) as ONE mesh — a concave part.
/// The empty gap is x ∈ (−(d−h), +(d−h)); the convex hull fills it.
fn dumbbell(h: f64, d: f64) -> Mesh {
    let mut vertices = cube_verts(-d, h);
    vertices.extend(cube_verts(d, h));
    let mut triangles = cube_tris(0);
    triangles.extend(cube_tris(8));
    Mesh {
        vertices,
        triangles,
    }
}

fn peg_at(id: u32, h: f64, x: f64) -> Instance {
    let mut inst = Instance::new(InstanceId(id), format!("peg_{id}"), single_cube(h));
    inst.translation = [x, 0.0, 0.0];
    inst
}

/// F6 PIN (ignored until convex decomposition lands). Dumbbell lobes span
/// [−2.8,−1.2] and [1.2,2.8]; the gap is (−1.2, 1.2). A peg of half-size 0.5 at
/// x=0 spans [−0.5,0.5] — 0.7 clear of each lobe. Correct: no interference.
#[test]
#[ignore = "F6: interference uses the convex hull (interference.rs:75); it fills \
            the dumbbell gap, so a peg seated clear IN the gap false-positives. \
            Fix = VHACD convex decomposition of concave parts. Un-ignore then."]
fn peg_in_concavity_is_clear_not_interfering() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance({
        let mut d = Instance::new(InstanceId(0), "dumbbell", dumbbell(0.8, 2.0));
        d.translation = [0.0, 0.0, 0.0];
        d
    });
    assembly.add_instance(peg_at(1, 0.5, 0.0));
    let report = assembly.interference_report();
    assert!(
        report.no_static_interference(),
        "F6: peg seated clear in the concave gap must not interfere; got {:?}",
        report.interfering
    );
}

/// TEETH (must always pass): a peg genuinely OVERLAPPING a lobe is real
/// interference — the fix must not blind the detector. Peg half-size 0.5 at
/// x=−2 spans [−2.5,−1.5], overlapping the lobe [−2.8,−1.2].
#[test]
fn peg_overlapping_a_lobe_is_real_interference() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(Instance::new(InstanceId(0), "dumbbell", dumbbell(0.8, 2.0)));
    assembly.add_instance(peg_at(1, 0.5, -2.0));
    let report = assembly.interference_report();
    assert!(
        !report.no_static_interference(),
        "real overlap of the peg with a lobe must be flagged as interference",
    );
}
