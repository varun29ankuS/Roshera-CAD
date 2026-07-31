import { useContext, useEffect, useState, type ReactNode } from 'react'
import { cn } from '@/lib/utils'
import { RevealContext } from './reveal-context'

/**
 * CHALK-DRAW REVEAL
 * =================
 * The differentiated write animation for the Blackboard: a completed key
 * result (a typed card, a certified frame) draws on like chalk — an SVG
 * stroke traces the frame (`stroke-dasharray` with `pathLength={1}`) while
 * the content wipes in left-to-right. Prose streams fast and plainly; this
 * is spent on THE result, so importance is legible without reading.
 *
 * Hard rules, honoured here:
 *  - SKIPPABLE AND INSTANT ON INTERACTION — any click on the element, or any
 *    pointer/key interaction anywhere while drawing, completes it instantly.
 *    `prefers-reduced-motion` disables it entirely.
 *  - NEVER GATES THE GEOMETRY — this is pure presentation inside a
 *    Blackboard line. The viewport is driven by ws-bridge/scene-store and
 *    updates when the kernel confirms, regardless of what is animating here.
 *
 * Whether to animate at all comes from `RevealContext` (reveal-context.ts):
 * the owning BlackboardLine sets it once at mount (true only for agent
 * content that just arrived), so a reload of persisted history renders
 * instantly.
 */

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

interface ChalkRevealProps {
  children: ReactNode
  className?: string
  /** Trace a chalk stroke around the content while it wipes in. */
  framed?: boolean
}

export function ChalkReveal({ children, className, framed = false }: ChalkRevealProps) {
  const { animate } = useContext(RevealContext)
  const [done, setDone] = useState(() => !animate || prefersReducedMotion())

  // While drawing, ANY interaction finishes the reveal instantly — the
  // animation must never make the user wait for the content.
  useEffect(() => {
    if (done) return
    const finish = () => setDone(true)
    const timer = window.setTimeout(finish, 1000)
    window.addEventListener('pointerdown', finish, true)
    window.addEventListener('keydown', finish, true)
    return () => {
      window.clearTimeout(timer)
      window.removeEventListener('pointerdown', finish, true)
      window.removeEventListener('keydown', finish, true)
    }
  }, [done])

  if (done) {
    return <div className={className}>{children}</div>
  }

  return (
    <div className={cn('relative', className)} onClick={() => setDone(true)}>
      <div className="chalk-wipe">{children}</div>
      {framed && (
        <svg
          className="chalk-stroke pointer-events-none absolute inset-0 h-full w-full"
          viewBox="0 0 100 40"
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          <rect
            x="0.4"
            y="0.6"
            width="99.2"
            height="38.8"
            rx="1.5"
            pathLength={1}
            fill="none"
            stroke="currentColor"
            strokeWidth={1.5}
            vectorEffect="non-scaling-stroke"
          />
        </svg>
      )}
    </div>
  )
}
