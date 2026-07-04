/**
 * REST client for the document-unit setting.
 *
 * `GET  /api/document/units` → `{ unit: UnitToken }`
 * `PATCH /api/document/units { unit }` → `{ unit: UnitToken }` | 400
 *
 * The frontend NEVER converts numbers — all labels arrive from the
 * kernel already formatted for the active unit. This module's only job
 * is reading and writing the document-wide display token so the backend
 * can format every subsequent label fetch in the chosen unit.
 *
 * On a 400 the backend returns `{ error: "invalid_unit", reason: "..." }`.
 * `setDocumentUnit` throws a `UnitSetError` carrying `reason` verbatim
 * so the UI can surface it without paraphrase.
 */

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/** The five tokens the backend accepts. */
export type UnitToken = 'mm' | 'cm' | 'm' | 'in' | 'ft'

/** Human-readable labels for the selector. */
export const UNIT_LABELS: Record<UnitToken, string> = {
  mm: 'mm',
  cm: 'cm',
  m: 'm',
  in: 'in',
  ft: 'ft',
}

/** Ordered list for the dropdown. */
export const UNIT_OPTIONS: UnitToken[] = ['mm', 'cm', 'm', 'in', 'ft']

/**
 * Thrown by `setDocumentUnit` when the backend returns 400.
 * `reason` is the backend's `reason` field verbatim.
 */
export class UnitSetError extends Error {
  readonly reason: string

  constructor(reason: string) {
    super(reason)
    this.name = 'UnitSetError'
    this.reason = reason
  }
}

/** GET the document's current display unit. */
export async function getDocumentUnit(): Promise<UnitToken> {
  const resp = await fetch(`${API_BASE}/document/units`, {
    headers: { Accept: 'application/json' },
  })
  if (!resp.ok) {
    throw new Error(`getDocumentUnit: ${resp.status}`)
  }
  const data = (await resp.json()) as { unit: UnitToken }
  return data.unit
}

/**
 * PATCH the document's display unit.
 *
 * On success returns the confirmed token (mirrors the response body).
 * On 400 throws {@link UnitSetError} with the backend `reason` verbatim.
 * On other HTTP / network errors throws a plain `Error`.
 */
export async function setDocumentUnit(token: UnitToken): Promise<UnitToken> {
  const resp = await fetch(`${API_BASE}/document/units`, {
    method: 'PATCH',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
    },
    body: JSON.stringify({ unit: token }),
  })

  if (!resp.ok) {
    const text = await resp.text().catch(() => '')
    let reason = text
    try {
      const parsed: unknown = JSON.parse(text)
      if (
        typeof parsed === 'object' &&
        parsed !== null &&
        'reason' in parsed &&
        typeof (parsed as { reason: unknown }).reason === 'string'
      ) {
        reason = (parsed as { reason: string }).reason
      }
    } catch {
      // Body was not JSON — keep the raw text verbatim.
    }
    if (resp.status === 400) {
      throw new UnitSetError(reason || 'invalid unit')
    }
    throw new Error(reason || `setDocumentUnit: ${resp.status}`)
  }

  const data = (await resp.json()) as { unit: UnitToken }
  return data.unit
}
