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
 * The kernel ships an explicit `stale` boolean on each label. A label
 * also goes stale implicitly when its entity has been deleted or
 * regenerated away: it no longer resolves to a live id, so the kernel
 * cannot produce a world anchor and `anchor` comes back `null`. We treat
 * EITHER signal (`stale === true` OR `anchor === null`) as stale: the
 * name is still held, but its assertion ("there is an entity here") no
 * longer holds. We surface such labels distinctly (amber + strikethrough)
 * rather than silently dropping them, matching the kernel's "I had it,
 * it's gone" honesty (`LabelError::Dangling`).
 *
 * ## Colour, measurement, conformance (the enriched contract)
 * Each label carries a deterministic `color` (`#rrggbb`) the viewport
 * paints the callout + the target entity's faces in, so the user can tell
 * which mark is which feature; an optional `measurement` (a verified
 * dimension with a pre-formatted `display` string such as `"Ø2.00 mm"`);
 * and an optional `conformance` verdict (in/out of spec). All three are
 * OPTIONAL on the wire: an older backend that has not shipped the
 * enrichment yet omits them, and {@link parseLabel} falls back to a
 * neutral chip with no measurement/badge. Never assume they are present.
 */

const API_HOST = import.meta.env.VITE_API_URL || ''

/** Which kind of topological entity (or section plane) a label points at. */
export type LabelKind = 'vertex' | 'edge' | 'face' | 'section'

/**
 * Conformance verdict for a label's measurement against its spec.
 *   * `in_spec`      → measured value meets the tolerance (✓, green).
 *   * `out_of_spec`  → measured value violates the tolerance (✗, red).
 *   * `not_verified` → no spec to check against (no badge).
 */
export type LabelConformance = 'in_spec' | 'out_of_spec' | 'not_verified'

/**
 * A verified dimension attached to a labelled entity. `display` is the
 * kernel's pre-formatted, units-bearing string (e.g. `"Ø2.00 mm"`,
 * `"12.50 mm"`) — render it verbatim so the unit/precision policy lives
 * in one place (the kernel), not scattered across the client.
 */
export interface LabelMeasurement {
  /** Raw numeric magnitude in {@link unit}. */
  value: number
  /** Unit symbol the value is expressed in (e.g. `"mm"`, `"deg"`). */
  unit: string
  /** What the value measures (e.g. `"diameter"`, `"length"`, `"angle"`). */
  kind: string
  /** Pre-formatted, units-bearing string the viewport shows verbatim. */
  display: string
}

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
  /**
   * The kernel `FaceId` / `EdgeId` / `VertexId` the name pins, when the
   * backend ships it. Used to tint the labelled entity's faces in the
   * label colour (see PartLabels' face-tint overlay). `null` for section
   * labels (a plane, not a topological entity) and for older backends /
   * stale labels that don't resolve to a live id.
   */
  entityId: number | null
  /** Optional free-text description the author attached. */
  description: string | null
  /**
   * Deterministic per-label callout colour (`#rrggbb`). The viewport
   * paints the chip border/accent/leader AND the target entity's faces
   * in this colour so the user can match a mark to its feature. `null`
   * on older backends → neutral chip.
   */
  color: string | null
  /** Verified, units-bearing dimension for this entity (if any). */
  measurement: LabelMeasurement | null
  /** Spec-conformance verdict for {@link measurement} (if any). */
  conformance: LabelConformance | null
  /**
   * True when the label's assertion no longer holds — its entity was
   * deleted/regenerated away. Honours the kernel's explicit `stale` flag
   * and, defensively, also treats a missing world anchor (`anchor ===
   * null`) as stale.
   */
  stale: boolean
}

/** Raw wire shape of one label from `GET /api/agent/parts/{id}/labels`. */
interface WireLabel {
  name: string
  kind: string
  anchor: [number, number, number] | null
  /** Optional (enriched contract): kernel entity id for face-tinting. */
  entity_id?: number | null
  description: string | null
  /** Optional (enriched contract): deterministic `#rrggbb` callout colour. */
  color?: string | null
  /** Optional (enriched contract): verified, units-bearing dimension. */
  measurement?: {
    value?: unknown
    unit?: unknown
    kind?: unknown
    display?: unknown
  } | null
  /** Optional (enriched contract): spec-conformance verdict. */
  conformance?: string | null
  /** Optional (enriched contract): explicit dangling/stale flag. */
  stale?: boolean
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

const KNOWN_CONFORMANCE: readonly LabelConformance[] = [
  'in_spec',
  'out_of_spec',
  'not_verified',
]

/** Accept only a `#rrggbb` / `#rgb` hex string; anything else → null. */
function parseHexColor(v: unknown): string | null {
  if (typeof v !== 'string') return null
  return /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(v) ? v : null
}

/** Validate the optional measurement object; require a usable `display`. */
function parseMeasurement(v: WireLabel['measurement']): LabelMeasurement | null {
  if (!v || typeof v !== 'object') return null
  const display = typeof v.display === 'string' ? v.display : null
  if (!display) return null
  const value = typeof v.value === 'number' && Number.isFinite(v.value) ? v.value : 0
  const unit = typeof v.unit === 'string' ? v.unit : ''
  const kind = typeof v.kind === 'string' ? v.kind : ''
  return { value, unit, kind, display }
}

function parseConformance(v: unknown): LabelConformance | null {
  return typeof v === 'string' &&
    (KNOWN_CONFORMANCE as readonly string[]).includes(v)
    ? (v as LabelConformance)
    : null
}

/**
 * The callout text the viewport shows for a label:
 * `name — measurement.display` when a verified, units-bearing dimension
 * exists, otherwise just the name. Shared by the in-scene chip and the
 * hover tooltip so the two never drift.
 */
export function labelText(label: Label): string {
  return label.measurement
    ? `${label.name} — ${label.measurement.display}`
    : label.name
}

/** Validate + normalise one wire label into a {@link Label}. */
function parseLabel(raw: WireLabel): Label | null {
  if (typeof raw.name !== 'string' || raw.name.length === 0) return null
  if (typeof raw.kind !== 'string' || !isLabelKind(raw.kind)) return null
  const anchor = isVec3(raw.anchor) ? raw.anchor : null
  const description =
    typeof raw.description === 'string' ? raw.description : null
  const entityId =
    typeof raw.entity_id === 'number' && Number.isFinite(raw.entity_id)
      ? raw.entity_id
      : null
  const color = parseHexColor(raw.color)
  const measurement = parseMeasurement(raw.measurement)
  const conformance = parseConformance(raw.conformance)
  // Honour the explicit flag when shipped; otherwise — and always as a
  // defensive backstop — a missing world anchor means the entity no
  // longer resolves, which is itself a stale signal.
  const stale = raw.stale === true || anchor === null
  return {
    name: raw.name,
    kind: raw.kind,
    anchor,
    entityId,
    description,
    color,
    measurement,
    conformance,
    stale,
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
