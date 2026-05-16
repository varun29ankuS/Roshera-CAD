/**
 * Document-mode store.
 *
 * Roshera supports three top-level document types — Part, Assembly,
 * Drawing — and the user picks one at the start of every session
 * (mirrors SolidWorks / Fusion / Onshape). The chosen mode decides
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
  /** Pick a mode (also dismisses the chooser). */
  setMode: (mode: DocumentMode) => void
  /** Drop back to the chooser without changing the kernel state. */
  clearMode: () => void
}

export const useDocModeStore = create<DocModeState>((set) => ({
  // Hash wins over storage so deep links work even after a saved mode.
  mode: modeFromHash() ?? modeFromStorage(),
  setMode: (mode) => {
    persist(mode)
    set({ mode })
  },
  clearMode: () => {
    persist(null)
    set({ mode: null })
  },
}))
