import { useEffect, useMemo } from 'react'
import * as THREE from 'three'
import { useThree } from '@react-three/fiber'
import { Html } from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'
import { useHistoryView } from '@/stores/history-view-store'
import { useFeatureLocationStore } from '@/stores/feature-location-store'
import { useFeatureLocations } from '@/hooks/use-feature-locations'
import { facePositions } from '@/lib/face-positions'
import { FeatureEditPanel } from '@/components/panels/FeatureEditPanel'
import { TintMesh } from './face-tint'

const AMBER = '#F2A516'

/**
 * The feature selected in the tree, shown ON the part (Varun 2026-11-14:
 * "when features are selected, they should be highlighted in the part with
 * an arrow and details shown which can be edited"): its faces tinted amber,
 * and its sizes in a card beside it with an arrow to the feature. A feature
 * that could not be found on the part (see lib/feature-faces) is not marked;
 * its card stays in the tree's column instead.
 */
export function FeatureHighlight() {
  // Find every feature's faces once, for the callout and the tree alike.
  const located = useFeatureLocations('main')
  const setLocated = useFeatureLocationStore((s) => s.setLocated)
  useEffect(() => setLocated(located), [located, setLocated])

  const selected = useHistoryView((s) => s.selectedChapter)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)
  const camera = useThree((s) => s.camera)

  const hit = selected === null ? undefined : located.get(selected)
  const tint = useMemo(() => {
    if (!hit) return null
    const mesh = meshRefs.get(hit.objectId)
    const faceIds = objects.get(hit.objectId)?.mesh.faceIds
    if (!mesh?.geometry || !faceIds) return null
    return facePositions(mesh, faceIds, new Set(hit.faceIds))
  }, [hit, meshRefs, objects])

  // The arrow points at the feature's triangle nearest the camera when it
  // was selected: on the side you are looking at, not inside the part.
  const anchor = useMemo(() => {
    if (!tint) return null
    let best: [number, number, number] | null = null
    let bestD = Infinity
    const c = new THREE.Vector3()
    for (let i = 0; i + 8 < tint.length; i += 9) {
      const x = (tint[i] + tint[i + 3] + tint[i + 6]) / 3
      const y = (tint[i + 1] + tint[i + 4] + tint[i + 7]) / 3
      const z = (tint[i + 2] + tint[i + 5] + tint[i + 8]) / 3
      const d = camera.position.distanceToSquared(c.set(x, y, z))
      if (d < bestD) {
        bestD = d
        best = [x, y, z]
      }
    }
    return best
  }, [tint, camera])

  if (!tint) return null
  return (
    <group>
      <TintMesh positions={tint} color={AMBER} opacity={0.45} />
      {anchor && (
        // Under the app's panels (they sit at z 10-30), over the model.
        <Html position={anchor} zIndexRange={[9, 0]} style={{ pointerEvents: 'none' }}>
          <div className="relative h-0 w-0">
            {/* The arrow: from the card down to the feature, head on it. */}
            <svg className="pointer-events-none absolute -left-1 -top-[76px] overflow-visible" width="90" height="80" aria-hidden>
              <defs>
                <marker id="feature-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="8" markerHeight="8" orient="auto">
                  <path d="M0 0 L8 4 L0 8 z" fill={AMBER} />
                </marker>
              </defs>
              <line x1="70" y1="4" x2="5" y2="74" stroke={AMBER} strokeWidth="2" markerEnd="url(#feature-arrow)" />
            </svg>
            <div className="pointer-events-auto absolute bottom-[72px] left-[64px]">
              <FeatureEditPanel placement="callout" />
            </div>
          </div>
        </Html>
      )}
    </group>
  )
}
