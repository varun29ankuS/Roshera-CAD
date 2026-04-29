import { useMemo } from 'react'
import { EffectComposer, Outline } from '@react-three/postprocessing'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'
import { BlendFunction, KernelSize } from 'postprocessing'
import type { Mesh } from 'three'

export function SelectionOutline() {
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const hoveredId = useSceneStore((s) => s.hoveredId)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const theme = useThemeStore((s) => s.theme)

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

  // Selection / hover ink follows the blueprint accent token. Hidden
  // edges use a desaturated derivative so they read as "occluded" without
  // blending into the geometry.
  const palette = useMemo(() => {
    const accent = resolveCssVar('--cad-selected').color.getHex()
    const tick = resolveCssVar('--cad-tick').color.getHex()
    return {
      selectVisible: accent,
      selectHidden: tick,
      hoverVisible: accent,
      hoverHidden: tick,
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme])

  if (selectedMeshes.length === 0 && hoveredMeshes.length === 0) return null

  return (
    <EffectComposer multisampling={4} autoClear={false}>
      {selectedMeshes.length > 0 ? (
        <Outline
          selection={selectedMeshes}
          blendFunction={BlendFunction.ALPHA}
          edgeStrength={3}
          pulseSpeed={0}
          visibleEdgeColor={palette.selectVisible}
          hiddenEdgeColor={palette.selectHidden}
          kernelSize={KernelSize.SMALL}
          blur
          xRay={false}
        />
      ) : (
        <></>
      )}
      {hoveredMeshes.length > 0 ? (
        <Outline
          selection={hoveredMeshes}
          blendFunction={BlendFunction.ALPHA}
          edgeStrength={1.5}
          pulseSpeed={0}
          visibleEdgeColor={palette.hoverVisible}
          hiddenEdgeColor={palette.hoverHidden}
          kernelSize={KernelSize.VERY_SMALL}
          blur
          xRay={false}
        />
      ) : (
        <></>
      )}
    </EffectComposer>
  )
}
