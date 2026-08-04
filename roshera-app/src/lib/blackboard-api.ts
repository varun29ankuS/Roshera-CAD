/**
 * BACKEND BLACKBOARD ADAPTER
 * ==========================
 * Wires the Blackboard store to the backend's one document notebook
 * (`/api/blackboard*`) instead of `localStorage`, WITHOUT touching any store
 * reducer — the store talks to persistence only through
 * `BlackboardPersistenceAdapter { load, save }`, and `setBlackboardAdapter`
 * swaps the concrete adapter at that seam.
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
 * sides. Polled snapshots are reconciled by id, so ids stay consistent. None
 * of these REST calls send a `scope`/`part_id` — the backend resolves an
 * un-scoped request to the Document notebook, which is the only notebook
 * this client ever addresses (see `stores/blackboard-store.ts`'s "one
 * notebook per document" doc). A GET still comes back as the UNION of the
 * Document notebook and any legacy per-part notebooks
 * (`BlackboardManager::document_snapshot`), so nothing written before that
 * decision goes missing here.
 *
 * # Failure handling
 *
 * `save` is fire-and-forget by contract and never throws into a reducer.
 * A failed backend write is NOT dropped: it stays in an outbox, retried on
 * the next save and on every poll tick, and the poll's reconciliation
 * re-applies pending writes (and the currently-streaming line) on top of the
 * server snapshot so a repaint can never erase a line the backend hasn't
 * confirmed yet. localStorage additionally mirrors every snapshot; if the
 * backend is unreachable at install time, hydration is skipped and the
 * store keeps its `localStorage`-seeded state, so the panel still works
 * offline.
 */

import {
  type BlackboardSnapshot,
  type BlackboardEvent,
  type BlackboardPersistenceAdapter,
  type BlackboardLine,
  isLegacySeedSnapshot,
  setBlackboardAdapter,
  useBlackboardStore,
} from '@/stores/blackboard-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`
const STORAGE_KEY = 'roshera.blackboard.v1.document'

/** Default poll interval (ms) for picking up agent-written lines. */
const POLL_INTERVAL_MS = 2500

// ─── localStorage fallback (same key/shape as the store's own) ──────

function loadLocal(): BlackboardSnapshot | null {
  if (typeof window === 'undefined') return null
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as Partial<BlackboardSnapshot>
    if (!Array.isArray(parsed.lines) || !Array.isArray(parsed.events)) return null
    const snapshot = { lines: parsed.lines, events: parsed.events }
    // An untouched copy of the retired demo notebook is not user content.
    if (isLegacySeedSnapshot(snapshot)) return null
    return snapshot
  } catch {
    return null
  }
}

function saveLocal(snapshot: BlackboardSnapshot): void {
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(snapshot))
  } catch {
    /* quota / private-mode — non-fatal */
  }
}

// ─── REST helpers ────────────────────────────────────────────────────

async function fetchSnapshot(): Promise<BlackboardSnapshot | null> {
  try {
    const res = await fetch(`${API_BASE}/blackboard`)
    if (!res.ok) return null
    const snap = (await res.json()) as Partial<BlackboardSnapshot>
    if (!Array.isArray(snap.lines) || !Array.isArray(snap.events)) return null
    return { lines: snap.lines, events: snap.events }
  } catch {
    return null
  }
}

/** Authorship now survives the round-trip: `api-server/src/blackboard.rs`'s
 *  `LineAuthor` carries `System` alongside `User`/`Agent`, so the board no
 *  longer records its own bookkeeping ("Created …" echoes, sync failures,
 *  toolbar feedback) as something the agent said. It previously downgraded
 *  `system` to `agent` on the way out, because the wire had no third value
 *  and a 422 would make the line vanish silently — a fetch only rejects on
 *  network failure, so `postEntry` "succeeded" while nothing landed, and
 *  the next poll's `applyRemoteSnapshot` repainted from a backend that had
 *  never seen it. The line survived; the truth about who wrote it did not.
 *
 *  ⚠ This requires the rebuilt backend. Against a binary predating the
 *  `System` variant every system line 422s and disappears exactly as
 *  described above, so the server must be restarted before the app is
 *  reloaded — not after. */
function wireAuthor(author: BlackboardLine['author']): 'user' | 'agent' | 'system' {
  return author
}

async function postEntry(line: BlackboardLine): Promise<void> {
  // The frontend owns line ids; the backend keeps `id` verbatim so edit /
  // delete address the same row. No scope is sent — every write lands in
  // the Document notebook.
  const res = await fetch(`${API_BASE}/blackboard/entries`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      id: line.id,
      text: line.text,
      author: wireAuthor(line.author),
    }),
  })
  if (!res.ok) throw new Error(`POST /blackboard/entries failed: ${res.status}`)
}

async function patchEntry(id: string, text: string): Promise<void> {
  const res = await fetch(`${API_BASE}/blackboard/entries/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text }),
  })
  if (!res.ok) throw new Error(`PATCH /blackboard/entries/${id} failed: ${res.status}`)
}

async function deleteEntry(id: string): Promise<void> {
  const res = await fetch(`${API_BASE}/blackboard/entries/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  })
  if (!res.ok) throw new Error(`DELETE /blackboard/entries/${id} failed: ${res.status}`)
}

async function clearBackend(): Promise<void> {
  const res = await fetch(`${API_BASE}/blackboard/clear`, { method: 'POST' })
  if (!res.ok) throw new Error(`POST /blackboard/clear failed: ${res.status}`)
}

// ─── Delta detection ─────────────────────────────────────────────────

/**
 * Last snapshot we either fetched from or wrote to the backend. `save` diffs
 * against it; polling replaces it. Module-scoped so the adapter is a stable
 * singleton across reducer calls.
 */
let lastApplied: BlackboardSnapshot = { lines: [], events: [] }

/** Suppress re-persisting a backend-sourced state we just pushed into the store. */
let applyingRemote = false

function findLine(snapshot: BlackboardSnapshot, id: string): BlackboardLine | undefined {
  return snapshot.lines.find((l) => l.id === id)
}

// ─── Pending-write outbox ────────────────────────────────────────────
//
// A backend write can fail transiently — verified live 2026-08-02: with
// several tabs open, the POST persisting the agent's line 429'd on the
// shared rate budget. The old code swallowed that (localStorage only) while
// STILL advancing the baseline, so the write was never retried — and the
// next poll repainted the store from a backend that had never seen the
// line. The agent's reply vanished from the board without a trace: a
// completed turn erased by its own persistence layer.
//
// Every failed (or not-yet-attempted) write now lands in a FIFO outbox,
// flushed in order on the next save and on every poll tick (a bounded retry
// cadence, never a loop), and `applyRemoteSnapshot` re-applies whatever is
// still pending on top of the server snapshot instead of letting the
// repaint clobber it. Nothing is dropped; nothing needs a reload.

type PendingOp =
  | { kind: 'add'; lineId: string; line: BlackboardLine }
  | { kind: 'edit'; lineId: string; after: string }
  | { kind: 'delete'; lineId: string }
  | { kind: 'clear' }

const outbox: PendingOp[] = []

async function sendOp(op: PendingOp): Promise<void> {
  switch (op.kind) {
    case 'add': {
      // Prefer the freshest local text — the line may have streamed or been
      // edited since the failed attempt captured it.
      const current = useBlackboardStore.getState().lines.find((l) => l.id === op.lineId)
      await postEntry(current ?? op.line)
      break
    }
    case 'edit':
      await patchEntry(op.lineId, op.after)
      break
    case 'delete':
      await deleteEntry(op.lineId)
      break
    case 'clear':
      await clearBackend()
      break
  }
}

/** One in-flight flush — a poll tick and a save arriving together share it
 *  rather than double-sending the head op. */
let flushInFlight: Promise<void> | null = null

/** Flush queued ops in FIFO order, stopping at the first failure. The
 *  remainder stays queued for the next save or poll tick — retry cadence is
 *  bounded by those triggers, never a spin loop. */
function flushOutbox(): Promise<void> {
  if (flushInFlight) return flushInFlight
  const run = (async () => {
    while (outbox.length > 0) {
      try {
        await sendOp(outbox[0])
      } catch {
        return
      }
      outbox.shift()
    }
  })().finally(() => {
    flushInFlight = null
  })
  flushInFlight = run
  return run
}

/**
 * Translate the delta between the last-applied baseline and `next` into
 * REST ops, enqueue them, and flush. A failure leaves the tail queued (see
 * the outbox doc above) — it is never silently dropped, and the caller's
 * localStorage mirror (written in `save`) still guarantees the session
 * survives offline.
 */
async function persistDelta(next: BlackboardSnapshot): Promise<void> {
  const prev = lastApplied
  // clearBoard resets events to empty (and lines to empty).
  if (next.events.length === 0 && prev.events.length > 0) {
    // A clear supersedes every queued write.
    outbox.length = 0
    outbox.push({ kind: 'clear' })
  } else {
    // Append-only log: any new event sits at the tail. We only ever apply one
    // reducer between saves, so a single new event is the common case; if the
    // log advanced by more than one (e.g. a streamed sequence), replay the tail.
    const newEvents: BlackboardEvent[] = next.events.slice(prev.events.length)
    if (newEvents.length === 0 && outbox.length === 0) {
      // No log change (e.g. `setLineText` streaming chunk, which does not
      // log) and nothing pending — nothing to send.
      return
    }
    for (const ev of newEvents) {
      switch (ev.kind) {
        case 'add': {
          const line = findLine(next, ev.lineId)
          if (line) outbox.push({ kind: 'add', lineId: ev.lineId, line })
          break
        }
        case 'edit':
          outbox.push({ kind: 'edit', lineId: ev.lineId, after: ev.after })
          break
        case 'delete':
          outbox.push({ kind: 'delete', lineId: ev.lineId })
          break
      }
    }
  }
  await flushOutbox()
}

// ─── The adapter ─────────────────────────────────────────────────────

/**
 * Backend-backed persistence adapter. `load()` is synchronous (the store
 * calls it once at init), so it returns the localStorage cache for an
 * instant first paint; the authoritative backend snapshot arrives via async
 * hydration in `installBackendBlackboard`.
 *
 * Serialized so two reducers firing back-to-back can't race: both would
 * otherwise read the same baseline (only updated in a `.finally`), diffing
 * — and sending — the earlier event tail twice. Chaining guarantees each
 * run sees the baseline its predecessor set.
 */
let persistChain: Promise<void> = Promise.resolve()

export const backendBlackboardAdapter: BlackboardPersistenceAdapter = {
  load() {
    return loadLocal()
  },
  save(snapshot) {
    // Always keep the localStorage mirror fresh (offline fallback) ...
    saveLocal(snapshot)
    // ... and skip the backend round-trip when WE are the ones writing the
    // store from a backend snapshot (hydrate / poll), which would echo every
    // line straight back to the server.
    if (applyingRemote) {
      lastApplied = snapshot
      return
    }
    persistChain = persistChain
      .then(() => persistDelta(snapshot))
      .catch(() => undefined)
      .then(() => {
        lastApplied = snapshot
      })
  },
}

// ─── Store hydration from a backend snapshot ────────────────────────

/**
 * Re-apply everything the backend has not confirmed yet on top of a server
 * snapshot: queued outbox writes, plus the line currently streaming (its
 * text lives only in the local store until the turn's final edit). Without
 * this, a poll landing while a write is pending — or mid-turn — repainted
 * the board from a backend that hadn't seen the newest lines and they
 * simply vanished (observed live 2026-08-02, agent reply erased).
 */
function mergeUnconfirmedLocal(snapshot: BlackboardSnapshot): BlackboardSnapshot {
  const state = useBlackboardStore.getState()
  const streamingId = state.streamingLineId
  if (outbox.length === 0 && !streamingId) return snapshot

  let lines = [...snapshot.lines]
  for (const op of outbox) {
    switch (op.kind) {
      case 'add':
        if (!lines.some((l) => l.id === op.lineId)) {
          const local = state.lines.find((l) => l.id === op.lineId)
          lines.push(local ?? op.line)
        }
        break
      case 'edit':
        lines = lines.map((l) => (l.id === op.lineId ? { ...l, text: op.after } : l))
        break
      case 'delete':
        lines = lines.filter((l) => l.id !== op.lineId)
        break
      case 'clear':
        lines = []
        break
    }
  }
  if (streamingId) {
    const local = state.lines.find((l) => l.id === streamingId)
    if (local) {
      lines = lines.some((l) => l.id === streamingId)
        ? lines.map((l) => (l.id === streamingId ? local : l))
        : [...lines, local]
    }
  }
  return { lines, events: snapshot.events }
}

function applyRemoteSnapshot(snapshot: BlackboardSnapshot): void {
  const prev = lastApplied
  const same =
    snapshot.events.length === prev.events.length &&
    snapshot.lines.length === prev.lines.length &&
    snapshot.lines.every((l, i) => {
      const p = prev.lines[i]
      return p && p.id === l.id && p.text === l.text
    })
  if (same) return

  const merged = mergeUnconfirmedLocal(snapshot)
  applyingRemote = true
  try {
    // The baseline records what the SERVER holds (the raw snapshot), never
    // the merged view — pending ops must stay diffable/flushable against
    // the server's actual state.
    lastApplied = snapshot
    saveLocal(merged)
    useBlackboardStore.setState({ lines: merged.lines, events: merged.events })
  } finally {
    applyingRemote = false
  }
}

// ─── Install + lifecycle ─────────────────────────────────────────────

let pollTimer: ReturnType<typeof setInterval> | null = null

/**
 * Fetch + reconcile the document notebook. Exported so a document switch
 * can force an immediate re-hydration instead of waiting up to
 * `POLL_INTERVAL_MS` for the ambient poll to notice the backend's
 * `active_document` changed underneath it (see `stores/document-store.ts`).
 */
export async function syncNotebook(): Promise<void> {
  // Retry anything a failed write left queued BEFORE fetching, so the
  // snapshot we reconcile against already includes it (and so a transient
  // failure heals within one poll interval, not never).
  await flushOutbox()
  const snap = await fetchSnapshot()
  if (snap) applyRemoteSnapshot(snap)
}

/**
 * Install the backend adapter and start syncing. Idempotent. Returns a
 * teardown that stops the poll (the adapter stays installed, the desired
 * steady state for the app).
 *
 * Call once at app bootstrap. It hydrates the document notebook from the
 * server and polls so lines other clients / an agent over MCP wrote appear
 * live. If the backend is unreachable the store keeps its
 * localStorage-seeded state and the poll keeps retrying.
 */
export function installBackendBlackboard(intervalMs: number = POLL_INTERVAL_MS): () => void {
  setBlackboardAdapter(backendBlackboardAdapter)

  // Initial hydration.
  void syncNotebook()

  // Live updates: re-fetch and reconcile. A failed poll is a no-op
  // (offline); the next tick retries.
  if (pollTimer === null && typeof window !== 'undefined') {
    pollTimer = setInterval(syncNotebook, intervalMs)
  }

  return () => {
    if (pollTimer !== null) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }
}
