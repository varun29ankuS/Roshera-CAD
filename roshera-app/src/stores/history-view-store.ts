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

/** Steps already fetched, newest last: sliding back and forth re-reads them.
 *  Bounded (a step's meshes can be several MB); the oldest is dropped. */
const MAX_KEPT = 16
const kept = new Map<string, CADObject[]>()
const inFlight = new Map<string, Promise<CADObject[]>>()

function keyOf(branch: string, step: number): string {
  return `${branch}:${step}`
}

function load(branch: string, step: number): Promise<CADObject[]> {
  const key = keyOf(branch, step)
  const have = kept.get(key)
  if (have) {
    // Refresh its place as the most recently used.
    kept.delete(key)
    kept.set(key, have)
    return Promise.resolve(have)
  }
  const pending = inFlight.get(key)
  if (pending) return pending
  const p = fetchScrub(branch, step)
    .then((objects) => {
      kept.set(key, objects)
      while (kept.size > MAX_KEPT) {
        const oldest = kept.keys().next().value
        if (oldest === undefined) break
        kept.delete(oldest)
      }
      return objects
    })
    .finally(() => inFlight.delete(key))
  inFlight.set(key, p)
  return p
}

/** Fetch a step ahead of time (the slider's neighbours) without showing it. */
export function prefetchStep(step: number, branch = 'main'): void {
  load(branch, step).catch(() => {
    // A failed prefetch is retried when the step is actually viewed.
  })
}

/** Forget every kept step: the history changed, so a step number may now
 *  mean a different state. */
export function forgetKeptSteps(): void {
  kept.clear()
}

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
    const ready = kept.get(keyOf(branch, step))
    if (ready) {
      set({ step, label, objects: ready, loading: false, error: null })
      return
    }
    set({ step, label, loading: true, error: null })
    load(branch, step)
      .then((objects) => {
        if (id === request) set({ objects, loading: false })
      })
      .catch((e: unknown) => {
        if (id === request) set({ loading: false, error: e instanceof Error ? e.message : String(e) })
      })
  },
}))
