import { useEffect, useRef, useMemo } from 'react'
import { TransformControls } from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'
import { useBlackboardStore } from '@/stores/blackboard-store'
import { useThree } from '@react-three/fiber'
import { Object3D } from 'three'
import { createGizmoSession, type GizmoPose } from '@/lib/gizmo-commit'
import { refreshSceneFromServer } from '@/lib/ws-bridge'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/** How long a committed drag waits for the server's rebroadcast of the part
 *  before reloading the scene snapshot. The rebroadcast is sent before the
 *  HTTP reply, and the bridge flushes within ~50 ms, so this only elapses
 *  when the socket missed the frame. */
const SETTLE_TIMEOUT_MS = 1500
const SETTLE_POLL_MS = 50

/** One session for the app: whether a part's last move has reached the
 *  viewport must survive the gizmo unmounting and remounting. */
const session = createGizmoSession({
  fetchFn: (input, init) => fetch(input, init),
  apiBase: API_BASE,
  // The store drove the Object3D through the drag, so putting the store back
  // puts the part back.
  restore: (objectId, pose) =>
    useSceneStore.getState().updateObject(objectId, {
      position: pose.position,
      rotation: pose.rotation,
      scale: pose.scale,
    }),
  report: (line) => useBlackboardStore.getState().addLine(line, 'system'),
  readMesh: (objectId) => useSceneStore.getState().objects.get(objectId)?.mesh,
  wait: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  resync: refreshSceneFromServer,
  settleTimeoutMs: SETTLE_TIMEOUT_MS,
  pollMs: SETTLE_POLL_MS,
})

/** Re-evaluates whether the mounted gizmo may be dragged; installed by the
 *  mounted gizmo, called when a commit settles. */
let rearmMountedGizmo: (() => void) | null = null

function poseOf(obj: Object3D): GizmoPose {
  return {
    position: [obj.position.x, obj.position.y, obj.position.z],
    quaternion: [obj.quaternion.x, obj.quaternion.y, obj.quaternion.z, obj.quaternion.w],
    rotation: [obj.rotation.x, obj.rotation.y, obj.rotation.z],
    scale: [obj.scale.x, obj.scale.y, obj.scale.z],
  }
}

export function TransformGizmo() {
  const activeTool = useSceneStore((s) => s.activeTool)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const transformSpace = useSceneStore((s) => s.transformSpace)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const objects = useSceneStore((s) => s.objects)
  const updateObject = useSceneStore((s) => s.updateObject)
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const controlsRef = useRef<any>(null)
  const { scene } = useThree()
  // Pose captured when the drag starts — the pose the part returns to if
  // the kernel does not take the move.
  const dragStartRef = useRef<GizmoPose | null>(null)

  const selectedId = selectedIds.size === 1 ? Array.from(selectedIds)[0] : null
  const selectedObj = selectedId ? objects.get(selectedId) : null
  // Sub-element selection modes (face / edge / vertex) own their own
  // per-element gizmos (e.g. `ExtrudeGizmo` for face-pull). Letting the
  // whole-object translate/rotate gizmo render on top of those produces
  // overlapping affordances and stray rotation rings around a face-extrude
  // arrow. Restrict to `object` mode.
  //
  // Assembly components (scene-store ids prefixed `asm-comp:`) have
  // their own gizmo (`AssemblyTransformGizmo`) that commits through
  // `setComponentTransform` REST, so we bail here to keep the two paths
  // from racing.
  const isAssemblyComponent = selectedId?.startsWith('asm-comp:') ?? false
  const showGizmo =
    selectedObj &&
    activeTool !== 'select' &&
    selectionMode === 'object' &&
    !isAssemblyComponent

  const targetMesh = useMemo((): Object3D | null => {
    if (!selectedId) return null
    let found: Object3D | null = null
    scene.traverse((child) => {
      if (child.userData?.cadObjectId === selectedId) {
        found = child
      }
    })
    return found
  }, [selectedId, scene])

  // The controls only exist while the gizmo is mounted; the listeners must
  // be (re)attached when it mounts, not only when the selection changes —
  // otherwise selecting first and switching tool afterwards gives a gizmo
  // that moves the part with no commit behind it.
  const gizmoMounted = Boolean(showGizmo && targetMesh)

  useEffect(() => {
    const controls = controlsRef.current
    if (!controls || !selectedId) return

    // Draggable only when no commit is settling and the viewport holds the
    // kernel's current geometry for this part.
    const rearm = () => {
      controls.enabled = !session.isBusy() && !session.isStale(selectedId)
    }
    rearm()
    rearmMountedGizmo = rearm
    // A stale part unlocks when a later server mesh for it lands.
    const unsubscribe = useSceneStore.subscribe(rearm)

    const handleMouseDown = () => {
      const obj = targetMesh
      dragStartRef.current = obj ? poseOf(obj) : null
    }

    // While dragging, the store follows the gizmo so the viewport renders the
    // proposed pose. It is a proposal: `handleMouseUp` commits it to the
    // kernel or puts the part back.
    const handleChange = () => {
      const obj = targetMesh
      if (!obj || !selectedId) return
      updateObject(selectedId, {
        position: [obj.position.x, obj.position.y, obj.position.z],
        rotation: [obj.rotation.x, obj.rotation.y, obj.rotation.z],
        scale: [obj.scale.x, obj.scale.y, obj.scale.z],
      })
    }

    const handleMouseUp = () => {
      const obj = targetMesh
      const start = dragStartRef.current
      dragStartRef.current = null
      if (!obj || !selectedId || !start) return
      const objectId = selectedId
      const end = poseOf(obj)
      // One commit at a time, and none composed on a mesh the kernel has
      // already moved past: the session holds until the server's rebroadcast
      // of this move has landed (or a snapshot resync delivered it).
      controls.enabled = false
      void session.drag(objectId, start, end).finally(() => {
        rearmMountedGizmo?.()
      })
    }

    controls.addEventListener('mouseDown', handleMouseDown)
    controls.addEventListener('objectChange', handleChange)
    controls.addEventListener('mouseUp', handleMouseUp)
    return () => {
      unsubscribe()
      if (rearmMountedGizmo === rearm) rearmMountedGizmo = null
      controls.removeEventListener('mouseDown', handleMouseDown)
      controls.removeEventListener('objectChange', handleChange)
      controls.removeEventListener('mouseUp', handleMouseUp)
    }
  }, [selectedId, targetMesh, updateObject, gizmoMounted])

  if (!gizmoMounted || !targetMesh || activeTool === 'select') return null

  // 'translate' | 'rotate' here: there is no scale tool, because the kernel
  // transform route has no scale.
  const mode = activeTool

  return (
    <TransformControls
      ref={controlsRef}
      object={targetMesh}
      mode={mode}
      space={transformSpace}
      size={0.7}
    />
  )
}
