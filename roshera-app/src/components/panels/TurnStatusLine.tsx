/**
 * TURN STATUS LINE
 * ================
 * What the agent is doing RIGHT NOW, shown where its reply arrives (under
 * the reply as it streams, or at the end of the thread before it starts) --
 * in place of a blinking cursor (Varun 2026-11-14): a turning mark, a word
 * for the work, how long it has been going, how much it has moved, and the
 * tool call actually running.
 *
 * The verb is play, not information, and is drawn only from mechanical
 * engineering so it reads as the shop floor rather than as noise. Everything
 * else on the line is real: the clock starts when the turn starts, the token
 * figure is this turn's own (session total minus the total at turn start),
 * and the activity is the ACP tool call in flight or, on the claude-code
 * path where tools run inside the CLI and no tool_call frame ever arrives,
 * the operation the kernel itself most recently served for this agent
 * (`lib/agent-activity.ts`) — never guessed. This is the turn's ONLY status
 * row; the reply line no longer carries its own. Between turns it is gone.
 */

import { useEffect, useMemo, useState } from 'react'
import {
  Square,
} from '@/components/ui/icons'
import { useBlackboardStore } from '@/stores/blackboard-store'
import { useAcpSessionStore } from '@/stores/acp-session-store'
import { useObservedTurnActivity } from '@/lib/agent-activity'
import { cancelAcpTurn } from '@/lib/acp-blackboard'

/** Shop-floor verbs, cycled while a turn runs. */
const SHOP_VERBS = [
  'Machining',
  'Deburring',
  'Chamfering',
  'Filleting',
  'Tolerancing',
  'Dimensioning',
  'Annealing',
  'Tempering',
  'Quenching',
  'Forging',
  'Milling',
  'Turning',
  'Boring',
  'Reaming',
  'Tapping',
  'Knurling',
  'Honing',
  'Lapping',
  'Broaching',
  'Shimming',
  'Torquing',
  'Brazing',
  'Riveting',
  'Extruding',
  'Casting',
  'Sintering',
  'Drafting',
  'Gauging',
  'Datuming',
  'Squaring',
  'Truing',
  'Facing',
  'Counterboring',
  'Countersinking',
  'Spot-facing',
  'Indexing',
  'Fixturing',
  'Hardening',
  'Polishing',
  'Anodizing',
  'Stress-relieving',
  'Load-testing',
] as const

/** The turning mark: the same breathing star Claude Code uses. */
const MARK_FRAMES = ['·', '✢', '✳', '✶', '✻', '✽', '✻', '✶', '✳', '✢'] as const

const FRAME_MS = 120
const VERB_MS = 3200

function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
}

function compact(n: number): string {
  if (n < 1000) return String(n)
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`
  return `${(n / 1_000_000).toFixed(1)}M`
}

function elapsedLabel(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`
}

export function TurnStatusLine() {
  const isProcessing = useBlackboardStore((s) => s.isProcessing)
  const startedAt = useBlackboardStore((s) => s.turnStartedAt)
  const activity = useBlackboardStore((s) => s.currentActivity)
  const attention = useBlackboardStore((s) => s.agentAttention)
  const streaming = useBlackboardStore((s) => s.streamingLineId !== null)
  const tokensUsed = useAcpSessionStore((s) => s.tokensUsed)
  const turnStartTokens = useAcpSessionStore((s) => s.turnStartTokens)

  const reduced = useMemo(() => prefersReducedMotion(), [])
  const [now, setNow] = useState(() => Date.now())
  // The turn (by its start time) a Stop was pressed for, so "Stopping…"
  // belongs to that turn only and the next turn starts clean.
  const [stoppedTurn, setStoppedTurn] = useState<number | null>(null)
  const observed = useObservedTurnActivity(isProcessing)
  const stopping = stoppedTurn !== null && stoppedTurn === startedAt

  useEffect(() => {
    if (!isProcessing) return
    const id = window.setInterval(() => setNow(Date.now()), reduced ? 1000 : FRAME_MS)
    return () => window.clearInterval(id)
  }, [isProcessing, reduced])

  if (!isProcessing) return null

  const start = startedAt ?? now
  const elapsed = now - start
  const mark = reduced ? '✻' : MARK_FRAMES[Math.floor(elapsed / FRAME_MS) % MARK_FRAMES.length]
  // Offset by the start time so consecutive turns do not open on the same word.
  const verb = SHOP_VERBS[(Math.floor(elapsed / VERB_MS) + Math.floor(start / 1000)) % SHOP_VERBS.length]
  const turnTokens =
    tokensUsed !== null && turnStartTokens !== null ? Math.max(0, tokensUsed - turnStartTokens) : null

  const observedLabel =
    observed.kind === 'operation' ? (observed.label ?? 'unnamed operation') : null
  const doing = activity
    ? activity
    : observedLabel
      ? observedLabel
      : attention === 'geometry'
        ? 'working on the model'
        : streaming
          ? 'writing the reply'
          : 'thinking — no kernel operation yet'
  const opCount = observed.kind === 'operation' ? observed.opsThisTurn : 0

  return (
    <div
      role="status"
      aria-live="polite"
      className="py-1 text-xs"
    >
      <div className="flex items-baseline gap-2">
        <span aria-hidden className="w-3 shrink-0 text-center text-primary">
          {mark}
        </span>
        <span className="font-medium text-foreground/90">{verb}…</span>
        <span
          className="text-muted-foreground/70 tabular-nums"
          title="Elapsed time, tokens moved this turn, and operations the kernel has served for it. Turns typically take 60-90 seconds."
        >
          ({elapsedLabel(elapsed)}
          {/* Usage arrives at turn end on most providers: "0 tokens" mid-turn
              would read as a measurement, so the figure waits until it is one. */}
          {turnTokens !== null && turnTokens > 0 ? ` · ${compact(turnTokens)} tokens` : ''}
          {opCount > 0 ? ` · ${opCount} op${opCount === 1 ? '' : 's'}` : ''})
        </span>
        <button
          type="button"
          onClick={() => {
            setStoppedTurn(startedAt)
            cancelAcpTurn()
          }}
          disabled={stopping}
          className="ml-auto inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-accent/30 hover:text-foreground disabled:opacity-50"
          title="End this turn"
          aria-label="Stop agent turn"
        >
          <Square size={8} />
          {stopping ? 'Stopping…' : 'Stop'}
        </button>
      </div>
      <div className="flex min-w-0 items-baseline gap-2 pl-5 text-muted-foreground">
        <span aria-hidden className="shrink-0 text-muted-foreground/50">
          ⎿
        </span>
        <span className="truncate font-mono text-[11px]" title={doing}>
          {doing}
        </span>
      </div>
    </div>
  )
}
