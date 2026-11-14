/**
 * The sketch workbench's tools (Varun 2026-11-14: "there are no buttons
 * that show up for sketching, for editing, for trimming, for mirror ...
 * no way to constraint two lines"). While a sketch is open this rail
 * REPLACES the part tools, as in every established CAD sketcher:
 * DRAW, CONSTRAIN (each lights up only when the selection fits it),
 * DIMENSION, MODIFY and EDIT. Every button is wired.
 */

import { useEffect, type ComponentType } from 'react'
import { cn } from '@/lib/utils'
import { constraintsFor, type BenchTool } from '@/lib/sketch-bench'
import type { GeometricConstraint } from '@/lib/csketch-api'
import { useSketchBench } from '@/stores/sketch-bench-store'
import * as I from './sketch-icons'

interface RailItem {
  id: string
  label: string
  key?: string
  icon: ComponentType<{ size?: number }>
  tool?: BenchTool
  action?: () => void
  disabled?: boolean
  active?: boolean
  hint?: string
}

const DRAW: RailItem[] = [
  { id: 'line', label: 'Line', key: 'L', icon: I.LineIcon, tool: 'line' },
  { id: 'rectangle', label: 'Rectangle', key: 'R', icon: I.RectIcon, tool: 'rectangle' },
  { id: 'center-rectangle', label: 'Centre rect', icon: I.CenterRectIcon, tool: 'center-rectangle' },
  { id: 'circle', label: 'Circle', key: 'C', icon: I.CircleIcon, tool: 'circle' },
  { id: 'arc3', label: '3-pt arc', key: 'A', icon: I.Arc3Icon, tool: 'arc3' },
  { id: 'arc-center', label: 'Centre arc', icon: I.CenterArcIcon, tool: 'arc-center' },
  { id: 'polygon', label: 'Polygon', key: 'P', icon: I.PolygonIcon, tool: 'polygon' },
  { id: 'slot', label: 'Slot', icon: I.SlotIcon, tool: 'slot' },
  { id: 'spline', label: 'Spline', key: 'S', icon: I.SplineIcon, tool: 'spline' },
  { id: 'point', label: 'Point', icon: I.PointIcon, tool: 'point' },
  { id: 'centerline', label: 'Centre line', icon: I.CenterlineIcon, tool: 'centerline' },
]

const CONSTRAINTS: { kind: GeometricConstraint; label: string; icon: ComponentType<{ size?: number }>; needs: string }[] = [
  { kind: 'Horizontal', label: 'Horizontal', icon: I.HorizontalIcon, needs: 'a line' },
  { kind: 'Vertical', label: 'Vertical', icon: I.VerticalIcon, needs: 'a line' },
  { kind: 'Coincident', label: 'Coincident', icon: I.CoincidentIcon, needs: 'two points' },
  { kind: 'PointOnCurve', label: 'On curve', icon: I.OnCurveIcon, needs: 'a point and a curve' },
  { kind: 'Midpoint', label: 'Midpoint', icon: I.MidpointIcon, needs: 'a point and a line' },
  { kind: 'Parallel', label: 'Parallel', icon: I.ParallelIcon, needs: 'two lines' },
  { kind: 'Perpendicular', label: 'Perpend.', icon: I.PerpendicularIcon, needs: 'two lines' },
  { kind: 'Equal', label: 'Equal', icon: I.EqualIcon, needs: 'two lines, or two circles / arcs' },
  { kind: 'Tangent', label: 'Tangent', icon: I.TangentIcon, needs: 'a line and a circle / arc, or two circles / arcs' },
  { kind: 'Concentric', label: 'Concentric', icon: I.ConcentricIcon, needs: 'two circles / arcs' },
  { kind: 'Symmetric', label: 'Symmetric', icon: I.SymmetricIcon, needs: 'two points and a line' },
]

/** Keyboard: one letter per draw tool; D dimension, T trim; Esc / Enter / Delete / X / Ctrl+Z. */
function useBenchKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
      const b = useSketchBench.getState()
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') {
        e.preventDefault()
        void b.undo()
        return
      }
      if (e.ctrlKey || e.metaKey || e.altKey) return
      switch (e.key) {
        case 'Escape':
          if (b.editing) b.setEditing(null)
          else if (b.action) b.setAction(null)
          else b.escape()
          break
        case 'Enter':
          b.endChain()
          break
        case 'Delete':
        case 'Backspace':
          void b.deleteSelection()
          break
        default: {
          const k = e.key.toUpperCase()
          const map: Record<string, BenchTool> = { V: 'select', D: 'dimension', T: 'trim', F: 'fillet', M: 'mirror' }
          if (k === 'X') {
            void b.toggleConstruction()
            break
          }
          const tool = map[k] ?? DRAW.find((d) => d.key === k)?.tool
          if (!tool) return
          b.setTool(tool)
        }
      }
      e.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
}

function RailButton({ item }: { item: RailItem }) {
  const set = useSketchBench((s) => s.setTool)
  const tip = [item.label, item.key && `· ${item.key}`, item.hint && `-- ${item.hint}`].filter(Boolean).join(' ')
  return (
    <button
      type="button"
      disabled={item.disabled}
      onClick={() => (item.tool ? set(item.tool) : item.action?.())}
      title={tip}
      aria-label={item.label}
      aria-pressed={item.active || undefined}
      className={cn(
        'cad-focus flex w-[48px] flex-col items-center gap-0.5 rounded-md px-0.5 py-1 disabled:opacity-30',
        item.active ? 'bg-[var(--cad-selected-wash)] text-foreground' : 'text-foreground/85 hover:bg-accent',
      )}
    >
      <item.icon size={18} />
      <span className="w-full truncate text-center text-[9.5px] leading-tight">{item.label}</span>
    </button>
  )
}

function Group({ label, items }: { label: string; items: RailItem[] }) {
  return (
    <>
      <span className="col-span-2 mt-1.5 text-center text-[10px] font-medium tracking-[0.08em] text-muted-foreground">{label}</span>
      {items.map((d) => (
        <RailButton key={d.id} item={d} />
      ))}
    </>
  )
}

export function SketchBenchRail() {
  useBenchKeys()
  const tool = useSketchBench((s) => s.tool)
  const action = useSketchBench((s) => s.action)
  const selection = useSketchBench((s) => s.selection)
  const selectedConstraint = useSketchBench((s) => s.selectedConstraint)
  const history = useSketchBench((s) => s.history)
  const sides = useSketchBench((s) => s.sides)
  const setSides = useSketchBench((s) => s.setSides)
  const b = useSketchBench.getState
  const offers = constraintsFor(selection)
  const has = selection.length > 0
  const withTool = (items: RailItem[]) => items.map((i) => ({ ...i, active: i.active || (i.tool !== undefined && tool === i.tool) }))

  const constrain: RailItem[] = CONSTRAINTS.map((c) => {
    const offer = offers.find((o) => o.kind === c.kind)
    return {
      id: c.label,
      label: c.label,
      icon: c.icon,
      disabled: !offer,
      hint: offer ? 'apply to the selection' : `select ${c.needs}`,
      action: () => {
        if (offer) void b().applyConstraint(offer)
      },
    }
  })
  const onePoint = selection.length === 1 && 'Point' in selection[0] ? selection[0].Point : null
  constrain.push({
    id: 'fix',
    label: 'Fix',
    icon: I.FixIcon,
    disabled: !onePoint,
    hint: onePoint ? 'pin this point where it is' : 'select a point',
    action: () => {
      if (onePoint) void b().fixPoint(onePoint)
    },
  })
  const modify: RailItem[] = [
    { id: 'trim', label: 'Trim', key: 'T', icon: I.TrimIcon, tool: 'trim' },
    { id: 'extend', label: 'Extend', icon: I.ExtendIcon, tool: 'extend' },
    { id: 'fillet', label: 'Fillet', key: 'F', icon: I.SketchFilletIcon, tool: 'fillet' },
    { id: 'chamfer', label: 'Chamfer', icon: I.SketchChamferIcon, tool: 'chamfer' },
    {
      id: 'offset',
      label: 'Offset',
      icon: I.OffsetIcon,
      disabled: !has,
      hint: has ? 'set the distance in the bar' : 'select a closed loop first',
      active: action === 'offset',
      action: () => b().setAction('offset'),
    },
    { id: 'mirror', label: 'Mirror', key: 'M', icon: I.MirrorIcon, tool: 'mirror', hint: 'select, then click the mirror line' },
    {
      id: 'linear-pattern',
      label: 'Lin. pattern',
      icon: I.LinearPatternIcon,
      disabled: !has,
      hint: has ? 'set count and spacing in the bar' : 'select what to pattern first',
      active: action === 'linear-pattern',
      action: () => b().setAction('linear-pattern'),
    },
    { id: 'circular-pattern', label: 'Circ. pattern', icon: I.CircularPatternIcon, tool: 'circular-pattern', hint: 'select, then click the centre' },
  ]
  const edit: RailItem[] = [
    { id: 'select', label: 'Select', key: 'V', icon: I.SelectIcon, tool: 'select' },
    {
      id: 'delete',
      label: 'Delete',
      key: 'Del',
      icon: I.DeleteIcon,
      action: () => void b().deleteSelection(),
      disabled: !has && !selectedConstraint,
    },
    {
      id: 'construction',
      label: 'Construct.',
      key: 'X',
      icon: I.ConstructionIcon,
      action: () => void b().toggleConstruction(),
      disabled: !has,
    },
    { id: 'undo', label: 'Undo', key: 'Ctrl+Z', icon: I.UndoIcon, action: () => void b().undo(), disabled: history.length === 0 },
  ]

  return (
    <div className="flex w-[104px] flex-col items-center border-r border-border bg-background">
      <div className="grid min-h-0 w-full flex-1 grid-cols-2 content-start justify-items-center gap-px overflow-y-auto py-1 [scrollbar-width:none]">
        <Group label="DRAW" items={withTool(DRAW)} />
        {tool === 'polygon' && (
          <label className="col-span-2 flex items-center justify-center gap-1 text-[10px] text-muted-foreground">
            Sides
            <input
              type="number"
              min={3}
              max={64}
              value={sides}
              onChange={(e) => setSides(Number(e.target.value))}
              className="cad-focus w-10 rounded border border-border bg-background px-1 text-center text-[11px] text-foreground"
            />
          </label>
        )}
        <Group label="CONSTRAIN" items={constrain} />
        <Group
          label="DIMENSION"
          items={withTool([{ id: 'dimension', label: 'Smart dim.', key: 'D', icon: I.DimensionIcon, tool: 'dimension' }])}
        />
        <Group label="MODIFY" items={withTool(modify)} />
        <Group label="EDIT" items={withTool(edit)} />
      </div>
    </div>
  )
}
