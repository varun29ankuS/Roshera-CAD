/**
 * The timeline's rows: chapters (GET /api/timeline/chapters/{branch}).
 *
 * A chapter is one engineering feature, the way a CAD feature tree reads --
 * grouped by the kernel from its recorded steps (a cutter + the subtract that
 * consumes it = one Hole; repeats at regular positions = one Pattern). Each
 * carries its verdict, who made it, and the step after which the part shows
 * it (`anchor`), which is what the film strip pictures
 * (GET /api/timeline/frame/{branch}/{seq}/picture).
 */
import type { ToolIcons } from '@/components/ui/icons'

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

export interface ChapterRefusal {
  sequence: number
  kind: string
  status: string
  reason?: string
}

export interface Chapter {
  /** What the feature is: hole, revolve, box, pattern, ... (the icon key). */
  kind: string
  /** The engineer's name for it, when one was recorded. */
  name: string | null
  /** Every recorded step in this chapter. */
  events: number[]
  /** The step after which the part shows this feature. */
  anchor: number
  /** Every step re-measured sound by the kernel. */
  sound: boolean
  /** The steps that are not, each with the kernel's own status and reason. */
  refusals: ChapterRefusal[]
  author: 'user' | 'agent' | 'system'
  author_detail: { kind: string; name?: string; model?: string }
  /** What an agent said it was doing, when it said. */
  intent: string | null
  /** A pattern's instances. */
  instances: Chapter[]
}

export interface ChaptersResponse {
  branch: string
  chapters: Chapter[]
  is_sound: boolean
}

export async function fetchChapters(branch = 'main'): Promise<ChaptersResponse> {
  const r = await fetch(`${API_HOST}/api/timeline/chapters/${encodeURIComponent(branch)}`)
  if (!r.ok) throw new Error(`chapters: HTTP ${r.status}`)
  return (await r.json()) as ChaptersResponse
}

/** The picture of the part right after step `seq` (a PNG the kernel renders). */
export function framePictureUrl(branch: string, seq: number, size: number): string {
  return `${API_HOST}/api/timeline/frame/${encodeURIComponent(branch)}/${seq}/picture?size=${size}`
}

const KIND_LABEL: Record<string, string> = {
  hole: 'Hole',
  cut: 'Cut',
  boss: 'Boss',
  subtract: 'Subtract',
  union: 'Union',
  intersect: 'Intersect',
  revolve: 'Revolve',
  extrude: 'Extrude',
  fillet: 'Fillet',
  chamfer: 'Chamfer',
  shell: 'Shell',
  loft: 'Loft',
  sweep: 'Sweep',
  box: 'Box',
  cylinder: 'Cylinder',
  sphere: 'Sphere',
  cone: 'Cone',
  torus: 'Torus',
  transform: 'Move',
  set_name: 'Name',
  'drawing.create_from_part': 'Drawing',
}

function readable(kind: string): string {
  const known = KIND_LABEL[kind]
  if (known) return known
  const words = kind.replace(/_/g, ' ').trim()
  return words.charAt(0).toUpperCase() + words.slice(1)
}

/** A chapter's title: its recorded name, else what it is ("Hole pattern ×4"). */
export function chapterTitle(c: Chapter): string {
  if (c.name) return c.name
  if (c.kind === 'pattern') {
    const first = c.instances[0]
    const of = first ? `${readable(first.kind)} pattern` : 'Pattern'
    return `${of} ×${c.instances.length}`
  }
  return readable(c.kind)
}

export type ChapterIconKey = keyof typeof ToolIcons

const ICON_OF: Record<string, ChapterIconKey> = {
  hole: 'hole',
  cut: 'subtract',
  boss: 'union',
  subtract: 'subtract',
  union: 'union',
  intersect: 'intersect',
  revolve: 'revolve',
  extrude: 'extrude',
  fillet: 'fillet',
  chamfer: 'chamfer',
  shell: 'shell',
  loft: 'loft',
  sweep: 'sweep',
  box: 'box',
  cylinder: 'cylinder',
  sphere: 'sphere',
  cone: 'cone',
  torus: 'torus',
  transform: 'translate',
}

/** The tool icon a chapter wears, or null when the kind has none (better no
 *  icon than a wrong one). A pattern wears the pattern icon. */
export function chapterIconKey(kind: string): ChapterIconKey | null {
  if (kind === 'pattern') return 'circularPattern'
  return ICON_OF[kind] ?? null
}
