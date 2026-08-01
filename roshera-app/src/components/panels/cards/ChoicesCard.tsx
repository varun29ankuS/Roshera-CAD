import { useContext } from 'react'
import { Check, CircleHelp } from 'lucide-react'
import type { ChoicesCard as ChoicesCardData } from '@/lib/blackboard-cards'
import { cn } from '@/lib/utils'
import { CardShell, Chip } from './card-chrome'
import { CardActionsContext } from './card-actions-context'

/**
 * CHOICES CARD
 * ============
 * The `roshera:choices` fence (an authoring convention, not a kernel wire
 * type — see `lib/blackboard-cards.ts`): a genuinely closed-set question
 * rendered as clickable option buttons instead of prose the user has to
 * retype. `label` is the button text, `detail` a dim secondary line beneath
 * it, `value` what gets sent verbatim as the next turn.
 *
 * Once answered (`card.selected` set — a value the UI writes into the
 * line's own text, never the agent), the card renders as a closed record:
 * the chosen option is marked, every other option is disabled. This is not
 * local component state — it survives reload because it lives in the
 * fenced source itself, the same way every other edit to a Blackboard line
 * does.
 *
 * `source` is the exact fence body this card was parsed from, needed only
 * to hand back to `CardActionsContext.selectChoice` so the owning line can
 * locate (and only) rewrite this fence, not guess at one.
 *
 * Clickability depends ONLY on this card's own `answered` state — NEVER on
 * whether some OTHER turn is in flight. Two live choices cards must both
 * stay independently answerable until each is individually answered
 * (Varun, 2026-08-01): gating on the store's global `isProcessing` used to
 * mean answering the first card disabled every other still-open card for
 * the full 60–90s of the turn that answer triggered. `selectChoice`
 * (`BlackboardLine.tsx`) now queues the resulting turn instead
 * (`lib/ai-client.ts`'s `turnQueue`) rather than this card ever refusing
 * the click.
 */
export function ChoicesCard({ card, source }: { card: ChoicesCardData; source: string }) {
  const actions = useContext(CardActionsContext)
  const answered = card.selected !== undefined
  // No CardActionsContext (e.g. the fixtures gallery) means there is no
  // line to answer — buttons render for preview but stay inert rather than
  // silently doing nothing that looks like it did something.
  const clickable = actions !== null && !answered

  return (
    <CardShell
      accent={answered ? 'pass' : 'info'}
      icon={CircleHelp}
      title={card.question}
      chip={
        answered ? (
          <Chip accent="pass" title="Answered — recorded on the board, not still open">
            answered
          </Chip>
        ) : (
          <Chip accent="info" dashed title="Click an option to send it as your reply">
            awaiting your choice
          </Chip>
        )
      }
    >
      <div className="mt-1.5 flex flex-col gap-1">
        {card.options.map((opt) => {
          const chosen = card.selected === opt.value
          return (
            <button
              key={opt.value}
              type="button"
              disabled={!clickable}
              onClick={(e) => {
                // The committed line renders inside a "click to edit" button
                // (BlackboardLine.tsx) — without this, clicking an option
                // would bubble up and drop the line into raw-text edit mode
                // instead of sending the choice.
                e.stopPropagation()
                actions?.selectChoice(source, opt.value)
              }}
              className={cn(
                'flex flex-col items-start gap-0.5 rounded-md border px-2.5 py-1.5 text-left text-[11px] leading-snug transition-colors',
                'disabled:cursor-not-allowed disabled:hover:border-border/40 disabled:hover:bg-transparent',
                chosen && 'border-emerald-500/60 bg-emerald-500/10 text-foreground',
                // Not-clickable covers two cases with one look: another
                // option on THIS card was chosen, or (the fixtures gallery)
                // there is no owning line to answer at all — neither should
                // look pressable. An in-flight turn elsewhere is NOT one of
                // these cases — see the module doc above.
                !chosen && !clickable && 'border-border/40 text-muted-foreground/50 opacity-60',
                !chosen &&
                  clickable &&
                  'border-border/70 text-foreground/90 hover:border-primary/50 hover:bg-accent/40 cursor-pointer',
              )}
            >
              <span className="flex items-center gap-1.5 font-medium">
                {chosen && <Check size={11} className="shrink-0 text-emerald-600 dark:text-emerald-400" />}
                {opt.label}
              </span>
              {opt.detail && (
                <span className="text-[10px] text-muted-foreground/80">{opt.detail}</span>
              )}
            </button>
          )
        })}
      </div>
    </CardShell>
  )
}
