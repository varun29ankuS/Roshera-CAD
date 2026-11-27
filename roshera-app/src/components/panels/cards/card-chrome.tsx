import {
  Children,
  cloneElement,
  createContext,
  isValidElement,
  useContext,
  type ComponentType,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
} from 'react'
import {
  Check,
  CircleSlash,
  X,
} from '@/components/ui/icons'
import { cn } from '@/lib/utils'
import { lineStarts } from '@/lib/print-reveal'
import { printLine, usePrintRun } from './PrintReveal'
import { RevealContext } from './reveal-context'

/** Whether the card this row sits in is printing right now (PrintReveal). */
const PrintingContext = createContext(false)

/**
 * Shared chrome for typed Blackboard cards. One visual system: a bordered
 * result block with a colour-coded left accent so a card's outcome is
 * legible at a glance —
 *   emerald = proven pass / certified in-spec / clean merge
 *   red     = proven violation / out of spec
 *   amber   = honest refusal-to-decide (unverifiable, conflicts)
 *   sky     = typed refusal (a RESULT with options, never styled as an error)
 *   neutral = informational (design intent, partial certificates)
 * A card that has just arrived PRINTS, line by line: its header, then each
 * block under it, a certificate table one row at a time (see PrintReveal).
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
    edge: 'border-l-caution-border',
    text: 'text-caution',
    chalk: 'text-caution',
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
  /** The title as plain text. When given, the header stays on ONE line: the
   *  title truncates with an ellipsis (this text on hover) and the chip keeps
   *  its place at the right edge instead of dropping to a line of its own. */
  titleText?: string
  children?: ReactNode
}

export function CardShell({ accent, icon: Icon, title, chip, titleText, children }: CardShellProps) {
  const a = ACCENT[accent]
  // The print order: the header is line 0, then each block; a certificate
  // table counts one line per row, so its rows arrive one after another.
  const { animate } = useContext(RevealContext)
  const blocks = Children.toArray(children)
  const isCertRows = (b: unknown): b is ReactElement<CertRowsProps> => isValidElement(b) && b.type === CertRows
  const { starts, total } = lineStarts([
    1,
    ...blocks.map((b) => (isCertRows(b) ? Children.count(b.props.children) : 1)),
  ])
  const run = usePrintRun(animate, total)
  return (
    <PrintingContext.Provider value={run.printing}>
    <div className="my-1.5" style={run.style}>
      <div
        className={cn(
          // `@container`: the card's contents lay themselves out by the CARD's
          // width (a 320px docked Blackboard vs a wide panel), not the window's.
          'board-type @container select-text rounded-md border border-border/70 border-l-2 bg-card/50 px-3 py-2 text-xs',
          a.edge,
        )}
      >
        <div
          {...printLine(run.printing, 0)}
          className={cn('flex items-center gap-x-2 gap-y-1', titleText === undefined && 'flex-wrap')}
        >
          <Icon size={13} className={cn('shrink-0', a.text)} />
          <span
            title={titleText}
            className={cn('font-medium text-foreground/90', titleText !== undefined && 'min-w-0 truncate')}
          >
            {title}
          </span>
          {chip !== undefined && (
            <span className="ml-auto flex shrink-0 items-center gap-1.5">{chip}</span>
          )}
        </div>
        {blocks.map((block, k) =>
          isCertRows(block) ? (
            cloneElement(block, { printStart: starts[k + 1] })
          ) : (
            <div key={isValidElement(block) && block.key != null ? block.key : k} {...printLine(run.printing, starts[k + 1])}>
              {block}
            </div>
          ),
        )}
      </div>
    </div>
    </PrintingContext.Provider>
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
        'inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-[11px] uppercase tracking-wide',
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
          status === null && 'text-caution ',
        )}
      >
        {status === true ? <Check size={11} /> : status === false ? <X size={11} /> : <CircleSlash size={11} />}
      </span>
      <span>
        {children}
        {detail !== undefined && (
          <span className="ml-1.5 text-[11px] text-muted-foreground/80">{detail}</span>
        )}
      </span>
    </div>
  )
}

/**
 * One certificate check on one line (Varun, 2026-11-26: "line by line with a
 * right tick at the end with icons ... so that the whole thing looks very
 * professional"): the check's icon, its full name, the measured evidence,
 * and the verdict at the right edge -- a tick, a cross, "n/a" (nothing to
 * check on this part) or a dashed "not run" (not run is never a pass).
 *
 * The row lays itself out by the width of the card it sits in (the nearest
 * `@container`, which `CardShell` provides). In a wide card it is four
 * aligned columns: icon | name | value | verdict. In a narrow card -- the
 * docked Blackboard is 320px -- it is still ONE line (Varun, 2026-11-27:
 * "every check should be single line only"): the name is never wrapped or
 * broken, and the value sits right-aligned beside the verdict, using
 * whatever that row has left; only a value that cannot fit is cut with an
 * ellipsis, and it stays readable on hover.
 */
export function CertRow({
  status,
  name,
  value,
  icon: Icon,
  title,
  printIndex = 0,
}: {
  /** This row's place in the card's print order (set by `CertRows`). */
  printIndex?: number
  status: boolean | null | 'na'
  /** The full claim, never abbreviated. */
  name: string
  /** The measured evidence ("0 open edges"), when the certificate carries it. */
  value?: ReactNode
  icon: ComponentType<{ size?: number | string; className?: string }>
  /** Hover text: the claim and its verdict word. */
  title: string
}) {
  const verdict =
    status === true ? 'proven' : status === false ? 'failed' : status === 'na' ? 'not applicable' : 'not run'
  const printing = useContext(PrintingContext)
  return (
    <div
      {...printLine(printing, printIndex)}
      title={title}
      aria-label={`${name}: ${verdict}`}
      className={cn(
        'col-span-full grid grid-cols-subgrid items-center border-b border-border/30 py-1 text-[11px] leading-4 last:border-b-0',
        status === false ? 'text-foreground' : status === true ? 'text-foreground/90' : 'text-foreground/70',
      )}
      role="row"
    >
      {/* A quiet line icon (cert-icons.tsx): it marks the row; the name
          beside it carries the meaning, and the verdict keeps the colour. */}
      <span role="presentation" className="inline-flex items-center text-muted-foreground">
        <Icon size={16} />
      </span>
      <span className="flex min-w-0 items-baseline justify-between gap-3 @xs:contents">
        <span role="cell" className={cn('shrink-0 whitespace-nowrap', status === false && 'font-medium')}>
          {name}
        </span>
        <span
          role="cell"
          title={typeof value === 'string' ? value : undefined}
          className="cad-readout min-w-0 truncate text-right font-mono text-[11px] text-muted-foreground empty:hidden @xs:pl-3"
        >
          {value}
        </span>
      </span>
      <span
        role="cell"
        // The verdict is stamped after its row has printed.
        data-print={printing ? 'stamp' : undefined}
        style={printing ? ({ '--i': printIndex } as CSSProperties) : undefined}
        className={cn(
          'col-end-[-1] inline-flex origin-right items-center justify-end pl-3',
          status === true && 'text-emerald-600 dark:text-emerald-400',
          status === false && 'text-red-600 dark:text-red-400',
          status === null && 'text-caution',
          status === 'na' && 'font-mono text-[11px] text-muted-foreground',
        )}
      >
        {status === true ? (
          <Check size={16} />
        ) : status === false ? (
          <X size={16} />
        ) : status === 'na' ? (
          'n/a'
        ) : (
          <span className="whitespace-nowrap rounded border border-dashed border-caution-border px-1 font-mono text-[11px] leading-4">
            not run
          </span>
        )}
      </span>
    </div>
  )
}

/**
 * The aligned grid `CertRow`s sit in. Three columns in a narrow card
 * (icon | name ... value | verdict, each row sharing its own line), four in
 * a wide one (icon | name | value | verdict, aligned down the card). The
 * name column is as wide as the longest name. Words are never split: the Blackboard's
 * message wrapper sets `break-words` for prose, which would otherwise cut a
 * claim name in half ("Watertig / ht") the moment a column ran short.
 */
interface CertRowsProps {
  children: ReactNode
  /** The print-order index of the first row (set by `CardShell`). */
  printStart?: number
}

export function CertRows({ children, printStart = 0 }: CertRowsProps) {
  let row = 0
  const rows = Children.map(children, (child) =>
    isValidElement<{ printIndex?: number }>(child) ? cloneElement(child, { printIndex: printStart + row++ }) : child,
  )
  return (
    <div
      role="table"
      className="mt-2 grid grid-cols-[24px_minmax(0,1fr)_auto] border-t border-border/60 [overflow-wrap:normal] [word-break:normal] @xs:grid-cols-[24px_max-content_minmax(0,1fr)_auto]"
    >
      {rows}
    </div>
  )
}

/**
 * Compact tri-state BADGE — the row counterpart to `Claim`, for a set of
 * invariants that are each a single word or two ("watertight", "manifold")
 * rather than a sentence. A wrapped row of these reads as one glance: a
 * sound part is a row of green, and a single failure cannot hide the way it
 * could inside nine stacked lines that all say "fine" (Varun, 2026-08-01 —
 * `Claim`'s vertical list was correct to kill the run-on prose it replaced,
 * but a row of badges is what a certificate with mostly-passing invariants
 * should look like).
 *
 * Same tri-state contract as `Claim`: `status === null` is "not run", kept
 * visually distinct (dashed amber) from both pass and fail — a check that
 * did not run is not a check that passed.
 *
 * `glyph` is an OPTIONAL, genuinely-recognisable pictograph for the concept
 * itself (a droplet for "watertight", an eye for the dual-eye consistency
 * check) — supplied ONLY where the mark is self-evident without a private
 * convention. Every other invariant renders its short text label instead: a
 * cryptic icon nobody can decode is worse than the wall of text it replaced.
 * Either way the full claim — long label, "proven"/"FAILED"/"not run", and
 * any extra detail — is on hover via `title`, and via `aria-label` so the
 * concept is never icon-only for assistive tech.
 */
export function ClaimBadge({
  status,
  label,
  detail,
  glyph: Glyph,
}: {
  /** `'na'`: the check does not apply to this part — neither a pass nor a
   *  fail, and not a missing run either. Shown plain grey with "n/a". */
  status: boolean | null | 'na'
  /** Short text shown when no `glyph` is supplied — also always the
   *  accessible name, so an icon-only badge is never unlabelled. */
  label: string
  /** Full hover text (the long-form claim, e.g. "watertight: FAILED"). */
  detail: string
  glyph?: ComponentType<{ size?: number | string; className?: string }>
}) {
  return (
    <span
      title={detail}
      aria-label={detail}
      className={cn(
        'inline-flex items-center gap-1 rounded border px-1.5 py-[3px] text-[11px] leading-none',
        status === true && 'border-emerald-500/30 bg-emerald-500/10 text-emerald-800 dark:text-emerald-300',
        status === false && 'border-red-500/40 bg-red-500/10 text-red-800 dark:text-red-300',
        status === null &&
          'border-dashed border-caution-border bg-caution-wash text-caution ',
        status === 'na' && 'border-border bg-transparent text-muted-foreground',
      )}
    >
      <span
        className={cn(
          'inline-flex shrink-0',
          status === true && 'text-emerald-600 dark:text-emerald-400',
          status === false && 'text-red-600 dark:text-red-400',
          status === null && 'text-caution ',
          status === 'na' && 'font-mono text-[11px] text-muted-foreground',
        )}
      >
        {status === true ? (
          <Check size={10} />
        ) : status === false ? (
          <X size={10} />
        ) : status === 'na' ? (
          'n/a'
        ) : (
          <CircleSlash size={10} />
        )}
      </span>
      {Glyph ? <Glyph size={10} /> : <span>{label}</span>}
    </span>
  )
}

