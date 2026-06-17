/**
 * Document-mode store.
 *
 * Roshera supports three top-level document types — Part, Assembly,
 * Drawing — and the user picks one at the start of every session
 * (mirrors mainstream parametric CAD). The chosen mode decides
 * which toolset, which panels, and which workspace layout are
 * presented; the kernel itself is mode-agnostic.
 *
 * # Persistence
 *
 * The selected mode is persisted in `localStorage.roshera_document_mode`
 * so reopening the tab lands back in the same workspace. A URL hash
 * (`#/part`, `#/assembly`, `#/drawing`) takes precedence on first load,
 * letting deep links override the saved value.
 *
 * # Routing semantics
 *
 * `mode === null` means "show the chooser overlay". Setting a mode
 * dismisses the chooser; calling `clearMode()` brings it back without
 * touching the kernel.
 */

import { create } from 'zustand'

export type DocumentMode = 'part' | 'assembly' | 'drawing'

const STORAGE_KEY = 'roshera_document_mode'

/** Parse `window.location.hash` for an explicit mode override. */
function modeFromHash(): DocumentMode | null {
  if (typeof window === 'undefined') return null
  const h = window.location.hash.replace(/^#\/?/, '').toLowerCase()
  if (h === 'part' || h === 'assembly' || h === 'drawing') return h
  return null
}

/** Read the persisted mode from localStorage, if any. */
function modeFromStorage(): DocumentMode | null {
  if (typeof window === 'undefined') return null
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY)
    if (raw === 'part' || raw === 'assembly' || raw === 'drawing') return raw
  } catch {
    // localStorage disabled (private mode, sandboxed iframe) — fall
    // through and start at the chooser, no error to the user.
  }
  return null
}

function persist(mode: DocumentMode | null) {
  if (typeof window === 'undefined') return
  try {
    if (mode === null) {
      window.localStorage.removeItem(STORAGE_KEY)
    } else {
      window.localStorage.setItem(STORAGE_KEY, mode)
    }
  } catch {
    // ignore — see note in modeFromStorage()
  }
}

interface DocModeState {
  /** Active workspace; `null` = show the chooser overlay. */
  mode: DocumentMode | null
  /**
   * Drawing the Drawing workspace should focus on its next mount, set by
   * the viewport "Create Drawing" flow. The workspace consumes and
   * clears this so it only steers the initial selection, not every
   * subsequent visit. `null` = no pending navigation.
   */
  pendingDrawingId: string | null
  /** Pick a mode (also dismisses the chooser). */
  setMode: (mode: DocumentMode) => void
  /** Drop back to the chooser without changing the kernel state. */
  clearMode: () => void
  /** Switch to the Drawing workspace and focus `id` once it mounts. */
  openDrawing: (id: string) => void
  /** Read-and-clear the pending drawing id (workspace mount handshake). */
  consumePendingDrawing: () => string | null
}

export const useDocModeStore = create<DocModeState>((set, get) => ({
  // Hash wins over storage so deep links work even after a saved mode.
  mode: modeFromHash() ?? modeFromStorage(),
  pendingDrawingId: null,
  setMode: (mode) => {
    persist(mode)
    set({ mode })
  },
  clearMode: () => {
    persist(null)
    set({ mode: null })
  },
  openDrawing: (id) => {
    persist('drawing')
    set({ mode: 'drawing', pendingDrawingId: id })
  },
  consumePendingDrawing: () => {
    const id = get().pendingDrawingId
    if (id !== null) set({ pendingDrawingId: null })
    return id
  },
}))
