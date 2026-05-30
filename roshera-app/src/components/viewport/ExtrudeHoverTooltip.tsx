import { useEffect, useState } from 'react'
import { useSceneStore } from '@/stores/scene-store'

/**
 * Inline floating tooltip that appears next to the cursor while the
 * user hovers an extruded body. Reads `analyticalGeometry.params`
 * (populated by `extrude_sketch` on the backend) and shows the
 * authoring intent — sketch plane, push distance, and the per-shape
 * tool roster — so the user can answer "what am I looking at?"
 * without opening the timeline or properties panel.
 *
 * Outer-vs-hole classification is decided geometrically at extrude
 * time via point-in-polygon containment, not stored on shapes, so
 * the tooltip simply lists the closed loops the user drew.
 *
 * Lives outside the R3F canvas: tracks raw window pointer position so
 * orbit / pan camera motion doesn't desync the panel from the cursor.
 * Only renders for `objectType === 'extrude'` — primitives and
 * imported bodies fall through to nothing.
 */
export function ExtrudeHoverTooltip() {
  const hoveredId = useSceneStore((s) => s.hoveredId)
  const objects = useSceneStore((s) => s.objects)
  const [pointer, setPointer] = useState<{ x: number; y: number } | null>(null)

  // Wire window-level pointermove only while a hover is active. Avoids
  // every mouse move in the app paying the setState cost when nothing
  // would render.
  useEffect(() => {
    if (!hoveredId) {
      // Clearing the cached pointer when the hover ends is the intended
      // use of an effect; the strict react-hooks rule flags the
      // synchronous setState but there is no render-time equivalent.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setPointer(null)
      return
    }
    const onMove = (e: PointerEvent) => {
      setPointer({ x: e.clientX, y: e.clientY })
    }
    window.addEventListener('pointermove', onMove)
    return () => window.removeEventListener('pointermove', onMove)
  }, [hoveredId])

  if (!hoveredId || !pointer) return null
  const obj = objects.get(hoveredId)
  if (!obj || obj.objectType !== 'extrude') return null

  const params = obj.analyticalGeometry?.params
  if (!params) return null

  const plane = formatPlane(params.plane)
  const distance = formatDistance(params.distance)
  const shapes = readShapes(params)

  // The extrude tooltip relies on at least one descriptive field — a
  // legacy single-shape extrude will still expose plane and distance,
  // while multi-shape adds the shape roster. Bail out if the params
  // object carries none of them (defensive: shouldn't happen for our
  // own backend, but other extrude producers might exist later).
  if (!plane && !distance && shapes.length === 0) return null

  // Offset so the tooltip never sits under the cursor (which would
  // intercept further raycasts and unhover the body). 14px diagonal
  // matches the native Windows / Three.js drei tooltip convention.
  const left = pointer.x + 14
  const top = pointer.y + 14

  return (
    <div
      className="fixed z-40 pointer-events-none cad-panel px-2.5 py-1.5 text-[10px] uppercase tracking-wider min-w-[160px] max-w-[240px] shadow-lg"
      style={{ left, top }}
      role="tooltip"
    >
      <div className="text-foreground font-semibold normal-case tracking-normal text-[11px] mb-1 truncate">
        {obj.name}
      </div>
      {plane && (
        <Row label="Plane" value={plane} />
      )}
      {distance && (
        <Row label="Distance" value={distance} />
      )}
      {shapes.length > 0 && (
        <>
          <div className="mt-1 mb-0.5 text-muted-foreground">
            Shapes · {shapes.length}
          </div>
          <ul className="space-y-0.5">
            {shapes.map((s, i) => (
              <li
                key={s.id || i}
                className="flex items-center justify-between gap-2 normal-case tracking-normal text-[10.5px]"
              >
                <span className="text-muted-foreground tabular-nums">
                  #{i + 1}
                </span>
                <span className="text-foreground/80">{s.tool}</span>
                <span className="text-muted-foreground tabular-nums">
                  {s.pointCount} pt
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  )
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="text-muted-foreground">{label}</span>
      <span className="text-foreground tabular-nums normal-case tracking-normal">
        {value}
      </span>
    </div>
  )
}

interface ShapeSummary {
  id: string
  tool: string
  pointCount: number
}

/**
 * Read the multi-shape descriptor produced by `extrude_sketch`. Each
 * entry on the wire is `{id, tool, polygon: [[x,y],…]}`.
 *
 * Falls back to the legacy single-shape encoding (top-level `polygon`
 * + `tool`) for bodies extruded before Slice 1, so older sessions
 * still display useful info.
 */
function readShapes(params: Record<string, unknown>): ShapeSummary[] {
  const raw = params['shapes']
  if (Array.isArray(raw)) {
    return raw
      .map((entry) => parseShapeEntry(entry))
      .filter((s): s is ShapeSummary => s !== null)
  }
  // Legacy: single polygon at the root with the active tool.
  const polygon = params['polygon']
  const tool = params['tool']
  if (Array.isArray(polygon) && typeof tool === 'string') {
    return [
      {
        id: 'legacy',
        tool,
        pointCount: polygon.length,
      },
    ]
  }
  return []
}

function parseShapeEntry(entry: unknown): ShapeSummary | null {
  if (!entry || typeof entry !== 'object') return null
  const o = entry as Record<string, unknown>
  const id = typeof o.id === 'string' ? o.id : ''
  const tool = typeof o.tool === 'string' ? o.tool : 'unknown'
  const polygon = o.polygon
  const pointCount = Array.isArray(polygon) ? polygon.length : 0
  return { id, tool, pointCount }
}

/**
 * Render the wire `plane` field. Standard planes serialise as the
 * lowercase string (`"xy" | "xz" | "yz"`); custom planes serialise as
 * an object with origin/u_axis/v_axis. Both shapes deserve a
 * single-line label.
 */
function formatPlane(plane: unknown): string {
  if (typeof plane === 'string') return plane.toUpperCase()
  if (plane && typeof plane === 'object') {
    const o = plane as Record<string, unknown>
    if (Array.isArray(o.origin) && o.origin.length === 3) {
      const [x, y, z] = o.origin as number[]
      return `face @ ${fmt(x)}, ${fmt(y)}, ${fmt(z)}`
    }
  }
  return ''
}

function formatDistance(d: unknown): string {
  if (typeof d !== 'number' || !Number.isFinite(d)) return ''
  return `${fmt(d)} mm`
}

function fmt(n: number): string {
  if (!Number.isFinite(n)) return '—'
  // Trim trailing zeros while keeping ≤2 decimals for readability.
  return Number(n.toFixed(2)).toString()
}
