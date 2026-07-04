import { useEffect, useRef } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useChatStore } from '@/stores/chat-store'
import { useUnitsStore } from '@/stores/units-store'
import { measureFaces, MeasureRefusalError } from '@/lib/measure-api'

/**
 * Re-validate every pinned measurement whenever the scene geometry
 * changes — a stale number is worse than none (spec section 3).
 *
 * ## Why a hook, and why here
 * This lives in its own hook (mounted by `PartDimensions`, the component
 * that renders the pins) rather than inside the store or a top-level
 * effect because: (a) the trigger is the same geometry fingerprint the
 * dimension fetch hooks already key on — a React effect over that string
 * is the established pattern (`use-part-labels.ts`,
 * `use-part-dimensions.ts`); (b) mounting it with the renderer
 * guarantees re-validation runs exactly when pins can be visible, and
 * exactly once (single mount point inside the Canvas).
 *
 * ## Behaviour
 * On a geometry-key CHANGE (not on first mount — the initial pin was
 * just measured against current geometry), each pin is re-POSTed to
 * `/api/agent/measure` sequentially, best-effort:
 *   - 2xx → `updatePinnedMeasurement` swaps the row in place (fresh
 *     value + anchor, same pin identity).
 *   - 4xx (kernel refusal / face no longer resolves) → the pin is
 *     dismissed and a chat note reports "measurement removed: <reason>"
 *     with the backend's reason verbatim.
 *   - 5xx / network errors → the pin is KEPT and the failure logged;
 *     transient backend trouble must not eat the user's pins.
 * Sequential (not parallel) so a burst of pins cannot stampede the
 * kernel mid-edit; best-effort because re-validation is advisory, not
 * transactional.
 *
 * Pins referencing objects that were REMOVED never reach this hook —
 * `removeObject` / `clearScene` drop them synchronously in the store.
 */
export function usePinnedMeasurementsRevalidation(): void {
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const objects = useSceneStore((s) => s.objects)
  const unitEpoch = useUnitsStore((s) => s.unitEpoch)

  // Same geometry fingerprint as use-part-dimensions / use-part-labels:
  // object id list + per-object vertex count. Any topology change
  // (create / boolean / transform / re-tessellate) changes this string.
  // The `unitEpoch` suffix makes a unit change also trigger re-POSTing
  // every pin so the returned labels are in the new unit.
  const geometryKey =
    objectOrder
      .map((id) => {
        const o = objects.get(id)
        return `${id}:${o ? o.mesh.vertices.length : 0}`
      })
      .join('|') + `|u${unitEpoch}`

  // Previous key, `null` until the first effect run. Re-validation only
  // fires on an actual change — the first observed key is the baseline
  // the existing pins were measured against.
  const prevKeyRef = useRef<string | null>(null)

  useEffect(() => {
    const prev = prevKeyRef.current
    prevKeyRef.current = geometryKey
    if (prev === null || prev === geometryKey) return

    const pins = useSceneStore.getState().pinnedMeasurements
    if (pins.length === 0) return

    let cancelled = false
    void (async () => {
      for (const pin of pins) {
        if (cancelled) return
        const state = useSceneStore.getState()
        // The pin may have been dismissed (by the user, or by a
        // removeObject) while earlier pins in this pass were validating.
        if (!state.pinnedMeasurements.some((p) => p.id === pin.id)) continue

        const solidA = state.objects.get(pin.a.objectId)?.analyticalGeometry
          ?.solidId
        const solidB = pin.b
          ? state.objects.get(pin.b.objectId)?.analyticalGeometry?.solidId
          : null
        if (solidA === undefined || solidB === undefined) {
          // The object survives but no longer resolves to a kernel solid
          // (e.g. replaced by a frame without analytical_geometry).
          state.dismissMeasurement(pin.id)
          useChatStore.getState().addMessage({
            role: 'assistant',
            content:
              'measurement removed: a measured object no longer resolves to a kernel solid',
          })
          continue
        }

        try {
          const row = await measureFaces(
            { part_id: solidA, kind: 'face', id: pin.a.faceId },
            pin.b && solidB !== null
              ? { part_id: solidB, kind: 'face', id: pin.b.faceId }
              : null,
          )
          if (cancelled) return
          useSceneStore.getState().updatePinnedMeasurement(pin.id, row)
        } catch (err) {
          if (cancelled) return
          if (err instanceof MeasureRefusalError) {
            useSceneStore.getState().dismissMeasurement(pin.id)
            useChatStore.getState().addMessage({
              role: 'assistant',
              // Backend reason verbatim — no paraphrase.
              content: `measurement removed: ${err.reason}`,
            })
          } else {
            // Transient (5xx / network): keep the pin, log, move on.
            console.warn(
              '[PinnedMeasurements] re-validation failed (pin kept):',
              err instanceof Error ? err.message : String(err),
            )
          }
        }
      }
    })()

    return () => {
      cancelled = true
    }
  }, [geometryKey])
}
