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
  Workflow,
  RectangleHorizontal,
  SquareDashedBottom,
  type LucideIcon,
} from 'lucide-react'
import { useSceneStore, type TransformTool } from '@/stores/scene-store'
import { useChatStore } from '@/stores/chat-store'
import { processUserMessage } from '@/lib/ai-client'
import { exportSceneAs } from '@/lib/export-api'
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

// ─── Direct geometry API (bypasses NLP pipeline) ────────────────────

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/**
 * Send a structured geometry command directly to the REST API.
 * Deterministic operations (create primitive, boolean, export) don't need
 * NLP parsing — this eliminates latency and misinterpretation risk.
 *
 * Backend remains the single source of truth: the kernel mutates topology,
 * registers the new object, and broadcasts `ObjectCreated` over the
 * WebSocket. `ws-bridge.ts` consumes that frame and adds the object to
 * the local scene store with the full mesh + per-triangle `faceIds` map.
 * This function never adds objects locally — doing so would (a) duplicate
 * the entry on the WS broadcast and (b) drop `faceIds` because the REST
 * response shape predates per-triangle face mapping.
 *
 * Falls back to NLP pipeline if the direct endpoint fails.
 */
async function sendDirectGeometry(
  shapeType: string,
  parameters: Record<string, number>,
) {
  const { addMessage, setProcessing } = useChatStore.getState()
  const label = `${shapeType} (${Object.entries(parameters).map(([k, v]) => `${k}=${v}`).join(', ')})`
  // eslint-disable-next-line no-console
  console.log('[toolbar] sendDirectGeometry click', { shapeType, parameters, url: `${API_BASE}/geometry` })
  addMessage({ role: 'user', content: `Create ${label}` })
  setProcessing(true)

  try {
    const resp = await fetch(`${API_BASE}/geometry`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        shape_type: shapeType,
        parameters,
        position: [0, 0, 0],
      }),
    })

    // eslint-disable-next-line no-console
    console.log('[toolbar] /api/geometry response', { ok: resp.ok, status: resp.status, statusText: resp.statusText })
    if (!resp.ok) {
      const errBody = await resp.text().catch(() => '')
      // eslint-disable-next-line no-console
      console.error('[toolbar] /api/geometry error body:', errBody)
      throw new Error(`${resp.status} ${errBody}`)
    }

    const data = await resp.json()
    // eslint-disable-next-line no-console
    console.log('[toolbar] /api/geometry data', {
      success: data?.success,
      objectId: data?.object?.id,
      vertices: data?.stats?.vertex_count,
      triangles: data?.stats?.triangle_count,
      ms: data?.stats?.tessellation_ms,
    })
    if (data?.success !== true || !data.object) {
      throw new Error(data?.error || 'malformed response')
    }

    const objectId = String(data.object.id)
    const stats = data.stats
      ? ` (${data.stats.vertex_count} verts, ${data.stats.triangle_count} tris, ${data.stats.tessellation_ms} ms)`
      : ''
    addMessage({
      role: 'assistant',
      content: `Created ${shapeType}${stats}.`,
      objectsAffected: [objectId],
    })
  } catch (err) {
    // Direct API unavailable — fall back to NLP pipeline
    // eslint-disable-next-line no-console
    console.warn('[toolbar] direct geometry failed, falling back to NLP', err)
    await processUserMessage(`create a ${shapeType} ${Object.entries(parameters).map(([k, v]) => `${k} ${v}`).join(' ')}`)
  } finally {
    useChatStore.getState().setProcessing(false)
  }
}

/**
 * Run a boolean operation against the two currently-selected objects
 * via the REST API. The kernel consumes both operands, broadcasts
 * `ObjectDeleted` for each, then broadcasts `ObjectCreated` for the
 * result — `ws-bridge.ts` reconciles the local scene store. This
 * function does not mutate scene state directly; doing so would
 * duplicate the result on the WS broadcast and drop the per-triangle
 * `faceIds` map (the REST response predates that field).
 */
async function sendDirectBoolean(
  operation: 'union' | 'intersection' | 'difference',
) {
  const { addMessage, setProcessing } = useChatStore.getState()
  const selectedIds = Array.from(useSceneStore.getState().selectedIds)

  if (selectedIds.length < 2) {
    addMessage({
      role: 'assistant',
      content: `Select two objects before running ${operation}.`,
    })
    return
  }

  const [a, b] = selectedIds
  addMessage({ role: 'user', content: `${operation} (${a.slice(0, 6)} ↔ ${b.slice(0, 6)})` })
  setProcessing(true)

  try {
    const resp = await fetch(`${API_BASE}/geometry/boolean`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ operation, object_a: a, object_b: b }),
    })

    if (!resp.ok) {
      const errBody = await resp.json().catch(() => ({}))
      throw new Error(errBody?.error || `${resp.status}`)
    }

    const data = await resp.json()
    if (data?.success !== true || !data.object) {
      throw new Error(data?.error || 'malformed response')
    }

    const objectId = String(data.object.id)
    const stats = data.stats
      ? ` (${data.stats.vertex_count} verts, ${data.stats.triangle_count} tris, ${data.stats.tessellation_ms} ms)`
      : ''
    addMessage({
      role: 'assistant',
      content: `${operation}${stats}.`,
      objectsAffected: [objectId],
    })
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    addMessage({
      role: 'assistant',
      content: `${operation} failed: ${msg}`,
    })
  } finally {
    useChatStore.getState().setProcessing(false)
  }
}

/**
 * Hollow the currently-selected solid via the existing direct REST
 * endpoint. Bypasses the NLP pipeline so a missing `ANTHROPIC_API_KEY`
 * — which 5xxs every `/api/ai/command` request — can't block this
 * deterministic op. Same swap-UUID semantics as the boolean handler:
 * the kernel registers a fresh UUID for the hollow solid and the
 * frontend's WS bridge reconciles the scene store.
 */
async function sendDirectShell(thickness: number) {
  const { addMessage, setProcessing } = useChatStore.getState()
  const selectedIds = Array.from(useSceneStore.getState().selectedIds)

  if (selectedIds.length !== 1) {
    addMessage({
      role: 'assistant',
      content: 'Select exactly one solid before running Shell.',
    })
    return
  }
  if (!Number.isFinite(thickness) || thickness <= 0) {
    addMessage({
      role: 'assistant',
      content: `Shell thickness must be a positive number, got ${thickness}.`,
    })
    return
  }

  const [object] = selectedIds
  addMessage({ role: 'user', content: `Shell ${object.slice(0, 6)} thickness ${thickness}` })
  setProcessing(true)

  try {
    const resp = await fetch(`${API_BASE}/geometry/shell`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ object, thickness }),
    })
    if (!resp.ok) {
      const errBody = await resp.json().catch(() => ({}))
      throw new Error(errBody?.error || `${resp.status}`)
    }
    const data = await resp.json()
    if (data?.success !== true || !data.object) {
      throw new Error(data?.error || 'malformed response')
    }
    addMessage({
      role: 'assistant',
      content: `Shelled ${object.slice(0, 6)} (thickness ${thickness}).`,
      objectsAffected: [String(data.object.id)],
    })
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    addMessage({ role: 'assistant', content: `Shell failed: ${msg}` })
  } finally {
    setProcessing(false)
  }
}

/**
 * Mirror the currently-selected solid across a coordinate plane. The
 * kernel mirror op is in-place; the backend swaps the public UUID so
 * viewers see a fresh ObjectCreated frame (no ObjectUpdated frame
 * exists in the protocol yet). Plane defaults to XY (mirror through
 * the world Z=0 plane).
 */
async function sendDirectMirror(plane: 'xy' | 'yz' | 'xz' = 'xy') {
  const { addMessage, setProcessing } = useChatStore.getState()
  const selectedIds = Array.from(useSceneStore.getState().selectedIds)

  if (selectedIds.length !== 1) {
    addMessage({
      role: 'assistant',
      content: 'Select exactly one solid before running Mirror.',
    })
    return
  }

  const [object] = selectedIds
  const plane_normal: [number, number, number] =
    plane === 'xy' ? [0, 0, 1] : plane === 'yz' ? [1, 0, 0] : [0, 1, 0]

  addMessage({ role: 'user', content: `Mirror ${object.slice(0, 6)} across ${plane.toUpperCase()} plane` })
  setProcessing(true)

  try {
    const resp = await fetch(`${API_BASE}/geometry/mirror`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        object,
        plane_origin: [0, 0, 0],
        plane_normal,
      }),
    })
    if (!resp.ok) {
      const errBody = await resp.json().catch(() => ({}))
      throw new Error(errBody?.error || `${resp.status}`)
    }
    const data = await resp.json()
    if (data?.success !== true || !data.object) {
      throw new Error(data?.error || 'malformed response')
    }
    addMessage({
      role: 'assistant',
      content: `Mirrored ${object.slice(0, 6)} across ${plane.toUpperCase()}.`,
      objectsAffected: [String(data.object.id)],
    })
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    addMessage({ role: 'assistant', content: `Mirror failed: ${msg}` })
  } finally {
    setProcessing(false)
  }
}

/**
 * Direct REST fillet against the currently picked edges of the
 * currently selected solid. Reads `subElementSelections` from the
 * scene store, filters to `type === 'edge'` matching the selected
 * solid, and POSTs `{object, edges, radius}` to
 * `/api/geometry/fillet`. Surfaces a clear chat message if no
 * edges are picked instead of routing through the NLP pipeline.
 */
async function sendDirectFillet(radius: number) {
  const { addMessage, setProcessing } = useChatStore.getState()
  const sceneState = useSceneStore.getState()
  const selectedIds = Array.from(sceneState.selectedIds)

  if (selectedIds.length !== 1) {
    addMessage({
      role: 'assistant',
      content: 'Select exactly one solid before running Fillet.',
    })
    return
  }
  if (!Number.isFinite(radius) || radius <= 0) {
    addMessage({
      role: 'assistant',
      content: 'Fillet radius must be a positive number.',
    })
    return
  }

  const [object] = selectedIds
  const edges = sceneState.subElementSelections
    .filter((s) => s.type === 'edge' && s.objectId === object)
    .map((s) => s.index)

  if (edges.length === 0) {
    addMessage({
      role: 'assistant',
      content:
        'Pick one or more edges (Edge selection mode → click edges) before running Fillet.',
    })
    return
  }

  addMessage({
    role: 'user',
    content: `Fillet ${edges.length} edge${edges.length === 1 ? '' : 's'} of ${object.slice(0, 6)} (radius ${radius})`,
  })
  setProcessing(true)

  try {
    const resp = await fetch(`${API_BASE}/geometry/fillet`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ object, edges, radius }),
    })
    if (!resp.ok) {
      const errBody = await resp.json().catch(() => ({}))
      throw new Error(errBody?.error || `${resp.status}`)
    }
    const data = await resp.json()
    if (data?.success !== true || !data.object) {
      throw new Error(data?.error || 'malformed response')
    }
    addMessage({
      role: 'assistant',
      content: `Filleted ${edges.length} edge${edges.length === 1 ? '' : 's'} of ${object.slice(0, 6)} at radius ${radius}.`,
      objectsAffected: [String(data.object.id)],
    })
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    addMessage({ role: 'assistant', content: `Fillet failed: ${msg}` })
  } finally {
    setProcessing(false)
  }
}

/**
 * Direct REST chamfer mirroring sendDirectFillet. Equal-distance
 * chamfer (distance1 == distance2 == distance) — most common case.
 */
async function sendDirectChamfer(distance: number) {
  const { addMessage, setProcessing } = useChatStore.getState()
  const sceneState = useSceneStore.getState()
  const selectedIds = Array.from(sceneState.selectedIds)

  if (selectedIds.length !== 1) {
    addMessage({
      role: 'assistant',
      content: 'Select exactly one solid before running Chamfer.',
    })
    return
  }
  if (!Number.isFinite(distance) || distance <= 0) {
    addMessage({
      role: 'assistant',
      content: 'Chamfer distance must be a positive number.',
    })
    return
  }

  const [object] = selectedIds
  const edges = sceneState.subElementSelections
    .filter((s) => s.type === 'edge' && s.objectId === object)
    .map((s) => s.index)

  if (edges.length === 0) {
    addMessage({
      role: 'assistant',
      content:
        'Pick one or more edges (Edge selection mode → click edges) before running Chamfer.',
    })
    return
  }

  addMessage({
    role: 'user',
    content: `Chamfer ${edges.length} edge${edges.length === 1 ? '' : 's'} of ${object.slice(0, 6)} (distance ${distance})`,
  })
  setProcessing(true)

  try {
    const resp = await fetch(`${API_BASE}/geometry/chamfer`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ object, edges, distance }),
    })
    if (!resp.ok) {
      const errBody = await resp.json().catch(() => ({}))
      throw new Error(errBody?.error || `${resp.status}`)
    }
    const data = await resp.json()
    if (data?.success !== true || !data.object) {
      throw new Error(data?.error || 'malformed response')
    }
    addMessage({
      role: 'assistant',
      content: `Chamfered ${edges.length} edge${edges.length === 1 ? '' : 's'} of ${object.slice(0, 6)} at distance ${distance}.`,
      objectsAffected: [String(data.object.id)],
    })
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    addMessage({ role: 'assistant', content: `Chamfer failed: ${msg}` })
  } finally {
    setProcessing(false)
  }
}

/**
 * Placeholder for modify ops that don't yet have a direct REST
 * endpoint. Tells the user the feature is pending instead of routing
 * through the NLP pipeline (which 5xxs without `ANTHROPIC_API_KEY`)
 * and surfacing a confusing "Failed to reach backend" error.
 */
function notYetWired(feature: string, reason?: string) {
  const { addMessage } = useChatStore.getState()
  const tail = reason ? ` (${reason})` : ''
  addMessage({
    role: 'assistant',
    content: `${feature} is not yet wired to a direct backend endpoint${tail}. Coming in a follow-up slice.`,
  })
}

/**
 * Export the current selection (or whole scene if nothing is selected)
 * directly via `POST /api/export`. Bypasses the NLP pipeline so a
 * missing `ANTHROPIC_API_KEY` (which 5xxs the AI command path) can't
 * block deterministic export operations. Reports success / failure to
 * the chat panel so the user gets visible feedback either way.
 */
async function sendDirectExport(format: string) {
  const { addMessage, setProcessing } = useChatStore.getState()
  addMessage({ role: 'user', content: `Export selected as ${format}` })
  setProcessing(true)
  try {
    const result = await exportSceneAs(format)
    if (result.ok) {
      addMessage({
        role: 'assistant',
        content: result.filename
          ? `Exported as ${result.filename}.`
          : `Export ready.`,
      })
    } else {
      addMessage({
        role: 'assistant',
        content: `Export failed: ${result.error ?? 'unknown error'}`,
      })
    }
  } finally {
    setProcessing(false)
  }
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
          'cad-focus w-14 py-2 flex flex-col items-center justify-center rounded-lg transition-colors cursor-pointer gap-1',
          anyActive && !isOpen && 'bg-primary/20 text-primary',
          isOpen && 'bg-accent text-foreground',
          !anyActive && !isOpen && 'text-muted-foreground hover:text-foreground hover:bg-accent',
        )}
        title={group.tooltip}
        aria-label={group.tooltip}
        aria-expanded={isOpen}
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
          <div className="cad-panel-floating min-w-[180px] py-1 rounded-lg">
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
            { icon: FlipHorizontal, label: 'Mirror', action: () => sendDirectMirror('xy') },
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
            { icon: Box, label: 'Box', action: () => sendDirectGeometry('box', { width: 10, height: 10, depth: 10 }) },
            { icon: Circle, label: 'Sphere', action: () => sendDirectGeometry('sphere', { radius: 5 }) },
            { icon: Cylinder, label: 'Cylinder', action: () => sendDirectGeometry('cylinder', { radius: 5, height: 10 }) },
            { icon: Triangle, label: 'Cone', action: () => sendDirectGeometry('cone', { radius: 5, height: 10 }) },
            { icon: Torus, label: 'Torus', action: () => sendDirectGeometry('torus', { major_radius: 8, minor_radius: 2 }) },
          ],
        },
        {
          label: 'Sketch',
          items: [
            { icon: PenTool, label: 'New Sketch', shortcut: 'K', action: () => useSceneStore.getState().enterSketch('xy', 'polyline') },
            { icon: PenTool, label: 'Polyline', action: () => useSceneStore.getState().enterSketch('xy', 'polyline') },
            { icon: RectangleHorizontal, label: 'Rectangle', action: () => useSceneStore.getState().enterSketch('xy', 'rectangle') },
            { icon: Circle, label: 'Circle', action: () => useSceneStore.getState().enterSketch('xy', 'circle') },
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
            { icon: ArrowUpFromLine, label: 'Extrude', action: () => notYetWired('Extrude', 'use Sketch → Finish → Extrude flow') },
            { icon: RefreshCcw, label: 'Revolve', action: () => notYetWired('Revolve') },
            { icon: Layers, label: 'Loft', action: () => notYetWired('Loft') },
            { icon: Workflow, label: 'Sweep', action: () => notYetWired('Sweep') },
          ],
        },
        {
          label: 'Boolean',
          items: [
            { icon: Combine, label: 'Union', action: () => sendDirectBoolean('union') },
            { icon: SquaresIntersect, label: 'Intersect', action: () => sendDirectBoolean('intersection') },
            { icon: Diff, label: 'Subtract', action: () => sendDirectBoolean('difference') },
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
            { icon: Disc, label: 'Fillet', action: () => sendDirectFillet(2) },
            { icon: Hexagon, label: 'Chamfer', action: () => sendDirectChamfer(1) },
            { icon: SquareDashedBottom, label: 'Shell', action: () => sendDirectShell(1) },
            { icon: ScanLine, label: 'Offset', action: () => notYetWired('Offset', 'needs face-selection UX') },
            { icon: Scissors, label: 'Split', action: () => notYetWired('Split') },
            { icon: Orbit, label: 'Draft', action: () => notYetWired('Draft', 'needs face-selection UX') },
          ],
        },
        {
          label: 'Pattern',
          items: [
            { icon: Grid3x3, label: 'Linear Pattern', action: () => notYetWired('Linear Pattern') },
            { icon: Orbit, label: 'Circular Pattern', action: () => notYetWired('Circular Pattern') },
            { icon: Hash, label: 'Rectangular', action: () => notYetWired('Rectangular Pattern') },
            { icon: Copy, label: 'Copy', action: () => notYetWired('Copy') },
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
            { icon: CircleDot, label: 'Hole', action: () => notYetWired('Hole') },
            { icon: Grip, label: 'Thread', action: () => notYetWired('Thread') },
            { icon: Component, label: 'Rib', action: () => notYetWired('Rib') },
          ],
        },
        {
          label: 'Analyze',
          items: [
            { icon: Ruler, label: 'Measure Distance', action: () => notYetWired('Measure Distance') },
            { icon: Pipette, label: 'Mass Properties', action: () => notYetWired('Mass Properties', 'use /api/agent/parts/{id}') },
            { icon: Eye, label: 'Section View', action: () => notYetWired('Section View') },
            { icon: Wrench, label: 'Interference', action: () => notYetWired('Interference') },
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
            { icon: FileDown, label: 'ROS (Roshera)', action: () => sendDirectExport('ROS') },
            { icon: FileDown, label: 'STEP', action: () => sendDirectExport('STEP') },
            { icon: FileDown, label: 'STL', action: () => sendDirectExport('STL') },
            { icon: FileDown, label: 'OBJ', action: () => sendDirectExport('OBJ') },
          ],
        },
      ],
    },
  ]

  return (
    <div ref={toolbarRef} className="flex flex-col items-center w-16 cad-panel border-r py-2 gap-1 overflow-visible">
      {groups.map((group) => (
        <FlyoutGroup key={group.id} group={group} openId={openId} onToggle={handleToggle} />
      ))}
    </div>
  )
}
