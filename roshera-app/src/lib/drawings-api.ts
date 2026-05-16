/**
 * REST client for the kernel drawing surface (`/api/drawings/...`).
 *
 * The kernel `Drawing` type is `Serialize` so the wire shape *is* the
 * kernel shape — no DTO translation. We mirror the relevant fields
 * here as `interface`s so consumers get full IDE assistance.
 *
 * Two-step SVG export is **not** required for drawings (the kernel
 * renders the whole document in <50 ms even for A0 sheets) so we hit
 * `GET /api/drawings/{id}/svg` and inline the response into a Blob URL.
 */

const API_HOST = import.meta.env.VITE_API_URL || ''

// ── Wire types — mirror geometry-engine/src/drawing/types.rs ────────

/** Engineering paper size. The kernel reports A4 as 297×210 (landscape). */
export type SheetSize =
  | 'A4'
  | 'A3'
  | 'A2'
  | 'A1'
  | 'A0'
  | { CUSTOM: { width: number; height: number } }

/** Projection preset. `kind` is the discriminator (serde tag = "kind"). */
export type ProjectionType =
  | { kind: 'front' }
  | { kind: 'top' }
  | { kind: 'right' }
  | { kind: 'bottom' }
  | { kind: 'left' }
  | { kind: 'isometric' }
  | { kind: 'custom'; rotation: number[] /* 9-element row-major */ }

export interface Polyline2d {
  points: [number, number][]
}

export interface ViewExtent {
  min_x: number
  max_x: number
  min_y: number
  max_y: number
}

export interface ProjectedView {
  id: string
  name: string
  projection: ProjectionType
  solid_id: number
  position_mm: [number, number]
  scale: number
  polylines: Polyline2d[]
  extent: ViewExtent
}

export interface Drawing {
  id: string
  name: string
  sheet_size: SheetSize
  views: ProjectedView[]
}

// ── REST helpers ─────────────────────────────────────────────────────

async function jsonOrThrow<T>(resp: Response, context: string): Promise<T> {
  if (!resp.ok) {
    let detail = ''
    try {
      detail = await resp.text()
    } catch {
      /* ignore body parse errors — the status code is the primary signal */
    }
    throw new Error(`${context}: ${resp.status} ${resp.statusText}${detail ? ` — ${detail}` : ''}`)
  }
  return resp.json() as Promise<T>
}

/** List every drawing the server knows about (returns just UUIDs). */
export async function listDrawings(): Promise<string[]> {
  const r = await fetch(`${API_HOST}/api/drawings`)
  return jsonOrThrow<string[]>(r, 'listDrawings')
}

/** Fetch the full `Drawing` (views + polylines) by id. */
export async function getDrawing(id: string): Promise<Drawing> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}`)
  return jsonOrThrow<Drawing>(r, 'getDrawing')
}

/** Allocate a fresh empty drawing. Returns its server-side UUID. */
export async function createDrawing(name: string, sheet_size: SheetSize = 'A3'): Promise<string> {
  const r = await fetch(`${API_HOST}/api/drawings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, sheet_size }),
  })
  const { id } = await jsonOrThrow<{ id: string }>(r, 'createDrawing')
  return id
}

/** Delete a drawing. The server returns 404 on unknown ids. */
export async function deleteDrawing(id: string): Promise<void> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}`, { method: 'DELETE' })
  await jsonOrThrow<unknown>(r, 'deleteDrawing')
}

/**
 * Project the active part's solid into the drawing and append a view.
 *
 * `solid_id` resolves against the currently active `BRepModel` (the
 * tab's part) — the server pulls the model via the same `ActiveModel`
 * extractor used by every other geometry handler.
 */
export async function addView(
  drawingId: string,
  body: {
    name: string
    solid_id: number
    projection: ProjectionType
    position_mm?: [number, number]
    scale?: number
  },
): Promise<string> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/views`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  const { view_id } = await jsonOrThrow<{ view_id: string }>(r, 'addView')
  return view_id
}

/** Remove a view from a drawing. */
export async function removeView(drawingId: string, viewId: string): Promise<void> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/views/${viewId}`, {
    method: 'DELETE',
  })
  await jsonOrThrow<unknown>(r, 'removeView')
}

/** Fetch the rendered SVG (raw XML string). Empty drawing → envelope only. */
export async function fetchDrawingSvg(id: string): Promise<string> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}/svg`)
  if (!r.ok) {
    throw new Error(`fetchDrawingSvg: ${r.status} ${r.statusText}`)
  }
  return r.text()
}
