import { useEffect, useRef, useState, type ReactNode } from 'react'
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  CircleSlash,
  Loader2,
  X,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { MessageMarkdown } from './MessageMarkdown'

/**
 * BLACKBOARD MESSAGE PARTS
 * ========================
 * The shapes a Blackboard line can take beyond "prose in the reading lane",
 * kept out of `BlackboardLine.tsx` so that file stays a router (which shape
 * does this line get?) rather than a router plus three renderers.
 *
 * Everything here obeys the two rules the rest of the board already obeys:
 *
 *  1. Nothing is rendered from a field the wire does not carry. Where a
 *     conventional agent-chat UI would show a tool-call duration or a
 *     machine-readable error code, this file shows nothing at all — see the
 *     per-component notes for exactly which fields were looked for and found
 *     absent.
 *  2. No information lives only in a `title=`. Every expand/collapse control
 *     is a visible button with a word on it, and every `title` here restates
 *     something already drawn on screen.
 */

// ── Tool calls ────────────────────────────────────────────────────────

/**
 * The ACP tool-call statuses `lib/acp-blackboard.ts` forwards verbatim from
 * `session/update` frames (`tool_call` / `tool_call_update`). Closed set on
 * purpose: `BlackboardLine.tsx`'s `parseToolCallLine` declines a line
 * carrying any other status rather than inventing a state for it.
 *
 * The parser itself lives beside its one caller in `BlackboardLine.tsx`, not
 * here: this file is a components-only module (Fast Refresh), and a plain
 * function exported alongside components breaks that.
 */
export type ToolCallStatus = 'pending' | 'in_progress' | 'completed' | 'failed'

export interface ParsedToolCall {
  /** The tool's own title, exactly as the agent transport reported it. */
  title: string
  status: ToolCallStatus
  /** Everything after the header — in practice the validated `roshera:*`
   *  card fence `renderToolLine` appends for a tool result that matched a
   *  known wire shape. `null` when the frame carried no renderable payload. */
  body: string | null
}

/** Status word, glyph and colour for a tool call — the same tick / cross /
 *  "not run" vocabulary `cards/card-chrome.tsx` uses for certificate claims,
 *  plus a spinner for the one state a certificate never has: still running. */
const TOOL_STATUS_STYLE: Record<
  ToolCallStatus,
  { word: string; chip: string; glyph: 'spin' | 'tick' | 'cross' | 'idle' }
> = {
  pending: {
    word: 'queued',
    chip: 'border-dashed border-amber-500/40 bg-amber-500/5 text-amber-700 dark:text-amber-300',
    glyph: 'idle',
  },
  in_progress: {
    word: 'running',
    chip: 'border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300',
    glyph: 'spin',
  },
  completed: {
    word: 'done',
    chip: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-800 dark:text-emerald-300',
    glyph: 'tick',
  },
  failed: {
    word: 'failed',
    chip: 'border-red-500/40 bg-red-500/10 text-red-800 dark:text-red-300',
    glyph: 'cross',
  },
}

function ToolStatusGlyph({ status }: { status: ToolCallStatus }) {
  const kind = TOOL_STATUS_STYLE[status].glyph
  if (kind === 'spin') return <Loader2 size={10} className="shrink-0 animate-spin" />
  if (kind === 'tick') return <Check size={10} className="shrink-0" />
  if (kind === 'cross') return <X size={10} className="shrink-0" />
  return <CircleSlash size={10} className="shrink-0" />
}

/**
 * TOOL CALL — a chip row, not a sentence.
 *
 * Tool name in mono (it is an identifier, and reads as one), a status chip
 * carrying both a glyph and its word, and — only when the frame actually
 * carried a payload — a visible button that expands it. Collapsed by
 * default: a turn that makes eight tool calls should read as eight rows,
 * not eight result blocks.
 *
 * There is deliberately NO duration. `session/update`'s `tool_call` and
 * `tool_call_update` frames carry `toolCallId`, `title`, `kind`, `status`
 * and content — no start time, no elapsed field — and `renderToolLine`
 * forwards nothing else, so any number here would be one this component
 * invented. Same for an argument summary: the transport gives one human
 * `title`, never the call's arguments.
 *
 * The expanded payload is rendered through the normal markdown/card
 * pipeline rather than as raw JSON, because `renderToolLine` only ever
 * appends a payload that ALREADY validated against a real wire schema
 * (`cardFenceForPayload`) — a certificate or refusal card is strictly more
 * legible than the same bytes in a `<pre>`, and anything that failed
 * validation never reaches here in the first place. The block is
 * height-capped and scrolls so a large certificate cannot push the rest of
 * the board off screen.
 */
export function ToolCallRow({ call }: { call: ParsedToolCall }) {
  const [expanded, setExpanded] = useState(false)
  const style = TOOL_STATUS_STYLE[call.status]
  return (
    <div className="min-w-0">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
        {/* Wraps rather than truncates: a clipped tool name would be
            readable nowhere at all (there is no hover text here by rule),
            and these titles are short enough that wrapping costs nothing. */}
        <span className="min-w-0 max-w-[48ch] break-words font-mono text-[11px] text-foreground/85">
          {call.title}
        </span>
        <span
          className={cn(
            'inline-flex shrink-0 items-center gap-1 rounded border px-1.5 py-[2px] text-[10px] leading-none',
            style.chip,
          )}
        >
          <ToolStatusGlyph status={call.status} />
          {style.word}
        </span>
        {call.body !== null && (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            aria-expanded={expanded}
            className="cad-icon-btn h-5 shrink-0 gap-1 px-1.5 text-[10px] text-muted-foreground/80"
          >
            {expanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
            {expanded ? 'hide result' : 'result'}
          </button>
        )}
      </div>
      {expanded && call.body !== null && (
        <div className="mt-1 max-h-64 max-w-[72ch] overflow-y-auto scrollbar-thin pr-1">
          <MessageMarkdown content={call.body} />
        </div>
      )}
    </div>
  )
}

// ── Failed turns ──────────────────────────────────────────────────────

/**
 * FAILED TURN — a designed block, never a red paragraph.
 *
 * Driven by `line.turnStatus === 'failed'`, a typed field the store sets in
 * `runAcpTurn`'s own `renderTurnFailure` — never by sniffing the word
 * "failed" out of prose. The message itself is reproduced VERBATIM through
 * the normal markdown pipeline (it is written by
 * `describeAcpTurnFailure`, which already names what failed and what would
 * fix it); the only text this component strips is the leading `⚠`, which
 * the block's own icon now carries.
 *
 * WHAT IS NOT SHOWN, AND WHY: an `error_code` chip and a retryable yes/no.
 * The backend's `ApiError` envelope does carry `error_code` / `retryable` /
 * `hint` (see `lib/provider-api.ts`), but nothing on the Blackboard's own
 * path preserves them: `refusalCardSchema` is `{ reason, subject?, source?,
 * options? }`, and a failed turn reaches this component as a rendered
 * sentence, not as a typed error. Drawing a code chip here would mean
 * fabricating one. The chip below instead carries `turnStatus` — a value
 * the store genuinely holds.
 */
export function FailedTurnBlock({ text }: { text: string }) {
  const body = text.replace(/^\s*⚠\s*/, '')
  return (
    <div className="my-0.5 w-full max-w-[72ch] rounded-md border border-red-500/35 border-l-2 border-l-red-500/80 bg-red-500/[0.06] px-2.5 py-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <AlertTriangle size={13} className="shrink-0 text-red-600 dark:text-red-400" />
        <span className="text-xs font-medium text-foreground/90">Turn failed</span>
        <span className="ml-auto inline-flex shrink-0 items-center gap-1 rounded border border-red-500/40 px-1.5 py-[2px] font-mono text-[10px] uppercase tracking-wide text-red-700 dark:text-red-300">
          <X size={9} />
          failed
        </span>
      </div>
      <div className="mt-1.5 border-l-2 border-red-500/25 pl-2 text-[11px] leading-relaxed text-foreground/85">
        <MessageMarkdown content={body} />
      </div>
    </div>
  )
}

// ── Long prose ────────────────────────────────────────────────────────

/** Height at which a settled prose line gets a "show more" control. Roughly
 *  a dozen lines at this panel's 13px/1.6 body — past that a single reply
 *  starts owning the whole board. */
const PROSE_CAP_PX = 320

/**
 * Cheap pre-filter: a line's SOURCE must be at least this long before its
 * rendered height is worth measuring at all. Measuring is what decides
 * whether the cap applies — this only decides whether an observer is worth
 * mounting.
 *
 * 400 characters is calibrated against the densest shape a short line can
 * take: a bulleted list at ~30 characters a bullet is roughly 13 rendered
 * lines by 400 characters, still short of the ~15 the cap allows at this
 * panel's 72ch measure. Prose at the same length is barely 6 lines. So the
 * gate cannot hide a line the cap would have clamped, and it keeps a
 * `ResizeObserver` off every one-sentence line on the board.
 */
const PROSE_MEASURE_FLOOR_CHARS = 400

/**
 * LONG AGENT TEXT — capped with a visible control, never a hover.
 *
 * A wall of prose is clamped, faded at the cut, and given a real button
 * that says how to see the rest. The fade is decorative only: the button
 * beneath it is what tells the reader there is more, so nothing depends on
 * noticing a gradient.
 *
 * Applied ONLY to settled prose. A streaming line is never clamped — it
 * would fight the board's autoscroll-to-bottom and hide the very text
 * currently arriving — and evidence lines are never clamped either, because
 * a certificate's verdict can sit anywhere in the card and must not fall
 * below a cut.
 *
 * Overflow is measured, not guessed: a `ResizeObserver` reports the real
 * rendered height (its first callback fires on `observe`, which is also the
 * initial measurement), so a line that is long only because it embeds a
 * card, a table or a KaTeX display is measured the same as one that is long
 * in characters.
 */
export function ExpandableProse({
  source,
  children,
}: {
  /** The line's raw markdown source — read ONLY for its length, to decide
   *  whether measuring is worth an observer (`PROSE_MEASURE_FLOOR_CHARS`).
   *  What actually renders is `children`. */
  source: string
  children: ReactNode
}) {
  const enabled = source.length > PROSE_MEASURE_FLOOR_CHARS
  const contentRef = useRef<HTMLDivElement>(null)
  const [overflows, setOverflows] = useState(false)
  const [expanded, setExpanded] = useState(false)

  useEffect(() => {
    const el = contentRef.current
    if (!el || !enabled) return
    // `ResizeObserver` fires once immediately on `observe`, so this single
    // subscription covers both the initial measurement and every later
    // reflow (fonts loading, a card expanding, the panel being resized).
    const observer = new ResizeObserver(() => {
      setOverflows(el.scrollHeight > PROSE_CAP_PX + 24)
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [enabled])

  if (!enabled) return <>{children}</>

  return (
    <div>
      <div
        ref={contentRef}
        className={cn(
          'relative',
          overflows &&
            !expanded &&
            // The cut is faded by masking the CONTENT to transparent, not by
            // laying an opaque gradient over it. The panel is
            // `bg-background/35 backdrop-blur-md` — translucent over the live
            // viewport — so a `from-background` overlay is a solid block of
            // the wrong colour and reads as a grey bar, which is what this
            // measured as before the mask.
            'max-h-80 overflow-hidden [mask-image:linear-gradient(to_bottom,black_calc(100%-3.5rem),transparent)] [-webkit-mask-image:linear-gradient(to_bottom,black_calc(100%-3.5rem),transparent)]',
        )}
      >
        {children}
      </div>
      {overflows && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          className="cad-icon-btn mt-1 h-5 gap-1 px-1.5 text-[10px] text-muted-foreground/80"
        >
          {expanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
          {expanded ? 'Show less' : 'Show full message'}
        </button>
      )}
    </div>
  )
}
