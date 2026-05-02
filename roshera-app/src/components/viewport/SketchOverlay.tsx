/**
 * In-canvas component active only while `sketch.active` is true.
 * Responsibilities:
 *   1. Render a large transparent plane oriented to the current
 *      sketch plane that catches pointer events for click-to-place.
 *   2. Maintain the live cursor position on the plane (`hover`).
 *   3. Render the in-progress sketch as 3D lines + ghost rubber-band
 *      to the cursor.
 *   4. Overlay live dimension labels (segment lengths, rectangle
 *      width/height, circle radius) using drei's `<Html>`. When the
 *      `measure` toggle is on, also annotate angles between adjacent
 *      polyline segments and the total perimeter / enclosed area.
 *
 * Coordinate convention matches `lib/sketch-extrude.ts`:
 *   xy: (u, v) → (u, v, 0), normal +Z
 *   xz: (u, v) → (u, 0, v), normal +Y
 *   yz: (u, v) → (0, u, v), normal +X
 */

import { useCallback, useMemo } from 'react'
import { Html, Line } from '@react-three/drei'
import type { ThreeEvent } from '@react-three/fiber'
import * as THREE from 'three'
import {
  useSceneStore,
  type SketchPlane,
  type SketchTool,
} from '@/stores/scene-store'

const PLANE_SIZE = 400 // world units; large enough that the user
                       // cannot click off-plane in normal use.

/** Project a world-space `THREE.Vector3` onto the chosen sketch plane. */
function pointToPlaneUV(
  p: THREE.Vector3,
  plane: SketchPlane,
): [number, number] {
  switch (plane) {
    case 'xy': return [p.x, p.y]
    case 'xz': return [p.x, p.z]
    case 'yz': return [p.y, p.z]
  }
}

/** Inverse of `pointToPlaneUV` — lift a (u, v) onto the world plane. */
function uvToWorld(
  point: [number, number],
  plane: SketchPlane,
): THREE.Vector3 {
  const [u, v] = point
  switch (plane) {
    case 'xy': return new THREE.Vector3(u, v, 0)
    case 'xz': return new THREE.Vector3(u, 0, v)
    case 'yz': return new THREE.Vector3(0, u, v)
  }
}

/** Euler rotation that aligns the +Z plane mesh with the sketch plane. */
function planeRotation(plane: SketchPlane): [number, number, number] {
  switch (plane) {
    case 'xy': return [0, 0, 0]                 // already +Z up
    case 'xz': return [-Math.PI / 2, 0, 0]      // rotate to ground
    case 'yz': return [0, Math.PI / 2, 0]       // rotate to side
  }
}

/** Snap a value to the nearest `step` (no snap when step ≤ 0). */
function snap(value: number, step: number): number {
  if (step <= 0) return value
  return Math.round(value / step) * step
}

/** Pretty-print a length: 4 sig figs, mm suffix, no trailing zeros. */
function fmtLen(value: number): string {
  const v = Math.abs(value)
  let s: string
  if (v >= 100) s = value.toFixed(1)
  else if (v >= 10) s = value.toFixed(2)
  else s = value.toFixed(3)
  // strip trailing zeros / trailing dot
  s = s.replace(/(\.\d*?)0+$/, '$1').replace(/\.$/, '')
  return `${s}`
}

function fmtAngle(rad: number): string {
  const deg = (rad * 180) / Math.PI
  return `${deg.toFixed(1)}°`
}

export function SketchOverlay() {
  const sketch = useSceneStore((s) => s.sketch)
  const addSketchPoint = useSceneStore((s) => s.addSketchPoint)
  const setSketchHover = useSceneStore((s) => s.setSketchHover)
  const showMeasure = sketch.measure
  const snapStep = sketch.snapStep

  const handlePointerMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (!sketch.active) return
      e.stopPropagation()
      const uv = pointToPlaneUV(e.point, sketch.plane)
      setSketchHover([snap(uv[0], snapStep), snap(uv[1], snapStep)])
    },
    [sketch.active, sketch.plane, setSketchHover, snapStep],
  )

  const handlePointerOut = useCallback(() => {
    if (!sketch.active) return
    setSketchHover(null)
  }, [sketch.active, setSketchHover])

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      if (!sketch.active) return
      // Suppress the orbit-controls click and any object pickers below.
      e.stopPropagation()
      const native = e.nativeEvent
      // Only react to left-click; right-click is intentionally
      // un-routed so OrbitControls / future menus stay free.
      if (native.button !== 0) return
      const uv = pointToPlaneUV(e.point, sketch.plane)
      const snapped: [number, number] = [
        snap(uv[0], snapStep),
        snap(uv[1], snapStep),
      ]

      // Tool-specific termination: rectangle = 2 corners, circle =
      // center + radius point. For polyline the panel's "Finish"
      // button (or Enter) closes the loop.
      const tool: SketchTool = sketch.tool
      if (tool === 'rectangle' && sketch.points.length >= 2) return
      if (tool === 'circle' && sketch.points.length >= 2) return

      addSketchPoint(snapped)
    },
    [sketch.active, sketch.plane, sketch.points.length, sketch.tool, addSketchPoint, snapStep],
  )

  if (!sketch.active) return null

  // Visible bordered square at the working size, sitting on the
  // capture plane. Helps the user see exactly which plane is active
  // and where its origin lies.
  const PLANE_VIS = 60
  const borderPts: Array<[number, number]> = [
    [-PLANE_VIS / 2, -PLANE_VIS / 2],
    [ PLANE_VIS / 2, -PLANE_VIS / 2],
    [ PLANE_VIS / 2,  PLANE_VIS / 2],
    [-PLANE_VIS / 2,  PLANE_VIS / 2],
    [-PLANE_VIS / 2, -PLANE_VIS / 2],
  ]
  const borderWorld = borderPts.map((p) => uvToWorld(p, sketch.plane))

  return (
    <group name="sketch-overlay">
      {/* Capture plane — large, faintly tinted, double-sided. */}
      <mesh
        rotation={planeRotation(sketch.plane)}
        onPointerMove={handlePointerMove}
        onPointerOut={handlePointerOut}
        onClick={handleClick}
      >
        <planeGeometry args={[PLANE_SIZE, PLANE_SIZE]} />
        <meshBasicMaterial
          color="#3498db"
          opacity={0.06}
          transparent
          side={THREE.DoubleSide}
          depthWrite={false}
        />
      </mesh>

      {/* Bordered working area at the chosen plane. */}
      <Line
        points={borderWorld}
        color="#3498db"
        lineWidth={1}
        dashed
        dashSize={0.6}
        gapSize={0.4}
        opacity={0.6}
        transparent
      />

      <SketchPreview showMeasure={showMeasure} />
    </group>
  )
}

// ─── Preview + dimensions ────────────────────────────────────────────

interface PreviewProps {
  showMeasure: boolean
}

function SketchPreview({ showMeasure }: PreviewProps) {
  const { tool, plane, points, hover } = useSceneStore((s) => s.sketch)

  // Lift confirmed + hover to world space once.
  const confirmedWorld = useMemo(
    () => points.map((p) => uvToWorld(p, plane)),
    [points, plane],
  )
  const hoverWorld = useMemo(
    () => (hover ? uvToWorld(hover, plane) : null),
    [hover, plane],
  )

  if (tool === 'polyline') {
    return (
      <PolylinePreview
        plane={plane}
        confirmedUV={points}
        hoverUV={hover}
        confirmedWorld={confirmedWorld}
        hoverWorld={hoverWorld}
        showMeasure={showMeasure}
      />
    )
  }

  if (tool === 'rectangle') {
    return (
      <RectanglePreview
        plane={plane}
        confirmedUV={points}
        hoverUV={hover}
      />
    )
  }

  return (
    <CirclePreview
      plane={plane}
      confirmedUV={points}
      hoverUV={hover}
    />
  )
}

// ─── Polyline preview ────────────────────────────────────────────────

interface PolylineProps {
  plane: SketchPlane
  confirmedUV: Array<[number, number]>
  hoverUV: [number, number] | null
  confirmedWorld: THREE.Vector3[]
  hoverWorld: THREE.Vector3 | null
  showMeasure: boolean
}

function PolylinePreview({
  plane,
  confirmedUV,
  hoverUV,
  confirmedWorld,
  hoverWorld,
  showMeasure,
}: PolylineProps) {
  const segmentLine = useMemo(() => {
    if (confirmedWorld.length === 0) return null
    const pts = [...confirmedWorld]
    if (hoverWorld) pts.push(hoverWorld)
    return pts.length >= 2 ? pts : null
  }, [confirmedWorld, hoverWorld])

  // Closing chord visualisation when we have ≥ 3 confirmed points.
  const closingLine = useMemo(() => {
    if (confirmedWorld.length < 3) return null
    return [confirmedWorld[confirmedWorld.length - 1], confirmedWorld[0]]
  }, [confirmedWorld])

  // Per-segment dimension labels at midpoints.
  const labels = useMemo(() => {
    const items: Array<{ pos: THREE.Vector3; text: string }> = []
    const allUV = hoverUV ? [...confirmedUV, hoverUV] : confirmedUV
    for (let i = 0; i < allUV.length - 1; i++) {
      const a = allUV[i]
      const b = allUV[i + 1]
      const len = Math.hypot(b[0] - a[0], b[1] - a[1])
      if (len < 1e-6) continue
      const mid = uvToWorld([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2], plane)
      items.push({ pos: mid, text: fmtLen(len) })
    }
    if (showMeasure && confirmedUV.length >= 3) {
      // Closing chord length
      const a = confirmedUV[confirmedUV.length - 1]
      const b = confirmedUV[0]
      const len = Math.hypot(b[0] - a[0], b[1] - a[1])
      const mid = uvToWorld([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2], plane)
      items.push({ pos: mid, text: `${fmtLen(len)} (close)` })
    }
    return items
  }, [confirmedUV, hoverUV, plane, showMeasure])

  // Angle annotations at each interior vertex (between segment i-1 and i).
  const angleLabels = useMemo(() => {
    if (!showMeasure || confirmedUV.length < 3) return []
    const items: Array<{ pos: THREE.Vector3; text: string }> = []
    for (let i = 1; i < confirmedUV.length - 1; i++) {
      const prev = confirmedUV[i - 1]
      const cur = confirmedUV[i]
      const next = confirmedUV[i + 1]
      const ax = prev[0] - cur[0]
      const ay = prev[1] - cur[1]
      const bx = next[0] - cur[0]
      const by = next[1] - cur[1]
      const la = Math.hypot(ax, ay)
      const lb = Math.hypot(bx, by)
      if (la < 1e-6 || lb < 1e-6) continue
      const cos = (ax * bx + ay * by) / (la * lb)
      const ang = Math.acos(Math.max(-1, Math.min(1, cos)))
      items.push({ pos: uvToWorld(cur, plane), text: fmtAngle(ang) })
    }
    return items
  }, [confirmedUV, plane, showMeasure])

  return (
    <>
      {segmentLine && (
        <Line
          points={segmentLine}
          color="#3498db"
          lineWidth={2}
          dashed={false}
        />
      )}
      {closingLine && (
        <Line
          points={closingLine}
          color="#3498db"
          lineWidth={1}
          dashed
          dashSize={0.3}
          gapSize={0.2}
          opacity={0.5}
          transparent
        />
      )}
      {confirmedWorld.map((pt, idx) => (
        <PointMarker key={idx} position={pt} active={idx === 0} />
      ))}
      {hoverWorld && <PointMarker position={hoverWorld} ghost />}

      {labels.map((l, i) => (
        <DimLabel key={`len-${i}`} position={l.pos} text={l.text} />
      ))}
      {angleLabels.map((l, i) => (
        <DimLabel key={`ang-${i}`} position={l.pos} text={l.text} variant="angle" />
      ))}
    </>
  )
}

// ─── Rectangle preview ───────────────────────────────────────────────

interface RectangleProps {
  plane: SketchPlane
  confirmedUV: Array<[number, number]>
  hoverUV: [number, number] | null
}

function RectanglePreview({ plane, confirmedUV, hoverUV }: RectangleProps) {
  const corners = useMemo(() => {
    if (confirmedUV.length === 0) return null
    if (confirmedUV.length === 1 && !hoverUV) return null
    const a = confirmedUV[0]
    const b = confirmedUV.length >= 2 ? confirmedUV[1] : (hoverUV as [number, number])
    return {
      a,
      b,
      width: Math.abs(b[0] - a[0]),
      height: Math.abs(b[1] - a[1]),
    }
  }, [confirmedUV, hoverUV])

  if (!corners) {
    // First click pending — render the cursor ghost only.
    if (hoverUV) {
      return <PointMarker position={uvToWorld(hoverUV, plane)} ghost />
    }
    return null
  }

  const { a, b, width, height } = corners
  const x0 = Math.min(a[0], b[0])
  const x1 = Math.max(a[0], b[0])
  const y0 = Math.min(a[1], b[1])
  const y1 = Math.max(a[1], b[1])

  const loop = [
    uvToWorld([x0, y0], plane),
    uvToWorld([x1, y0], plane),
    uvToWorld([x1, y1], plane),
    uvToWorld([x0, y1], plane),
    uvToWorld([x0, y0], plane), // closed
  ]

  const widthLabelPos = uvToWorld([(x0 + x1) / 2, y0], plane)
  const heightLabelPos = uvToWorld([x1, (y0 + y1) / 2], plane)

  return (
    <>
      <Line points={loop} color="#3498db" lineWidth={2} />
      <PointMarker position={uvToWorld(a, plane)} active />
      <PointMarker position={uvToWorld(b, plane)} ghost={confirmedUV.length < 2} />
      <DimLabel position={widthLabelPos} text={fmtLen(width)} />
      <DimLabel position={heightLabelPos} text={fmtLen(height)} />
    </>
  )
}

// ─── Circle preview ──────────────────────────────────────────────────

interface CircleProps {
  plane: SketchPlane
  confirmedUV: Array<[number, number]>
  hoverUV: [number, number] | null
}

function CirclePreview({ plane, confirmedUV, hoverUV }: CircleProps) {
  const data = useMemo(() => {
    if (confirmedUV.length === 0) return null
    const center = confirmedUV[0]
    const edge =
      confirmedUV.length >= 2
        ? confirmedUV[1]
        : hoverUV ?? center
    const r = Math.hypot(edge[0] - center[0], edge[1] - center[1])
    return { center, edge, r }
  }, [confirmedUV, hoverUV])

  if (!data) {
    if (hoverUV) {
      return <PointMarker position={uvToWorld(hoverUV, plane)} ghost />
    }
    return null
  }

  const { center, edge, r } = data
  const N = 64
  const ring: THREE.Vector3[] = []
  for (let i = 0; i <= N; i++) {
    const t = (i / N) * Math.PI * 2
    ring.push(
      uvToWorld([center[0] + r * Math.cos(t), center[1] + r * Math.sin(t)], plane),
    )
  }

  const radiusLine = [uvToWorld(center, plane), uvToWorld(edge, plane)]
  const labelPos = uvToWorld(
    [(center[0] + edge[0]) / 2, (center[1] + edge[1]) / 2],
    plane,
  )

  return (
    <>
      {r > 1e-6 && <Line points={ring} color="#3498db" lineWidth={2} />}
      <Line
        points={radiusLine}
        color="#3498db"
        lineWidth={1}
        dashed
        dashSize={0.3}
        gapSize={0.2}
        opacity={0.6}
        transparent
      />
      <PointMarker position={uvToWorld(center, plane)} active />
      <PointMarker
        position={uvToWorld(edge, plane)}
        ghost={confirmedUV.length < 2}
      />
      <DimLabel position={labelPos} text={`R ${fmtLen(r)}`} />
    </>
  )
}

// ─── Helpers ─────────────────────────────────────────────────────────

interface PointMarkerProps {
  position: THREE.Vector3
  active?: boolean
  ghost?: boolean
}

function PointMarker({ position, active, ghost }: PointMarkerProps) {
  const color = active ? '#e74c3c' : '#3498db'
  return (
    <mesh position={position}>
      <sphereGeometry args={[0.12, 12, 12]} />
      <meshBasicMaterial
        color={color}
        opacity={ghost ? 0.4 : 1}
        transparent={ghost}
        depthTest={false}
      />
    </mesh>
  )
}

interface DimLabelProps {
  position: THREE.Vector3
  text: string
  variant?: 'length' | 'angle'
}

function DimLabel({ position, text, variant = 'length' }: DimLabelProps) {
  const tone =
    variant === 'angle' ? 'border-amber-400/40 text-amber-300' : 'border-border/60 text-foreground'
  return (
    <Html
      position={position}
      center
      pointerEvents="none"
      zIndexRange={[100, 0]}
    >
      <div
        className={`pointer-events-none px-1.5 py-0.5 text-[10px] font-mono uppercase tracking-wider bg-background/80 border ${tone} backdrop-blur-sm whitespace-nowrap`}
      >
        {text}
      </div>
    </Html>
  )
}
