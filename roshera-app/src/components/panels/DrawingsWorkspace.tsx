/**
 * Drawing-mode workspace.
 *
 * Replaces the 3D viewport when the user selects the "Drawing"
 * document-mode tab. Layout:
 *
 *   ┌───────────┬─────────────────────────────────────────┐
 *   │ Drawings  │                                         │
 *   │ list      │           SVG preview                   │
 *   │           │           (the active sheet)            │
 *   │ + New     │                                         │
 *   ├───────────┴─────────────────────────────────────────┤
 *   │  Add View [Front ▾]  scale [1.0] position [0,0]     │
 *   └─────────────────────────────────────────────────────┘
 *
 * All state is fetched from the backend on demand. The kernel renders
 * the SVG so we just drop the response into a `dangerouslySetInnerHTML`
 * wrapper (input is server-rendered, deterministic, no user-supplied
 * markup — the only XML-escaped channel is the drawing name which the
 * kernel's `escape_xml()` helper sanitises).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  type Drawing,
  type DrawingFormat,
  type ProjectionType,
  type SheetSize,
  addView,
  createDrawing,
  deleteDrawing,
  downloadDrawing,
  fetchDrawingSvg,
  getDrawing,
  listDrawings,
  removeView,
  renameDrawing,
  updateTitleBlock,
  defaultTitleBlock,
  type TitleBlock,
  type TitleBlockPatch,
} from '@/lib/drawings-api'
import { type PartSummary, listParts } from '@/lib/parts-api'

// Default solid id used when a part is freshly selected. Solid ids in
// the kernel are u32 sequence numbers starting at 1 — the first solid
// in any newly-created part is always 1. Users can edit the field to
// pick a later solid if the part contains several bodies.
const DEFAULT_SOLID_ID = 1

// The UI exposes only the 6 standard orthographic + isometric presets.
// The kernel's `custom` projection carries a 9-element rotation matrix
// which has no good keyboard-only editor; that variant is reachable
// only via the agent/REST surface.
type PresetProjectionKind = Exclude<ProjectionType['kind'], 'custom'>

const PROJECTION_OPTIONS: { kind: PresetProjectionKind; label: string }[] = [
  { kind: 'front', label: 'Front' },
  { kind: 'top', label: 'Top' },
  { kind: 'right', label: 'Right' },
  { kind: 'bottom', label: 'Bottom' },
  { kind: 'left', label: 'Left' },
  { kind: 'isometric', label: 'Isometric' },
]

const SHEET_OPTIONS: { value: SheetSize; label: string }[] = [
  { value: 'A4', label: 'A4 (297 × 210 mm)' },
  { value: 'A3', label: 'A3 (420 × 297 mm)' },
  { value: 'A2', label: 'A2 (594 × 420 mm)' },
  { value: 'A1', label: 'A1 (841 × 594 mm)' },
  { value: 'A0', label: 'A0 (1189 × 841 mm)' },
]

// CSS pixels per millimetre at the standard 96 DPI mapping. At zoom
// `1.0` the sheet renders at its true physical size.
const PX_PER_MM = 96 / 25.4

// Padding (CSS px) preserved around the sheet when computing the
// fit-to-window scale so the corners + drop shadow stay visible.
const FIT_PADDING_PX = 24

function sheetDimensionsMm(sheet: Drawing['sheet_size']): [number, number] {
  if (typeof sheet === 'string') {
    switch (sheet) {
      case 'A4':
        return [297, 210]
      case 'A3':
        return [420, 297]
      case 'A2':
        return [594, 420]
      case 'A1':
        return [841, 594]
      case 'A0':
        return [1189, 841]
      default:
        return [297, 210]
    }
  }
  // Custom variant: serde tags the struct payload as `{ CUSTOM: { width, height } }`.
  if (sheet && typeof sheet === 'object' && 'CUSTOM' in sheet) {
    const c = (sheet as { CUSTOM: { width: number; height: number } }).CUSTOM
    return [c.width, c.height]
  }
  return [297, 210]
}

// Drawing summary held in the sidebar — { id, name } pulled by issuing a
// `getDrawing` per id (the kernel doesn't expose a summary list endpoint;
// the drawing payload is small so per-row fetches are fine).
interface DrawingSummary {
  id: string
  name: string
}

// Suggest the next free "Roshera N" name given the names already on the
// server. Matches `^Roshera (\d+)$` and picks `max + 1`, falling back to
// `1` when no Roshera-N drawing exists yet. Custom names are ignored so
// the user's renames don't pollute the auto-suggested counter.
function suggestRosheraName(existing: { name: string }[]): string {
  let max = 0
  const re = /^Roshera\s+(\d+)$/
  for (const d of existing) {
    const m = re.exec(d.name)
    if (m) {
      const n = parseInt(m[1], 10)
      if (Number.isFinite(n) && n > max) max = n
    }
  }
  return `Roshera ${max + 1}`
}

export function DrawingsWorkspace() {
  const [drawings, setDrawings] = useState<DrawingSummary[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [activeDrawing, setActiveDrawing] = useState<Drawing | null>(null)
  const [svg, setSvg] = useState<string>('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Inline rename: which drawing's name is currently being edited in the
  // sidebar, and the draft value the user is typing.
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renameDraft, setRenameDraft] = useState<string>('')

  // Right-click context menu for sidebar drawing rows. `null` = closed.
  // `x`/`y` are viewport coordinates (we render at `position: fixed`).
  const [contextMenu, setContextMenu] = useState<{
    id: string
    name: string
    x: number
    y: number
  } | null>(null)

  // ── Form state ───────────────────────────────────────────────────
  const [newName, setNewName] = useState('Roshera 1')
  const [newSheet, setNewSheet] = useState<SheetSize>('A3')
  const [viewName, setViewName] = useState('Front')
  const [viewProjection, setViewProjection] = useState<PresetProjectionKind>('front')
  const [viewSolidId, setViewSolidId] = useState<number>(DEFAULT_SOLID_ID)
  const [viewScale, setViewScale] = useState<number>(1.0)
  const [viewX, setViewX] = useState<number>(50)
  const [viewY, setViewY] = useState<number>(50)
  // Part picker — populated from `GET /api/parts`. `viewPartId` is the
  // durable id passed into the `ViewSource::Part` on every add-view
  // call. Without a selection we cannot resolve a `BRepModel` server-
  // side, so the "Add View" button is disabled.
  const [parts, setParts] = useState<PartSummary[]>([])
  const [viewPartId, setViewPartId] = useState<string>('')
  // Download format selector. PDF is the default — it's the universal
  // review/archive deliverable and what most engineers expect when
  // they hit "Download" on a drawing. DXF is the editable interchange
  // format; SVG stays available for the lightweight web preview.
  const [downloadFormat, setDownloadFormat] = useState<DrawingFormat>('pdf')

  // ── Zoom / pan ────────────────────────────────────────────────────
  // `zoomMode = 'fit'` makes the sheet auto-shrink to fill the canvas
  // (recomputed on every container resize). `'manual'` pins the zoom
  // factor; 1.0 is true physical scale (1 mm of sheet = 1 mm on screen
  // at 96 DPI).
  //
  // Pan is a CSS `translate` offset applied to the sheet element. We
  // do NOT use container scrollbars because they bound pan to the
  // wrapper's overflow extent, which kept clipping access to the
  // right/bottom of the sheet at high zoom. Translating instead lets
  // the user drag the sheet to any position — same model used by
  // every production CAD viewer (Fusion, SolidWorks, Onshape).
  const [zoomMode, setZoomMode] = useState<'fit' | 'manual'>('fit')
  const [zoom, setZoom] = useState<number>(1)
  const [pan, setPan] = useState<{ x: number; y: number }>({ x: 0, y: 0 })
  const canvasRef = useRef<HTMLDivElement | null>(null)
  const [containerSize, setContainerSize] = useState({ w: 0, h: 0 })
  // Drag-pan working state. We mirror `pan` into a ref so the
  // mouse-move handler can read the latest value without re-binding
  // the listener; React state would lag by a frame.
  const panStateRef = useRef<{
    pan: { x: number; y: number }
    dragging: boolean
    startX: number
    startY: number
    startPan: { x: number; y: number }
  }>({ pan: { x: 0, y: 0 }, dragging: false, startX: 0, startY: 0, startPan: { x: 0, y: 0 } })
  useEffect(() => {
    panStateRef.current.pan = pan
  }, [pan])

  useEffect(() => {
    const el = canvasRef.current
    if (!el) return
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const r = entry.contentRect
        setContainerSize({ w: r.width, h: r.height })
      }
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // ── Data fetchers ────────────────────────────────────────────────
  const refreshList = useCallback(async () => {
    setError(null)
    try {
      const ids = await listDrawings()
      // Resolve summaries in parallel — payload is tiny (no polylines
      // would be cheaper, but the kernel's `Drawing` JSON for a fresh
      // drawing is already < 200 B and we hit the cache after the
      // first round-trip).
      const summaries = await Promise.all(
        ids.map((id) =>
          getDrawing(id)
            .then((d) => ({ id, name: d.name }))
            .catch(() => ({ id, name: id.slice(0, 8) }) as DrawingSummary),
        ),
      )
      setDrawings(summaries)
      // If the previously active drawing was deleted, clear selection.
      if (activeId && !ids.includes(activeId)) {
        setActiveId(null)
        setActiveDrawing(null)
        setSvg('')
      }
      // If nothing is selected yet, pick the first.
      if (!activeId && ids.length > 0) {
        setActiveId(ids[0])
      }
      // Keep the "New Drawing" name input one ahead of the existing
      // Roshera-N counter so the user can spam "+ New" without
      // bumping the field by hand.
      setNewName(suggestRosheraName(summaries))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [activeId])

  const refreshActive = useCallback(async (id: string) => {
    setLoading(true)
    setError(null)
    try {
      const [d, s] = await Promise.all([getDrawing(id), fetchDrawingSvg(id)])
      setActiveDrawing(d)
      setSvg(s)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setActiveDrawing(null)
      setSvg('')
    } finally {
      setLoading(false)
    }
  }, [])

  const refreshParts = useCallback(async () => {
    try {
      const ps = await listParts()
      setParts(ps)
      // Seed the picker with the first part if nothing is selected yet
      // (or if the previously selected part has been deleted).
      setViewPartId((current) => {
        if (current && ps.some((p) => p.id === current)) return current
        return ps[0]?.id ?? ''
      })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useEffect(() => {
    void refreshList()
    void refreshParts()
    // refreshList captures activeId for the deleted-id check; we only
    // want this on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (activeId) {
      void refreshActive(activeId)
    } else {
      setActiveDrawing(null)
      setSvg('')
    }
    // Switching drawings resets the view so the new sheet is centred
    // in fit mode without inheriting the previous drawing's pan.
    setPan({ x: 0, y: 0 })
  }, [activeId, refreshActive])

  // ── Mutations ────────────────────────────────────────────────────
  const handleCreate = useCallback(async () => {
    if (!newName.trim()) return
    setError(null)
    try {
      const id = await createDrawing(newName.trim(), newSheet)
      await refreshList()
      setActiveId(id)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [newName, newSheet, refreshList])

  // Commit the rename to the server, refresh the active drawing so the
  // SVG (title-block TITLE text) re-renders, and exit edit mode. On
  // empty/whitespace input we silently exit without firing the PATCH.
  const commitRename = useCallback(
    async (id: string, name: string) => {
      const trimmed = name.trim()
      setRenamingId(null)
      setRenameDraft('')
      if (!trimmed) return
      // No-op if unchanged.
      const current = drawings.find((d) => d.id === id)?.name
      if (current === trimmed) return
      setError(null)
      try {
        await renameDrawing(id, trimmed)
        await refreshList()
        if (activeId === id) {
          await refreshActive(id)
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [drawings, activeId, refreshList, refreshActive],
  )

  const beginRename = useCallback((id: string, currentName: string) => {
    setRenamingId(id)
    setRenameDraft(currentName)
  }, [])

  const handleDelete = useCallback(
    async (id: string) => {
      if (!confirm('Delete this drawing?')) return
      setError(null)
      try {
        await deleteDrawing(id)
        if (activeId === id) {
          setActiveId(null)
        }
        await refreshList()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [activeId, refreshList],
  )

  const handleAddView = useCallback(async () => {
    if (!activeId) return
    if (!viewName.trim()) return
    if (!viewPartId) {
      setError('Select a part to project before adding a view.')
      return
    }
    setError(null)
    try {
      await addView(activeId, {
        name: viewName.trim(),
        source: { kind: 'part', part_id: viewPartId, solid_id: viewSolidId },
        projection: { kind: viewProjection },
        position_mm: [viewX, viewY],
        scale: viewScale,
      })
      await refreshActive(activeId)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [
    activeId,
    viewName,
    viewPartId,
    viewSolidId,
    viewProjection,
    viewX,
    viewY,
    viewScale,
    refreshActive,
  ])

  const handleRemoveView = useCallback(
    async (viewId: string) => {
      if (!activeId) return
      setError(null)
      try {
        await removeView(activeId, viewId)
        await refreshActive(activeId)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [activeId, refreshActive],
  )

  const handleDownload = useCallback(async () => {
    if (!activeDrawing) return
    setError(null)
    try {
      await downloadDrawing(activeDrawing.id, activeDrawing.name, downloadFormat)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [activeDrawing, downloadFormat])

  // The SVG is server-rendered & deterministic. The kernel's
  // `escape_xml` helper sanitises every text-node string, and we never
  // splice user-supplied markup back into the payload. Safe to inline.
  const svgMarkup = useMemo(() => ({ __html: svg }), [svg])

  // Sheet physical dimensions + scaled display dimensions.
  const [sheetWMm, sheetHMm] = useMemo(
    () =>
      activeDrawing ? sheetDimensionsMm(activeDrawing.sheet_size) : [297, 210],
    [activeDrawing],
  )
  const sheetWPx = sheetWMm * PX_PER_MM
  const sheetHPx = sheetHMm * PX_PER_MM

  const fitScale = useMemo(() => {
    if (containerSize.w <= 0 || containerSize.h <= 0) return 1
    const pad = FIT_PADDING_PX * 2
    const sx = Math.max(0.01, (containerSize.w - pad) / sheetWPx)
    const sy = Math.max(0.01, (containerSize.h - pad) / sheetHPx)
    return Math.min(sx, sy)
  }, [containerSize, sheetWPx, sheetHPx])

  const effectiveScale = zoomMode === 'fit' ? fitScale : zoom
  const displayW = Math.max(1, sheetWPx * effectiveScale)
  const displayH = Math.max(1, sheetHPx * effectiveScale)

  const zoomIn = useCallback(() => {
    setZoom((prev) => {
      const base = zoomMode === 'fit' ? fitScale : prev
      return Math.min(base * 1.25, 16)
    })
    setZoomMode('manual')
  }, [zoomMode, fitScale])

  const zoomOut = useCallback(() => {
    setZoom((prev) => {
      const base = zoomMode === 'fit' ? fitScale : prev
      return Math.max(base / 1.25, 0.05)
    })
    setZoomMode('manual')
  }, [zoomMode, fitScale])

  const zoomFit = useCallback(() => {
    setZoomMode('fit')
    setPan({ x: 0, y: 0 })
  }, [])
  const zoomActual = useCallback(() => {
    setZoom(1)
    setZoomMode('manual')
    setPan({ x: 0, y: 0 })
  }, [])

  // ── Wheel / trackpad zoom ────────────────────────────────────────
  // Every wheel event zooms — no Ctrl required. That covers:
  //   • mouse wheel scroll → zoom (CAD convention, not page scroll)
  //   • trackpad two-finger scroll → zoom
  //   • trackpad pinch (wheel + ctrlKey) → zoom
  // Zoom is anchored at the cursor: the world point under the
  // cursor stays fixed across the zoom step, so users can drill into
  // any region by scrolling over it. React's synthetic `onWheel` is
  // passive in modern React; we attach the native listener with
  // `{ passive: false }` so `preventDefault()` actually stops the
  // browser from scrolling the page.
  useEffect(() => {
    const el = canvasRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      e.preventDefault()
      const rect = el.getBoundingClientRect()
      const cursorX = e.clientX - rect.left
      const cursorY = e.clientY - rect.top
      const oldScale = zoomMode === 'fit' ? fitScale : zoom
      // Exponential mapping smooths over the wide deltaY range
      // browsers report (Chromium ±1-4, macOS pinch ±10-30, mouse
      // wheel ±100).
      const factor = Math.exp(-e.deltaY * 0.005)
      const newScale = Math.max(0.05, Math.min(16, oldScale * factor))
      // Sheet is rendered centred in the canvas + translated by
      // `pan`. Top-left of the sheet in canvas coords is:
      //   tl = centre − sheetSize·scale/2 + pan
      // The world point under the cursor (in unscaled sheet px) is:
      //   world = (cursor − tl) / oldScale
      // After zoom we want the same world point under the same
      // cursor position:
      //   pan' = cursor − centre + sheetSize·newScale/2 − world·newScale
      const centerX = rect.width / 2
      const centerY = rect.height / 2
      const currentPan = panStateRef.current.pan
      const tlX = centerX - (sheetWPx * oldScale) / 2 + currentPan.x
      const tlY = centerY - (sheetHPx * oldScale) / 2 + currentPan.y
      const worldX = (cursorX - tlX) / oldScale
      const worldY = (cursorY - tlY) / oldScale
      const newPanX = cursorX - centerX + (sheetWPx * newScale) / 2 - worldX * newScale
      const newPanY = cursorY - centerY + (sheetHPx * newScale) / 2 - worldY * newScale
      setZoom(newScale)
      setZoomMode('manual')
      setPan({ x: newPanX, y: newPanY })
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [zoomMode, zoom, fitScale, sheetWPx, sheetHPx])

  // ── Drag-to-pan ──────────────────────────────────────────────────
  // Left-click drag updates the `pan` translation directly. Because
  // the sheet is positioned with CSS `transform: translate(...)` and
  // its parent is `overflow-hidden`, the user can drag it anywhere —
  // there are no scroll boundaries to clip against.
  const onCanvasMouseDown = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    // Ignore clicks on the zoom toolbar so the buttons still work.
    if ((e.target as HTMLElement).closest('button')) return
    if (e.button !== 0) return
    const el = canvasRef.current
    if (!el) return
    panStateRef.current.dragging = true
    panStateRef.current.startX = e.clientX
    panStateRef.current.startY = e.clientY
    panStateRef.current.startPan = { ...panStateRef.current.pan }
    el.style.cursor = 'grabbing'
    e.preventDefault()
  }, [])

  useEffect(() => {
    const onMouseMove = (e: MouseEvent) => {
      const s = panStateRef.current
      if (!s.dragging) return
      const dx = e.clientX - s.startX
      const dy = e.clientY - s.startY
      setPan({ x: s.startPan.x + dx, y: s.startPan.y + dy })
    }
    const onMouseUp = () => {
      const el = canvasRef.current
      if (el) el.style.cursor = ''
      panStateRef.current.dragging = false
    }
    window.addEventListener('mousemove', onMouseMove)
    window.addEventListener('mouseup', onMouseUp)
    return () => {
      window.removeEventListener('mousemove', onMouseMove)
      window.removeEventListener('mouseup', onMouseUp)
    }
  }, [])

  // Context-menu dismissal: any click outside the menu, Escape, or
  // window scroll/resize closes it. Registered only while open so we
  // don't pay listener cost in the common case.
  useEffect(() => {
    if (!contextMenu) return
    const dismiss = () => setContextMenu(null)
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') dismiss()
    }
    // mousedown (not click) so we dismiss before the underlying row's
    // click handler can fire — the user expects "click elsewhere closes
    // the menu without also activating that elsewhere".
    window.addEventListener('mousedown', dismiss)
    window.addEventListener('keydown', onKey)
    window.addEventListener('resize', dismiss)
    window.addEventListener('scroll', dismiss, true)
    return () => {
      window.removeEventListener('mousedown', dismiss)
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('resize', dismiss)
      window.removeEventListener('scroll', dismiss, true)
    }
  }, [contextMenu])

  return (
    <div className="flex flex-col flex-1 min-h-0 bg-background text-foreground">
      {/* Error banner */}
      {error && (
        <div className="px-3 py-2 text-xs bg-destructive/10 text-destructive border-b border-destructive/30">
          {error}
        </div>
      )}

      <div className="flex flex-1 min-h-0">
        {/* ── Drawings list sidebar ───────────────────────────────── */}
        <aside className="w-64 flex flex-col border-r border-border/60 bg-background/40">
          <div className="px-3 py-2 border-b border-border/60">
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
              Drawings
            </div>
          </div>

          {/* New drawing form */}
          <div className="px-3 py-2 space-y-2 border-b border-border/40">
            <input
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder="Name"
              className="cad-focus w-full px-2 py-1 text-xs rounded border border-border bg-background"
            />
            <select
              value={typeof newSheet === 'string' ? newSheet : 'CUSTOM'}
              onChange={(e) => setNewSheet(e.target.value as SheetSize)}
              className="cad-focus w-full px-2 py-1 text-xs rounded border border-border bg-background"
            >
              {SHEET_OPTIONS.map((s) => (
                <option key={s.label} value={s.value as string}>
                  {s.label}
                </option>
              ))}
            </select>
            <button
              type="button"
              onClick={handleCreate}
              className="cad-focus w-full py-1 text-xs font-medium rounded bg-primary text-primary-foreground hover:opacity-90"
            >
              + New Drawing
            </button>
          </div>

          {/* Existing drawings */}
          <div className="flex-1 min-h-0 overflow-y-auto py-1">
            {drawings.length === 0 ? (
              <div className="px-3 py-4 text-xs text-muted-foreground text-center">
                No drawings yet.
              </div>
            ) : (
              drawings.map((d) => {
                const isActive = d.id === activeId
                const isRenaming = renamingId === d.id
                // Row layout — no onClick on the row itself; the
                // activation click target is a sibling button next to
                // the icon buttons. This removes ALL click-bubble
                // ambiguity (the previous nested layout could swallow
                // the icon-button clicks even with stopPropagation).
                return (
                  <div
                    key={d.id}
                    className={[
                      'group flex items-stretch gap-0.5 text-xs',
                      isActive
                        ? 'bg-accent/40 text-foreground'
                        : 'text-muted-foreground hover:bg-accent/20',
                    ].join(' ')}
                    onContextMenu={(e) => {
                      e.preventDefault()
                      setContextMenu({ id: d.id, name: d.name, x: e.clientX, y: e.clientY })
                    }}
                  >
                    {isRenaming ? (
                      // Inline rename input. Enter commits, Escape cancels.
                      // No onBlur auto-commit: it raced with autoFocus on
                      // mount (focus transitioning through body could fire
                      // a stale blur immediately) AND double-fired after
                      // Enter/Escape (the unmount blur triggered a redundant
                      // commitRename). Explicit Enter/Escape is predictable
                      // and matches Finder/Explorer rename UX.
                      <input
                        autoFocus
                        value={renameDraft}
                        onChange={(e) => setRenameDraft(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter') {
                            e.preventDefault()
                            void commitRename(d.id, renameDraft)
                          } else if (e.key === 'Escape') {
                            e.preventDefault()
                            setRenamingId(null)
                            setRenameDraft('')
                          }
                        }}
                        title="Press Enter to save, Esc to cancel"
                        className="cad-focus flex-1 min-w-0 mx-3 my-0.5 px-1 py-0 text-xs rounded border border-border bg-background"
                      />
                    ) : (
                      <>
                        {/* Activation target — selects the drawing.
                            Double-click enters rename. Single-click
                            elsewhere does NOT trigger the icon buttons
                            because they are siblings, not children. */}
                        <button
                          type="button"
                          onClick={() => setActiveId(d.id)}
                          onDoubleClick={() => beginRename(d.id, d.name)}
                          title="Double-click to rename"
                          className="cad-focus flex-1 min-w-0 px-3 py-1.5 text-left truncate"
                        >
                          {d.name}
                        </button>
                        {/* Icon button group — siblings of the
                            activation button. `flex-shrink-0` keeps
                            them visible regardless of name length;
                            `opacity-60` keeps them discoverable at
                            rest. The 24×24 click target meets the
                            24-px MAS touch-target minimum. */}
                        <div className="flex-shrink-0 flex items-center gap-0.5 pr-2 opacity-60 group-hover:opacity-100 transition-opacity">
                          <button
                            type="button"
                            title="Rename drawing"
                            aria-label="Rename drawing"
                            onClick={() => beginRename(d.id, d.name)}
                            className="cad-focus inline-flex items-center justify-center w-6 h-6 rounded text-muted-foreground hover:text-foreground hover:bg-accent/40 transition-colors"
                          >
                            <svg
                              width="12"
                              height="12"
                              viewBox="0 0 16 16"
                              fill="none"
                              stroke="currentColor"
                              strokeWidth="1.5"
                              strokeLinecap="round"
                              strokeLinejoin="round"
                              aria-hidden="true"
                            >
                              <path d="M11 2.5l2.5 2.5L5 13.5H2.5V11z" />
                            </svg>
                          </button>
                          <button
                            type="button"
                            title="Delete drawing"
                            aria-label="Delete drawing"
                            onClick={() => void handleDelete(d.id)}
                            className="cad-focus inline-flex items-center justify-center w-6 h-6 rounded text-destructive hover:text-destructive hover:bg-destructive/15 transition-colors"
                          >
                            <svg
                              width="12"
                              height="12"
                              viewBox="0 0 16 16"
                              fill="none"
                              stroke="currentColor"
                              strokeWidth="1.75"
                              strokeLinecap="round"
                              strokeLinejoin="round"
                              aria-hidden="true"
                            >
                              <path d="M4 4l8 8M12 4l-8 8" />
                            </svg>
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                )
              })
            )}
          </div>
        </aside>

        {/* ── Center pane: SVG preview ─────────────────────────────── */}
        <main className="flex-1 flex flex-col min-h-0">
          {/* Active-drawing header. The title acts as a one-click
              rename entry-point — clicking the displayed name swaps it
              for an inline input, and committing flows through the
              same `renameDrawing` PATCH the sidebar uses. The title
              block in the SVG is regenerated from `drawing.name`, so
              committing here re-renders the title block. */}
          <div className="flex items-center justify-between px-4 py-2 border-b border-border/60">
            <div className="flex items-center gap-3">
              {activeDrawing && renamingId === activeDrawing.id ? (
                // See sidebar input: Enter commits, Esc cancels. No onBlur
                // auto-commit (raced with autoFocus + double-fired on
                // Enter/Escape unmount blur).
                <input
                  autoFocus
                  value={renameDraft}
                  onChange={(e) => setRenameDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault()
                      void commitRename(activeDrawing.id, renameDraft)
                    } else if (e.key === 'Escape') {
                      e.preventDefault()
                      setRenamingId(null)
                      setRenameDraft('')
                    }
                  }}
                  title="Press Enter to save, Esc to cancel"
                  className="cad-focus text-sm font-medium px-2 py-0.5 rounded border border-border bg-background min-w-[18ch]"
                />
              ) : (
                <button
                  type="button"
                  onClick={() => {
                    if (activeDrawing) beginRename(activeDrawing.id, activeDrawing.name)
                  }}
                  disabled={!activeDrawing}
                  className="cad-focus text-sm font-medium hover:underline disabled:cursor-default disabled:no-underline"
                  title={activeDrawing ? 'Click to rename (title-block TITLE)' : ''}
                >
                  {activeDrawing?.name ?? (loading ? 'Loading…' : '— no drawing selected —')}
                </button>
              )}
              {activeDrawing && (
                <span className="text-[10px] uppercase tracking-wider text-muted-foreground">
                  {typeof activeDrawing.sheet_size === 'string'
                    ? activeDrawing.sheet_size
                    : 'Custom'}
                  &nbsp;·&nbsp;
                  {activeDrawing.views.length} view{activeDrawing.views.length === 1 ? '' : 's'}
                </span>
              )}
            </div>
            {activeDrawing && (
              <div className="flex items-center gap-1">
                <select
                  value={downloadFormat}
                  onChange={(e) => setDownloadFormat(e.target.value as DrawingFormat)}
                  title="Download format"
                  className="cad-focus px-2 py-1 text-xs rounded border border-border bg-background"
                >
                  <option value="pdf">PDF</option>
                  <option value="dxf">DXF</option>
                  <option value="svg">SVG</option>
                </select>
                <button
                  type="button"
                  onClick={() => void handleDownload()}
                  className="cad-focus px-3 py-1 text-xs rounded border border-border hover:bg-accent/20"
                >
                  Download
                </button>
              </div>
            )}
          </div>

          {/* SVG canvas. Outer is `overflow-hidden` — pan/zoom is
              driven by a CSS transform on the sheet, so there are no
              scroll boundaries. The sheet is absolutely positioned at
              the canvas centre and translated by `pan`; that lets the
              user drag it anywhere in 2D regardless of zoom level.
              The svg's intrinsic `width="297mm"` is overridden by CSS
              (`[&>svg]:w-full [&>svg]:h-full`) so it scales to the
              wrapper's pixel dimensions while keeping its `viewBox`
              aspect.
          */}
          <div
            ref={canvasRef}
            className="flex-1 min-h-0 overflow-hidden bg-muted/20 relative cursor-grab"
            onMouseDown={onCanvasMouseDown}
          >
            {activeDrawing ? (
              svg ? (
                <div
                  className="absolute top-1/2 left-1/2 bg-white shadow-md [&>svg]:block [&>svg]:w-full [&>svg]:h-full"
                  style={{
                    width: `${displayW}px`,
                    height: `${displayH}px`,
                    transform: `translate(calc(-50% + ${pan.x}px), calc(-50% + ${pan.y}px))`,
                    willChange: 'transform',
                  }}
                  dangerouslySetInnerHTML={svgMarkup}
                />
              ) : (
                <div className="absolute inset-0 flex items-center justify-center text-xs text-muted-foreground">
                  Rendering…
                </div>
              )
            ) : (
              <div className="absolute inset-0 flex items-center justify-center text-xs text-muted-foreground text-center px-6">
                {drawings.length === 0
                  ? 'Create a drawing from the sidebar to get started.'
                  : 'Select a drawing from the sidebar.'}
              </div>
            )}

            {/* Zoom toolbar. Always visible while a drawing is open
                so the user can rescue the view if the sheet renders
                off-screen at native physical size. */}
            {activeDrawing && (
              <div className="absolute bottom-3 right-3 flex items-center gap-0.5 bg-background/95 backdrop-blur border border-border rounded-md shadow-md px-1 py-1 select-none">
                <button
                  type="button"
                  onClick={zoomOut}
                  title="Zoom out"
                  className="cad-focus px-2 py-0.5 text-sm leading-none hover:bg-accent/30 rounded"
                >
                  −
                </button>
                <button
                  type="button"
                  onClick={zoomFit}
                  title="Fit to window"
                  className={[
                    'cad-focus px-2 py-0.5 text-[11px] hover:bg-accent/30 rounded',
                    zoomMode === 'fit' ? 'bg-accent/40' : '',
                  ].join(' ')}
                >
                  Fit
                </button>
                <span className="px-1.5 text-[11px] text-muted-foreground tabular-nums w-12 text-center">
                  {Math.round(effectiveScale * 100)}%
                </span>
                <button
                  type="button"
                  onClick={zoomActual}
                  title="Actual size (1 mm = 1 mm at 96 DPI)"
                  className={[
                    'cad-focus px-2 py-0.5 text-[11px] hover:bg-accent/30 rounded',
                    zoomMode === 'manual' && zoom === 1 ? 'bg-accent/40' : '',
                  ].join(' ')}
                >
                  100%
                </button>
                <button
                  type="button"
                  onClick={zoomIn}
                  title="Zoom in"
                  className="cad-focus px-2 py-0.5 text-sm leading-none hover:bg-accent/30 rounded"
                >
                  +
                </button>
              </div>
            )}
          </div>

          {/* Views list + add-view form */}
          {activeDrawing && (
            <div className="border-t border-border/60 bg-background/40">
              {/* Existing views */}
              {activeDrawing.views.length > 0 && (
                <div className="px-4 py-2 border-b border-border/40 flex flex-wrap gap-2">
                  {activeDrawing.views.map((v) => (
                    <span
                      key={v.id}
                      className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-accent/30 text-[11px]"
                    >
                      <span className="font-mono">{v.name}</span>
                      <span className="text-muted-foreground">({v.projection.kind})</span>
                      <button
                        type="button"
                        onClick={() => void handleRemoveView(v.id)}
                        title="Remove view"
                        className="cad-focus ml-1 text-destructive hover:text-destructive/80 text-[10px]"
                      >
                        ✕
                      </button>
                    </span>
                  ))}
                </div>
              )}

              {/* Title block editor — collapsed by default so it never
                  steals attention from the sheet, but accessible in one
                  click. All edits PATCH on blur and merge the response
                  back into local state so the rendered SVG refreshes
                  immediately. */}
              <TitleBlockEditor
                drawing={activeDrawing}
                onUpdated={(updated) => {
                  setActiveDrawing((prev) =>
                    prev ? { ...prev, title_block: updated } : prev,
                  )
                  // Re-fetch the SVG so the live sheet preview reflects
                  // the new title-block text. `refreshActive` is async
                  // but we don't await it — the local state update has
                  // already redrawn the editor; the SVG catches up on
                  // its own.
                  void refreshActive(activeDrawing.id)
                }}
                onError={setError}
              />

              {/* Add-view form */}
              <div className="flex flex-wrap items-end gap-2 px-4 py-2">
                <Field label="View name">
                  <input
                    value={viewName}
                    onChange={(e) => setViewName(e.target.value)}
                    className="cad-focus w-28 px-2 py-1 text-xs rounded border border-border bg-background"
                  />
                </Field>
                <Field label="Projection">
                  <select
                    value={viewProjection}
                    onChange={(e) =>
                      setViewProjection(e.target.value as PresetProjectionKind)
                    }
                    className="cad-focus px-2 py-1 text-xs rounded border border-border bg-background"
                  >
                    {PROJECTION_OPTIONS.map((p) => (
                      <option key={p.kind} value={p.kind}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Part">
                  <select
                    value={viewPartId}
                    onChange={(e) => setViewPartId(e.target.value)}
                    onFocus={() => void refreshParts()}
                    title="Source part — geometry is resolved server-side from the active PartManager."
                    className="cad-focus min-w-[10rem] px-2 py-1 text-xs rounded border border-border bg-background"
                  >
                    {parts.length === 0 ? (
                      <option value="">No parts</option>
                    ) : (
                      parts.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.name} ({p.solid_count} solid{p.solid_count === 1 ? '' : 's'})
                        </option>
                      ))
                    )}
                  </select>
                </Field>
                <Field label="Solid id">
                  <input
                    type="number"
                    min={1}
                    value={viewSolidId}
                    onChange={(e) => setViewSolidId(parseInt(e.target.value, 10) || 1)}
                    className="cad-focus w-16 px-2 py-1 text-xs rounded border border-border bg-background"
                  />
                </Field>
                <Field label="Scale">
                  <input
                    type="number"
                    min={0.01}
                    step={0.1}
                    value={viewScale}
                    onChange={(e) => setViewScale(parseFloat(e.target.value) || 1.0)}
                    className="cad-focus w-16 px-2 py-1 text-xs rounded border border-border bg-background"
                  />
                </Field>
                <Field label="X (mm)">
                  <input
                    type="number"
                    value={viewX}
                    onChange={(e) => setViewX(parseFloat(e.target.value) || 0)}
                    className="cad-focus w-16 px-2 py-1 text-xs rounded border border-border bg-background"
                  />
                </Field>
                <Field label="Y (mm)">
                  <input
                    type="number"
                    value={viewY}
                    onChange={(e) => setViewY(parseFloat(e.target.value) || 0)}
                    className="cad-focus w-16 px-2 py-1 text-xs rounded border border-border bg-background"
                  />
                </Field>
                <button
                  type="button"
                  onClick={handleAddView}
                  disabled={!viewPartId}
                  title={viewPartId ? '' : 'Select a part first.'}
                  className="cad-focus px-3 py-1 text-xs font-medium rounded bg-primary text-primary-foreground hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed"
                >
                  + Add View
                </button>
              </div>
            </div>
          )}
        </main>
      </div>
      {/* Right-click context menu for sidebar drawing rows. Rendered
          at the document root via `position: fixed` so it escapes any
          ancestor `overflow:hidden`. `mousedown` stop-propagation is
          load-bearing: the global dismiss listener (also `mousedown`)
          would otherwise close the menu before its `onClick` fires. */}
      {contextMenu && (
        <div
          role="menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onMouseDown={(e) => e.stopPropagation()}
          className="fixed z-50 min-w-[160px] rounded border border-border bg-popover text-popover-foreground shadow-lg py-1 text-xs"
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              beginRename(contextMenu.id, contextMenu.name)
              setContextMenu(null)
            }}
            className="w-full text-left px-3 py-1.5 hover:bg-accent/40"
          >
            Rename
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              const id = contextMenu.id
              setContextMenu(null)
              void handleDelete(id)
            }}
            className="w-full text-left px-3 py-1.5 hover:bg-destructive/15 text-destructive"
          >
            Delete
          </button>
        </div>
      )}
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-0.5">
      <span className="text-[10px] uppercase tracking-wider text-muted-foreground">{label}</span>
      {children}
    </label>
  )
}

/**
 * Compact, collapsible editor for the drawing's title-block metadata.
 *
 * Strategy: maintain a local `draft` copy of the title block so users
 * can type freely. On blur, diff against the original and PATCH only
 * the changed fields. The server returns the canonical updated block,
 * which we lift back up via `onUpdated`.
 *
 * `drawing.name` and the SCALE / SIZE cells are NOT shown here — they
 * are edited elsewhere (the sidebar rename for TITLE; the sheet size
 * picker on create; per-view scale). This keeps the editor focused on
 * the cells that are *only* reachable through this surface.
 */
function TitleBlockEditor({
  drawing,
  onUpdated,
  onError,
}: {
  drawing: Drawing
  onUpdated: (updated: TitleBlock) => void
  onError: (msg: string) => void
}) {
  // Server truth — re-seeded when the drawing's title_block reference
  // changes (e.g. after the user switches drawings or a PATCH lifts
  // canonical state back up). Falls back to defaults if the api-server
  // is pre-upgrade and doesn't ship the field yet — the editor stays
  // visible so the user can still type values; the PATCH will land
  // once the backend is restarted.
  //
  // **Critical**: memoise on `drawing.title_block` identity. Without
  // this, the `?? defaultTitleBlock()` fallback minted a fresh object
  // every render, the effect below fired every render, and every
  // keystroke was immediately overwritten — input fields appeared to
  // be read-only.
  const original = useMemo(
    () => drawing.title_block ?? defaultTitleBlock(),
    [drawing.title_block],
  )
  const [draft, setDraft] = useState<TitleBlock>(original)
  useEffect(() => {
    setDraft(original)
  }, [original])

  // Diff helper: produces the smallest patch that explains the
  // difference between `draft` and `original`. `drawing_number` is
  // special-cased because the wire shape distinguishes "omit", "null"
  // (clear), and "string" (set).
  const buildPatch = useCallback((): TitleBlockPatch | null => {
    const patch: TitleBlockPatch = {}
    if (draft.drawn_by !== original.drawn_by) patch.drawn_by = draft.drawn_by
    if (draft.date !== original.date) patch.date = draft.date
    if (draft.material !== original.material) patch.material = draft.material
    if (draft.revision !== original.revision) patch.revision = draft.revision
    if (draft.sheet_index !== original.sheet_index) patch.sheet_index = draft.sheet_index
    if (draft.sheet_count !== original.sheet_count) patch.sheet_count = draft.sheet_count
    if (draft.drawing_number !== original.drawing_number) {
      // Empty string is treated as "clear override" on the client.
      const v = draft.drawing_number
      patch.drawing_number = v && v.trim() !== '' ? v : null
    }
    return Object.keys(patch).length === 0 ? null : patch
  }, [draft, original])

  const commit = useCallback(async () => {
    const patch = buildPatch()
    if (!patch) return
    try {
      const updated = await updateTitleBlock(drawing.id, patch)
      onUpdated(updated)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      // Roll back to the server-known good state so the user is not
      // left looking at edits that didn't land.
      setDraft(original)
    }
  }, [buildPatch, drawing.id, onError, onUpdated, original])

  // `clearDrawingNumber` is the explicit "revert to auto-derived" path.
  const clearDrawingNumber = useCallback(() => {
    setDraft((d) => ({ ...d, drawing_number: null }))
  }, [])

  return (
    <details className="border-b border-border/40 group">
      <summary className="cursor-pointer select-none px-4 py-1.5 text-[11px] uppercase tracking-wider text-muted-foreground hover:text-foreground hover:bg-accent/20 transition-colors">
        Title block
        <span className="ml-2 text-foreground/70 normal-case tracking-normal">
          {summariseTitleBlock(draft)}
        </span>
      </summary>
      <form
        className="flex flex-wrap items-end gap-2 px-4 pb-2 pt-1"
        onSubmit={(e) => {
          e.preventDefault()
          void commit()
        }}
      >
        <Field label="Drawn by">
          <input
            value={draft.drawn_by}
            placeholder="—"
            onChange={(e) => setDraft({ ...draft, drawn_by: e.target.value })}
            onBlur={() => void commit()}
            className="cad-focus w-28 px-2 py-1 text-xs rounded border border-border bg-background"
          />
        </Field>
        <Field label="Date">
          <input
            value={draft.date}
            placeholder="YYYY-MM-DD"
            onChange={(e) => setDraft({ ...draft, date: e.target.value })}
            onBlur={() => void commit()}
            className="cad-focus w-28 px-2 py-1 text-xs rounded border border-border bg-background"
          />
        </Field>
        <Field label="Material">
          <input
            value={draft.material}
            placeholder="e.g. 6061-T6"
            onChange={(e) => setDraft({ ...draft, material: e.target.value })}
            onBlur={() => void commit()}
            className="cad-focus w-32 px-2 py-1 text-xs rounded border border-border bg-background"
          />
        </Field>
        <Field label="Drawing no.">
          <div className="flex items-center gap-1">
            <input
              value={draft.drawing_number ?? ''}
              placeholder="auto"
              onChange={(e) => setDraft({ ...draft, drawing_number: e.target.value })}
              onBlur={() => void commit()}
              className="cad-focus w-32 px-2 py-1 text-xs rounded border border-border bg-background"
            />
            {draft.drawing_number !== null && (
              <button
                type="button"
                onClick={() => {
                  clearDrawingNumber()
                  // Persist immediately so the user gets a clean "auto"
                  // state without needing to tab out of a field that no
                  // longer holds focus.
                  setTimeout(() => void commit(), 0)
                }}
                title="Revert to auto-derived ID"
                className="cad-focus text-[10px] text-muted-foreground hover:text-foreground px-1"
              >
                auto
              </button>
            )}
          </div>
        </Field>
        <Field label="Rev">
          <input
            value={draft.revision}
            placeholder="A"
            onChange={(e) => setDraft({ ...draft, revision: e.target.value })}
            onBlur={() => void commit()}
            className="cad-focus w-12 px-2 py-1 text-xs rounded border border-border bg-background"
          />
        </Field>
        <Field label="Sheet">
          <div className="flex items-center gap-1">
            <input
              type="number"
              min={1}
              value={draft.sheet_index}
              onChange={(e) =>
                setDraft({
                  ...draft,
                  sheet_index: Math.max(1, parseInt(e.target.value, 10) || 1),
                })
              }
              onBlur={() => void commit()}
              className="cad-focus w-12 px-2 py-1 text-xs rounded border border-border bg-background"
            />
            <span className="text-[10px] text-muted-foreground">of</span>
            <input
              type="number"
              min={1}
              value={draft.sheet_count}
              onChange={(e) =>
                setDraft({
                  ...draft,
                  sheet_count: Math.max(1, parseInt(e.target.value, 10) || 1),
                })
              }
              onBlur={() => void commit()}
              className="cad-focus w-12 px-2 py-1 text-xs rounded border border-border bg-background"
            />
          </div>
        </Field>
        {/* Submit on Enter without showing a button — the form's
            onSubmit handles it. */}
        <button type="submit" className="sr-only">
          Save
        </button>
      </form>
    </details>
  )
}

/** One-line summary shown next to the `<summary>` so the collapsed
 *  state still reveals who/when/material at a glance. */
function summariseTitleBlock(tb: TitleBlock): string {
  const drawn = tb.drawn_by.trim() || '—'
  const date = tb.date.trim() || '—'
  const rev = tb.revision.trim() || '—'
  return `${drawn} · ${date} · Rev ${rev} · Sheet ${tb.sheet_index} of ${tb.sheet_count}`
}
