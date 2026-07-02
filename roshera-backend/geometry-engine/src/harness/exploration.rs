//! Certified autonomous exploration sweep (Move 3).
//!
//! This is the search layer over the parametric rocket-engine recipe in
//! [`crate::harness::engine_variant`]. It samples the design space
//! deterministically, builds and certifies every variant on a fresh
//! [`BRepModel`] (embarrassingly parallel via rayon — no shared model), and
//! records an honest per-variant outcome:
//!
//! * **REFUSED** — an op (or the recipe's up-front guard) rejected the input
//!   before a solid existed. A typed [`VariantRefusal`].
//! * **CERT_KILLED** — the variant built, but the ambient
//!   [`ValidityCertificate`] found it unsound. The failing cert dimensions are
//!   recorded verbatim. This is a *genuine* geometry failure the certificate
//!   caught, not a scripted one.
//! * **SOUND** — built and every sound-affecting cert dimension passed.
//! * **TIMED_OUT** — the variant exceeded the per-variant wall-clock soft
//!   budget. Reported honestly as its own outcome; it never blocks the sweep.
//!
//! The sampler is **seeded** (a `u64` seed → a SplitMix64 stream): the same
//! `(n, seed)` always yields the same parameter sequence, so a sweep is
//! reproducible. Nothing here reads the clock for randomness.
//!
//! The ranking helper implements the design's honest objective: among SOUND
//! variants that also fit the envelope and hold their internal cavity volume
//! within a band of a target, order by minimum wall-material volume. No thrust,
//! no CFD — pure geometry read straight off the kernel.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::harness::engine_variant::{
    build_variant, certify_variant, EngineParams, Envelope, VariantRefusal,
};
use crate::primitives::topology_builder::BRepModel;

/// Per-variant wall-clock soft budget. A variant that exceeds this is recorded
/// as [`VariantOutcome::TimedOut`] rather than allowed to stall the sweep. The
/// budget is checked *after* the (single-threaded, uninterruptible) build+cert
/// returns — so it catches a slow variant honestly without forcibly killing a
/// running kernel call. Generous per design §3 (release builds are ~0.5–2 s/
/// variant; 60 s is far above any healthy variant).
pub const PER_VARIANT_BUDGET: Duration = Duration::from_secs(60);

/// Inclusive numeric range `[lo, hi]` the sampler draws a parameter from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub lo: f64,
    pub hi: f64,
}

impl Range {
    const fn new(lo: f64, hi: f64) -> Self {
        Self { lo, hi }
    }

    /// Map a unit sample `u ∈ [0, 1)` into the range.
    fn at(&self, u: f64) -> f64 {
        self.lo + (self.hi - self.lo) * u
    }
}

/// The design-space ranges the sampler draws from. Chosen (and tuned at G3) so a
/// healthy fraction of the sweep lands in genuine failure regimes:
///
/// * **fold-through** — `wall_t` can exceed the throat radius, so the outer
///   offset wall self-intersects at the throat (caught at op level or by
///   `self_intersection_free`).
/// * **rim-grazing** — `ring_frac` reaches near 1, so the injector ring grazes
///   the plate rim (sliver / open boundary).
/// * **overlapping bores** — `hole_count × hole_r` can be too large for the
///   ring, tripping the recipe's [`VariantRefusal::OverlappingHoles`] guard.
///
/// The healthy interior of each range still produces sound engines, so the
/// winner is a real design, not a survivor of an all-failure space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerRanges {
    pub throat_r: Range,
    pub expansion_ratio: Range,
    pub chamber_r: Range,
    pub chamber_l_over_d: Range,
    pub wall_t: Range,
    /// Injector hole count is integer; drawn from `[lo, hi]` and rounded.
    pub hole_count: Range,
    pub hole_r: Range,
    pub ring_frac: Range,
}

impl Default for SamplerRanges {
    /// The tuned G3 ranges (see `move3-g3-report.md`). The dominant genuine kill
    /// is `self_intersection_free`, and the empirical G3 finding is that those
    /// kills cluster at HIGH chamber L/D: a long chamber makes the single smooth
    /// cubic that fits the whole (chamber→throat→bell) contour overshoot, so the
    /// offset outer wall folds through itself — an authentic geometry defect the
    /// certificate catches, NOT a scripted one. Capping L/D at 1.35 keeps most
    /// contours fittable (healthy interior survives) while the aggressive tail
    /// still folds through, landing the sweep at ~25% cert-kill. `ring_frac`
    /// reaches the rim for grazing cases; small ring fractions with several fat
    /// holes trip the [`VariantRefusal::OverlappingHoles`] guard (counted as a
    /// refusal, separate from cert kills).
    fn default() -> Self {
        Self {
            throat_r: Range::new(4.5, 8.0),
            expansion_ratio: Range::new(8.0, 25.0),
            chamber_r: Range::new(10.0, 22.0),
            chamber_l_over_d: Range::new(0.8, 1.35),
            wall_t: Range::new(1.0, 5.0),
            // Small ring fractions with several fat holes push adjacent bores
            // into overlap → OverlappingHoles refusal (counted separately from
            // cert kills). The ring reaches the rim (0.95) for grazing cases.
            hole_count: Range::new(4.0, 12.0),
            hole_r: Range::new(1.2, 3.0),
            ring_frac: Range::new(0.30, 0.95),
        }
    }
}

/// A seeded SplitMix64 stream — a tiny, well-distributed, deterministic PRNG
/// (Steele, Lea & Flood, "Fast Splittable Pseudorandom Number Generators",
/// OOPSLA 2014). No external dependency; the same seed always reproduces the
/// same stream.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform double in `[0, 1)` (53-bit mantissa resolution).
    fn next_unit(&mut self) -> f64 {
        // Top 53 bits → [0, 2^53) → divide by 2^53.
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

/// Sample one [`EngineParams`] from the given ranges using the stream.
fn sample_one(rng: &mut SplitMix64, r: &SamplerRanges) -> EngineParams {
    let throat_r = r.throat_r.at(rng.next_unit());
    let expansion_ratio = r.expansion_ratio.at(rng.next_unit());
    let chamber_r = r.chamber_r.at(rng.next_unit());
    let chamber_l_over_d = r.chamber_l_over_d.at(rng.next_unit());
    let wall_t = r.wall_t.at(rng.next_unit());
    let hole_count = r.hole_count.at(rng.next_unit()).round().max(0.0) as usize;
    let hole_r = r.hole_r.at(rng.next_unit());
    let ring_frac = r.ring_frac.at(rng.next_unit());
    EngineParams {
        throat_r,
        expansion_ratio,
        chamber_r,
        chamber_l_over_d,
        wall_t,
        hole_count,
        hole_r,
        ring_frac,
    }
}

/// Generate `n` deterministic variants from `seed` over the given ranges. Same
/// `(n, seed, ranges)` → identical output every call.
pub fn sample_params(n: usize, seed: u64, ranges: &SamplerRanges) -> Vec<EngineParams> {
    let mut rng = SplitMix64::new(seed);
    (0..n).map(|_| sample_one(&mut rng, ranges)).collect()
}

/// Convenience: sample with the tuned default ranges.
pub fn sample_default(n: usize, seed: u64) -> Vec<EngineParams> {
    sample_params(n, seed, &SamplerRanges::default())
}

/// The honest outcome of evaluating one variant.
#[derive(Debug, Clone, PartialEq)]
pub enum VariantOutcome {
    /// An op (or the recipe's up-front guard) rejected the input.
    Refused(VariantRefusal),
    /// Built, then the certificate found it unsound. Carries the failing
    /// sound-affecting cert dimensions.
    CertKilled(Vec<&'static str>),
    /// Built and every cert dimension passed.
    Sound,
    /// Exceeded the per-variant wall-clock budget.
    TimedOut,
    /// A build or certify step panicked (caught, never propagated). Carries the
    /// stage label for diagnosis. Distinct from a clean typed refusal.
    Panicked(String),
}

impl VariantOutcome {
    /// A short stable tag for scoreboards.
    pub fn tag(&self) -> &'static str {
        match self {
            VariantOutcome::Refused(_) => "REFUSED",
            VariantOutcome::CertKilled(_) => "CERT_KILLED",
            VariantOutcome::Sound => "SOUND",
            VariantOutcome::TimedOut => "TIMED_OUT",
            VariantOutcome::Panicked(_) => "PANICKED",
        }
    }
}

/// One row of the exploration scoreboard: the params, the outcome, the measured
/// volumes, envelope fit, and wall-clock cost. Every field is measured — nothing
/// is assumed.
#[derive(Debug, Clone)]
pub struct VariantRow {
    /// Filesystem-safe params label (from [`EngineParams::label`]).
    pub label: String,
    /// The full params (for the JSON artifact / winner rebuild).
    pub params: EngineParams,
    /// The honest outcome.
    pub outcome: VariantOutcome,
    /// Wall-material volume (shell + plate), if the variant built and both
    /// solids yielded a physical volume.
    pub wall_material_volume: Option<f64>,
    /// Internal cavity volume, if computable.
    pub internal_volume: Option<f64>,
    /// Whether the combined bounds fit the envelope (only meaningful when built).
    pub in_envelope: bool,
    /// Wall-clock elapsed for this variant's build + certify, in milliseconds.
    pub elapsed_ms: u128,
}

/// Aggregate timings for the whole sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepTimings {
    /// Total wall-clock of the parallel sweep (the driver's own span).
    pub total_ms: u128,
    /// Sum of per-variant elapsed times (parallel work; exceeds `total_ms` on
    /// multiple threads).
    pub sum_variant_ms: u128,
    /// Mean per-variant elapsed (`sum_variant_ms / n`), in milliseconds.
    pub mean_variant_ms: f64,
    /// Threads requested for the sweep.
    pub threads: usize,
}

/// The full report of one sweep: every row plus derived counts. All counts are
/// derived from `rows` (a benchmark about honest search must not hardcode its
/// table).
#[derive(Debug, Clone)]
pub struct ExplorationReport {
    pub rows: Vec<VariantRow>,
    pub refused: usize,
    pub cert_killed: usize,
    pub sound: usize,
    pub timed_out: usize,
    pub panicked: usize,
    pub timings: SweepTimings,
    /// The envelope the variants were certified against.
    pub envelope: Envelope,
}

impl ExplorationReport {
    /// Kill histogram: how many CERT_KILLED variants failed each cert dimension
    /// (a variant failing several dimensions counts once per dimension). Derived.
    pub fn kill_histogram(&self) -> Vec<(&'static str, usize)> {
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for row in &self.rows {
            if let VariantOutcome::CertKilled(dims) = &row.outcome {
                for &d in dims {
                    match counts.iter_mut().find(|(name, _)| *name == d) {
                        Some((_, c)) => *c += 1,
                        None => counts.push((d, 1)),
                    }
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        counts
    }

    /// Refusal histogram: how many variants each refusal *kind* accounted for.
    pub fn refusal_histogram(&self) -> Vec<(&'static str, usize)> {
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for row in &self.rows {
            if let VariantOutcome::Refused(r) = &row.outcome {
                let kind = refusal_kind(r);
                match counts.iter_mut().find(|(name, _)| *name == kind) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((kind, 1)),
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        counts
    }
}

/// Stable kind tag for a refusal (for the refusal histogram).
fn refusal_kind(r: &VariantRefusal) -> &'static str {
    match r {
        VariantRefusal::InvalidParams(_) => "InvalidParams",
        VariantRefusal::Revolve(_) => "Revolve",
        VariantRefusal::PlatePrimitive(_) => "PlatePrimitive",
        VariantRefusal::HoleDrill(_) => "HoleDrill",
        VariantRefusal::OverlappingHoles(_) => "OverlappingHoles",
    }
}

/// Evaluate one variant on a FRESH model. Never panics: build/certify are
/// wrapped in `catch_unwind` (the kernel is `panic = "deny"` in normal paths,
/// but a sweep over hundreds of adversarial inputs must be armored). The
/// per-variant soft budget is enforced by measuring the elapsed span and
/// reclassifying an over-budget variant as [`VariantOutcome::TimedOut`].
fn evaluate_one(params: &EngineParams, envelope: &Envelope) -> VariantRow {
    let start = Instant::now();

    // `catch_unwind` needs `AssertUnwindSafe` because `BRepModel` is not
    // `UnwindSafe`; we build it fresh inside the closure and discard it on the
    // way out, so no observer sees a poisoned value — the assertion holds.
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut model = BRepModel::new();
        match build_variant(&mut model, params) {
            Ok(variant) => {
                let verdict = certify_variant(&mut model, &variant, envelope);
                EvalInner::Built {
                    sound: verdict.cert_sound,
                    failed_dimensions: verdict.failed_dimensions,
                    wall_material_volume: verdict.wall_material_volume,
                    internal_volume: verdict.internal_volume,
                    in_envelope: verdict.in_envelope,
                }
            }
            Err(refusal) => EvalInner::Refused(refusal),
        }
    }));

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis();
    let over_budget = elapsed > PER_VARIANT_BUDGET;

    let (outcome, wall_material_volume, internal_volume, in_envelope) = match result {
        Ok(EvalInner::Refused(r)) => (VariantOutcome::Refused(r), None, None, false),
        Ok(EvalInner::Built {
            sound,
            failed_dimensions,
            wall_material_volume,
            internal_volume,
            in_envelope,
        }) => {
            let outcome = if over_budget {
                VariantOutcome::TimedOut
            } else if sound {
                VariantOutcome::Sound
            } else {
                VariantOutcome::CertKilled(failed_dimensions)
            };
            (outcome, wall_material_volume, internal_volume, in_envelope)
        }
        Err(_) => {
            // A panic escaped build/certify. Record honestly; do not re-raise.
            let outcome = if over_budget {
                VariantOutcome::TimedOut
            } else {
                VariantOutcome::Panicked("build/certify panicked".to_string())
            };
            (outcome, None, None, false)
        }
    };

    VariantRow {
        label: params.label(),
        params: params.clone(),
        outcome,
        wall_material_volume,
        internal_volume,
        in_envelope,
        elapsed_ms,
    }
}

/// Internal carrier for the caught closure's result (keeps `catch_unwind`'s
/// return type simple).
enum EvalInner {
    Refused(VariantRefusal),
    Built {
        sound: bool,
        failed_dimensions: Vec<&'static str>,
        wall_material_volume: Option<f64>,
        internal_volume: Option<f64>,
        in_envelope: bool,
    },
}

/// Run a full sweep: sample `n` variants from `seed`, evaluate each on a fresh
/// model in parallel across `threads` rayon workers, and aggregate. Uses the
/// tuned default ranges and a default envelope sized to admit healthy variants.
pub fn explore(n: usize, seed: u64, threads: usize) -> ExplorationReport {
    explore_with(
        n,
        seed,
        threads,
        &SamplerRanges::default(),
        default_envelope(),
    )
}

/// A default design envelope generous enough to admit the healthy interior of
/// the default ranges (max chamber_r 22 + max wall_t 9 = 31 → dia 62; plus
/// margin), while still failing the extreme tails.
pub fn default_envelope() -> Envelope {
    Envelope {
        max_diameter: 80.0,
        max_length: 600.0,
    }
}

/// The general sweep entry point: explicit ranges and envelope. Each variant is
/// built on its own fresh [`BRepModel`] inside the rayon map — there is no
/// shared model, so the sweep is embarrassingly parallel and free of the
/// single-model serialization the api-server would impose.
pub fn explore_with(
    n: usize,
    seed: u64,
    threads: usize,
    ranges: &SamplerRanges,
    envelope: Envelope,
) -> ExplorationReport {
    let params = sample_params(n, seed, ranges);
    let start = Instant::now();

    // A local rayon pool bounds parallelism to `threads` without touching the
    // global pool (so callers who set their own pool are unaffected). If pool
    // construction fails, fall back to the global pool — the sweep still runs.
    let rows: Vec<VariantRow> = match rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
    {
        Ok(pool) => pool.install(|| {
            params
                .par_iter()
                .map(|p| evaluate_one(p, &envelope))
                .collect()
        }),
        Err(_) => params
            .par_iter()
            .map(|p| evaluate_one(p, &envelope))
            .collect(),
    };

    let total_ms = start.elapsed().as_millis();

    let mut refused = 0usize;
    let mut cert_killed = 0usize;
    let mut sound = 0usize;
    let mut timed_out = 0usize;
    let mut panicked = 0usize;
    let mut sum_variant_ms = 0u128;
    for row in &rows {
        sum_variant_ms += row.elapsed_ms;
        match &row.outcome {
            VariantOutcome::Refused(_) => refused += 1,
            VariantOutcome::CertKilled(_) => cert_killed += 1,
            VariantOutcome::Sound => sound += 1,
            VariantOutcome::TimedOut => timed_out += 1,
            VariantOutcome::Panicked(_) => panicked += 1,
        }
    }
    let mean_variant_ms = if rows.is_empty() {
        0.0
    } else {
        sum_variant_ms as f64 / rows.len() as f64
    };

    ExplorationReport {
        rows,
        refused,
        cert_killed,
        sound,
        timed_out,
        panicked,
        timings: SweepTimings {
            total_ms,
            sum_variant_ms,
            mean_variant_ms,
            threads: threads.max(1),
        },
        envelope,
    }
}

/// The band, as a fraction, within which a variant's internal cavity volume must
/// sit relative to the target to be a valid candidate (design §4: ±2%). Used by
/// the ranking helper.
pub const INTERNAL_VOLUME_BAND: f64 = 0.02;

/// Rank the SOUND, in-envelope, internal-volume-banded rows by minimum
/// wall-material volume (the honest objective). Returns candidate rows ordered
/// best-first (least wall material).
///
/// * Only [`VariantOutcome::Sound`] rows are eligible — refused, cert-killed,
///   timed-out and panicked variants are excluded by construction.
/// * The row must fit the envelope and have a computable wall-material volume.
/// * The internal cavity volume must sit within `INTERNAL_VOLUME_BAND` of
///   `internal_target` (a fixed internal volume is the design's equality
///   constraint; without it "minimize wall material" trivially favours tiny
///   engines). Pass `None` for `internal_target` to skip the band (rank purely
///   by wall material among sound in-envelope variants).
pub fn rank_candidates(rows: &[VariantRow], internal_target: Option<f64>) -> Vec<&VariantRow> {
    let mut candidates: Vec<&VariantRow> = rows
        .iter()
        .filter(|row| matches!(row.outcome, VariantOutcome::Sound))
        .filter(|row| row.in_envelope)
        .filter(|row| row.wall_material_volume.is_some())
        .filter(|row| match (internal_target, row.internal_volume) {
            (None, _) => true,
            (Some(target), Some(v)) => {
                target > 0.0 && (v - target).abs() <= INTERNAL_VOLUME_BAND * target
            }
            (Some(_), None) => false,
        })
        .collect();

    candidates.sort_by(|a, b| {
        let va = a.wall_material_volume.unwrap_or(f64::INFINITY);
        let vb = b.wall_material_volume.unwrap_or(f64::INFINITY);
        va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates
}

/// The single best candidate (least wall material), if any. Convenience over
/// [`rank_candidates`].
pub fn winner(rows: &[VariantRow], internal_target: Option<f64>) -> Option<&VariantRow> {
    rank_candidates(rows, internal_target).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Sampler determinism ------------------------------------------------

    #[test]
    fn same_seed_same_params() {
        let a = sample_default(32, 42);
        let b = sample_default(32, 42);
        assert_eq!(a, b, "same (n, seed) must reproduce identical params");
    }

    #[test]
    fn different_seed_differs() {
        let a = sample_default(32, 42);
        let b = sample_default(32, 43);
        assert_ne!(a, b, "different seeds should (overwhelmingly) differ");
    }

    #[test]
    fn prefix_is_stable() {
        // The first k of an n-sample equals a k-sample (stream is prefix-stable).
        let big = sample_default(64, 7);
        let small = sample_default(16, 7);
        assert_eq!(
            &big[..16],
            &small[..],
            "sample stream must be prefix-stable"
        );
    }

    #[test]
    fn ranges_are_respected() {
        let r = SamplerRanges::default();
        let params = sample_params(500, 12345, &r);
        for p in &params {
            assert!(
                p.throat_r >= r.throat_r.lo && p.throat_r <= r.throat_r.hi,
                "throat_r {} out of range",
                p.throat_r
            );
            assert!(
                p.expansion_ratio >= r.expansion_ratio.lo
                    && p.expansion_ratio <= r.expansion_ratio.hi
            );
            assert!(p.chamber_r >= r.chamber_r.lo && p.chamber_r <= r.chamber_r.hi);
            assert!(
                p.chamber_l_over_d >= r.chamber_l_over_d.lo
                    && p.chamber_l_over_d <= r.chamber_l_over_d.hi
            );
            assert!(p.wall_t >= r.wall_t.lo && p.wall_t <= r.wall_t.hi);
            let hc = p.hole_count as f64;
            assert!(
                hc >= r.hole_count.lo.floor() && hc <= r.hole_count.hi.ceil(),
                "hole_count {} out of range",
                p.hole_count
            );
            assert!(p.hole_r >= r.hole_r.lo && p.hole_r <= r.hole_r.hi);
            assert!(p.ring_frac >= r.ring_frac.lo && p.ring_frac <= r.ring_frac.hi);
        }
    }

    #[test]
    fn count_matches_request() {
        assert_eq!(sample_default(0, 1).len(), 0);
        assert_eq!(sample_default(1, 1).len(), 1);
        assert_eq!(sample_default(37, 1).len(), 37);
    }

    // ---- Ranking ------------------------------------------------------------

    /// Build a synthetic row with a chosen outcome and measures. Params are a
    /// placeholder; only the outcome/volume/envelope fields drive ranking.
    fn row(
        label: &str,
        outcome: VariantOutcome,
        wall: Option<f64>,
        internal: Option<f64>,
        in_env: bool,
    ) -> VariantRow {
        VariantRow {
            label: label.to_string(),
            params: EngineParams {
                throat_r: 5.0,
                expansion_ratio: 8.0,
                chamber_r: 15.0,
                chamber_l_over_d: 1.0,
                wall_t: 2.0,
                hole_count: 6,
                hole_r: 2.0,
                ring_frac: 0.6,
            },
            outcome,
            wall_material_volume: wall,
            internal_volume: internal,
            in_envelope: in_env,
            elapsed_ms: 1,
        }
    }

    #[test]
    fn ranking_picks_least_wall_material() {
        let rows = vec![
            row(
                "heavy",
                VariantOutcome::Sound,
                Some(300.0),
                Some(1000.0),
                true,
            ),
            row(
                "light",
                VariantOutcome::Sound,
                Some(100.0),
                Some(1000.0),
                true,
            ),
            row(
                "medium",
                VariantOutcome::Sound,
                Some(200.0),
                Some(1000.0),
                true,
            ),
        ];
        let w = winner(&rows, Some(1000.0)).expect("a sound candidate exists");
        assert_eq!(w.label, "light");
        let ranked = rank_candidates(&rows, Some(1000.0));
        let labels: Vec<&str> = ranked.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["light", "medium", "heavy"]);
    }

    #[test]
    fn ranking_excludes_unsound_and_refused() {
        let rows = vec![
            // Least wall material BUT unsound → excluded.
            row(
                "unsound_light",
                VariantOutcome::CertKilled(vec!["watertight"]),
                Some(10.0),
                Some(1000.0),
                true,
            ),
            // Least wall material BUT refused → excluded.
            row(
                "refused_light",
                VariantOutcome::Refused(VariantRefusal::OverlappingHoles("x".into())),
                Some(20.0),
                Some(1000.0),
                true,
            ),
            // Sound but out of envelope → excluded.
            row(
                "out_of_env",
                VariantOutcome::Sound,
                Some(30.0),
                Some(1000.0),
                false,
            ),
            // The only eligible one.
            row(
                "winner",
                VariantOutcome::Sound,
                Some(250.0),
                Some(1000.0),
                true,
            ),
        ];
        let w = winner(&rows, Some(1000.0)).expect("one eligible candidate");
        assert_eq!(w.label, "winner");
        assert_eq!(rank_candidates(&rows, Some(1000.0)).len(), 1);
    }

    #[test]
    fn ranking_enforces_internal_volume_band() {
        let rows = vec![
            // Inside ±2% of 1000 → eligible.
            row(
                "in_band",
                VariantOutcome::Sound,
                Some(100.0),
                Some(1015.0),
                true,
            ),
            // Least wall BUT internal volume way off → excluded by band.
            row(
                "off_band",
                VariantOutcome::Sound,
                Some(50.0),
                Some(1500.0),
                true,
            ),
        ];
        let w = winner(&rows, Some(1000.0)).expect("the in-band variant");
        assert_eq!(w.label, "in_band");
        assert_eq!(rank_candidates(&rows, Some(1000.0)).len(), 1);
    }

    #[test]
    fn ranking_without_target_ranks_all_sound_in_envelope() {
        let rows = vec![
            row("a", VariantOutcome::Sound, Some(200.0), None, true),
            row("b", VariantOutcome::Sound, Some(100.0), None, true),
        ];
        // No internal target → band skipped, pure wall-material ordering.
        let ranked = rank_candidates(&rows, None);
        let labels: Vec<&str> = ranked.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["b", "a"]);
    }

    #[test]
    fn empty_rows_have_no_winner() {
        assert!(winner(&[], Some(1000.0)).is_none());
        assert!(winner(&[], None).is_none());
    }
}
