/**
 * Only the newest of several overlapping requests may apply its answer.
 *
 * Each request takes a ticket when it starts; when its answer arrives it asks
 * whether its ticket is still the newest. A slow answer to an older request
 * that lands after a newer one is dropped instead of overwriting it -- the
 * scene of the previously open document landing over a fresh, empty one
 * (found rehearsing the demo video, 2026-11-18).
 */
export function latestOnly(): { begin: () => number; isLatest: (ticket: number) => boolean } {
  let newest = 0
  return {
    begin: () => ++newest,
    isLatest: (ticket: number) => ticket === newest,
  }
}
