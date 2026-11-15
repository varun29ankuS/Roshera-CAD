/**
 * One request for many simultaneous readers of the same server state.
 *
 * Several panels read the document's chapters (status bar, Blackboard,
 * feature tree, timeline strip, feature locations), each through its own
 * `useChapters`. Each asked the server separately, so every model change and
 * every tab switch sent the same `GET /api/timeline/chapters/main` four or
 * five times at once, and that route replays and certifies the history on
 * the server every time.
 *
 * The sharing rule keeps every reader correct: a reader reacting to a change
 * at time `changedAt` may share an in-flight request only if that request
 * STARTED at or after `changedAt`, so it cannot have been answered from the
 * state before the change. A finished request is never reused (this is not
 * a cache), and a failure reaches every reader that shared it.
 */
export type Coalesce<T> = (key: string, changedAt: number, start: () => Promise<T>) => Promise<T>

export function createCoalescer<T>(now: () => number = () => performance.now()): Coalesce<T> {
  const inFlight = new Map<string, { startedAt: number; promise: Promise<T> }>()
  return (key, changedAt, start) => {
    const have = inFlight.get(key)
    if (have && have.startedAt >= changedAt) return have.promise
    const startedAt = now()
    const promise: Promise<T> = start().finally(() => {
      if (inFlight.get(key)?.promise === promise) inFlight.delete(key)
    })
    inFlight.set(key, { startedAt, promise })
    return promise
  }
}
