/**
 * The docked Blackboard's width: the user's choice (remembered), the window's
 * width, and the width that follows from the two (lib/dock-width.ts).
 *
 * The app root publishes it as the CSS variable `--bb-w`; the dock, and
 * everything laid out beside it (view bar, tool panels, the history strip,
 * the view cube), reads that one number.
 */

import { useEffect } from 'react'
import { create } from 'zustand'
import { DOCK_MAX, DOCK_MIN, dockWidth } from '@/lib/dock-width'

const KEY = 'roshera.blackboard.dockWidth'

function readSaved(): number | null {
  try {
    const raw = window.localStorage.getItem(KEY)
    if (raw === null) return null
    const n = Number(raw)
    return Number.isFinite(n) ? n : null
  } catch {
    return null
  }
}

interface DockState {
  /** The width the user dragged to, or null for the default. */
  saved: number | null
  windowWidth: number
  /** Choose a width (clamped to the allowed range) and remember it. */
  setSaved: (px: number) => void
  /** Back to the default for the window. */
  reset: () => void
  setWindowWidth: (px: number) => void
}

export const useDockStore = create<DockState>((set) => ({
  saved: typeof window === 'undefined' ? null : readSaved(),
  windowWidth: typeof window === 'undefined' ? 1920 : window.innerWidth,
  setSaved: (px) => {
    const clamped = Math.round(Math.min(Math.max(px, DOCK_MIN), DOCK_MAX))
    try {
      window.localStorage.setItem(KEY, String(clamped))
    } catch {
      // Remembering is a convenience; the width still applies for this session.
    }
    set({ saved: clamped })
  },
  reset: () => {
    try {
      window.localStorage.removeItem(KEY)
    } catch {
      // As above.
    }
    set({ saved: null })
  },
  setWindowWidth: (px) => set({ windowWidth: px }),
}))

/** The dock's width in CSS pixels. */
export function useDockWidth(): number {
  return useDockStore((s) => dockWidth(s.saved, s.windowWidth))
}

/** Keeps the store's window width current. Mount once, at the app root. */
export function useTrackWindowWidth(): void {
  const setWindowWidth = useDockStore((s) => s.setWindowWidth)
  useEffect(() => {
    const onResize = () => setWindowWidth(window.innerWidth)
    onResize()
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [setWindowWidth])
}
