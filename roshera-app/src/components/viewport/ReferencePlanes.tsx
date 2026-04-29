import { useMemo } from 'react'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'

/**
 * Datum axis lines along the world origin — X, Y, Z all rendered in the
 * blueprint tick color. Orientation is conveyed by the gizmo widget, not
 * by RGB encoding, so the viewport reads as a single-hue technical drawing.
 */
export function ReferencePlanes() {
  const Y = 0.005
  const LEN = 100
  const theme = useThemeStore((s) => s.theme)

  const tick = useMemo(
    () => resolveCssVar('--cad-tick'),
    // theme switches change the resolved CSS var; depend on it explicitly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [theme],
  )

  return (
    <group name="reference-planes">
      {/* X axis */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            args={[new Float32Array([-LEN, Y, 0, LEN, Y, 0]), 3]}
          />
        </bufferGeometry>
        <lineBasicMaterial color={tick.color} transparent opacity={tick.alpha * 0.45} />
      </line>

      {/* Y axis (vertical datum — slightly stronger) */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            args={[new Float32Array([0, 0, 0, 0, LEN, 0]), 3]}
          />
        </bufferGeometry>
        <lineBasicMaterial color={tick.color} transparent opacity={tick.alpha * 0.7} />
      </line>

      {/* Z axis */}
      <line>
        <bufferGeometry>
          <bufferAttribute
            attach="attributes-position"
            args={[new Float32Array([0, Y, -LEN, 0, Y, LEN]), 3]}
          />
        </bufferGeometry>
        <lineBasicMaterial color={tick.color} transparent opacity={tick.alpha * 0.45} />
      </line>
    </group>
  )
}
