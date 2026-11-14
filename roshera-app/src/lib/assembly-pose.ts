import * as THREE from 'three'

/** An instance's 4×4 row-major world transform as the viewport's position,
 *  XYZ Euler rotation and scale. */
export function poseOf(m: number[][]): {
  position: [number, number, number]
  rotation: [number, number, number]
  scale: [number, number, number]
} {
  const mat = new THREE.Matrix4().set(
    m[0][0], m[0][1], m[0][2], m[0][3],
    m[1][0], m[1][1], m[1][2], m[1][3],
    m[2][0], m[2][1], m[2][2], m[2][3],
    m[3][0], m[3][1], m[3][2], m[3][3],
  )
  const p = new THREE.Vector3()
  const q = new THREE.Quaternion()
  const s = new THREE.Vector3()
  mat.decompose(p, q, s)
  const e = new THREE.Euler().setFromQuaternion(q, 'XYZ')
  return { position: [p.x, p.y, p.z], rotation: [e.x, e.y, e.z], scale: [s.x, s.y, s.z] }
}

/** The scene id an instance is drawn under (never a part's own id). */
export const INSTANCE_PREFIX = 'asmi:'
