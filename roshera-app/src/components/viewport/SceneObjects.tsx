import { useSceneStore } from '@/stores/scene-store'
import { useMemo } from 'react'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useAssemblyDocStore } from '@/stores/assembly-doc-store'
import { INSTANCE_PREFIX, poseOf } from '@/lib/assembly-pose'
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
  // In Assembly, with an assembly open: draw its instances -- each part's
  // own mesh at the instance's solved pose -- in place of the loose parts.
  const mode = useDocModeStore((s) => s.mode)
  const assembly = useAssemblyDocStore((s) => s.summary)
  const instances = useMemo(() => {
    if (mode !== 'assembly' || !assembly) return null
    const byUuid = new Map([...objects.values()].map((o) => [o.id, o]))
    return assembly.instances.flatMap((inst) => {
      const part = byUuid.get(inst.part_id)
      if (!part) return []
      return [{ ...part, id: `${INSTANCE_PREFIX}${inst.id}`, name: inst.name ?? part.name, ...poseOf(inst.transform) }]
    })
  }, [mode, assembly, objects])

  if (pastStep !== null) {
    return (
      <group name="scene-objects-past">
        {pastObjects.map((obj) => (
          <CADMesh key={obj.id} object={obj} isSelected={false} isHovered={false} />
        ))}
      </group>
    )
  }

  if (instances) {
    return (
      <group name="assembly-instances">
        {instances.map((obj) => (
          <CADMesh key={obj.id} object={obj} isSelected={selectedIds.has(obj.id)} isHovered={hoveredId === obj.id} />
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
