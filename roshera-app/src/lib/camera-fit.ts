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
