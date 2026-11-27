/**
 * The Blackboard's arrival motion: an entry PRINTS, one line at a time, like
 * a teleprinter -- each line set in a few discrete steps, the next one a
 * fixed interval later, a certificate row's verdict stamped at the right
 * edge after its row. Stepped and fixed, never eased or floaty (Varun,
 * 2026-11-27: "the animations must feel mechanical").
 *
 * This module is the timing and the guard; `PrintReveal.tsx` draws it.
 */

/** The interval between one line starting and the next. */
export const PRINT_STEP_MS = 36
/** How long one line takes to set. */
export const PRINT_LINE_MS = 80
/** A verdict stamps this long after its row starts, and takes this long. */
export const PRINT_STAMP_DELAY_MS = 70
export const PRINT_STAMP_MS = 90
/** A whole card is on the board within this, however long it is. */
export const PRINT_CAP_MS = 500
/** An entry created within this window has "just arrived". */
const ARRIVAL_WINDOW_MS = 4000

/**
 * Whether an entry prints when it mounts. Only what has just arrived does:
 * the history already on the board when a document opens is simply there.
 * Decided once, at mount, so a re-render never replays it.
 */
export function shouldPrint(
  line: { createdAt: number; streaming: boolean },
  nowMs: number,
  reducedMotion: boolean,
): boolean {
  if (reducedMotion) return false
  if (line.streaming) return true
  const age = nowMs - line.createdAt
  return Number.isFinite(age) && age >= 0 && age < ARRIVAL_WINDOW_MS
}

/** The line interval and the total time for a card of `lineCount` lines.
 *  Long cards print faster so the total never passes the cap. */
export function printSchedule(lineCount: number): { stepMs: number; totalMs: number } {
  const n = Math.max(1, Math.floor(lineCount))
  const tail = PRINT_STAMP_DELAY_MS + PRINT_STAMP_MS
  const stepMs = n > 1 ? Math.min(PRINT_STEP_MS, (PRINT_CAP_MS - tail) / (n - 1)) : PRINT_STEP_MS
  return { stepMs, totalMs: (n - 1) * stepMs + tail }
}

/** The index of each block's first line, given how many lines each block
 *  prints, and the total. */
export function lineStarts(sizes: readonly number[]): { starts: number[]; total: number } {
  const starts: number[] = []
  let total = 0
  for (const size of sizes) {
    starts.push(total)
    total += Math.max(0, size)
  }
  return { starts, total }
}
