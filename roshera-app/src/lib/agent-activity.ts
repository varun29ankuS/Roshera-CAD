import { useEffect, useState } from 'react'
import { getAcpSessionId } from './acp-blackboard'

/**
 * OBSERVED TURN ACTIVITY — what the agent is actually doing, from the
 * backend's own testimony.
 * =====================================================================
 * On the `claude-code` provider path ZERO `tool_call` frames reach this
 * client over ACP (tools run inside the CLI subprocess), so for months
 * the in-flight status line could only say "Waiting for the model" —
 * phases inferred from whether any text had arrived. That constraint is
 * gone: `api-server/src/agent_activity.rs` observes every operation the
 * agent performs against the server's own authenticated REST surface and
 * serves it at `GET /api/acp/activity`, with an operation-level `label`
 * ("create box", "boolean difference") or an honest `null` when the
 * route cannot be named. This module is the consumer: it polls that
 * endpoint while a turn is in flight and derives the one fact the status
 * line renders.
 *
 * # Honesty rules (inherited from the endpoint, kept here)
 * - The label is shown VERBATIM or not at all. A `null` label renders as
 *   "unnamed operation" — never the HTTP method, never the path, never a
 *   guessed name.
 * - "No operation observed yet" is a real state, distinct from idle: the
 *   backend reports `state:"active"` with `operations_this_turn: 0` when
 *   the model is thinking (or working invisibly) and nothing has reached
 *   the kernel. The line says exactly that.
 * - Sessions are matched by THIS tab's ACP session id
 *   (`getAcpSessionId`), never "the most recent session" — another tab's
 *   agent must not narrate this tab's turn.
 * - Once the backend reports the turn ended (`idle`) while this line is
 *   still streaming out its final text, the last derived activity is
 *   kept rather than regressing to "no operation observed yet" — the
 *   last observed operation remains the last observed operation.
 *
 * # Poll cadence: 5 s, aligned with the Timeline history poll
 * The endpoint is classified `RateLimitClass::Poll` (300 req/min,
 * separate from the mutation budget — `auth_middleware::POLL_PREFIXES`),
 * but the shared limits have been tripped live by open tabs before (the
 * checkpoints fetch had to move to 15 s for exactly that reason), so no
 * new fast poll: 5 s matches the existing Timeline history cadence, runs
 * ONLY while a turn is in flight (the hook is mounted by the in-flight
 * status row alone), and pauses when the tab is hidden. Worst case that
 * is 12 requests/min per tab, only during a turn.
 */

export type ObservedTurnActivity =
  | { kind: 'nothing-observed' }
  | { kind: 'operation'; label: string | null; opsThisTurn: number }

const POLL_INTERVAL_MS = 5_000

interface ActivityOperation {
  label: string | null
  at: string
}

interface ActivitySession {
  acp_session_id: string
  turn: { state: string; operations_this_turn?: number }
  recent_operations: ActivityOperation[]
}

/** Narrow, defensive read of the snapshot — a shape this parser cannot
 *  vouch for yields `null` (treated as nothing observed), never a throw
 *  and never a fabricated field. */
function parseSessions(snapshot: unknown): ActivitySession[] {
  if (typeof snapshot !== 'object' || snapshot === null) return []
  const sessions = (snapshot as { sessions?: unknown }).sessions
  if (!Array.isArray(sessions)) return []
  const out: ActivitySession[] = []
  for (const s of sessions) {
    if (typeof s !== 'object' || s === null) continue
    const rec = s as Record<string, unknown>
    const id = rec.acp_session_id
    const turn = rec.turn
    if (typeof id !== 'string' || typeof turn !== 'object' || turn === null) continue
    const turnRec = turn as Record<string, unknown>
    if (typeof turnRec.state !== 'string') continue
    const ops: ActivityOperation[] = []
    if (Array.isArray(rec.recent_operations)) {
      for (const op of rec.recent_operations) {
        if (typeof op !== 'object' || op === null) continue
        const opRec = op as Record<string, unknown>
        ops.push({
          label: typeof opRec.label === 'string' ? opRec.label : null,
          at: typeof opRec.at === 'string' ? opRec.at : '',
        })
      }
    }
    out.push({
      acp_session_id: id,
      turn: {
        state: turnRec.state,
        operations_this_turn:
          typeof turnRec.operations_this_turn === 'number'
            ? turnRec.operations_this_turn
            : undefined,
      },
      recent_operations: ops,
    })
  }
  return out
}

/**
 * Derive the activity for one session record. Exported for the module
 * test — pure, no fetch.
 *
 * `recent_operations` is ordered oldest→newest (a ring appended at the
 * back), and `operations_this_turn` counts how many of them belong to
 * the current turn — so when it is > 0 the LAST entry is this turn's
 * most recent operation.
 */
export function deriveActivity(session: ActivitySession | undefined): ObservedTurnActivity | null {
  if (!session) return { kind: 'nothing-observed' }
  const { state, operations_this_turn: opsThisTurn } = session.turn
  if (state !== 'active') {
    // The backend already saw the turn end (or never saw it start).
    // Returning null tells the hook to keep whatever it last showed —
    // never to regress a named operation back to "nothing observed".
    return state === 'idle' ? null : { kind: 'nothing-observed' }
  }
  if (!opsThisTurn || session.recent_operations.length === 0) {
    return { kind: 'nothing-observed' }
  }
  const latest = session.recent_operations[session.recent_operations.length - 1]
  return { kind: 'operation', label: latest.label, opsThisTurn }
}

/**
 * Poll the observed-activity endpoint while `active`, returning what the
 * agent is actually doing this turn. Starts as nothing-observed (true at
 * turn start, and the honest fallback when the endpoint is unreachable
 * or this tab's session is not in the snapshot).
 */
export function useObservedTurnActivity(active: boolean): ObservedTurnActivity {
  const [activity, setActivity] = useState<ObservedTurnActivity>({ kind: 'nothing-observed' })

  useEffect(() => {
    if (!active) return
    let cancelled = false

    const tick = async () => {
      if (document.visibilityState !== 'visible') return
      const sessionId = getAcpSessionId()
      if (sessionId === null) return
      try {
        const resp = await fetch('/api/acp/activity')
        if (!resp.ok) return // keep the last honest state; never invent
        const sessions = parseSessions(await resp.json())
        const derived = deriveActivity(sessions.find((s) => s.acp_session_id === sessionId))
        if (!cancelled && derived !== null) setActivity(derived)
      } catch {
        // Backend unreachable — keep the last observed state.
      }
    }

    void tick()
    const timer = setInterval(() => void tick(), POLL_INTERVAL_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [active])

  return activity
}
