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

/**
 * Durable reference to the geometry a `ProjectedView` is rendering.
 *
 * Today only `Part` is wired. The `part_id` is the UUID held by the
 * server's `PartManager`; `solid_id` indexes into that part's
 * `BRepModel`. Storing both on the view (instead of relying on whichever
 * tab the client happens to have active) is what makes drawings
 * round-trip cleanly through reloads and tab switches.
 */
export type ViewSource = {
  kind: 'part'
  part_id: string
  solid_id: number
}

export interface ProjectedView {
  id: string
  name: string
  projection: ProjectionType
  source: ViewSource
  position_mm: [number, number]
  scale: number
  polylines: Polyline2d[]
  extent: ViewExtent
}

export interface TitleBlock {
  drawn_by: string
  date: string
  material: string
  /** `null` = use auto-derived `RSH-XXXXXXXX` short id. */
  drawing_number: string | null
  revision: string
  sheet_index: number
  sheet_count: number
}

export interface Drawing {
  id: string
  name: string
  sheet_size: SheetSize
  views: ProjectedView[]
  /** Optional in the wire shape so the frontend continues to render
   *  against a pre-upgrade api-server that doesn't know the field
   *  exists. Use {@link defaultTitleBlock} for a safe accessor. */
  title_block?: TitleBlock
}

/** Default title-block values matching the kernel's `Default` impl.
 *  Use this when a `Drawing` may have come from a server that pre-dates
 *  the title-block schema. */
export function defaultTitleBlock(): TitleBlock {
  return {
    drawn_by: '',
    date: '',
    material: '',
    drawing_number: null,
    revision: 'A',
    sheet_index: 1,
    sheet_count: 1,
  }
}

/**
 * Partial update for a title block. Omitted fields stay untouched.
 *
 * `drawing_number` is the awkward one: backend treats omission, `null`
 * and a string distinctly (no change / clear override / set override).
 * The frontend only ever sends omission or a string in practice, but
 * the `null` case is exposed for the "Clear override" button.
 */
export interface TitleBlockPatch {
  drawn_by?: string
  date?: string
  material?: string
  drawing_number?: string | null
  revision?: string
  sheet_index?: number
  sheet_count?: number
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

/** Rename a drawing in-place. The server returns 404 on unknown ids and
 *  400 if the new name is empty / whitespace. */
export async function renameDrawing(id: string, name: string): Promise<void> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}/rename`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  })
  await jsonOrThrow<unknown>(r, 'renameDrawing')
}

/**
 * Patch a drawing's title-block metadata. Only fields supplied in
 * `patch` are changed. Returns the updated `TitleBlock` so the caller
 * can refresh local state without a follow-up `getDrawing`.
 */
export async function updateTitleBlock(
  id: string,
  patch: TitleBlockPatch,
): Promise<TitleBlock> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}/title-block`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  })
  return jsonOrThrow<TitleBlock>(r, 'updateTitleBlock')
}

/**
 * Project a part's solid into the drawing and append a view.
 *
 * `source` is a durable `{ part_id, solid_id }` pair — the server
 * resolves `part_id` against its `PartManager` registry, not against
 * whichever tab is open in the client. This makes the drawing portable:
 * a view added from one tab can be re-rendered from any other.
 */
export async function addView(
  drawingId: string,
  body: {
    name: string
    source: ViewSource
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

/** Fetch the rendered PDF as a Blob suitable for direct download. */
export async function fetchDrawingPdf(id: string): Promise<Blob> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}/pdf`)
  if (!r.ok) {
    throw new Error(`fetchDrawingPdf: ${r.status} ${r.statusText}`)
  }
  return r.blob()
}

/** Fetch the rendered DXF (ASCII) as a Blob suitable for direct download. */
export async function fetchDrawingDxf(id: string): Promise<Blob> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}/dxf`)
  if (!r.ok) {
    throw new Error(`fetchDrawingDxf: ${r.status} ${r.statusText}`)
  }
  return r.blob()
}

/** Supported download formats. PDF + DXF are the industry deliverables;
 *  SVG is kept for web-only preview and as the lightweight option. */
export type DrawingFormat = 'pdf' | 'dxf' | 'svg'

/**
 * Trigger a browser download of `drawing` rendered in `format`. Wraps
 * the format-specific fetcher in a Blob URL + transient anchor click
 * so callers don't have to repeat the boilerplate per format.
 */
export async function downloadDrawing(
  id: string,
  name: string,
  format: DrawingFormat,
): Promise<void> {
  let blob: Blob
  if (format === 'pdf') {
    blob = await fetchDrawingPdf(id)
  } else if (format === 'dxf') {
    blob = await fetchDrawingDxf(id)
  } else {
    const svg = await fetchDrawingSvg(id)
    blob = new Blob([svg], { type: 'image/svg+xml' })
  }
  const safe = (name.trim() || id).replace(/[^A-Za-z0-9_-]+/g, '_')
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `${safe}.${format}`
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  // Give the browser a tick to start the download before revoking.
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
