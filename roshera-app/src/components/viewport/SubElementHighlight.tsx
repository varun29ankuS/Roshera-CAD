import { useMemo } from 'react'
import * as THREE from 'three'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'

/**
 * Renders visual feedback for sub-element selections (face / edge / vertex).
 *
 * The CADMesh click handler stores a `SubElementSelection { objectId, type,
 * index }`. For `face` selections, `index` is the kernel `FaceId` (resolved
 * via the per-triangle `mesh.faceIds` map shipped on every broadcast). For
 * `edge` and `vertex` it is still the raw Three.js triangle index — the
 * kernel doesn't yet ship per-triangle edge/vertex maps, so the backend
 * resolves those from the picked point.
 *
 * Face overlay strategy: collect every triangle whose `faceIds[t]` matches
 * the selected face id and render them as a single merged BufferGeometry,
 * inflated slightly along each triangle's normal to win the depth test.
 * If the mesh has no `faceIds` map (legacy frame), fall back to drawing
 * just the single triangle the user clicked.
 *
 * Edge / vertex paths still draw the three corners of the picked triangle —
 * a real backend topology lookup is what unlocks proper edge/vertex
 * highlighting and is handled by `SubElementResult` from the server.
 */
export function SubElementHighlight() {
  const subElementSelections = useSceneStore((s) => s.subElementSelections)
  const hoveredSubElement = useSceneStore((s) => s.hoveredSubElement)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)
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

  // Resolve every selection to its world-space corner positions. Face
  // selections may fan out to many triangles (every triangle whose
  // `faceIds[t]` matches the selected face id). Edge / vertex
  // selections always resolve to a single triangle.
  const resolved = useMemo(() => {
    const out: Array<{
      key: string
      type: 'face' | 'edge' | 'vertex'
      triangles: Array<{
        a: THREE.Vector3
        b: THREE.Vector3
        c: THREE.Vector3
        normal: THREE.Vector3
      }>
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
      const indexAttr = geom.getIndex()

      const cadObject: CADObject | undefined = objects.get(sel.objectId)
      const faceIds = cadObject?.mesh.faceIds

      // Decide which triangle indices to render.
      const triangleIndices: number[] = []
      if (sel.type === 'face' && faceIds) {
        // Fan out: every triangle in the mesh whose face id matches the
        // selected kernel FaceId belongs to the same B-Rep face.
        for (let t = 0; t < faceIds.length; t++) {
          if (faceIds[t] === sel.index) {
            triangleIndices.push(t)
          }
        }
        // Defensive: if nothing matched (stale frame, mismatched face id)
        // fall back to the single picked triangle so the click still
        // registers visually.
        if (triangleIndices.length === 0) {
          triangleIndices.push(sel.index)
        }
      } else {
        triangleIndices.push(sel.index)
      }

      const triangles: Array<{
        a: THREE.Vector3
        b: THREE.Vector3
        c: THREE.Vector3
        normal: THREE.Vector3
      }> = []

      for (const t of triangleIndices) {
        const i0 = t * 3
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

        triangles.push({ a, b, c, normal })
      }

      if (triangles.length === 0) continue

      out.push({
        key: `${hover ? 'hover' : 'sel'}:${sel.objectId}:${sel.type}:${sel.index}`,
        type: sel.type,
        triangles,
        hover,
      })
    }
    return out
  }, [subElementSelections, hoveredSubElement, meshRefs, objects])

  if (resolved.length === 0) return null

  return (
    <group>
      {resolved.map((r) => (
        <SelectionMark
          key={r.key}
          type={r.type}
          triangles={r.triangles}
          colors={palette}
          hover={r.hover}
        />
      ))}
    </group>
  )
}

interface Triangle {
  a: THREE.Vector3
  b: THREE.Vector3
  c: THREE.Vector3
  normal: THREE.Vector3
}

interface SelectionMarkProps {
  type: 'face' | 'edge' | 'vertex'
  triangles: Triangle[]
  colors: { face: string; edge: string; vertex: string }
  hover: boolean
}

const OFFSET = 0.002

function inflate(p: THREE.Vector3, normal: THREE.Vector3): THREE.Vector3 {
  return p.clone().addScaledVector(normal, OFFSET)
}

function SelectionMark({ type, triangles, colors, hover }: SelectionMarkProps) {
  if (type === 'face') {
    return <FaceMark triangles={triangles} color={colors.face} hover={hover} />
  }
  // Edge / vertex always operate on the single picked triangle.
  const tri = triangles[0]
  const ao = inflate(tri.a, tri.normal)
  const bo = inflate(tri.b, tri.normal)
  const co = inflate(tri.c, tri.normal)
  if (type === 'edge') {
    return <EdgeMark ao={ao} bo={bo} co={co} color={colors.edge} hover={hover} />
  }
  return <VertexMark ao={ao} bo={bo} co={co} color={colors.vertex} hover={hover} />
}

interface FaceMarkProps {
  triangles: Triangle[]
  color: string
  hover: boolean
}

function FaceMark({ triangles, color, hover }: FaceMarkProps) {
  // Merge every matching triangle into one BufferGeometry so the overlay
  // is a single draw call regardless of tessellation density.
  const geometry = useMemo(() => {
    const positions = new Float32Array(triangles.length * 9)
    for (let i = 0; i < triangles.length; i++) {
      const { a, b, c, normal } = triangles[i]
      const ao = inflate(a, normal)
      const bo = inflate(b, normal)
      const co = inflate(c, normal)
      const off = i * 9
      positions[off + 0] = ao.x
      positions[off + 1] = ao.y
      positions[off + 2] = ao.z
      positions[off + 3] = bo.x
      positions[off + 4] = bo.y
      positions[off + 5] = bo.z
      positions[off + 6] = co.x
      positions[off + 7] = co.y
      positions[off + 8] = co.z
    }
    const geom = new THREE.BufferGeometry()
    geom.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    geom.computeVertexNormals()
    return geom
  }, [triangles])

  return (
    <mesh geometry={geometry}>
      <meshBasicMaterial
        color={color}
        side={THREE.DoubleSide}
        transparent
        opacity={hover ? 0.2 : 0.45}
        depthWrite={false}
      />
    </mesh>
  )
}

interface EdgeMarkProps {
  ao: THREE.Vector3
  bo: THREE.Vector3
  co: THREE.Vector3
  color: string
  hover: boolean
}

function EdgeMark({ ao, bo, co, color, hover }: EdgeMarkProps) {
  // Three line segments forming the triangle outline. Without backend
  // topology resolution we don't yet know which of the three is the
  // user-intended edge; outlining all three still makes "I clicked
  // somewhere and something happened" obvious.
  const geometry = useMemo(() => {
    return new THREE.BufferGeometry().setFromPoints([ao, bo, bo, co, co, ao])
  }, [ao, bo, co])
  return (
    <lineSegments geometry={geometry}>
      <lineBasicMaterial
        color={color}
        linewidth={hover ? 1 : 2}
        depthTest={false}
        transparent
        opacity={hover ? 0.5 : 1}
      />
    </lineSegments>
  )
}

interface VertexMarkProps {
  ao: THREE.Vector3
  bo: THREE.Vector3
  co: THREE.Vector3
  color: string
  hover: boolean
}

function VertexMark({ ao, bo, co, color, hover }: VertexMarkProps) {
  return (
    <group>
      {[ao, bo, co].map((p, i) => (
        <mesh key={i} position={p}>
          <sphereGeometry args={[hover ? 0.1 : 0.15, 8, 8]} />
          <meshBasicMaterial
            color={color}
            depthTest={false}
            transparent
            opacity={hover ? 0.5 : 1}
          />
        </mesh>
      ))}
    </group>
  )
}
