/**
 * MATH DELIMITERS — one notation for the Blackboard's typesetter.
 * ================================================================
 * The Blackboard typesets `$…$` (inline) and `$$…$$` (display) through
 * remark-math + KaTeX. Models also write LaTeX's own delimiters, `\(…\)` and
 * `\[…\]`, which remark-math does not know — they reached the board as
 * literal backslash-paren text. `normalizeMathDelimiters` rewrites them to the
 * dollar forms BEFORE anything else sees the text, so the streaming splitter
 * (streaming-math.ts) and the committed render (MessageMarkdown) agree on
 * where every expression starts and ends.
 *
 * Never rewritten: fenced blocks (a `roshera:*` card's JSON is data, and a
 * code sample is code) and inline code spans. An escaped backslash (`\\(`)
 * is not a delimiter; an opener with no closer before a blank line is left
 * exactly as written (a stream that has not finished the expression yet).
 *
 * Display placement is decided from the OPENER alone: `\[` that starts its
 * line becomes a `$$` block on its own lines; anywhere else it becomes an
 * inline `$$…$$`. Nothing written after the closer can change how the
 * expression was rewritten, so an already-settled streaming chunk never
 * changes (the splitter's append-only contract).
 *
 * `splitInlineMath` is the card-field counterpart: a card's note, hint or
 * table cell is one line of text with inline math in it, not a markdown
 * document, so it is split into text and math segments directly.
 */

function isEscaped(text: string, i: number): boolean {
  let backslashes = 0
  for (let j = i - 1; j >= 0 && text[j] === '\\'; j--) backslashes++
  return backslashes % 2 === 1
}

const FENCE_RE = /^ {0,3}(`{3,}|~{3,})/

/** Index of the unescaped two-char `closer` at or after `from`, stopping at a
 *  blank line (math never spans a paragraph break). -1 when absent. */
function findCloser(text: string, from: number, end: number, closer: string): number {
  for (let j = from; j + 1 < end; j++) {
    if (text[j] === '\n' && text[j + 1] === '\n') return -1
    if (text[j] === closer[0] && text[j + 1] === closer[1] && !isEscaped(text, j)) return j
  }
  return -1
}

/** Length of the backtick run starting at `i`. */
function backtickRun(text: string, i: number): number {
  let n = 0
  while (text[i + n] === '`') n++
  return n
}

/** Rewrite `\(…\)` / `\[…\]` within one non-fence span. */
function normalizeSpan(text: string): string {
  let out = ''
  let i = 0
  const end = text.length
  while (i < end) {
    const ch = text[i]
    if (ch === '`') {
      // Inline code span: copy through the matching run verbatim.
      const run = backtickRun(text, i)
      const marker = '`'.repeat(run)
      let close = text.indexOf(marker, i + run)
      while (close !== -1 && backtickRun(text, close) !== run) {
        close = text.indexOf(marker, close + backtickRun(text, close))
      }
      if (close === -1) {
        out += marker
        i += run
      } else {
        out += text.slice(i, close + run)
        i = close + run
      }
      continue
    }
    if (ch === '\\' && !isEscaped(text, i) && (text[i + 1] === '(' || text[i + 1] === '[')) {
      const display = text[i + 1] === '['
      const close = findCloser(text, i + 2, end, display ? '\\]' : '\\)')
      const inner = close === -1 ? '' : text.slice(i + 2, close).trim()
      if (close !== -1 && inner.length > 0 && !/(^|[^\\])\$/.test(inner)) {
        if (!display) {
          out += `$${inner}$`
        } else {
          const lineStart = text.lastIndexOf('\n', i - 1) + 1
          const startsLine = text.slice(lineStart, i).trim() === ''
          if (startsLine) {
            const nl = text.indexOf('\n', close + 2)
            const restOfLine = text.slice(close + 2, nl === -1 ? end : nl)
            out += `$$\n${inner}\n$$${restOfLine.trim() !== '' ? '\n' : ''}`
          } else {
            out += `$$${inner}$$`
          }
        }
        i = close + 2
        continue
      }
    }
    out += ch
    i++
  }
  return out
}

/** Rewrite LaTeX `\(…\)` / `\[…\]` math to `$…$` / `$$…$$`, outside code. */
export function normalizeMathDelimiters(text: string): string {
  if (!text.includes('\\(') && !text.includes('\\[')) return text
  const lines = text.split('\n')
  const parts: string[] = []
  let span: string[] = []
  let fence: string | null = null
  const flush = () => {
    if (span.length > 0) parts.push(normalizeSpan(span.join('\n')))
    span = []
  }
  for (const line of lines) {
    const marker = FENCE_RE.exec(line)?.[1] ?? null
    if (fence === null) {
      if (marker !== null) {
        flush()
        fence = marker
        parts.push(line)
      } else {
        span.push(line)
      }
    } else {
      parts.push(line)
      if (marker !== null && marker[0] === fence[0] && marker.length >= fence.length) fence = null
    }
  }
  flush()
  return parts.join('\n')
}

export type InlineMathSegment =
  | { kind: 'text'; value: string }
  | { kind: 'math'; value: string; display: boolean }

/**
 * Split one line of card text into text and math segments. Recognises
 * `$$…$$`, `$…$` (remark-math's rule: the opener is followed, and the closer
 * preceded, by a non-space — so "$5 and $10" stays text), `\(…\)` and
 * `\[…\]`. An escaped `\$` never delimits; anything unclosed stays text.
 */
export function splitInlineMath(text: string): InlineMathSegment[] {
  const segments: InlineMathSegment[] = []
  const pushText = (value: string) => {
    if (value.length === 0) return
    const last = segments[segments.length - 1]
    if (last && last.kind === 'text') last.value += value
    else segments.push({ kind: 'text', value })
  }
  let i = 0
  let textStart = 0
  const end = text.length
  const emit = (from: number, to: number, value: string, display: boolean) => {
    pushText(text.slice(textStart, from))
    segments.push({ kind: 'math', value, display })
    i = to
    textStart = to
  }
  while (i < end) {
    const ch = text[i]
    if (isEscaped(text, i)) {
      i++
      continue
    }
    if (ch === '\\' && (text[i + 1] === '(' || text[i + 1] === '[')) {
      const display = text[i + 1] === '['
      const close = findCloser(text, i + 2, end, display ? '\\]' : '\\)')
      const inner = close === -1 ? '' : text.slice(i + 2, close).trim()
      if (inner.length > 0) {
        emit(i, close + 2, inner, display)
        continue
      }
    } else if (ch === '$' && text[i + 1] === '$') {
      const close = findCloser(text, i + 2, end, '$$')
      const inner = close === -1 ? '' : text.slice(i + 2, close).trim()
      if (inner.length > 0) {
        emit(i, close + 2, inner, true)
        continue
      }
    } else if (ch === '$') {
      const next = text[i + 1]
      if (next !== undefined && !/\s/.test(next)) {
        let close = -1
        for (let j = i + 1; j < end; j++) {
          if (text[j] === '$' && !isEscaped(text, j) && j > i + 1 && !/\s/.test(text[j - 1])) {
            close = j
            break
          }
        }
        if (close !== -1) {
          emit(i, close + 1, text.slice(i + 1, close), false)
          continue
        }
      }
    }
    i++
  }
  pushText(text.slice(textStart))
  return segments
}
