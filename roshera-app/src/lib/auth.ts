/**
 * Minimal frontend authentication for Roshera.
 *
 * Deliberately small (Auth Slice 1): a bearer token in localStorage, a
 * single `window.fetch` interceptor that attaches it to same-origin API
 * requests, and helpers to log in / out. No signup flow, no password
 * reset, no refresh-token rotation — those are later work. The backend
 * enforces authentication by default (AuthPosture::Required); a local
 * dev backend running `ROSHERA_DEV_INSECURE=1` needs no token, so an
 * unauthenticated frontend keeps working there unchanged.
 *
 * The token is a JWT minted by `POST /api/auth/login`. The backend's
 * `auth_middleware` verifies it as `Authorization: Bearer <jwt>`. The
 * WebSocket cannot carry a header in the browser, so `ws-client` sends
 * the same token in-band via an `Authenticate` frame (see that module).
 */

const TOKEN_KEY = 'roshera_token'

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
// Set when an API request comes back 401. The login UI observes this to
// decide whether to prompt. It is a hint, not a gate: a dev backend that
// never 401s simply never trips it.

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
      | { success?: boolean; token?: string; error?: string }
      | null
    if (res.ok && body?.success && body.token) {
      authRequired = false
      setToken(body.token)
      return { success: true }
    }
    return { success: false, error: body?.error ?? `Login failed (${res.status})` }
  } catch (err) {
    return { success: false, error: err instanceof Error ? err.message : 'Network error' }
  }
}

export function logout(): void {
  clearToken()
}

// --- fetch interceptor -------------------------------------------------

// Captured before patching so `login` and re-entrancy use the unpatched
// implementation.
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
 * token (when present) and a 401 flips the auth-required signal. This is
 * what lets a single change cover all ~100 existing `fetch` call sites
 * and any future ones without touching them.
 */
export function installFetchAuth(): void {
  if ((window.fetch as { __rosheraAuth?: boolean }).__rosheraAuth) return

  const patched: typeof fetch = async (input, init) => {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url

    let nextInit = init
    if (memoryToken && isSameApiOrigin(url)) {
      const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined))
      if (!headers.has('Authorization')) {
        headers.set('Authorization', `Bearer ${memoryToken}`)
      }
      nextInit = { ...init, headers }
    }

    const res = await originalFetch(input as RequestInfo, nextInit)
    if (res.status === 401 && isSameApiOrigin(url)) {
      markAuthRequired()
    }
    return res
  }

  ;(patched as { __rosheraAuth?: boolean }).__rosheraAuth = true
  window.fetch = patched
}
