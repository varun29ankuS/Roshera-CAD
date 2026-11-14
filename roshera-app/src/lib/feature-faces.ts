/**
 * Where a feature is on the part: which of the part's faces it made.
 *
 * Each step recorded its geometry (a cylinder's radius, height and base
 * point: GET /api/timeline/parameters), and the kernel lists every face of
 * the finished part with its analytic surface (GET /api/agent/parts/{id}/
 * features). A feature's faces are the ones lying EXACTLY on what it
 * recorded:
 *  - its wall: a cylindrical face of the same radius whose axis passes
 *    through the feature's base point;
 *  - its ends: a plane square to that axis, through the base or the top.
 * A face that fits two features (the flange's top is also the plane the hub
 * sits on) belongs to the earlier one -- the one that made it. A feature
 * with nothing to match (a fillet records only its radius) is NOT placed:
 * no guess, the caller says so.
 */
import type { EditableParameter } from './feature-edit.ts'

export interface PartFace {
  face_id: number
  surface_kind: string
  radius: number | null
  axis: number[] | null
  origin: number[]
  normal: number[] | null
}

export interface LocatableChapter {
  anchor: number
  events: number[]
  instances: { events: number[] }[]
}

type V = [number, number, number]

interface RecordedCylinder {
  radius: number
  height: number
  base: V
}

const sub = (a: readonly number[], b: readonly number[]): V => [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
const dot = (a: readonly number[], b: readonly number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
const len = (a: readonly number[]) => Math.sqrt(dot(a, a))
const cross = (a: readonly number[], b: readonly number[]): V => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
]

/** The cylinders a chapter's steps recorded, one per step that made one. */
function recordedCylinders(events: number[], params: EditableParameter[]): RecordedCylinder[] {
  const out: RecordedCylinder[] = []
  for (const seq of events) {
    const mine = params.filter((p) => p.sequence === seq && p.command === 'create_cylinder_3d')
    const v = (k: string) => mine.find((p) => p.key === k)?.value
    const radius = v('radius')
    const height = v('height')
    if (radius === undefined || height === undefined) continue
    out.push({ radius, height, base: [v('base_x') ?? 0, v('base_y') ?? 0, v('base_z') ?? 0] })
  }
  return out
}

/** The faces lying on one recorded cylinder (its wall, and its ends). */
function facesOn(c: RecordedCylinder, faces: PartFace[]): number[] {
  const tol = 1e-6 * Math.max(1, c.radius, c.height)
  const walls = faces.filter((f) => {
    if (f.surface_kind !== 'cylinder' || f.radius === null || !f.axis) return false
    if (Math.abs(f.radius - c.radius) > tol) return false
    const a = f.axis
    const n = len(a)
    if (n === 0) return false
    // The base point lies on the face's axis line.
    return len(cross(sub(c.base, f.origin), a)) / n <= tol
  })
  if (walls.length === 0) return []
  const axis = walls[0].axis as number[]
  const unit = axis.map((x) => x / len(axis))
  const top: V = [c.base[0] + unit[0] * c.height, c.base[1] + unit[1] * c.height, c.base[2] + unit[2] * c.height]
  const ends = faces.filter((f) => {
    if (f.surface_kind !== 'plane' || !f.normal) return false
    const nn = len(f.normal)
    if (nn === 0 || Math.abs(Math.abs(dot(f.normal, unit)) / nn - 1) > 1e-9) return false
    return [c.base, top].some((p) => Math.abs(dot(sub(f.origin, p), unit)) <= tol)
  })
  return [...walls, ...ends].map((f) => f.face_id)
}

/** Each placeable chapter's faces, keyed by the chapter's anchor. */
export function locateFeatures(
  chapters: LocatableChapter[],
  params: EditableParameter[],
  faces: PartFace[],
): Map<number, number[]> {
  const owned = new Set<number>()
  const out = new Map<number, number[]>()
  for (const ch of chapters) {
    const groups = ch.instances.length > 0 ? ch.instances.map((i) => i.events) : [ch.events]
    const mine: number[] = []
    for (const g of groups) {
      for (const c of recordedCylinders(g, params)) {
        for (const id of facesOn(c, faces)) {
          if (owned.has(id)) continue
          owned.add(id)
          mine.push(id)
        }
      }
    }
    if (mine.length > 0) out.set(ch.anchor, mine)
  }
  return out
}
