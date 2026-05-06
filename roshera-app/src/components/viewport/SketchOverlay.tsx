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
 * Coordinate convention matches `lib/sketch-extrude.ts` and the
 * backend `SketchPlane::lift`:
 *   xy: (u, v) → (u, v, 0), normal +Z
 *   xz: (u, v) → (u, 0, v), normal +Y
 *   yz: (u, v) → (0, u, v), normal +X
 *   custom: (u, v) → origin + u·u_axis + v·v_axis, normal = u × v
 */

import { useCallback, useMemo, useState, useEffect, useRef } from 'react'
import { Html, Line } from '@react-three/drei'
import type { ThreeEvent } from '@react-three/fiber'
import * as THREE from 'three'
import {
  isStandardPlane,
  useSceneStore,
  type SketchPlane,
  type SketchTool,
  type ServerSketchShape,
  type SnapTarget,
  type InferenceAxis,
} from '@/stores/scene-store'
import { buildProfile2D } from '@/lib/sketch-extrude'

const PLANE_SIZE = 400 // world units; large enough that the user
                       // cannot click off-plane in normal use.

/** Project a world-space `THREE.Vector3` onto the chosen sketch plane. */
function pointToPlaneUV(
  p: THREE.Vector3,
  plane: SketchPlane,
): [number, number] {
  if (isStandardPlane(plane)) {
    switch (plane) {
      case 'xy': return [p.x, p.y]
      case 'xz': return [p.x, p.z]
      case 'yz': return [p.y, p.z]
    }
  }
  // Custom face-anchored plane: subtract origin then dot against the
  // in-plane basis. Backend's SketchPlane::from_face guarantees u_axis
  // and v_axis are orthonormal, so this gives the same (u, v) the
  // backend's lift() inverts.
  const origin = new THREE.Vector3(...plane.origin)
  const uAxis = new THREE.Vector3(...plane.u_axis)
  const vAxis = new THREE.Vector3(...plane.v_axis)
  const d = p.clone().sub(origin)
  return [d.dot(uAxis), d.dot(vAxis)]
}

/** Inverse of `pointToPlaneUV` — lift a (u, v) onto the world plane. */
function uvToWorld(
  point: [number, number],
  plane: SketchPlane,
): THREE.Vector3 {
  const [u, v] = point
  if (isStandardPlane(plane)) {
    switch (plane) {
      case 'xy': return new THREE.Vector3(u, v, 0)
      case 'xz': return new THREE.Vector3(u, 0, v)
      case 'yz': return new THREE.Vector3(0, u, v)
    }
  }
  // origin + u·u_axis + v·v_axis — exactly mirrors the backend's
  // SketchPlane::lift so frontend ghost geometry agrees with what the
  // server records.
  return new THREE.Vector3(...plane.origin)
    .add(new THREE.Vector3(...plane.u_axis).multiplyScalar(u))
    .add(new THREE.Vector3(...plane.v_axis).multiplyScalar(v))
}

/** World position for the capture-plane mesh (origin for custom planes). */
function planeMeshPosition(plane: SketchPlane): THREE.Vector3 {
  if (isStandardPlane(plane)) return new THREE.Vector3(0, 0, 0)
  return new THREE.Vector3(...plane.origin)
}

/**
 * Quaternion that orients the default `<planeGeometry>` (whose local
 * +X is right, +Y is up, +Z is normal) to lie on the sketch plane.
 * For standard planes we replay the same Euler rotations the previous
 * implementation used so the visual result is unchanged. For custom
 * planes we build an orthonormal basis directly from u_axis / v_axis
 * and extract the quaternion from the basis matrix.
 */
function planeMeshQuaternion(plane: SketchPlane): THREE.Quaternion {
  const q = new THREE.Quaternion()
  if (isStandardPlane(plane)) {
    const euler = (() => {
      switch (plane) {
        case 'xy': return new THREE.Euler(0, 0, 0)
        case 'xz': return new THREE.Euler(-Math.PI / 2, 0, 0)
        case 'yz': return new THREE.Euler(0, Math.PI / 2, 0)
      }
    })()
    q.setFromEuler(euler)
    return q
  }
  const uAxis = new THREE.Vector3(...plane.u_axis)
  const vAxis = new THREE.Vector3(...plane.v_axis)
  const nAxis = new THREE.Vector3().crossVectors(uAxis, vAxis)
  const m = new THREE.Matrix4().makeBasis(uAxis, vAxis, nAxis)
  q.setFromRotationMatrix(m)
  return q
}

/**
 * Origin/axis/grid snap stage of the snap pipeline. Magnetically
 * attracts the cursor to the plane's origin (0, 0) and to its two
 * axes (u = 0, v = 0), then rounds to the configured grid step. The
 * axis-snap radius is half a grid step (or 0.25 mm when grid snap is
 * off) so the attractor feels like a "notch" rather than fighting
 * the grid.
 */
function snapAxisGrid(uv: [number, number], step: number): [number, number] {
  let [u, v] = uv
  const axisRadius = step > 0 ? step * 0.5 : 0.25
  if (Math.abs(u) < axisRadius) u = 0
  if (Math.abs(v) < axisRadius) v = 0
  if (step > 0) {
    u = Math.round(u / step) * step
    v = Math.round(v / step) * step
  }
  return [u, v]
}

/**
 * Collect the magnetic snap targets exposed by every committed shape
 * (every entry of `sketch.shapes` other than the active last one when
 * `excludeActive=true`):
 *   - Polyline: each vertex + each segment midpoint.
 *   - Rectangle: 4 corners + 4 edge midpoints + center.
 *   - Circle: center + 4 quadrant points (top/bottom/left/right of
 *     the ring).
 *
 * Pure function of the input shapes; no side effects.
 */
function collectSnapTargets(
  shapes: ServerSketchShape[],
  excludeActive: boolean,
): SnapTarget[] {
  const out: SnapTarget[] = []
  const upper = excludeActive ? Math.max(0, shapes.length - 1) : shapes.length
  for (let i = 0; i < upper; i++) {
    const s = shapes[i]
    if (!s) continue
    const pts = s.points
    switch (s.tool) {
      case 'polyline': {
        for (let j = 0; j < pts.length; j++) {
          out.push({ uv: pts[j], kind: 'vertex' })
        }
        for (let j = 0; j < pts.length - 1; j++) {
          const a = pts[j]
          const b = pts[j + 1]
          out.push({
            uv: [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2],
            kind: 'midpoint',
          })
        }
        break
      }
      case 'rectangle': {
        if (pts.length < 2) {
          if (pts.length === 1) out.push({ uv: pts[0], kind: 'vertex' })
          break
        }
        const a = pts[0]
        const b = pts[1]
        const x0 = Math.min(a[0], b[0])
        const x1 = Math.max(a[0], b[0])
        const y0 = Math.min(a[1], b[1])
        const y1 = Math.max(a[1], b[1])
        if (x1 - x0 < 1e-6 || y1 - y0 < 1e-6) {
          out.push({ uv: a, kind: 'vertex' })
          break
        }
        // 4 corners
        out.push({ uv: [x0, y0], kind: 'vertex' })
        out.push({ uv: [x1, y0], kind: 'vertex' })
        out.push({ uv: [x1, y1], kind: 'vertex' })
        out.push({ uv: [x0, y1], kind: 'vertex' })
        // 4 edge midpoints
        out.push({ uv: [(x0 + x1) / 2, y0], kind: 'midpoint' })
        out.push({ uv: [(x0 + x1) / 2, y1], kind: 'midpoint' })
        out.push({ uv: [x0, (y0 + y1) / 2], kind: 'midpoint' })
        out.push({ uv: [x1, (y0 + y1) / 2], kind: 'midpoint' })
        // center
        out.push({ uv: [(x0 + x1) / 2, (y0 + y1) / 2], kind: 'center' })
        break
      }
      case 'circle': {
        if (pts.length < 2) {
          if (pts.length === 1) out.push({ uv: pts[0], kind: 'center' })
          break
        }
        const center = pts[0]
        const edge = pts[1]
        const r = Math.hypot(edge[0] - center[0], edge[1] - center[1])
        if (r < 1e-6) {
          out.push({ uv: center, kind: 'center' })
          break
        }
        out.push({ uv: center, kind: 'center' })
        out.push({ uv: [center[0] + r, center[1]], kind: 'quadrant' })
        out.push({ uv: [center[0] - r, center[1]], kind: 'quadrant' })
        out.push({ uv: [center[0], center[1] + r], kind: 'quadrant' })
        out.push({ uv: [center[0], center[1] - r], kind: 'quadrant' })
        break
      }
    }
  }
  return out
}

/**
 * Geometry-snap stage: pick the snap target nearest to the cursor
 * that lies within `radius`, returning both the snapped uv and the
 * locked target descriptor. When no target is in range, returns the
 * raw uv with `target: null`.
 */
function snapToGeometry(
  uv: [number, number],
  targets: SnapTarget[],
  radius: number,
): { uv: [number, number]; target: SnapTarget | null } {
  let best: SnapTarget | null = null
  let bestD2 = radius * radius
  for (const t of targets) {
    const dx = t.uv[0] - uv[0]
    const dy = t.uv[1] - uv[1]
    const d2 = dx * dx + dy * dy
    if (d2 < bestD2) {
      bestD2 = d2
      best = t
    }
  }
  if (best) return { uv: best.uv, target: best }
  return { uv, target: null }
}

/**
 * Inference-snap stage: when the cursor is within `angleTolRad` of
 * being perfectly horizontal or vertical relative to `anchor`, lock
 * the off-axis coordinate to the anchor's so the user can place the
 * next point on a clean H/V line through the previous one. Mimics
 * Fusion's "Horizontal" / "Vertical" inference cues.
 */
function snapToInference(
  uv: [number, number],
  anchor: [number, number],
  angleTolRad: number,
): { uv: [number, number]; axis: InferenceAxis | null } {
  const du = uv[0] - anchor[0]
  const dv = uv[1] - anchor[1]
  const adu = Math.abs(du)
  const adv = Math.abs(dv)
  if (adu < 1e-9 && adv < 1e-9) return { uv, axis: null }
  const tolTan = Math.tan(angleTolRad)
  // Close to horizontal (cursor.v ≈ anchor.v): collapse v.
  if (adu > 0 && adv / adu < tolTan) {
    return { uv: [uv[0], anchor[1]], axis: 'h' }
  }
  // Close to vertical (cursor.u ≈ anchor.u): collapse u.
  if (adv > 0 && adu / adv < tolTan) {
    return { uv: [anchor[0], uv[1]], axis: 'v' }
  }
  return { uv, axis: null }
}

/**
 * Full snap pipeline: geometry → inference → axis/grid. Geometry
 * runs first so a corner near the origin still wins over the origin
 * itself (corners carry richer information). Axis/grid runs last so
 * a snapped-to-corner cursor doesn't get nudged off by grid rounding.
 *
 * Returns the snapped uv plus the descriptors needed to render the
 * cyan ring marker (`snapTarget`) and the dashed inference line
 * (`inferenceAxis`).
 */
function snapPipeline(
  rawUv: [number, number],
  step: number,
  targets: SnapTarget[],
  anchor: [number, number] | null,
): {
  uv: [number, number]
  snapTarget: SnapTarget | null
  inferenceAxis: InferenceAxis | null
} {
  const radius = Math.max(0.4, step > 0 ? step * 0.8 : 0.4)
  const geom = snapToGeometry(rawUv, targets, radius)
  if (geom.target) {
    return { uv: geom.uv, snapTarget: geom.target, inferenceAxis: null }
  }
  if (anchor) {
    // 2° tolerance — wide enough to catch a slightly wobbly mouse,
    // narrow enough that a deliberate diagonal doesn't get hijacked.
    const inf = snapToInference(rawUv, anchor, (Math.PI / 180) * 2)
    if (inf.axis) {
      const after = snapAxisGrid(inf.uv, step)
      // Re-pin the locked coordinate after grid rounding so a 2.5 mm
      // grid doesn't round the locked-to-anchor coord away from the
      // anchor.
      if (inf.axis === 'h') after[1] = anchor[1]
      else after[0] = anchor[0]
      return { uv: after, snapTarget: null, inferenceAxis: inf.axis }
    }
  }
  return {
    uv: snapAxisGrid(rawUv, step),
    snapTarget: null,
    inferenceAxis: null,
  }
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
  const setSketchSnapState = useSceneStore((s) => s.setSketchSnapState)
  const showMeasure = sketch.measure
  const snapStep = sketch.snapStep

  // Magnetic snap targets recomputed from committed shapes only when
  // the shape list actually changes — pointermove never re-derives
  // them. The active polyline's confirmed points are also exposed so
  // the user can close a loop back to an earlier vertex.
  const snapTargets = useMemo(() => {
    const t = collectSnapTargets(sketch.shapes, true)
    if (sketch.tool === 'polyline') {
      for (const p of sketch.points) t.push({ uv: p, kind: 'vertex' as const })
    }
    return t
  }, [sketch.shapes, sketch.tool, sketch.points])

  // Inference anchor = the last confirmed point the user can build
  // off of. For polyline that's any earlier vertex. For rectangle /
  // circle the inference is only useful between the 1st and 2nd
  // click; after the 2nd click the click handler is a no-op anyway.
  const inferenceAnchor = useMemo<[number, number] | null>(() => {
    if (sketch.points.length === 0) return null
    if (sketch.tool !== 'polyline' && sketch.points.length !== 1) return null
    return sketch.points[sketch.points.length - 1]
  }, [sketch.points, sketch.tool])

  const computeSnap = useCallback(
    (rawUv: [number, number]) =>
      snapPipeline(rawUv, snapStep, snapTargets, inferenceAnchor),
    [snapStep, snapTargets, inferenceAnchor],
  )

  // Crosshair cursor while sketching — the OrbitControls grab cursor
  // is ambiguous when the user is supposed to be placing precise
  // points. Body-level set/restore so the cursor reaches over R3F's
  // canvas without us tracking the canvas ref.
  useEffect(() => {
    if (!sketch.active) return
    const prev = document.body.style.cursor
    document.body.style.cursor = 'crosshair'
    return () => {
      document.body.style.cursor = prev
    }
  }, [sketch.active])

  const handlePointerMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (!sketch.active) return
      e.stopPropagation()
      const rawUv = pointToPlaneUV(e.point, sketch.plane)
      const result = computeSnap(rawUv)
      setSketchHover(result.uv)
      setSketchSnapState({
        snapTarget: result.snapTarget,
        inferenceAxis: result.inferenceAxis,
      })
    },
    [sketch.active, sketch.plane, computeSnap, setSketchHover, setSketchSnapState],
  )

  const handlePointerOut = useCallback(() => {
    if (!sketch.active) return
    setSketchHover(null)
    setSketchSnapState({ snapTarget: null, inferenceAxis: null })
  }, [sketch.active, setSketchHover, setSketchSnapState])

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      if (!sketch.active) return
      // Suppress the orbit-controls click and any object pickers below.
      e.stopPropagation()
      const native = e.nativeEvent
      // Only react to left-click; right-click is intentionally
      // un-routed so OrbitControls / future menus stay free.
      if (native.button !== 0) return
      const rawUv = pointToPlaneUV(e.point, sketch.plane)
      const { uv: snapped } = computeSnap(rawUv)

      // Tool-specific termination: rectangle = 2 corners, circle =
      // center + radius point. For polyline the panel's "Finish"
      // button (or Enter) closes the loop.
      const tool: SketchTool = sketch.tool
      if (tool === 'rectangle' && sketch.points.length >= 2) return
      if (tool === 'circle' && sketch.points.length >= 2) return

      addSketchPoint(snapped)
    },
    [sketch.active, sketch.plane, sketch.points.length, sketch.tool, addSketchPoint, computeSnap],
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
        position={planeMeshPosition(sketch.plane)}
        quaternion={planeMeshQuaternion(sketch.plane)}
        onPointerMove={handlePointerMove}
        onPointerOut={handlePointerOut}
        onClick={handleClick}
      >
        <planeGeometry args={[PLANE_SIZE, PLANE_SIZE]} />
        <meshBasicMaterial
          color="#3498db"
          opacity={0.1}
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

      {/* Committed (non-active) shapes — render as faint coloured
          loops behind the active drawing so the user sees the bracket
          outline while laying out hole circles inside it. */}
      <CommittedShapesGuides />

      <SketchPreview showMeasure={showMeasure} />

      {/* Inference line drawn through the anchor whenever the cursor
          is locked to a horizontal/vertical axis through it. Drawn
          before the snap ring so the ring overlays it cleanly. */}
      {inferenceAnchor && sketch.inferenceAxis && (
        <InferenceLine
          anchor={inferenceAnchor}
          axis={sketch.inferenceAxis}
          plane={sketch.plane}
        />
      )}

      {/* Cyan ring at the magnetic snap target — gives the user a
          visual lock-on cue before they click. */}
      {sketch.snapTarget && (
        <SnapMarker target={sketch.snapTarget} plane={sketch.plane} />
      )}
    </group>
  )
}

// ─── Committed-shape guides ──────────────────────────────────────────

/**
 * Render the closed polygon of every shape EXCEPT the last (active)
 * one. All committed loops draw in a faint neutral colour — outer-vs-
 * hole classification is decided geometrically at extrude time, so
 * tinting per-shape would lie about a state that doesn't exist yet.
 *
 * Shapes that don't yet materialise to a valid polygon (too few
 * points, degenerate radius, etc.) are skipped silently.
 */
function CommittedShapesGuides() {
  const shapes = useSceneStore((s) => s.sketch.shapes)
  const plane = useSceneStore((s) => s.sketch.plane)
  const circleSegments = useSceneStore((s) => s.sketch.circleSegments)

  const committed = useMemo(
    () => shapes.slice(0, Math.max(0, shapes.length - 1)),
    [shapes],
  )

  const loops = useMemo(() => {
    return committed
      .map((shape: ServerSketchShape, idx: number) => {
        const profile = buildProfile2D(shape.tool, shape.points, circleSegments)
        if (!profile || profile.length < 3) return null
        const world = profile.map((p) => uvToWorld(p, plane))
        // Close the loop visually by duplicating the first point.
        world.push(world[0])
        return { idx, points: world }
      })
      .filter((l): l is { idx: number; points: THREE.Vector3[] } => l !== null)
  }, [committed, plane, circleSegments])

  if (loops.length === 0) return null

  return (
    <group name="sketch-committed-shapes">
      {loops.map((l) => (
        <Line
          key={`committed-${l.idx}`}
          points={l.points}
          // Committed loops use a desaturated steel-blue so they read as
          // "already drawn" against the brighter cyan of the active
          // shape, but stay legible against the plane tint at typical
          // working zoom levels.
          color="#7dd3fc"
          lineWidth={2}
          opacity={0.85}
          transparent
        />
      ))}
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
  const setSketchPoint = useSceneStore((s) => s.setSketchPoint)

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

  // Per-segment dimension labels at midpoints. Each label tracks
  // its segment endpoint indices so that confirmed→confirmed segments
  // can be made editable (segment i→i+1: keep point i, move point i+1
  // along the segment direction so its length matches the typed
  // value). Hover-tail segments are read-only — there's no point on
  // the backend to update yet.
  const labels = useMemo(() => {
    type Item = {
      pos: THREE.Vector3
      text: string
      len: number
      // index of the moving endpoint in confirmedUV; -1 = read-only
      // (e.g. tail segment to hover, or closing chord).
      movingIndex: number
      anchor?: [number, number]
    }
    const items: Item[] = []
    const allUV = hoverUV ? [...confirmedUV, hoverUV] : confirmedUV
    for (let i = 0; i < allUV.length - 1; i++) {
      const a = allUV[i]
      const b = allUV[i + 1]
      const len = Math.hypot(b[0] - a[0], b[1] - a[1])
      if (len < 1e-6) continue
      const mid = uvToWorld([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2], plane)
      const isConfirmedSegment = i + 1 < confirmedUV.length
      items.push({
        pos: mid,
        text: fmtLen(len),
        len,
        movingIndex: isConfirmedSegment ? i + 1 : -1,
        anchor: isConfirmedSegment ? a : undefined,
      })
    }
    if (showMeasure && confirmedUV.length >= 3) {
      // Closing chord length — read-only; editing it would require
      // moving either the first or last point and the right answer
      // depends on intent.
      const a = confirmedUV[confirmedUV.length - 1]
      const b = confirmedUV[0]
      const len = Math.hypot(b[0] - a[0], b[1] - a[1])
      const mid = uvToWorld([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2], plane)
      items.push({ pos: mid, text: `${fmtLen(len)} (close)`, len, movingIndex: -1 })
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
          lineWidth={1.5}
          dashed
          dashSize={0.3}
          gapSize={0.2}
          opacity={0.75}
          transparent
        />
      )}
      {confirmedWorld.map((pt, idx) => (
        <PointMarker key={idx} position={pt} active={idx === 0} />
      ))}
      {hoverWorld && <PointMarker position={hoverWorld} ghost />}

      {labels.map((l, i) => {
        // Editing a confirmed→confirmed segment: keep the anchor end
        // fixed, slide the moving end along the existing direction so
        // the segment length matches the typed value. Subsequent
        // segments retain their original lengths because they only
        // depend on later confirmed points; the moved point shifts the
        // chain rigidly downstream — that's the right answer for a
        // free-form polyline (the user typed a length they wanted
        // exactly there).
        const editable = l.movingIndex >= 0 && l.anchor !== undefined
        const onCommit = editable
          ? (next: number) => {
              if (next <= 0) return
              const anchor = l.anchor as [number, number]
              const movingIdx = l.movingIndex
              const cur = confirmedUV[movingIdx]
              const dx = cur[0] - anchor[0]
              const dy = cur[1] - anchor[1]
              const oldLen = Math.hypot(dx, dy)
              if (oldLen < 1e-6) return
              const k = next / oldLen
              setSketchPoint(movingIdx, [
                anchor[0] + dx * k,
                anchor[1] + dy * k,
              ])
            }
          : undefined
        return (
          <DimLabel
            key={`len-${i}`}
            position={l.pos}
            text={l.text}
            value={editable ? l.len : undefined}
            onCommit={onCommit}
          />
        )
      })}
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
  const setSketchPoint = useSceneStore((s) => s.setSketchPoint)
  const editable = confirmedUV.length >= 2

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

  // Width edit: keep corner A, move corner B along the existing
  // x-direction so |B.x - A.x| = newWidth. Sign preserved so the
  // user doesn't see the rectangle flip across A.
  const commitWidth = useCallback(
    (next: number) => {
      if (!editable || next <= 0) return
      const a = confirmedUV[0]
      const b = confirmedUV[1]
      const sign = b[0] >= a[0] ? 1 : -1
      const newB: [number, number] = [a[0] + sign * next, b[1]]
      setSketchPoint(1, newB)
    },
    [editable, confirmedUV, setSketchPoint],
  )

  const commitHeight = useCallback(
    (next: number) => {
      if (!editable || next <= 0) return
      const a = confirmedUV[0]
      const b = confirmedUV[1]
      const sign = b[1] >= a[1] ? 1 : -1
      const newB: [number, number] = [b[0], a[1] + sign * next]
      setSketchPoint(1, newB)
    },
    [editable, confirmedUV, setSketchPoint],
  )

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
      <DimLabel
        position={widthLabelPos}
        text={fmtLen(width)}
        value={editable ? width : undefined}
        onCommit={editable ? commitWidth : undefined}
      />
      <DimLabel
        position={heightLabelPos}
        text={fmtLen(height)}
        value={editable ? height : undefined}
        onCommit={editable ? commitHeight : undefined}
      />
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
  const setSketchPoint = useSceneStore((s) => s.setSketchPoint)
  const editable = confirmedUV.length >= 2

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

  // Radius edit: scale the (edge - center) vector to the new length.
  // Falls back to a +u-axis edge point when the existing radius is
  // degenerate (center and edge coincide), so the user can recover
  // from a 0-radius sketch by typing a length.
  const commitRadius = useCallback(
    (next: number) => {
      if (!editable || next <= 0) return
      const center = confirmedUV[0]
      const edge = confirmedUV[1]
      const dx = edge[0] - center[0]
      const dy = edge[1] - center[1]
      const oldR = Math.hypot(dx, dy)
      const newEdge: [number, number] =
        oldR > 1e-6
          ? [center[0] + (dx / oldR) * next, center[1] + (dy / oldR) * next]
          : [center[0] + next, center[1]]
      setSketchPoint(1, newEdge)
    },
    [editable, confirmedUV, setSketchPoint],
  )

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
        lineWidth={1.5}
        dashed
        dashSize={0.3}
        gapSize={0.2}
        opacity={0.75}
        transparent
      />
      <PointMarker position={uvToWorld(center, plane)} active />
      <PointMarker
        position={uvToWorld(edge, plane)}
        ghost={confirmedUV.length < 2}
      />
      <DimLabel
        position={labelPos}
        text={`R ${fmtLen(r)}`}
        value={editable ? r : undefined}
        onCommit={editable ? commitRadius : undefined}
      />
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
  /**
   * Current numeric value (length in mm, angle in degrees). When set
   * together with `onCommit`, the label becomes editable: double-click
   * swaps it for a text input. Enter commits, Escape cancels, blur
   * commits if the value parses. Without these props the label is a
   * read-only annotation, matching the original behaviour during
   * click-to-place.
   */
  value?: number
  onCommit?: (next: number) => void
}

function DimLabel({
  position,
  text,
  variant = 'length',
  value,
  onCommit,
}: DimLabelProps) {
  const editable = onCommit !== undefined && value !== undefined
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')
  const inputRef = useRef<HTMLInputElement | null>(null)

  // Focus the input on entry and select its contents so the user can
  // type a replacement value without clearing first.
  useEffect(() => {
    if (editing && inputRef.current) {
      inputRef.current.focus()
      inputRef.current.select()
    }
  }, [editing])

  const commit = useCallback(() => {
    if (!onCommit) return
    const parsed = parseFloat(draft)
    if (Number.isFinite(parsed)) {
      onCommit(parsed)
    }
    setEditing(false)
  }, [draft, onCommit])

  const cancel = useCallback(() => {
    setEditing(false)
  }, [])

  const beginEdit = useCallback(() => {
    if (!editable || value === undefined) return
    // Round to 3 decimals so the input doesn't show 4.999999... .
    setDraft(value.toFixed(3).replace(/\.?0+$/, ''))
    setEditing(true)
  }, [editable, value])

  const tone =
    variant === 'angle' ? 'border-amber-400/40 text-amber-300' : 'border-border/60 text-foreground'

  // While editing, the label needs pointer events to receive keystrokes
  // and clicks. While read-only (no editable wiring) we keep
  // `pointerEvents="none"` so labels don't intercept the sketch capture
  // plane during click-to-place.
  return (
    <Html
      position={position}
      center
      pointerEvents={editable ? 'auto' : 'none'}
      zIndexRange={[100, 0]}
    >
      {editing ? (
        <input
          ref={inputRef}
          type="text"
          inputMode="decimal"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            // Stop key events bubbling — without this, hotkeys on the
            // viewport (1/2/3 view switch, Del to delete) would fire
            // while the user is typing dimension values.
            e.stopPropagation()
            if (e.key === 'Enter') commit()
            else if (e.key === 'Escape') cancel()
          }}
          className={`px-1.5 py-0.5 w-20 text-[10px] font-mono uppercase tracking-wider bg-background border ${tone} outline-none focus:ring-1 focus:ring-primary/40`}
        />
      ) : (
        <div
          // Double-click rather than single-click so the user can still
          // marquee-select / pan over labels without accidentally
          // entering edit mode. `cursor-text` only when editable so
          // the affordance signals which labels are interactive.
          onDoubleClick={editable ? beginEdit : undefined}
          className={`px-1.5 py-0.5 text-[10px] font-mono uppercase tracking-wider bg-background/80 border ${tone} backdrop-blur-sm whitespace-nowrap select-none ${editable ? 'cursor-text hover:bg-background' : 'pointer-events-none'}`}
        >
          {text}
        </div>
      )}
    </Html>
  )
}

// ─── Snap marker + inference line ────────────────────────────────────

interface SnapMarkerProps {
  target: SnapTarget
  plane: SketchPlane
}

/**
 * Cyan ring at the locked snap target — the user's lock-on cue. The
 * ring colour modulates by snap kind so different targets read
 * differently (vertices = bright cyan; midpoints = muted cyan;
 * centers = amber; quadrants = green) without needing extra glyphs.
 */
function SnapMarker({ target, plane }: SnapMarkerProps) {
  const pos = uvToWorld(target.uv, plane)
  // Torus's local axis is +Z; reuse the same orientation helper that
  // sets the capture-plane mesh so the ring lies flat against the
  // chosen sketch plane regardless of which standard / custom plane
  // is active.
  const quat = planeMeshQuaternion(plane)
  const color = (() => {
    switch (target.kind) {
      case 'vertex': return '#22d3ee' // cyan-400
      case 'midpoint': return '#0891b2' // cyan-600
      case 'center': return '#f59e0b' // amber-500
      case 'quadrant': return '#10b981' // emerald-500
    }
  })()
  return (
    <mesh position={pos} quaternion={quat}>
      {/* Thin torus reads as a ring at any zoom level. depthTest off
          so it draws on top of the existing point markers / preview. */}
      <torusGeometry args={[0.32, 0.045, 8, 24]} />
      <meshBasicMaterial color={color} depthTest={false} transparent opacity={0.9} />
    </mesh>
  )
}

interface InferenceLineProps {
  anchor: [number, number]
  axis: InferenceAxis
  plane: SketchPlane
}

/**
 * Dashed construction line through the inference anchor along the
 * locked axis ('h' = horizontal in plane uv = constant v; 'v' =
 * vertical = constant u). Drawn long enough to read at any sensible
 * zoom but bounded so the line doesn't dominate the scene.
 */
function InferenceLine({ anchor, axis, plane }: InferenceLineProps) {
  const L = 60 // matches PLANE_VIS so the line spans the working area
  const a: [number, number] =
    axis === 'h' ? [anchor[0] - L, anchor[1]] : [anchor[0], anchor[1] - L]
  const b: [number, number] =
    axis === 'h' ? [anchor[0] + L, anchor[1]] : [anchor[0], anchor[1] + L]
  const points = [uvToWorld(a, plane), uvToWorld(b, plane)]
  const color = axis === 'h' ? '#f97316' : '#a855f7' // orange / violet
  return (
    <Line
      points={points}
      color={color}
      lineWidth={1}
      dashed
      dashSize={0.5}
      gapSize={0.3}
      opacity={0.65}
      transparent
    />
  )
}
