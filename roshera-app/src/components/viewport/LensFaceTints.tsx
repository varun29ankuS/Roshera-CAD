import { useMemo } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useLensStore } from '@/stores/lens-store'
import { lensFaceTints } from '@/lib/lens'
import { TintMesh } from './face-tint'
import { facePositions } from '@/lib/face-positions'

// The DFM tints' colours: red where a check is broken, amber where the
// kernel changed something. No new colours.
const BLOCKING = '#D23B3B'
const REPAIRED = '#E3A51A'

/**
 * Kernel Lens on the canvas: while Agent Eye is in Lens mode, the part's
 * faces the kernel repaired are amber and the faces that block its
 * certificate are red. The face whose story is open is drawn strongest.
 * Faces are matched by kernel face id through the mesh's `faceIds`, as the
 * DFM tints do.
 */
export function LensFaceTints() {
  const objectId = useLensStore((s) => s.objectId)
  const part = useLensStore((s) => s.part)
  const openFace = useLensStore((s) => s.face?.face_id ?? null)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)

  const layers = useMemo(() => {
    if (!part || !objectId) return []
    const mesh = meshRefs.get(objectId)
    const faceIds = objects.get(objectId)?.mesh.faceIds
    if (!faceIds || !mesh?.geometry) return []
    const tints = lensFaceTints(part)
    const soft = (faces: number[]) => new Set(faces.filter((f) => f !== openFace))
    const groups: { color: string; opacity: number; faces: Set<number> }[] = [
      { color: REPAIRED, opacity: 0.45, faces: soft(tints.repaired) },
      { color: BLOCKING, opacity: 0.55, faces: soft(tints.blocking) },
    ]
    if (openFace !== null) {
      const color = tints.blocking.includes(openFace)
        ? BLOCKING
        : tints.repaired.includes(openFace)
          ? REPAIRED
          : null
      if (color) groups.push({ color, opacity: 0.9, faces: new Set([openFace]) })
    }
    return groups
      .map((g) => ({ ...g, positions: facePositions(mesh, faceIds, g.faces) }))
      .filter((g) => g.positions !== null) as { color: string; opacity: number; positions: Float32Array }[]
  }, [part, objectId, openFace, meshRefs, objects])

  return (
    <group>
      {layers.map((l, i) => (
        <TintMesh key={`${i}-${l.color}-${l.opacity}`} positions={l.positions} color={l.color} opacity={l.opacity} />
      ))}
    </group>
  )
}
