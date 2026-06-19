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
  type BlackboardPersistenceAdapter,
  type BlackboardLine,
  setBlackboardAdapter,
  useBlackboardStore,
} from '@/stores/blackboard-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`
const STORAGE_KEY = 'roshera.blackboard.v1'

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
    return { lines: parsed.lines, events: parsed.events }
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

async function postEntry(line: BlackboardLine): Promise<void> {
  // The frontend owns line ids; the backend keeps `id` verbatim so edit /
  // delete address the same row. We send id alongside text/author.
  await fetch(`${API_BASE}/blackboard/entries`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id: line.id, text: line.text, author: line.author }),
  })
}

async function patchEntry(id: string, text: string): Promise<void> {
  await fetch(`${API_BASE}/blackboard/entries/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text }),
  })
}

async function deleteEntry(id: string): Promise<void> {
  await fetch(`${API_BASE}/blackboard/entries/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  })
}

async function clearBackend(): Promise<void> {
  await fetch(`${API_BASE}/blackboard/clear`, { method: 'POST' })
}

// ─── Delta detection ─────────────────────────────────────────────────

/**
 * Last snapshot we either fetched from or wrote to the backend. `save`
 * diffs against it; polling replaces it. Module-scoped so the adapter is a
 * stable singleton across reducer calls.
 */
let lastApplied: BlackboardSnapshot = { lines: [], events: [] }

/** Suppress re-persisting a backend-sourced state we just pushed into the store. */
let applyingRemote = false

function findLine(snapshot: BlackboardSnapshot, id: string): BlackboardLine | undefined {
  return snapshot.lines.find((l) => l.id === id)
}

/**
 * Translate the single delta between `lastApplied` and `next` into one REST
 * call. Best-effort: any failure falls back to a full-snapshot localStorage
 * save so the session is never lost.
 */
async function persistDelta(next: BlackboardSnapshot): Promise<void> {
  // clearBoard resets events to empty (and lines to just the welcome line).
  if (next.events.length === 0 && lastApplied.events.length > 0) {
    try {
      await clearBackend()
      return
    } catch {
      saveLocal(next)
      return
    }
  }

  // Append-only log: any new event sits at the tail. We only ever apply one
  // reducer between saves, so a single new event is the common case; if the
  // log advanced by more than one (e.g. a streamed sequence), replay the tail.
  const newEvents: BlackboardEvent[] = next.events.slice(lastApplied.events.length)
  if (newEvents.length === 0) {
    // No log change (e.g. `setLineText` streaming chunk, which does not log) —
    // mirror to localStorage but don't spam the backend.
    saveLocal(next)
    return
  }

  try {
    for (const ev of newEvents) {
      switch (ev.kind) {
        case 'add': {
          const line = findLine(next, ev.lineId)
          if (line) await postEntry(line)
          break
        }
        case 'edit':
          await patchEntry(ev.lineId, ev.after)
          break
        case 'delete':
          await deleteEntry(ev.lineId)
          break
      }
    }
  } catch {
    // Backend unreachable mid-sequence — fall back to a local snapshot so the
    // user's edits survive the session. The next successful poll/hydrate
    // reconciles state.
    saveLocal(next)
  }
}

// ─── The adapter ─────────────────────────────────────────────────────

/**
 * Backend-backed persistence adapter. `load` is synchronous (the store calls
 * it once at module init), so it returns the localStorage cache for an instant
 * first paint; the authoritative backend snapshot arrives via async hydration
 * in `installBackendBlackboard`.
 */
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
    const next = snapshot
    void persistDelta(next).finally(() => {
      lastApplied = next
    })
  },
}

// ─── Store hydration from a backend snapshot ────────────────────────

/**
 * Replace the store's document with a backend snapshot WITHOUT going through
 * the mutating reducers (which would re-POST every line). We set `lines` /
 * `events` directly via the store's own `setState` — state shape only, no
 * reducer logic touched — guarded by `applyingRemote` so the resulting `save`
 * is treated as a no-op against the backend.
 */
function applyRemoteSnapshot(snapshot: BlackboardSnapshot): void {
  // Skip if nothing changed since we last applied — avoids needless renders
  // on every poll tick.
  const same =
    snapshot.events.length === lastApplied.events.length &&
    snapshot.lines.length === lastApplied.lines.length &&
    snapshot.lines.every((l, i) => {
      const prev = lastApplied.lines[i]
      return prev && prev.id === l.id && prev.text === l.text
    })
  if (same) return

  applyingRemote = true
  try {
    useBlackboardStore.setState({ lines: snapshot.lines, events: snapshot.events })
    lastApplied = snapshot
    saveLocal(snapshot)
  } finally {
    applyingRemote = false
  }
}

// ─── Install + lifecycle ─────────────────────────────────────────────

let pollTimer: ReturnType<typeof setInterval> | null = null

/**
 * Install the backend adapter and start syncing. Idempotent. Returns a
 * teardown that restores nothing destructive — it only stops the poll (the
 * adapter stays installed, which is the desired steady state for the app).
 *
 * Call once at app bootstrap. On the first successful GET it hydrates the
 * store from the server; thereafter a short poll picks up lines other clients
 * (or an agent over MCP) wrote. If the backend is unreachable the store keeps
 * its localStorage-seeded state and the poll keeps retrying.
 */
export function installBackendBlackboard(intervalMs: number = POLL_INTERVAL_MS): () => void {
  setBlackboardAdapter(backendBlackboardAdapter)

  // Initial hydration — authoritative document from the server.
  void fetchSnapshot().then((snap) => {
    if (snap) applyRemoteSnapshot(snap)
  })

  // Live updates: re-fetch and reconcile. A failed poll is a no-op (offline);
  // the next tick retries.
  if (pollTimer === null && typeof window !== 'undefined') {
    pollTimer = setInterval(() => {
      void fetchSnapshot().then((snap) => {
        if (snap) applyRemoteSnapshot(snap)
      })
    }, intervalMs)
  }

  return () => {
    if (pollTimer !== null) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }
}
