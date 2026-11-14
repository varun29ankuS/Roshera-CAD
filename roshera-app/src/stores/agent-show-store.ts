import { create } from 'zustand'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useHistoryView } from '@/stores/history-view-store'
import { FRAME_FIT, useSceneStore } from '@/stores/scene-store'
import { fetchChapters, partChapters } from '@/lib/chapters-api'
import { findFeature, presetFor, showCaption, type ShowAction } from '@/lib/agent-show'

/**
 * The agent driving the app (Varun 2026-11-14: drive, with Pause / Take
 * over). Each `AgentShow` frame is carried out at once unless the user has
 * paused it; the bar (AgentShowBar) says what is being shown and offers
 * Pause and Take over.
 */
interface AgentShowState {
  paused: boolean
  /** What the bar says now; null hides it. */
  showing: { caption: string; at: number; failed?: boolean } | null
  /** Shows that arrived while paused (the bar counts them). */
  missed: number
  /** Faces the agent lit up, drawn by AgentFacesHighlight. */
  faces: { objectId: string; faceIds: number[] } | null
  receive: (a: ShowAction) => Promise<void>
  pause: () => void
  resume: () => void
  /** The user takes over: stop, clear what was shown, stay paused. */
  takeOver: () => void
  dismiss: () => void
}

export const useAgentShowStore = create<AgentShowState>((set, get) => ({
  paused: false,
  showing: null,
  missed: 0,
  faces: null,
  receive: async (a) => {
    if (get().paused) {
      set((s) => ({ missed: s.missed + 1 }))
      return
    }
    const caption = showCaption(a)
    set({ showing: { caption, at: Date.now() } })
    const fail = (why: string) => set({ showing: { caption: `${caption} — ${why}`, at: Date.now(), failed: true } })
    const scene = useSceneStore.getState()
    const toPart = () => {
      const mode = useDocModeStore.getState().mode
      if (mode !== 'part' && mode !== 'dfm') useDocModeStore.getState().setMode('part')
    }
    switch (a.action) {
      case 'workspace':
        useDocModeStore.getState().setMode(a.mode)
        return
      case 'view': {
        const preset = presetFor(a.view)
        if (preset) scene.setCameraPreset(preset)
        else scene.setPendingFrameObject(FRAME_FIT)
        return
      }
      case 'feature': {
        toPart()
        try {
          const { part } = partChapters((await fetchChapters('main')).chapters)
          const f = findFeature(part, a)
          if (!f) return fail('no feature by that name')
          useHistoryView.setState({ selectedChapter: f.anchor })
        } catch {
          fail('the history could not be read')
        }
        return
      }
      case 'faces': {
        toPart()
        const obj = [...scene.objects.values()].find((o) => o.analyticalGeometry?.solidId === a.solid_id)
        if (!obj) return fail(`no part ${a.solid_id} in view`)
        set({ faces: { objectId: obj.id, faceIds: a.face_ids } })
        return
      }
      case 'step':
        toPart()
        useHistoryView.getState().view(a.sequence)
        return
    }
  },
  pause: () => set({ paused: true }),
  resume: () => set({ paused: false, missed: 0 }),
  takeOver: () => set({ paused: true, showing: null, faces: null }),
  dismiss: () => set({ showing: null, faces: null }),
}))
