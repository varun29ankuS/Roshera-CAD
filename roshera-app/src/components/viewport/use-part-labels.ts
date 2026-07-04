import { useEffect, useState } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useUnitsStore } from '@/stores/units-store'
import { listLabels, type Label } from '@/lib/labels-api'

/**
 * Fetch + cache the active part's labels, refreshing whenever the scene
 * geometry changes.
 *
 * ## Refresh trigger
 * Labels are part-scoped and the api-server resolves the part from the
 * active model (the viewport's single live document — see `labels-api`),
 * so a single fetch covers every visible solid. Geometry mutations
 * (create / extrude / boolean / transform / …) all flow through the
 * scene store's `objects` map and bump `objectOrder` / replace meshes.
 * We key the refetch off a cheap fingerprint of `objectOrder` plus each
 * object's mesh-vertex length: any topology change (which is what can
 * make a label go stale, or add a new one) changes that fingerprint and
 * re-pulls the kernel's current answer. This reuses the existing
 * WS-driven object pipeline rather than adding a second poll loop.
 *
 * The fetch is gated on `enabled` so neither consumer pays for it while
 * the overlay is hidden AND nothing is being hovered.
 */
export function usePartLabels(enabled: boolean): {
  labels: Label[]
  loading: boolean
  error: string | null
} {
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const objects = useSceneStore((s) => s.objects)
  const unitEpoch = useUnitsStore((s) => s.unitEpoch)

  // Fingerprint the geometry so the effect re-runs on any topology
  // change without depending on the (frequently re-created) Map identity
  // alone. Mesh vertex length is a cheap proxy for "this solid changed
  // shape"; combined with the object id list it captures add / remove /
  // re-tessellate. Stable string so React's dep compare is by value.
  // The `unitEpoch` suffix ensures a unit change also triggers a re-fetch
  // so label strings arrive in the new unit without a topology mutation.
  const geometryKey =
    objectOrder
      .map((id) => {
        const o = objects.get(id)
        return `${id}:${o ? o.mesh.vertices.length : 0}`
      })
      .join('|') + `|u${unitEpoch}`

  const [labels, setLabels] = useState<Label[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!enabled) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setError(null)
      return
    }
    let cancelled = false
    setLoading(true)
    listLabels()
      .then((next) => {
        if (cancelled) return
        setLabels(next)
        setError(null)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        // A labels fetch failure must never break the viewport; surface
        // it to the consumer (which can choose to ignore) and keep the
        // last good cache so a transient blip doesn't flicker callouts.
        setError(err instanceof Error ? err.message : String(err))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [enabled, geometryKey])

  return { labels, loading, error }
}
