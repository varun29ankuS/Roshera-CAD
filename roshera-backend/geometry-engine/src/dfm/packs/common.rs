//! The one verdict fold every process pack after FDM/molding shares.
//!
//! Each rule observes a list of FEATURES (a hole, a corner, a wall pair, a
//! flange), each with the faces that carry it and one measured value, and
//! compares that value against ONE limit in ONE sense. The fold is the same
//! per-rule precedence `fdm.rs`/`molding.rs` hand-roll per rule:
//!
//! 1. any proven violation ⇒ `Violation`, `measured` = the WORST feature,
//!    `witnesses` = every violating feature's faces (ascending, deduped);
//! 2. else any refusal ⇒ `Unverifiable` naming every refused region, with
//!    the lowest-face refusal's reason (deterministic);
//! 3. else `Pass` with the SMALLEST proven margin — the binding feature.
//!
//! A rule with no candidate feature at all passes vacuously and says so in
//! its margin's derivation (`VACUOUS_MARGIN`, the same sentinel the FDM
//! rules use) — an empty part has no deep hole.
//!
//! [`RuleFold::observe_lower_bound`] is the honesty hinge for values the
//! analyzer can only bound from below (a bore wall a cross hole split: the
//! wall is SHORTER than the hole). A lower bound above a maximum PROVES the
//! violation; a lower bound below it proves nothing and refuses.

use std::collections::BTreeSet;

use crate::dfm::analyzers::{pair_thickness, PairThicknessOutcome};
use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{
    Derivation, DfmError, DfmValue, FaceRef, RuleVerdict, SurfaceKind, UnverifiableReason, Verdict,
};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Margin of a rule that had no candidate feature (matches the FDM rules'
/// `NO_PAIR_SENTINEL_THICKNESS` convention: finite so it serializes).
pub(crate) const VACUOUS_MARGIN: f64 = 1.0e300;

/// Which side of the limit is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sense {
    /// `value >= limit` passes (a minimum radius, wall, diameter).
    AtLeast,
    /// `value <= limit` passes (a maximum ratio, deviation, angle).
    AtMost,
}

/// A derivation for a value that is a configured constant or a plain
/// arithmetic combination of analytic quantities (see `fdm.rs`'s
/// `constant_derivation` for the in-tree precedent of tagging it `Plane`
/// with the meaning in `method`).
pub(crate) fn analytic(method: impl Into<String>) -> Derivation {
    Derivation::Analytic {
        surface_type: SurfaceKind::Plane,
        method: method.into(),
    }
}

/// Analytic derivation on a named surface kind.
pub(crate) fn analytic_on(kind: SurfaceKind, method: impl Into<String>) -> Derivation {
    Derivation::Analytic {
        surface_type: kind,
        method: method.into(),
    }
}

pub(crate) struct RuleFold {
    rule: &'static str,
    provenance: RuleProvenance,
    sense: Sense,
    limit: DfmValue,
    violations: Vec<(Vec<FaceRef>, DfmValue)>,
    refusals: Vec<(Vec<FaceRef>, UnverifiableReason)>,
    best_margin: Option<DfmValue>,
}

impl RuleFold {
    pub(crate) fn new(
        rule: &'static str,
        provenance: RuleProvenance,
        sense: Sense,
        limit: f64,
        limit_method: impl Into<String>,
    ) -> Self {
        Self {
            rule,
            provenance,
            sense,
            limit: DfmValue::new(limit, analytic(limit_method)),
            violations: Vec::new(),
            refusals: Vec::new(),
            best_margin: None,
        }
    }

    fn violates(&self, value: f64) -> bool {
        match self.sense {
            Sense::AtLeast => value < self.limit.value,
            Sense::AtMost => value > self.limit.value,
        }
    }

    /// Observe one feature with an EXACT measured value.
    pub(crate) fn observe(&mut self, faces: Vec<FaceRef>, value: f64, derivation: Derivation) {
        if !value.is_finite() {
            self.refuse(
                faces,
                UnverifiableReason::UnsoundPrecondition {
                    detail: format!("{}: measured value is not finite", self.rule),
                },
            );
            return;
        }
        if self.violates(value) {
            self.violations
                .push((faces, DfmValue::new(value, derivation)));
            return;
        }
        let margin = match self.sense {
            Sense::AtLeast => value - self.limit.value,
            Sense::AtMost => self.limit.value - value,
        };
        let replace = self
            .best_margin
            .as_ref()
            .is_none_or(|current| margin < current.value);
        if replace {
            self.best_margin = Some(DfmValue::new(margin, derivation));
        }
    }

    /// Observe a feature whose value is only known as a LOWER bound. Only
    /// meaningful for [`Sense::AtMost`]: a lower bound above the maximum
    /// proves the violation; anything else is an honest refusal naming why.
    pub(crate) fn observe_lower_bound(
        &mut self,
        faces: Vec<FaceRef>,
        lower_bound: f64,
        derivation: Derivation,
        why_only_a_bound: &str,
    ) {
        if self.sense == Sense::AtMost && lower_bound.is_finite() && lower_bound > self.limit.value
        {
            self.violations
                .push((faces, DfmValue::new(lower_bound, derivation)));
        } else {
            self.refuse(
                faces,
                UnverifiableReason::UnsupportedTopology {
                    detail: format!(
                        "{}: only a lower bound {lower_bound:.6} is known ({why_only_a_bound}); \
                         it does not decide the limit {:.6}",
                        self.rule, self.limit.value
                    ),
                },
            );
        }
    }

    pub(crate) fn refuse(&mut self, faces: Vec<FaceRef>, reason: UnverifiableReason) {
        self.refusals.push((faces, reason));
    }

    pub(crate) fn finish(self) -> RuleVerdict {
        let RuleFold {
            rule,
            provenance,
            sense,
            limit,
            violations,
            mut refusals,
            best_margin,
        } = self;

        if !violations.is_empty() {
            let worst = violations
                .iter()
                .map(|(_, v)| v)
                .fold(None::<&DfmValue>, |acc, v| match acc {
                    None => Some(v),
                    Some(a) => {
                        let worse = match sense {
                            Sense::AtLeast => v.value < a.value,
                            Sense::AtMost => v.value > a.value,
                        };
                        Some(if worse { v } else { a })
                    }
                })
                .cloned()
                .unwrap_or_else(|| limit.clone());
            let witnesses: BTreeSet<FaceRef> = violations
                .iter()
                .flat_map(|(f, _)| f.iter().copied())
                .collect();
            return RuleVerdict {
                rule: rule.to_string(),
                verdict: Verdict::Violation {
                    witnesses: witnesses.into_iter().collect(),
                    measured: worst,
                    limit,
                },
                provenance,
            };
        }

        if !refusals.is_empty() {
            refusals.sort_by_key(|(f, _)| f.iter().min().copied().unwrap_or(FaceRef::MAX));
            let reason = refusals[0].1.clone();
            let regions: BTreeSet<FaceRef> = refusals
                .iter()
                .flat_map(|(f, _)| f.iter().copied())
                .collect();
            return RuleVerdict {
                rule: rule.to_string(),
                verdict: Verdict::Unverifiable {
                    regions: regions.into_iter().collect(),
                    reason,
                },
                provenance,
            };
        }

        let margin = best_margin.unwrap_or_else(|| {
            DfmValue::new(
                VACUOUS_MARGIN,
                analytic(format!(
                    "{rule}: no candidate feature on this part — holds vacuously"
                )),
            )
        });
        RuleVerdict {
            rule: rule.to_string(),
            verdict: Verdict::Pass { margin },
            provenance,
        }
    }
}

/// A verdict another pack's evaluator produced, re-issued under this
/// pack's rule id and provenance (the measurement is identical; only WHO
/// asks, and on whose authority, changes).
pub(crate) fn relabel(
    mut verdict: RuleVerdict,
    rule: &'static str,
    provenance: RuleProvenance,
) -> RuleVerdict {
    verdict.rule = rule.to_string();
    verdict.provenance = provenance;
    verdict
}

/// Every face of the solid (outer, inner and peer shells), or
/// [`DfmError::DanglingSolidRef`].
pub(crate) fn solid_face_ids(
    model: &BRepModel,
    solid_id: SolidId,
) -> Result<Vec<FaceId>, DfmError> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or(DfmError::DanglingSolidRef { solid: solid_id })?;
    let mut faces = Vec::new();
    for shell_id in solid.all_shells() {
        if let Some(shell) = model.shells.get(shell_id) {
            faces.extend(shell.faces.iter().copied());
        }
    }
    Ok(faces)
}

/// Fold every proven wall pair of [`pair_thickness`] into `fold` (one
/// feature per pair, both faces as witnesses) and every unpaired region
/// into a refusal.
pub(crate) fn fold_wall_pairs(fold: &mut RuleFold, outcome: &PairThicknessOutcome) {
    for pair in &outcome.pairs {
        fold.observe(
            vec![pair.face_a, pair.face_b],
            pair.thickness.value,
            pair.thickness.derivation.clone(),
        );
    }
    for u in &outcome.unverifiable {
        fold.refuse(vec![u.face], u.reason.clone());
    }
}

/// Wall pairs of the whole solid.
pub(crate) fn solid_wall_pairs(
    model: &BRepModel,
    faces: &[FaceId],
) -> Result<PairThicknessOutcome, DfmError> {
    pair_thickness(
        faces,
        &model.faces,
        &model.loops,
        &model.edges,
        &model.curves,
        &model.surfaces,
    )
}

/// Normalize caller-supplied directions; a zero / non-finite one is
/// malformed input.
pub(crate) fn unit_directions(dirs: &[[f64; 3]], what: &str) -> Result<Vec<[f64; 3]>, DfmError> {
    dirs.iter().map(|d| unit_direction(*d, what)).collect()
}

pub(crate) fn unit_direction(d: [f64; 3], what: &str) -> Result<[f64; 3], DfmError> {
    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if n > 0.0 && n.is_finite() {
        Ok([d[0] / n, d[1] / n, d[2] / n])
    } else {
        Err(DfmError::InvalidParams {
            detail: format!("{what} {d:?} is not a usable direction"),
        })
    }
}

/// Angle between two unit vectors, degrees, accurate near 0 (atan2 of
/// |a×b| and a·b, never acos).
pub(crate) fn angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let s = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    s.atan2(d).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prov() -> RuleProvenance {
        RuleProvenance::ShopPractice {
            note: "test".to_string(),
        }
    }

    #[test]
    fn at_least_violation_reports_worst_and_all_witnesses() {
        let mut f = RuleFold::new("t.min", prov(), Sense::AtLeast, 1.5, "limit");
        f.observe(vec![4, 2], 1.0, analytic("a"));
        f.observe(vec![9], 0.5, analytic("b"));
        f.observe(vec![1], 3.0, analytic("c"));
        match f.finish().verdict {
            Verdict::Violation {
                witnesses,
                measured,
                limit,
            } => {
                assert_eq!(witnesses, vec![2, 4, 9]);
                assert_eq!(measured.value, 0.5);
                assert_eq!(limit.value, 1.5);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn at_most_pass_reports_smallest_margin_and_ties_pass() {
        let mut f = RuleFold::new("t.max", prov(), Sense::AtMost, 10.0, "limit");
        f.observe(vec![1], 4.0, analytic("a"));
        f.observe(vec![2], 10.0, analytic("tie"));
        match f.finish().verdict {
            Verdict::Pass { margin } => assert_eq!(margin.value, 0.0),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_lower_bound_under_the_max_refuses_never_passes() {
        let mut f = RuleFold::new("t.max", prov(), Sense::AtMost, 10.0, "limit");
        f.observe_lower_bound(vec![3], 4.0, analytic("lb"), "split wall");
        assert!(matches!(f.finish().verdict, Verdict::Unverifiable { .. }));
        let mut g = RuleFold::new("t.max", prov(), Sense::AtMost, 10.0, "limit");
        g.observe_lower_bound(vec![3], 12.0, analytic("lb"), "split wall");
        assert!(matches!(g.finish().verdict, Verdict::Violation { .. }));
    }

    #[test]
    fn violation_dominates_refusal_and_empty_is_vacuous_pass() {
        let mut f = RuleFold::new("t.min", prov(), Sense::AtLeast, 1.0, "limit");
        f.refuse(
            vec![5],
            UnverifiableReason::UnsupportedTopology { detail: "x".into() },
        );
        f.observe(vec![6], 0.1, analytic("a"));
        assert!(matches!(f.finish().verdict, Verdict::Violation { .. }));
        let e = RuleFold::new("t.min", prov(), Sense::AtLeast, 1.0, "limit");
        match e.finish().verdict {
            Verdict::Pass { margin } => assert_eq!(margin.value, VACUOUS_MARGIN),
            o => panic!("{o:?}"),
        }
    }
}
