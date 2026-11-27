import { create } from 'zustand'
import type { SheetPoint } from '@/lib/sheet-cursor'

/** What the status bar shows of the open drawing sheet: where the cursor is
 *  on the paper, and the sheet's paper size and scale. */
interface SheetCursorState {
  point: SheetPoint | null
  /** "A3 · 1:1", or null when no sheet is open. */
  sheet: string | null
  setPoint: (point: SheetPoint | null) => void
  setSheet: (sheet: string | null) => void
}

export const useSheetCursorStore = create<SheetCursorState>((set, get) => ({
  point: null,
  sheet: null,
  setPoint: (point) => {
    const cur = get().point
    if (cur === point || (cur && point && cur.x === point.x && cur.y === point.y)) return
    set({ point })
  },
  setSheet: (sheet) => {
    if (get().sheet !== sheet) set({ sheet })
  },
}))
