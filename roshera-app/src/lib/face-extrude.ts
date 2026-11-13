/**
 * Face extrude (push / pull a face of a solid) -- the request, built and
 * checked once, for both ways in: dragging the arrow on a face and typing a
 * distance in the Extrude panel. The kernel route is
 * `POST /api/geometry/face/extrude` (commit) and `.../preview` (a ghost,
 * no side effects).
 */

const API_BASE = `${(import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''}/api`

/** Out = add material along the face normal; in = push the face in (cut). */
export type ExtrudeDirection = 'out' | 'in'

/** Smallest pull the tool accepts, in mm (the arrow snaps no finer). */
const MIN_MM = 1e-6

/** A typed distance in mm, signed by the direction, or null when it is not
 *  a usable distance (empty, zero, not a number). An explicit minus sign
 *  is kept as typed. */
export function parseExtrudeDistance(text: string, direction: ExtrudeDirection): number | null {
  const m = /^\s*(-?\d+(?:\.\d+)?|-?\.\d+)\s*(mm)?\s*$/i.exec(text)
  if (!m) return null
  const v = Number(m[1])
  if (!Number.isFinite(v) || Math.abs(v) < MIN_MM) return null
  return direction === 'in' && v > 0 ? -v : v
}

export interface FaceExtrudeBody {
  object_uuid: string
  face_id: number
  distance: number
  direction: [number, number, number]
}

/** The request body, or an error naming what is wrong -- a request the
 *  kernel would refuse is never sent. */
export function faceExtrudeBody(
  objectUuid: string,
  faceId: number,
  distance: number,
  direction: [number, number, number],
): FaceExtrudeBody {
  if (!objectUuid) throw new Error('no part to extrude')
  if (!Number.isInteger(faceId) || faceId < 0) throw new Error(`not a face id: ${faceId}`)
  if (!Number.isFinite(distance) || Math.abs(distance) < MIN_MM) throw new Error('the distance must be non-zero')
  if (!direction.every(Number.isFinite) || Math.hypot(...direction) < 1e-9) {
    throw new Error('the direction must be a non-zero vector')
  }
  return { object_uuid: objectUuid, face_id: faceId, distance, direction: [...direction] }
}

export interface PreviewMesh {
  vertices: number[]
  indices: number[]
  normals: number[]
}

/** The ghost of the result, or null when the kernel has no shape for this
 *  distance (a degenerate step mid-drag is normal and stays quiet). */
export async function previewFaceExtrude(body: FaceExtrudeBody, signal?: AbortSignal): Promise<PreviewMesh | null> {
  const resp = await fetch(`${API_BASE}/geometry/face/extrude/preview`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal,
  })
  if (!resp.ok) return null
  const data = (await resp.json()) as { mesh?: Partial<PreviewMesh> }
  const m = data.mesh
  if (!m || !Array.isArray(m.vertices) || !Array.isArray(m.indices) || !Array.isArray(m.normals) || m.indices.length === 0) {
    return null
  }
  return { vertices: m.vertices, indices: m.indices, normals: m.normals }
}

/** Commit the extrude. The part updates through the server's broadcast;
 *  a refusal comes back as the kernel's own words. */
export async function commitFaceExtrude(body: FaceExtrudeBody): Promise<{ ok: true } | { ok: false; reason: string }> {
  try {
    const resp = await fetch(`${API_BASE}/geometry/face/extrude`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
    if (resp.ok) return { ok: true }
    const err = (await resp.json().catch(() => ({}))) as { message?: string; error?: string }
    return { ok: false, reason: err.message || err.error || `HTTP ${resp.status}` }
  } catch (e) {
    return { ok: false, reason: e instanceof Error ? e.message : String(e) }
  }
}
