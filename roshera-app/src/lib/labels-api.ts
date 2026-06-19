/**
 * REST client for the LABELLER (`GET /api/agent/parts/{id}/labels`).
 *
 * A label is a human/agent-supplied NAME pinned to a durable topological
 * entity (a vertex, edge, or face) or a named section plane. The kernel
 * is the single source of truth: it stores the name against a persistent
 * id and, on read, resolves that id back to the live entity and reports
 * the entity's current world anchor (face centroid / edge midpoint /
 * vertex position / section-plane origin).
 *
 * ## The stale signal
 * The endpoint does not (yet) ship an explicit `stale`/`consistent` flag.
 * Instead, a label whose entity has been deleted or regenerated away no
 * longer resolves to a live id, so the kernel cannot produce a world
 * anchor for it — `anchor` comes back `null`. That `null` IS the stale
 * signal: the name is still held, but its assertion ("there is an entity
 * here") no longer holds. We surface such labels distinctly (amber +
 * strikethrough) rather than silently dropping them, matching the
 * kernel's "I had it, it's gone" honesty (`LabelError::Dangling`).
 *
 * If the backend later adds an explicit flag, switch {@link Label.stale}
 * to read it directly — the consumer code already keys off that boolean,
 * so the only change would be here in {@link parseLabel}.
 */

const API_HOST = import.meta.env.VITE_API_URL || ''

/** Which kind of topological entity (or section plane) a label points at. */
export type LabelKind = 'vertex' | 'edge' | 'face' | 'section'

/** One label as the viewport consumes it. */
export interface Label {
  /** The human-readable name the user/agent pinned. Unique per part. */
  name: string
  /** What the name points at. */
  kind: LabelKind
  /**
   * World-space callout point: face centroid / edge midpoint / vertex
   * position / section-plane origin. `null` when the named entity no
   * longer resolves (see {@link stale}).
   */
  anchor: [number, number, number] | null
  /** Optional free-text description the author attached. */
  description: string | null
  /**
   * True when the label's assertion no longer holds — its entity was
   * deleted/regenerated away, so the kernel could not produce an anchor.
   * Derived from `anchor === null` until the backend exposes an explicit
   * consistency flag.
   */
  stale: boolean
}

/** Raw wire shape of one label from `GET /api/agent/parts/{id}/labels`. */
interface WireLabel {
  name: string
  kind: string
  anchor: [number, number, number] | null
  description: string | null
}

/** Raw wire shape of the labels-list response. */
interface WireLabelsResponse {
  part_id: number
  labels: WireLabel[]
}

const KNOWN_KINDS: readonly LabelKind[] = ['vertex', 'edge', 'face', 'section']

function isLabelKind(s: string): s is LabelKind {
  return (KNOWN_KINDS as readonly string[]).includes(s)
}

function isVec3(v: unknown): v is [number, number, number] {
  return (
    Array.isArray(v) &&
    v.length === 3 &&
    v.every((n) => typeof n === 'number' && Number.isFinite(n))
  )
}

/** Validate + normalise one wire label into a {@link Label}. */
function parseLabel(raw: WireLabel): Label | null {
  if (typeof raw.name !== 'string' || raw.name.length === 0) return null
  if (typeof raw.kind !== 'string' || !isLabelKind(raw.kind)) return null
  const anchor = isVec3(raw.anchor) ? raw.anchor : null
  const description =
    typeof raw.description === 'string' ? raw.description : null
  return {
    name: raw.name,
    kind: raw.kind,
    anchor,
    description,
    // anchor === null ⇒ the named entity no longer resolves ⇒ stale.
    stale: anchor === null,
  }
}

/**
 * List every label on the active part.
 *
 * The route is part-scoped (`/parts/{id}/labels`), but the api-server
 * selects the model from the `X-Roshera-Part-Id` header (falling back to
 * the active single-document model when the header is absent) and the
 * `{id}` path segment is only echoed back in the response. The viewport
 * works against that active document, so we send `0` as a placeholder id
 * and let the backend's active-model fallback resolve the part — exactly
 * how the rest of the viewport's REST calls already work.
 *
 * Returns labels in the kernel's name order. Throws on a non-2xx so the
 * caller can decide whether a fetch failure is worth surfacing.
 */
export async function listLabels(): Promise<Label[]> {
  const r = await fetch(`${API_HOST}/api/agent/parts/0/labels`)
  if (!r.ok) {
    let detail = ''
    try {
      detail = await r.text()
    } catch {
      /* ignore body parse errors */
    }
    throw new Error(
      `listLabels: ${r.status} ${r.statusText}${detail ? ` — ${detail}` : ''}`,
    )
  }
  const body = (await r.json()) as WireLabelsResponse
  if (!Array.isArray(body.labels)) return []
  return body.labels
    .map(parseLabel)
    .filter((l): l is Label => l !== null)
}
