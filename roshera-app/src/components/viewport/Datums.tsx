import { useEffect, useState } from 'react'

/**
 * Kernel-driven datum visualisation. Replaces the old decorative
 * `ReferencePlanes` that hardcoded its own X/Y/Z lines.
 *
 * Slice 1 of the datum-system plan: the kernel is the source of truth
 * for the seven default datums (Origin + three reference planes + three
 * reference axes). This component fetches `/api/datums` and renders
 * each entity, honouring the kernel-side `visible` flag.
 *
 * Standard CAD colour convention used here:
 *   X-axis / YZ-plane → red    (#e74c3c)
 *   Y-axis / XZ-plane → green  (#2ecc71)
 *   Z-axis / XY-plane → blue   (#3498db)
 *
 * Geometry conventions match the kernel: the "XY plane" spans local X
 * and Y with normal +Z, etc. Axes extend ±100 mm. Planes are 50 mm
 * squares so they hint at orientation without dominating the viewport.
 */

const API_BASE = import.meta.env.VITE_API_URL || ''

type DatumKindWire = 'origin' | 'plane' | 'axis'

interface DatumDto {
  id: number
  name: string
  kind: DatumKindWire
  plane_orientation?: 'xy' | 'xz' | 'yz' | 'custom'
  axis_direction?: 'x' | 'y' | 'z'
  origin: [number, number, number]
  visible: boolean
  is_default: boolean
}

interface DatumListResponse {
  datums: DatumDto[]
}

const AXIS_LEN = 100
const PLANE_HALF = 25

const AXIS_COLOR: Record<'x' | 'y' | 'z', string> = {
  x: '#e74c3c',
  y: '#2ecc71',
  z: '#3498db',
}

// All reference planes render as a uniform darker shade of the
// viewport background — `#000000` over a light bg darkens it; over a
// dark bg it darkens further. Per-axis color is communicated by the
// axis lines themselves, so the planes don't need to repeat it.
const PLANE_TINT = '#000000'

/**
 * Rotation that takes Three.js's default XY-plane geometry (normal +Z)
 * to the orientation the kernel describes. The "custom" orientation is
 * not yet a real authored plane in Slice 1, so we leave it identity.
 */
function planeRotation(orient: 'xy' | 'xz' | 'yz' | 'custom'): [number, number, number] {
  switch (orient) {
    case 'xy':
      return [0, 0, 0]
    case 'xz':
      return [-Math.PI / 2, 0, 0]
    case 'yz':
      return [0, Math.PI / 2, 0]
    case 'custom':
      return [0, 0, 0]
  }
}

export function Datums() {
  const [datums, setDatums] = useState<DatumDto[]>([])

  useEffect(() => {
    let cancelled = false

    const fetchDatums = async () => {
      try {
        const res = await fetch(`${API_BASE}/api/datums`)
        if (!res.ok) return
        const data: DatumListResponse = await res.json()
        if (!cancelled) setDatums(data.datums)
      } catch {
        // Backend unreachable — leave whatever we already had so the
        // viewport doesn't flicker between loaded and empty states.
      }
    }

    fetchDatums()
    const interval = window.setInterval(fetchDatums, 5000)
    return () => {
      cancelled = true
      window.clearInterval(interval)
    }
  }, [])

  return (
    <group name="datums">
      {datums.map((datum) => {
        if (!datum.visible) return null
        const [ox, oy, oz] = datum.origin

        if (datum.kind === 'origin') {
          // Render on top of the axes + reference planes (which pass
          // straight through the origin and would otherwise paint
          // colored stripes across the sphere whenever they're in
          // front of it relative to the camera). Higher segment count
          // gives a smooth silhouette at any zoom.
          return (
            <mesh
              key={datum.id}
              position={[ox, oy, oz]}
              renderOrder={2}
            >
              <sphereGeometry args={[0.4, 32, 32]} />
              <meshBasicMaterial color="#f1c40f" depthTest={false} />
            </mesh>
          )
        }

        if (datum.kind === 'axis' && datum.axis_direction) {
          const dir = datum.axis_direction
          const color = AXIS_COLOR[dir]
          const dx = dir === 'x' ? AXIS_LEN : 0
          const dy = dir === 'y' ? AXIS_LEN : 0
          const dz = dir === 'z' ? AXIS_LEN : 0
          const positions = new Float32Array([
            ox - dx, oy - dy, oz - dz,
            ox + dx, oy + dy, oz + dz,
          ])
          return (
            <line key={datum.id}>
              <bufferGeometry>
                <bufferAttribute attach="attributes-position" args={[positions, 3]} />
              </bufferGeometry>
              <lineBasicMaterial color={color} transparent opacity={0.55} />
            </line>
          )
        }

        if (datum.kind === 'plane' && datum.plane_orientation) {
          const orient = datum.plane_orientation
          const rotation = planeRotation(orient)
          const size = PLANE_HALF * 2
          return (
            <group key={datum.id} position={[ox, oy, oz]} rotation={rotation}>
              <mesh>
                <planeGeometry args={[size, size]} />
                <meshBasicMaterial
                  color={PLANE_TINT}
                  transparent
                  opacity={0.04}
                  depthWrite={false}
                  side={2}
                />
              </mesh>
            </group>
          )
        }

        return null
      })}
    </group>
  )
}
