//! Shared programmatic sketch generator for SKETCH-DCM #45 Slice 2.
//!
//! Builds a realistic dimensioned plate — rectangular outline + ordinate-
//! dimensioned bolt-hole pattern + a horizontal reference dimension chain —
//! at parameterized scale. Used by both the Slice-2 scale-floor /
//! decomposition test binary (`tests/sketch_dcm_slice2_scale_decompose.rs`)
//! and the `sketch_solver` criterion bench (`benches/sketch_solver.rs`,
//! via `#[path]`).
//!
//! Structure (deliberate, and true to how plate drawings are dimensioned):
//!
//! - **Outline** — 4 corner points + 4 shared-endpoint lines,
//!   Horizontal/Vertical on each side, width/height `Distance` dims,
//!   X/Y anchor on the origin corner: 8 constraints, 8 free DOFs
//!   (fully constrained). One connected component.
//! - **Bolt holes** — each hole is a shared-center circle whose center
//!   point is ordinate-dimensioned from the sketch origin
//!   (`XCoordinate` + `YCoordinate`) plus a `Radius` dim: 3 constraints,
//!   3 free DOFs per hole. Each hole is its own connected component —
//!   exactly the "plate outline + independent hole circles" shape the
//!   campaign spec (§3.1 pipeline step 2) names as the common agent
//!   workflow.
//! - **Dimension chain** — a row of reference points, first point
//!   anchored (X+Y), each subsequent point pinned by
//!   `Distance`-to-previous + `YCoordinate`: 2·n constraints, 2·n free
//!   DOFs. One connected component.
//!
//! Every feature is generated FULLY CONSTRAINED and every initial
//! position is deterministically jittered off its dimensioned target,
//! so a solve has genuine Newton work to do and the expected verdict is
//! `Converged` / `FullyConstrained` at exactly-known coordinates.

// Test-support module: failing loudly at the fixture site is the desired
// failure mode; the workspace deny lints target production code. Some
// helpers are used only by one of the two consumers (test binary vs
// bench), hence dead_code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(dead_code)]

use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintPriority, DimensionalConstraint, EntityRef, GeometricConstraint,
};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::{Circle2dId, Point2d, Point2dId};

/// Plate dimensions shared by every generated size.
pub const PLATE_W: f64 = 200.0;
pub const PLATE_H: f64 = 160.0;
/// Bolt-hole radius (all holes identical, like a real fastener pattern).
pub const HOLE_R: f64 = 4.0;
/// Chain pitch and rail height.
pub const CHAIN_PITCH: f64 = 6.0;
pub const CHAIN_Y: f64 = -40.0;
pub const CHAIN_X0: f64 = 10.0;

/// Scale knob for the generated plate.
#[derive(Debug, Clone, Copy)]
pub struct PlateSpec {
    /// Number of ordinate-dimensioned bolt holes (3 constraints each).
    pub holes: usize,
    /// Number of points in the reference dimension chain
    /// (2 constraints per point, including the anchored first point).
    pub chain_points: usize,
}

impl PlateSpec {
    /// 8 + 2·2 + 3·6 = 30 constraints.
    pub const SMALL: PlateSpec = PlateSpec {
        holes: 6,
        chain_points: 2,
    };
    /// 8 + 2·7 + 3·26 = 100 constraints.
    pub const MEDIUM: PlateSpec = PlateSpec {
        holes: 26,
        chain_points: 7,
    };
    /// 8 + 2·32 + 3·76 = 300 constraints.
    pub const LARGE: PlateSpec = PlateSpec {
        holes: 76,
        chain_points: 32,
    };

    /// Exact constraint count the generator will emit for this spec.
    pub fn constraint_count(&self) -> usize {
        8 + 2 * self.chain_points + 3 * self.holes
    }

    /// Expected connected-component count: outline + chain + one per hole.
    pub fn expected_components(&self) -> usize {
        2 + self.holes
    }
}

/// One generated bolt hole with its dimensioned targets.
pub struct HoleTarget {
    pub center: Point2dId,
    pub circle: Circle2dId,
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
}

/// A generated plate sketch plus everything needed to verify the solve.
pub struct GeneratedPlate {
    pub sketch: Sketch,
    pub constraint_count: usize,
    /// Corner point ids with their dimensioned (x, y) targets, in order
    /// a(0,0) b(W,0) c(W,H) d(0,H).
    pub corners: [(Point2dId, f64, f64); 4],
    pub holes: Vec<HoleTarget>,
    /// Chain point ids with their dimensioned (x, y) targets.
    pub chain: Vec<(Point2dId, f64, f64)>,
}

/// Deterministic zero-dependency jitter in [-0.35, 0.35] — enough to
/// give Newton real work, small enough to stay in the intended basin
/// (`Distance` constraints have a sign-symmetric mirror solution).
pub fn jitter(seed: usize) -> f64 {
    let x = (seed as f64 * 12.989_8).sin() * 43_758.545_3;
    (x - x.trunc()) * 0.35
}

fn required_dim(dc: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(dc, entities, ConstraintPriority::Required)
}

fn required_geo(gc: GeometricConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_geometric(gc, entities, ConstraintPriority::Required)
}

/// Rectangular outline: 4 corner points + 4 shared-endpoint lines,
/// H/V per side, width + height `Distance` dims, X/Y anchor on corner a.
/// 8 constraints, 8 free DOFs — fully constrained, one component.
pub fn add_outline(sketch: &Sketch, salt: usize) -> [(Point2dId, f64, f64); 4] {
    let corner = |i: usize, x: f64, y: f64| {
        sketch.add_point(Point2d::new(
            x + jitter(salt + 2 * i),
            y + jitter(salt + 2 * i + 1),
        ))
    };
    let a = corner(0, 0.0, 0.0);
    let b = corner(1, PLATE_W, 0.0);
    let c = corner(2, PLATE_W, PLATE_H);
    let d = corner(3, 0.0, PLATE_H);
    let ab = sketch.add_line(a, b).expect("outline line ab");
    let bc = sketch.add_line(b, c).expect("outline line bc");
    let cd = sketch.add_line(c, d).expect("outline line cd");
    let da = sketch.add_line(d, a).expect("outline line da");

    sketch.add_constraint(required_geo(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(ab)],
    ));
    sketch.add_constraint(required_geo(
        GeometricConstraint::Vertical,
        vec![EntityRef::Line(bc)],
    ));
    sketch.add_constraint(required_geo(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(cd)],
    ));
    sketch.add_constraint(required_geo(
        GeometricConstraint::Vertical,
        vec![EntityRef::Line(da)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::Distance(PLATE_W),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::Distance(PLATE_H),
        vec![EntityRef::Point(a), EntityRef::Point(d)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(a)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(a)],
    ));

    [
        (a, 0.0, 0.0),
        (b, PLATE_W, 0.0),
        (c, PLATE_W, PLATE_H),
        (d, 0.0, PLATE_H),
    ]
}

/// Dimensioned target of hole `index` on the 10-wide grid.
pub fn hole_target(index: usize) -> (f64, f64) {
    let col = index % 10;
    let row = index / 10;
    (15.0 + 18.0 * col as f64, 15.0 + 18.0 * row as f64)
}

/// One ordinate-dimensioned bolt hole: shared-center circle, center
/// pinned by XCoordinate + YCoordinate, radius dimensioned.
/// 3 constraints, 3 free DOFs — fully constrained, its own component.
pub fn add_hole(sketch: &Sketch, index: usize, salt: usize) -> HoleTarget {
    let (cx, cy) = hole_target(index);
    let center = sketch.add_point(Point2d::new(
        cx + jitter(salt + 1000 + 2 * index),
        cy + jitter(salt + 1000 + 2 * index + 1),
    ));
    let circle = sketch
        .add_circle_centered(center, HOLE_R + 0.5 * jitter(salt + 5000 + index))
        .expect("hole circle");
    sketch.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(cx),
        vec![EntityRef::Point(center)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(cy),
        vec![EntityRef::Point(center)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::Radius(HOLE_R),
        vec![EntityRef::Circle(circle)],
    ));
    HoleTarget {
        center,
        circle,
        cx,
        cy,
        r: HOLE_R,
    }
}

/// Horizontal reference dimension chain of `points` points: first point
/// anchored (X+Y), each next point `Distance`-to-previous + YCoordinate.
/// 2·points constraints, 2·points free DOFs — fully constrained, one
/// component.
pub fn add_chain(sketch: &Sketch, points: usize, salt: usize) -> Vec<(Point2dId, f64, f64)> {
    let mut out = Vec::with_capacity(points);
    let mut prev: Option<Point2dId> = None;
    for i in 0..points {
        let tx = CHAIN_X0 + CHAIN_PITCH * i as f64;
        let id = sketch.add_point(Point2d::new(
            tx + jitter(salt + 9000 + 2 * i),
            CHAIN_Y + jitter(salt + 9000 + 2 * i + 1),
        ));
        match prev {
            None => {
                sketch.add_constraint(required_dim(
                    DimensionalConstraint::XCoordinate(tx),
                    vec![EntityRef::Point(id)],
                ));
                sketch.add_constraint(required_dim(
                    DimensionalConstraint::YCoordinate(CHAIN_Y),
                    vec![EntityRef::Point(id)],
                ));
            }
            Some(p) => {
                sketch.add_constraint(required_dim(
                    DimensionalConstraint::Distance(CHAIN_PITCH),
                    vec![EntityRef::Point(p), EntityRef::Point(id)],
                ));
                sketch.add_constraint(required_dim(
                    DimensionalConstraint::YCoordinate(CHAIN_Y),
                    vec![EntityRef::Point(id)],
                ));
            }
        }
        out.push((id, tx, CHAIN_Y));
        prev = Some(id);
    }
    out
}

/// Generate the full plate for a spec. `salt` de-correlates jitter
/// between independently generated plates of the same spec.
pub fn generate_plate_salted(spec: &PlateSpec, salt: usize) -> GeneratedPlate {
    let sketch = Sketch::new("dcm_slice2_plate".to_string(), SketchAnchor::xy());
    let corners = add_outline(&sketch, salt);
    let chain = add_chain(&sketch, spec.chain_points, salt);
    let holes = (0..spec.holes)
        .map(|i| add_hole(&sketch, i, salt))
        .collect::<Vec<_>>();
    let constraint_count = sketch.all_constraints().len();
    assert_eq!(
        constraint_count,
        spec.constraint_count(),
        "generator drifted from its documented constraint arithmetic"
    );
    GeneratedPlate {
        sketch,
        constraint_count,
        corners,
        holes,
        chain,
    }
}

/// Generate the full plate for a spec with the default salt.
pub fn generate_plate(spec: &PlateSpec) -> GeneratedPlate {
    generate_plate_salted(spec, 0)
}

fn dist(a: Point2d, b: Point2d) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Assert every dimensioned target of the plate was reached to `tol`.
pub fn assert_plate_solved(plate: &GeneratedPlate, tol: f64) {
    for (id, x, y) in &plate.corners {
        let p = plate.sketch.get_point(id).expect("corner present");
        assert!(
            dist(p, Point2d::new(*x, *y)) < tol,
            "corner missed its dimensioned target: {:?} vs ({x}, {y})",
            p
        );
    }
    for (id, x, y) in &plate.chain {
        let p = plate.sketch.get_point(id).expect("chain point present");
        assert!(
            dist(p, Point2d::new(*x, *y)) < tol,
            "chain point missed its dimensioned target: {:?} vs ({x}, {y})",
            p
        );
    }
    for hole in &plate.holes {
        let center = plate
            .sketch
            .get_point(&hole.center)
            .expect("hole center present");
        assert!(
            dist(center, Point2d::new(hole.cx, hole.cy)) < tol,
            "hole center missed its ordinate dimensions: {:?} vs ({}, {})",
            center,
            hole.cx,
            hole.cy
        );
        let entry = plate
            .sketch
            .circles()
            .get(&hole.circle)
            .expect("hole circle present");
        let r = entry.value().circle.radius;
        assert!(
            (r - hole.r).abs() < tol,
            "hole radius missed its dimension: {r} vs {}",
            hole.r
        );
    }
}
