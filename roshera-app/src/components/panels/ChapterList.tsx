/**
 * The chapter list: the part's history read like a CAD feature tree -- one
 * row per engineering feature, in build order, each with the icon of the tool
 * that made it, its name, and the kernel's verdict (✓ sound / ✗ with the
 * kernel's own reason). A pattern opens to its instances. Rows are chapters
 * grouped by the kernel from its recorded steps (GET /api/timeline/chapters).
 */
import { useEffect, useState } from 'react'
import { cn } from '@/lib/utils'
import { chapterIconKey, chapterTitle, type Chapter } from '@/lib/chapters-api'
import { useChapters } from '@/hooks/use-chapters'
import { useHistoryView } from '@/stores/history-view-store'
import { featureFields, fetchEditableParameters, sizeSummary, type EditableParameter } from '@/lib/feature-edit'
import { Box, Check, ChevronDown, ChevronRight, X, ToolIcons } from '@/components/ui/icons'

function ChapterIcon({ c, size }: { c: Chapter; size: number }) {
  const key = chapterIconKey(c.kind)
  const Icon = key ? ToolIcons[key] : Box
  return <Icon size={size} />
}

function Verdict({ c }: { c: Chapter }) {
  if (c.sound)
    return (
      <Check size={12} className="text-emerald-600/60 dark:text-emerald-400/60" title="sound — the kernel re-measured every step" />
    )
  const why = c.refusals.map((r) => `step ${r.sequence}: ${r.status}${r.reason ? ` — ${r.reason}` : ''}`).join('\n')
  return <X size={14} title={why || 'not sound'} />
}

/** Who made the feature and why, for the row's hover: the tree shows one
 *  line per feature, as CAD trees do (the approved mockup). */
function byline(c: Chapter): string {
  if (c.author === 'agent') return `Made by ${c.author_detail.model ?? 'the agent'}${c.intent ? ` · ${c.intent}` : ''}`
  if (c.author === 'user') return `Made by ${c.author_detail.name ?? 'you'}`
  return ''
}

/** The feature's name as the tree shows it: capitalised, as a CAD tree
 *  reads ("Flange", not "flange"); the stored name is unchanged. */
function displayName(c: Chapter): string {
  const t = chapterTitle(c)
  return t.charAt(0).toUpperCase() + t.slice(1)
}

function Row({
  c,
  depth,
  selected,
  onSelect,
  ahead = false,
  params,
}: {
  c: Chapter
  depth: number
  /** Every size an edit can change, for the row's size column. */
  params: EditableParameter[]
  selected: number | null
  onSelect: (anchor: number) => void
  /** After the slider's position: not applied in what the viewport shows. */
  ahead?: boolean
}) {
  const [open, setOpen] = useState(false)
  const isPattern = c.instances.length > 0
  return (
    <li>
      <div
        role="treeitem"
        aria-selected={selected === c.anchor}
        aria-expanded={isPattern ? open : undefined}
        tabIndex={0}
        onClick={() => onSelect(c.anchor)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') onSelect(c.anchor)
          if (isPattern && (e.key === 'ArrowRight' || e.key === 'ArrowLeft')) setOpen(e.key === 'ArrowRight')
        }}
        title={byline(c) || undefined}
        className={cn(
          'flex h-6 items-center gap-1.5 rounded px-1.5 cursor-default outline-none',
          'hover:bg-accent/40 focus-visible:ring-1 focus-visible:ring-ring',
          selected === c.anchor && 'bg-accent/60',
          ahead && 'opacity-45',
        )}
        style={{ paddingLeft: 6 + depth * 14 }}
      >
        {isPattern ? (
          <button
            type="button"
            aria-label={open ? 'Fold the pattern' : 'Open the pattern'}
            onClick={(e) => {
              e.stopPropagation()
              setOpen((o) => !o)
            }}
            className="shrink-0 text-muted-foreground"
          >
            {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
          </button>
        ) : (
          <span className="w-3 shrink-0" />
        )}
        <ChapterIcon c={c} size={17} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-foreground">{displayName(c)}</span>
        {/* The feature's sizes as an engineer writes them (the mockup's size
            column): Ø80 × 12, R3, 4× Ø8. */}
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-muted-foreground">
          {sizeSummary(c.kind, featureFields(c, params), c.instances.length)}
        </span>
        {c.author === 'agent' && (
          <span aria-label="made by the agent" className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500" />
        )}
        <span className="shrink-0">
          <Verdict c={c} />
        </span>
      </div>
      {!c.sound && c.refusals[0]?.reason ? (
        <div className="pb-1 text-[11px] leading-4 text-destructive" style={{ paddingLeft: 30 + depth * 14 }}>
          {c.refusals[0].reason}
        </div>
      ) : null}
      {isPattern && open ? (
        <ul role="group">
          {c.instances.map((i) => (
            <Row key={i.anchor} c={i} depth={depth + 1} selected={selected} onSelect={onSelect} ahead={ahead} params={params} />
          ))}
        </ul>
      ) : null}
    </li>
  )
}

export function ChapterList({ branch = 'main' }: { branch?: string }) {
  const { chapters, loading, error } = useChapters(branch)
  // Selecting a row highlights that feature, as in a feature tree. It never
  // moves the timeline: only the slider travels in time. Rows after the
  // slider's position are greyed, like features below a rollback bar.
  const selected = useHistoryView((s) => s.selectedChapter)
  const select = useHistoryView((s) => s.selectChapter)
  const viewing = useHistoryView((s) => s.step)
  // The sizes, refetched whenever the history changes. A failure leaves the
  // size column empty -- the rows and verdicts still stand.
  const [params, setParams] = useState<EditableParameter[]>([])
  useEffect(() => {
    let live = true
    fetchEditableParameters(branch)
      .then((p) => live && setParams(p))
      .catch(() => live && setParams([]))
    return () => {
      live = false
    }
  }, [branch, chapters])

  if (error && chapters.length === 0) {
    return <div className="px-3 py-2 text-[11px] text-destructive">history unavailable — {error}</div>
  }
  if (chapters.length === 0) {
    return (
      <div className="px-3 py-2 text-[11px] text-muted-foreground">
        {loading ? '⋯ loading history' : '∅ no features yet — build something and it appears here'}
      </div>
    )
  }
  return (
    <ul role="tree" aria-label="Part history" className="px-1 py-1">
      {chapters.map((c) => (
        <Row
          key={c.anchor}
          c={c}
          depth={0}
          selected={selected}
          onSelect={select}
          ahead={viewing !== null && c.anchor > viewing}
          params={params}
        />
      ))}
    </ul>
  )
}
