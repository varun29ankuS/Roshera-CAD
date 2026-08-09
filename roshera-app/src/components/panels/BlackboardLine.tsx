import { useEffect, useState, useCallback, useMemo } from 'react'
import { stringify as yamlStringify } from 'yaml'
import { cn } from '@/lib/utils'
import { processBlackboardMessage } from '@/lib/ai-client'
import { sendPrompt } from '@/lib/blackboard-composer'
import { detectEnumeratedChoices } from '@/lib/blackboard-cards'
import { classifyBlackboardContent } from '@/lib/blackboard-content'
import { lineVerdict, type LineVerdict } from '@/lib/blackboard-line-state'
import { useObservedTurnActivity, type ObservedTurnActivity } from '@/lib/agent-activity'
import { useSceneStore } from '@/stores/scene-store'
import type { BlackboardLine as Line } from '@/stores/blackboard-store'
import { MessageMarkdown } from './MessageMarkdown'
import { StreamingLineText } from './StreamingLineText'
import {
  ExpandableProse,
  FailedTurnBlock,
  ToolCallRow,
  type ParsedToolCall,
  type ToolCallStatus,
} from './BlackboardMessageParts'
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

/** The ACP statuses `lib/acp-blackboard.ts` forwards verbatim. Closed set:
 *  anything else makes `parseToolCallLine` decline the line entirely. */
const TOOL_STATUSES: readonly ToolCallStatus[] = [
  'pending',
  'in_progress',
  'completed',
  'failed',
]

/**
 * `⚙ <title> — <status>` — the one line shape `lib/acp-blackboard.ts`'s
 * `renderToolLine` writes for a tool call, optionally followed by a blank
 * line and a validated `roshera:*` card fence.
 *
 * Recognising a format produced in `lib/` from the component that renders
 * it is the pattern this panel already uses for `isBuildStepLine`
 * ("Created **…**") and `detectEnumeratedChoices` ("Option A: …"): narrow,
 * anchored at both ends, and DECLINING rather than guessing. The `—`
 * separator also occurs inside tool titles, so the status is matched
 * against the closed set above at the END of the header, never by
 * splitting on the first dash.
 */
function parseToolCallLine(text: string): ParsedToolCall | null {
  const newline = text.indexOf('\n')
  const head = (newline === -1 ? text : text.slice(0, newline)).trim()
  if (!head.startsWith('⚙')) return null
  const match = /^⚙\s+(.+?)\s+—\s+([a-z_]+)$/.exec(head)
  if (!match) return null
  const status = TOOL_STATUSES.find((s) => s === match[2])
  if (!status) return null
  const body = newline === -1 ? '' : text.slice(newline).trim()
  return { title: match[1].trim(), status, body: body.length > 0 ? body : null }
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
 * PART TAG — which part a UNIONED legacy line was about.
 * ---------------------------------------------------------------------
 * The blackboard became one notebook per document on 2026-08-04; before
 * that, a line could live in its own part's notebook. Those lines are not
 * gone — the backend's read-side union surfaces them here too (`partId` /
 * `partUuid` on the line, set only by that union — see
 * `stores/blackboard-store.ts`'s doc). This is the one place that
 * association is still visible: resolve `partUuid` against the live scene
 * (the same id `scene-store`'s `objects` map is keyed by) to show a name,
 * falling back to the bare numeric id when the part is no longer
 * registered (deleted/retired) — `partId` alone still proves the line WAS
 * about a part, even with nothing left to look up.
 */
function PartTag({ line }: { line: Line }) {
  const name = useSceneStore((s) =>
    line.partUuid ? s.objects.get(line.partUuid)?.name : undefined,
  )
  if (line.partId === undefined) return null
  const label = name ?? `part ${line.partId}`
  return (
    <span
      className="ml-1 text-[10px] text-muted-foreground/50"
      title="Written in this part's own notebook before the blackboard became one notebook per document — carried into the document view here."
    >
      · {label}
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

  // A tool call the agent transport reported (`lib/acp-blackboard.ts`'s
  // `renderToolLine`) — its own shape, a chip row, rather than a sentence
  // in the evidence lane. System authorship is required: the `⚙ … — status`
  // header is a format only that writer produces, and a person or the model
  // typing the same characters must not be re-rendered as machinery.
  const toolCall = useMemo(
    () => (isSystem && !streaming ? parseToolCallLine(line.text) : null),
    [isSystem, streaming, line.text],
  )

  // A turn that ended in failure gets a designed block instead of a
  // paragraph — keyed on the store's typed `turnStatus`, never on the word
  // "failed" appearing in prose. The one exception is a failure that
  // already carries a fenced `roshera:*` card (a kernel refusal riding the
  // JSON-RPC error payload — see `describeAcpTurnFailure`): that card is
  // the better rendering of itself, so the line stays on the normal path
  // and keeps its terminal glyph rather than nesting a card inside a block.
  const failedTurn =
    isAgent &&
    !streaming &&
    line.turnStatus === 'failed' &&
    !line.text.includes('```roshera:') &&
    line.text.trim().length > 0

  // Colour channel for the origin marker — derived ONLY from this line's
  // own fenced verdict cards (never streaming text mid-arrival: an unclosed
  // fence simply does not match yet, so the marker stays neutral until the
  // card itself is complete, same as the card renderer).
  const verdict = useMemo(() => lineVerdict(line.text), [line.text])
  const authorLabel = isAgent ? 'Agent-authored' : isSystem ? 'App-generated' : 'You'
  const markerTitle = verdict ? `${authorLabel} — ${verdictDetail(verdict)}` : authorLabel

  // AUTHORSHIP WITHOUT COLOUR — position, weight, SURFACE.
  // -------------------------------------------------------
  // "difficult to differentiate between ai and user" (Varun, 2026-08-01):
  // authorship was carried only by the ~10px marker glyph, which fails the
  // readable-without-a-mouse bar at a scan. Verdict colour is unavailable —
  // the marker's fill already spends it on the line's certificate — so the
  // free channels are position, weight and shape: the USER's lines are
  // indented (the ask steps in; the agent's work stays flush as the body of
  // the notebook), set at medium weight, and — since 2026-08-09 — drawn
  // inside a shrink-wrapped, faintly-tinted container while every other
  // voice runs the full lane unboxed. Indent and weight alone were still
  // too quiet at arm's length; a container is the largest difference
  // available that costs no colour the verdict channel needs (the tint is
  // the app's primary, never emerald/red/amber/sky).
  // A left rule was rejected for this job: `control` lines already use a
  // ruled shape (sky, "awaiting your choice") and a second ruled family
  // would blur that. Evidence stays flush, dim and dense — still visibly
  // subordinate to both voices.
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
          // The in-flight message is the one line on the board that is
          // still changing, so it gets a surface of its own: a live primary
          // rule and a faint wash, matching the header's own primary
          // "writing" pulse. Both vanish the moment the turn settles — the
          // message drops back into the flush notebook body — so "live" is
          // never a state a finished line can be mistaken for. Deliberately
          // NOT height-capped: clamping the text currently arriving would
          // fight the board's scroll-to-newest.
          <div className="w-full max-w-[72ch] select-text rounded-sm border-l-2 border-primary/50 bg-primary/[0.03] pl-2 text-left text-sm leading-relaxed text-foreground/90">
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
        ) : toolCall ? (
          // A tool call reads as a chip row — mono name, status glyph +
          // word, payload behind a visible button. See `ToolCallRow` for
          // why there is no duration and no argument summary.
          <div className="select-text text-left">
            <ToolCallRow call={toolCall} />
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
              <PartTag line={line} />
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
              'flex items-start gap-1.5 select-text text-left text-sm leading-relaxed text-foreground/90',
              // The user's container shrink-wraps its own words; every
              // other voice runs the full lane. A one-line ask therefore
              // reads as a small object against a full-bleed paragraph —
              // the difference is visible before a single word is read.
              isUser ? 'w-fit max-w-full' : 'w-full',
              // Weight channel of the authorship split (see the comment on
              // the row wrapper): the user's ask reads medium against the
              // agent's regular-weight working prose. Same size, same ramp.
              //
              // Plus a SURFACE channel, added 2026-08-09: the user's line
              // sits in a real container — rounded, faintly filled, hairline
              // bordered — while the agent's prose stays flush and unboxed
              // as the body of the notebook. Indent and weight alone were
              // too quiet to read at arm's length, and colour was already
              // spoken for (the marker's fill carries the line's verdict),
              // so shape is the free channel. The tint is the app's own
              // primary, never one of the verdict hues.
              isUser && 'font-medium',
              isUser && 'rounded-lg border border-primary/25 bg-primary/[0.06] px-2.5 py-1.5',
              // Control: a closed-set question — reads as interactive.
              // Reuses card-chrome's `info` (sky) accent, the same colour
              // ChoicesCard/DetectedChoicesCard already use for "awaiting
              // your choice" — state, not decoration.
              isControl &&
                'rounded-sm border-l-2 border-sky-500/40 bg-sky-500/[0.04] pl-1.5 hover:bg-sky-500/[0.08]',
            )}
          >
            {/* The terminal glyph is suppressed for a failed turn: the
                block below carries its own cross and the word, so a second
                mark in the gutter would say the same thing twice. */}
            {isAgent && line.turnStatus && !failedTurn && (
              <TurnStatusGlyph status={line.turnStatus} />
            )}
            <span className="min-w-0 max-w-[72ch] flex-1">
              {failedTurn ? (
                <FailedTurnBlock text={line.text} />
              ) : line.text.trim() ? (
                <CardActionsContext.Provider value={cardActions}>
                  <RevealContext.Provider value={reveal}>
                    {/* Only the prose is height-capped; the choice buttons
                        below it are a control and must never sit behind a
                        "show more". */}
                    <ExpandableProse source={line.text}>
                      <MessageMarkdown content={line.text} />
                    </ExpandableProse>
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
              <PartTag line={line} />
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
