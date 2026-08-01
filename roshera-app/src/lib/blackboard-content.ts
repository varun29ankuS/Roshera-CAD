import type { BlackboardLine } from '@/stores/blackboard-store'
import { cardKindFromLanguage, detectEnumeratedChoices, type CardKind } from './blackboard-cards'
import { isBuildStepLine } from './blackboard-groups'

/**
 * CONTENT CLASS
 * =============
 * `BlackboardLine.tsx` used to render every line through one primitive — a
 * `<button onClick={() => setEditing(true)}>` — regardless of whether the
 * text was agent prose, the user's own words, or a verbatim kernel payload
 * (a soundness certificate, a DFM verdict, a typed refusal). That made the
 * kernel's own testimony an editable text box: click in, rewrite what it
 * said, and the edit persists looking authored. In a product whose claim is
 * that the kernel cannot lie, that is the bug.
 *
 * This module classifies a line into one of three content classes so the
 * renderer can give each a genuinely different shape AND, for `evidence`,
 * remove the edit path entirely rather than merely disable it.
 *
 *   - `evidence`  — machine-authored or verbatim-forwarded. Never editable.
 *   - `control`   — a closed-set question (fenced or detected). Editable,
 *                   same as today.
 *   - `reasoning` — agent prose and the user's own writing. Editable, same
 *                   as today. The default/fallback class.
 *
 * Precedence is deliberate: a line mixing prose AND a certificate fence is
 * `evidence` — the strict class wins, because the reason to forbid editing
 * is the payload it carries, not how much prose surrounds it. `choices` is
 * `control` even though an agent wrote it — it is a question, not a
 * certificate.
 */

export type BlackboardContentClass = 'evidence' | 'control' | 'reasoning'

/** Fence-opener line, e.g. ` ```roshera:soundness `. Only the language
 *  token is pulled out; `cardKindFromLanguage` (the existing fence
 *  vocabulary in `blackboard-cards.ts`) decides whether it names a real
 *  card kind — this module never re-lists the kinds itself. */
const FENCE_OPEN_RE = /```\s*([^\s`]+)/g

/** Every `roshera:<kind>` fence opener found in a line's raw text, in
 *  order. Unknown or malformed fence languages are silently skipped —
 *  classification only cares about kinds `cardKindFromLanguage` recognises. */
function fencedCardKinds(text: string): CardKind[] {
  const kinds: CardKind[] = []
  for (const match of text.matchAll(FENCE_OPEN_RE)) {
    const kind = cardKindFromLanguage(match[1])
    if (kind) kinds.push(kind)
  }
  return kinds
}

/**
 * Classify a Blackboard line's content for rendering + editability.
 *
 * `evidence` when ANY of:
 *   - `line.author === 'system'` (app bookkeeping), OR
 *   - the text contains a verbatim kernel card fence — anything from
 *     `cardKindFromLanguage` other than `choices` (soundness/dfm/fcf/merge/
 *     refusal), OR
 *   - `isBuildStepLine(line)` is true (the kernel's own "Created …" echo).
 *
 * Otherwise `control` when the text carries a `roshera:choices` fence or a
 * detected "Option A: … Option B: …" enumeration (agent lines only, per
 * `detectEnumeratedChoices`'s own gating contract).
 *
 * Otherwise `reasoning` — agent prose or the user's own writing.
 */
export function classifyBlackboardContent(line: BlackboardLine): BlackboardContentClass {
  if (line.author === 'system') return 'evidence'
  if (isBuildStepLine(line)) return 'evidence'

  const kinds = fencedCardKinds(line.text)
  if (kinds.some((kind) => kind !== 'choices')) return 'evidence'
  if (kinds.includes('choices')) return 'control'
  if (line.author === 'agent' && detectEnumeratedChoices(line.text)) return 'control'

  return 'reasoning'
}
