import { useContext } from 'react'
import { CircleHelp } from 'lucide-react'
import type { ChoicesCard as ChoicesCardData } from '@/lib/blackboard-cards'
import { CardShell, Chip } from './card-chrome'
import { CardActionsContext } from './card-actions-context'
import { ChoiceButtons } from './ChoiceButtons'

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
      <ChoiceButtons
        options={card.options}
        selected={card.selected}
        clickable={clickable}
        onSelect={(value) => actions?.selectChoice(source, value)}
      />
    </CardShell>
  )
}
