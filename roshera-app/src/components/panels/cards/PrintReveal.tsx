import { useEffect, useState, type CSSProperties } from 'react'
import { printSchedule } from '@/lib/print-reveal'

/**
 * PRINT REVEAL -- how the Blackboard writes.
 * ==========================================
 * A new entry prints one line at a time, like a teleprinter: each line is
 * set in a few discrete steps, the next starts a fixed interval later, and a
 * certificate row's verdict is stamped at the right edge after its row
 * (lib/print-reveal.ts holds the timing; index.css holds the keyframes).
 * It replaced the chalk-draw reveal: stepped and exact, not soft.
 *
 * The same hard rules as the reveal it replaced:
 *  - Only what has just arrived prints. Whether to print is decided once, by
 *    the owning BlackboardLine (`RevealContext`); history on the board when a
 *    document opens is simply there.
 *  - Any click or key finishes it at once. Reduced motion never starts it.
 *  - It is presentation only: nothing waits on it.
 *
 * A line is marked `data-print="line"` (a verdict `data-print="stamp"`) only
 * WHILE the run is on, with its position in `--i`; when the run ends the
 * marks are removed, so a later re-render or re-mount can never replay it.
 */

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

/** Runs one print of `lineCount` lines. `printing` is true until it ends or
 *  is skipped; `style` carries the line interval for the keyframes. */
export function usePrintRun(animate: boolean, lineCount: number): { printing: boolean; style: CSSProperties } {
  const [printing, setPrinting] = useState(() => animate && !prefersReducedMotion())
  const { stepMs, totalMs } = printSchedule(lineCount)

  useEffect(() => {
    if (!printing) return
    const finish = () => setPrinting(false)
    const timer = window.setTimeout(finish, totalMs + 40)
    window.addEventListener('pointerdown', finish, true)
    window.addEventListener('keydown', finish, true)
    return () => {
      window.clearTimeout(timer)
      window.removeEventListener('pointerdown', finish, true)
      window.removeEventListener('keydown', finish, true)
    }
  }, [printing, totalMs])

  return {
    printing,
    style: printing ? ({ '--print-step': `${stepMs}ms` } as CSSProperties) : {},
  }
}

/** The attributes that make an element line `index` of the current print,
 *  or nothing when no print is running. */
export function printLine(printing: boolean, index: number): { 'data-print'?: 'line'; style?: CSSProperties } {
  return printing ? { 'data-print': 'line', style: { '--i': index } as CSSProperties } : {}
}
