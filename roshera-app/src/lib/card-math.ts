/**
 * Typeset the inline math in one line of authored card text (a note, a
 * hint, a next action, a table cell). Returns text runs and KaTeX HTML runs
 * for `CardText` to place; kept out of the component so the exact output the
 * card shows is what the tests check.
 *
 * KaTeX runs with `throwOnError: false` (malformed LaTeX shows its own
 * source in the error colour) and its default `trust: false`, so `\href`,
 * `\htmlClass` and friends are refused: the HTML is KaTeX's own markup only.
 */
import katex from 'katex'
import { splitInlineMath } from './math-delimiters.ts'

export type CardTextPart = { kind: 'text'; value: string } | { kind: 'math'; html: string; display: boolean }

export function cardTextParts(text: string): CardTextPart[] {
  return splitInlineMath(text).map((s) =>
    s.kind === 'text'
      ? s
      : {
          kind: 'math',
          display: s.display,
          html: katex.renderToString(s.value, { throwOnError: false, displayMode: false }),
        },
  )
}
