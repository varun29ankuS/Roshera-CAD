import { useMemo } from 'react'
import { EffectComposer, Outline } from '@react-three/postprocessing'
import { useSceneStore } from '@/stores/scene-store'
import { BlendFunction, KernelSize } from 'postprocessing'
import type { Mesh } from 'three'

export function SelectionOutline() {
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const hoveredId = useSceneStore((s) => s.hoveredId)
  const meshRefs = useSceneStore((s) => s.meshRefs)

  const selectedMeshes = useMemo(() => {
    const meshes: Mesh[] = []
    for (const id of selectedIds) {
      const mesh = meshRefs.get(id)
      if (mesh) meshes.push(mesh)
    }
    return meshes
  }, [selectedIds, meshRefs])

  const hoveredMeshes = useMemo(() => {
    if (!hoveredId || selectedIds.has(hoveredId)) return []
    const mesh = meshRefs.get(hoveredId)
    return mesh ? [mesh] : []
  }, [hoveredId, selectedIds, meshRefs])

  if (selectedMeshes.length === 0 && hoveredMeshes.length === 0) return null

  return (
    <EffectComposer multisampling={4} autoClear={false}>
      {selectedMeshes.length > 0 && (
        <Outline
          selection={selectedMeshes}
          blendFunction={BlendFunction.ALPHA}
          edgeStrength={3}
          pulseSpeed={0}
          visibleEdgeColor={0x5b9cf5}
          hiddenEdgeColor={0x2a4a80}
          kernelSize={KernelSize.SMALL}
          blur
          xRay={false}
        />
      )}
      {hoveredMeshes.length > 0 && (
        <Outline
          selection={hoveredMeshes}
          blendFunction={BlendFunction.ALPHA}
          edgeStrength={1.5}
          pulseSpeed={0}
          visibleEdgeColor={0x7ab5ff}
          hiddenEdgeColor={0x3a5a90}
          kernelSize={KernelSize.VERY_SMALL}
          blur
          xRay={false}
        />
      )}
    </EffectComposer>
  )
}
