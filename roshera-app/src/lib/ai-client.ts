/**
 * BLACKBOARD TURN DISPATCH — ACP only, failures are visible, success is
 * never manufactured.
 * =====================================================================
 * There used to be a second transport here: a legacy single-shot
 * `/api/ai/command[/stream]` path that `dispatchTurn` fell back to when
 * the ACP turn failed. It committed "Command processed." for any 2xx —
 * including a stream that delivered zero chunks — which is how a dead
 * agent answered the board convincingly for two full turns on 2026-08-01
 * (backend restart → stale `Acp-Connection-Id` → 404 → silent fallback)
 * and burned an evening on a network fault that did not exist. That path
 * is REMOVED, not gated: this frontend requires the `/acp` surface, and a
 * backend without one now reads as an explicit, named failure on the
 * board instead of a plausible sentence. Nothing may fail in a way that
 * looks like success.
 *
 * Recovery is layered below this module and stays bounded (one retry,
 * never a loop):
 * - stale connection (backend restart / provider repin) → `AcpClient.prompt`
 *   rebuilds the connection+session and retries the same turn once;
 * - dead client (SSE drop, failed rebuild) → `resetAcpClient()` here, and
 *   the NEXT `getAcpClient()` builds a fresh one — no reload, ever;
 * - 429 → `runAcpTurn` shows the wait on the turn's own line and retries
 *   once after the server-mandated cooldown.
 * Everything that still fails after that lands on the board in
 * engineering language via `runAcpTurn`'s own line edit or
 * `renderAcpFailure` below — exactly one line per failure, never both
 * (`isAcpTurnFailureRendered`).
 */

import { useBlackboardStore } from '@/stores/blackboard-store'
import {
  describeAcpConnectFailure,
  getAcpClient,
  isAcpTurnFailureRendered,
  resetAcpClient,
  runAcpTurn,
} from '@/lib/acp-blackboard'
import { AcpConnectionDeadError, AcpRateLimitError } from '@/lib/acp-client'

/**
 * Render an ACP failure that `runAcpTurn` did NOT already put on the board
 * (connect-phase errors have no placeholder agent line — `initialize`,
 * `session/new`, or the config fetch failed before a turn existed). Always
 * a system line in engineering language: what failed, and what would fix
 * it — copy shared with the provider dialog's connect flow via
 * `describeAcpConnectFailure` (context `'board'`: the user's prompt is
 * already on the board, so "resend it" is honest here — see that
 * function's doc for why the dialog gets different wording for the exact
 * same failures). Never falls back to another transport, never rethrows —
 * the board line IS the outcome of the turn.
 */
function renderAcpFailure(err: unknown): void {
  if (isAcpTurnFailureRendered(err)) return
  console.error('[ai-client] ACP connect-phase failure:', err)
  useBlackboardStore.getState().addLine(describeAcpConnectFailure(err, 'board'), 'system')
}

/**
 * Dispatch one turn over the ACP transport. Never throws — every failure
 * path renders its own board line (`runAcpTurn` for in-turn failures,
 * `renderAcpFailure` for connect-phase ones) — so chaining this behind the
 * serial queue below never needs a `.catch()` to keep the chain alive;
 * that's belt-and-braces only.
 */
async function dispatchTurn(text: string): Promise<void> {
  try {
    let client
    try {
      client = await getAcpClient()
    } catch (connectErr) {
      // Connect-phase 429: the app's own polling can transiently exhaust
      // the shared budget (verified live 2026-08-02 — the burst right
      // after a page load did it twice in a row). The 429 set a cooldown
      // that `rateLimitedFetch` waits out before the next request, so ONE
      // bounded retry here (a few seconds, silent, correct) turns a dead
      // turn into a served one. A second 429 — or any other failure —
      // renders below; never a loop.
      if (!(connectErr instanceof AcpRateLimitError)) throw connectErr
      client = await getAcpClient()
    }
    await runAcpTurn(client, text)
  } catch (err) {
    // A dead client can never serve another turn — discard it so the
    // next prompt transparently builds a fresh connection + session.
    if (err instanceof AcpConnectionDeadError) resetAcpClient()
    renderAcpFailure(err)
  }
}

/**
 * SERIAL TURN QUEUE
 * -----------------
 * The ACP session (`acp-client.ts`) is one prompt at a time — `session/prompt`
 * has no concurrency story on the wire, and firing a second one at the same
 * session while the first is still running would interleave two turns'
 * `session/update` streams unpredictably. So a call arriving while a turn is
 * already in flight must WAIT its turn rather than race the transport —
 * and, just as important, must never be refused outright.
 *
 * This is what makes a second `roshera:choices` card answerable while the
 * first one's turn is still running (Varun, 2026-08-01: answering one
 * choices card silently made a second, still-open one unclickable for the
 * full 60–90s the first turn ran, with no feedback that the click even
 * registered). `BlackboardLine.tsx`'s `selectChoice` rewrites the card's
 * OWN line with `selected: <value>` synchronously on click — and this
 * function appends the user's line to the board synchronously too — so the
 * answer is recorded and visible immediately regardless of queue position;
 * only the actual agent dispatch waits here.
 */
let turnQueue: Promise<void> = Promise.resolve()

/**
 * Send a user prompt, get an agent response. ACP-only: the prompt is
 * driven over the live `/acp` transport (`acp-client.ts` +
 * `acp-blackboard.ts`), which streams the agent's real reasoning/tool
 * activity into the Blackboard's `agentAttention`/streaming seams. There
 * is no other transport: if the agent cannot serve the turn, the board
 * says exactly what failed and what would fix it (see the module doc for
 * why the legacy fallback was removed). The user prompt and the agent
 * reply are appended to the Blackboard as *editable lines* rather than
 * chat bubbles.
 *
 * The user's line is added IMMEDIATELY, never delayed by a turn already in
 * flight; only the dispatch itself is serialized — see `turnQueue` above.
 */
export function processBlackboardMessage(text: string): Promise<void> {
  const board = useBlackboardStore.getState()
  board.addLine(text, 'user')

  const run = turnQueue.then(() => dispatchTurn(text))
  turnQueue = run.catch(() => undefined)
  return run
}
