import { useEffect, useMemo } from 'react'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useDfmStore } from '@/stores/dfm-store'
import { dfmFaceTints } from '@/lib/dfm'

/** How far tint triangles sit off the face, so they paint on it rather than
 *  fight it in the depth buffer (the GD&T tints use the same offset). */
const TINT_OFFSET = 0.002

const FIX = '#D23B3B'
const UNKNOWN = '#E3A51A'

/**
 * Colours the faces a DFM report names, in the DFM workspace only: red
 * where a rule is proven broken, amber where a check could not run. The
 * finding in focus is drawn at full strength, the rest softer. Faces are
 * matched by kernel face id through the mesh's per-triangle `faceIds`,
 * the same map the GD&T tints use.
 */
export function DfmFaceTints() {
  const mode = useDocModeStore((s) => s.mode)
  const objectId = useDfmStore((s) => s.objectId)
  const report = useDfmStore((s) => s.report)
  const focusRule = useDfmStore((s) => s.focusRule)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)

  const layers = useMemo(() => {
    if (mode !== 'dfm' || !report || !objectId) return []
    const obj = objects.get(objectId)
    const mesh = meshRefs.get(objectId)
    const faceIds = obj?.mesh.faceIds
    if (!faceIds || !mesh?.geometry) return []
    const tints = dfmFaceTints(report)
    const focused = new Set(
      report.verdicts
        .filter((rv) => rv.rule === focusRule)
        .flatMap((rv) =>
          rv.verdict.kind === 'violation'
            ? rv.verdict.witnesses
            : rv.verdict.kind === 'unverifiable'
              ? rv.verdict.regions
              : [],
        ),
    )
    const groups: { color: string; opacity: number; faces: Set<number> }[] = [
      { color: FIX, opacity: 0.55, faces: new Set(tints.fix.filter((f) => !focused.has(f))) },
      { color: UNKNOWN, opacity: 0.4, faces: new Set(tints.unknown.filter((f) => !focused.has(f))) },
      {
        color: tints.fix.some((f) => focused.has(f)) ? FIX : UNKNOWN,
        opacity: 0.9,
        faces: focused,
      },
    ]
    return groups
      .map((g) => ({ ...g, positions: facePositions(mesh, faceIds, g.faces) }))
      .filter((g) => g.positions !== null) as { color: string; opacity: number; positions: Float32Array }[]
  }, [mode, report, objectId, focusRule, meshRefs, objects])

  return (
    <group>
      {layers.map((l, i) => (
        <TintMesh key={`${i}-${l.color}-${l.opacity}`} positions={l.positions} color={l.color} opacity={l.opacity} />
      ))}
    </group>
  )
}

function TintMesh({ positions, color, opacity }: { positions: Float32Array; color: string; opacity: number }) {
  const geometry = useMemo(() => {
    const g = new THREE.BufferGeometry()
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    g.computeVertexNormals()
    return g
  }, [positions])
  useEffect(() => () => geometry.dispose(), [geometry])
  return (
    <mesh geometry={geometry} renderOrder={2}>
      {/* Pulled toward the camera in depth, so the tint wins over the face
          it lies on instead of speckling against it. */}
      <meshBasicMaterial
        color={color}
        transparent
        opacity={opacity}
        side={THREE.DoubleSide}
        depthWrite={false}
        polygonOffset
        polygonOffsetFactor={-2}
        polygonOffsetUnits={-4}
      />
    </mesh>
  )
}

/** World-space triangles of the given faces, lifted a hair off the surface. */
function facePositions(mesh: THREE.Mesh, faceIds: ArrayLike<number>, wanted: Set<number>): Float32Array | null {
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
