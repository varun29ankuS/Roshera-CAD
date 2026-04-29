import { GizmoHelper, GizmoViewport } from '@react-three/drei'

/**
 * Orientation widget in the top-right corner. Axis colours keep the
 * conventional X=red / Y=green / Z=blue encoding — this widget's whole
 * purpose is to disambiguate the three world axes. Labels rendered in
 * black so they read clearly against the saturated coloured spheres in
 * both themes.
 */
export function GizmoNav() {
  return (
    <GizmoHelper alignment="top-right" margin={[72, 72]}>
      <GizmoViewport
        axisColors={['#e74c3c', '#2ecc71', '#3498db']}
        labelColor="#000000"
      />
    </GizmoHelper>
  )
}
