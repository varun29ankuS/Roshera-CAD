import { Children, isValidElement, memo, type ReactNode } from 'react'
import ReactMarkdown from 'react-markdown'
import type { Components } from 'react-markdown'
import remarkMath from 'remark-math'
import rehypeKatex from 'rehype-katex'
import { cn } from '@/lib/utils'
import { cardKindFromLanguage } from '@/lib/blackboard-cards'
import { CardRenderer } from './cards/CardRenderer'

/**
 * Renders agent prose with embedded LaTeX math. Inline `$...$` and block
 * `$$...$$` are typeset with KaTeX; everything else is rendered as light
 * markdown so the agent can emit derivations (prose + equations) naturally.
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
  p: ({ children }) => <p className="my-1 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => (
    <ul className="my-1 ml-4 list-disc space-y-0.5">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="my-1 ml-4 list-decimal space-y-0.5">{children}</ol>
  ),
  li: ({ children }) => <li className="leading-snug">{children}</li>,
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
  // No `table` handler: GFM tables need remark-gfm, which isn't a dependency
  // here — adding one mid-task is out of scope. Bare CommonMark tables
  // render as literal pipe text, same as before this change.
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

const REHYPE_KATEX_OPTIONS = { throwOnError: false } as const

function MessageMarkdownImpl({ content, className }: Props) {
  return (
    <div
      className={cn(
        // Center block-level equations and let them scroll horizontally
        // instead of overflowing the bubble.
        'space-y-1 break-words [&_.katex-display]:my-1 [&_.katex-display]:overflow-x-auto [&_.katex-display]:overflow-y-hidden',
        className,
      )}
    >
      <ReactMarkdown
        remarkPlugins={[remarkMath]}
        rehypePlugins={[[rehypeKatex, REHYPE_KATEX_OPTIONS]]}
        components={markdownComponents}
      >
        {content}
      </ReactMarkdown>
    </div>
  )
}

export const MessageMarkdown = memo(MessageMarkdownImpl)
