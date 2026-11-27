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

import { ISO_SHEET_SIZES, scaleLabel, sheetRefusal, sheetRequest, type SheetChoice, type SheetRefusal } from '@/lib/sheet-size'
import { sheetPointMm } from '@/lib/sheet-cursor'
import { useSheetCursorStore } from '@/stores/sheet-cursor-store'
import { SheetPanel } from './SheetPanel'
import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { X } from '@/components/ui/icons'
import {
  DrawingSwitcher,
  SheetActions,
  SheetHint,
  SheetNotices,
  TitleBlockCard,
  ViewMenu,
  type PartRow,
} from '@/components/panels/drawing-overlays'
import {
  type Drawing,
  type DrawingFormat,
  type ProjectionAngle,
  type SheetDimension,
  type DimensionDriverResolution,
  type DrawingQualityReport,
  type DimensionListing,
  annotateDimension,
  listDimensions,
  getDimensionDriver,
  DrawingRefusal,
  setDrawingDimension,
  setDrawingProjection,
  setDrawingSheet,
  SheetChangeRefused,
  sheetDimensionsMm,
  addView,
  createSolidDrawing,
  listDocumentParts,
  deleteDrawing,
  downloadDrawing,
  previewDrawingSvg,
  getDrawing,
  getDrawingQuality,
  listDrawings,
  removeView,
  renameDrawing,
  updateView,
  type ViewEdit,
} from '@/lib/drawings-api'
import { useDocumentStore } from '@/stores/document-store'
import { CompletenessChip, DimensionPanel } from '@/components/panels/DimensionPanel'
import { draggedShift } from '@/lib/dimension-annotate'
import { latestOnly } from '@/lib/latest-only'
import { sheetPress } from '@/lib/sheet-press'
import { releaseWasMissed } from '@/lib/sheet-drag-release'
import { sheetRequests } from '@/lib/sheet-requests'
import { applySavedTitleBlock } from '@/lib/title-block-draft'
import { drawingsForDocument } from '@/lib/drawings-for-document'
import { currentDocument } from '@/lib/document-scope'
import { HISTORY_CHANGED_EVENT, timelineAction } from '@/lib/history'
import { useDocModeStore } from '@/stores/doc-mode-store'
import {
  describeMove,
  draggedPosition,
  isDrag,
  newLayoutIssues,
  planViewDrag,
  screenPxPerMm,
  type SheetQuality,
} from '@/lib/drawing-drag'
import { fitSheetScale } from '@/lib/sheet-fit'
import { lookGlide, prefersReducedMotion, PX_PER_MM, sheetLookTarget, type SheetRect } from '@/lib/sheet-look'
import { shouldPlot } from '@/lib/sheet-plot'
import { titleBlockVerified } from '@/lib/title-block-verified'
import { plotSheet } from '@/lib/sheet-plot-dom'

/** The sheets already plotted in this session (lib/sheet-plot.ts). */
const plottedSheets = new Set<string>()
import {
  actRunning,
  flashOpacity,
  ghostOpacity,
  GHOST_OPACITY,
  judgeFlashes,
  judgeRunning,
  movedBoxAt,
} from '@/lib/sheet-hands'
import { useAgentShowStore } from '@/stores/agent-show-store'
import { withScreenInk } from '@/lib/sheet-screen-ink'
import { exportRefusalText } from '@/lib/export-refusal'

// CSS pixels per millimetre at the standard 96 DPI mapping (PX_PER_MM, from
// lib/sheet-look, the one number the agent's look aims with too). At zoom
// `1.0` the sheet renders at its true physical size (`sheetDimensionsMm`
// gives the paper's width x height, orientation applied).

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
  fillet: 'fillet radius',
  chamfer: 'chamfer',
}

/**
 * The Drawing workspace of the document that is open.
 *
 * The workspace stays mounted for the life of the page (parked under the
 * other workspaces), so anything it holds would outlive the document it was
 * about: the title block card and the "moved" note of one document were
 * still on screen in a freshly created one (2026-11-26). It is therefore ONE
 * COMPONENT PER DOCUMENT -- keyed by the document store's `epoch`, which
 * moves when the server confirms a switch. A new document starts a new
 * component: its sheets, its selection, its open cards, its notes, its zoom,
 * and every request still in flight for the previous document (whose answer
 * now lands on nothing).
 */
export function DrawingsWorkspace() {
  const documentEpoch = useDocumentStore((st) => st.epoch)
  return <DocumentDrawings key={documentEpoch} />
}

function DocumentDrawings() {
  const [drawings, setDrawings] = useState<DrawingSummary[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [activeDrawing, setActiveDrawing] = useState<Drawing | null>(null)
  const [svg, setSvg] = useState<string>('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // ── Overlay state ────────────────────────────────────────────────
  const [titleBlockOpen, setTitleBlockOpen] = useState(false)
  const [viewsBusy, setViewsBusy] = useState(false)
  // The last removed view, so its removal can be undone. The offer's Undo is
  // a history undo: it takes back the NEWEST step, so it stands only while
  // the removal is that step. Every other recorded edit of the sheet -- and
  // any undo or redo, and an agent's turn -- withdraws it (`setRemovedView(
  // null)` at each); otherwise "Removed view TOP -- Undo" took back the
  // dimension moved since, and said the view was restored.
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
  // Workspaces stay mounted when parked: a part imported in the Part
  // workspace must be offered here when this workspace is opened again.
  const docMode = useDocModeStore((s) => s.mode)
  useEffect(() => {
    if (docMode === 'drawing') void refreshParts()
  }, [docMode, refreshParts])

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
  // The Sheet control: laying out on other paper, and the kernel's refusal.
  const [sheetBusy, setSheetBusy] = useState(false)
  const [sheetRefused, setSheetRefused] = useState<SheetRefusal | null>(null)

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
  // Listings overlap (this panel's mount and a document switch); only the
  // newest may apply, or an older listing made while the previous document
  // was still open lands last and reopens that document's drawing.
  const listRequests = useRef(latestOnly())
  const refreshList = useCallback(async (preferId?: string) => {
    const ticket = listRequests.current.begin()
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
      // The open document is the app's own record of the confirmed switch
      // (document-scope), not the "active" flag in a list that may not yet
      // hold a document just created.
      const docs = useDocumentStore.getState()
      if (docs.documents.length === 0 && currentDocument() === null) await docs.refresh()
      const openDoc = currentDocument() ?? useDocumentStore.getState().documents.find((d) => d.active)?.id ?? null
      const entries = drawingsForDocument(await listDrawings(), openDoc)
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
      if (!listRequests.current.isLatest(ticket)) return
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

  // The kernel's reading of the sheet's dimensions (as inked, what is
  // authored on each) and its completeness: what the property panel and the
  // completeness chip show. Reloaded with the sheet; `null` while unknown.
  const [dimListing, setDimListing] = useState<DimensionListing | null>(null)
  // Reads of the sheet overlap (an edit's re-read, an agent turn ending,
  // another sheet chosen): only the newest read of the sheet ON SCREEN may
  // apply, or a slow answer puts the previous sheet -- or its certificate --
  // on the one now open (lib/sheet-requests).
  const sheetReads = useRef(sheetRequests())
  const refreshActive = useCallback(async (id: string) => {
    const ticket = sheetReads.current.begin(id)
    if (ticket === null) return
    setLoading(true)
    setError(null)
    try {
      // The on-screen sheet is a draft for review: shown even when its layout
      // check fails (SheetPanel lists the findings); downloads stay refused.
      const [d, s] = await Promise.all([getDrawing(id), previewDrawingSvg(id)])
      if (!sheetReads.current.isCurrent(ticket)) return
      setActiveDrawing(d)
      setSvg(s)
      // Not fatal to the sheet: without it the panel shows the measured
      // label and the chip says "not verified".
      void listDimensions(id)
        .then((listing) => {
          if (sheetReads.current.isCurrent(ticket)) setDimListing(listing)
        })
        .catch(() => {
          if (sheetReads.current.isCurrent(ticket)) setDimListing(null)
        })
    } catch (e) {
      if (!sheetReads.current.isCurrent(ticket)) return
      setError(e instanceof Error ? e.message : String(e))
      setActiveDrawing(null)
      setSvg('')
    } finally {
      // A newer read owns the busy state from here.
      if (sheetReads.current.isCurrent(ticket)) setLoading(false)
    }
  }, [])
  /** Say a note about one sheet -- only while that sheet is on screen. An
   *  edit answers after a wait; its note must not appear on another sheet. */
  const noteFor = useCallback((drawingId: string, kind: 'done' | 'info', text: string) => {
    if (!sheetReads.current.isShowing(drawingId)) return
    setNoteKind(kind)
    setEditNote(text)
  }, [])

  // Undo / redo rewinds the history; the server rebuilds the sheets from it.
  // Reload what this window shows.
  useEffect(() => {
    const onHistory = () => {
      setRemovedView(null)
      void refreshList(activeId ?? undefined)
      if (activeId) void refreshActive(activeId)
    }
    window.addEventListener(HISTORY_CHANGED_EVENT, onHistory)
    return () => window.removeEventListener(HISTORY_CHANGED_EVENT, onHistory)
  }, [activeId, refreshList, refreshActive])

  // An agent turn ended: the sheet on screen may have been opened before the
  // agent finished editing it (the bracket's H7 was set after the sheet
  // appeared and never reached the screen, 2026-11-19). Re-read it.
  useEffect(() => {
    const onTurnEnded = () => {
      setRemovedView(null)
      if (activeId) void refreshActive(activeId)
    }
    window.addEventListener('roshera:agent-turn-ended', onTurnEnded)
    return () => window.removeEventListener('roshera:agent-turn-ended', onTurnEnded)
  }, [activeId, refreshActive])

  useEffect(() => {
    // A document switch starts this component afresh (see DrawingsWorkspace),
    // so this also lists the new document's sheets and parts.
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

  // A drawing opened while this workspace is already mounted (parked under
  // another workspace) -- the agent made one, or "Create Drawing" ran from
  // the viewport: load the list landing on it. The mount effect above only
  // covers the first visit; the agent's wing-rib drawing was left unshown
  // ("No drawings yet") when the workspace had been opened before
  // (2026-11-19).
  const pendingDrawingId = useDocModeStore((s) => s.pendingDrawingId)
  useEffect(() => {
    if (pendingDrawingId === null) return
    const pending = useDocModeStore.getState().consumePendingDrawing()
    if (pending) void refreshList(pending)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingDrawingId])

  useEffect(() => {
    // From here every read in flight is for the sheet that was open before.
    sheetReads.current.show(activeId)
    if (activeId) {
      void refreshActive(activeId)
    } else {
      setActiveDrawing(null)
      setSvg('')
      setLoading(false)
    }
    // Switching drawings resets the view so the new sheet is centred
    // in fit mode without inheriting the previous drawing's pan -- or its
    // zoom: an A4 detail zoomed to 400% used to open the next sheet (an A1)
    // at 400% too, showing a blank corner of it.
    setZoomMode('fit')
    setPan({ x: 0, y: 0 })
  }, [activeId, refreshActive])

  // Coming back to this workspace: the part may have been edited while it
  // was parked, and the sheet and certificate on screen were read before
  // that. Re-read them, so the chip never vouches for a part that has since
  // changed.
  useEffect(() => {
    if (docMode === 'drawing' && activeId) void refreshActive(activeId)
    // Only entering the workspace triggers this; a change of sheet loads itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docMode])

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
        setRemovedView(null)
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

  // The one place a drawing is deleted, so the one place that asks: whatever
  // offers Delete (the switcher today) only passes the request on. It used
  // to ask too, and the engineer answered the same question twice.
  const handleDelete = useCallback(
    async (id: string) => {
      const name = drawings.find((d) => d.id === id)?.name
      if (!confirm(name ? `Delete the drawing "${name}"?` : 'Delete this drawing?')) return
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
    [drawings, activeId, refreshList],
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
          noteFor(
            activeId,
            'info',
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
    [activeId, activeDrawing, refreshActive, noteFor],
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
        // Its row in the Sheet panel is gone without the pointer leaving it.
        setHoverViewId((cur) => (cur === viewId ? null : cur))
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
  // The kernel's sheet check for the open sheet, re-run whenever the sheet
  // changes (a view moved, a dimension edited the part, a new projection).
  const [sheetCheck, setSheetCheck] = useState<DrawingQualityReport | null>(null)
  useEffect(() => {
    if (!activeDrawing) return
    let live = true
    getDrawingQuality(activeDrawing.id)
      .then((q) => {
        if (live) setSheetCheck(q)
      })
      .catch(() => {
        if (live) setSheetCheck(null)
      })
    return () => {
      live = false
    }
  }, [activeDrawing])
  const [viewMenu, setViewMenu] = useState<{ viewId: string; x: number; y: number } | null>(null)
  const closeViewMenu = useCallback(() => setViewMenu(null), [])
  const openViewMenu = useCallback((viewId: string, x: number, y: number) => {
    setSelectedViewId(viewId)
    setViewMenu({ viewId, x, y })
  }, [])
  // Another sheet starts clean: everything the engineer opened, was told or
  // had in hand belonged to the sheet that was on screen. Left in place, the
  // "moved" note and the title block card of one sheet stayed over the next,
  // the removed-view Undo would undo a step of another sheet, the dimension
  // editor would send one sheet's dimension to another, and the chip showed
  // the previous sheet's certificate until the new one answered.
  useEffect(() => {
    setSelectedViewId(null)
    setViewMenu(null)
    setEditNote(null)
    setRemovedView(null)
    dimEditAbort.current?.abort()
    setDimEdit(null)
    setTitleBlockOpen(false)
    setDimListing(null)
    setSheetCheck(null)
    setViewGhost(null)
    setHoverViewId(null)
    dimDragRef.current = null
    viewDragRef.current = null
    panStateRef.current.dragging = false
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
        const { moved, aligned, position } = await updateView(activeId, viewId, edit)
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
                      ? aligned === false
                        ? '; moved to the nearest free space (no room in line with FRONT)'
                        : aligned
                          ? '; moved to fit, still in line with FRONT'
                          : '; moved to fit'
                      : ''
                  }`
                : edit.position_mm !== undefined
                  ? describeMove(v.name, v.position_mm, position ?? edit.position_mm)
                  : `${v.name} renamed ${edit.name ?? ''}`
        noteFor(activeId, moved && aligned === false ? 'info' : 'done', `${what}. Ctrl+Z undoes it.`)
      } catch (e) {
        if (e instanceof DrawingRefusal) noteFor(activeId, 'info', `Not changed: ${e.message}`)
        else setError(e instanceof Error ? e.message : String(e))
      } finally {
        setViewsBusy(false)
      }
    },
    [activeId, activeDrawing, refreshActive, noteFor],
  )

  // Delete removes the selected view; Escape lets go of it. Not while the
  // engineer types, and not while a dimension's change box is open. And only
  // while this workspace is the one open: it stays mounted when parked, and
  // a key listener on the window hears every key -- Delete pressed on a part
  // in the Part workspace also removed the view left selected here.
  useEffect(() => {
    if (docMode !== 'drawing') return
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
  }, [docMode, selectedViewId, dimEdit, handleRemoveView])

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
        // In plain words: the server's refusals are written for agents.
        setError(exportRefusalText(e))
      }
    },
    [activeDrawing],
  )

  // The SVG is server-rendered & deterministic. The kernel's
  // `escape_xml` helper sanitises every text-node string, and we never
  // splice user-supplied markup back into the payload. Safe to inline.
  // Screen line weights: visible vs hidden readable at any zoom (paper keeps ISO).
  const svgMarkup = useMemo(() => ({ __html: withScreenInk(svg) }), [svg])
  // A view pointed at in the Sheet panel, marked on the sheet.
  const [hoverViewId, setHoverViewId] = useState<string | null>(null)
  const [hoverRect, setHoverRect] = useState<{ left: number; top: number; width: number; height: number } | null>(null)

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
      activeDrawing
        ? sheetDimensionsMm(activeDrawing.sheet_size, activeDrawing.orientation)
        : [297, 210],
    [activeDrawing],
  )
  const sheetWPx = sheetWMm * PX_PER_MM
  const sheetHPx = sheetHMm * PX_PER_MM

  // Clear of the toolbar band (top) and the zoom/verdict band (bottom).
  const fitScale = useMemo(
    () => fitSheetScale(containerSize.w, containerSize.h, sheetWPx, sheetHPx),
    [containerSize, sheetWPx, sheetHPx],
  )

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
    // Measured from the workspace, where the box is drawn -- the sheet area
    // starts right of the Sheet panel, so its own edge is the wrong origin.
    const base = (root.parentElement ?? root).getBoundingClientRect()
    const pad = 4
    setSelRect({
      left: r.left - base.left - pad,
      top: r.top - base.top - pad,
      width: r.width + 2 * pad,
      height: r.height + 2 * pad,
    })
  }, [selectedViewId, svgMarkup, pan, displayW, displayH, containerSize])

  // The view pointed at in the Sheet panel: a light box on the sheet.
  useLayoutEffect(() => {
    const root = canvasRef.current
    const el = root && hoverViewId ? root.querySelector(`[data-view-id="${CSS.escape(hoverViewId)}"]`) : null
    if (!root || !el || hoverViewId === selectedViewId) {
      setHoverRect(null)
      return
    }
    const r = el.getBoundingClientRect()
    const base = (root.parentElement ?? root).getBoundingClientRect()
    setHoverRect({ left: r.left - base.left - 4, top: r.top - base.top - 4, width: r.width + 8, height: r.height + 8 })
  }, [hoverViewId, selectedViewId, svgMarkup, pan, displayW, displayH, containerSize])

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

  // ── The agent's look (agent eyes, slice 2) ────────────────────────
  // An AgentShow `sheet_space` frame (the server sends one when the agent
  // looks at a space) lands in the show store; once its drawing is the one
  // on screen, the sheet glides to the space's box and outlines it, and the
  // show bar carries the caption. Instant under reduced motion. The user's
  // own wheel or drag stops the glide; Take over clears the outline and
  // leaves the sheet where it is.
  const sheetLook = useAgentShowStore((s) => s.sheetLook)
  const lookShown = sheetLook !== null && activeDrawing?.id === sheetLook.drawingId ? sheetLook : null
  const lookFrame = useRef<number | null>(null)
  const lookDone = useRef<number>(0)
  const scaleRef = useRef(effectiveScale)
  useEffect(() => {
    scaleRef.current = effectiveScale
  }, [effectiveScale])
  const cancelLook = useCallback(() => {
    if (lookFrame.current !== null) cancelAnimationFrame(lookFrame.current)
    lookFrame.current = null
  }, [])
  useEffect(() => cancelLook, [cancelLook])
  // Take over (or the bar's x) clears the look: a glide in flight stops
  // where it is, and the sheet stays there.
  useEffect(() => {
    if (sheetLook === null) cancelLook()
  }, [sheetLook, cancelLook])
  useEffect(() => {
    if (!lookShown || !svg || containerSize.w <= 0 || containerSize.h <= 0) return
    if (lookDone.current === lookShown.seq) return
    lookDone.current = lookShown.seq
    cancelLook()
    const to = sheetLookTarget(containerSize, { w: sheetWMm, h: sheetHMm }, lookShown.rect)
    const from = { zoom: scaleRef.current, pan: panStateRef.current.pan }
    const glide = lookGlide(from, to, prefersReducedMotion())
    const land = () => {
      lookFrame.current = null
      if (lookShown.rect) {
        setZoomMode('manual')
        setZoom(to.zoom)
        setPan(to.pan)
      } else {
        setZoomMode('fit')
        setPan({ x: 0, y: 0 })
      }
    }
    if (glide.durationMs === 0) return land()
    setZoomMode('manual')
    const start = performance.now()
    const step = (now: number) => {
      const t = now - start
      if (t >= glide.durationMs) return land()
      const p = glide.at(t)
      setZoom(p.zoom)
      setPan(p.pan)
      lookFrame.current = requestAnimationFrame(step)
    }
    lookFrame.current = requestAnimationFrame(step)
  }, [lookShown, svg, containerSize, sheetWMm, sheetHMm, cancelLook])

  // ── The agent's hands (agent eyes, slice 3) ──────────────────────
  // ACT: a layout verb's `sheet_act` frame -- the sheet is re-read (the verb
  // already changed it) and each moved thing's box glides from its old
  // place to its new one over the fresh sheet, a faint ghost fading at the
  // old place. JUDGE: the `sheet_judge` frame that follows -- each touched
  // space flashes its score change, green better, red worse, worst first.
  // Reduced motion: no glide, no fade, no stagger -- the same boxes and
  // captions at once. Take over clears both (the store), leaving the sheet.
  const sheetAct = useAgentShowStore((s) => s.sheetAct)
  const sheetJudge = useAgentShowStore((s) => s.sheetJudge)
  const actShown = sheetAct !== null && activeDrawing?.id === sheetAct.drawingId ? sheetAct : null
  const judgeShown = sheetJudge !== null && activeDrawing?.id === sheetJudge.drawingId ? sheetJudge : null
  const [actT, setActT] = useState(0)
  const [judgeT, setJudgeT] = useState(0)
  const handsReduced = prefersReducedMotion()
  const actSeq = actShown?.seq ?? null
  const actDrawing = actShown?.drawingId ?? null
  // The verb changed the sheet: show the new one under the animation.
  useEffect(() => {
    if (actSeq !== null && actDrawing !== null) void refreshActive(actDrawing)
  }, [actSeq, actDrawing, refreshActive])
  useEffect(() => {
    if (actSeq === null) return
    let frame = 0
    const start = performance.now()
    const tick = (now: number) => {
      const t = now - start
      setActT(t)
      if (actRunning(t)) frame = requestAnimationFrame(tick)
    }
    frame = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(frame)
  }, [actSeq])
  const flashes = useMemo(
    () => (judgeShown ? judgeFlashes(judgeShown.spaces, handsReduced, judgeShown.afterMs) : []),
    [judgeShown, handsReduced],
  )
  const judgeSeq = judgeShown?.seq ?? null
  useEffect(() => {
    if (judgeSeq === null) return
    let frame = 0
    const start = performance.now()
    const tick = (now: number) => {
      const t = now - start
      setJudgeT(t)
      if (judgeRunning(flashes, t)) frame = requestAnimationFrame(tick)
    }
    frame = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(frame)
  }, [judgeSeq, flashes])
  const pct = (r: SheetRect) => ({
    left: `${(r.x0 / sheetWMm) * 100}%`,
    top: `${(r.y0 / sheetHMm) * 100}%`,
    width: `${(Math.max(r.x1 - r.x0, 0.5) / sheetWMm) * 100}%`,
    height: `${(Math.max(r.y1 - r.y0, 0.5) / sheetHMm) * 100}%`,
  })

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
      // The user's own zoom stops the agent's glide.
      cancelLook()
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
  }, [zoomMode, zoom, fitScale, sheetWPx, sheetHPx, cancelLook])

  // ── Drag a selected view to move it ──────────────────────────────
  // Press on the SELECTED view and drag: a ghost outline follows the cursor
  // and the drop sends ONE position edit (`PATCH .../views/{id}`), recorded
  // in the history so Ctrl+Z puts the view back. The kernel applies or
  // refuses it; a refusal leaves the view where it was and says why. After
  // the move the kernel's sheet check is asked what the move broke (an
  // overlap, past the frame, out of line with FRONT) and that is reported.
  // Pressing anywhere else still pans the sheet.
  const sheetRef = useRef<HTMLDivElement | null>(null)

  // A sheet is PLOTTED the first time it is shown: border and title block,
  // each view's visible edges, hidden lines, dimensions, then the lettering
  // (lib/sheet-plot.ts). Once per sheet per session -- the effect depends on
  // which sheet is open and whether it has markup, never on the markup
  // itself, so an edit, a re-render, a pan or a zoom cannot replay it. In a
  // layout effect, so the first paint is already the empty sheet rather than
  // the finished one flashing before the pen starts. Any key or click
  // finishes it; so does leaving the sheet (the cleanup).
  const plotId = activeDrawing?.id ?? null
  const plotReady = svg !== '' && docMode === 'drawing'
  useLayoutEffect(() => {
    if (!plotReady || !shouldPlot(plottedSheets, plotId, prefersReducedMotion())) return
    const sheetSvg = sheetRef.current?.querySelector('svg')
    if (!sheetSvg || plotId === null) return
    plottedSheets.add(plotId)
    return plotSheet(sheetSvg)
  }, [plotId, plotReady])

  const viewDragRef = useRef<{
    drawingId: string
    viewId: string
    startX: number
    startY: number
    /** The view's `position_mm` when the drag began. */
    start: [number, number]
    pxPerMm: number
    /** The selection outline when the drag began (canvas px). */
    base: { left: number; top: number; width: number; height: number }
    /** The sheet check before the move, asked once the press became a drag. */
    before: Promise<SheetQuality | null> | null
  } | null>(null)
  const [viewGhost, setViewGhost] = useState<{
    left: number
    top: number
    width: number
    height: number
    /** Offset from where the view sits, in sheet mm (y up the sheet). */
    dxMm: number
    dyMm: number
  } | null>(null)

  const commitViewMove = useCallback(
    async (
      drawingId: string,
      viewId: string,
      from: [number, number],
      edit: { position_mm: [number, number] },
      before: Promise<SheetQuality | null> | null,
    ) => {
      const v = activeDrawing?.views.find((x) => x.id === viewId)
      if (!v || activeDrawing?.id !== drawingId) return
      setError(null)
      setViewsBusy(true)
      try {
        // The before-check must answer before the move is sent, or it could
        // read the moved sheet and hide what the move broke.
        const had = before ? await before : null
        const result = await updateView(drawingId, viewId, edit)
        setRemovedView(null)
        await refreshActive(drawingId)
        // Where the kernel actually put it: a projected view keeps to its
        // line from FRONT, and FRONT carries the views in line with it.
        const carried = result.movedViews.slice(1).map((m) => m.name)
        const moved =
          describeMove(v.name, from, result.position ?? edit.position_mm) +
          (result.projected ? ' (kept in line with FRONT)' : '') +
          (carried.length ? `, carrying ${carried.join(', ')}` : '')
        const now = await getDrawingQuality(drawingId).catch(() => null)
        if (!now) {
          noteFor(drawingId, 'info', `${moved}; the sheet check did not answer. Ctrl+Z undoes the move.`)
          return
        }
        const problems = newLayoutIssues(had, now)
        if (problems.length > 0) {
          noteFor(
            drawingId,
            'info',
            `${moved}, but the sheet check now finds: ${problems.join('; ')}. Ctrl+Z undoes the move.`,
          )
        } else {
          noteFor(drawingId, 'done', `${moved}. Ctrl+Z undoes it.`)
        }
      } catch (e) {
        // The view did not move; the sheet still shows it where it was.
        setError(
          e instanceof DrawingRefusal
            ? `${v.name} not moved: ${e.message}`
            : e instanceof Error
              ? e.message
              : String(e),
        )
      } finally {
        setViewsBusy(false)
      }
    },
    [activeDrawing, refreshActive, noteFor],
  )
  // ── Dragging a dimension ─────────────────────────────────────────
  // A press on a dimension that travels is a move: its text follows the
  // cursor, and the drop sends ONE annotation (the shift from where the
  // layout puts it, added to any shift it already had). The kernel redraws
  // the dimension line and extension lines; an overlap the move causes is
  // kept and shown by the sheet check, never refused.
  const dimDragRef = useRef<{
    id: string
    startX: number
    startY: number
    pxPerMm: number
    els: SVGElement[]
    moved: boolean
  } | null>(null)
  // The press in hand: whether the click that ends it is a drag's drop is
  // decided per press, never carried to the next one (lib/sheet-press).
  const press = useRef(sheetPress())
  const commitDimMove = useCallback(
    async (id: string, dxPx: number, dyPx: number, pxPerMm: number, els: SVGElement[]) => {
      if (!activeDrawing) return
      const drawingId = activeDrawing.id
      const known = dimListing?.dimensions.find((d) => d.id === id)
      const label = known?.ink ?? 'Dimension'
      try {
        await annotateDimension(drawingId, id, {
          text_shift_mm: draggedShift(known?.edit?.text_shift_mm, dxPx, dyPx, pxPerMm),
        })
        setRemovedView(null)
        noteFor(drawingId, 'done', `${label} moved. Ctrl+Z undoes it.`)
        await refreshActive(drawingId)
      } catch (e) {
        els.forEach((el) => {
          el.style.translate = ''
        })
        noteFor(drawingId, 'info', `Not moved: ${e instanceof Error ? e.message : String(e)}`)
      }
    },
    [activeDrawing, dimListing, refreshActive, noteFor],
  )
  const commitDimMoveRef = useRef(commitDimMove)
  useEffect(() => {
    commitDimMoveRef.current = commitDimMove
  }, [commitDimMove])

  // The window-level mouse-up listener is bound once; it reaches the
  // current commit through this ref.
  const commitViewMoveRef = useRef(commitViewMove)
  useEffect(() => {
    commitViewMoveRef.current = commitViewMove
  }, [commitViewMove])

  /** Start dragging the selected view when the press lands on it. */
  const beginViewDrag = useCallback(
    (clientX: number, clientY: number): boolean => {
      if (!selectedViewId || !activeDrawing || !selRect || viewsBusy || dimEdit) return false
      if (viewAt(clientX, clientY) !== selectedViewId) return false
      const view = activeDrawing.views.find((v) => v.id === selectedViewId)
      const sheet = sheetRef.current
      const pxPerMm = sheet ? screenPxPerMm(sheet.getBoundingClientRect().width, sheetWMm) : null
      if (!view || pxPerMm === null) return false
      viewDragRef.current = {
        drawingId: activeDrawing.id,
        viewId: view.id,
        startX: clientX,
        startY: clientY,
        start: [view.position_mm[0], view.position_mm[1]],
        pxPerMm,
        base: selRect,
        before: null,
      }
      return true
    },
    [selectedViewId, activeDrawing, selRect, viewsBusy, dimEdit, viewAt, sheetWMm],
  )

  // Over the selected view the cursor says it can be moved.
  const onCanvasHover = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const el = canvasRef.current
      if (!el || panStateRef.current.dragging || viewDragRef.current) return
      const over = !!selectedViewId && !viewsBusy && viewAt(e.clientX, e.clientY) === selectedViewId
      el.style.cursor = over ? 'move' : ''
      // Where the cursor is on the paper, for the status bar. The sheet's
      // millimetres are its SVG's own viewBox.
      const sheet = sheetRef.current
      const box = sheet?.querySelector('svg')?.viewBox.baseVal
      useSheetCursorStore
        .getState()
        .setPoint(
          sheet && box && box.width > 0
            ? sheetPointMm({ x: e.clientX, y: e.clientY }, sheet.getBoundingClientRect(), {
                widthMm: box.width,
                heightMm: box.height,
              })
            : null,
        )
    },
    [selectedViewId, viewsBusy, viewAt],
  )

  // ── Drag-to-pan ──────────────────────────────────────────────────
  // Left-click drag updates the `pan` translation directly. Because
  // the sheet is positioned with CSS `transform: translate(...)` and
  // its parent is `overflow-hidden`, the user can drag it anywhere —
  // there are no scroll boundaries to clip against.
  const onCanvasMouseDown = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    // A new press: nothing the last one did (a drag whose drop never reached
    // the sheet as a click) decides what this one's click means.
    press.current.begin()
    // Ignore clicks on the zoom toolbar so the buttons still work.
    if ((e.target as HTMLElement).closest('button')) return
    // The user's own press stops the agent's glide.
    cancelLook()
    // A dimension is clicked to open its properties, or dragged to move it.
    const dimEl = (e.target as Element).closest('[data-dim-id]')
    if (dimEl) {
      const sheet = sheetRef.current
      const id = (dimEl.getAttribute('data-dim-id') ?? '').replace(/-note$/, '')
      const pxPerMm = sheet ? screenPxPerMm(sheet.getBoundingClientRect().width, sheetWMm) : null
      if (e.button === 0 && sheet && pxPerMm && id) {
        dimDragRef.current = {
          id,
          startX: e.clientX,
          startY: e.clientY,
          pxPerMm,
          // Every piece of the dimension's text (value, stacked deviations,
          // a callout's second line).
          els: Array.from(
            sheet.querySelectorAll<SVGElement>(
              `[data-dim-id="${CSS.escape(id)}"], [data-dim-id="${CSS.escape(`${id}-note`)}"]`,
            ),
          ),
          moved: false,
        }
        // No text selection while dragging.
        e.preventDefault()
      }
      return
    }
    if (e.button !== 0) return
    const el = canvasRef.current
    if (!el) return
    // The click that ends this press must not reselect: record where it began.
    panStateRef.current.startX = e.clientX
    panStateRef.current.startY = e.clientY
    if (beginViewDrag(e.clientX, e.clientY)) {
      el.style.cursor = 'move'
      e.preventDefault()
      return
    }
    panStateRef.current.dragging = true
    panStateRef.current.startX = e.clientX
    panStateRef.current.startY = e.clientY
    panStateRef.current.startPan = { ...panStateRef.current.pan }
    el.style.cursor = 'grabbing'
    e.preventDefault()
  }, [beginViewDrag, sheetWMm, cancelLook])

  useEffect(() => {
    const dragInHand = () =>
      dimDragRef.current !== null || viewDragRef.current !== null || panStateRef.current.dragging
    // Let go of whatever is in hand and send nothing: the dimension's text
    // goes back, the view's ghost goes, the sheet stops following. Every way
    // a drag ends WITHOUT its own mouse-up comes here -- Escape, a release
    // the page never saw, the window losing focus, a new press.
    const abandonDrags = () => {
      const dd = dimDragRef.current
      if (dd) {
        dd.els.forEach((el) => {
          el.style.translate = ''
        })
        dimDragRef.current = null
      }
      if (viewDragRef.current) {
        viewDragRef.current = null
        setViewGhost(null)
      }
      panStateRef.current.dragging = false
      const el = canvasRef.current
      if (el) el.style.cursor = ''
    }
    const onMouseMove = (e: MouseEvent) => {
      // The button is up but no mouse-up reached the page (Alt+Tab mid-drag,
      // a system dialog): the drag is over, and where it would have been
      // dropped is not known -- so nothing is sent (lib/sheet-drag-release).
      if (dragInHand() && releaseWasMissed(e)) {
        abandonDrags()
        return
      }
      const dd = dimDragRef.current
      if (dd) {
        const dx = e.clientX - dd.startX
        const dy = e.clientY - dd.startY
        if (!dd.moved && !isDrag(dx, dy)) return
        dd.moved = true
        press.current.dragged()
        // The SVG's user unit is the sheet millimetre. `translate` (not
        // `transform`) so a vertical dimension keeps its rotation.
        const t = `${dx / dd.pxPerMm}px ${dy / dd.pxPerMm}px`
        dd.els.forEach((el) => {
          el.style.translate = t
        })
        return
      }
      const d = viewDragRef.current
      if (d) {
        const dx = e.clientX - d.startX
        const dy = e.clientY - d.startY
        if (!isDrag(dx, dy)) {
          setViewGhost(null)
          return
        }
        // Ask the kernel's sheet check now, so the drop can tell what the
        // move itself changed.
        if (!d.before) d.before = getDrawingQuality(d.drawingId).catch(() => null)
        const p = draggedPosition(d.start, dx, dy, d.pxPerMm)
        setViewGhost({
          left: d.base.left + dx,
          top: d.base.top + dy,
          width: d.base.width,
          height: d.base.height,
          dxMm: Number((p[0] - d.start[0]).toFixed(2)),
          dyMm: Number((p[1] - d.start[1]).toFixed(2)),
        })
        return
      }
      const s = panStateRef.current
      if (!s.dragging) return
      const dx = e.clientX - s.startX
      const dy = e.clientY - s.startY
      setPan({ x: s.startPan.x + dx, y: s.startPan.y + dy })
    }
    const onMouseUp = (e: MouseEvent) => {
      const el = canvasRef.current
      if (el) el.style.cursor = ''
      panStateRef.current.dragging = false
      const dd = dimDragRef.current
      if (dd) {
        dimDragRef.current = null
        if (dd.moved) {
          void commitDimMoveRef.current(dd.id, e.clientX - dd.startX, e.clientY - dd.startY, dd.pxPerMm, dd.els)
        }
        return
      }
      const d = viewDragRef.current
      if (!d) return
      viewDragRef.current = null
      const edit = planViewDrag(d.start, e.clientX - d.startX, e.clientY - d.startY, d.pxPerMm)
      if (!edit) {
        setViewGhost(null)
        return
      }
      // The ghost stays where it was dropped until the kernel answers: on
      // success the view is redrawn under it, on a refusal it snaps back.
      void commitViewMoveRef.current(d.drawingId, d.viewId, d.start, edit, d.before).finally(() =>
        setViewGhost(null),
      )
    }
    // Escape while dragging a view or a dimension drops the drag; nothing
    // is sent.
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && (dimDragRef.current || viewDragRef.current)) abandonDrags()
    }
    // Any press, anywhere, begins with nothing in hand: a drag whose release
    // was missed must not ride along with the next press. In the capture
    // phase, so it runs before the sheet's own mouse-down starts a new drag
    // and whatever a panel does with the event.
    const onPressBegins = () => abandonDrags()
    // The window lost focus mid-drag: the release will happen elsewhere.
    const onWindowBlur = () => abandonDrags()
    window.addEventListener('mousemove', onMouseMove)
    window.addEventListener('mouseup', onMouseUp)
    window.addEventListener('mousedown', onPressBegins, true)
    window.addEventListener('blur', onWindowBlur)
    window.addEventListener('keydown', onKeyDown)
    return () => {
      // Unmounted mid-drag (the document changed): the translated text and
      // the cursor are this component's to put back.
      abandonDrags()
      window.removeEventListener('mousemove', onMouseMove)
      window.removeEventListener('mouseup', onMouseUp)
      window.removeEventListener('mousedown', onPressBegins, true)
      window.removeEventListener('blur', onWindowBlur)
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [])

  /** Open a dimension's edit box at a screen point -- from a click on the
   *  sheet, or on its row in the Sheet panel. */
  const openDimension = useCallback(
    (id: string, atX: number, atY: number) => {
      if (!activeDrawing) return
      const view = activeDrawing.views.find((v) => (v.dimensions ?? []).some((d) => d.id === id))
      const dim = view?.dimensions?.find((d) => d.id === id)
      if (!view || !dim) return
      setEditNote(null)
      // One property card at a time, where the title block card sits.
      setTitleBlockOpen(false)
      // Keep the box on screen: 300 x 150 is its largest size.
      const x = Math.min(atX, window.innerWidth - 312)
      const y = Math.min(atY, window.innerHeight - 162)
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
    [activeDrawing],
  )

  const onSheetClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      // The click that ends a dimension drag is the drop, not a selection
      // (also when Escape abandoned the drag before the release).
      if (press.current.clickIsDrop()) return
      const el = (e.target as Element).closest('[data-dim-id]')
      if (!el) {
        // A drag pans the sheet; only a click selects.
        const s = panStateRef.current
        if (Math.hypot(e.clientX - s.startX, e.clientY - s.startY) > 4) return
        setSelectedViewId(viewAt(e.clientX, e.clientY))
        return
      }
      openDimension(
        (el.getAttribute('data-dim-id') ?? '').replace(/-note$/, ''),
        e.clientX + 12,
        e.clientY + 12,
      )
    },
    [viewAt, openDimension],
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
      const drawingId = activeDrawing.id
      const r = await setDrawingDimension(drawingId, dimEdit.dim.id, value, abort.signal)
      setRemovedView(null)
      // Another sheet was opened while the part rebuilt: its editor and its
      // note are not this edit's to touch.
      if (!sheetReads.current.isShowing(drawingId)) return
      // Close THIS dimension's editor; one opened on another dimension while
      // the part rebuilt stays.
      setDimEdit((cur) => (cur && cur.dim.id === dimEdit.dim.id ? null : cur))
      const fmt = (n: number) => String(Number(n.toFixed(4)))
      const steps = [r.driver.sequence, ...(r.driver.linked ?? []).map((l) => l.target_sequence)]
      const where = steps.length > 1 ? `steps ${steps.join(' + ')}` : `step ${steps[0]}`
      noteFor(
        drawingId,
        'done',
        `${dimEdit.dim.label} \u2192 ${fmt(value)}: ${r.driver.parameter} ` +
          `${fmt(r.driver.from_value)} \u2192 ${fmt(r.driver.new_value)} (${where})`,
      )
      await refreshActive(drawingId)
    } catch (e) {
      const stopped = e instanceof DOMException && e.name === 'AbortError'
      const reason = stopped
        ? 'Stopped waiting. The server may still finish this edit; the sheet refreshes when you reopen it.'
        : e instanceof DrawingRefusal || e instanceof Error
          ? e.message
          : String(e)
      setDimEdit((cur) =>
        cur && cur.dim.id === dimEdit.dim.id ? { ...cur, busy: false, startedAt: null, refusal: reason } : cur,
      )
    } finally {
      if (dimEditAbort.current === abort) dimEditAbort.current = null
    }
  }, [dimEdit, activeDrawing, refreshActive, noteFor])

  /** A dimension of `drawingId` was annotated (tolerance, text, suppression,
   *  put back): say so and show the sheet as the kernel now draws it. */
  const annotated = useCallback(
    (drawingId: string, note: string) => {
      setRemovedView(null)
      noteFor(drawingId, 'done', note)
      void refreshActive(drawingId)
    },
    [noteFor, refreshActive],
  )

  const changeProjection = useCallback(
    async (projection: ProjectionAngle) => {
      if (!activeDrawing || (activeDrawing.projection ?? 'third') === projection) return
      setProjectionBusy(true)
      setError(null)
      try {
        await setDrawingProjection(activeDrawing.id, projection)
        setRemovedView(null)
        await refreshActive(activeDrawing.id)
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setProjectionBusy(false)
      }
    },
    [activeDrawing, refreshActive],
  )

  // What the open sheet is on: its ISO size, way up and its FRONT view's
  // scale (every automatic sheet has one). `null` for a custom size.
  const sheetNow = useMemo<SheetChoice | null>(() => {
    if (!activeDrawing || typeof activeDrawing.sheet_size !== 'string') return null
    const size = ISO_SHEET_SIZES.find((s) => s === activeDrawing.sheet_size)
    const main =
      activeDrawing.views.find((v) => v.projection.kind === 'front') ?? activeDrawing.views[0]
    if (!size || !main) return null
    return { size, way: activeDrawing.orientation ?? 'landscape', scale: main.scale }
  }, [activeDrawing])
  // The open sheet's paper and scale, for the status bar.
  useEffect(() => {
    useSheetCursorStore.getState().setSheet(sheetNow ? `${sheetNow.size} \u00B7 ${scaleLabel(sheetNow.scale)}` : null)
    return () => {
      useSheetCursorStore.getState().setSheet(null)
      useSheetCursorStore.getState().setPoint(null)
    }
  }, [sheetNow])

  const changeSheet = useCallback(
    async (asked: SheetChoice) => {
      if (!activeDrawing || !sheetNow) return
      const request = sheetRequest(sheetNow, asked)
      if (!request) return
      setSheetBusy(true)
      setSheetRefused(null)
      setError(null)
      try {
        const done = await setDrawingSheet(activeDrawing.id, request)
        setRemovedView(null)
        const kept =
          done.not_applied.length > 0
            ? ` ${done.not_applied.length} earlier move${done.not_applied.length === 1 ? '' : 's'} did not fit and were laid out again.`
            : ''
        noteFor(
          activeDrawing.id,
          done.not_applied.length > 0 ? 'info' : 'done',
          `Sheet is now ${done.sheet.sheet_size} ${done.sheet.orientation} at ${done.sheet.scale_label}.${kept}`,
        )
        await refreshActive(activeDrawing.id)
      } catch (e) {
        if (e instanceof SheetChangeRefused) setSheetRefused(sheetRefusal(e.message, e.details))
        else setError(e instanceof Error ? e.message : String(e))
      } finally {
        setSheetBusy(false)
      }
    },
    [activeDrawing, sheetNow, noteFor, refreshActive],
  )
  const dismissSheetRefusal = useCallback(() => setSheetRefused(null), [])

  return (
    <div className="relative flex-1 min-h-0 bg-background text-foreground">
      {/* The sheet fills the window; every control is a small overlay on it. */}
      <div
        ref={canvasRef}
        // Right of the Sheet panel (w-64 + its margin) when a sheet is open,
        // so Fit never lays the sheet under the panel.
        className={`absolute inset-y-0 right-0 overflow-hidden bg-muted/20 cursor-grab ${activeDrawing ? 'left-[17rem]' : 'left-0'}`}
        onMouseDown={onCanvasMouseDown}
        onMouseMove={onCanvasHover}
        onMouseLeave={() => useSheetCursorStore.getState().setPoint(null)}
      >
        {activeDrawing ? (
          svg ? (
            <>
            <div
              ref={sheetRef}
              className="absolute top-1/2 left-1/2 bg-white shadow-md [&>svg]:block [&>svg]:w-full [&>svg]:h-full [&_[data-dim-id]]:cursor-pointer [&_[data-dim-id]:hover]:fill-[#d97706] [&_[data-dim-id]:hover]:font-bold [&_[data-dim-editing]]:fill-[#d97706] [&_[data-dim-editing]]:font-bold"
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
            {lookShown?.rect && (
              // The space the agent is looking at, outlined on the sheet. It
              // rides the sheet's own box and transform, in % of the paper,
              // so it follows every pan and zoom without re-measuring.
              <div
                aria-hidden
                data-agent-look={lookShown.space ?? ''}
                className="pointer-events-none absolute top-1/2 left-1/2"
                style={{
                  width: `${displayW}px`,
                  height: `${displayH}px`,
                  transform: `translate(calc(-50% + ${pan.x}px), calc(-50% + ${pan.y}px))`,
                }}
              >
                <div
                  className="absolute rounded-[2px] border-2 border-[#d4006a] shadow-[0_0_0_3px_rgba(212,0,106,0.18)]"
                  style={{
                    left: `${(lookShown.rect.x0 / sheetWMm) * 100}%`,
                    top: `${(lookShown.rect.y0 / sheetHMm) * 100}%`,
                    width: `${((lookShown.rect.x1 - lookShown.rect.x0) / sheetWMm) * 100}%`,
                    height: `${((lookShown.rect.y1 - lookShown.rect.y0) / sheetHMm) * 100}%`,
                  }}
                >
                  {lookShown.space && (
                    <span className="absolute bottom-full left-0 mb-1 whitespace-nowrap rounded-sm bg-[#d4006a] px-1.5 py-0.5 font-mono text-[11px] leading-none text-white">
                      {lookShown.space}
                    </span>
                  )}
                </div>
              </div>
            )}
            {(actShown || flashes.length > 0) && (
              // The agent's hands: the act's moving boxes and ghosts, then the
              // judge's score flashes, riding the sheet's box like the look.
              <div
                aria-hidden
                data-agent-hands={actShown?.verb ?? 'judge'}
                className="pointer-events-none absolute top-1/2 left-1/2"
                style={{
                  width: `${displayW}px`,
                  height: `${displayH}px`,
                  transform: `translate(calc(-50% + ${pan.x}px), calc(-50% + ${pan.y}px))`,
                }}
              >
                {actShown?.moves.map((m) => {
                  const ghost = ghostOpacity(m, actT, handsReduced)
                  const box = movedBoxAt(m, actT, handsReduced)
                  return (
                    <Fragment key={`${m.kind}:${m.id}`}>
                      {ghost > 0 && m.from && (
                        <div
                          data-act-ghost={m.id}
                          className="absolute rounded-[2px] border border-dashed border-[#d4006a] bg-[#d4006a]/10"
                          style={{ ...pct(m.from), opacity: ghost / GHOST_OPACITY }}
                        />
                      )}
                      {box && actRunning(actT) && (
                        <div
                          data-act-box={m.id}
                          className="absolute rounded-[2px] border-2 border-[#d4006a] shadow-[0_0_0_3px_rgba(212,0,106,0.18)]"
                          style={pct(box)}
                        />
                      )}
                    </Fragment>
                  )
                })}
                {flashes.map((f) => {
                  const o = flashOpacity(f, judgeT, handsReduced)
                  if (o <= 0) return null
                  return (
                    <div
                      key={f.space}
                      data-judge-space={f.space}
                      data-judge-verdict={f.verdict}
                      className="absolute rounded-[2px] border-2"
                      style={{ ...pct(f.rect), borderColor: f.color, backgroundColor: `${f.color}1f`, opacity: o }}
                    >
                      <span
                        className="absolute bottom-full left-0 mb-1 whitespace-nowrap rounded-sm px-1.5 py-0.5 font-mono text-[11px] leading-none text-white"
                        style={{ backgroundColor: f.color }}
                      >
                        {f.space} {f.label}
                      </span>
                    </div>
                  )
                })}
              </div>
            )}
            </>
          ) : (
            <div className="absolute inset-0 flex items-center justify-center text-xs text-muted-foreground">
              {loading ? 'Rendering…' : ''}
            </div>
          )
        ) : (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 px-6 text-center text-sm text-muted-foreground">
            {drawings.length === 0 ? (
              parts.length === 0 ? (
                'No parts yet. Build or import a part in the Part workspace, then make its drawing here.'
              ) : (
                <>
                  <span>No drawings yet. Make one from a part:</span>
                  <div className="flex max-w-xl flex-wrap justify-center gap-2">
                    {/* The same action the menu's "New drawing from a part" takes,
                        offered where the engineer is looking. */}
                    {parts.map((p) => (
                      <button
                        key={p.id}
                        type="button"
                        onClick={() => void handleCreateFromPart(p)}
                        className="cad-focus rounded-md border border-border bg-card px-3 py-1.5 text-[13px] text-foreground shadow-sm hover:bg-accent/40"
                      >
                        Make a drawing of {p.name}
                      </button>
                    ))}
                  </div>
                </>
              )
            ) : (
              'Choose a drawing from the menu at the top left.'
            )}
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
        <SheetPanel
          drawing={activeDrawing}
          quality={sheetCheck}
          selectedViewId={selectedViewId}
          onSelectView={setSelectedViewId}
          busy={viewsBusy}
          onViewMenu={openViewMenu}
          onRemoveView={(id) => void handleRemoveView(id)}
          onAddView={(kind, label) => void handleAddLike(kind, label)}
          onHoverView={setHoverViewId}
        />
      )}
      {activeDrawing && (
        <SheetActions
          projection={activeDrawing.projection ?? 'third'}
          projectionBusy={projectionBusy}
          onProjection={(pr) => void changeProjection(pr)}
          sheet={sheetNow}
          sheetBusy={sheetBusy}
          sheetRefused={sheetRefused}
          onSheet={(asked) => void changeSheet(asked)}
          onDismissSheetRefusal={dismissSheetRefusal}
          titleBlockOpen={titleBlockOpen}
          onToggleTitleBlock={() => {
            // One property card at a time.
            dimEditAbort.current?.abort()
            setDimEdit(null)
            setTitleBlockOpen((o) => !o)
          }}
          onDownload={(f) => void handleDownload(f)}
          onPickDimension={() => {
            setNoteKind('info')
            setEditNote('Click a dimension on the sheet to open its properties. Drag one to move it.')
          }}
        />
      )}
      {activeDrawing && titleBlockOpen && (
        <TitleBlockCard
          key={`title-block:${activeDrawing.id}`}
          drawing={activeDrawing}
          verified={titleBlockVerified(svg)}
          onUpdated={(updated) => {
            // The card of THIS drawing saved: the answer may arrive after
            // another sheet was opened, and must not land on it.
            const id = activeDrawing.id
            setRemovedView(null)
            setActiveDrawing((prev) => applySavedTitleBlock(prev, id, updated))
            void refreshActive(id)
          }}
          onError={setError}
          onClose={() => setTitleBlockOpen(false)}
        />
      )}
      {hoverRect && (
        <div
          aria-hidden
          className="pointer-events-none absolute z-10 rounded-sm border border-[var(--cad-selected)]/70 bg-[var(--cad-selected-wash)]"
          style={hoverRect}
        />
      )}
      {selRect && (
        <div
          aria-hidden
          className="pointer-events-none absolute z-10 rounded-sm border border-dashed border-sky-600"
          style={selRect}
        />
      )}
      {viewGhost && (
        <div
          aria-hidden
          className="pointer-events-none absolute z-10 rounded-sm border border-dashed border-sky-600 bg-sky-500/5"
          style={{
            left: viewGhost.left,
            top: viewGhost.top,
            width: viewGhost.width,
            height: viewGhost.height,
          }}
        >
          <span className="absolute -top-5 left-0 whitespace-nowrap rounded-sm bg-card/95 px-1 font-mono text-[11px] text-sky-700">
            {`x ${viewGhost.dxMm >= 0 ? '+' : ''}${viewGhost.dxMm} · y ${viewGhost.dyMm >= 0 ? '+' : ''}${viewGhost.dyMm} mm`}
          </span>
        </div>
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

      <SheetNotices
        note={editNote === null ? null : { kind: noteKind, text: editNote }}
        onDismissNote={() => setEditNote(null)}
        removedView={removedView?.name ?? null}
        undoBusy={viewsBusy}
        onUndo={() => void undoRemoveView()}
        onDismissRemoved={() => setRemovedView(null)}
      />

            {dimEdit && activeDrawing && (
              <DimensionPanel
                key={`${activeDrawing.id}:${dimEdit.dim.id}`}
                drawing={activeDrawing}
                dim={dimEdit.dim}
                view={dimEdit.view}
                kindWords={DIM_KIND_WORDS[dimEdit.dim.kind] ?? dimEdit.dim.kind.replace(/_/g, ' ')}
                info={dimListing?.dimensions.find((d) => d.id === dimEdit.dim.id) ?? null}
                certificate={dimListing?.certificate ?? null}
                driver={dimEdit.driver}
                driverLine={driverLine(dimEdit.driver)}
                draft={dimEdit.draft}
                busy={dimEdit.busy}
                startedAt={dimEdit.startedAt}
                valueRefusal={dimEdit.refusal}
                onDraft={(draft) => setDimEdit((cur) => (cur ? { ...cur, draft, refusal: null } : cur))}
                onApplyValue={() => void applyDimEdit()}
                onStop={() => dimEditAbort.current?.abort()}
                onClose={() => setDimEdit(null)}
                onChanged={(note) => annotated(activeDrawing.id, note)}
              />
            )}
            {activeDrawing && svg && (
              <CompletenessChip
                key={`certificate:${activeDrawing.id}`}
                drawing={activeDrawing}
                certificate={dimListing?.certificate ?? null}
                onChanged={(note) => annotated(activeDrawing.id, note)}
              />
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
                  className="cad-focus inline-flex h-6 min-w-6 items-center justify-center px-2 text-sm leading-none hover:bg-accent/30 rounded"
                >
                  −
                </button>
                <button
                  type="button"
                  onClick={zoomFit}
                  title="Fit to window"
                  className={[
                    'cad-focus inline-flex h-6 items-center px-2 text-[11px] hover:bg-accent/30 rounded',
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
                    'cad-focus inline-flex h-6 items-center px-2 text-[11px] hover:bg-accent/30 rounded',
                    zoomMode === 'manual' && zoom === 1 ? 'bg-accent/40' : '',
                  ].join(' ')}
                >
                  100%
                </button>
                <button
                  type="button"
                  onClick={zoomIn}
                  title="Zoom in"
                  className="cad-focus inline-flex h-6 min-w-6 items-center justify-center px-2 text-sm leading-none hover:bg-accent/30 rounded"
                >
                  +
                </button>
              </div>
            )}

    </div>
  )
}

