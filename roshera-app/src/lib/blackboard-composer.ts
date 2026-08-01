import { useSyncExternalStore } from 'react'

/**
 * COMPOSER SEAM — the act of asking, separated from the record of having
 * asked.
 * =====================================================================
 * The Blackboard is a notebook: every line, including the user's prompt,
 * lands in the document as a normal line. That is right for the RECORD and
 * was wrong for the ACT — the old UI had no draft state, no recall, and no
 * signal when a prompt was accepted while a turn was already in flight
 * (the transport queue in `ai-client.ts` is real but silent). This module
 * carries the three pieces of state the composer needs that the notebook
 * itself must NOT carry:
 *
 *   1. TURN QUEUE VISIBILITY — `sendPrompt` wraps the serial dispatch in
 *      `ai-client.ts`'s `processBlackboardMessage` and tracks each turn
 *      from submission to settlement. The transport already serializes
 *      (one `session/prompt` at a time, FIFO); this never re-implements
 *      that ordering, it only OBSERVES it: the first unsettled entry is
 *      the turn in flight, everything after it is waiting. If the system
 *      makes you wait, it owes you the queue.
 *   2. DRAFT PERSISTENCE — per notebook scope, so an accidental blur,
 *      panel toggle, or reload never eats a half-written prompt. A draft
 *      is not a line: it does not exist in the record until sent.
 *   3. PROMPT HISTORY — what you last asked, recallable (ArrowUp in an
 *      empty composer), because the sent prompt scrolls away into the
 *      notebook and re-typing it is the alternative.
 *
 * Storage-touching helpers take an injectable `Storage`-like so the pure
 * logic is testable outside a browser; callers omit it and get
 * `window.localStorage`. `dispatch` is injected into `sendPrompt` for the
 * same reason — this module must stay importable (and provable) without
 * dragging in the transport stack.
 */

// ── Turn queue ───────────────────────────────────────────────────────

export interface QueuedTurn {
  id: string
  /** The prompt text, verbatim — shown truncated in the queue strip. */
  text: string
  enqueuedAt: number
}

let queue: readonly QueuedTurn[] = []
const listeners = new Set<() => void>()
let queueCounter = 0

function emit(): void {
  for (const l of listeners) l()
}

function subscribeTurnQueue(cb: () => void): () => void {
  listeners.add(cb)
  return () => listeners.delete(cb)
}

/** Referentially stable between mutations — safe for useSyncExternalStore. */
export function getTurnQueue(): readonly QueuedTurn[] {
  return queue
}

/**
 * Everything except the head. The dispatch behind `sendPrompt` is strictly
 * FIFO (see `ai-client.ts`'s `turnQueue`), so the head entry is the turn in
 * flight — already signalled by the in-flight status row on the agent's own
 * line — and only the rest are "received, waiting". Pure; exported for the
 * real-module test.
 */
export function waitingTurns(q: readonly QueuedTurn[]): readonly QueuedTurn[] {
  return q.slice(1)
}

/**
 * Send a prompt through `dispatch` (in the app: `processBlackboardMessage`)
 * while tracking it in the visible queue for its whole lifetime. The entry
 * is removed when the turn settles — success or failure equally, because
 * failures already render their own line on the board and a queue entry
 * that lingered after its turn died would be a claim the transport does
 * not back.
 */
export function sendPrompt(
  text: string,
  dispatch: (text: string) => Promise<void>,
): Promise<void> {
  const entry: QueuedTurn = {
    id: `qt-${Date.now().toString(36)}-${++queueCounter}`,
    text,
    enqueuedAt: Date.now(),
  }
  queue = [...queue, entry]
  emit()
  const run = dispatch(text)
  const settle = () => {
    queue = queue.filter((q) => q.id !== entry.id)
    emit()
  }
  run.then(settle, settle)
  return run
}

/** React binding for the queue — one hook, no store dependency. */
export function useTurnQueue(): readonly QueuedTurn[] {
  return useSyncExternalStore(subscribeTurnQueue, getTurnQueue)
}

// ── Draft persistence (per notebook scope) ───────────────────────────

type StorageLike = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

function defaultStorage(): StorageLike | null {
  return typeof window === 'undefined' ? null : window.localStorage
}

const DRAFT_PREFIX = 'roshera.blackboard.composer-draft.v1'

/** Pure; exported for the real-module test. */
export function draftStorageKey(scope: string): string {
  return `${DRAFT_PREFIX}.${scope}`
}

export function loadDraft(scope: string, storage: StorageLike | null = defaultStorage()): string {
  if (!storage) return ''
  try {
    return storage.getItem(draftStorageKey(scope)) ?? ''
  } catch {
    return ''
  }
}

/** A blank draft is REMOVED, not stored — an empty key is not a draft. */
export function saveDraft(
  scope: string,
  text: string,
  storage: StorageLike | null = defaultStorage(),
): void {
  if (!storage) return
  try {
    if (text === '') storage.removeItem(draftStorageKey(scope))
    else storage.setItem(draftStorageKey(scope), text)
  } catch {
    // Quota/private-mode failure — the in-memory draft still lives in the
    // composer's own state for this session.
  }
}

// ── Prompt history ───────────────────────────────────────────────────
//
// One global history (not per scope): recall follows the person, not the
// part — shell muscle memory, and the common case is re-asking the same
// thing of a different part.

const HISTORY_KEY = 'roshera.blackboard.prompt-history.v1'
export const PROMPT_HISTORY_CAP = 50

/**
 * Append a sent prompt: trimmed, deduplicated (a re-sent prompt moves to
 * most-recent rather than stacking), capped to the newest
 * `PROMPT_HISTORY_CAP`. Oldest first, newest last — ArrowUp walks from the
 * end. Pure; exported for the real-module test.
 */
export function pushPromptHistory(
  history: readonly string[],
  text: string,
  cap: number = PROMPT_HISTORY_CAP,
): string[] {
  const trimmed = text.trim()
  if (!trimmed) return [...history]
  const next = [...history.filter((h) => h !== trimmed), trimmed]
  return next.slice(Math.max(0, next.length - cap))
}

export function loadPromptHistory(storage: StorageLike | null = defaultStorage()): string[] {
  if (!storage) return []
  try {
    const raw = storage.getItem(HISTORY_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed.filter((p): p is string => typeof p === 'string')
  } catch {
    return []
  }
}

export function savePromptHistory(
  history: readonly string[],
  storage: StorageLike | null = defaultStorage(),
): void {
  if (!storage) return
  try {
    storage.setItem(HISTORY_KEY, JSON.stringify(history))
  } catch {
    // Non-fatal — recall degrades to this session only.
  }
}
