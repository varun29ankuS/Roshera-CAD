import { memo } from 'react'
import { cardTextParts } from '@/lib/card-math'

/**
 * One line of AUTHORED card text — a note, a hint, a next action, a table
 * label — with its inline math typeset (`$K_t = 1 + 2b/a$`, `\(\varnothing
 * 20\,H7\)`). Not a markdown document: a card field is a phrase, so it is
 * split into text and math directly (lib/card-math.ts) rather than nested
 * through `MessageMarkdown` (which itself renders cards).
 *
 * Verbatim kernel strings (a refusal's `reason`, a certificate's `verdict`)
 * are NOT rendered through this: they are quoted exactly as the kernel said
 * them.
 */
function CardTextImpl({ text }: { text: string }) {
  return (
    <>
      {cardTextParts(text).map((p, i) =>
        p.kind === 'text' ? (
          <span key={i}>{p.value}</span>
        ) : (
          <span
            key={i}
            className={p.display ? 'mx-0.5 inline-block align-middle' : undefined}
            // KaTeX's own markup for the agent's expression (trust: false).
            dangerouslySetInnerHTML={{ __html: p.html }}
          />
        ),
      )}
    </>
  )
}

export const CardText = memo(CardTextImpl)
