import { useSceneStore } from '@/stores/scene-store'
import { CADMesh } from './CADMesh'

export function SceneObjects() {
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const objects = useSceneStore((s) => s.objects)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const hoveredId = useSceneStore((s) => s.hoveredId)

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
