/**
 * Convert an interactive 2D sketch into a closed polygon profile and
 * ship it to the backend `/api/geometry/extrude` endpoint. The kernel
 * builds a face from the polygon and lifts it by `thickness` along the
 * sketch plane's outward normal, then broadcasts `ObjectCreated` so
 * `ws-bridge` adds the resulting solid to the scene.
 *
 * Coordinate convention: sketch points are stored in plane-local
 * (u, v) coordinates. Lifting back to world coordinates:
 *   xy → (u, v, 0)   normal +Z
 *   xz → (u, 0, v)   normal +Y
 *   yz → (0, u, v)   normal +X
 */

import type { SketchPlane, SketchTool } from '@/stores/scene-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

const MIN_AREA = 1e-6
const MIN_PERIMETER = 1e-4

export interface SketchExtrudeRequest {
  plane: SketchPlane
  tool: SketchTool
  /** Confirmed sketch points in plane-local (u, v) coordinates. */
  points: Array<[number, number]>
  /** Extrude distance along the plane's outward normal (millimetres). */
  thickness: number
  /** Tessellation steps for `circle`. Ignored for polyline / rectangle. */
  circleSegments: number
  /** Optional human-readable name for the resulting solid. */
  name?: string
}

export interface SketchExtrudeResult {
  solidId: string
  objectId: string
  vertexCount?: number
  triangleCount?: number
  tessellationMs?: number
}

/** Convert a (u, v) point to a [x, y, z] tuple on the chosen sketch plane. */
export function liftToPlane(
  point: [number, number],
  plane: SketchPlane,
): [number, number, number] {
  const [u, v] = point
  switch (plane) {
    case 'xy':
      return [u, v, 0]
    case 'xz':
      return [u, 0, v]
    case 'yz':
      return [0, u, v]
  }
}

/** Outward normal of the sketch plane, used as the extrude direction. */
export function planeNormal(plane: SketchPlane): [number, number, number] {
  switch (plane) {
    case 'xy': return [0, 0, 1]
    case 'xz': return [0, 1, 0]
    case 'yz': return [1, 0, 0]
  }
}

/**
 * Materialise the current sketch tool's input into the closed polygon
 * the backend expects. Returns `null` when the input is degenerate
 * (e.g. polyline with < 3 points, rectangle with < 2 corners,
 * zero-radius circle). Output is in plane-local (u, v) coordinates and
 * does NOT repeat the first point at the end — the kernel closes the
 * loop implicitly.
 */
export function buildProfile2D(
  tool: SketchTool,
  points: Array<[number, number]>,
  circleSegments: number,
): Array<[number, number]> | null {
  switch (tool) {
    case 'polyline': {
      if (points.length < 3) return null
      // Strip an accidentally repeated terminal point (some UIs close the
      // loop with an extra click on the first vertex).
      const tail = points[points.length - 1]
      const head = points[0]
      if (
        Math.abs(tail[0] - head[0]) < 1e-6 &&
        Math.abs(tail[1] - head[1]) < 1e-6
      ) {
        return points.slice(0, -1)
      }
      return points.slice()
    }

    case 'rectangle': {
      if (points.length < 2) return null
      const [a, b] = points
      const x0 = Math.min(a[0], b[0])
      const x1 = Math.max(a[0], b[0])
      const y0 = Math.min(a[1], b[1])
      const y1 = Math.max(a[1], b[1])
      if (x1 - x0 < 1e-6 || y1 - y0 < 1e-6) return null
      // CCW so the resulting face normal points along the plane's +N.
      return [
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y1],
      ]
    }

    case 'circle': {
      if (points.length < 2) return null
      const [center, edge] = points
      const dx = edge[0] - center[0]
      const dy = edge[1] - center[1]
      const r = Math.hypot(dx, dy)
      if (r < 1e-6) return null
      const N = Math.max(8, Math.floor(circleSegments))
      const out: Array<[number, number]> = []
      // CCW starting at +u axis (consistent face-normal orientation).
      for (let i = 0; i < N; i++) {
        const t = (i / N) * Math.PI * 2
        out.push([center[0] + r * Math.cos(t), center[1] + r * Math.sin(t)])
      }
      return out
    }
  }
}

/** Signed polygon area in (u, v) — positive = CCW. */
export function signedArea(points: Array<[number, number]>): number {
  let a = 0
  for (let i = 0, n = points.length; i < n; i++) {
    const [x1, y1] = points[i]
    const [x2, y2] = points[(i + 1) % n]
    a += x1 * y2 - x2 * y1
  }
  return a * 0.5
}

/** Total perimeter of a closed (or open) polyline. */
export function perimeter(points: Array<[number, number]>, closed: boolean): number {
  let p = 0
  for (let i = 0; i < points.length - 1; i++) {
    p += Math.hypot(
      points[i + 1][0] - points[i][0],
      points[i + 1][1] - points[i][1],
    )
  }
  if (closed && points.length > 2) {
    const a = points[points.length - 1]
    const b = points[0]
    p += Math.hypot(b[0] - a[0], b[1] - a[1])
  }
  return p
}

/**
 * POST the finished sketch to `/api/geometry/extrude`. Throws on HTTP
 * failure or backend `success: false`. The caller should listen for
 * `ObjectCreated` on the WebSocket bridge to add the resulting solid
 * to the scene — this function does not touch the scene store.
 */
export async function extrudeSketch(
  req: SketchExtrudeRequest,
): Promise<SketchExtrudeResult> {
  const profile2D = buildProfile2D(req.tool, req.points, req.circleSegments)
  if (!profile2D) {
    throw new Error('sketch is empty or degenerate')
  }
  const area = Math.abs(signedArea(profile2D))
  if (area < MIN_AREA) {
    throw new Error('sketch encloses zero area')
  }
  if (perimeter(profile2D, true) < MIN_PERIMETER) {
    throw new Error('sketch is too small to extrude')
  }

  // Ensure CCW so the kernel's face normal aligns with our extrude
  // direction (avoids inverted-solid topology).
  const oriented =
    signedArea(profile2D) >= 0 ? profile2D : profile2D.slice().reverse()

  const profile3D = oriented.map((p) => liftToPlane(p, req.plane))
  const direction = planeNormal(req.plane)

  if (!Number.isFinite(req.thickness) || req.thickness <= 0) {
    throw new Error('thickness must be a positive finite number')
  }

  const resp = await fetch(`${API_BASE}/geometry/extrude`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      profile: profile3D,
      direction,
      distance: req.thickness,
      name: req.name ?? 'Sketch',
    }),
  })

  if (!resp.ok) {
    const errBody = await resp.json().catch(() => ({}))
    throw new Error(errBody?.message || errBody?.error || `HTTP ${resp.status}`)
  }

  const data = await resp.json()
  if (data?.success !== true) {
    throw new Error(data?.error || 'extrude failed')
  }

  return {
    solidId: data.solid_id,
    objectId: data.object?.id ?? data.solid_id,
    vertexCount: data.stats?.vertex_count,
    triangleCount: data.stats?.triangle_count,
    tessellationMs: data.stats?.tessellation_ms,
  }
}
