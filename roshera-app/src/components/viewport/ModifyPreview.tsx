/**
 * Live cross-section preview for the fillet / chamfer modify dialog.
 *
 * While `scene.modifyPreview` is non-null and edges are picked on the
 * selected solid, this component renders one indicator at the midpoint
 * of each picked edge:
 *
 *   - Fillet : a circle of radius `value` in the plane perpendicular
 *     to the edge tangent. Visualises the fillet cross-section so the
 *     user can see how big the rounded corner will be relative to
 *     the part.
 *
 *   - Chamfer : a square of side `2 × value` in the same plane,
 *     rotated 45° (a diamond) — visually distinct from fillet so the
 *     user can tell at a glance which mode they're in.
 *
 * No backend round-trip; this is purely a frontend visualisation that
 * couples the dialog's numeric input to a geometric scale. A real
 * "ghost mesh" preview (running the kernel and displaying the result)
 * is a follow-up — see Task #85 description.
 */

import { useMemo } from 'react'
import { Line } from '@react-three/drei'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'

const FILLET_COLOR = '#22d3ee' // cyan-400
const CHAMFER_COLOR = '#f59e0b' // amber-500
const SEGMENTS = 48

export function ModifyPreview() {
  const modifyPreview = useSceneStore((s) => s.modifyPreview)
  const subElementSelections = useSceneStore((s) => s.subElementSelections)
  const selectedIds = useSceneStore((s) => s.selectedIds)

  const indicators = useMemo(() => {
    if (!modifyPreview) return []
    if (!Number.isFinite(modifyPreview.value) || modifyPreview.value <= 0) return []
    const ids = Array.from(selectedIds)
    if (ids.length !== 1) return []
    const [object] = ids
    const value = modifyPreview.value

    const out: { points: THREE.Vector3[]; color: string }[] = []

    for (const sel of subElementSelections) {
      if (sel.type !== 'edge') continue
      if (sel.objectId !== object) continue
      const flat = sel.polyline
      if (!flat || flat.length < 6) continue

      // Sample three points around the polyline mid-index to compute
      // a stable tangent. Using `n/2 - 1` and `n/2 + 1` averages out
      // local discretisation noise for short straight edges.
      const nVerts = Math.floor(flat.length / 3)
      const mid = Math.floor(nVerts / 2)
      const before = Math.max(0, mid - 1)
      const after = Math.min(nVerts - 1, mid + 1)

      const m = new THREE.Vector3(flat[mid * 3], flat[mid * 3 + 1], flat[mid * 3 + 2])
      const a = new THREE.Vector3(flat[before * 3], flat[before * 3 + 1], flat[before * 3 + 2])
      const b = new THREE.Vector3(flat[after * 3], flat[after * 3 + 1], flat[after * 3 + 2])
      const tangent = new THREE.Vector3().subVectors(b, a)
      if (tangent.lengthSq() < 1e-12) continue
      tangent.normalize()

      // Build an orthonormal basis (u, v) in the plane perpendicular
      // to the tangent. Reference axis: world +Y unless the tangent is
      // near-parallel to it, then world +X.
      const refAxis =
        Math.abs(tangent.y) < 0.9
          ? new THREE.Vector3(0, 1, 0)
          : new THREE.Vector3(1, 0, 0)
      const u = new THREE.Vector3().crossVectors(refAxis, tangent).normalize()
      const v = new THREE.Vector3().crossVectors(tangent, u).normalize()

      if (modifyPreview.mode === 'fillet') {
        const points: THREE.Vector3[] = []
        for (let i = 0; i <= SEGMENTS; i++) {
          const theta = (i / SEGMENTS) * Math.PI * 2
          const cu = Math.cos(theta) * value
          const cv = Math.sin(theta) * value
          points.push(
            new THREE.Vector3(
              m.x + u.x * cu + v.x * cv,
              m.y + u.y * cu + v.y * cv,
              m.z + u.z * cu + v.z * cv,
            ),
          )
        }
        out.push({ points, color: FILLET_COLOR })
      } else {
        // Chamfer: diamond with vertices at ±value along u and v.
        const pu = new THREE.Vector3().copy(u).multiplyScalar(value)
        const pv = new THREE.Vector3().copy(v).multiplyScalar(value)
        const points = [
          new THREE.Vector3().addVectors(m, pu),
          new THREE.Vector3().addVectors(m, pv),
          new THREE.Vector3().subVectors(m, pu),
          new THREE.Vector3().subVectors(m, pv),
          new THREE.Vector3().addVectors(m, pu),
        ]
        out.push({ points, color: CHAMFER_COLOR })
      }
    }

    return out
  }, [modifyPreview, subElementSelections, selectedIds])

  if (indicators.length === 0) return null

  return (
    <group>
      {indicators.map((ind, i) => (
        <Line
          // The indicator set rebuilds whenever the radius / edge
          // selection changes, so a stable index key is fine here.
          key={i}
          points={ind.points}
          color={ind.color}
          lineWidth={2}
          depthTest={false}
          transparent
          opacity={0.9}
        />
      ))}
    </group>
  )
}
