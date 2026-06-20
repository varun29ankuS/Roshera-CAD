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
      return { lines: parsed.lines, events: parsed.events }
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

const WELCOME_LINES: BlackboardLine[] = [
  { id: 'rao-0', text: '# Rao Bell Nozzle — Notebook\nParabolic-approximation (80% bell), built and verified **watertight** in the kernel. Every line is editable — click to edit, Enter to commit.', author: 'agent', createdAt: 0, updatedAt: 0 },
  { id: 'rao-1', text: '**Given:** $p_c = 7\\,\\text{MPa}$, $T_c = 3500\\,\\text{K}$, $\\gamma = 1.22$, $R = 320\\,\\text{J·kg}^{-1}\\text{K}^{-1}$, $p_a = 101\\,\\text{kPa}$, throat radius $R_t = 50\\,\\text{mm}$.', author: 'agent', createdAt: 0, updatedAt: 0 },
  { id: 'rao-2', text: '**Expansion ratio** — areas measured on the as-built solid:\n$$\\varepsilon = \\frac{A_e}{A_t} = \\frac{\\pi R_e^2}{\\pi R_t^2} = \\frac{0.1963}{0.00785} = 25.0$$', author: 'agent', createdAt: 0, updatedAt: 0 },
  { id: 'rao-3', text: '**Rao contour (80% bell):** initial angle $\\theta_n = 33^\\circ$, exit angle $\\theta_e = 9^\\circ$, divergent length\n$$L_n = 0.8\\,\\frac{R_t(\\sqrt{\\varepsilon}-1)}{\\tan 15^\\circ} = 11.94\\,R_t$$\nabout 20% shorter than a $15^\\circ$ cone of the same $\\varepsilon$.', author: 'agent', createdAt: 0, updatedAt: 0 },
  { id: 'rao-4', text: '**Exit Mach** — supersonic root of the area–Mach relation:\n$$\\frac{A_e}{A_t} = \\frac{1}{M_e}\\left[\\frac{2}{\\gamma+1}\\Big(1+\\tfrac{\\gamma-1}{2}M_e^2\\Big)\\right]^{\\frac{\\gamma+1}{2(\\gamma-1)}} = 25 \\;\\Rightarrow\\; M_e = 4.01$$', author: 'agent', createdAt: 0, updatedAt: 0 },
  { id: 'rao-5', text: '**Exit pressure ratio** (isentropic): $\\dfrac{p_e}{p_c} = \\left(1+\\tfrac{\\gamma-1}{2}M_e^2\\right)^{-\\frac{\\gamma}{\\gamma-1}} = 3.5\\times10^{-3}$.', author: 'agent', createdAt: 0, updatedAt: 0 },
  { id: 'rao-6', text: '**Thrust coefficient:**\n$$C_F = \\sqrt{\\frac{2\\gamma^2}{\\gamma-1}\\Big(\\frac{2}{\\gamma+1}\\Big)^{\\frac{\\gamma+1}{\\gamma-1}}\\!\\Big[1-\\big(\\tfrac{p_e}{p_c}\\big)^{\\frac{\\gamma-1}{\\gamma}}\\Big]} + \\Big(\\frac{p_e-p_a}{p_c}\\Big)\\varepsilon = 1.46$$', author: 'agent', createdAt: 0, updatedAt: 0 },
  { id: 'rao-7', text: '**Performance:** $c^{*} = 1622\\,\\text{m/s}$, thrust $F = C_F\\,p_c\\,A_t = 80.4\\,\\text{kN}$, specific impulse $I_{sp} = \\dfrac{C_F\\,c^{*}}{g_0} = 242\\,\\text{s}$ (sea level, this propellant).', author: 'agent', createdAt: 0, updatedAt: 0 },
]

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

/** The seed for a scope when it has no cached snapshot. Only the Document
 *  scope carries the welcome content; a part starts as an empty notebook. */
function seedFor(scope: BlackboardScope): BlackboardSnapshot {
  if (scope === DOCUMENT_SCOPE) return { lines: WELCOME_LINES, events: [] }
  return { lines: [], events: [] }
}

const initial = adapter.load(DOCUMENT_SCOPE) ?? seedFor(DOCUMENT_SCOPE)

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
      const cached = adapter.load(scope) ?? seedFor(scope)
      return { activeScope: scope, lines: cached.lines, events: cached.events }
    }),

  setProcessing: (v) => set({ isProcessing: v }),
  togglePanel: () => set((s) => ({ isPanelOpen: !s.isPanelOpen })),
  setPanel: (open) => set({ isPanelOpen: open }),

  clearBoard: () => {
    void get
    set((state) => {
      // Reseed only the document scope with the welcome content; a part's
      // notebook clears to empty.
      const seed = seedFor(state.activeScope)
      const lines = [...seed.lines]
      const events: BlackboardEvent[] = []
      persist(state.activeScope, { lines, events })
      return { lines, events }
    })
  },
}))
