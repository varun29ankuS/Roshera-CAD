/**
 * The sketch workbench's bar over the viewport: which plane, what the tool
 * wants next, how many freedoms are left, and Finish -- Extrude / Cut /
 * Revolve with its size -- or Cancel. Refusals show in the server's words.
 */

import { Check, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { freedomLabel, type BenchTool } from '@/lib/sketch-bench'
import { useSketchBench, type FinishOp } from '@/stores/sketch-bench-store'

/** What a tool wants next, by how many clicks it has. */
function hint(tool: BenchTool, n: number): string {
  switch (tool) {
    case 'select':
      return 'Click to select · Shift adds · Delete removes'
    case 'line':
      return n === 0 ? 'Click the start point' : 'Click the next point · Enter or Esc stops'
    case 'centerline':
      return n === 0 ? 'Click the start of the centre line' : 'Click its end'
    case 'rectangle':
      return n === 0 ? 'Click a corner' : 'Click the opposite corner'
    case 'center-rectangle':
      return n === 0 ? 'Click the centre' : 'Click a corner'
    case 'circle':
      return n === 0 ? 'Click the centre' : 'Click a point on the circle'
    case 'arc3':
      return ['Click the start', 'Click the end', 'Click a point on the arc'][n] ?? ''
    case 'arc-center':
      return ['Click the centre', 'Click the start (sets the radius)', 'Click the end'][n] ?? ''
    case 'polygon':
      return n === 0 ? 'Click the centre' : 'Click a corner'
    case 'slot':
      return ['Click one end centre', 'Click the other end centre', 'Click to set the width'][n] ?? ''
    case 'spline':
      return n < 2 ? 'Click control points' : 'Click more points · Enter or double-click to place'
    case 'point':
      return 'Click to place a point'
  }
}

function planeName(p: ReturnType<typeof useSketchBench.getState>['plane']): string {
  return typeof p === 'string' ? p.toUpperCase() : 'face'
}

const OPS: { op: FinishOp; label: string }[] = [
  { op: 'extrude', label: 'Extrude' },
  { op: 'cut', label: 'Cut' },
  { op: 'revolve', label: 'Revolve' },
]

export function SketchBenchBar() {
  const sketchId = useSketchBench((s) => s.sketchId)
  const plane = useSketchBench((s) => s.plane)
  const tool = useSketchBench((s) => s.tool)
  const clicks = useSketchBench((s) => s.clicks.length)
  const dof = useSketchBench((s) => s.dof)
  const busy = useSketchBench((s) => s.busy)
  const error = useSketchBench((s) => s.error)
  const finish = useSketchBench((s) => s.finish)
  const onPart = useSketchBench((s) => s.onPart)
  const setFinish = useSketchBench((s) => s.setFinish)
  const empty = useSketchBench((s) => {
    const m = s.summary
    return !m || m.lines.length + m.circles.length + (m.arcs?.length ?? 0) + (m.splines?.length ?? 0) === 0
  })
  if (!sketchId) return null
  const free = freedomLabel(dof, empty)
  return (
    <div className="pointer-events-auto flex flex-col gap-1 rounded-lg border border-border bg-card px-3 py-2 text-xs shadow-md">
      <div className="flex items-center gap-3">
        <span className="font-semibold tracking-wide">SKETCH</span>
        <span className="text-muted-foreground">on {planeName(plane)}</span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">{hint(tool, clicks)}</span>
        {free && (
          <span
            title={
              free.tone === 'done'
                ? 'Fully defined: nothing can move'
                : free.tone === 'conflict'
                  ? 'Constraints fight each other: remove one'
                  : 'Freedoms left: add dimensions or constraints to fix them'
            }
            className={cn(
              'rounded-full px-2 py-0.5 tabular-nums',
              free.tone === 'done' && 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
              free.tone === 'free' && 'bg-sky-500/15 text-sky-700 dark:text-sky-300',
              free.tone === 'conflict' && 'bg-red-500/15 text-red-700 dark:text-red-300',
            )}
          >
            {free.text}
          </span>
        )}
      </div>
      <div className="flex items-center gap-1.5">
        {OPS.map(({ op, label }) => (
          <button
            key={op}
            type="button"
            onClick={() => setFinish({ op })}
            disabled={op === 'cut' && !onPart}
            title={op === 'cut' && !onPart ? 'Cut needs a sketch started on a face of the part' : undefined}
            aria-pressed={finish.op === op}
            className={cn(
              'cad-focus rounded px-2 py-0.5 disabled:opacity-35',
              finish.op === op ? 'bg-[var(--cad-selected-wash)] text-foreground' : 'text-muted-foreground hover:bg-accent',
            )}
          >
            {label}
          </button>
        ))}
        {finish.op === 'revolve' ? (
          <label className="ml-1 flex items-center gap-1 text-muted-foreground">
            <input
              type="number"
              value={finish.angle}
              min={1}
              max={360}
              onChange={(e) => setFinish({ angle: Number(e.target.value) })}
              className="cad-focus w-14 rounded border border-border bg-background px-1 text-right text-foreground"
            />
            ° about the selected line
          </label>
        ) : (
          <label className="ml-1 flex items-center gap-1 text-muted-foreground">
            <input
              type="number"
              value={finish.distance}
              min={0.01}
              step={1}
              onChange={(e) => setFinish({ distance: Number(e.target.value) })}
              className="cad-focus w-16 rounded border border-border bg-background px-1 text-right text-foreground"
            />
            mm
          </label>
        )}
        <span className="flex-1" />
        <button
          type="button"
          onClick={() => useSketchBench.getState().close()}
          className="cad-focus flex items-center gap-1 rounded px-2 py-0.5 text-muted-foreground hover:bg-accent"
        >
          <X size={12} /> Cancel
        </button>
        <button
          type="button"
          disabled={busy || empty}
          onClick={() => void useSketchBench.getState().complete()}
          className="cad-focus flex items-center gap-1 rounded bg-[var(--cad-selected)] px-2.5 py-0.5 font-medium text-black disabled:opacity-40"
        >
          <Check size={12} /> {OPS.find((o) => o.op === finish.op)?.label}
        </button>
      </div>
      {error && <p className="text-[11px] text-red-700 dark:text-red-300">{error}</p>}
    </div>
  )
}
