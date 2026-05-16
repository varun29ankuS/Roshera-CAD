/**
 * Command-palette store.
 *
 * Tracks whether the palette overlay is mounted-and-visible. Kept
 * deliberately tiny — the palette's command list is *derived* in
 * `CommandPalette.tsx` from whichever stores are currently relevant
 * (scene, doc-mode, theme, WS), not duplicated here, so we don't end
 * up with a second source of truth for what an action does.
 *
 * Opening is idempotent. `openWith(prefill)` lets the caller seed the
 * search query (useful for "AI Chat" / contextual deep links later);
 * closing always clears the query so the next open starts fresh.
 */

import { create } from 'zustand'

interface CommandPaletteState {
  open: boolean
  query: string
  setOpen: (open: boolean) => void
  setQuery: (query: string) => void
  openWith: (prefill?: string) => void
  close: () => void
  toggle: () => void
}

export const useCommandPaletteStore = create<CommandPaletteState>((set, get) => ({
  open: false,
  query: '',
  setOpen: (open) => set({ open, query: open ? get().query : '' }),
  setQuery: (query) => set({ query }),
  openWith: (prefill) => set({ open: true, query: prefill ?? '' }),
  close: () => set({ open: false, query: '' }),
  toggle: () => {
    const next = !get().open
    set({ open: next, query: next ? get().query : '' })
  },
}))
