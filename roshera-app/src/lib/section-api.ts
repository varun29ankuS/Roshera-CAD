/**
 * REST client for the kernel section-preview surface
 * (`POST /api/section/preview`).
 *
 * Section preview is a pure read-only query: hand the kernel a plane,
 * receive triangulated cap meshes (one per closed cross-section loop)
 * with vertex normals pinned to the plane normal. The frontend renders
 * the caps as flat polygons, which fills the visual "hollow" left by
 * Three.js's per-material clipping plane (CADMesh.tsx).
 *
 * No WebSocket broadcast — the section state never mutates the model;
 * dragging the offset slider re-issues this query at debounce cadence.
 */

const API_HOST = import.meta.env.VITE_API_URL || ''

/**
 * One cap mesh: a flat polygon lying on the cutting plane, in world
 * coordinates. `solidId` is the UUID of the parent solid (matches the
 * frontend `CADObject.id` directly — frontend object identity *is* the
 * kernel UUID; see `ws-bridge.ts::ObjectCreated`).
 */
export interface SectionCapMesh {
  solidId: string
  planeOrigin: [number, number, number]
  planeNormal: [number, number, number]
  vertices: Float32Array
  indices: Uint32Array
  normals: Float32Array
}

/**
 * Wire shape for `POST /api/section/preview`. `solids` is optional:
 * when present, only the listed solids are sectioned; when omitted the
 * server sections every live solid in the active model — the all-solids
 * path is what the viewport uses for the global section view.
 */
interface SectionPreviewBody {
  plane_origin: [number, number, number]
  plane_normal: [number, number, number]
  solids?: string[]
}

interface SectionCapDto {
  solid_id: string
  plane_origin: [number, number, number]
  plane_normal: [number, number, number]
  vertices: number[]
  indices: number[]
  normals: number[]
}

interface SectionPreviewResponse {
  caps: SectionCapDto[]
}

/**
 * Request triangulated cross-section caps for the given plane.
 *
 * `planeOrigin` is any point on the plane; `planeNormal` selects the
 * cutting direction (need not be unit-length — server tolerates non-
 * normalised vectors, will reject zero). `solidIds`, when supplied,
 * restricts the section to those solids; omit (or pass `undefined`) to
 * section every solid in the active model.
 *
 * Failures of a single solid (e.g. open-loop chain) are logged
 * server-side and that solid is skipped — the partial cap list still
 * returns 200, mirroring the kernel's degrade-gracefully policy.
 * Network / 4xx errors throw.
 */
export async function fetchSectionPreview(
  planeOrigin: [number, number, number],
  planeNormal: [number, number, number],
  solidIds?: string[],
): Promise<SectionCapMesh[]> {
  const body: SectionPreviewBody = {
    plane_origin: planeOrigin,
    plane_normal: planeNormal,
  }
  if (solidIds && solidIds.length > 0) {
    body.solids = solidIds
  }

  const resp = await fetch(`${API_HOST}/api/section/preview`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })

  if (!resp.ok) {
    let detail = ''
    try {
      detail = await resp.text()
    } catch {
      /* status code is the primary signal */
    }
    throw new Error(
      `section preview: ${resp.status} ${resp.statusText}${detail ? ` — ${detail}` : ''}`,
    )
  }

  const json = (await resp.json()) as SectionPreviewResponse
  return json.caps.map((cap) => ({
    solidId: cap.solid_id,
    planeOrigin: cap.plane_origin,
    planeNormal: cap.plane_normal,
    vertices: new Float32Array(cap.vertices),
    indices: new Uint32Array(cap.indices),
    normals: new Float32Array(cap.normals),
  }))
}
