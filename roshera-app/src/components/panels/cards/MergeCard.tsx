import { GitMerge } from 'lucide-react'
import type {
  MergeCard as MergeCardData,
  MergeConflictWitness,
} from '@/lib/blackboard-cards'
import { CardShell, Chip } from './card-chrome'
import { fmtNum } from './format'

/**
 * BRANCH MERGE RESULT CARD
 * ========================
 * Renders `MergeView` (api-server/src/branches.rs) without reinterpretation:
 * the merge's own evidence, not a bare bool. A clean merge shows the fold
 * statistics; a blocked merge shows each TYPED conflict — the subject, the
 * taxonomy verdict, and BOTH witness events (the colliding operations
 * themselves), because an agent resolves on the divergence shape, never on
 * prose. The `summary` line is derived from the typed fields and shown as
 * the human reading.
 */

function humanizeConflictType(t: string): string {
  return t.replace(/_/g, ' ')
}

function Witness({ side, w }: { side: 'source' | 'target'; w: MergeConflictWitness }) {
  // RFC 3339 → keep the time portion for the compact row; the full stamp
  // stays on the tooltip.
  const time = /T(\d{2}:\d{2}:\d{2})/.exec(w.timestamp)?.[1] ?? w.timestamp
  return (
    <div className="flex flex-wrap items-baseline gap-x-2 text-[10px] text-muted-foreground" title={w.timestamp}>
      <span className="w-10 shrink-0 font-mono uppercase tracking-wide">{side}</span>
      <span className="cad-readout text-foreground/85">{w.operation_type}</span>
      <span>by {w.author}</span>
      <span>seq {w.sequence_number}</span>
      <span>{time}</span>
    </div>
  )
}

export function MergeCard({ card }: { card: MergeCardData }) {
  const stats = card.statistics
  return (
    <CardShell
      accent={card.success ? 'pass' : 'warn'}
      icon={GitMerge}
      title={
        card.success ? (
          <>
            Merged{card.source ? ` ${card.source}` : ''} into{' '}
            <span className="cad-readout">{card.merged_into}</span>
          </>
        ) : (
          <>
            Merge into <span className="cad-readout">{card.merged_into}</span> blocked —{' '}
            {card.conflicts.length} conflict{card.conflicts.length === 1 ? '' : 's'}
          </>
        )
      }
      chip={
        <Chip accent={card.success ? 'pass' : 'warn'}>
          {card.strategy ?? 'merge'}
          {card.success ? ' · clean' : ' · conflicts'}
        </Chip>
      }
    >
      {stats !== undefined && (
        <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 text-[10px] text-muted-foreground">
          <span>
            <span className="cad-readout text-foreground/85">{stats.events_merged}</span> events merged
          </span>
          <span>
            <span className="cad-readout text-foreground/85">{stats.auto_resolved}</span> auto-resolved
          </span>
          <span>
            <span className="cad-readout text-foreground/85">{stats.entities_affected}</span> entities
          </span>
          <span>
            <span className="cad-readout text-foreground/85">{fmtNum(stats.duration_ms)}</span> ms
          </span>
        </div>
      )}

      {card.conflicts.length > 0 && (
        <div className="mt-1.5 space-y-1.5">
          {card.conflicts.map((c, i) => (
            <div key={i} className="rounded border border-amber-500/25 bg-amber-500/5 px-2 py-1.5">
              <div className="flex flex-wrap items-center gap-1.5">
                {/* Geometry-anchoring slice: the conflict subject
                    ("solid:0", "entity:<uuid>") is the attach point for
                    highlighting the contested body in the viewport once
                    persistent-ID selection integration lands. */}
                <span className="cad-readout rounded border border-border px-1 py-px text-[10px] text-foreground/90">
                  {c.subject}
                </span>
                <span className="rounded border border-amber-500/40 px-1 py-px text-[10px] text-amber-400">
                  {humanizeConflictType(c.conflict_type)}
                </span>
              </div>
              <div className="mt-1 text-[11px] text-foreground/85">{c.summary}</div>
              <div className="mt-1 space-y-0.5">
                {c.source_event != null && <Witness side="source" w={c.source_event} />}
                {c.target_event != null && <Witness side="target" w={c.target_event} />}
              </div>
            </div>
          ))}
        </div>
      )}
    </CardShell>
  )
}
