import { ArrowRight, Ban, CircleSlash, Lightbulb, RotateCw } from 'lucide-react'
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
 *
 * When the refusal rode the backend's structured error catalog
 * (`error_catalog.rs` via the MCP layer's `fail()` — see
 * `refusalCardSchema`), its typed fields are drawn too, each as visible
 * text (icon + word, never hover-only):
 *   - `error_code` — a mono chip, verbatim case (it is an identifier; the
 *     catalog's own doc names it the stable thing to branch on);
 *   - `retryable`  — icon + word. Absent renders NOTHING: the wire not
 *     saying is not the same as "not retryable";
 *   - `hint`       — the producer's own one-step guidance, labeled.
 */
export function RefusalCard({ card }: { card: RefusalCardData }) {
  return (
    <CardShell
      accent="info"
      icon={Ban}
      title={card.subject ? <>Refused — {card.subject}</> : 'Refused'}
      chip={
        <>
          {card.error_code !== undefined && (
            <Chip
              accent="info"
              title="Stable error-catalog code — the field to branch on; the prose reason is free to evolve"
            >
              <span className="normal-case">{card.error_code}</span>
            </Chip>
          )}
          {card.retryable !== undefined &&
            (card.retryable ? (
              <Chip
                accent="warn"
                title="The catalog marks this refusal retryable — the same call can succeed on a later attempt"
              >
                <RotateCw size={9} /> retryable
              </Chip>
            ) : (
              <Chip
                accent="neutral"
                title="The catalog marks this refusal not retryable — re-issuing the identical call will refuse again; change the design, the threshold, or escalate"
              >
                <CircleSlash size={9} /> not retryable
              </Chip>
            ))}
          <Chip
            accent="info"
            title="Honest refusal over a silent wrong answer — the operation is outside the verified envelope"
          >
            <CircleSlash size={9} /> typed refusal{card.source ? ` · ${card.source}` : ''}
          </Chip>
        </>
      }
    >
      <div className="mt-1.5 border-l-2 border-sky-500/30 pl-2 text-[11px] leading-relaxed text-foreground/90">
        {card.reason}
      </div>
      {card.hint !== undefined && (
        <div className="mt-1.5 flex items-start gap-1.5 text-[11px] leading-relaxed text-foreground/80">
          <Lightbulb size={10} className="mt-0.5 shrink-0 text-sky-400/70" />
          <span>
            <span className="mr-1.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
              hint
            </span>
            {card.hint}
          </span>
        </div>
      )}
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
