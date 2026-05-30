/**
 * Sketch-plane ↔ world-space coordinate helpers.
 *
 * Extracted from `SketchOverlay.tsx` so the component module exports only
 * components (Vite fast-refresh requires this — see
 * `react-refresh/only-export-components`). `uvToWorld` is consumed by
 * `SketchOverlay` and `ExtrudeRegionPreview`.
 *
 * Coordinate convention matches `lib/sketch-extrude.ts` and the backend
 * `SketchPlane::lift`:
 *   xy: (u, v) → (u, v, 0), normal +Z
 *   xz: (u, v) → (u, 0, v), normal +Y
 *   yz: (u, v) → (0, u, v), normal +X
 *   custom: (u, v) → origin + u·u_axis + v·v_axis, normal = u × v
 */

import * as THREE from 'three'
import { isStandardPlane, type SketchPlane } from '@/stores/scene-store'

/** Lift a sketch-plane (u, v) onto its world-space point. Inverse of the
 *  `pointToPlaneUV` projection. Mirrors the backend's `SketchPlane::lift`
 *  so frontend ghost geometry agrees with what the server records. */
export function uvToWorld(point: [number, number], plane: SketchPlane): THREE.Vector3 {
  const [u, v] = point
  if (isStandardPlane(plane)) {
    switch (plane) {
      case 'xy':
        return new THREE.Vector3(u, v, 0)
      case 'xz':
        return new THREE.Vector3(u, 0, v)
      case 'yz':
        return new THREE.Vector3(0, u, v)
    }
  }
  // origin + u·u_axis + v·v_axis — exactly mirrors the backend's
  // SketchPlane::lift so frontend ghost geometry agrees with the server.
  return new THREE.Vector3(...plane.origin)
    .add(new THREE.Vector3(...plane.u_axis).multiplyScalar(u))
    .add(new THREE.Vector3(...plane.v_axis).multiplyScalar(v))
}
