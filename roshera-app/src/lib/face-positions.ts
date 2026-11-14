import * as THREE from 'three'

/** How far tint triangles sit off the face, so they paint on it rather than
 *  fight it in the depth buffer (the GD&T tints use the same offset). */
const TINT_OFFSET = 0.002

/** World-space triangles of the given faces, lifted a hair off the surface. */
export function facePositions(mesh: THREE.Mesh, faceIds: ArrayLike<number>, wanted: Set<number>): Float32Array | null {
  if (wanted.size === 0) return null
  const geom = mesh.geometry
  const pos = geom.getAttribute('position') as THREE.BufferAttribute | undefined
  if (!pos) return null
  const index = geom.getIndex()
  mesh.updateWorldMatrix(true, false)
  const world = mesh.matrixWorld
  const a = new THREE.Vector3()
  const b = new THREE.Vector3()
  const c = new THREE.Vector3()
  const n = new THREE.Vector3()
  const ab = new THREE.Vector3()
  const ac = new THREE.Vector3()
  const out: number[] = []
  for (let t = 0; t < faceIds.length; t++) {
    if (!wanted.has(faceIds[t])) continue
    const i0 = t * 3
    const v0 = index ? index.getX(i0) : i0
    const v1 = index ? index.getX(i0 + 1) : i0 + 1
    const v2 = index ? index.getX(i0 + 2) : i0 + 2
    if (v0 >= pos.count || v1 >= pos.count || v2 >= pos.count) continue
    a.fromBufferAttribute(pos, v0).applyMatrix4(world)
    b.fromBufferAttribute(pos, v1).applyMatrix4(world)
    c.fromBufferAttribute(pos, v2).applyMatrix4(world)
    n.crossVectors(ab.subVectors(b, a), ac.subVectors(c, a)).normalize()
    for (const p of [a, b, c]) out.push(p.x + n.x * TINT_OFFSET, p.y + n.y * TINT_OFFSET, p.z + n.z * TINT_OFFSET)
  }
  return out.length > 0 ? new Float32Array(out) : null
}
