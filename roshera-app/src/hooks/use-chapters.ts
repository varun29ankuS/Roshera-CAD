/**
 * The document's chapters (GET /api/timeline/chapters/main), reloaded after
 * every change to the part: an operation (the scene's object set changes),
 * an undo or redo (HISTORY_CHANGED_EVENT). The chapter list and the film
 * strip read the same copy.
 */
import { useEffect, useRef, useState } from 'react'
import { fetchChapters, type Chapter } from '@/lib/chapters-api'
import { HISTORY_CHANGED_EVENT } from '@/lib/history'
import { useSceneStore } from '@/stores/scene-store'

export interface ChaptersState {
  chapters: Chapter[]
  isSound: boolean
  loading: boolean
  error: string | null
}

export function useChapters(branch = 'main'): ChaptersState {
  const [state, setState] = useState<ChaptersState>({
    chapters: [],
    isSound: true,
    loading: true,
    error: null,
  })
  // Any change to the scene is a new step in the history.
  const objects = useSceneStore((s) => s.objects)
  const [historyTick, setHistoryTick] = useState(0)
  const request = useRef(0)

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
          setState({ chapters: r.chapters, isSound: r.is_sound, loading: false, error: null })
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
