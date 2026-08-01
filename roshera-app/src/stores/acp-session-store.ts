import { create } from 'zustand'

/**
 * LIVE ACP SESSION STATS — for the Blackboard header
 * ====================================================
 * Display-only. goose already emits everything here on the `/acp` stream
 * (`api-server/src/goose_acp.rs`); this store just holds the latest values
 * so `Blackboard.tsx` can render them without owning `AcpClient` itself.
 * Wired from `lib/acp-blackboard.ts`, the only module that touches the
 * shared `AcpClient` lifecycle.
 *
 * Honesty rules this store exists to enforce (never relaxed for a nicer
 * header):
 *   - `model` is `null` whenever no session exists, OR the session's
 *     model is still goose's unresolved `"default"` sentinel — see
 *     `resolveModelFromConfigOptions` (`lib/acp-client.ts`), which is the
 *     ONLY place that string is produced. Never printed as if it were a
 *     real model name.
 *   - `turns` counts prompts sent on the CURRENT session only — reset by
 *     `startSession`, never cumulative across a dropped/recreated
 *     connection.
 *   - `tokensUsed` is `usage_update.used` — a token COUNT, not a cost.
 *     goose's `usage_update.cost` field is deliberately never read
 *     anywhere in this codebase: goose cannot distinguish
 *     subscription-vs-API billing, so a dollar figure would be fiction on
 *     a Max/Pro session. If you are tempted to add a cost display, don't
 *     — read the module doc on `acp-client.ts`'s `AcpSessionUpdate` type
 *     first.
 *   - `live` is true only while a session is connected. `endSession()`
 *     resets every counter to its "no session" value — a header reading
 *     stale numbers from a dead connection is worse than reading nothing.
 */
interface AcpSessionStats {
  model: string | null
  turns: number
  tokensUsed: number | null
  contextSize: number | null
  live: boolean

  /** Session total at the moment the current turn's prompt was sent.
   *  Internal bookkeeping for `lastTurnTokens`; not for display. */
  turnStartTokens: number | null

  /** Tokens consumed by the MOST RECENT turn alone, derived as
   *  `tokensUsed - turnStartTokens`.
   *
   *  This exists because the session total, shown by itself next to a
   *  just-completed action, reads as the price of that action. Measured
   *  2026-08-01: "clear the viewport" made exactly two tool calls and
   *  answered in 51 characters, while the header showed ~70k — a figure
   *  accumulated over three prompts, most of it context re-sent on each
   *  round-trip. Nothing displayed was false; it was unlabelled, which
   *  was enough to mislead.
   *
   *  `null` until a turn completes, and null rather than a negative if
   *  the total ever moves backwards (a session restart resets it): a
   *  number we cannot derive honestly is not shown.
   *
   *  ⚠ Still a COUNT, not a cost, and still not a breakdown. `used` is one
   *  scalar mixing fresh input, output and cached context; the wire
   *  carries no split, and cached reads bill far below fresh input. So
   *  this says how much moved, never how much it cost. */
  lastTurnTokens: number | null

  /** A prompt was just sent on the current session. */
  incrementTurns: () => void
  /** Latest `usage_update` — replaces, never accumulates. `used` is itself
   *  a CUMULATIVE token count across the whole session (not a per-update
   *  delta) and `size` is the static context window; the two are not
   *  comparable and `used` legitimately exceeds `size` on a long session
   *  (measured live: used=359346, size=128000). Never derive a percentage
   *  from them — see `Blackboard.tsx`'s `formatContextUsage` doc. */
  setUsage: (used: number, size: number) => void
  /** A late-resolving `config_option_update` corrected the model. */
  setModel: (model: string | null) => void
  /** A new session was created — resets every counter. */
  startSession: (model: string | null) => void
  /** The session ended (stream dropped, or an explicit reset). */
  endSession: () => void
}

export const useAcpSessionStore = create<AcpSessionStats>((set) => ({
  model: null,
  turns: 0,
  tokensUsed: null,
  contextSize: null,
  live: false,
  turnStartTokens: null,
  lastTurnTokens: null,

  // Sending a prompt opens a new turn: freeze the running total as this
  // turn's baseline and drop the previous turn's figure, so a stale number
  // never sits next to work in flight.
  incrementTurns: () =>
    set((s) => ({
      turns: s.turns + 1,
      turnStartTokens: s.tokensUsed ?? 0,
      lastTurnTokens: null,
    })),

  setUsage: (used, size) =>
    set((s) => {
      const base = s.turnStartTokens
      // A backwards total means the counter reset under us (session
      // restart). There is no honest delta across that discontinuity, so
      // report none and re-baseline rather than render a negative.
      const delta = base === null || used < base ? null : used - base
      return {
        tokensUsed: used,
        contextSize: size,
        turnStartTokens: base !== null && used < base ? used : base,
        lastTurnTokens: delta,
      }
    }),

  setModel: (model) => set({ model }),
  startSession: (model) =>
    set({
      model,
      turns: 0,
      tokensUsed: null,
      contextSize: null,
      live: true,
      turnStartTokens: null,
      lastTurnTokens: null,
    }),
  endSession: () =>
    set({
      model: null,
      turns: 0,
      tokensUsed: null,
      contextSize: null,
      live: false,
      turnStartTokens: null,
      lastTurnTokens: null,
    }),
}))
