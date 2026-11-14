import { useState } from 'react'
import { ChevronDown, ChevronRight, MoreHorizontal, Plus, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { Drawing, DrawingQualityReport, ProjectedView } from '@/lib/drawings-api'
import { VIEW_PRESETS, checkSummary, dimensionWord, sheetContents, type ViewPreset } from '@/lib/sheet-contents'

/** A view's kind as a small drawn glyph: an outline for an orthographic
 *  view, a cube for the iso, a hatched square for a section. */
function ViewGlyph({ view }: { view: ProjectedView }) {
  const section = /^section/i.test(view.name)
  const iso = view.projection.kind === 'isometric'
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden className="shrink-0 text-muted-foreground">
      {iso ? (
        <g fill="none" stroke="currentColor" strokeWidth="1.2">
          <path d="M8 2 13.5 5v6L8 14 2.5 11V5z" />
          <path d="M2.5 5 8 8l5.5-3M8 8v6" />
        </g>
      ) : section ? (
        <g fill="none" stroke="currentColor" strokeWidth="1.2">
          <rect x="2.5" y="2.5" width="11" height="11" />
          <path d="M2.5 9 9 2.5M2.5 13.5 13.5 2.5M7 13.5 13.5 7" />
        </g>
      ) : (
        <g fill="none" stroke="currentColor" strokeWidth="1.2">
          <rect x="2.5" y="4" width="11" height="8" />
          <path d="M8 2v12" strokeDasharray="1.5 1.5" />
        </g>
      )}
    </svg>
  )
}

const BADGE = {
  positive: 'border-emerald-600/40 text-emerald-700 dark:text-emerald-400',
  caution: 'border-amber-500/50 text-amber-700 dark:text-amber-400',
  negative: 'border-red-500/50 text-red-700 dark:text-red-400',
  muted: 'border-border text-muted-foreground',
} as const

/**
 * The sheet's one view manager (Varun 2026-11-14: "think from the actual
 * designer's POV ... everything with a click ... icons are better than a
 * wall of text"): a compact header (sheet size, the sheet check as a badge,
 * + to add a view), then each view as one row -- its kind as a glyph, its
 * name and scale, its menu and remove -- with its dimensions as value chips
 * under it. A chip opens the edit (the new value edits the part). Details
 * live in tooltips and behind the badge, not in sentences.
 */
export function SheetPanel({
  drawing,
  quality,
  selectedViewId,
  editingDimId,
  busy,
  onSelectView,
  onOpenDimension,
  onViewMenu,
  onRemoveView,
  onAddView,
}: {
  drawing: Drawing
  quality: DrawingQualityReport | null
  selectedViewId: string | null
  editingDimId: string | null
  busy: boolean
  onSelectView: (viewId: string) => void
  /** Open the dimension's edit box at a screen point. */
  onOpenDimension: (dimId: string, x: number, y: number) => void
  onViewMenu: (viewId: string, x: number, y: number) => void
  onRemoveView: (viewId: string) => void
  onAddView: (kind: ViewPreset, label: string) => void
}) {
  const [open, setOpen] = useState(true)
  const [issuesOpen, setIssuesOpen] = useState(false)
  const [adding, setAdding] = useState(false)
  const { views, dimensionCount } = sheetContents(drawing)
  const check = checkSummary(quality)
  const issues = quality?.issues ?? []
  const present = new Set(drawing.views.map((v) => v.projection.kind))
  const missing = VIEW_PRESETS.filter((p) => !present.has(p.kind))
  const byId = new Map(drawing.views.map((v) => [v.id, v]))

  return (
    <section
      aria-label="Sheet contents"
      className="absolute left-3 top-[3.75rem] z-20 flex max-h-[calc(100%-4.5rem)] w-60 flex-col rounded-lg border border-border bg-card/95 text-xs shadow-sm"
      onMouseDown={(e) => e.stopPropagation()}
    >
      {/* Header: fold, sheet size, check badge, add a view. */}
      <div className="flex items-center gap-1.5 border-b border-border px-2 py-1.5">
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          aria-expanded={open}
          aria-label={open ? 'Fold the sheet panel' : 'Open the sheet panel'}
          className="cad-focus rounded p-0.5 text-muted-foreground hover:text-foreground"
        >
          {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </button>
        <span className="rounded border border-border px-1.5 py-px font-mono text-[11px]" title="Sheet size">
          {typeof drawing.sheet_size === 'string' ? drawing.sheet_size : `${drawing.sheet_size.CUSTOM.width}×${drawing.sheet_size.CUSTOM.height}`}
        </span>
        <span className="text-[11px] text-muted-foreground" title={`${dimensionCount} dimensions on ${views.length} views`}>
          {views.length} views · {dimensionCount} dims
        </span>
        <span className="flex-1" />
        <button
          type="button"
          onClick={() => setIssuesOpen((o) => !o)}
          aria-expanded={issuesOpen}
          title={check.text}
          aria-label={`Sheet check: ${check.text}`}
          className={cn('cad-focus rounded border px-1.5 py-px text-[11px] font-medium', BADGE[check.tone])}
        >
          {check.tone === 'positive' ? '✓' : check.tone === 'negative' ? `✗ ${issues.length}` : check.tone === 'caution' ? `⚠ ${issues.length}` : '…'}
        </button>
        {missing.length > 0 && (
          <div className="relative">
            <button
              type="button"
              disabled={busy}
              onClick={() => setAdding((a) => !a)}
              aria-label="Add a view"
              title="Add a view"
              className="cad-focus rounded border border-border p-0.5 hover:bg-accent"
            >
              <Plus size={13} />
            </button>
            {adding && (
              <div className="absolute right-0 top-full z-50 mt-1 w-32 rounded-md border border-border bg-popover py-1 shadow-lg">
                {missing.map((p) => (
                  <button
                    key={p.kind}
                    type="button"
                    onClick={() => {
                      onAddView(p.kind, p.label)
                      setAdding(false)
                    }}
                    className="cad-focus block w-full px-3 py-1 text-left hover:bg-accent"
                  >
                    {p.label}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* The check's findings, only when asked for. */}
      {issuesOpen && (
        <div className="border-b border-border px-3 py-1.5">
          <div className={cn('font-medium', BADGE[check.tone].split(' ').slice(1).join(' '))}>{check.text}</div>
          {issues.map((i, n) => (
            <div key={n} className={cn('mt-0.5 text-[11px]', i.severity === 'error' ? 'text-red-700 dark:text-red-400' : 'text-amber-700 dark:text-amber-400')}>
              {i.view ? <b className="font-medium">{i.view}: </b> : null}
              {i.message}
            </div>
          ))}
        </div>
      )}

      {open && (
        <div className="min-h-0 overflow-y-auto py-1 [scrollbar-width:thin]">
          {views.map((v) => {
            const view = byId.get(v.id)
            return (
              <div key={v.id} className="px-1.5 py-0.5">
                <div
                  className={cn(
                    'group flex items-center gap-1.5 rounded px-1 py-0.5',
                    selectedViewId === v.id ? 'bg-[var(--cad-selected-wash)]' : 'hover:bg-accent/50',
                  )}
                >
                  {view && <ViewGlyph view={view} />}
                  <button
                    type="button"
                    onClick={() => onSelectView(v.id)}
                    className="cad-focus min-w-0 flex-1 truncate text-left font-medium"
                    title="Select this view on the sheet"
                  >
                    {v.name}
                  </button>
                  <span className="font-mono text-[10.5px] text-muted-foreground" title="Scale">
                    {v.scale}
                  </span>
                  <button
                    type="button"
                    aria-label={`${v.name}: scale, hidden lines, more`}
                    title="Scale, hidden lines, more"
                    onClick={(e) => {
                      const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
                      onViewMenu(v.id, r.right + 6, r.top)
                    }}
                    className="cad-focus rounded p-0.5 text-muted-foreground opacity-60 hover:bg-accent hover:text-foreground group-hover:opacity-100"
                  >
                    <MoreHorizontal size={13} />
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    aria-label={`Remove ${v.name}`}
                    title="Remove the view (undo offered)"
                    onClick={() => onRemoveView(v.id)}
                    className="cad-focus rounded p-0.5 text-muted-foreground opacity-0 hover:text-destructive group-hover:opacity-100"
                  >
                    <X size={12} />
                  </button>
                </div>
                {v.dimensions.length > 0 && (
                  <div className="flex flex-wrap gap-1 pb-1 pl-6 pt-0.5">
                    {v.dimensions.map((d) => (
                      <button
                        key={d.id}
                        type="button"
                        onClick={(e) => {
                          const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
                          onOpenDimension(d.id, r.left, r.bottom + 6)
                        }}
                        title={`${dimensionWord(d.kind)} -- click to change the part`}
                        className={cn(
                          'cad-focus rounded border px-1.5 py-px font-mono text-[11px] transition-colors',
                          editingDimId === d.id
                            ? 'border-[var(--cad-selected)] bg-[var(--cad-selected-wash)] text-foreground'
                            : 'border-border bg-background/60 hover:border-[var(--cad-selected)] hover:text-foreground',
                        )}
                      >
                        {d.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )
          })}
        </div>
      )}
    </section>
  )
}
