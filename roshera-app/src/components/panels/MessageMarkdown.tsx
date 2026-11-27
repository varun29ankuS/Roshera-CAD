import { Children, isValidElement, memo, useMemo, type ReactNode } from 'react'
import ReactMarkdown from 'react-markdown'
import type { Components } from 'react-markdown'
import { cn } from '@/lib/utils'
import { cardKindFromLanguage } from '@/lib/blackboard-cards'
import { REHYPE_PLUGINS, REMARK_PLUGINS } from '@/lib/markdown-pipeline'
import { normalizeMathDelimiters } from '@/lib/math-delimiters'
import { CardRenderer } from './cards/CardRenderer'

/**
 * Renders agent prose with embedded LaTeX math. Inline `$...$` / `\(...\)`
 * and block `$$...$$` / `\[...\]` are typeset with KaTeX (the backslash
 * forms are rewritten to dollars by `normalizeMathDelimiters`); everything
 * else is rendered as markdown with GFM tables (lib/markdown-pipeline.ts),
 * so the agent can emit derivations and value tables naturally.
 *
 * Malformed LaTeX never crashes the panel: rehype-katex is configured with
 * `throwOnError: false`, so a broken expression falls back to its raw source
 * (styled in KaTeX's error colour) instead of throwing.
 *
 * The KaTeX stylesheet is imported once here so any consumer of this
 * component gets correct math typesetting without a separate global import.
 */
import 'katex/dist/katex.min.css'

interface Props {
  content: string
  className?: string
}

/** True when a <pre> wraps a `roshera:*` typed-card code block — the card
 *  renderer replaces the whole block, so the pre must not add code chrome. */
function containsCardBlock(children: ReactNode): boolean {
  return Children.toArray(children).some(
    (child) =>
      isValidElement(child) &&
      cardKindFromLanguage((child.props as { className?: string }).className) !== null,
  )
}

/** Flatten a code element's children to the raw fenced source. */
function codeText(children: ReactNode): string {
  return Children.toArray(children)
    .map((c) => (typeof c === 'string' || typeof c === 'number' ? String(c) : ''))
    .join('')
}

// Tighten the default markdown element spacing so equations and prose sit
// comfortably inside the compact chat bubble rather than the browser defaults.
const markdownComponents: Components = {
  p: ({ children }) => <p className="my-1.5 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => (
    <ul className="my-1 ml-4 list-disc space-y-0.5">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="my-1 ml-4 list-decimal space-y-0.5">{children}</ol>
  ),
  li: ({ children }) => <li className="leading-normal">{children}</li>,
  strong: ({ children }) => <strong className="font-semibold text-foreground">{children}</strong>,
  em: ({ children }) => <em className="text-foreground/90">{children}</em>,
  h1: ({ children }) => <h1 className="mt-2 mb-1 text-sm font-semibold text-foreground first:mt-0">{children}</h1>,
  h2: ({ children }) => <h2 className="mt-2 mb-1 text-[13px] font-semibold text-foreground first:mt-0">{children}</h2>,
  h3: ({ children }) => (
    <h3 className="mt-1.5 mb-0.5 text-[12px] font-medium text-foreground/90 first:mt-0">{children}</h3>
  ),
  blockquote: ({ children }) => (
    <blockquote className="my-1 border-l-2 border-border/70 pl-2 text-foreground/75">{children}</blockquote>
  ),
  hr: () => <hr className="my-2 border-border/50" />,
  // GFM tables (remark-gfm, see lib/markdown-pipeline.ts) in the card
  // grammar: hairline borders, a mono uppercase header like a card's section
  // labels, tabular numerals so a column of values lines up, and horizontal
  // scroll instead of overflowing the panel.
  table: ({ children }) => (
    <div className="my-1.5 overflow-x-auto rounded-md border border-border/70">
      {/* A value never breaks across lines ("2.50 /" over "2.20"): every
          column after the first keeps its cell on one line, and the label
          column takes what is left. */}
      <table className="w-full border-collapse text-[12px] tabular-nums [&_td:not(:first-child)]:whitespace-nowrap [&_th:not(:first-child)]:whitespace-nowrap">
        {children}
      </table>
    </div>
  ),
  thead: ({ children }) => <thead className="bg-foreground/5">{children}</thead>,
  tr: ({ children }) => <tr className="border-b border-border/50 last:border-b-0">{children}</tr>,
  th: ({ children, style }) => (
    <th
      style={style}
      className="px-2 py-1 text-left font-mono text-[11px] font-normal uppercase tracking-wide text-muted-foreground"
    >
      {children}
    </th>
  ),
  td: ({ children, style }) => (
    <td style={style} className="px-1.5 py-1 align-baseline text-foreground/90">
      {children}
    </td>
  ),
  code: ({ children, className: codeClass }) => {
    // A ```roshera:<kind> fence is a TYPED CARD, not a code sample: the
    // payload (validated against the real wire shapes) renders as a
    // structured result block. See lib/blackboard-cards.ts.
    const cardKind = cardKindFromLanguage(codeClass)
    if (cardKind !== null) {
      return <CardRenderer kind={cardKind} source={codeText(children)} />
    }
    return (
      <code
        className={cn(
          'rounded bg-foreground/10 px-1 py-0.5 font-mono text-[0.85em]',
          codeClass,
        )}
      >
        {children}
      </code>
    )
  },
  pre: ({ children }) => {
    // Unwrap typed-card blocks — the card carries its own chrome.
    if (containsCardBlock(children)) {
      return <>{children}</>
    }
    return (
      <pre className="my-1 overflow-x-auto rounded bg-foreground/10 p-2 text-[0.85em]">
        {children}
      </pre>
    )
  },
  a: ({ children, href }) => (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className="underline underline-offset-2"
    >
      {children}
    </a>
  ),
}

function MessageMarkdownImpl({ content, className }: Props) {
  const source = useMemo(() => normalizeMathDelimiters(content), [content])
  return (
    <div
      className={cn(
        // Center block-level equations and let them scroll horizontally
        // instead of overflowing the bubble.
        'space-y-1 break-words [&_.katex-display]:my-1 [&_.katex-display]:overflow-x-auto [&_.katex-display]:overflow-y-hidden',
        className,
      )}
    >
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS} rehypePlugins={REHYPE_PLUGINS} components={markdownComponents}>
        {source}
      </ReactMarkdown>
    </div>
  )
}

export const MessageMarkdown = memo(MessageMarkdownImpl)
