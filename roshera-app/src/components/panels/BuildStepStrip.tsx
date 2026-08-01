import { useState } from 'react'
import { ChevronDown, ChevronRight, Wrench } from 'lucide-react'
import type { BlackboardLine as Line } from '@/stores/blackboard-store'
import { buildStepBreadcrumb, groupBuildStepsByName } from '@/lib/blackboard-groups'
import { BlackboardLine } from './BlackboardLine'

/**
 * BUILD STEP STRIP
 * ================
 * Consecutive machine-authored "Created …" lines (`lib/blackboard-groups.ts`'s
 * `isBuildStepLine` — the kernel's own per-operation bookkeeping, one line
 * per solid/boolean op) collapse into ONE row: a breadcrumb of the operation
 * NAMES, e.g. `bore ×4 · Difference ×4 — 9 steps`, read straight off each
 * line's own "Created **name** — …" text
 * (`lib/blackboard-groups.ts`'s `groupBuildStepsByName`).
 *
 * A first cut of this strip (Varun, 2026-08-01) rendered a numbered tick
 * per step — "✓1 ✓2 … ✓9" — which is strictly worse than the nine lines it
 * replaced: an ordinal indexes nothing a user can act on, and reading it
 * required a hover. The fix keeps the same "one mark per real operation,
 * never summarise" rule but spends the row's width on NAMES instead of
 * counters: consecutive lines that share an operation name fold to
 * `name ×N` — the same names with repetition folded, never coerced
 * together if the names actually differ, and never a fabricated summary
 * like "4 bores" standing in for text the lines don't literally contain.
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
  const segments = groupBuildStepsByName(lines)
  const { full, display } = buildStepBreadcrumb(segments)

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
          className="flex w-full items-center gap-1.5 text-left"
          title={`${full} — ${lines.length} steps — click to expand`}
          aria-label={`${full} — ${lines.length} steps — click to expand`}
        >
          <ChevronRight size={11} className="shrink-0 text-muted-foreground/60" />
          <span className="min-w-0 truncate text-[11px] text-foreground/85">{display}</span>
          <span className="shrink-0 text-[10px] text-muted-foreground/60">
            — {lines.length} steps
          </span>
        </button>
      </div>
    </div>
  )
}
