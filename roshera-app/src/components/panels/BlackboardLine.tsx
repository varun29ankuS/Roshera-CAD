import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import { stringify as yamlStringify } from 'yaml'
import { cn } from '@/lib/utils'
import { processBlackboardMessage } from '@/lib/ai-client'
import type { BlackboardLine as Line } from '@/stores/blackboard-store'
import { MessageMarkdown } from './MessageMarkdown'
import { StreamingLineText } from './StreamingLineText'
import { RevealContext } from './cards/reveal-context'
import { CardActionsContext, type CardActions } from './cards/card-actions-context'
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
function TurnStatus({ elapsedMs, onCancel }: { elapsedMs: number; onCancel?: () => void }) {
  const [stopping, setStopping] = useState(false)
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-2 text-[10px] text-muted-foreground/70">
      <span className="flex items-center gap-1.5" title="Turn in progress — elapsed time">
        <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-primary" />
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
  const cardActions = useMemo<CardActions>(() => ({ selectChoice }), [selectChoice])

  const isAgent = line.author === 'agent'
  const isSystem = line.author === 'system'

  return (
    <div className="group/line flex items-start gap-2 px-3 py-1.5 hover:bg-white/[0.03] rounded-md">
      {/* Origin marker — subtle, distinguishes agent vs user vs system authorship. */}
      <div
        className={cn(
          'mt-1 flex h-4 w-4 shrink-0 items-center justify-center rounded-full',
          isAgent ? 'bg-accent' : isSystem ? 'bg-muted-foreground/20' : 'bg-primary/20',
        )}
        title={isAgent ? 'Agent-authored' : isSystem ? 'App-generated' : 'You'}
      >
        {isAgent ? (
          <Bot size={10} className="text-foreground" />
        ) : isSystem ? (
          <Wrench size={9} className="text-muted-foreground" />
        ) : (
          <User size={10} className="text-primary" />
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
            <TurnStatus elapsedMs={elapsedMs} onCancel={onCancel} />
          </div>
        ) : (
          <button
            type="button"
            onClick={() => setEditing(true)}
            className="flex w-full items-start gap-1.5 cursor-text select-text text-left text-sm leading-relaxed text-foreground/90"
            title="Click to edit"
          >
            {isAgent && line.turnStatus && <TurnStatusGlyph status={line.turnStatus} />}
            <span className="min-w-0 flex-1">
              {line.text.trim() ? (
                <CardActionsContext.Provider value={cardActions}>
                  <RevealContext.Provider value={reveal}>
                    <MessageMarkdown content={line.text} />
                  </RevealContext.Provider>
                </CardActionsContext.Provider>
              ) : (
                <span className="text-white/30 italic">Empty line — click to edit</span>
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
