import { useCallback, useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'

const STORE_KEY = 'roshera.rail.agentEyeHeight'
const MIN_PX = 96
/** Leave the tenant above at least this much, whatever the rail's height. */
const MIN_ABOVE_PX = 160

/**
 * Read the persisted height once, defensively.
 *
 * A blocked or unavailable `localStorage` must not stop the rail rendering, and
 * a corrupt value must not size a panel to `NaN` — either way the answer is
 * "no stored preference", which the caller reads as auto.
 */
function readStored(): number | null {
  try {
    const raw = localStorage.getItem(STORE_KEY)
    if (raw === null) return null
    const n = Number.parseFloat(raw)
    return Number.isFinite(n) && n >= MIN_PX ? n : null
  } catch {
    return null
  }
}

export interface RailSplitState {
  /** `null` = auto: the panel below is sized by its own content. */
  height: number | null
  /** Props for the draggable divider. */
  handleProps: {
    onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => void
    onDoubleClick: () => void
    onKeyDown: (e: React.KeyboardEvent<HTMLDivElement>) => void
  }
  dragging: boolean
}

/**
 * A horizontal splitter between two stacked rail tenants.
 *
 * The panel BELOW is the one given an explicit height; the panel above takes
 * what is left. That direction matters: the tenant above is the conversation,
 * which grows without bound, while the one below is an instrument with a
 * natural size — so "how tall is the instrument" is the question a human is
 * actually answering when they drag this, and the transcript absorbs the
 * remainder either way.
 *
 * The height is a MAX, not a fixed size, so a collapsed panel below still
 * renders at its own small height instead of being padded out to a number the
 * user chose while it was open.
 */
export function useRailSplit(containerRef: React.RefObject<HTMLElement | null>): RailSplitState {
  const [height, setHeight] = useState<number | null>(() => readStored())
  const [dragging, setDragging] = useState(false)
  const originRef = useRef<{ y: number; h: number } | null>(null)

  const persist = useCallback((next: number | null) => {
    try {
      if (next === null) localStorage.removeItem(STORE_KEY)
      else localStorage.setItem(STORE_KEY, String(Math.round(next)))
    } catch {
      /* a preference that cannot be stored is still honoured this session */
    }
  }, [])

  const clamp = useCallback(
    (px: number) => {
      const railH = containerRef.current?.getBoundingClientRect().height ?? 0
      // The ceiling is the rail's own height minus what the tenant above
      // needs, so this cannot be dragged into hiding the conversation
      // entirely — and it re-derives per drag, so it stays correct when the
      // window is resized rather than trusting a number from a bigger screen.
      const ceiling = Math.max(MIN_PX, railH - MIN_ABOVE_PX)
      return Math.min(Math.max(px, MIN_PX), ceiling)
    },
    [containerRef],
  )

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const current =
        height ??
        // Starting from auto: measure what the panel below is actually
        // occupying right now, so the first drag continues from where the
        // eye sees it rather than jumping to a default.
        (e.currentTarget.nextElementSibling?.getBoundingClientRect().height ?? MIN_PX)
      originRef.current = { y: e.clientY, h: current }
      setDragging(true)
      try {
        // Capture keeps the drag alive when the cursor outruns this 1px
        // divider, but it throws for a pointer the browser no longer considers
        // active. The window-level listeners below are what actually drive the
        // drag, so a failed capture must not abort it.
        e.currentTarget.setPointerCapture(e.pointerId)
      } catch {
        /* drag continues on the window listeners */
      }
      e.preventDefault()
    },
    [height],
  )

  useEffect(() => {
    if (!dragging) return
    const move = (e: PointerEvent) => {
      const origin = originRef.current
      if (!origin) return
      // Dragging UP grows the panel below, which is the direction the handle
      // visibly moves.
      setHeight(clamp(origin.h + (origin.y - e.clientY)))
    }
    const up = () => {
      setDragging(false)
      originRef.current = null
      setHeight((h) => {
        persist(h)
        return h
      })
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
    window.addEventListener('pointercancel', up)
    return () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      window.removeEventListener('pointercancel', up)
    }
  }, [dragging, clamp, persist])

  const reset = useCallback(() => {
    setHeight(null)
    persist(null)
  }, [persist])

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      // A divider that only responds to a pointer is unreachable for anyone
      // driving by keyboard, and this one controls how much of the agent's
      // output is visible.
      const step = e.shiftKey ? 48 : 16
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        setHeight((h) => {
          const next = clamp((h ?? MIN_PX) + step)
          persist(next)
          return next
        })
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setHeight((h) => {
          const next = clamp((h ?? MIN_PX) - step)
          persist(next)
          return next
        })
      } else if (e.key === 'Home' || e.key === 'Escape') {
        e.preventDefault()
        reset()
      }
    },
    [clamp, persist, reset],
  )

  return { height, dragging, handleProps: { onPointerDown, onDoubleClick: reset, onKeyDown } }
}

/**
 * The visible divider. A hairline at rest with a generous invisible grab
 * area — a 1px target is honest about where the boundary is and dishonest
 * about how hard it is to hit.
 */
export function RailSplitter({
  dragging,
  ...handlers
}: RailSplitState['handleProps'] & { dragging: boolean }) {
  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize the agent view — drag, or arrow keys; double-click to fit its content"
      title="Drag to resize · double-click to fit"
      tabIndex={0}
      className={cn(
        'group relative h-1 shrink-0 cursor-ns-resize touch-none',
        'focus-visible:outline-none',
      )}
      {...handlers}
    >
      {/* The grab area extends past the visible line without taking layout
          space from either neighbour. */}
      <span aria-hidden className="absolute inset-x-0 -top-1.5 -bottom-1.5" />
      <span
        aria-hidden
        className={cn(
          'absolute inset-x-0 top-0 h-px transition-colors',
          dragging
            ? 'bg-primary'
            : 'bg-border group-hover:bg-primary/60 group-focus-visible:bg-primary',
        )}
      />
    </div>
  )
}
