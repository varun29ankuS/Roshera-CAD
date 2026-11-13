/**
 * The document's chapters (GET /api/timeline/chapters/main), reloaded after
 * every change to the part: an operation (the scene's object set changes),
 * an undo or redo (HISTORY_CHANGED_EVENT). The chapter list and the film
 * strip read the same copy.
 */
import { useEffect, useRef, useState } from 'react'
import { fetchChapters, partChapters, type Chapter } from '@/lib/chapters-api'
import { HISTORY_CHANGED_EVENT } from '@/lib/history'
import { useSceneStore } from '@/stores/scene-store'
import { forgetKeptSteps } from '@/stores/history-view-store'

export interface ChaptersState {
  /** The part's own features: the tree, the slider and the film strip. */
  chapters: Chapter[]
  /** The document's steps (drawings made from the part), kept apart. */
  documents: Chapter[]
  isSound: boolean
  loading: boolean
  error: string | null
}

export function useChapters(branch = 'main'): ChaptersState {
  const [state, setState] = useState<ChaptersState>({
    chapters: [],
    documents: [],
    isSound: true,
    loading: true,
    error: null,
  })
  // Any change to the scene is a new step in the history.
  const objects = useSceneStore((s) => s.objects)
  const [historyTick, setHistoryTick] = useState(0)
  const request = useRef(0)
  const lastShape = useRef('')

  useEffect(() => {
    const bump = () => setHistoryTick((t) => t + 1)
    window.addEventListener(HISTORY_CHANGED_EVENT, bump)
    return () => window.removeEventListener(HISTORY_CHANGED_EVENT, bump)
  }, [])

  useEffect(() => {
    const id = ++request.current
    // A short settle so a burst of edits (a pattern's instances) loads once.
    const t = window.setTimeout(() => {
      fetchChapters(branch)
        .then((r) => {
          if (id !== request.current) return
          // The history changed shape (undo, redo, an edited chapter): a
          // kept step may now mean a different state -- forget them all.
          const shape = JSON.stringify(r.chapters.map((c) => [c.anchor, c.events]))
          if (lastShape.current && shape !== lastShape.current) forgetKeptSteps()
          lastShape.current = shape
          const { part, documents } = partChapters(r.chapters)
          setState({ chapters: part, documents, isSound: r.is_sound, loading: false, error: null })
        })
        .catch((e: unknown) => {
          if (id !== request.current) return
          setState((s) => ({ ...s, loading: false, error: e instanceof Error ? e.message : String(e) }))
        })
    }, 150)
    return () => window.clearTimeout(t)
  }, [branch, objects, historyTick])

  return state
}
