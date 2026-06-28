//! Dogfood (loop S10): the rocket engine as a mate-constrained assembly.
//!
//! This is the critique that started the whole module, distilled to a test. The
//! hand-placed engine had the turbopump "floating ~56mm off the chamber held up
//! by nothing", and `part_distance` (AABB) couldn't even tell. The assembly
//! certificate now CATCHES that float automatically — and a mount mate fixes it.

use assembly_engine::{Assembly, FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh};

/// A cube of side `2*h` — a stand-in body so the interference check has geometry.
fn cube(h: f64) -> Mesh {
    Mesh {
        vertices: vec![
            [-h, -h, -h],
            [h, -h, -h],
            [h, h, -h],
            [-h, h, -h],
            [-h, -h, h],
            [h, -h, h],
            [h, h, h],
            [-h, h, h],
        ],
        triangles: vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [1, 2, 6],
            [1, 6, 5],
            [3, 0, 4],
            [3, 4, 7],
        ],
    }
}

fn part_at(id: u32, name: &str, pos: [f64; 3]) -> Instance {
    let mut instance = Instance::new(InstanceId(id), name.to_string(), cube(2.0));
    instance.translation = pos;
    instance
}

/// Ground instance `b` (placed at `axis_origin`) onto the z-axis through
/// `axis_origin` — the abstraction of a flange/bracket that fixes the part.
fn mount(b: u32, axis_origin: [f64; 3]) -> Mate {
    Mate {
        kind: MateKind::Concentric,
        a: InstanceId(0),
        feature_a: FeatureRef::Axis {
            origin: axis_origin,
            direction: [0.0, 0.0, 1.0],
        },
        b: InstanceId(b),
        feature_b: FeatureRef::Axis {
            origin: [0.0, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
        },
    }
}

/// chamber (ground) + injector (seated on top) + turbopump (seated to the side).
/// Parts in `part_at` are 4-unit cubes, so these seat flush against the chamber.
fn engine() -> Assembly {
    let mut engine = Assembly::new(InstanceId(0)); // chamber is ground, [-2,2]^3
    engine.add_instance(part_at(0, "thrust_chamber", [0.0, 0.0, 0.0]));
    engine.add_instance(part_at(1, "injector", [0.0, 0.0, 4.0])); // on top, touching at z=2
    engine.add_instance(part_at(2, "turbopump", [4.0, 0.0, 0.0])); // to the side, touching at x=2
    engine.add_mate(mount(1, [0.0, 0.0, 0.0])); // injector bolted to the chamber axis
    engine
}

#[test]
fn the_floating_turbopump_is_caught() {
    // The exact defect: the turbopump has no mount, so it floats.
    let engine = engine();
    let cert = engine.certify(&[], 0.01);
    assert!(
        !cert.is_sound(),
        "an engine with a floating turbopump must NOT certify sound"
    );
    assert!(!cert.fully_grounded, "the turbopump float must be flagged");
    assert_eq!(
        engine.grounding_report().floating,
        vec![InstanceId(2)],
        "and the certificate names the turbopump as the floating part"
    );
}

#[test]
fn a_mount_bracket_grounds_it() {
    // Add the bracket that ties the turbopump to the structure — now nothing floats.
    let mut engine = engine();
    engine.add_mate(mount(2, [4.0, 0.0, 0.0])); // turbopump mounted to the chamber
    let cert = engine.certify(&[], 0.01);
    assert!(cert.fully_grounded, "the mount grounds the turbopump");
    assert!(
        cert.no_static_interference,
        "the parts seat flush — contact, not penetration"
    );
    assert!(
        cert.mates_in_contact,
        "every mated part actually touches the structure"
    );
    assert!(
        cert.is_sound(),
        "mounted, seated, solvable → sound: {cert:?}"
    );
}
