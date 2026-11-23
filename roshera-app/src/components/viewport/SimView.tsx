import { useEffect, useMemo } from 'react'
import * as THREE from 'three'
import { useThree } from '@react-three/fiber'
import { useSceneStore } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useSimStore } from '@/stores/sim-store'
import { deformScale, fieldBuffers, forceOf, wireSegments } from '@/lib/sim'
import { facePositions } from '@/lib/face-positions'
import { TintMesh } from './face-tint'

/** Held faces: blue, as a support reads in every FEA tool. */
export const SUPPORT_COLOR = '#2F6FD6'
/** Loaded faces and their force arrows. */
export const LOAD_COLOR = '#E07A1F'
/** The mesh's element edges at the mesh stage. */
export const MESH_COLOR = '#1E3A5F'

/**
 * The Simulate workspace on the part. Before a run: the held faces tinted
 * blue, the loaded faces orange with an arrow along each force, the row in
 * focus strongest. After a run: the part is replaced by the kernel's result
 * surface, coloured by stress or displacement and drawn in its (exaggerated)
 * displaced shape. Kernel coordinates map through the part mesh's world
 * matrix, as the DFM overlays do.
 */
export function SimView() {
  const mode = useDocModeStore((s) => s.mode)
  const objectId = useSimStore((s) => s.objectId)
  const setup = useSimStore((s) => s.setup)
  const report = useSimStore((s) => s.report)
  const shown = useSimStore((s) => s.shown)
  const deformed = useSimStore((s) => s.deformed)
  const focus = useSimStore((s) => s.focus)
  const step = useSimStore((s) => s.step)
  const wire = useSimStore((s) => s.wire)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)
  const invalidate = useThree((s) => s.invalidate)

  const active = mode === 'simulate' && !!objectId
  const mesh = active && objectId ? meshRefs.get(objectId) : undefined
  const faceIds = active && objectId ? objects.get(objectId)?.mesh.faceIds : undefined
  const showResult = active && !!report && !!mesh

  // The result surface stands in for the part while it is shown.
  useEffect(() => {
    if (!showResult || !mesh) return
    const was = mesh.visible
    // The part's three.js mesh belongs to the viewport scene, not to React
    // state: hiding it while the result skin stands in is a scene change,
    // undone on cleanup (the DFM animation player does the same).
    mesh.visible = false
    invalidate()
    return () => {
      mesh.visible = was
      invalidate()
    }
  }, [showResult, mesh, invalidate])

  const world = useMemo(() => {
    if (!mesh) return null
    mesh.updateWorldMatrix(true, false)
    return mesh.matrixWorld.clone()
    // `objects` changes when the part moves: the world matrix is re-read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mesh, objects])

  const marks = useMemo(() => {
    if (!active || report || !mesh || !faceIds || !world) return { tints: [], arrows: [] }
    const strong = (kind: 'support' | 'load', i: number) => focus?.kind === kind && focus.index === i
    const tints: { key: string; color: string; opacity: number; positions: Float32Array }[] = []
    setup.supports.forEach((s, i) => {
      const p = facePositions(mesh, faceIds, new Set(s.faces))
      if (p) tints.push({ key: `s${i}`, color: SUPPORT_COLOR, opacity: strong('support', i) ? 0.85 : 0.5, positions: p })
    })
    const arrows: { key: string; object: THREE.ArrowHelper }[] = []
    const size = (() => {
      const g = mesh.geometry
      if (!g.boundingSphere) g.computeBoundingSphere()
      return g.boundingSphere?.radius ?? 10
    })()
    setup.loads.forEach((l, i) => {
      const p = facePositions(mesh, faceIds, new Set(l.faces))
      if (!p) return
      tints.push({ key: `l${i}`, color: LOAD_COLOR, opacity: strong('load', i) ? 0.85 : 0.5, positions: p })
      const f = forceOf(l)
      if (!f) return
      // Arrow tip on the faces' centre, shaft behind it along the force.
      const c = new THREE.Vector3()
      for (let k = 0; k < p.length; k += 3) c.add(new THREE.Vector3(p[k], p[k + 1], p[k + 2]))
      c.divideScalar(p.length / 3)
      const o = new THREE.Vector3(0, 0, 0).applyMatrix4(world)
      const dir = new THREE.Vector3(...f).applyMatrix4(world).sub(o).normalize()
      const length = size * 0.45
      const origin = c.clone().addScaledVector(dir, -length)
      arrows.push({ key: `a${i}`, object: new THREE.ArrowHelper(dir, origin, length, LOAD_COLOR, length * 0.28, length * 0.16) })
    })
    return { tints, arrows }
  }, [active, report, mesh, faceIds, world, setup, focus])
  useEffect(() => () => marks.arrows.forEach((a) => a.object.dispose()), [marks])

  const result = useMemo(() => {
    if (!showResult || !report) return null
    const scale = deformed ? deformScale(report.field) : 0
    const b = fieldBuffers(report.field, shown, scale)
    const g = new THREE.BufferGeometry()
    g.setAttribute('position', new THREE.BufferAttribute(b.positions, 3))
    g.setAttribute('color', new THREE.BufferAttribute(b.colors, 3))
    g.setIndex(new THREE.BufferAttribute(b.index, 1))
    g.computeVertexNormals()
    return g
  }, [showResult, report, shown, deformed])
  useEffect(() => () => result?.dispose(), [result])

  // The mesh stage: the proving mesh's element edges over the part.
  const wireGeometry = useMemo(() => {
    if (!active || step !== 'mesh' || !wire) return null
    const g = new THREE.BufferGeometry()
    g.setAttribute('position', new THREE.BufferAttribute(wireSegments(wire), 3))
    return g
  }, [active, step, wire])
  useEffect(() => () => wireGeometry?.dispose(), [wireGeometry])

  if (!active) return null
  if (wireGeometry && world) {
    return (
      <group matrixAutoUpdate={false} matrix={world}>
        <lineSegments geometry={wireGeometry} renderOrder={3}>
          <lineBasicMaterial color={MESH_COLOR} transparent opacity={0.75} depthTest />
        </lineSegments>
      </group>
    )
  }
  if (result && world) {
    return (
      <group matrixAutoUpdate={false} matrix={world}>
        <mesh geometry={result}>
          <meshLambertMaterial vertexColors side={THREE.FrontSide} />
        </mesh>
      </group>
    )
  }
  return (
    <group>
      {marks.tints.map((t) => (
        <TintMesh key={t.key} positions={t.positions} color={t.color} opacity={t.opacity} />
      ))}
      {marks.arrows.map((a) => (
        <primitive key={a.key} object={a.object} />
      ))}
    </group>
  )
}
