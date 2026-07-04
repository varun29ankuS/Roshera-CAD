/**
 * Interactive-measure client for `POST /api/agent/measure`, plus the
 * shared `DimensionRow` wire type.
 *
 * `DimensionRow` lives here (lib level) rather than in
 * `use-part-dimensions.ts` because it is consumed by three layers:
 * the dimensions fetch hook, the `PartDimensions` renderer, and the
 * scene store's `PinnedMeasurement` — and the store must not import
 * from a component file.
 */

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/**
 * Wire shape of a single dimension record — both the rows of
 * `GET /api/agent/parts/{id}/dimensions` and the response of
 * `POST /api/agent/measure` (the measure endpoint is deliberately
 * DimensionRecord-shaped so the frontend renders measurements with the
 * same component as ambient dimensions; see spec section 2).
 */
export interface DimensionRow {
  /** Per-call id (d0, d1, …) — stable only within a single response. */
  id: string
  /** Semantic kind: `diameter`, `radius`, `length`, `angle`, `extent`, etc. */
  kind: string
  /** Numeric magnitude in `unit`. All computation is kernel-side. */
  value: number
  /** Unit string, e.g. `"mm"` or `"deg"`. */
  unit: string
  /** Human-readable label already formatted by the kernel, e.g. `"Ø 10 mm"`. */
  label: string
  /** Kernel entity ids this dimension references. Empty for whole-part extents. */
  entities: number[]
  /**
   * World-space anchor point `[x, y, z]` where the leader line starts
   * (and where extent-kind labels sit, sans leader).
   */
  anchor: [number, number, number]
  /**
   * Unit direction vector `[x, y, z]` pointing outward from the feature.
   * Used to derive the leader perpendicular for non-extent rows.
   * `null` for angle and face-info results, which have no single direction.
   */
  direction: [number, number, number] | null
  /** Optional rotation axis for angle dimensions — `null` when absent. */
  axis?: [number, number, number] | null
  /**
   * Durable persistent id (UUIDv5) — `null` for pre-PID solids,
   * whole-part extents lacking entity PIDs, and interactive measure
   * results (which are session-local by design).
   */
  pid: string | null
}

/** One leg of a measure request — a kernel face on a kernel solid. */
export interface MeasureFaceRef {
  part_id: number
  kind: 'face'
  id: number
}

/**
 * Typed refusal from the measure endpoint. The kernel REFUSES (422,
 * typed reason) when faces don't admit the requested relation — never
 * a guessed number. 404 covers a face/solid that no longer resolves.
 * `reason` is the backend's text VERBATIM; consumers must surface it
 * without paraphrase.
 */
export class MeasureRefusalError extends Error {
  readonly status: number
  readonly reason: string

  constructor(status: number, reason: string) {
    super(reason)
    this.name = 'MeasureRefusalError'
    this.status = status
    this.reason = reason
  }
}

/**
 * POST `/api/agent/measure` between two faces (or one face when `b` is
 * `null` — a single cylindrical face measures its diameter, a single
 * planar face reports area + normal).
 *
 * - 2xx → the kernel's DimensionRecord-shaped result. The spec's field
 *   list omits the per-call `id`, so a missing `id` is normalised to a
 *   fresh UUID — display and React keying need one, and measure results
 *   are session-local anyway.
 * - 4xx → throws {@link MeasureRefusalError} carrying the backend's
 *   `reason` verbatim (the JSON `reason` field when present, else the
 *   raw body text).
 * - 5xx / network → throws a plain `Error` (transient; callers treat it
 *   as best-effort, not as a refusal).
 */
export async function measureFaces(
  a: MeasureFaceRef,
  b: MeasureFaceRef | null,
): Promise<DimensionRow> {
  const resp = await fetch(`${API_BASE}/agent/measure`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify({ a, b }),
  })

  if (!resp.ok) {
    const text = await resp.text().catch(() => '')
    let reason = text
    try {
      const parsed: unknown = JSON.parse(text)
      if (
        typeof parsed === 'object' &&
        parsed !== null &&
        'reason' in parsed &&
        typeof (parsed as { reason: unknown }).reason === 'string'
      ) {
        reason = (parsed as { reason: string }).reason
      }
    } catch {
      // Body was not JSON — keep the raw text verbatim.
    }
    if (resp.status >= 400 && resp.status < 500) {
      throw new MeasureRefusalError(resp.status, reason || `${resp.status}`)
    }
    throw new Error(reason || `measure failed: ${resp.status}`)
  }

  const data = (await resp.json()) as Omit<DimensionRow, 'id'> & {
    id?: string
  }
  return { ...data, id: data.id ?? crypto.randomUUID() }
}
