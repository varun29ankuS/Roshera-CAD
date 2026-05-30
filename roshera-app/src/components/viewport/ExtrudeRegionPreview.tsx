/**
 * Hover-time region-decomposition overlay (Slice D).
 *
 * Active only while `sketch.extrudeHover.active` is `true` — the
 * `SketchPanel` Finish & Extrude button drives that flag via
 * `onPointerEnter` / `onPointerLeave`. When active, paint the
 * server-authoritative region decomposition so the user sees exactly
 * what the backend would extrude: outer loops in blue, hole loops in
 * red, all lifted onto the active sketch plane.
 *
 * Backend is the source of truth — region classification arrives via
 * the `SketchRegionsUpdated` WS frame the api-server pushes after
 * every sketch mutation. The frontend never re-runs the classifier;
 * this component only renders what the kernel produced. Matches the
 * "backend-driven, frontend is a thin display layer" architectural
 * rule.
 *
 * Renders nothing unless:
 *   • `sketch.active` and `sketch.extrudeHover.active` are both true,
 *   • `sketch.regions` is non-empty (no extrudable layout = nothing
 *     to preview; `regionError`, if set, surfaces via the panel),
 *   • each region's outer shape materialises to a valid polygon.
 */

import { useMemo } from 'react'
import { Line } from '@react-three/drei'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { buildProfile2D } from '@/lib/sketch-extrude'
import { uvToWorld } from './sketch-plane-uv'

const OUTER_COLOR = '#3b82f6' // tailwind blue-500
const HOLE_COLOR = '#ef4444' // tailwind red-500

export function ExtrudeRegionPreview() {
  const active = useSceneStore((s) => s.sketch.active)
  const hover = useSceneStore((s) => s.sketch.extrudeHover.active)
  const shapes = useSceneStore((s) => s.sketch.shapes)
  const plane = useSceneStore((s) => s.sketch.plane)
  const circleSegments = useSceneStore((s) => s.sketch.circleSegments)
  const regions = useSceneStore((s) => s.sketch.regions)

  // Lift every region's loops into world-space `THREE.Vector3[]`s,
  // pre-closed (first vertex duplicated at the end) so drei's `<Line>`
  // renders a visually-closed polygon. Memoised on the inputs that
  // actually affect geometry — the hover flag only gates the render,
  // it doesn't invalidate the cached world-space buffers.
  const loops = useMemo(() => {
    const buildClosedWorld = (
      shapeIdx: number,
    ): THREE.Vector3[] | null => {
      const shape = shapes[shapeIdx]
      if (!shape) return null
      const profile = buildProfile2D(shape.tool, shape.points, circleSegments)
      if (!profile || profile.length < 3) return null
      const world = profile.map((p) => uvToWorld(p, plane))
      world.push(world[0])
      return world
    }
    const out: Array<{
      key: string
      outer: THREE.Vector3[]
      holes: THREE.Vector3[][]
    }> = []
    for (let i = 0; i < regions.length; i++) {
      const region = regions[i]
      const outer = buildClosedWorld(region.outer_shape_idx)
      if (!outer) continue
      const holes: THREE.Vector3[][] = []
      for (const holeIdx of region.hole_shape_idxs) {
        const hole = buildClosedWorld(holeIdx)
        if (hole) holes.push(hole)
      }
      out.push({
        key: `region-${i}-${region.outer_shape_idx}`,
        outer,
        holes,
      })
    }
    return out
  }, [shapes, plane, circleSegments, regions])

  if (!active || !hover || loops.length === 0) return null

  return (
    <group name="sketch-extrude-region-preview">
      {loops.map((region) => (
        <group key={region.key}>
          <Line
            points={region.outer}
            color={OUTER_COLOR}
            lineWidth={3}
            opacity={0.95}
            transparent
          />
          {region.holes.map((hole, i) => (
            <Line
              key={`${region.key}-hole-${i}`}
              points={hole}
              color={HOLE_COLOR}
              lineWidth={3}
              opacity={0.95}
              transparent
            />
          ))}
        </group>
      ))}
    </group>
  )
}
