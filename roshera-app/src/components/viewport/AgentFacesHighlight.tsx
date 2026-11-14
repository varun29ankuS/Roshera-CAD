import { useMemo } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useAgentShowStore } from '@/stores/agent-show-store'
import { facePositions } from '@/lib/face-positions'
import { TintMesh } from './face-tint'

/** The faces the agent is showing the user, tinted amber on the part. */
export function AgentFacesHighlight() {
  const faces = useAgentShowStore((s) => s.faces)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)
  const positions = useMemo(() => {
    if (!faces) return null
    const mesh = meshRefs.get(faces.objectId)
    const ids = objects.get(faces.objectId)?.mesh.faceIds
    if (!mesh?.geometry || !ids) return null
    return facePositions(mesh, ids, new Set(faces.faceIds))
  }, [faces, meshRefs, objects])
  if (!positions) return null
  return <TintMesh positions={positions} color="#F2A516" opacity={0.55} />
}
