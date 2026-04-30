import { GizmoHelper, GizmoViewport } from '@react-three/drei'
import { useFrame } from '@react-three/fiber'
import { useSceneStore } from '@/stores/scene-store'

/**
 * Render-priority dance — the long version, because this has bitten us
 * twice now.
 *
 * `@react-three/fiber` disables its automatic main-scene render the
 * moment any subscriber calls `useFrame(_, priority)` with `priority > 0`.
 * `<GizmoHelper>` requires a positive `renderPriority` to render its
 * corner widget at all (it draws into an offscreen render target whose
 * frame callback only fires when priority > 0). So we cannot simply
 * leave the gizmo at priority 0 to keep the main scene's auto-render
 * alive — that hides the gizmo.
 *
 * Two consumers want non-zero priority:
 *   1. `<GizmoHelper>` — always, so the corner axes are visible.
 *   2. `<EffectComposer>` inside `<SelectionOutline>` — only mounts when
 *      there is a live selection or hover.
 *
 * When the EffectComposer is mounted it renders the main scene at
 * priority 1, then the gizmo renders on top at priority 2. When it is
 * NOT mounted nothing else drives the main scene render — that is the
 * job of `<ManualSceneRender>` below, which fires at priority 1 only
 * while no outline target exists. Conditioning on `!hasOutlineTarget`
 * avoids double-rendering the scene the frame the EffectComposer takes
 * over (and the frame it tears down).
 *
 * Axis colours keep the conventional X=red / Y=green / Z=blue encoding —
 * this widget's whole purpose is to disambiguate the three world axes.
 * Labels are rendered in black so they read clearly against the
 * saturated coloured spheres in both themes.
 */
export function GizmoNav() {
  const hasOutlineTarget = useSceneStore(
    (s) => s.selectedIds.size > 0 || s.hoveredId !== null,
  )
  return (
    <>
      <ManualSceneRender active={!hasOutlineTarget} />
      <GizmoHelper alignment="top-right" margin={[72, 72]} renderPriority={2}>
        <GizmoViewport
          axisColors={['#e74c3c', '#2ecc71', '#3498db']}
          labelColor="#000000"
        />
      </GizmoHelper>
    </>
  )
}

/**
 * Drives the main-scene render at priority 1 whenever no priority-1
 * subscriber (`<EffectComposer>`) is mounted. Without this, fiber's
 * disabled auto-render leaves the scene blank as soon as the gizmo
 * registers its priority-2 frame.
 */
function ManualSceneRender({ active }: { active: boolean }) {
  useFrame(({ gl, scene, camera }) => {
    if (!active) return
    gl.render(scene, camera)
  }, 1)
  return null
}
