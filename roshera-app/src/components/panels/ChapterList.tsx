/**
 * The chapter list: the part's history read like a CAD feature tree -- one
 * row per engineering feature, in build order, each with the icon of the tool
 * that made it, its name, and the kernel's verdict (✓ sound / ✗ with the
 * kernel's own reason). A pattern opens to its instances. Rows are chapters
 * grouped by the kernel from its recorded steps (GET /api/timeline/chapters).
 */
import { useState } from 'react'
import { cn } from '@/lib/utils'
import { chapterIconKey, chapterTitle, type Chapter } from '@/lib/chapters-api'
import { useChapters } from '@/hooks/use-chapters'
import { Bot, Box, Check, ChevronDown, ChevronRight, User, X, ToolIcons } from '@/components/ui/icons'

function ChapterIcon({ c, size }: { c: Chapter; size: number }) {
  const key = chapterIconKey(c.kind)
  const Icon = key ? ToolIcons[key] : Box
  return <Icon size={size} />
}

function Verdict({ c }: { c: Chapter }) {
  if (c.sound) return <Check size={14} title="sound — the kernel re-measured every step" />
  const why = c.refusals.map((r) => `step ${r.sequence}: ${r.status}${r.reason ? ` — ${r.reason}` : ''}`).join('\n')
  return <X size={14} title={why || 'not sound'} />
}

function Byline({ c }: { c: Chapter }) {
  if (c.author === 'agent') {
    return (
      <span className="inline-flex items-center gap-1">
        <Bot size={12} />
        {c.author_detail.model ?? 'agent'}
        {c.intent ? <span className="truncate"> · {c.intent}</span> : null}
      </span>
    )
  }
  if (c.author === 'user') {
    return (
      <span className="inline-flex items-center gap-1">
        <User size={12} />
        {c.author_detail.name ?? 'you'}
      </span>
    )
  }
  return null
}

function Row({
  c,
  depth,
  selected,
  onSelect,
}: {
  c: Chapter
  depth: number
  selected: number | null
  onSelect: (anchor: number) => void
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
        className={cn(
          'flex items-start gap-2 rounded px-1.5 py-1 cursor-default outline-none',
          'hover:bg-accent/40 focus-visible:ring-1 focus-visible:ring-ring',
          selected === c.anchor && 'bg-accent/60',
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
            className="mt-1 shrink-0"
          >
            {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </button>
        ) : (
          <span className="w-3 shrink-0" />
        )}
        <ChapterIcon c={c} size={22} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12px] leading-5 text-foreground">{chapterTitle(c)}</div>
          <div className="truncate text-[11px] leading-4 text-muted-foreground">
            <Byline c={c} />
          </div>
          {!c.sound && c.refusals[0]?.reason ? (
            <div className="text-[11px] leading-4 text-destructive">{c.refusals[0].reason}</div>
          ) : null}
        </div>
        <span className="mt-1 shrink-0">
          <Verdict c={c} />
        </span>
      </div>
      {isPattern && open ? (
        <ul role="group">
          {c.instances.map((i) => (
            <Row key={i.anchor} c={i} depth={depth + 1} selected={selected} onSelect={onSelect} />
          ))}
        </ul>
      ) : null}
    </li>
  )
}

export function ChapterList({ branch = 'main' }: { branch?: string }) {
  const { chapters, loading, error } = useChapters(branch)
  const [selected, setSelected] = useState<number | null>(null)

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
        <Row key={c.anchor} c={c} depth={0} selected={selected} onSelect={setSelected} />
      ))}
    </ul>
  )
}
