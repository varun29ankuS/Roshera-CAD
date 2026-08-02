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
  describeBackendDown,
  getAcpClient,
  isAcpTurnFailureRendered,
  isBackendDownStatus,
  resetAcpClient,
  runAcpTurn,
} from '@/lib/acp-blackboard'
import {
  AcpConnectionDeadError,
  AcpHttpError,
  AcpProtocolError,
  AcpRateLimitError,
} from '@/lib/acp-client'

/**
 * Render an ACP failure that `runAcpTurn` did NOT already put on the board
 * (connect-phase errors have no placeholder agent line — `initialize`,
 * `session/new`, or the config fetch failed before a turn existed). Always
 * a system line in engineering language: what failed, and what would fix
 * it. Never falls back to another transport, never rethrows — the board
 * line IS the outcome of the turn.
 */
function renderAcpFailure(err: unknown): void {
  if (isAcpTurnFailureRendered(err)) return
  const board = useBlackboardStore.getState()
  if (err instanceof AcpHttpError) {
    if (err.status === 404 || err.status === 405) {
      // The backend answered HTTP but does not serve /acp. Either the
      // running build predates the agent surface, or it is mid-start and
      // the router isn't up yet. There is deliberately NO fallback: a
      // reply from any other path here would be an answer from something
      // that is not the agent.
      board.addLine(
        `Agent surface missing: the backend answered, but /acp returned HTTP ${err.status}. ` +
          `The api-server build is stale or still starting — restart/update the backend, ` +
          `then resend this prompt. No reload needed.`,
        'system',
      )
      return
    }
    if (err.status === 401) {
      // The global fetch interceptor (installFetchAuth) has already
      // flipped the sign-in-required signal and the LoginDialog is
      // opening — this line just makes the Blackboard's own state honest.
      board.addLine('Sign-in required to continue — see the sign-in prompt.', 'system')
      return
    }
    if (isBackendDownStatus(err.status)) {
      // A proxy sits in front of the api-server (Vite in dev, any reverse
      // proxy in prod); 502/504 is its way of saying the backend is down.
      board.addLine(describeBackendDown(err.status), 'system')
      return
    }
    board.addLine(`Agent request failed (HTTP ${err.status}): ${err.message}`, 'system')
    return
  }
  if (err instanceof AcpRateLimitError) {
    // Connect-phase 429 (initialize / session/new) — there is no turn to
    // retry yet; the cooldown gate will hold the next attempt, and
    // `runAcpTurn` will show that hold on the next turn's own line.
    board.addLine(
      `The agent surface is rate-limited right now (shared 100 requests/min budget — ` +
        `scene polling and every open Roshera tab count against it). ` +
        `Retry in about ${Math.ceil(err.retryAfterMs / 1000)}s.`,
      'system',
    )
    return
  }
  if (err instanceof AcpConnectionDeadError) {
    board.addLine(
      `Agent connection lost — the backend restarted or the event stream closed. ` +
        `The connection rebuilds itself on the next prompt; resend to continue.`,
      'system',
    )
    return
  }
  if (err instanceof TypeError) {
    // fetch's own failure mode for "could not reach the server at all".
    // The network stack never connected — the api-server is down or not
    // listening on this address. Named as a backend outage, NOT as a
    // vague network hint, and with the recovery stated: resend after the
    // backend is up; the client rebuilds itself, no reload.
    board.addLine(
      `Backend unreachable: the request to the api-server never connected ` +
        `(${err.message}). Start (or wait for) the backend, then resend this ` +
        `prompt — the connection rebuilds automatically, no reload needed.`,
      'system',
    )
    return
  }
  if (err instanceof AcpProtocolError) {
    // `.message` alone is often just the generic JSON-RPC error class
    // (e.g. "Internal error"); the actionable detail — verified live
    // against a real "Provider not set" failure from the goose agent —
    // lives in `.data`. Surface that when it's a plain string; never
    // paraphrase, per the same rule the refusal card follows.
    const detail = typeof err.data === 'string' ? err.data : err.message
    board.addLine(`Agent turn failed: ${detail}`, 'system')
    return
  }
  console.error('[ai-client] ACP turn failed:', err)
  board.addLine(
    `Agent turn failed: ${err instanceof Error ? err.message : 'unknown error'}`,
    'system',
  )
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
