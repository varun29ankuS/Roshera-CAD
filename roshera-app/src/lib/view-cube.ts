/**
 * The view cube: which view each of its 26 pieces means, and the steps a
 * view can take from there -- a quarter-turn roll, or over to the
 * neighbouring face. All in the kernel's Z-up world: +Z top, -Y front,
 * +X right.
 *
 * A view is two vectors: `direction`, from the model toward the camera (the
 * side the camera stands on), and `up`, the world direction drawn up the
 * screen. The cube is three kinds of piece: 6 faces (centred at the origin),
 * 12 edge boxes and 8 corner boxes placed out on the cube. A click on an
 * edge or corner box reports the normal of whichever side of the little box
 * was hit, so those go by where the box sits, not by the side hit.
 */

type V3 = [number, number, number]

interface Vec {
  x: number
  y: number
  z: number
}

export interface CubeView {
  /** Unit vector from the model toward the camera. */
  direction: V3
  /** The world direction drawn up the screen. */
  up: V3
}

export type FaceName = 'front' | 'back' | 'left' | 'right' | 'top' | 'bottom'

/** The six face views, as the app's named views draw them (scene-store's
 *  CAMERA_PRESETS): Z up, except straight down / up, where +Y / -Y is. */
export const FACE_VIEWS: Readonly<Record<FaceName, CubeView>> = {
  front: { direction: [0, -1, 0], up: [0, 0, 1] },
  back: { direction: [0, 1, 0], up: [0, 0, 1] },
  left: { direction: [-1, 0, 0], up: [0, 0, 1] },
  right: { direction: [1, 0, 0], up: [0, 0, 1] },
  top: { direction: [0, 0, 1], up: [0, 1, 0] },
  bottom: { direction: [0, 0, -1], up: [0, -1, 0] },
}

/** The word on each face, in the order the cube draws them
 *  (+X, -X, +Y, -Y, +Z, -Z): each is the view a click on that face gives. */
export const CUBE_FACE_LABELS: readonly string[] = ['RIGHT', 'LEFT', 'BACK', 'FRONT', 'TOP', 'BOTTOM']

/**
 * How many quarter turns ANTICLOCKWISE each face's word is drawn at on its
 * texture, in the same order, so that it reads upright in the view it names.
 * A box's texture axes are fixed for a Y-up world (on +X, "up" on the texture
 * is +Y); in the kernel's Z-up world the side faces' words would lie on their
 * sides without this. Derived per face from the box's texture axes (u, v) and
 * the view's screen axes:
 *   +X RIGHT : u = -Z, v = +Y; screen right +Y, up +Z  -> one turn
 *   -X LEFT  : u = +Z, v = +Y; screen right -Y, up +Z  -> three turns
 *   +Y BACK  : u = +X, v = -Z; screen right -X, up +Z  -> two turns
 *   -Y FRONT : u = +X, v = +Z; screen right +X, up +Z  -> none
 *   +Z TOP   : u = +X, v = +Y; screen right +X, up +Y  -> none
 *   -Z BOTTOM: u = -X, v = +Y; screen right +X, up -Y  -> two turns
 */
export const CUBE_FACE_TEXT_TURNS: readonly number[] = [1, 3, 2, 0, 0, 2]

const EPS = 1e-6
const sign = (c: number) => (Math.abs(c) < EPS ? 0 : c > 0 ? 1 : -1)

function normalize(v: V3): V3 {
  const n = Math.hypot(v[0], v[1], v[2])
  return n > 0 ? [v[0] / n, v[1] / n, v[2] / n] : [0, 0, 0]
}

const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
const neg = (a: V3): V3 => [-a[0], -a[1], -a[2]]
/** Snap away the float dust a quarter turn leaves (0.9999999 -> 1, 1e-17 -> 0). */
const tidy = (a: V3): V3 => a.map((c) => (Math.abs(c - Math.round(c)) < 1e-9 ? Math.round(c) : c)) as V3

/** The face a direction points at: its largest component (ties: Z, then X). */
function faceOf(n: V3): FaceName {
  const ax = Math.abs(n[0])
  const ay = Math.abs(n[1])
  const az = Math.abs(n[2])
  if (az >= ax && az >= ay) return n[2] > 0 ? 'top' : 'bottom'
  if (ax >= ay) return n[0] > 0 ? 'right' : 'left'
  return n[1] > 0 ? 'back' : 'front'
}

/**
 * The view a click on the cube means. `piece`: where the clicked piece sits
 * (the origin for a face); `normal`: the normal of the side that was hit.
 * `preset` names the app's own named view when the click is one (the six
 * faces, and the front-right-top corner, which is the home isometric), so
 * the view bar's buttons stay in step with the cube.
 */
export function cubeView(
  piece: Vec,
  normal: Vec,
): CubeView & { kind: 'face' | 'edge' | 'corner'; preset?: FaceName | 'isometric' } {
  const at: V3 = [sign(piece.x), sign(piece.y), sign(piece.z)]
  const off = at.filter((c) => c !== 0).length
  if (off === 0) {
    const face = faceOf([normal.x, normal.y, normal.z])
    return { kind: 'face', preset: face, ...FACE_VIEWS[face] }
  }
  const direction = normalize(at)
  // No edge or corner looks straight along Z, so Z is a valid screen-up for
  // every one of them.
  const up: V3 = [0, 0, 1]
  if (off === 3) {
    const home = at[0] === 1 && at[1] === -1 && at[2] === 1
    return { kind: 'corner', direction, up, ...(home ? { preset: 'isometric' as const } : {}) }
  }
  return { kind: 'edge', direction, up }
}

/** The face the camera is square-on to, or null. `direction` need not be a
 *  unit vector; within about a degree counts. */
export function squareOnFace(direction: V3): FaceName | null {
  const d = normalize(direction)
  if (d[0] === 0 && d[1] === 0 && d[2] === 0) return null
  const big = Math.max(Math.abs(d[0]), Math.abs(d[1]), Math.abs(d[2]))
  return big > 0.9998 ? faceOf(d) : null
}

/** Rotate `v` a quarter turn about the unit axis `k` (right-hand rule). */
function quarterTurn(v: V3, k: V3): V3 {
  // Rodrigues at 90 degrees: v' = k x v + k (k . v)
  const dot = k[0] * v[0] + k[1] * v[1] + k[2] * v[2]
  const c = cross(k, v)
  return [c[0] + k[0] * dot, c[1] + k[1] * dot, c[2] + k[2] * dot]
}

/**
 * Roll the view about the line of sight by quarter turns. Positive turns
 * the PICTURE clockwise on screen (so the camera's up goes anticlockwise).
 */
export function rollView(direction: V3, up: V3, quarterTurns: number): CubeView {
  const d = normalize(direction)
  let u: V3 = [up[0], up[1], up[2]]
  const n = ((Math.round(quarterTurns) % 4) + 4) % 4
  for (let k = 0; k < n; k++) u = quarterTurn(u, d)
  return { direction: tidy(d), up: tidy(u) }
}

/**
 * Step from a square-on view to the neighbouring face: `right` is the face
 * to the right of what is on screen, `up` the one above, and so on. Going
 * over the top keeps the picture continuous: what was "away from me" becomes
 * screen-up.
 */
export function adjacentView(direction: V3, up: V3, side: 'left' | 'right' | 'up' | 'down'): CubeView {
  const d = normalize(direction)
  const u = normalize(up)
  const right = cross(neg(d), u)
  switch (side) {
    case 'right':
      return { direction: tidy(right), up: tidy(u) }
    case 'left':
      return { direction: tidy(neg(right)), up: tidy(u) }
    case 'up':
      return { direction: tidy(u), up: tidy(neg(d)) }
    case 'down':
      return { direction: tidy(neg(u)), up: tidy(d) }
  }
}

/**
 * The face an arrow goes to. An arrow always arrives at that face's
 * STANDARD view (the one the view bar's button gives), so there is one Top,
 * one Right: only a roll turns the picture.
 */
export function neighbourFace(direction: V3, up: V3, side: 'left' | 'right' | 'up' | 'down'): FaceName {
  return faceOf(adjacentView(direction, up, side).direction)
}

/** Whether a world axis points straight at the viewer or straight away
 *  (within about 14 degrees): drawn, it would be a dot, so the triad drops
 *  it. `direction` is the view's (from the model toward the camera). */
export function axisEndOn(axis: V3, direction: V3): boolean {
  const a = normalize(axis)
  const d = normalize(direction)
  return Math.abs(a[0] * d[0] + a[1] * d[1] + a[2] * d[2]) > 0.97
}
