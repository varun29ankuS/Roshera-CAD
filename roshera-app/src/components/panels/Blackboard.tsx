import { useRef, useEffect, useState, useCallback, useMemo } from 'react'
import {
  useBlackboardStore,
  DOCUMENT_SCOPE,
  partScope,
} from '@/stores/blackboard-store'
import { useAcpSessionStore } from '@/stores/acp-session-store'
import { useSceneStore } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { cn } from '@/lib/utils'
import { processBlackboardMessage } from '@/lib/ai-client'
import { cancelAcpTurn } from '@/lib/acp-blackboard'
import { groupBlackboardByCheckpoint, type CheckpointMarker } from '@/lib/blackboard-groups'
import {
  loadDraft,
  saveDraft,
  loadPromptHistory,
  pushPromptHistory,
  savePromptHistory,
  sendPrompt,
  useTurnQueue,
  waitingTurns,
} from '@/lib/blackboard-composer'
import { BlackboardSection } from './BlackboardSection'
import { Button } from '@/components/ui/button'
import {
  NotebookPen,
  Send,
  Loader2,
  ChevronDown,
  GripHorizontal,
  Plus,
  Trash2,
  Zap,
  Clock,
  Bot,
  User,
  Wrench,
  Check,
  X,
  CircleSlash,
} from 'lucide-react'
import { VendorMark } from '@/components/settings/vendor-marks'

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

/** What the header says about tokens, and why it says it that way.
 *
 *  The session total alone was technically true and practically
 *  misleading: shown beside a just-finished action it reads as that
 *  action's price. Measured 2026-08-01 — "clear the viewport" made two
 *  tool calls and replied in 51 characters while the header read ~70k,
 *  a total accumulated across three prompts.
 *
 *  So the turn's own figure leads and the session total is explicitly
 *  labelled behind it. Both are counts of tokens MOVED, never cost:
 *  most of a turn's figure is context re-sent on each round-trip, and
 *  re-sent context bills far below fresh input. The wire gives one
 *  scalar with no split, so no breakdown is offered — an invented one
 *  would be worse than none. */
const TOKENS_TITLE =
  'Tokens moved, not cost. Most of a turn is context re-sent on every ' +
  'round-trip, and re-sent context bills far below fresh input. The ' +
  'session figure accumulates across all turns.'

/** How heavy the last turn was, as a colour on the bolt.
 *
 *  Deliberately NOT a percentage of the context window. `used` is
 *  cumulative and legitimately exceeds `size`, so any ratio against the
 *  window is meaningless — that is the same trap the header already
 *  refuses in `formatContextUsage`. These are bands on the TURN's own
 *  size, calibrated against measured turns: a trivial action that made two
 *  tool calls still moved ~33k because every round-trip replays the whole
 *  context, so "light" has to start well above zero or everything would
 *  read as heavy.
 *
 *  The colour says how much moved, never how much it cost, and never
 *  whether the turn was worth it — a 200k turn that built a certified
 *  assembly is a good turn. */
function turnWeight(tokens: number): { className: string; label: string } {
  if (tokens < 40_000) return { className: 'text-emerald-500', label: 'light turn' }
  if (tokens < 120_000) return { className: 'text-amber-500', label: 'moderate turn' }
  return { className: 'text-red-500', label: 'heavy turn' }
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
  const acpProvider = useAcpSessionStore((s) => s.provider)
  const acpTokens = useAcpSessionStore((s) => s.tokensUsed)
  const acpLastTurnTokens = useAcpSessionStore((s) => s.lastTurnTokens)
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

  // ── Checkpoint sections ─────────────────────────────────────────────
  //
  // `GET /api/timeline/checkpoints` is the authoritative list of named
  // design-intent checkpoints (`CheckpointSummary` in
  // `roshera-backend/api-server/src/handlers/timeline.rs`) — the same
  // record `timeline_checkpoint(...)` mints and the Timeline strip's ◈
  // button posts to. The Blackboard fetches it independently (mirroring
  // `Timeline.tsx`'s own direct-fetch pattern) rather than reading it off
  // a shared store, since none exists yet for timeline data. Wire
  // `timestamp` (ISO 8601) is converted to epoch ms once here, at the
  // fetch boundary, so `blackboard-groups.ts` only ever deals in numbers.
  const wsStatus = useWSStore((s) => s.status)
  const [checkpoints, setCheckpoints] = useState<CheckpointMarker[]>([])

  const fetchCheckpoints = useCallback(async () => {
    try {
      const resp = await fetch('/api/timeline/checkpoints')
      if (!resp.ok) return
      const data = (await resp.json()) as Array<{
        id: string
        name: string
        timestamp: string
      }>
      if (!Array.isArray(data)) return
      const marks: CheckpointMarker[] = data
        .map((c) => ({ id: c.id, name: c.name, createdAt: new Date(c.timestamp).getTime() }))
        .filter((c) => !isNaN(c.createdAt))
      setCheckpoints(marks)
    } catch {
      // Backend not running — the board still renders as one unlabelled
      // section rather than failing to load.
    }
  }, [])

  useEffect(() => {
    if (wsStatus === 'connected') {
      // Data-sync fetch on (re)connect, mirroring `Timeline.tsx`'s
      // `fetchHistory` — this project's established pattern for pulling
      // timeline state into a component on mount/reconnect.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      fetchCheckpoints()
    }
  }, [wsStatus, fetchCheckpoints])

  // Sections group lines under the checkpoint that was open when each was
  // written; within a section, consecutive machine-authored "Created …"
  // lines still collapse into one compact step strip (`BuildStepStrip.tsx`)
  // so a bolt circle of bores reads as one row of marks, not one paragraph
  // per hole. Agent prose and user lines are never candidates — see
  // `groupBlackboardLines`'s doc.
  const sections = useMemo(
    () => groupBlackboardByCheckpoint(lines, checkpoints),
    [lines, checkpoints],
  )

  // ── Composer state ──────────────────────────────────────────────────
  //
  // The composer is the ACT of asking; the notebook is the RECORD of having
  // asked. Its draft lives outside the document (a draft is not a line),
  // persisted per notebook scope so an accidental blur, panel toggle, or
  // reload never eats a half-written prompt — see `lib/blackboard-composer.ts`.
  const [draft, setDraft] = useState(() => loadDraft(activeScope))
  // History browse position: null = not browsing. ArrowUp from an empty
  // composer recalls what you last asked; ArrowDown walks back forward.
  const [histIdx, setHistIdx] = useState<number | null>(null)
  const historyRef = useRef<string[]>(loadPromptHistory())
  const scrollRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLTextAreaElement>(null)

  // Scope switch swaps in that scope's own persisted draft. In-render
  // reconcile (React's "adjusting state on prop change" pattern, same as
  // BlackboardLine's streaming sync) — no setState-in-effect cascade.
  const [draftScope, setDraftScope] = useState(activeScope)
  if (draftScope !== activeScope) {
    setDraftScope(activeScope)
    setDraft(loadDraft(activeScope))
    setHistIdx(null)
  }

  // Autosize the composer to its content (recall can change the value
  // without an input event, so this tracks `draft`, not keystrokes). The
  // cap matches the `max-h-40` on the textarea.
  useEffect(() => {
    const el = inputRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`
  }, [draft])

  // The dispatch queue, made visible. The transport serializes prompts
  // (one at a time — `ai-client.ts`'s `turnQueue`); the head entry is the
  // turn in flight (already signalled on the agent's own line), so only
  // the WAITING tail renders here. If the system makes you wait, it owes
  // you the queue.
  const waiting = waitingTurns(useTurnQueue())

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
      const text = draft.trim()
      // No `isProcessing` guard: the transport queues serially and the
      // queue is VISIBLE (the strip above the composer), so a prompt sent
      // mid-turn is accepted and shown waiting — never silently refused.
      if (!text) return
      const nextHistory = pushPromptHistory(historyRef.current, text)
      historyRef.current = nextHistory
      savePromptHistory(nextHistory)
      setDraft('')
      saveDraft(activeScope, '')
      setHistIdx(null)
      // Routes to the agent via the existing ai-client path (wrapped for
      // queue visibility); the user's line lands in the notebook
      // immediately, the reply as an editable line — never a chat bubble.
      void sendPrompt(text, processBlackboardMessage)
    },
    [draft, activeScope],
  )

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        // Enter sends; Shift+Enter falls through to the textarea's native
        // newline — multi-line input without fighting Enter-to-send.
        e.preventDefault()
        handleSubmit()
        return
      }
      if (e.key === 'ArrowUp') {
        const h = historyRef.current
        // Recall only from an empty composer (or while already browsing) —
        // never clobber text being edited just because the caret moved up.
        if (h.length === 0 || (histIdx === null && draft.trim() !== '')) return
        e.preventDefault()
        const idx = histIdx === null ? h.length - 1 : Math.max(0, histIdx - 1)
        setHistIdx(idx)
        setDraft(h[idx])
        return
      }
      if (e.key === 'ArrowDown' && histIdx !== null) {
        e.preventDefault()
        const h = historyRef.current
        const idx = histIdx + 1
        if (idx >= h.length) {
          setHistIdx(null)
          setDraft('')
        } else {
          setHistIdx(idx)
          setDraft(h[idx])
        }
      }
    },
    [handleSubmit, histIdx, draft],
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
            {/* Vendor mark, drawn only when the backend actually named a
                provider — see `acp-session-store.ts`'s `provider` doc for
                why a defaulted logo is refused. */}
            {acpProvider && (
              // `displayName` feeds the initials fallback for vendors with no
              // genuine published mark. The provider id is what the backend
              // reported, so it is the honest label here — this header has no
              // access to the allowlist's prettier `display_name`.
              <VendorMark
                providerId={acpProvider}
                displayName={acpProvider}
                className="h-3 w-3 shrink-0"
              />
            )}
            <span className="font-mono">{acpModel ?? '—'}</span>
            {acpLive && (
              <>
                <span className="text-muted-foreground/40">·</span>
                <span>
                  {acpTurns} turn{acpTurns === 1 ? '' : 's'}
                </span>
                <span className="text-muted-foreground/40">·</span>
                {/* Turn figure first — it answers "what did THAT cost me",
                    which is the question the bare session total kept
                    answering wrongly. Absent until a turn completes. The
                    bolt colours on the TURN's weight, never on a ratio to
                    the context window (which `used` legitimately exceeds). */}
                <span
                  className="inline-flex items-center gap-1"
                  title={
                    acpLastTurnTokens !== null
                      ? `${turnWeight(acpLastTurnTokens).label} — ${TOKENS_TITLE}`
                      : TOKENS_TITLE
                  }
                >
                  <Zap
                    size={10}
                    className={cn(
                      'shrink-0',
                      acpLastTurnTokens !== null
                        ? turnWeight(acpLastTurnTokens).className
                        : 'text-muted-foreground/40',
                    )}
                  />
                  {acpLastTurnTokens !== null ? (
                    <>
                      {formatContextUsage(acpLastTurnTokens)}
                      <span className="text-muted-foreground/50"> this turn</span>
                      <span className="text-muted-foreground/40"> · </span>
                    </>
                  ) : null}
                  <span className={acpLastTurnTokens !== null ? 'text-muted-foreground/60' : ''}>
                    {formatContextUsage(acpTokens ?? 0)}
                  </span>
                  <span className="text-muted-foreground/50"> session</span>
                </span>
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
          {/* EMPTY NOTEBOOK — the common case today. Instead of a blank
              region (or one unlabelled section of nothing), say what this
              panel IS and teach the marker vocabulary the reader is about
              to meet — shape = author, colour = verdict — so the first
              real line is legible on sight (time-to-recognition, not a
              manual). Disappears the moment any line exists. */}
          {lines.length === 0 && !isProcessing && (
            <div className="px-4 py-3 text-xs leading-relaxed">
              <p className="font-medium text-foreground/85">
                An engineering notebook, not a chat.
              </p>
              <p className="mt-0.5 max-w-[60ch] text-muted-foreground/70">
                Ask below — your question, the agent&apos;s reasoning, and the
                kernel&apos;s certificates all land here as lines, grouped under
                named checkpoints.
              </p>
              <div className="mt-3 grid gap-1.5 text-[11px] text-muted-foreground/70">
                <div className="flex items-center gap-2">
                  <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-accent">
                    <Bot size={10} className="text-foreground" />
                  </span>
                  <span>the agent — reasoning, editable</span>
                </div>
                {/* Indented like a real user line — the legend teaches the
                    position channel by using it, not by describing it. */}
                <div className="ml-6 flex items-center gap-2 font-medium">
                  <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-primary/20">
                    <User size={10} className="text-primary" />
                  </span>
                  <span>you — indented, sent from the composer below</span>
                </div>
                <div className="flex items-center gap-2">
                  <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-muted-foreground/20">
                    <Wrench size={9} className="text-muted-foreground" />
                  </span>
                  <span>the kernel — certificates and build steps, never editable</span>
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1">
                  <span>marker colour is the verdict:</span>
                  <span className="inline-flex items-center gap-1 text-emerald-600 dark:text-emerald-400">
                    <Check size={10} /> pass
                  </span>
                  <span className="inline-flex items-center gap-1 text-red-600 dark:text-red-400">
                    <X size={10} /> fail
                  </span>
                  <span className="inline-flex items-center gap-1 text-amber-600 dark:text-amber-400">
                    <CircleSlash size={10} /> inconclusive
                  </span>
                </div>
              </div>
            </div>
          )}
          {sections.map((section, i) => (
            <BlackboardSection
              key={section.checkpoint?.id ?? `leading-${i}`}
              section={section}
              onCommit={editLine}
              onDelete={deleteLine}
              streamingLineId={streamingLineId}
              onCancel={cancelAcpTurn}
              // Earlier checkpoint sections open collapsed so a long
              // notebook lands the eye on CURRENT work, not on scrollback
              // — collapse is a click with a named header and a visible
              // line count, never a hover, so nothing is hidden (reduce
              // scrolling without hiding anything).
              defaultCollapsed={section.checkpoint !== null && i < sections.length - 1}
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

      {/* COMPOSER — the act, kept apart from the record. Visually its own
          anchored region (raised tint + border), never a line of the
          transcript; what it sends still lands in the notebook as a normal
          line. Never disabled mid-turn: the transport queues serially and
          the strip below makes that queue visible. */}
      <div className="shrink-0 border-t border-white/10 bg-white/[0.04]">
        {/* QUEUE STRIP — prompts received but not yet dispatched. Dashed
            amber is the app's existing "not run yet" grammar (ClaimBadge's
            null state in cards/card-chrome.tsx) — waiting is a state, so
            it may have a colour; the full prompt text is readable in the
            chip itself, hover only widens truncation. */}
        {waiting.length > 0 && (
          <div
            role="status"
            className="flex flex-wrap items-center gap-1.5 border-b border-white/5 px-3 py-1.5 text-[10px]"
          >
            <Clock size={10} className="shrink-0 text-amber-500" />
            <span className="shrink-0 font-medium text-amber-600 dark:text-amber-400">
              {waiting.length} queued
            </span>
            <span className="shrink-0 text-muted-foreground/60">
              — sends after the current turn
            </span>
            {waiting.map((w) => (
              <span
                key={w.id}
                title={w.text}
                className="max-w-[16rem] truncate rounded border border-dashed border-amber-500/40 bg-amber-500/5 px-1.5 py-0.5 text-amber-700 dark:text-amber-300"
              >
                {w.text}
              </span>
            ))}
          </div>
        )}
        <form onSubmit={handleSubmit} className="flex items-end gap-1.5 px-2 py-2">
          <textarea
            ref={inputRef}
            value={draft}
            rows={1}
            onChange={(e) => {
              setDraft(e.target.value)
              // Persist per scope on every keystroke — the draft survives
              // blur, panel toggle, and reload; it is not a line until sent.
              saveDraft(activeScope, e.target.value)
              setHistIdx(null)
            }}
            onKeyDown={handleKeyDown}
            placeholder={
              isProcessing
                ? 'Turn in flight — a new prompt queues behind it…'
                : 'Ask the agent — Enter sends, Shift+Enter for a new line…'
            }
            spellCheck={false}
            className="max-h-40 min-h-8 flex-1 resize-none overflow-y-auto rounded-md border border-white/10 bg-background/40 px-2.5 py-[7px] text-xs leading-relaxed text-foreground outline-none placeholder:text-white/30 focus:border-primary/50 scrollbar-thin"
          />
          {/* A visible, labelled send control — the verb, not a paragraph
              keystroke. Disabled only by an empty draft, never by a turn
              in flight (that case queues, visibly). */}
          <Button
            type="submit"
            size="sm"
            disabled={!draft.trim()}
            className="h-8 shrink-0 gap-1 px-2.5"
            title="Send — Enter sends, Shift+Enter for a new line, ArrowUp recalls your last prompt"
            aria-label="Send prompt"
          >
            <Send size={13} />
            <span className="text-[11px]">Send</span>
          </Button>
        </form>
      </div>
    </div>
  )
}
