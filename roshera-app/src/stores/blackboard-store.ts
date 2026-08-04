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
 *
 * ONE NOTEBOOK PER DOCUMENT (2026-08-04)
 * ---------------------------------------
 * The board used to be addressable per PART — a per-scope notebook the
 * viewport's active selection switched between. Varun reversed that: the
 * agent session is already scoped per document (`document-store.ts` calls
 * `resetAcpClient()` on every document switch); the notebook a human reads
 * now matches it 1:1. There is exactly one notebook, always the document's.
 * Lines written under the old per-part model before this change are not
 * lost — the backend's read side unions them in (see
 * `api-server/src/blackboard.rs`'s `BlackboardManager::document_snapshot`),
 * tagged with `partId`/`partUuid` so the reader can still tell which part a
 * line was about.
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
  /** Set (and incremented) only when `addLine` collapses a repeated,
   *  identical, CONSECUTIVE system line into this one rather than
   *  appending a new line — e.g. a resync failure that reposts the same
   *  text on every WS reconnect. Undefined/1 means "posted once."
   *  `BlackboardLine.tsx` renders it as a trailing `(×N)`. Never set for
   *  agent/user lines — a person's or the model's repeated words are
   *  content, not bookkeeping spam. */
  repeatCount?: number
  /** Which part this line was originally about, if it was written under the
   *  old per-part notebook model (retired 2026-08-04 — the blackboard is
   *  one notebook per document now). Set only by the backend's read-side
   *  union (`api-server/src/blackboard.rs`'s `document_snapshot`) for a
   *  legacy line; never set by anything this store itself writes. */
  partId?: number
  /** The part's current UUID alias (`AppState::get_uuid`, the id
   *  `scene-store`'s `objects` map is keyed by), so a legacy line can be
   *  resolved to a live part's name. Undefined when `partId` is, or when
   *  that part is no longer registered (deleted/retired) — `partId` alone
   *  still says the line was about *a* part. */
  partUuid?: string
}

export type BlackboardEvent =
  | { kind: 'add'; lineId: string; text: string; author: LineAuthor; at: number; index: number }
  | { kind: 'edit'; lineId: string; before: string; after: string; at: number }
  | { kind: 'delete'; lineId: string; text: string; at: number; index: number }

/**
 * PERSISTENCE SEAM
 * ----------------
 * The store talks to persistence ONLY through this interface, for the one
 * document notebook (there is nothing else to address any more — see
 * `stores/document-store.ts`'s per-document session reset, which this
 * mirrors 1:1). Today the concrete adapter is `localStorageAdapter`;
 * `installBackendBlackboard` (`lib/blackboard-api.ts`) swaps in the
 * backend-backed one at app bootstrap.
 *
 * `save` is intentionally fire-and-forget (sync-or-async): the store does not
 * await it, so a slow/absent backend never blocks an edit.
 */
export interface BlackboardSnapshot {
  lines: BlackboardLine[]
  events: BlackboardEvent[]
}

export interface BlackboardPersistenceAdapter {
  /** Load the document notebook (synchronously, e.g. from a local cache). */
  load(): BlackboardSnapshot | null
  /** Persist the document notebook. */
  save(snapshot: BlackboardSnapshot): void
}

const STORAGE_KEY = 'roshera.blackboard.v1.document'

const localStorageAdapter: BlackboardPersistenceAdapter = {
  load() {
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
      // Corrupt payload — start clean rather than crash the panel.
      return null
    }
  },
  save(snapshot) {
    if (typeof window === 'undefined') return
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(snapshot))
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
 * document notebook with `rao-*` line ids. The Blackboard now starts empty —
 * it shows only what an agent or the user actually wrote — but cached copies
 * of that demo still live in localStorage. This detects one: seed lines only
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

// ── Repeated system-line collapsing ─────────────────────────────────
//
// A resync failure ("Scene sync failed (HTTP 401)…") reposts on every WS
// reconnect. Verified live: during a reconnect storm those reposts are NOT
// literally back-to-back — a re-delivered geometry echo ("Created …") from
// the same reconnect lands BETWEEN them, so requiring strict array
// adjacency (the first cut of this fix) let duplicates back through and
// still fragmented the build strip. Tracked instead by exact TEXT within a
// short recency window, independent of what else was appended in between:
// the line's POSITION stays where it first appeared (never reordered to
// the end), only its `repeatCount`/`updatedAt` change. `system`-only, by
// exact text — an agent's or a user's repeated words are content, never
// merged.
const RECENT_SYSTEM_LINE_WINDOW_MS = 15_000
const recentSystemLines = new Map<string, { id: string; lastAt: number }>()

function persist(state: Pick<BlackboardState, 'lines' | 'events'>): void {
  adapter.save({ lines: state.lines, events: state.events })
}

/** No cached snapshot starts as an EMPTY notebook — the Blackboard carries
 *  only lines an agent or the user actually wrote. */
function emptyNotebook(): BlackboardSnapshot {
  return { lines: [], events: [] }
}

const initial = adapter.load() ?? emptyNotebook()

export const useBlackboardStore = create<BlackboardState>((set) => ({
  lines: initial.lines,
  events: initial.events,
  isProcessing: false,
  isPanelOpen: true,
  agentAttention: 'idle',
  streamingLineId: null,

  addLine: (text, author) => {
    const now = Date.now()
    const recent = author === 'system' ? recentSystemLines.get(text) : undefined
    if (recent && now - recent.lastAt <= RECENT_SYSTEM_LINE_WINDOW_MS) {
      let stillPresent = false
      set((state) => {
        stillPresent = state.lines.some((l) => l.id === recent.id)
        if (!stillPresent) return state
        const lines = state.lines.map((l) =>
          l.id === recent.id
            ? { ...l, updatedAt: now, repeatCount: (l.repeatCount ?? 1) + 1 }
            : l,
        )
        persist({ lines, events: state.events })
        return { lines }
      })
      if (stillPresent) {
        recentSystemLines.set(text, { id: recent.id, lastAt: now })
        return recent.id
      }
      // The tracked line was deleted (e.g. the user removed it) — fall
      // through and mint a fresh one below.
    }

    const id = nextLineId()
    if (author === 'system') recentSystemLines.set(text, { id, lastAt: now })
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
      persist({ lines, events })
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
      persist({ lines, events })
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
      persist({ lines, events })
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
      persist({ lines, events: state.events })
      return { lines }
    }),

  setProcessing: (v) => set({ isProcessing: v }),
  setAgentAttention: (attention) => set({ agentAttention: attention }),
  setStreamingLine: (id) => set({ streamingLineId: id }),
  togglePanel: () => set((s) => ({ isPanelOpen: !s.isPanelOpen })),
  setPanel: (open) => set({ isPanelOpen: open }),

  clearBoard: () =>
    set(() => {
      const lines: BlackboardLine[] = []
      const events: BlackboardEvent[] = []
      persist({ lines, events })
      return { lines, events }
    }),
}))
