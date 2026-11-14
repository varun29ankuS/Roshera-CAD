/**
 * Sliding through the timeline: the viewport shows the part as it stood
 * right after a chapter. The kernel rebuilds that state in a scratch model
 * from its frame cache (GET /api/timeline/scrub/{branch}/{seq}) -- the live
 * model is never touched -- and the viewport draws it read-only in place of
 * the live part until the user rolls back to the end.
 */
import type { CADObject } from '@/stores/scene-store'
import { DEFAULT_PART_MATERIAL } from './part-material.ts'

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
    material: { ...DEFAULT_PART_MATERIAL },
    position: [0, 0, 0],
    rotation: [0, 0, 0],
    scale: [1, 1, 1],
    visible: true,
    locked: true,
  }))
}

interface BinaryObject {
  id: string
  name?: string
  nv: number
  ni: number
  nn: number
  nf: number
}

/**
 * Read a binary step (`RSCB` v1, `?format=bin`): the magic, a u32 version,
 * a u32 JSON length and that JSON (the objects and their array lengths),
 * padding to 4 bytes, then each object's vertices (f32), indices (u32),
 * normals (f32), face ids (u32). The arrays are VIEWS into the buffer -- no
 * text is parsed and nothing is copied, which for a 3 MB step is most of the
 * cost of showing it. Anything that is not RSCB v1 is refused.
 */
export function decodeScrubBinary(buf: ArrayBuffer): CADObject[] {
  const u8 = new Uint8Array(buf)
  const magic = String.fromCharCode(u8[0] ?? 0, u8[1] ?? 0, u8[2] ?? 0, u8[3] ?? 0)
  if (buf.byteLength < 12 || magic !== 'RSCB') throw new Error('not a binary step (RSCB)')
  const dv = new DataView(buf)
  const version = dv.getUint32(4, true)
  if (version !== 1) throw new Error(`RSCB version ${version} is not supported`)
  const jsonLen = dv.getUint32(8, true)
  const header = JSON.parse(new TextDecoder().decode(u8.subarray(12, 12 + jsonLen))) as {
    objects?: BinaryObject[]
  }
  let at = Math.ceil((12 + jsonLen) / 4) * 4
  const f32 = (n: number) => {
    const v = new Float32Array(buf, at, n)
    at += 4 * n
    return v
  }
  const u32 = (n: number) => {
    const v = new Uint32Array(buf, at, n)
    at += 4 * n
    return v
  }
  return (header.objects ?? []).map((o) => {
    const vertices = f32(o.nv)
    const indices = u32(o.ni)
    const normals = f32(o.nn)
    const faceIds = u32(o.nf)
    return {
      id: o.id,
      name: o.name ?? o.id,
      objectType: 'solid',
      mesh: { vertices, indices, normals, faceIds: o.nf > 0 ? faceIds : undefined },
      material: { ...DEFAULT_PART_MATERIAL },
      position: [0, 0, 0],
      rotation: [0, 0, 0],
      scale: [1, 1, 1],
      visible: true,
      locked: true,
    }
  })
}

export async function fetchScrub(branch: string, seq: number): Promise<CADObject[]> {
  const r = await fetch(`${API_HOST}/api/timeline/scrub/${encodeURIComponent(branch)}/${seq}?format=bin`)
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  return decodeScrubBinary(await r.arrayBuffer())
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
