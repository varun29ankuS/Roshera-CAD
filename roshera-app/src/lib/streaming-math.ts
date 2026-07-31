/**
 * STREAMING SEGMENTATION — buffer math (and cards) to completeness.
 * =================================================================
 * Streaming raw LaTeX shows broken markup and flickers; re-typesetting every
 * settled expression on every incoming token wastes work for the same ugly
 * result. So a streaming line is split into:
 *
 *   - `settled` chunks — APPEND-ONLY. A chunk is cut every time a math
 *     expression (`$…$` / `$$…$$`) or a fenced block closes; once cut, its
 *     string never changes again, so a memoized renderer typesets each chunk
 *     exactly once. KaTeX only ever sees complete expressions.
 *   - `tail` — prose after the last settled boundary, guaranteed to contain
 *     no math opener and no fence opener. Safe to re-render per token (plain
 *     markdown, no KaTeX involved).
 *   - `pending` — when the stream is mid-expression (`'math'`) or mid-fence
 *     (`'block'`, which covers typed cards), the unfinished source is
 *     WITHHELD; the UI shows a writing indicator instead. The expression
 *     renders once, complete, the moment its closing delimiter arrives.
 *
 * Delimiter rules mirror remark-math's: an inline `$` opens only when the
 * next char is not whitespace, closes only when the previous char is not
 * whitespace; `\$` never delimits. A lone unclosed `$` therefore withholds
 * only until the stream settles — on commit the full text renders through the
 * normal (non-streaming) path regardless.
 */

export type PendingKind = 'math' | 'block' | null

export interface StreamView {
  /** Append-only settled chunks; every expression inside is complete. */
  settled: string[]
  /** Prose after the last boundary — contains no math/fence opener. */
  tail: string
  /** Non-null while an expression/fence is being written (source withheld). */
  pending: PendingKind
}

function isEscaped(text: string, i: number): boolean {
  let backslashes = 0
  for (let j = i - 1; j >= 0 && text[j] === '\\'; j--) backslashes++
  return backslashes % 2 === 1
}

/** Does a fence line (``` or ~~~, optionally indented ≤3 spaces) start at
 *  line-start index `i`? Returns the fence marker or null. */
function fenceMarkerAt(line: string): string | null {
  const m = /^ {0,3}(`{3,}|~{3,})/.exec(line)
  return m ? m[1] : null
}

/**
 * Split streaming text into settled chunks, a safe tail, and a pending
 * marker. Boundaries only ever move forward as text is appended, so the
 * settled chunk list is stable (chunk N's content never changes once chunk
 * N+1 exists — and never changes at all once cut).
 */
export function splitStreamingText(text: string): StreamView {
  const boundaries: number[] = []
  let pending: PendingKind = null
  let pendingStart = -1

  const lines = text.split('\n')
  let offset = 0
  let inFence: string | null = null
  let fenceStart = -1
  // Indices into `text` of line starts let us walk lines for fence detection
  // while scanning math within non-fence spans.
  const nonFenceSpans: Array<[number, number]> = []
  let spanStart = 0

  for (let li = 0; li < lines.length; li++) {
    const line = lines[li]
    const lineEnd = offset + line.length // exclusive of '\n'
    const marker = fenceMarkerAt(line)
    if (inFence === null) {
      if (marker !== null) {
        // Fence opens: non-fence span ends at this line's start.
        if (offset > spanStart) nonFenceSpans.push([spanStart, offset])
        inFence = marker
        fenceStart = offset
      }
    } else if (
      marker !== null &&
      marker[0] === inFence[0] &&
      marker.length >= inFence.length
    ) {
      // Fence closes at the end of this line → settled boundary EXACTLY at
      // the line end, never including a trailing newline: the newline may
      // not have streamed in yet, and a boundary that moves when it arrives
      // would rewrite an already-settled chunk (breaking append-only). The
      // newline simply opens the next chunk.
      const boundary = lineEnd
      boundaries.push(boundary)
      inFence = null
      fenceStart = -1
      spanStart = boundary
    }
    offset = lineEnd + 1
  }
  if (inFence !== null) {
    pending = 'block'
    pendingStart = fenceStart
  } else if (spanStart < text.length) {
    nonFenceSpans.push([spanStart, text.length])
  }

  // Math scan over non-fence spans (only up to the pending fence, if any).
  // NOTE: `pending` may already be 'block' from the fence pass — that must
  // NOT stop this scan (boundaries before the fence are still valid, and
  // dropping them would rewrite already-settled chunks). Only an unclosed
  // MATH opener halts scanning, via `mathHalt`.
  let mathHalt = false
  const unclosedMathAt = (i: number) => {
    if (pending === null || i < pendingStart) {
      pending = 'math'
      pendingStart = i
    }
    mathHalt = true
  }
  for (const [start, end] of nonFenceSpans) {
    let i = start
    while (i < end) {
      const ch = text[i]
      if (ch !== '$' || isEscaped(text, i)) {
        i++
        continue
      }
      const isDisplay = text[i + 1] === '$' && i + 1 < end
      if (isDisplay) {
        // $$ … $$
        let close = -1
        for (let j = i + 2; j + 1 < end; j++) {
          if (text[j] === '$' && text[j + 1] === '$' && !isEscaped(text, j)) {
            close = j
            break
          }
        }
        if (close === -1) {
          unclosedMathAt(i)
          i = end
        } else {
          boundaries.push(close + 2)
          i = close + 2
        }
      } else {
        // Inline $ … $ — remark-math-style whitespace rules.
        const next = text[i + 1]
        if (next === undefined || /\s/.test(next)) {
          i++
          continue
        }
        let close = -1
        for (let j = i + 1; j < end; j++) {
          if (
            text[j] === '$' &&
            !isEscaped(text, j) &&
            j > i + 1 &&
            !/\s/.test(text[j - 1])
          ) {
            close = j
            break
          }
        }
        if (close === -1) {
          unclosedMathAt(i)
          i = end
        } else {
          boundaries.push(close + 1)
          i = close + 1
        }
      }
    }
    if (mathHalt) break
  }

  // Fence and math boundaries were collected in separate passes — merge
  // them into document order before cutting chunks.
  boundaries.sort((a, b) => a - b)

  // Build settled chunks from the boundaries that precede any pending start.
  const cutoff = pending !== null ? pendingStart : text.length
  const settled: string[] = []
  let prev = 0
  for (const b of boundaries) {
    if (b > cutoff) break
    settled.push(text.slice(prev, b))
    prev = b
  }
  const tail = text.slice(prev, cutoff)
  return { settled, tail, pending }
}
