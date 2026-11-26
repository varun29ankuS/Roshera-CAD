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

import { drawingSvgUrl } from './drawing-svg-url'
import type {
  CertificateSummary,
  DimTolerance,
  DimensionEdit,
  Waiver,
} from './dimension-annotate'
import type { SheetRefusalDetails, SheetRequest } from './sheet-size'

const API_HOST = import.meta.env.VITE_API_URL || ''

// ── Wire types — mirror geometry-engine/src/drawing/types.rs ────────

/** Engineering paper size (ISO 216): the size only. Which way up it is drawn
 *  is the drawing's {@link SheetOrientation}; {@link sheetDimensionsMm} gives
 *  the two together as the paper's width × height. */
export type SheetSize =
  | 'A4'
  | 'A3'
  | 'A2'
  | 'A1'
  | 'A0'
  | { CUSTOM: { width: number; height: number } }

/** Which way up the paper is drawn (ISO 5457). Absent on sheets from before
 *  the choice existed — those are landscape. */
export type SheetOrientation = 'landscape' | 'portrait'

/** The paper's [width, height] in mm, as the kernel lays it: an ISO size
 *  landscape (long edge across) or portrait (long edge up); a custom size as
 *  given, whatever the orientation says (the kernel never turns one). */
export function sheetDimensionsMm(
  sheet: SheetSize,
  orientation: SheetOrientation = 'landscape',
): [number, number] {
  if (typeof sheet === 'object' && sheet && 'CUSTOM' in sheet) {
    return [sheet.CUSTOM.width, sheet.CUSTOM.height]
  }
  const landscape: Record<string, [number, number]> = {
    A4: [297, 210],
    A3: [420, 297],
    A2: [594, 420],
    A1: [841, 594],
    A0: [1189, 841],
  }
  const [w, h] = landscape[sheet] ?? [297, 210]
  return orientation === 'portrait' ? [h, w] : [w, h]
}

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
  /** The view's dimension callouts (kernel `Dimension2d`). */
  dimensions?: SheetDimension[]
  /** Hidden (occluded) edges drawn dashed; false = "hidden lines removed". */
  show_hidden?: boolean
}

/** One dimension callout on a sheet. `id` is what an edit addresses. */
export interface SheetDimension {
  id: string
  kind: string
  value: number
  unit: string
  label: string
  /** Durable handle its annotations are recorded under; absent when the
   *  dimension cannot be annotated (its faces carry no persistent id). */
  key?: string | null
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
  /** ISO 7200 supplementary title (second line under the title). */
  supplementary_title?: string
  /** ISO 7200 approval person. Empty = FOR REVIEW. */
  approved_by?: string
  /** ISO 7200 document type, e.g. PART DRAWING. */
  document_type?: string
}

export interface Drawing {
  id: string
  name: string
  sheet_size: SheetSize
  /** Landscape or portrait; absent (pre-orientation server) = landscape. */
  orientation?: SheetOrientation
  views: ProjectedView[]
  /** Optional in the wire shape so the frontend continues to render
   *  against a pre-upgrade api-server that doesn't know the field
   *  exists. Use {@link defaultTitleBlock} for a safe accessor. */
  title_block?: TitleBlock
  /** First- or third-angle arrangement. Absent on sheets from a server
   *  that pre-dates the choice — those are third-angle. */
  projection?: ProjectionAngle
  /** The general-tolerance note inked on the sheet. */
  tolerance_note?: string
}

/** First-angle (ISO; India, Europe) or third-angle (ASME) projection. */
export type ProjectionAngle = 'first' | 'third'

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
    supplementary_title: '',
    approved_by: '',
    document_type: 'PART DRAWING',
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
  supplementary_title?: string
  approved_by?: string
  document_type?: string
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

/**
 * Which model a drawing's measurements are actually about.
 *
 * A drawing stores kernel solid ids, and those are small integers REUSED
 * across documents — so a sheet is only meaningful against the model that
 * produced it. The server binds each drawing to that model and discloses it
 * here, which is why the listing is no longer a bare array of ids: reading
 * the list while a different document or part is active, you can now see
 * which one each entry belongs to instead of assuming it is the active one.
 */
export type DrawingOwner =
  | { kind: 'part'; part_id: string }
  | { kind: 'legacy'; document_id: string }

export interface DrawingListEntry {
  id: string
  owner: DrawingOwner
}

/** List every drawing the server knows about, each with its disclosed owner. */
export async function listDrawings(): Promise<DrawingListEntry[]> {
  const r = await fetch(`${API_HOST}/api/drawings`)
  return jsonOrThrow<DrawingListEntry[]>(r, 'listDrawings')
}

/** Human-readable one-liner for a drawing's owner, for UI disclosure. */
export function describeOwner(owner: DrawingOwner): string {
  return owner.kind === 'part'
    ? `part ${owner.part_id.slice(0, 8)}`
    : `document ${owner.document_id.slice(0, 8)}`
}

/** Fetch the full `Drawing` (views + polylines) by id. */
export async function getDrawing(id: string): Promise<Drawing> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}`)
  return jsonOrThrow<Drawing>(r, 'getDrawing')
}

/** Allocate a fresh empty drawing. Returns its server-side UUID. */
export async function createDrawing(
  name: string,
  sheet_size: SheetSize = 'A3',
  orientation: SheetOrientation = 'landscape',
): Promise<string> {
  const r = await fetch(`${API_HOST}/api/drawings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, sheet_size, orientation }),
  })
  const { id } = await jsonOrThrow<{ id: string }>(r, 'createDrawing')
  return id
}

/**
 * One-call "right-click → drawing": build the standard third-angle sheet
 * (Front / Top / Right with hidden-line removal, centerlines, and auto
 * dimensions) for a viewport object and register it in the drawing
 * registry. The scale auto-fits the part to the A3 sheet. Returns the
 * new drawing's UUID, ready to open in the Drawing workspace.
 *
 * `objectUuid` is the viewport object id (scene-store key); the server
 * resolves it to the kernel solid id.
 */
export async function createPartDrawing(
  objectUuid: string,
  name?: string,
): Promise<string> {
  const qs = name && name.trim() ? `?name=${encodeURIComponent(name.trim())}` : ''
  const r = await fetch(
    `${API_HOST}/api/parts/uuid/${objectUuid}/drawing${qs}`,
    { method: 'POST' },
  )
  const { id } = await jsonOrThrow<{ id: string }>(r, 'createPartDrawing')
  return id
}

/** A solid in the active document that a drawing can start from. */
export interface DocumentPart {
  /** Kernel solid id. */
  id: number
  name: string
}

/** The active document's parts, from the server (not the viewport's scene
 *  store, which is empty when the app opens straight into the Drawing
 *  workspace). */
export async function listDocumentParts(): Promise<DocumentPart[]> {
  const r = await fetch(`${API_HOST}/api/agent/parts`)
  const rows = await jsonOrThrow<{ id: number; name: string }[]>(r, 'listDocumentParts')
  return rows.map((p) => ({ id: p.id, name: p.name }))
}

/** Build the standard sheet for kernel solid `solidId` and register it. */
export async function createSolidDrawing(solidId: number, name?: string): Promise<string> {
  const qs = name && name.trim() ? `?name=${encodeURIComponent(name.trim())}` : ''
  const r = await fetch(`${API_HOST}/api/parts/${solidId}/drawing${qs}`, { method: 'POST' })
  const { id } = await jsonOrThrow<{ id: string }>(r, 'createSolidDrawing')
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
    /** The part to project; omit and pass `like_view` instead. */
    source?: ViewSource
    /** Another view of the same part as this view of the drawing. */
    like_view?: string
    projection: ProjectionType
    /** Omit and the kernel places the view in free space. */
    position_mm?: [number, number]
    scale?: number
  },
): Promise<{ viewId: string; aligned: boolean }> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/views`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  const { view_id, aligned } = await jsonOrThrow<{ view_id: string; aligned?: boolean }>(
    r,
    'addView',
  )
  return { viewId: view_id, aligned: aligned ?? true }
}

/** Remove a view from a drawing. */
export async function removeView(drawingId: string, viewId: string): Promise<void> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/views/${viewId}`, {
    method: 'DELETE',
  })
  await jsonOrThrow<unknown>(r, 'removeView')
}

/** An engineer's display edit to one view; absent fields stay as they are. */
export interface ViewEdit {
  show_hidden?: boolean
  name?: string
  scale?: number
  /** Where the view sits on the sheet (mm; y runs up the sheet). Recorded
   *  with the edit, so undo puts the view back. */
  position_mm?: [number, number]
}

/**
 * Change one view's display: hidden lines, name, scale, position.
 * All-or-nothing; a refusal (bad scale, a name another view already has, a
 * non-finite position, a drop onto another view, the title block or past the
 * frame) throws `DrawingRefusal` with the reason. A drag is resolved by the
 * kernel: a projected view stays on its line from FRONT (`projected` when the
 * off-line part was dropped) and FRONT carries its views (`movedViews`, the
 * dragged view first). `position` is where the view actually went. `moved`
 * says a RESCALE had to move the view; `aligned` is MEASURED after the edit
 * -- `null` for a view with no projection line (isometric, custom).
 */
export interface ViewEditResult {
  moved: boolean
  aligned: boolean | null
  position: [number, number] | null
  projected: boolean
  movedViews: { id: string; name: string; position_mm: [number, number] }[]
}

export async function updateView(
  drawingId: string,
  viewId: string,
  edit: ViewEdit,
): Promise<ViewEditResult> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/views/${viewId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(edit),
  })
  const body = await jsonOrRefusal<{
    moved?: boolean
    aligned?: boolean | null
    projected?: boolean
    view?: { position_mm?: [number, number] }
    moved_views?: { id: string; name: string; position_mm: [number, number] }[]
  }>(r)
  return {
    // A rescale moves the view only when the bigger footprint did not fit.
    moved: body.moved ?? false,
    aligned: body.aligned ?? null,
    position: body.view?.position_mm ?? null,
    projected: body.projected ?? false,
    movedViews: body.moved_views ?? [],
  }
}

/**
 * A typed refusal from the server, carrying its plain-language reason. The
 * drawing-edit routes refuse rather than approximate, and the reason is what
 * a person needs to read (`error`), not the status line.
 */
export class DrawingRefusal extends Error {
  readonly gate: string | null
  constructor(message: string, gate: string | null) {
    super(message)
    this.name = 'DrawingRefusal'
    this.gate = gate
  }
}

/**
 * The server's refusal of a sheet request, in its own words: the plain
 * reason (`error`, without the "REFUSED:" stamp) and its gate. A bare
 * "409 Conflict" told the engineer nothing.
 */
async function refusalOf(resp: Response): Promise<DrawingRefusal> {
  let message = `${resp.status} ${resp.statusText}`
  let gate: string | null = null
  try {
    const body = await resp.json()
    if (typeof body?.error === 'string') message = body.error.replace(/^REFUSED:\s*/, '')
    if (typeof body?.details?.gate === 'string') gate = body.details.gate
    else if (typeof body?.error_code === 'string') gate = body.error_code
  } catch {
    /* non-JSON body: the status line is the reason */
  }
  return new DrawingRefusal(message, gate)
}

async function jsonOrRefusal<T>(resp: Response): Promise<T> {
  if (resp.ok) return resp.json() as Promise<T>
  let message = `${resp.status} ${resp.statusText}`
  let gate: string | null = null
  try {
    const body = await resp.json()
    if (typeof body?.error === 'string') message = body.error
    if (typeof body?.details?.gate === 'string') gate = body.details.gate
  } catch {
    /* non-JSON body: the status line is the reason */
  }
  throw new DrawingRefusal(message, gate)
}

/** What changes when a sheet dimension changes (the server's driver search). */
export type DimensionDriverResolution =
  | {
      verdict: 'driven'
      current: number
      driver: {
        command: string
        parameter: string
        target_sequence: number
        value: number
        linked?: { target_sequence: number; parameter: string; value: number }[]
      }
      unchecked?: { target_sequence: number; parameter: string; reason: string }[]
    }
  | { verdict: 'reference_only'; current: number | null; reason: string }
  | { verdict: 'ambiguous'; current: number; drivers: { parameter: string; target_sequence: number }[] }
  | { verdict: 'undecided'; reason: string }

/** Ask what drives a dimension. Called when the dimension is selected, so the
 *  search runs while the new value is typed; the edit then reuses it. */
export async function getDimensionDriver(
  drawingId: string,
  dimensionId: string,
  signal?: AbortSignal,
): Promise<DimensionDriverResolution> {
  const r = await fetch(
    `${API_HOST}/api/drawings/${drawingId}/dimensions/${encodeURIComponent(dimensionId)}/driver`,
    { signal },
  )
  const body = await jsonOrThrow<{ resolution: DimensionDriverResolution }>(r, 'getDimensionDriver')
  return body.resolution
}

/** Result of editing a sheet dimension (which edits the part). */
export interface DimensionEditResult {
  dimension: { id: string; from: number; to: number; sheet_value: number | null }
  driver: {
    command: string
    parameter: string
    sequence: number
    from_value: number
    new_value: number
    /** Other parameters of the same feature that moved with it (e.g. the
     *  second cutter of a bore cut twice). */
    linked?: { target_sequence: number; parameter: string; value: number }[]
  }
}

/**
 * Set a sheet dimension to `value`. The server finds the one recorded
 * parameter that drives it, changes it, rebuilds the part and regenerates the
 * sheet. Throws {@link DrawingRefusal} when it cannot (reference-only
 * dimension, several drivers, unreachable size, or the part refused).
 */
export async function setDrawingDimension(
  drawingId: string,
  dimensionId: string,
  value: number,
  signal?: AbortSignal,
): Promise<DimensionEditResult> {
  const r = await fetch(
    `${API_HOST}/api/drawings/${drawingId}/dimensions/${encodeURIComponent(dimensionId)}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ value }),
      signal,
    },
  )
  return jsonOrRefusal<DimensionEditResult>(r)
}

// ── Annotating a dimension (tolerance, text, move, suppress) ─────────────

/** One sheet dimension as the property panel reads it. */
export interface DimensionInfo {
  id: string
  key: string | null
  view: string
  kind: string
  value: number
  unit: string
  /** The measured label. */
  label: string
  /** The label as inked, with the tolerance and text authored on it. */
  ink: string
  /** False when the dimension has no persistent identity to record under. */
  editable: boolean
  /** False when the view carries it but the layout does not draw it. */
  placed: boolean
  suppressed: boolean
  edit: DimensionEdit | null
  /** Set when an ISO 286 class is inked without limits, saying why. */
  unresolved: string | null
  /** The tolerance the dimension is inked with: authored on it, or (with a
   *  `feature_pid`) the size tolerance authored on its feature. */
  tolerance: { kind: string; feature_pid?: string | null } | null
}

/** A sheet's dimensions and what it leaves out. */
export interface DimensionListing {
  dimensions: DimensionInfo[]
  /** `null` when the sheet's part is not loaded (nothing to verify against). */
  certificate: CertificateSummary | null
}

export async function listDimensions(drawingId: string): Promise<DimensionListing> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/dimensions`)
  return jsonOrRefusal<DimensionListing>(r)
}

/**
 * What to change on a dimension. A field left out is not touched; `null`
 * clears it (removes the tolerance, puts the dimension back where the layout
 * places it, lifts a suppression).
 */
export interface DimensionAnnotation {
  tolerance?: DimTolerance | null
  prefix?: string
  suffix?: string
  text_shift_mm?: [number, number] | null
  suppressed?: Waiver | null
}

export interface DimensionAnnotationResult {
  dimension: Partial<DimensionInfo> & { key: string | null }
  certificate: CertificateSummary | null
}

/**
 * Annotate one dimension (`dimension` is its id on the sheet, or the key of
 * a dimension the sheet leaves out, to sign for it). One recorded history
 * step. Throws {@link DrawingRefusal} with the kernel's reason when it is
 * refused (a fit outside the ISO 286 tables, an unsigned suppression, a
 * dimension that cannot be annotated).
 */
export async function annotateDimension(
  drawingId: string,
  dimension: string,
  change: DimensionAnnotation,
): Promise<DimensionAnnotationResult> {
  const r = await fetch(
    `${API_HOST}/api/drawings/${drawingId}/dimensions/${encodeURIComponent(dimension)}`,
    {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(change),
    },
  )
  if (!r.ok) throw await refusalOf(r)
  return r.json() as Promise<DimensionAnnotationResult>
}

/** The limits of one ISO 286 class at a size, from the kernel's tables. */
export interface Iso286Limits {
  class: string
  kind: 'hole' | 'shaft'
  upper_deviation_mm: number
  lower_deviation_mm: number
  upper_limit_mm: number
  lower_limit_mm: number
}

/** Throws {@link DrawingRefusal} when the class or size is outside the tables. */
export async function iso286Limits(
  nominalMm: number,
  fitClass: string,
  signal?: AbortSignal,
): Promise<Iso286Limits> {
  const r = await fetch(
    `${API_HOST}/api/iso286?nominal=${encodeURIComponent(String(nominalMm))}&class=${encodeURIComponent(fitClass)}`,
    { signal },
  )
  if (!r.ok) throw await refusalOf(r)
  return r.json() as Promise<Iso286Limits>
}

/** Lay a sheet out first- or third-angle (kept when the part is edited). */
export async function setDrawingProjection(
  drawingId: string,
  projection: ProjectionAngle,
): Promise<void> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/projection`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ projection }),
  })
  await jsonOrRefusal<unknown>(r)
}

/** What `PATCH /api/drawings/{id}/sheet` answers. */
export interface SheetChangeResult {
  sheet: { sheet_size: SheetSize; orientation: SheetOrientation; scale: number; scale_label: string }
  previous: { sheet_size: SheetSize; orientation: SheetOrientation; scale: number }
  /** Edits authored for the old paper that no longer apply on the new one
   *  (a moved view that does not fit), in the kernel's words. */
  not_applied: string[]
}

/** The kernel would not put the sheet on that paper: its sentence and the
 *  measured parts of it (overflow, the scale that would fit). */
export class SheetChangeRefused extends Error {
  readonly details: SheetRefusalDetails | null
  constructor(message: string, details: SheetRefusalDetails | null) {
    super(message)
    this.name = 'SheetChangeRefused'
    this.details = details
  }
}

/**
 * Put an existing sheet on other paper and/or draw it at another scale. The
 * kernel lays it out again and keeps what was authored on it; it refuses
 * ({@link SheetChangeRefused}) when the views at that scale do not fit.
 * Recorded: Ctrl+Z takes it back and a part edit keeps it.
 */
export async function setDrawingSheet(
  drawingId: string,
  request: SheetRequest,
): Promise<SheetChangeResult> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/sheet`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request),
  })
  if (r.ok) return r.json() as Promise<SheetChangeResult>
  let message = `${r.status} ${r.statusText}`
  let details: SheetRefusalDetails | null = null
  try {
    const body = await r.json()
    if (typeof body?.error === 'string') message = body.error
    if (body?.details && typeof body.details === 'object') details = body.details as SheetRefusalDetails
  } catch {
    /* non-JSON body: the status line is the reason */
  }
  throw new SheetChangeRefused(message, details)
}

/** One finding of the kernel's sheet check (`verify_drawing`). */
export interface DrawingQualityIssue {
  severity: 'error' | 'warning'
  /** Machine-stable kind, e.g. `view_overlap`, `view_outside_frame`. */
  kind: string
  message: string
  /** The view the finding belongs to, when view-scoped. */
  view?: string | null
}

/** The kernel's sheet check (`GET /api/drawings/{id}/quality`). */
export interface DrawingQualityReport {
  /** `true` iff there are no error-severity findings. */
  passed: boolean
  sheet_utilization: number
  issues: DrawingQualityIssue[]
}

/** Re-run the kernel's sheet check on a drawing: overlaps, frame, alignment. */
export async function getDrawingQuality(drawingId: string): Promise<DrawingQualityReport> {
  const r = await fetch(`${API_HOST}/api/drawings/${drawingId}/quality`)
  return jsonOrThrow<DrawingQualityReport>(r, 'getDrawingQuality')
}

/** Fetch the rendered SVG (raw XML string) for a DOWNLOAD: refused while the
 *  sheet's layout check fails. Empty drawing → envelope only. */
export async function fetchDrawingSvg(id: string): Promise<string> {
  const r = await fetch(drawingSvgUrl(API_HOST, id, { review: false }))
  if (!r.ok) {
    throw await refusalOf(r)
  }
  return r.text()
}

/** Fetch the sheet to SHOW on screen: a draft for human review even when
 *  its layout check fails (the sheet panel lists the findings). An unsound
 *  sheet is still refused. */
export async function previewDrawingSvg(id: string): Promise<string> {
  const r = await fetch(drawingSvgUrl(API_HOST, id, { review: true }))
  if (!r.ok) {
    throw await refusalOf(r)
  }
  return r.text()
}

/** Fetch the rendered PDF as a Blob suitable for direct download. */
export async function fetchDrawingPdf(id: string): Promise<Blob> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}/pdf`)
  if (!r.ok) {
    throw await refusalOf(r)
  }
  return r.blob()
}

/** Fetch the rendered DXF (ASCII) as a Blob suitable for direct download. */
export async function fetchDrawingDxf(id: string): Promise<Blob> {
  const r = await fetch(`${API_HOST}/api/drawings/${id}/dxf`)
  if (!r.ok) {
    throw await refusalOf(r)
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
