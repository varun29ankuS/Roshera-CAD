/**
 * Which past step the viewport is showing (null = the live part), and that
 * step's meshes. Set by the film strip and the chapter list; the viewport
 * draws `objects` read-only in place of the live part while `step` is set.
 * A newer request always wins over an older one still in flight, so a fast
 * drag along the strip settles on where the pointer stopped.
 */
import { create } from 'zustand'
import type { CADObject } from '@/stores/scene-store'
import { fetchScrub } from '@/lib/history-view'

interface HistoryViewState {
  /** The step being shown, or null for the live part. */
  step: number | null
  /** What the user called that step ("Hole", "Bolt hole 1 of 4"). */
  label: string | null
  objects: CADObject[]
  loading: boolean
  error: string | null
  /** Show the part right after `step` (null: back to the live part). */
  view: (step: number | null, label?: string | null, branch?: string) => void
  /** The chapter selected in the chapter list (its anchor step), or null.
   *  Selecting a chapter highlights it, as in a feature tree; it never moves
   *  the timeline -- only the slider travels in time (Varun 2026-11-13). */
  selectedChapter: number | null
  /** Select a chapter; selecting the selected one again clears it. */
  selectChapter: (anchor: number | null) => void
}

let request = 0

export const useHistoryView = create<HistoryViewState>((set) => ({
  step: null,
  label: null,
  objects: [],
  loading: false,
  error: null,
  selectedChapter: null,
  selectChapter: (anchor) =>
    set((s) => ({ selectedChapter: anchor !== null && s.selectedChapter === anchor ? null : anchor })),
  view: (step, label = null, branch = 'main') => {
    const id = ++request
    if (step === null) {
      set({ step: null, label: null, objects: [], loading: false, error: null })
      return
    }
    set({ step, label, loading: true, error: null })
    fetchScrub(branch, step)
      .then((objects) => {
        if (id === request) set({ objects, loading: false })
      })
      .catch((e: unknown) => {
        if (id === request) set({ loading: false, error: e instanceof Error ? e.message : String(e) })
      })
  },
}))
