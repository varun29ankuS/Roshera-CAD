/**
 * Floating control panel for the interactive 2D sketch tool. Visible
 * only while `sketch.active`. Provides:
 *   - Plane selector (XY / XZ / YZ)
 *   - Tool selector (polyline / rectangle / circle)
 *   - Snap step input
 *   - Measure toggle (extra dimensions: angles, perimeter, area)
 *   - Thickness input
 *   - Buttons: Finish & Extrude · Undo · Clear · Cancel
 *
 * Keyboard shortcuts (handled here):
 *   1 / 2 / 3       → polyline / rectangle / circle
 *   Enter           → Finish & Extrude
 *   Backspace       → Undo last point
 *   Esc             → Cancel (exit sketch)
 *
 * The in-canvas overlay (`SketchOverlay`) reads the same `snapStep`
 * and `measure` flags from the store, so this panel only needs to
 * mutate them.
 */

import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  PenTool,
  RectangleHorizontal,
  Circle as CircleIcon,
  Ruler,
  Undo2,
  Trash2,
  Check,
  X,
  Scissors,
  RotateCw,
  Dot,
  Minus,
} from 'lucide-react'
import {
  isStandardPlane,
  useSceneStore,
  type CSketchTool,
  type SketchPlane,
  type SketchTool,
  type StandardPlane,
} from '@/stores/scene-store'
import { useChatStore } from '@/stores/chat-store'
import {
  buildProfile2D,
  perimeter,
  signedArea,
} from '@/lib/sketch-extrude'
import { sketchApi } from '@/lib/sketch-api'
import { cn } from '@/lib/utils'

const PLANE_OPTIONS: Array<{ value: StandardPlane; label: string }> = [
  { value: 'xy', label: 'XY' },
  { value: 'xz', label: 'XZ' },
  { value: 'yz', label: 'YZ' },
]

/**
 * Human-readable label for the active plane. Standard planes get the
 * usual XY/XZ/YZ. Custom (face-anchored) planes don't carry a name on
 * the wire, so we render a stable "FACE" tag — enough for the chat
 * confirmation and any future status text without leaking origin /
 * basis numbers into the UI.
 */
function planeLabel(plane: SketchPlane): string {
  return isStandardPlane(plane) ? plane.toUpperCase() : 'FACE'
}

const TOOL_OPTIONS: Array<{
  value: SketchTool
  label: string
  icon: typeof PenTool
}> = [
  { value: 'polyline', label: 'Polyline', icon: PenTool },
  { value: 'rectangle', label: 'Rectangle', icon: RectangleHorizontal },
  { value: 'circle', label: 'Circle', icon: CircleIcon },
]

/**
 * Draw tools for the parametric csketch (D-2-b). These speak the
 * `/api/csketch/{id}/{point,line,circle}` REST surface — distinct
 * from the legacy click-to-place TOOL_OPTIONS above which target
 * `/api/sketch`. The row only renders when a csketch is open
 * (`csketch.activeId !== null`); the buttons toggle on click so
 * clicking the active tool again returns the editor to read-only /
 * drag mode.
 */
const CSKETCH_TOOL_OPTIONS: Array<{
  value: CSketchTool
  label: string
  icon: typeof PenTool
}> = [
  { value: 'point', label: 'Point', icon: Dot },
  { value: 'line', label: 'Line', icon: Minus },
  { value: 'circle', label: 'Circle', icon: CircleIcon },
]

/**
 * Three ways to finalise a sketch into 3D geometry. `extrude` is the
 * existing default (build a prism from the profile). `extrude_cut`
 * builds the same prism but boolean-subtracts it from the selected
 * target solid. `revolve` spins the profile around a world-space
 * axis. Sweep/Loft live in a later slice (J-5) because they need
 * cross-sketch coordination.
 */
type FinishOp = 'extrude' | 'extrude_cut' | 'revolve'

const FINISH_OP_OPTIONS: Array<{
  value: FinishOp
  label: string
  icon: typeof PenTool
  title: string
}> = [
  { value: 'extrude', label: 'Extrude', icon: Check, title: 'Pull the profile along the plane normal' },
  { value: 'extrude_cut', label: 'Cut', icon: Scissors, title: 'Pull the profile and subtract it from a target body' },
  { value: 'revolve', label: 'Revolve', icon: RotateCw, title: 'Spin the profile around an axis' },
]

export function SketchPanel() {
  const sketch = useSceneStore((s) => s.sketch)
  const setSketchTool = useSceneStore((s) => s.setSketchTool)
  const setSketchPlane = useSceneStore((s) => s.setSketchPlane)
  const popSketchPoint = useSceneStore((s) => s.popSketchPoint)
  const clearSketchPoints = useSceneStore((s) => s.clearSketchPoints)
  const exitSketch = useSceneStore((s) => s.exitSketch)
  const setSketchView = useSceneStore((s) => s.setSketchView)
  const setExtrudeHover = useSceneStore((s) => s.setExtrudeHover)
  const setSketchPoint = useSceneStore((s) => s.setSketchPoint)
  const awaitSketchReady = useSceneStore((s) => s.awaitSketchReady)
  // Cut target resolution: first selection wins; falls back to the
  // most recently created solid if nothing is selected (matches the
  // user mental model of "I just drew this, now cut from it").
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const objectOrder = useSceneStore((s) => s.objectOrder)

  // Constrained-sketch (csketch) tool state (D-2-b). The csketch tool
  // row only renders when a csketch is open; toggling the active tool
  // returns the editor to read-only / drag mode.
  const csketchActiveId = useSceneStore((s) => s.csketch.activeId)
  const csketchActiveTool = useSceneStore((s) => s.csketch.activeTool)
  const setCSketchTool = useSceneStore((s) => s.setCSketchTool)

  const [busy, setBusy] = useState<boolean>(false)
  const [error, setError] = useState<string | null>(null)
  const [finishOp, setFinishOp] = useState<FinishOp>('extrude')

  // Revolve axis: default to the world X-axis through the origin —
  // works for any plane-aligned sketch as a starting point. The user
  // edits the six numbers below if their axis differs. (Selecting a
  // sketch line as the axis is a follow-up slice — needs an extra
  // selection mode in the overlay.)
  const [revolveAngleDeg, setRevolveAngleDeg] = useState<number>(360)
  const [revolveSegments, setRevolveSegments] = useState<number>(32)
  const [revolveAxisOrigin, setRevolveAxisOrigin] =
    useState<[number, number, number]>([0, 0, 0])
  const [revolveAxisDir, setRevolveAxisDir] =
    useState<[number, number, number]>([1, 0, 0])

  // Resolve the cut target: prefer first selected solid, fall back
  // to the last-created one. `null` if the scene is empty.
  const cutTargetId = useMemo<string | null>(() => {
    for (const id of selectedIds) return id
    if (objectOrder.length === 0) return null
    return objectOrder[objectOrder.length - 1] ?? null
  }, [selectedIds, objectOrder])

  // Reset transient feedback every time we (re)enter sketch mode.
  useEffect(() => {
    if (sketch.active) {
      setError(null)
      setBusy(false)
    }
  }, [sketch.active])

  const handleFinish = useCallback(async () => {
    if (busy) return
    setError(null)
    // Local guard 1: every shape must materialise to a valid polygon.
    // Backend re-validates per shape (the source of truth), so this
    // is just to surface "shape #2 has only 1 point" as a panel error
    // before the round-trip rather than after.
    //
    // Trailing empty shapes (the auto-commit "on-deck" shape the
    // overlay creates after polyline closure or rect/circle complete)
    // are filtered out — they exist only as drawing affordances and
    // the backend now skips them too. Without this filter, hitting
    // Enter right after closing a loop would surface a confusing
    // "Shape 2 needs more points" error for a shape the user never
    // intended to draw.
    let drawn = 0
    for (let i = 0; i < sketch.shapes.length; i += 1) {
      const s = sketch.shapes[i]
      if (s.points.length === 0) continue
      const profile = buildProfile2D(s.tool, s.points, sketch.circleSegments)
      if (!profile) {
        setError(`Shape ${i + 1} needs more points`)
        return
      }
      drawn += 1
    }
    if (drawn === 0) {
      setError('Draw at least one closed shape before finishing.')
      return
    }
    setBusy(true)
    let serverId: string
    try {
      // Wait for the backend create round-trip to land and the
      // pending-op queue to drain. Resolves immediately if serverId
      // is already set; otherwise blocks until `enterSketch`'s create
      // promise resolves (and any operations the user fired in that
      // window have replayed in order). This is the "Preparing sketch
      // session" race fix — a fast user can no longer hit Finish
      // before the session exists.
      serverId = await awaitSketchReady()
    } catch (err) {
      setBusy(false)
      const msg = err instanceof Error ? err.message : String(err)
      setError(`Sketch session unavailable: ${msg}`)
      return
    }
    try {
      // Re-edit replace semantics: when this Finish is closing out an
      // edit pass on an existing feature (sketch was opened via "Edit
      // sketch" from the model tree), delete the prior solid first so
      // the upcoming extrude *replaces* it rather than appending a
      // duplicate alongside. The DELETE cascades to a kernel
      // `delete_solid` + `ObjectDeleted` broadcast; the WS bridge
      // removes the object from the local store.
      if (sketch.editingSourceObjectId) {
        const apiBase = import.meta.env.VITE_API_URL || ''
        try {
          const resp = await fetch(
            `${apiBase}/api/geometry/${sketch.editingSourceObjectId}`,
            { method: 'DELETE' },
          )
          if (!resp.ok) {
            // 404 here is fine — the prior solid was already removed
            // (e.g. via the model tree's Delete action mid-edit). Any
            // other status is logged but we still proceed; the user
            // gets a duplicate at worst, not a hard failure.
            if (resp.status !== 404) {
              console.warn(
                '[sketch] re-edit: failed to delete prior solid',
                sketch.editingSourceObjectId,
                resp.status,
              )
            }
          }
        } catch (err) {
          console.warn('[sketch] re-edit: delete prior solid threw', err)
        }
      }

      // `consume: false` keeps the sketch session alive on the backend
      // after the extrude lands, so the user can right-click the
      // resulting feature in the model tree → "Edit sketch" and reopen
      // the same profile. Without this, the backend deletes the
      // session (default behaviour) and re-edit silently fails because
      // there's nothing to load.
      const { addMessage } = useChatStore.getState()
      let affectedId: string

      if (finishOp === 'extrude_cut') {
        if (!cutTargetId) {
          setBusy(false)
          setError('Select a body to cut from (click it in the viewport), then retry.')
          return
        }
        const result = await sketchApi.extrudeCut(serverId, {
          distance: sketch.thickness,
          target_id: cutTargetId,
          consume: false,
        })
        affectedId = result.target_id
        addMessage({
          role: 'assistant',
          content: `Cut ${sketch.tool} (t=${sketch.thickness}) from body ${cutTargetId.slice(0, 8)} on ${planeLabel(sketch.plane)} plane.`,
          objectsAffected: [affectedId],
        })
      } else if (finishOp === 'revolve') {
        const dirMag = Math.hypot(...revolveAxisDir)
        if (!Number.isFinite(dirMag) || dirMag < 1e-9) {
          setBusy(false)
          setError('Revolve axis direction must be non-zero.')
          return
        }
        const angleRad = (revolveAngleDeg * Math.PI) / 180
        const result = await sketchApi.revolve(serverId, {
          axis_origin: revolveAxisOrigin,
          axis_direction: revolveAxisDir,
          angle: angleRad,
          segments: revolveSegments,
          name: `${sketch.tool}-revolve`,
          consume: false,
        })
        affectedId = result.object.id
        const stats =
          result.stats?.vertex_count && result.stats?.triangle_count
            ? ` (${result.stats.vertex_count} verts, ${result.stats.triangle_count} tris${
                result.stats.tessellation_ms ? `, ${result.stats.tessellation_ms} ms` : ''
              })`
            : ''
        addMessage({
          role: 'assistant',
          content: `Revolved ${sketch.tool} on ${planeLabel(sketch.plane)} plane (${revolveAngleDeg}°)${stats}.`,
          objectsAffected: [affectedId],
        })
      } else {
        const result = await sketchApi.extrude(serverId, {
          distance: sketch.thickness,
          name: `${sketch.tool}-extrude`,
          consume: false,
        })
        affectedId = result.object.id
        const stats =
          result.stats?.vertex_count && result.stats?.triangle_count
            ? ` (${result.stats.vertex_count} verts, ${result.stats.triangle_count} tris${
                result.stats.tessellation_ms ? `, ${result.stats.tessellation_ms} ms` : ''
              })`
            : ''
        addMessage({
          role: 'assistant',
          content: `Extruded ${sketch.tool} on ${planeLabel(sketch.plane)} plane (t=${sketch.thickness})${stats}.`,
          objectsAffected: [affectedId],
        })
      }
      // We pass `consume: false` above, so the backend keeps the
      // session alive — but the user is done with this editing pass,
      // so tear down the local active flag. Pass `deleteBackend:
      // false` so the in-memory `SketchSession` survives; otherwise
      // the default `exitSketch` path issues `DELETE /api/sketch/{id}`
      // and the "Edit sketch" lookup later 404s.
      exitSketch({ deleteBackend: false })
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setError(msg)
    } finally {
      setBusy(false)
    }
  }, [
    awaitSketchReady,
    busy,
    cutTargetId,
    exitSketch,
    finishOp,
    revolveAngleDeg,
    revolveAxisDir,
    revolveAxisOrigin,
    revolveSegments,
    sketch.circleSegments,
    sketch.editingSourceObjectId,
    sketch.plane,
    sketch.points,
    sketch.shapes,
    sketch.thickness,
    sketch.tool,
  ])

  // Keyboard shortcuts — guarded against typing inside inputs.
  useEffect(() => {
    if (!sketch.active) return
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        exitSketch()
      } else if (e.key === 'Enter') {
        e.preventDefault()
        void handleFinish()
      } else if (e.key === 'Backspace' || e.key === 'Delete') {
        e.preventDefault()
        popSketchPoint()
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z' && !e.shiftKey) {
        // Ctrl+Z (Cmd+Z on macOS) inside sketch mode pops the last
        // confirmed point. Same effect as Backspace, mapped to the
        // standard undo gesture.
        e.preventDefault()
        popSketchPoint()
      } else if (e.key === '1') {
        setSketchTool('polyline')
      } else if (e.key === '2') {
        setSketchTool('rectangle')
      } else if (e.key === '3') {
        setSketchTool('circle')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [sketch.active, exitSketch, popSketchPoint, setSketchTool, handleFinish])

  // Live measurements summary for the bottom of the panel.
  // Important: compute these from the *materialised* profile (rectangle
  // has 4 corners, circle has N edge samples) — using the raw click
  // points would give nonsense for rectangle/circle since the user
  // only ever places 2 anchor points for those.
  const summary = useMemo(() => {
    if (sketch.points.length < 2) return null
    const profile = buildProfile2D(
      sketch.tool,
      sketch.points,
      sketch.circleSegments,
    )
    if (!profile) return null
    const closed = profile.length >= 3
    const peri = perimeter(profile, closed)
    const area = closed ? Math.abs(signedArea(profile)) : 0
    return { count: profile.length, perimeter: peri, area, closed }
  }, [sketch.tool, sketch.points, sketch.circleSegments])

  // Whether Finish is allowed: at least one shape (active or already
  // committed) materialises to a valid polygon. Tracks the same
  // "skip empty trailing shapes" rule `handleFinish` applies, so a
  // closed polyline followed by the auto-created on-deck shape still
  // enables the button.
  const hasFinishableShape = useMemo(() => {
    for (const s of sketch.shapes) {
      if (s.points.length === 0) continue
      const profile = buildProfile2D(s.tool, s.points, sketch.circleSegments)
      if (profile) return true
    }
    return false
  }, [sketch.shapes, sketch.circleSegments])

  if (!sketch.active) return null

  // Hints surface the multi-shape flow: completing a shape rolls
  // into a new one of the same tool automatically, and the sketch
  // only ends when the user explicitly Finishes (Enter) or Cancels
  // (Esc). For polyline the close trigger is clicking back at the
  // first point (snap engine already attracts the cursor there);
  // for rectangle / circle the next click after the second anchor
  // commits and starts a fresh shape.
  // After a polyline closes (or rect/circle completes), the active
  // shape is reset to empty and `hasFinishableShape` is true thanks
  // to the just-committed loop. Surface that state explicitly so the
  // user doesn't think "Click 3 more points" means their previous
  // loop didn't register.
  const justClosed =
    sketch.points.length === 0 && hasFinishableShape
  const hint = justClosed
    ? 'Loop closed · keep drawing or press Enter to finish & extrude'
    : sketch.tool === 'polyline'
      ? sketch.points.length < 3
        ? `Click ${3 - sketch.points.length} more point${sketch.points.length === 2 ? '' : 's'} · Enter to finish & extrude`
        : 'Click first point to close loop · Enter to finish & extrude'
      : sketch.tool === 'rectangle'
        ? sketch.points.length < 1
          ? 'Click first corner · keep going for more rectangles'
          : sketch.points.length < 2
            ? 'Click opposite corner'
            : 'Click again for another rectangle · Enter to finish & extrude'
        : sketch.points.length < 1
          ? 'Click circle center · keep going for more circles'
          : sketch.points.length < 2
            ? 'Click radius point'
            : 'Click again for another circle · Enter to finish & extrude'

  return (
    <div className="absolute top-3 left-1/2 -translate-x-1/2 pointer-events-auto cad-panel px-3 py-2 flex flex-col gap-2 text-[11px] uppercase tracking-wider min-w-[460px] max-w-[640px]">
      {/* Top row: title + plane + close */}
      <div className="flex items-center gap-3">
        <span className="w-2 h-2 rounded-full bg-foreground animate-pulse" />
        <span className="text-foreground font-semibold">Sketch</span>
        <span className="text-muted-foreground normal-case tracking-normal">
          {hint}
        </span>
        <div className="ml-auto flex items-center gap-1">
          {!isStandardPlane(sketch.plane) && (
            <span
              className="px-2 py-0.5 border border-amber-400/60 text-amber-300 bg-amber-500/10 text-[10px] font-mono"
              title="Sketch is anchored to a model face. Click XY/XZ/YZ to pivot off the face."
            >
              FACE
            </span>
          )}
          {PLANE_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              type="button"
              onClick={() => setSketchPlane(opt.value)}
              className={cn(
                'px-2 py-0.5 border text-[10px] font-mono transition-colors',
                isStandardPlane(sketch.plane) && sketch.plane === opt.value
                  ? 'border-border text-foreground bg-foreground/10'
                  : 'border-border/40 text-muted-foreground hover:text-foreground hover:border-border/80',
              )}
              title={`Sketch on ${opt.label} plane`}
            >
              {opt.label}
            </button>
          ))}
          <button
            type="button"
            onClick={() => exitSketch()}
            className="ml-1 p-1 border border-border/40 text-muted-foreground hover:text-foreground hover:border-border transition-colors"
            title="Cancel sketch (Esc)"
            aria-label="Cancel sketch"
          >
            <X className="w-3 h-3" />
          </button>
        </div>
      </div>

      {/* Tool selector + numeric inputs + measure */}
      <div className="flex items-center gap-1">
        {TOOL_OPTIONS.map((opt, i) => {
          const Icon = opt.icon
          return (
            <button
              key={opt.value}
              type="button"
              onClick={() => setSketchTool(opt.value)}
              className={cn(
                'flex items-center gap-1.5 px-2 py-1 border text-[10px] transition-colors',
                sketch.tool === opt.value
                  ? 'border-border text-foreground bg-foreground/10'
                  : 'border-border/40 text-muted-foreground hover:text-foreground hover:border-border/80',
              )}
              title={`${opt.label} (${i + 1})`}
            >
              <Icon className="w-3 h-3" />
              <span>{opt.label}</span>
              <span className="text-muted-foreground/60 ml-1">{i + 1}</span>
            </button>
          )
        })}

        <div className="ml-auto flex items-center gap-2">
          <NumberField
            label="Snap"
            value={sketch.snapStep}
            onChange={(n) => setSketchView({ snapStep: Math.max(0, n) })}
            min={0}
            step={0.1}
          />
          <NumberField
            label="Thick"
            value={sketch.thickness}
            onChange={(n) => setSketchView({ thickness: n })}
            min={0.001}
            step={0.5}
          />
          <button
            type="button"
            onClick={() => setSketchView({ measure: !sketch.measure })}
            className={cn(
              'flex items-center gap-1 px-2 py-1 border text-[10px] transition-colors',
              sketch.measure
                ? 'border-amber-400/60 text-amber-300 bg-amber-500/10'
                : 'border-border/40 text-muted-foreground hover:text-foreground hover:border-border/80',
            )}
            title="Show angles, perimeter, area"
            aria-pressed={sketch.measure}
          >
            <Ruler className="w-3 h-3" />
            <span>Measure</span>
          </button>
        </div>
      </div>

      {/* Constrained-sketch (csketch) tool row (D-2-b). Renders only
          when a csketch is open. Clicking the active tool again
          deselects it (toggle behaviour) and returns the editor to
          read-only / drag mode. Selecting a tool routes capture-plane
          clicks through `csketchApi.addPoint` / `addLine` /
          `addCircle` instead of the legacy sketch handler. */}
      {csketchActiveId !== null && (
        <div className="flex items-center gap-1">
          <span className="text-[10px] text-muted-foreground/80 mr-1">
            cSketch
          </span>
          {CSKETCH_TOOL_OPTIONS.map((opt) => {
            const Icon = opt.icon
            const active = csketchActiveTool === opt.value
            return (
              <button
                key={opt.value}
                type="button"
                onClick={() =>
                  setCSketchTool(active ? null : opt.value)
                }
                className={cn(
                  'flex items-center gap-1.5 px-2 py-1 border text-[10px] transition-colors',
                  active
                    ? 'border-sky-400/60 text-sky-300 bg-sky-500/10'
                    : 'border-border/40 text-muted-foreground hover:text-foreground hover:border-border/80',
                )}
                title={`Constrained ${opt.label.toLowerCase()}`}
                aria-pressed={active}
              >
                <Icon className="w-3 h-3" />
                <span>{opt.label}</span>
              </button>
            )
          })}
        </div>
      )}

      {/* Multi-loop shapes are an implementation detail of the kernel —
          they exist so the extrude pipeline can classify outer vs hole
          loops via point-in-polygon containment. Surfacing them as
          shape pills makes the sketch feel like a sequence of
          disjoint mini-canvases ("draw shape 1, now draw shape 2");
          Fusion / SolidWorks present a single continuous 2D design
          space and let the user keep adding geometry. We match that
          by hiding the pill row entirely — every closed polyline /
          completed rectangle / circle still rolls into its own
          backend shape silently, but the user never has to think
          about it. The next pointer click after a closure just
          starts the next primitive on the same canvas. */}

      {/* Per-tool dimension inputs — type exact lengths instead of
          (or in addition to) clicking. Visible once enough points
          exist for the dimensions to be meaningful. */}
      <DimensionInputs
        tool={sketch.tool}
        points={sketch.points}
        setSketchPoint={setSketchPoint}
      />

      {/* Finalise operation selector. Three ways to turn the 2D
          profile into 3D: pull it (Extrude), pull-and-subtract
          (Cut), or spin it around an axis (Revolve). Sweep/Loft are
          out of scope for the single-sketch session (need cross-
          sketch coordination — see slice J-5). */}
      <div className="flex items-center gap-1 pt-1 border-t border-border/30 flex-wrap">
        <span className="text-muted-foreground text-[10px] mr-1">Op</span>
        {FINISH_OP_OPTIONS.map((opt) => {
          const Icon = opt.icon
          return (
            <button
              key={opt.value}
              type="button"
              onClick={() => setFinishOp(opt.value)}
              className={cn(
                'flex items-center gap-1 px-2 py-0.5 border text-[10px] transition-colors',
                finishOp === opt.value
                  ? 'border-border text-foreground bg-foreground/10'
                  : 'border-border/40 text-muted-foreground hover:text-foreground hover:border-border/80',
              )}
              title={opt.title}
              aria-pressed={finishOp === opt.value}
            >
              <Icon className="w-3 h-3" />
              <span>{opt.label}</span>
            </button>
          )
        })}
        {finishOp === 'extrude_cut' && (
          <span className="ml-2 text-[10px] font-mono text-muted-foreground">
            target:{' '}
            <span className={cn(cutTargetId ? 'text-foreground' : 'text-rose-400')}>
              {cutTargetId ? cutTargetId.slice(0, 8) : '— select a body —'}
            </span>
          </span>
        )}
      </div>

      {finishOp === 'revolve' && (
        <div className="flex items-center gap-2 flex-wrap pt-1 border-t border-border/30">
          <NumberField
            label="Angle°"
            value={revolveAngleDeg}
            onChange={(n) => setRevolveAngleDeg(n)}
            step={15}
          />
          <NumberField
            label="Segs"
            value={revolveSegments}
            onChange={(n) => setRevolveSegments(Math.max(3, Math.round(n)))}
            min={3}
            step={1}
          />
          <AxisInputs
            label="origin"
            value={revolveAxisOrigin}
            onChange={setRevolveAxisOrigin}
          />
          <AxisInputs
            label="dir"
            value={revolveAxisDir}
            onChange={setRevolveAxisDir}
          />
        </div>
      )}

      {/* Action row */}
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={popSketchPoint}
          disabled={sketch.points.length === 0 || busy}
          className="flex items-center gap-1.5 px-2 py-1 border border-border/40 text-muted-foreground hover:text-foreground hover:border-border/80 disabled:opacity-40 disabled:hover:text-muted-foreground transition-colors text-[10px]"
          title="Undo last point (Backspace)"
        >
          <Undo2 className="w-3 h-3" />
          <span>Undo</span>
        </button>
        <button
          type="button"
          onClick={clearSketchPoints}
          disabled={sketch.points.length === 0 || busy}
          className="flex items-center gap-1.5 px-2 py-1 border border-border/40 text-muted-foreground hover:text-foreground hover:border-border/80 disabled:opacity-40 disabled:hover:text-muted-foreground transition-colors text-[10px]"
          title="Clear all points"
        >
          <Trash2 className="w-3 h-3" />
          <span>Clear</span>
        </button>
        <span className="text-muted-foreground text-[10px] ml-2">
          {sketch.points.length} pts
        </span>

        <button
          type="button"
          onClick={() => void handleFinish()}
          onPointerEnter={() => setExtrudeHover(true)}
          onPointerLeave={() => setExtrudeHover(false)}
          disabled={
            busy ||
            // At least one shape across the whole session must be
            // finishable — i.e. materialise to a valid polygon. The
            // active shape can legitimately be empty right after the
            // user closes a polyline (the auto-commit pattern creates
            // a fresh on-deck shape); in that case the committed loop
            // is still extrudable and the button should stay live.
            !hasFinishableShape ||
            (finishOp === 'extrude_cut' && cutTargetId === null)
          }
          className={cn(
            'ml-auto flex items-center gap-1.5 px-3 py-1 border text-[10px] font-semibold transition-colors',
            busy
              ? 'border-border/40 text-muted-foreground'
              : 'border-emerald-400/60 text-emerald-300 hover:bg-emerald-500/10 disabled:opacity-40',
          )}
          title={
            finishOp === 'extrude_cut'
              ? 'Finish sketch and cut from target (Enter)'
              : finishOp === 'revolve'
                ? 'Finish sketch and revolve around axis (Enter)'
                : 'Finish sketch and extrude (Enter)'
          }
        >
          <Check className="w-3 h-3" />
          <span>
            {busy
              ? finishOp === 'extrude_cut'
                ? 'Cutting…'
                : finishOp === 'revolve'
                  ? 'Revolving…'
                  : 'Extruding…'
              : finishOp === 'extrude_cut'
                ? 'Finish & Cut'
                : finishOp === 'revolve'
                  ? 'Finish & Revolve'
                  : 'Finish & Extrude'}
          </span>
        </button>
      </div>

      {/* Live measurements + error */}
      {(summary || error) && (
        <div className="flex items-center gap-4 pt-1 border-t border-border/30 text-[10px] text-muted-foreground font-mono">
          {summary && (
            <>
              <span>
                Perimeter <span className="text-foreground">{summary.perimeter.toFixed(2)}</span>
              </span>
              {summary.closed && (
                <span>
                  Area <span className="text-foreground">{summary.area.toFixed(2)}</span>
                </span>
              )}
            </>
          )}
          {error && (
            <span className="ml-auto text-rose-400 normal-case tracking-normal">
              {error}
            </span>
          )}
        </div>
      )}
    </div>
  )
}

// ─── Helpers ─────────────────────────────────────────────────────────

interface NumberFieldProps {
  label: string
  value: number
  onChange: (n: number) => void
  min?: number
  step?: number
}

function NumberField({ label, value, onChange, min, step }: NumberFieldProps) {
  return (
    <label className="flex items-center gap-1 text-[10px] text-muted-foreground">
      <span>{label}</span>
      <input
        type="number"
        value={Number.isFinite(value) ? value : ''}
        min={min}
        step={step}
        onChange={(e) => {
          const n = Number(e.target.value)
          if (Number.isFinite(n)) onChange(n)
        }}
        className="w-16 px-1.5 py-0.5 bg-background/40 border border-border/40 text-foreground text-[10px] font-mono focus:outline-none focus:border-border"
      />
    </label>
  )
}

interface AxisInputsProps {
  label: string
  value: [number, number, number]
  onChange: (v: [number, number, number]) => void
}

/**
 * Three tightly-packed number boxes labelled x/y/z. Used for revolve
 * axis origin + direction. Each box edits one component; the parent
 * holds the tuple. Invalid numerics are dropped (the box clears
 * to empty but the underlying state keeps its prior value).
 */
function AxisInputs({ label, value, onChange }: AxisInputsProps) {
  return (
    <span className="flex items-center gap-1 text-[10px] text-muted-foreground">
      <span>{label}</span>
      {(['x', 'y', 'z'] as const).map((axis, i) => (
        <input
          key={axis}
          type="number"
          value={Number.isFinite(value[i]) ? value[i] : ''}
          step={0.1}
          onChange={(e) => {
            const n = Number(e.target.value)
            if (!Number.isFinite(n)) return
            const next: [number, number, number] = [...value]
            next[i] = n
            onChange(next)
          }}
          className="w-12 px-1 py-0.5 bg-background/40 border border-border/40 text-foreground text-[10px] font-mono focus:outline-none focus:border-border"
          title={`${label} ${axis}`}
          aria-label={`${label} ${axis}`}
        />
      ))}
    </span>
  )
}

// ─── Per-tool dimension entry ────────────────────────────────────────

interface DimensionInputsProps {
  tool: SketchTool
  points: Array<[number, number]>
  setSketchPoint: (index: number, point: [number, number]) => void
}

/**
 * Lets the user type exact lengths for the current tool. The inputs
 * mutate the existing sketch points in place so the live preview +
 * dimension labels update immediately:
 *   - Rectangle: width / height drive points[1] relative to points[0]
 *   - Circle:    radius drives points[1] along the existing direction
 *                from points[0] (or +u when no direction yet)
 *   - Polyline:  per-segment length list; editing length L of segment
 *                {i, i+1} moves points[i+1] along the segment's unit
 *                direction so the magnitude is exactly L.
 *
 * Returns `null` until there is enough confirmed input for the entry
 * to be meaningful (avoids showing controls that have nothing to bind).
 */
function DimensionInputs({ tool, points, setSketchPoint }: DimensionInputsProps) {
  if (tool === 'rectangle') {
    if (points.length < 1) return null
    const [a] = points
    const b = points[1] ?? a
    const width = b[0] - a[0]
    const height = b[1] - a[1]
    const setWidth = (w: number) => {
      if (!Number.isFinite(w)) return
      // Preserve sign of the existing offset so the corner stays on
      // the same side of `a`; if the user hasn't moved off `a` yet,
      // assume positive.
      const sign = width === 0 ? 1 : Math.sign(width)
      const target: [number, number] = [a[0] + sign * Math.abs(w), b[1]]
      if (points.length < 2) {
        // Promote the hover-only second corner into a confirmed point.
        useSceneStore.getState().addSketchPoint(target)
      } else {
        setSketchPoint(1, target)
      }
    }
    const setHeight = (h: number) => {
      if (!Number.isFinite(h)) return
      const sign = height === 0 ? 1 : Math.sign(height)
      const target: [number, number] = [b[0], a[1] + sign * Math.abs(h)]
      if (points.length < 2) {
        useSceneStore.getState().addSketchPoint(target)
      } else {
        setSketchPoint(1, target)
      }
    }
    return (
      <div className="flex items-center gap-3 pt-1 border-t border-border/30">
        <span className="text-muted-foreground text-[10px]">Dimensions</span>
        <NumberField label="W" value={Math.abs(width)} onChange={setWidth} min={0} step={1} />
        <NumberField label="H" value={Math.abs(height)} onChange={setHeight} min={0} step={1} />
      </div>
    )
  }

  if (tool === 'circle') {
    if (points.length < 1) return null
    const [c] = points
    const e = points[1] ?? [c[0] + 1, c[1]]
    const dx = e[0] - c[0]
    const dy = e[1] - c[1]
    const r = Math.hypot(dx, dy)
    const setRadius = (newR: number) => {
      if (!Number.isFinite(newR) || newR <= 0) return
      const ux = r > 1e-9 ? dx / r : 1
      const uy = r > 1e-9 ? dy / r : 0
      const target: [number, number] = [c[0] + ux * newR, c[1] + uy * newR]
      if (points.length < 2) {
        useSceneStore.getState().addSketchPoint(target)
      } else {
        setSketchPoint(1, target)
      }
    }
    return (
      <div className="flex items-center gap-3 pt-1 border-t border-border/30">
        <span className="text-muted-foreground text-[10px]">Dimensions</span>
        <NumberField label="R" value={r} onChange={setRadius} min={0.001} step={0.5} />
        <span className="text-muted-foreground/60 text-[10px] font-mono">
          Ø {(r * 2).toFixed(2)}
        </span>
      </div>
    )
  }

  // Polyline — list each segment as an editable length.
  if (points.length < 2) return null
  const segments = points.slice(0, -1).map((p, i) => {
    const q = points[i + 1]
    return { i, length: Math.hypot(q[0] - p[0], q[1] - p[1]) }
  })
  const setSegmentLength = (i: number, newLen: number) => {
    if (!Number.isFinite(newLen) || newLen <= 0) return
    const p = points[i]
    const q = points[i + 1]
    const dx = q[0] - p[0]
    const dy = q[1] - p[1]
    const cur = Math.hypot(dx, dy)
    if (cur < 1e-9) return
    const ux = dx / cur
    const uy = dy / cur
    setSketchPoint(i + 1, [p[0] + ux * newLen, p[1] + uy * newLen])
  }
  return (
    <div className="flex items-start gap-3 pt-1 border-t border-border/30">
      <span className="text-muted-foreground text-[10px] mt-1">Segments</span>
      <div className="flex flex-wrap items-center gap-2 flex-1">
        {segments.map((s) => (
          <NumberField
            key={s.i}
            label={`${s.i + 1}→${s.i + 2}`}
            value={s.length}
            onChange={(n) => setSegmentLength(s.i, n)}
            min={0.001}
            step={0.5}
          />
        ))}
      </div>
    </div>
  )
}

