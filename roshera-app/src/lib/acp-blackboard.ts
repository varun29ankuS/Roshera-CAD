/**
 * ACP ↔ BLACKBOARD WIRING
 * =======================
 * Drives the Blackboard's real presentation seams (`agentAttention`,
 * `streamingLineId`, `setLineText`, `editLine` — all in
 * `stores/blackboard-store.ts`) from a live `AcpClient` (`acp-client.ts`)
 * instead of the legacy `/api/ai/command/stream` SSE path. Nothing here
 * invents a new persistence path: every `addLine`/`editLine`/`setLineText`
 * call goes through the same store reducers the legacy path used, so the
 * installed adapter (`blackboard-api.ts`) auto-persists exactly as before.
 *
 * # Session lifecycle
 * One shared `AcpClient` per browser tab, lazily created on first use and
 * kept alive across turns (a fresh `session/new` per message would throw
 * away the agent's conversational context). A dropped SSE stream cannot
 * resume (protocol contract — see `acp-client.ts`), so a dead client is
 * discarded and the next call transparently creates + connects a new one.
 *
 * # Card mapping — never weaken the renderer, always validate first
 * `blackboard-cards.ts`'s `parseCard` is the single source of truth for
 * what a `roshera:<kind>` fence may contain; this module never invents a
 * looser acceptance path. A tool call's output is only ever rendered as a
 * typed card when it VALIDATES against one of the existing schemas
 * (dfm / fcf / refusal / merge / soundness) — tried in an order that
 * favors the more specific shapes first so an ambiguous payload doesn't
 * get mis-typed. Anything that doesn't validate stays a plain system
 * line describing the tool call, never a fabricated card.
 */

import { useBlackboardStore } from '@/stores/blackboard-store'
import { useAcpSessionStore } from '@/stores/acp-session-store'
import { parseCard, type CardKind } from '@/lib/blackboard-cards'
import {
  AcpClient,
  AcpConnectionDeadError,
  AcpHttpError,
  AcpProtocolError,
  AcpRateLimitError,
  acpCooldownRemainingMs,
  type AcpSessionUpdate,
  type AcpStopReason,
} from '@/lib/acp-client'

/** "The backend behind this address is not answering." 502/504 is what the
 *  Vite dev proxy (and any reverse proxy) returns when the api-server is
 *  down — verified live 2026-08-02 by killing the backend mid-session; a
 *  raw "ACP session/prompt failed: 502" is jargon, not a diagnosis. 503 is
 *  the server itself refusing while starting up. */
export function isBackendDownStatus(status: number): boolean {
  return status === 502 || status === 503 || status === 504
}

/** The diagnosis half of "backend is down" — no recovery clause, because
 *  the right next step differs by caller (see `describeAcpConnectFailure`'s
 *  `context` param). Shared so the FACTS never drift between the two. */
function backendDownDiagnosis(status: number): string {
  return (
    `the api-server did not answer (HTTP ${status} from the proxy in front of it). ` +
    `It is down or still starting`
  )
}

/** The one sentence both render paths use for a down backend: what failed,
 *  and that recovery is resend-after-restart — never a reload. Used where a
 *  prompt genuinely IS waiting to be resent (the Blackboard, mid-turn). */
export function describeBackendDown(status: number): string {
  return (
    `Backend unreachable: ${backendDownDiagnosis(status)} — start (or wait for) the ` +
    `backend, then resend this prompt. The connection rebuilds automatically; no ` +
    `page reload is needed.`
  )
}

/** Which caller is rendering a connect-phase failure — the only thing that
 *  differs between them is the retry clause: the Blackboard already has the
 *  user's prompt sitting on the board and can honestly say "resend it"; the
 *  provider dialog's connect flow (`ProviderSettingsDialog.tsx`) never sent
 *  a prompt at all, so "resend" would be false there. */
export type AcpConnectFailureContext = 'board' | 'dialog'

/** Diagnosis + recovery clause for a connect-phase ACP failure — `initialize`,
 *  `session/new`, or the `GET /api/acp/config` cwd/provider fetch failed
 *  before any turn (or, for the dialog, any session) existed. ONE place for
 *  this copy, shared by `ai-client.ts`'s `renderAcpFailure` (the Blackboard's
 *  connect-phase line) and the provider dialog's "starting the agent
 *  harness" stage — not two texts that can silently drift apart. */
export function describeAcpConnectFailure(err: unknown, context: AcpConnectFailureContext): string {
  const retry = context === 'board' ? 'resend this prompt' : 'try again'
  if (err instanceof AcpHttpError) {
    if (err.status === 404 || err.status === 405) {
      // The backend answered HTTP but does not serve /acp. Either the
      // running build predates the agent surface, or it is mid-start and
      // the router isn't up yet. There is deliberately no fallback: an
      // answer from any other path here would be an answer from something
      // that is not the agent.
      return (
        `Agent surface missing: the backend answered, but /acp returned HTTP ${err.status}. ` +
        `The api-server build is stale or still starting — restart/update the backend, ` +
        `then ${retry}. No reload needed.`
      )
    }
    if (err.status === 401) {
      // The global fetch interceptor (installFetchAuth) has already
      // flipped the sign-in-required signal and the LoginDialog is
      // opening — this line just makes the caller's own state honest.
      return 'Sign-in required to continue — see the sign-in prompt.'
    }
    if (isBackendDownStatus(err.status)) {
      return (
        `Backend unreachable: ${backendDownDiagnosis(err.status)} — start (or wait for) the ` +
        `backend, then ${retry}. The connection rebuilds automatically; no page reload is needed.`
      )
    }
    return `Agent request failed (HTTP ${err.status}): ${err.message}`
  }
  if (err instanceof AcpRateLimitError) {
    // Connect-phase 429 (initialize / session/new) — there is no turn to
    // retry yet; the cooldown gate will hold the next attempt.
    return (
      `The agent surface is rate-limited right now (shared 100 requests/min budget — ` +
      `scene polling and every open Roshera tab count against it). ` +
      `Retry in about ${Math.ceil(err.retryAfterMs / 1000)}s.`
    )
  }
  if (err instanceof AcpConnectionDeadError) {
    return (
      `Agent connection lost — the backend restarted or the event stream closed. ` +
      `The connection rebuilds itself automatically; ${retry}.`
    )
  }
  if (err instanceof TypeError) {
    // fetch's own failure mode for "could not reach the server at all".
    return (
      `Backend unreachable: the request to the api-server never connected ` +
      `(${err.message}). Start (or wait for) the backend, then ${retry} — the connection ` +
      `rebuilds automatically, no reload needed.`
    )
  }
  if (err instanceof AcpProtocolError) {
    // `.message` alone is often just the generic JSON-RPC error class
    // (e.g. "Internal error"); the actionable detail lives in `.data`.
    const detail = typeof err.data === 'string' ? err.data : err.message
    return `Agent setup failed: ${detail}`
  }
  return `Agent setup failed: ${err instanceof Error ? err.message : 'unknown error'}`
}

const FENCE = '```'

// Tried in this order: a refusal is structurally the smallest/most
// distinctive shape (bare `reason`), so it's checked first to avoid a
// refusal payload accidentally validating against a looser schema.
const CARD_KIND_PROBE_ORDER: CardKind[] = ['refusal', 'dfm', 'fcf', 'merge', 'soundness']

/** Wrap `payload` as a `roshera:<kind>` fence IF it validates against that
 *  kind's schema; otherwise `null`. Never widens what the renderer accepts. */
function fenceIfValid(kind: CardKind, payload: unknown): string | null {
  const source = JSON.stringify(payload, null, 2)
  const parsed = parseCard(kind, source)
  return parsed.ok ? `${FENCE}roshera:${kind}\n${source}\n${FENCE}` : null
}

/** Structural sniff across every known card schema — never by tool name
 *  (a tool's declared name is not proof of its output shape). The first
 *  schema that validates wins. */
function cardFenceForPayload(payload: unknown): string | null {
  if (payload === null || typeof payload !== 'object') return null
  for (const kind of CARD_KIND_PROBE_ORDER) {
    const fence = fenceIfValid(kind, payload)
    if (fence) return fence
  }
  return null
}

/** Best-effort extraction of a tool call's structured result. Checked in
 *  order of how literal the JSON is likely to be: `rawOutput` (the MCP
 *  tool result, if the transport threads it through) first, then text
 *  content blocks that happen to parse as JSON. Anything that doesn't
 *  parse cleanly is left for `cardFenceForPayload` to reject. */
function extractToolPayload(update: {
  rawOutput?: unknown
  content?: Array<{ type: string; [key: string]: unknown }>
}): unknown {
  if (update.rawOutput !== undefined && update.rawOutput !== null) return update.rawOutput
  if (!Array.isArray(update.content)) return null
  for (const block of update.content) {
    const text =
      typeof block.text === 'string'
        ? block.text
        : typeof (block.content as { text?: unknown } | undefined)?.text === 'string'
          ? ((block.content as { text: string }).text as string)
          : null
    if (!text) continue
    try {
      return JSON.parse(text)
    } catch {
      continue
    }
  }
  return null
}

function describeStopReason(reason: AcpStopReason): string {
  switch (reason) {
    case 'end_turn':
      return ''
    case 'cancelled':
      return 'Cancelled.'
    case 'refusal':
      return 'The agent declined this request.'
    case 'max_tokens':
      return "Response truncated at the model's output limit."
    case 'max_turn_requests':
      return 'Turn stopped after reaching its tool-call budget.'
    default:
      return ''
  }
}

// ── Failed-turn rendering ────────────────────────────────────────────
//
// A `session/prompt` failure (the JSON-RPC *error* response, not a
// `session/update` notification) previously left the placeholder agent
// line — created blank by `addLine('', 'agent')` below and only ever
// filled in by the SUCCESS branch — blank forever: verified live, a turn
// that emitted `usage_update` / `available_commands_update` / two
// `session_info_update`s and then errored rendered as empty lines with no
// indication anything had failed. `describeAcpTurnFailure` + the
// try/catch in `runAcpTurn` below fix that at the source: the SAME line
// is edited to state what happened, never left blank.

/** Errors already rendered onto the Blackboard by `runAcpTurn`'s own
 *  catch, keyed by the thrown error object itself (not cloned/wrapped —
 *  `runAcpTurn` rethrows the identical instance). `ai-client.ts`'s
 *  `renderAcpFailure` checks this before posting its own system line
 *  for the same failure, so a rethrown error is never double-rendered. */
const renderedTurnFailures = new WeakSet<object>()

/** Whether `err` was already rendered onto the Blackboard by `runAcpTurn`.
 *  Exported for `ai-client.ts` — see `renderedTurnFailures`'s doc. */
export function isAcpTurnFailureRendered(err: unknown): boolean {
  return typeof err === 'object' && err !== null && renderedTurnFailures.has(err)
}

/** Errors from the agent process whose text points at reaching the MODEL
 *  PROVIDER, not at Roshera. goose reports "network"-flavoured wording for
 *  a wrong provider base URL AND for a missing TLS backend in its own
 *  build (both observed live 2026-08-01) — so the framing below never
 *  says "check your network"; it names the provider and lists the
 *  non-network causes that produce the same words. */
const PROVIDER_CONNECTIVITY_RE =
  /provider|network|connection|connect|dns|tls|certificat|timed?\s?out|unreachable|refused|handshake|proxy|overloaded/i

/** Turn a `client.prompt()` rejection into a visible line, in engineering
 *  language naming what failed and what would fix it — never an internal
 *  identifier. Prefers the existing refusal-card path when
 *  `AcpProtocolError.data` validates against it (a kernel refusal riding
 *  the JSON-RPC error payload) — never a looser acceptance path than
 *  `cardFenceForPayload` already allows. Otherwise the concrete detail:
 *  `.data` when it's a plain string (per `ai-client.ts`'s rule —
 *  `.message` alone is often just the generic JSON-RPC error class, e.g.
 *  "Internal error"), else `.message`. */
function describeAcpTurnFailure(err: unknown, provider: string | null): string {
  if (err instanceof AcpRateLimitError) {
    // Only reachable after `runAcpTurn` already waited out one cooldown
    // and retried once — this is the second consecutive 429.
    const waitS = Math.ceil(err.retryAfterMs / 1000)
    return (
      `⚠ Turn not sent: still rate-limited after one retry. The backend's shared ` +
      `request budget (100 requests/min — scene polling and every open Roshera tab ` +
      `count against it) is exhausted. Wait ~${waitS}s and resend; closing extra ` +
      `tabs frees the budget.`
    )
  }
  if (err instanceof AcpConnectionDeadError) {
    return (
      `⚠ Agent connection dropped mid-turn — the backend restarted or the event ` +
      `stream closed. Everything shown above this line did happen; nothing after ` +
      `it did. The connection rebuilds itself on the next prompt — resend to continue.`
    )
  }
  if (err instanceof AcpHttpError && isBackendDownStatus(err.status)) {
    return `⚠ ${describeBackendDown(err.status)}`
  }
  if (err instanceof AcpProtocolError) {
    if (err.data !== null && typeof err.data === 'object') {
      const fence = fenceIfValid('refusal', err.data)
      if (fence) return `⚠ Agent turn failed\n\n${fence}`
    }
    const detail = typeof err.data === 'string' ? err.data : err.message
    if (PROVIDER_CONNECTIVITY_RE.test(detail)) {
      // The JSON-RPC error arrived over a working /acp transport, so the
      // backend and the agent bridge are alive — the failure is between
      // the agent and the model provider. Say so, keep the provider's
      // words verbatim, and name the config causes that masquerade as
      // network problems.
      const who = provider ? `the "${provider}" provider` : 'the model provider'
      return (
        `⚠ Model provider failure — Roshera's backend and agent bridge are running ` +
        `(they relayed this error); ${who} did not serve the turn.\n\n` +
        `Provider error, verbatim: ${detail}\n\n` +
        `The agent reports "network" wording for a wrong provider base URL and for a ` +
        `missing TLS backend too — check the provider configuration before the network.`
      )
    }
    return `⚠ Agent turn failed: ${detail}`
  }
  if (err instanceof Error) return `⚠ Agent turn failed: ${err.message}`
  return '⚠ Agent turn failed: unknown error'
}

/**
 * Drive one prompt turn against a live, already-connected `AcpClient`,
 * wiring `session/update` notifications into the Blackboard the same way
 * the legacy streaming path drove it: a single agent line receives
 * progressive text via `setLineText`, tool activity flips
 * `agentAttention` to `'geometry'` and appends its own system line (with
 * a validated card fence when the tool's output matches a known wire
 * shape), and the turn commits a single `editLine` once `stopReason`
 * arrives. Rethrows on failure so the caller (`ai-client.ts`) can reset a
 * dead client — every failure is already rendered onto this turn's own
 * line before the rethrow; there is no fallback transport.
 */
export async function runAcpTurn(client: AcpClient, text: string): Promise<void> {
  const board = useBlackboardStore.getState()
  board.setProcessing(true)
  board.setAgentAttention('writing')
  // One prompt == one turn, counted client-side (the ACP wire carries no
  // turn counter) — session-scoped, reset by `startSession`/`endSession`
  // in `getAcpClient`/`resetAcpClient` below, never cumulative across a
  // dropped connection.
  useAcpSessionStore.getState().incrementTurns()

  const lineId = board.addLine('', 'agent')
  board.setStreamingLine(lineId)
  let accumulated = ''
  let sawContent = false
  const toolLineIds = new Map<string, string>()

  const setAttention = (a: 'idle' | 'writing' | 'geometry') =>
    useBlackboardStore.getState().setAgentAttention(a)

  const renderToolLine = (
    toolCallId: string,
    title: string,
    status: string,
    payload: unknown,
  ) => {
    const cardFence = payload !== null ? cardFenceForPayload(payload) : null
    const line = cardFence ? `⚙ ${title} — ${status}\n\n${cardFence}` : `⚙ ${title} — ${status}`
    const existing = toolLineIds.get(toolCallId)
    if (existing) {
      useBlackboardStore.getState().editLine(existing, line)
    } else {
      const id = useBlackboardStore.getState().addLine(line, 'system')
      toolLineIds.set(toolCallId, id)
    }
  }

  const unsubscribe = client.onUpdate((update: AcpSessionUpdate) => {
    switch (update.sessionUpdate) {
      case 'agent_message_chunk':
      case 'agent_thought_chunk': {
        const content = update.content
        const chunk = content.type === 'text' ? (content as { text: string }).text : ''
        if (!chunk) return
        sawContent = true
        accumulated += chunk
        useBlackboardStore.getState().setLineText(lineId, accumulated)
        return
      }
      // NOTE: `tool_call`/`tool_call_update` are the only frames that can
      // honestly drive `agentAttention = 'geometry'` (see `AgentAttention`'s
      // doc in `stores/blackboard-store.ts` for the full rejection of
      // `activeRunId` as a substitute). On the current default provider —
      // goose's `claude-code` ACP bridge — these two cases never fire:
      // verified live across two full turns, `toolCalls: 0`, because tools
      // execute inside the CLI subprocess and are never surfaced over ACP.
      // The handlers stay wired for a provider path that DOES emit them.
      case 'tool_call': {
        const status = update.status ?? 'pending'
        if (status === 'pending' || status === 'in_progress') setAttention('geometry')
        renderToolLine(update.toolCallId, update.title ?? update.kind ?? 'tool call', status, null)
        return
      }
      case 'tool_call_update': {
        const status = update.status ?? 'in_progress'
        if (status === 'pending' || status === 'in_progress') {
          setAttention('geometry')
        } else {
          // completed / failed — the agent is back to writing/reasoning
          // unless the turn has already ended (handled in `finally` below).
          setAttention('writing')
        }
        const payload = extractToolPayload(update)
        renderToolLine(update.toolCallId, update.title ?? 'tool call', status, payload)
        return
      }
      case 'usage_update': {
        // Token COUNT only — `update.cost` is never read here or
        // anywhere else in this codebase. See acp-session-store.ts's
        // module doc for why a cost figure would be dishonest on a
        // Max/Pro subscription session.
        useAcpSessionStore.getState().setUsage(update.used, update.size)
        return
      }
      case 'config_option_update': {
        // `client.currentModel` is already up to date by the time this
        // callback runs — `AcpClient.handleNotification` resolves it
        // before dispatching to subscribers.
        useAcpSessionStore.getState().setModel(client.currentModel)
        return
      }
      default:
        return // plan / session_info / available_commands — not yet surfaced
    }
  })

  // Render a failure onto the SAME line the success path would have
  // filled in — never leave it at its blank initial text. Callers rethrow
  // right after, so `ai-client.ts`'s connection-reset logic still runs;
  // `renderedTurnFailures` stops it from posting a second, duplicate
  // line for this same error.
  const renderTurnFailure = (err: unknown): void => {
    useBlackboardStore.getState().editLine(lineId, describeAcpTurnFailure(err, client.provider))
    useBlackboardStore.getState().setLineTurnStatus(lineId, 'failed')
    if (err && typeof err === 'object') renderedTurnFailures.add(err)
  }

  try {
    let stopReason: AcpStopReason
    try {
      // A previous 429 set a shared cooldown that `rateLimitedFetch`
      // sleeps out SILENTLY before sending — without this line the user
      // stares at a blank agent line for the whole wait, indistinguishable
      // from a dead turn. Sub-second remnants aren't worth a notice.
      const holdMs = acpCooldownRemainingMs()
      if (holdMs > 1000) {
        useBlackboardStore
          .getState()
          .setLineText(
            lineId,
            `Rate-limit cooldown active — holding this turn ~${Math.ceil(holdMs / 1000)}s ` +
              `for the shared request budget (100 requests/min), then sending.`,
          )
      }
      ;({ stopReason } = await client.prompt(text))
    } catch (err) {
      if (!(err instanceof AcpRateLimitError)) {
        renderTurnFailure(err)
        throw err
      }
      // 429 on the send: show the wait, then retry exactly ONCE.
      // `rateLimitedFetch` itself waits out the cooldown the 429 just set
      // before re-issuing, so this is one bounded re-send after the
      // server-mandated delay — never a loop, and a second consecutive
      // 429 surfaces via `renderTurnFailure` (which names it as
      // post-retry). The 429 happens on the POST, before the agent has
      // started the turn, so re-sending cannot double-execute anything.
      const waitS = Math.ceil(err.retryAfterMs / 1000)
      useBlackboardStore
        .getState()
        .setLineText(
          lineId,
          `Rate-limited — the shared request budget (100 requests/min, shared with scene ` +
            `polling and other open tabs) is exhausted. Waiting ~${waitS}s, then retrying ` +
            `this turn once.`,
        )
      try {
        ;({ stopReason } = await client.prompt(text))
      } catch (retryErr) {
        renderTurnFailure(retryErr)
        throw retryErr
      }
    }
    const trailing = describeStopReason(stopReason)
    const finalText = sawContent
      ? trailing
        ? `${accumulated}\n\n${trailing}`
        : accumulated
      : trailing || 'Done.'
    useBlackboardStore.getState().editLine(lineId, finalText)
    // Cancelled (user pressed Stop) gets the neutral mark, never the same
    // red cross as an error — see `AgentTurnStatus`'s doc. Every other
    // stop reason (end_turn / refusal / max_tokens / max_turn_requests)
    // means the turn concluded without erroring or being stopped, so it
    // reads as "completed" — a TURN verdict only, never a geometry one.
    useBlackboardStore
      .getState()
      .setLineTurnStatus(lineId, stopReason === 'cancelled' ? 'cancelled' : 'completed')
  } finally {
    unsubscribe()
    const b = useBlackboardStore.getState()
    b.setStreamingLine(null)
    b.setAgentAttention('idle')
    b.setProcessing(false)
  }
}

// ── Shared client lifecycle ─────────────────────────────────────────

let sharedClient: AcpClient | null = null
let sharedClientPromise: Promise<AcpClient> | null = null

async function createAndConnect(): Promise<AcpClient> {
  const client = new AcpClient()
  // Subscribed BEFORE the first newSession() so one code path serves both
  // session starts: the initial connect below AND every rebuild the
  // client makes on its own (`reestablish` after a backend restart or a
  // provider repin invalidated the connection). Without this, the header
  // kept the OLD provider mark and model after a repin — the rebuild
  // happened inside `prompt()`, past the one manual startSession call
  // that used to live here.
  //
  // `client.currentModel` is `null` when `session/new` reported no model
  // (or the unresolved "default" sentinel) — an honest, real state; the
  // header renders "—" for it rather than fabricating a name.
  // `client.provider` is resolved by the same `GET /api/acp/config` that
  // `newSession()` already made for the cwd, so the vendor mark costs no
  // extra request. Null when the backend named no provider — the header
  // then draws no logo rather than assuming one.
  client.onSessionChanged(() =>
    useAcpSessionStore.getState().startSession(client.currentModel, client.provider),
  )
  try {
    await client.initialize()
    await client.newSession()
  } catch (err) {
    // A connect that got as far as opening the connection-scoped SSE
    // stream and then failed at `session/new` must not strand that stream
    // open on a client nobody holds — close before discarding.
    client.close()
    throw err
  }
  // A stream drop (not just an explicit `resetAcpClient()` call below)
  // must also end the session in the header — a dead connection showing
  // live counts would be worse than showing nothing.
  client.onDisconnect(() => useAcpSessionStore.getState().endSession())
  return client
}

/** Returns a live, connected `AcpClient`, creating (or re-creating, after
 *  a drop) one as needed. Concurrent callers during connection share the
 *  same in-flight promise rather than racing two `session/new` calls. */
export async function getAcpClient(): Promise<AcpClient> {
  if (sharedClient && !sharedClient.isDead) return sharedClient
  if (sharedClientPromise) return sharedClientPromise
  sharedClientPromise = createAndConnect()
    .then((client) => {
      sharedClient = client
      sharedClientPromise = null
      return client
    })
    .catch((err) => {
      sharedClientPromise = null
      throw err
    })
  return sharedClientPromise
}

/** Establish the shared ACP session NOW — `initialize()` + `session/new`,
 *  same as the lazy path `getAcpClient()` has always taken on a turn's
 *  first prompt — WITHOUT sending a turn. This is the harness-start step
 *  the provider dialog's connect flow awaits: connecting a provider used
 *  to leave the agent unstarted until the user's first blackboard message,
 *  which is exactly the gap that let the chip read "connected" over a
 *  harness that was never actually running. Every side effect (the header
 *  store's `startSession`/`endSession`, the shared-client cache) is the
 *  same `createAndConnect()`/`getAcpClient()` machinery every other caller
 *  already goes through — nothing is duplicated here. */
export async function establishAcpSession(): Promise<void> {
  await getAcpClient()
}

/** Discard the shared client (a dead connection, or a caller-requested
 *  reset). The next `getAcpClient()` call creates a fresh one. */
export function resetAcpClient(): void {
  sharedClient?.close()
  sharedClient = null
  sharedClientPromise = null
  // `close()` does not fire `onDisconnect` (it's an intentional close,
  // not a drop) — end the session in the header explicitly here.
  useAcpSessionStore.getState().endSession()
}

/** Send a `session/cancel` on the shared client, if one exists and is
 *  live. Wired to the Blackboard's stop control. */
export function cancelAcpTurn(): void {
  if (sharedClient && !sharedClient.isDead) sharedClient.cancel()
}

/** The live ACP session id, or `null` when no live session exists. Used by
 *  `lib/agent-activity.ts` to pick THIS tab's session out of the
 *  `GET /api/acp/activity` snapshot — matching by id, never by "most
 *  recent" (another tab's agent must not narrate this tab's turn). */
export function getAcpSessionId(): string | null {
  return sharedClient && !sharedClient.isDead ? sharedClient.currentSessionId : null
}

export { AcpConnectionDeadError }
