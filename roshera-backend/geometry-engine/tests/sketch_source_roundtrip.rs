// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A sketch saved with its feature (Varun 2026-11-14: "no way to edit the
//! sketch"). The extrude event kept only the finished outline; to reopen a
//! sketch the history must hold its SOURCE -- every entity with its id and
//! links, and every constraint naming them. Written out, read back, it must
//! be the same sketch: the same entities under the same ids, the same
//! constraints, solving to the same place.

use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintPriority, DimensionalConstraint, EntityRef, GeometricConstraint,
};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::sketch_source::SketchSource;
use geometry_engine::sketch2d::Point2d;

fn drawn() -> Sketch {
    let s = Sketch::new("Profile".into(), SketchAnchor::xy());
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(10.0, 1.0));
    let line = s.add_line(a, b).unwrap();
    s.add_arc_center_angles(Point2d::new(20.0, 0.0), 4.0, 0.0, 1.5)
        .unwrap();
    let circle = s.add_circle(Point2d::new(0.0, 20.0), 3.0).unwrap();
    s.add_rectangle(Point2d::new(30.0, 0.0), Point2d::new(40.0, 6.0))
        .unwrap();
    s.add_ellipse(Point2d::new(50.0, 0.0), 6.0, 3.0, 0.2)
        .unwrap();
    s.add_polyline(
        vec![
            Point2d::new(0.0, 40.0),
            Point2d::new(5.0, 40.0),
            Point2d::new(5.0, 45.0),
        ],
        false,
    )
    .unwrap();
    s.set_construction(&EntityRef::Circle(circle), true)
        .unwrap();
    s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(line)],
        ConstraintPriority::Required,
    ));
    s.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(12.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    s
}

#[test]
fn a_sketch_written_out_and_read_back_is_the_same_sketch() {
    let original = drawn();
    let source = original.to_source();
    // It travels as JSON inside a history event.
    let json = serde_json::to_value(&source).unwrap();
    let back: SketchSource = serde_json::from_value(json).unwrap();
    let restored = Sketch::from_source(&back, SketchAnchor::xy()).unwrap();

    assert_eq!(
        restored.to_source(),
        source,
        "same entities, ids, links, constraints"
    );
    assert_eq!(restored.points().len(), original.points().len());
    assert_eq!(restored.lines().len(), 1);
    assert_eq!(restored.arcs().len(), 1);
    assert_eq!(restored.circles().len(), 1);
    assert_eq!(restored.rectangles().len(), 1);
    assert_eq!(restored.ellipses().len(), 1);
    assert_eq!(restored.polylines().len(), 1);
    assert_eq!(restored.all_constraints().len(), 2);
    // The construction flag and the line's endpoint links survive.
    assert!(restored.circles().iter().all(|c| c.value().is_construction));
    assert!(restored
        .lines()
        .iter()
        .all(|l| l.value().endpoints.is_some()));
}

#[test]
fn a_restored_sketch_solves_to_where_the_original_does() {
    let original = drawn();
    let restored = Sketch::from_source(&original.to_source(), SketchAnchor::xy()).unwrap();
    original.solve_constraints().unwrap();
    restored.solve_constraints().unwrap();
    let pts = |s: &Sketch| {
        let mut v: Vec<(String, f64, f64)> = s
            .points()
            .iter()
            .map(|p| {
                (
                    p.key().to_string(),
                    p.value().position.x,
                    p.value().position.y,
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    let (a, b) = (pts(&original), pts(&restored));
    assert_eq!(a.len(), b.len());
    for (p, q) in a.iter().zip(&b) {
        assert_eq!(p.0, q.0);
        assert!(
            (p.1 - q.1).abs() < 1e-9 && (p.2 - q.2).abs() < 1e-9,
            "{p:?} vs {q:?}"
        );
    }
}

#[test]
fn a_source_naming_a_missing_entity_is_refused() {
    let mut source = drawn().to_source();
    // Drop the entities but keep the constraints that name them.
    source.entities.clear();
    assert!(Sketch::from_source(&source, SketchAnchor::xy()).is_err());
}
