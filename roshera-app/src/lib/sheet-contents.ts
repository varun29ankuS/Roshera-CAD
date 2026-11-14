/**
 * A drawing sheet read as a list (the Drawing workspace's Sheet panel): its
 * views in sheet order, each with its dimensions, and the kernel's sheet
 * check (GET /api/drawings/{id}/quality) as one verdict line.
 */
import type { Drawing, DrawingQualityReport, SheetDimension } from './drawings-api.ts'

const KIND_WORDS: Record<string, string> = {
  extent: 'overall size',
  length: 'length',
  diameter: 'diameter',
  radius: 'radius',
  position: 'location',
  angle: 'angle',
  hole_callout: 'hole',
  pcd: 'pitch circle',
  fillet: 'fillet radius',
  chamfer: 'chamfer',
}

/** A dimension kind in plain words. */
export function dimensionWord(kind: string): string {
  return KIND_WORDS[kind] ?? kind.replace(/_/g, ' ')
}

/** A view scale as a drawing writes it: 1:1, 1:2, 2:1. */
function scaleLabel(s: number): string {
  if (s >= 1) return `${Number(s.toFixed(3))}:1`
  return `1:${Number((1 / s).toFixed(3))}`
}

export interface SheetViewRow {
  id: string
  name: string
  scale: string
  dimensions: SheetDimension[]
}

export function sheetContents(d: Pick<Drawing, 'views'>): { views: SheetViewRow[]; dimensionCount: number } {
  const views = d.views.map((v) => ({ id: v.id, name: v.name, scale: scaleLabel(v.scale), dimensions: v.dimensions ?? [] }))
  return { views, dimensionCount: views.reduce((n, v) => n + v.dimensions.length, 0) }
}

/** "A3 · third angle · 5 views". */
export function sheetHeading(d: Pick<Drawing, 'sheet_size' | 'projection' | 'views'>): string {
  const n = d.views.length
  return `${d.sheet_size} · ${d.projection === 'first' ? 'first' : 'third'} angle · ${n} view${n === 1 ? '' : 's'}`
}

export type CheckTone = 'positive' | 'caution' | 'negative' | 'muted'

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

/** The kernel's sheet check as one line; problems before praise. */
export function checkSummary(q: DrawingQualityReport | null): { tone: CheckTone; text: string } {
  if (!q) return { tone: 'muted', text: 'Sheet not checked yet' }
  const errors = q.issues.filter((i) => i.severity === 'error').length
  const warnings = q.issues.length - errors
  const used = `${Math.round(q.sheet_utilization * 100)}% of the sheet used`
  if (!q.passed || errors > 0) {
    return { tone: 'negative', text: [plural(errors, 'problem'), warnings > 0 ? plural(warnings, 'warning') : null].filter(Boolean).join(', ') }
  }
  if (warnings > 0) return { tone: 'caution', text: `Passed with ${plural(warnings, 'warning')} · ${used}` }
  return { tone: 'positive', text: `Sheet check passed · ${used}` }
}
