import { GizmoHelper, GizmoViewport } from '@react-three/drei'

export function GizmoNav() {
  return (
    <GizmoHelper alignment="top-right" margin={[72, 72]}>
      <GizmoViewport
        axisColors={['#e74c3c', '#2ecc71', '#3498db']}
        labelColor="white"
      />
    </GizmoHelper>
  )
}
