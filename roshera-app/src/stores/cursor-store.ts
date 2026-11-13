import { create } from 'zustand'

/** The point under the cursor on a part, for the status bar. */
export interface CursorHit {
  objectId: string
  point: [number, number, number]
  /** The kernel face under the cursor, when the mesh carries face ids. */
  faceId: number | undefined
}

interface CursorState {
  hit: CursorHit | null
  setHit: (hit: CursorHit | null) => void
}

export const useCursorStore = create<CursorState>((set) => ({
  hit: null,
  setHit: (hit) => set({ hit }),
}))
