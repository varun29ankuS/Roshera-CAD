import { useEffect, useMemo, useRef } from 'react'
import { useFrame } from '@react-three/fiber'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useDfmStore } from '@/stores/dfm-store'
import {
  animationFor,
  animationTime,
  sampleActor,
  sampleFaceTrack,
  type Actor,
  type DfmAnimation,
  type FaceState,
} from '@/lib/dfm-animation'
import { facePositions } from '@/lib/face-positions'
import { InSlot } from './overlay-slots'

const FAIL = '#D23B3B'
const OK = '#2E9E5B'
const CONTEXT = '#9AA3AE'
const TOOL = '#C9CED6'
const FLUTE = '#5B8DEF'
const NEEDED = '#E3A51A'

const STATE_COLOR: Record<FaceState, string> = { fail: FAIL, ok: OK, context: CONTEXT }
const Z = new THREE.Vector3(0, 0, 1)

/** Kinds whose face tracks MOVE the part (halves parting, the part
 *  spinning): the static mesh is hidden while they play, so the moving
 *  copy is the only part on screen. */
const MOVES_PART = new Set(['mold_separation', 'spindle_spin'])

/**
 * Plays the kernel's explanation of one DFM finding over the part.
 *
 * Everything drawn comes from the descriptor: tool sizes, positions,
 * directions and timing are the kernel's keyframes (built from the rule's
 * measured value and limit); this component only interpolates them and
 * maps kernel coordinates through the part mesh's world matrix. Shape
 * conventions (kernel `ActorShape` docs): a tool's tip is at its keyed
 * position with its body BEHIND it along −axis; an arrow starts at its
 * position and points along +axis; a plane / caliper / axis is centred.
 */
export function DfmAnimationPlayer() {
  const mode = useDocModeStore((s) => s.mode)
  const objectId = useDfmStore((s) => s.objectId)
  const report = useDfmStore((s) => s.report)
  const playing = useDfmStore((s) => s.playingRule)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)

  const anim = mode === 'dfm' && report && playing ? animationFor(report.animations, playing) : null
  const mesh = objectId ? meshRefs.get(objectId) : undefined
  const faceIds = objectId ? objects.get(objectId)?.mesh.faceIds : undefined

  // Hide the static part while a part-moving animation plays.
  useEffect(() => {
    if (!anim || !mesh || !MOVES_PART.has(anim.kind)) return
    const was = mesh.visible
    mesh.visible = false
    return () => {
      mesh.visible = was
    }
  }, [anim, mesh])

  if (!anim || !mesh) return null
  return <Playback key={`${anim.rule}`} anim={anim} mesh={mesh} faceIds={faceIds} />
}

function Playback({
  anim,
  mesh,
  faceIds,
}: {
  anim: DfmAnimation
  mesh: THREE.Mesh
  faceIds: ArrayLike<number> | undefined
}) {
  const start = useRef<number | null>(null)
  const world = useMemo(() => {
    mesh.updateWorldMatrix(true, false)
    return mesh.matrixWorld.clone()
  }, [mesh])
  const toWorldPoint = (p: [number, number, number]) => new THREE.Vector3(...p).applyMatrix4(world)
  const toWorldDir = (d: [number, number, number]) => {
    const o = new THREE.Vector3(0, 0, 0).applyMatrix4(world)
    return new THREE.Vector3(...d).applyMatrix4(world).sub(o).normalize()
  }

  const tracks = useMemo(
    () =>
      faceIds
        ? anim.face_tracks.map((tr) => ({
            track: tr,
            positions: facePositions(mesh, faceIds, new Set(tr.faces)),
          }))
        : [],
    [anim, mesh, faceIds],
  )
  const trackRefs = useRef<(THREE.Group | null)[]>([])
  const trackMats = useRef<(THREE.MeshBasicMaterial | null)[]>([])
  const actorRefs = useRef<(THREE.Group | null)[]>([])
  const actorMats = useRef<THREE.Material[][]>([])
  const caliperJaws = useRef<Record<number, [THREE.Object3D | null, THREE.Object3D | null]>>({})

  const spin = anim.spin_axis
    ? { p: toWorldPoint(anim.spin_axis[0]), k: toWorldDir(anim.spin_axis[1]) }
    : null

  useFrame((state) => {
    const now = state.clock.getElapsedTime()
    if (start.current === null) start.current = now
    const t = animationTime(anim, now - start.current)

    tracks.forEach(({ track }, i) => {
      const g = trackRefs.current[i]
      const pose = sampleFaceTrack(track, t)
      if (!g || !pose) return
      const off = toWorldDir(pose.offset).multiplyScalar(
        Math.hypot(pose.offset[0], pose.offset[1], pose.offset[2]),
      )
      const m = new THREE.Matrix4().makeTranslation(off.x, off.y, off.z)
      if (spin && pose.rotationDeg !== 0) {
        const r = new THREE.Matrix4().makeRotationAxis(spin.k, THREE.MathUtils.degToRad(pose.rotationDeg))
        const to = new THREE.Matrix4().makeTranslation(spin.p.x, spin.p.y, spin.p.z)
        const from = new THREE.Matrix4().makeTranslation(-spin.p.x, -spin.p.y, -spin.p.z)
        m.multiply(to).multiply(r).multiply(from)
      }
      g.matrix.copy(m)
      const mat = trackMats.current[i]
      if (mat) {
        mat.color.set(STATE_COLOR[pose.state])
        // A failing face pulses so it reads as "this is the problem".
        mat.opacity =
          pose.state === 'fail'
            ? 0.55 + 0.35 * Math.sin(now * 6)
            : MOVES_PART.has(anim.kind)
              ? 0.92
              : 0.0
      }
    })

    anim.actors.forEach((actor, i) => {
      const g = actorRefs.current[i]
      const pose = sampleActor(actor, t)
      if (!g || !pose) return
      g.position.copy(toWorldPoint(pose.position))
      g.quaternion.setFromUnitVectors(Z, toWorldDir(pose.axis))
      for (const m of actorMats.current[i] ?? []) {
        m.opacity = pose.opacity
        m.visible = pose.opacity > 0.01
      }
      if (actor.shape.type === 'caliper') {
        const jaws = caliperJaws.current[i]
        const h = pose.extent / 2
        if (jaws?.[0]) jaws[0].position.set(0, 0, h)
        if (jaws?.[1]) jaws[1].position.set(0, 0, -h)
      }
    })
  })

  const collectMat = (i: number) => (m: THREE.Material | null) => {
    if (!m) return
    const list = (actorMats.current[i] ??= [])
    if (!list.includes(m)) list.push(m)
  }

  return (
    <group>
      {tracks.map(({ positions }, i) =>
        positions ? (
          <group
            key={`tr-${i}`}
            matrixAutoUpdate={false}
            ref={(g) => {
              trackRefs.current[i] = g
            }}
          >
            <TrackTint
              positions={positions}
              onMaterial={(m) => {
                trackMats.current[i] = m
              }}
            />
          </group>
        ) : null,
      )}
      {anim.actors.map((actor, i) => (
        <group
          key={actor.id}
          ref={(g) => {
            actorRefs.current[i] = g
          }}
        >
          <ActorBody
            actor={actor}
            onMaterial={collectMat(i)}
            onJaws={(a, b) => {
              caliperJaws.current[i] = [a, b]
            }}
          />
        </group>
      ))}
    </group>
  )
}

/**
 * The playing finding's caption, as a transparent overlay on the canvas
 * (DOM, outside the Three canvas): every number in it is the kernel's.
 */
export function DfmAnimationCaption() {
  const mode = useDocModeStore((s) => s.mode)
  const report = useDfmStore((s) => s.report)
  const playing = useDfmStore((s) => s.playingRule)
  const play = useDfmStore((s) => s.play)
  const anim = mode === 'dfm' && report && playing ? animationFor(report.animations, playing) : null
  if (!anim) return null
  return (
    <InSlot name="bottom">
      <div className="pointer-events-auto cad-panel flex max-w-[480px] items-center gap-2 rounded border px-2.5 py-1.5 text-[11px]">
        <span className="font-mono text-muted-foreground">{anim.rule}</span>
        <span className="min-w-0 flex-1">{anim.caption}</span>
        <button
          type="button"
          onClick={() => play(null)}
          className="cad-focus rounded px-1.5 py-0.5 hover:bg-accent"
          title="Stop the explanation"
        >
          ✕
        </button>
      </div>
    </InSlot>
  )
}

/** A face group painted with one material the player recolours per frame. */
function TrackTint({
  positions,
  onMaterial,
}: {
  positions: Float32Array
  onMaterial: (m: THREE.MeshBasicMaterial | null) => void
}) {
  const geometry = useMemo(() => {
    const g = new THREE.BufferGeometry()
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    g.computeVertexNormals()
    return g
  }, [positions])
  useEffect(() => () => geometry.dispose(), [geometry])
  return (
    <mesh geometry={geometry} renderOrder={3}>
      <meshBasicMaterial
        ref={onMaterial}
        color={CONTEXT}
        transparent
        opacity={0}
        side={THREE.DoubleSide}
        depthWrite={false}
        polygonOffset
        polygonOffsetFactor={-3}
        polygonOffsetUnits={-6}
      />
    </mesh>
  )
}

/** The actor's geometry in its local frame (+Z = its axis). */
function ActorBody({
  actor,
  onMaterial,
  onJaws,
}: {
  actor: Actor
  onMaterial: (m: THREE.Material | null) => void
  onJaws: (a: THREE.Object3D | null, b: THREE.Object3D | null) => void
}) {
  const jawA = useRef<THREE.Mesh>(null)
  const jawB = useRef<THREE.Mesh>(null)
  useEffect(() => {
    onJaws(jawA.current, jawB.current)
  })
  const color = actor.role === 'needed' ? NEEDED : TOOL
  const mat = (c: string) => (
    <meshStandardMaterial ref={onMaterial} color={c} transparent opacity={0} metalness={0.4} roughness={0.4} />
  )
  // Cylinder geometry is Y-up: rotate it onto local Z.
  const alongZ: [number, number, number] = [Math.PI / 2, 0, 0]
  const s = actor.shape
  switch (s.type) {
    case 'end_mill': {
      const r = s.diameter / 2
      const shank = Math.max(s.length - s.flute_length, 0)
      return (
        <group>
          <mesh rotation={alongZ} position={[0, 0, -s.flute_length / 2]}>
            <cylinderGeometry args={[r, r, s.flute_length, 32]} />
            {mat(FLUTE)}
          </mesh>
          {shank > 0 && (
            <mesh rotation={alongZ} position={[0, 0, -s.flute_length - shank / 2]}>
              <cylinderGeometry args={[r, r, shank, 32]} />
              {mat(color)}
            </mesh>
          )}
        </group>
      )
    }
    case 'drill': {
      const r = s.diameter / 2
      const tip = r / Math.tan(THREE.MathUtils.degToRad(s.point_angle_deg / 2))
      const body = Math.max(s.length - tip, 0)
      return (
        <group>
          {/* Cone apex at the origin (pointing +Z), widening back along −Z. */}
          <mesh rotation={alongZ} position={[0, 0, -tip / 2]}>
            <coneGeometry args={[r, tip, 32]} />
            {mat(FLUTE)}
          </mesh>
          <mesh rotation={alongZ} position={[0, 0, -tip - body / 2]}>
            <cylinderGeometry args={[r, r, body, 32]} />
            {mat(color)}
          </mesh>
        </group>
      )
    }
    case 'arrow': {
      const head = Math.min(s.length * 0.25, 6)
      const shaft = s.length - head
      const w = Math.max(s.length * 0.025, 0.4)
      return (
        <group>
          <mesh rotation={alongZ} position={[0, 0, shaft / 2]}>
            <cylinderGeometry args={[w, w, shaft, 16]} />
            {mat(color)}
          </mesh>
          <mesh rotation={alongZ} position={[0, 0, shaft + head / 2]}>
            <coneGeometry args={[w * 3, head, 16]} />
            {mat(color)}
          </mesh>
        </group>
      )
    }
    case 'plane':
      return (
        <mesh>
          <planeGeometry args={[s.size, s.size]} />
          <meshBasicMaterial ref={onMaterial} color={FLUTE} transparent opacity={0} side={THREE.DoubleSide} depthWrite={false} />
        </mesh>
      )
    case 'caliper': {
      const w = Math.max(s.opening, 4)
      return (
        <group>
          <mesh ref={jawA}>
            <boxGeometry args={[w, w * 0.25, 0.4]} />
            {mat(TOOL)}
          </mesh>
          <mesh ref={jawB}>
            <boxGeometry args={[w, w * 0.25, 0.4]} />
            {mat(TOOL)}
          </mesh>
        </group>
      )
    }
    case 'axis':
      return (
        <mesh rotation={alongZ}>
          <cylinderGeometry args={[0.15, 0.15, s.length, 8]} />
          <meshBasicMaterial ref={onMaterial} color={NEEDED} transparent opacity={0} />
        </mesh>
      )
  }
}
