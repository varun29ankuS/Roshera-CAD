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

import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  type Drawing,
  type ProjectionType,
  type SheetSize,
  addView,
  createDrawing,
  deleteDrawing,
  fetchDrawingSvg,
  getDrawing,
  listDrawings,
  removeView,
} from '@/lib/drawings-api'

// Active solid-id for the projection. In the part workspace this is
// implicit (the tab's part owns one or more solids); for the drawing
// MVP we default to the lowest-id solid (`1`) and allow the user to
// override. F1-class refinement is "pick from a dropdown of the active
// part's solids" — out of scope for the initial wiring.
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

export function DrawingsWorkspace() {
  const [drawingIds, setDrawingIds] = useState<string[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [activeDrawing, setActiveDrawing] = useState<Drawing | null>(null)
  const [svg, setSvg] = useState<string>('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // ── Form state ───────────────────────────────────────────────────
  const [newName, setNewName] = useState('Drawing 1')
  const [newSheet, setNewSheet] = useState<SheetSize>('A3')
  const [viewName, setViewName] = useState('Front')
  const [viewProjection, setViewProjection] = useState<PresetProjectionKind>('front')
  const [viewSolidId, setViewSolidId] = useState<number>(DEFAULT_SOLID_ID)
  const [viewScale, setViewScale] = useState<number>(1.0)
  const [viewX, setViewX] = useState<number>(50)
  const [viewY, setViewY] = useState<number>(50)

  // ── Data fetchers ────────────────────────────────────────────────
  const refreshList = useCallback(async () => {
    setError(null)
    try {
      const ids = await listDrawings()
      setDrawingIds(ids)
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

  useEffect(() => {
    void refreshList()
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
  }, [activeId, refreshActive])

  // ── Mutations ────────────────────────────────────────────────────
  const handleCreate = useCallback(async () => {
    if (!newName.trim()) return
    setError(null)
    try {
      const id = await createDrawing(newName.trim(), newSheet)
      setNewName('Drawing ' + (drawingIds.length + 2))
      await refreshList()
      setActiveId(id)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [newName, newSheet, drawingIds.length, refreshList])

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
    setError(null)
    try {
      await addView(activeId, {
        name: viewName.trim(),
        solid_id: viewSolidId,
        projection: { kind: viewProjection },
        position_mm: [viewX, viewY],
        scale: viewScale,
      })
      await refreshActive(activeId)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [activeId, viewName, viewSolidId, viewProjection, viewX, viewY, viewScale, refreshActive])

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

  const handleDownload = useCallback(() => {
    if (!svg || !activeDrawing) return
    const blob = new Blob([svg], { type: 'image/svg+xml' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `${activeDrawing.name || 'drawing'}.svg`
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
    URL.revokeObjectURL(url)
  }, [svg, activeDrawing])

  // The SVG is server-rendered & deterministic. The kernel's
  // `escape_xml` helper sanitises every text-node string, and we never
  // splice user-supplied markup back into the payload. Safe to inline.
  const svgMarkup = useMemo(() => ({ __html: svg }), [svg])

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
            {drawingIds.length === 0 ? (
              <div className="px-3 py-4 text-xs text-muted-foreground text-center">
                No drawings yet.
              </div>
            ) : (
              drawingIds.map((id) => {
                const isActive = id === activeId
                return (
                  <div
                    key={id}
                    className={[
                      'group flex items-center justify-between gap-1 px-3 py-1.5 text-xs cursor-pointer',
                      isActive
                        ? 'bg-accent/40 text-foreground'
                        : 'text-muted-foreground hover:bg-accent/20',
                    ].join(' ')}
                    onClick={() => setActiveId(id)}
                  >
                    <span className="truncate font-mono">{id.slice(0, 8)}…</span>
                    <button
                      type="button"
                      title="Delete drawing"
                      onClick={(e) => {
                        e.stopPropagation()
                        void handleDelete(id)
                      }}
                      className="cad-focus opacity-0 group-hover:opacity-100 text-destructive hover:text-destructive/80 text-[10px]"
                    >
                      ✕
                    </button>
                  </div>
                )
              })
            )}
          </div>
        </aside>

        {/* ── Center pane: SVG preview ─────────────────────────────── */}
        <main className="flex-1 flex flex-col min-h-0">
          {/* Active-drawing header */}
          <div className="flex items-center justify-between px-4 py-2 border-b border-border/60">
            <div className="flex items-center gap-3">
              <span className="text-sm font-medium">
                {activeDrawing?.name ?? (loading ? 'Loading…' : '— no drawing selected —')}
              </span>
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
              <button
                type="button"
                onClick={handleDownload}
                className="cad-focus px-3 py-1 text-xs rounded border border-border hover:bg-accent/20"
              >
                Download SVG
              </button>
            )}
          </div>

          {/* SVG canvas */}
          <div className="flex-1 min-h-0 overflow-auto bg-muted/20 p-6 flex items-start justify-center">
            {activeDrawing ? (
              svg ? (
                <div
                  className="bg-white shadow-md max-w-full"
                  // eslint-disable-next-line react/no-danger
                  dangerouslySetInnerHTML={svgMarkup}
                />
              ) : (
                <div className="text-xs text-muted-foreground py-12">Rendering…</div>
              )
            ) : (
              <div className="text-xs text-muted-foreground py-12 text-center">
                {drawingIds.length === 0
                  ? 'Create a drawing from the sidebar to get started.'
                  : 'Select a drawing from the sidebar.'}
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
                  className="cad-focus px-3 py-1 text-xs font-medium rounded bg-primary text-primary-foreground hover:opacity-90"
                >
                  + Add View
                </button>
              </div>
            </div>
          )}
        </main>
      </div>
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
