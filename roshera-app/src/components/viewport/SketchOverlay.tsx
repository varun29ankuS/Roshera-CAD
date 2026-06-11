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
import { useThree, type ThreeEvent } from '@react-three/fiber'
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
import { ExtrudeRegionPreview } from './ExtrudeRegionPreview'
import { uvToWorld } from './sketch-plane-uv'
import {
  CSketchConstraintConflictError,
  csketchApi,
  pointRef,
  type Constraint,
  type CSketchCircleSummary,
  type CSketchLineSummary,
  type CSketchPointSummary,
  type CSketchSummary,
  type EntityRef,
  type SnapCandidate,
} from '@/lib/csketch-api'
import { applyInferredConstraints } from '@/lib/csketch-inference'

const PLANE_SIZE = 400 // world units; large enough that the user
                       // cannot click off-plane in normal use.

// D-2-c snap-search radius in plane (u, v) units. Matches the
// visual scale of `CSKETCH_GLYPH_SIZE` so the snap engine only
// engages within a glyph-width of an existing entity — the same
// "feel" as mainstream CAD hover-on-vertex magnetism.
const CSKETCH_SNAP_RADIUS = 1.5

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
 * the standard "Horizontal" / "Vertical" inference cues.
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
  const addNewSketchShape = useSceneStore((s) => s.addNewSketchShape)
  const setSketchHover = useSceneStore((s) => s.setSketchHover)
  const setSketchSnapState = useSceneStore((s) => s.setSketchSnapState)
  const showMeasure = sketch.measure
  const snapStep = sketch.snapStep

  // Constrained-sketch draw routing (D-2-b). When a csketch is open
  // and the user has picked a `point` / `line` / `circle` tool from
  // the panel, capture-plane clicks are dispatched through the
  // csketch REST surface instead of the legacy `addSketchPoint`.
  // Line and circle tools are stateful (2 clicks each) — `csketchDraftRef`
  // holds the first click's (u, v) until the second click lands.
  const csketchActiveId = useSceneStore((s) => s.csketch.activeId)
  const csketchActiveTool = useSceneStore((s) => s.csketch.activeTool)
  const addCSketchPoint = useSceneStore((s) => s.addCSketchPoint)
  const addCSketchLine = useSceneStore((s) => s.addCSketchLine)
  const addCSketchCircle = useSceneStore((s) => s.addCSketchCircle)
  const addCSketchConstraint = useSceneStore((s) => s.addCSketchConstraint)
  // `pid` is set when the first click of a line gesture landed on
  // an existing csketch point (via D-2-c snap reuse). Carrying it
  // through to the second-click commit lets us skip the redundant
  // addPoint call and reference the existing entity directly,
  // avoiding stacked-point duplicates at shared vertices.
  const csketchDraftRef = useRef<
    { uv: [number, number]; pid: string | null } | null
  >(null)
  // Whenever the user switches tool (or closes the csketch), forget
  // any half-committed first click — otherwise a stale anchor leaks
  // into the next gesture.
  useEffect(() => {
    csketchDraftRef.current = null
  }, [csketchActiveTool, csketchActiveId])

  // D-2-c: snap glyphs while a csketch draw tool is active.
  // POST /csketch/:id/snap returns candidates sorted by (priority,
  // distance); we keep only [0]. The candidate lives in a ref so
  // the click handler can read the latest without React re-renders
  // shifting the resolved point mid-gesture; it is also mirrored
  // into state so the kind-aware glyph re-renders.
  //
  // A generation counter ensures stale responses arriving after a
  // tool/csketch switch are dropped instead of overwriting the
  // freshly-cleared state.
  const csketchSnapRef = useRef<SnapCandidate | null>(null)
  const [csketchSnapBest, setCSketchSnapBest] =
    useState<SnapCandidate | null>(null)
  const csketchSnapDispatchRef = useRef<{
    inFlight: boolean
    pendingUV: [number, number] | null
  } | null>(null)
  const csketchSnapGenRef = useRef(0)

  useEffect(() => {
    csketchSnapGenRef.current += 1
    csketchSnapRef.current = null
    csketchSnapDispatchRef.current = null
    // Cleanup-on-deps-change is the textbook use of effect-driven
    // setState — there is no render-time derivation that can wipe
    // a stale async result. The lint rule overfits to the
    // "derive instead" case so we suppress with intent.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setCSketchSnapBest(null)
  }, [csketchActiveId, csketchActiveTool])

  // Trailing-throttle dispatcher (same shape as `CSketchPoints`'
  // `dispatchDrag`): at most one snap request in flight; the
  // freshest cursor wins via `pendingUV` + `.finally()` chain.
  const dispatchCSketchSnap = useCallback(
    (uv: [number, number]) => {
      const id = csketchActiveId
      if (id === null) return
      if (!csketchSnapDispatchRef.current) {
        csketchSnapDispatchRef.current = { inFlight: false, pendingUV: null }
      }
      const d = csketchSnapDispatchRef.current
      if (d.inFlight) {
        d.pendingUV = uv
        return
      }
      const myGen = csketchSnapGenRef.current
      d.inFlight = true
      const fire = (target: [number, number]) =>
        csketchApi
          .snap(id, {
            cursor: { x: target[0], y: target[1] },
            radius: CSKETCH_SNAP_RADIUS,
          })
          .then((cands) => {
            // Drop stale responses arriving after a tool/csketch
            // switch — the cleanup effect above already wiped the
            // visible state, and resurrecting it here would flash a
            // ghost glyph for a frame.
            if (csketchSnapGenRef.current !== myGen) return
            const best = cands.length > 0 ? cands[0] : null
            csketchSnapRef.current = best
            setCSketchSnapBest(best)
          })
          .catch((err) => {
            console.error('[CSketchOverlay] snap failed:', err)
          })
      const chain = (uvNow: [number, number]) => {
        fire(uvNow).finally(() => {
          const live = csketchSnapDispatchRef.current
          if (!live) return
          if (live.pendingUV) {
            const next = live.pendingUV
            live.pendingUV = null
            chain(next)
          } else {
            live.inFlight = false
          }
        })
      }
      chain(uv)
    },
    [csketchActiveId],
  )

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
      // D-2-c: while a csketch draw tool is active, fire the
      // kernel snap query alongside the legacy local snap so the
      // glyph below can render the locked-on target. The dispatch
      // is in-flight-throttled — no event-loop pressure even at
      // 240 Hz pointermove.
      if (csketchActiveId !== null && csketchActiveTool !== null) {
        dispatchCSketchSnap(rawUv)
      }
      const result = computeSnap(rawUv)
      setSketchHover(result.uv)
      setSketchSnapState({
        snapTarget: result.snapTarget,
        inferenceAxis: result.inferenceAxis,
      })
    },
    [
      sketch.active,
      sketch.plane,
      computeSnap,
      setSketchHover,
      setSketchSnapState,
      csketchActiveId,
      csketchActiveTool,
      dispatchCSketchSnap,
    ],
  )

  const handlePointerOut = useCallback(() => {
    if (!sketch.active) return
    setSketchHover(null)
    setSketchSnapState({ snapTarget: null, inferenceAxis: null })
    // D-2-c: clear the csketch snap glyph when the cursor leaves
    // the capture plane — otherwise the marker lingers at the last
    // known position even though the user has moved away.
    csketchSnapRef.current = null
    setCSketchSnapBest(null)
  }, [sketch.active, setSketchHover, setSketchSnapState])

  // Click vs. drag arbitration on the sketch capture plane.
  //
  // OrbitControls now keeps LMB-orbit enabled while sketching so the
  // user has a way to reorient the view mid-draw. That means a bare
  // R3F `onClick` would also fire at the end of every orbit gesture
  // that started or ended over the plane, producing spurious sketch
  // points. We replace it with a manual pointerdown handler that
  // registers a one-shot window-level pointerup listener and only
  // places a point when total pointer travel stays under the 4 px
  // budget desktop UIs use for "click vs. drag" (matches mainstream
  // CAD behaviour). pointerup is registered on `window` rather
  // than the mesh so the gesture is resilient to the camera moving
  // mid-drag — by the time the user releases, the raycaster may have
  // long lost the plane.
  const handlePointerDown = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (!sketch.active) return
      if (e.nativeEvent.button !== 0) return
      const downX = e.nativeEvent.clientX
      const downY = e.nativeEvent.clientY
      const downUv = pointToPlaneUV(e.point, sketch.plane)

      const onUp = (upEv: PointerEvent) => {
        window.removeEventListener('pointerup', onUp)
        if (upEv.button !== 0) return
        const dx = upEv.clientX - downX
        const dy = upEv.clientY - downY
        // 16 = 4²; the conventional click-vs-drag threshold.
        if (dx * dx + dy * dy > 16) return

        // D-2-b: when a csketch tool is selected, capture-plane
        // clicks dispatch through `csketchApi.{addPoint,addLine,
        // addCircle}` instead of the legacy sketch handler.
        // D-2-c: the snap layer above has resolved the cursor to
        // its best attachment (vertex / midpoint / centre /
        // quadrant / on-curve), so we commit the snapped point —
        // never the raw cursor. For line endpoints that snap to an
        // existing csketch point we additionally reuse the point's
        // id instead of spawning a duplicate stacked on top.
        if (csketchActiveId !== null && csketchActiveTool !== null) {
          const id = csketchActiveId
          const snap = csketchSnapRef.current
          const clickUv: [number, number] =
            snap !== null ? [snap.point.x, snap.point.y] : [downUv[0], downUv[1]]
          const reusePid: string | null =
            snap !== null && snap.kind === 'point' && 'Point' in snap.entity
              ? snap.entity.Point
              : null

          if (csketchActiveTool === 'point') {
            // If the cursor latched onto an existing point, the
            // user almost certainly meant to keep it — a duplicate
            // would just over-constrain the sketch. Treat the
            // click as a no-op in that case.
            if (reusePid !== null) return
            void (async () => {
              const pid = await addCSketchPoint(id, {
                x: clickUv[0],
                y: clickUv[1],
              })
              // D-2-d: auto-apply inference proposals against the
              // just-committed point. Coincident /
              // PointOnCurve / Midpoint are the common picks; all
              // arrive via `point_self`.
              await applyInferredConstraints(
                id,
                {
                  kind: 'point',
                  position: { x: clickUv[0], y: clickUv[1] },
                },
                { point_self: pid },
                addCSketchConstraint,
              )
            })()
            return
          }
          if (csketchActiveTool === 'line') {
            // First click anchors a draft endpoint; second click
            // commits the line. Each endpoint is either an
            // existing point id (snap-reuse) or a fresh point
            // committed inline.
            const first = csketchDraftRef.current
            if (first === null) {
              csketchDraftRef.current = { uv: clickUv, pid: reusePid }
              return
            }
            csketchDraftRef.current = null
            // Same-vertex degenerate gesture — the kernel rejects
            // zero-length lines anyway; silently drop.
            if (
              first.pid !== null &&
              reusePid !== null &&
              first.pid === reusePid
            ) {
              return
            }
            void (async () => {
              const p1 =
                first.pid ??
                (await addCSketchPoint(id, {
                  x: first.uv[0],
                  y: first.uv[1],
                }))
              const p2 =
                reusePid ??
                (await addCSketchPoint(id, {
                  x: clickUv[0],
                  y: clickUv[1],
                }))
              if (p1 === p2) return
              const lineId = await addCSketchLine(id, { start: p1, end: p2 })
              // D-2-d: line inference covers Horizontal /
              // Vertical (unary, `line_self`), Parallel /
              // Perpendicular / Tangent (binary, `line_self`),
              // plus Coincident proposals targeting either
              // endpoint (`line_start` / `line_end`). The kernel
              // dedupes against constraints already applied via
              // snap-driven point reuse, so calling this even
              // when both endpoints came from existing points
              // (`first.pid` + `reusePid`) stays cheap.
              await applyInferredConstraints(
                id,
                {
                  kind: 'line',
                  start: { x: first.uv[0], y: first.uv[1] },
                  end: { x: clickUv[0], y: clickUv[1] },
                },
                {
                  line_self: lineId,
                  line_start: p1,
                  line_end: p2,
                },
                addCSketchConstraint,
              )
            })()
            return
          }
          if (csketchActiveTool === 'circle') {
            // First click anchors a centre; second click commits a
            // circle whose radius is the Euclidean distance between
            // the two clicks. addCSketchCircle takes raw (cx, cy,
            // r) — the kernel materialises the centre internally,
            // so there is no entity-reuse opportunity here.
            const first = csketchDraftRef.current
            if (first === null) {
              csketchDraftRef.current = { uv: clickUv, pid: null }
              return
            }
            csketchDraftRef.current = null
            const ddu = clickUv[0] - first.uv[0]
            const ddv = clickUv[1] - first.uv[1]
            const radius = Math.sqrt(ddu * ddu + ddv * ddv)
            // Below the kernel's positive-radius guard — silently
            // drop, the user will click again to restart.
            if (radius <= 1e-9) return
            void (async () => {
              const cid = await addCSketchCircle(id, {
                cx: first.uv[0],
                cy: first.uv[1],
                radius,
              })
              // D-2-d: circle inference primarily surfaces
              // Concentric / Equal proposals against existing
              // circles, both arriving via `circle_self`.
              // `circle_center` proposals are dropped inside the
              // helper — see csketch-inference.ts module doc.
              await applyInferredConstraints(
                id,
                {
                  kind: 'circle',
                  center: { x: first.uv[0], y: first.uv[1] },
                  radius,
                },
                { circle_self: cid },
                addCSketchConstraint,
              )
            })()
            return
          }
        }

        const { uv: snapped } = computeSnap(downUv)

        // Multi-shape sketch flow: a single sketch session can carry
        // multiple closed loops, each becoming its own ServerSketchShape.
        // The user should never have to reach for "Add shape" — every
        // completed loop rolls into a new shape automatically, and the
        // sketch only ends when the user explicitly Finishes or Cancels.
        // Three completion triggers, one per tool:
        //
        //   - Rectangle / circle: each tool consumes exactly 2 clicks.
        //     A 3rd click on the same plane means "I'm done with this
        //     one, start another" — auto-commit the current shape and
        //     drop this click as the first point of a new shape with
        //     the same tool.
        //
        //   - Polyline: the user signals "close this loop" by clicking
        //     back at the first vertex (which the snap engine already
        //     attracts the cursor to as a magnetic target). The clicked
        //     point is NOT appended — the polygon naturally closes via
        //     the first→last edge — and we auto-commit + start a fresh
        //     polyline ready for the next loop.
        const tool: SketchTool = sketch.tool
        const pts = sketch.points

        if (tool === 'polyline' && pts.length >= 3) {
          const p0 = pts[0]
          const ddx = snapped[0] - p0[0]
          const ddy = snapped[1] - p0[1]
          // Tolerance matches the snap-target rounding in snapToGeometry:
          // when the cursor latches onto the first vertex, the snapped
          // uv is the exact stored point, so any difference > epsilon
          // means the user clicked somewhere else.
          if (ddx * ddx + ddy * ddy < 1e-12) {
            addNewSketchShape(tool)
            return
          }
        }

        if ((tool === 'rectangle' || tool === 'circle') && pts.length >= 2) {
          // The current rectangle / circle has both anchor points, so
          // it's a complete shape on the backend's terms. Commit it,
          // start a fresh shape with the same tool, and use this click
          // as that new shape's first anchor — all in one gesture.
          addNewSketchShape(tool)
          addSketchPoint(snapped)
          return
        }

        addSketchPoint(snapped)
      }

      window.addEventListener('pointerup', onUp)
    },
    [
      sketch.active,
      sketch.plane,
      sketch.points,
      sketch.tool,
      addSketchPoint,
      addNewSketchShape,
      computeSnap,
      csketchActiveId,
      csketchActiveTool,
      addCSketchPoint,
      addCSketchLine,
      addCSketchCircle,
      addCSketchConstraint,
    ],
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
        onPointerDown={handlePointerDown}
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

      {/* Hover-time region-decomposition preview (Slice D). Renders
          server-authoritative outer/hole loops in blue/red whenever
          the SketchPanel's Finish & Extrude button is hovered. */}
      <ExtrudeRegionPreview />

      {/* Dimensional-constraint labels for the active csketch (N-3).
          Read-only in this slice — N-4 will rewire the same labels
          through `updateCSketchConstraintValue` for double-click
          editing. Renders nothing unless there is both an active
          csketch and an active sketch plane to project onto. */}
      <CSketchDimensions plane={sketch.plane} />

      {/* Geometric-constraint badges (D-3b). Small one-glyph
          bordered chips anchored to the constraint's entity
          centroid. Badges turn rose for `conflicts` and amber for
          `redundant` so the H-slice diagnosis surfaces visually
          without an extra panel. */}
      <CSketchGeometricBadges plane={sketch.plane} />

      {/* Read-only csketch line + circle visuals (D-2-b). Mounted
          alongside `CSketchPoints` so a freshly committed entity
          shows up the instant the panel's tool row dispatches its
          REST call. Lines render as crisp drei `<Line>` segments;
          circles as 64-sample closed polylines. Construction
          entities render in violet, regular ones in neutral white,
          matching the point conventions. */}
      <CSketchLines plane={sketch.plane} />
      <CSketchCircles plane={sketch.plane} />

      {/* Draggable disc handles for every csketch point (D-3c).
          Picks bypass the capture-plane click handler via
          `stopPropagation`. While a drag is in flight, a throttled
          POST /csketch/:id/drag streams new (x, y) targets — the
          solver pins everything else and minimises movement of the
          unconstrained DOFs, so dragging one point re-solves the
          rest of the sketch in real time. */}
      <CSketchPoints plane={sketch.plane} />

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

      {/* D-2-c: kind-aware glyph at the kernel-resolved csketch
          snap point. Only renders when a csketch draw tool is
          active AND the snap engine returned a candidate within
          `CSKETCH_SNAP_RADIUS`. */}
      {csketchSnapBest && (
        <CSketchSnapMarker
          candidate={csketchSnapBest}
          plane={sketch.plane}
        />
      )}
    </group>
  )
}

// ─── Committed-shape guides ──────────────────────────────────────────

/**
 * Render the closed polygon of every shape EXCEPT the last (active)
 * one. All committed loops draw in a saturated cyan so they read
 * unambiguously as part of the live sketch — earlier versions used
 * a desaturated steel-blue which made closed loops look "vanished"
 * after the auto-commit pattern reset the active shape to empty.
 *
 * Each committed segment carries a read-only length label so the
 * user keeps visual confirmation of their dimensions after closure.
 * Editing committed-shape dimensions is the job of the constrained-
 * sketch path (`csketch`) — this surface stays read-only because the
 * legacy sketch session has no per-point set-by-shape endpoint.
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
        // Segment lengths + midpoint label positions, computed before
        // we duplicate the first vertex to close the visual line.
        // Circles tessellate into many short segments — labelling each
        // would clutter the viewport, so we surface a single radius
        // label at the rightmost vertex instead.
        const isCircle = shape.tool === 'circle'
        const labels: Array<{ pos: THREE.Vector3; text: string }> = []
        if (isCircle && shape.points.length >= 2) {
          const [cx, cy] = shape.points[0]
          const [ex, ey] = shape.points[1]
          const r = Math.hypot(ex - cx, ey - cy)
          // Place the R-label at the edge anchor in plane space.
          labels.push({
            pos: uvToWorld([ex, ey], plane),
            text: `R ${fmtLen(r)}`,
          })
        } else {
          for (let i = 0; i < profile.length; i += 1) {
            const a = profile[i]
            const b = profile[(i + 1) % profile.length]
            const len = Math.hypot(b[0] - a[0], b[1] - a[1])
            if (len < 1e-6) continue
            labels.push({
              pos: uvToWorld([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2], plane),
              text: fmtLen(len),
            })
          }
        }
        // Close the loop visually by duplicating the first point.
        world.push(world[0])
        return { idx, points: world, labels }
      })
      .filter(
        (l): l is {
          idx: number
          points: THREE.Vector3[]
          labels: Array<{ pos: THREE.Vector3; text: string }>
        } => l !== null,
      )
  }, [committed, plane, circleSegments])

  if (loops.length === 0) return null

  return (
    <group name="sketch-committed-shapes">
      {loops.map((l) => (
        <group key={`committed-${l.idx}`}>
          <Line
            points={l.points}
            // Bright cyan matches the active-shape preview so committed
            // loops stay visually anchored as part of the sketch.
            color="#3498db"
            lineWidth={2}
            opacity={0.95}
            transparent
          />
          {l.labels.map((lab, i) => (
            <DimLabel
              key={`committed-${l.idx}-len-${i}`}
              position={lab.pos}
              text={lab.text}
            />
          ))}
        </group>
      ))}
    </group>
  )
}

// ─── Constrained-sketch dimension labels (N-3) ───────────────────────

/**
 * Render a read-only label for every persisted `DimensionalConstraint`
 * on the active csketch. Renders nothing unless `csketch.activeId` is
 * set and an entity summary for that id is in the store — both are
 * guaranteed by `openCSketch`, so a freshly-opened csketch yields
 * labels on the next render.
 *
 * Coordinate convention: the kernel csketch lives in a pure 2D (u, v)
 * frame with no embedded plane of its own. We lift each label onto
 * the active sketch's plane via `uvToWorld`, matching the conceptual
 * UX where the csketch and the click-to-place sketch share a working
 * plane. When the live editor is not bound to a plane the labels are
 * still useful as flat-projected annotations on the xy-plane — but
 * for slice N-3 we gate on `sketch.active` so the labels only appear
 * while the user is in a sketching context.
 *
 * Placement rules per dimensional kind:
 *   - Distance(p1, p2)         → midpoint of the two points
 *   - Distance(p,  l) / (l, p) → projection of `p` onto `l`'s midpoint
 *   - Distance(l1, l2)         → midpoint of l1 (parallel offset)
 *   - Length(line)             → midpoint of the line segment
 *   - Radius(circle)           → rightmost point on the circle
 *   - Diameter(circle)         → rightmost point, prefixed "⌀"
 *   - Angle(line1, line2)      → midpoint of the first line as anchor
 *   - XCoordinate(point)       → at the point (small "x = …" badge)
 *   - YCoordinate(point)       → at the point (small "y = …" badge)
 *
 * Variants without a single natural placement (Perimeter, Area,
 * MomentOfInertia, CenterOfMass, AspectRatio, …) are skipped here;
 * they will surface in slice H's constraint-diagnosis panel instead.
 *
 * `Conflicting` and `Violated` constraints are still rendered — the
 * label colour reflects the status so the user sees which dimension
 * the solver could not satisfy without having to open a diagnostics
 * panel.
 */
function CSketchDimensions({ plane }: { plane: SketchPlane }) {
  const activeId = useSceneStore((s) => s.csketch.activeId)
  const summary = useSceneStore((s) =>
    s.csketch.activeId ? s.csketch.summaries.get(s.csketch.activeId) ?? null : null,
  )
  const constraints = useSceneStore((s) => s.csketch.activeConstraints)
  const updateValue = useSceneStore((s) => s.updateCSketchConstraintValue)

  const labels = useMemo(() => {
    if (!activeId || !summary) return []
    return buildCSketchDimensionLabels(summary, constraints, plane)
  }, [activeId, summary, constraints, plane])

  if (labels.length === 0 || !activeId) return null

  return (
    <group name="csketch-dimensions">
      {labels.map((l) => (
        <DimLabel
          key={l.key}
          position={l.position}
          text={l.text}
          variant={l.variant}
          value={l.displayValue}
          onCommit={(next) => {
            // For angle constraints the displayed unit is degrees but
            // the kernel stores radians — round-trip through the
            // angle's native unit on the wire.
            const payload = l.variant === 'angle' ? (next * Math.PI) / 180 : next
            updateValue(activeId, l.constraintId, payload).catch((err) => {
              if (err instanceof CSketchConstraintConflictError) {
                // Server already reverted to the pre-edit value; the
                // refreshed summary will arrive via the conflict
                // payload's revert. Surface to the console until
                // slice H's diagnosis panel exists.
                console.warn(
                  `csketch constraint ${l.constraintId} conflict:`,
                  err.details,
                )
              } else {
                console.error('csketch constraint update failed:', err)
              }
            })
          }}
        />
      ))}
    </group>
  )
}

interface CSketchDimensionLabel {
  /** Stable React key; equals the constraint id. */
  key: string
  /** Same as `key` but typed for the API call. */
  constraintId: string
  position: THREE.Vector3
  text: string
  variant: 'length' | 'angle'
  /**
   * Numeric value shown to the user when the label enters edit mode.
   * Length constraints carry the raw scalar (mm / model units);
   * angle constraints carry **degrees** (the same unit the read-only
   * text displays), so the input the user types matches what they
   * see. The `onCommit` callback converts back to radians on the
   * wire before hitting the API.
   */
  displayValue: number
}

/**
 * Pure derivation: given a csketch entity summary, its constraint
 * list, and the active sketch plane, produce one positioned label
 * per renderable dimensional constraint. Skips constraints whose
 * referenced entities are absent from the summary (the summary may
 * lag a constraint add by one render frame — graceful skip avoids
 * crashing on the transient missing-id).
 *
 * Exported (file-local) so a future N-3.5 test harness can pin the
 * label-placement geometry without mounting React.
 */
function buildCSketchDimensionLabels(
  summary: CSketchSummary,
  constraints: Constraint[],
  plane: SketchPlane,
): CSketchDimensionLabel[] {
  const pointsById = new Map<string, CSketchPointSummary>()
  for (const p of summary.points) pointsById.set(p.id, p)
  const linesById = new Map<string, CSketchLineSummary>()
  for (const l of summary.lines) linesById.set(l.id, l)
  const circlesById = new Map<string, CSketchCircleSummary>()
  for (const c of summary.circles) circlesById.set(c.id, c)

  const out: CSketchDimensionLabel[] = []
  for (const c of constraints) {
    if (!('Dimensional' in c.constraint_type)) continue
    const d = c.constraint_type.Dimensional

    if ('Distance' in d) {
      const placement = distancePlacement(c.entities, pointsById, linesById)
      if (!placement) continue
      out.push({
        key: c.id,
        constraintId: c.id,
        position: uvToWorld(placement, plane),
        text: fmtLen(d.Distance),
        variant: 'length',
        displayValue: d.Distance,
      })
      continue
    }
    if ('Length' in d) {
      const uv = lineMidpoint(c.entities, linesById)
      if (!uv) continue
      out.push({
        key: c.id,
        constraintId: c.id,
        position: uvToWorld(uv, plane),
        text: fmtLen(d.Length),
        variant: 'length',
        displayValue: d.Length,
      })
      continue
    }
    if ('Radius' in d) {
      const placement = circleRightmost(c.entities, circlesById)
      if (!placement) continue
      out.push({
        key: c.id,
        constraintId: c.id,
        position: uvToWorld(placement, plane),
        text: `R ${fmtLen(d.Radius)}`,
        variant: 'length',
        displayValue: d.Radius,
      })
      continue
    }
    if ('Diameter' in d) {
      const placement = circleRightmost(c.entities, circlesById)
      if (!placement) continue
      out.push({
        key: c.id,
        constraintId: c.id,
        position: uvToWorld(placement, plane),
        text: `\u2300 ${fmtLen(d.Diameter)}`,
        variant: 'length',
        displayValue: d.Diameter,
      })
      continue
    }
    if ('Angle' in d) {
      const uv = anglePlacement(c.entities, linesById)
      if (!uv) continue
      out.push({
        key: c.id,
        constraintId: c.id,
        position: uvToWorld(uv, plane),
        text: fmtAngle(d.Angle),
        variant: 'angle',
        // The kernel stores radians; the editor shows degrees so the
        // input matches the visible text. `CSketchDimensions`
        // converts back to radians before PATCH.
        displayValue: (d.Angle * 180) / Math.PI,
      })
      continue
    }
    if ('XCoordinate' in d) {
      const uv = singlePoint(c.entities, pointsById)
      if (!uv) continue
      out.push({
        key: c.id,
        constraintId: c.id,
        position: uvToWorld(uv, plane),
        text: `x ${fmtLen(d.XCoordinate)}`,
        variant: 'length',
        displayValue: d.XCoordinate,
      })
      continue
    }
    if ('YCoordinate' in d) {
      const uv = singlePoint(c.entities, pointsById)
      if (!uv) continue
      out.push({
        key: c.id,
        constraintId: c.id,
        position: uvToWorld(uv, plane),
        text: `y ${fmtLen(d.YCoordinate)}`,
        variant: 'length',
        displayValue: d.YCoordinate,
      })
      continue
    }
    // Perimeter / Area / MomentOfInertia / CenterOfMass / AspectRatio /
    // MinDistance / MaxDistance / ArcLength / Curvature / Slope /
    // OffsetDistance — no single natural placement in the viewport;
    // surfaced by the constraint-list panel instead.
  }
  return out
}

/** Centre of a `LineGeometry`'s natural representative point. */
function lineRepresentative(line: CSketchLineSummary): [number, number] {
  const g = line.geometry
  if ('Segment' in g) {
    return [(g.Segment.start.x + g.Segment.end.x) / 2, (g.Segment.start.y + g.Segment.end.y) / 2]
  }
  if ('Ray' in g) {
    return [g.Ray.origin.x, g.Ray.origin.y]
  }
  return [g.Infinite.point.x, g.Infinite.point.y]
}

function entityPoint(
  ref: EntityRef,
  points: Map<string, CSketchPointSummary>,
): [number, number] | null {
  if (!('Point' in ref)) return null
  const p = points.get(ref.Point)
  return p ? [p.x, p.y] : null
}

function entityLine(
  ref: EntityRef,
  lines: Map<string, CSketchLineSummary>,
): CSketchLineSummary | null {
  if (!('Line' in ref)) return null
  return lines.get(ref.Line) ?? null
}

function entityCircle(
  ref: EntityRef,
  circles: Map<string, CSketchCircleSummary>,
): CSketchCircleSummary | null {
  if (!('Circle' in ref)) return null
  return circles.get(ref.Circle) ?? null
}

function singlePoint(
  refs: EntityRef[],
  points: Map<string, CSketchPointSummary>,
): [number, number] | null {
  if (refs.length === 0) return null
  return entityPoint(refs[0], points)
}

function lineMidpoint(
  refs: EntityRef[],
  lines: Map<string, CSketchLineSummary>,
): [number, number] | null {
  if (refs.length === 0) return null
  const l = entityLine(refs[0], lines)
  return l ? lineRepresentative(l) : null
}

/**
 * Place a `Distance` label sensibly across the (point, line) variants
 * the kernel allows. `point–point` is the dominant case; `point–line`
 * and `line–line` fall back to the most readable midpoint.
 */
function distancePlacement(
  refs: EntityRef[],
  points: Map<string, CSketchPointSummary>,
  lines: Map<string, CSketchLineSummary>,
): [number, number] | null {
  if (refs.length < 2) return null
  const a = entityPoint(refs[0], points)
  const b = entityPoint(refs[1], points)
  if (a && b) return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2]
  // point–line: anchor on the point so the label tracks the visible
  // dimension's natural endpoint.
  if (a) {
    const lb = entityLine(refs[1], lines)
    if (lb) {
      const mid = lineRepresentative(lb)
      return [(a[0] + mid[0]) / 2, (a[1] + mid[1]) / 2]
    }
    return a
  }
  if (b) {
    const la = entityLine(refs[0], lines)
    if (la) {
      const mid = lineRepresentative(la)
      return [(b[0] + mid[0]) / 2, (b[1] + mid[1]) / 2]
    }
    return b
  }
  // line–line: parallel-offset case; place at the first line's midpoint.
  const la = entityLine(refs[0], lines)
  return la ? lineRepresentative(la) : null
}

function anglePlacement(
  refs: EntityRef[],
  lines: Map<string, CSketchLineSummary>,
): [number, number] | null {
  if (refs.length === 0) return null
  const la = entityLine(refs[0], lines)
  return la ? lineRepresentative(la) : null
}

/** Rightmost point on a circle in (u, v) — `cx + r, cy`. */
function circleRightmost(
  refs: EntityRef[],
  circles: Map<string, CSketchCircleSummary>,
): [number, number] | null {
  if (refs.length === 0) return null
  const c = entityCircle(refs[0], circles)
  return c ? [c.cx + c.radius, c.cy] : null
}

// ─── Constrained-sketch geometric badges (D-3b) ──────────────────────

/**
 * Status of a geometric constraint as far as the viewport overlay
 * cares. Drives badge tint:
 *
 *   - `ok`        → neutral border, foreground glyph (the common case)
 *   - `redundant` → amber border + glyph; from `DofReport.redundant`
 *                   (linearly dependent row with zero residual)
 *   - `conflict`  → rose border + glyph; from `DofReport.conflicts`
 *                   (linearly dependent row with non-zero residual)
 */
type GeometricBadgeStatus = 'ok' | 'redundant' | 'conflict'

interface GeometricBadge {
  /** Stable React key; equals the constraint id. */
  constraintId: string
  /** Single-glyph (or short string) for the badge body. */
  glyph: string
  /** Human-readable name shown as the badge's `title` (browser tooltip). */
  title: string
  status: GeometricBadgeStatus
  /** Plane-local (u, v) position of the badge anchor. */
  uv: [number, number]
}

/**
 * Map a `GeometricConstraint` variant to its display glyph + tooltip.
 * Returns `null` for variants we don't render — those would clutter
 * the viewport (e.g. `EqualArea` is a multi-loop property that has no
 * natural single-point anchor).
 *
 * Glyph choices:
 *   - One-character where a recognisable Unicode exists (∥ ⊥ ≡ ◎ ∠)
 *   - Two-character ASCII where Unicode is ambiguous (Eq, Tg, Co)
 *   - Avoid emoji — the badges live on a 16-px line and emoji break
 *     vertical centering in tabular CSS layouts.
 */
function geometricGlyph(
  g: import('@/lib/csketch-api').GeometricConstraint,
): { glyph: string; title: string } | null {
  if (typeof g === 'string') {
    switch (g) {
      case 'Horizontal': return { glyph: 'H', title: 'Horizontal' }
      case 'Vertical': return { glyph: 'V', title: 'Vertical' }
      case 'Perpendicular': return { glyph: '\u22A5', title: 'Perpendicular' }
      case 'Parallel': return { glyph: '\u2225', title: 'Parallel' }
      case 'Coincident': return { glyph: '\u2261', title: 'Coincident' }
      case 'Tangent': return { glyph: 'Tg', title: 'Tangent' }
      case 'Concentric': return { glyph: '\u25CE', title: 'Concentric' }
      case 'Equal': return { glyph: '=', title: 'Equal' }
      case 'Collinear': return { glyph: 'Co', title: 'Collinear' }
      case 'Midpoint': return { glyph: 'M', title: 'Midpoint' }
      case 'Symmetric': return { glyph: 'Sy', title: 'Symmetric' }
      case 'PointOnCurve': return { glyph: '\u2022', title: 'Point on Curve' }
      case 'SmoothTangent': return { glyph: 'G2', title: 'Smooth Tangent (G2)' }
      case 'CurvatureContinuity': return { glyph: 'G3', title: 'Curvature Continuity (G3)' }
      case 'ContactConstraint': return { glyph: 'Ct', title: 'Contact' }
      // Variants below have no canonical viewport anchor — skip.
      case 'Offset':
      case 'MultiTangent':
      case 'EqualArea':
      case 'EqualPerimeter':
      case 'Centroid':
      case 'CurvatureExtremum':
        return null
    }
  }
  if ('IntersectionAngle' in g) {
    const deg = ((g.IntersectionAngle * 180) / Math.PI).toFixed(0)
    return { glyph: '\u2220', title: `Intersection Angle ${deg}\u00B0` }
  }
  return null
}

/**
 * Compute the (u, v) anchor for a constraint's badge: the centroid
 * of its entity representatives. Single-entity constraints anchor at
 * the entity itself; multi-entity constraints anchor at the average
 * so a `Parallel(line1, line2)` badge sits between the two lines.
 *
 * Returns `null` if no entity referenced by the constraint resolves
 * — the summary may lag a constraint mutation by one render frame.
 */
function constraintAnchor(
  refs: EntityRef[],
  points: Map<string, CSketchPointSummary>,
  lines: Map<string, CSketchLineSummary>,
  circles: Map<string, CSketchCircleSummary>,
): [number, number] | null {
  const uvs: [number, number][] = []
  for (const e of refs) {
    if ('Point' in e) {
      const p = points.get(e.Point)
      if (p) uvs.push([p.x, p.y])
    } else if ('Line' in e) {
      const l = lines.get(e.Line)
      if (l) uvs.push(lineRepresentative(l))
    } else if ('Circle' in e) {
      const c = circles.get(e.Circle)
      if (c) uvs.push([c.cx, c.cy])
    }
    // Other entity kinds (Arc, Rectangle, Ellipse, Spline, Polyline)
    // are not yet in the summary surface — once they land, extend
    // this switch.
  }
  if (uvs.length === 0) return null
  const ux = uvs.reduce((s, u) => s + u[0], 0) / uvs.length
  const uy = uvs.reduce((s, u) => s + u[1], 0) / uvs.length
  return [ux, uy]
}

/**
 * Pure derivation: produce one badge per geometric constraint that
 * has a canonical anchor. Badges share the `conflict`/`redundant`
 * classification computed by slice H's `/dof` diagnose pass, so a
 * constraint that participates in a redundant/inconsistent set is
 * tinted to draw the user's eye.
 */
function buildCSketchGeometricBadges(
  summary: CSketchSummary,
  constraints: Constraint[],
  conflictIds: Set<string>,
  redundantIds: Set<string>,
): GeometricBadge[] {
  const pointsById = new Map<string, CSketchPointSummary>()
  for (const p of summary.points) pointsById.set(p.id, p)
  const linesById = new Map<string, CSketchLineSummary>()
  for (const l of summary.lines) linesById.set(l.id, l)
  const circlesById = new Map<string, CSketchCircleSummary>()
  for (const c of summary.circles) circlesById.set(c.id, c)

  const out: GeometricBadge[] = []
  for (const c of constraints) {
    if (!('Geometric' in c.constraint_type)) continue
    const meta = geometricGlyph(c.constraint_type.Geometric)
    if (!meta) continue
    const anchor = constraintAnchor(c.entities, pointsById, linesById, circlesById)
    if (!anchor) continue
    const status: GeometricBadgeStatus = conflictIds.has(c.id)
      ? 'conflict'
      : redundantIds.has(c.id)
        ? 'redundant'
        : 'ok'
    out.push({
      constraintId: c.id,
      glyph: meta.glyph,
      title: meta.title,
      status,
      uv: anchor,
    })
  }
  return out
}

/**
 * Render a small bordered glyph next to every geometric constraint
 * on the active csketch. Slice D-3b — pairs with the dimension
 * labels (N-3) so the user sees the full constraint state, not just
 * the dimensional half.
 *
 * Multiple badges that share the same anchor (e.g. `Horizontal` and
 * `Equal` on the same line endpoint) stack into a single flex row
 * inside one `<Html>`, so they never overlap pixel-for-pixel.
 *
 * Read-only in this slice; right-click-to-delete and click-to-select
 * land in D-3d once the selection state is wired.
 */
function CSketchGeometricBadges({ plane }: { plane: SketchPlane }) {
  const activeId = useSceneStore((s) => s.csketch.activeId)
  const summary = useSceneStore((s) =>
    s.csketch.activeId ? s.csketch.summaries.get(s.csketch.activeId) ?? null : null,
  )
  const constraints = useSceneStore((s) => s.csketch.activeConstraints)
  const dofReport = useSceneStore((s) => s.csketch.lastDofReport)

  const groups = useMemo(() => {
    if (!activeId || !summary) return []
    const conflictIds = new Set(dofReport?.conflicts ?? [])
    const redundantIds = new Set(dofReport?.redundant ?? [])
    const badges = buildCSketchGeometricBadges(
      summary,
      constraints,
      conflictIds,
      redundantIds,
    )
    // Group badges that land at (almost) the same (u, v) so they
    // stack visually instead of overlapping. 1e-4 in sketch units
    // is well below the visual threshold yet captures all "same
    // entity" cases (everything anchored to the same point/line).
    const byKey = new Map<string, GeometricBadge[]>()
    for (const b of badges) {
      const key = `${b.uv[0].toFixed(4)}|${b.uv[1].toFixed(4)}`
      const existing = byKey.get(key)
      if (existing) existing.push(b)
      else byKey.set(key, [b])
    }
    return Array.from(byKey.entries()).map(([key, list]) => ({
      key,
      uv: list[0].uv,
      badges: list,
    }))
  }, [activeId, summary, constraints, dofReport])

  if (groups.length === 0 || !activeId) return null

  return (
    <group name="csketch-geometric-badges">
      {groups.map((g) => (
        <Html
          key={g.key}
          position={uvToWorld(g.uv, plane)}
          center
          pointerEvents="none"
          zIndexRange={[100, 0]}
        >
          <div className="flex items-center gap-0.5 select-none">
            {g.badges.map((b) => {
              const tone =
                b.status === 'conflict'
                  ? 'border-rose-400/60 text-rose-300 bg-rose-950/40'
                  : b.status === 'redundant'
                    ? 'border-amber-400/50 text-amber-300 bg-amber-950/40'
                    : 'border-border/60 text-foreground bg-background/80'
              return (
                <div
                  key={b.constraintId}
                  title={b.title}
                  className={`px-1 min-w-[16px] h-4 inline-flex items-center justify-center text-[9px] font-mono font-semibold uppercase tracking-tight border ${tone} backdrop-blur-sm`}
                >
                  {b.glyph}
                </div>
              )
            })}
          </div>
        </Html>
      ))}
    </group>
  )
}

// ─── Draggable csketch point handles ─────────────────────────────────

/**
 * Build a `THREE.Plane` representing the active sketch plane in world
 * coordinates. Mirrors the `pointToPlaneUV` / `uvToWorld` convention:
 *   xy → origin (0,0,0), normal +Z
 *   xz → origin (0,0,0), normal +Y
 *   yz → origin (0,0,0), normal +X
 *   custom → origin = plane.origin, normal = u_axis × v_axis (which
 *            the backend guarantees orthonormal so the cross is unit).
 *
 * Pure function of the input plane; safe to call inside event handlers.
 */
function sketchPlaneAsThreePlane(plane: SketchPlane): THREE.Plane {
  const normal = new THREE.Vector3()
  const origin = new THREE.Vector3()
  if (isStandardPlane(plane)) {
    switch (plane) {
      case 'xy': normal.set(0, 0, 1); break
      case 'xz': normal.set(0, 1, 0); break
      case 'yz': normal.set(1, 0, 0); break
    }
  } else {
    origin.set(...plane.origin)
    normal
      .crossVectors(
        new THREE.Vector3(...plane.u_axis),
        new THREE.Vector3(...plane.v_axis),
      )
      .normalize()
  }
  return new THREE.Plane().setFromNormalAndCoplanarPoint(normal, origin)
}

/**
 * Visible disc handle plus per-point drag-on-pointerdown gesture.
 * Renders nothing if there is no active csketch. The capture-plane
 * pointerdown handler does NOT fire on point picks because each
 * handle calls `e.stopPropagation()` — the click-to-place flow on the
 * sketch plane is therefore unaffected.
 *
 * Drag pipeline (per point):
 *   1. pointerdown on the disc → suppress orbit (`gizmoDragging=true`),
 *      record pointer-down NDC, install window pointermove + pointerup.
 *   2. pointermove → raycast cursor onto the sketch plane, project to
 *      (u, v). Only fires the backend call once travel exceeds the
 *      4-px click-vs-drag budget (matches mainstream CAD).
 *   3. Backend call is throttled to "one in flight at a time"; later
 *      moves overwrite a `pendingUV` so the final position is always
 *      delivered, but we never stack requests on a slow solver.
 *   4. pointerup → release suppression, run the trailing pendingUV
 *      (if any), drop the window listeners.
 *
 * Fixed points (`is_fixed == true`) render as a smaller grey disc with
 * no pointer handlers — the solver pins them and dragging them would
 * either fail constraint-check or be a no-op, neither of which is
 * useful UX.
 */
function CSketchPoints({ plane }: { plane: SketchPlane }) {
  const activeId = useSceneStore((s) => s.csketch.activeId)
  const summary = useSceneStore((s) =>
    s.csketch.activeId ? s.csketch.summaries.get(s.csketch.activeId) ?? null : null,
  )
  const dragCSketch = useSceneStore((s) => s.dragCSketch)
  const setGizmoDragging = useSceneStore((s) => s.setGizmoDragging)
  const { camera, gl } = useThree()

  // `null` = no drag, `{...}` = drag in progress. Stored in a ref so
  // updates don't trigger re-renders mid-gesture.
  const dragRef = useRef<{
    pointId: string
    downX: number
    downY: number
    started: boolean
    inFlight: boolean
    pendingUV: [number, number] | null
  } | null>(null)
  const [hoverId, setHoverId] = useState<string | null>(null)
  const [draggingId, setDraggingId] = useState<string | null>(null)

  // Trailing-throttle dispatcher: at most one drag POST in flight per
  // point. When a move arrives during an in-flight request we cache
  // the latest UV in `pendingUV`; the `.finally()` chain flushes that
  // cached value, guaranteeing the final cursor position always wins.
  const dispatchDrag = useCallback(
    (pointId: string, uv: [number, number]) => {
      if (!activeId) return
      const drag = dragRef.current
      if (!drag || drag.pointId !== pointId) return
      if (drag.inFlight) {
        drag.pendingUV = uv
        return
      }
      drag.inFlight = true
      const fire = (target: [number, number]) => {
        return dragCSketch(activeId, {
          entity: pointRef(pointId),
          target: { kind: 'point', params: { x: target[0], y: target[1] } },
        }).catch((err) => {
          // eslint-disable-next-line no-console
          console.error('[CSketchPoints] drag failed:', err)
        })
      }
      const chain = (uvNow: [number, number]) => {
        fire(uvNow).finally(() => {
          const live = dragRef.current
          if (!live || live.pointId !== pointId) {
            // Drag ended before this request returned — done.
            if (drag) drag.inFlight = false
            return
          }
          if (live.pendingUV) {
            const next = live.pendingUV
            live.pendingUV = null
            chain(next)
          } else {
            live.inFlight = false
          }
        })
      }
      chain(uv)
    },
    [activeId, dragCSketch],
  )

  // Window-level pointer listeners for the duration of a drag.
  // Registered only while `draggingId` is set so non-dragging time
  // costs no event-loop work.
  useEffect(() => {
    if (!draggingId) return

    const ndc = new THREE.Vector2()
    const raycaster = new THREE.Raycaster()
    const intersection = new THREE.Vector3()
    const worldPlane = sketchPlaneAsThreePlane(plane)

    const onMove = (e: PointerEvent) => {
      const drag = dragRef.current
      if (!drag) return
      const dx = e.clientX - drag.downX
      const dy = e.clientY - drag.downY
      if (!drag.started && dx * dx + dy * dy < 16) {
        // Below the 4-px click-vs-drag threshold — don't emit yet.
        return
      }
      drag.started = true
      const rect = gl.domElement.getBoundingClientRect()
      ndc.x = ((e.clientX - rect.left) / rect.width) * 2 - 1
      ndc.y = -((e.clientY - rect.top) / rect.height) * 2 + 1
      raycaster.setFromCamera(ndc, camera)
      if (!raycaster.ray.intersectPlane(worldPlane, intersection)) return
      const uv = pointToPlaneUV(intersection, plane)
      dispatchDrag(drag.pointId, uv)
    }

    const onUp = () => {
      const drag = dragRef.current
      dragRef.current = null
      setDraggingId(null)
      setGizmoDragging(false)
      if (drag && drag.started && drag.pendingUV) {
        // Final flush — pendingUV captured during in-flight request
        // never made it to the wire. Send it now so the resting
        // position matches the last cursor pose.
        const final = drag.pendingUV
        drag.pendingUV = null
        if (activeId) {
          void dragCSketch(activeId, {
            entity: pointRef(drag.pointId),
            target: { kind: 'point', params: { x: final[0], y: final[1] } },
          }).catch((err) => {
            // eslint-disable-next-line no-console
            console.error('[CSketchPoints] final drag failed:', err)
          })
        }
      }
    }

    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    return () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
    }
  }, [draggingId, plane, camera, gl, dispatchDrag, dragCSketch, setGizmoDragging, activeId])

  const handlePointerDown = useCallback(
    (point: CSketchPointSummary) => (e: ThreeEvent<PointerEvent>) => {
      if (point.is_fixed) return
      if (e.nativeEvent.button !== 0) return
      e.stopPropagation()
      dragRef.current = {
        pointId: point.id,
        downX: e.nativeEvent.clientX,
        downY: e.nativeEvent.clientY,
        started: false,
        inFlight: false,
        pendingUV: null,
      }
      setDraggingId(point.id)
      setGizmoDragging(true)
    },
    [setGizmoDragging],
  )

  const handlePointerOver = useCallback(
    (point: CSketchPointSummary) => (e: ThreeEvent<PointerEvent>) => {
      if (point.is_fixed) return
      e.stopPropagation()
      setHoverId(point.id)
    },
    [],
  )

  const handlePointerOut = useCallback(
    (point: CSketchPointSummary) => () => {
      if (point.is_fixed) return
      setHoverId((prev) => (prev === point.id ? null : prev))
    },
    [],
  )

  if (!activeId || !summary || summary.points.length === 0) return null

  return (
    <group name="csketch-points">
      {summary.points.map((p) => {
        const world = uvToWorld([p.x, p.y], plane)
        const isDragging = draggingId === p.id
        const isHover = hoverId === p.id
        // Visual states — order matters: dragging beats hover beats
        // construction, all override the default solid white.
        const color = p.is_fixed
          ? '#6b7280' // gray-500 — pinned, not draggable
          : isDragging
            ? '#fb923c' // orange-400 — live drag
            : isHover
              ? '#22d3ee' // cyan-400 — hover lock-on
              : p.is_construction
                ? '#a78bfa' // violet-400 — construction-only point
                : '#f5f5f5' // neutral-100 — default
        const radius = p.is_fixed ? 0.18 : 0.24
        return (
          <mesh
            key={p.id}
            position={world}
            onPointerDown={p.is_fixed ? undefined : handlePointerDown(p)}
            onPointerOver={p.is_fixed ? undefined : handlePointerOver(p)}
            onPointerOut={p.is_fixed ? undefined : handlePointerOut(p)}
          >
            <sphereGeometry args={[radius, 12, 12]} />
            <meshBasicMaterial
              color={color}
              transparent
              opacity={p.is_fixed ? 0.7 : 0.95}
              depthTest={false}
              depthWrite={false}
            />
          </mesh>
        )
      })}
    </group>
  )
}

// ─── csketch line + circle visuals (D-2-b) ──────────────────────────
//
// Read-only renderers for the active csketch's lines and circles.
// Mirrors the `CSketchPoints` discipline: subscribe to the active
// summary, lift each (u, v) to world space, render with drei's
// `<Line>` for crisp resolution-independent strokes. Construction
// entities use a softer violet, regular ones use neutral white so
// they read clearly against the sketch plane tint.

const CSKETCH_CIRCLE_SEGMENTS = 64

/**
 * Render every `LineSegment2d` from the active csketch summary. Lines
 * whose `geometry` is `Infinite` or `Ray` are skipped — they have no
 * intrinsic endpoints to draw without clipping against the sketch
 * plane border, and the existing csketch entry points only ever
 * produce `Segment` lines. When that changes a future slice can add
 * the clip path.
 */
function CSketchLines({ plane }: { plane: SketchPlane }) {
  const activeId = useSceneStore((s) => s.csketch.activeId)
  const summary = useSceneStore((s) =>
    s.csketch.activeId
      ? s.csketch.summaries.get(s.csketch.activeId) ?? null
      : null,
  )
  if (!activeId || !summary || summary.lines.length === 0) return null

  return (
    <group name="csketch-lines">
      {summary.lines.map((l) => {
        if (!('Segment' in l.geometry)) return null
        const seg = l.geometry.Segment
        const a = uvToWorld([seg.start.x, seg.start.y], plane)
        const b = uvToWorld([seg.end.x, seg.end.y], plane)
        const color = l.is_construction ? '#a78bfa' : '#f5f5f5'
        return (
          <Line
            key={l.id}
            points={[a, b]}
            color={color}
            lineWidth={1.5}
            transparent
            opacity={l.is_construction ? 0.7 : 0.95}
            depthTest={false}
            depthWrite={false}
          />
        )
      })}
    </group>
  )
}

/**
 * Render every csketch circle as a closed 64-segment polyline lifted
 * onto the active sketch plane. The kernel does not pre-tessellate
 * circles — they're parametric (centre + radius) on the wire — so
 * we sample at draw time. 64 segments is the same fidelity the
 * legacy click-to-place circle uses for its extrude profile and
 * keeps a 10-unit-radius circle visibly smooth at default zoom.
 */
function CSketchCircles({ plane }: { plane: SketchPlane }) {
  const activeId = useSceneStore((s) => s.csketch.activeId)
  const summary = useSceneStore((s) =>
    s.csketch.activeId
      ? s.csketch.summaries.get(s.csketch.activeId) ?? null
      : null,
  )
  if (!activeId || !summary || summary.circles.length === 0) return null

  return (
    <group name="csketch-circles">
      {summary.circles.map((c) => {
        const pts: THREE.Vector3[] = []
        for (let i = 0; i <= CSKETCH_CIRCLE_SEGMENTS; i++) {
          const t = (i / CSKETCH_CIRCLE_SEGMENTS) * Math.PI * 2
          const u = c.cx + c.radius * Math.cos(t)
          const v = c.cy + c.radius * Math.sin(t)
          pts.push(uvToWorld([u, v], plane))
        }
        const color = c.is_construction ? '#a78bfa' : '#f5f5f5'
        return (
          <Line
            key={c.id}
            points={pts}
            color={color}
            lineWidth={1.5}
            transparent
            opacity={c.is_construction ? 0.7 : 0.95}
            depthTest={false}
            depthWrite={false}
          />
        )
      })}
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

// ─── csketch snap glyph (D-2-c) ──────────────────────────────────────

const CSKETCH_GLYPH_SIZE = 0.5

interface CSketchSnapMarkerProps {
  candidate: SnapCandidate
  plane: SketchPlane
}

/**
 * Kind-aware glyph at the locked csketch snap point. The kernel's
 * `SnapKind::priority()` partitions variants into three tiers:
 *
 *   - 0 (vertex-like)     → filled square
 *   - 1 (centre)          → ring
 *   - 1 (mid / quadrant)  → diamond
 *   - 2 (on-curve)        → small cross
 *
 * Tier 0/1 glyphs read as "discrete attachment" while tier-2
 * reads as "anywhere along this curve". Colour is uniform cyan so
 * the tier hierarchy travels through shape alone (matches the
 * legacy `SnapMarker` palette).
 */
function CSketchSnapMarker({ candidate, plane }: CSketchSnapMarkerProps) {
  const pos = uvToWorld([candidate.point.x, candidate.point.y], plane)
  const quat = planeMeshQuaternion(plane)
  const color = '#22d3ee' // cyan-400
  const S = CSKETCH_GLYPH_SIZE

  const glyphCategory = ((): 'vertex' | 'centre' | 'midquad' | 'oncurve' => {
    switch (candidate.kind) {
      case 'point':
      case 'line_endpoint':
      case 'arc_endpoint':
      case 'rectangle_corner':
        return 'vertex'
      case 'circle_center':
      case 'arc_center':
      case 'ellipse_center':
      case 'rectangle_center':
        return 'centre'
      case 'line_midpoint':
      case 'arc_midpoint':
      case 'circle_quadrant':
      case 'ellipse_quadrant':
      case 'rectangle_edge_midpoint':
        return 'midquad'
      case 'on_line':
      case 'on_circle':
      case 'on_arc':
      case 'on_ellipse':
        return 'oncurve'
    }
  })()

  // All glyphs share the in-plane orientation. depthTest off so
  // the marker draws on top of already-committed csketch geometry
  // — same convention as the legacy `SnapMarker` ring.
  return (
    <group position={pos} quaternion={quat}>
      {glyphCategory === 'vertex' && (
        <mesh>
          <planeGeometry args={[S, S]} />
          <meshBasicMaterial
            color={color}
            depthTest={false}
            transparent
            opacity={0.9}
            side={THREE.DoubleSide}
          />
        </mesh>
      )}
      {glyphCategory === 'centre' && (
        <mesh>
          <torusGeometry args={[S * 0.55, S * 0.08, 8, 24]} />
          <meshBasicMaterial
            color={color}
            depthTest={false}
            transparent
            opacity={0.9}
          />
        </mesh>
      )}
      {glyphCategory === 'midquad' && (
        <mesh rotation={[0, 0, Math.PI / 4]}>
          <planeGeometry args={[S * 0.85, S * 0.85]} />
          <meshBasicMaterial
            color={color}
            depthTest={false}
            transparent
            opacity={0.9}
            side={THREE.DoubleSide}
          />
        </mesh>
      )}
      {glyphCategory === 'oncurve' && (
        <>
          <mesh rotation={[0, 0, Math.PI / 4]}>
            <planeGeometry args={[S * 0.9, S * 0.14]} />
            <meshBasicMaterial
              color={color}
              depthTest={false}
              transparent
              opacity={0.9}
              side={THREE.DoubleSide}
            />
          </mesh>
          <mesh rotation={[0, 0, -Math.PI / 4]}>
            <planeGeometry args={[S * 0.9, S * 0.14]} />
            <meshBasicMaterial
              color={color}
              depthTest={false}
              transparent
              opacity={0.9}
              side={THREE.DoubleSide}
            />
          </mesh>
        </>
      )}
    </group>
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
