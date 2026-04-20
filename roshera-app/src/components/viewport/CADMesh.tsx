import { useRef, useMemo, useCallback, useEffect } from 'react'
import { Edges } from '@react-three/drei'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import * as THREE from 'three'
import type { ThreeEvent } from '@react-three/fiber'

interface CADMeshProps {
  object: CADObject
  isSelected: boolean
  isHovered: boolean
}

const DEFAULT_EDGE_COLOR = '#2a2d3a'

export function CADMesh({ object, isSelected, isHovered }: CADMeshProps) {
  const meshRef = useRef<THREE.Mesh>(null!)
  const selectObject = useSceneStore((s) => s.selectObject)
  const setHovered = useSceneStore((s) => s.setHovered)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const registerMeshRef = useSceneStore((s) => s.registerMeshRef)
  const unregisterMeshRef = useSceneStore((s) => s.unregisterMeshRef)
  const edgeSettings = useSceneStore((s) => s.edgeSettings)

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

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      e.stopPropagation()
      // Object-level selection only — face/edge/vertex selection
      // is a backend concern (send click coords to backend, it resolves topology)
      if (selectionMode === 'object') {
        selectObject(object.id, e.shiftKey)
      } else {
        // For sub-element modes, send the click info to backend
        // which resolves topology and sends back highlight data
        selectObject(object.id, false)
        // TODO: send sub-element pick request to backend with:
        // - object_id, faceIndex (from e.faceIndex), selectionMode
        // Backend returns which faces/edges/vertices to highlight
      }
    },
    [selectObject, object.id, selectionMode],
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

  const handlePointerOut = useCallback(() => {
    setHovered(null)
    document.body.style.cursor = 'default'
  }, [setHovered])

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
      onPointerOver={handlePointerOver}
      onPointerOut={handlePointerOut}
      userData={{ cadObjectId: object.id }}
    >
      {edgeSettings.visible && (
        <Edges
          threshold={edgeSettings.threshold}
          color={isHovered || isSelected ? '#5b9cf5' : (edgeSettings.color || DEFAULT_EDGE_COLOR)}
          lineWidth={isSelected ? edgeSettings.lineWidth * 1.5 : edgeSettings.lineWidth}
        />
      )}
    </mesh>
  )
}
