import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  MousePointer2,
  Move3d,
  RotateCw,
  Maximize,
  Box,
  Circle,
  Cylinder,
  Triangle,
  Minus,
  Hexagon,
  Disc,
  ArrowUpFromLine,
  RefreshCcw,
  Layers,
  Spline,
  Scissors,
  Combine,
  Diff,
  SquaresIntersect,
  PenTool,
  Ruler,
  Grid3x3,
  Copy,
  FlipHorizontal,
  Wrench,
  Pipette,
  Eye,
  FileDown,
  CircleDot,
  Torus,
  Component,
  Orbit,
  ScanLine,
  Grip,
  Hash,
  Waypoints,
  Workflow,
  RectangleHorizontal,
  SquareDashedBottom,
  type LucideIcon,
} from 'lucide-react'
import { useSceneStore, type TransformTool } from '@/stores/scene-store'
import { processUserMessage } from '@/lib/ai-client'
import { cn } from '@/lib/utils'

// ─── Types ──────────────────────────────────────────────────────────

interface ToolItem {
  icon: LucideIcon
  label: string
  shortcut?: string
  action: () => void
  active?: boolean
}

interface ToolSection {
  label: string
  items: ToolItem[]
}

interface ToolGroup {
  id: string
  icon: LucideIcon
  tooltip: string
  sections: ToolSection[]
}

// ─── AI command helper ──────────────────────────────────────────────

function sendCommand(cmd: string) {
  processUserMessage(cmd)
}

// ─── Flyout group — pure CSS hover, no timers ────────────────────

function FlyoutGroup({ group, openId, onToggle }: {
  group: ToolGroup
  openId: string | null
  onToggle: (id: string) => void
}) {
  const anyActive = group.sections.some((s) => s.items.some((i) => i.active))
  const isOpen = openId === group.id
  const triggerRef = useRef<HTMLButtonElement>(null)
  const [pos, setPos] = useState({ top: 0, left: 0 })

  useLayoutEffect(() => {
    if (isOpen && triggerRef.current) {
      const rect = triggerRef.current.getBoundingClientRect()
      setPos({ top: rect.top, left: rect.right + 4 })
    }
  }, [isOpen])

  return (
    <div className="relative">
      <button
        ref={triggerRef}
        onClick={() => onToggle(group.id)}
        className={cn(
          'w-14 py-2 flex flex-col items-center justify-center rounded-lg transition-colors cursor-pointer gap-1',
          anyActive && !isOpen && 'bg-primary/20 text-primary',
          isOpen && 'bg-accent text-foreground',
          !anyActive && !isOpen && 'text-muted-foreground hover:text-foreground hover:bg-accent',
        )}
        title={group.tooltip}
      >
        <group.icon size={22} strokeWidth={1.5} />
        <span className="text-[9px] leading-none tracking-wide">{group.tooltip.split(' ')[0]}</span>
      </button>

      {/* Portal to body so Three.js canvas cannot intercept pointer events */}
      {isOpen && createPortal(
        <div
          data-flyout-portal
          className="fixed z-[9999]"
          style={{ top: pos.top, left: pos.left }}
        >
          <div className="min-w-[180px] py-1 rounded-lg border border-border bg-card/95 backdrop-blur-md shadow-2xl">
          {group.sections.map((section, si) => (
            <div key={section.label}>
              {si > 0 && <div className="h-px bg-border/40 mx-2 my-1" />}
              <div className="px-3 py-1 text-[9px] uppercase tracking-widest text-muted-foreground/50 font-medium">
                {section.label}
              </div>
              {section.items.map((item) => (
                <button
                  key={item.label}
                  onClick={() => { item.action(); onToggle('') }}
                  className={cn(
                    'flex items-center gap-2.5 w-full px-3 py-1.5 text-xs transition-colors',
                    item.active
                      ? 'bg-primary/15 text-primary'
                      : 'text-foreground/80 hover:bg-accent hover:text-foreground',
                  )}
                >
                  <item.icon size={14} strokeWidth={1.5} className="shrink-0" />
                  <span className="flex-1 text-left">{item.label}</span>
                  {item.shortcut && (
                    <span className="text-[10px] text-muted-foreground/50 font-mono">{item.shortcut}</span>
                  )}
                </button>
              ))}
            </div>
          ))}
          </div>
        </div>,
        document.body,
      )}
    </div>
  )
}

// ─── Main toolbar ───────────────────────────────────────────────────

export function ToolBar() {
  const activeTool = useSceneStore((s) => s.activeTool)
  const setActiveTool = useSceneStore((s) => s.setActiveTool)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)
  const [openId, setOpenId] = useState<string | null>(null)
  const toolbarRef = useRef<HTMLDivElement>(null)

  const handleToolChange = useCallback((tool: TransformTool) => {
    if (useSceneStore.getState().selectionMode !== 'object') {
      setSelectionMode('object')
    }
    setActiveTool(tool)
  }, [setActiveTool, setSelectionMode])

  const handleToggle = useCallback((id: string) => {
    setOpenId((prev) => (prev === id ? null : id))
  }, [])

  // Close flyout on click outside toolbar + flyout portal
  useEffect(() => {
    if (!openId) return
    function onPointerDown(e: PointerEvent) {
      const target = e.target as HTMLElement
      // Keep open if clicking inside toolbar or inside a portal flyout
      if (toolbarRef.current?.contains(target)) return
      if (target.closest('[data-flyout-portal]')) return
      setOpenId(null)
    }
    document.addEventListener('pointerdown', onPointerDown, true)
    return () => document.removeEventListener('pointerdown', onPointerDown, true)
  }, [openId])

  const groups: ToolGroup[] = [
    // 1. Pointer / Transform / Selection — the core interaction
    {
      id: 'interact',
      icon: MousePointer2,
      tooltip: 'Transform & Selection',
      sections: [
        {
          label: 'Transform',
          items: [
            { icon: MousePointer2, label: 'Select', shortcut: 'V', active: activeTool === 'select', action: () => handleToolChange('select') },
            { icon: Move3d, label: 'Translate', shortcut: 'G', active: activeTool === 'translate', action: () => handleToolChange('translate') },
            { icon: RotateCw, label: 'Rotate', shortcut: 'R', active: activeTool === 'rotate', action: () => handleToolChange('rotate') },
            { icon: Maximize, label: 'Scale', shortcut: 'S', active: activeTool === 'scale', action: () => handleToolChange('scale') },
            { icon: FlipHorizontal, label: 'Mirror', action: () => sendCommand('mirror selected') },
          ],
        },
        {
          label: 'Selection Mode',
          items: [
            { icon: Box, label: 'Object', shortcut: '1', active: selectionMode === 'object', action: () => setSelectionMode('object') },
            { icon: Triangle, label: 'Face', shortcut: '2', active: selectionMode === 'face', action: () => setSelectionMode('face') },
            { icon: Minus, label: 'Edge', shortcut: '3', active: selectionMode === 'edge', action: () => setSelectionMode('edge') },
            { icon: CircleDot, label: 'Vertex', shortcut: '4', active: selectionMode === 'vertex', action: () => setSelectionMode('vertex') },
          ],
        },
      ],
    },

    // 2. Create — primitives + sketch
    {
      id: 'create',
      icon: Box,
      tooltip: 'Create Geometry',
      sections: [
        {
          label: 'Primitives',
          items: [
            { icon: Box, label: 'Box', action: () => sendCommand('create a box 10 10 10') },
            { icon: Circle, label: 'Sphere', action: () => sendCommand('create a sphere radius 5') },
            { icon: Cylinder, label: 'Cylinder', action: () => sendCommand('create a cylinder radius 5 height 10') },
            { icon: Triangle, label: 'Cone', action: () => sendCommand('create a cone bottom radius 5 height 10') },
            { icon: Torus, label: 'Torus', action: () => sendCommand('create a torus major radius 8 minor radius 2') },
          ],
        },
        {
          label: 'Sketch',
          items: [
            { icon: PenTool, label: 'New Sketch', action: () => sendCommand('create sketch on XY plane') },
            { icon: Minus, label: 'Line', action: () => sendCommand('sketch line') },
            { icon: Circle, label: 'Circle', action: () => sendCommand('sketch circle') },
            { icon: RectangleHorizontal, label: 'Rectangle', action: () => sendCommand('sketch rectangle') },
            { icon: Spline, label: 'Spline', action: () => sendCommand('sketch spline') },
            { icon: Waypoints, label: 'Arc', action: () => sendCommand('sketch arc') },
          ],
        },
      ],
    },

    // 3. Operations — extrude, revolve, booleans
    {
      id: 'operations',
      icon: ArrowUpFromLine,
      tooltip: 'Operations',
      sections: [
        {
          label: 'Solid',
          items: [
            { icon: ArrowUpFromLine, label: 'Extrude', action: () => sendCommand('extrude selected 10') },
            { icon: RefreshCcw, label: 'Revolve', action: () => sendCommand('revolve selected 360') },
            { icon: Layers, label: 'Loft', action: () => sendCommand('loft selected profiles') },
            { icon: Workflow, label: 'Sweep', action: () => sendCommand('sweep selected along path') },
          ],
        },
        {
          label: 'Boolean',
          items: [
            { icon: Combine, label: 'Union', action: () => sendCommand('union selected objects') },
            { icon: SquaresIntersect, label: 'Intersect', action: () => sendCommand('intersect selected objects') },
            { icon: Diff, label: 'Subtract', action: () => sendCommand('subtract selected objects') },
          ],
        },
      ],
    },

    // 4. Modify — fillet, chamfer, shell, pattern
    {
      id: 'modify',
      icon: Disc,
      tooltip: 'Modify & Pattern',
      sections: [
        {
          label: 'Modify',
          items: [
            { icon: Disc, label: 'Fillet', action: () => sendCommand('fillet selected edges radius 2') },
            { icon: Hexagon, label: 'Chamfer', action: () => sendCommand('chamfer selected edges distance 2') },
            { icon: SquareDashedBottom, label: 'Shell', action: () => sendCommand('shell selected thickness 1') },
            { icon: ScanLine, label: 'Offset', action: () => sendCommand('offset selected faces distance 2') },
            { icon: Scissors, label: 'Split', action: () => sendCommand('split selected body') },
            { icon: Orbit, label: 'Draft', action: () => sendCommand('apply draft angle 3 to selected faces') },
          ],
        },
        {
          label: 'Pattern',
          items: [
            { icon: Grid3x3, label: 'Linear Pattern', action: () => sendCommand('linear pattern selected count 3 spacing 15') },
            { icon: Orbit, label: 'Circular Pattern', action: () => sendCommand('circular pattern selected count 6 angle 360') },
            { icon: Hash, label: 'Rectangular', action: () => sendCommand('rectangular pattern selected 3x3 spacing 15') },
            { icon: Copy, label: 'Copy', action: () => sendCommand('copy selected') },
          ],
        },
      ],
    },

    // 5. Manufacturing
    {
      id: 'mfg',
      icon: Wrench,
      tooltip: 'Manufacturing & Analyze',
      sections: [
        {
          label: 'Manufacturing',
          items: [
            { icon: CircleDot, label: 'Hole', action: () => sendCommand('create hole diameter 5 depth 10') },
            { icon: Grip, label: 'Thread', action: () => sendCommand('create thread M10 pitch 1.5 length 15') },
            { icon: Component, label: 'Rib', action: () => sendCommand('create rib thickness 2') },
          ],
        },
        {
          label: 'Analyze',
          items: [
            { icon: Ruler, label: 'Measure Distance', action: () => sendCommand('measure distance between selected') },
            { icon: Pipette, label: 'Mass Properties', action: () => sendCommand('analyze mass of selected') },
            { icon: Eye, label: 'Section View', action: () => sendCommand('create section view XZ plane') },
            { icon: Wrench, label: 'Interference', action: () => sendCommand('check interference between selected') },
          ],
        },
      ],
    },

    // 6. Export
    {
      id: 'export',
      icon: FileDown,
      tooltip: 'Export',
      sections: [
        {
          label: 'Export',
          items: [
            { icon: FileDown, label: 'ROS (Roshera)', action: () => sendCommand('export selected as ROS') },
            { icon: FileDown, label: 'STEP', action: () => sendCommand('export selected as STEP') },
            { icon: FileDown, label: 'STL', action: () => sendCommand('export selected as STL') },
            { icon: FileDown, label: 'glTF', action: () => sendCommand('export selected as glTF') },
            { icon: FileDown, label: 'IGES', action: () => sendCommand('export selected as IGES') },
            { icon: FileDown, label: 'OBJ', action: () => sendCommand('export selected as OBJ') },
            { icon: FileDown, label: 'FBX', action: () => sendCommand('export selected as FBX') },
          ],
        },
      ],
    },
  ]

  return (
    <div ref={toolbarRef} className="flex flex-col items-center w-16 bg-card/80 backdrop-blur-sm border-r border-border py-2 gap-1 overflow-visible">
      {groups.map((group) => (
        <FlyoutGroup key={group.id} group={group} openId={openId} onToggle={handleToggle} />
      ))}
    </div>
  )
}
