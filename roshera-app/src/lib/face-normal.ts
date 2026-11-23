import type * as THREE from 'three'
import type { Vec3 } from './sim'

/**
 * The outward normal of some of a part's kernel faces, area-weighted over
 * their triangles, in the MESH's own coordinates -- the kernel's model
 * axes, which a study's forces are given in. Null when the faces have no
 * area or their normals cancel (a whole cylinder wall, say).
 */
export function faceNormal(mesh: THREE.Mesh, faceIds: ArrayLike<number>, wanted: Set<number>): Vec3 | null {
  const geom = mesh.geometry
  const pos = geom.getAttribute('position') as THREE.BufferAttribute | undefined
  if (!pos || wanted.size === 0) return null
  const index = geom.getIndex()
  let nx = 0
  let ny = 0
  let nz = 0
  let area = 0
  for (let t = 0; t < faceIds.length; t++) {
    if (!wanted.has(faceIds[t])) continue
    const i0 = t * 3
    const v = [0, 1, 2].map((k) => (index ? index.getX(i0 + k) : i0 + k))
    if (v.some((x) => x >= pos.count)) continue
    const [a, b, c] = v.map((x) => [pos.getX(x), pos.getY(x), pos.getZ(x)])
    const u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]]
    const w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]]
    const cx = u[1] * w[2] - u[2] * w[1]
    const cy = u[2] * w[0] - u[0] * w[2]
    const cz = u[0] * w[1] - u[1] * w[0]
    nx += cx
    ny += cy
    nz += cz
    area += Math.hypot(cx, cy, cz)
  }
  const len = Math.hypot(nx, ny, nz)
  // Normals that mostly cancel give no one direction to push along.
  if (!(area > 0) || len < 0.5 * area) return null
  return [nx / len, ny / len, nz / len]
}
