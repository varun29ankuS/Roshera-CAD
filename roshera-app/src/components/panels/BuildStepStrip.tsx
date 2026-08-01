import { useState } from 'react'
import { ChevronDown, ChevronRight, Wrench } from 'lucide-react'
import type { BlackboardLine as Line } from '@/stores/blackboard-store'
import { ClaimBadge } from './cards/card-chrome'
import { BlackboardLine } from './BlackboardLine'

/**
 * BUILD STEP STRIP
 * ================
 * Consecutive machine-authored "Created …" lines (`lib/blackboard-groups.ts`'s
 * `isBuildStepLine` — the kernel's own per-operation bookkeeping, one line
 * per solid/boolean op) collapse into ONE row: a small numbered `ClaimBadge`
 * per step, in order, reusing the exact glyph vocabulary `cards/card-chrome.tsx`
 * already uses for every certificate in the app — a step reads the same as
 * everything else on the board. The full original line (name, dimensions,
 * triangle count) is that badge's hover text, VERBATIM — never paraphrased
 * or reduced to a count (Varun, 2026-08-01: nine lines of bookkeeping for
 * one bolt circle buried the engineering the agent actually wrote, and "4
 * bores" would be exactly the fabricated summary this product refuses to
 * show — one mark per real operation, always).
 *
 * Collapsed by default. Expanding swaps the strip for the REAL
 * `BlackboardLine`s it stands for — same component, same edit/delete
 * affordances — nothing here ever deletes or rewrites the underlying lines;
 * this is a render-time grouping only (`groupBlackboardLines`), so the
 * board stays the record it is everywhere else.
 */
export function BuildStepStrip({
  lines,
  onCommit,
  onDelete,
}: {
  lines: Line[]
  onCommit: (id: string, text: string) => void
  onDelete: (id: string) => void
}) {
  const [expanded, setExpanded] = useState(false)

  if (expanded) {
    return (
      <div>
        <button
          type="button"
          onClick={() => setExpanded(false)}
          className="cad-icon-btn ml-3 mt-1 h-5 gap-1 px-1.5 text-[10px] text-muted-foreground/70"
          title="Collapse back into one row"
        >
          <ChevronDown size={10} />
          collapse {lines.length} steps
        </button>
        {lines.map((line) => (
          <BlackboardLine key={line.id} line={line} onCommit={onCommit} onDelete={onDelete} />
        ))}
      </div>
    )
  }

  return (
    <div className="group/line flex items-start gap-2 px-3 py-1.5 hover:bg-white/[0.03] rounded-md">
      {/* Same origin marker BlackboardLine uses for a system-authored line —
          a step strip is still app-generated bookkeeping, not the agent's
          own prose. */}
      <div
        className="mt-1 flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-muted-foreground/20"
        title="App-generated"
      >
        <Wrench size={9} className="text-muted-foreground" />
      </div>
      <div className="min-w-0 flex-1">
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className="flex w-full flex-wrap items-center gap-1 text-left"
          title={`${lines.length} build steps — click to expand`}
        >
          <ChevronRight size={11} className="shrink-0 text-muted-foreground/60" />
          {lines.map((line, i) => (
            // status=true: each mark records a step that completed — there
            // is no failure state in this fence (a failed op posts its own,
            // differently-worded system line and is never grouped here, see
            // `isBuildStepLine`). `detail` is the line's exact original
            // text — the ClaimBadge convention already puts it verbatim on
            // both `title` and `aria-label`.
            <ClaimBadge key={line.id} status={true} label={String(i + 1)} detail={line.text} />
          ))}
          <span className="ml-1 text-[10px] text-muted-foreground/60">
            {lines.length} build steps
          </span>
        </button>
      </div>
    </div>
  )
}
