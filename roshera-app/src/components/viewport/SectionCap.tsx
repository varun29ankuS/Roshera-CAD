import { useMemo } from 'react'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import type { SectionCapMesh } from '@/lib/section-api'

interface SectionCapProps {
  cap: SectionCapMesh
}

/**
 * One filled cross-section polygon on the cutting plane. Replaces the
 * "hollow" appearance the bare Three.js clipping plane leaves when a
 * solid is cut — the parent CADMesh hides the back half-space, this
 * mesh paints the cut opening with real geometry coloured to match the
 * parent solid.
 *
 * Three render decisions worth noting:
 *
 *   1. **No `clippingPlanes`.** The cap *is* the cutting plane; clipping
 *      it against itself would either pass everything (cap behind the
 *      plane normal) or kill it entirely (cap in front), neither of
 *      which is what we want.
 *
 *   2. **`MeshBasicMaterial`, `DoubleSide`.** The cap has a constant
 *      normal so lighting is uniform across it — `MeshStandardMaterial`
 *      would just compute the same Lambert term per fragment for no
 *      visual gain. `DoubleSide` covers the `flipped` toggle: the
 *      kernel emits cap normals oriented along the cut direction, but
 *      flipping the section side flips which face the user looks at.
 *
 *   3. **No raycast hits** (`raycast={() => null}`). The cap should not
 *      intercept face / edge / vertex picks — those resolve against the
 *      real B-Rep geometry. Without this the user would pick the cap
 *      instead of the face directly behind it on every click into the
 *      cut opening.
 */
export function SectionCap({ cap }: SectionCapProps) {
  const objects = useSceneStore((s) => s.objects)

  // Push the cap forward along its normal by `CAP_LIFT` so it sits
  // strictly in front of the clipped-solid edge fragments at the cut.
  // PolygonOffset alone is depth-buffer-precision-dependent and fails
  // at long camera distances / steep grazing angles; an actual
  // geometric offset is camera-independent.
  //
  // 1e-2 world units is the empirical floor for X/Y-axis sections at
  // typical CAD camera positions — for cameras viewing the cut at a
  // shallow grazing angle the geometric offset projects to only a
  // fraction of itself in depth space (the lift direction is the
  // plane normal, which is nearly orthogonal to the camera view ray
  // when looking *along* a non-Z axis at standard isometric-ish
  // camera positions). At 1e-3 the depth advantage collapsed below
  // the 24-bit depth buffer noise floor at moderate zoom and the cap
  // alternated wins/losses with the clipped solid's edge fragments.
  // 1e-2 sits two orders of magnitude above depth-buffer noise at
  // any reasonable camera distance while staying well below the
  // smallest dimension a user can perceive (0.01 mm at the default
  // 1 unit = 1 mm CAD scale).
  const geometry = useMemo(() => {
    const CAP_LIFT = 1e-2
    const lifted = new Float32Array(cap.vertices.length)
    const nx = cap.planeNormal[0]
    const ny = cap.planeNormal[1]
    const nz = cap.planeNormal[2]
    const nlen = Math.hypot(nx, ny, nz) || 1
    const lx = (nx / nlen) * CAP_LIFT
    const ly = (ny / nlen) * CAP_LIFT
    const lz = (nz / nlen) * CAP_LIFT
    for (let i = 0; i < cap.vertices.length; i += 3) {
      lifted[i] = cap.vertices[i] + lx
      lifted[i + 1] = cap.vertices[i + 1] + ly
      lifted[i + 2] = cap.vertices[i + 2] + lz
    }
    const geom = new THREE.BufferGeometry()
    geom.setAttribute('position', new THREE.BufferAttribute(lifted, 3))
    geom.setAttribute('normal', new THREE.BufferAttribute(cap.normals, 3))
    geom.setIndex(new THREE.BufferAttribute(cap.indices, 1))
    geom.computeBoundingBox()
    geom.computeBoundingSphere()
    return geom
  }, [cap.vertices, cap.normals, cap.indices, cap.planeNormal])

  // Match the parent solid's diffuse colour so the cap reads as the
  // same material exposed at the cut. Falls back to a neutral grey if
  // the parent has somehow gone away between the section fetch and
  // this render — the cap should still appear so the user isn't left
  // staring at a hollow shell while React reconciles.
  const color = useMemo(() => {
    const parent = objects.get(cap.solidId)
    return parent?.material.color ?? '#888888'
  }, [objects, cap.solidId])

  // `polygonOffset` biases the cap's fragment depth toward the camera
  // by ~1 depth unit, so the cap *always* wins the depth tie against
  // any underlying-solid fragment that the rasterizer keeps right at
  // the clipping boundary. Without this the cap z-fights with the
  // solid's surviving edge-fragments (every triangle that straddles
  // the cut has fragments interpolated *to* the plane) and flickers
  // across the cut surface as the camera moves. Geometry stays
  // mathematically on the plane — depth bias is a pure rasterizer
  // adjustment.
  const material = useMemo(() => {
    const m = new THREE.MeshBasicMaterial({
      color,
      side: THREE.DoubleSide,
    })
    m.polygonOffset = true
    m.polygonOffsetFactor = -8
    m.polygonOffsetUnits = -8
    return m
  }, [color])

  return (
    <mesh
      geometry={geometry}
      material={material}
      raycast={() => null}
      renderOrder={1}
    />
  )
}
