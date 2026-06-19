import { memo } from 'react'
import ReactMarkdown from 'react-markdown'
import type { Components } from 'react-markdown'
import remarkMath from 'remark-math'
import rehypeKatex from 'rehype-katex'
import { cn } from '@/lib/utils'

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
  code: ({ children, className: codeClass }) => (
    <code
      className={cn(
        'rounded bg-foreground/10 px-1 py-0.5 font-mono text-[0.85em]',
        codeClass,
      )}
    >
      {children}
    </code>
  ),
  pre: ({ children }) => (
    <pre className="my-1 overflow-x-auto rounded bg-foreground/10 p-2 text-[0.85em]">
      {children}
    </pre>
  ),
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
