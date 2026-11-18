import { Check, CircleSlash, ShieldCheck, X } from '@/components/ui/icons'
import { cn } from '@/lib/utils'
import type { LineVerdict } from '@/lib/blackboard-line-state'
import type { EvidenceMark, TurnEvidence } from '@/lib/turn-evidence'

/**
 * TURN EVIDENCE STRIP
 * ===================
 * Drawn after the last line of every agent turn that carries kernel
 * evidence or built geometry (`lib/turn-evidence.ts`): the kernel's answer
 * to the turn, readable without reading the turn.
 *
 *   ─ Kernel evidence  ✓ sound  ✗ GD&T out of spec  ⊘ refused
 *
 * One glyph + word per verdict-carrying card, in the order written, in the
 * same emerald / red / amber vocabulary `cards/card-chrome.tsx`'s `Claim`
 * uses. The left rule takes the turn's WORST verdict, so a turn whose last
 * certificate failed can never read clean. No counts: the words are the
 * record, and numbers live in the status bar.
 *
 * Each mark is a button that brings its card's own line into view, so the
 * evidence opens where it was written rather than being copied here.
 * No background: the strip is a rule and a row of words on the board, not
 * another surface over the viewport.
 */

const RULE: Record<LineVerdict, string> = {
  pass: 'border-l-emerald-500/70',
  fail: 'border-l-red-500/80',
  inconclusive: 'border-l-caution-border',
}

const MARK: Record<LineVerdict, string> = {
  pass: 'text-emerald-700 dark:text-emerald-300',
  fail: 'text-red-700 dark:text-red-300 font-medium',
  inconclusive: 'text-caution',
}

function MarkGlyph({ verdict }: { verdict: LineVerdict }) {
  if (verdict === 'pass') return <Check size={11} className="shrink-0" aria-hidden />
  if (verdict === 'fail') return <X size={11} className="shrink-0" aria-hidden />
  return <CircleSlash size={11} className="shrink-0" aria-hidden />
}

const VERDICT_WORD: Record<LineVerdict, string> = {
  pass: 'certified',
  fail: 'failed',
  inconclusive: 'not decided',
}

/** Bring a line into view: the line itself, or — when it is folded into a
 *  collapsed build strip — the strip standing in for it. Ids are
 *  store-minted, but escaped anyway: the selector must never be built from
 *  text it cannot vouch for. */
function revealLine(lineId: string): void {
  const id = CSS.escape(lineId)
  const el =
    document.querySelector<HTMLElement>(`[data-line-id="${id}"]`) ??
    document.querySelector<HTMLElement>(`[data-line-ids~="${id}"]`)
  el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
}

function Mark({ mark }: { mark: EvidenceMark }) {
  return (
    <button
      type="button"
      onClick={() => revealLine(mark.lineId)}
      className={cn(
        'cad-focus inline-flex items-center gap-1 rounded px-1 py-[1px] leading-none hover:bg-accent',
        MARK[mark.verdict],
      )}
      title={mark.source === 'uncertified' ? 'Geometry was built this turn, but no soundness certificate reached the board. Show the last build step.' : 'Show this evidence'}
    >
      <MarkGlyph verdict={mark.verdict} />
      {mark.word}
    </button>
  )
}

export function TurnEvidenceStrip({ evidence }: { evidence: TurnEvidence }) {
  if (evidence.verdict === null) return null
  const summary = evidence.marks.map((m) => m.word).join(', ')
  return (
    <div
      role="group"
      aria-label={`Kernel evidence for this turn, ${VERDICT_WORD[evidence.verdict]}: ${summary}`}
      className={cn(
        'ml-9 mr-3 mb-1.5 mt-0.5 flex flex-wrap items-center gap-x-1 gap-y-1 border-l-2 pl-2 text-[11px]',
        RULE[evidence.verdict],
      )}
    >
      <span className="mr-1 inline-flex shrink-0 items-center gap-1 text-muted-foreground">
        <ShieldCheck size={11} className="shrink-0" aria-hidden />
        Kernel evidence
      </span>
      {evidence.marks.map((m, i) => (
        <Mark key={`${m.lineId}-${i}`} mark={m} />
      ))}
    </div>
  )
}
