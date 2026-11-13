import { create } from 'zustand'
import type { DfmReport } from '@/lib/dfm'

/**
 * The DFM workspace's result: the last report, which part it was for, and
 * the finding the user is looking at. The viewport reads it to colour the
 * faces; the panel reads it to list the findings.
 */
interface DfmState {
  /** Scene object id (viewport UUID) the report is about. */
  objectId: string | null
  report: DfmReport | null
  /** Rule id of the finding in focus; its faces are drawn strongest. */
  focusRule: string | null
  setResult: (objectId: string, report: DfmReport) => void
  clear: () => void
  focus: (rule: string | null) => void
}

export const useDfmStore = create<DfmState>((set) => ({
  objectId: null,
  report: null,
  focusRule: null,
  setResult: (objectId, report) => set({ objectId, report, focusRule: null }),
  clear: () => set({ objectId: null, report: null, focusRule: null }),
  focus: (focusRule) => set({ focusRule }),
}))
