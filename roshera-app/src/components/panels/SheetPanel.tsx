import { useState } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { Drawing, DrawingQualityReport } from '@/lib/drawings-api'
import { checkSummary, dimensionWord, sheetContents, sheetHeading } from '@/lib/sheet-contents'

const TONE = {
  positive: 'text-emerald-700 dark:text-emerald-400',
  caution: 'text-amber-700 dark:text-amber-400',
  negative: 'text-red-700 dark:text-red-400',
  muted: 'text-muted-foreground',
} as const

/**
 * The sheet as a list, beside the sheet (the approved mockup's "Sheet
 * contents"): its size and projection, the kernel's sheet check, and every
 * view with its dimensions. A view row selects the view; a dimension row
 * opens the same edit a click on the sheet does -- the new value edits the
 * part, and the sheet follows.
 */
export function SheetPanel({
  drawing,
  quality,
  selectedViewId,
  editingDimId,
  onSelectView,
  onOpenDimension,
}: {
  drawing: Drawing
  quality: DrawingQualityReport | null
  selectedViewId: string | null
  editingDimId: string | null
  onSelectView: (viewId: string) => void
  /** Open the dimension's edit box at a screen point. */
  onOpenDimension: (dimId: string, x: number, y: number) => void
}) {
  const [open, setOpen] = useState(true)
  const [shut, setShut] = useState<Set<string>>(new Set())
  const { views, dimensionCount } = sheetContents(drawing)
  const check = checkSummary(quality)

  return (
    <section
      aria-label="Sheet contents"
      className={cn(
        'absolute left-3 top-[3.75rem] z-20 flex w-64 flex-col overflow-hidden rounded-lg border border-border bg-card/95 text-xs shadow-sm',
        open && 'bottom-16',
      )}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="cad-focus flex items-center gap-1.5 border-b border-border px-3 py-2 text-left"
      >
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <span className="text-[12.5px] font-semibold">Sheet</span>
        <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">{sheetHeading(drawing)}</span>
      </button>
      {open && (
        <>
          {/* The kernel's sheet check, problems listed by name. */}
          <div className="border-b border-border px-3 py-2">
            <div className={cn('font-medium', TONE[check.tone])}>
              {check.tone === 'positive' ? '✓ ' : check.tone === 'negative' ? '✗ ' : ''}
              {check.text}
            </div>
            {quality && quality.issues.length > 0 && (
              <ul className="mt-1 space-y-0.5">
                {quality.issues.map((i, n) => (
                  <li key={n} className={cn('text-[11px]', i.severity === 'error' ? TONE.negative : TONE.caution)}>
                    {i.view ? `${i.view}: ` : ''}
                    {i.message}
                  </li>
                ))}
              </ul>
            )}
            <div className="mt-0.5 text-[11px] text-muted-foreground">
              {dimensionCount} dimension{dimensionCount === 1 ? '' : 's'} · click one to change the part
            </div>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto py-1 [scrollbar-width:thin]">
            {views.map((v) => {
              const folded = shut.has(v.id)
              return (
                <div key={v.id}>
                  <div
                    className={cn(
                      'flex items-center gap-1 px-2 py-1',
                      selectedViewId === v.id ? 'bg-[var(--cad-selected-wash)]' : 'hover:bg-accent/50',
                    )}
                  >
                    <button
                      type="button"
                      aria-label={folded ? `Open ${v.name}` : `Fold ${v.name}`}
                      disabled={v.dimensions.length === 0}
                      onClick={() =>
                        setShut((s) => {
                          const next = new Set(s)
                          if (next.has(v.id)) next.delete(v.id)
                          else next.add(v.id)
                          return next
                        })
                      }
                      className="cad-focus w-3 shrink-0 text-muted-foreground disabled:opacity-0"
                    >
                      {folded ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
                    </button>
                    <button
                      type="button"
                      onClick={() => onSelectView(v.id)}
                      className="cad-focus flex min-w-0 flex-1 items-baseline gap-2 text-left"
                    >
                      <span className="truncate font-medium">{v.name}</span>
                      <span className="text-[11px] text-muted-foreground">{v.scale}</span>
                      <span className="ml-auto text-[11px] text-muted-foreground">
                        {v.dimensions.length > 0 ? `${v.dimensions.length} dim${v.dimensions.length === 1 ? '' : 's'}` : 'no dims'}
                      </span>
                    </button>
                  </div>
                  {!folded &&
                    v.dimensions.map((d) => (
                      <button
                        key={d.id}
                        type="button"
                        onClick={(e) => {
                          const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
                          onOpenDimension(d.id, r.right + 8, r.top)
                        }}
                        className={cn(
                          'cad-focus flex w-full items-baseline gap-2 py-0.5 pl-7 pr-3 text-left',
                          editingDimId === d.id ? 'bg-[var(--cad-selected-wash)]' : 'hover:bg-accent/50',
                        )}
                        title={`Change this ${dimensionWord(d.kind)} -- edits the part`}
                      >
                        <span className="min-w-0 flex-1 truncate font-mono text-foreground">{d.label}</span>
                        <span className="shrink-0 text-[11px] text-muted-foreground">{dimensionWord(d.kind)}</span>
                      </button>
                    ))}
                </div>
              )
            })}
          </div>
        </>
      )}
    </section>
  )
}
