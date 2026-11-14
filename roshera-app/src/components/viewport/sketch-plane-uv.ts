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

/** Project a world-space `THREE.Vector3` onto the chosen sketch plane. */
export function pointToPlaneUV(
  p: THREE.Vector3,
  plane: SketchPlane,
): [number, number] {
  if (isStandardPlane(plane)) {
    switch (plane) {
      case 'xy': return [p.x, p.y]
      case 'xz': return [p.x, p.z]
      case 'yz': return [p.y, p.z]
    }
  }
  // Custom face-anchored plane: subtract origin then dot against the
  // in-plane basis. Backend's SketchPlane::from_face guarantees u_axis
  // and v_axis are orthonormal, so this gives the same (u, v) the
  // backend's lift() inverts.
  const origin = new THREE.Vector3(...plane.origin)
  const uAxis = new THREE.Vector3(...plane.u_axis)
  const vAxis = new THREE.Vector3(...plane.v_axis)
  const d = p.clone().sub(origin)
  return [d.dot(uAxis), d.dot(vAxis)]
}

/** World position for the capture-plane mesh (origin for custom planes). */
export function planeMeshPosition(plane: SketchPlane): THREE.Vector3 {
  if (isStandardPlane(plane)) return new THREE.Vector3(0, 0, 0)
  return new THREE.Vector3(...plane.origin)
}

/**
 * Quaternion that orients the default `<planeGeometry>` (whose local
 * +X is right, +Y is up, +Z is normal) to lie on the sketch plane.
 * For standard planes we replay the same Euler rotations the previous
 * implementation used so the visual result is unchanged. For custom
 * planes we build an orthonormal basis directly from u_axis / v_axis
 * and extract the quaternion from the basis matrix.
 */
export function planeMeshQuaternion(plane: SketchPlane): THREE.Quaternion {
  const q = new THREE.Quaternion()
  if (isStandardPlane(plane)) {
    const euler = (() => {
      switch (plane) {
        case 'xy': return new THREE.Euler(0, 0, 0)
        case 'xz': return new THREE.Euler(-Math.PI / 2, 0, 0)
        case 'yz': return new THREE.Euler(0, Math.PI / 2, 0)
      }
    })()
    q.setFromEuler(euler)
    return q
  }
  const uAxis = new THREE.Vector3(...plane.u_axis)
  const vAxis = new THREE.Vector3(...plane.v_axis)
  const nAxis = new THREE.Vector3().crossVectors(uAxis, vAxis)
  const m = new THREE.Matrix4().makeBasis(uAxis, vAxis, nAxis)
  q.setFromRotationMatrix(m)
  return q
}
