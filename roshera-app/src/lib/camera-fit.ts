/**
 * How far the camera stands to fit something in view (Fit, a named view,
 * a document opening). The part fills about HALF the view -- the margin a
 * CAD tool leaves, and room for the panels laid over the viewport (tree,
 * history, Blackboard) -- and the tighter of height and width decides, so a
 * narrow viewport (Assembly, DFM with its panel) never cuts the part at the
 * sides. Varun 2026-11-14: "why is the part always all the way zoomed in?"
 */

/** Half the view, measured on the tighter axis. */
export const FIT_MARGIN = 2

/** Distance at which a sphere of `radius` fits with the margin, for a
 *  perspective camera of vertical field of view `vfovDeg` and `aspect`
 *  (width / height). */
export function fitDistance(radius: number, vfovDeg: number, aspect: number): number {
  const halfV = (vfovDeg * Math.PI) / 360
  const halfH = Math.atan(Math.tan(halfV) * Math.max(aspect, 1e-3))
  const half = Math.min(halfV, halfH)
  return (radius / Math.tan(half)) * FIT_MARGIN
}

/** The isometric viewing direction (from the target toward the camera),
 *  Z up: the view a part is shown in when it first appears. */
export const ISO_DIRECTION: [number, number, number] = (() => {
  const v: [number, number, number] = [1, -1, 0.75]
  const n = Math.hypot(v[0], v[1], v[2])
  return [v[0] / n, v[1] / n, v[2] / n]
})()

const Z_UP: [number, number, number] = [0, 0, 1]

/** Which way the camera looks when it frames something.
 *
 *  - `'all'` (a document opening) and `'new'` (a part just built) are fresh
 *    looks at the model: the iso direction, Z up. Varun 2026-11-20: "once
 *    the part is build, it should be zoomed in and rotated to be looked at
 *    properly" -- a part framed from wherever the camera happened to point
 *    (straight down, after a sketch on the top plane) is not looked at
 *    properly.
 *  - `'fit'` (the Fit button) keeps the user's direction and up.
 *  - While sketching, nothing turns the view off the sketch plane.
 *  A degenerate current direction falls back to iso. */
export function frameView(
  kind: 'all' | 'new' | 'fit',
  currentDirection: [number, number, number],
  currentUp: [number, number, number],
  sketching: boolean,
): { direction: [number, number, number]; up: [number, number, number] } {
  const n = Math.hypot(currentDirection[0], currentDirection[1], currentDirection[2])
  const usable = Number.isFinite(n) && n > 1e-6
  const current: [number, number, number] = usable
    ? [currentDirection[0] / n, currentDirection[1] / n, currentDirection[2] / n]
    : ISO_DIRECTION
  if (kind === 'all') return { direction: ISO_DIRECTION, up: Z_UP }
  if (kind === 'new' && !sketching) return { direction: ISO_DIRECTION, up: Z_UP }
  return { direction: current, up: usable ? currentUp : Z_UP }
}

/** How long a view change takes (a named view, Fit, a part being framed).
 *  A third of a second: long enough to see which way the model turned,
 *  short enough never to be waited for. It was 0.6 s. */
export const FLY_SECONDS = 0.3

/** The longest step one frame may take. The canvas renders on demand, so the
 *  first frame of a fly reports the time since the LAST frame drawn -- often
 *  many seconds -- and an unclamped step finished the whole fly in that one
 *  frame: the view jumped instead of turning. */
const FLY_MAX_STEP_SECONDS = 1 / 30

/** A fly's progress (0..1) after one more frame of `deltaSeconds`. With
 *  reduced motion the view arrives at once. */
export function advanceFly(progress: number, deltaSeconds: number, reducedMotion: boolean): number {
  if (reducedMotion) return 1
  const step = Math.min(Math.max(deltaSeconds, 0), FLY_MAX_STEP_SECONDS)
  return Math.min(progress + step / FLY_SECONDS, 1)
}

/** The view a camera opens with when the model is already loaded and no view
 *  was kept for it (the Assembly viewport's first visit): the iso view,
 *  fitted, Z up. Its fixed start pose is 30 mm from the origin -- inside any
 *  real part. `zoom` gives an orthographic camera the same footprint on a
 *  viewport `viewportHeightPx` tall. */
export function openingPose(
  center: [number, number, number],
  radius: number,
  vfovDeg: number,
  aspect: number,
  viewportHeightPx: number,
): { position: [number, number, number]; target: [number, number, number]; up: [number, number, number]; zoom: number } {
  const d = fitDistance(radius, vfovDeg, aspect)
  const zoom = viewportHeightPx > 0 ? viewportHeightPx / (2 * d * Math.tan((vfovDeg * Math.PI) / 360)) : 1
  return {
    position: [center[0] + ISO_DIRECTION[0] * d, center[1] + ISO_DIRECTION[1] * d, center[2] + ISO_DIRECTION[2] * d],
    target: [center[0], center[1], center[2]],
    up: [Z_UP[0], Z_UP[1], Z_UP[2]],
    zoom,
  }
}
