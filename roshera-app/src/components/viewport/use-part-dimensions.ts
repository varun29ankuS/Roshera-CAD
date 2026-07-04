import { useEffect, useState } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useUnitsStore } from '@/stores/units-store'
import type { DimensionRow } from '@/lib/measure-api'

// Re-exported so viewport consumers can keep importing the row type
// from the hook that produces it. The definition lives in
// `lib/measure-api.ts` because the scene store's `PinnedMeasurement`
// needs it too and the store must not import from a component file.
export type { DimensionRow }

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

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
  const unitEpoch = useUnitsStore((s) => s.unitEpoch)

  // Geometry fingerprint: objectOrder joined with each solid's vertex
  // count. Stable string so React's dependency comparison works by value.
  // Topology mutations (create / extrude / boolean / transform) all flow
  // through the WS pipeline and change this fingerprint, triggering a
  // refetch of the kernel's current dimension table.
  // The `unitEpoch` suffix ensures a unit change (which does not alter
  // topology) also re-fires the fetch so labels arrive in the new unit.
  const geometryKey =
    objectOrder
      .map((id) => {
        const o = objects.get(id)
        return `${id}:${o ? o.mesh.vertices.length : 0}`
      })
      .join('|') + `|u${unitEpoch}`

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
