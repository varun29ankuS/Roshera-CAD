/**
 * The sketch workbench's tools (Varun 2026-11-14: "there are no buttons
 * that show up for sketching, for editing ..."). While a sketch is open
 * this rail REPLACES the part tools, as in every established CAD sketcher:
 * DRAW tools, EDIT tools, and Exit. Every button is wired; constraints,
 * smart dimension and trim / mirror / offset arrive as their slices land
 * (no button here that does nothing).
 */

import { useEffect, type ComponentType } from 'react'
import { cn } from '@/lib/utils'
import type { BenchTool } from '@/lib/sketch-bench'
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

/** Keyboard: one letter per draw tool; Esc / Enter / Delete / X / Ctrl+Z. */
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
          b.escape()
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
          if (k === 'X') {
            void b.toggleConstruction()
            break
          }
          if (k === 'V') {
            b.setTool('select')
            break
          }
          const item = DRAW.find((d) => d.key === k)
          if (!item?.tool) return
          b.setTool(item.tool)
        }
      }
      e.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
}

function RailButton({ item, active }: { item: RailItem; active: boolean }) {
  const set = useSketchBench((s) => s.setTool)
  return (
    <button
      type="button"
      disabled={item.disabled}
      onClick={() => (item.tool ? set(item.tool) : item.action?.())}
      title={item.key ? `${item.label} · ${item.key}` : item.label}
      aria-label={item.label}
      aria-pressed={active || undefined}
      className={cn(
        'cad-focus flex w-[54px] flex-col items-center gap-0.5 rounded-md px-0.5 py-1 disabled:opacity-35',
        active ? 'bg-[var(--cad-selected-wash)] text-foreground' : 'text-foreground/85 hover:bg-accent',
      )}
    >
      <item.icon size={20} />
      <span className="w-full truncate text-center text-[10px] leading-tight">{item.label}</span>
    </button>
  )
}

function Cap({ label }: { label: string }) {
  return <span className="mt-1.5 text-[10px] font-medium tracking-[0.08em] text-muted-foreground">{label}</span>
}

export function SketchBenchRail() {
  useBenchKeys()
  const tool = useSketchBench((s) => s.tool)
  const selection = useSketchBench((s) => s.selection)
  const history = useSketchBench((s) => s.history)
  const sides = useSketchBench((s) => s.sides)
  const setSides = useSketchBench((s) => s.setSides)
  const b = useSketchBench.getState
  const edit: RailItem[] = [
    { id: 'select', label: 'Select', key: 'V', icon: I.SelectIcon, tool: 'select' },
    {
      id: 'delete',
      label: 'Delete',
      key: 'Del',
      icon: I.DeleteIcon,
      action: () => void b().deleteSelection(),
      disabled: selection.length === 0,
    },
    {
      id: 'construction',
      label: 'Construction',
      key: 'X',
      icon: I.ConstructionIcon,
      action: () => void b().toggleConstruction(),
      disabled: selection.length === 0,
    },
    { id: 'undo', label: 'Undo', key: 'Ctrl+Z', icon: I.UndoIcon, action: () => void b().undo(), disabled: history.length === 0 },
  ]
  return (
    <div className="flex w-[60px] flex-col items-center border-r border-border bg-background">
      <div className="flex min-h-0 w-full flex-1 flex-col items-center gap-px overflow-y-auto py-1 [scrollbar-width:none]">
        <Cap label="DRAW" />
        {DRAW.map((d) => (
          <RailButton key={d.id} item={d} active={tool === d.tool} />
        ))}
        {tool === 'polygon' && (
          <label className="flex w-[54px] flex-col items-center text-[10px] text-muted-foreground">
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
        <Cap label="EDIT" />
        {edit.map((d) => (
          <RailButton key={d.id} item={d} active={d.tool !== undefined && tool === d.tool} />
        ))}
      </div>
    </div>
  )
}
