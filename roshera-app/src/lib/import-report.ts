/**
 * How the STEP import card states the entities whose handler raised a
 * warning or an error (the importer's `counts.failed` bucket).
 *
 * That bucket mixes warnings (repairs, tolerances) with errors, so on a solid
 * the kernel checked sound it is said as warnings, in amber; only when a
 * solid is not sound is it a failure, in red. The per-type counts go on the
 * hover. Found rehearsing the demo video (2026-11-18): "✓ imported clean"
 * above a red "6306 entities failed".
 */
export interface EntityIssueLine {
  text: string
  tone: 'caution' | 'negative'
  /** Each entity type and its count, one per line. */
  title: string
}

export function entityIssueLine(failed: Record<string, number>, allSolidsSound: boolean): EntityIssueLine | null {
  const entries = Object.entries(failed).filter(([, n]) => n > 0)
  const total = entries.reduce((s, [, n]) => s + n, 0)
  if (total === 0) return null
  const types = entries.length
  const typesText = `${types} type${types === 1 ? '' : 's'}`
  const entitiesText = `${total} entit${total === 1 ? 'y' : 'ies'}`
  const title = entries
    .sort((a, b) => b[1] - a[1])
    .map(([k, n]) => `${k}: ${n}`)
    .join('\n')
  return allSolidsSound
    ? { text: `${entitiesText} raised warnings or errors (${typesText}) — the solid still checks sound`, tone: 'caution', title }
    : { text: `${entitiesText} failed (${typesText})`, tone: 'negative', title }
}
