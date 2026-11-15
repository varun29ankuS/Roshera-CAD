import { create } from 'zustand'
import type { LensFace, LensPart } from '@/lib/lens'

/**
 * Kernel Lens: the part being looked at, its journal, and the face whose
 * story is open. Agent Eye's Lens mode fills it; the viewport tints faces
 * from it and the story card reads the open face.
 */
interface LensState {
  /** Scene object id (viewport UUID) of the part, for its mesh. */
  objectId: string | null
  /** Kernel solid id, for the routes. */
  solidId: number | null
  part: LensPart | null
  face: LensFace | null
  setPart: (objectId: string, solidId: number, part: LensPart) => void
  setFace: (face: LensFace | null) => void
  clear: () => void
}

export const useLensStore = create<LensState>((set) => ({
  objectId: null,
  solidId: null,
  part: null,
  face: null,
  setPart: (objectId, solidId, part) =>
    set((s) => ({
      objectId,
      solidId,
      part,
      // The open story stays open while it is the same part.
      face: s.solidId === solidId ? s.face : null,
    })),
  setFace: (face) => set({ face }),
  clear: () => set({ objectId: null, solidId: null, part: null, face: null }),
}))
