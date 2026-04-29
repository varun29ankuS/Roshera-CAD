import { GizmoHelper, GizmoViewport } from '@react-three/drei'

/**
 * Orientation widget in the top-right corner. Axis colours keep the
 * conventional X=red / Y=green / Z=blue encoding — this widget's whole
 * purpose is to disambiguate the three world axes. Labels rendered in
 * black so they read clearly against the saturated coloured spheres in
 * both themes.
 *
 * `renderPriority={2}` is required so the gizmo's scissored viewport
 * render runs *after* the `EffectComposer` mounted by `SelectionOutline`
 * (priority 1). Without this the gizmo vanishes the moment a solid is
 * selected, because the composer's full-screen quad pass overwrites the
 * gizmo's corner overlay.
 */
export function GizmoNav() {
  return (
    <GizmoHelper alignment="top-right" margin={[72, 72]} renderPriority={2}>
      <GizmoViewport
        axisColors={['#e74c3c', '#2ecc71', '#3498db']}
        labelColor="#000000"
      />
    </GizmoHelper>
  )
}
