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

/// A square TUBE — outer half-size `o`, a square through-hole half-size `i`,
/// z half-height `zh`. THIS is the faithful bore topology (a closed through-hole,
/// unlike the open dumbbell gap): the convex hull of its 16 corners is the solid
/// outer box, and convex decomposition of a ring still yields pieces whose hulls
/// fill the hole — so a neighbour seated in the hole false-positives against the
/// pieces. The exact TriMesh distance (this fix) sees the hole and clears it.
fn square_tube(o: f64, i: f64, zh: f64) -> Mesh {
    let vertices = vec![
        [-o, -o, -zh],
        [o, -o, -zh],
        [o, o, -zh],
        [-o, o, -zh], // 0-3 outer bottom
        [-o, -o, zh],
        [o, -o, zh],
        [o, o, zh],
        [-o, o, zh], // 4-7 outer top
        [-i, -i, -zh],
        [i, -i, -zh],
        [i, i, -zh],
        [-i, i, -zh], // 8-11 inner bottom
        [-i, -i, zh],
        [i, -i, zh],
        [i, i, zh],
        [-i, i, zh], // 12-15 inner top
    ];
    let triangles = vec![
        // outer walls
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
        // inner walls (the hole surface)
        [8, 12, 13],
        [8, 13, 9],
        [9, 13, 14],
        [9, 14, 10],
        [10, 14, 15],
        [10, 15, 11],
        [11, 15, 12],
        [11, 12, 8],
        // bottom annulus (outer 0-3 ↔ inner 8-11)
        [0, 8, 9],
        [0, 9, 1],
        [1, 9, 10],
        [1, 10, 2],
        [2, 10, 11],
        [2, 11, 3],
        [3, 11, 8],
        [3, 8, 0],
        // top annulus (outer 4-7 ↔ inner 12-15)
        [4, 5, 13],
        [4, 13, 12],
        [5, 6, 14],
        [5, 14, 13],
        [6, 7, 15],
        [6, 15, 14],
        [7, 4, 12],
        [7, 12, 15],
    ];
    Mesh {
        vertices,
        triangles,
    }
}

/// F6 (the original flange-bore case, faithfully): a peg seated CLEAR inside a
/// square through-hole must NOT interfere. Tube hole half-size 1.5; peg half-size
/// 0.5 at the centre → 1.0 clear of every wall. The convex pieces of the tube fill
/// the hole (VHACD cannot empty a closed through-hole); only the exact TriMesh
/// distance sees the 1.0 gap. This is what the dumbbell (open gap) missed.
#[test]
fn peg_in_a_through_hole_is_clear_not_interfering() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(Instance::new(
        InstanceId(0),
        "tube",
        square_tube(3.0, 1.5, 3.0),
    ));
    assembly.add_instance(peg_at(1, 0.5, 0.0));
    let report = assembly.interference_report();
    assert!(
        report.no_static_interference(),
        "F6: a peg seated clear inside a through-hole must not interfere; got {:?}",
        report.interfering
    );
}

/// TEETH — a peg WIDER than the hole (half-size 1.8 > hole 1.5) bites into the
/// tube walls: real interference, must still be flagged after the TriMesh gate.
#[test]
fn peg_wider_than_the_hole_is_real_interference() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(Instance::new(
        InstanceId(0),
        "tube",
        square_tube(3.0, 1.5, 3.0),
    ));
    assembly.add_instance(peg_at(1, 1.8, 0.0));
    let report = assembly.interference_report();
    assert!(
        !report.no_static_interference(),
        "a peg wider than the hole overlaps the tube walls and must interfere: {:?}",
        report.interfering
    );
}

/// F6 PIN (ignored until convex decomposition lands). Dumbbell lobes span
/// [−2.8,−1.2] and [1.2,2.8]; the gap is (−1.2, 1.2). A peg of half-size 0.5 at
/// x=0 spans [−0.5,0.5] — 0.7 clear of each lobe. Correct: no interference.
#[test]
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

/// TEETH — the "pipe OD > bore" analogue on a concave part. The dumbbell gap is
/// (−1.2, 1.2), i.e. 2.4 wide. A peg of half-size 1.4 at x=0 spans [−1.4, 1.4],
/// so it bites 0.2mm into BOTH lobes — a genuine press-past-fit. Decomposing the
/// dumbbell into two lobes must still flag this: the fix must not turn every
/// concave interference clear.
#[test]
fn oversized_peg_in_gap_is_real_interference() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(Instance::new(InstanceId(0), "dumbbell", dumbbell(0.8, 2.0)));
    assembly.add_instance(peg_at(1, 1.4, 0.0));
    let report = assembly.interference_report();
    assert!(
        !report.no_static_interference(),
        "a peg wider than the gap overlaps both lobes and must interfere: {:?}",
        report.interfering
    );
}

/// TEETH — flush FIT inside the concavity. A peg of half-size 1.2 at x=0 spans
/// [−1.2, 1.2], seating face-to-face against both lobe inner walls (x=∓1.2).
/// That is mating CONTACT, not interference — the decomposition must preserve
/// the flush-vs-overlap distinction inside a concavity, not just in open space.
#[test]
fn peg_flush_in_gap_is_contact_not_interference() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(Instance::new(InstanceId(0), "dumbbell", dumbbell(0.8, 2.0)));
    assembly.add_instance(peg_at(1, 1.2, 0.0));
    let report = assembly.interference_report();
    assert!(
        report.no_static_interference(),
        "a peg seating flush against the gap walls is contact, not interference: {:?}",
        report.interfering
    );
}
