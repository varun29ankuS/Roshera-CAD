import { useEffect, useState } from 'react'
import { useSceneStore } from '@/stores/scene-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/**
 * Wire shape of a single dimension row from
 * `GET /api/agent/parts/{id}/dimensions`.
 *
 * Mirrors the backend `DimensionRecord` exactly, including the additive
 * `pid` field from spec section 1 (`None` arrives as `null`).
 */
export interface DimensionRow {
  /** Per-call id (d0, d1, …) — stable only within a single response. */
  id: string
  /** Semantic kind: `diameter`, `radius`, `length`, `angle`, `extent_x`, etc. */
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
   */
  direction: [number, number, number]
  /** Optional rotation axis for angle dimensions — `null` when absent. */
  axis: [number, number, number] | null
  /**
   * Durable persistent id (UUIDv5) — `null` for pre-PID solids and
   * whole-part extents that lack entity PIDs. The frontend displays the
   * annotation regardless; only pinning requires a non-null pid.
   */
  pid: string | null
}

/**
 * Fetch + cache the dimension table for a single kernel solid, refreshing
 * whenever the scene geometry changes.
 *
 * ## Refresh trigger
 * Mirrors `use-part-labels.ts` exactly: the geometry key is built from
 * `objectOrder` (captures add / remove) joined with each object's
 * mesh-vertex length (captures re-tessellation / shape change). Any
 * topology mutation flows through the WS pipeline and bumps this key,
 * triggering a re-fetch of the kernel's current dimension table.
 *
 * ## `enabled` gate
 * The fetch is suppressed when `enabled` is false so toggled-off overlays
 * pay zero network cost. This maps cleanly onto `showDimensions.has(id)`.
 *
 * ## Failure policy
 * A failed fetch surfaces to `console.warn` (no toast utility exists in
 * this codebase — `ViewportContextMenu.tsx` references "no toast" as a
 * deliberate choice) and leaves the dimension list empty rather than
 * stale, matching the spec's "never stale annotations" requirement.
 * The error is not re-thrown; a transient 500 must never break the
 * viewport.
 *
 * ## No parts/0 fallback
 * `partId` is the kernel `solid_id` (a positive integer) resolved from
 * `CADObject.analyticalGeometry.solidId`. When `partId` is `null` the
 * hook is a no-op (returns empty, no fetch). Callers must pass a real
 * id — never a sentinel.
 */
export function usePartDimensions(
  partId: number | null,
  enabled: boolean,
): { dimensions: DimensionRow[]; loading: boolean } {
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const objects = useSceneStore((s) => s.objects)

  // Geometry fingerprint: objectOrder joined with each solid's vertex
  // count. Stable string so React's dependency comparison works by value.
  // Topology mutations (create / extrude / boolean / transform) all flow
  // through the WS pipeline and change this fingerprint, triggering a
  // refetch of the kernel's current dimension table.
  const geometryKey = objectOrder
    .map((id) => {
      const o = objects.get(id)
      return `${id}:${o ? o.mesh.vertices.length : 0}`
    })
    .join('|')

  const [dimensions, setDimensions] = useState<DimensionRow[]>([])
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!enabled || partId === null) {
      return
    }

    let cancelled = false
    setLoading(true)

    fetch(`${API_BASE}/agent/parts/${partId}/dimensions`, {
      method: 'GET',
      headers: { Accept: 'application/json' },
    })
      .then((resp) => {
        if (!resp.ok) {
          throw new Error(`dimensions fetch ${resp.status} for solid ${partId}`)
        }
        return resp.json() as Promise<{ dimensions: DimensionRow[] }>
      })
      .then((data) => {
        if (cancelled) return
        setDimensions(data.dimensions)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        // Failure must never break the viewport; warn and show nothing
        // rather than stale annotations. No toast utility exists in this
        // codebase (see ViewportContextMenu.tsx comment), so console.warn
        // is the correct surface for a transient fetch failure.
        console.warn(
          `[PartDimensions] dimension fetch failed for solid ${partId}:`,
          err instanceof Error ? err.message : String(err),
        )
        setDimensions([])
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [enabled, partId, geometryKey])

  return { dimensions, loading }
}
