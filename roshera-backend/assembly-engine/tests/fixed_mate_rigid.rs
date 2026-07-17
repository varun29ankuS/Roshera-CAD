//! RED → GREEN for the §2.2 "Fixed is not rigid" lie (kinematic-assembly
//! campaign, Slice 1, defect b).
//!
//! `MateKind::Fixed` documents itself as "fully rigid — a bolt pattern"
//! (`types.rs`), but the residual routed `Coincident | Fixed` to the same
//! face-flush function: rank 3, leaving 2 in-plane translations + spin about
//! the normal FREE while the DOF report presented the stack as designed.
//! These tests pin the honest contract: one Fixed mate consumes ALL SIX
//! degrees of freedom.
//!
//! Pre-fix signatures (captured 2026-07-17, HEAD 45d8ffee):
//!   fixed_mate_consumes_all_six_dof:      rank = 3, dof = 3, Mobile
//!   fixed_mate_seats_in_plane_offset:     x stays 3.0 (in-plane slide never corrected)
//!   fixed_mate_locks_spin_about_normal:   spin about z survives the solve

use assembly_engine::{Assembly, FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh, Mobility};

fn part(id: u32) -> Instance {
    Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
}

/// A Fixed (bolt-pattern) mate between the ground's top face and the part's
/// bottom face, both declared at their local origins with ±z normals.
fn fixed_mate() -> Mate {
    Mate {
        kind: MateKind::Fixed,
        a: InstanceId(0),
        feature_a: FeatureRef::Face {
            point: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        },
        b: InstanceId(1),
        feature_b: FeatureRef::Face {
            point: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, -1.0], // flush = antiparallel normals
        },
    }
}

#[test]
fn fixed_mate_consumes_all_six_dof() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0)); // ground
    assembly.add_instance(part(1));
    assembly.add_mate(fixed_mate());

    let report = assembly.dof_analysis();
    assert_eq!(report.config_dim, 6);
    assert_eq!(
        report.rank, 6,
        "a bolt pattern locks every DOF; a rank of 3 is the face-flush lie"
    );
    assert_eq!(report.dof, 0, "Fixed must leave NO free motion");
    assert_eq!(report.mobility, Mobility::FullyConstrained);
}

#[test]
fn fixed_mate_seats_in_plane_offset() {
    // The part starts slid 3mm in x and 4mm in y INSIDE the mating plane —
    // exactly the motion the face-flush residual cannot see. A true Fixed
    // mate must pull it back onto the declared bolt position.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut p1 = part(1);
    p1.translation = [3.0, 4.0, 0.0];
    assembly.add_instance(p1);
    assembly.add_mate(fixed_mate());

    let report = assembly.solve();
    assert!(
        report.converged,
        "a lone Fixed mate is satisfiable: {report:?}"
    );
    let t = assembly
        .instance(InstanceId(1))
        .map(|i| i.translation)
        .unwrap_or([f64::NAN; 3]);
    assert!(
        t[0].abs() < 1e-6 && t[1].abs() < 1e-6 && t[2].abs() < 1e-6,
        "in-plane offset must be corrected by a rigid mate, got {t:?}"
    );
}

#[test]
fn fixed_mate_locks_spin_about_normal() {
    // The part starts spun 30° about the mating normal — the third freedom the
    // face-flush residual leaves silent. Fixed must remove the spin.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut p1 = part(1);
    let half = (30.0_f64).to_radians() / 2.0;
    p1.rotation = [0.0, 0.0, half.sin(), half.cos()];
    assembly.add_instance(p1);
    assembly.add_mate(fixed_mate());

    let report = assembly.solve();
    assert!(
        report.converged,
        "satisfiable from a spun start: {report:?}"
    );
    let r = assembly
        .instance(InstanceId(1))
        .map(|i| i.rotation)
        .unwrap_or([f64::NAN; 4]);
    // Identity up to quaternion double cover.
    let dot = r[3].abs();
    assert!(
        dot > 1.0 - 1e-9,
        "spin about the normal must be locked, got rotation {r:?}"
    );
}

#[test]
fn coincident_still_leaves_three_dof() {
    // Regression guard for the fix: Coincident (plain face-flush) must KEEP
    // its rank-3 semantics — only Fixed gains the full rigid lock.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    let mut m = fixed_mate();
    m.kind = MateKind::Coincident;
    assembly.add_mate(m);

    let report = assembly.dof_analysis();
    assert_eq!(report.rank, 3, "coincident = face-flush, rank 3");
    assert_eq!(report.dof, 3, "slide x/y + spin remain by design");
}
