/**
 * The sketch workbench's bar over the viewport: which plane, what the tool
 * wants next, how many freedoms are left, and Finish -- Extrude / Cut /
 * Revolve with its size -- or Cancel. Refusals show in the server's words.
 */

import { useMemo, useState } from 'react'
import { Check, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { dimensionFor, freedomLabel, overlaps, type BenchTool } from '@/lib/sketch-bench'
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
    case 'trim':
      return 'Click the piece to cut away (back to the nearest crossings)'
    case 'extend':
      return 'Click near a line end to run it to the next curve'
    case 'fillet':
      return 'Click a corner to round it'
    case 'chamfer':
      return 'Click a corner to cut it'
    case 'mirror':
      return 'Select what to mirror, then click the mirror line'
    case 'circular-pattern':
      return 'Select what to pattern, then click the centre'
    case 'dimension':
      return 'Click one or two items, type the size, Enter'
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
      <ContextRow />
      {error && <p className="text-[11px] text-red-700 dark:text-red-300">{error}</p>}
    </div>
  )
}

/** A number field in the bar: commits on Enter or when it loses focus. */
function Num({
  label,
  value,
  unit,
  onChange,
  step = 1,
  autoFocus = false,
  onEnter,
}: {
  label: string
  value: number
  unit?: string
  onChange: (v: number) => void
  step?: number
  autoFocus?: boolean
  onEnter?: () => void
}) {
  return (
    <label className="flex items-center gap-1 text-muted-foreground">
      {label}
      <input
        type="number"
        value={Number.isFinite(value) ? Number(value.toFixed(4)) : ''}
        step={step}
        autoFocus={autoFocus}
        onChange={(e) => onChange(Number(e.target.value))}
        onKeyDown={(e) => {
          if (e.key === 'Enter') onEnter?.()
        }}
        className="cad-focus w-16 rounded border border-border bg-background px-1 text-right text-foreground"
      />
      {unit}
    </label>
  )
}

/** The bar's third row: the sizes the active tool or action needs, the
 *  dimension being placed, and warnings about the sketch itself. */
function ContextRow() {
  const tool = useSketchBench((s) => s.tool)
  const action = useSketchBench((s) => s.action)
  const opts = useSketchBench((s) => s.opts)
  const setOpts = useSketchBench((s) => s.setOpts)
  const setAction = useSketchBench((s) => s.setAction)
  const selection = useSketchBench((s) => s.selection)
  const summary = useSketchBench((s) => s.summary)
  const busy = useSketchBench((s) => s.busy)
  const offer = useMemo(() => (summary ? dimensionFor(selection, summary) : null), [selection, summary])
  // A typed size belongs to the selection it was typed for: a new
  // selection proposes its own current size.
  const selKey = JSON.stringify(selection)
  const [dimDraft, setDimDraft] = useState<{ key: string; v: number } | null>(null)
  const dimValue = dimDraft && dimDraft.key === selKey ? dimDraft.v : null
  const setDimValue = (v: number) => setDimDraft({ key: selKey, v })
  const ov = useMemo(() => (summary ? overlaps(summary).length : 0), [summary])
  const b = useSketchBench.getState

  const editing = useSketchBench((s) => s.editing)
  const [editDraft, setEditDraft] = useState<{ id: string; v: number } | null>(null)
  const editValue = editing && editDraft && editDraft.id === editing.id ? editDraft.v : null
  const setEditValue = (v: number) => editing && setEditDraft({ id: editing.id, v })
  const parts: React.ReactNode[] = []
  if (editing) {
    const v = editValue ?? editing.value
    const commit = () => {
      if (!Number.isFinite(v) || v <= 0) return
      void b().editDimension(editing.id, editing.unit === 'deg' ? (v * Math.PI) / 180 : v)
      b().setEditing(null)
    }
    parts.push(
      <span key="edit" className="flex items-center gap-1">
        <span className="font-medium text-foreground">Change {editing.kind}</span>
        <Num label="" unit={editing.unit === 'deg' ? '°' : 'mm'} value={v} onChange={setEditValue} autoFocus onEnter={commit} />
        <button type="button" onClick={commit} disabled={busy} className="cad-focus rounded px-1.5 py-0.5 hover:bg-accent" title="Apply (Enter)">
          <Check size={12} />
        </button>
        <button type="button" onClick={() => b().setEditing(null)} className="cad-focus rounded px-1.5 py-0.5 hover:bg-accent" title="Cancel (Esc)">
          <X size={12} />
        </button>
      </span>,
    )
  }
  if (tool === 'fillet') parts.push(<Num key="f" label="Radius" unit="mm" value={opts.fillet} onChange={(v) => setOpts({ fillet: v })} />)
  if (tool === 'chamfer') parts.push(<Num key="c" label="Distance" unit="mm" value={opts.chamfer} onChange={(v) => setOpts({ chamfer: v })} />)
  if (tool === 'circular-pattern') {
    parts.push(<Num key="n" label="Count" value={opts.circCount} onChange={(v) => setOpts({ circCount: v })} />)
    parts.push(<Num key="a" label="Over" unit="°" value={opts.circAngle} onChange={(v) => setOpts({ circAngle: v })} />)
  }
  if (action === 'offset') {
    parts.push(
      <Num key="o" label="Offset" unit="mm (− shrinks)" value={opts.offset} onChange={(v) => setOpts({ offset: v })} autoFocus onEnter={() => void b().offsetSelection().then(() => setAction(null))} />,
    )
  }
  if (action === 'linear-pattern') {
    parts.push(<Num key="n" label="Count" value={opts.linCount} onChange={(v) => setOpts({ linCount: v })} autoFocus />)
    parts.push(<Num key="x" label="Step X" unit="mm" value={opts.linDx} onChange={(v) => setOpts({ linDx: v })} />)
    parts.push(<Num key="y" label="Y" unit="mm" value={opts.linDy} onChange={(v) => setOpts({ linDy: v })} />)
  }
  if (action) {
    parts.push(
      <button
        key="apply"
        type="button"
        disabled={busy || selection.length === 0}
        onClick={() => void (action === 'offset' ? b().offsetSelection() : b().linearPatternSelection()).then(() => setAction(null))}
        className="cad-focus rounded bg-[var(--cad-selected)] px-2 py-0.5 font-medium text-black disabled:opacity-40"
      >
        Apply
      </button>,
    )
  }
  if (offer && !editing && (tool === 'dimension' || tool === 'select') && !action) {
    const v = dimValue ?? offer.value
    const commit = () => {
      if (Number.isFinite(v) && v > 0) void b().addDimension(offer, v)
    }
    parts.push(
      <span key="dim" className="flex items-center gap-1">
        <span className="font-medium text-foreground">{offer.kind}</span>
        <Num label="" unit={offer.unit === 'deg' ? '°' : 'mm'} value={v} onChange={setDimValue} autoFocus={tool === 'dimension'} onEnter={commit} />
        <button type="button" onClick={commit} disabled={busy} className="cad-focus rounded px-1.5 py-0.5 hover:bg-accent" title="Add this dimension (Enter)">
          <Check size={12} />
        </button>
      </span>,
    )
  }
  if (ov > 0) {
    parts.push(
      <span key="ov" className="text-red-700 dark:text-red-300" title="A profile cannot close through curves that lie on each other">
        {ov === 1 ? '2 curves overlap' : `${ov} overlaps`} -- trim or delete one
      </span>,
    )
  }
  if (parts.length === 0) return null
  return <div className="flex flex-wrap items-center gap-2 border-t border-border/60 pt-1">{parts}</div>
}
