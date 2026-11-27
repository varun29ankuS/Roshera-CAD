/**
 * One poll for one endpoint, however many parts of the app read it.
 *
 * `/api/datums` was fetched by three independent loops (the tree, and one
 * per mounted viewport), each on its own 5 s timer, one of them even while
 * the tab was hidden. A shared poll makes it one request per interval for
 * all readers, none while the tab is hidden, one at once on return, and
 * none at all when nobody is reading.
 *
 * The server has no event for these lists, so an interval it is; where a
 * change is made from this app, the caller asks for a `refresh()` instead of
 * waiting for the next tick.
 */

export interface SharedPollOptions<T> {
  /** Fetch the value. A rejection keeps the last value; the next tick retries. */
  load: () => Promise<T>
  intervalMs: number
  setInterval: (fn: () => void, ms: number) => unknown
  clearInterval: (handle: unknown) => void
  isVisible: () => boolean
  /** Listen for the tab being shown or hidden; returns the un-listener. */
  onVisibilityChange: (fn: () => void) => () => void
}

export interface SharedPoll<T> {
  /** Start reading. `listener` gets the value already known (if any) at
   *  once, and every later one. Returns the function that stops reading. */
  subscribe: (listener: (value: T) => void) => () => void
  /** Ask now (after a change made here). */
  refresh: () => Promise<void>
}

export function createSharedPoll<T>(options: SharedPollOptions<T>): SharedPoll<T> {
  const listeners = new Set<(value: T) => void>()
  let known: { value: T } | null = null
  let timer: unknown = null
  let stopWatchingVisibility: (() => void) | null = null
  let inFlight: Promise<void> | null = null

  const ask = (): Promise<void> => {
    if (inFlight) return inFlight
    inFlight = options
      .load()
      .then((value) => {
        known = { value }
        for (const listener of [...listeners]) listener(value)
      })
      .catch(() => {
        // Unreachable or refused: keep what was known. The next tick retries.
      })
      .finally(() => {
        inFlight = null
      })
    return inFlight
  }

  const start = () => {
    timer = options.setInterval(() => {
      if (options.isVisible()) void ask()
    }, options.intervalMs)
    stopWatchingVisibility = options.onVisibilityChange(() => {
      if (options.isVisible() && listeners.size > 0) void ask()
    })
    if (options.isVisible()) void ask()
  }

  const stop = () => {
    if (timer !== null) options.clearInterval(timer)
    timer = null
    stopWatchingVisibility?.()
    stopWatchingVisibility = null
  }

  return {
    subscribe: (listener) => {
      listeners.add(listener)
      if (listeners.size === 1) start()
      else if (known) listener(known.value)
      return () => {
        listeners.delete(listener)
        if (listeners.size === 0) stop()
      }
    },
    refresh: () => ask(),
  }
}

/** The browser's timers and tab visibility, for `createSharedPoll`. */
export function browserPollClock(): Pick<
  SharedPollOptions<unknown>,
  'setInterval' | 'clearInterval' | 'isVisible' | 'onVisibilityChange'
> {
  return {
    setInterval: (fn, ms) => window.setInterval(fn, ms),
    clearInterval: (handle) => window.clearInterval(handle as number),
    isVisible: () => document.visibilityState === 'visible',
    onVisibilityChange: (fn) => {
      document.addEventListener('visibilitychange', fn)
      return () => document.removeEventListener('visibilitychange', fn)
    },
  }
}
