/**
 * World-origin axis lines on the XZ ground plane plus the Y vertical.
 * Uses the conventional X=red / Y=green / Z=blue encoding so the
 * orientation matches the gizmo widget and standard CAD muscle memory.
 * Drawn slightly above the grid plane to avoid z-fighting.
 */
export function ReferencePlanes() {
  const Y = 0
  const LEN = 100

  return (
    <group name="reference-planes">
      {/* X axis (red) */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            args={[new Float32Array([-LEN, Y, 0, LEN, Y, 0]), 3]}
          />
        </bufferGeometry>
        <lineBasicMaterial color="#e74c3c" transparent opacity={0.45} />
      </line>

      {/* Y axis (green) */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            args={[new Float32Array([0, 0, 0, 0, LEN, 0]), 3]}
          />
        </bufferGeometry>
        <lineBasicMaterial color="#2ecc71" transparent opacity={0.45} />
      </line>

      {/* Z axis (blue) */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            args={[new Float32Array([0, Y, -LEN, 0, Y, LEN]), 3]}
          />
        </bufferGeometry>
        <lineBasicMaterial color="#3498db" transparent opacity={0.45} />
      </line>
    </group>
  )
}
