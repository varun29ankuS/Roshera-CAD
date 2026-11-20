/**
 * The Blackboard's markdown pipeline — the remark/rehype plugin lists
 * `MessageMarkdown` renders every line with, kept in a plain module so the
 * exact configuration the panel uses is the one the tests render.
 *
 * - remark-gfm: tables (an agent's measured-values table is a table, not
 *   pipe text), task lists, autolinks. `singleTilde: false` — engineers
 *   write `~5 mm` for "approximately", and GFM's default reads two such
 *   tildes on one line as strikethrough. `~~double~~` still strikes through.
 * - remark-math + rehype-katex: inline `$…$` and display `$$…$$` are typeset
 *   with KaTeX. `throwOnError: false` — a malformed expression falls back to
 *   its raw source in KaTeX's error colour instead of breaking the panel.
 *   `\(…\)` / `\[…\]` reach these as `$` delimiters via
 *   `normalizeMathDelimiters` (math-delimiters.ts), applied before render.
 *
 * GFM runs before math so a table's cells are split first and each cell's
 * `$…$` is then typeset in place. (A `|` inside a cell's math still ends the
 * cell, as in every GFM renderer — write `\lvert x \rvert` there.)
 */
import type { Options } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import rehypeKatex from 'rehype-katex'

export const REMARK_PLUGINS: NonNullable<Options['remarkPlugins']> = [
  [remarkGfm, { singleTilde: false }],
  remarkMath,
]

export const REHYPE_PLUGINS: NonNullable<Options['rehypePlugins']> = [[rehypeKatex, { throwOnError: false }]]
