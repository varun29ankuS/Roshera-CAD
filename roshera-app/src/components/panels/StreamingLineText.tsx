import { useMemo } from 'react'
import { splitStreamingText } from '@/lib/streaming-math'
import { MessageMarkdown } from './MessageMarkdown'

/**
 * STREAMING LINE RENDERER
 * =======================
 * Used while a line is actively receiving tokens. Enforces the streaming
 * rule: LaTeX (and typed-card fences) are buffered to a COMPLETE expression
 * and typeset once — raw `$\sig…` mid-stream is broken markup and flickers.
 *
 * `splitStreamingText` cuts the text into append-only settled chunks (every
 * expression inside complete) plus a math-free prose tail. Each settled
 * chunk is a stable string, and `MessageMarkdown` is memoized, so a chunk is
 * typeset exactly once for the whole stream — incoming tokens only re-render
 * the plain-prose tail. While an expression or card fence is mid-write its
 * source is withheld behind a chalk cursor; it appears whole the moment the
 * closing delimiter arrives.
 */
export function StreamingLineText({ text }: { text: string }) {
  const view = useMemo(() => splitStreamingText(text), [text])
  return (
    <div>
      {view.settled.map((chunk, i) => (
        <MessageMarkdown key={i} content={chunk} />
      ))}
      {view.tail.trim().length > 0 && <MessageMarkdown content={view.tail} />}
      {view.pending !== null && (
        <span
          className="mt-0.5 inline-flex items-center gap-1.5 text-[11px] text-muted-foreground/80"
          title="The expression renders complete — streaming raw LaTeX is withheld by design"
        >
          <span className="chalk-cursor" />
          {view.pending === 'math' ? 'writing an expression…' : 'writing a result…'}
        </span>
      )}
    </div>
  )
}
