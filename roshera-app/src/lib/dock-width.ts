/**
 * How wide the docked Blackboard is.
 *
 * About 380 px (Varun 2026-11-27): at 320 a certificate card was 205 px wide
 * and a part's name did not fit its title. The user can drag the dock's edge
 * to any width from 320 to 520 and it is remembered. A window too narrow to
 * give 380 away (under 1280) starts at 320, and the dock never takes more
 * than 40% of the window: the viewport is the work.
 */

export const DOCK_DEFAULT = 380
export const DOCK_MIN = 320
export const DOCK_MAX = 520
/** Below this window width the default is the minimum. */
const ROOMY_WINDOW = 1280
const MAX_SHARE = 0.4

/** The dock's width for a saved choice (null = none) and a window width. */
export function dockWidth(saved: number | null, windowWidth: number): number {
  const chosen = saved !== null && Number.isFinite(saved) ? saved : null
  const wanted = chosen ?? (windowWidth >= ROOMY_WINDOW ? DOCK_DEFAULT : DOCK_MIN)
  const ceiling = Math.max(DOCK_MIN, Math.min(DOCK_MAX, Math.floor(windowWidth * MAX_SHARE)))
  return Math.min(Math.max(wanted, DOCK_MIN), ceiling)
}
