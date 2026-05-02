/**
 * Geometry helpers for the in-canvas 2D sketch preview.
 *
 * The actual extrude pipeline has moved to the backend
 * (`POST /api/sketch/{id}/extrude` — see `sketch-api.ts`); the
 * sketch session is the source of truth and the kernel materialises
 * the polygon + lifts it. The helpers below are kept for the panel's
 * live perimeter / area summary, where running the materialisation
 * client-side avoids a REST round-trip on every cursor tick.
 *
 * Coordinate convention: sketch points are stored in plane-local
 * (u, v) coordinates; the backend lifts to world coordinates on
 * extrude.
 */

import type { SketchTool } from '@/stores/scene-store'

/**
 * Materialise the current sketch tool's input into the closed polygon
 * the backend would build on extrude. Returns `null` when the input is
 * degenerate (e.g. polyline with < 3 points, rectangle with < 2
 * corners, zero-radius circle). Output is in plane-local (u, v) and
 * does NOT repeat the first point at the end.
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
