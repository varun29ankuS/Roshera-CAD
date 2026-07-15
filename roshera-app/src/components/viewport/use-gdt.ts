import { useEffect, useState } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useUnitsStore } from '@/stores/units-store'

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

// ─── Wire types ──────────────────────────────────────────────────────

/** Resolution status for a datum at query time. */
export type DatumResolutionWire =
  | { status: 'live'; origin: [number, number, number]; direction: [number, number, number] }
  | { status: 'dangling' }

/** One datum in the GET /gdt response. */
export interface GdtDatumWire {
  label: string
  /** `"plane"` | `"axis"` | `"point"` */
  kind: string
  persistent_id: string
  resolution: DatumResolutionWire
}

/** One datum's resolution status embedded in a verdict. */
export interface DatumStatusWire {
  label: string
  resolution: DatumResolutionWire
}

/** Conformance verdict for one annotation. */
export interface VerdictWire {
  characteristic: string
  tolerance_mm: number
  /** Formatted in document units, e.g. `"0.030mm"` or `"0.001in"`. */
  tolerance_label: string
  measured_mm: number | null
  /** Formatted in document units when `measured_mm` is non-null. */
  measured_label: string | null
  /** `"in_spec"` | `"out_of_spec"` | `"not_evaluable"` */
  conforms: 'in_spec' | 'out_of_spec' | 'not_evaluable'
  reason: string | null
  fit_residual_mm: number | null
  datum_statuses: DatumStatusWire[]
}

/** One annotation entry in the GET /gdt response. */
export interface GdtAnnotationWire {
  feature_pid: string
  verdict: VerdictWire
  /**
   * Live-resolved kernel face id for the annotated feature.
   * `null` when the feature is dangling (PID no longer resolves to a face).
   * Used by the viewport to fan-out the hover tint via the per-triangle
   * `faceIds[t]` map — the same path as `DimensionFaceTints`.
   */
  target_face_id: number | null
  /**
   * A world-space point ON the toleranced feature in mm.
   * - Planar face: the analytic `Plane::origin`.
   * - Cylindrical face: `cyl.origin + cyl.axis * v_mid` (axial mid-height).
   * `null` when the feature is dangling or the surface kind has no anchor
   * convention in current scope.
   */
  anchor_mm: [number, number, number] | null
}

/** Full GET /api/agent/parts/{id}/gdt response shape. */
export interface GdtResponse {
  part_id: number
  datums: GdtDatumWire[]
  annotations: GdtAnnotationWire[]
  /** `"session"` — GD&T state does not survive a server restart. */
  persistence: string
}

/**
 * Fetch + cache the GD&T table for a single kernel solid, refreshing
 * whenever the scene geometry changes or the document unit changes.
 *
 * ## Refresh trigger
 * Mirrors `use-part-dimensions.ts` exactly: the geometry key is built
 * from `objectOrder` joined with each object's mesh-vertex length, plus
 * the `unitEpoch` suffix so a unit change re-fires the fetch and verdict
 * labels arrive in the new unit.
 *
 * ## `enabled` gate
 * The fetch is suppressed when `enabled` is false so toggled-off overlays
 * pay zero network cost.
 *
 * ## Failure policy
 * A failed fetch is surfaced to `console.warn` and leaves the GDT state
 * empty rather than stale. The error is not re-thrown; a transient 500
 * must never break the viewport.
 *
 * ## Session-persistence honesty
 * The backend's `"persistence": "session"` field is honoured implicitly:
 * if the GET returns empty arrays (server restart cleared all GD&T state),
 * this hook returns empty arrays and callers render nothing.
 */
export function useGdt(
  partId: number | null,
  enabled: boolean,
): { datums: GdtDatumWire[]; annotations: GdtAnnotationWire[]; loading: boolean } {
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const objects = useSceneStore((s) => s.objects)
  const unitEpoch = useUnitsStore((s) => s.unitEpoch)

  // Geometry fingerprint: same strategy as use-part-dimensions.ts.
  // Topology mutations flow through the WS pipeline and bump this key,
  // triggering a re-fetch of the kernel's current GD&T state.
  const geometryKey =
    objectOrder
      .map((id) => {
        const o = objects.get(id)
        return `${id}:${o ? o.mesh.vertices.length : 0}`
      })
      .join('|') + `|u${unitEpoch}`

  const [datums, setDatums] = useState<GdtDatumWire[]>([])
  const [annotations, setAnnotations] = useState<GdtAnnotationWire[]>([])
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!enabled || partId === null) {
      return
    }

    let cancelled = false
    // Immediate loading flag before the async fetch starts; matches use-part-labels.ts.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setLoading(true)

    fetch(`${API_BASE}/agent/parts/${partId}/gdt`, {
      method: 'GET',
      headers: { Accept: 'application/json' },
    })
      .then((resp) => {
        if (!resp.ok) {
          throw new Error(`gdt fetch ${resp.status} for solid ${partId}`)
        }
        return resp.json() as Promise<GdtResponse>
      })
      .then((data) => {
        if (cancelled) return
        setDatums(data.datums)
        setAnnotations(data.annotations)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        // Failure must never break the viewport; warn and show nothing.
        // No toast utility exists in this codebase (see ViewportContextMenu.tsx).
        console.warn(
          `[GdtAnnotations] gdt fetch failed for solid ${partId}:`,
          err instanceof Error ? err.message : String(err),
        )
        setDatums([])
        setAnnotations([])
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })

    return () => {
      cancelled = true
    }
  }, [enabled, partId, geometryKey])

  return { datums, annotations, loading }
}
