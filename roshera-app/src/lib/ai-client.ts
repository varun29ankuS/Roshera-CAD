import { useBlackboardStore } from '@/stores/blackboard-store'
import { useWSStore } from '@/stores/ws-store'
import {
  getAcpClient,
  isAcpTurnFailureRendered,
  resetAcpClient,
  runAcpTurn,
} from '@/lib/acp-blackboard'
import {
  AcpConnectionDeadError,
  AcpHttpError,
  AcpProtocolError,
  AcpRateLimitError,
} from '@/lib/acp-client'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

interface AICommandResponse {
  success: boolean
  cached?: boolean
  result?: {
    original_text: string
    command?: {
      original_text: string
      intent: Record<string, unknown>
      parameters: Record<string, unknown>
      confidence: number
    }
    result?: {
      status: string
      message: string
      object_id?: string
      properties?: Record<string, unknown>
    }
    execution_time_ms: number
  }
  error?: string
  execution_time_ms: number
  session_id?: string
}

export async function sendAICommand(
  command: string,
  sessionId?: string,
): Promise<AICommandResponse> {
  const response = await fetch(`${API_BASE}/ai/command`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      command,
      session_id: sessionId,
      use_cache: true,
    }),
  })

  if (!response.ok) {
    throw new Error(`AI command failed: ${response.status} ${response.statusText}`)
  }

  return response.json()
}

export async function sendAICommandStreaming(
  command: string,
  sessionId?: string,
  onChunk?: (content: string) => void,
): Promise<void> {
  const response = await fetch(`${API_BASE}/ai/command/stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      command,
      session_id: sessionId,
      stream_response: true,
    }),
  })

  if (!response.ok) {
    throw new Error(`AI stream failed: ${response.status}`)
  }

  const reader = response.body?.getReader()
  if (!reader) throw new Error('No response body')

  const decoder = new TextDecoder()
  let buffer = ''

  while (true) {
    const { done, value } = await reader.read()
    if (done) break

    buffer += decoder.decode(value, { stream: true })
    const lines = buffer.split('\n')
    buffer = lines.pop() || ''

    for (const line of lines) {
      if (line.startsWith('data: ')) {
        try {
          const data = JSON.parse(line.slice(6))
          if (data.content && onChunk) {
            onChunk(data.content)
          }
          if (data.result) {
            onChunk?.(data.result)
          }
        } catch {
          // non-JSON SSE line
        }
      }
    }
  }
}

/**
 * The legacy path: `/api/ai/command/stream` (falling back to the
 * non-streaming `/api/ai/command` on failure), driving the same
 * `addLine`/`setLineText`/`editLine` seams the ACP path uses. Kept
 * verbatim as the fallback for a backend build that doesn't carry the
 * `/acp` surface yet (404/405) or is simply unreachable (network error) —
 * see `processBlackboardMessage` below for the ACP-first dispatch.
 */
async function legacyBlackboardTurn(text: string, sessionId?: string): Promise<void> {
  const wsSession = useWSStore.getState().sessionId
  const sid = sessionId || wsSession || undefined

  const board = useBlackboardStore.getState()
  board.setProcessing(true)
  board.setAgentAttention('writing')

  // Placeholder agent line for progressive streaming. Created empty so the
  // `add` event is logged immediately; final text is committed via `editLine`.
  const lineId = board.addLine('', 'agent')
  // While marked streaming, the line renders through the buffered path that
  // never feeds KaTeX (or the card parser) an incomplete expression.
  board.setStreamingLine(lineId)
  let accumulated = ''

  // `turnStatus` here is a TURN-lifecycle marker (drives the
  // completed/cancelled/failed glyph in `BlackboardLine.tsx`), never a
  // geometry verdict — this path has no cancel control, so only
  // 'completed'/'failed' are reachable.
  const commit = (content: string, status: 'completed' | 'failed' = 'completed') => {
    useBlackboardStore.getState().editLine(lineId, content)
    useBlackboardStore.getState().setLineTurnStatus(lineId, status)
  }

  try {
    await sendAICommandStreaming(text, sid, (chunk) => {
      accumulated += chunk
      useBlackboardStore.getState().setLineText(lineId, accumulated)
    })
    commit(accumulated || 'Command processed.')
  } catch {
    try {
      const resp = await sendAICommand(text, sid)
      if (resp.success && resp.result?.result) {
        commit(resp.result.result.message || 'Command executed.')
      } else if (resp.error) {
        commit(resp.error)
      } else {
        commit('Command processed.')
      }
      if (resp.session_id) {
        useWSStore.getState().setSessionId(resp.session_id)
      }
    } catch (fallbackErr) {
      const message = fallbackErr instanceof Error ? fallbackErr.message : 'Unknown error'
      commit(`Failed to reach backend: ${message}`, 'failed')
    }
  } finally {
    const b = useBlackboardStore.getState()
    b.setStreamingLine(null)
    b.setAgentAttention('idle')
    b.setProcessing(false)
  }
}

/**
 * Classify an ACP-path failure into either a handled outcome (a system
 * line was posted; the caller does nothing further) or a signal to fall
 * back to the legacy transport. Fallback is deliberately narrow — ONLY
 * "this backend build has no `/acp` surface" (404/405) or "the backend is
 * unreachable" (a `fetch`-level `TypeError`, e.g. connection refused/CORS)
 * count. Everything else (rate limits, protocol errors, an authenticated-
 * but-refused turn) is a real ACP-path failure and is surfaced honestly
 * rather than silently retried on a different transport.
 */
function classifyAcpFailure(err: unknown): { fallback: boolean } {
  const board = useBlackboardStore.getState()
  // `runAcpTurn` (acp-blackboard.ts) already rendered a `client.prompt()`
  // failure onto the turn's own placeholder line — never a blank one, see
  // its module doc. Skip re-posting the same failure as a second system
  // line here; the fallback/reset DECISION below still runs unchanged.
  const alreadyRendered = isAcpTurnFailureRendered(err)
  if (err instanceof AcpHttpError) {
    if (err.status === 404 || err.status === 405) return { fallback: true }
    if (err.status === 401) {
      // The global fetch interceptor (installFetchAuth) has already
      // flipped the sign-in-required signal and the LoginDialog is
      // opening — this line just makes the Blackboard's own state honest.
      if (!alreadyRendered) {
        board.addLine('Sign-in required to continue — see the sign-in prompt.', 'system')
      }
      return { fallback: false }
    }
    if (!alreadyRendered) board.addLine(`Agent request failed: ${err.message}`, 'system')
    return { fallback: false }
  }
  if (err instanceof AcpRateLimitError) {
    if (!alreadyRendered) {
      board.addLine(
        `The agent surface is rate-limited right now — retry in about ${Math.ceil(err.retryAfterMs / 1000)}s.`,
        'system',
      )
    }
    return { fallback: false }
  }
  if (err instanceof TypeError) {
    // fetch's own failure mode for "could not reach the server at all".
    return { fallback: true }
  }
  if (err instanceof AcpProtocolError) {
    // `.message` alone is often just the generic JSON-RPC error class
    // (e.g. "Internal error"); the actionable detail — verified live
    // against a real "Provider not set" failure from the goose agent —
    // lives in `.data`. Surface that when it's a plain string; never
    // paraphrase, per the same rule the refusal card follows.
    const detail = typeof err.data === 'string' ? err.data : err.message
    if (!alreadyRendered) board.addLine(`Agent turn failed: ${detail}`, 'system')
    return { fallback: false }
  }
  console.error('[ai-client] ACP turn failed:', err)
  if (!alreadyRendered) {
    board.addLine(
      `Agent turn failed: ${err instanceof Error ? err.message : 'unknown error'}`,
      'system',
    )
  }
  return { fallback: false }
}

/**
 * Actually dispatch one turn over the ACP transport, falling back to the
 * legacy transport per `classifyAcpFailure`'s narrow rule. Never throws —
 * every failure path already renders its own line onto the board — so
 * chaining this behind the serial queue below never needs a `.catch()` to
 * keep the chain alive; that's belt-and-braces only.
 */
async function dispatchTurn(text: string, sessionId?: string): Promise<void> {
  try {
    const client = await getAcpClient()
    await runAcpTurn(client, text)
    return
  } catch (err) {
    if (err instanceof AcpConnectionDeadError) resetAcpClient()
    const { fallback } = classifyAcpFailure(err)
    if (!fallback) return
  }

  await legacyBlackboardTurn(text, sessionId)
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
 * Send a user prompt, get an agent response. ACP-first: the prompt is
 * driven over the live `/acp` transport (`acp-client.ts` +
 * `acp-blackboard.ts`), which streams the agent's real reasoning/tool
 * activity into the Blackboard's `agentAttention`/streaming seams. Falls
 * back to the legacy `/api/ai/command/stream` transport ONLY when the
 * backend build genuinely has no ACP surface (404/405) or is unreachable
 * — see `classifyAcpFailure`. The user prompt and the agent reply are
 * appended to the Blackboard as *editable lines* rather than chat bubbles.
 *
 * The user's line is added IMMEDIATELY, never delayed by a turn already in
 * flight; only the dispatch itself is serialized — see `turnQueue` above.
 */
export function processBlackboardMessage(text: string, sessionId?: string): Promise<void> {
  const board = useBlackboardStore.getState()
  board.addLine(text, 'user')

  const run = turnQueue.then(() => dispatchTurn(text, sessionId))
  turnQueue = run.catch(() => undefined)
  return run
}
