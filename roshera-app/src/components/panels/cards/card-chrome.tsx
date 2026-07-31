import type { ComponentType, ReactNode } from 'react'
import { Check, CircleSlash, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { ChalkReveal } from './ChalkReveal'

/**
 * Shared chrome for typed Blackboard cards. One visual system: a bordered
 * result block with a colour-coded left accent so a card's outcome is
 * legible at a glance —
 *   emerald = proven pass / certified in-spec / clean merge
 *   red     = proven violation / out of spec
 *   amber   = honest refusal-to-decide (unverifiable, conflicts)
 *   sky     = typed refusal (a RESULT with options, never styled as an error)
 *   neutral = informational (design intent, partial certificates)
 * Cards arrive with the chalk-draw reveal (skippable; see ChalkReveal).
 */

export type CardAccent = 'pass' | 'fail' | 'warn' | 'info' | 'neutral'

const ACCENT = {
  pass: {
    edge: 'border-l-emerald-500/70',
    text: 'text-emerald-400',
    chalk: 'text-emerald-400/60',
  },
  fail: {
    edge: 'border-l-red-500/70',
    text: 'text-red-400',
    chalk: 'text-red-400/60',
  },
  warn: {
    edge: 'border-l-amber-500/70',
    text: 'text-amber-400',
    chalk: 'text-amber-400/60',
  },
  info: {
    edge: 'border-l-sky-500/70',
    text: 'text-sky-400',
    chalk: 'text-sky-400/60',
  },
  neutral: {
    edge: 'border-l-border',
    text: 'text-muted-foreground',
    chalk: 'text-muted-foreground/60',
  },
} as const satisfies Record<CardAccent, { edge: string; text: string; chalk: string }>

interface CardShellProps {
  accent: CardAccent
  icon: ComponentType<{ size?: number | string; className?: string }>
  title: ReactNode
  /** Right-aligned chip(s) in the header row. */
  chip?: ReactNode
  children?: ReactNode
}

export function CardShell({ accent, icon: Icon, title, chip, children }: CardShellProps) {
  const a = ACCENT[accent]
  return (
    <ChalkReveal framed className={cn('my-1.5', a.chalk)}>
      <div
        className={cn(
          'select-text rounded-md border border-border/70 border-l-2 bg-card/50 px-3 py-2 text-xs',
          a.edge,
        )}
      >
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <Icon size={13} className={cn('shrink-0', a.text)} />
          <span className="font-medium text-foreground/90">{title}</span>
          {chip !== undefined && <span className="ml-auto flex items-center gap-1.5">{chip}</span>}
        </div>
        {children}
      </div>
    </ChalkReveal>
  )
}

/** Small status chip (header corner). */
export function Chip({
  accent,
  children,
  dashed = false,
  title,
}: {
  accent: CardAccent
  children: ReactNode
  dashed?: boolean
  title?: string
}) {
  const a = ACCENT[accent]
  return (
    <span
      title={title}
      className={cn(
        'inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide',
        a.text,
        dashed ? 'border-dashed border-current/50' : 'border-current/40',
      )}
    >
      {children}
    </span>
  )
}

/** Label → value row using the leader-line convention from side panels. */
export function KV({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="cad-leader flex items-baseline gap-0 text-[11px]">
      <span>{label}</span>
      <span className="cad-readout text-foreground/90">{children}</span>
    </div>
  )
}

/**
 * A single certified claim, one per line: a status glyph — green tick /
 * red cross / amber "not checked" — followed by the claim text and an
 * optional dim secondary detail (a derivation, a witness, a refinement
 * trail). This is the shape Varun asked for twice: bullet points, each with
 * its own glyph, not a paragraph the verdict is buried inside.
 *
 * `status` is tri-state on purpose: `null` is NEITHER a pass nor a fail —
 * it means the kernel could not (or did not) check this claim, which is
 * exactly the "inconclusive/unverifiable" case the DFM policy already
 * forbids reporting as a pass. Collapsing it into green or red would be the
 * dishonesty this product exists to prevent, so it gets its own glyph and
 * its own colour (amber), never reused from pass or fail.
 */
export function Claim({
  status,
  children,
  detail,
  title,
}: {
  status: boolean | null
  children: ReactNode
  detail?: ReactNode
  title?: string
}) {
  return (
    <div
      title={title}
      className={cn(
        'flex items-baseline gap-1.5 text-[11px] leading-snug',
        status === true && 'text-foreground/90',
        status === false && 'text-foreground/90',
        status === null && 'text-foreground/70',
      )}
    >
      <span
        className={cn(
          'inline-flex shrink-0 translate-y-px',
          status === true && 'text-emerald-600 dark:text-emerald-400',
          status === false && 'text-red-600 dark:text-red-400',
          status === null && 'text-amber-600 dark:text-amber-400',
        )}
      >
        {status === true ? <Check size={11} /> : status === false ? <X size={11} /> : <CircleSlash size={11} />}
      </span>
      <span>
        {children}
        {detail !== undefined && (
          <span className="ml-1.5 text-[10px] text-muted-foreground/80">{detail}</span>
        )}
      </span>
    </div>
  )
}

