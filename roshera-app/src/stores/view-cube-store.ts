/**
 * What the view cube's controls need to know about the live camera and the
 * cube: which face the camera is square-on to, if any; which face is up the
 * screen (a roll changes that); whether the view is the home isometric; and
 * whether the cube is being pointed at. Written from inside the canvas (a
 * frame callback in GizmoNav, pointer events on the cube) only when an
 * answer changes, so the HTML controls beside the cube re-render on a change
 * of view, not on every frame.
 */

import { create } from 'zustand'
import type { FaceName } from '@/lib/view-cube'

interface ViewCubeState {
  /** The face the camera is square-on to, or null. */
  squareOn: FaceName | null
  /** The face whose direction is up the screen, when square-on. */
  upFace: FaceName | null
  /** The camera is at the home isometric view. */
  atHome: boolean
  /** The cursor is on the cube. */
  hovered: boolean
  setSquareOn: (face: FaceName | null, upFace: FaceName | null) => void
  setAtHome: (atHome: boolean) => void
  setHovered: (hovered: boolean) => void
}

export const useViewCubeStore = create<ViewCubeState>((set, get) => ({
  squareOn: null,
  upFace: null,
  atHome: true,
  hovered: false,
  setSquareOn: (face, upFace) => {
    const s = get()
    if (s.squareOn !== face || s.upFace !== upFace) set({ squareOn: face, upFace })
  },
  setAtHome: (atHome) => {
    if (get().atHome !== atHome) set({ atHome })
  },
  setHovered: (hovered) => {
    if (get().hovered !== hovered) set({ hovered })
  },
}))
