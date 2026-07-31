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

  /** A prompt was just sent on the current session. */
  incrementTurns: () => void
  /** Latest `usage_update` — replaces, never accumulates (goose already
   *  reports the running context size, not a delta). */
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

  incrementTurns: () => set((s) => ({ turns: s.turns + 1 })),
  setUsage: (used, size) => set({ tokensUsed: used, contextSize: size }),
  setModel: (model) => set({ model }),
  startSession: (model) =>
    set({ model, turns: 0, tokensUsed: null, contextSize: null, live: true }),
  endSession: () =>
    set({ model: null, turns: 0, tokensUsed: null, contextSize: null, live: false }),
}))
