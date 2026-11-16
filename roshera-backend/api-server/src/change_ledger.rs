//! What each mutating operation changed, by persistent id, on the response.
//!
//! The kernel's [`geometry_engine::lens::changes`] compares two snapshots of
//! a model's faces and solids. This module keeps, per model handle, the
//! snapshot taken at that model's previous mutating response, and on the next
//! one reports the difference: the faces and solids created, deleted and
//! modified since. The first response on a model has no earlier snapshot and
//! says so (`baseline: false`) instead of calling every face new.
//!
//! Keyed by the model handle's address, guarded by a weak reference: an entry
//! whose model has been dropped can never be read as the baseline of a new
//! model that happens to reuse the address, and such entries are pruned.
//! A process-wide table rather than an `AppState` field: every handler that
//! answers with a certificate already passes its model handle to
//! `certified_response`, and the table needs nothing else.

use std::sync::{Arc, LazyLock, Weak};

use dashmap::DashMap;
use geometry_engine::lens::changes::{changes, snapshot, ChangeSet, Snapshot};
use geometry_engine::primitives::topology_builder::BRepModel;

use crate::reconcile_task::ModelHandle;

type Entry = (Weak<tokio::sync::RwLock<BRepModel>>, Snapshot);

static LEDGER: LazyLock<DashMap<usize, Entry>> = LazyLock::new(DashMap::new);

fn key(handle: &ModelHandle) -> usize {
    Arc::as_ptr(handle) as usize
}

/// What changed on `model` (behind `handle`) since its previous report, and
/// remember the model as it is now for the next one.
pub(crate) fn report(handle: &ModelHandle, model: &BRepModel) -> ChangeSet {
    let now = snapshot(model);
    let k = key(handle);
    let before = LEDGER.get(&k).and_then(|e| {
        let (weak, snap) = e.value();
        weak.upgrade()
            .filter(|live| Arc::ptr_eq(live, handle))
            .map(|_| snap.clone())
    });
    let delta = changes(before.as_ref(), &now);
    LEDGER.insert(k, (Arc::downgrade(handle), now));
    // Drop the entries of models that no longer exist.
    LEDGER.retain(|_, (weak, _)| weak.strong_count() > 0);
    delta
}

/// Remember `model` as it is now without reporting: after a document is
/// opened or replaced, so its first operation is compared with what was
/// opened rather than reported without a baseline.
pub(crate) fn rebase(handle: &ModelHandle, model: &BRepModel) {
    LEDGER.insert(key(handle), (Arc::downgrade(handle), snapshot(model)));
}
