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

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { X } from 'lucide-react'
import {
  DrawingSwitcher,
  SheetActions,
  SheetHint,
  TitleBlockCard,
  ViewsBar,
  ViewMenu,
  type PartRow,
} from '@/components/panels/drawing-overlays'
import {
  type Drawing,
  type DrawingFormat,
  type ProjectionAngle,
  type SheetDimension,
  type DimensionDriverResolution,
  getDimensionDriver,
  DrawingRefusal,
  setDrawingDimension,
  setDrawingProjection,
  addView,
  createSolidDrawing,
  listDocumentParts,
  deleteDrawing,
  downloadDrawing,
  fetchDrawingSvg,
  getDrawing,
  listDrawings,
  removeView,
  renameDrawing,
  updateView,
  type ViewEdit,
} from '@/lib/drawings-api'
import { useDocumentStore } from '@/stores/document-store'
import { HISTORY_CHANGED_EVENT, timelineAction } from '@/lib/history'
import { useDocModeStore } from '@/stores/doc-mode-store'

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

/** What the change box says will change, before Apply. */
function driverLine(d: DimensionDriverResolution | null): React.ReactNode {
  if (!d) return <span className="text-muted-foreground">Finding what drives it…</span>
  switch (d.verdict) {
    case 'driven': {
      const steps = [d.driver.target_sequence, ...(d.driver.linked ?? []).map((l) => l.target_sequence)]
      const where = steps.length > 1 ? `steps ${steps.join(' + ')}` : `step ${steps[0]}`
      return (
        <span>
          Changes <span className="font-medium">{d.driver.parameter}</span> ({where}, now{' '}
          {Number(d.driver.value.toFixed(4))})
        </span>
      )
    }
    case 'reference_only':
      return <span className="text-amber-700">Reference only: {d.reason}</span>
    case 'ambiguous':
      return (
        <span className="text-amber-700">
          Several values drive it ({d.drivers.map((x) => `${x.parameter} step ${x.target_sequence}`).join(', ')})
        </span>
      )
    case 'undecided':
      return <span className="text-amber-700">Could not tell what drives it: {d.reason}</span>
  }
}

/** A dimension kind in a drafter's words, for the change box. */
const DIM_KIND_WORDS: Record<string, string> = {
  extent: 'overall size',
  length: 'length',
  diameter: 'diameter',
  radius: 'radius',
  position: 'location',
  angle: 'angle',
  hole_callout: 'hole',
  pcd: 'pitch circle',
}

export function DrawingsWorkspace() {
  const [drawings, setDrawings] = useState<DrawingSummary[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [activeDrawing, setActiveDrawing] = useState<Drawing | null>(null)
  const [svg, setSvg] = useState<string>('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // ── Overlay state ────────────────────────────────────────────────
  const [titleBlockOpen, setTitleBlockOpen] = useState(false)
  const [viewsBusy, setViewsBusy] = useState(false)
  // The last removed view, so its removal can be undone.
  const [removedView, setRemovedView] = useState<{
    name: string
    projection: Drawing['views'][number]['projection']
    position_mm: [number, number]
    scale: number
  } | null>(null)
  // Parts in this document a drawing can start from, asked of the server.
  const [parts, setParts] = useState<PartRow[]>([])
  const refreshParts = useCallback(async () => {
    try {
      const ps = await listDocumentParts()
      setParts(ps.map((p) => ({ id: String(p.id), name: p.name })))
    } catch {
      // The menu then says there are no parts; the server is unreachable.
      setParts([])
    }
  }, [])

  // ── Editing the part from the sheet ──────────────────────────────
  // Clicking a dimension opens this editor at the cursor. Applying a new
  // size edits the parameter that drives it, so the PART changes and the
  // sheet regenerates. A refusal (reference-only dimension, several
  // drivers, unreachable size) is shown in the editor, never swallowed.
  const [dimEdit, setDimEdit] = useState<{
    dim: SheetDimension
    /** The view the dimension is drawn in, e.g. "TOP". */
    view: string
    x: number
    y: number
    draft: string
    busy: boolean
    /** When the current request started (ms), for the elapsed counter. */
    startedAt: number | null
    /** What changes when this dimension changes; null while searching. */
    driver: DimensionDriverResolution | null
    refusal: string | null
  } | null>(null)
  // The in-flight edit request, so Stop can abandon it.
  const dimEditAbort = useRef<AbortController | null>(null)
  // Ticks once a second while an edit is running (drives the counter).
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!dimEdit?.busy) return
    const t = window.setInterval(() => setTick((n) => n + 1), 1000)
    return () => window.clearInterval(t)
  }, [dimEdit?.busy])
  // One-line outcome of the last edit, e.g. "Ø6.00 -> 8 (radius 3 -> 4)".
  const [editNote, setEditNote] = useState<string | null>(null)
  // A note is either a confirmation (✓) or information the engineer should
  // read (ⓘ) -- a fallback is not a success.
  const [noteKind, setNoteKind] = useState<'done' | 'info'>('done')
  const [projectionBusy, setProjectionBusy] = useState(false)

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
  // every production CAD viewer.
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
  const refreshList = useCallback(async (preferId?: string) => {
    setError(null)
    try {
      // `GET /api/drawings` now returns `{id, owner}` per entry rather than
      // a bare id: a drawing is bound to the model that produced it, and the
      // server discloses which. Only the ids are needed here, but the owner
      // is deliberately available rather than discarded at the fetch layer.
      // Only this document's sheets: a drawing bound to another document
      // cannot render against this model (the server answers 409), so
      // listing it here only produces an error banner. Part-owned drawings
      // carry no document id and stay listed.
      const docs = useDocumentStore.getState()
      if (docs.documents.length === 0) await docs.refresh()
      const activeDoc = useDocumentStore.getState().documents.find((d) => d.active)?.id
      const entries = (await listDrawings()).filter(
        (e) => e.owner.kind !== 'legacy' || !activeDoc || e.owner.document_id === activeDoc,
      )
      const ids = entries.map((e) => e.id)
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
      // A caller-preferred drawing (e.g. the viewport "Create Drawing"
      // flow) wins over every other selection rule when it actually
      // exists on the server.
      if (preferId && ids.includes(preferId)) {
        setActiveId(preferId)
      } else {
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
      }
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

  // A document switch (including the app reopening the last document right
  // after load, which can finish AFTER this panel first listed sheets) must
  // reload the sheet list and the parts: otherwise the panel shows the
  // previous document's sheets and loading them fails (409).
  // Undo / redo rewinds the history; the server rebuilds the sheets from it.
  // Reload what this window shows.
  useEffect(() => {
    const onHistory = () => {
      void refreshList(activeId ?? undefined)
      if (activeId) void refreshActive(activeId)
    }
    window.addEventListener(HISTORY_CHANGED_EVENT, onHistory)
    return () => window.removeEventListener(HISTORY_CHANGED_EVENT, onHistory)
  }, [activeId, refreshList, refreshActive])

  const documentEpoch = useDocumentStore((st) => st.epoch)
  const firstEpoch = useRef(documentEpoch)
  useEffect(() => {
    if (documentEpoch === firstEpoch.current) return
    setActiveId(null)
    setActiveDrawing(null)
    setSvg('')
    setError(null)
    void refreshList()
    void refreshParts()
    // Only a change of document triggers this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentEpoch])

  useEffect(() => {
    // The viewport "Create Drawing" flow stashes the new drawing id and
    // flips the workspace mode; consume it here so this mount lands on
    // that sheet instead of the first in the list.
    const pending = useDocModeStore.getState().consumePendingDrawing()
    void refreshList(pending ?? undefined)
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
  // A drawing starts from a part: the server builds the standard sheet.
  const handleCreateFromPart = useCallback(
    async (part: PartRow) => {
      setError(null)
      try {
        const id = await createSolidDrawing(Number(part.id), part.name)
        await refreshList(id)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [refreshList],
  )

  // Commit the rename to the server, refresh the active drawing so the
  // SVG (title-block TITLE text) re-renders, and exit edit mode. On
  // empty/whitespace input we silently exit without firing the PATCH.
  const commitRename = useCallback(
    async (id: string, name: string) => {
      const trimmed = name.trim()
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

  // "Add view ▸ Left": another view of the part already on the sheet; the
  // kernel places it in free space (or refuses when the sheet is full).
  const handleAddLike = useCallback(
    async (kind: 'front' | 'top' | 'right' | 'left' | 'bottom' | 'isometric', label: string) => {
      if (!activeId || !activeDrawing) return
      const like =
        activeDrawing.views.find((v) => v.projection.kind === 'front') ?? activeDrawing.views[0]
      if (!like) return
      setError(null)
      setViewsBusy(true)
      try {
        const { aligned } = await addView(activeId, {
          name: label.toUpperCase(),
          like_view: like.id,
          projection: { kind },
        })
        // The view is back (or new): the last removal's Undo no longer applies.
        setRemovedView(null)
        if (!aligned && kind !== 'isometric') {
          setNoteKind('info')
          setEditNote(
            `No room ${label === 'Left' || label === 'Right' ? 'beside' : 'above or below'} ${like.name} for ${label.toUpperCase()}; placed in the nearest free space. A larger sheet keeps views aligned.`,
          )
        }
        await refreshActive(activeId)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setViewsBusy(false)
      }
    },
    [activeId, activeDrawing, refreshActive],
  )

  // Remove a view, keeping what is needed to put it back exactly.
  const handleRemoveView = useCallback(
    async (viewId: string) => {
      if (!activeId || !activeDrawing) return
      const v = activeDrawing.views.find((x) => x.id === viewId)
      if (!v) return
      setError(null)
      setViewsBusy(true)
      try {
        await removeView(activeId, viewId)
        setRemovedView({
          name: v.name,
          projection: v.projection,
          position_mm: v.position_mm,
          scale: v.scale,
        })
        await refreshActive(activeId)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setViewsBusy(false)
      }
    },
    [activeId, activeDrawing, refreshActive],
  )

  const undoRemoveView = useCallback(async () => {
    if (!removedView) return
    setViewsBusy(true)
    try {
      // History undo: the view comes back exactly as it was (its id, its
      // hidden-line setting, its scale), not a fresh copy.
      if (await timelineAction('undo')) setRemovedView(null)
      else setError('Undo did not go through; see the Blackboard for the reason.')
    } finally {
      setViewsBusy(false)
    }
  }, [removedView])

  // ── Selecting a view and its menu ──────────────────────────────────────────────
  const [selectedViewId, setSelectedViewId] = useState<string | null>(null)
  const [viewMenu, setViewMenu] = useState<{ viewId: string; x: number; y: number } | null>(null)
  const closeViewMenu = useCallback(() => setViewMenu(null), [])
  const openViewMenu = useCallback((viewId: string, x: number, y: number) => {
    setSelectedViewId(viewId)
    setViewMenu({ viewId, x, y })
  }, [])
  // A different sheet (or document) drops the selection.
  useEffect(() => {
    setSelectedViewId(null)
    setViewMenu(null)
  }, [activeId])

  /** The view whose drawn box contains a screen point (smallest wins). */
  const viewAt = useCallback((clientX: number, clientY: number): string | null => {
    const root = canvasRef.current
    if (!root) return null
    let best: { id: string; area: number } | null = null
    root.querySelectorAll('[data-view-id]').forEach((el) => {
      const r = el.getBoundingClientRect()
      const pad = 4
      if (
        clientX >= r.left - pad &&
        clientX <= r.right + pad &&
        clientY >= r.top - pad &&
        clientY <= r.bottom + pad
      ) {
        const area = r.width * r.height
        const id = el.getAttribute('data-view-id')
        if (id && (!best || area < best.area)) best = { id, area }
      }
    })
    return (best as { id: string; area: number } | null)?.id ?? null
  }, [])

  const handleViewEdit = useCallback(
    async (viewId: string, edit: ViewEdit) => {
      if (!activeId || !activeDrawing) return
      const v = activeDrawing.views.find((x) => x.id === viewId)
      if (!v) return
      setError(null)
      setViewsBusy(true)
      try {
        const { moved, aligned } = await updateView(activeId, viewId, edit)
        await refreshActive(activeId)
        setRemovedView(null)
        const what =
          edit.show_hidden === false
            ? `Hidden lines removed on ${v.name}`
            : edit.show_hidden === true
              ? `Hidden lines shown on ${v.name}`
              : edit.scale !== undefined
                ? `${v.name} now at ${edit.scale >= 1 ? `${edit.scale}:1` : `1:${Number((1 / edit.scale).toFixed(2))}`}${
                    moved
                      ? aligned
                        ? '; moved to fit, still in line with FRONT'
                        : '; moved to the nearest free space (no room in line with FRONT)'
                      : ''
                  }`
                : `${v.name} renamed ${edit.name ?? ''}`
        setNoteKind(moved && !aligned ? 'info' : 'done')
        setEditNote(`${what}. Ctrl+Z undoes it.`)
      } catch (e) {
        setNoteKind('info')
        setEditNote(e instanceof DrawingRefusal ? `Not changed: ${e.message}` : null)
        if (!(e instanceof DrawingRefusal)) setError(e instanceof Error ? e.message : String(e))
      } finally {
        setViewsBusy(false)
      }
    },
    [activeId, activeDrawing, refreshActive],
  )

  // Delete removes the selected view; Escape lets go of it. Not while the
  // engineer types, and not while a dimension's change box is open.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!selectedViewId) return
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
      if (dimEdit) return
      if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault()
        const id = selectedViewId
        setSelectedViewId(null)
        setViewMenu(null)
        void handleRemoveView(id)
      } else if (e.key === 'Escape') {
        setSelectedViewId(null)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selectedViewId, dimEdit, handleRemoveView])

  // The selected view's outline, measured off the rendered sheet (follows
  // pan, zoom and re-renders).
  const [selRect, setSelRect] = useState<{ left: number; top: number; width: number; height: number } | null>(null)


  const handleDownload = useCallback(
    async (format: DrawingFormat) => {
      if (!activeDrawing) return
      setError(null)
      try {
        await downloadDrawing(activeDrawing.id, activeDrawing.name, format)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [activeDrawing],
  )

  // The SVG is server-rendered & deterministic. The kernel's
  // `escape_xml` helper sanitises every text-node string, and we never
  // splice user-supplied markup back into the payload. Safe to inline.
  const svgMarkup = useMemo(() => ({ __html: svg }), [svg])
  // Mark the dimension being edited on the sheet, so it is clear which one
  // the box changes.
  const editingDimId = dimEdit?.dim.id ?? null
  useEffect(() => {
    const root = canvasRef.current
    if (!root || !editingDimId) return
    const marked = Array.from(root.querySelectorAll(`[data-dim-id="${CSS.escape(editingDimId)}"]`))
    marked.forEach((el) => el.setAttribute('data-dim-editing', ''))
    return () => marked.forEach((el) => el.removeAttribute('data-dim-editing'))
  }, [editingDimId, svgMarkup])

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

  useLayoutEffect(() => {
    const root = canvasRef.current
    if (!root || !selectedViewId) {
      setSelRect(null)
      return
    }
    const el = root.querySelector(`[data-view-id="${CSS.escape(selectedViewId)}"]`)
    if (!el) {
      setSelRect(null)
      return
    }
    const r = el.getBoundingClientRect()
    const base = root.getBoundingClientRect()
    const pad = 4
    setSelRect({
      left: r.left - base.left - pad,
      top: r.top - base.top - pad,
      width: r.width + 2 * pad,
      height: r.height + 2 * pad,
    })
  }, [selectedViewId, svgMarkup, pan, displayW, displayH])

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
    // A dimension is clicked to edit it, not dragged.
    if ((e.target as Element).closest('[data-dim-id]')) return
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

  const onSheetClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const el = (e.target as Element).closest('[data-dim-id]')
      if (!el) {
        // A drag pans the sheet; only a click selects.
        const s = panStateRef.current
        if (Math.hypot(e.clientX - s.startX, e.clientY - s.startY) > 4) return
        setSelectedViewId(viewAt(e.clientX, e.clientY))
        return
      }
      if (!activeDrawing) return
      const id = el.getAttribute('data-dim-id') ?? ''
      const view = activeDrawing.views.find((v) => (v.dimensions ?? []).some((d) => d.id === id))
      const dim = view?.dimensions?.find((d) => d.id === id)
      if (!view || !dim) return
      setEditNote(null)
      // Keep the box on screen: 300 x 150 is its largest size.
      const x = Math.min(e.clientX + 12, window.innerWidth - 312)
      const y = Math.min(e.clientY + 12, window.innerHeight - 162)
      setDimEdit({
        dim,
        view: view.name,
        x,
        y,
        draft: String(Number(dim.value.toFixed(4))),
        busy: false,
        startedAt: null,
        driver: null,
        refusal: null,
      })
      // Find what drives it now, while the new value is typed: the edit then
      // reuses the answer and only solves.
      const drawingId = activeDrawing.id
      void getDimensionDriver(drawingId, dim.id)
        .then((driver) =>
          setDimEdit((cur) => (cur && cur.dim.id === dim.id ? { ...cur, driver } : cur)),
        )
        .catch(() =>
          setDimEdit((cur) =>
            cur && cur.dim.id === dim.id
              ? { ...cur, driver: { verdict: 'undecided', reason: 'the server did not answer' } }
              : cur,
          ),
        )
    },
    [activeDrawing, viewAt],
  )

  const onSheetContextMenu = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const id = viewAt(e.clientX, e.clientY)
      if (!id) return
      e.preventDefault()
      openViewMenu(id, e.clientX, e.clientY)
    },
    [viewAt, openViewMenu],
  )

  const applyDimEdit = useCallback(async () => {
    if (!dimEdit || !activeDrawing) return
    const value = Number(dimEdit.draft)
    if (!Number.isFinite(value) || value <= 0) {
      setDimEdit({ ...dimEdit, refusal: 'Enter a positive number.' })
      return
    }
    const abort = new AbortController()
    dimEditAbort.current = abort
    setDimEdit({ ...dimEdit, busy: true, startedAt: Date.now(), refusal: null })
    try {
      const r = await setDrawingDimension(activeDrawing.id, dimEdit.dim.id, value, abort.signal)
      setDimEdit(null)
      setNoteKind('done')
      const fmt = (n: number) => String(Number(n.toFixed(4)))
      const steps = [r.driver.sequence, ...(r.driver.linked ?? []).map((l) => l.target_sequence)]
      const where = steps.length > 1 ? `steps ${steps.join(' + ')}` : `step ${steps[0]}`
      setEditNote(
        `${dimEdit.dim.label} \u2192 ${fmt(value)}: ${r.driver.parameter} ` +
          `${fmt(r.driver.from_value)} \u2192 ${fmt(r.driver.new_value)} (${where})`,
      )
      await refreshActive(activeDrawing.id)
    } catch (e) {
      const stopped = e instanceof DOMException && e.name === 'AbortError'
      const reason = stopped
        ? 'Stopped waiting. The server may still finish this edit; the sheet refreshes when you reopen it.'
        : e instanceof DrawingRefusal || e instanceof Error
          ? e.message
          : String(e)
      setDimEdit((cur) => (cur ? { ...cur, busy: false, startedAt: null, refusal: reason } : cur))
    } finally {
      if (dimEditAbort.current === abort) dimEditAbort.current = null
    }
  }, [dimEdit, activeDrawing, refreshActive])

  const changeProjection = useCallback(
    async (projection: ProjectionAngle) => {
      if (!activeDrawing || (activeDrawing.projection ?? 'third') === projection) return
      setProjectionBusy(true)
      setError(null)
      try {
        await setDrawingProjection(activeDrawing.id, projection)
        await refreshActive(activeDrawing.id)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setProjectionBusy(false)
      }
    },
    [activeDrawing, refreshActive],
  )

  return (
    <div className="relative flex-1 min-h-0 bg-background text-foreground">
      {/* The sheet fills the window; every control is a small overlay on it. */}
      <div
        ref={canvasRef}
        className="absolute inset-0 overflow-hidden bg-muted/20 cursor-grab"
        onMouseDown={onCanvasMouseDown}
      >
        {activeDrawing ? (
          svg ? (
            <div
              className="absolute top-1/2 left-1/2 bg-white shadow-md [&>svg]:block [&>svg]:w-full [&>svg]:h-full [&_[data-dim-id]]:cursor-pointer [&_[data-dim-id]:hover]:fill-blue-600 [&_[data-dim-editing]]:fill-blue-700 [&_[data-dim-editing]]:font-bold"
              onClick={onSheetClick}
              onContextMenu={onSheetContextMenu}
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
              {loading ? 'Rendering…' : ''}
            </div>
          )
        ) : (
          <div className="absolute inset-0 flex items-center justify-center text-sm text-muted-foreground text-center px-6">
            {drawings.length === 0
              ? 'No drawings yet. Open the menu at the top left and start one from a part.'
              : 'Choose a drawing from the menu at the top left.'}
          </div>
        )}
      </div>

      <DrawingSwitcher
        drawings={drawings}
        active={activeDrawing}
        parts={parts}
        onSelect={setActiveId}
        onRename={(id, name) => void commitRename(id, name)}
        onDelete={(id) => void handleDelete(id)}
        onCreateFromPart={(part) => void handleCreateFromPart(part)}
      />
      {activeDrawing && (
        <SheetActions
          projection={activeDrawing.projection ?? 'third'}
          projectionBusy={projectionBusy}
          onProjection={(pr) => void changeProjection(pr)}
          titleBlockOpen={titleBlockOpen}
          onToggleTitleBlock={() => setTitleBlockOpen((o) => !o)}
          onDownload={(f) => void handleDownload(f)}
        />
      )}
      {activeDrawing && titleBlockOpen && (
        <TitleBlockCard
          key={`${activeDrawing.id}:${JSON.stringify(activeDrawing.title_block ?? null)}`}
          drawing={activeDrawing}
          onUpdated={(updated) => {
            setActiveDrawing((prev) => (prev ? { ...prev, title_block: updated } : prev))
            void refreshActive(activeDrawing.id)
          }}
          onError={setError}
          onClose={() => setTitleBlockOpen(false)}
        />
      )}
      {activeDrawing && (
        <ViewsBar
          drawing={activeDrawing}
          busy={viewsBusy}
          onRemove={(id) => void handleRemoveView(id)}
          onAdd={(kind, label) => void handleAddLike(kind, label)}
          onMenu={openViewMenu}
          selectedId={selectedViewId}
        />
      )}
      {selRect && (
        <div
          aria-hidden
          className="pointer-events-none absolute z-10 rounded-sm border border-dashed border-sky-600"
          style={selRect}
        />
      )}
      {activeDrawing && viewMenu && (
        <ViewMenu
          key={`${viewMenu.viewId}:${viewMenu.x}:${viewMenu.y}`}
          drawing={activeDrawing}
          viewId={viewMenu.viewId}
          x={viewMenu.x}
          y={viewMenu.y}
          busy={viewsBusy}
          onClose={closeViewMenu}
          onEdit={(id, edit) => void handleViewEdit(id, edit)}
          onDelete={(id) => {
            setSelectedViewId(null)
            void handleRemoveView(id)
          }}
        />
      )}
      {activeDrawing && svg && <SheetHint />}

      {error && (
        <div
          role="alert"
          className="absolute top-14 left-1/2 -translate-x-1/2 z-30 flex items-baseline gap-1.5 max-w-[min(640px,calc(100%-24px))] rounded-md border border-destructive/30 bg-card px-3 py-2 text-xs shadow-md"
        >
          <span className="inline-flex shrink-0 translate-y-px text-red-600 dark:text-red-400">
            <X size={11} />
          </span>
          <span className="select-text text-red-700 dark:text-red-300">{error}</span>
          <button
            type="button"
            aria-label="Dismiss"
            onClick={() => setError(null)}
            className="cad-focus ml-1 text-muted-foreground hover:text-foreground"
          >
            <X size={12} />
          </button>
        </div>
      )}

      {removedView && (
        <div className="absolute bottom-14 left-3 z-30 flex items-center gap-2 rounded-md border border-border bg-card px-3 py-1.5 text-xs shadow-md">
          <span>Removed view {removedView.name}</span>
          <button
            type="button"
            disabled={viewsBusy}
            onClick={() => void undoRemoveView()}
            className="cad-focus font-medium text-primary hover:underline"
          >
            Undo
          </button>
          <button
            type="button"
            aria-label="Dismiss"
            onClick={() => setRemovedView(null)}
            className="cad-focus text-muted-foreground hover:text-foreground"
          >
            <X size={12} />
          </button>
        </div>
      )}

            {dimEdit && (
              <div
                className="fixed z-50 w-[300px] bg-card border border-border rounded-md shadow-lg p-3 text-xs"
                style={{ left: dimEdit.x, top: dimEdit.y }}
                onMouseDown={(e) => e.stopPropagation()}
              >
                <div className="flex items-start justify-between gap-2 mb-2">
                  <div className="min-w-0">
                    <div className="font-medium text-sm">Change {dimEdit.dim.label}</div>
                    <div className="text-muted-foreground">
                      {DIM_KIND_WORDS[dimEdit.dim.kind] ?? dimEdit.dim.kind.replace(/_/g, ' ')} · {dimEdit.view} view · edits the part
                    </div>
                    <div className="mt-1">{driverLine(dimEdit.driver)}</div>
                  </div>
                  <button
                    type="button"
                    aria-label="Close"
                    onClick={() => {
                      dimEditAbort.current?.abort()
                      setDimEdit(null)
                    }}
                    className="cad-focus shrink-0 text-muted-foreground hover:text-foreground"
                  >
                    <X size={14} />
                  </button>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    autoFocus
                    inputMode="decimal"
                    value={dimEdit.draft}
                    disabled={dimEdit.busy}
                    onChange={(e) => setDimEdit({ ...dimEdit, draft: e.target.value, refusal: null })}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault()
                        void applyDimEdit()
                      } else if (e.key === 'Escape') {
                        e.preventDefault()
                        dimEditAbort.current?.abort()
                        setDimEdit(null)
                      }
                    }}
                    className="cad-focus w-28 min-w-0 font-mono px-2 py-1 rounded border border-border bg-background"
                  />
                  <span className="text-muted-foreground">{dimEdit.dim.unit}</span>
                  {dimEdit.busy ? (
                    <button
                      type="button"
                      onClick={() => dimEditAbort.current?.abort()}
                      className="cad-focus ml-auto shrink-0 px-2 py-1 rounded border border-border hover:bg-accent/20"
                    >
                      Stop
                    </button>
                  ) : (
                    <button
                      type="button"
                      onClick={() => void applyDimEdit()}
                      className="cad-focus ml-auto shrink-0 px-3 py-1 rounded bg-primary text-primary-foreground hover:opacity-90"
                    >
                      Apply
                    </button>
                  )}
                </div>
                {dimEdit.busy && dimEdit.startedAt !== null && (
                  <div className="mt-2 text-muted-foreground">
                    Rebuilding the part and its sheet…{' '}
                    <span className="font-mono">
                      {Math.floor((Date.now() - dimEdit.startedAt) / 1000)} s
                    </span>
                  </div>
                )}
                {dimEdit.refusal && (
                  <div className="mt-2 flex gap-1 text-red-700">
                    <span aria-hidden>✗</span>
                    <span>{dimEdit.refusal}</span>
                  </div>
                )}
              </div>
            )}

            {editNote && (
              <div className="absolute top-14 left-3 z-20 flex items-center gap-2 bg-card border border-border rounded-md shadow-md px-2 py-1 text-xs">
                <span aria-hidden className={noteKind === 'done' ? 'text-green-700' : 'text-sky-700'}>
                  {noteKind === 'done' ? '✓' : 'ⓘ'}
                </span>
                <span className="font-mono">{editNote}</span>
                <button
                  type="button"
                  aria-label="Dismiss"
                  onClick={() => setEditNote(null)}
                  className="cad-focus text-muted-foreground hover:text-foreground"
                >
                  <X size={12} />
                </button>
              </div>
            )}

            {/* Zoom toolbar. Always visible while a drawing is open
                so the user can rescue the view if the sheet renders
                off-screen at native physical size. */}
            {activeDrawing && (
              <div className="absolute bottom-3 right-3 z-20 flex items-center gap-0.5 bg-card border border-border rounded-md shadow-md px-1 py-1 select-none">
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
  )
}

