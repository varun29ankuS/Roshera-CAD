import { useEffect, useRef, useState, useCallback } from 'react'
import { cn } from '@/lib/utils'
import type { BlackboardLine as Line } from '@/stores/blackboard-store'
import { MessageMarkdown } from './MessageMarkdown'
import { StreamingLineText } from './StreamingLineText'
import { RevealContext } from './cards/reveal-context'
import { Bot, User, Wrench, Trash2 } from 'lucide-react'

interface Props {
  line: Line
  onCommit: (id: string, text: string) => void
  onDelete: (id: string) => void
  /** True while this line is receiving streamed agent text — routes display
   *  through the buffered renderer (math/cards typeset only when complete). */
  streaming?: boolean
}

/**
 * One Blackboard line. A committed line renders through `MessageMarkdown`
 * (markdown + KaTeX math). Clicking the line enters edit mode: a textarea
 * shows the raw source; Enter (without Shift) or blur commits, Escape cancels.
 * Agent-, user-, and system-authored lines are all editable; origin is shown
 * by a subtle leading marker (agent → bot icon, user → person icon, system →
 * wrench icon for app-generated toolbar/operation feedback).
 */
export function BlackboardLine({ line, onCommit, onDelete, streaming = false }: Props) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(line.text)
  const textareaRef = useRef<HTMLTextAreaElement>(null)

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
        ) : (
          <button
            type="button"
            onClick={streaming ? undefined : () => setEditing(true)}
            className="block w-full cursor-text select-text text-left text-sm leading-relaxed text-foreground/90"
            title={streaming ? undefined : 'Click to edit'}
          >
            {streaming ? (
              line.text.trim() ? (
                <RevealContext.Provider value={reveal}>
                  <StreamingLineText text={line.text} />
                </RevealContext.Provider>
              ) : (
                <span className="mt-0.5 inline-flex items-center">
                  <span className="chalk-cursor" />
                </span>
              )
            ) : line.text.trim() ? (
              <RevealContext.Provider value={reveal}>
                <MessageMarkdown content={line.text} />
              </RevealContext.Provider>
            ) : (
              <span className="text-white/30 italic">Empty line — click to edit</span>
            )}
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
