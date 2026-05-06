/**
 * Typed REST client for the backend `/api/sketch/*` surface. The
 * backend's `SketchManager` is the source of truth for in-progress
 * sketch sessions; the frontend only mirrors snapshots received via
 * REST responses and `Sketch*` WebSocket frames.
 *
 * Wire shape mirrors `roshera-backend/api-server/src/sketch.rs`
 * `SketchSession` (snake_case `circle_segments`, `created_at`,
 * `updated_at`). The store translates to its camelCase view.
 *
 * Multi-shape model (Slice 1+2 of the multi-shape plan): a session
 * carries `shapes: SketchShape[]`, each with its own tool / role /
 * points. The active (in-progress) shape is invariantly the *last*
 * element. Legacy point/tool/clear endpoints target the active shape;
 * `/shape/{idx}/...` endpoints address shapes explicitly.
 */

import type { SketchPlane, SketchTool } from '@/stores/scene-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/** Role of a sketch shape — Outer adds material, Hole subtracts. */
export type ShapeRole = 'outer' | 'hole'

/** Backend `SketchShape` wire shape. */
export interface ServerSketchShape {
  id: string
  tool: SketchTool
  role: ShapeRole
  points: Array<[number, number]>
}

/** Backend `SketchSession` wire shape — snake_case as serialised. */
export interface ServerSketchSession {
  id: string
  plane: SketchPlane
  shapes: ServerSketchShape[]
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
  /** Append a point to the active (last) shape. */
  addPoint(id: string, point: [number, number]): Promise<ServerSketchSession> {
    return request('POST', `/sketch/${id}/point`, { point })
  },
  /** Pop the last point of the active shape. */
  popPoint(id: string): Promise<ServerSketchSession> {
    return request('DELETE', `/sketch/${id}/point/last`)
  },
  /** Replace a single point on the active shape. */
  setPoint(
    id: string,
    index: number,
    point: [number, number],
  ): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/point/${index}`, { point })
  },
  /** Clear all points on the active shape. */
  clearPoints(id: string): Promise<ServerSketchSession> {
    return request('DELETE', `/sketch/${id}/points`)
  },
  setPlane(id: string, plane: SketchPlane): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/plane`, { plane })
  },
  /**
   * Resolve a face on a B-Rep object into a face-anchored
   * `SketchPlane::Custom { origin, u_axis, v_axis }`. The endpoint
   * downcasts the face's surface to a `Plane`, takes the face's
   * outward normal at (0.5, 0.5), and re-derives v_axis = n × u_axis
   * for a right-handed in-plane frame. Returns the plane in the
   * untagged object form the rest of the sketch surface accepts.
   */
  planeFromFace(
    objectId: string,
    faceId: number,
  ): Promise<SketchPlane> {
    return request('POST', '/sketch/plane-from-face', {
      object_id: objectId,
      face_id: faceId,
    })
  },
  /** Set the tool of the active (last) shape. Clears its points. */
  setTool(id: string, tool: SketchTool): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/tool`, { tool })
  },
  setCircleSegments(id: string, segments: number): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/circle-segments`, { segments })
  },
  /**
   * Append a fresh empty shape with the given tool + role; it becomes
   * the new active (last) shape. Used by the multi-shape UI flow when
   * the user has finished an outline and wants to start a hole inside.
   */
  addShape(
    id: string,
    body: { tool: SketchTool; role?: ShapeRole },
  ): Promise<ServerSketchSession> {
    return request('POST', `/sketch/${id}/shape`, body)
  },
  /**
   * Drop the shape at `idx`. Backend refuses to remove the last
   * remaining shape so the session invariant (≥1 shape) holds.
   */
  deleteShape(id: string, idx: number): Promise<ServerSketchSession> {
    return request('DELETE', `/sketch/${id}/shape/${idx}`)
  },
  setShapeRole(
    id: string,
    idx: number,
    role: ShapeRole,
  ): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/shape/${idx}/role`, { role })
  },
  setShapeTool(
    id: string,
    idx: number,
    tool: SketchTool,
  ): Promise<ServerSketchSession> {
    return request('PUT', `/sketch/${id}/shape/${idx}/tool`, { tool })
  },
  addPointToShape(
    id: string,
    idx: number,
    point: [number, number],
  ): Promise<ServerSketchSession> {
    return request('POST', `/sketch/${id}/shape/${idx}/point`, { point })
  },
  /**
   * Finalise a sketch into a solid. Backend extrudes every shape
   * independently and folds via boolean (Outer→Union, Hole→Difference)
   * starting from the first Outer. The resulting solid is broadcast
   * as `ObjectCreated`; the sketch is then dropped (unless
   * `consume: false` is passed).
   */
  extrude(
    id: string,
    body: { distance: number; direction?: [number, number, number]; name?: string; consume?: boolean },
  ): Promise<ExtrudeSketchResponse> {
    return request('POST', `/sketch/${id}/extrude`, body)
  },
}
