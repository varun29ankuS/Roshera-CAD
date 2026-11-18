/**
 * TAIL FOLLOWING — the board scrolls to new lines only while the reader is
 * already reading the newest ones.
 * ======================================================================
 * Before this, every change to the notebook (a streamed chunk, a kernel
 * "Created …" echo, the agent's attention flipping) set the scroll to the
 * bottom, so an engineer scrolled up to read a certificate was thrown off
 * it by the next line. Now the board follows the tail while the view sits
 * within `TAIL_STICK_PX` of it; once the reader scrolls up, the view stays
 * put and `Blackboard.tsx` offers "Jump to latest" instead.
 */

/** How close to the bottom still counts as "at the tail": a little more
 *  than one line, so sub-pixel layout and a line arriving mid-scroll do not
 *  unpin a reader who is, to the eye, at the bottom. */
export const TAIL_STICK_PX = 48

export interface ScrollMetrics {
  scrollTop: number
  scrollHeight: number
  clientHeight: number
}

/** Whether the view is at (or within `slack` px of) the newest line.
 *  Content shorter than the view is always at the tail. */
export function isAtTail(m: ScrollMetrics, slack: number = TAIL_STICK_PX): boolean {
  return m.scrollHeight - m.clientHeight - m.scrollTop <= slack
}
