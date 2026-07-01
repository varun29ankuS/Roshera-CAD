//! Async dual-eye reconcile worker. Runs the heavy cross-eye reconcile
//! (truth certificate · scene render · semantic features) OFF the model
//! write lock, on an owned snapshot — the precise inverse of the auto-cert
//! regression that froze the backend by running certification synchronously
//! under the write lock.
//!
//! Freeze avoidance (the whole point):
//!   1. Take a BRIEF read lock and deep-copy the model into a `ModelSnapshot`
//!      (O(topology), sub-ms) — the ONLY lock hold.
//!   2. DROP the guard, restore the snapshot into an owned `BRepModel`.
//!   3. Render N viewpoints + certify + recognize features on the owned
//!      snapshot with NO lock held.
//!
//! `BRepModel` is not `Clone` and the model lives behind a `tokio::sync::RwLock`,
//! so we cannot naively `guard.clone()`; `ModelSnapshot::take`/`restore` is the
//! kernel's own deep-copy primitive and is the supported way to obtain an
//! independent, renderable model.
//!
//! Machine safety: a global semaphore caps concurrent reconciles at
//! [`MAX_CONCURRENT_RECONCILES`]. A burst of mutating ops therefore never spawns
//! many concurrent multi-viewpoint renders; excess ops SKIP (the report stays
//! `Pending` and is recomputed on a later op/read — it is advisory).

use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{RwLock, Semaphore};
use tracing::debug;

use geometry_engine::math::Vector3;
use geometry_engine::perception::reconcile::{reconcile_full, ReconcileReport};
use geometry_engine::primitives::feature_recognition::recognize_features;
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::render::{render_solids_dir, RenderFrame, RenderMode, RenderOptions};
use geometry_engine::tessellation::TessellationParams;

/// Shared model handle (the ACTIVE model — may be a branch model, not
/// `AppState::model`, so it must be passed in explicitly).
pub type ModelHandle = Arc<RwLock<BRepModel>>;
/// Completed reports, keyed by `(solid_id, cert_fingerprint)`.
pub type ReconcileCache = Arc<DashMap<(u32, u64), Arc<ReconcileReport>>>;
/// In-flight guard set, keyed identically to the cache.
pub type ReconcileInflight = Arc<DashMap<(u32, u64), ()>>;
/// Global concurrency limiter (see [`MAX_CONCURRENT_RECONCILES`]).
pub type ReconcileLimiter = Arc<Semaphore>;

/// Viewpoints on the sphere. 14 (not 26) for perf/machine-safety on Move 1:
/// the reconcile only needs face-id legends + edge counts, not a dense eye.
const VIEWPOINTS: u32 = 14;

/// At most this many reconcile tasks run concurrently. Concurrent compute
/// bursts are the machine-safety hazard this cap exists to remove.
pub const MAX_CONCURRENT_RECONCILES: usize = 2;

/// Near-uniform unit directions on the sphere (no clustering). Fibonacci-sphere
/// sampling (Saff–Kuijlaars / González) — the standard even-coverage NBV seed.
/// Every returned vector is unit length.
pub fn fibonacci_sphere(n: u32) -> Vec<Vector3> {
    let mut out = Vec::with_capacity(n as usize);
    // Golden angle ≈ 2.399963 rad.
    let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let nf = n as f64;
    for i in 0..n {
        let fi = i as f64;
        // y sweeps 1 → -1; guard n == 1 against a zero denominator.
        let y = 1.0 - (fi / (nf - 1.0).max(1.0)) * 2.0;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * fi;
        out.push(Vector3::new(theta.cos() * r, y, theta.sin() * r));
    }
    out
}

/// An all-zero diagnostic frame — the honest "nothing rendered" fallback when
/// `render_solids_dir` returns `None` (missing / empty solid). Never panics.
fn empty_frame() -> RenderFrame {
    RenderFrame {
        width: 0,
        height: 0,
        pixels: Vec::new(),
        face_legend: Vec::new(),
        open_edges: 0,
        nonmanifold_edges: 0,
    }
}

/// Every live face id of `solid_id` in `model` (outer + void shells).
fn live_face_ids(model: &BRepModel, solid_id: u32) -> HashSet<u32> {
    model
        .solids
        .get(solid_id)
        .map(|solid| {
            solid
                .all_shells()
                .iter()
                .filter_map(|shell_id| model.shells.get(*shell_id))
                .flat_map(|shell| shell.face_ids().iter().copied())
                .collect()
        })
        .unwrap_or_default()
}

/// Fire-and-forget: snapshot the solid off the write lock, render
/// [`VIEWPOINTS`] viewpoints, reconcile the three eyes, and cache the report.
///
/// Skips (no spawn) when the report is already cached or in flight for this
/// exact `(solid_id, fingerprint)`, or when no concurrency permit is available.
/// A skip is not an error — the report stays `Pending` and recomputes on a
/// later op/read.
pub fn spawn_reconcile(
    model: ModelHandle,
    cache: ReconcileCache,
    inflight: ReconcileInflight,
    limiter: ReconcileLimiter,
    solid_id: u32,
    fingerprint: u64,
) {
    let key = (solid_id, fingerprint);
    if cache.contains_key(&key) {
        return; // already reconciled for this exact state
    }
    // Machine-safety cap FIRST: if two reconciles are already running, skip.
    let permit = match limiter.try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            debug!(
                solid_id,
                fingerprint, "reconcile skipped: concurrency cap reached (advisory, will retry)"
            );
            return;
        }
    };
    // Claim the in-flight slot; a racing spawn for the same key backs off and
    // returns the permit by dropping it here.
    if inflight.insert(key, ()).is_some() {
        return;
    }
    // Spawning requires a Tokio runtime. Every mutating handler runs on one;
    // guard anyway so a non-runtime caller (e.g. a unit test) degrades to a
    // no-op instead of panicking.
    if tokio::runtime::Handle::try_current().is_err() {
        inflight.remove(&key);
        return;
    }

    tokio::task::spawn_blocking(move || {
        // Hold the permit for the whole task; released on drop.
        let _permit = permit;
        let started = std::time::Instant::now();

        // --- snapshot phase: BRIEF read lock, deep-copy, DROP the guard ---
        // `blocking_read` is correct here: this closure runs on a blocking
        // thread (spawn_blocking), never on an async worker.
        let snap = {
            let guard = model.blocking_read();
            ModelSnapshot::take(&guard)
        };
        // Restore into an independent model; all heavy work below is lock-free.
        let mut owned = BRepModel::new();
        snap.restore(&mut owned);

        let live = live_face_ids(&owned, solid_id);
        // `certify_solid` needs `&mut` (it warms per-face caches); the geometry
        // is never mutated. Runs on the owned snapshot, off any lock.
        let cert = owned.certify_solid(solid_id);
        let features = recognize_features(solid_id, &owned);

        // COARSE tessellation: reconcile needs face-id legends + edge counts,
        // not a display-fine mesh.
        let coarse = TessellationParams::coarse();
        let up = Vector3::new(0.0, 1.0, 0.0);
        let alt_up = Vector3::new(1.0, 0.0, 0.0);

        let id_opts = RenderOptions {
            mode: RenderMode::FaceIds,
            tessellation: coarse.clone(),
            ..RenderOptions::default()
        };
        let mut faceid_frames: Vec<RenderFrame> = Vec::with_capacity(VIEWPOINTS as usize);
        for dir in fibonacci_sphere(VIEWPOINTS) {
            // Resolve pole degeneracy: swap to +X up when the view axis is
            // (anti)parallel to +Y.
            let up_hint = if dir.cross(&up).magnitude() < 1e-6 {
                alt_up
            } else {
                up
            };
            if let Some(frame) = render_solids_dir(&owned, &[solid_id], &[], dir, up_hint, &id_opts)
            {
                faceid_frames.push(frame);
            }
        }

        let diag_opts = RenderOptions {
            mode: RenderMode::Diagnostic,
            tessellation: coarse,
            ..RenderOptions::default()
        };
        let diagnostic_frame = render_solids_dir(
            &owned,
            &[solid_id],
            &[],
            Vector3::new(1.0, 1.0, 1.0),
            up,
            &diag_opts,
        )
        .unwrap_or_else(empty_frame);

        let report = reconcile_full(
            solid_id,
            fingerprint,
            &live,
            &cert,
            &features,
            &faceid_frames,
            &diagnostic_frame,
            VIEWPOINTS,
            started.elapsed().as_millis() as u64,
        );
        cache.insert(key, Arc::new(report));
        inflight.remove(&key);
    });
}

#[cfg(test)]
mod viewpoint_tests {
    use super::*;

    #[test]
    fn fibonacci_sphere_count_and_unit_length() {
        let dirs = fibonacci_sphere(14);
        assert_eq!(dirs.len(), 14);
        for d in &dirs {
            assert!(
                (d.magnitude() - 1.0).abs() < 1e-9,
                "viewpoints must be unit vectors"
            );
        }
    }

    #[test]
    fn fibonacci_sphere_single_point_is_unit() {
        // n == 1 must not divide by zero and must still be unit length.
        let dirs = fibonacci_sphere(1);
        assert_eq!(dirs.len(), 1);
        assert!((dirs[0].magnitude() - 1.0).abs() < 1e-9);
    }
}
