import { useEffect, useRef, useCallback } from 'react'
import { TransformControls } from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'
import { useThree } from '@react-three/fiber'
import { wsClient } from '@/lib/ws-client'
import type * as THREE from 'three'

export function TransformGizmo() {
  const activeTool = useSceneStore((s) => s.activeTool)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const transformSpace = useSceneStore((s) => s.transformSpace)
  const objects = useSceneStore((s) => s.objects)
  const updateObject = useSceneStore((s) => s.updateObject)
  const controlsRef = useRef<any>(null)
  const { scene } = useThree()
  const syncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const selectedId = selectedIds.size === 1 ? Array.from(selectedIds)[0] : null
  const selectedObj = selectedId ? objects.get(selectedId) : null
  const showGizmo = selectedObj && activeTool !== 'select'

  const targetMesh = useRef<THREE.Object3D | null>(null)

  useEffect(() => {
    if (!selectedId) {
      targetMesh.current = null
      return
    }

    scene.traverse((child) => {
      if (child.userData?.cadObjectId === selectedId) {
        targetMesh.current = child
      }
    })
  }, [selectedId, scene])

  const syncToBackend = useCallback((objectId: string) => {
    const obj = targetMesh.current
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
  }, [])

  useEffect(() => {
    const controls = controlsRef.current
    if (!controls) return

    const handleChange = () => {
      if (!targetMesh.current || !selectedId) return
      const obj = targetMesh.current
      updateObject(selectedId, {
        position: [obj.position.x, obj.position.y, obj.position.z],
        rotation: [obj.rotation.x, obj.rotation.y, obj.rotation.z],
        scale: [obj.scale.x, obj.scale.y, obj.scale.z],
      })
    }

    const handleMouseUp = () => {
      if (!selectedId) return
      // Debounce backend sync — send after gizmo interaction ends
      if (syncTimerRef.current) clearTimeout(syncTimerRef.current)
      syncTimerRef.current = setTimeout(() => syncToBackend(selectedId), 100)
    }

    controls.addEventListener('objectChange', handleChange)
    controls.addEventListener('mouseUp', handleMouseUp)
    return () => {
      controls.removeEventListener('objectChange', handleChange)
      controls.removeEventListener('mouseUp', handleMouseUp)
    }
  }, [selectedId, updateObject, syncToBackend])

  useEffect(() => {
    return () => {
      if (syncTimerRef.current) clearTimeout(syncTimerRef.current)
    }
  }, [])

  if (!showGizmo || !targetMesh.current) return null

  const mode = activeTool as 'translate' | 'rotate' | 'scale'

  return (
    <TransformControls
      ref={controlsRef}
      object={targetMesh.current}
      mode={mode}
      space={transformSpace}
      size={0.7}
    />
  )
}
