import { Fragment, useState } from 'react'
import {
  ChevronDown,
  ChevronRight,
} from '@/components/ui/icons'
import { cn } from '@/lib/utils'
import type { BlackboardSection as BlackboardSectionData } from '@/lib/blackboard-groups'
import { BlackboardLine } from './BlackboardLine'
import type { TurnEvidence } from '@/lib/turn-evidence'
import { BuildStepStrip } from './BuildStepStrip'
import { TurnEvidenceStrip } from './TurnEvidenceStrip'

/**
 * One notebook section: a sticky, collapsible header naming the checkpoint
 * that was open when its lines were written, followed by those lines
 * (still run through `groupBlackboardLines`'s own build-strip collapsing —
 * see `blackboard-groups.ts`'s `groupBlackboardByCheckpoint` doc). `null`
 * checkpoint = the unlabelled leading section; it renders with no header
 * at all rather than an invented name.
 *
 * Collapsing is purely a render decision — the section's `groups` (and the
 * lines inside them) are untouched either way, so toggling can never
 * reorder, alter, or drop board content.
 */
export function BlackboardSection({
  section,
  onCommit,
  onDelete,
  streamingLineId,
  onCancel,
  evidence,
  defaultCollapsed = false,
}: {
  section: BlackboardSectionData
  onCommit: (id: string, text: string) => void
  onDelete: (id: string) => void
  streamingLineId: string | null
  onCancel?: (() => void) | undefined
  /** Each turn's kernel evidence, keyed by the turn's last line
   *  (`lib/turn-evidence.ts`'s `evidenceByLastLine`, computed once over the
   *  whole notebook by `Blackboard.tsx`). */
  evidence: ReadonlyMap<string, TurnEvidence>
  /** Initial state only (Blackboard.tsx passes true for every checkpoint
   *  section except the last, so a long notebook opens on CURRENT work).
   *  The user's toggle always wins afterwards — this never forces a
   *  section shut on re-render, and the leading unlabelled section (no
   *  header, no toggle) is never collapsed by the caller. */
  defaultCollapsed?: boolean
}) {
  const [collapsed, setCollapsed] = useState(defaultCollapsed)
  const { checkpoint, groups } = section

  // How many real lines this section holds (a build strip stands for each
  // of its underlying lines). Shown in the header whether open or closed:
  // a collapsed section must say how much it holds without a hover — the
  // count is the "nothing is hidden" receipt.
  const lineCount = groups.reduce(
    (n, g) => n + (g.kind === 'build-strip' ? g.lines.length : 1),
    0,
  )

  // The turn that ends on this group, if any: its evidence strip is drawn
  // right after it. A build strip can be a turn's last group (kernel echoes
  // arrive after the reply), so every line it stands for is checked.
  const turnEndingAt = (g: (typeof groups)[number]): TurnEvidence | undefined => {
    if (g.kind === 'line') return evidence.get(g.line.id)
    for (const l of g.lines) {
      const t = evidence.get(l.id)
      if (t) return t
    }
    return undefined
  }

  const body = collapsed ? null : (
    <div>
      {groups.map((g) => {
        const turn = turnEndingAt(g)
        const key = g.kind === 'build-strip' ? g.lines[0].id : g.line.id
        return (
          <Fragment key={key}>
            {g.kind === 'build-strip' ? (
              <BuildStepStrip lines={g.lines} onCommit={onCommit} onDelete={onDelete} />
            ) : (
              <BlackboardLine
                line={g.line}
                onCommit={onCommit}
                onDelete={onDelete}
                streaming={g.line.id === streamingLineId}
                onCancel={g.line.id === streamingLineId ? onCancel : undefined}
              />
            )}
            {turn && <TurnEvidenceStrip evidence={turn} />}
          </Fragment>
        )
      })}
    </div>
  )

  // The leading (unlabelled) section carries no checkpoint name and no
  // header chrome — it's the notebook's un-sectioned prologue, not a
  // feature with an invented title.
  if (!checkpoint) {
    return body
  }

  return (
    <div>
      <button
        type="button"
        onClick={() => setCollapsed((v) => !v)}
        title={checkpoint.name}
        aria-expanded={!collapsed}
        className={cn(
          'sticky top-0 z-10 flex w-full items-center gap-1.5 px-3 py-1',
          'bg-card border-b border-border',
          'text-left text-[11px] font-medium text-foreground/80 hover:text-foreground transition-colors',
        )}
      >
        {collapsed ? (
          <ChevronRight size={11} className="shrink-0 text-muted-foreground/60" />
        ) : (
          <ChevronDown size={11} className="shrink-0 text-muted-foreground/60" />
        )}
        <span className="truncate">{checkpoint.name}</span>
        <span className="ml-1 shrink-0 font-mono text-[11px] tabular-nums text-muted-foreground/50">
          {lineCount} {lineCount === 1 ? 'line' : 'lines'}
        </span>
      </button>
      {body}
    </div>
  )
}
