/**
 * REST client for the kernel part registry (`/api/parts/...`).
 *
 * Parts are the per-tab `BRepModel` documents. The drawing module needs
 * to list them (to populate a part picker) and read their solid lists
 * (to populate a solid picker once a part is chosen). For everything
 * else, parts are still routed through the `X-Roshera-Part-Id` header
 * on the legacy handlers — this module only exposes the read paths the
 * drawing UI needs.
 */

const API_HOST = import.meta.env.VITE_API_URL || ''

/** Summary returned by `GET /api/parts` and `GET /api/parts/{id}`. */
export interface PartSummary {
  id: string
  name: string
  solid_count: number
  created_at: string
  updated_at: string
}

async function jsonOrThrow<T>(resp: Response, context: string): Promise<T> {
  if (!resp.ok) {
    let detail = ''
    try {
      detail = await resp.text()
    } catch {
      /* ignore body parse errors */
    }
    throw new Error(`${context}: ${resp.status} ${resp.statusText}${detail ? ` — ${detail}` : ''}`)
  }
  return resp.json() as Promise<T>
}

/** List every open part. Empty array if no parts have been created. */
export async function listParts(): Promise<PartSummary[]> {
  const r = await fetch(`${API_HOST}/api/parts`)
  return jsonOrThrow<PartSummary[]>(r, 'listParts')
}

/** Fetch a single part summary by id. */
export async function getPart(id: string): Promise<PartSummary> {
  const r = await fetch(`${API_HOST}/api/parts/${id}`)
  return jsonOrThrow<PartSummary>(r, 'getPart')
}
