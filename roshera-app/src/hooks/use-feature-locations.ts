import { useEffect, useState } from 'react'
import { useChapters } from '@/hooks/use-chapters'
import { useSceneStore } from '@/stores/scene-store'
import { fetchEditableParameters } from '@/lib/feature-edit'
import { locateFeatures, type PartFace } from '@/lib/feature-faces'

const API_HOST: string = import.meta.env.VITE_API_URL ?? ''

/** Where each placeable feature is on the part: its scene object and the
 *  kernel faces it made (see lib/feature-faces). */
export interface FeatureLocation {
  objectId: string
  faceIds: number[]
}

async function partFaces(solidId: number): Promise<PartFace[]> {
  const r = await fetch(`${API_HOST}/api/agent/parts/${solidId}/features`)
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  const body = (await r.json()) as { features?: PartFace[] }
  return body.features ?? []
}

/**
 * The feature locations for the history as it is now: refetched when the
 * history or the parts change. A part whose faces cannot be read places
 * nothing (the feature then shows its sizes in the tree's column, without
 * an arrow) -- never a guessed spot.
 */
export function useFeatureLocations(branch = 'main'): Map<number, FeatureLocation> {
  const { chapters } = useChapters(branch)
  const objects = useSceneStore((s) => s.objects)
  const [located, setLocated] = useState<Map<number, FeatureLocation>>(new Map())

  useEffect(() => {
    let live = true
    const parts = [...objects.values()]
      .map((o) => ({ objectId: o.id, solidId: o.analyticalGeometry?.solidId }))
      .filter((p): p is { objectId: string; solidId: number } => typeof p.solidId === 'number')
    Promise.all([
      fetchEditableParameters(branch),
      Promise.all(parts.map(async (p) => ({ ...p, faces: await partFaces(p.solidId).catch(() => []) }))),
    ])
      .then(([params, perPart]) => {
        if (!live) return
        const ownerOf = new Map<number, string>()
        for (const p of perPart) for (const f of p.faces) ownerOf.set(f.face_id, p.objectId)
        const faces = perPart.flatMap((p) => p.faces)
        const out = new Map<number, FeatureLocation>()
        for (const [anchor, ids] of locateFeatures(chapters, params, faces)) {
          const objectId = ownerOf.get(ids[0])
          if (!objectId) continue
          out.set(anchor, { objectId, faceIds: ids.filter((id) => ownerOf.get(id) === objectId) })
        }
        setLocated(out)
      })
      .catch(() => live && setLocated(new Map()))
    return () => {
      live = false
    }
  }, [branch, chapters, objects])

  return located
}
