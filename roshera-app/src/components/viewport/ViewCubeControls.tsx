import type { ReactNode } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useViewCubeStore } from '@/stores/view-cube-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { FACE_VIEWS, neighbourFace, rollView, type FaceName } from '@/lib/view-cube'
import { cn } from '@/lib/utils'

/**
 * The controls around the view cube: light, icon-only, and only there when
 * they mean something.
 *
 * Home -- the isometric view, fitted -- is a small house at the cube's
 * top-left, where Fusion puts it. It shows whenever the view is NOT home,
 * and while the cube is pointed at. Square-on to a face (Front, Top, any of
 * the six) an arrow sits on each side of the cube, just outside its edge,
 * and goes to the face on that side; two roll arrows at the top-right turn
 * the view a quarter turn about the line of sight. They appear only when
 * square-on, because "the face to the right" has no meaning from a corner.
 * An arrow always arrives at that face's standard view -- the one its button
 * in the view bar gives -- so there is one Top, one Right; each arrow's
 * tooltip names the view it goes to.
 *
 * Every control is a 24 px target, drawn as a bare icon that takes the hover
 * blue. Laid over the canvas where GizmoNav draws the cube: `CUBE_TOP` px
 * down, and clear of the docked Blackboard by its width (`--bb-w`).
 */

/** The cube's centre, from the viewport's top edge and from its right edge
 *  (plus the dock's width where the Blackboard floats over the viewport). */
export const CUBE_TOP = 104
export const CUBE_RIGHT = 76

/** How far a control's centre sits from the cube's: just outside the edge of
 *  a square-on face (23 px half-width), with the triad's tips clear. */
const REACH = 40

const TITLE: Record<FaceName, string> = {
  front: 'Front',
  back: 'Back',
  left: 'Left',
  right: 'Right',
  top: 'Top',
  bottom: 'Bottom',
}

function Control({
  x,
  y,
  title,
  onClick,
  shown = true,
  children,
}: {
  x: number
  y: number
  title: string
  onClick: () => void
  /** Hidden controls stay in place and reachable: pointing at one, or
   *  focusing it with the keyboard, shows it. */
  shown?: boolean
  children: ReactNode
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      style={{ left: x, top: y }}
      className={cn(
        'cad-focus pointer-events-auto absolute flex h-6 w-6 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded text-muted-foreground transition-opacity duration-100',
        'hover:bg-[color-mix(in_oklch,var(--cad-hovered)_16%,transparent)] hover:text-[var(--cad-hovered)] hover:opacity-100 focus-visible:opacity-100',
        shown ? 'opacity-100' : 'opacity-0',
      )}
    >
      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        {children}
      </svg>
    </button>
  )
}

export function ViewCubeControls() {
  const squareOn = useViewCubeStore((s) => s.squareOn)
  const upFace = useViewCubeStore((s) => s.upFace)
  const atHome = useViewCubeStore((s) => s.atHome)
  const hovered = useViewCubeStore((s) => s.hovered)
  const overlaid = useDocModeStore((s) => s.mode) !== 'assembly'
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const setCameraView = useSceneStore((s) => s.setCameraView)

  // The view on screen, from the two faces the store tracks.
  const here = squareOn && upFace ? { direction: FACE_VIEWS[squareOn].direction, up: FACE_VIEWS[upFace].direction } : null
  const to = (side: 'left' | 'right' | 'up' | 'down') => (here ? neighbourFace(here.direction, here.up, side) : null)

  const arrow = (side: 'left' | 'right' | 'up' | 'down', x: number, y: number, path: ReactNode) => {
    const face = to(side)
    if (!face) return null
    return (
      <Control x={x} y={y} title={`${TITLE[face]} view`} onClick={() => setCameraPreset(face)}>
        {path}
      </Control>
    )
  }
  const roll = (turns: number) => {
    if (here) setCameraView(rollView(here.direction, here.up, turns))
  }

  return (
    <div
      role="group"
      aria-label="View cube controls"
      className="pointer-events-none absolute z-10 h-0 w-0 select-none"
      style={{
        top: CUBE_TOP,
        right: overlaid ? `calc(var(--bb-w) + 16px + ${CUBE_RIGHT}px)` : CUBE_RIGHT,
      }}
    >
      <Control
        x={-REACH - 6}
        y={-REACH - 2}
        title="Home: isometric view, fitted (Shift+7)"
        shown={!atHome || hovered}
        onClick={() => setCameraPreset('isometric')}
      >
        <path d="M2.5 7.5 8 2.75l5.5 4.75" />
        <path d="M4 6.5v6.75h8V6.5" />
      </Control>
      {arrow('left', -REACH, 0, <path d="M10 3 5 8l5 5" />)}
      {arrow('right', REACH, 0, <path d="m6 3 5 5-5 5" />)}
      {arrow('up', 0, -REACH, <path d="m3 10 5-5 5 5" />)}
      {arrow('down', 0, REACH, <path d="m3 6 5 5 5-5" />)}
      {here && (
        <>
          <Control x={REACH - 8} y={-REACH - 2} title="Roll the view 90° anticlockwise" onClick={() => roll(-1)}>
            <path d="M3.5 8a4.5 4.5 0 1 0 1.6-3.4" />
            <path d="M5.5 1.75 5 4.75l3 .5" />
          </Control>
          <Control x={REACH + 16} y={-REACH - 2} title="Roll the view 90° clockwise" onClick={() => roll(1)}>
            <path d="M12.5 8a4.5 4.5 0 1 1-1.6-3.4" />
            <path d="M10.5 1.75l.5 3-3 .5" />
          </Control>
        </>
      )}
    </div>
  )
}
