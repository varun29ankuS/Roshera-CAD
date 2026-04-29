import { useMemo } from 'react'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'

/**
 * Renders visual feedback for sub-element selections (face / edge / vertex).
 *
 * The CADMesh click handler stores a `SubElementSelection { objectId, type,
 * index }` where `index` is the THREE.js triangle index produced by the
 * raycast. We resolve that to the three corner positions in world space and
 * paint:
 *   - face   → a filled triangle overlay (slightly inflated along its
 *              normal to win the depth test)
 *   - edge   → the three line segments of the triangle (closest one to the
 *              click is the "edge" — without backend topology resolution we
 *              currently render all three so the user has something to see)
 *   - vertex → a small sphere at each corner
 *
 * NOTE: This is a frontend-only heuristic — `index` is a triangle index, not
 * a B-Rep face id. Proper face/edge highlighting (covering all triangles
 * that belong to the same B-Rep face) requires the backend SubElementPick
 * handler to return the topology id and the triangle range. Tracked as
 * task #121 (extrude wiring) since it's the same data dependency.
 */
export function SubElementHighlight() {
  const subElementSelections = useSceneStore((s) => s.subElementSelections)
  const hoveredSubElement = useSceneStore((s) => s.hoveredSubElement)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const theme = useThemeStore((s) => s.theme)

  const palette = useMemo(() => {
    const face = resolveCssVar('--cad-face-selected').color.getHexString()
    const edge = resolveCssVar('--cad-edge-selected').color.getHexString()
    const vertex = resolveCssVar('--cad-vertex-selected').color.getHexString()
    return {
      face: `#${face}`,
      edge: `#${edge}`,
      vertex: `#${vertex}`,
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme])

  // Resolve every selection to its three world-space corner positions.
  const resolved = useMemo(() => {
    const out: Array<{
      key: string
      type: 'face' | 'edge' | 'vertex'
      a: THREE.Vector3
      b: THREE.Vector3
      c: THREE.Vector3
      normal: THREE.Vector3
      hover: boolean
    }> = []

    const items: Array<{ sel: typeof subElementSelections[number]; hover: boolean }> = [
      ...subElementSelections.map((sel) => ({ sel, hover: false })),
    ]
    if (
      hoveredSubElement &&
      !subElementSelections.some(
        (s) =>
          s.objectId === hoveredSubElement.objectId &&
          s.type === hoveredSubElement.type &&
          s.index === hoveredSubElement.index,
      )
    ) {
      items.push({ sel: hoveredSubElement, hover: true })
    }

    for (const { sel, hover } of items) {
      const mesh = meshRefs.get(sel.objectId)
      if (!mesh) continue
      const geom = mesh.geometry
      if (!geom) continue

      const positions = geom.getAttribute('position') as
        | THREE.BufferAttribute
        | undefined
      if (!positions) continue

      const i0 = sel.index * 3
      const indexAttr = geom.getIndex()

      // If the geometry is indexed (typical for our backend meshes), the
      // triangle's three vertex positions are looked up via the index
      // buffer. Otherwise the three positions are stored sequentially.
      let vi0: number, vi1: number, vi2: number
      if (indexAttr) {
        if (i0 + 2 >= indexAttr.count) continue
        vi0 = indexAttr.getX(i0)
        vi1 = indexAttr.getX(i0 + 1)
        vi2 = indexAttr.getX(i0 + 2)
      } else {
        vi0 = i0
        vi1 = i0 + 1
        vi2 = i0 + 2
      }
      if (
        vi0 >= positions.count ||
        vi1 >= positions.count ||
        vi2 >= positions.count
      ) {
        continue
      }

      const a = new THREE.Vector3()
        .fromBufferAttribute(positions, vi0)
        .applyMatrix4(mesh.matrixWorld)
      const b = new THREE.Vector3()
        .fromBufferAttribute(positions, vi1)
        .applyMatrix4(mesh.matrixWorld)
      const c = new THREE.Vector3()
        .fromBufferAttribute(positions, vi2)
        .applyMatrix4(mesh.matrixWorld)

      const ab = new THREE.Vector3().subVectors(b, a)
      const ac = new THREE.Vector3().subVectors(c, a)
      const normal = new THREE.Vector3().crossVectors(ab, ac).normalize()

      out.push({
        key: `${hover ? 'hover' : 'sel'}:${sel.objectId}:${sel.type}:${sel.index}`,
        type: sel.type,
        a,
        b,
        c,
        normal,
        hover,
      })
    }
    return out
  }, [subElementSelections, hoveredSubElement, meshRefs])

  if (resolved.length === 0) return null

  return (
    <group>
      {resolved.map((r) => (
        <SelectionMark
          key={r.key}
          type={r.type}
          a={r.a}
          b={r.b}
          c={r.c}
          normal={r.normal}
          colors={palette}
          hover={r.hover}
        />
      ))}
    </group>
  )
}

interface SelectionMarkProps {
  type: 'face' | 'edge' | 'vertex'
  a: THREE.Vector3
  b: THREE.Vector3
  c: THREE.Vector3
  normal: THREE.Vector3
  colors: { face: string; edge: string; vertex: string }
  hover: boolean
}

function SelectionMark({ type, a, b, c, normal, colors, hover }: SelectionMarkProps) {
  // Inflate the triangle slightly along its normal so the overlay wins the
  // depth test against the underlying mesh without z-fighting.
  const offset = 0.002
  const ao = a.clone().addScaledVector(normal, offset)
  const bo = b.clone().addScaledVector(normal, offset)
  const co = c.clone().addScaledVector(normal, offset)

  const opacity = hover ? 0.2 : 0.45

  if (type === 'face') {
    const geometry = new THREE.BufferGeometry()
    const positions = new Float32Array([
      ao.x, ao.y, ao.z,
      bo.x, bo.y, bo.z,
      co.x, co.y, co.z,
    ])
    geometry.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    geometry.computeVertexNormals()
    return (
      <mesh geometry={geometry}>
        <meshBasicMaterial
          color={colors.face}
          side={THREE.DoubleSide}
          transparent
          opacity={opacity}
          depthWrite={false}
        />
      </mesh>
    )
  }

  if (type === 'edge') {
    // Three line segments forming the triangle outline. Without backend
    // topology resolution we don't yet know which of the three is the
    // user-intended edge; outlining all three still makes "I clicked
    // somewhere and something happened" obvious.
    const points = [ao, bo, bo, co, co, ao]
    const geometry = new THREE.BufferGeometry().setFromPoints(points)
    return (
      <lineSegments geometry={geometry}>
        <lineBasicMaterial
          color={colors.edge}
          linewidth={hover ? 1 : 2}
          depthTest={false}
          transparent
          opacity={hover ? 0.5 : 1}
        />
      </lineSegments>
    )
  }

  // vertex — small spheres at each corner
  return (
    <group>
      {[ao, bo, co].map((p, i) => (
        <mesh key={i} position={p}>
          <sphereGeometry args={[hover ? 0.1 : 0.15, 8, 8]} />
          <meshBasicMaterial
            color={colors.vertex}
            depthTest={false}
            transparent
            opacity={hover ? 0.5 : 1}
          />
        </mesh>
      ))}
    </group>
  )
}
