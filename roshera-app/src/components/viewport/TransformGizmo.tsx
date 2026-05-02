import { useEffect, useRef, useCallback, useMemo } from 'react'
import { TransformControls } from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'
import { useThree } from '@react-three/fiber'
import { wsClient } from '@/lib/ws-client'
import { Object3D } from 'three'

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
  const syncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const selectedId = selectedIds.size === 1 ? Array.from(selectedIds)[0] : null
  const selectedObj = selectedId ? objects.get(selectedId) : null
  // Sub-element selection modes (face / edge / vertex) own their own
  // per-element gizmos (e.g. `ExtrudeGizmo` for face-pull). Letting the
  // whole-object translate/rotate/scale gizmo render on top of those
  // produces overlapping affordances and stray rotation rings around a
  // face-extrude arrow. Restrict to `object` mode.
  const showGizmo =
    selectedObj && activeTool !== 'select' && selectionMode === 'object'

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

  const syncToBackend = useCallback((objectId: string) => {
    const obj = targetMesh
    if (!obj) return

    wsClient.send({
      type: 'Command',
      payload: {
        cmd: 'Transform',
        object_id: objectId,
        position: [obj.position.x, obj.position.y, obj.position.z],
        rotation: [obj.rotation.x, obj.rotation.y, obj.rotation.z],
        scale: [obj.scale.x, obj.scale.y, obj.scale.z],
      },
    })
  }, [targetMesh])

  useEffect(() => {
    const controls = controlsRef.current
    if (!controls) return

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
      if (!selectedId) return
      if (syncTimerRef.current) clearTimeout(syncTimerRef.current)
      syncTimerRef.current = setTimeout(() => syncToBackend(selectedId), 100)
    }

    controls.addEventListener('objectChange', handleChange)
    controls.addEventListener('mouseUp', handleMouseUp)
    return () => {
      controls.removeEventListener('objectChange', handleChange)
      controls.removeEventListener('mouseUp', handleMouseUp)
    }
  }, [selectedId, targetMesh, updateObject, syncToBackend])

  useEffect(() => {
    return () => {
      if (syncTimerRef.current) clearTimeout(syncTimerRef.current)
    }
  }, [])

  if (!showGizmo || !targetMesh) return null

  const mode = activeTool as 'translate' | 'rotate' | 'scale'

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
