/**
 * Sliding through the timeline: the viewport shows the part as it stood
 * right after a chapter. The kernel rebuilds that state in a scratch model
 * from its frame cache (GET /api/timeline/scrub/{branch}/{seq}) -- the live
 * model is never touched -- and the viewport draws it read-only in place of
 * the live part until the user rolls back to the end.
 */
import type { CADObject } from '@/stores/scene-store'

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

interface ScrubMesh {
  vertices: number[]
  indices: number[]
  normals: number[]
  face_ids?: number[]
}

interface ScrubObject {
  id: string
  name?: string
  mesh: ScrubMesh
}

export interface ScrubAnswer {
  at_sequence?: number
  objects?: ScrubObject[]
}

/** The part at a past step, as the viewport draws it: read-only (`locked`),
 *  at its own coordinates (scrub meshes are already in world space). */
export function scrubObjects(answer: ScrubAnswer): CADObject[] {
  return (answer.objects ?? []).map((o) => ({
    id: o.id,
    name: o.name ?? o.id,
    objectType: 'solid',
    mesh: {
      vertices: new Float32Array(o.mesh.vertices),
      indices: new Uint32Array(o.mesh.indices),
      normals: new Float32Array(o.mesh.normals),
      faceIds: o.mesh.face_ids ? new Uint32Array(o.mesh.face_ids) : undefined,
    },
    material: { color: '#9a9a9a', metalness: 0.1, roughness: 0.6, opacity: 1 },
    position: [0, 0, 0],
    rotation: [0, 0, 0],
    scale: [1, 1, 1],
    visible: true,
    locked: true,
  }))
}

export async function fetchScrub(branch: string, seq: number): Promise<CADObject[]> {
  const r = await fetch(`${API_HOST}/api/timeline/scrub/${encodeURIComponent(branch)}/${seq}`)
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  return scrubObjects((await r.json()) as ScrubAnswer)
}

/** Which frame a drag at screen x is over: frames `width` px wide starting
 *  at `left`, clamped to the first and last. */
export function frameIndexAt(x: number, left: number, width: number, count: number): number {
  if (count <= 0) return 0
  const i = Math.floor((x - left) / width)
  return Math.min(count - 1, Math.max(0, i))
}

/**
 * One chapter back (`dir` -1) or forward (+1) from the step being viewed
 * (`current`; null = the live part). The last chapter's picture IS the live
 * part, so stepping onto it returns null (back to live), and stepping back
 * from live lands on the chapter before the last. Stops at the first.
 */
export function stepTo(anchors: number[], current: number | null, dir: -1 | 1): number | null {
  const last = anchors.length - 1
  if (current === null) {
    if (dir === 1) return null
    return last >= 1 ? anchors[last - 1] : null
  }
  const i = anchors.indexOf(current)
  if (i < 0) return null
  if (dir === 1) return i + 1 >= last ? null : anchors[i + 1]
  return anchors[Math.max(0, i - 1)]
}
