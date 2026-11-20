/**
 * The agent LOOKING at a sheet space (agent eyes, slice 2): where the Drawing
 * workspace's viewer glides to, and how. The server emits one AgentShow
 * `sheet_space` frame when the agent looks (GET /api/drawings/{id}/render.png
 * with the agent's header); the store (stores/agent-show-store) keeps it and
 * the workspace (DrawingsWorkspace) carries it out with these pure pieces.
 *
 * The viewer's model (DrawingsWorkspace): the sheet is `sheetW x sheetH` px
 * at zoom 1 (96 dpi, `PX_PER_MM`), drawn centred in the canvas, scaled by
 * `zoom` and translated by `pan`. A paper point (x, y) mm is on screen at
 *   canvasCentre + pan + zoom * ((x, y) * PX_PER_MM - sheetSize / 2).
 */
import { fitSheetScale, SHEET_FIT_INSETS } from './sheet-fit.ts'

/** CSS pixels per paper millimetre at zoom 1 (the workspace's 96 dpi). */
export const PX_PER_MM = 96 / 25.4

/** A box on the paper, mm, origin top-left, y down (the space map's frame). */
export interface SheetRect {
  x0: number
  y0: number
  x1: number
  y1: number
}

/** The screen room kept round a looked-at space, px on each side. */
export const LOOK_MARGIN_PX = 48
/** The viewer's own zoom limits (wheel and buttons clamp to the same). */
export const LOOK_ZOOM_MIN = 0.05
export const LOOK_ZOOM_MAX = 16
/** How long the glide takes, ms (instant under reduced motion). */
export const LOOK_GLIDE_MS = 650

export interface ViewerPose {
  zoom: number
  pan: { x: number; y: number }
}

/** True for a finite box with x1 >= x0 and y1 >= y0. */
export function isSheetRect(r: unknown): r is SheetRect {
  if (!r || typeof r !== 'object') return false
  const o = r as Record<string, unknown>
  const v = [o.x0, o.y0, o.x1, o.y1]
  if (!v.every((n) => typeof n === 'number' && Number.isFinite(n))) return false
  return (o.x1 as number) >= (o.x0 as number) && (o.y1 as number) >= (o.y0 as number)
}

/**
 * Where the viewer goes to show `rect`: the zoom that fits the box with
 * `LOOK_MARGIN_PX` on every side inside the room the sheet's overlays leave
 * (`SHEET_FIT_INSETS`), and the pan that puts the box's centre at the centre
 * of that room. No box (the whole sheet) is the workspace's own Fit.
 */
export function sheetLookTarget(
  canvas: { w: number; h: number },
  sheetMm: { w: number; h: number },
  rect: SheetRect | null,
): ViewerPose {
  const sheetW = sheetMm.w * PX_PER_MM
  const sheetH = sheetMm.h * PX_PER_MM
  if (!rect) return { zoom: fitSheetScale(canvas.w, canvas.h, sheetW, sheetH), pan: { x: 0, y: 0 } }
  const roomW = canvas.w - 2 * SHEET_FIT_INSETS.side - 2 * LOOK_MARGIN_PX
  const roomH = canvas.h - SHEET_FIT_INSETS.top - SHEET_FIT_INSETS.bottom - 2 * LOOK_MARGIN_PX
  // A zero-width lane is a line: fit it as a 1 mm box.
  const boxW = Math.max(rect.x1 - rect.x0, 1) * PX_PER_MM
  const boxH = Math.max(rect.y1 - rect.y0, 1) * PX_PER_MM
  const fit = Math.min(roomW / boxW, roomH / boxH)
  const zoom = Math.min(LOOK_ZOOM_MAX, Math.max(LOOK_ZOOM_MIN, Number.isFinite(fit) && fit > 0 ? fit : LOOK_ZOOM_MIN))
  // The room is centred vertically (equal top and bottom insets), so the
  // box's centre goes to the canvas centre.
  const cx = ((rect.x0 + rect.x1) / 2) * PX_PER_MM
  const cy = ((rect.y0 + rect.y1) / 2) * PX_PER_MM
  return { zoom, pan: { x: zoom * (sheetW / 2 - cx), y: zoom * (sheetH / 2 - cy) } }
}

/**
 * The glide from `from` to `to`: how long it runs and the pose at time `t`
 * ms. Zoom moves geometrically (a steady feel at every scale), pan linearly,
 * both eased. Reduced motion is instant: zero duration, every `t` the end.
 */
export function lookGlide(
  from: ViewerPose,
  to: ViewerPose,
  reducedMotion: boolean,
  durationMs: number = LOOK_GLIDE_MS,
): { durationMs: number; at: (t: number) => ViewerPose } {
  if (reducedMotion || durationMs <= 0) return { durationMs: 0, at: () => to }
  const ease = (u: number) => (u < 0.5 ? 4 * u * u * u : 1 - Math.pow(-2 * u + 2, 3) / 2)
  return {
    durationMs,
    at: (t) => {
      const u = ease(Math.min(1, Math.max(0, t / durationMs)))
      if (u >= 1) return to
      const zoom = from.zoom * Math.pow(to.zoom / from.zoom, u)
      return {
        zoom,
        pan: { x: from.pan.x + (to.pan.x - from.pan.x) * u, y: from.pan.y + (to.pan.y - from.pan.y) * u },
      }
    },
  }
}

/** The user's motion setting, read where a browser is present. */
export function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true
}
