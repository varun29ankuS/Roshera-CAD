/**
 * The sketch workbench's drawing surface (Varun 2026-11-14). Draws the open
 * constraint sketch on its plane -- curves, points, construction dashed,
 * the selection highlighted -- and turns clicks into the active tool's
 * input. A mouse move updates only the rubber band and the snap marker
 * (`useBenchCursor`); the sketch itself re-renders when the sketch changes.
 */

import { useCallback, useMemo, useRef, useState } from 'react'
import { Html, Line } from '@react-three/drei'
import { useThree, type ThreeEvent } from '@react-three/fiber'
import * as THREE from 'three'
import { useThemeStore } from '@/stores/theme-store'
import { cn } from '@/lib/utils'
import { useBenchCursor, useSketchBench } from '@/stores/sketch-bench-store'
import { useSceneStore } from '@/stores/scene-store'
import {
  anchorOf,
  CONSTRAINT_GLYPH,
  crossings,
  entityPaths,
  overlaps,
  PICK_TOOLS,
  pickEntity,
  planPaths,
  planShape,
  sameRef,
  showsMark,
  snap,
  snapTargets,
  TOOL_CLICKS,
  type Vec2,
} from '@/lib/sketch-bench'
import type { CSketchPlane } from '@/lib/csketch-api'
import { planeMeshPosition, planeMeshQuaternion, pointToPlaneUV, uvToWorld } from './sketch-plane-uv'

/** The capture plane's size (mm): big enough that the cursor never runs off it. */
const PLANE_SIZE = 20000
/** Snap / pick reach on screen, in pixels. */
const REACH_PX = 9

/** Colours, by theme: ink for geometry, amber for the selection and snaps. */
function palette(dark: boolean) {
  return {
    ink: dark ? '#e6e8eb' : '#1d2430',
    construction: dark ? '#7f8ea3' : '#6b7a90',
    selected: '#F2A516',
    ghost: dark ? '#9fd0ff' : '#2f7fd8',
    snap: '#F2A516',
    bad: '#e5484d',
    cross: dark ? '#c084fc' : '#9333ea',
    tint: dark ? '#3b82f6' : '#2563eb',
  }
}

/** World size of one screen pixel at `p` -- reach scales with zoom. */
function worldPerPixel(camera: THREE.Camera, p: THREE.Vector3, heightPx: number): number {
  if ((camera as THREE.OrthographicCamera).isOrthographicCamera) {
    const c = camera as THREE.OrthographicCamera
    return (c.top - c.bottom) / c.zoom / heightPx
  }
  const c = camera as THREE.PerspectiveCamera
  const d = c.position.distanceTo(p)
  return (2 * d * Math.tan((c.fov * Math.PI) / 360)) / heightPx
}

function lift(pts: Vec2[], plane: CSketchPlane): THREE.Vector3[] {
  return pts.map((p) => uvToWorld(p, plane))
}

export function SketchBenchCanvas() {
  const sketchId = useSketchBench((s) => s.sketchId)
  if (!sketchId) return null
  return <BenchSurface />
}

function BenchSurface() {
  const plane = useSketchBench((s) => s.plane)
  const summary = useSketchBench((s) => s.summary)
  const selection = useSketchBench((s) => s.selection)
  const ghost = useSketchBench((s) => s.ghost)
  const dark = useThemeStore((s) => s.theme) === 'dark'
  const colors = palette(dark)
  const { camera, size } = useThree()
  const reachRef = useRef(1)

  const paths = useMemo(() => (summary ? entityPaths(summary) : []), [summary])
  const cross = useMemo(() => (summary ? crossings(summary) : []), [summary])
  const overlapping = useMemo(() => (summary ? overlaps(summary).flat() : []), [summary])
  const targets = useMemo(
    () => [...(summary ? snapTargets(summary) : []), ...cross.map((c) => ({ at: c.at, kind: 'midpoint' as const }))],
    [summary, cross],
  )
  // Marker size follows zoom (a few pixels on screen), updated when the
  // zoom changes enough to show -- not on every mouse move.
  const [mark, setMark] = useState(0.5)
  // A sketch point being dragged (select tool): where it is now.
  const [drag, setDrag] = useState<{ id: string; at: Vec2 } | null>(null)
  const dragRef = useRef<{ id: string; at: Vec2 } | null>(null)
  const known = useMemo(() => new Map<string, Vec2>((summary?.points ?? []).map((p) => [p.id, [p.x, p.y]])), [summary])

  const toUv = useCallback(
    (e: ThreeEvent<PointerEvent>): Vec2 => {
      reachRef.current = REACH_PX * worldPerPixel(camera, e.point, size.height)
      return pointToPlaneUV(e.point, plane)
    },
    [camera, size.height, plane],
  )

  const onMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      const uv = toUv(e)
      const m = reachRef.current * 0.5
      if (Math.abs(m - mark) > mark * 0.15) setMark(m)
      const grid = useSketchBench.getState().grid
      const snapped = snap(uv, targets, { reach: reachRef.current, grid })
      useBenchCursor.getState().set(snapped)
      if (dragRef.current) {
        dragRef.current = { id: dragRef.current.id, at: snapped.at }
        setDrag(dragRef.current)
      }
    },
    [toUv, targets, mark],
  )

  // Click vs. drag: orbiting the view over the plane must not place points.
  const onDown = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (e.nativeEvent.button !== 0) return
      const x0 = e.nativeEvent.clientX
      const y0 = e.nativeEvent.clientY
      const uv = toUv(e)
      const additive = e.nativeEvent.shiftKey || e.nativeEvent.ctrlKey || e.nativeEvent.metaKey
      const start = useSketchBench.getState()
      // Select tool on a point: drag it; the solver moves what it drives.
      const under = start.summary ? pickEntity(uv, paths, start.summary, reachRef.current) : null
      if (start.tool === 'select' && under && 'Point' in under) {
        dragRef.current = { id: under.Point, at: uv }
        useSceneStore.getState().setGizmoDragging(true)
      }
      const onUp = (up: PointerEvent) => {
        window.removeEventListener('pointerup', onUp)
        const dragged = dragRef.current
        dragRef.current = null
        setDrag(null)
        useSceneStore.getState().setGizmoDragging(false)
        if (up.button !== 0) return
        const dx = up.clientX - x0
        const dy = up.clientY - y0
        if (dx * dx + dy * dy > 16) {
          if (dragged) void useSketchBench.getState().dragPoint(dragged.id, dragged.at)
          return
        }
        const bench = useSketchBench.getState()
        const s = bench.summary
        const hit = s ? pickEntity(uv, paths, s, reachRef.current) : null
        if (bench.tool === 'select' || bench.tool === 'dimension') {
          bench.selectConstraint(null)
          bench.select(hit, additive || bench.tool === 'dimension')
          return
        }
        if (PICK_TOOLS.has(bench.tool)) {
          const at = useBenchCursor.getState().at
          void bench.pick(at?.at ?? uv, hit, at?.pointId)
          return
        }
        const snapped = useBenchCursor.getState().at ?? snap(uv, targets, { reach: reachRef.current, grid: bench.grid })
        bench.click({ at: snapped.at, ...(snapped.pointId ? { pointId: snapped.pointId } : {}) })
      }
      window.addEventListener('pointerup', onUp)
    },
    [toUv, paths, targets],
  )

  const onDouble = useCallback(() => useSketchBench.getState().endChain(), [])
  const onOut = useCallback(() => useBenchCursor.getState().set(null), [])

  const isSelected = (ref: Parameters<typeof sameRef>[0]) => selection.some((r) => sameRef(r, ref))
  const ghostPaths = useMemo(() => planPaths(ghost, known), [ghost, known])

  return (
    <group name="sketch-bench">
      <mesh
        position={planeMeshPosition(plane)}
        quaternion={planeMeshQuaternion(plane)}
        onPointerMove={onMove}
        onPointerDown={onDown}
        onPointerOut={onOut}
        onDoubleClick={onDouble}
        renderOrder={-1}
      >
        <planeGeometry args={[PLANE_SIZE, PLANE_SIZE]} />
        <meshBasicMaterial color={colors.tint} opacity={0.04} transparent side={THREE.DoubleSide} depthWrite={false} />
      </mesh>

      {paths.map((p) => {
        const sel = isSelected(p.ref)
        const bad = overlapping.some((r) => sameRef(r, p.ref))
        return (
          <Line
            key={JSON.stringify(p.ref)}
            points={lift(p.pts, plane)}
            color={sel ? colors.selected : bad ? colors.bad : p.construction ? colors.construction : colors.ink}
            lineWidth={sel ? 3 : 2}
            dashed={p.construction}
            dashSize={1.2}
            gapSize={0.8}
            depthTest={false}
            renderOrder={5}
          />
        )
      })}

      {ghostPaths.map((pts, i) => (
        <Line key={`ghost-${i}`} points={lift(pts, plane)} color={colors.ink} lineWidth={2} depthTest={false} renderOrder={5} />
      ))}

      {cross.map((c, i) => {
        const r = mark
        return (
          <Line
            key={`x-${i}`}
            points={lift(
              [
                [c.at[0] - r, c.at[1] - r],
                [c.at[0] + r, c.at[1] + r],
                [c.at[0], c.at[1]],
                [c.at[0] - r, c.at[1] + r],
                [c.at[0] + r, c.at[1] - r],
              ],
              plane,
            )}
            color={colors.cross}
            lineWidth={1.5}
            depthTest={false}
            renderOrder={6}
          />
        )
      })}

      {drag && (
        <Line
          points={lift(
            Array.from({ length: 13 }, (_, i) => {
              const t = (i / 12) * 2 * Math.PI
              const r = mark * 1.2
              return [drag.at[0] + r * Math.cos(t), drag.at[1] + r * Math.sin(t)] as Vec2
            }),
            plane,
          )}
          color={colors.selected}
          lineWidth={2.5}
          depthTest={false}
          renderOrder={9}
        />
      )}

      <BenchPoints colors={colors} />
      <ConstraintMarks />
      <RubberBand colors={colors} known={known} />
    </group>
  )
}

/** Sketch points as dots, constant on screen; fixed points filled amber. */
function BenchPoints({ colors }: { colors: ReturnType<typeof palette> }) {
  const plane = useSketchBench((s) => s.plane)
  const points = useSketchBench((s) => s.summary?.points)
  const selection = useSketchBench((s) => s.selection)
  const geom = useMemo(() => {
    const g = new THREE.BufferGeometry()
    const pts = points ?? []
    const pos = new Float32Array(pts.length * 3)
    const col = new Float32Array(pts.length * 3)
    const ink = new THREE.Color(colors.ink)
    const amber = new THREE.Color(colors.selected)
    pts.forEach((p, i) => {
      const w = uvToWorld([p.x, p.y], plane)
      pos.set([w.x, w.y, w.z], i * 3)
      const sel = selection.some((r) => 'Point' in r && r.Point === p.id)
      const c = sel || p.is_fixed ? amber : ink
      col.set([c.r, c.g, c.b], i * 3)
    })
    g.setAttribute('position', new THREE.BufferAttribute(pos, 3))
    g.setAttribute('color', new THREE.BufferAttribute(col, 3))
    return g
  }, [points, plane, selection, colors.ink, colors.selected])
  if (!points || points.length === 0) return null
  return (
    <points geometry={geom} renderOrder={6}>
      <pointsMaterial size={7} sizeAttenuation={false} vertexColors depthTest={false} transparent />
    </points>
  )
}

/** What the tool would build from the clicks so far plus the cursor, and the snap marker. */
function RubberBand({ colors, known }: { colors: ReturnType<typeof palette>; known: Map<string, Vec2> }) {
  const plane = useSketchBench((s) => s.plane)
  const tool = useSketchBench((s) => s.tool)
  const clicks = useSketchBench((s) => s.clicks)
  const sides = useSketchBench((s) => s.sides)
  const cursor = useBenchCursor((s) => s.at)
  const band = useMemo(() => {
    if (!cursor || tool === 'select' || clicks.length === 0) return []
    const all = [...clicks, { at: cursor.at, ...(cursor.pointId ? { pointId: cursor.pointId } : {}) }]
    const need = TOOL_CLICKS[tool]
    // Two-step tools whose last click is a direction (arcs, slot) preview
    // the first stretch as a straight line until the last click is due.
    if (typeof need === 'number' && need === 3 && all.length === 2) return [[all[0].at, all[1].at]]
    const kind = tool === 'line' ? 'line' : tool
    return planPaths(planShape(kind, all, { sides }), known)
  }, [cursor, tool, clicks, sides, known])
  const marker = useMemo(() => {
    if (!cursor || cursor.kind === 'none' || cursor.kind === 'grid') return null
    const r = 0.8
    return Array.from({ length: 17 }, (_, i) => {
      const t = (i / 16) * 2 * Math.PI
      return [cursor.at[0] + r * Math.cos(t), cursor.at[1] + r * Math.sin(t)] as Vec2
    })
  }, [cursor])
  return (
    <>
      {band.map((pts, i) => (
        <Line key={`band-${i}`} points={lift(pts, plane)} color={colors.ghost} lineWidth={1.5} dashed dashSize={0.8} gapSize={0.5} depthTest={false} renderOrder={7} />
      ))}
      {marker && <Line points={lift(marker, plane)} color={colors.snap} lineWidth={2} depthTest={false} renderOrder={8} />}
    </>
  )
}

/** Constraint glyphs and dimension values ON the sketch, as established
 *  sketchers show them: click a glyph to pick it (Delete removes it);
 *  double-click a dimension to change it. */
function ConstraintMarks() {
  const plane = useSketchBench((s) => s.plane)
  const summary = useSketchBench((s) => s.summary)
  const constraints = useSketchBench((s) => s.constraints)
  const picked = useSketchBench((s) => s.selectedConstraint)
  const marks = useMemo(() => {
    if (!summary) return []
    // Several marks on one entity fan out instead of stacking.
    const seen = new Map<string, number>()
    return constraints.flatMap((c) => {
      if (!showsMark(c.entities, summary)) return []
      const at = anchorOf(c.entities, summary)
      if (!at) return []
      const key = `${at[0].toFixed(3)},${at[1].toFixed(3)}`
      const n = seen.get(key) ?? 0
      seen.set(key, n + 1)
      const t = c.constraint_type
      if ('Geometric' in t) {
        const name = typeof t.Geometric === 'string' ? t.Geometric : 'IntersectionAngle'
        return [{ id: c.id, at, n, text: CONSTRAINT_GLYPH[name] ?? name.slice(0, 2), title: name, dim: null as number | null, unit: '' }]
      }
      const [kind, raw] = Object.entries(t.Dimensional)[0] as [string, number]
      if (typeof raw !== 'number') return []
      const deg = kind === 'Angle'
      const value = deg ? (raw * 180) / Math.PI : raw
      const prefix = kind === 'Diameter' ? 'Ø' : kind === 'Radius' ? 'R' : ''
      return [{ id: c.id, at, n, text: `${prefix}${Number(value.toFixed(3))}${deg ? '°' : ''}`, title: kind, dim: value, unit: deg ? '°' : 'mm' }]
    })
  }, [summary, constraints])
  return (
    <>
      {marks.map((m) => (
        <Html key={m.id} position={uvToWorld(m.at, plane)} center zIndexRange={[20, 0]} style={{ pointerEvents: 'none' }}>
          <button
            type="button"
            title={m.dim === null ? `${m.title} -- click, then Delete removes it` : `${m.title} -- double-click to change`}
            onClick={(e) => {
              e.stopPropagation()
              useSketchBench.getState().selectConstraint(m.id)
            }}
            onDoubleClick={(e) => {
              e.stopPropagation()
              if (m.dim === null) return
              useSketchBench.getState().setEditing({ id: m.id, kind: m.title, value: m.dim, unit: m.unit === '°' ? 'deg' : 'mm' })
            }}
            style={{ pointerEvents: 'auto', transform: `translate(${10 + m.n * 14}px, ${-10 - m.n * 2}px)` }}
            className={cn(
              'cad-focus whitespace-nowrap rounded border px-1 text-[10px] leading-4 shadow-sm',
              m.dim === null ? 'min-w-4 bg-card/90 text-muted-foreground' : 'bg-card font-medium text-foreground',
              picked === m.id ? 'border-[var(--cad-selected)] text-foreground' : 'border-border',
            )}
          >
            {m.text}
          </button>
        </Html>
      ))}
    </>
  )
}
