/**
 * The drawing window's controls, as small overlays ON the sheet canvas (the
 * layout law: the viewport is the work; every panel is a transparent,
 * closable overlay; peers hug, roles bookend).
 *
 * - top-left  : which drawing (switch, rename, delete, new from a part)
 * - top-right : how it is laid out, annotated and delivered (projection,
 *               annotate, title block, download)
 * - bottom-left: its views (remove with undo, "Add view ▸ Left"); right-click
 *               a view (on the sheet or its chip) for its menu
 *
 * Nothing here asks for an internal id or a coordinate: views are added
 * "like" a view already on the sheet and the kernel places them.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  ChevronDown,
  Pencil,
  Plus,
  Trash2,
  X,
} from '@/components/ui/icons'
import {
  type Drawing,
  type DrawingFormat,
  type ProjectionAngle,
  type ProjectionType,
  type TitleBlock,
  type TitleBlockPatch,
  type ViewEdit,
  defaultTitleBlock,
  updateTitleBlock,
} from '@/lib/drawings-api'

/** A drawing in the switcher list. */
export interface DrawingRow {
  id: string
  name: string
}

/** A part the user can start a drawing from. */
export interface PartRow {
  id: string
  name: string
}

const card = 'bg-card/95 border border-border rounded-md shadow-md'

/** Close a popover on an outside click or Escape. */
function useDismiss(open: boolean, onClose: () => void) {
  const ref = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open, onClose])
  return ref
}

// ── Drawing switcher (top-left) ──────────────────────────────────────────

export function DrawingSwitcher({
  drawings,
  active,
  parts,
  onSelect,
  onRename,
  onDelete,
  onCreateFromPart,
}: {
  drawings: DrawingRow[]
  active: Drawing | null
  parts: PartRow[]
  onSelect: (id: string) => void
  onRename: (id: string, name: string) => void
  onDelete: (id: string) => void
  onCreateFromPart: (part: PartRow) => void
}) {
  const [open, setOpen] = useState(false)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  const close = useCallback(() => {
    setOpen(false)
    setRenaming(null)
  }, [])
  const ref = useDismiss(open, close)

  return (
    <div ref={ref} className="absolute top-3 left-3 z-20 max-w-[min(420px,calc(100%-24px))]">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`${card} cad-focus flex items-center gap-2 px-3 py-1.5 text-sm max-w-full`}
        title="Switch, rename or create drawings"
      >
        <span className="truncate font-medium">{active?.name ?? 'No drawing open'}</span>
        {active && (
          <span className="shrink-0 text-[11px] text-muted-foreground">
            {typeof active.sheet_size === 'string' ? active.sheet_size : 'Custom'}
          </span>
        )}
        <ChevronDown size={14} className="shrink-0 text-muted-foreground" />
      </button>
      {open && (
        <div className={`${card} mt-1 w-[320px] max-w-full py-1 text-xs`}>
          <div className="px-3 py-1 text-[11px] uppercase tracking-wider text-muted-foreground">
            Drawings
          </div>
          {drawings.length === 0 && (
            <div className="px-3 py-2 text-muted-foreground">None yet. Start one from a part below.</div>
          )}
          {drawings.map((d) => (
            <div
              key={d.id}
              className={`group flex items-center gap-1 px-1 ${
                d.id === active?.id ? 'bg-accent/30' : 'hover:bg-accent/20'
              }`}
            >
              {renaming === d.id ? (
                <input
                  autoFocus
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault()
                      if (draft.trim()) onRename(d.id, draft.trim())
                      setRenaming(null)
                    } else if (e.key === 'Escape') {
                      e.stopPropagation()
                      setRenaming(null)
                    }
                  }}
                  className="cad-focus flex-1 min-w-0 my-1 mx-2 px-1 py-0.5 rounded border border-border bg-background"
                />
              ) : (
                <button
                  type="button"
                  onClick={() => {
                    onSelect(d.id)
                    close()
                  }}
                  className="cad-focus flex-1 min-w-0 truncate text-left px-2 py-1.5"
                >
                  {d.id === active?.id && <Check size={12} className="inline mr-1 -mt-0.5" />}
                  {d.name}
                </button>
              )}
              <button
                type="button"
                aria-label={`Rename ${d.name}`}
                title="Rename"
                onClick={() => {
                  setRenaming(d.id)
                  setDraft(d.name)
                }}
                className="cad-focus p-1 rounded text-muted-foreground opacity-0 group-hover:opacity-100 hover:text-foreground"
              >
                <Pencil size={12} />
              </button>
              <button
                type="button"
                aria-label={`Delete ${d.name}`}
                title="Delete drawing"
                onClick={() => {
                  if (confirm(`Delete the drawing "${d.name}"? This cannot be undone.`)) onDelete(d.id)
                }}
                className="cad-focus p-1 rounded text-muted-foreground opacity-0 group-hover:opacity-100 hover:text-destructive"
              >
                <Trash2 size={12} />
              </button>
            </div>
          ))}
          <div className="my-1 border-t border-border/60" />
          <div className="px-3 py-1 text-[11px] uppercase tracking-wider text-muted-foreground">
            New drawing from a part
          </div>
          {parts.length === 0 ? (
            <div className="px-3 py-2 text-muted-foreground">No parts in this document yet.</div>
          ) : (
            parts.map((p) => (
              <button
                key={p.id}
                type="button"
                onClick={() => {
                  onCreateFromPart(p)
                  close()
                }}
                className="cad-focus flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-accent/20"
              >
                <Plus size={12} className="shrink-0 text-muted-foreground" />
                <span className="truncate">{p.name}</span>
              </button>
            ))
          )}
        </div>
      )}
    </div>
  )
}

// ── Sheet actions (top-right) ────────────────────────────────────────────

export function SheetActions({
  projection,
  projectionBusy,
  onProjection,
  titleBlockOpen,
  onToggleTitleBlock,
  onDownload,
}: {
  projection: ProjectionAngle
  projectionBusy: boolean
  onProjection: (p: ProjectionAngle) => void
  titleBlockOpen: boolean
  onToggleTitleBlock: () => void
  onDownload: (f: DrawingFormat) => void
}) {
  const [menu, setMenu] = useState(false)
  const close = useCallback(() => setMenu(false), [])
  const ref = useDismiss(menu, close)
  return (
    <div className="absolute top-3 right-3 z-20 flex items-center gap-2">
      <div
        role="radiogroup"
        aria-label="Projection"
        className={`${card} flex items-center overflow-hidden`}
      >
        {(['first', 'third'] as const).map((p) => (
          <button
            key={p}
            type="button"
            role="radio"
            aria-checked={projection === p}
            disabled={projectionBusy}
            onClick={() => onProjection(p)}
            title={
              p === 'first'
                ? 'First-angle projection (ISO): TOP below FRONT, RIGHT to its left'
                : 'Third-angle projection (ASME): TOP above FRONT, RIGHT to its right'
            }
            className={`cad-focus px-2.5 py-1.5 text-xs ${
              projection === p ? 'bg-accent/40 font-medium' : 'hover:bg-accent/20'
            }`}
          >
            {p === 'first' ? '1st angle' : '3rd angle'}
          </button>
        ))}
      </div>
      <AnnotateMenu />
      <button
        type="button"
        onClick={onToggleTitleBlock}
        aria-pressed={titleBlockOpen}
        className={`${card} cad-focus px-2.5 py-1.5 text-xs ${titleBlockOpen ? 'bg-accent/40' : 'hover:bg-accent/20'}`}
      >
        Title block
      </button>
      <div ref={ref} className="relative">
        <button
          type="button"
          onClick={() => setMenu((m) => !m)}
          className={`${card} cad-focus flex items-center gap-1 px-2.5 py-1.5 text-xs hover:bg-accent/20`}
        >
          Download
          <ChevronDown size={12} />
        </button>
        {menu && (
          <div className={`${card} absolute right-0 mt-1 w-56 py-1 text-xs`}>
            {(
              [
                ['pdf', 'PDF', 'for review and the shop floor'],
                ['dxf', 'DXF', 'to edit in another CAD tool'],
                ['svg', 'SVG', 'for the web'],
              ] as const
            ).map(([f, label, why]) => (
              <button
                key={f}
                type="button"
                onClick={() => {
                  onDownload(f)
                  close()
                }}
                className="cad-focus flex w-full items-baseline gap-2 px-3 py-1.5 text-left hover:bg-accent/20"
              >
                <span className="font-medium w-8">{label}</span>
                <span className="text-muted-foreground">{why}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

// ── Annotate menu (top-right) ────────────────────────────────────────────

/**
 * What an engineer may want to add to a sheet, and whether the kernel can do
 * it from the sheet today. An item is enabled only when a drawing route
 * places that annotation on this sheet AND records it in the history (so
 * Ctrl+Z and a restart keep it). `why` says, in a drafter's words, what is
 * there instead. Surveyed against `api-server/src/main.rs` (the
 * `/api/drawings/...` routes) and `drawing/types.rs`: there is no route that
 * adds a note, dimension, centre mark, datum flag, feature control frame or
 * surface-finish symbol to a sheet, so none is enabled; none is faked.
 */
const ANNOTATIONS: { key: string; label: string; why: string }[] = [
  {
    key: 'note',
    label: 'Note',
    why: 'Free text on the sheet is not available yet. The title block holds the drawing text.',
  },
  {
    key: 'dimension',
    label: 'Dimension',
    why: 'Dimensions are placed by the kernel from the part. Click one to change the part; adding one is not available yet.',
  },
  {
    key: 'centre',
    label: 'Centre mark / line',
    why: 'Drawn automatically on holes and round features; placing one by hand is not available yet.',
  },
  {
    key: 'datum',
    label: 'Datum flag',
    why: 'Placing a datum from the sheet is not available yet. Datums designated through the agent API (this session only) are drawn when a sheet is made.',
  },
  {
    key: 'fcf',
    label: 'GD&T frame',
    why: 'Adding a frame from the sheet is not available yet. Frames authored through the agent API (this session only) are drawn when a sheet is made.',
  },
  {
    key: 'finish',
    label: 'Surface finish',
    why: 'Surface-finish symbols are not available yet.',
  },
]

export function AnnotateMenu() {
  const [open, setOpen] = useState(false)
  const close = useCallback(() => setOpen(false), [])
  const ref = useDismiss(open, close)
  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        aria-haspopup="menu"
        className={`${card} cad-focus flex items-center gap-1 px-2.5 py-1.5 text-xs hover:bg-accent/20`}
      >
        Annotate
        <ChevronDown size={12} />
      </button>
      {open && (
        <div role="menu" aria-label="Annotate" className={`${card} absolute right-0 mt-1 w-72 py-1 text-xs`}>
          {ANNOTATIONS.map((a) => (
            // Not a disabled <button>: browsers drop the tooltip on one, and
            // the reason is what the engineer needs, so it is shown in the row.
            <div
              key={a.key}
              role="menuitem"
              aria-disabled="true"
              title={a.why}
              className="cursor-default px-3 py-1.5"
            >
              <div className="text-muted-foreground">{a.label}</div>
              <div className="text-[11px] leading-snug text-muted-foreground/80">{a.why}</div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

// ── Views (bottom-left) ──────────────────────────────────────────────────

type Preset = Exclude<ProjectionType['kind'], 'custom'>

const PRESETS: { kind: Preset; label: string }[] = [
  { kind: 'front', label: 'Front' },
  { kind: 'top', label: 'Top' },
  { kind: 'right', label: 'Right' },
  { kind: 'left', label: 'Left' },
  { kind: 'bottom', label: 'Bottom' },
  { kind: 'isometric', label: 'Isometric' },
]

export function ViewsBar({
  drawing,
  busy,
  onRemove,
  onAdd,
  onMenu,
  selectedId,
}: {
  drawing: Drawing
  busy: boolean
  onRemove: (viewId: string) => void
  onAdd: (kind: Preset, label: string) => void
  /** Open a view's menu at a screen point. */
  onMenu: (viewId: string, x: number, y: number) => void
  selectedId: string | null
}) {
  const [menu, setMenu] = useState(false)
  const close = useCallback(() => setMenu(false), [])
  const ref = useDismiss(menu, close)
  const present = useMemo(
    () => new Set(drawing.views.map((v) => v.projection.kind)),
    [drawing.views],
  )
  const missing = PRESETS.filter((p) => !present.has(p.kind))
  return (
    <div className="absolute bottom-3 left-3 z-20 flex flex-wrap items-center gap-1.5 max-w-[calc(100%-260px)]">
      {drawing.views.map((v) => (
        <span
          key={v.id}
          className={`${card} group inline-flex items-center gap-1 pl-2 pr-1 py-1 text-[11px] ${
            selectedId === v.id ? 'ring-1 ring-sky-600' : ''
          }`}
          onContextMenu={(e) => {
            e.preventDefault()
            onMenu(v.id, e.clientX, e.clientY)
          }}
        >
          <button
            type="button"
            title="View options (or right-click the view on the sheet)"
            onClick={(e) => {
              const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
              onMenu(v.id, r.left, r.top - 8)
            }}
            className="cad-focus font-medium hover:underline"
          >
            {v.name}
          </button>
          {v.show_hidden === false && (
            <span className="text-muted-foreground" title="Hidden lines removed">
              no hidden
            </span>
          )}
          {v.scale !== 1 && <span className="text-muted-foreground">{scaleLabel(v.scale)}</span>}
          <button
            type="button"
            aria-label={`Remove view ${v.name}`}
            title="Remove view (you can undo)"
            disabled={busy}
            onClick={() => onRemove(v.id)}
            className="cad-focus rounded p-0.5 text-muted-foreground opacity-50 group-hover:opacity-100 hover:text-destructive"
          >
            <X size={11} />
          </button>
        </span>
      ))}
      {drawing.views.length > 0 && missing.length > 0 && (
        <div ref={ref} className="relative">
          <button
            type="button"
            disabled={busy}
            onClick={() => setMenu((m) => !m)}
            className={`${card} cad-focus inline-flex items-center gap-1 px-2 py-1 text-[11px] hover:bg-accent/20`}
          >
            <Plus size={11} />
            Add view
          </button>
          {menu && (
            <div className={`${card} absolute bottom-full left-0 mb-1 w-40 py-1 text-xs`}>
              {missing.map((p) => (
                <button
                  key={p.kind}
                  type="button"
                  onClick={() => {
                    onAdd(p.kind, p.label)
                    close()
                  }}
                  className="cad-focus block w-full px-3 py-1.5 text-left hover:bg-accent/20"
                >
                  {p.label}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ── View menu (right-click a view) ────────────────────────────────────

/** The scales a drafter picks from (ISO 5455 preferred ratios). */
const SCALES = [0.1, 0.2, 0.5, 1, 2, 5, 10]

export function ViewMenu({
  drawing,
  viewId,
  x,
  y,
  busy,
  onClose,
  onEdit,
  onDelete,
}: {
  drawing: Drawing
  viewId: string
  x: number
  y: number
  busy: boolean
  onClose: () => void
  onEdit: (viewId: string, edit: ViewEdit) => void
  onDelete: (viewId: string) => void
}) {
  const ref = useDismiss(true, onClose)
  const view = drawing.views.find((v) => v.id === viewId)
  const [renaming, setRenaming] = useState(false)
  const [draft, setDraft] = useState(view?.name ?? '')
  if (!view) return null
  const hiddenShown = view.show_hidden !== false
  // Keep the menu on screen: 224 x 230 is its largest size.
  const left = Math.max(8, Math.min(x, window.innerWidth - 232))
  const top = Math.max(8, Math.min(y, window.innerHeight - 238))
  const item =
    'cad-focus flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left hover:bg-accent/20 disabled:opacity-50'
  return (
    <div
      ref={ref}
      role="menu"
      aria-label={`View ${view.name}`}
      className={`${card} fixed z-40 w-56 py-1 text-xs`}
      style={{ left, top }}
    >
      <div className="px-3 pb-1 pt-0.5 text-[11px] text-muted-foreground">
        {view.name} · {scaleLabel(view.scale)}
      </div>
      <button
        type="button"
        role="menuitem"
        disabled={busy}
        className={item}
        onClick={() => {
          onEdit(view.id, { show_hidden: !hiddenShown })
          onClose()
        }}
      >
        <span>{hiddenShown ? 'Remove hidden lines' : 'Show hidden lines'}</span>
        <span className="text-muted-foreground">{hiddenShown ? 'now dashed' : 'now removed'}</span>
      </button>
      <div className="px-3 py-1.5">
        <div className="mb-1">Scale</div>
        <div className="flex flex-wrap gap-1">
          {SCALES.map((s) => {
            const current = Math.abs(s - view.scale) < 1e-9
            return (
              <button
                key={s}
                type="button"
                disabled={busy || current}
                aria-pressed={current}
                onClick={() => {
                  onEdit(view.id, { scale: s })
                  onClose()
                }}
                className={`cad-focus rounded border border-border px-1.5 py-0.5 text-[11px] hover:bg-accent/20 ${
                  current ? 'bg-accent/30 font-medium' : ''
                }`}
              >
                {scaleLabel(s)}
              </button>
            )
          })}
        </div>
      </div>
      {renaming ? (
        <form
          className="flex items-center gap-1 px-3 py-1.5"
          onSubmit={(e) => {
            e.preventDefault()
            const n = draft.trim()
            if (n && n !== view.name) onEdit(view.id, { name: n })
            onClose()
          }}
        >
          <input
            autoFocus
            value={draft}
            maxLength={40}
            onChange={(e) => setDraft(e.target.value)}
            className="cad-focus min-w-0 flex-1 rounded border border-border bg-background px-1.5 py-0.5"
            aria-label="View name"
          />
          <button type="submit" className="cad-focus rounded p-0.5 hover:bg-accent/20" aria-label="Rename">
            <Check size={12} />
          </button>
        </form>
      ) : (
        <button type="button" role="menuitem" disabled={busy} className={item} onClick={() => setRenaming(true)}>
          <span>Rename…</span>
        </button>
      )}
      <div className="my-1 border-t border-border" />
      <button
        type="button"
        role="menuitem"
        disabled={busy}
        className={`${item} hover:text-destructive`}
        onClick={() => {
          onDelete(view.id)
          onClose()
        }}
      >
        <span>Delete view</span>
        <span className="text-muted-foreground">Del</span>
      </button>
    </div>
  )
}

function scaleLabel(s: number): string {
  return s >= 1 ? `${Number(s.toFixed(2))}:1` : `1:${Number((1 / s).toFixed(2))}`
}

// ── Title block card (opens under the actions) ──────────────────────────

const FIELDS: { key: keyof TitleBlock; label: string; placeholder: string; wide?: boolean }[] = [
  { key: 'supplementary_title', label: 'Supplementary title', placeholder: 'detail that does not fit the title', wide: true },
  { key: 'material', label: 'Material', placeholder: 'e.g. S275, 6061-T6 (gives the mass)' },
  { key: 'drawn_by', label: 'Created by', placeholder: '—' },
  { key: 'approved_by', label: 'Approved by', placeholder: 'a person; sets status APPROVED' },
  { key: 'date', label: 'Date of issue', placeholder: 'YYYY-MM-DD' },
  { key: 'document_type', label: 'Document type', placeholder: 'PART DRAWING' },
  { key: 'revision', label: 'Revision', placeholder: 'A' },
]

export function TitleBlockCard({
  drawing,
  onUpdated,
  onError,
  onClose,
}: {
  drawing: Drawing
  onUpdated: (tb: TitleBlock) => void
  onError: (msg: string) => void
  onClose: () => void
}) {
  const original = useMemo(
    () => ({ ...defaultTitleBlock(), ...(drawing.title_block ?? {}) }),
    [drawing.title_block],
  )
  // Seeded once; the parent remounts the card (its `key`) whenever the
  // server's title block changes, so the draft never shows stale values.
  const [draft, setDraft] = useState<TitleBlock>(original)

  const commit = useCallback(async () => {
    const patch: TitleBlockPatch = {}
    for (const f of FIELDS) {
      const a = (draft[f.key] ?? '') as string
      const b = (original[f.key] ?? '') as string
      if (a !== b) (patch as Record<string, string>)[f.key] = a
    }
    const num = draft.drawing_number?.trim() ? draft.drawing_number.trim() : null
    if (num !== (original.drawing_number ?? null)) patch.drawing_number = num
    if (Object.keys(patch).length === 0) return
    try {
      onUpdated(await updateTitleBlock(drawing.id, patch))
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setDraft(original)
    }
  }, [draft, original, drawing.id, onUpdated, onError])

  const approved = !!(draft.approved_by ?? '').trim()
  return (
    <div className={`${card} absolute top-14 right-3 z-30 w-[360px] p-3 text-xs`}>
      <div className="flex items-center justify-between mb-2">
        <div className="font-medium text-sm">Title block</div>
        <div className="flex items-center gap-2">
          <span
            className={`px-1.5 py-0.5 rounded text-[11px] font-medium ${
              approved ? 'bg-green-600/15 text-green-700' : 'bg-amber-500/15 text-amber-700'
            }`}
          >
            {approved ? 'APPROVED' : 'FOR REVIEW'}
          </span>
          <button type="button" aria-label="Close" onClick={onClose} className="cad-focus text-muted-foreground hover:text-foreground">
            <X size={14} />
          </button>
        </div>
      </div>
      <form
        className="grid grid-cols-2 gap-x-2 gap-y-2"
        onSubmit={(e) => {
          e.preventDefault()
          void commit()
        }}
      >
        {FIELDS.map((f) => (
          <label key={f.key} className={`flex flex-col gap-0.5 ${f.wide ? 'col-span-2' : ''}`}>
            <span className="text-[11px] uppercase tracking-wider text-muted-foreground">{f.label}</span>
            <input
              value={(draft[f.key] ?? '') as string}
              placeholder={f.placeholder}
              onChange={(e) => setDraft({ ...draft, [f.key]: e.target.value })}
              onBlur={() => void commit()}
              className="cad-focus px-2 py-1 rounded border border-border bg-background"
            />
          </label>
        ))}
        <label className="flex flex-col gap-0.5">
          <span className="text-[11px] uppercase tracking-wider text-muted-foreground">Drawing no.</span>
          <input
            value={draft.drawing_number ?? ''}
            placeholder="automatic"
            onChange={(e) => setDraft({ ...draft, drawing_number: e.target.value })}
            onBlur={() => void commit()}
            className="cad-focus px-2 py-1 rounded border border-border bg-background"
          />
        </label>
        <button type="submit" className="sr-only">
          Save
        </button>
      </form>
      <p className="mt-2 text-muted-foreground">
        Mass, units, the verification stamp and the tolerance class are filled in by the kernel.
      </p>
    </div>
  )
}

// ── First-run hint ───────────────────────────────────────────────────────

const HINT_KEY = 'roshera.drawingHintDismissed'

export function SheetHint() {
  const [show, setShow] = useState(() => {
    try {
      return localStorage.getItem(HINT_KEY) !== '1'
    } catch {
      return true
    }
  })
  if (!show) return null
  return (
    <div className={`${card} absolute top-3 left-1/2 -translate-x-1/2 z-10 flex items-center gap-2 px-3 py-1.5 text-xs`}>
      <span>
        Click a dimension to change the part · click a view, then drag it to move it · drag the
        sheet to pan
      </span>
      <button
        type="button"
        aria-label="Dismiss hint"
        onClick={() => {
          setShow(false)
          try {
            localStorage.setItem(HINT_KEY, '1')
          } catch {
            // Storage unavailable: the hint simply returns next time.
          }
        }}
        className="cad-focus text-muted-foreground hover:text-foreground"
      >
        <X size={12} />
      </button>
    </div>
  )
}
