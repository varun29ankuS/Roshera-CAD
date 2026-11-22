/**
 * DFM finding animations: the kernel's descriptor (`geometry_engine::dfm::
 * animation::DfmAnimation`, the `animations` list on a DFM report) and the
 * pure sampling the viewport player draws from.
 *
 * The kernel is the source of truth. Every dimension (the end mill's
 * diameter, the drill's plunge depth, the caliper's gap) and every caption
 * number comes from the rule's own measured value and limit; the player
 * only interpolates the keyframes it was given — it never invents a size
 * or a position. Coordinates are kernel millimetres in the part's frame.
 */

export type Vec3 = [number, number, number]

export type AnimationKind =
  | 'tool_approach'
  | 'drill_plunge'
  | 'setup_cycle'
  | 'mold_separation'
  | 'layer_build'
  | 'caliper'
  | 'spindle_spin'
  | 'highlight'

export type ActorShape =
  | { type: 'end_mill'; diameter: number; flute_length: number; length: number }
  | { type: 'drill'; diameter: number; point_angle_deg: number; length: number }
  | { type: 'arrow'; length: number }
  | { type: 'plane'; size: number }
  | { type: 'caliper'; opening: number }
  | { type: 'axis'; length: number }

export interface ActorKey {
  t: number
  position: Vec3
  axis: Vec3
  opacity: number
  extent?: number
}

export interface Actor {
  id: string
  /** `tool` | `guide` | `needed` (the direction the feature needs). */
  role: string
  shape: ActorShape
  keys: ActorKey[]
}

export type FaceState = 'context' | 'fail' | 'ok'

export interface FaceKey {
  t: number
  offset: Vec3
  rotation_deg?: number
  state: FaceState
}

export interface FaceTrack {
  faces: number[]
  keys: FaceKey[]
}

export interface DfmAnimation {
  rule: string
  kind: AnimationKind
  duration_s: number
  caption: string
  measured: number
  limit: number
  faces: number[]
  actors: Actor[]
  face_tracks: FaceTrack[]
  spin_axis?: [Vec3, Vec3]
}

export interface ActorPose {
  position: Vec3
  axis: Vec3
  opacity: number
  extent: number
}

export interface FacePose {
  offset: Vec3
  rotationDeg: number
  state: FaceState
}

const clamp01 = (x: number) => (x < 0 ? 0 : x > 1 ? 1 : x)
const lerp = (a: number, b: number, s: number) => a + (b - a) * s
const lerp3 = (a: Vec3, b: Vec3, s: number): Vec3 => [lerp(a[0], b[0], s), lerp(a[1], b[1], s), lerp(a[2], b[2], s)]

function normalize(v: Vec3, fallback: Vec3): Vec3 {
  const n = Math.hypot(v[0], v[1], v[2])
  return n > 1e-12 ? [v[0] / n, v[1] / n, v[2] / n] : fallback
}

/** Smoothstep easing: motion starts and ends gently, keyframe values are
 *  still hit exactly at their own times. */
const ease = (s: number) => s * s * (3 - 2 * s)

/** The segment of `keys` (sorted by t) that `t` falls in, and how far. */
function segment<K extends { t: number }>(keys: K[], t: number): [K, K, number] | null {
  if (keys.length === 0) return null
  const tt = clamp01(t)
  if (tt <= keys[0].t) return [keys[0], keys[0], 0]
  for (let i = 0; i < keys.length - 1; i++) {
    const a = keys[i]
    const b = keys[i + 1]
    if (tt <= b.t) {
      const span = b.t - a.t
      return [a, b, span > 0 ? ease((tt - a.t) / span) : 1]
    }
  }
  const last = keys[keys.length - 1]
  return [last, last, 0]
}

/** An actor's pose at normalized time `t` (0..1). */
export function sampleActor(actor: Actor, t: number): ActorPose | null {
  const seg = segment(actor.keys, t)
  if (!seg) return null
  const [a, b, s] = seg
  return {
    position: lerp3(a.position, b.position, s),
    axis: normalize(lerp3(a.axis, b.axis, s), a.axis),
    opacity: clamp01(lerp(a.opacity, b.opacity, s)),
    extent: lerp(a.extent ?? 0, b.extent ?? 0, s),
  }
}

/** A face group's pose at `t`: offset and rotation interpolate; the state
 *  is the one most recently reached (a face turns red when its key says so,
 *  it does not fade through pink). */
export function sampleFaceTrack(track: FaceTrack, t: number): FacePose | null {
  const seg = segment(track.keys, t)
  if (!seg) return null
  const [a, b, s] = seg
  const tt = clamp01(t)
  let state: FaceState = track.keys[0].state
  for (const k of track.keys) if (k.t <= tt) state = k.state
  return {
    offset: lerp3(a.offset, b.offset, s),
    rotationDeg: lerp(a.rotation_deg ?? 0, b.rotation_deg ?? 0, s),
    state,
  }
}

/** Normalized time of a running animation `elapsedS` seconds in, holding
 *  the last frame for a beat, then looping. */
export function animationTime(anim: DfmAnimation, elapsedS: number, holdS = 1.2): number {
  const d = Math.max(anim.duration_s, 0.1)
  const cycle = d + holdS
  const e = ((elapsedS % cycle) + cycle) % cycle
  return e >= d ? 1 : e / d
}

/** The animation that explains a rule's finding, if the kernel sent one. */
export function animationFor(animations: DfmAnimation[] | undefined, rule: string): DfmAnimation | null {
  return animations?.find((a) => a.rule === rule) ?? null
}
