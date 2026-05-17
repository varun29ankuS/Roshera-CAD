import { useMemo } from 'react'
import * as THREE from 'three'
import type { SectionCapMesh } from '@/lib/section-api'

interface SectionCapHatchProps {
  cap: SectionCapMesh
}

/**
 * Mechanical-drawing-style hatch overlay on a section cap.
 *
 * Parallel diagonal lines at 45° in the cap's tangent basis, spaced
 * proportionally to the cap's largest extent so a cut through a tiny
 * gusset hatches as densely as one through a 1 m casting (this is the
 * ISO 128 / ASME Y14.3 convention — spacing scales with the section
 * size). The hatch is clipped to the cap polygon by walking each
 * triangle in the cap's triangulation: for every hatch ray, the
 * triangle contributes a sub-segment iff the ray pierces two of its
 * edges.
 *
 * Geometry notes:
 *
 *   1. **Tangent basis derived from `cap.planeNormal`.** We replicate
 *      the kernel's `compute_plane_axes` logic — pick the world axis
 *      least aligned with the normal as the reference, cross it with
 *      the normal to get `u`, then `v = normal × u`. This matches the
 *      kernel basis byte-for-byte so the 2D projection of every cap
 *      vertex agrees with the kernel's view of it.
 *
 *   2. **45° rotated basis.** Hatch direction = `(u + v) / √2`,
 *      perpendicular = `(-u + v) / √2`. Each cap vertex gets a "row
 *      coordinate" along the perpendicular; hatch lines are the
 *      iso-rows.
 *
 *   3. **Per-triangle clipping in 2D.** A hatch ray at row `s` crosses
 *      a triangle iff one of its three vertices is on the opposite
 *      row-side from the other two. The two crossing edges produce
 *      one segment endpoint each via linear interpolation on the row
 *      coordinate. Triangles that touch the line tangentially (one
 *      vertex exactly on the row) are skipped — the surrounding
 *      triangles cover the segment.
 *
 *   4. **Lift along plane normal** by `HATCH_LIFT > CAP_LIFT` so the
 *      hatch sits above the filled cap and below nothing else. Avoids
 *      z-fighting against the cap mesh (which is itself lifted by
 *      `CAP_LIFT` above the underlying clipped solid).
 *
 *   5. **`LineBasicMaterial`, `raycast` disabled.** Hatch lines do
 *      not participate in picking; they are a render-only decoration.
 */
export function SectionCapHatch({ cap }: SectionCapHatchProps) {
  const geometry = useMemo(() => buildHatchGeometry(cap), [cap])

  // No geometry if the cap is degenerate / empty.
  if (geometry === null) {
    return null
  }

  return (
    <lineSegments
      geometry={geometry}
      raycast={() => null}
      renderOrder={2}
    >
      <lineBasicMaterial color="#222222" transparent opacity={0.8} />
    </lineSegments>
  )
}

/**
 * Right-handed (u, v) tangent basis for the plane with given normal,
 * matching the kernel's `tessellation::adaptive::compute_plane_axes`
 * convention. Choosing the world axis least aligned with the normal
 * as the reference vector avoids the singularity when the normal
 * itself is one of the world axes.
 */
function computePlaneAxes(
  normal: [number, number, number],
): {
  uAxis: THREE.Vector3
  vAxis: THREE.Vector3
  normalUnit: THREE.Vector3
} {
  const n = new THREE.Vector3(normal[0], normal[1], normal[2])
  const nlen = n.length() || 1
  n.divideScalar(nlen)

  const ax = Math.abs(n.x)
  const ay = Math.abs(n.y)
  const az = Math.abs(n.z)
  // Reference axis = whichever world axis is least aligned with n.
  let ref: THREE.Vector3
  if (ax <= ay && ax <= az) {
    ref = new THREE.Vector3(1, 0, 0)
  } else if (ay <= az) {
    ref = new THREE.Vector3(0, 1, 0)
  } else {
    ref = new THREE.Vector3(0, 0, 1)
  }

  const u = new THREE.Vector3().crossVectors(ref, n)
  const ulen = u.length() || 1
  u.divideScalar(ulen)
  const v = new THREE.Vector3().crossVectors(n, u)
  return { uAxis: u, vAxis: v, normalUnit: n }
}

interface HatchVert2D {
  u: number
  v: number
  s: number // row coordinate along the perpendicular-to-hatch direction
}

function buildHatchGeometry(cap: SectionCapMesh): THREE.BufferGeometry | null {
  const triCount = cap.indices.length / 3
  if (triCount < 1) return null
  if (cap.vertices.length < 3) return null

  const { uAxis, vAxis, normalUnit } = computePlaneAxes(cap.planeNormal)
  const origin = new THREE.Vector3(
    cap.planeOrigin[0],
    cap.planeOrigin[1],
    cap.planeOrigin[2],
  )

  // 45° hatch axes in tangent space.
  // hatchDir = (u + v) / √2 (the line direction along which hatches run)
  // hatchPerp = (-u + v) / √2 (perpendicular; "row" coordinate selects which hatch ray)
  const INV_SQRT2 = 1 / Math.SQRT2
  const hatchPerpU = -INV_SQRT2
  const hatchPerpV = INV_SQRT2

  // Project every cap vertex to (u, v, s).
  const vertCount = cap.vertices.length / 3
  const proj: HatchVert2D[] = new Array(vertCount)
  let sMin = Infinity
  let sMax = -Infinity
  for (let i = 0; i < vertCount; i++) {
    const px = cap.vertices[i * 3] - origin.x
    const py = cap.vertices[i * 3 + 1] - origin.y
    const pz = cap.vertices[i * 3 + 2] - origin.z
    const u = px * uAxis.x + py * uAxis.y + pz * uAxis.z
    const v = px * vAxis.x + py * vAxis.y + pz * vAxis.z
    const s = u * hatchPerpU + v * hatchPerpV
    proj[i] = { u, v, s }
    if (s < sMin) sMin = s
    if (s > sMax) sMax = s
  }

  const sRange = sMax - sMin
  if (sRange < 1e-9) return null

  // Hatch spacing: target ~25 lines across the cap's perpendicular
  // extent, with a floor of 0.5 world units (so a tiny cap doesn't
  // collapse to a single line and a huge cap doesn't render 10000).
  // ~25 lines hits the perceptual sweet spot — dense enough to read
  // as a hatched fill at typical zoom, sparse enough not to alias on
  // a 1080p display.
  const TARGET_LINES = 25
  const MIN_SPACING = 0.5
  const spacing = Math.max(sRange / TARGET_LINES, MIN_SPACING)

  // Number of hatch lines. Walk the perpendicular extent at uniform
  // spacing, anchored so the centre row sits at s = (sMin + sMax) / 2
  // (a 0-centred hatch reads as natural and stays stable as the
  // section offset moves).
  const sCentre = 0.5 * (sMin + sMax)
  const halfRange = 0.5 * sRange
  const halfCount = Math.ceil(halfRange / spacing) + 1

  // For each hatch row, walk every triangle and collect the entry/exit
  // points where the row line crosses the triangle.
  const segments: number[] = [] // flat (ax, ay, az, bx, by, bz, ...)

  const HATCH_LIFT = 2e-2 // > CAP_LIFT (1e-2) so hatch sits above cap fill

  // Lift origin so that hatch sits above the cap polygon in 3D.
  const liftedOrigin = new THREE.Vector3(
    origin.x + normalUnit.x * HATCH_LIFT,
    origin.y + normalUnit.y * HATCH_LIFT,
    origin.z + normalUnit.z * HATCH_LIFT,
  )

  const writeSegment = (
    uA: number,
    vA: number,
    uB: number,
    vB: number,
  ) => {
    const ax = liftedOrigin.x + uA * uAxis.x + vA * vAxis.x
    const ay = liftedOrigin.y + uA * uAxis.y + vA * vAxis.y
    const az = liftedOrigin.z + uA * uAxis.z + vA * vAxis.z
    const bx = liftedOrigin.x + uB * uAxis.x + vB * vAxis.x
    const by = liftedOrigin.y + uB * uAxis.y + vB * vAxis.y
    const bz = liftedOrigin.z + uB * uAxis.z + vB * vAxis.z
    segments.push(ax, ay, az, bx, by, bz)
  }

  for (let k = -halfCount; k <= halfCount; k++) {
    const sLine = sCentre + k * spacing
    if (sLine < sMin - spacing * 0.5 || sLine > sMax + spacing * 0.5) {
      continue
    }

    // Per triangle: locate the edges crossed by the row line and
    // emit one segment per triangle.
    for (let t = 0; t < triCount; t++) {
      const ia = cap.indices[t * 3]
      const ib = cap.indices[t * 3 + 1]
      const ic = cap.indices[t * 3 + 2]
      const a = proj[ia]
      const b = proj[ib]
      const c = proj[ic]

      // Sign of (s_vertex - s_line). When all three are the same
      // sign (or zero), the line doesn't cross the triangle interior.
      const da = a.s - sLine
      const db = b.s - sLine
      const dc = c.s - sLine

      // Skip degenerate (all on one side or all tangent).
      const posCount =
        (da > 0 ? 1 : 0) + (db > 0 ? 1 : 0) + (dc > 0 ? 1 : 0)
      const negCount =
        (da < 0 ? 1 : 0) + (db < 0 ? 1 : 0) + (dc < 0 ? 1 : 0)
      if (posCount === 0 || negCount === 0) {
        // Line misses this triangle interior (possibly grazing one
        // vertex / one edge — neighbour triangle handles it).
        continue
      }

      // Two of the three edges must be crossed. Linearly interpolate
      // along each crossed edge to the point where d = 0.
      const crossings: Array<[number, number]> = []
      const tryEdge = (
        p0: HatchVert2D,
        p1: HatchVert2D,
        d0: number,
        d1: number,
      ) => {
        if ((d0 > 0 && d1 > 0) || (d0 < 0 && d1 < 0)) return
        const denom = d0 - d1
        if (Math.abs(denom) < 1e-18) return
        const tt = d0 / denom
        const u = p0.u + (p1.u - p0.u) * tt
        const v = p0.v + (p1.v - p0.v) * tt
        crossings.push([u, v])
      }
      tryEdge(a, b, da, db)
      tryEdge(b, c, db, dc)
      tryEdge(c, a, dc, da)

      if (crossings.length >= 2) {
        // Use the first two crossings; a third (when a vertex sits
        // exactly on the line) would be a duplicate of one of the
        // first two within float error.
        writeSegment(
          crossings[0][0],
          crossings[0][1],
          crossings[1][0],
          crossings[1][1],
        )
      }
    }
  }

  if (segments.length < 6) return null

  const geom = new THREE.BufferGeometry()
  geom.setAttribute(
    'position',
    new THREE.BufferAttribute(new Float32Array(segments), 3),
  )
  geom.computeBoundingBox()
  geom.computeBoundingSphere()
  return geom
}
