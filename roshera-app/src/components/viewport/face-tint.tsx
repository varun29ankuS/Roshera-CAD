import { useEffect, useMemo } from 'react'
import * as THREE from 'three'

/**
 * Painting some of a part's kernel faces: the triangles whose per-triangle
 * face id (`mesh.faceIds`) is wanted, lifted a hair off the surface, drawn
 * as a translucent overlay. Shared by the DFM tints and the selected
 * feature's highlight.
 */
export function TintMesh({ positions, color, opacity }: { positions: Float32Array; color: string; opacity: number }) {
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
