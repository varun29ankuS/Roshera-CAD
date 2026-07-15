// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Move 3 gates G1 + G2 for the parametric rocket-engine variant recipe.
//!
//! * G1 — one known-good parameter set builds, certifies SOUND on every
//!   dimension, yields positive wall + internal volumes in a sane band, fits its
//!   envelope, and renders to a valid PNG.
//! * G2 — three deliberately-bad parameter sets prove the kill mechanisms are
//!   AUTHENTIC (a genuine geometry failure the certificate/op catches, not a
//!   scripted verdict). Each runs under a soft time budget so a boolean hang is a
//!   reported BLOCKED finding, never a silent freeze.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use geometry_engine::harness::engine_variant::{
    build_variant, certify_variant, EngineParams, Envelope,
};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::render::{render_solids_dir, RenderOptions};

/// Per-variant soft budget. A boolean that blows past this is reported BLOCKED.
const BUDGET: Duration = Duration::from_secs(120);

/// Owned, `Send` summary of a build+certify run (crosses the guard thread).
#[derive(Debug, Clone)]
struct Outcome {
    /// Op-level refusal (built nothing), if any.
    refusal: Option<String>,
    /// Certificate soundness (both solids), when built.
    sound: Option<bool>,
    /// Failed cert dimensions, when built.
    failed: Vec<String>,
    wall_volume: Option<f64>,
    internal_volume: Option<f64>,
    in_envelope: Option<bool>,
    /// PNG bytes of the composed render, when requested + built.
    png: Option<Vec<u8>>,
}

/// Run `f` on a worker thread with a hard-ish wall-clock budget. `None` means the
/// worker did not answer in time (a BLOCKED finding — the kernel likely hung).
fn run_guarded<T, F>(budget: Duration, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(budget).ok()
}

/// Build + certify one variant in a fresh model; optionally render it. Returns an
/// owned [`Outcome`] safe to send across the guard thread.
fn evaluate(p: EngineParams, envelope: Envelope, render: bool) -> Outcome {
    let mut model = BRepModel::new();
    match build_variant(&mut model, &p) {
        Err(refusal) => Outcome {
            refusal: Some(format!("{refusal:?}")),
            sound: None,
            failed: Vec::new(),
            wall_volume: None,
            internal_volume: None,
            in_envelope: None,
            png: None,
        },
        Ok(variant) => {
            let verdict = certify_variant(&mut model, &variant, &envelope);
            let png = if render {
                // Copper chamber+nozzle, steel injector plate.
                let colors: [[u8; 3]; 2] = [[184, 115, 51], [176, 181, 189]];
                let ids = [variant.chamber_nozzle, variant.injector_plate];
                let opts = RenderOptions {
                    width: 800,
                    height: 800,
                    ..RenderOptions::default()
                };
                render_solids_dir(
                    &model,
                    &ids,
                    &colors,
                    geometry_engine::Vector3::new(-1.0, -1.0, -0.6),
                    geometry_engine::Vector3::Z,
                    &opts,
                )
                .and_then(|frame| frame.to_png().ok())
            } else {
                None
            };
            Outcome {
                refusal: None,
                sound: Some(verdict.cert_sound),
                failed: verdict
                    .failed_dimensions
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                wall_volume: verdict.wall_material_volume,
                internal_volume: verdict.internal_volume,
                in_envelope: Some(verdict.in_envelope),
                png: png,
            }
        }
    }
}

/// The known-good nominal variant.
fn nominal() -> EngineParams {
    EngineParams {
        throat_r: 8.0,
        expansion_ratio: 6.0,
        chamber_r: 20.0,
        chamber_l_over_d: 1.0,
        wall_t: 2.0,
        hole_count: 8,
        hole_r: 1.5,
        ring_frac: 0.6,
    }
}

const PNG_MAGIC: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

// ---------------------------------------------------------------------------
// G1 — single variant end-to-end
// ---------------------------------------------------------------------------

#[test]
fn g1_nominal_variant_sound_measured_and_rendered() {
    let p = nominal();
    assert!(!p.label().is_empty(), "G1: variant label must be non-empty");
    eprintln!("[G1] variant label: {}", p.label());
    let envelope = Envelope {
        max_diameter: 60.0,
        max_length: 160.0,
    };

    let t0 = Instant::now();
    let out = run_guarded(BUDGET, move || evaluate(p, envelope, true))
        .expect("G1 nominal variant timed out (BLOCKED — kernel hang)");
    let elapsed = t0.elapsed();
    eprintln!("[G1] wall-clock: {:.2}s", elapsed.as_secs_f64());

    assert!(
        out.refusal.is_none(),
        "G1: nominal variant was REFUSED: {:?}",
        out.refusal
    );
    assert_eq!(
        out.sound,
        Some(true),
        "G1: nominal variant NOT sound — failed dimensions: {:?}",
        out.failed
    );
    assert!(
        out.failed.is_empty(),
        "G1: expected zero failed dimensions, got {:?}",
        out.failed
    );

    // Volumes present + positive.
    let wall = out
        .wall_volume
        .expect("G1: wall-material volume must be Some");
    let internal = out
        .internal_volume
        .expect("G1: internal volume must be Some");
    assert!(wall > 0.0, "G1: wall volume must be positive, got {wall}");
    assert!(
        internal > 0.0,
        "G1: internal volume must be positive, got {internal}"
    );

    // Loose analytic sanity band for the internal cavity: chamber cylinder + two
    // frustums (converging + diverging). Just catches nonsense, not tail digits.
    let rc = 20.0_f64;
    let rt = 8.0_f64;
    let re = rt * 6.0_f64.sqrt();
    let lc = 1.0 * 2.0 * rc;
    let lconv = (rc - rt).abs();
    let ldiv = 2.0 * (re - rt).abs();
    let pi = std::f64::consts::PI;
    let est = pi * rc * rc * lc
        + pi * lconv / 3.0 * (rc * rc + rc * rt + rt * rt)
        + pi * ldiv / 3.0 * (rt * rt + rt * re + re * re);
    assert!(
        internal > 0.4 * est && internal < 2.5 * est,
        "G1: internal volume {internal:.1} outside sanity band [{:.1}, {:.1}] (est {est:.1})",
        0.4 * est,
        2.5 * est
    );

    assert_eq!(
        out.in_envelope,
        Some(true),
        "G1: nominal variant must fit its envelope"
    );

    // Render: valid PNG, correct dimensions.
    let png = out.png.expect("G1: render must produce a PNG");
    assert!(
        png.len() > 1000,
        "G1: PNG suspiciously small ({} bytes)",
        png.len()
    );
    assert_eq!(png[0..8], PNG_MAGIC, "G1: PNG signature mismatch");
    // IHDR width/height (big-endian u32 at byte offsets 16 and 20).
    let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    assert_eq!((w, h), (800, 800), "G1: PNG dimensions wrong");

    eprintln!(
        "[G1] SOUND wall_vol={:.1} internal_vol={:.1} in_env={:?} png={}B",
        wall,
        internal,
        out.in_envelope,
        png.len()
    );
}

// ---------------------------------------------------------------------------
// G2 — kill authenticity
// ---------------------------------------------------------------------------

/// Big envelope so ENVELOPE never masks a geometry failure in the kill probes.
fn open_envelope() -> Envelope {
    Envelope {
        max_diameter: 1.0e6,
        max_length: 1.0e6,
    }
}

/// Kill 1 — overlapping injector holes (n·rh too big for the ring). Expected: a
/// built-but-CERT-KILLED variant (intersecting drilled bores → cyl-cyl saddle →
/// open/non-manifold topology), OR a typed refusal if the boolean rejects it.
/// This is the mandatory cert-kill of the trio.
#[test]
fn g2_kill_overlapping_holes() {
    let p = EngineParams {
        throat_r: 8.0,
        expansion_ratio: 6.0,
        chamber_r: 20.0,
        chamber_l_over_d: 1.0,
        wall_t: 2.0,
        hole_count: 12,
        hole_r: 4.0,
        ring_frac: 0.35,
    };
    let env = open_envelope();
    let t0 = Instant::now();
    let out = run_guarded(BUDGET, move || evaluate(p, env, false));
    let out = match out {
        Some(o) => o,
        None => panic!("[G2 overlap] BLOCKED — kernel did not finish within {BUDGET:?} (hang)"),
    };
    eprintln!(
        "[G2 overlap] {:.2}s refusal={:?} sound={:?} failed={:?}",
        t0.elapsed().as_secs_f64(),
        out.refusal,
        out.sound,
        out.failed
    );

    // Honest outcome: either REFUSED or CERT-KILLED (not sound). A sound result
    // would mean the overlap was NOT a real failure — that is the thing under test.
    if let Some(reason) = &out.refusal {
        // The refusal must be the OverlappingHoles guard specifically — any other
        // refusal variant firing here would mean the overlap regime was rejected
        // for the wrong reason (review finding: an assertion-free branch could
        // pass silently on e.g. InvalidParams).
        assert!(
            reason.contains("OverlappingHoles"),
            "[G2 overlap] expected the OverlappingHoles guard, got refusal: {reason}"
        );
        eprintln!("[G2 overlap] outcome=REFUSED reason={reason}");
    } else {
        assert_eq!(
            out.sound,
            Some(false),
            "[G2 overlap] overlapping holes should NOT certify sound"
        );
        // The design predicts watertight and/or manifold in the failed set.
        assert!(
            out.failed
                .iter()
                .any(|d| d == "watertight" || d == "manifold"),
            "[G2 overlap] expected watertight/manifold failure, got {:?}",
            out.failed
        );
        eprintln!("[G2 overlap] outcome=CERT-KILLED dims={:?}", out.failed);
    }
}

/// Kill 2 — wall fold-through at a tight throat (wall_t too thick for throat_r).
/// The design predicts a self_intersection_free failure. Assert a GENUINE failure
/// (refused OR unsound) and record the actual class.
#[test]
fn g2_kill_wall_fold_through() {
    let p = EngineParams {
        throat_r: 1.5,
        expansion_ratio: 8.0,
        chamber_r: 4.0,
        chamber_l_over_d: 1.5,
        wall_t: 12.0,
        hole_count: 6,
        hole_r: 0.6,
        ring_frac: 0.5,
    };
    let env = open_envelope();
    let t0 = Instant::now();
    let out = run_guarded(BUDGET, move || evaluate(p, env, false));
    let out = match out {
        Some(o) => o,
        None => panic!("[G2 fold] BLOCKED — kernel did not finish within {BUDGET:?} (hang)"),
    };
    eprintln!(
        "[G2 fold] {:.2}s refusal={:?} sound={:?} failed={:?}",
        t0.elapsed().as_secs_f64(),
        out.refusal,
        out.sound,
        out.failed
    );

    if let Some(reason) = &out.refusal {
        eprintln!("[G2 fold] outcome=REFUSED reason={reason}");
    } else {
        assert_eq!(
            out.sound,
            Some(false),
            "[G2 fold] a fold-through wall should NOT certify sound (failed={:?})",
            out.failed
        );
        eprintln!("[G2 fold] outcome=CERT-KILLED dims={:?}", out.failed);
    }
}

/// Kill 3 — rim-grazing holes (ring_frac ≈ 1): the ring pushes the bores off the
/// plate rim → sliver / open boundary. Assert a GENUINE failure (refused OR
/// unsound) and record the actual class.
#[test]
fn g2_kill_rim_grazing_holes() {
    let p = EngineParams {
        throat_r: 8.0,
        expansion_ratio: 6.0,
        chamber_r: 20.0,
        chamber_l_over_d: 1.0,
        wall_t: 2.0,
        hole_count: 8,
        hole_r: 1.5,
        ring_frac: 0.99,
    };
    let env = open_envelope();
    let t0 = Instant::now();
    let out = run_guarded(BUDGET, move || evaluate(p, env, false));
    let out = match out {
        Some(o) => o,
        None => panic!("[G2 rim] BLOCKED — kernel did not finish within {BUDGET:?} (hang)"),
    };
    eprintln!(
        "[G2 rim] {:.2}s refusal={:?} sound={:?} failed={:?}",
        t0.elapsed().as_secs_f64(),
        out.refusal,
        out.sound,
        out.failed
    );

    if let Some(reason) = &out.refusal {
        eprintln!("[G2 rim] outcome=REFUSED reason={reason}");
    } else {
        assert_eq!(
            out.sound,
            Some(false),
            "[G2 rim] rim-grazing holes should NOT certify sound (failed={:?})",
            out.failed
        );
        eprintln!("[G2 rim] outcome=CERT-KILLED dims={:?}", out.failed);
    }
}
