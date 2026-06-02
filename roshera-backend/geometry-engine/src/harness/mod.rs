//! Kernel measurement + ablation harness.
//!
//! Every multi-stage kernel pipeline — contact determination, booleans,
//! tessellation — is a funnel: candidates enter, each stage prunes or
//! transforms them at some cost, an answer comes out. This module is the common
//! substrate for *measuring* that funnel, so the contribution of each stage is a
//! number rather than a claim, and so an **ablation study** (turn a stage off,
//! re-measure) is a first-class, repeatable thing rather than an ad-hoc script.
//!
//! Two types carry it:
//! * [`StageMetric`] — one stage's input/output counts and the work it cost.
//! * [`AblationReport`] — the ordered stages of one configuration's run, plus an
//!   optional correctness verdict against an oracle.
//!
//! The first complete study is [`cd`] (the contact-determination pipeline). Other
//! kernel areas plug in by producing their own `AblationReport`s the same way.

pub mod boolean;
pub mod cd;

/// One stage of a measured pipeline. `input`/`output` are candidate counts
/// (e.g. feature-pairs), and `cost` is the work the stage performed in whatever
/// unit is natural for it (cone tests, node visits, LMD solves) — comparable
/// only within a stage across configurations, which is exactly what ablation
/// needs.
#[derive(Debug, Clone)]
pub struct StageMetric {
    pub name: String,
    pub input: usize,
    pub output: usize,
    pub cost: u64,
}

impl StageMetric {
    pub fn new(name: impl Into<String>, input: usize, output: usize, cost: u64) -> Self {
        Self {
            name: name.into(),
            input,
            output,
            cost,
        }
    }

    /// Fraction of candidates that survived this stage (`1.0` if nothing entered).
    pub fn survival(&self) -> f64 {
        if self.input == 0 {
            1.0
        } else {
            self.output as f64 / self.input as f64
        }
    }

    /// Candidates removed by this stage.
    pub fn pruned(&self) -> usize {
        self.input.saturating_sub(self.output)
    }
}

/// A measured run of one pipeline configuration: its ordered stages and, when an
/// oracle is available, whether it produced the correct answer. The backbone of
/// every kernel ablation study.
#[derive(Debug, Clone)]
pub struct AblationReport {
    pub label: String,
    pub stages: Vec<StageMetric>,
    /// `Some(true/false)` once checked against an oracle; `None` if unverified.
    pub correct: Option<bool>,
}

impl AblationReport {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            stages: Vec::new(),
            correct: None,
        }
    }

    /// Append a stage (builder-style).
    pub fn stage(mut self, m: StageMetric) -> Self {
        self.stages.push(m);
        self
    }

    /// Record the correctness verdict (builder-style).
    pub fn verified(mut self, ok: bool) -> Self {
        self.correct = Some(ok);
        self
    }

    /// Total work across all stages.
    pub fn total_cost(&self) -> u64 {
        self.stages.iter().map(|s| s.cost).sum()
    }

    /// Candidates entering the first stage.
    pub fn input(&self) -> usize {
        self.stages.first().map_or(0, |s| s.input)
    }

    /// Candidates leaving the last stage.
    pub fn output(&self) -> usize {
        self.stages.last().map_or(0, |s| s.output)
    }

    /// A human-readable funnel table.
    pub fn render(&self) -> String {
        let mut out = format!("ablation: {}\n", self.label);
        for s in &self.stages {
            out.push_str(&format!(
                "  {:<22} {:>7} → {:>7}  (pruned {:>7}, cost {:>8})\n",
                s.name,
                s.input,
                s.output,
                s.pruned(),
                s.cost
            ));
        }
        out.push_str(&format!("  total cost: {}", self.total_cost()));
        match self.correct {
            Some(true) => out.push_str("  [verified ✓]"),
            Some(false) => out.push_str("  [INCORRECT ✗]"),
            None => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_survival_and_pruned() {
        let s = StageMetric::new("cull", 100, 12, 100);
        assert_eq!(s.pruned(), 88);
        assert!((s.survival() - 0.12).abs() < 1e-12);
        let empty = StageMetric::new("noop", 0, 0, 0);
        assert_eq!(empty.survival(), 1.0);
    }

    #[test]
    fn report_aggregates_and_renders() {
        let r = AblationReport::new("demo")
            .stage(StageMetric::new("broad", 36, 6, 11))
            .stage(StageMetric::new("narrow", 6, 1, 6))
            .verified(true);
        assert_eq!(r.input(), 36);
        assert_eq!(r.output(), 1);
        assert_eq!(r.total_cost(), 17);
        assert_eq!(r.correct, Some(true));
        let text = r.render();
        assert!(text.contains("broad") && text.contains("verified"));
    }
}
