/**
 * Which documents the tab strip shows.
 *
 * Varun (2026-11-18): "there are few 10's of tabs open". The strip showed
 * every document ever made except the ones closed in that browser, so a fresh
 * browser opened onto a wall of tabs (33 on this server). Now: only the
 * documents opened in this browser, plus the active one (it can never
 * vanish); every other document stays one click away under +.
 */

export const OPEN_TABS_KEY = 'roshera.tabs.open.v1'

type KeyValueStore = Pick<Storage, 'getItem' | 'setItem'>

interface DocLike {
  id: string
  active: boolean
  createdAt: number
}

export function visibleTabs<T extends DocLike>(docs: readonly T[], open: ReadonlySet<string>): T[] {
  return docs.filter((d) => d.active || open.has(d.id))
}

/** The documents under +: not shown as tabs, newest first. */
export function hiddenDocuments<T extends DocLike>(docs: readonly T[], open: ReadonlySet<string>): T[] {
  return docs.filter((d) => !d.active && !open.has(d.id)).sort((a, b) => b.createdAt - a.createdAt)
}

export function withOpened(open: ReadonlySet<string>, id: string): Set<string> {
  if (open.has(id)) return open as Set<string>
  return new Set(open).add(id)
}

export function withClosed(open: ReadonlySet<string>, id: string): Set<string> {
  if (!open.has(id)) return open as Set<string>
  const next = new Set(open)
  next.delete(id)
  return next
}

export function readOpenIds(storage: KeyValueStore): Set<string> {
  try {
    const raw = storage.getItem(OPEN_TABS_KEY)
    if (!raw) return new Set()
    const parsed: unknown = JSON.parse(raw)
    return Array.isArray(parsed) ? new Set(parsed.filter((x): x is string => typeof x === 'string')) : new Set()
  } catch {
    return new Set()
  }
}

export function writeOpenIds(storage: KeyValueStore, open: ReadonlySet<string>): void {
  try {
    storage.setItem(OPEN_TABS_KEY, JSON.stringify(Array.from(open)))
  } catch {
    /* storage blocked: the strip works for this session */
  }
}
