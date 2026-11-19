/**
 * Annotating a sheet dimension: the pure part.
 *
 * The drawing tab's property panel edits what stands beside a dimension's
 * value -- its tolerance, the text before and after it, where it sits, and
 * whether it is on the sheet at all. The value itself is measured by the
 * kernel and is never typed here. Everything in this file is the logic
 * between the panel's fields and the one `PATCH
 * /api/drawings/{id}/dimensions/{dim}` the panel sends: what a draft means,
 * what is refused before the server is asked, and what a drag becomes. The
 * kernel validates again and has the last word (an ISO 286 class is resolved
 * from ITS tables, not here).
 *
 * Dependency free so it can be unit-tested directly
 * (`test/dimension-annotate.test.mjs`).
 */

/** A tolerance on one sheet dimension, as the server stores it. */
export type DimTolerance =
  | { type: 'symmetric'; pm: number }
  | { type: 'bilateral'; upper: number; lower: number }
  | { type: 'limits'; upper: number; lower: number }
  | { type: 'fit'; designation: string }
  | { type: 'basic' }
  | { type: 'reference' }

/** Who took a dimension off the sheet, and why. */
export interface Waiver {
  by: string
  reason?: string
}

/** Everything authored on one dimension (kernel `DimensionEdit`). */
export interface DimensionEdit {
  tolerance?: DimTolerance
  prefix?: string
  suffix?: string
  text_shift_mm?: [number, number]
  suppressed?: Waiver
}

/** The tolerance forms the panel offers, in the order a drafter expects. */
export const TOLERANCE_FORMS = [
  { key: 'none', label: 'None', hint: 'General tolerance applies' },
  { key: 'symmetric', label: '±', hint: 'Symmetric: 25 ±0.1' },
  { key: 'bilateral', label: '+/−', hint: 'Two deviations: 25 +0.05 / −0.10' },
  { key: 'limits', label: 'Limits', hint: 'Limits of size: 25.15 / 24.85' },
  { key: 'fit', label: 'Fit', hint: 'ISO 286 class: H7, g6' },
  { key: 'basic', label: 'Basic', hint: 'Theoretically exact, boxed' },
  { key: 'reference', label: 'Ref', hint: 'Reference, in parentheses' },
] as const

export type ToleranceForm = (typeof TOLERANCE_FORMS)[number]['key']

/** What the panel's tolerance fields hold while the engineer types. */
export interface ToleranceDraft {
  form: ToleranceForm
  /** Symmetric value. */
  pm: string
  /** Upper deviation (bilateral) or upper limit (limits). */
  upper: string
  /** Lower deviation (bilateral) or lower limit (limits). */
  lower: string
  /** Fit: hole (upper-case letter) or shaft (lower-case). */
  fitKind: 'hole' | 'shaft'
  fitLetter: string
  fitGrade: string
}

/** Fundamental deviations the kernel's ISO 286 tables carry (shaft letters;
 *  the hole is the same letter in upper case). */
export const FIT_LETTERS = ['c', 'd', 'e', 'f', 'g', 'h', 'js', 'k', 'm', 'n', 'p', 'r', 's', 'u'] as const
/** Grades the kernel's tables carry. */
export const FIT_GRADES = ['5', '6', '7', '8', '9', '10', '11', '12'] as const

const num = (v: number): string => String(Number(v.toFixed(4)))

/** The draft that shows what is authored now. */
export function draftOf(tolerance: DimTolerance | undefined | null): ToleranceDraft {
  const base: ToleranceDraft = {
    form: 'none',
    pm: '',
    upper: '',
    lower: '',
    fitKind: 'hole',
    fitLetter: 'h',
    fitGrade: '7',
  }
  if (!tolerance) return base
  switch (tolerance.type) {
    case 'symmetric':
      return { ...base, form: 'symmetric', pm: num(tolerance.pm) }
    case 'bilateral':
      return { ...base, form: 'bilateral', upper: num(tolerance.upper), lower: num(tolerance.lower) }
    case 'limits':
      return { ...base, form: 'limits', upper: num(tolerance.upper), lower: num(tolerance.lower) }
    case 'fit': {
      const m = /^([A-Za-z]+)(\d+)$/.exec(tolerance.designation.trim())
      if (!m) return { ...base, form: 'fit' }
      const hole = m[1] === m[1].toUpperCase()
      return {
        ...base,
        form: 'fit',
        fitKind: hole ? 'hole' : 'shaft',
        fitLetter: m[1].toLowerCase(),
        fitGrade: m[2],
      }
    }
    case 'basic':
      return { ...base, form: 'basic' }
    case 'reference':
      return { ...base, form: 'reference' }
  }
}

/** The ISO 286 class a draft names: `H7` for a hole, `g6` for a shaft. */
export function fitDesignation(d: Pick<ToleranceDraft, 'fitKind' | 'fitLetter' | 'fitGrade'>): string {
  const letter = d.fitKind === 'hole' ? d.fitLetter.toUpperCase() : d.fitLetter.toLowerCase()
  return `${letter}${d.fitGrade}`
}

/** A number as an engineer types it: a decimal comma is a decimal point. */
export function parseNumber(text: string): number | null {
  const t = text.trim().replace(',', '.')
  if (t === '' || !/^[+-]?(\d+\.?\d*|\.\d+)$/.test(t)) return null
  const v = Number(t)
  return Number.isFinite(v) ? v : null
}

export type ToleranceResult =
  | { ok: true; tolerance: DimTolerance | null }
  | { ok: false; reason: string }

/**
 * The tolerance a draft means: `null` removes it. Refused here, before the
 * server is asked, when the fields are not a tolerance at all; the kernel
 * checks the rest (and resolves the fit from its own tables).
 */
export function toleranceOf(draft: ToleranceDraft): ToleranceResult {
  switch (draft.form) {
    case 'none':
      return { ok: true, tolerance: null }
    case 'basic':
      return { ok: true, tolerance: { type: 'basic' } }
    case 'reference':
      return { ok: true, tolerance: { type: 'reference' } }
    case 'symmetric': {
      const pm = parseNumber(draft.pm)
      if (pm === null || pm <= 0) return { ok: false, reason: 'Enter the tolerance as a positive number, e.g. 0.1.' }
      return { ok: true, tolerance: { type: 'symmetric', pm } }
    }
    case 'bilateral': {
      const upper = parseNumber(draft.upper)
      const lower = parseNumber(draft.lower)
      if (upper === null || lower === null)
        return { ok: false, reason: 'Enter both deviations with their signs, e.g. +0.05 and -0.10.' }
      if (upper <= lower) return { ok: false, reason: 'The upper deviation must be greater than the lower one.' }
      return { ok: true, tolerance: { type: 'bilateral', upper, lower } }
    }
    case 'limits': {
      const upper = parseNumber(draft.upper)
      const lower = parseNumber(draft.lower)
      if (upper === null || lower === null) return { ok: false, reason: 'Enter both limits of size, e.g. 25.15 and 24.85.' }
      if (upper <= lower) return { ok: false, reason: 'The upper limit must be greater than the lower one.' }
      return { ok: true, tolerance: { type: 'limits', upper, lower } }
    }
    case 'fit':
      return { ok: true, tolerance: { type: 'fit', designation: fitDesignation(draft) } }
  }
}

/** Longest text the kernel accepts before or after a value. */
export const MAX_DIM_TEXT = 40

/** What a click on a palette chip inserts into the text field. */
export const TEXT_CHIPS = [
  { label: 'Ø', insert: 'Ø', title: 'Diameter' },
  { label: 'R', insert: 'R', title: 'Radius' },
  { label: '□', insert: '□', title: 'Square' },
  { label: '°', insert: '°', title: 'Degree' },
  { label: '±', insert: '±', title: 'Plus-minus' },
  { label: '⌴', insert: '⌴', title: 'Counterbore' },
  { label: '⌵', insert: '⌵', title: 'Countersink' },
  { label: '↧', insert: '↧', title: 'Depth' },
  { label: 'nX', insert: '4X', title: 'Count, e.g. 4X' },
  { label: 'TYP', insert: 'TYP', title: 'Typical' },
  { label: 'THRU', insert: 'THRU', title: 'Through' },
  { label: 'M', insert: 'M12x1.75-6H', title: 'Thread designation' },
] as const

/** `text` with a chip added: a word gets a space before it, a symbol does not. */
export function withChip(text: string, insert: string): string {
  const word = insert.length > 1
  const sep = text !== '' && word && !text.endsWith(' ') ? ' ' : ''
  return `${text}${sep}${insert}`.slice(0, MAX_DIM_TEXT)
}

/** Why a prefix / suffix cannot be inked, or `null` when it can. */
export function textRefusal(text: string): string | null {
  if (text.length > MAX_DIM_TEXT) return `At most ${MAX_DIM_TEXT} characters; put long text in a note.`
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(text)) return 'One line of printable text.'
  return null
}

/** Round to 0.01 mm, never yielding -0. */
function mm(v: number): number {
  return Math.round(v * 100) / 100 + 0
}

/**
 * Where a dimension's text / line sits after a drag of (`dxPx`, `dyPx`)
 * screen px, as the shift the server records: sheet mm `[right, down]` from
 * where the layout puts it, added to the shift it already had. `null` when
 * the drag ends back where the layout puts it (the shift is then cleared).
 */
export function draggedShift(
  current: [number, number] | undefined | null,
  dxPx: number,
  dyPx: number,
  pxPerMm: number,
): [number, number] | null {
  const from = current ?? [0, 0]
  const to: [number, number] = [mm(from[0] + dxPx / pxPerMm), mm(from[1] + dyPx / pxPerMm)]
  return to[0] === 0 && to[1] === 0 ? null : to
}

/** The certificate fields the panel's chip reads. */
export interface CertificateSummary {
  sound: boolean
  complete: boolean
  counts: {
    stale: number
    dangling: number
    omitted: number
    not_dimensioned?: number
    waived?: number
  }
  omission_summary: string[]
  omissions: SheetOmission[]
  /** Set when the part the sheet was drawn from is not in the model. */
  missing_part?: string | null
}

/** One model dimension no view of the sheet states. */
export interface SheetOmission {
  view: string
  key: string | null
  kind: string
  label: string
  reason: string
  waiver: Waiver | null
}

export type ChipTone = 'good' | 'warn' | 'bad'

/**
 * The one-glance verdict after an edit: is what the sheet shows true, and
 * does it show everything. A tick or a cross, then the count that matters.
 */
export function certificateChip(c: CertificateSummary | null): { tone: ChipTone; mark: string; text: string } {
  if (!c) return { tone: 'warn', mark: '?', text: 'Not verified: the part is not loaded' }
  // The part itself is gone: nothing on the sheet can be fixed, so say what
  // can (the kernel's `missing_part`), never a count of dimensions to fix.
  if (c.missing_part) {
    return { tone: 'bad', mark: '✗', text: 'Part missing: rebuild it or reopen its document' }
  }
  if (!c.sound) {
    const n = c.counts.stale + c.counts.dangling + c.counts.omitted
    return { tone: 'bad', mark: '✗', text: `Not sound: ${n} to fix` }
  }
  const open = c.counts.not_dimensioned ?? 0
  if (!c.complete) {
    return { tone: 'warn', mark: '!', text: `Sound, incomplete: ${open} not dimensioned` }
  }
  const waived = c.counts.waived ?? 0
  return {
    tone: 'good',
    mark: '✓',
    text: waived > 0 ? `Sound and complete (${waived} waived)` : 'Sound and complete',
  }
}

/** Why the selection left a dimension off, in a drafter's words. */
export function omissionReason(reason: string): string {
  switch (reason) {
    case 'angle_clutter':
      return 'more than two angles in the view'
    case 'merged_with_equal_value':
      return 'another dimension of the same value was kept'
    case 'diameter_limit':
      return 'more diameters than the view keeps'
    case 'view_limit':
      return 'more dimensions than the view keeps'
    case 'not_placed':
      return 'carried by the view but not drawn'
    case 'suppressed':
      return 'suppressed'
    default:
      return 'on no view of the sheet'
  }
}
