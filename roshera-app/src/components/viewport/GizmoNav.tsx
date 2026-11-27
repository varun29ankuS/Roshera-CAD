import { useMemo } from 'react'
import { GizmoHelper } from '@react-three/drei'
import { useFrame, useThree } from '@react-three/fiber'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { cubeView, squareOnFace } from '@/lib/view-cube'
import { resolveCssVar } from '@/lib/css-color'
import { ISO_DIRECTION } from '@/lib/camera-fit'
import { useViewCubeStore } from '@/stores/view-cube-store'
import { ViewCube, type ViewCubeColors } from './ViewCube'
import { CUBE_RIGHT, CUBE_TOP } from './ViewCubeControls'
import { useDockWidth } from '@/stores/dock-store'


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
const _forward = new THREE.Vector3()
const _up = new THREE.Vector3()

export function GizmoNav() {
  const hasOutlineTarget = useSceneStore(
    (s) => s.selectedIds.size > 0 || s.hoveredId !== null,
  )
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const dark = useThemeStore((s) => s.theme) === 'dark'
  // In Part and DFM the Blackboard floats over the viewport's right side
  // (its dock width + its 8 px gap), so the cube sits left of it. Elsewhere
  // (Assembly) the Blackboard is its own column: the cube hugs the edge.
  const overlaid = useDocModeStore((s) => s.mode) !== 'assembly'
  const dockPx = useDockWidth()
  const setCameraView = useSceneStore((s) => s.setCameraView)
  // A click on any of the cube's 26 pieces turns to that piece's view. The
  // six faces and the home corner go by the app's named views, so the view
  // bar's buttons stay in step; the other edges and corners go by direction.
  const onPick = (piece: THREE.Vector3, normal: THREE.Vector3) => {
    const view = cubeView(piece, normal)
    if (view.preset) setCameraPreset(view.preset)
    else setCameraView(view)
  }
  // Light faces on the dark viewport, mid-grey on the light one, in both
  // themes; the pre-highlight is the app's hover blue.
  const colors = useMemo<ViewCubeColors>(
    () => ({
      face: dark ? '#d9dce1' : '#cfd4da',
      text: dark ? '#23282e' : '#1f242a',
      stroke: dark ? '#5b626b' : '#6f7781',
      hover: `#${resolveCssVar('--cad-hovered').color.getHexString()}`,
    }),
    [dark],
  )
  // Tell the controls beside the cube when the camera is square-on to a
  // face (their arrows only mean something then). Read every rendered frame,
  // stored only when the answer changes.
  const camera = useThree((s) => s.camera)
  const setSquareOn = useViewCubeStore((s) => s.setSquareOn)
  const setAtHome = useViewCubeStore((s) => s.setAtHome)
  useFrame(() => {
    const f = camera.getWorldDirection(_forward)
    const face = squareOnFace([-f.x, -f.y, -f.z])
    // Which face is up the screen (a roll changes it): the camera's own up,
    // read off its orientation.
    const u = _up.set(0, 1, 0).applyQuaternion(camera.quaternion)
    setSquareOn(face, face ? squareOnFace([u.x, u.y, u.z]) : null)
    // Home is the isometric view with Z up the screen (within a few degrees).
    setAtHome(-f.x * ISO_DIRECTION[0] - f.y * ISO_DIRECTION[1] - f.z * ISO_DIRECTION[2] > 0.999 && u.z > 0.6)
  })
  return (
    <>
      <ManualSceneRender active={!hasOutlineTarget} />
      <GizmoHelper
        alignment="top-right"
        // Below the view bar, with room above the cube for its "face above"
        // arrow; clear of the docked Blackboard by its width.
        margin={[overlaid ? CUBE_RIGHT + dockPx + 16 : CUBE_RIGHT, CUBE_TOP]}
        renderPriority={2}
      >
        <ViewCube colors={colors} onPick={onPick} mainCamera={camera} />
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
