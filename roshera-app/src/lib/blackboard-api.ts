/**
 * BACKEND BLACKBOARD ADAPTER
 * ==========================
 * Wires the Blackboard store to the backend notebook (`/api/blackboard*`)
 * instead of `localStorage`, WITHOUT touching any store reducer — the store
 * talks to persistence only through `BlackboardPersistenceAdapter { load,
 * save }`, and `setBlackboardAdapter` swaps the concrete adapter at that
 * seam.
 *
 * The backend is the source of truth:
 *   - On install we GET the snapshot and hydrate the store, so a reload (and
 *     any other client) sees the same document.
 *   - A short poll re-fetches the snapshot so an agent-written line (added
 *     over MCP / REST) appears live in this client. WS broadcast is the
 *     eventual upgrade; a poll is the accepted v1 (the WS frame surface is
 *     geometry-shaped and heavy to extend for this).
 *
 * # How `save` maps to REST
 *
 * `save(snapshot)` receives the full document after every reducer. The store
 * is event-sourced: `events` is append-only except `clearBoard`, which empties
 * both arrays. So we diff the incoming snapshot against the last one WE
 * applied and translate the single delta into one REST call:
 *   - one new `add`    event → POST   /api/blackboard/entries
 *   - one new `edit`   event → PATCH  /api/blackboard/entries/{id}
 *   - one new `delete` event → DELETE /api/blackboard/entries/{id}
 *   - events shrank to empty → POST   /api/blackboard/clear
 * The frontend allocates its own line ids; the backend keeps the client's id
 * verbatim on add, so subsequent edit/delete address the same row on both
 * sides. Polled snapshots are reconciled by id, so ids stay consistent.
 *
 * # Offline fallback
 *
 * Every backend call falls back to `localStorage` on failure and never throws
 * into a reducer (`save` is fire-and-forget by contract). If the backend is
 * unreachable at install time, hydration is skipped and the store keeps its
 * `localStorage`-seeded state, so the panel still works offline.
 */

import {
  type BlackboardSnapshot,
  type BlackboardEvent,
  type BlackboardScope,
  type BlackboardPersistenceAdapter,
  type BlackboardLine,
  DOCUMENT_SCOPE,
  setBlackboardAdapter,
  useBlackboardStore,
} from '@/stores/blackboard-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`
const STORAGE_PREFIX = 'roshera.blackboard.v1'

/** Default poll interval (ms) for picking up agent-written lines. */
const POLL_INTERVAL_MS = 2500

/** Per-scope localStorage key — mirrors the store's own keying so the offline
 *  cache for one part never overwrites another's. */
function storageKey(scope: BlackboardScope): string {
  return `${STORAGE_PREFIX}.${scope}`
}

/** The `?scope=` query suffix that routes a request to a scope's notebook. The
 *  document scope is the backend default, so it needs no query. */
function scopeQuery(scope: BlackboardScope): string {
  return scope === DOCUMENT_SCOPE ? '' : `?scope=${encodeURIComponent(scope)}`
}

// ─── localStorage fallback (same key/shape as the store's own) ──────

function loadLocal(scope: BlackboardScope): BlackboardSnapshot | null {
  if (typeof window === 'undefined') return null
  try {
    const raw = window.localStorage.getItem(storageKey(scope))
    if (!raw) return null
    const parsed = JSON.parse(raw) as Partial<BlackboardSnapshot>
    if (!Array.isArray(parsed.lines) || !Array.isArray(parsed.events)) return null
    return { lines: parsed.lines, events: parsed.events }
  } catch {
    return null
  }
}

function saveLocal(scope: BlackboardScope, snapshot: BlackboardSnapshot): void {
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(storageKey(scope), JSON.stringify(snapshot))
  } catch {
    /* quota / private-mode — non-fatal */
  }
}

// ─── REST helpers ────────────────────────────────────────────────────

async function fetchSnapshot(scope: BlackboardScope): Promise<BlackboardSnapshot | null> {
  try {
    const res = await fetch(`${API_BASE}/blackboard${scopeQuery(scope)}`)
    if (!res.ok) return null
    const snap = (await res.json()) as Partial<BlackboardSnapshot>
    if (!Array.isArray(snap.lines) || !Array.isArray(snap.events)) return null
    return { lines: snap.lines, events: snap.events }
  } catch {
    return null
  }
}

async function postEntry(scope: BlackboardScope, line: BlackboardLine): Promise<void> {
  // The frontend owns line ids; the backend keeps `id` verbatim so edit /
  // delete address the same row. We send id + scope alongside text/author so
  // the line lands in the active part's notebook.
  await fetch(`${API_BASE}/blackboard/entries`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      id: line.id,
      text: line.text,
      author: line.author,
      ...(scope === DOCUMENT_SCOPE ? {} : { scope }),
    }),
  })
}

async function patchEntry(scope: BlackboardScope, id: string, text: string): Promise<void> {
  await fetch(
    `${API_BASE}/blackboard/entries/${encodeURIComponent(id)}${scopeQuery(scope)}`,
    {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text }),
    },
  )
}

async function deleteEntry(scope: BlackboardScope, id: string): Promise<void> {
  await fetch(
    `${API_BASE}/blackboard/entries/${encodeURIComponent(id)}${scopeQuery(scope)}`,
    { method: 'DELETE' },
  )
}

async function clearBackend(scope: BlackboardScope): Promise<void> {
  await fetch(`${API_BASE}/blackboard/clear${scopeQuery(scope)}`, { method: 'POST' })
}

// ─── Delta detection ─────────────────────────────────────────────────

/**
 * Last snapshot we either fetched from or wrote to the backend, PER SCOPE.
 * `save` diffs the active scope against its entry; polling replaces it. A part
 * and the document each track their own baseline so a delta is never computed
 * against the wrong notebook. Module-scoped so the adapter is a stable
 * singleton across reducer calls.
 */
const lastApplied = new Map<BlackboardScope, BlackboardSnapshot>()
const EMPTY: BlackboardSnapshot = { lines: [], events: [] }
function baseline(scope: BlackboardScope): BlackboardSnapshot {
  return lastApplied.get(scope) ?? EMPTY
}

/** Suppress re-persisting a backend-sourced state we just pushed into the store. */
let applyingRemote = false

function findLine(snapshot: BlackboardSnapshot, id: string): BlackboardLine | undefined {
  return snapshot.lines.find((l) => l.id === id)
}

/**
 * Translate the single delta between `scope`'s baseline and `next` into one
 * REST call against that scope's notebook. Best-effort: any failure falls back
 * to a full-snapshot localStorage save so the session is never lost.
 */
async function persistDelta(scope: BlackboardScope, next: BlackboardSnapshot): Promise<void> {
  const prev = baseline(scope)
  // clearBoard resets events to empty (and lines to just the welcome line).
  if (next.events.length === 0 && prev.events.length > 0) {
    try {
      await clearBackend(scope)
      return
    } catch {
      saveLocal(scope, next)
      return
    }
  }

  // Append-only log: any new event sits at the tail. We only ever apply one
  // reducer between saves, so a single new event is the common case; if the
  // log advanced by more than one (e.g. a streamed sequence), replay the tail.
  const newEvents: BlackboardEvent[] = next.events.slice(prev.events.length)
  if (newEvents.length === 0) {
    // No log change (e.g. `setLineText` streaming chunk, which does not log) —
    // mirror to localStorage but don't spam the backend.
    saveLocal(scope, next)
    return
  }

  try {
    for (const ev of newEvents) {
      switch (ev.kind) {
        case 'add': {
          const line = findLine(next, ev.lineId)
          if (line) await postEntry(scope, line)
          break
        }
        case 'edit':
          await patchEntry(scope, ev.lineId, ev.after)
          break
        case 'delete':
          await deleteEntry(scope, ev.lineId)
          break
      }
    }
  } catch {
    // Backend unreachable mid-sequence — fall back to a local snapshot so the
    // user's edits survive the session. The next successful poll/hydrate
    // reconciles state.
    saveLocal(scope, next)
  }
}

// ─── The adapter ─────────────────────────────────────────────────────

/**
 * Backend-backed persistence adapter. `load(scope)` is synchronous (the store
 * calls it on init and on every scope switch), so it returns the localStorage
 * cache for that scope for an instant first paint; the authoritative backend
 * snapshot arrives via async hydration in `installBackendBlackboard`.
 */
export const backendBlackboardAdapter: BlackboardPersistenceAdapter = {
  load(scope) {
    return loadLocal(scope)
  },
  save(scope, snapshot) {
    // Always keep the localStorage mirror fresh (offline fallback) ...
    saveLocal(scope, snapshot)
    // ... and skip the backend round-trip when WE are the ones writing the
    // store from a backend snapshot (hydrate / poll), which would echo every
    // line straight back to the server.
    if (applyingRemote) {
      lastApplied.set(scope, snapshot)
      return
    }
    void persistDelta(scope, snapshot).finally(() => {
      lastApplied.set(scope, snapshot)
    })
  },
}

// ─── Store hydration from a backend snapshot ────────────────────────

/**
 * Replace the ACTIVE scope's document with a backend snapshot WITHOUT going
 * through the mutating reducers (which would re-POST every line). Guarded by
 * `applyingRemote` so the resulting `save` is treated as a no-op against the
 * backend. `scope` is the notebook the snapshot belongs to; if the user has
 * since switched parts, the snapshot is cached but not painted (it would clash
 * with the now-active notebook).
 */
function applyRemoteSnapshot(scope: BlackboardScope, snapshot: BlackboardSnapshot): void {
  const prev = baseline(scope)
  const same =
    snapshot.events.length === prev.events.length &&
    snapshot.lines.length === prev.lines.length &&
    snapshot.lines.every((l, i) => {
      const p = prev.lines[i]
      return p && p.id === l.id && p.text === l.text
    })
  if (same) return

  applyingRemote = true
  try {
    lastApplied.set(scope, snapshot)
    saveLocal(scope, snapshot)
    // Only repaint the panel if this snapshot is for the notebook on screen.
    if (useBlackboardStore.getState().activeScope === scope) {
      useBlackboardStore.setState({ lines: snapshot.lines, events: snapshot.events })
    }
  } finally {
    applyingRemote = false
  }
}

// ─── Install + lifecycle ─────────────────────────────────────────────

let pollTimer: ReturnType<typeof setInterval> | null = null
let unsubScope: (() => void) | null = null

/** Fetch + reconcile the currently-active scope's notebook. */
function syncActiveScope(): void {
  const scope = useBlackboardStore.getState().activeScope
  void fetchSnapshot(scope).then((snap) => {
    if (snap) applyRemoteSnapshot(scope, snap)
  })
}

/**
 * Install the backend adapter and start syncing. Idempotent. Returns a
 * teardown that stops the poll and the scope subscription (the adapter stays
 * installed, the desired steady state for the app).
 *
 * Call once at app bootstrap. It hydrates the active scope's notebook from the
 * server, re-hydrates whenever the user selects a different part (the store's
 * `activeScope` changes), and polls so lines other clients / an agent over MCP
 * wrote appear live. If the backend is unreachable the store keeps its
 * localStorage-seeded state and the poll keeps retrying.
 */
export function installBackendBlackboard(intervalMs: number = POLL_INTERVAL_MS): () => void {
  setBlackboardAdapter(backendBlackboardAdapter)

  // Initial hydration — authoritative document for whatever scope is active
  // at boot (the Document notebook).
  syncActiveScope()

  // Re-hydrate immediately when the active scope changes (the user selected a
  // different part). The store has already painted that scope's local cache;
  // this fetches the authoritative backend document for it.
  if (unsubScope === null) {
    unsubScope = useBlackboardStore.subscribe((state, prev) => {
      if (state.activeScope !== prev.activeScope) syncActiveScope()
    })
  }

  // Live updates: re-fetch and reconcile the active scope. A failed poll is a
  // no-op (offline); the next tick retries.
  if (pollTimer === null && typeof window !== 'undefined') {
    pollTimer = setInterval(syncActiveScope, intervalMs)
  }

  return () => {
    if (pollTimer !== null) {
      clearInterval(pollTimer)
      pollTimer = null
    }
    if (unsubScope !== null) {
      unsubScope()
      unsubScope = null
    }
  }
}
