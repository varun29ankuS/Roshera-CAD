import { cardKindFromLanguage, parseCard, type BlackboardCard } from './blackboard-cards'

/**
 * LINE VERDICT — the fixed-gutter marker's colour input
 * =======================================================
 * One Blackboard line can carry a verdict — a certificate that came out
 * proven, violated, or the kernel's own honest "cannot decide" — and that
 * fact should be legible from the marker column alone, no hover required.
 * This module derives that verdict from the line's OWN fenced `roshera:*`
 * cards (the same parser `blackboard-cards.ts` already validates against
 * the real wire shapes) — never from prose, never invented.
 *
 * Tri-state, deliberately not boolean: `pass` / `fail` / `inconclusive` /
 * `null`. `inconclusive` covers everything the kernel could not decide —
 * DFM `unverifiable`, GD&T `not_evaluable`, a soundness certificate with no
 * overall `sound` bit, a typed refusal — and must never collapse into
 * `pass` (that is exactly the dishonesty the DFM policy forbids) or `fail`
 * (a check that did not run did not fail either). `null` means the line
 * carries no verdict at all: no card, a card that failed schema validation,
 * an FCF frame that is design intent with no kernel verdict attached yet,
 * or a still-open `roshera:choices` question — the marker stays neutral,
 * not decorated with a fabricated state.
 *
 * A line can carry more than one fenced card (rare, but the fence grammar
 * allows it). Verdicts combine by severity — fail beats inconclusive beats
 * pass — so a line can never read clean because one card passed while
 * another, further down the same line, failed.
 */

export type LineVerdict = 'pass' | 'fail' | 'inconclusive'

/** Matches a closed ```roshera:<kind>\n...\n``` fence, capturing the
 *  language token and the raw body between the delimiters — the same fence
 *  grammar `blackboard-content.ts`'s `FENCE_OPEN_RE` recognises the opener
 *  of, extended to also capture the body `parseCard` needs. An unclosed
 *  fence (still streaming in) simply does not match yet, same as the real
 *  card renderer. */
const FENCE_RE = /```\s*([^\s`\n]+)\n([\s\S]*?)```/g

function cardsInText(text: string): BlackboardCard[] {
  const cards: BlackboardCard[] = []
  for (const match of text.matchAll(FENCE_RE)) {
    const kind = cardKindFromLanguage(match[1])
    if (kind === null) continue
    const result = parseCard(kind, match[2].replace(/\n$/, ''))
    if (result.ok) cards.push(result.card)
  }
  return cards
}

/** One card's verdict, read only from the fields the kernel/API actually
 *  populate — never re-derived from a summary string. */
function cardVerdict(card: BlackboardCard): LineVerdict | null {
  switch (card.kind) {
    case 'soundness': {
      const sound = card.card.sound
      if (sound === true) return 'pass'
      if (sound === false) return 'fail'
      // `sound` absent/null: the full certificate did not run — not-run is
      // not a pass (the exact rule `SoundnessCard.tsx` already renders).
      return 'inconclusive'
    }
    case 'dfm': {
      const kind = card.card.verdict.kind
      if (kind === 'pass') return 'pass'
      if (kind === 'violation') return 'fail'
      return 'inconclusive' // kind === 'unverifiable'
    }
    case 'fcf': {
      const conforms = card.card.verdict?.conforms
      // No verdict attached at all → design intent awaiting evaluation,
      // not a state to colour the marker with.
      if (conforms === undefined) return null
      if (conforms === 'in_spec') return 'pass'
      if (conforms === 'out_of_spec') return 'fail'
      return 'inconclusive' // conforms === 'not_evaluable'
    }
    case 'refusal':
      // A typed refusal is a result the kernel could not decide inside its
      // verified envelope — the same "honest can't-say" bucket as
      // unverifiable/not-evaluable, never styled as a pass or a fail.
      return 'inconclusive'
    case 'merge':
      // Any conflict — even on an otherwise `success: true` fold — means
      // the merge needs a human decision, not a clean bill.
      if (card.card.conflicts.length > 0) return 'inconclusive'
      return card.card.success ? 'pass' : 'fail'
    case 'choices':
      // A question, not a verdict — open or answered, it has no pass/fail
      // state of its own.
      return null
  }
}

const SEVERITY: Record<LineVerdict, number> = { fail: 3, inconclusive: 2, pass: 1 }

/** Derive a Blackboard line's verdict marker state from its own fenced
 *  `roshera:*` cards. `null` means render with no colour at all. */
export function lineVerdict(text: string): LineVerdict | null {
  let best: LineVerdict | null = null
  for (const card of cardsInText(text)) {
    const v = cardVerdict(card)
    if (v !== null && (best === null || SEVERITY[v] > SEVERITY[best])) best = v
  }
  return best
}
