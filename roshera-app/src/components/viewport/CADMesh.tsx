import { useRef, useMemo, useCallback, useEffect } from 'react'
import { Edges } from '@react-three/drei'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'
import { wsClient } from '@/lib/ws-client'
import * as THREE from 'three'
import type { ThreeEvent } from '@react-three/fiber'

interface CADMeshProps {
  object: CADObject
  isSelected: boolean
  isHovered: boolean
}

export function CADMesh({ object, isSelected, isHovered }: CADMeshProps) {
  const meshRef = useRef<THREE.Mesh>(null!)
  const selectObject = useSceneStore((s) => s.selectObject)
  const setHovered = useSceneStore((s) => s.setHovered)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const registerMeshRef = useSceneStore((s) => s.registerMeshRef)
  const unregisterMeshRef = useSceneStore((s) => s.unregisterMeshRef)
  const edgeSettings = useSceneStore((s) => s.edgeSettings)
  const theme = useThemeStore((s) => s.theme)

  const { defaultEdgeHex, accentEdgeHex } = useMemo(() => {
    const tick = resolveCssVar('--cad-tick')
    const accent = resolveCssVar('--cad-selected')
    return {
      defaultEdgeHex: `#${tick.color.getHexString()}`,
      accentEdgeHex: `#${accent.color.getHexString()}`,
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme])

  // Register mesh ref for outline post-processing
  useEffect(() => {
    if (meshRef.current) {
      registerMeshRef(object.id, meshRef.current)
    }
    return () => unregisterMeshRef(object.id)
  }, [object.id, registerMeshRef, unregisterMeshRef])

  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry()
    geom.setAttribute(
      'position',
      new THREE.BufferAttribute(object.mesh.vertices, 3),
    )
    if (object.mesh.indices.length > 0) {
      geom.setIndex(new THREE.BufferAttribute(object.mesh.indices, 1))
    }
    if (object.mesh.normals.length > 0) {
      geom.setAttribute(
        'normal',
        new THREE.BufferAttribute(object.mesh.normals, 3),
      )
    } else {
      geom.computeVertexNormals()
    }
    geom.computeBoundingBox()
    geom.computeBoundingSphere()
    return geom
  }, [object.mesh])

  // Material — no color swap for selection (outline post-processing handles that)
  const material = useMemo(() => {
    return new THREE.MeshStandardMaterial({
      color: object.material.color,
      metalness: object.material.metalness,
      roughness: object.material.roughness,
      transparent: object.material.opacity < 1,
      opacity: object.material.opacity,
      side: THREE.DoubleSide,
    })
  }, [object.material])

  const toggleSubElementSelection = useSceneStore((s) => s.toggleSubElementSelection)

  // Resolve a Three.js raycast triangle index to the kernel `FaceId`. The
  // backend tessellator stores one FaceId per triangle in `mesh.faceIds`
  // (length = indices/3); when present we use it directly. When absent
  // (legacy frame, or merged client-side mesh) we fall back to the raw
  // triangle index — backend topology lookup will reject anything stale.
  const resolveFaceId = useCallback(
    (triangleIndex: number): number => {
      const map = object.mesh.faceIds
      if (map && triangleIndex < map.length) {
        return map[triangleIndex]
      }
      return triangleIndex
    },
    [object.mesh.faceIds],
  )

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      e.stopPropagation()

      if (selectionMode === 'object') {
        selectObject(object.id, e.shiftKey)
        return
      }

      // Sub-element picking: triangle index from the raycast → kernel
      // FaceId via the per-triangle face_map shipped on the mesh. Edge
      // and vertex modes still fall back to the raw triangle index for
      // now — the kernel doesn't ship per-triangle edge/vertex maps yet,
      // so the backend resolves those from the picked point.
      const triangleIndex = e.faceIndex ?? 0
      const point = e.point.toArray() as [number, number, number]
      const subType = selectionMode as 'face' | 'edge' | 'vertex'
      const elementIndex = subType === 'face' ? resolveFaceId(triangleIndex) : triangleIndex

      // Optimistic local selection so the UI feels instant
      toggleSubElementSelection({
        objectId: object.id,
        type: subType,
        index: elementIndex,
      })

      // Send pick request to backend for authoritative topology resolution.
      // Backend responds with a SubElementResult message handled by ws-bridge.
      wsClient.send({
        type: 'Query',
        data: {
          query_type: {
            SubElementPick: {
              object_id: object.id,
              face_index: elementIndex,
              triangle_index: triangleIndex,
              point,
              mode: selectionMode,
            },
          },
        },
        request_id: `pick-${object.id}-${elementIndex}-${Date.now()}`,
      })
    },
    [selectObject, toggleSubElementSelection, resolveFaceId, object.id, selectionMode],
  )

  const setHoveredSubElement = useSceneStore((s) => s.setHoveredSubElement)
  const openContextMenu = useSceneStore((s) => s.openContextMenu)

  const handleContextMenu = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      e.stopPropagation()
      // Three.js gives us a synthetic event — pull screen coords from the
      // underlying native event so the menu lands at the cursor.
      const native = e.nativeEvent
      native.preventDefault?.()
      // Right-click implicitly selects the object so subsequent menu
      // actions (delete, hide) act on the visible target.
      selectObject(object.id, false)
      openContextMenu({
        x: native.clientX,
        y: native.clientY,
        objectId: object.id,
      })
    },
    [object.id, selectObject, openContextMenu],
  )

  const handlePointerOver = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      e.stopPropagation()
      setHovered(object.id)
      document.body.style.cursor = selectionMode === 'face' ? 'cell'
        : selectionMode === 'edge' || selectionMode === 'vertex' ? 'crosshair'
        : 'pointer'
    },
    [setHovered, object.id, selectionMode],
  )

  const handlePointerMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (selectionMode === 'object') return
      e.stopPropagation()
      const faceIndex = e.faceIndex ?? 0
      const subType = selectionMode as 'face' | 'edge' | 'vertex'
      setHoveredSubElement({
        objectId: object.id,
        type: subType,
        index: faceIndex,
      })
    },
    [selectionMode, object.id, setHoveredSubElement],
  )

  const handlePointerOut = useCallback(() => {
    setHovered(null)
    setHoveredSubElement(null)
    document.body.style.cursor = 'default'
  }, [setHovered, setHoveredSubElement])

  return (
    <mesh
      ref={meshRef}
      geometry={geometry}
      material={material}
      position={object.position}
      rotation={object.rotation}
      scale={object.scale}
      castShadow
      receiveShadow
      onClick={handleClick}
      onContextMenu={handleContextMenu}
      onPointerOver={handlePointerOver}
      onPointerMove={handlePointerMove}
      onPointerOut={handlePointerOut}
      userData={{ cadObjectId: object.id }}
    >
      {edgeSettings.visible && (
        <Edges
          threshold={edgeSettings.threshold}
          color={isHovered ? accentEdgeHex : (edgeSettings.color || defaultEdgeHex)}
          lineWidth={isSelected ? edgeSettings.lineWidth * 1.5 : edgeSettings.lineWidth}
        />
      )}
    </mesh>
  )
}
