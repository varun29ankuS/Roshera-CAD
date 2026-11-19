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

/** STEP application protocols by the ISO 10303 part number in the schema's
 *  object id, and by the schema names that carry no id. */
const PROTOCOLS: { part: string; names: RegExp; label: string }[] = [
  { part: '203', names: /^CONFIG_CONTROL_DESIGN/, label: 'STEP AP203 · configuration controlled design' },
  { part: '214', names: /^AUTOMOTIVE_DESIGN/, label: 'STEP AP214 · automotive design' },
  { part: '442', names: /^AP242_/, label: 'STEP AP242 · model-based 3D engineering' },
]

/**
 * The file's STEP schema the way engineers say it ("STEP AP214 · automotive
 * design"), from the raw header string ("AUTOMOTIVE_DESIGN { 1 0 10303 214 1
 * 1 1 1 }"). An unknown schema is shown by its name alone; none, as null.
 */
export function schemaLabel(schema: string | null | undefined): string | null {
  const raw = (schema ?? '').trim()
  if (!raw) return null
  const name = raw.split('{')[0].trim()
  const part = raw.match(/\{\s*1\s+0\s+10303\s+(\d+)/)?.[1]
  const known = PROTOCOLS.find((p) => p.part === part || p.names.test(name))
  return known ? known.label : name
}