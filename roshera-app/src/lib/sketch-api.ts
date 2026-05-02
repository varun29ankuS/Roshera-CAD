/**
 * Typed REST client for the backend `/api/sketch/*` surface. The
 * backend's `SketchManager` is the source of truth for in-progress
 * sketch sessions; the frontend only mirrors snapshots received via
 * REST responses and `Sketch*` WebSocket frames.
 *
 * Wire shape mirrors `roshera-backend/api-server/src/sketch.rs`
 * `SketchSession` (snake_case `circle_segments`, `created_at`,
 * `updated_at`). The store translates to its camelCase view.
 */

import type { SketchPlane, SketchTool } from '@/stores/scene-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/** Backend `SketchSession` wire shape — snake_case as serialised. */
export interface ServerSketchSession {
  id: string
  plane: SketchPlane
  tool: SketchTool
  points: Array<[number, number]>
  circle_segments: number
  created_at: number
  updated_at: number
}

export interface ExtrudeSketchResponse {
  success: boolean
  sketch_id: string
  consumed: boolean
  solid_id: number
  object: {
    id: string
    name: string
    objectType: string
  }
  stats?: {
    vertex_count?: number
    triangle_count?: number
    tessellation_ms?: number
  }
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const resp = await fetch(`${API_BASE}${path}`, {
    method,
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  if (!resp.ok) {
    const err = await resp.json().catch(() => ({}))
    throw new Error(err?.message || err?.error || `HTTP ${resp.status}`)
  }
  return (await resp.json()) as T
}

export const sketchApi = {
  create(plane: SketchPlane, tool: SketchTool): Promise<ServerSketchSession> {
    return request('POST', '/sketch', { plane, tool })
  },
  get(id: string): Promise<ServerSketchSession> {
    return request('GET', `/sketch/${id}`)
  },
  list(): Promise<ServerSketchSession[]> {
    return request('GET', '/sketch')
  },
  delete(id: string): Promise<{ ok: boolean; removed: boolean }> {
    return request('DELETE', `/sketch/${id}`)
  },
  addPoint(id: string, point: [number, number]): Promise<ServerSketchSession> {
    return request('POST', `/sketch/${id}/point`, { point })
  },
  popPoint(id: string): Promise<ServerSketchSession> {
    return request('DELETE', `/sketch/${id}/point/last`)
  },
  setPoint(
    id: string,
    index: number,
    point: [number, number],
  ): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/point/${index}`, { point })
  },
  clearPoints(id: string): Promise<ServerSketchSession> {
    return request('DELETE', `/sketch/${id}/points`)
  },
  setPlane(id: string, plane: SketchPlane): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/plane`, { plane })
  },
  setTool(id: string, tool: SketchTool): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/tool`, { tool })
  },
  setCircleSegments(id: string, segments: number): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/circle-segments`, { segments })
  },
  /**
   * Finalise a sketch into a solid. Backend materialises the polygon,
   * lifts to the plane, and runs the same `extrude_profile` pipeline
   * as `/api/geometry/extrude`. The resulting solid is broadcast as
   * `ObjectCreated`; the sketch is then dropped (unless
   * `consume: false` is passed).
   */
  extrude(
    id: string,
    body: { distance: number; direction?: [number, number, number]; name?: string; consume?: boolean },
  ): Promise<ExtrudeSketchResponse> {
    return request('POST', `/sketch/${id}/extrude`, body)
  },
}
