/**
 * First-run guidance: what a first-time engineer sees after signing in, and
 * the sign-in screen's plain-language failures.
 *
 * Varun (2026-11-18): "there is also no first time sign in explanations ...
 * no way for anyone to know what to do". The guide is four cards -- one per
 * thing a newcomer must know to get anywhere -- shown once per browser until
 * dismissed, and reopenable from the command palette (Ctrl/Cmd-K, "Getting
 * started"). Pure logic here so it is testable (test/first-run.test.mjs);
 * the cards are components/FirstRunGuide.tsx.
 */

/** One card of the guide: a label, one or two sentences, and where to look. */
export interface GuideStep {
  title: string
  body: string
  /** Where on the screen this lives, so the reader can find it. */
  where: string
}

export const GUIDE_STEPS: readonly GuideStep[] = [
  {
    title: 'Ask the agent',
    body: 'Type what you want, in your own words: "a 60 × 40 × 10 mm plate with four Ø6.6 holes". The agent builds it in the viewport while you watch.',
    where: 'The Blackboard, on the right',
  },
  {
    title: 'Point at what you mean',
    body: 'Click a face or edge, then say "fillet this edge 2 mm". Whatever you select goes with your message, so "this" is never ambiguous.',
    where: 'The "Agent sees" chip in the status bar shows what is shared',
  },
  {
    title: 'Read the verdict',
    body: 'Every step the agent takes is checked by the kernel. ✓ means the part is sound; ✗ names what failed and where. Nothing is claimed unchecked.',
    where: 'The "Kernel evidence" row at the end of each agent turn',
  },
  {
    title: 'Get the drawing',
    body: 'Drop a STEP file on the viewport, or use a part you built, and open the Drawing tab for a certified production drawing you can edit.',
    where: 'The Drawing tab, top bar',
  },
]

/** localStorage key recording that this browser has seen the guide. */
export const GUIDE_SEEN_KEY = 'roshera_getting_started_seen'

type KeyValueStore = Pick<Storage, 'getItem' | 'setItem'>

/** Show the guide unless this browser has dismissed it. Storage that cannot
 *  be read (private mode, blocked site data) shows it: a newcomer missing
 *  the guide is worse than seeing it twice. */
export function shouldShowGuide(storage: KeyValueStore): boolean {
  try {
    return storage.getItem(GUIDE_SEEN_KEY) === null
  } catch {
    return true
  }
}

/** Remember that the guide was dismissed. Never throws. */
export function markGuideSeen(storage: KeyValueStore): void {
  try {
    storage.setItem(GUIDE_SEEN_KEY, new Date().toISOString())
  } catch {
    /* non-fatal: the guide just shows again next time */
  }
}

/** The sign-in failure in words a person can act on. */
export function friendlyLoginError(raw: string): string {
  const s = raw.toLowerCase()
  if (s.includes('failed to fetch') || s.includes('networkerror') || s.includes('network error') || s.includes('load failed')) {
    return "Can't reach the Roshera server. Check that it is running, then try again."
  }
  if (s.includes('429') || s.includes('too many') || s.includes('rate')) {
    return 'Too many attempts. Wait a minute, then try again.'
  }
  if (s.includes('invalid credentials') || s.includes('401') || s.includes('unauthor') || s.includes('invalid username') || s.includes('invalid password')) {
    return "That username and password don't match. Check for caps lock and try again."
  }
  return 'The server could not sign you in. Try again; if it keeps failing, restart the server.'
}

// --- the sign-in showcase ------------------------------------------------

/** What a showcase image shows. */
export type ShowcaseKind = 'part' | 'assembly' | 'drawing'

/** One image on the sign-in screen: real Roshera output (a part render, an
 *  assembly render, a drawing sheet), listed in /showcase/manifest.json. */
export interface ShowcaseItem {
  kind: ShowcaseKind
  title: string
  caption: string
  src: string
}

export const SHOWCASE_LABEL: Record<ShowcaseKind, string> = {
  part: 'Part',
  assembly: 'Assembly',
  drawing: 'Drawing',
}

/** The well-formed items of a showcase manifest. Only same-origin images
 *  under /showcase/ of a known kind with a title are kept: the screen shows
 *  what we made, never a picture from elsewhere. */
export function parseShowcase(raw: unknown): ShowcaseItem[] {
  if (!raw || typeof raw !== 'object') return []
  const items = (raw as { items?: unknown }).items
  if (!Array.isArray(items)) return []
  const kinds = new Set<string>(['part', 'assembly', 'drawing'])
  const out: ShowcaseItem[] = []
  for (const it of items) {
    if (!it || typeof it !== 'object') continue
    const { kind, title, caption, src } = it as Record<string, unknown>
    if (typeof kind !== 'string' || !kinds.has(kind)) continue
    if (typeof title !== 'string' || title.trim() === '') continue
    if (typeof src !== 'string' || !src.startsWith('/showcase/') || !/\.(png|jpe?g|webp|svg)$/i.test(src)) continue
    out.push({
      kind: kind as ShowcaseKind,
      title: title.trim(),
      caption: typeof caption === 'string' ? caption.trim() : '',
      src,
    })
  }
  return out
}

// --- reopening the guide -------------------------------------------------

type OpenListener = () => void
const openListeners = new Set<OpenListener>()

/** Ask the mounted guide to open (the command palette's "Getting started"). */
export function openGuide(): void {
  for (const fn of openListeners) fn()
}

/** Subscribe the mounted guide to open requests. */
export function onOpenGuide(fn: OpenListener): () => void {
  openListeners.add(fn)
  return () => openListeners.delete(fn)
}
