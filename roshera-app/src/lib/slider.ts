/**
 * The history slider's geometry: chapter marks spread end to end along one
 * track (the right end is the live part), the mark nearest a pointer, and
 * how play advances. A general time axis: today its marks are chapters; the
 * same control will carry simulation time steps.
 */

/** Where mark `i` of `count` sits along the track, 0 (left) .. 1 (right).
 *  A single mark is the live end. */
export function markFraction(i: number, count: number): number {
  if (count <= 1) return 1
  return i / (count - 1)
}

/** The mark nearest screen x on a track `width` px wide starting at `left`,
 *  clamped to the first and last. */
export function nearestMark(x: number, left: number, width: number, count: number): number {
  if (count <= 1 || width <= 0) return 0
  const f = Math.min(1, Math.max(0, (x - left) / width))
  return Math.round(f * (count - 1))
}

/** The next mark while playing, or null once the live end is reached. */
export function nextPlayIndex(i: number, count: number): number | null {
  return i + 1 < count ? i + 1 : null
}
