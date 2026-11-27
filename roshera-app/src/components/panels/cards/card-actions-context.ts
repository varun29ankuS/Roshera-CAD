import { createContext } from 'react'

/**
 * Actions a typed card can trigger against its OWN Blackboard line: the
 * Choices cards' answers and the Concept card's build request. Kept as a
 * context (rather than threading callbacks through `MessageMarkdown` /
 * `CardRenderer` props) because only these cards act; every other card is a
 * pure, read-only render of a wire shape.
 *
 * Provided by the owning `BlackboardLine` (which knows the line's id and raw
 * text); absent in read-only contexts such as the fixtures gallery, where a
 * Choices card still renders its buttons but clicking is inert — there is no
 * line to answer.
 */
export interface CardActions {
  /**
   * A Choices card option was clicked. `rawSource` is the EXACT `roshera:choices`
   * fence body the card was rendered from (used to locate that fence, and
   * only that fence, inside the owning line's text — never a fuzzy match).
   * `value` is the clicked option's `value`, sent verbatim as the next turn.
   */
  selectChoice: (rawSource: string, value: string) => void
  /**
   * A DetectedChoicesCard option was clicked — an "Option A: …" enumeration
   * the agent wrote as prose, not a `roshera:choices` fence (see
   * `lib/blackboard-cards.ts`'s `detectEnumeratedChoices`). Unlike
   * `selectChoice`, there is no fence to mark `selected:` on, so this never
   * rewrites the owning line — it only sends `value` (the option's own
   * text) as the next turn, exactly as if it had been typed.
   */
  sendDetectedChoice: (value: string) => void
  /**
   * A Concept card's "Build this one" was clicked. `request` is the turn text
   * (`lib/concept-build.ts` `buildRequestText`): it is sent as the user's
   * next turn, exactly as if typed, and the AGENT builds the concept through
   * the kernel and certifies it. The app never makes geometry from a concept.
   */
  requestBuild: (request: string) => void
}

export const CardActionsContext = createContext<CardActions | null>(null)
