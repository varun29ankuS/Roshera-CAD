import { GizmoHelper, GizmoViewport } from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'

/**
 * Orientation widget in the top-right corner. Axis colours keep the
 * conventional X=red / Y=green / Z=blue encoding — this widget's whole
 * purpose is to disambiguate the three world axes. Labels rendered in
 * black so they read clearly against the saturated coloured spheres in
 * both themes.
 *
 * Render-priority dance: `@react-three/fiber` disables its automatic
 * render loop the moment *any* child has `renderPriority > 0`. We only
 * want manual ordering when `SelectionOutline` actually mounts an
 * `EffectComposer` (priority 1) — which only happens with a live
 * selection or hover. Outside that window we leave priority at 0 so
 * fiber auto-renders the main scene normally; otherwise the grid,
 * reference axes, and scene objects vanish while the gizmo (the only
 * priority-driven subscriber) keeps drawing.
 */
export function GizmoNav() {
  const hasOutlineTarget = useSceneStore(
    (s) => s.selectedIds.size > 0 || s.hoveredId !== null,
  )
  return (
    <GizmoHelper
      alignment="top-right"
      margin={[72, 72]}
      renderPriority={hasOutlineTarget ? 2 : 0}
    >
      <GizmoViewport
        axisColors={['#e74c3c', '#2ecc71', '#3498db']}
        labelColor="#000000"
      />
    </GizmoHelper>
  )
}
