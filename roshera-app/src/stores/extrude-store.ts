import { create } from 'zustand'
import type { PreviewMesh } from '@/lib/face-extrude'

/**
 * The Extrude tool: open while the user extrudes a face by typing a
 * distance. The face arrow publishes the face it sits on (`target`); the
 * Extrude panel previews and commits through lib/face-extrude.ts and puts
 * its ghost here for the arrow to draw, so a typed extrude and a dragged
 * one look the same.
 */
export interface ExtrudeTarget {
  objectId: string
  faceId: number
  /** The direction the arrow points (the face's outward normal). */
  axis: [number, number, number]
}

interface ExtrudeState {
  open: boolean
  target: ExtrudeTarget | null
  preview: PreviewMesh | null
  /** The signed distance being previewed (mm), for the arrow's length. */
  distance: number | null
  setOpen: (open: boolean) => void
  setTarget: (t: ExtrudeTarget | null) => void
  setPreview: (distance: number | null, preview: PreviewMesh | null) => void
}

export const useExtrudeStore = create<ExtrudeState>((set) => ({
  open: false,
  target: null,
  preview: null,
  distance: null,
  setOpen: (open) => set(open ? { open } : { open, preview: null, distance: null }),
  setTarget: (target) => set({ target, preview: null, distance: null }),
  setPreview: (distance, preview) => set({ distance, preview }),
}))
