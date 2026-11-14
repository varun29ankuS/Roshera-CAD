import { useMemo } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useDfmStore } from '@/stores/dfm-store'
import { dfmFaceTints } from '@/lib/dfm'
import { TintMesh } from './face-tint'
import { facePositions } from '@/lib/face-positions'

const FIX = '#D23B3B'
const UNKNOWN = '#E3A51A'

/**
 * Colours the faces a DFM report names, in the DFM workspace only: red
 * where a rule is proven broken, amber where a check could not run. The
 * finding in focus is drawn at full strength, the rest softer. Faces are
 * matched by kernel face id through the mesh's per-triangle `faceIds`,
 * the same map the GD&T tints use.
 */
export function DfmFaceTints() {
  const mode = useDocModeStore((s) => s.mode)
  const objectId = useDfmStore((s) => s.objectId)
  const report = useDfmStore((s) => s.report)
  const focusRule = useDfmStore((s) => s.focusRule)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)

  const layers = useMemo(() => {
    if (mode !== 'dfm' || !report || !objectId) return []
    const obj = objects.get(objectId)
    const mesh = meshRefs.get(objectId)
    const faceIds = obj?.mesh.faceIds
    if (!faceIds || !mesh?.geometry) return []
    const tints = dfmFaceTints(report)
    const focused = new Set(
      report.verdicts
        .filter((rv) => rv.rule === focusRule)
        .flatMap((rv) =>
          rv.verdict.kind === 'violation'
            ? rv.verdict.witnesses
            : rv.verdict.kind === 'unverifiable'
              ? rv.verdict.regions
              : [],
        ),
    )
    const groups: { color: string; opacity: number; faces: Set<number> }[] = [
      { color: FIX, opacity: 0.55, faces: new Set(tints.fix.filter((f) => !focused.has(f))) },
      { color: UNKNOWN, opacity: 0.4, faces: new Set(tints.unknown.filter((f) => !focused.has(f))) },
      {
        color: tints.fix.some((f) => focused.has(f)) ? FIX : UNKNOWN,
        opacity: 0.9,
        faces: focused,
      },
    ]
    return groups
      .map((g) => ({ ...g, positions: facePositions(mesh, faceIds, g.faces) }))
      .filter((g) => g.positions !== null) as { color: string; opacity: number; positions: Float32Array }[]
  }, [mode, report, objectId, focusRule, meshRefs, objects])

  return (
    <group>
      {layers.map((l, i) => (
        <TintMesh key={`${i}-${l.color}-${l.opacity}`} positions={l.positions} color={l.color} opacity={l.opacity} />
      ))}
    </group>
  )
}
