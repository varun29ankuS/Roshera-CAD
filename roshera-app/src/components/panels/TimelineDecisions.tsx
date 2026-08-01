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
 *  - checkpoints are VOLATILE (they do not survive a restart) and the
 *    policy that the agent declares one before every feature is not yet
 *    enforced — so "N ops recorded without a named decision" is usually
 *    the truth, and this row says exactly that;
 *  - a quarantined boot (the kernel refusing to replay a tail it cannot
 *    certify) is disclosed in the strip header via `DurabilityChip` —
 *    amber, dashed, CircleSlash: the same "withheld / not checked"
 *    vocabulary the Blackboard cards already use. Never styled as an
 *    error — it is the kernel keeping its no-lying promise.
 */
import { CircleSlash, X } from 'lucide-react'
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
          'No checkpoints exist on this document right now. Checkpoints are volatile — ' +
          'they do not survive a server restart — and the agent policy of declaring one ' +
          'before each feature is not yet enforced, so raw operations routinely outlive ' +
          'the decisions that produced them.'
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
