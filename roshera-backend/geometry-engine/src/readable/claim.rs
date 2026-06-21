//! Blackboard claim verification — "the notebook that can't lie" (Track 6).
//!
//! Checks an equation or numeric claim written on the blackboard against the
//! kernel's GROUND-TRUTH measurements, deterministically. Each claim variable
//! BINDS to an exact kernel measurement (volume, surface area, …); the
//! expression is evaluated by a math evaluator — NOT an LLM, exact arithmetic
//! over a variable→measurement context — and compared to the asserted value
//! within tolerance.
//!
//! Three honest verdicts, never a silent pass:
//! * **verified** — computed matches expected within tolerance,
//! * **false** — a real mismatch (with the error),
//! * **refused** — a binding could not be resolved, or the expression did not
//!   evaluate to a number. The kernel refuses rather than guessing.
//!
//! Pairs with the recognition moat: recognition checks the agent's *labels*
//! (is the thing it called a gear a gear?); this checks the agent's *reasoning*
//! (does the equation it wrote hold against measured geometry?).

use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};

/// What the kernel should measure for a claim variable. A CLOSED enum — every
/// binding either resolves to an exact measurement or is REFUSED, never guessed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Measurement {
    /// Solid volume.
    Volume { solid: SolidId },
    /// Solid surface area.
    SurfaceArea { solid: SolidId },
    /// Area of a single face (from its exact analytic surface).
    FaceArea { face: u32 },
    /// Length of a single edge (from its exact curve).
    EdgeLength { edge: u32 },
    /// A non-geometric supplied constant (recorded, taken as given — the honest
    /// way external/physics inputs enter a check).
    Constant { value: f64 },
}

/// Binds a variable name in the claim expression to a kernel measurement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaimBinding {
    pub var: String,
    pub measure: Measurement,
}

/// A checkable claim: an expression over the bound variables that should equal
/// `expected` within `tolerance`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckableClaim {
    /// Math expression over the binding variable names, e.g. `"A_exit / A_throat"`
    /// or simply `"v"`.
    pub expr: String,
    pub bindings: Vec<ClaimBinding>,
    /// The asserted value `expr` should equal.
    pub expected: f64,
    /// Absolute tolerance; `None` → derived from `expected`'s magnitude.
    pub tolerance: Option<f64>,
}

/// The verdict — three honest states (verified / false / refused).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimVerdict {
    pub verified: bool,
    /// A binding could not resolve, or the expression failed to evaluate to a
    /// number — REFUSED rather than guessed.
    pub refused: bool,
    pub computed: Option<f64>,
    pub expected: f64,
    pub abs_error: Option<f64>,
    pub tolerance_used: f64,
    /// Resolved `(var, value)` pairs — the provenance of the check.
    pub resolved: Vec<(String, f64)>,
    /// Variables whose binding could not be resolved.
    pub unresolved: Vec<String>,
}

fn resolve(model: &mut BRepModel, measure: &Measurement) -> Option<f64> {
    match measure {
        Measurement::Volume { solid } => model.mass_properties_for(*solid).map(|mp| mp.volume),
        Measurement::SurfaceArea { solid } => {
            model.mass_properties_for(*solid).map(|mp| mp.surface_area)
        }
        Measurement::FaceArea { face } => model.query_face(*face).and_then(|f| f.area),
        Measurement::EdgeLength { edge } => model.query_edge(*edge).and_then(|e| e.length),
        Measurement::Constant { value } => Some(*value),
    }
}

/// Verify a claim against the kernel's ground-truth geometry. Takes `&mut` because
/// mass-properties measurement caches on the model.
pub fn verify_claim(claim: &CheckableClaim, model: &mut BRepModel) -> ClaimVerdict {
    use evalexpr::{ContextWithMutableVariables, HashMapContext, Value};

    let tolerance_used = claim
        .tolerance
        .unwrap_or_else(|| claim.expected.abs().max(1.0) * 1e-6);

    let mut ctx = HashMapContext::new();
    let mut resolved: Vec<(String, f64)> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for b in &claim.bindings {
        match resolve(model, &b.measure) {
            Some(val) => {
                // `set_value` only errors on a reserved identifier; if so the
                // variable stays unbound and the expr eval will refuse below.
                let _ = ctx.set_value(b.var.clone(), Value::Float(val));
                resolved.push((b.var.clone(), val));
            }
            None => unresolved.push(b.var.clone()),
        }
    }

    if !unresolved.is_empty() {
        return ClaimVerdict {
            verified: false,
            refused: true,
            computed: None,
            expected: claim.expected,
            abs_error: None,
            tolerance_used,
            resolved,
            unresolved,
        };
    }

    let computed = evalexpr::eval_with_context(&claim.expr, &ctx)
        .ok()
        .and_then(|v| v.as_number().ok());
    let Some(computed) = computed else {
        return ClaimVerdict {
            verified: false,
            refused: true,
            computed: None,
            expected: claim.expected,
            abs_error: None,
            tolerance_used,
            resolved,
            unresolved: vec![format!(
                "expression did not evaluate to a number: {}",
                claim.expr
            )],
        };
    };

    let abs_error = (computed - claim.expected).abs();
    ClaimVerdict {
        verified: abs_error <= tolerance_used,
        refused: false,
        computed: Some(computed),
        expected: claim.expected,
        abs_error: Some(abs_error),
        tolerance_used,
        resolved,
        unresolved: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    //! Build-it-then-verify harness: a part with KNOWN geometry, a true claim
    //! verifies, a wrong claim is flagged (not refused), an unresolvable claim is
    //! refused (not silently passed).
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn box_solid(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
        match TopologyBuilder::new(m).create_box_3d(w, h, d).expect("box") {
            GeometryId::Solid(id) => id,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    fn vol_binding(s: SolidId) -> ClaimBinding {
        ClaimBinding {
            var: "v".to_string(),
            measure: Measurement::Volume { solid: s },
        }
    }

    #[test]
    fn true_numeric_claim_verifies() {
        let mut m = BRepModel::new();
        let s = box_solid(&mut m, 20.0, 15.0, 10.0); // volume 3000
        let claim = CheckableClaim {
            expr: "v".to_string(),
            bindings: vec![vol_binding(s)],
            expected: 3000.0,
            tolerance: None,
        };
        let verdict = verify_claim(&claim, &mut m);
        assert!(verdict.verified && !verdict.refused, "{verdict:?}");
    }

    #[test]
    fn wrong_claim_is_flagged_not_refused() {
        let mut m = BRepModel::new();
        let s = box_solid(&mut m, 20.0, 15.0, 10.0); // volume 3000
        let claim = CheckableClaim {
            expr: "v".to_string(),
            bindings: vec![vol_binding(s)],
            expected: 2500.0, // wrong by 500
            tolerance: None,
        };
        let verdict = verify_claim(&claim, &mut m);
        assert!(!verdict.verified && !verdict.refused, "{verdict:?}");
        assert!(verdict.abs_error.unwrap_or(0.0) > 400.0);
    }

    #[test]
    fn formula_over_two_measurements_verifies() {
        let mut m = BRepModel::new();
        // 20×15×10: volume 3000, surface area 2(300+200+150)=1300.
        let s = box_solid(&mut m, 20.0, 15.0, 10.0);
        let claim = CheckableClaim {
            expr: "a / v".to_string(),
            bindings: vec![
                ClaimBinding {
                    var: "a".to_string(),
                    measure: Measurement::SurfaceArea { solid: s },
                },
                vol_binding(s),
            ],
            expected: 1300.0 / 3000.0,
            tolerance: Some(1e-3),
        };
        assert!(verify_claim(&claim, &mut m).verified);
    }

    #[test]
    fn claim_with_supplied_constant_verifies() {
        let mut m = BRepModel::new();
        let s = box_solid(&mut m, 10.0, 10.0, 10.0); // volume 1000
                                                     // mass = volume * density, density supplied as a Constant.
        let claim = CheckableClaim {
            expr: "v * rho".to_string(),
            bindings: vec![
                vol_binding(s),
                ClaimBinding {
                    var: "rho".to_string(),
                    measure: Measurement::Constant { value: 2.7 },
                },
            ],
            expected: 2700.0,
            tolerance: Some(1e-3),
        };
        assert!(verify_claim(&claim, &mut m).verified);
    }

    #[test]
    fn unresolvable_binding_is_refused() {
        // `s` is a valid solid id in model A but queried against an EMPTY model B,
        // so its measurement cannot resolve → REFUSED, never a silent pass.
        let mut m_a = BRepModel::new();
        let s = box_solid(&mut m_a, 10.0, 10.0, 10.0);
        let mut m_b = BRepModel::new();
        let claim = CheckableClaim {
            expr: "v".to_string(),
            bindings: vec![vol_binding(s)],
            expected: 1000.0,
            tolerance: None,
        };
        let verdict = verify_claim(&claim, &mut m_b);
        assert!(verdict.refused && !verdict.verified, "{verdict:?}");
        assert!(!verdict.unresolved.is_empty());
    }

    #[test]
    fn face_area_and_edge_length_formula_verifies() {
        // A 10-cube: every face is 10×10=100, every edge is length 10. The claim
        // "fa / (el*el) == 1" exercises both new measurements in one formula.
        let mut m = BRepModel::new();
        let _s = box_solid(&mut m, 10.0, 10.0, 10.0);
        let face = m.faces.iter().next().expect("a face").0;
        let edge = m.edges.iter().next().expect("an edge").0;
        let claim = CheckableClaim {
            expr: "fa / (el * el)".to_string(),
            bindings: vec![
                ClaimBinding {
                    var: "fa".to_string(),
                    measure: Measurement::FaceArea { face },
                },
                ClaimBinding {
                    var: "el".to_string(),
                    measure: Measurement::EdgeLength { edge },
                },
            ],
            expected: 1.0,
            tolerance: Some(1e-6),
        };
        let verdict = verify_claim(&claim, &mut m);
        assert!(verdict.verified && !verdict.refused, "{verdict:?}");
    }

    #[test]
    fn wrong_face_area_is_flagged() {
        let mut m = BRepModel::new();
        let _s = box_solid(&mut m, 10.0, 10.0, 10.0); // face area is 100
        let face = m.faces.iter().next().expect("a face").0;
        let claim = CheckableClaim {
            expr: "fa".to_string(),
            bindings: vec![ClaimBinding {
                var: "fa".to_string(),
                measure: Measurement::FaceArea { face },
            }],
            expected: 50.0, // wrong
            tolerance: None,
        };
        let verdict = verify_claim(&claim, &mut m);
        assert!(!verdict.verified && !verdict.refused, "{verdict:?}");
    }

    #[test]
    fn unknown_face_id_is_refused() {
        let mut m = BRepModel::new();
        let _s = box_solid(&mut m, 10.0, 10.0, 10.0);
        let claim = CheckableClaim {
            expr: "fa".to_string(),
            bindings: vec![ClaimBinding {
                var: "fa".to_string(),
                measure: Measurement::FaceArea { face: 99_999 },
            }],
            expected: 100.0,
            tolerance: None,
        };
        let verdict = verify_claim(&claim, &mut m);
        assert!(verdict.refused && !verdict.verified, "{verdict:?}");
    }
}
