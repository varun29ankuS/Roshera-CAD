/**
 * The DECISION layer of the timeline strip: named design states
 * (checkpoints) + the durability boot outcome.
 *
 * Organising idea (vault `Research/2026-07-29-timeline-beyond-git.md`,
 * ui-pass-spec §0/§3): the raw event log answers "what happened"; this
 * row answers "what was DECIDED". A checkpoint is a declared intent —
 * "bolt circle, 8×⌀18" — covering a span of raw operations, and it is
 * the unit a person actually interrogates. The row must be readable
 * without a mouse: name, covered range, author, age all visible; the
 * description and exact timestamp ride on hover.
 *
 * The two honest gaps are rendered as first-class states, not hidden:
 *  - a span of operations can lack a covering checkpoint: the MCP
 *    dispatch gate refuses an agent's solid-mutating call with no open
 *    intent (roshera-mcp gates.ts), but UI actions and direct REST
 *    clients carry no such gate, and history recorded before the gate
 *    existed has none — so "N ops recorded without a named decision"
 *    can still be the truth, and this row says exactly that.
 *    (Checkpoints themselves are durable — persisted on creation and
 *    restored at boot, api-server durability.rs.);
 *  - a quarantined boot (the kernel refusing to replay a tail it cannot
 *    certify) is disclosed in the strip header via `DurabilityChip` —
 *    amber, dashed, CircleSlash: the same "withheld / not checked"
 *    vocabulary the Blackboard cards already use. Never styled as an
 *    error — it is the kernel keeping its no-lying promise.
 */
import { useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, CircleSlash, Clock, Ruler, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import {
  type CheckpointSummary,
  type DurabilityNotice,
  formatEventRange,
  formatTimestamp,
  relativeTime,
  shortLabel,
  symbolForOperation,
} from '@/lib/timeline-events'

// ─── Durability chip (strip header) ─────────────────────────────────
//
// Tri-state colour language borrowed from `cards/card-chrome.tsx`:
// amber dashed = withheld/not-checked, red = failure, neutral = off.
// Colour carries STATE only; the calm cases (full replay, empty log)
// produce no chip at all — `durabilityNotice` returns null for them.

export function DurabilityChip({ notice }: { notice: DurabilityNotice }) {
  return (
    <span
      title={notice.detail}
      aria-label={notice.detail}
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded border px-1.5 py-[2px] text-[10px] leading-none whitespace-nowrap',
        notice.tone === 'withheld' &&
          'border-dashed border-amber-500/40 bg-amber-500/5 text-amber-800 dark:text-amber-300',
        notice.tone === 'failed' &&
          'border-red-500/40 bg-red-500/10 text-red-800 dark:text-red-300',
        notice.tone === 'off' &&
          'border-dashed border-border text-muted-foreground',
      )}
    >
      {notice.tone === 'failed' ? (
        <X size={10} className="shrink-0" />
      ) : (
        <CircleSlash size={10} className="shrink-0" />
      )}
      {notice.label}
    </span>
  )
}

// ─── Decision rail (between the controls row and the ops strip) ─────

function DecisionChip({
  cp,
  onOpen,
}: {
  cp: CheckpointSummary
  onOpen?: (cp: CheckpointSummary) => void
}) {
  const hover = [
    cp.description || '(no description recorded)',
    `${formatTimestamp(cp.timestamp)} · ${cp.author}`,
    cp.tags.length > 0 ? `tags: ${cp.tags.join(', ')}` : '',
    onOpen ? 'click → open on the map' : '',
  ]
    .filter(Boolean)
    .join('\n')
  return (
    <button
      type="button"
      onClick={onOpen ? () => onOpen(cp) : undefined}
      title={hover}
      className={cn(
        'shrink-0 inline-flex items-baseline gap-1.5 rounded border border-border/70 px-2 py-0.5 text-[11px] leading-tight',
        'text-foreground/90 transition-colors',
        onOpen ? 'hover:bg-accent/40 hover:border-foreground/30 cursor-pointer' : 'cursor-default',
      )}
    >
      <span aria-hidden className="text-foreground/60">◈</span>
      {/* 44ch fits real engineering names ("M8 clearance holes, close
          fit, 4x base corners" is 44 chars — measured against the first
          live intent declared through the new picker) without letting a
          run-on paragraph eat the rail. */}
      <span className="font-medium max-w-[44ch] truncate">{cp.name}</span>
      <span className="text-muted-foreground/80 font-mono text-[10px]">
        {formatEventRange(cp.event_range)}
      </span>
      <span className="text-muted-foreground/60 text-[10px]">
        {cp.author} · {relativeTime(cp.timestamp)}
      </span>
    </button>
  )
}

/**
 * One horizontal line of declared intents, oldest → newest, newest
 * nearest the eye (rightmost, same reading direction as the ops strip
 * below it). Renders nothing at all when there are neither events nor
 * checkpoints — the ops strip's own empty state speaks then, and two
 * stacked empty rows would be noise.
 */
export function DecisionRail({
  checkpoints,
  eventCount,
  onOpen,
}: {
  checkpoints: CheckpointSummary[]
  eventCount: number
  /** Open the map view focused on structure — where a decision's covered
   *  operations carry its name. Optional; chips degrade to read-only. */
  onOpen?: (cp: CheckpointSummary) => void
}) {
  if (checkpoints.length === 0) {
    if (eventCount === 0) return null
    // The common, true, and unflattering state: work happened, nobody
    // declared what it was for. Saying so is the point — the gap in the
    // decision record is itself information a reviewer needs.
    return (
      <div
        className="flex items-center gap-1.5 px-3 py-1 text-[11px] text-muted-foreground/60"
        title={
          'No checkpoints exist on this document right now. Agents must open a named ' +
          'intent before mutating the model, but operations made from the UI or by ' +
          'direct API calls — and history recorded before that gate existed — carry no ' +
          'declared decision, so raw operations can outlive the intent behind them.'
        }
      >
        <span aria-hidden className="text-muted-foreground/50">◈</span>
        <span>
          no declared intents — {eventCount} op{eventCount === 1 ? '' : 's'} recorded without
          a named decision
        </span>
      </div>
    )
  }
  return (
    <div className="flex items-center gap-1.5 px-3 py-1 overflow-x-auto whitespace-nowrap">
      <span
        aria-hidden
        className="shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground/50"
        title="Named design states — what each span of operations was for"
      >
        decisions
      </span>
      {checkpoints.map((cp) => (
        <DecisionChip key={cp.id} cp={cp} onOpen={onOpen} />
      ))}
    </div>
  )
}

// ═══ Decision list — the map's PRIMARY view ══════════════════════════
//
// The map used to open as a field of operation cards (`create_box_3d`,
// `transform_solid`, `set_name` …). Varun, 2026-08-09, looking at a real
// document: "its just a wall of cards ... who can zoom in and look at it
// in detail .. nobody". The operations were never the thing a person
// came to read — the DECISIONS were, and on a real part their names and
// descriptions carry the engineering rationale ("Raised face D102 x
// 10 mm … height 10 mm is the engineer's call (not a standard value)").
//
// So the ordering inverts: decisions are the list, operations are the
// drill-down inside a decision. The graph is not deleted — it moves
// behind a tab, because "what derives from what" is a real question,
// just not the FIRST one.
//
// Nothing is hidden by the inversion: every operation not covered by any
// decision on this branch lands in one trailing "unattributed" row, so
// the count of rows always accounts for the whole branch.

/**
 * The minimal per-operation shape this view needs. `LineageMapNode`
 * (`lib/timeline-events.ts`) satisfies it structurally, and passing one
 * is the intended call — declaring it here rather than importing keeps
 * the dependency one-way (`TimelineGraph` → this module, never back;
 * this module is loaded eagerly by the strip and must never drag
 * `@xyflow` into the initial bundle).
 */
export interface DecisionOp {
  id: string
  sequence_number: number
  operation_type: string
}

/**
 * How the active branch's operation list arrived. Typed rather than
 * `DecisionOp[] | null` because "still loading", "the endpoint refused"
 * and "genuinely zero operations" are three different facts and must
 * never share a rendering — the same rule the graph tab already follows.
 */
export type DecisionOpsState =
  | { state: 'loading' }
  | { state: 'ready'; ops: DecisionOp[]; truncated?: boolean }
  | { state: 'refused'; reason: string }
  | { state: 'unreachable'; reason: string }

// Branch-id spelling, reconciled locally. `GET /api/branches` spells the
// trunk as the nil UUID; a checkpoint spells the SAME branch as the
// literal "main". `lib/timeline-events.ts` keeps this reconciliation
// private to `checkpointCovering`, and its own comment records that
// `Timeline.tsx` and `TimelineGraph.tsx` each hold an independent copy
// of the literal for their own bookkeeping — this is the third, for the
// same reason. Get it wrong and the default tab on the default branch
// renders an empty list over a document full of decisions.
const MAIN_BRANCH_ID = '00000000-0000-0000-0000-000000000000'

function canonicalBranchId(id: string): string {
  return id === 'main' ? MAIN_BRANCH_ID : id
}

/**
 * The span of events a decision claims. `covers` is what it actually
 * AUTHORED; `event_range` is the restore marker, which starts at 0 and
 * therefore NESTS (see the field docs in `lib/timeline-events.ts`). The
 * fallback is deliberate — degrading to overlapping rows is honest,
 * inventing an authored span from `previous.end + 1` would not be.
 */
function spanOf(cp: CheckpointSummary): [number, number] {
  return cp.covers ?? cp.event_range
}

function spansAuthoredNothing(cp: CheckpointSummary): boolean {
  const [a, b] = spanOf(cp)
  return a > b
}

/**
 * A standard cited anywhere in the decision's name or description —
 * "ISO 273", "ASME B18.2.2", "DIN 912" — returned VERBATIM, or `null`.
 * First match only: the chip is a recognition aid, not a bibliography.
 * Deliberately not global (no `g`), so there is no `lastIndex` to leak
 * between calls.
 */
const STANDARD_CITATION = /\b(ASME|ISO|EN|DIN|ASTM|IEC)[\s-]?[A-Z]?[\d.-]+[^\s,;)]*/

function citedStandard(cp: CheckpointSummary): string | null {
  return `${cp.name} ${cp.description}`.match(STANDARD_CITATION)?.[0] ?? null
}

/**
 * Decisions declared on `branchId`, oldest span-end first — reading
 * order is build order. Ties break on timestamp then id so the list is
 * stable across the strip's 5s poll instead of shuffling under the eye.
 */
function decisionsOnBranch(
  checkpoints: CheckpointSummary[],
  branchId: string,
): CheckpointSummary[] {
  const target = canonicalBranchId(branchId)
  return checkpoints
    .filter((cp) => canonicalBranchId(cp.branch_id ?? 'main') === target)
    .sort((a, b) => {
      const bySpan = spanOf(a)[1] - spanOf(b)[1]
      if (bySpan !== 0) return bySpan
      const byTime = Date.parse(a.timestamp) - Date.parse(b.timestamp)
      if (Number.isFinite(byTime) && byTime !== 0) return byTime
      return a.id.localeCompare(b.id)
    })
}

// ─── Chips (card-chrome's vocabulary; colour stays reserved for STATE) ──
//
// A cited standard is not a state — it is neither a pass, a failure, nor
// a refusal — so it is NOT given one of the four state colours. It is
// distinguished the way `ClaimBadge` distinguishes a concept: by icon,
// weight and fill.

function StandardChip({ text }: { text: string }) {
  return (
    <span className="inline-flex shrink-0 items-center gap-1 rounded border border-foreground/25 bg-accent/60 px-1.5 py-[3px] text-[10px] font-medium leading-none text-foreground">
      <Ruler size={10} className="shrink-0" aria-hidden />
      {text}
    </span>
  )
}

function TimeChip({ timestamp }: { timestamp: string }) {
  return (
    <span className="inline-flex shrink-0 items-center gap-1 rounded border border-border/70 px-1.5 py-[3px] text-[10px] leading-none text-muted-foreground">
      <Clock size={10} className="shrink-0" aria-hidden />
      <span className="font-mono">{formatTimestamp(timestamp)}</span>
      <span className="text-muted-foreground/70">{relativeTime(timestamp)}</span>
    </span>
  )
}

function CountChip({ label, dashed = false }: { label: string; dashed?: boolean }) {
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center rounded border bg-accent/25 px-1.5 py-[3px] font-mono text-[10px] leading-none text-foreground/80',
        dashed ? 'border-dashed border-border' : 'border-border/70',
      )}
    >
      {label}
    </span>
  )
}

/** One operation, as the glyph vocabulary the rest of the app already
 *  uses (`symbolForOperation` / `shortLabel`) plus its sequence number.
 *  Neutral by design: family COLOUR is the graph tab's language and is
 *  defined there — duplicating the palette here would give the app two
 *  copies to drift apart. */
function OpChip({ op }: { op: DecisionOp }) {
  return (
    <span className="inline-flex items-center gap-1 rounded border border-border/60 bg-card px-1.5 py-[3px] font-mono text-[10px] leading-none text-foreground/80">
      <span aria-hidden className="text-[11px] leading-none text-foreground/60">
        {symbolForOperation(op.operation_type)}
      </span>
      {shortLabel(op.operation_type)}
      <span className="text-muted-foreground/60">#{op.sequence_number}</span>
    </span>
  )
}

/** The operation chips, or the honest account of why there are none.
 *  `ops === null` means the list was never read — never rendered as an
 *  empty set. */
function OpsBody({
  ops,
  unread,
  emptyNote,
}: {
  ops: DecisionOp[] | null
  unread: string | null
  emptyNote: string
}) {
  if (ops === null) {
    return <div className="text-[11px] italic text-muted-foreground/70">{unread}</div>
  }
  if (ops.length === 0) {
    return <div className="text-[11px] italic text-muted-foreground/70">{emptyNote}</div>
  }
  return (
    <div className="flex flex-wrap gap-1">
      {ops.map((op) => (
        <OpChip key={op.id} op={op} />
      ))}
    </div>
  )
}

// ─── One decision row ────────────────────────────────────────────────

function DecisionListRow({
  cp,
  step,
  ops,
  unread,
  expanded,
  onToggle,
}: {
  cp: CheckpointSummary
  step: number
  /** Operations on this branch inside this decision's span, or `null`
   *  when the operation list could not be read. */
  ops: DecisionOp[] | null
  /** Short reason the operation list is absent (`null` when it is not). */
  unread: string | null
  expanded: boolean
  onToggle: () => void
}) {
  const standard = citedStandard(cp)
  const authoredNothing = spansAuthoredNothing(cp)
  const bodyId = `decision-body-${cp.id}`
  const count =
    ops === null
      ? 'ops not read'
      : authoredNothing
        ? 'authored nothing'
        : `${ops.length} op${ops.length === 1 ? '' : 's'}`

  return (
    <div className="rounded-md border border-border/70 border-l-2 border-l-[var(--primary)] bg-card/50">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        aria-controls={bodyId}
        className="flex w-full items-center gap-2 px-2 py-1.5 text-left hover:bg-accent/30"
      >
        {expanded ? (
          <ChevronDown size={13} className="shrink-0 text-muted-foreground" aria-hidden />
        ) : (
          <ChevronRight size={13} className="shrink-0 text-muted-foreground" aria-hidden />
        )}
        <span
          aria-hidden
          className="flex h-5 w-5 shrink-0 items-center justify-center rounded bg-accent/70 font-mono text-[10px] text-foreground"
        >
          {step}
        </span>
        {/* Truncated while collapsed, FULL while expanded — a long name is
            never left readable only on hover. */}
        <span
          className={cn(
            'min-w-0 flex-1 text-[12.5px] font-medium text-foreground',
            expanded ? 'whitespace-normal break-words' : 'truncate',
          )}
        >
          {cp.name}
        </span>
        {standard && <StandardChip text={standard} />}
        <TimeChip timestamp={cp.timestamp} />
        <CountChip label={count} dashed={ops === null || authoredNothing} />
      </button>
      {expanded && (
        <div id={bodyId} className="space-y-1.5 border-t border-border/50 px-3 py-2">
          {cp.description ? (
            <p className="max-w-[95ch] whitespace-pre-wrap text-[11.5px] leading-snug text-foreground/85">
              {cp.description}
            </p>
          ) : (
            <p className="text-[11.5px] italic text-muted-foreground/70">
              no description recorded — the name is all this decision says
            </p>
          )}
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[10px] text-muted-foreground/80">
            <span className="font-mono">{formatEventRange(spanOf(cp))}</span>
            <span aria-hidden className="text-muted-foreground/40">
              │
            </span>
            <span>{cp.author}</span>
            <span aria-hidden className="text-muted-foreground/40">
              │
            </span>
            <span className="font-mono">{formatTimestamp(cp.timestamp)}</span>
            {cp.covers === undefined && (
              <span className="text-muted-foreground/70">
                span is a restore marker, not an authored range
              </span>
            )}
            {cp.tags.length > 0 && (
              <span className="flex flex-wrap gap-1">
                {cp.tags.map((t) => (
                  <span key={t} className="rounded border border-border/60 px-1 py-[1px]">
                    {t}
                  </span>
                ))}
              </span>
            )}
          </div>
          <OpsBody
            ops={ops}
            unread={unread}
            emptyNote={
              authoredNothing
                ? 'this decision authored no operations — it names an intent and nothing followed it'
                : 'no operations on this branch fall inside this span'
            }
          />
        </div>
      )}
    </div>
  )
}

/** The trailing row for everything no decision claims. Dashed and muted
 *  — the same "honest gap" vocabulary the strip's empty rail and the
 *  graph's unlinked band already use. Never a colour: an undeclared
 *  operation is a gap in the record, not a failure of the kernel. */
function UnattributedRow({
  ops,
  expanded,
  onToggle,
}: {
  ops: DecisionOp[]
  expanded: boolean
  onToggle: () => void
}) {
  return (
    <div className="rounded-md border border-dashed border-border bg-card/30">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        aria-controls="decision-body-unattributed"
        className="flex w-full items-center gap-2 px-2 py-1.5 text-left hover:bg-accent/30"
      >
        {expanded ? (
          <ChevronDown size={13} className="shrink-0 text-muted-foreground" aria-hidden />
        ) : (
          <ChevronRight size={13} className="shrink-0 text-muted-foreground" aria-hidden />
        )}
        <span
          aria-hidden
          className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-dashed border-muted-foreground/60 text-[10px] text-muted-foreground"
        >
          ?
        </span>
        <span className="min-w-0 flex-1 truncate text-[12.5px] text-muted-foreground">
          Unattributed operations ({ops.length})
        </span>
        <CountChip label={`${ops.length} op${ops.length === 1 ? '' : 's'}`} dashed />
      </button>
      {expanded && (
        <div
          id="decision-body-unattributed"
          className="space-y-1.5 border-t border-border/50 px-3 py-2"
        >
          <p className="max-w-[95ch] text-[11.5px] leading-snug text-muted-foreground/80">
            No decision on this branch covers these operations. Agents must open a named intent
            before mutating the model, but operations made from the UI or by direct API calls —
            and history recorded before that gate existed — carry none.
          </p>
          <OpsBody ops={ops} unread={null} emptyNote="" />
        </div>
      )}
    </div>
  )
}

/**
 * The decisions view: one row per declared intent on the active branch,
 * plus one trailing row for everything undeclared.
 *
 * Row membership is plain span containment over the operations actually
 * present — NOT `checkpointCovering`, which answers the inverse question
 * (one operation → its single owning decision) and whose last-wins rule
 * would credit every early operation to the most recent declaration.
 * Counting from the present operations rather than from `b - a + 1` is
 * what keeps the collapsed count equal to the number of chips the
 * expansion renders, under a truncated lineage window and on a child
 * branch that only shows post-fork operations alike.
 */
export function DecisionList({
  checkpoints,
  branchId,
  branchLabel,
  opsState,
  onRetry,
}: {
  checkpoints: CheckpointSummary[]
  /** The branch whose decisions and operations this shows. */
  branchId: string
  /** Human name for that branch, for the empty/summary lines. */
  branchLabel: string
  opsState: DecisionOpsState
  /** Re-read the operation list. Offered only for `unreachable` — a
   *  typed refusal is an ANSWER, not a failed read, and re-asking would
   *  return the same refusal; the graph tab draws the same line. */
  onRetry?: () => void
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const decisions = useMemo(
    () => decisionsOnBranch(checkpoints, branchId),
    [checkpoints, branchId],
  )

  const ops = opsState.state === 'ready' ? opsState.ops : null
  const unread =
    opsState.state === 'loading'
      ? '⋯ reading operations'
      : opsState.state === 'refused'
        ? `operations not listed — the recorded lineage was refused: ${opsState.reason}`
        : opsState.state === 'unreachable'
          ? `operations not read (${opsState.reason}) — the log was not consulted`
          : null

  /** Per-decision operations, and the leftovers, from ONE predicate. */
  const { perDecision, unattributed } = useMemo(() => {
    const perDecision = new Map<string, DecisionOp[]>()
    if (!ops) return { perDecision, unattributed: [] as DecisionOp[] }
    const sorted = [...ops].sort((a, b) => a.sequence_number - b.sequence_number)
    for (const cp of decisions) {
      const [a, b] = spanOf(cp)
      perDecision.set(
        cp.id,
        sorted.filter((op) => op.sequence_number >= a && op.sequence_number <= b),
      )
    }
    const unattributed = sorted.filter(
      (op) =>
        !decisions.some((cp) => {
          const [a, b] = spanOf(cp)
          return op.sequence_number >= a && op.sequence_number <= b
        }),
    )
    return { perDecision, unattributed }
  }, [ops, decisions])

  // Restore markers nest, so rows genuinely overlap and one operation can
  // appear under more than one decision. Disclosed, never smoothed over
  // by inventing an authored span the backend did not send.
  const nesting = decisions.some((cp) => cp.covers === undefined)
  const truncated = opsState.state === 'ready' && opsState.truncated === true

  return (
    <div className="h-full overflow-y-auto px-3 py-2">
      <div className="mb-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-muted-foreground/80">
        <span className="text-foreground/80">
          {decisions.length} decision{decisions.length === 1 ? '' : 's'} on {branchLabel}
        </span>
        {ops !== null && (
          <>
            <span aria-hidden className="text-muted-foreground/40">
              │
            </span>
            <span>
              {ops.length} operation{ops.length === 1 ? '' : 's'}, {unattributed.length}{' '}
              undeclared
            </span>
          </>
        )}
        {unread !== null && (
          <>
            <span aria-hidden className="text-muted-foreground/40">
              │
            </span>
            <span className="italic">{unread}</span>
            {opsState.state === 'unreachable' && onRetry && (
              <button
                type="button"
                onClick={onRetry}
                className="rounded border border-border px-1.5 py-[1px] text-[10px] hover:bg-accent/40"
              >
                retry
              </button>
            )}
          </>
        )}
      </div>

      {(nesting || truncated) && (
        <div className="mb-2 space-y-1 rounded border border-dashed border-amber-500/40 bg-amber-500/5 px-2 py-1.5 text-[10.5px] text-amber-800 dark:text-amber-300">
          {nesting && (
            <div>
              This backend does not send authored spans, so a decision's range is its RESTORE
              marker — it starts at the branch's first event and nests inside the next one. Rows
              therefore overlap: an operation can appear under more than one decision, and the
              counts do not add up to the total.
            </div>
          )}
          {truncated && (
            <div>
              The lineage window filled up — operations outside it are not listed here, so every
              count below is a floor, not a total.
            </div>
          )}
        </div>
      )}

      {decisions.length === 0 && (
        <div className="mb-2 flex items-center gap-1.5 px-1 py-1 text-[11.5px] text-muted-foreground/70">
          <span aria-hidden className="text-muted-foreground/50">
            ◈
          </span>
          <span>
            no declared intents on {branchLabel} — nobody named what this work was for
          </span>
        </div>
      )}

      <div className="space-y-1.5">
        {decisions.map((cp, i) => (
          <DecisionListRow
            key={cp.id}
            cp={cp}
            step={i + 1}
            ops={perDecision.get(cp.id) ?? (ops === null ? null : [])}
            unread={unread}
            expanded={expanded.has(cp.id)}
            onToggle={() => toggle(cp.id)}
          />
        ))}
        {unattributed.length > 0 && (
          <UnattributedRow
            ops={unattributed}
            expanded={expanded.has('unattributed')}
            onToggle={() => toggle('unattributed')}
          />
        )}
      </div>
    </div>
  )
}
