import { GizmoHelper, GizmoViewcube } from '@react-three/drei'
import { useFrame, type ThreeEvent } from '@react-three/fiber'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { cubePreset } from '@/lib/view-cube'

/** The cube's faces in drei's order (+X, −X, +Y, −Y, +Z, −Z), named for the
 *  kernel's Z-up world: +Y is the back, +Z the top. */
const Z_UP_FACES = ['Right', 'Left', 'Back', 'Front', 'Top', 'Bottom']

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
 * The corner widget is a view cube (the approved mockup, 2026-11-14), its
 * faces named for the kernel's Z-up world.
 */
export function GizmoNav() {
  const hasOutlineTarget = useSceneStore(
    (s) => s.selectedIds.size > 0 || s.hoveredId !== null,
  )
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const dark = useThemeStore((s) => s.theme) === 'dark'
  // In Part and DFM the Blackboard floats over the viewport's right side
  // (w-80 = 320 px + its 8 px gap), so the cube sits left of it. Elsewhere
  // (Assembly) the Blackboard is its own column: the cube hugs the edge.
  const overlaid = useDocModeStore((s) => s.mode) !== 'assembly'
  const onFace = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation()
    // The app's preset (drei's own tween assumes Y-up and degenerates on Top).
    if (e.face) setCameraPreset(cubePreset(e.object.position, e.face.normal))
    return null
  }
  return (
    <>
      <ManualSceneRender active={!hasOutlineTarget} />
      <GizmoHelper alignment="top-right" margin={[overlaid ? 72 + 336 : 72, 72]} renderPriority={2}>
        {/* A view cube (the approved mockup): click a face, edge or corner
            to look from that side. */}
        <group scale={1.15}>
          <GizmoViewcube
            faces={Z_UP_FACES}
            // Bold, larger labels: the thin default read as noise at this size.
            font="700 30px Inter, system-ui, sans-serif"
            color={dark ? '#4a5059' : '#f4f5f7'}
            hoverColor="#f2a516"
            textColor={dark ? '#f3f4f6' : '#1d2125'}
            strokeColor={dark ? '#7b828c' : '#9aa1a9'}
            opacity={1}
            onClick={onFace}
          />
        </group>
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
