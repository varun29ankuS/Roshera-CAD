/**
 * How an agent's streamed words land on the Blackboard so a person can follow
 * what happened, in the order it happened.
 *
 * Two failures this replaces (Varun 2026-11-23, a refused fillet): the
 * agent's messages were glued into one run ("...in the model.I'm finding the
 * inside corner..."), and the whole turn's reply sat on ONE line created when
 * the turn began -- so the Intent line the agent opened mid-turn, and the
 * kernel's lines, landed BELOW a reply that already ended "The fillet was
 * refused". The conclusion read before the step that led to it.
 */

/** A streamed chunk added to the text so far. A new message that begins
 *  right after a finished sentence starts a new paragraph instead of running
 *  on ("model.I'm" -> "model.\n\nI'm"); a chunk inside a sentence or a word
 *  is joined as it came. */
export function joinAgentText(prev: string, chunk: string): string {
  if (prev === '' || chunk === '') return prev + chunk
  const ended = /[.!?:]$/.test(prev)
  const opensSentence = /^[A-Z*`#]/.test(chunk)
  return ended && opensSentence ? `${prev}\n\n${chunk}` : prev + chunk
}

/** Whether the agent's next words must start a new line: something else
 *  (an Intent, a kernel line, a tool line) was added after the line it is
 *  writing, so continuing that line would put later words above earlier
 *  events. */
export function needsNewSegment(lineIds: readonly string[], writing: string): boolean {
  return lineIds.length > 0 && lineIds[lineIds.length - 1] !== writing
}
