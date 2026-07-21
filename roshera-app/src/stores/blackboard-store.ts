import { create } from 'zustand'

/**
 * BLACKBOARD STORE
 * ================
 * The Blackboard supersedes the chat-transcript UX. Instead of a conversation
 * log of message bubbles, the panel is an editable *document of lines*. Every
 * line is independently editable in place; the agent appends its responses as
 * editable lines (not bubbles), and the user can edit any line — agent- or
 * user-authored.
 *
 * Two things are kept in lock-step (Varun's "logged = both" choice):
 *   1. `lines`  — the ordered *current state* of the document.
 *   2. `events` — an append-only, timestamped *event log* of every
 *      create / edit / delete, so the document's history can be viewed or
 *      scrubbed later. This mirrors the kernel's event-sourced philosophy.
 *
 * Every reducer that mutates `lines` ALSO pushes an event onto `events`, and
 * then asks the persistence adapter to save. State and log never drift.
 */

export type LineAuthor = 'user' | 'agent'

/**
 * SCOPE
 * -----
 * The north star is 100-part assemblies; one global notebook mixing every
 * part's calculations is unusable at that scale. So a notebook belongs to an
 * OWNER, addressed by a canonical scope token that mirrors the backend
 * `BlackboardScope`:
 *   - `'document'`        — document / session-wide notes (the default, and
 *                           the migration home for legacy un-scoped entries).
 *   - `'part:<uuid>'`     — a single part's own notebook (the primary case).
 *   - `'assembly:<uuid>'` — cross-part / assembly-level calcs.
 * The panel shows the ACTIVE scope's notebook; selecting a different part
 * switches scope and reloads that part's lines.
 */
export type BlackboardScope = string
export const DOCUMENT_SCOPE: BlackboardScope = 'document'
export function partScope(partUuid: string): BlackboardScope {
  return `part:${partUuid}`
}

export interface BlackboardLine {
  id: string
  /** Raw source (markdown + `$...$` / `$$...$$` math). Rendered via MessageMarkdown. */
  text: string
  author: LineAuthor
  createdAt: number
  updatedAt: number
}

export type BlackboardEvent =
  | { kind: 'add'; lineId: string; text: string; author: LineAuthor; at: number; index: number }
  | { kind: 'edit'; lineId: string; before: string; after: string; at: number }
  | { kind: 'delete'; lineId: string; text: string; at: number; index: number }

/**
 * PERSISTENCE SEAM
 * ----------------
 * The store talks to persistence ONLY through this interface. Today the
 * concrete adapter is `localStorageAdapter` (no backend Blackboard endpoint
 * exists yet). When a backend lands, swap in an adapter that POSTs the
 * snapshot (or streams the event log) — nothing else in the store changes.
 *
 * `save` is intentionally fire-and-forget (sync-or-async): the store does not
 * await it, so a slow/absent backend never blocks an edit.
 */
export interface BlackboardSnapshot {
  lines: BlackboardLine[]
  events: BlackboardEvent[]
}

export interface BlackboardPersistenceAdapter {
  /** Load the snapshot for a scope (synchronously, e.g. from a local cache). */
  load(scope: BlackboardScope): BlackboardSnapshot | null
  /** Persist a scope's snapshot. */
  save(scope: BlackboardScope, snapshot: BlackboardSnapshot): void
}

const STORAGE_PREFIX = 'roshera.blackboard.v1'

/** Per-scope localStorage key, so one part's cache never overwrites another. */
function storageKey(scope: BlackboardScope): string {
  return `${STORAGE_PREFIX}.${scope}`
}

const localStorageAdapter: BlackboardPersistenceAdapter = {
  load(scope) {
    if (typeof window === 'undefined') return null
    try {
      const raw = window.localStorage.getItem(storageKey(scope))
      if (!raw) return null
      const parsed = JSON.parse(raw) as Partial<BlackboardSnapshot>
      if (!Array.isArray(parsed.lines) || !Array.isArray(parsed.events)) return null
      const snapshot = { lines: parsed.lines, events: parsed.events }
      // An untouched copy of the retired demo notebook is not user content.
      if (isLegacySeedSnapshot(snapshot)) return null
      return snapshot
    } catch {
      // Corrupt payload — start clean rather than crash the panel.
      return null
    }
  },
  save(scope, snapshot) {
    if (typeof window === 'undefined') return
    try {
      window.localStorage.setItem(storageKey(scope), JSON.stringify(snapshot))
    } catch {
      // Quota / private-mode failures are non-fatal; the in-memory store
      // remains the source of truth for the session.
    }
  },
}

// Single module-level adapter reference. A future backend wiring replaces this
// (e.g. via `setBlackboardAdapter`) without touching any reducer.
let adapter: BlackboardPersistenceAdapter = localStorageAdapter

export function setBlackboardAdapter(next: BlackboardPersistenceAdapter): void {
  adapter = next
}

/**
 * The retired hard-coded demo notebook ("Rao Bell Nozzle") seeded every fresh
 * Document scope with `rao-*` line ids. The Blackboard now starts empty — it
 * shows only what an agent or the user actually wrote — but cached copies of
 * that demo still live in localStorage. This detects one: seed lines only
 * (`rao-*` ids) and an empty event log, i.e. the user never touched it. Such
 * a snapshot is discarded on load; an edited one is user content and kept.
 */
export function isLegacySeedSnapshot(snapshot: BlackboardSnapshot): boolean {
  return (
    snapshot.events.length === 0 &&
    snapshot.lines.length > 0 &&
    snapshot.lines.every((l) => l.id.startsWith('rao-'))
  )
}

interface BlackboardState {
  /** The notebook currently shown — `'document'`, `part:<uuid>`, or
   *  `assembly:<uuid>`. `lines`/`events` always belong to THIS scope. */
  activeScope: BlackboardScope
  lines: BlackboardLine[]
  events: BlackboardEvent[]
  isProcessing: boolean
  isPanelOpen: boolean

  /** Append a line; returns its id. Pushes an `add` event + persists. */
  addLine: (text: string, author: LineAuthor) => string
  /** Replace a line's text (commit from in-place edit). Pushes an `edit` event + persists. */
  editLine: (id: string, text: string) => void
  /** Remove a line. Pushes a `delete` event + persists. */
  deleteLine: (id: string) => void
  /** Live progressive update (agent streaming). Same as editLine but does not
   *  spam the event log per chunk — see `processBlackboardMessage`. */
  setLineText: (id: string, text: string) => void

  /**
   * Switch the active notebook to `scope`. Resets `lines`/`events` to that
   * scope's local cache immediately (so the panel never shows the previous
   * part's calcs for a frame); the backend adapter then hydrates the
   * authoritative document for the scope. No-op if already active.
   */
  setActiveScope: (scope: BlackboardScope) => void

  setProcessing: (v: boolean) => void
  togglePanel: () => void
  setPanel: (open: boolean) => void
  clearBoard: () => void
}

let lineCounter = 0
function nextLineId(): string {
  return `bb-${Date.now().toString(36)}-${++lineCounter}`
}

function persist(scope: BlackboardScope, state: Pick<BlackboardState, 'lines' | 'events'>): void {
  adapter.save(scope, { lines: state.lines, events: state.events })
}

/** Every scope with no cached snapshot starts as an EMPTY notebook — the
 *  Blackboard carries only lines an agent or the user actually wrote. */
function emptyNotebook(): BlackboardSnapshot {
  return { lines: [], events: [] }
}

const initial = adapter.load(DOCUMENT_SCOPE) ?? emptyNotebook()

export const useBlackboardStore = create<BlackboardState>((set, get) => ({
  activeScope: DOCUMENT_SCOPE,
  lines: initial.lines,
  events: initial.events,
  isProcessing: false,
  isPanelOpen: true,

  addLine: (text, author) => {
    const id = nextLineId()
    const now = Date.now()
    set((state) => {
      const index = state.lines.length
      const lines = [
        ...state.lines,
        { id, text, author, createdAt: now, updatedAt: now },
      ]
      const events: BlackboardEvent[] = [
        ...state.events,
        { kind: 'add', lineId: id, text, author, at: now, index },
      ]
      persist(state.activeScope, { lines, events })
      return { lines, events }
    })
    return id
  },

  editLine: (id, text) =>
    set((state) => {
      const existing = state.lines.find((l) => l.id === id)
      if (!existing || existing.text === text) return state
      const now = Date.now()
      const lines = state.lines.map((l) =>
        l.id === id ? { ...l, text, updatedAt: now } : l,
      )
      const events: BlackboardEvent[] = [
        ...state.events,
        { kind: 'edit', lineId: id, before: existing.text, after: text, at: now },
      ]
      persist(state.activeScope, { lines, events })
      return { lines, events }
    }),

  deleteLine: (id) =>
    set((state) => {
      const index = state.lines.findIndex((l) => l.id === id)
      if (index === -1) return state
      const existing = state.lines[index]
      const now = Date.now()
      const lines = state.lines.filter((l) => l.id !== id)
      const events: BlackboardEvent[] = [
        ...state.events,
        { kind: 'delete', lineId: id, text: existing.text, at: now, index },
      ]
      persist(state.activeScope, { lines, events })
      return { lines, events }
    }),

  // Progressive streaming target: mutates state in place WITHOUT logging an
  // event per chunk. The caller logs a single `edit` event (via editLine) once
  // the stream settles, so the event log stays meaningful rather than noisy.
  setLineText: (id, text) =>
    set((state) => {
      const lines = state.lines.map((l) =>
        l.id === id ? { ...l, text, updatedAt: Date.now() } : l,
      )
      return { lines }
    }),

  setActiveScope: (scope) =>
    set((state) => {
      if (scope === state.activeScope) return state
      // Show the scope's local cache instantly (empty notebook for a fresh
      // part — never the previous part's lines); the adapter hydrates the
      // authoritative backend document right after.
      const cached = adapter.load(scope) ?? emptyNotebook()
      return { activeScope: scope, lines: cached.lines, events: cached.events }
    }),

  setProcessing: (v) => set({ isProcessing: v }),
  togglePanel: () => set((s) => ({ isPanelOpen: !s.isPanelOpen })),
  setPanel: (open) => set({ isPanelOpen: open }),

  clearBoard: () => {
    void get
    set((state) => {
      // Every scope clears to an empty notebook.
      const lines: BlackboardLine[] = []
      const events: BlackboardEvent[] = []
      persist(state.activeScope, { lines, events })
      return { lines, events }
    })
  },
}))
