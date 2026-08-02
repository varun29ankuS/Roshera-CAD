import { useContext } from 'react'
import { CircleHelp } from 'lucide-react'
import type { ChoicesCard as ChoicesCardData } from '@/lib/blackboard-cards'
import { useTurnQueue } from '@/lib/blackboard-composer'
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
 * # The chip tracks the states that actually exist — no more, no fewer
 * A click does two observable things, in order: it RECORDS the choice
 * (`selected:` written into the fence — durable, survives reload) and it
 * DISPATCHES the value as a turn through the same queue as a typed prompt
 * (`lib/blackboard-composer.ts`'s visible turn queue: head = in flight,
 * rest = waiting). So the chip has exactly four honest states:
 *
 *   awaiting your choice → recorded · queued → recorded · sent → answered
 *   (sky, dashed)          (amber)             (amber)           (emerald)
 *
 * and colour moves WITH the words at every step — a chip whose colour
 * changes while its words do not is the worst of both (Varun, 2026-08-02).
 * Deliberately NO "executed": we can observe that the reply was recorded
 * and that its turn was dispatched, but whether the agent then *did* the
 * thing is the turn's outcome — `TurnStatusGlyph` reports that on the
 * turn's own line. Claiming "executed" from a successful dispatch would be
 * a fabricated completion. On reload an answered card shows "answered"
 * (the durable fact); the transient queue states belong to the session
 * that clicked.
 *
 * The queue match is by prompt text (`q.text === card.selected`) — the
 * queue entry `selectChoice`'s dispatch created carries the option value
 * verbatim, so while such an entry exists, "a turn with this reply is
 * queued/in flight" is a true statement about the transport.
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
  const queue = useTurnQueue()
  const answered = card.selected !== undefined
  // Position of this answer's dispatched turn in the visible queue:
  // 0 = in flight, >0 = waiting behind the head, -1 = settled (or another
  // session's click — same rendering either way: the durable record).
  const queueIndex = answered ? queue.findIndex((q) => q.text === card.selected) : -1
  const inTransit = answered && queueIndex !== -1
  // No CardActionsContext (e.g. the fixtures gallery) means there is no
  // line to answer — buttons render for preview but stay inert rather than
  // silently doing nothing that looks like it did something.
  const clickable = actions !== null && !answered

  return (
    <CardShell
      accent={answered ? (inTransit ? 'warn' : 'pass') : 'info'}
      icon={CircleHelp}
      title={card.question}
      chip={
        !answered ? (
          <Chip accent="info" dashed title="Click an option to send it as your reply">
            awaiting your choice
          </Chip>
        ) : queueIndex > 0 ? (
          <Chip
            accent="warn"
            title="Your choice is recorded on the board; its reply waits behind the turn already in flight"
          >
            recorded · queued
          </Chip>
        ) : queueIndex === 0 ? (
          <Chip
            accent="warn"
            title="Your choice is recorded on the board and its reply's turn is in flight now — the turn's own status line says what the agent is doing with it"
          >
            recorded · sent
          </Chip>
        ) : (
          <Chip
            accent="pass"
            title="Answered — recorded on the board, not still open. How the turn it triggered ended is reported on that turn's own line, not here."
          >
            answered
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
