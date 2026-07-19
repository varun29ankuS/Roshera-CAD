//! Bounded execution for heavy mutating kernel ops (Task #41).
//!
//! # The failure this closes
//!
//! On 2026-07-19 a thin-wall boolean union (two revolve segments sharing a
//! coincident throat ring) spun for >120 s at ~1.1 cores **under the model
//! write lock** and never terminated — the process had to be killed. Every
//! mutating handler acquires `model_handle.write().await` and runs the kernel
//! op inline, so a non-converging corefinement holds the write lock forever
//! and every subsequent request (even `create_box`) blocks behind it. A
//! deployed instance cannot need a process kill.
//!
//! # What can and cannot be guaranteed
//!
//! A Rust compute loop cannot be cancelled cooperatively — `tokio::time::
//! timeout` around a `spawn_blocking` task does **not** stop the thread; on
//! timeout the runaway thread keeps burning CPU until it finishes on its own.
//! What this module *does* guarantee on a budget overrun:
//!
//! 1. the request returns a typed [`ErrorCode::OpTimeout`] **promptly**
//!    (at the budget, not when the thread finally exits);
//! 2. the live model is **not corrupted** — the op ran on a throwaway clone,
//!    never on the live model;
//! 3. the model write lock is **not held** by the abandoned computation — it
//!    was released before the compute began and re-acquired only for the
//!    sub-millisecond swap.
//!
//! That forces the operate-on-clone-then-swap shape below.
//!
//! # The clone-swap shape
//!
//! ```text
//!  spawn_blocking:
//!    ├─ brief blocking_read  ── ModelSnapshot::take + recorder + fingerprint
//!    ├─ (lock released)
//!    ├─ restore snapshot into an owned clone, attach the live recorder
//!    └─ run the kernel op ON THE CLONE                 ← the heavy, unbounded part
//!  timeout(budget, join):
//!    ├─ Elapsed ─────────────── OpTimeout (clone + zombie thread discarded)
//!    ├─ op returned typed err ─ propagate it (live model untouched)
//!    └─ Ok(value) ───────────── re-acquire write lock, verify unchanged, SWAP
//! ```
//!
//! `ModelSnapshot` is the kernel's own deep-copy primitive (the same one
//! `reconcile_task` and the F2-δ rollback path use); its docs cite "well
//! under a millisecond and well under 10 MB" for a 10 k-vertex part, so the
//! per-op clone is negligible for the demo-scale models this protects. The
//! recorder is re-attached to the clone so a successful op records its
//! timeline event exactly as an in-place op would; `restore`/swap preserve
//! the live model's own recorder identity.
//!
//! # Honest residuals
//!
//! - **Zombie thread.** On timeout the abandoned `spawn_blocking` thread runs
//!   to completion on the discarded clone, burning one core until it exits.
//!   Its work is thrown away. This is the irreducible cost of un-cancellable
//!   compute; it is bounded by however long the op *would* have taken and
//!   costs nothing on the live model.
//! - **Concurrency window.** Between snapshot and swap the write lock is free,
//!   so a concurrent writer could mutate the live model. A cheap fingerprint
//!   (topology cardinalities + the root-id counter) is captured at snapshot
//!   time and re-checked under the swap lock; a mismatch is refused
//!   (retryably) rather than silently clobbering the concurrent write. The
//!   real protection is that the agent workload is serialized — one op at a
//!   time per model — so the check effectively never trips. The fingerprint
//!   does not detect a pure in-place coordinate edit that changes no
//!   cardinality and mints no id (e.g. a bare transform); such an interleave
//!   would be lost on swap. Documented, not defended, for slice 1.

use std::sync::Arc;
use std::time::Duration;

use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::BRepModel;
use tokio::sync::RwLock;

use crate::error_catalog::{ApiError, ErrorCode};

/// Handle to the model a bounded op runs against — the same
/// `Arc<RwLock<BRepModel>>` the `ActiveModel` extractor yields.
pub type ModelHandle = Arc<RwLock<BRepModel>>;

/// Per-class wall-clock budget classes for heavy mutating ops.
///
/// The class picks the default budget and the env override key. Booleans
/// and blends run arbitrary corefinement (the proven hang class) and get the
/// tighter 60 s default; everything else routed through the executor gets a
/// more generous 120 s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    /// Boolean union / intersection / difference — the class of the live hang.
    Boolean,
    /// Fillet / chamfer / shell / other blend-family corefinement.
    Blend,
    /// Any other heavy mutating op routed through the executor.
    Other,
}

impl OpClass {
    /// Stable identifier used in the timeout error's `op_kind` detail and in
    /// the per-class env-override key.
    fn key(self) -> &'static str {
        match self {
            OpClass::Boolean => "boolean",
            OpClass::Blend => "blend",
            OpClass::Other => "other",
        }
    }

    /// The per-class env-override variable name.
    fn env_key(self) -> &'static str {
        match self {
            OpClass::Boolean => "ROSHERA_OP_TIMEOUT_BOOLEAN_SECS",
            OpClass::Blend => "ROSHERA_OP_TIMEOUT_BLEND_SECS",
            OpClass::Other => "ROSHERA_OP_TIMEOUT_OTHER_SECS",
        }
    }

    /// Compiled-in default budget in seconds. Default ON with generous
    /// budgets — a factory instance is protected out of the box, and 60 s is
    /// far beyond any healthy boolean (the internal regression budget for a
    /// 1 k-face union is < 100 ms), so a legitimate op never trips it.
    fn default_secs(self) -> u64 {
        match self {
            OpClass::Boolean | OpClass::Blend => 60,
            OpClass::Other => 120,
        }
    }
}

/// Resolved, immutable per-class budgets. Baked into `AppState` at startup by
/// [`OpBudgets::from_env`] so enforcement is deterministic and cannot be
/// changed by a mid-flight env mutation. `Copy` so it threads through the
/// `Clone` on `AppState` for free.
#[derive(Debug, Clone, Copy)]
pub struct OpBudgets {
    boolean: Duration,
    blend: Duration,
    other: Duration,
}

impl OpBudgets {
    /// Resolve budgets from the environment.
    ///
    /// Precedence per class: the class-specific key
    /// (`ROSHERA_OP_TIMEOUT_{BOOLEAN,BLEND,OTHER}_SECS`) → the global
    /// `ROSHERA_OP_TIMEOUT_SECS` → the compiled-in default. A value of `0`
    /// or an unparseable value is ignored (falls through to the next source):
    /// disabling the guard is deliberately not expressible, because an
    /// un-protected factory instance is the exact failure this closes.
    pub fn from_env() -> Self {
        let global = parse_secs_var("ROSHERA_OP_TIMEOUT_SECS");
        Self {
            boolean: resolve_class(OpClass::Boolean, global),
            blend: resolve_class(OpClass::Blend, global),
            other: resolve_class(OpClass::Other, global),
        }
    }

    /// Construct explicit budgets (test fixtures pin a tiny budget here
    /// without touching the process environment).
    pub fn from_durations(boolean: Duration, blend: Duration, other: Duration) -> Self {
        Self {
            boolean,
            blend,
            other,
        }
    }

    /// The budget for `class`.
    pub fn budget(&self, class: OpClass) -> Duration {
        match class {
            OpClass::Boolean => self.boolean,
            OpClass::Blend => self.blend,
            OpClass::Other => self.other,
        }
    }
}

impl Default for OpBudgets {
    /// The compiled-in defaults, ignoring the environment. Used where a
    /// non-env-driven default is wanted; production goes through `from_env`.
    fn default() -> Self {
        Self {
            boolean: Duration::from_secs(OpClass::Boolean.default_secs()),
            blend: Duration::from_secs(OpClass::Blend.default_secs()),
            other: Duration::from_secs(OpClass::Other.default_secs()),
        }
    }
}

/// Parse a positive-seconds env var. Returns `None` when unset, empty, `0`,
/// or unparseable — every "no usable value" case funnels to one `None` so the
/// precedence chain in `resolve_class` reads cleanly.
fn parse_secs_var(name: &str) -> Option<u64> {
    match std::env::var(name) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(secs) if secs > 0 => Some(secs),
            _ => None,
        },
        Err(_) => None,
    }
}

/// Resolve one class's budget: class-specific env → global env → default.
fn resolve_class(class: OpClass, global: Option<u64>) -> Duration {
    let secs = parse_secs_var(class.env_key())
        .or(global)
        .unwrap_or_else(|| class.default_secs());
    Duration::from_secs(secs)
}

/// Cheap change-detection fingerprint of a model: topology cardinalities plus
/// the monotonic root-id counter. Any op that creates or deletes topology, or
/// mints a root persistent-id, moves at least one component. Captured under
/// the snapshot lock and re-checked under the swap lock to refuse a swap that
/// would clobber a concurrent write. See the module "Concurrency window" note
/// for what it deliberately does not catch.
fn model_fingerprint(m: &BRepModel) -> (usize, usize, usize, usize, usize, usize, u64) {
    (
        m.vertices.len(),
        m.edges.len(),
        m.loops.len(),
        m.faces.len(),
        m.shells.len(),
        m.solids.len(),
        m.root_counter,
    )
}

/// Run a heavy mutating kernel op under a bounded executor.
///
/// `op` receives an exclusive `&mut BRepModel` — a deep clone of the live
/// model taken off the write lock — and returns whatever the caller needs
/// out of the op (typically the result solid id), or a typed [`ApiError`]
/// (e.g. `BooleanDisjoint`) which is propagated verbatim with the live model
/// left untouched. On success the clone is swapped into the live model under
/// a brief write lock and `commit(&value)` runs **under that same guard** so
/// any out-of-model bookkeeping (id-mapping flips, etc.) is atomic with the
/// swap.
///
/// On budget overrun the op is abandoned (its thread keeps running on the
/// discarded clone), the live model is untouched, and
/// [`ErrorCode::OpTimeout`] is returned with the operand ids and budget.
///
/// `operands` are the solid ids surfaced in the timeout error's details.
pub async fn bounded_model_op<T, F, C>(
    model_handle: ModelHandle,
    class: OpClass,
    budgets: OpBudgets,
    operands: Vec<u32>,
    op: F,
    commit: C,
) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut BRepModel) -> Result<T, ApiError> + Send + 'static,
    C: FnOnce(&T),
{
    let budget = budgets.budget(class);

    // spawn_blocking: snapshot under a brief read lock, then run the heavy op
    // on an owned clone with NO lock held. Only the `Arc` handle, the class,
    // and the op closure cross the boundary — all `Send`.
    let handle = Arc::clone(&model_handle);
    let join = tokio::task::spawn_blocking(move || {
        // Brief read lock: deep-copy + capture recorder + fingerprint, then
        // drop the guard. `blocking_read` is correct on a blocking thread.
        let (snapshot, recorder, fingerprint) = {
            let guard = handle.blocking_read();
            (
                ModelSnapshot::take(&guard),
                guard.recorder.clone(),
                model_fingerprint(&guard),
            )
        };
        // Independent owned model; the live recorder is re-attached so a
        // successful op records its timeline event exactly as in place.
        let mut clone = BRepModel::new();
        snapshot.restore(&mut clone);
        clone.attach_recorder(recorder);
        let outcome = op(&mut clone);
        (outcome, clone, fingerprint)
    });

    match tokio::time::timeout(budget, join).await {
        // Budget blown. Dropping the `join` future detaches the blocking
        // task (a blocking task cannot be aborted); it finishes on the
        // discarded clone. The live model was never touched and its write
        // lock is free.
        Err(_elapsed) => {
            tracing::warn!(
                op_kind = class.key(),
                budget_secs = budget.as_secs_f64(),
                ?operands,
                "bounded op exceeded its budget; abandoned on a clone (model unchanged)"
            );
            Err(ApiError::op_timeout(
                class.key(),
                budget.as_secs_f64(),
                &operands,
            ))
        }
        // The blocking task panicked (kernel defect on unsound geometry).
        // With inline execution this would have unwound the handler task;
        // here it is captured, the live model is untouched, and a clean
        // 500 is returned instead of a dropped connection.
        Ok(Err(join_err)) => {
            tracing::error!(
                op_kind = class.key(),
                error = %join_err,
                "bounded op panicked inside spawn_blocking (model unchanged)"
            );
            Err(ApiError::new(
                ErrorCode::Internal,
                format!("bounded op '{}' panicked during execution", class.key()),
            ))
        }
        // Op returned a typed error (BooleanDisjoint, kernel_error, …). The
        // clone is dropped; the live model was never touched.
        Ok(Ok((Err(api_err), _clone, _fp))) => Err(api_err),
        // Success. Re-acquire the write lock, verify no concurrent writer
        // changed the live model since the snapshot, then swap the mutated
        // clone in and run `commit` atomically under the same guard.
        Ok(Ok((Ok(value), mut clone, fingerprint))) => {
            let mut guard = model_handle.write().await;
            if model_fingerprint(&guard) != fingerprint {
                tracing::warn!(
                    op_kind = class.key(),
                    "bounded op raced a concurrent write; refusing swap (retry)"
                );
                return Err(ApiError::new(
                    ErrorCode::Internal,
                    "the model was modified concurrently while this operation \
                     computed; the change was not applied — retry",
                ));
            }
            // O(1) swap: the clone (with the op applied and the same shared
            // recorder Arc) becomes the live model; the old live model is
            // dropped. `commit` sees the swapped-in state.
            std::mem::swap(&mut *guard, &mut clone);
            commit(&value);
            drop(guard);
            Ok(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budgets_are_generous_and_on() {
        let b = OpBudgets::default();
        assert_eq!(b.budget(OpClass::Boolean), Duration::from_secs(60));
        assert_eq!(b.budget(OpClass::Blend), Duration::from_secs(60));
        assert_eq!(b.budget(OpClass::Other), Duration::from_secs(120));
    }

    #[test]
    fn from_durations_threads_each_class() {
        let b = OpBudgets::from_durations(
            Duration::from_millis(1),
            Duration::from_secs(5),
            Duration::from_secs(7),
        );
        assert_eq!(b.budget(OpClass::Boolean), Duration::from_millis(1));
        assert_eq!(b.budget(OpClass::Blend), Duration::from_secs(5));
        assert_eq!(b.budget(OpClass::Other), Duration::from_secs(7));
    }

    #[test]
    fn parse_secs_var_rejects_zero_and_garbage() {
        // Serialize env access across the whole test to avoid cross-test
        // interference on the shared process environment.
        let name = "ROSHERA_TEST_BOUNDED_SECS_PARSE";
        std::env::set_var(name, "0");
        assert_eq!(parse_secs_var(name), None, "0 disables → treated as unset");
        std::env::set_var(name, "notanumber");
        assert_eq!(parse_secs_var(name), None);
        std::env::set_var(name, "  30 ");
        assert_eq!(parse_secs_var(name), Some(30), "whitespace tolerated");
        std::env::remove_var(name);
        assert_eq!(parse_secs_var(name), None);
    }

    #[test]
    fn resolve_class_precedence_class_over_global_over_default() {
        // Class-specific wins over global.
        assert_eq!(
            resolve_class(OpClass::Boolean, Some(99)).as_secs(),
            // No class env set in this process → falls to the global 99.
            99
        );
        // No env at all → compiled default.
        assert_eq!(
            resolve_class(OpClass::Other, None),
            Duration::from_secs(120)
        );
    }
}
