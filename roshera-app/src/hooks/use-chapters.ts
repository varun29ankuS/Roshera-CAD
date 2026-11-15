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
import { useDocumentStore } from '@/stores/document-store'
import { isCurrentScope, scopeToken } from '@/lib/document-scope'
import { createCoalescer } from '@/lib/shared-fetch'

/** Every `useChapters` reader shares one in-flight request per branch and
 *  document (`lib/shared-fetch.ts`): four or five panels react to the same
 *  change, and the route replays and certifies the history each time. */
const shareChapters = createCoalescer<Awaited<ReturnType<typeof fetchChapters>>>()

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
  // A confirmed document switch (`stores/document-store.ts`). The scene's
  // object set usually changes too, but not always (two empty documents, a
  // scene sync that failed), and the chapters must never wait on it: the
  // previous document's history under the new tab is a wrong answer.
  const documentEpoch = useDocumentStore((s) => s.epoch)
  const request = useRef(0)
  const lastShape = useRef('')
  const shapeEpoch = useRef(documentEpoch)

  // Another document: drop its history NOW (empty + loading), in the same
  // render the switch lands in, so the strip, the tree and the counter never
  // show the previous document's steps under the new tab. (React's "adjust
  // state when a prop changes" pattern -- no effect, no stale frame.)
  const [shownEpoch, setShownEpoch] = useState(documentEpoch)
  if (shownEpoch !== documentEpoch) {
    setShownEpoch(documentEpoch)
    setState({ chapters: [], documents: [], isSound: true, loading: true, error: null })
  }

  useEffect(() => {
    const bump = () => setHistoryTick((t) => t + 1)
    window.addEventListener(HISTORY_CHANGED_EVENT, bump)
    return () => window.removeEventListener(HISTORY_CHANGED_EVENT, bump)
  }, [])

  useEffect(() => {
    const id = ++request.current
    const token = scopeToken()
    // The change this load reacts to happened now: a request already in
    // flight that started before it is not shared (it may predate the change).
    const changedAt = performance.now()
    // A new document's history is not a reshaped old one: compare shapes
    // within one document only.
    if (shapeEpoch.current !== documentEpoch) {
      shapeEpoch.current = documentEpoch
      lastShape.current = ''
    }
    // A short settle so a burst of edits (a pattern's instances) loads once.
    const t = window.setTimeout(() => {
      shareChapters(`${token.generation}:${branch}`, changedAt, () => fetchChapters(branch))
        .then((r) => {
          // A newer request, or an answer about a document no longer open.
          if (id !== request.current || !isCurrentScope(token)) return
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
  }, [branch, objects, historyTick, documentEpoch])

  return state
}
