/**
 * Dragging a view on the drawing sheet: the pure part.
 *
 * The sheet on screen is the kernel's SVG. A view group is drawn at
 * `translate(position_mm[0], sheet_h - position_mm[1]) scale(s, -s)`
 * (`geometry-engine/src/drawing/svg.rs`), so a view's `position_mm[0]` runs
 * with the screen's x and its `position_mm[1]` runs UP the sheet. A drag is
 * measured in screen px and becomes one `PATCH .../views/{id}` with the new
 * `position_mm` on the drop. Placement rules (what may overlap, what stays on
 * a projection line) are the kernel's; nothing here decides them.
 *
 * Dependency free so it can be unit-tested directly (`test/drawing-drag.test.mjs`).
 */

/** Mouse travel (screen px) at or below which a press is a click, not a drag. */
export const DRAG_THRESHOLD_PX = 4

/** True when a press moved far enough to be a drag. */
export function isDrag(dxPx: number, dyPx: number, threshold: number = DRAG_THRESHOLD_PX): boolean {
  return Math.hypot(dxPx, dyPx) > threshold
}

/**
 * Screen px per sheet mm as the sheet is rendered right now, measured off the
 * sheet element (follows zoom), or `null` when the sheet has no usable size.
 */
export function screenPxPerMm(renderedWidthPx: number, sheetWidthMm: number): number | null {
  const v = renderedWidthPx / sheetWidthMm
  return Number.isFinite(v) && v > 0 ? v : null
}

/** Round to 0.01 mm, never yielding -0. */
function mm(v: number): number {
  return Math.round(v * 100) / 100 + 0
}

/**
 * A view's `position_mm` after a drag of (`dxPx`, `dyPx`) screen px: right
 * on screen is +x on the sheet; down on screen is -y in view position.
 */
export function draggedPosition(
  start: readonly [number, number],
  dxPx: number,
  dyPx: number,
  pxPerMm: number,
): [number, number] {
  return [mm(start[0] + dxPx / pxPerMm), mm(start[1] - dyPx / pxPerMm)]
}

/**
 * The edit a drop sends, or `null` when it sends nothing: a click (a
 * zero-length move would still record an undo step), an unusable sheet
 * scale, or a move that rounds to the view's own position.
 */
export function planViewDrag(
  start: readonly [number, number],
  dxPx: number,
  dyPx: number,
  pxPerMm: number,
): { position_mm: [number, number] } | null {
  if (!isDrag(dxPx, dyPx)) return null
  if (!Number.isFinite(pxPerMm) || pxPerMm <= 0) return null
  const p = draggedPosition(start, dxPx, dyPx, pxPerMm)
  if (p[0] === mm(start[0]) && p[1] === mm(start[1])) return null
  return { position_mm: p }
}

function fmtMm(v: number): string {
  return String(Number(Math.abs(v).toFixed(2)))
}

/** "FRONT moved 12.5 mm right, 3 mm down", from the old and new position. */
export function describeMove(
  name: string,
  from: readonly [number, number],
  to: readonly [number, number],
): string {
  const dx = mm(to[0] - from[0])
  const dy = mm(to[1] - from[1])
  const parts: string[] = []
  if (dx !== 0) parts.push(`${fmtMm(dx)} mm ${dx > 0 ? 'right' : 'left'}`)
  if (dy !== 0) parts.push(`${fmtMm(dy)} mm ${dy > 0 ? 'up' : 'down'}`)
  return parts.length ? `${name} moved ${parts.join(', ')}` : `${name} did not move`
}

/** The slice of the kernel's `DrawingQualityReport` a move is checked against. */
export interface SheetQuality {
  passed: boolean
  issues: { severity: string; kind: string; message: string; view?: string | null }[]
}

/**
 * Findings of the kernel's sheet check (`GET /api/drawings/{id}/quality`)
 * that are about where views sit: overlapping each other or the title
 * block, past the frame, out of line with FRONT.
 */
const LAYOUT_ISSUE_KINDS = new Set([
  'view_overlap',
  'view_outside_frame',
  'view_overlaps_title_block',
  'projection_misaligned',
])

/**
 * The layout findings present after a move that were not there before it, in
 * the kernel's words. Without a before-check (`null`), every layout finding
 * after the move is returned.
 */
export function newLayoutIssues(before: SheetQuality | null, after: SheetQuality): string[] {
  const had = new Set(
    (before?.issues ?? []).filter((i) => LAYOUT_ISSUE_KINDS.has(i.kind)).map((i) => i.message),
  )
  return after.issues
    .filter((i) => LAYOUT_ISSUE_KINDS.has(i.kind) && !had.has(i.message))
    .map((i) => i.message)
}
