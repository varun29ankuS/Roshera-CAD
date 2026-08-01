/**
 * Frontend authentication for Roshera.
 *
 * A bearer token in localStorage, a single `window.fetch` interceptor that
 * attaches it to same-origin API requests, and helpers to log in / out.
 * No signup flow, no password reset — those are later work. The backend
 * enforces authentication by default (AuthPosture::Required); a local dev
 * backend running `ROSHERA_DEV_INSECURE=1` needs no token, so an
 * unauthenticated frontend keeps working there unchanged.
 *
 * The token is a JWT minted by `POST /api/auth/login`. The backend's
 * `auth_middleware` verifies it as `Authorization: Bearer <jwt>`. The
 * WebSocket cannot carry a header in the browser, so `ws-client` sends
 * the same token in-band via an `Authenticate` frame (see that module).
 *
 * Refresh: login also returns a `refresh_token`, persisted alongside the
 * access token. The access token's own `exp` claim drives a proactive
 * renewal timer (fired a small skew before expiry via
 * `POST /api/auth/refresh`) so a session never needs to hit a visible 401
 * in the first place. As a backstop, the fetch interceptor also reacts to
 * a 401 by refreshing once and retrying the failed request once — never a
 * loop, and never a second retry of a request that itself came back 401
 * after the retry. Concurrent 401s share a single in-flight refresh
 * (single-flight) rather than each starting their own. If refresh fails
 * (or there is no refresh token to use), the session is cleared honestly
 * and the existing `isAuthRequired` / `LoginDialog` re-auth path fires —
 * never a silent retry loop, never a UI that claims to be synced when it
 * is not.
 */

const TOKEN_KEY = 'roshera_token'
const REFRESH_TOKEN_KEY = 'roshera_refresh_token'

/** The API origin the app talks to (empty string ⇒ same origin). */
const API_BASE = import.meta.env.VITE_API_URL || ''

/** Read the stored bearer token, or `null` if none. */
export function getToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY)
  } catch {
    // localStorage can throw in private-mode / sandboxed contexts.
    return null
  }
}

/** Persist a bearer token and notify listeners. */
export function setToken(token: string): void {
  try {
    localStorage.setItem(TOKEN_KEY, token)
  } catch {
    // Non-fatal: the token still applies for this session via the
    // in-memory interceptor closure below.
  }
  memoryToken = token
  notify()
}

/** Clear the stored token (logout) and notify listeners. */
export function clearToken(): void {
  try {
    localStorage.removeItem(TOKEN_KEY)
  } catch {
    /* ignore */
  }
  memoryToken = null
  notify()
}

// --- refresh token storage (mirrors the access-token helpers above) ----
// Kept internal: nothing outside this module needs to read the refresh
// token directly, only trigger the refresh flow below.

function getRefreshToken(): string | null {
  try {
    return localStorage.getItem(REFRESH_TOKEN_KEY)
  } catch {
    return null
  }
}

function setRefreshToken(token: string): void {
  try {
    localStorage.setItem(REFRESH_TOKEN_KEY, token)
  } catch {
    /* non-fatal, mirrors setToken */
  }
}

function clearRefreshToken(): void {
  try {
    localStorage.removeItem(REFRESH_TOKEN_KEY)
  } catch {
    /* ignore */
  }
}

// In-memory mirror so the interceptor never pays a localStorage read per
// request and still works when storage is unavailable.
let memoryToken: string | null = getToken()

export function currentToken(): string | null {
  return memoryToken
}

export function isAuthenticated(): boolean {
  return memoryToken !== null
}

// --- change notification (so the login dialog + WS can react) ----------

type Listener = () => void
const listeners = new Set<Listener>()

/** Subscribe to auth-state changes (token set/cleared, 401 observed). */
export function onAuthChange(fn: Listener): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

function notify(): void {
  for (const fn of listeners) fn()
}

// --- "authentication required" signal ----------------------------------
//
// Set when an API request comes back 401 and the refresh backstop could
// not recover it. The login UI observes this to decide whether to
// prompt. It is a hint, not a gate: a dev backend that never 401s simply
// never trips it.

let authRequired = false
export function isAuthRequired(): boolean {
  return authRequired
}
function markAuthRequired(): void {
  if (!authRequired) {
    authRequired = true
    notify()
  }
}

// --- proactive renewal ---------------------------------------------------
//
// Decode the access token's own `exp` claim (seconds since epoch, per
// RFC 7519) and schedule a refresh a small skew before it elapses, so a
// long-lived session renews itself before any request ever sees a 401.

const REFRESH_SKEW_MS = 60_000

function decodeJwtExpMs(token: string): number | null {
  const parts = token.split('.')
  if (parts.length < 2) return null
  try {
    const payloadB64 = parts[1].replace(/-/g, '+').replace(/_/g, '/')
    const padded = payloadB64 + '='.repeat((4 - (payloadB64.length % 4)) % 4)
    const json = atob(padded)
    const payload = JSON.parse(json) as { exp?: number }
    if (typeof payload.exp !== 'number') return null
    return payload.exp * 1000
  } catch {
    return null
  }
}

let refreshTimer: ReturnType<typeof setTimeout> | null = null

function clearScheduledRefresh(): void {
  if (refreshTimer !== null) {
    clearTimeout(refreshTimer)
    refreshTimer = null
  }
}

function scheduleProactiveRefresh(accessToken: string): void {
  clearScheduledRefresh()
  const expMs = decodeJwtExpMs(accessToken)
  if (expMs === null) return
  const delay = Math.max(0, expMs - Date.now() - REFRESH_SKEW_MS)
  refreshTimer = setTimeout(() => {
    void refreshAccessToken()
  }, delay)
}

// If a session survives a page reload, pick up where it left off.
if (memoryToken) {
  scheduleProactiveRefresh(memoryToken)
}

// --- refresh flow (single-flight) ---------------------------------------

/** Session ended and could not be renewed: clear it and surface re-auth. */
function handleAuthFailure(): void {
  clearScheduledRefresh()
  clearToken()
  clearRefreshToken()
  markAuthRequired()
}

let inFlightRefresh: Promise<boolean> | null = null

/**
 * Exchange the stored refresh token for a new access token. Concurrent
 * callers (a scene poll, a blackboard poll, and a turn can all 401 at
 * once) share exactly one in-flight request rather than each starting
 * their own.
 */
function refreshAccessToken(): Promise<boolean> {
  if (inFlightRefresh) return inFlightRefresh
  inFlightRefresh = doRefresh().finally(() => {
    inFlightRefresh = null
  })
  return inFlightRefresh
}

async function doRefresh(): Promise<boolean> {
  const refreshToken = getRefreshToken()
  if (!refreshToken) {
    handleAuthFailure()
    return false
  }
  try {
    const res = await originalFetch(`${API_BASE}/api/auth/refresh`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ refresh_token: refreshToken }),
    })
    const body = (await res.json().catch(() => null)) as
      | { success?: boolean; token?: string; error?: string }
      | null
    // RefreshResponse carries a new access token but no new refresh
    // token (the refresh token itself is not rotated) — the one already
    // in storage keeps working for the next renewal.
    if (res.ok && body?.success && body.token) {
      setToken(body.token)
      scheduleProactiveRefresh(body.token)
      return true
    }
    handleAuthFailure()
    return false
  } catch {
    handleAuthFailure()
    return false
  }
}

// --- login / logout ----------------------------------------------------

export interface LoginResult {
  success: boolean
  error?: string
}

/**
 * Authenticate against the backend and store the resulting token.
 * Uses the raw `originalFetch` so the interceptor's 401 handling does
 * not fire on the login round-trip itself.
 */
export async function login(username: string, password: string): Promise<LoginResult> {
  try {
    const res = await originalFetch(`${API_BASE}/api/auth/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
    })
    const body = (await res.json().catch(() => null)) as
      | { success?: boolean; token?: string; refresh_token?: string; error?: string }
      | null
    if (res.ok && body?.success && body.token) {
      authRequired = false
      setToken(body.token)
      if (body.refresh_token) {
        setRefreshToken(body.refresh_token)
      }
      scheduleProactiveRefresh(body.token)
      return { success: true }
    }
    return { success: false, error: body?.error ?? `Login failed (${res.status})` }
  } catch (err) {
    return { success: false, error: err instanceof Error ? err.message : 'Network error' }
  }
}

export function logout(): void {
  clearScheduledRefresh()
  clearToken()
  clearRefreshToken()
}

// --- fetch interceptor -------------------------------------------------

// Captured before patching so `login`, `doRefresh`, and re-entrancy use
// the unpatched implementation.
const originalFetch: typeof fetch = window.fetch.bind(window)

/**
 * Return true when `url` targets our own API (relative path or the
 * configured API base). We never attach the token to cross-origin
 * requests — a bearer token must not leak to third-party hosts.
 */
function isSameApiOrigin(url: string): boolean {
  if (url.startsWith('/')) return true
  if (API_BASE && url.startsWith(API_BASE)) return true
  try {
    return new URL(url, window.location.origin).origin === window.location.origin
  } catch {
    return false
  }
}

/**
 * Patch `window.fetch` once so every API request carries the bearer
 * token (when present) and a 401 triggers exactly one refresh-and-retry
 * before it surfaces as an auth failure. This is what lets a single
 * change cover all ~100 existing `fetch` call sites and any future ones
 * without touching them.
 */
export function installFetchAuth(): void {
  if ((window.fetch as { __rosheraAuth?: boolean }).__rosheraAuth) return

  const patched: typeof fetch = async (input, init) => {
    // Normalize to `RequestInfo` (fetch's `input` is `RequestInfo | URL`)
    // so every subsequent use — including the retry — has a consistent,
    // clonable type.
    const requestInput: RequestInfo = input instanceof URL ? input.toString() : input
    const url = typeof requestInput === 'string' ? requestInput : requestInput.url

    const isApi = isSameApiOrigin(url)
    // Never let a 401 from the refresh endpoint itself re-enter the
    // refresh-and-retry path — that would be the loop this exists to
    // forbid.
    const isRefreshEndpoint = isApi && url.includes('/api/auth/refresh')

    const buildInit = (): RequestInit | undefined => {
      if (!memoryToken || !isApi) return init
      const headers = new Headers(
        init?.headers ?? (requestInput instanceof Request ? requestInput.headers : undefined),
      )
      headers.set('Authorization', `Bearer ${memoryToken}`)
      return { ...init, headers }
    }
    // A Request's body can only be read once; clone so the same logical
    // request can be sent twice (original attempt + retry) when input
    // arrived as a Request object.
    const attemptInput = (): RequestInfo =>
      requestInput instanceof Request ? requestInput.clone() : requestInput

    let res = await originalFetch(attemptInput(), buildInit())

    if (res.status === 401 && isApi && !isRefreshEndpoint) {
      const refreshed = await refreshAccessToken()
      if (refreshed) {
        // Exactly one retry, with the freshly-renewed token.
        res = await originalFetch(attemptInput(), buildInit())
      }
      if (res.status === 401) {
        // Either refresh failed outright, or the retry itself 401'd —
        // either way we stop here and surface re-auth rather than
        // looping.
        markAuthRequired()
      }
    }

    return res
  }

  ;(patched as { __rosheraAuth?: boolean }).__rosheraAuth = true
  window.fetch = patched
}
