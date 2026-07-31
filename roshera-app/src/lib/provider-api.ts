/**
 * AI PROVIDER SETTINGS — REST client
 * ===================================
 * `GET/PUT/DELETE /api/ai/provider` + `POST /api/ai/provider/test`. Types
 * mirror the backend's serde shapes VERBATIM — verified directly against
 * the (in-progress, same-repo) handler source, not guessed:
 *   - `api-server/src/handlers/ai_provider.rs` for the request/response
 *     JSON shapes (`ProviderConfigRequest`, `get_provider`'s `json!{...}`,
 *     `put_provider`'s per-mode responses, `delete_provider`,
 *     `test_provider`).
 *   - `ai-integration/src/providers/allowlist.rs` for `CredentialMode`
 *     (`#[serde(rename_all = "snake_case")]`) and `WiringStatus`
 *     (`#[serde(rename_all = "snake_case", tag = "status", content =
 *     "reason")]`: `Wired` → `{"status":"wired"}`, `SeamOnly(reason)` →
 *     `{"status":"seam_only","reason":"..."}`).
 *   - `api-server/src/error_catalog.rs` `ApiError` for the error body
 *     shape (`error_code`, `error`, `retryable`, `hint?`, `details?`).
 *
 * As of this slice the endpoints 404 live (confirmed against the running
 * dev backend) — another agent is landing the handler in parallel. Every
 * call here returns a typed `ProviderApiResult` that distinguishes "not
 * built yet" from "reached the backend and it refused" so the UI never
 * fabricates a working settings page for either case.
 */

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

// ── Wire types (verbatim mirror of allowlist.rs) ───────────────────────

export type CredentialMode = 'api_key' | 'oauth_profile' | 'workload_identity' | 'subscription_cli'

export type WiringStatus = { status: 'wired' } | { status: 'seam_only'; reason: string }

export interface ModeEntry {
  mode: CredentialMode
  spawns_local_process: boolean
  wiring: WiringStatus
  reason: string
}

export interface AllowlistedProvider {
  id: string
  display_name: string
  reason: string
  modes: ModeEntry[]
}

/** `ai_provider_config::detect_claude_cli` / `detect_codex_cli` — local
 *  CLI presence + sign-in, read defensively (a field the client doesn't
 *  recognize is just ignored, never inferred). */
export interface CliDetection {
  installed: boolean
  signed_in: boolean
  path?: string | null
}

export interface ActiveProviderConfig {
  provider: string
  mode: string
  profile_name?: string | null
  saved_at?: string | null
  has_api_key: boolean
  /** `null`/absent means "the provider's own default choice" — never a
   *  fabricated model name. */
  model?: string | null
  /** `true` when tested against the live provider's model-listing
   *  endpoint at save time; `false` when accepted but unverified
   *  (`subscription_cli` only — the CLI has no side-effect-free
   *  synchronous check this server can call per save); `null`/absent
   *  when `model` itself is unset. */
  model_verified?: boolean | null
}

export interface ResolutionChainEntry {
  source: string
  active: boolean
  [key: string]: unknown
}

export interface ProviderStatusResponse {
  active: ActiveProviderConfig | null
  ai_configured: boolean
  resolution: { chain: ResolutionChainEntry[]; active_source: string | null }
  allowlist: AllowlistedProvider[]
  /** Keyed by CLI, not by provider id — `anthropic`'s subscription_cli
   *  detection is `cli.claude`; `openai`'s would be `cli.codex`. */
  cli: { claude: CliDetection; codex: CliDetection }
}

/** Shared by `PUT /api/ai/provider` and `POST /api/ai/provider/test` —
 *  the backend runs the identical validation path for both, so the
 *  request shape is identical too (`ai_provider.rs`'s
 *  `ProviderConfigRequest`). */
export interface ProviderConfigRequest {
  provider: string
  mode: CredentialMode
  api_key?: string
  profile_name?: string
  /** Absent or `"default"` means "the provider's own choice" — the
   *  backend normalizes both to no override. Do NOT hardcode a menu of
   *  model names here: which models a mode can serve depends on the
   *  live credential, so an explicit value is validated server-side
   *  (`POST /api/ai/provider/test` / `PUT /api/ai/provider`) before it
   *  is ever treated as active. */
  model?: string
  /** Must be `true` for any mode the allowlist marks
   *  `spawns_local_process` — refused by name server-side without it. */
  consent_spawn_local_process?: boolean
}

export interface PutProviderResponse {
  success: boolean
  provider: string
  mode: string
  profile_name?: string
  model?: string | null
  model_verified?: boolean | null
  /** `subscription_cli` only, present when a `model` was requested:
   *  explains why `model_verified` is `false` rather than pretending the
   *  save round-tripped through a check it didn't. */
  model_verification_note?: string
  note?: string
}

export interface DeleteProviderResponse {
  success: boolean
  ai_configured: boolean
  fallback_source: string | null
}

export interface TestProviderResponse {
  success: boolean
  provider: string
  mode: string
  detail?: unknown
}

/** `error_catalog.rs::ApiError`'s wire shape. */
export interface ApiErrorBody {
  error_code: string
  error: string
  retryable: boolean
  hint?: string
  details?: unknown
  success: false
}

// ── Result envelope — "not built yet" is a distinct case from "error" ──

export type ProviderApiResult<T> =
  | { ok: true; data: T }
  /** The endpoint 404/405'd, or the request never reached a server
   *  (network failure). Indistinguishable to the client, and both mean
   *  the same thing to the UI: this build/environment doesn't have the
   *  feature wired yet. */
  | { ok: false; kind: 'unavailable' }
  /** The backend was reached and refused/failed the request. `hint`
   *  carries `ApiError.hint` when present — surfaced alongside the
   *  message rather than dropped. */
  | { ok: false; kind: 'error'; status: number; message: string; hint?: string }

async function call<T>(path: string, init: RequestInit): Promise<ProviderApiResult<T>> {
  let res: Response
  try {
    res = await fetch(`${API_BASE}${path}`, {
      ...init,
      headers: { 'Content-Type': 'application/json', ...init.headers },
    })
  } catch {
    return { ok: false, kind: 'unavailable' }
  }
  if (res.status === 404 || res.status === 405) {
    return { ok: false, kind: 'unavailable' }
  }
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as Partial<ApiErrorBody> | null
    return {
      ok: false,
      kind: 'error',
      status: res.status,
      message: body?.error ?? `${res.status} ${res.statusText}`,
      hint: body?.hint,
    }
  }
  try {
    const data = (await res.json()) as T
    return { ok: true, data }
  } catch {
    return { ok: false, kind: 'error', status: res.status, message: 'Response body was not valid JSON' }
  }
}

export function getProviderStatus(): Promise<ProviderApiResult<ProviderStatusResponse>> {
  return call('/ai/provider', { method: 'GET' })
}

export function putProvider(
  req: ProviderConfigRequest,
): Promise<ProviderApiResult<PutProviderResponse>> {
  return call('/ai/provider', { method: 'PUT', body: JSON.stringify(req) })
}

export function deleteProvider(): Promise<ProviderApiResult<DeleteProviderResponse>> {
  return call('/ai/provider', { method: 'DELETE' })
}

export function testProvider(
  req: ProviderConfigRequest,
): Promise<ProviderApiResult<TestProviderResponse>> {
  return call('/ai/provider/test', { method: 'POST', body: JSON.stringify(req) })
}
