import { useSceneStore } from '@/stores/scene-store'
import { CADMesh } from './CADMesh'
import { useHistoryView } from '@/stores/history-view-store'

export function SceneObjects() {
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const objects = useSceneStore((s) => s.objects)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const hoveredId = useSceneStore((s) => s.hoveredId)
  // Viewing a past step: draw that state, read-only, in place of the live
  // part (the live model is untouched; Roll to end brings it back).
  const pastStep = useHistoryView((s) => s.step)
  const pastObjects = useHistoryView((s) => s.objects)

  if (pastStep !== null) {
    return (
      <group name="scene-objects-past">
        {pastObjects.map((obj) => (
          <CADMesh key={obj.id} object={obj} isSelected={false} isHovered={false} />
        ))}
      </group>
    )
  }

  return (
    <group name="scene-objects">
      {objectOrder.map((id) => {
        const obj = objects.get(id)
        if (!obj || !obj.visible) return null

        return (
          <CADMesh
            key={id}
            object={obj}
            isSelected={selectedIds.has(id)}
            isHovered={hoveredId === id}
          />
        )
      })}
    </group>
  )
}
