import { useEffect, useMemo } from 'react'
import { useThree } from '@react-three/fiber'
import { EffectComposer, Outline } from '@react-three/postprocessing'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'
import { BlendFunction, KernelSize } from 'postprocessing'
import type { Mesh } from 'three'

export function SelectionOutline() {
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const hoveredId = useSceneStore((s) => s.hoveredId)
  const treeHoverIds = useSceneStore((s) => s.treeHoverIds)
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

  // The hovered part, plus every part of a browser GROUP row under the
  // pointer (a sub-assembly, a `×N` group) — selected parts keep their
  // selection outline instead.
  const hoveredMeshes = useMemo(() => {
    const meshes: Mesh[] = []
    const ids = hoveredId ? [hoveredId, ...treeHoverIds] : treeHoverIds
    for (const id of ids) {
      if (selectedIds.has(id)) continue
      const mesh = meshRefs.get(id)
      if (mesh) meshes.push(mesh)
    }
    return meshes
  }, [hoveredId, treeHoverIds, selectedIds, meshRefs])

  // The canvas renders on demand. The outline passes take their selection
  // through postprocessing's own effect objects, not through R3F props, so a
  // selection change requests no frame by itself: a clicked part showed no
  // outline until something else (a hover) happened to render. Ask for the
  // frame that shows it.
  const invalidate = useThree((s) => s.invalidate)
  useEffect(() => {
    invalidate()
  }, [selectedMeshes, hoveredMeshes, invalidate])

  // Selected silhouettes render in yellow (`--cad-edge-selected`)
  // so a clicked solid is unambiguously distinguished from a hovered or
  // unselected one. Hover stays on neutral ink for low-key feedback.
  // Hidden edges stay on the tick token so they read as "occluded"
  // without blending into the geometry.
  const palette = useMemo(() => {
    const tick = resolveCssVar('--cad-tick').color.getHex()
    const selected = resolveCssVar('--cad-edge-selected').color.getHex()
    const ink = 0x000000
    return {
      selectVisible: selected,
      selectHidden: tick,
      hoverVisible: ink,
      hoverHidden: tick,
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme])

  // EffectComposer must stay mounted across selection-state changes.
  // @react-three/postprocessing replaces the renderer's render function
  // when EffectComposer mounts; when it unmounts (e.g. selection cleared
  // by clicking a non-mesh tree node like a sketch) the original render
  // function is not always restored, leaving the canvas rendering
  // through a stale composer pipeline — solids appear to vanish until
  // a subsequent state change forces a remount (e.g. mouse-hover sets
  // hoveredId → selectedMeshes/hoveredMeshes non-empty → composer
  // remounts → render pipeline reinitialises → solid reappears).
  // Always render EffectComposer with at least one Outline child; when
  // there's nothing to outline pass an empty selection array, which
  // Outline treats as a no-op pass.
  const showSelect = selectedMeshes.length > 0
  const showHover = hoveredMeshes.length > 0

  // The `postprocessing` `OutlineEffect` binds its colour uniforms at
  // construction; mutating `visibleEdgeColor` on a live effect doesn't
  // always re-flow to the shader. Keying the composer on the palette
  // ints forces a clean remount when the theme tokens change, so the
  // selected silhouette always paints with the current `--cad-edge-
  // selected` value rather than the value captured at first mount.
  const composerKey = `${palette.selectVisible}-${palette.hoverVisible}`

  return (
    <EffectComposer key={composerKey} multisampling={4}>
      <Outline
        selection={showSelect ? selectedMeshes : []}
        blendFunction={BlendFunction.ALPHA}
        // Thick and bright: a selection must read at a glance against dark
        // parts in a dense assembly (strength 3 / SMALL was easy to miss).
        edgeStrength={8}
        pulseSpeed={0}
        visibleEdgeColor={palette.selectVisible}
        hiddenEdgeColor={palette.selectHidden}
        kernelSize={KernelSize.MEDIUM}
        blur
        xRay={false}
      />
      <Outline
        selection={showHover ? hoveredMeshes : []}
        blendFunction={BlendFunction.ALPHA}
        edgeStrength={1.5}
        pulseSpeed={0}
        visibleEdgeColor={palette.hoverVisible}
        hiddenEdgeColor={palette.hoverHidden}
        kernelSize={KernelSize.VERY_SMALL}
        blur
        xRay={false}
      />
    </EffectComposer>
  )
}
