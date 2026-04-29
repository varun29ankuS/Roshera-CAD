//! Atomic transactions over the kernel surface.
//!
//! An agent composing a part out of multiple primitives needs the
//! "all or nothing" guarantee that comes naturally to a database but
//! has historically been alien to CAD kernels: if step 4 of a 5-step
//! plan fails, the partial state from steps 1–3 must not pollute the
//! main timeline. Without this, an agent's mental model drifts from
//! the kernel's after every error and the user is left with three
//! orphaned solids no one asked for.
//!
//! # Lifecycle
//!
//! ```text
//!   POST /api/tx/begin                 → { "tx_id": "<uuid>" }
//!   POST /api/geometry  (X-Tx-Id: ...) → solid created, tracked under tx
//!   POST /api/geometry  (X-Tx-Id: ...) → solid created, tracked under tx
//!   POST /api/tx/{id}/commit           → solids become permanent (no-op
//!                                        on the kernel; just flips state)
//!   POST /api/tx/{id}/rollback         → every solid created under the
//!                                        transaction is removed
//! ```
//!
//! # Semantics
//!
//! - **Implicit transactions are still allowed.** Mutating endpoints
//!   without an `X-Tx-Id` header behave exactly as before. The
//!   transaction header is opt-in.
//! - **Commit is currently a state flip, not a journal flush.** Solids
//!   are written to the model immediately so other observers see them
//!   live. This matches the WYSIWYG agent expectation and avoids a
//!   second copy of the model. If a strict isolation model is later
//!   needed (one agent's in-flight tx invisible to another agent),
//!   add a per-tx shadow `BRepModel`; the public API does not change.
//! - **Rollback removes the tracked solids from the kernel store.**
//!   The timeline replay log keeps the events for auditability — they
//!   are flagged as `rolled_back` rather than deleted, so an agent can
//!   inspect what was attempted.
//! - **Auto-expire after `TX_TTL`.** A transaction that is neither
//!   committed nor rolled back within the window is rolled back by a
//!   background sweeper. Without a TTL, a crashed agent could leak
//!   solid IDs forever.
//!
//! # Why a separate module
//!
//! Transactions cut across the geometry, timeline, and session layers.
//! Pinning the state machine here (rather than scattered across
//! handlers) keeps the invariants — "track only when Active", "no
//! double-commit", "rollback once" — checkable in one place.

use crate::error_catalog::{ApiError, ErrorCode};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

/// HTTP header agents send to associate a request with a transaction.
/// Lowercase per HTTP/2 convention; axum's `HeaderMap` lookup is
/// case-insensitive so client casing does not matter.
pub const TX_ID_HEADER: &str = "x-roshera-tx-id";

/// How long an open transaction may sit idle before the sweeper rolls
/// it back. 1 hour is generous for an interactive agent, tight enough
/// to bound leaks if an agent crashes between begin and commit.
const TX_TTL: Duration = Duration::from_secs(60 * 60);

/// Lifecycle phase. The transition graph is strictly:
/// `Active → Committed` or `Active → RolledBack`. Once terminal, no
/// further mutations are accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TxStatus {
    /// Open and accepting `track_solid` calls.
    Active,
    /// Committed — solids are permanent in the kernel.
    Committed,
    /// Rolled back — solids have been removed from the kernel.
    RolledBack,
    /// Auto-expired and rolled back by the background sweeper.
    Expired,
}

/// Snapshot view of a transaction, safe to serialise to clients.
/// Mirrors the inner `TransactionState` minus the lock.
#[derive(Debug, Clone, Serialize)]
pub struct TransactionView {
    pub tx_id: Uuid,
    pub status: TxStatus,
    pub created_solids: Vec<u32>,
    /// Seconds elapsed since `begin`. Cheap to compute; agents use it
    /// to decide whether to abandon a long-running plan.
    pub age_seconds: u64,
}

/// Inner state guarded by a fast user-space lock. Held only for the
/// duration of `track_solid` / `commit` / `rollback`, so contention
/// is bounded.
#[derive(Debug)]
struct TransactionState {
    id: Uuid,
    status: TxStatus,
    created_at: Instant,
    created_solids: Vec<u32>,
}

/// Concurrent registry of in-flight transactions.
///
/// `DashMap<Uuid, Mutex<TransactionState>>` lets concurrent agents run
/// disjoint transactions without contention; a single transaction is
/// serialised through its inner mutex so two parallel writes from the
/// same agent see consistent state. Cloning the manager itself is
/// cheap (`Arc` everywhere).
#[derive(Debug, Default)]
pub struct TransactionManager {
    transactions: DashMap<Uuid, Arc<Mutex<TransactionState>>>,
}

impl TransactionManager {
    /// Construct an empty manager. Wrapped in `Arc` by `AppState`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a fresh transaction. The returned UUID is what callers put
    /// in the `X-Roshera-Tx-Id` header for subsequent mutations.
    pub fn begin(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.transactions.insert(
            id,
            Arc::new(Mutex::new(TransactionState {
                id,
                status: TxStatus::Active,
                created_at: Instant::now(),
                created_solids: Vec::new(),
            })),
        );
        id
    }

    /// Look up a transaction's state for serialisation. Returns `None`
    /// if the ID is unknown (e.g. expired-and-pruned, or the agent
    /// fabricated a UUID).
    pub fn view(&self, tx_id: Uuid) -> Option<TransactionView> {
        let entry = self.transactions.get(&tx_id)?;
        let guard = entry.value().lock();
        Some(TransactionView {
            tx_id: guard.id,
            status: guard.status,
            created_solids: guard.created_solids.clone(),
            age_seconds: guard.created_at.elapsed().as_secs(),
        })
    }

    /// Record a freshly-created solid against the transaction. Called
    /// by mutating handlers when the request carried `X-Roshera-Tx-Id`.
    /// Errors if the transaction is unknown or no longer active — the
    /// handler should treat this as an `ApiError` and not commit the
    /// solid.
    pub fn track_solid(&self, tx_id: Uuid, solid_id: u32) -> Result<(), ApiError> {
        let entry = self
            .transactions
            .get(&tx_id)
            .ok_or_else(|| transaction_not_found(tx_id))?;
        let mut guard = entry.value().lock();
        if guard.status != TxStatus::Active {
            return Err(transaction_not_active(tx_id, guard.status));
        }
        guard.created_solids.push(solid_id);
        Ok(())
    }

    /// Mark a transaction committed. The solids stay in the kernel as
    /// they already are — this is a state-machine flip plus a return
    /// of the IDs the agent successfully claimed. Errors if the
    /// transaction is missing or already terminal.
    pub fn commit(&self, tx_id: Uuid) -> Result<TransactionView, ApiError> {
        let entry = self
            .transactions
            .get(&tx_id)
            .ok_or_else(|| transaction_not_found(tx_id))?;
        let mut guard = entry.value().lock();
        if guard.status != TxStatus::Active {
            return Err(transaction_not_active(tx_id, guard.status));
        }
        guard.status = TxStatus::Committed;
        Ok(TransactionView {
            tx_id: guard.id,
            status: guard.status,
            created_solids: guard.created_solids.clone(),
            age_seconds: guard.created_at.elapsed().as_secs(),
        })
    }

    /// Mark a transaction rolled back and return the list of solids
    /// the caller must remove from the kernel. We do not own the
    /// `BRepModel` here, so the caller (the HTTP handler) performs
    /// the removals against its already-held write lock. This keeps
    /// lock acquisition order consistent across the codebase: model
    /// lock first, transaction lock second.
    ///
    /// Returns the solids that need removal. Errors if the transaction
    /// is missing or already terminal.
    pub fn begin_rollback(&self, tx_id: Uuid) -> Result<Vec<u32>, ApiError> {
        let entry = self
            .transactions
            .get(&tx_id)
            .ok_or_else(|| transaction_not_found(tx_id))?;
        let mut guard = entry.value().lock();
        if guard.status != TxStatus::Active {
            return Err(transaction_not_active(tx_id, guard.status));
        }
        guard.status = TxStatus::RolledBack;
        Ok(guard.created_solids.clone())
    }

    /// Sweeper hook: roll back every transaction past `TX_TTL`.
    /// Returns the IDs to remove from the kernel for each expired
    /// transaction so the caller can clean up under the model lock.
    /// Idempotent: a transaction already terminal is left alone.
    pub fn sweep_expired(&self) -> Vec<(Uuid, Vec<u32>)> {
        let mut to_clean = Vec::new();
        for entry in self.transactions.iter() {
            let mut guard = entry.value().lock();
            if guard.status == TxStatus::Active && guard.created_at.elapsed() > TX_TTL {
                guard.status = TxStatus::Expired;
                to_clean.push((guard.id, guard.created_solids.clone()));
            }
        }
        to_clean
    }

    /// Total live transactions (any status). Exposed for `/health`
    /// introspection and tests.
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// `len() == 0`; required by clippy when `len` is public.
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }
}

fn transaction_not_found(tx_id: Uuid) -> ApiError {
    ApiError::new(
        ErrorCode::TransactionNotFound,
        format!("transaction {tx_id} is unknown or has been pruned"),
    )
    .with_hint(
        "Open a fresh transaction with POST /api/tx/begin and use \
         the returned tx_id within the next hour.",
    )
    .with_details(serde_json::json!({ "tx_id": tx_id }))
}

fn transaction_not_active(tx_id: Uuid, status: TxStatus) -> ApiError {
    let status_str = match status {
        TxStatus::Active => "active",
        TxStatus::Committed => "committed",
        TxStatus::RolledBack => "rolled_back",
        TxStatus::Expired => "expired",
    };
    ApiError::new(
        ErrorCode::TransactionNotActive,
        format!("transaction {tx_id} is in terminal state '{status_str}'"),
    )
    .with_details(serde_json::json!({
        "tx_id": tx_id,
        "status": status_str,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_returns_active_transaction() {
        let m = TransactionManager::new();
        let id = m.begin();
        let view = m.view(id).unwrap();
        assert_eq!(view.status, TxStatus::Active);
        assert!(view.created_solids.is_empty());
    }

    #[test]
    fn track_solid_records_in_order() {
        let m = TransactionManager::new();
        let id = m.begin();
        m.track_solid(id, 7).unwrap();
        m.track_solid(id, 11).unwrap();
        m.track_solid(id, 13).unwrap();
        assert_eq!(m.view(id).unwrap().created_solids, vec![7, 11, 13]);
    }

    #[test]
    fn commit_makes_transaction_terminal() {
        let m = TransactionManager::new();
        let id = m.begin();
        m.track_solid(id, 1).unwrap();
        let view = m.commit(id).unwrap();
        assert_eq!(view.status, TxStatus::Committed);
        assert_eq!(view.created_solids, vec![1]);

        // Second commit must fail loudly — the agent's mental model
        // would silently drift otherwise.
        let err = m.commit(id).unwrap_err();
        assert_eq!(err.code, ErrorCode::TransactionNotActive);

        // Tracking after commit is also rejected.
        let err = m.track_solid(id, 99).unwrap_err();
        assert_eq!(err.code, ErrorCode::TransactionNotActive);
    }

    #[test]
    fn begin_rollback_returns_tracked_solids() {
        let m = TransactionManager::new();
        let id = m.begin();
        m.track_solid(id, 5).unwrap();
        m.track_solid(id, 6).unwrap();
        let solids = m.begin_rollback(id).unwrap();
        assert_eq!(solids, vec![5, 6]);
        assert_eq!(m.view(id).unwrap().status, TxStatus::RolledBack);

        // Cannot roll back twice.
        let err = m.begin_rollback(id).unwrap_err();
        assert_eq!(err.code, ErrorCode::TransactionNotActive);
    }

    #[test]
    fn unknown_tx_id_returns_not_found() {
        let m = TransactionManager::new();
        let err = m.commit(Uuid::new_v4()).unwrap_err();
        assert_eq!(err.code, ErrorCode::TransactionNotFound);
    }

    #[test]
    fn cannot_commit_after_rollback() {
        let m = TransactionManager::new();
        let id = m.begin();
        let _ = m.begin_rollback(id).unwrap();
        let err = m.commit(id).unwrap_err();
        assert_eq!(err.code, ErrorCode::TransactionNotActive);
    }
}
