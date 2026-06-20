//! Sketch-validity oracle — the verification-layer entry for 2D sketches.
//!
//! The "can't lie" moat extends from solids to sketches via the
//! [`SketchValidityCertificate`] ([`crate::sketch2d::certify_sketch`]): a sketch
//! is SOUND only when its constraints are mutually satisfiable, its entities are
//! geometrically valid, and it does not self-intersect. DOF freedom and open
//! profiles are REPORTED, not failed.
//!
//! This module makes that certificate a first-class part of the kernel's
//! verification harness, alongside the solid oracles ([`super::watertight`],
//! [`super::self_intersection`], [`super::brep_integrity`], …), so a verification
//! run over a document can check its SKETCHES with the same contract idiom it
//! uses for solids — not just its solids. The exhaustive adversarial gate lives
//! in `tests/sketch_certificate_gate.rs`; this is the reusable oracle it builds on.

use crate::sketch2d::{certify_sketch, Sketch, SketchValidityCertificate};

/// Certify a sketch and return its full validity certificate — the
/// verification-layer view of a 2D sketch. Pure (no mutation).
pub fn certify(sketch: &Sketch) -> SketchValidityCertificate {
    certify_sketch(sketch)
}

/// Verification CONTRACT for a sketch: `Ok(())` when the sketch is sound,
/// `Err(issues)` enumerating every defect otherwise. Mirrors the contract idiom
/// of the solid oracles so a sketch slots into the same verification bundle.
///
/// "Sound" gates only on real defects — inconsistent constraints, degenerate
/// entities, self-intersection. An under-constrained or open sketch is a *legal*
/// sketch and passes the contract (the certificate still reports those facts).
pub fn sketch_soundness_contract(sketch: &Sketch) -> Result<(), Vec<String>> {
    let cert = certify_sketch(sketch);
    if cert.is_sound() {
        Ok(())
    } else {
        let mut issues = cert.issues.clone();
        if issues.is_empty() {
            issues.push(format!("sketch unsound: {}", cert.summary()));
        }
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::{
        Constraint, ConstraintPriority, DimensionalConstraint, EntityRef, Point2d, SketchAnchor,
    };

    #[test]
    fn clean_sketch_passes_the_contract() {
        let sketch = Sketch::new("ok".to_string(), SketchAnchor::xy());
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(10.0, 0.0));
        let c = sketch.add_point(Point2d::new(5.0, 8.0));
        sketch.add_line(a, b).expect("a-b");
        sketch.add_line(b, c).expect("b-c");
        sketch.add_line(c, a).expect("c-a");
        assert!(
            sketch_soundness_contract(&sketch).is_ok(),
            "a clean triangle must pass the verification contract"
        );
    }

    #[test]
    fn contradictory_sketch_fails_the_contract_with_issues() {
        let sketch = Sketch::new("bad".to_string(), SketchAnchor::xy());
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(10.0, 0.0));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(10.0),
            vec![EntityRef::Point(a), EntityRef::Point(b)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(20.0),
            vec![EntityRef::Point(a), EntityRef::Point(b)],
            ConstraintPriority::Required,
        ));
        let result = sketch_soundness_contract(&sketch);
        assert!(result.is_err(), "contradictory dimensions must fail");
        assert!(
            !result.err().unwrap_or_default().is_empty(),
            "a failing contract must enumerate the defects"
        );
    }
}
