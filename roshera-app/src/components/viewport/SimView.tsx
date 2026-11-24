import { useEffect, useMemo } from 'react'
import * as THREE from 'three'
import { useThree } from '@react-three/fiber'
import { Html } from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useSimStore } from '@/stores/sim-store'
import { deformScale, fieldBuffers, finalRun, forceOf, wireSegments } from '@/lib/sim'
import { facePositions } from '@/lib/face-positions'
import { TintMesh } from './face-tint'

/** Held faces: blue, as a support reads in every FEA tool. */
export const SUPPORT_COLOR = '#2F6FD6'
/** Loaded faces and their force arrows. */
export const LOAD_COLOR = '#E07A1F'
/** The mesh's element edges at the mesh stage. */
export const MESH_COLOR = '#1E3A5F'

const UP = new THREE.Vector3(0, 1, 0)

/** A solid force arrow: shaft and head, its tip at `tip`, pointing along
 *  `dir`. Drawn over the part (no depth test) so it is never hidden. */
function Arrow3D({ tip, dir, length }: { tip: THREE.Vector3; dir: THREE.Vector3; length: number }) {
  const head = length * 0.28
  const shaft = length - head
  const r = length * 0.035
  const tail = tip.clone().addScaledVector(dir, -length)
  const q = new THREE.Quaternion().setFromUnitVectors(UP, dir)
  return (
    <group position={tail} quaternion={q} renderOrder={10}>
      <mesh position={[0, shaft / 2, 0]} renderOrder={10}>
        <cylinderGeometry args={[r, r, shaft, 16]} />
        <meshBasicMaterial color={LOAD_COLOR} depthTest={false} transparent opacity={0.95} />
      </mesh>
      <mesh position={[0, shaft + head / 2, 0]} renderOrder={10}>
        <coneGeometry args={[r * 2.6, head, 20]} />
        <meshBasicMaterial color={LOAD_COLOR} depthTest={false} transparent opacity={0.95} />
      </mesh>
    </group>
  )
}

/** A support marker: a small cone pressing into the held face at `at`
 *  (its apex on the face, its body outside along the outward `normal`).
 *  Drawn over the part, so a held face underneath still shows. */
function SupportGlyph({ at, normal, size }: { at: THREE.Vector3; normal: THREE.Vector3; size: number }) {
  const q = new THREE.Quaternion().setFromUnitVectors(UP, normal.clone().negate())
  return (
    <mesh position={at.clone().addScaledVector(normal, size / 2)} quaternion={q} renderOrder={9}>
      <coneGeometry args={[size * 0.35, size, 12]} />
      <meshBasicMaterial color={SUPPORT_COLOR} depthTest={false} transparent opacity={0.9} />
    </mesh>
  )
}

/** Up to `max` points spread over a tinted face set, with their outward
 *  normals (world coordinates): four per triangle, at barycentric points
 *  that cover it evenly, thinned to `max` -- so a large flat face made of
 *  two triangles still gets a row of markers, not two. */
function samplePoints(p: Float32Array, max: number): { at: THREE.Vector3; normal: THREE.Vector3 }[] {
  const weights: [number, number, number][] = [
    [1 / 3, 1 / 3, 1 / 3],
    [2 / 3, 1 / 6, 1 / 6],
    [1 / 6, 2 / 3, 1 / 6],
    [1 / 6, 1 / 6, 2 / 3],
  ]
  const all: { at: THREE.Vector3; normal: THREE.Vector3 }[] = []
  for (let t = 0; t + 8 < p.length; t += 9) {
    const a = new THREE.Vector3(p[t], p[t + 1], p[t + 2])
    const b = new THREE.Vector3(p[t + 3], p[t + 4], p[t + 5])
    const c = new THREE.Vector3(p[t + 6], p[t + 7], p[t + 8])
    const n = b.clone().sub(a).cross(c.clone().sub(a))
    if (n.lengthSq() === 0) continue
    n.normalize()
    for (const [wa, wb, wc] of weights) {
      all.push({
        at: a.clone().multiplyScalar(wa).addScaledVector(b, wb).addScaledVector(c, wc),
        normal: n,
      })
    }
  }
  const step = Math.max(1, Math.ceil(all.length / max))
  return all.filter((_, k) => k % step === 0).slice(0, max)
}/**
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
  // The viewport follows the step: the result on Run, the set-up (held
  // faces, force arrows) on the steps before it.
  const showResult = active && !!report && !!mesh && step === 'run'

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
    if (!active || showResult || !mesh || !faceIds || !world) return { tints: [], arrows: [], glyphs: [], size: 10 }
    const strong = (kind: 'support' | 'load', i: number) => focus?.kind === kind && focus.index === i
    const tints: { key: string; color: string; opacity: number; positions: Float32Array }[] = []
    const size = (() => {
      const g = mesh.geometry
      if (!g.boundingSphere) g.computeBoundingSphere()
      return g.boundingSphere?.radius ?? 10
    })()
    const glyphs: { key: string; at: THREE.Vector3; normal: THREE.Vector3 }[] = []
    setup.supports.forEach((s, i) => {
      const p = facePositions(mesh, faceIds, new Set(s.faces))
      if (!p) return
      tints.push({ key: `s${i}`, color: SUPPORT_COLOR, opacity: strong('support', i) ? 0.85 : 0.5, positions: p })
      samplePoints(p, 12).forEach((g, k) => glyphs.push({ key: `g${i}-${k}`, ...g }))
    })
    const arrows: { key: string; tip: THREE.Vector3; dir: THREE.Vector3; length: number }[] = []
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
      arrows.push({ key: `a${i}`, tip: c, dir, length: size * 0.45 })
    })
    return { tints, arrows, glyphs, size }
  }, [active, showResult, mesh, faceIds, world, setup, focus])

  const result = useMemo(() => {
    if (!showResult || !report) return null
    const scale = deformed ? deformScale(report.field) : 0
    const b = fieldBuffers(report.field, shown, scale)
    const g = new THREE.BufferGeometry()
    g.setAttribute('position', new THREE.BufferAttribute(b.positions, 3))
    g.setAttribute('color', new THREE.BufferAttribute(b.colors, 3))
    g.setIndex(new THREE.BufferAttribute(b.index, 1))
    g.computeVertexNormals()
    g.computeBoundingSphere()
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
        {report && shown === 'stress' && (
          // Where the peak is: the number engineers look for first.
          <group position={finalRun(report).peak_at}>
            <mesh renderOrder={10}>
              <sphereGeometry args={[Math.max(0.4, (result.boundingSphere?.radius ?? 20) * 0.012), 16, 12]} />
              <meshBasicMaterial color="#B91C1C" depthTest={false} />
            </mesh>
            <Html center style={{ pointerEvents: 'none', transform: 'translate(14px, -18px)' }}>
              <div className="whitespace-nowrap rounded border border-red-700/40 bg-white/95 px-1.5 py-0.5 font-mono text-[11px] font-semibold text-red-700 shadow-sm dark:bg-neutral-900/95">
                Peak {finalRun(report).peak_von_mises_mpa.toFixed(1)} MPa
              </div>
            </Html>
          </group>
        )}
      </group>
    )
  }
  return (
    <group>
      {marks.tints.map((t) => (
        <TintMesh key={t.key} positions={t.positions} color={t.color} opacity={t.opacity} />
      ))}
      {marks.arrows.map((a) => (
        <Arrow3D key={a.key} tip={a.tip} dir={a.dir} length={a.length} />
      ))}
      {marks.glyphs.map((g) => (
        <SupportGlyph key={g.key} at={g.at} normal={g.normal} size={marks.size * 0.07} />
      ))}
    </group>
  )
}
