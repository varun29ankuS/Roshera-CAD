import { useState } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { BlackboardSection as BlackboardSectionData } from '@/lib/blackboard-groups'
import { BlackboardLine } from './BlackboardLine'
import { BuildStepStrip } from './BuildStepStrip'

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
}: {
  section: BlackboardSectionData
  onCommit: (id: string, text: string) => void
  onDelete: (id: string) => void
  streamingLineId: string | null
  onCancel?: (() => void) | undefined
}) {
  const [collapsed, setCollapsed] = useState(false)
  const { checkpoint, groups } = section

  const body = collapsed ? null : (
    <div>
      {groups.map((g) =>
        g.kind === 'build-strip' ? (
          <BuildStepStrip
            key={g.lines[0].id}
            lines={g.lines}
            onCommit={onCommit}
            onDelete={onDelete}
          />
        ) : (
          <BlackboardLine
            key={g.line.id}
            line={g.line}
            onCommit={onCommit}
            onDelete={onDelete}
            streaming={g.line.id === streamingLineId}
            onCancel={g.line.id === streamingLineId ? onCancel : undefined}
          />
        ),
      )}
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
          'bg-background/80 backdrop-blur-sm border-b border-white/5',
          'text-left text-[11px] font-medium text-foreground/80 hover:text-foreground transition-colors',
        )}
      >
        {collapsed ? (
          <ChevronRight size={11} className="shrink-0 text-muted-foreground/60" />
        ) : (
          <ChevronDown size={11} className="shrink-0 text-muted-foreground/60" />
        )}
        <span className="truncate">{checkpoint.name}</span>
      </button>
      {body}
    </div>
  )
}
