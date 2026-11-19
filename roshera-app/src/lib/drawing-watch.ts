/**
 * Open a drawing the agent makes the moment it exists (Varun 2026-11-19:
 * "when the agent starts to make drawing, shouldn't the tab change to
 * drawing?"). The Drawing workspace used to open only when the agent's whole
 * turn ended -- often a minute after the sheet was made, while the agent was
 * still tolerancing and fixing it. The agent's tool calls do not reach the
 * app over ACP on the default provider, so the drawing list is the signal:
 * polled during a turn, the first sheet that was not there before is opened,
 * once.
 */
export interface DrawingWatchDeps {
  /** The ids of every drawing now. */
  list: () => Promise<string[]>
  /** Show this drawing in the Drawing workspace. */
  open: (id: string) => void
  /** Run `fn` repeatedly; returns a function that stops it. */
  every: (fn: () => Promise<void>) => () => void
}

/** Poll every 1.5 s by default. */
export function pollEvery(ms = 1500): DrawingWatchDeps['every'] {
  return (fn) => {
    const h = setInterval(() => void fn(), ms)
    return () => clearInterval(h)
  }
}

/** Start watching; returns the function that stops the watch. */
export function watchForNewDrawing(before: Set<string>, deps: DrawingWatchDeps): () => void {
  let opened = false
  let busy = false
  const stop = deps.every(async () => {
    if (opened || busy) return
    busy = true
    try {
      const ids = await deps.list()
      const created = ids.filter((id) => !before.has(id))
      const newest = created[created.length - 1]
      if (newest) {
        opened = true
        deps.open(newest)
      }
    } catch {
      // The list could not be read this time; the next poll tries again.
    } finally {
      busy = false
    }
  })
  return stop
}
