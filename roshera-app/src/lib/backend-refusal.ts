/**
 * Backend refusal surfacing.
 *
 * The timeline/branch endpoints answer typed refusals two different
 * ways: undo/redo return HTTP 200 with `{ success: false, message }`
 * for expected refusals (nothing to undo, session not found) — see
 * `handlers/timeline.rs::undo_operation` / `redo_operation` — while
 * malformed input or a kernel-side failure comes back as a bare
 * non-2xx status with no body, or (for `/api/branches/active`) an
 * `ApiError` JSON body shaped `{ success: false, error, error_code }`.
 * Some endpoints (e.g. `/api/timeline/checkpoint`) return no JSON body
 * at all, success or failure — a bare status code is the only signal.
 *
 * These two functions read whichever shape is present so a refusal is
 * never silently treated as success. Kept separate from
 * `action-error-store.ts` so the store stays pure UI state (a message +
 * two mutators) with no knowledge of `Response`/fetch — any caller,
 * not just the ones parsing these particular wire shapes, can flash a
 * message into it.
 */

/** Best-effort JSON parse; `null` when the body is absent or not JSON. */
export async function tryReadJson(resp: Response): Promise<Record<string, unknown> | null> {
  try {
    const data: unknown = await resp.json()
    return data && typeof data === 'object' ? (data as Record<string, unknown>) : null
  } catch {
    return null
  }
}

/** Human-readable message for a non-2xx response or a 200 with `success: false`. */
export function refusalMessage(body: Record<string, unknown> | null, status: number): string {
  if (body) {
    if (typeof body.message === 'string' && body.message) return body.message
    if (typeof body.error === 'string' && body.error) return body.error
  }
  return `Request failed (HTTP ${status})`
}
