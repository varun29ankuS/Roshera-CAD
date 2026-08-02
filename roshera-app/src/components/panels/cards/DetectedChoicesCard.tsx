import { useContext, useState } from 'react'
import { ListChecks } from 'lucide-react'
import type { DetectedChoiceSet } from '@/lib/blackboard-cards'
import { useTurnQueue } from '@/lib/blackboard-composer'
import { CardShell, Chip } from './card-chrome'
import { CardActionsContext } from './card-actions-context'
import { ChoiceButtons } from './ChoiceButtons'

/** Button text is truncated for layout; the full option text is always on
 *  the button's `title` (hover). */
const LABEL_MAX = 88

function truncateLabel(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text
}

/**
 * DETECTED CHOICES CARD
 * ======================
 * `.goosehints` tells the agent to ask a closed-set question as a
 * `roshera:choices` fence; an agent that instead writes "Option A: … Option
 * B: …" as plain prose leaves the human to retype an answer. That
 * instruction is steering — ignorable — so this card is the constraint:
 * `lib/blackboard-cards.ts`'s `detectEnumeratedChoices` recognises the
 * agent's OWN labelled enumeration and `BlackboardLine.tsx` renders this
 * card underneath it, regardless of whether the fence was used.
 *
 * Deliberately narrower than `ChoicesCard`: there is no fence to rewrite
 * with `selected: <value>` on click, and this NEVER edits the owning line
 * or invents a fence — the agent's prose stays exactly as written. `chosen`
 * below is local, ephemeral render state for immediate feedback only; it is
 * not a persisted record (a reload shows the same buttons, live again),
 * which is the honest consequence of not touching the agent's line — see
 * the module doc on `sendDetectedChoice` (`card-actions-context.ts`).
 */
export function DetectedChoicesCard({ set }: { set: DetectedChoiceSet }) {
  const actions = useContext(CardActionsContext)
  const [chosen, setChosen] = useState<string | null>(null)
  const queue = useTurnQueue()
  // Same chip contract as ChoicesCard (see its module doc): colour moves
  // with the words through the states that actually exist — detected →
  // queued (waiting behind the head turn) → sent · in flight → sent
  // (settled). No "executed": whether the agent then DID the thing is the
  // turn's outcome, reported by TurnStatusGlyph on the turn's own line.
  // 0 = in flight, >0 = waiting, -1 = settled.
  const queueIndex = chosen !== null ? queue.findIndex((q) => q.text === chosen) : -1
  // Same rule as ChoicesCard: no CardActionsContext (the fixtures gallery)
  // means there is no line to answer — buttons render for preview but stay
  // inert.
  const clickable = actions !== null && chosen === null

  return (
    <CardShell
      accent={chosen !== null ? (queueIndex === -1 ? 'pass' : 'warn') : 'info'}
      icon={ListChecks}
      title="Options detected in the text above"
      chip={
        chosen === null ? (
          <Chip
            accent="info"
            dashed
            title="Added by the board from the agent's own 'Option A: / Option B: …' enumeration above — not a roshera:choices fence the agent authored"
          >
            detected, not fenced
          </Chip>
        ) : queueIndex > 0 ? (
          <Chip
            accent="warn"
            title="Your pick was accepted; its reply waits behind the turn already in flight"
          >
            queued
          </Chip>
        ) : queueIndex === 0 ? (
          <Chip
            accent="warn"
            title="Sent as your reply — its turn is in flight now; the turn's own status line says what the agent is doing with it"
          >
            sent · in flight
          </Chip>
        ) : (
          <Chip
            accent="pass"
            title="Sent as your reply; that turn has settled. How it ended is reported on the turn's own line, not here."
          >
            sent
          </Chip>
        )
      }
    >
      <ChoiceButtons
        options={set.options.map((opt) => {
          const full = `Option ${opt.label}: ${opt.text}`
          return {
            value: opt.text,
            label: truncateLabel(full, LABEL_MAX),
            title: full,
          }
        })}
        selected={chosen ?? undefined}
        clickable={clickable}
        onSelect={(value) => {
          setChosen(value)
          actions?.sendDetectedChoice(value)
        }}
      />
    </CardShell>
  )
}
