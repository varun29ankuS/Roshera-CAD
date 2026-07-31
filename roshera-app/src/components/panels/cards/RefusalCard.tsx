import { ArrowRight, Ban } from 'lucide-react'
import type { RefusalCard as RefusalCardData } from '@/lib/blackboard-cards'
import { CardShell, Chip } from './card-chrome'

/**
 * TYPED REFUSAL CARD
 * ==================
 * A refusal is a RESULT, not an error: the kernel declining to answer
 * outside its verified envelope is the product working. So this card is calm
 * (sky accent, never destructive red), names its reason VERBATIM (the
 * backend message is never paraphrased — the gdt.ts rule), and, where the
 * agent can offer them, lists the next actions that would make the operation
 * answerable.
 */
export function RefusalCard({ card }: { card: RefusalCardData }) {
  return (
    <CardShell
      accent="info"
      icon={Ban}
      title={card.subject ? <>Refused — {card.subject}</> : 'Refused'}
      chip={
        <Chip
          accent="info"
          title="Honest refusal over a silent wrong answer — the operation is outside the verified envelope"
        >
          typed refusal{card.source ? ` · ${card.source}` : ''}
        </Chip>
      }
    >
      <div className="mt-1.5 border-l-2 border-sky-500/30 pl-2 text-[11px] leading-relaxed text-foreground/90">
        {card.reason}
      </div>
      {card.options !== undefined && card.options.length > 0 && (
        <div className="mt-1.5 space-y-0.5">
          <div className="text-[10px] font-mono uppercase tracking-wide text-muted-foreground">
            next
          </div>
          {card.options.map((opt, i) => (
            <div key={i} className="flex items-start gap-1.5 text-[11px] text-foreground/80">
              <ArrowRight size={10} className="mt-0.5 shrink-0 text-sky-400/70" />
              <span>{opt}</span>
            </div>
          ))}
        </div>
      )}
    </CardShell>
  )
}
