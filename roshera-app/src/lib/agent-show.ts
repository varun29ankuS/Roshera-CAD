/**
 * The agent SHOWING the user something (server: POST /api/agent/show, one
 * `AgentShow` frame on the live channel). The pure part: what each action
 * means for the app -- which camera preset, which feature, what the
 * "Claude is showing you" bar says. The store (stores/agent-show-store)
 * carries it out.
 */
import { chapterTitle, type Chapter } from './chapters-api.ts'
import type { SheetRect } from './sheet-look.ts'

export type Workspace = 'part' | 'assembly' | 'drawing' | 'dfm'
export type NamedView = 'iso' | 'front' | 'back' | 'top' | 'bottom' | 'left' | 'right' | 'fit'

export type ShowAction =
  | { action: 'workspace'; mode: Workspace; caption?: string | null }
  | { action: 'view'; view: NamedView; caption?: string | null }
  | { action: 'feature'; name?: string | null; anchor?: number | null; caption?: string | null }
  | { action: 'faces'; solid_id: number; face_ids: number[]; caption?: string | null }
  | { action: 'step'; sequence: number; caption?: string | null }
  // The agent looked at a space of a drawing sheet (or the whole sheet when
  // `space` is absent): the Drawing workspace opens that drawing and glides
  // to the space's box (lib/sheet-look).
  | {
      action: 'sheet_space'
      drawing_id: string
      space?: string | null
      rect?: SheetRect | null
      caption?: string | null
    }

/** The app's camera preset for an agent view name; null for Fit. */
export function presetFor(v: NamedView): string | null {
  if (v === 'fit') return null
  return v === 'iso' ? 'isometric' : v
}

const norm = (s: string) => s.trim().toLowerCase().replace(/\s+/g, ' ')

/** The feature the agent means: by anchor, or by name (case and spacing
 *  ignored), a pattern's instances included. Null when none matches --
 *  never a guess. */
export function findFeature(chapters: Chapter[], q: { name?: string | null; anchor?: number | null }): Chapter | null {
  const all = chapters.flatMap((c) => [c, ...c.instances])
  if (typeof q.anchor === 'number') return all.find((c) => c.anchor === q.anchor) ?? null
  if (!q.name || !norm(q.name)) return null
  const want = norm(q.name)
  return all.find((c) => norm(chapterTitle(c)) === want || (c.name !== null && norm(c.name) === want)) ?? null
}

const WORKSPACE_NAMES: Record<Workspace, string> = { part: 'Part', assembly: 'Assembly', drawing: 'Drawing', dfm: 'DFM' }

/** What the bar says: the agent's own caption, else what is being shown. */
export function showCaption(a: ShowAction): string {
  if (a.caption && a.caption.trim()) return a.caption.trim()
  switch (a.action) {
    case 'workspace':
      return `the ${WORKSPACE_NAMES[a.mode]} workspace`
    case 'view':
      return a.view === 'fit' ? 'the whole model' : `the ${a.view} view`
    case 'feature':
      return a.name ? `the feature ${a.name}` : `the feature at step ${a.anchor}`
    case 'faces':
      return `${a.face_ids.length} face${a.face_ids.length === 1 ? '' : 's'}`
    case 'step':
      return `the part after step ${a.sequence}`
    case 'sheet_space':
      return a.space ? `the sheet space ${a.space}` : 'the whole sheet'
  }
}
