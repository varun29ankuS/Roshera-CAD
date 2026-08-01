import { useRef, useEffect, useState, useCallback } from 'react'
import {
  useBlackboardStore,
  DOCUMENT_SCOPE,
  partScope,
} from '@/stores/blackboard-store'
import { useAcpSessionStore } from '@/stores/acp-session-store'
import { useSceneStore } from '@/stores/scene-store'
import { cn } from '@/lib/utils'
import { processBlackboardMessage } from '@/lib/ai-client'
import { cancelAcpTurn } from '@/lib/acp-blackboard'
import { BlackboardLine } from './BlackboardLine'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  NotebookPen,
  Send,
  Loader2,
  ChevronDown,
  GripHorizontal,
  Plus,
  Trash2,
} from 'lucide-react'

/**
 * BLACKBOARD
 * ==========
 * A wide, editable, logged document that supersedes the chat-transcript UX.
 * The body is a document of independently-editable lines (no message bubbles).
 * The agent writes replies as editable lines; the user can edit any line and
 * add their own. Every create/edit/delete is event-sourced + persisted by the
 * blackboard store (localStorage today, backend-swappable seam).
 *
 * Width note: the legacy AI chat panel was `w-80` (20rem). Per spec this panel
 * is 2.5× as wide → `w-[50rem]` (50rem / 800px), capped to the viewport so the
 * 3D scene stays usable on narrow screens.
 */
const compactCount = new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 })

/** Render `usage_update`'s `used` (`AcpSessionStats.tokensUsed`,
 *  `stores/acp-session-store.ts`) as a running token total. Measured live
 *  (2026-07-31): a real turn reported
 *  `{"sessionUpdate":"usage_update","used":359346,"size":128000}` — `used`
 *  is CUMULATIVE tokens consumed across the whole session, `size` is the
 *  static context window; they are not comparable, and `used` legitimately
 *  exceeds `size` once a session runs long (359346/128000 → a meaningless
 *  280%). No `usage_update` field observed on the wire reports current
 *  context OCCUPANCY (how full the window is right now), so there is no
 *  honest percentage to compute — this renders the total alone, never a
 *  used/size ratio, and never clamps to 100% (that would hide the truth
 *  rather than tell it). */
function formatContextUsage(used: number): string {
  return `${compactCount.format(used)} tokens`
}

export function Blackboard() {
  const lines = useBlackboardStore((s) => s.lines)
  const isProcessing = useBlackboardStore((s) => s.isProcessing)
  const isPanelOpen = useBlackboardStore((s) => s.isPanelOpen)
  const togglePanel = useBlackboardStore((s) => s.togglePanel)
  const clearBoard = useBlackboardStore((s) => s.clearBoard)
  const editLine = useBlackboardStore((s) => s.editLine)
  const deleteLine = useBlackboardStore((s) => s.deleteLine)
  const addLine = useBlackboardStore((s) => s.addLine)
  const activeScope = useBlackboardStore((s) => s.activeScope)
  const setActiveScope = useBlackboardStore((s) => s.setActiveScope)
  const agentAttention = useBlackboardStore((s) => s.agentAttention)
  const streamingLineId = useBlackboardStore((s) => s.streamingLineId)

  // Live ACP session stats (Feature B) — display-only, driven by
  // `lib/acp-blackboard.ts`'s wiring of the shared `AcpClient`. See
  // `stores/acp-session-store.ts` for the honesty rules (no cost figure,
  // session-scoped counts, "default" never printed as a model name).
  const acpModel = useAcpSessionStore((s) => s.model)
  const acpTurns = useAcpSessionStore((s) => s.turns)
  const acpTokens = useAcpSessionStore((s) => s.tokensUsed)
  const acpLive = useAcpSessionStore((s) => s.live)

  // Drive the notebook scope from the viewport selection: the active part's
  // notebook is shown, so each part has its OWN blackboard. The primary
  // selected scene object IS a part (its id is the kernel part UUID); when
  // nothing (or a non-part) is selected, fall back to the document notebook.
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const objects = useSceneStore((s) => s.objects)
  const selectedPart = useSceneStore((s) => {
    for (const id of s.selectedIds) {
      const obj = s.objects.get(id)
      if (obj) return obj
    }
    return null
  })
  useEffect(() => {
    setActiveScope(selectedPart ? partScope(selectedPart.id) : DOCUMENT_SCOPE)
    // `selectedIds`/`objects` are dependencies via `selectedPart`; listing the
    // raw stores keeps the effect honest if selection changes within the set.
  }, [selectedPart, selectedIds, objects, setActiveScope])

  const scopeLabel =
    activeScope === DOCUMENT_SCOPE
      ? 'Document'
      : selectedPart
        ? selectedPart.name
        : 'Part'

  const [input, setInput] = useState('')
  const scrollRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  /**
   * ATTENTION-FOLLOWING SPLIT
   * -------------------------
   * The body's height follows what the agent is doing: expand while it
   * writes/reasons, collapse to a strip (last line visible) the moment it is
   * executing geometry so the viewport takes the space. A user drag on the
   * top grip OVERRIDES the attention state and sticks until released
   * (double-click the grip, or the "auto" chip). Pure presentation — the
   * viewport is driven by ws-bridge/scene-store and updates when the kernel
   * confirms, never gated by anything this panel does.
   */
  const [overrideHeight, setOverrideHeight] = useState<number | null>(null)
  const [dragging, setDragging] = useState(false)
  const dragRef = useRef<{ pointerId: number; startY: number; startHeight: number } | null>(null)

  const attentionMaxHeight =
    agentAttention === 'writing'
      ? '62vh'
      : agentAttention === 'geometry'
        ? '5.25rem'
        : '42vh'

  const startDrag = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    const body = scrollRef.current
    if (!body) return
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    dragRef.current = {
      pointerId: e.pointerId,
      startY: e.clientY,
      startHeight: body.getBoundingClientRect().height,
    }
    setDragging(true)
  }, [])

  const moveDrag = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== e.pointerId) return
    // Panel is bottom-anchored: dragging the top edge up grows the body.
    const next = drag.startHeight + (drag.startY - e.clientY)
    const max = Math.round(window.innerHeight * 0.8)
    setOverrideHeight(Math.min(Math.max(next, 72), max))
  }, [])

  const endDrag = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== e.pointerId) return
    dragRef.current = null
    // The override STICKS after the pointer lifts — attention-following
    // resumes only when the user releases it explicitly.
    setDragging(false)
  }, [])

  const releaseOverride = useCallback(() => {
    dragRef.current = null
    setDragging(false)
    setOverrideHeight(null)
  }, [])

  // Auto-scroll to the newest content: new lines, streamed text growing the
  // last line, and height changes (the geometry strip must show the latest
  // line, not the top of the document).
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight
    }
  }, [lines, agentAttention, overrideHeight])

  useEffect(() => {
    if (isPanelOpen) {
      setTimeout(() => inputRef.current?.focus(), 100)
    }
  }, [isPanelOpen])

  const handleSubmit = useCallback(
    (e?: React.FormEvent) => {
      e?.preventDefault()
      const text = input.trim()
      if (!text || isProcessing) return
      setInput('')
      // Routes to the agent via the existing ai-client path; the reply is
      // appended to the board as an editable line, not a chat bubble.
      void processBlackboardMessage(text)
    },
    [input, isProcessing],
  )

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        handleSubmit()
      }
    },
    [handleSubmit],
  )

  // Collapsed state — floating button.
  if (!isPanelOpen) {
    return (
      <button
        onClick={togglePanel}
        className="cad-focus absolute bottom-10 left-4 z-20 w-10 h-10 rounded-full bg-primary text-primary-foreground flex items-center justify-center shadow-lg hover:scale-105 transition-transform"
        aria-label="Open Blackboard"
        title="Blackboard"
      >
        <NotebookPen size={18} />
      </button>
    )
  }

  return (
    <div className="absolute bottom-8 left-3 z-20 w-[50rem] max-w-[calc(100vw-1.5rem)] flex flex-col rounded-xl overflow-hidden bg-background/35 backdrop-blur-md border border-border/60">
      {/* Resize grip — a drag here overrides the attention-following split
          and STICKS until released (double-click, or the "auto" chip). */}
      <div
        className="flex h-2.5 shrink-0 cursor-ns-resize items-center justify-center touch-none hover:bg-white/5"
        onPointerDown={startDrag}
        onPointerMove={moveDrag}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={releaseOverride}
        title="Drag to size the board; it otherwise follows the agent's attention. Double-click to release."
      >
        <GripHorizontal size={11} className="text-muted-foreground/40" />
      </div>
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-white/5">
        <div className="flex items-center gap-2 min-w-0">
          <NotebookPen size={14} className="text-primary shrink-0" />
          <span className="text-xs font-medium shrink-0">Blackboard</span>
          {/* Which part's notebook is on screen — the per-part scope. */}
          <span
            className="text-[11px] text-muted-foreground truncate"
            title={`Notebook scope: ${scopeLabel}`}
          >
            · {scopeLabel}
          </span>
          {/* Attention state, legible without reading the board. */}
          {agentAttention === 'writing' && (
            <span className="flex shrink-0 items-center gap-1 text-[10px] text-primary/90">
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-primary" />
              writing
            </span>
          )}
          {agentAttention === 'geometry' && (
            <span className="flex shrink-0 items-center gap-1 text-[10px] text-amber-400/90">
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-amber-400" />
              executing — viewport has focus
            </span>
          )}
          {/* Live ACP session stats: model · turns · tokens, all
              session-scoped (reset when the stream drops — never a
              cumulative total), plus a live/idle dot. No session yet
              (or a dropped one) reads as "—", never a fabricated model
              name — see acp-session-store.ts's doc for why. */}
          <span
            className="hidden shrink-0 items-center gap-1.5 whitespace-nowrap text-[10px] text-muted-foreground/70 sm:flex"
            title={acpLive ? 'Agent session live' : 'No agent session'}
          >
            <span
              className={cn(
                'h-1.5 w-1.5 shrink-0 rounded-full',
                acpLive ? 'animate-pulse bg-emerald-400' : 'bg-muted-foreground/30',
              )}
            />
            <span className="font-mono">{acpModel ?? '—'}</span>
            {acpLive && (
              <>
                <span className="text-muted-foreground/40">·</span>
                <span>
                  {acpTurns} turn{acpTurns === 1 ? '' : 's'}
                </span>
                <span className="text-muted-foreground/40">·</span>
                <span>{formatContextUsage(acpTokens ?? 0)}</span>
                <span className="text-muted-foreground/50">(session)</span>
              </>
            )}
          </span>
        </div>
        <div className="flex items-center gap-0.5">
          {overrideHeight !== null && (
            <button
              onClick={releaseOverride}
              className="cad-icon-btn h-6 px-1.5 font-mono text-[10px] uppercase tracking-wide"
              title="Release the manual size — follow the agent's attention again"
              aria-label="Release manual size"
            >
              auto
            </button>
          )}
          <button
            onClick={() => addLine('', 'user')}
            className="cad-icon-btn h-6 w-6"
            title="Add line"
            aria-label="Add line"
          >
            <Plus size={13} />
          </button>
          <button
            onClick={clearBoard}
            className="cad-icon-btn h-6 w-6"
            title="Clear board"
            aria-label="Clear board"
          >
            <Trash2 size={12} />
          </button>
          <button
            onClick={togglePanel}
            className="cad-icon-btn h-6 w-6"
            title="Minimize"
            aria-label="Minimize Blackboard"
          >
            <ChevronDown size={14} />
          </button>
        </div>
      </div>

      {/* Document of editable lines. Height follows the agent's attention
          (or the user's sticky drag override); the transition is suppressed
          while dragging so the grip tracks the pointer exactly. */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto min-h-0 scrollbar-thin"
        style={
          overrideHeight !== null
            ? { height: `${overrideHeight}px`, maxHeight: '80vh' }
            : {
                maxHeight: attentionMaxHeight,
                transition: dragging
                  ? undefined
                  : 'max-height 380ms cubic-bezier(0.4, 0, 0.2, 1)',
              }
        }
      >
        <div className="py-2">
          {lines.map((line) => (
            <BlackboardLine
              key={line.id}
              line={line}
              onCommit={editLine}
              onDelete={deleteLine}
              streaming={line.id === streamingLineId}
              onCancel={line.id === streamingLineId ? cancelAcpTurn : undefined}
            />
          ))}
          {/* Shown only until the reply line exists — once streaming, the
              line's own chalk cursor is the progress signal. */}
          {isProcessing && streamingLineId === null && (
            <div className="flex items-center gap-2 px-3 py-2">
              <Loader2 size={14} className="animate-spin text-primary" />
              <span className="text-xs text-muted-foreground">Thinking...</span>
            </div>
          )}
        </div>
      </div>

      {/* Prompt — still routes to the agent */}
      <form
        onSubmit={handleSubmit}
        className="flex items-center gap-1.5 px-2 py-2 border-t border-white/5"
      >
        <Input
          ref={inputRef}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Ask the agent — its reply lands as an editable line…"
          disabled={isProcessing}
          className="h-8 text-xs bg-transparent border-white/10 placeholder:text-white/30"
        />
        <Button
          type="submit"
          size="sm"
          disabled={!input.trim() || isProcessing}
          className="h-8 w-8 p-0 shrink-0"
        >
          {isProcessing ? (
            <Loader2 size={14} className="animate-spin" />
          ) : (
            <Send size={14} />
          )}
        </Button>
      </form>
    </div>
  )
}
