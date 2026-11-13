/**
 * The viewport's view, kept while the viewport is away. Switching to the
 * Assembly or Drawing workspace unmounts the 3D viewport; coming back
 * mounts a fresh camera at its fixed start pose, 30 mm from the origin --
 * inside any part bigger than a few centimetres. Keeping the pose the user
 * left and restoring it on return is what a CAD tool does.
 */

type V3 = [number, number, number]

export interface CameraPose {
  position: V3
  target: V3
  up: V3
  /** Orthographic zoom (perspective ignores it). */
  zoom: number
}

let kept: CameraPose | null = null

const finite = (v: V3) => v.every((x) => Number.isFinite(x))

/** Remember the view. A pose that cannot be a view (non-finite numbers,
 *  the camera on its own target, zero zoom) is ignored -- the last good
 *  one stays. */
export function keepCameraPose(p: CameraPose): void {
  if (!finite(p.position) || !finite(p.target) || !finite(p.up)) return
  if (!(Number.isFinite(p.zoom) && p.zoom > 0)) return
  const d = Math.hypot(p.position[0] - p.target[0], p.position[1] - p.target[1], p.position[2] - p.target[2])
  if (!(d > 0)) return
  kept = { position: [...p.position], target: [...p.target], up: [...p.up], zoom: p.zoom }
}

/** The view to return to, or null if none was kept. A copy: changing it
 *  does not change what is kept. */
export function keptCameraPose(): CameraPose | null {
  return kept && { position: [...kept.position], target: [...kept.target], up: [...kept.up], zoom: kept.zoom }
}
