import { create } from 'zustand'

/**
 * Transient, auto-clearing surface for backend action refusals (undo,
 * redo, branch switch, checkpoint, truncate, ...) that would otherwise
 * be silently identical to success in the UI.
 *
 * A zustand store (not component-local state) so non-React modules —
 * e.g. `lib/shortcuts.ts`'s keyboard-shortcut handlers, which run
 * outside any component tree — can flash a message into the same
 * channel a panel renders. `Timeline.tsx` is the only current renderer,
 * but the store itself has no opinion about who reads it.
 *
 * The 6s auto-clear timer lives here (not in the renderer) so it is
 * "latest flash wins": a new `flash()` call always restarts the single
 * timer rather than each caller racing its own.
 */
interface ActionErrorState {
  message: string | null
  /** Set (or replace) the visible message; restarts the auto-clear timer. */
  flash: (message: string) => void
  /** Clear immediately and cancel the pending auto-clear. */
  clear: () => void
}

const AUTO_CLEAR_MS = 6000

let clearTimer: ReturnType<typeof setTimeout> | null = null

export const useActionErrorStore = create<ActionErrorState>((set) => ({
  message: null,

  flash: (message) => {
    if (clearTimer !== null) {
      clearTimeout(clearTimer)
    }
    set({ message })
    clearTimer = setTimeout(() => {
      clearTimer = null
      set({ message: null })
    }, AUTO_CLEAR_MS)
  },

  clear: () => {
    if (clearTimer !== null) {
      clearTimeout(clearTimer)
      clearTimer = null
    }
    set({ message: null })
  },
}))
