/**
 * Passive renderer for backend-authored sketch sessions.
 *
 * `SketchOverlay` only paints the sketch the *local* user is actively
 * drawing (`sketch.active === true`). Sketches created through the REST
 * surface — the MCP agent, a script, or a peer — arrive as
 * `SketchCreated` / `SketchUpdated` frames and land in the store's
 * `serverSketches` map, but nothing put them on screen: the build looked
 * like the solid simply popped into existence with no visible sketch
 * step.
 *
 * This component closes that gap. It renders every known server sketch as
 * a closed profile loop on its own plane, so an agent-driven
 * `create_sketch → add_points → extrude` sequence shows the 2D profile
 * first and then the extruded solid — the step-by-step story a viewer
 * expects to see. The sketch disappears when the backend deletes it
 * (typically on extrude with `consume`), which reads as the profile
 * "becoming" the solid.
 *
 * The one session the *local* user is editing is skipped here — it is
 * already drawn by `SketchOverlay` (active preview, snap markers,
 * dimensions), and double-drawing it would z-fight.
 */

import { useMemo } from 'react'
import { Line } from '@react-three/drei'
import * as THREE from 'three'

import { useSceneStore } from '@/stores/scene-store'
import { buildProfile2D } from '@/lib/sketch-extrude'
import { uvToWorld } from './sketch-plane-uv'

export function ServerSketches() {
  const serverSketches = useSceneStore((s) => s.serverSketches)
  // Sketch ids the user has hidden in the model tree — skip them entirely,
  // exactly as a hidden solid's mesh is not drawn.
  const hiddenSketchIds = useSceneStore((s) => s.hiddenSketchIds)
  // The locally-edited session (if any) — drawn by SketchOverlay, so we
  // must not redraw it here. `null` when the user isn't sketching, in
  // which case every server sketch is fair game.
  const activeLocalId = useSceneStore((s) =>
    s.sketch.active ? s.sketch.serverId : null,
  )

  const loops = useMemo(() => {
    const out: Array<{ key: string; points: THREE.Vector3[] }> = []
    for (const session of serverSketches.values()) {
      if (session.id === activeLocalId) continue
      if (hiddenSketchIds.has(session.id)) continue
      const shapes = Array.isArray(session.shapes) ? session.shapes : []
      shapes.forEach((shape, idx) => {
        const profile = buildProfile2D(
          shape.tool,
          shape.points,
          session.circle_segments,
        )
        if (!profile || profile.length < 3) return
        const world = profile.map((p) => uvToWorld(p, session.plane))
        // Close the loop visually by repeating the first vertex.
        world.push(world[0])
        out.push({ key: `${session.id}-${shape.id ?? idx}`, points: world })
      })
    }
    return out
  }, [serverSketches, activeLocalId, hiddenSketchIds])

  if (loops.length === 0) return null

  return (
    <group name="server-sketches" renderOrder={999}>
      {loops.map((l) => (
        <Line
          key={l.key}
          points={l.points}
          // Same cyan as the active-sketch preview so an agent-built
          // profile reads identically to a hand-drawn one.
          color="#3498db"
          lineWidth={2}
          opacity={0.95}
          transparent
          // Draw the generating curve ON TOP of solids. A revolve/extrude
          // profile is COINCIDENT with the solid's wall, so with the default
          // depth test the opaque mesh hides it entirely ("I can't see the
          // sketch"). depthTest:false + a high renderOrder keeps the curve
          // visible through the part — the standard "ghost generating geometry"
          // treatment in CAD sketchers.
          depthTest={false}
          renderOrder={999}
        />
      ))}
    </group>
  )
}
