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

/** Whether a chapter is a step of the DOCUMENT rather than a feature of the
 *  part: a drawing made from the part changes nothing about it (its picture
 *  is the step before's). In SolidWorks or Inventor a drawing is its own
 *  document, never a row in the part's tree (Varun 2026-11-13: "why is
 *  there a drawing on the timeline?"). */
export function isDocumentChapter(c: Chapter): boolean {
  // Drawings and assemblies are documents MADE FROM the part: building
  // one changes nothing about it.
  return c.kind.startsWith('drawing') || c.kind.startsWith('assembly')
}

/** A size edit (`param.mould`, `param.name`) is recorded history, never a
 *  feature: editing the hole radius changes the hole, it adds no hole. */
function isEditStep(kind: string): boolean {
  return kind.startsWith('param.')
}

/** Whether a chapter is a folded edit (see `foldEdits`). */
export function isEdit(c: Chapter): boolean {
  return c.kind === 'edit'
}

/**
 * Edits folded: consecutive size-edit steps become ONE `edit` chapter (a
 * pattern's edit writes one step per instance -- four holes, four steps --
 * for what the engineer did once). The slider and film strip keep it as a
 * moment of the history, because the part after it really differs (a frame
 * cut before the edit still shows the old size); the feature tree leaves it
 * out (`isEdit`), the edited feature showing its new size itself.
 */
export function foldEdits(cs: Chapter[]): Chapter[] {
  const out: Chapter[] = []
  for (const c of cs) {
    if (!isEditStep(c.kind)) {
      out.push(c)
      continue
    }
    // A delete and a size edit are different moments: grouped apart, and
    // named for what they did.
    const name = c.kind === 'param.suppress' ? 'Delete' : 'Edit'
    const prev = out[out.length - 1]
    if (prev && isEdit(prev) && prev.name === name) {
      out[out.length - 1] = {
        ...prev,
        events: [...prev.events, ...c.events],
        anchor: c.anchor,
        sound: prev.sound && c.sound,
        refusals: [...prev.refusals, ...c.refusals],
      }
    } else {
      out.push({ ...c, kind: 'edit', name })
    }
  }
  return out
}

/** The part's own history (tree, slider, film strip) and, apart, the
 *  document's steps (drawings) -- listed, not hidden. */
export function partChapters(cs: Chapter[]): { part: Chapter[]; documents: Chapter[] } {
  return {
    part: foldEdits(cs.filter((c) => !isDocumentChapter(c))),
    documents: cs.filter(isDocumentChapter),
  }
}
