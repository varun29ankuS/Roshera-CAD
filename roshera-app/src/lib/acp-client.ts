/**
 * ACP CLIENT — JSON-RPC 2.0 over HTTP + Server-Sent Events
 * =========================================================
 * Talks to the backend's `/acp` surface (goose's Agent Client Protocol
 * bridge — see `api-server/src/goose_acp.rs` / `api-server/src/acp_gate.rs`).
 * This is a from-scratch client, not a wrapper around `EventSource`: SSE
 * with a custom `Acp-Connection-Id` / `Acp-Session-Id` header requires
 * `fetch` + a manual `ReadableStream` reader, since `EventSource` cannot
 * set request headers.
 *
 * # Wire shape (verified live against the running backend)
 * - `POST /acp` `initialize` → 200, the JSON-RPC response is in the body,
 *   and the `Acp-Connection-Id` response header is the connection's
 *   identity for every subsequent call.
 * - `POST /acp` `session/new` `{cwd, mcpServers}` → 202. The JSON-RPC
 *   *response* to this call does NOT come back in the POST body — it
 *   arrives as a message on the **connection-scoped** SSE stream (a `GET
 *   /acp` with only `Acp-Connection-Id`, no `Acp-Session-Id`).
 * - Once a session exists, a second, **session-scoped** SSE stream (`GET
 *   /acp` with both `Acp-Connection-Id` and `Acp-Session-Id`) carries
 *   `session/prompt` responses, `session/update` notifications (the
 *   agent's streamed thinking/text/tool-call activity), and server→client
 *   requests (permission prompts, fs/* calls this client structurally
 *   never honors beyond what's below).
 * - The server buffers each stream (cap 1024) until its first subscriber,
 *   so POST-then-GET is race-free — the response/notification a POST
 *   provokes is never lost even if the GET starts a beat later.
 * - A dropped SSE connection cannot be resumed (no `Last-Event-ID`
 *   replay on this surface) — this client treats any stream failure as
 *   connection-dead and requires a fresh `initialize()`.
 *
 * # What this client intentionally does NOT do
 * - It never sets `Authorization` itself. Per `main.tsx`, `installFetchAuth()`
 *   patches the *global* `fetch` before any component mounts, and every
 *   call here goes through that same global `fetch` — the bearer token
 *   (and the 401 → sign-in-required signal) are already handled centrally.
 *   Capturing `fetch` locally here would silently bypass both.
 * - It never retries a 429 in a loop. The shared 100 req/min budget is
 *   shared with the polling frontend (`blackboard-api.ts`'s poll), so a
 *   naive retry storm would starve it. Instead a single shared cooldown
 *   gate delays the NEXT request until the `Retry-After` window elapses,
 *   and a request that still lands inside an active cooldown throws
 *   `AcpRateLimitError` verbatim for the caller to surface — never a
 *   silently-swallowed retry.
 * - It never approves anything beyond `allow_once` and never answers an
 *   unrecognized server→client method by hanging — session mode is
 *   `auto` (no human approval round-trip is expected), so the one
 *   real-world case (`session/request_permission`) is answered
 *   immediately and everything else gets a typed JSON-RPC `-32601`.
 */

// ── JSON-RPC message shapes ─────────────────────────────────────────────

export type JsonRpcId = number | string

export interface JsonRpcErrorPayload {
  code: number
  message: string
  data?: unknown
}

interface JsonRpcRequestMsg {
  jsonrpc: '2.0'
  id: JsonRpcId
  method: string
  params?: unknown
}

interface JsonRpcResponseMsg {
  jsonrpc: '2.0'
  id: JsonRpcId
  result?: unknown
  error?: JsonRpcErrorPayload
}

interface JsonRpcNotificationMsg {
  jsonrpc: '2.0'
  method: string
  params?: unknown
}

// ── Session update / content shapes (Agent Client Protocol) ────────────

export interface AcpTextContent {
  type: 'text'
  text: string
}

/** A content block. Only `text` is interpreted; other block types
 *  (image, audio, resource) pass through untouched for a future slice. */
export type AcpContentBlock = AcpTextContent | { type: string; [key: string]: unknown }

export type AcpToolCallStatus = 'pending' | 'in_progress' | 'completed' | 'failed'

/** `session/update` notification payloads. The `tool_call` /
 *  `tool_call_update` fields are read defensively (all optional beyond
 *  the discriminant + id) since this is a live-verified transport but an
 *  unverified detailed tool-call schema — callers must not assume a field
 *  is present. */
/** One entry of `session/new`'s (or `config_option_update`'s) `configOptions`
 *  array (`agent-client-protocol-schema`'s `SessionConfigOption`, flattened
 *  `SessionConfigSelect`). Only `id` + `currentValue` are interpreted here;
 *  everything else (`options`, `category`, …) passes through untouched. */
export interface AcpSessionConfigOption {
  id: string
  currentValue?: string
  [key: string]: unknown
}

export type AcpSessionUpdate =
  | { sessionUpdate: 'user_message_chunk'; content: AcpContentBlock }
  | { sessionUpdate: 'agent_message_chunk'; content: AcpContentBlock }
  | { sessionUpdate: 'agent_thought_chunk'; content: AcpContentBlock }
  | {
      sessionUpdate: 'tool_call'
      toolCallId: string
      title?: string
      kind?: string
      status?: AcpToolCallStatus
      content?: Array<{ type: string; [key: string]: unknown }>
      rawInput?: unknown
      rawOutput?: unknown
    }
  | {
      sessionUpdate: 'tool_call_update'
      toolCallId: string
      title?: string
      status?: AcpToolCallStatus
      content?: Array<{ type: string; [key: string]: unknown }>
      rawOutput?: unknown
    }
  | { sessionUpdate: 'plan'; entries?: unknown[] }
  | { sessionUpdate: 'available_commands_update'; availableCommands?: unknown[] }
  | { sessionUpdate: 'current_mode_update'; currentModeId?: string }
  | { sessionUpdate: 'config_option_update'; configOptions?: AcpSessionConfigOption[] }
  | { sessionUpdate: 'session_info_update'; title?: string | null; updatedAt?: string | null }
  | {
      /** Token usage for the CURRENT session only — resets whenever the
       *  ACP stream drops (a fresh session starts from zero). `used` is a
       *  CUMULATIVE count of tokens consumed so far this session; `size`
       *  is the static context window. They are NOT directly comparable —
       *  `used` legitimately exceeds `size` once a session runs long
       *  (measured live: `{"used":359346,"size":128000}`), so used/size is
       *  not a valid occupancy percentage. No field observed on this wire
       *  reports current context occupancy; callers must render `used`
       *  alone, never a fabricated ratio. `cost` (present when goose can
       *  compute one) is deliberately never read anywhere in this client —
       *  goose cannot know subscription-vs-API billing, so a dollar figure
       *  would be fiction on a Max/Pro session. */
      sessionUpdate: 'usage_update'
      used: number
      size: number
      cost?: unknown
    }

/** Read a session's currently-resolved model out of a `session/new`
 *  response or a `config_option_update` notification's `configOptions`
 *  array. `"default"` is `claude-code`'s own not-yet-resolved sentinel
 *  (`CLAUDE_CODE_DEFAULT_MODEL`), never a real model name — callers must
 *  not print it as one, so this returns `null` for it exactly like the
 *  "no model option present" case. */
export function resolveModelFromConfigOptions(
  configOptions: AcpSessionConfigOption[] | undefined,
): string | null {
  const current = configOptions?.find((o) => o.id === 'model')?.currentValue
  return current && current !== 'default' ? current : null
}

export type AcpStopReason =
  | 'end_turn'
  | 'max_tokens'
  | 'max_turn_requests'
  | 'refusal'
  | 'cancelled'
  | string

// ── Typed errors ─────────────────────────────────────────────────────

/** An HTTP-level failure from the `/acp` transport itself (not a JSON-RPC
 *  error). Callers branch on `.status` — 404/405 mean "no ACP surface on
 *  this backend build", anything else is a genuine failure to surface. */
export class AcpHttpError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = 'AcpHttpError'
    this.status = status
  }
}

/** A JSON-RPC-level error (from either the initialize body or a matched
 *  SSE response/error). */
export class AcpProtocolError extends Error {
  readonly code: number
  readonly data?: unknown
  constructor(code: number, message: string, data?: unknown) {
    super(message)
    this.name = 'AcpProtocolError'
    this.code = code
    this.data = data
  }
}

/** 429 from `/acp`, surfaced verbatim rather than retried blindly. */
export class AcpRateLimitError extends Error {
  readonly retryAfterMs: number
  constructor(retryAfterMs: number, message?: string) {
    super(message ?? `ACP rate-limited — retry after ${Math.ceil(retryAfterMs / 1000)}s`)
    this.name = 'AcpRateLimitError'
    this.retryAfterMs = retryAfterMs
  }
}

/** The connection's SSE stream(s) dropped, or a request was attempted
 *  after that happened. A dropped stream cannot resume by protocol
 *  contract — the caller must discard this client and initialize a new one. */
export class AcpConnectionDeadError extends Error {
  constructor(message?: string) {
    super(message ?? 'ACP connection is dead — a fresh initialize() is required')
    this.name = 'AcpConnectionDeadError'
  }
}

// ── Rate-limit backoff (module-scoped: shared across every AcpClient
//    instance, since the 100 req/min budget is per-identity, not per-object) ──

let cooldownUntilMs = 0
const DEFAULT_RATE_LIMIT_COOLDOWN_MS = 5000

async function sleep(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms))
}

/** Wraps `fetch` (the global, already-patched-for-auth one) with the
 *  shared 429 cooldown gate. Waits out any active cooldown before
 *  issuing a request; a 429 on the request itself sets the next
 *  cooldown and throws `AcpRateLimitError` — no automatic re-issue. */
async function rateLimitedFetch(input: string, init: RequestInit): Promise<Response> {
  const now = Date.now()
  if (now < cooldownUntilMs) {
    await sleep(cooldownUntilMs - now)
  }
  const res = await fetch(input, init)
  if (res.status === 429) {
    const retryAfterHeader = res.headers.get('Retry-After')
    const parsed = retryAfterHeader ? Number(retryAfterHeader) * 1000 : NaN
    const ms = Number.isFinite(parsed) && parsed > 0 ? parsed : DEFAULT_RATE_LIMIT_COOLDOWN_MS
    cooldownUntilMs = Date.now() + ms
    throw new AcpRateLimitError(ms)
  }
  return res
}

async function safeText(res: Response): Promise<string> {
  try {
    return await res.text()
  } catch {
    return ''
  }
}

// ── SSE stream reader (fetch + manual parse; EventSource can't carry headers) ──

class AcpSseStream {
  private readonly controller = new AbortController()
  private started = false
  private readonly url: string
  private readonly headers: Record<string, string>
  private readonly onEvent: (data: string) => void
  private readonly onDone: (err?: Error) => void

  constructor(
    url: string,
    headers: Record<string, string>,
    onEvent: (data: string) => void,
    onDone: (err?: Error) => void,
  ) {
    this.url = url
    this.headers = headers
    this.onEvent = onEvent
    this.onDone = onDone
  }

  async start(): Promise<void> {
    if (this.started) return
    this.started = true
    try {
      const res = await rateLimitedFetch(this.url, {
        method: 'GET',
        headers: { Accept: 'text/event-stream', ...this.headers },
        signal: this.controller.signal,
      })
      if (!res.ok || !res.body) {
        throw new AcpHttpError(res.status, `ACP SSE GET failed: ${res.status} ${await safeText(res)}`)
      }
      const reader = res.body.getReader()
      const decoder = new TextDecoder()
      let buffer = ''
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, '\n')
        let boundary: number
        while ((boundary = buffer.indexOf('\n\n')) !== -1) {
          const rawEvent = buffer.slice(0, boundary)
          buffer = buffer.slice(boundary + 2)
          const dataLines = rawEvent
            .split('\n')
            .filter((line) => line.startsWith('data:'))
            .map((line) => line.slice(5).replace(/^ /, ''))
          if (dataLines.length > 0) this.onEvent(dataLines.join('\n'))
        }
      }
      this.onDone()
    } catch (err) {
      if (this.controller.signal.aborted) return // intentional close, not a drop
      this.onDone(err instanceof Error ? err : new Error(String(err)))
    }
  }

  close(): void {
    this.controller.abort()
  }
}

// ── The client ──────────────────────────────────────────────────────

export interface AcpClientOptions {
  /** The `/acp` path. Overridable for tests; defaults to same-origin `/acp`. */
  acpPath?: string
  /** Absolute working directory for `session/new`. The backend OWNS this
   *  path (`goose_acp::agent_workspace_dir` — the same directory it writes
   *  `.goosehints` into) and serves it over `GET /api/acp/config`;
   *  `newSession()` fetches it lazily on first use. Pass this option (or
   *  set `VITE_ACP_CWD`) only to override that for an unusual setup — it
   *  must never be relied on as the default source of truth, since a
   *  frontend value and a backend value that can silently drift apart is
   *  exactly the defect class this client used to carry. */
  cwd?: string
}

type PendingEntry = { resolve: (value: unknown) => void; reject: (err: Error) => void }

const DEFAULT_ACP_PATH = '/acp'

export class AcpClient {
  private readonly acpPath: string
  /** Explicit override only (ctor option or `VITE_ACP_CWD`) — empty means
   *  "ask the backend", resolved and cached lazily by {@link resolveCwd}.
   *  Not `readonly`: the backend-resolved value is written into this same
   *  field on first use so every subsequent `newSession()` reuses it —
   *  and {@link reestablish} clears it again (backend-resolved case only)
   *  so a rebuild re-asks the backend and picks up a provider change. */
  private cwd: string
  /** Whether `cwd` came from the ctor/`VITE_ACP_CWD` (never cleared) or
   *  from the backend (cleared on {@link reestablish} to force a fresh
   *  `GET /api/acp/config`, which also re-resolves {@link provider}). */
  private readonly cwdIsExplicitOverride: boolean
  private connectionId: string | null = null
  private sessionId: string | null = null
  private _currentModel: string | null = null
  /** Provider id the backend reported alongside `cwd`. See {@link activeProvider}. */
  private activeProvider: string | null = null
  private nextRequestId = 1
  private readonly pending = new Map<JsonRpcId, PendingEntry>()
  private connStream: AcpSseStream | null = null
  private sessStream: AcpSseStream | null = null
  private dead = false
  /** True while {@link reestablish} is rebuilding the connection —
   *  guards against a reconnect attempt triggering another reconnect. */
  private reconnecting = false
  /** The `mcpServers` array the caller last passed to {@link newSession},
   *  so a stale-connection rebuild recreates the session with the same
   *  wiring rather than silently dropping the MCP servers. */
  private lastMcpServers: unknown[] = []
  private readonly updateHandlers = new Set<(update: AcpSessionUpdate) => void>()
  private readonly disconnectHandlers = new Set<(reason: string) => void>()
  /** Fired after every successful {@link newSession} — including the one
   *  {@link reestablish} makes after the backend invalidated us (restart
   *  OR provider repin), which is exactly when `currentModel`/`provider`
   *  change under a subscriber's feet. UI that mirrors those must resync
   *  here, not only at first connect. */
  private readonly sessionChangedHandlers = new Set<() => void>()

  constructor(opts: AcpClientOptions = {}) {
    this.acpPath = opts.acpPath ?? DEFAULT_ACP_PATH
    this.cwd = opts.cwd ?? (import.meta.env.VITE_ACP_CWD as string | undefined) ?? ''
    this.cwdIsExplicitOverride = this.cwd !== ''
  }

  get isDead(): boolean {
    return this.dead
  }

  get currentSessionId(): string | null {
    return this.sessionId
  }

  /** The model `session/new` resolved for this session, or `null` when
   *  none was reported (a real, honest state — `build_session_setup_config`
   *  on the backend returns no `configOptions` for a provider it has no
   *  inventory entry for, e.g. `claude-code`) or it is still the
   *  unresolved `"default"` sentinel. Never a guess. */
  get currentModel(): string | null {
    return this._currentModel
  }

  /** The provider id backing this session (`anthropic`, `kimi`, …) as
   *  reported by `GET /api/acp/config`, or `null` before the first
   *  `newSession()` resolves it or when the backend named none. Drives the
   *  header's vendor mark ONLY — a null draws no logo rather than a
   *  default one, since the mark is a claim about who is serving the
   *  model and a wrong logo is a worse answer than no logo. */
  get provider(): string | null {
    return this.activeProvider
  }

  /** Subscribe to `session/update` notifications. Returns an unsubscribe fn. */
  onUpdate(cb: (update: AcpSessionUpdate) => void): () => void {
    this.updateHandlers.add(cb)
    return () => this.updateHandlers.delete(cb)
  }

  /** Fires once when either SSE stream drops (including an intentional
   *  `close()` — callers that care about the difference check `isDead`
   *  themselves before calling `close()`). */
  onDisconnect(cb: (reason: string) => void): () => void {
    this.disconnectHandlers.add(cb)
    return () => this.disconnectHandlers.delete(cb)
  }

  /** Subscribe to session (re)creation — fires after every successful
   *  `newSession()`, first connect and rebuild alike. Read
   *  `currentModel`/`provider` inside the callback: both are already
   *  fresh when it fires. Returns an unsubscribe fn. */
  onSessionChanged(cb: () => void): () => void {
    this.sessionChangedHandlers.add(cb)
    return () => this.sessionChangedHandlers.delete(cb)
  }

  private markDead(reason: string): void {
    if (this.dead) return
    this.dead = true
    for (const entry of this.pending.values()) entry.reject(new AcpConnectionDeadError(reason))
    this.pending.clear()
    for (const cb of this.disconnectHandlers) cb(reason)
  }

  private headersFor(includeSession: boolean): Record<string, string> {
    const headers: Record<string, string> = {}
    if (this.connectionId) headers['Acp-Connection-Id'] = this.connectionId
    if (includeSession && this.sessionId) headers['Acp-Session-Id'] = this.sessionId
    return headers
  }

  private async post(body: unknown, includeSession: boolean): Promise<Response> {
    if (this.dead) throw new AcpConnectionDeadError()
    return rateLimitedFetch(this.acpPath, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...this.headersFor(includeSession) },
      body: JSON.stringify(body),
    })
  }

  private handleInbound(raw: string): void {
    let msg: unknown
    try {
      msg = JSON.parse(raw)
    } catch {
      return // a non-JSON-RPC keepalive/comment line — ignore, don't crash the stream
    }
    if (typeof msg !== 'object' || msg === null) return
    const m = msg as Record<string, unknown>
    if ('result' in m || 'error' in m) {
      this.handleResponse(m as unknown as JsonRpcResponseMsg)
      return
    }
    if (typeof m.method === 'string' && 'id' in m) {
      void this.handleServerRequest(m as unknown as JsonRpcRequestMsg)
      return
    }
    if (typeof m.method === 'string') {
      this.handleNotification(m as unknown as JsonRpcNotificationMsg)
    }
  }

  private handleResponse(resp: JsonRpcResponseMsg): void {
    const entry = this.pending.get(resp.id)
    if (!entry) return
    this.pending.delete(resp.id)
    if (resp.error) {
      entry.reject(new AcpProtocolError(resp.error.code, resp.error.message, resp.error.data))
    } else {
      entry.resolve(resp.result)
    }
  }

  private handleNotification(note: JsonRpcNotificationMsg): void {
    if (note.method !== 'session/update') return
    const params = note.params as { sessionId?: string; update?: AcpSessionUpdate } | undefined
    if (!params?.update) return
    // A model chosen late (e.g. resolved after the provider inventory
    // loads) corrects `currentModel` here, ahead of notifying
    // subscribers — so a subscriber reading `client.currentModel` from
    // inside its own callback for this same event sees the fresh value.
    if (params.update.sessionUpdate === 'config_option_update') {
      this._currentModel = resolveModelFromConfigOptions(params.update.configOptions)
    }
    for (const cb of this.updateHandlers) cb(params.update)
  }

  /** Server→client requests. `auto` session mode means no human approval
   *  round-trip is expected, so a permission prompt is answered
   *  immediately with `allow_once`; anything this client doesn't
   *  recognize gets a typed JSON-RPC `-32601` rather than hanging the
   *  agent's turn forever. */
  private async handleServerRequest(req: JsonRpcRequestMsg): Promise<void> {
    if (req.method === 'session/request_permission') {
      const params = req.params as
        | { options?: Array<{ optionId: string; kind?: string }> }
        | undefined
      const chosen =
        params?.options?.find((o) => o.kind === 'allow_once') ?? params?.options?.[0]
      await this.respondToServer(req.id, {
        outcome: { outcome: 'selected', optionId: chosen?.optionId ?? 'allow_once' },
      })
      return
    }
    await this.respondToServer(req.id, undefined, {
      code: -32601,
      message: `Method not found: ${req.method}`,
    })
  }

  private async respondToServer(
    id: JsonRpcId,
    result?: unknown,
    error?: JsonRpcErrorPayload,
  ): Promise<void> {
    const body: JsonRpcResponseMsg = error
      ? { jsonrpc: '2.0', id, error }
      : { jsonrpc: '2.0', id, result }
    try {
      await this.post(body, true)
    } catch {
      // Best-effort: a failed reply to a server-initiated request must
      // not throw out of an SSE event handler.
    }
  }

  /** A JSON-RPC request. `initialize` answers synchronously in the POST
   *  body (200); `session/new` and `session/prompt` answer 202 and
   *  deliver their result asynchronously over SSE — the promise this
   *  returns is resolved by `handleResponse` when that arrives. */
  private async requestAsync(
    method: string,
    params: unknown,
    includeSession: boolean,
  ): Promise<unknown> {
    const id = this.nextRequestId++
    const body: JsonRpcRequestMsg = { jsonrpc: '2.0', id, method, params }
    const waiter = new Promise<unknown>((resolve, reject) => this.pending.set(id, { resolve, reject }))
    const res = await this.post(body, includeSession)
    if (res.status === 200) {
      this.pending.delete(id)
      const json = (await res.json()) as JsonRpcResponseMsg
      if (json.error) throw new AcpProtocolError(json.error.code, json.error.message, json.error.data)
      return json.result
    }
    if (res.status !== 202) {
      this.pending.delete(id)
      throw new AcpHttpError(res.status, `ACP ${method} failed: ${res.status} ${await safeText(res)}`)
    }
    return waiter
  }

  private notify(method: string, params: unknown, includeSession: boolean): void {
    const body: JsonRpcNotificationMsg = { jsonrpc: '2.0', method, params }
    void this.post(body, includeSession).catch(() => {
      // A dropped cancel notification is not worth surfacing — the turn
      // either already finished or the connection is already dead.
    })
  }

  /**
   * Step 1. `POST /acp` `initialize` (200, `Acp-Connection-Id` header),
   * then open the connection-scoped SSE stream (no `Acp-Session-Id`)
   * that will carry `session/new`'s response.
   */
  async initialize(): Promise<void> {
    const id = this.nextRequestId++
    const body: JsonRpcRequestMsg = {
      jsonrpc: '2.0',
      id,
      method: 'initialize',
      params: {
        protocolVersion: 1,
        clientCapabilities: { fs: { readTextFile: false, writeTextFile: false } },
      },
    }
    const res = await this.post(body, false)
    if (!res.ok) {
      throw new AcpHttpError(res.status, `ACP initialize failed: ${res.status} ${await safeText(res)}`)
    }
    const connectionId = res.headers.get('Acp-Connection-Id')
    if (!connectionId) {
      throw new AcpProtocolError(-32000, 'ACP initialize response carried no Acp-Connection-Id header')
    }
    const json = (await res.json()) as JsonRpcResponseMsg
    if (json.error) throw new AcpProtocolError(json.error.code, json.error.message, json.error.data)
    this.connectionId = connectionId

    this.connStream = new AcpSseStream(
      this.acpPath,
      { 'Acp-Connection-Id': connectionId },
      (data) => this.handleInbound(data),
      (err) => this.markDead(err ? `connection stream: ${err.message}` : 'connection stream ended'),
    )
    void this.connStream.start()
  }

  /**
   * Resolve the ACP session working directory: an explicit override (the
   * ctor's `cwd` option, or `VITE_ACP_CWD`) if one was given, otherwise
   * ask the backend for the ONE path it owns (`GET /api/acp/config` —
   * `goose_acp::get_acp_config`, the same directory `initialize()` writes
   * `.goosehints` into). Cached in `this.cwd` after the first successful
   * resolution so a session's later calls don't re-fetch.
   *
   * Fails loudly rather than falling back to a guessed default: a
   * wrong-but-plausible cwd would mean the agent reads a different
   * directory and behaves to a policy nobody can see — precisely the
   * failure this replaces (previously `VITE_ACP_CWD` unset meant a hard
   * refusal; now an unreachable/empty backend response refuses the same
   * way, naming what's missing, instead of ever inventing a path).
   */
  private async resolveCwd(): Promise<string> {
    if (this.cwd) return this.cwd

    let res: Response
    try {
      res = await rateLimitedFetch('/api/acp/config', { method: 'GET' })
    } catch (err) {
      if (err instanceof AcpRateLimitError) throw err
      throw new AcpProtocolError(
        -32000,
        `could not reach the backend to resolve the ACP working directory ` +
          `(GET /api/acp/config): ${err instanceof Error ? err.message : String(err)} — ` +
          `set VITE_ACP_CWD to override`,
      )
    }
    if (!res.ok) {
      throw new AcpProtocolError(
        -32000,
        `GET /api/acp/config failed: ${res.status} ${await safeText(res)} — the backend ` +
          `could not resolve the ACP working directory and no VITE_ACP_CWD override is set`,
      )
    }
    const body = (await res.json()) as { cwd?: string; active?: { provider?: string } }
    if (!body.cwd) {
      throw new AcpProtocolError(
        -32000,
        'GET /api/acp/config returned no cwd — the backend has no ACP working ' +
          'directory to serve and no VITE_ACP_CWD override is set',
      )
    }
    // The same response already names the active provider, so the header's
    // vendor mark rides along rather than costing a second request. Left
    // null when absent: an unknown provider draws NO mark, because a
    // defaulted logo would assert a vendor the backend never reported.
    this.activeProvider = body.active?.provider ?? null
    this.cwd = body.cwd
    return this.cwd
  }

  /**
   * Step 2. Open a session. Requires `initialize()` to have completed.
   * `mcpServers` is `[]` for this slice — Roshera's own MCP tools are
   * wired in a later slice, not invented here.
   */
  async newSession(mcpServers: unknown[] = []): Promise<string> {
    if (!this.connectionId) throw new AcpProtocolError(-32000, 'newSession() called before initialize()')
    this.lastMcpServers = mcpServers
    const cwd = await this.resolveCwd()
    const result = (await this.requestAsync(
      'session/new',
      { cwd, mcpServers },
      false,
    )) as { sessionId?: string; configOptions?: AcpSessionConfigOption[] } | undefined
    if (!result?.sessionId) {
      throw new AcpProtocolError(-32000, 'session/new response carried no sessionId')
    }
    this.sessionId = result.sessionId
    this._currentModel = resolveModelFromConfigOptions(result.configOptions)

    this.sessStream = new AcpSseStream(
      this.acpPath,
      { 'Acp-Connection-Id': this.connectionId, 'Acp-Session-Id': this.sessionId },
      (data) => this.handleInbound(data),
      (err) => this.markDead(err ? `session stream: ${err.message}` : 'session stream ended'),
    )
    void this.sessStream.start()
    for (const cb of this.sessionChangedHandlers) cb()
    return this.sessionId
  }

  /**
   * Rebuild the connection + session in place after the backend reported
   * that our `Acp-Connection-Id` no longer exists (`POST /acp` → 404 with
   * an empty body). TWO backend causes share that exact signature, on
   * purpose:
   *
   * - a backend restart while this tab stayed open (verified live
   *   2026-08-01: the stale id 404s every call, and without this the
   *   Blackboard silently fell back to the legacy single-shot path
   *   ("Command processed.") while looking connected);
   * - a provider repin (`PUT /api/ai/provider`): goose stores the
   *   provider ON THE SESSION and restores it, so the backend now ends
   *   every connection minted under the old provider
   *   (`api-server/src/acp_provider_epoch.rs`) precisely so this one
   *   recovery path starts the next session on the new provider — no
   *   page reload, no second mechanism.
   *
   * Any failure here (initialize or session/new) propagates verbatim —
   * an unreachable agent must surface as an error, never as a healthy-
   * looking fallback.
   */
  private async reestablish(): Promise<void> {
    this.reconnecting = true
    try {
      // Drop the old streams silently — `close()` aborts, and an aborted
      // stream's onDone early-returns, so `markDead` does not fire here.
      this.connStream?.close()
      this.sessStream?.close()
      this.connStream = null
      this.sessStream = null
      this.connectionId = null
      this.sessionId = null
      this._currentModel = null
      // Re-ask the backend (never a cached copy) which provider now backs
      // the agent surface: in the repin case this rebuild EXISTS because
      // that answer changed, and serving the old vendor mark from cache
      // would claim the wrong provider is serving the new session. An
      // explicit ctor/VITE_ACP_CWD override keeps its cwd, but the
      // provider is still re-fetched by `resolveCwd` only in the
      // backend-owned case — under an override there is no config fetch,
      // and `provider` honestly stays null rather than guessing.
      if (!this.cwdIsExplicitOverride) {
        this.cwd = ''
      }
      this.activeProvider = null
      // Anything still pending was addressed to the dead connection; its
      // response can never arrive.
      for (const entry of this.pending.values()) {
        entry.reject(
          new AcpConnectionDeadError('ACP connection no longer exists on the backend (404)'),
        )
      }
      this.pending.clear()
      this.dead = false
      await this.initialize()
      await this.newSession(this.lastMcpServers)
    } finally {
      this.reconnecting = false
    }
  }

  /** Send one user turn on the active session and await its `stopReason`.
   *  Progressive content/tool activity arrives via `onUpdate` while this
   *  promise is pending.
   *
   *  A 404 means the backend no longer serves our connection id — it
   *  restarted, or a provider repin invalidated every connection minted
   *  under the old provider (see {@link reestablish}): rebuild and retry
   *  the prompt exactly once, so this same turn is served by a fresh
   *  session on whatever the backend now pins. A 404 on the retried call
   *  — or any failure during the rebuild itself — propagates; there is
   *  deliberately no loop. */
  async prompt(text: string): Promise<{ stopReason: AcpStopReason }> {
    if (!this.sessionId) throw new AcpProtocolError(-32000, 'prompt() called before newSession()')
    try {
      return await this.promptRequest(text)
    } catch (err) {
      const staleConnection =
        err instanceof AcpHttpError && err.status === 404 && !this.reconnecting
      if (!staleConnection) throw err
      await this.reestablish()
      return await this.promptRequest(text)
    }
  }

  private async promptRequest(text: string): Promise<{ stopReason: AcpStopReason }> {
    const result = (await this.requestAsync(
      'session/prompt',
      { sessionId: this.sessionId, prompt: [{ type: 'text', text }] },
      true,
    )) as { stopReason?: AcpStopReason } | undefined
    return { stopReason: result?.stopReason ?? 'end_turn' }
  }

  /** `session/cancel` is a notification, not a request — there is no
   *  response to await; the in-flight `prompt()` resolves (with
   *  `stopReason: 'cancelled'`) once the agent acknowledges. */
  cancel(): void {
    if (!this.sessionId) return
    this.notify('session/cancel', { sessionId: this.sessionId }, true)
  }

  /** Tear down both SSE streams. Terminal — this client cannot be reused. */
  close(): void {
    this.connStream?.close()
    this.sessStream?.close()
    this.dead = true
  }
}
