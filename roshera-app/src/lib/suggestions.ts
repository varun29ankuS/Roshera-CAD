/**
 * The prompts suggested under an empty composer, from the part's own
 * history (Varun 2026-11-14: "the suggestive prompts should show based on
 * the previous history"). Plain rules over the recorded features -- what is
 * there and what is missing -- so each suggestion has a reason a reader can
 * see, and none is made up: a fillet is suggested only where a boss meets
 * what it stands on and no fillet exists yet, a chamfer only where there are
 * holes and no chamfer, a drawing only when none has been made.
 */
import type { Chapter } from './chapters-api.ts'

const HOLES = new Set(['hole', 'cut', 'subtract', 'pattern'])

function nameOf(c: Chapter | undefined, fallback: string): string {
  const n = c?.name?.trim()
  return n ? n.charAt(0).toUpperCase() + n.slice(1) : fallback
}

/** Up to three prompts. `chapters`: the part's history (edits folded);
 *  `documents`: its document steps (drawings). */
export function suggestPrompts(chapters: Chapter[], documents: Chapter[]): string[] {
  if (chapters.length === 0) {
    return [
      'Make a flange Ø80 × 12 with a Ø45 hub and four bolt holes',
      'Sketch a bracket and extrude it 10 mm',
      'What can you build?',
    ]
  }
  const out: string[] = []
  const kinds = new Set(chapters.map((c) => c.kind))
  const last = chapters[chapters.length - 1]

  if (last.kind === 'edit') out.push('Check the part after my last edit')

  const boss = chapters.find((c) => c.kind === 'boss')
  if (boss && !kinds.has('fillet')) {
    out.push(`Fillet where the ${nameOf(boss, 'boss')} meets the ${nameOf(chapters[0], 'base')} (R2)`)
  }
  if (chapters.some((c) => HOLES.has(c.kind)) && !kinds.has('chamfer')) {
    out.push('Chamfer the hole edges 0.5 mm')
  }
  if (documents.length === 0) out.push('Make a drawing of this part')

  for (const fallback of ['Check it for 3D printing', 'Is this part sound?', 'Walk me through the history']) {
    if (out.length >= 3) break
    out.push(fallback)
  }
  return out.slice(0, 3)
}
