import type { ComponentType, ReactNode } from 'react'
import { Check, Minus, X } from 'lucide-react'
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
 * Tri-state invariant chip: proven true / proven false / NOT RUN. The third
 * state is rendered distinctly on purpose — a check that did not run is not
 * a check that passed.
 */
export function TriState({ label, value }: { label: string; value: boolean | null | undefined }) {
  const v = value ?? null
  return (
    <span
      title={v === null ? `${label}: not run — not-run is not a pass` : `${label}: ${v ? 'proven' : 'FAILED'}`}
      className={cn(
        'inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px]',
        v === true && 'border-emerald-500/30 text-emerald-400',
        v === false && 'border-red-500/40 text-red-400',
        v === null && 'border-border text-muted-foreground/70 border-dashed',
      )}
    >
      {v === true ? <Check size={9} /> : v === false ? <X size={9} /> : <Minus size={9} />}
      {label}
      {v === null && <span className="opacity-70">not run</span>}
    </span>
  )
}

