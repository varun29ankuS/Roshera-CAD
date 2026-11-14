/**
 * The status bar's readouts (the approved mockup): where the cursor is on
 * the part, what it is over, and the history's verdict.
 */

function mm(v: number): string {
  // A true minus sign, so columns of numbers line up.
  return v.toFixed(2).replace('-', '−')
}

/** The cursor's point on the part, in millimetres, or a dash when off it. */
export function formatPoint(p: readonly [number, number, number] | null): string {
  if (!p) return '—'
  return `X ${mm(p[0])}  Y ${mm(p[1])}  Z ${mm(p[2])}`
}

/** What the cursor is over: the part, and its kernel face when known. */
export function underCursorLabel(hit: { part: string; faceId: number | undefined } | null): string {
  if (!hit) return 'nothing'
  return hit.faceId === undefined ? hit.part : `${hit.part} · face ${hit.faceId}`
}

export type Tone = 'positive' | 'negative' | 'muted'

/** The history's verdict: every step re-measured sound by the kernel, or not.
 *  Unknown until the history has been read -- never "sound" by default. */
export function soundLabel(s: { loaded: boolean; sound: boolean; empty?: boolean }): { text: string; tone: Tone } {
  if (!s.loaded) return { text: 'history unread', tone: 'muted' }
  // Nothing built yet: no verdict to give either way.
  if (s.empty) return { text: 'no features yet', tone: 'muted' }
  return s.sound ? { text: '✓ sound', tone: 'positive' } : { text: '✗ not sound', tone: 'negative' }
}

/** Whether `self` is the nearest object under the cursor. A pointer event
 *  reaches every mesh the ray crosses, nearest first, unless one stops it;
 *  only the nearest may report, or a part behind overwrites the readout. */
export function isNearestHit(intersections: readonly { eventObject: unknown }[], self: unknown): boolean {
  return intersections.length > 0 && intersections[0].eventObject === self
}
