import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import { stringify as yamlStringify } from 'yaml'
import { cn } from '@/lib/utils'
import { processBlackboardMessage } from '@/lib/ai-client'
import { detectEnumeratedChoices } from '@/lib/blackboard-cards'
import { classifyBlackboardContent } from '@/lib/blackboard-content'
import { lineVerdict, type LineVerdict } from '@/lib/blackboard-line-state'
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
 * `acp-blackboard.ts`'s `AgentAttention` doc). Elapsed time is the one
 * honest signal available: no percentage, no step count, no ETA. After 30s
 * a quiet note explains that this is normal, and a Stop control lets a
 * user who thinks it hung end the turn instead of reloading the page.
 */
/**
 * What the agent is doing, in a few words, from state we can actually see.
 *
 * ⚠ The honest limit: on the default provider path (goose's `claude-code`
 * bridge) NO `tool_call` frames reach us — tools execute inside the CLI
 * subprocess and are never surfaced over ACP (verified live across two
 * full turns, `toolCalls: 0`; see `lib/acp-blackboard.ts`). So this can
 * never say "looking up ISO 273" or "cutting the bore", and inventing
 * such a label would be fabricated activity in the one panel whose job is
 * to be an honest record.
 *
 * What IS observable: whether the prompt has been answered with any text
 * yet, and how long it has been. Once output starts the agent alternates
 * between writing and running tools invisibly, so the wording stays
 * "working" rather than "writing" — claiming it is writing while it is
 * actually mid-tool would be a small lie told constantly.
 *
 * Naming the actual operation is possible, but through the BACKEND: our
 * own MCP server sees every tool invocation even though the ACP stream
 * does not. That is the wiring that would let this say something real.
 */
function turnActivity(elapsedMs: number, hasOutput: boolean): string {
  if (!hasOutput) {
    return elapsedMs < 15_000 ? 'Waiting for the model' : 'Still waiting for the model'
  }
  return elapsedMs < 90_000 ? 'Working through the request' : 'Still working, longer than usual'
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

function TurnStatus({
  elapsedMs,
  hasOutput,
  onCancel,
}: {
  elapsedMs: number
  hasOutput: boolean
  onCancel?: () => void
}) {
  const [stopping, setStopping] = useState(false)
  const activity = turnActivity(elapsedMs, hasOutput)
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-2 text-[10px] text-muted-foreground/70">
      <span
        className="flex items-center gap-1.5 font-medium text-amber-600 dark:text-amber-400"
        title="What the agent is doing, from what this client can observe. Tool names are not available on this provider path."
      >
        <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-amber-500" />
        <span>{activity}</span>
        <EllipsisDots />
      </span>
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
 * One Blackboard line. A committed line renders through `MessageMarkdown`
 * (markdown + KaTeX math). Clicking the line enters edit mode: a textarea
 * shows the raw source; Enter (without Shift) or blur commits, Escape cancels.
 * Agent-, user-, and system-authored lines are all editable; origin is shown
 * by a subtle leading marker (agent → bot icon, user → person icon, system →
 * wrench icon for app-generated toolbar/operation feedback).
 */
export function BlackboardLine({ line, onCommit, onDelete, streaming = false, onCancel }: Props) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(line.text)
  const textareaRef = useRef<HTMLTextAreaElement>(null)

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

  // Keep the draft in sync when the line text changes underneath us (e.g. an
  // agent line streaming in) — but only while we're NOT actively editing it.
  // Done as an in-render reconcile (React's "adjusting state on prop change"
  // pattern) rather than an effect, so streaming updates show without a second
  // render pass and without a setState-in-effect cascade.
  const [lastSeenText, setLastSeenText] = useState(line.text)
  if (!editing && line.text !== lastSeenText) {
    setLastSeenText(line.text)
    setDraft(line.text)
  }

  // Autosize + focus the textarea on entering edit mode.
  useEffect(() => {
    if (editing && textareaRef.current) {
      const el = textareaRef.current
      el.focus()
      el.setSelectionRange(el.value.length, el.value.length)
      el.style.height = 'auto'
      el.style.height = `${el.scrollHeight}px`
    }
  }, [editing])

  const commit = useCallback(() => {
    setEditing(false)
    const next = draft.replace(/\s+$/, '')
    if (next !== line.text) onCommit(line.id, next)
  }, [draft, line.id, line.text, onCommit])

  const cancel = useCallback(() => {
    setDraft(line.text)
    setEditing(false)
  }, [line.text])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        commit()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        cancel()
      }
    },
    [commit, cancel],
  )

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
      void processBlackboardMessage(value)
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
    void processBlackboardMessage(value)
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
  // NO edit path at all, not a disabled one: no `onClick`, no `setEditing`,
  // no "Click to edit" affordance. `control`/`reasoning` keep today's
  // editing behaviour exactly.
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

  return (
    <div className="group/line flex items-start gap-2 px-3 py-1.5 hover:bg-white/[0.03] rounded-md">
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
        {editing ? (
          <textarea
            ref={textareaRef}
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value)
              e.target.style.height = 'auto'
              e.target.style.height = `${e.target.scrollHeight}px`
            }}
            onKeyDown={handleKeyDown}
            onBlur={commit}
            spellCheck={false}
            className="w-full resize-none bg-transparent font-mono text-xs leading-relaxed text-foreground outline-none placeholder:text-white/30"
            placeholder="Empty line — type markdown or $math$…"
          />
        ) : streaming ? (
          // Not a <button> — a turn in flight is not editable, and a stop
          // control lives inside this block (nested <button>s are invalid).
          <div className="w-full select-text text-left text-sm leading-relaxed text-foreground/90">
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
            <TurnStatus
              elapsedMs={elapsedMs}
              hasOutput={Boolean(line.text.trim())}
              onCancel={onCancel}
            />
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
            <span className="min-w-0 flex-1">
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
          <button
            type="button"
            onClick={() => setEditing(true)}
            className={cn(
              'flex w-full items-start gap-1.5 cursor-text select-text text-left text-sm leading-relaxed text-foreground/90',
              // Control: a closed-set question — reads as interactive.
              // Reuses card-chrome's `info` (sky) accent, the same colour
              // ChoicesCard/DetectedChoicesCard already use for "awaiting
              // your choice" — state, not decoration.
              isControl &&
                'rounded-sm border-l-2 border-sky-500/40 bg-sky-500/[0.04] pl-1.5 hover:bg-sky-500/[0.08]',
            )}
            title="Click to edit"
          >
            {isAgent && line.turnStatus && <TurnStatusGlyph status={line.turnStatus} />}
            <span className="min-w-0 flex-1">
              {line.text.trim() ? (
                <CardActionsContext.Provider value={cardActions}>
                  <RevealContext.Provider value={reveal}>
                    <MessageMarkdown content={line.text} />
                    {detectedChoices && <DetectedChoicesCard set={detectedChoices} />}
                  </RevealContext.Provider>
                </CardActionsContext.Provider>
              ) : (
                <span className="text-white/30 italic">Empty line — click to edit</span>
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
          </button>
        )}
      </div>

      {/* Delete affordance — appears on hover, never while editing. */}
      {!editing && (
        <button
          type="button"
          onClick={() => onDelete(line.id)}
          className="cad-icon-btn mt-0.5 h-5 w-5 shrink-0 opacity-0 transition-opacity group-hover/line:opacity-60 hover:opacity-100"
          title="Delete line"
          aria-label="Delete line"
        >
          <Trash2 size={11} />
        </button>
      )}
    </div>
  )
}
