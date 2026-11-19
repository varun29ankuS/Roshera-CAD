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
 *    certify) is disclosed in the strip header via `DurabilityChip` — a
 *    CAUTION pill, which is the shared "withheld / not checked" tone rather
 *    than a look invented here. Never styled as an ERROR: withholding a tail
 *    it cannot certify is the kernel keeping its no-lying promise, and a
 *    negative tone would accuse it of failing at the moment it behaved.
 */
import { StatusPill } from '@/components/ui/status-pill'
import { cn } from '@/lib/utils'
import {
  type CheckpointSummary,
  type DurabilityNotice,
  formatEventRange,
  formatTimestamp,
  relativeTime,
} from '@/lib/timeline-events'

// ─── Durability chip (strip header) ─────────────────────────────────
//
// Tri-state, mapped onto the shared status tones: caution = withheld /
// not-checked, negative = failure, neutral = off. Colour carries STATE only;
// the calm cases (full replay, empty log) produce no chip at all —
// `durabilityNotice` returns null for them, and a component that renders
// nothing when there is nothing to say is the whole point of the row.

export function DurabilityChip({ notice }: { notice: DurabilityNotice }) {
  // Withholding a tail the kernel cannot faithfully replay is a caution, and a
  // failed boot is a negative. Both used bespoke amber/red class stacks that
  // made them a third species next to every other badge on the bar; they wear
  // the one status grammar now, and the ▲ / ✕ come from the tone rather than
  // from a caller remembering an icon import.
  return (
    <StatusPill
      tone={notice.tone === 'failed' ? 'negative' : notice.tone === 'withheld' ? 'caution' : 'neutral'}
      // caution and negative carry their own ▲ / ✕. `off` maps to NEUTRAL,
      // whose glyph is optional — so it is passed explicitly here rather than
      // dropped: the chip used to show a CircleSlash in that state, and a
      // silent relabel would have quietly deleted the signal while the commit
      // claimed to be re-skinning.
      glyph={notice.tone === 'off' ? '○' : undefined}
      label={notice.label}
      title={notice.detail}
      ariaLabel={notice.detail}
      className="shrink-0"
    />
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
        'inline-flex min-w-0 items-baseline gap-1.5 rounded border border-border/70 px-2 py-0.5 text-[11px] leading-tight',
        'text-foreground/90 transition-colors',
        onOpen ? 'hover:bg-accent/40 hover:border-foreground/30 cursor-pointer' : 'cursor-default',
      )}
    >
      <span aria-hidden className="text-foreground/60">◈</span>
      {/* 44ch fits real engineering names ("M8 clearance holes, close
          fit, 4x base corners" is 44 chars — measured against the first
          live intent declared through the new picker) without letting a
          run-on paragraph eat the rail. */}
      <span className="min-w-0 break-words font-medium [display:-webkit-box] [-webkit-box-orient:vertical] [-webkit-line-clamp:2] overflow-hidden leading-snug">
        {cp.name}
      </span>
      {/* `spanOf`, not `cp.event_range`. The raw field is the RESTORE marker
          and always starts at 0, so every chip on the rail read `#0-#N` — the
          same left edge on every card, widening forever, telling the reader
          nothing about which events this decision authored. `spanOf` returns
          `covers` (the authored span) and is what the expanded row at the
          bottom of this file already uses; this chip was the one place still
          reaching past it. */}
      <span className="text-muted-foreground/80 font-mono text-[11px]">
        {formatEventRange(spanOf(cp))}
      </span>
      <span className="text-muted-foreground/60 text-[11px]">
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
        className="flex shrink-0 items-center gap-1.5 px-3 py-1 text-[11px] text-muted-foreground/60"
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
  // ONE decision, plus a count. The rail used to render every checkpoint in a
  // horizontally scrolling row, which turned the most valuable line on the
  // screen into a ticker: four near-identical cards, no marginal information
  // per card, and the newest one liable to be off-screen. The latest decision
  // is the one that describes the model as it stands; the rest are history and
  // history has a drawer.
  const latest = checkpoints[checkpoints.length - 1]

  return (
    // flex-1, not shrink-0 with a 52% ceiling. This line is the only string on
    // the strip written in the product's own language — the human's stated
    // intent — and it was being truncated at the panel edge so that a row of
    // kernel call names could have the width. The ops are the evidence layer;
    // the crime was promotion, not their existence.
    <div className="flex min-w-0 flex-1 items-center gap-2 px-3 py-1">
      <span
        aria-hidden
        className="shrink-0 text-[11px] uppercase tracking-wide text-muted-foreground/50"
        title="Named design states — what each span of operations was for"
      >
        decisions
      </span>
      {latest && (
        <>
          <div className="min-w-0 flex-1">
            <DecisionChip cp={latest} onOpen={onOpen} />
          </div>
          {checkpoints.length > 1 && (
            <button
              type="button"
              className="shrink-0 cursor-pointer appearance-none border-0 bg-transparent p-0 disabled:cursor-default"
              onClick={() => onOpen?.(latest)}
              disabled={!onOpen}
              title="Open the full decision history"
              aria-label={`Show ${checkpoints.length - 1} earlier decisions`}
            >
              <StatusPill tone="neutral" label={`+${checkpoints.length - 1} earlier`} />
            </button>
          )}
        </>
      )}
    </div>
  )
}

/**
 * The span of events a decision claims. `covers` is what it actually
 * AUTHORED; `event_range` is the restore marker, which starts at 0 and
 * therefore NESTS (see the field docs in `lib/timeline-events.ts`).
 */
function spanOf(cp: CheckpointSummary): [number, number] {
  return cp.covers ?? cp.event_range
}
