import { useRef } from 'react'
import { useFrame, useThree } from '@react-three/fiber'
import * as THREE from 'three'

const AXIS_LENGTH = 0.8
const AXIS_COLORS = {
  x: '#e74c3c',
  y: '#2ecc71',
  z: '#3498db',
} as const

/**
 * Origin axis indicator — small colored lines at the world origin
 * showing X (red), Y (green), Z (blue) directions.
 */
export function AxisIndicator() {
  const groupRef = useRef<THREE.Group>(null)
  const { camera } = useThree()

  useFrame(() => {
    if (!groupRef.current) return
    const dist = camera.position.length()
    const s = Math.max(0.5, Math.min(dist * 0.03, 2))
    groupRef.current.scale.setScalar(s)
  })

  return (
    <group ref={groupRef} position={[0, 0.01, 0]}>
      {/* X axis */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            count={2}
            array={new Float32Array([0, 0, 0, AXIS_LENGTH, 0, 0])}
            itemSize={3}
          />
        </bufferGeometry>
        <lineBasicMaterial color={AXIS_COLORS.x} linewidth={2} />
      </line>
      {/* Y axis */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            count={2}
            array={new Float32Array([0, 0, 0, 0, AXIS_LENGTH, 0])}
            itemSize={3}
          />
        </bufferGeometry>
        <lineBasicMaterial color={AXIS_COLORS.y} linewidth={2} />
      </line>
      {/* Z axis */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            count={2}
            array={new Float32Array([0, 0, 0, 0, 0, AXIS_LENGTH])}
            itemSize={3}
          />
        </bufferGeometry>
        <lineBasicMaterial color={AXIS_COLORS.z} linewidth={2} />
      </line>
    </group>
  )
}
