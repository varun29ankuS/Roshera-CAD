import type { BlackboardLine } from '@/stores/blackboard-store'
import type { BlackboardCard, CardKind } from './blackboard-cards.ts'
import { isDfmReport } from './blackboard-cards.ts'
import { isBuildStepLine } from './blackboard-groups.ts'
import { cardsInText, cardVerdict, type LineVerdict } from './blackboard-line-state.ts'

/**
 * TURN EVIDENCE — every agent turn ends in the kernel's answer
 * =============================================================
 * The kernel certifies every operation; the conversation has to make that
 * legible at a glance. Per-line verdict colour (`lineVerdict`) shows it
 * one 16 px marker at a time, so learning whether a six-step turn ended
 * sound meant scrolling the turn and reading every card. This module folds
 * a turn into the strip `components/panels/TurnEvidenceStrip.tsx` draws
 * after its last line: one tick / cross / slash per verdict-carrying card,
 * in the order the cards were written, plus the overall (worst) verdict.
 *
 * # What a turn is
 * The run of non-user lines between two user lines. `user` is the only
 * authorship that survives the backend round-trip (system lines come back
 * as `agent` — see `blackboard-groups.ts`), so it is the only honest
 * boundary. Two asks queued back to back sit adjacent and the work that
 * follows answers both; the run is therefore never labelled as the answer
 * to one particular prompt.
 *
 * # Where verdicts come from
 * Only from the cards the kernel's own payloads validate into
 * (`cardsInText` / `cardVerdict`, the same parser and fold the line marker
 * uses), and from `ws-bridge.ts`'s `Server refused: …` line, a text
 * signature only that writer produces. Never from prose.
 *
 * # The honesty case
 * A turn that built geometry (`isBuildStepLine`: the kernel's own
 * "Created …" echo) but carried no soundness certificate gets a
 * "no certificate shown" mark, can't-say severity. The work happened; the
 * board holds no proof of it, and the strip must not read as certified.
 */

/** What a mark is about: a card kind, or one of the two board facts that
 *  are not cards (a WS refusal line, built geometry with no certificate). */
export type EvidenceSource = CardKind | 'server-refusal' | 'uncertified'

export interface EvidenceMark {
  /** The line the evidence was written on — the strip scrolls to it. For
   *  `uncertified`, the turn's last build-step line. */
  lineId: string
  source: EvidenceSource
  verdict: LineVerdict
  /** Glyph caption: a word, never a count. */
  word: string
}

export interface TurnEvidence {
  /** Every line of the run, in board order. */
  lineIds: string[]
  /** The line after which the strip is drawn. */
  lastLineId: string
  marks: EvidenceMark[]
  /** Worst verdict over `marks` (fail > inconclusive > pass); `null` when
   *  the turn carries nothing to certify. */
  verdict: LineVerdict | null
}

const SEVERITY: Record<LineVerdict, number> = { fail: 3, inconclusive: 2, pass: 1 }

const SERVER_REFUSED = /^Server refused: /

/** The caption for one card's verdict, in the engineer's words. Read from
 *  the same fields `cardVerdict` reads, so word and glyph cannot disagree. */
function markWord(card: BlackboardCard, verdict: LineVerdict): string {
  switch (card.kind) {
    case 'soundness':
      return verdict === 'pass' ? 'sound' : verdict === 'fail' ? 'unsound' : 'soundness not run'
    case 'dfm':
      if (verdict === 'pass') return 'DFM pass'
      if (verdict === 'fail') return isDfmReport(card.card) ? 'DFM violations' : 'DFM violation'
      return 'DFM inconclusive'
    case 'fcf':
      return verdict === 'pass' ? 'GD&T in spec' : verdict === 'fail' ? 'GD&T out of spec' : 'GD&T not evaluable'
    case 'merge':
      return verdict === 'pass' ? 'merged' : verdict === 'fail' ? 'merge failed' : 'merge needs a decision'
    case 'refusal':
      return 'refused'
    case 'choices':
      return 'choice'
  }
}

/** `inFlight`: the run holds the line still streaming. Its certificate may
 *  simply not have arrived yet — the kernel echoes "Created …" at the first
 *  bore and the agent posts `verify_part` at the end — so "no certificate
 *  shown" waits until the turn settles. Real card marks still show live. */
function evidenceForRun(run: BlackboardLine[], inFlight: boolean): TurnEvidence {
  const marks: EvidenceMark[] = []
  let sawSoundness = false
  let lastBuildLineId: string | null = null
  for (const line of run) {
    if (isBuildStepLine(line)) lastBuildLineId = line.id
    if (SERVER_REFUSED.test(line.text)) {
      marks.push({ lineId: line.id, source: 'server-refusal', verdict: 'inconclusive', word: 'refused' })
      continue
    }
    for (const card of cardsInText(line.text)) {
      const verdict = cardVerdict(card)
      if (verdict === null) continue
      if (card.kind === 'soundness') sawSoundness = true
      marks.push({ lineId: line.id, source: card.kind, verdict, word: markWord(card, verdict) })
    }
  }
  if (lastBuildLineId !== null && !sawSoundness && !inFlight) {
    marks.push({ lineId: lastBuildLineId, source: 'uncertified', verdict: 'inconclusive', word: 'no certificate shown' })
  }
  let verdict: LineVerdict | null = null
  for (const m of marks) {
    if (verdict === null || SEVERITY[m.verdict] > SEVERITY[verdict]) verdict = m.verdict
  }
  return {
    lineIds: run.map((l) => l.id),
    lastLineId: run[run.length - 1].id,
    marks,
    verdict,
  }
}

/** Every turn on the board, in order — one per maximal run of non-user
 *  lines. User lines are never part of a turn and are never read for
 *  evidence: a person typing a fence or "Server refused:" is content.
 *  `streamingLineId` is the line still arriving (the store's own
 *  `streamingLineId`), or `null` between turns. */
export function turnEvidence(
  lines: readonly BlackboardLine[],
  streamingLineId: string | null = null,
): TurnEvidence[] {
  const turns: TurnEvidence[] = []
  let run: BlackboardLine[] = []
  const flush = () => {
    if (run.length === 0) return
    turns.push(evidenceForRun(run, streamingLineId !== null && run.some((l) => l.id === streamingLineId)))
    run = []
  }
  for (const line of lines) {
    if (line.author === 'user') {
      flush()
      continue
    }
    run.push(line)
  }
  flush()
  return turns
}

/** The turns that have something to show, keyed by the line the strip is
 *  drawn after. A prose-only turn has nothing to certify and no entry. */
export function evidenceByLastLine(
  lines: readonly BlackboardLine[],
  streamingLineId: string | null = null,
): Map<string, TurnEvidence> {
  const out = new Map<string, TurnEvidence>()
  for (const turn of turnEvidence(lines, streamingLineId)) {
    if (turn.marks.length > 0) out.set(turn.lastLineId, turn)
  }
  return out
}
