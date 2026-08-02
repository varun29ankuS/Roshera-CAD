import { useEffect, useState, useCallback, useMemo } from 'react'
import { stringify as yamlStringify } from 'yaml'
import { cn } from '@/lib/utils'
import { processBlackboardMessage } from '@/lib/ai-client'
import { sendPrompt } from '@/lib/blackboard-composer'
import { detectEnumeratedChoices } from '@/lib/blackboard-cards'
import { classifyBlackboardContent } from '@/lib/blackboard-content'
import { lineVerdict, type LineVerdict } from '@/lib/blackboard-line-state'
import { useObservedTurnActivity, type ObservedTurnActivity } from '@/lib/agent-activity'
import type { BlackboardLine as Line } from '@/stores/blackboard-store'
import { MessageMarkdown } from './MessageMarkdown'
import { StreamingLineText } from './StreamingLineText'
import { RevealContext } from './cards/reveal-context'
import { CardActionsContext, type CardActions } from './cards/card-actions-context'
import { DetectedChoicesCard } from './cards/DetectedChoicesCard'
import { Bot, User, Wrench, Trash2, Square, Check, X, CircleSlash } from 'lucide-react'
import type { AgentTurnStatus } from '@/stores/blackboard-store'

interface Props {
  line: Line
  onCommit: (id: string, text: string) => void
  onDelete: (id: string) => void
  /** True while this line is receiving streamed agent text — routes display
   *  through the buffered renderer (math/cards typeset only when complete). */
  streaming?: boolean
  /** Ends the in-flight turn (`session/cancel`). Only rendered while
   *  `streaming` — a user who thinks the turn has hung can end it rather
   *  than reload the page. See `cancelAcpTurn` (`lib/acp-blackboard.ts`). */
  onCancel?: () => void
}

/**
 * ORIGIN MARKER — shape says who wrote it, colour says what happened.
 * ---------------------------------------------------------------------
 * The leading marker already carries authorship (Bot / User / Wrench,
 * below). This is its second, colour channel: when the line carries a
 * verdict (`lineVerdict`, from the line's own fenced `roshera:*` cards —
 * never invented), the SAME marker's fill and ring pick up that verdict's
 * colour — emerald pass / red fail / amber inconclusive, the exact
 * vocabulary `cards/card-chrome.tsx`'s `Claim` already uses for a
 * certificate's tri-state glyphs. No second column, no new icon: the shape
 * (which icon sits inside the circle) never changes, only the circle's own
 * colour does. A line with no verdict keeps today's neutral per-author
 * fill — most lines have no state and must not acquire a decorative one.
 */
const VERDICT_MARKER: Record<LineVerdict, { fill: string; icon: string }> = {
  pass: { fill: 'bg-emerald-500/20 ring-1 ring-emerald-500/60', icon: 'text-emerald-600 dark:text-emerald-400' },
  fail: { fill: 'bg-red-500/20 ring-1 ring-red-500/60', icon: 'text-red-600 dark:text-red-400' },
  inconclusive: {
    fill: 'bg-amber-500/20 ring-1 ring-amber-500/60',
    icon: 'text-amber-600 dark:text-amber-400',
  },
}

/** Hover/`aria-label` detail — the pass/fail/inconclusive fact is legible
 *  from the marker's colour alone, but the full sentence is one hover away
 *  (never the only way to read it, per the "no mouse" rule). */
function verdictDetail(verdict: LineVerdict): string {
  switch (verdict) {
    case 'pass':
      return 'carries a certified pass'
    case 'fail':
      return 'carries a proven failure'
    case 'inconclusive':
      return 'carries an inconclusive/unverifiable result — not checked as a pass or a fail'
  }
}

/** `12s` under a minute, `1m 05s` at/after — never a fabricated percentage
 *  or step count, just the honest clock. */
function formatElapsed(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000))
  if (totalSeconds < 60) return `${totalSeconds}s`
  const m = Math.floor(totalSeconds / 60)
  const s = totalSeconds % 60
  return `${m}m ${s.toString().padStart(2, '0')}s`
}

/**
 * IN-FLIGHT STATUS ROW
 * ---------------------
 * Rendered on the agent's own line for the entire duration of a turn (not
 * just while text is arriving — a turn spends most of its 60-90s inside a
 * tool call that emits no wire frames on this provider path, see
 * `acp-blackboard.ts`'s `AgentAttention` doc). Two honest signals: the
 * elapsed clock (no percentage, no ETA) and the operation the backend
 * actually observed (`lib/agent-activity.ts` — named or explicitly
 * unobserved, never inferred). After 30s a quiet note explains that this
 * is normal, and a Stop control lets a user who thinks it hung end the
 * turn instead of reloading the page.
 */
/**
 * What the agent is doing, named at operation level, from state the
 * backend actually observed.
 *
 * The old version of this inferred phases ("Waiting for the model" /
 * "Working through the request") from whether any text had arrived —
 * honest when written, because the ACP stream surfaces ZERO `tool_call`
 * frames on the `claude-code` path. That limit is gone: the backend's
 * `agent_activity.rs` observes every operation the agent performs
 * against the server's own REST surface and names it at operation level
 * ("create box", "boolean difference") or reports `label: null` when it
 * cannot name one honestly. `useObservedTurnActivity`
 * (`lib/agent-activity.ts`) polls that endpoint while the turn runs.
 *
 * The honesty rules move here intact:
 * - a named operation is shown VERBATIM — never a method, a route, or an
 *   internal identifier;
 * - `label: null` renders as "unnamed operation" — recorded, honestly
 *   nameless, never guessed;
 * - nothing observed renders as exactly that. "No operation observed
 *   yet" is TRUE both while the model thinks and when its work has not
 *   reached the kernel — inventing anything more specific is the thing
 *   this whole design refuses.
 */
function turnActivity(activity: ObservedTurnActivity): string {
  if (activity.kind === 'operation') {
    return activity.label ?? 'unnamed operation'
  }
  return 'no operation observed yet'
}

/** Three dots that actually move, so a stalled turn and a live one do not
 *  look identical — the complaint that started this ("it stopped here,
 *  there is no way to know if it's actually working"). */
function EllipsisDots() {
  return (
    <span aria-hidden className="inline-flex">
      <span className="animate-pulse">.</span>
      <span className="animate-pulse [animation-delay:200ms]">.</span>
      <span className="animate-pulse [animation-delay:400ms]">.</span>
    </span>
  )
}

function TurnStatus({ elapsedMs, onCancel }: { elapsedMs: number; onCancel?: () => void }) {
  const [stopping, setStopping] = useState(false)
  // This component only mounts while the turn streams, so the poll runs
  // exactly for the turn's lifetime — see `lib/agent-activity.ts` for the
  // cadence rationale (5 s, Poll rate class, hidden-tab pause).
  const observed = useObservedTurnActivity(true)
  const activity = turnActivity(observed)
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-2 text-[10px] text-muted-foreground/70">
      <span
        className="flex items-center gap-1.5 font-medium text-amber-600 dark:text-amber-400"
        title={
          observed.kind === 'operation'
            ? 'The operation the kernel most recently served for this agent — observed from its own authenticated requests, never inferred.'
            : 'The turn is running but no operation has reached the kernel yet — the model may be thinking, or working without touching geometry. Nothing is invented here.'
        }
      >
        <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-amber-500" />
        <span>{activity}</span>
        <EllipsisDots />
      </span>
      {observed.kind === 'operation' && observed.opsThisTurn > 1 && (
        <span
          className="tabular-nums"
          title="Operations the kernel has served for this turn so far — a count of what genuinely happened, not a progress estimate"
        >
          op {observed.opsThisTurn}
        </span>
      )}
      <span className="flex items-center gap-1.5" title="Elapsed time for this turn">
        <span className="font-mono tabular-nums">{formatElapsed(elapsedMs)}</span>
      </span>
      {elapsedMs >= 30_000 && (
        <span className="text-muted-foreground/50">Turns typically take 60–90 seconds.</span>
      )}
      {onCancel && (
        <button
          type="button"
          onClick={() => {
            setStopping(true)
            onCancel()
          }}
          disabled={stopping}
          className="cad-icon-btn h-5 gap-1 px-1.5 text-[10px] disabled:opacity-50"
          title="End this turn"
          aria-label="Stop agent turn"
        >
          <Square size={8} />
          {stopping ? 'Stopping…' : 'Stop'}
        </button>
      )}
    </div>
  )
}

/**
 * TERMINAL-STATUS GLYPH
 * ----------------------
 * Prefixes a settled agent line with how its turn ended, using the exact
 * Check/X/CircleSlash + emerald/red/amber vocabulary `cards/card-chrome.tsx`'s
 * `Claim` already uses for certificate verdicts — one glyph language across
 * the app. This is NOT a geometry verdict: it says the turn completed,
 * failed, or was cancelled, nothing about whether the resulting part is
 * sound (that is the certificate cards' job, on their own line).
 */
function TurnStatusGlyph({ status }: { status: AgentTurnStatus }) {
  const title =
    status === 'completed'
      ? 'Turn completed'
      : status === 'failed'
        ? 'Turn failed'
        : 'Turn cancelled'
  return (
    <span
      className={cn(
        'mt-0.5 inline-flex shrink-0',
        status === 'completed' && 'text-emerald-600 dark:text-emerald-400',
        status === 'failed' && 'text-red-600 dark:text-red-400',
        status === 'cancelled' && 'text-amber-600 dark:text-amber-400',
      )}
      title={title}
      aria-label={title}
    >
      {status === 'completed' ? (
        <Check size={12} />
      ) : status === 'failed' ? (
        <X size={12} />
      ) : (
        <CircleSlash size={12} />
      )}
    </span>
  )
}

/**
 * One Blackboard line, rendered through `MessageMarkdown` (markdown +
 * KaTeX math). **Read-only** — no line is editable, whoever wrote it.
 *
 * The notebook is a record. Editing let a certificate, a refusal or an
 * agent's own reasoning be rewritten in place, and the result still looked
 * like something its author had written; nothing on screen distinguished
 * an original from an alteration. Deleting survives, because a deletion is
 * visible: the line is gone and the event log says so. New text comes from
 * the composer, which is the only thing that produces a line.
 *
 * Origin shows in the leading marker — SHAPE says who wrote it (agent →
 * bot, user → person, system → wrench), COLOUR says what that line's own
 * certificate concluded.
 */
export function BlackboardLine({ line, onCommit, onDelete, streaming = false, onCancel }: Props) {
  // No edit state. The notebook is read-only as of 2026-08-01 — a textarea,
  // a draft, an autosize effect and commit/cancel/keydown handlers all lived
  // here to serve `setEditing(true)`, which no longer exists. They are gone
  // rather than left unreachable: dead UI machinery in a file whose whole
  // point is now that the record cannot be rewritten is exactly the kind of
  // thing a later reader reinstates by accident.

  // Elapsed-time clock for the in-flight status row. `line.createdAt` is set
  // when the (blank) agent line is minted at turn start, so it IS the turn's
  // start time — using it (not component-mount time) keeps the clock correct
  // even if this line remounts (e.g. a key change) mid-turn.
  const [elapsedMs, setElapsedMs] = useState(() => Date.now() - line.createdAt)
  useEffect(() => {
    if (!streaming) return
    // The lazy initializer above already covers the first tick (this line
    // is always minted already-streaming — see the comment above); the
    // effect only needs to keep the clock advancing.
    const id = setInterval(() => setElapsedMs(Date.now() - line.createdAt), 1000)
    return () => clearInterval(id)
  }, [streaming, line.createdAt])

  // Chalk-reveal gate, decided ONCE at mount: only agent content that just
  // arrived animates (actively streaming, or created moments ago). Persisted
  // history re-renders instantly — the reveal is for arrival, not reload.
  const [reveal] = useState(() => ({
    animate: line.author === 'agent' && (streaming || Date.now() - line.createdAt < 4000),
  }))

  // Answering a Choices card (`cards/ChoicesCard.tsx`) does two things: sends
  // the clicked option's value as the next turn, and rewrites THIS line's
  // own text so the fence carries `selected: <value>` — the board's normal
  // edit path (`onCommit`, same as a manual edit), not extra component
  // state, so the answer survives reload and shows in the event log like
  // any other change to the line. `rawSource` is the exact fence body the
  // card rendered from; if it is not found verbatim in the current text
  // (the line was edited or reloaded from elsewhere between render and
  // click), this is a no-op — never guess which fence to rewrite.
  const selectChoice = useCallback(
    (rawSource: string, value: string) => {
      const escaped = rawSource.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
      const fenceRe = new RegExp('```roshera:choices\\n' + escaped + '\\n?```')
      const match = fenceRe.exec(line.text)
      if (match) {
        const selectedLine = yamlStringify({ selected: value }).trimEnd()
        const nextBody = `${rawSource}\n${selectedLine}`
        const nextText =
          line.text.slice(0, match.index) +
          '```roshera:choices\n' +
          nextBody +
          '\n```' +
          line.text.slice(match.index + match[0].length)
        onCommit(line.id, nextText)
      }
      // Through the queue-visible wrapper, not the raw transport: a choice
      // clicked while a turn is in flight is the exact silent-queue case
      // (Varun, 2026-08-01) — the composer's queue strip now shows it as
      // received-and-waiting instead of nothing happening for 60–90s.
      void sendPrompt(value, processBlackboardMessage)
    },
    [line.id, line.text, onCommit],
  )
  // A DetectedChoicesCard option was clicked (`cards/DetectedChoicesCard.tsx`)
  // — an "Option A: …" enumeration the agent wrote as prose, not a
  // `roshera:choices` fence. Unlike `selectChoice`, there is nothing to
  // rewrite in the line's own text (no fence to mark `selected:` on): the
  // agent's prose stays exactly as written, and this only sends the
  // option's own text as the next turn, same as `processBlackboardMessage`
  // does for anything the user types.
  const sendDetectedChoice = useCallback((value: string) => {
    // Queue-visible for the same reason as `selectChoice` above.
    void sendPrompt(value, processBlackboardMessage)
  }, [])
  const cardActions = useMemo<CardActions>(
    () => ({ selectChoice, sendDetectedChoice }),
    [selectChoice, sendDetectedChoice],
  )

  const isAgent = line.author === 'agent'
  // `.goosehints` tells the agent to ask a closed-set question as a
  // `roshera:choices` fence; the agent can ignore that instruction (it's
  // steering), so the board enforces the outcome as a constraint instead —
  // detect the agent's OWN "Option A: … Option B: …" enumeration and offer
  // buttons underneath it regardless. Gated to agent authorship here (never
  // user or system lines) and to the settled render only: mid-stream, a
  // still-arriving "Option B:" line would flicker buttons in and out and
  // could be clicked before its text has finished arriving.
  const detectedChoices = useMemo(
    () => (isAgent && !streaming ? detectEnumeratedChoices(line.text) : null),
    [isAgent, streaming, line.text],
  )
  const isSystem = line.author === 'system'

  // Content class decides both editability and shape (see
  // `lib/blackboard-content.ts`'s module doc). `evidence` is machine-
  // authored or verbatim-forwarded — the kernel's own testimony — and gets
  // NO edit path at all, not a disabled one. That is now true of EVERY
  // class, not just evidence — the notebook became read-only on
  // 2026-08-01. The class still decides how a line READS: `reasoning` gets
  // the readable lane, `evidence` is quieter and denser, `control` reads
  // as interactive.
  const contentClass = classifyBlackboardContent(line)
  const isEvidence = contentClass === 'evidence'
  const isControl = contentClass === 'control'

  // Colour channel for the origin marker — derived ONLY from this line's
  // own fenced verdict cards (never streaming text mid-arrival: an unclosed
  // fence simply does not match yet, so the marker stays neutral until the
  // card itself is complete, same as the card renderer).
  const verdict = useMemo(() => lineVerdict(line.text), [line.text])
  const authorLabel = isAgent ? 'Agent-authored' : isSystem ? 'App-generated' : 'You'
  const markerTitle = verdict ? `${authorLabel} — ${verdictDetail(verdict)}` : authorLabel

  // AUTHORSHIP WITHOUT COLOUR — position + weight.
  // ------------------------------------------------
  // "difficult to differentiate between ai and user" (Varun, 2026-08-01):
  // authorship was carried only by the ~10px marker glyph, which fails the
  // readable-without-a-mouse bar at a scan. Colour is unavailable — it is
  // reserved for state, and the marker's colour channel already carries
  // the line's certificate verdict — so the free channels are position and
  // weight: the USER's lines are indented (the ask steps in; the agent's
  // work stays flush as the body of the notebook) and set at medium
  // weight (a short instruction reads as a heading-like interjection).
  // A left rule was rejected: `control` lines already use a ruled shape
  // (sky, "awaiting your choice") and a second ruled family would blur
  // that; indentation collides with nothing. Evidence stays flush, dim
  // and dense — still visibly subordinate to both voices.
  const isUser = !isAgent && !isSystem
  return (
    <div
      className={cn(
        'group/line flex items-start gap-2 px-3 py-1.5 hover:bg-white/[0.03] rounded-md',
        isUser && 'ml-6',
      )}
    >
      {/* Origin marker — shape (icon) says who wrote it; colour says what
          happened, when this line carries a verdict. */}
      <div
        className={cn(
          'mt-1 flex h-4 w-4 shrink-0 items-center justify-center rounded-full',
          verdict
            ? VERDICT_MARKER[verdict].fill
            : isAgent
              ? 'bg-accent'
              : isSystem
                ? 'bg-muted-foreground/20'
                : 'bg-primary/20',
        )}
        title={markerTitle}
        aria-label={markerTitle}
      >
        {isAgent ? (
          <Bot size={10} className={verdict ? VERDICT_MARKER[verdict].icon : 'text-foreground'} />
        ) : isSystem ? (
          <Wrench size={9} className={verdict ? VERDICT_MARKER[verdict].icon : 'text-muted-foreground'} />
        ) : (
          <User size={10} className={verdict ? VERDICT_MARKER[verdict].icon : 'text-primary'} />
        )}
      </div>

      <div className="min-w-0 flex-1">
        {streaming ? (
          // Not a <button> — a turn in flight is not editable, and a stop
          // control lives inside this block (nested <button>s are invalid).
          // Content is measure-capped at 72ch everywhere below (streaming,
          // evidence, prose): the panel is 50rem wide and full-bleed body
          // text at that width is unreadable — vault ui-pass-spec §1 sets
          // the measure at ~68–75ch.
          <div className="w-full max-w-[72ch] select-text text-left text-sm leading-relaxed text-foreground/90">
            {line.text.trim() ? (
              <CardActionsContext.Provider value={cardActions}>
                <RevealContext.Provider value={reveal}>
                  <StreamingLineText text={line.text} />
                </RevealContext.Provider>
              </CardActionsContext.Provider>
            ) : (
              <span className="mt-0.5 inline-flex items-center">
                <span className="chalk-cursor" />
              </span>
            )}
            <TurnStatus elapsedMs={elapsedMs} onCancel={onCancel} />
          </div>
        ) : isEvidence ? (
          // Evidence: the kernel's own testimony (a certificate fence, a
          // "Created …" echo, app bookkeeping). The edit path is ABSENT,
          // not disabled — no onClick, no setEditing, no "Click to edit"
          // title. Denser and quieter than prose: this is the record, not
          // the argument. Text stays selectable; deleting stays allowed
          // (a deletion is visible, an edit is invisible and looks
          // authored — that is the whole distinction).
          <div className="flex w-full items-start gap-1.5 select-text text-left text-xs leading-snug text-foreground/60">
            {isAgent && line.turnStatus && <TurnStatusGlyph status={line.turnStatus} />}
            <span className="min-w-0 max-w-[72ch] flex-1">
              {line.text.trim() ? (
                <CardActionsContext.Provider value={cardActions}>
                  <RevealContext.Provider value={reveal}>
                    <MessageMarkdown content={line.text} />
                  </RevealContext.Provider>
                </CardActionsContext.Provider>
              ) : (
                <span className="text-white/30 italic">Empty line</span>
              )}
              {isSystem && line.repeatCount !== undefined && line.repeatCount > 1 && (
                <span
                  className="ml-1 text-[10px] text-muted-foreground/50"
                  title={`Reposted ${line.repeatCount} times — identical consecutive lines collapse into one`}
                >
                  (×{line.repeatCount})
                </span>
              )}
            </span>
          </div>
        ) : (
          // Not editable, and not a <button>. The notebook is a RECORD:
          // every line is now read-only, evidence and prose alike (Varun,
          // 2026-08-01 — "stop the ability to edit the text in blackboard").
          // Writing happens in the composer, which is the only place that
          // produces a line; the transcript stops being a scratchpad you can
          // silently rewrite after the fact.
          //
          // Dropping the <button> also fixes a latent invalidity: a `control`
          // line renders ChoicesCard/DetectedChoicesCard INSIDE this element,
          // so the old markup nested buttons inside a button.
          //
          // `onCommit` survives and is still used — `selectChoice` writes the
          // chosen option back into the line. That is a programmatic record
          // of an answer, not a person retyping the kernel.
          <div
            className={cn(
              'flex w-full items-start gap-1.5 select-text text-left text-sm leading-relaxed text-foreground/90',
              // Weight channel of the authorship split (see the comment on
              // the row wrapper): the user's ask reads medium against the
              // agent's regular-weight working prose. Same size, same ramp.
              isUser && 'font-medium',
              // Control: a closed-set question — reads as interactive.
              // Reuses card-chrome's `info` (sky) accent, the same colour
              // ChoicesCard/DetectedChoicesCard already use for "awaiting
              // your choice" — state, not decoration.
              isControl &&
                'rounded-sm border-l-2 border-sky-500/40 bg-sky-500/[0.04] pl-1.5 hover:bg-sky-500/[0.08]',
            )}
          >
            {isAgent && line.turnStatus && <TurnStatusGlyph status={line.turnStatus} />}
            <span className="min-w-0 max-w-[72ch] flex-1">
              {line.text.trim() ? (
                <CardActionsContext.Provider value={cardActions}>
                  <RevealContext.Provider value={reveal}>
                    <MessageMarkdown content={line.text} />
                    {detectedChoices && <DetectedChoicesCard set={detectedChoices} />}
                  </RevealContext.Provider>
                </CardActionsContext.Provider>
              ) : (
                <span className="text-white/30 italic">Empty line</span>
              )}
              {isSystem && line.repeatCount !== undefined && line.repeatCount > 1 && (
                <span
                  className="ml-1 text-[10px] text-muted-foreground/60"
                  title={`Reposted ${line.repeatCount} times — identical consecutive lines collapse into one`}
                >
                  (×{line.repeatCount})
                </span>
              )}
            </span>
          </div>
        )}
      </div>

      {/* Delete stays. A deletion is VISIBLE — the line is gone and the
          event log says who removed it. An edit was invisible: the altered
          text still looked like something its author wrote. That asymmetry
          is why one survived read-only and the other did not. */}
      {
        <button
          type="button"
          onClick={() => onDelete(line.id)}
          className="cad-icon-btn mt-0.5 h-5 w-5 shrink-0 opacity-0 transition-opacity group-hover/line:opacity-60 hover:opacity-100"
          title="Delete line"
          aria-label="Delete line"
        >
          <Trash2 size={11} />
        </button>
      }
    </div>
  )
}
