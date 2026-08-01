import { create } from 'zustand'

/**
 * BLACKBOARD STORE
 * ================
 * The Blackboard supersedes the chat-transcript UX. Instead of a conversation
 * log of message bubbles, the panel is an editable *document of lines*. Every
 * line is independently editable in place; the agent appends its responses as
 * editable lines (not bubbles), and the user can edit any line — agent-,
 * user-, or system-authored.
 *
 * `chat-store.ts` (the old toolbar-feedback channel — ~30 call sites, not a
 * parallel chat lane) was folded in here rather than kept as a separate
 * notification channel: the timeline already records both human and agent
 * actions, and one log matching that is more coherent. Its feedback lands as
 * `'system'` lines — the app reporting what it did (a toolbar operation's
 * result), as distinct from `'user'` (someone typed it) or `'agent'` (the AI
 * said it).
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

export type LineAuthor = 'user' | 'agent' | 'system'

/**
 * ATTENTION STATE
 * ---------------
 * The agent is always in one of two working modes — writing/reasoning on the
 * board, or executing a tool that changes geometry — plus idle between turns.
 * The Blackboard's split against the viewport FOLLOWS this state: expand
 * while the agent writes, collapse to a strip the moment geometry is being
 * changed so the viewport takes the space (a user drag overrides either).
 *
 * This is deliberately just a value + setter: it is driven by the legacy
 * streaming path and by the dev fixture harness (both set only `writing`/
 * `idle`), and by `lib/acp-blackboard.ts`'s ACP `tool_call`/`tool_call_update`
 * handlers, which DO set `geometry` — for a provider that actually emits
 * those frames. Measured live (2026-07-31, two full turns) on the current
 * default provider path — goose's `claude-code` ACP bridge, a subscription
 * CLI where tools execute inside the CLI process itself — ZERO `tool_call`
 * frames arrive; only `usage_update` / `available_commands_update` /
 * `session_info_update` / `agent_message_chunk` do. So `geometry` is
 * unreachable on that live path today, even though the agent genuinely
 * calls tools and builds geometry. `session_info_update`'s `activeRunId`
 * (non-null while a turn runs) was considered as a substitute signal and
 * rejected: it toggles once per turn at the same boundaries `runAcpTurn`
 * already uses for `writing`/`idle`, so it adds no information and cannot
 * distinguish tool execution from text generation — using it to fake
 * `geometry` would be exactly the prose-heuristic dishonesty this state is
 * supposed to avoid. Do NOT build progressive-build pacing on `geometry`
 * until a provider path is confirmed to emit real `tool_call` frames. The
 * panel resize is pure presentation and NEVER gates geometry: the viewport
 * is driven by ws-bridge/scene-store and updates when the kernel confirms,
 * regardless of this state.
 */
export type AgentAttention = 'idle' | 'writing' | 'geometry'

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

/**
 * TURN OUTCOME
 * ------------
 * Set once on the agent's own line when its turn concludes — drives the
 * completed/cancelled/failed glyph in `BlackboardLine.tsx`, using the same
 * Check/X/CircleSlash vocabulary as `cards/card-chrome.tsx`'s `Claim`
 * (emerald / red / amber). This is a TURN-lifecycle marker, not a geometry
 * verdict: `'completed'` means the agent's turn ended without erroring or
 * being cancelled — it says nothing about whether the resulting geometry is
 * sound. That verdict belongs to the certificate cards alone; never let this
 * glyph imply one the kernel did not give. `'cancelled'` (the user pressed
 * Stop) gets the neutral amber mark, not a red cross — stopping a turn is
 * not a failure of it.
 */
export type AgentTurnStatus = 'completed' | 'cancelled' | 'failed'

export interface BlackboardLine {
  id: string
  /** Raw source (markdown + `$...$` / `$$...$$` math). Rendered via MessageMarkdown. */
  text: string
  author: LineAuthor
  createdAt: number
  updatedAt: number
  /** How the agent turn that produced this line ended. Undefined for
   *  user/system lines and for an agent line still streaming (no verdict
   *  yet — see `AgentTurnStatus`'s doc). */
  turnStatus?: AgentTurnStatus
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
  /** What the agent is doing right now — drives the attention-following
   *  Blackboard/viewport split. See `AgentAttention`. */
  agentAttention: AgentAttention
  /** The line currently receiving streamed agent text (via `setLineText`),
   *  or null. While set, that line renders through the streaming path that
   *  buffers math/cards to completeness instead of typesetting per token. */
  streamingLineId: string | null

  /** Append a line; returns its id. Pushes an `add` event + persists. */
  addLine: (text: string, author: LineAuthor) => string
  /** Replace a line's text (commit from in-place edit). Pushes an `edit` event + persists. */
  editLine: (id: string, text: string) => void
  /** Remove a line. Pushes a `delete` event + persists. */
  deleteLine: (id: string) => void
  /** Live progressive update (agent streaming). Same as editLine but does not
   *  spam the event log per chunk — see `processBlackboardMessage`. */
  setLineText: (id: string, text: string) => void
  /** Mark how a turn concluded on this line (see `AgentTurnStatus`). Presentation
   *  metadata, persisted alongside the line; not logged as its own event —
   *  it rides along with the `editLine` call that commits the final text. */
  setLineTurnStatus: (id: string, status: AgentTurnStatus) => void

  /**
   * Switch the active notebook to `scope`. Resets `lines`/`events` to that
   * scope's local cache immediately (so the panel never shows the previous
   * part's calcs for a frame); the backend adapter then hydrates the
   * authoritative document for the scope. No-op if already active.
   */
  setActiveScope: (scope: BlackboardScope) => void

  setProcessing: (v: boolean) => void
  /** Simple setter for the attention state — the seam the ACP wiring will
   *  drive in a later slice. */
  setAgentAttention: (attention: AgentAttention) => void
  /** Mark (or clear) the line receiving streamed text. */
  setStreamingLine: (id: string | null) => void
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
  agentAttention: 'idle',
  streamingLineId: null,

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
      if (!existing) return state
      // NOTE: this used to also bail out when `existing.text === text` (a
      // pure no-op optimization). That is UNSOUND for the streaming path:
      // `setLineText` (below) mutates `lines` in place WITHOUT persisting,
      // by design, so the in-memory text can already equal the final
      // streamed value by the time `runAcpTurn` (`lib/acp-blackboard.ts`)
      // calls `editLine(lineId, finalText)` to commit it. The equality
      // check then saw "nothing changed" and skipped BOTH the event log
      // entry and `persist()` — so the agent's whole reply rendered live
      // but was never written to the backend. The next poll's
      // `applyRemoteSnapshot` (`lib/blackboard-api.ts`) then repainted the
      // panel from the (still-empty) backend truth, silently blanking a
      // reply that had just streamed in correctly — verified live
      // (2026-07-31/08-01): the line read "I'll create a 30 mm cube…"
      // immediately after the turn, then "Empty line — click to edit"
      // after a reload. A redundant PATCH for a genuinely no-op manual
      // edit is a harmless idempotent write; a silently dropped agent
      // reply is not.
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

  setLineTurnStatus: (id, status) =>
    set((state) => {
      const lines = state.lines.map((l) => (l.id === id ? { ...l, turnStatus: status } : l))
      persist(state.activeScope, { lines, events: state.events })
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
  setAgentAttention: (attention) => set({ agentAttention: attention }),
  setStreamingLine: (id) => set({ streamingLineId: id }),
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
