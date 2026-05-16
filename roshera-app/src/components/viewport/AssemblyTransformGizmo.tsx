/**
 * AssemblyTransformGizmo — Three.js TransformControls bound to the
 * currently-selected assembly component (scene-store id prefixed
 * `asm-comp:`). Translate by default (W); rotate on E; Q toggles
 * world/local space. Drag-end commits the new pose to the kernel via
 * `PATCH /api/assemblies/:aid/components/:cid/transform`, then routes
 * the returned snapshot through `useAssemblyStore.setSnapshot` so the
 * sidebar's inline editor re-seeds to the committed numbers.
 *
 * Why a separate gizmo? The shared `TransformGizmo` commits via the
 * WebSocket `Transform` command which is part-mode plumbing — it does
 * not know about assemblies. Bailing that gizmo for `asm-comp:` ids
 * and giving the assembly path its own commit lane keeps both flows
 * isolated and easy to reason about.
 *
 * Bail conditions (renders null):
 *   - doc mode is not 'assembly'
 *   - selection is not exactly one object
 *   - the selected id does not carry the `asm-comp:` prefix
 *   - the matching component is marked `is_fixed` (kernel would reject
 *     the transform anyway — surface the deadness as missing gizmo)
 *   - no assembly snapshot loaded yet
 *
 * Scale is intentionally excluded: assembly components are placement
 * transforms, not geometry edits. Scaling a component would rebuild
 * its mesh on the kernel side and is a separate feature.
 */

import { useEffect, useMemo, useRef, useState } from 'react'
import { TransformControls } from '@react-three/drei'
import { useThree } from '@react-three/fiber'
import { type Object3D } from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useAssemblyStore } from '@/stores/assembly-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { composeRowMajor, setComponentTransform } from '@/lib/assembly-api'

const ASM_OBJ_PREFIX = 'asm-comp:'

type GizmoMode = 'translate' | 'rotate'

export function AssemblyTransformGizmo() {
  const docMode = useDocModeStore((s) => s.mode)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const updateObject = useSceneStore((s) => s.updateObject)
  const transformSpace = useSceneStore((s) => s.transformSpace)
  const active = useAssemblyStore((s) => s.active)
  const setSnapshot = useAssemblyStore((s) => s.setSnapshot)
  const setError = useAssemblyStore((s) => s.setError)
  const { scene } = useThree()

  const [mode, setMode] = useState<GizmoMode>('translate')
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const controlsRef = useRef<any>(null)

  const selectedId = selectedIds.size === 1 ? Array.from(selectedIds)[0] : null
  const isAssemblyComponent = selectedId?.startsWith(ASM_OBJ_PREFIX) ?? false
  const componentId =
    isAssemblyComponent && selectedId
      ? selectedId.slice(ASM_OBJ_PREFIX.length)
      : null
  const component = useMemo(() => {
    if (!componentId || !active) return null
    return active.components.find((c) => c.id === componentId) ?? null
  }, [componentId, active])

  const showGizmo =
    docMode === 'assembly' &&
    !!active &&
    !!component &&
    !component.is_fixed

  // Locate the live three.js Object3D our `SceneObjects` renderer
  // tagged with `userData.cadObjectId` so we can hand it to the
  // TransformControls helper. The lookup is recomputed when the
  // selection changes, but Three only stamps userData on mount, so
  // the traversal is cheap (we expect a handful of objects).
  const targetMesh = useMemo((): Object3D | null => {
    if (!selectedId || !showGizmo) return null
    let found: Object3D | null = null
    scene.traverse((child) => {
      if (child.userData?.cadObjectId === selectedId) {
        found = child
      }
    })
    return found
  }, [selectedId, scene, showGizmo])

  // W/E mode toggle — only while the gizmo is active so we don't
  // intercept the same keys elsewhere in the app.
  useEffect(() => {
    if (!showGizmo) return
    const onKey = (e: KeyboardEvent) => {
      const tgt = e.target as HTMLElement | null
      if (
        tgt &&
        (tgt.tagName === 'INPUT' ||
          tgt.tagName === 'TEXTAREA' ||
          tgt.isContentEditable)
      ) {
        return
      }
      if (e.key === 'w' || e.key === 'W') {
        e.preventDefault()
        setMode('translate')
      } else if (e.key === 'e' || e.key === 'E') {
        e.preventDefault()
        setMode('rotate')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [showGizmo])

  // Live in-store mirror during the drag (so any inline UI bound to
  // the scene object stays consistent) + REST commit on release.
  useEffect(() => {
    const controls = controlsRef.current
    if (!controls || !showGizmo || !targetMesh || !active || !componentId) {
      return
    }

    const handleChange = () => {
      const obj = targetMesh
      if (!obj || !selectedId) return
      updateObject(selectedId, {
        position: [obj.position.x, obj.position.y, obj.position.z],
        rotation: [obj.rotation.x, obj.rotation.y, obj.rotation.z],
        scale: [obj.scale.x, obj.scale.y, obj.scale.z],
      })
    }

    const handleMouseUp = async () => {
      const obj = targetMesh
      if (!obj) return
      const matrix = composeRowMajor(
        [obj.position.x, obj.position.y, obj.position.z],
        [obj.quaternion.x, obj.quaternion.y, obj.quaternion.z, obj.quaternion.w],
        [obj.scale.x, obj.scale.y, obj.scale.z],
      )
      try {
        const snap = await setComponentTransform(active.id, componentId, matrix)
        setSnapshot(snap)
      } catch (e) {
        // Surface to the assembly error banner. The mesh-sync effect
        // in AssemblyWorkspace will re-snap the object to the last
        // good snapshot on the next render, so we don't need to
        // explicitly revert here.
        setError(e instanceof Error ? e.message : String(e))
      }
    }

    controls.addEventListener('objectChange', handleChange)
    controls.addEventListener('mouseUp', handleMouseUp)
    return () => {
      controls.removeEventListener('objectChange', handleChange)
      controls.removeEventListener('mouseUp', handleMouseUp)
    }
  }, [
    selectedId,
    targetMesh,
    active,
    componentId,
    showGizmo,
    updateObject,
    setSnapshot,
    setError,
  ])

  if (!showGizmo || !targetMesh) return null

  return (
    <TransformControls
      ref={controlsRef}
      object={targetMesh}
      mode={mode}
      space={transformSpace}
      size={0.9}
    />
  )
}
