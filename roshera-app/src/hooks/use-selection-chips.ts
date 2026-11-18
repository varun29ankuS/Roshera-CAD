import { useCallback, useEffect, useMemo, useState } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useUserContextStore } from '@/stores/user-context-store'
import {
  MAX_ATTACHED,
  attachmentOf,
  composerChips,
  entityKey,
  resolutionsFromContext,
  sizeFromPartReport,
  type ComposerChip,
  type Resolution,
  type SelectionAttachment,
} from '@/lib/selection-attachment'

const API_HOST: string = import.meta.env.VITE_API_URL || ''

/** Quiet time after the selection changes before the kernel is asked. */
const RESOLVE_SETTLE_MS = 250

/**
 * What the kernel says about the selected entities: their persistent ids
 * and measures (`GET /api/agent/user-context`, which resolves the last
 * acknowledged report against the live model), and each selected part's
 * size (`GET /api/agent/parts/{solid}`). Sent WITHOUT `X-Roshera-Agent`:
 * the app reading its own context is not an agent looking. A read that
 * fails leaves the chips with what the app itself knows (part, kind, id).
 */
async function resolve(): Promise<Map<string, Resolution>> {
  const out = new Map<string, Resolution>()
  try {
    const r = await fetch(`${API_HOST}/api/agent/user-context`)
    if (r.ok) for (const [k, v] of resolutionsFromContext(await r.json())) out.set(k, v)
  } catch {
    // Unreachable server: the chips carry the app's own facts only.
  }
  return out
}

async function partSize(solid: number): Promise<Resolution['size_mm']> {
  try {
    const r = await fetch(`${API_HOST}/api/agent/parts/${solid}`)
    return r.ok ? sizeFromPartReport(await r.json()) : undefined
  } catch {
    return undefined
  }
}

/** A number per mesh object, so "the selected parts' meshes" is one
 *  comparable value. */
const meshNumbers = new WeakMap<object, number>()
let nextMeshNumber = 1
function meshNumber(mesh: object | undefined): number {
  if (mesh === undefined) return 0
  let n = meshNumbers.get(mesh)
  if (n === undefined) {
    n = nextMeshNumber++
    meshNumbers.set(mesh, n)
  }
  return n
}

export interface SelectionChips {
  /** Chips shown on the composer (removed ones included, marked). */
  chips: readonly ComposerChip[]
  /** Selected entities beyond the chips. */
  more: number
  removed: ReadonlySet<string>
  remove: (key: string) => void
  /** What the next message carries: exactly what the row shows. */
  attachment: SelectionAttachment
  /** Forget this ask's removals (after a send). */
  reset: () => void
  /** Sharing is paused while something is selected: nothing is attached. */
  pausedWithSelection: boolean
}

/**
 * The composer's selection chips (lib/selection-attachment.ts): the last
 * selection the server acknowledged, named from the scene, completed by
 * the kernel's ids and measures. Removing a chip drops it from this ask
 * only, never from the viewport selection; the removals reset when the
 * selection changes and after each send. `open`: the composer is on
 * screen.
 */
export function useSelectionChips(open: boolean): SelectionChips {
  const sharing = useUserContextStore((s) => s.sharing)
  const acknowledged = useUserContextStore((s) => s.acknowledged)
  const objects = useSceneStore((s) => s.objects)
  const hasSelection = useSceneStore((s) => s.selectedIds.size > 0 || s.subElementSelections.length > 0)
  const [resolved, setResolved] = useState<ReadonlyMap<string, Resolution>>(() => new Map())
  const [removed, setRemoved] = useState<ReadonlySet<string>>(() => new Set())

  const selection = useMemo(
    () => (sharing && acknowledged?.sharing ? (acknowledged.selection ?? []) : []),
    [sharing, acknowledged],
  )
  // What is selected, as one value: a new selection, not a new report
  // object, is what resets removals and asks the kernel again.
  const signature = useMemo(() => selection.map(entityKey).join('\n'), [selection])

  useEffect(() => {
    setRemoved(new Set())
  }, [signature])

  // The selected parts' meshes, as one value: an edit hands a part a new
  // mesh, and its faces' areas and its size may then differ.
  const meshKey = useMemo(() => {
    const seen = new Set<string>()
    const out: number[] = []
    for (const e of selection) {
      if (seen.has(e.object_id)) continue
      seen.add(e.object_id)
      out.push(meshNumber(objects.get(e.object_id)?.mesh))
    }
    return out.join(',')
  }, [selection, objects])

  useEffect(() => {
    // The kernel is asked only while the composer is open: the read takes
    // the model's lock, and a closed board shows no chips.
    if (signature === '' || !open) {
      setResolved(new Map())
      return
    }
    let live = true
    const timer = window.setTimeout(() => {
      void (async () => {
        const map = await resolve()
        if (!live) return
        // Sizes only for the part chips that can be shown, read together.
        const scene = useSceneStore.getState().objects
        await Promise.all(
          selection.slice(0, MAX_ATTACHED).map(async (e) => {
            if (e.kind !== 'part') return
            const key = entityKey(e)
            const solid = scene.get(e.object_id)?.analyticalGeometry?.solidId ?? map.get(key)?.solid_id
            if (solid === undefined) return
            const size = await partSize(solid)
            if (size !== undefined) map.set(key, { ...map.get(key), size_mm: size })
          }),
        )
        if (live) setResolved(map)
      })()
    }, RESOLVE_SETTLE_MS)
    return () => {
      live = false
      window.clearTimeout(timer)
    }
    // `selection` is read through `signature`; `meshKey` re-asks after an edit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature, meshKey, open])

  const { chips, more } = useMemo(
    () =>
      composerChips({
        sharing,
        report: acknowledged,
        nameOf: (id) => objects.get(id)?.name ?? null,
        solidIdOf: (id) => objects.get(id)?.analyticalGeometry?.solidId,
        resolved,
      }),
    [sharing, acknowledged, objects, resolved],
  )
  const attachment = useMemo(() => attachmentOf(chips, more, removed), [chips, more, removed])
  const remove = useCallback((key: string) => setRemoved((r) => new Set(r).add(key)), [])
  const reset = useCallback(() => setRemoved(new Set()), [])

  return { chips, more, removed, remove, attachment, reset, pausedWithSelection: !sharing && hasSelection }
}
