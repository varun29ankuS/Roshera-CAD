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
} from 'lucide-react'
import {
  useSceneStore,
  type SketchPlane,
  type SketchTool,
} from '@/stores/scene-store'
import { useChatStore } from '@/stores/chat-store'
import {
  buildProfile2D,
  extrudeSketch,
  perimeter,
  signedArea,
} from '@/lib/sketch-extrude'
import { cn } from '@/lib/utils'

const PLANE_OPTIONS: Array<{ value: SketchPlane; label: string }> = [
  { value: 'xy', label: 'XY' },
  { value: 'xz', label: 'XZ' },
  { value: 'yz', label: 'YZ' },
]

const TOOL_OPTIONS: Array<{
  value: SketchTool
  label: string
  icon: typeof PenTool
}> = [
  { value: 'polyline', label: 'Polyline', icon: PenTool },
  { value: 'rectangle', label: 'Rectangle', icon: RectangleHorizontal },
  { value: 'circle', label: 'Circle', icon: CircleIcon },
]

export function SketchPanel() {
  const sketch = useSceneStore((s) => s.sketch)
  const setSketchTool = useSceneStore((s) => s.setSketchTool)
  const setSketchPlane = useSceneStore((s) => s.setSketchPlane)
  const popSketchPoint = useSceneStore((s) => s.popSketchPoint)
  const clearSketchPoints = useSceneStore((s) => s.clearSketchPoints)
  const exitSketch = useSceneStore((s) => s.exitSketch)
  const setSketchView = useSceneStore((s) => s.setSketchView)
  const setSketchPoint = useSceneStore((s) => s.setSketchPoint)

  const [busy, setBusy] = useState<boolean>(false)
  const [error, setError] = useState<string | null>(null)

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
    const profile = buildProfile2D(
      sketch.tool,
      sketch.points,
      sketch.circleSegments,
    )
    if (!profile) {
      setError('Need more points')
      return
    }
    setBusy(true)
    try {
      const result = await extrudeSketch({
        plane: sketch.plane,
        tool: sketch.tool,
        points: sketch.points,
        thickness: sketch.thickness,
        circleSegments: sketch.circleSegments,
        name: `${sketch.tool}-extrude`,
      })
      const { addMessage } = useChatStore.getState()
      const stats =
        result.vertexCount && result.triangleCount
          ? ` (${result.vertexCount} verts, ${result.triangleCount} tris${
              result.tessellationMs ? `, ${result.tessellationMs} ms` : ''
            })`
          : ''
      addMessage({
        role: 'assistant',
        content: `Extruded ${sketch.tool} on ${sketch.plane.toUpperCase()} plane (t=${sketch.thickness})${stats}.`,
        objectsAffected: [result.objectId],
      })
      exitSketch()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setError(msg)
    } finally {
      setBusy(false)
    }
  }, [
    busy,
    exitSketch,
    sketch.circleSegments,
    sketch.plane,
    sketch.points,
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

  if (!sketch.active) return null

  const hint =
    sketch.tool === 'polyline'
      ? sketch.points.length < 3
        ? `Click ${3 - sketch.points.length} more point${sketch.points.length === 2 ? '' : 's'} · Enter to finish`
        : 'Enter to finish · Backspace to undo'
      : sketch.tool === 'rectangle'
        ? sketch.points.length < 1
          ? 'Click first corner'
          : sketch.points.length < 2
            ? 'Click opposite corner'
            : 'Enter to extrude'
        : sketch.points.length < 1
          ? 'Click circle center'
          : sketch.points.length < 2
            ? 'Click radius point'
            : 'Enter to extrude'

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
          {PLANE_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              type="button"
              onClick={() => setSketchPlane(opt.value)}
              className={cn(
                'px-2 py-0.5 border text-[10px] font-mono transition-colors',
                sketch.plane === opt.value
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
            onClick={exitSketch}
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

      {/* Per-tool dimension inputs — type exact lengths instead of
          (or in addition to) clicking. Visible once enough points
          exist for the dimensions to be meaningful. */}
      <DimensionInputs
        tool={sketch.tool}
        points={sketch.points}
        setSketchPoint={setSketchPoint}
      />

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
          disabled={busy || sketch.points.length < 2}
          className={cn(
            'ml-auto flex items-center gap-1.5 px-3 py-1 border text-[10px] font-semibold transition-colors',
            busy
              ? 'border-border/40 text-muted-foreground'
              : 'border-emerald-400/60 text-emerald-300 hover:bg-emerald-500/10 disabled:opacity-40',
          )}
          title="Finish sketch and extrude (Enter)"
        >
          <Check className="w-3 h-3" />
          <span>{busy ? 'Extruding…' : 'Finish & Extrude'}</span>
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
