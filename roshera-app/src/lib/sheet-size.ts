// The paper and scale of an existing sheet: what the Sheet control offers,
// what it sends, and how a refusal reads.
//
// The kernel lays a drawing out again on the paper and scale asked for
// (`PATCH /api/drawings/{id}/sheet`) and REFUSES when the views at that scale
// do not fit -- it never quietly shrinks them. Everything here is plain data
// so it can be checked without a browser.

/** The ISO 216 sizes the control offers, smallest first. */
export const ISO_SHEET_SIZES = ['A4', 'A3', 'A2', 'A1', 'A0'] as const
export type IsoSheetSize = (typeof ISO_SHEET_SIZES)[number]

export type SheetWay = 'landscape' | 'portrait'

/** The ISO 5455 scales the control offers, largest first (the 1-2-5 series
 *  between 10:1 and 1:20; the kernel accepts any ratio, an agent may ask for
 *  others). */
export const ISO_SCALES = [10, 5, 2, 1, 0.5, 0.2, 0.1, 0.05] as const

/** A scale as a drawing states it: `2:1`, `1:1`, `1:2`. */
export function scaleLabel(scale: number): string {
  if (!Number.isFinite(scale) || scale <= 0) return '?'
  const r = (v: number) => String(Math.round(v * 100) / 100)
  return scale >= 1 ? `${r(scale)}:1` : `1:${r(1 / scale)}`
}

/** A sheet's paper and scale. */
export interface SheetChoice {
  size: IsoSheetSize
  way: SheetWay
  scale: number
}

/** The body of `PATCH /api/drawings/{id}/sheet`. */
export interface SheetRequest {
  sheet_size: IsoSheetSize
  orientation: SheetWay
  scale: number
}

const SAME_SCALE = 1e-9

/** The scales to offer for a sheet drawn at `current`: the ISO series, with
 *  the sheet's own scale added in its place when it is not one of them (a
 *  sheet must always show the scale it is at). Largest first. */
export function scaleOptions(current: number): number[] {
  const all: number[] = [...ISO_SCALES]
  if (Number.isFinite(current) && current > 0 && !all.some((s) => Math.abs(s - current) < SAME_SCALE)) {
    all.push(current)
  }
  return all.sort((a, b) => b - a)
}

/**
 * What to send to put a sheet that is `now` on `asked`: the whole choice
 * (size, way up and scale are one layout), or `null` when nothing changes --
 * asking for the paper a sheet is already on is not an edit and must not be
 * recorded as one.
 */
export function sheetRequest(now: SheetChoice, asked: SheetChoice): SheetRequest | null {
  const same =
    now.size === asked.size && now.way === asked.way && Math.abs(now.scale - asked.scale) < SAME_SCALE
  if (same) return null
  return { sheet_size: asked.size, orientation: asked.way, scale: asked.scale }
}

/** The measured parts of a refusal (`details` of the route's error body). */
export interface SheetRefusalDetails {
  gate?: string
  scale?: number
  sheet?: string
  overflow_x_mm?: number
  overflow_y_mm?: number
  largest_scale_that_fits?: number | null
}

/** Why the kernel would not put the sheet on that paper. */
export interface SheetRefusal {
  /** One plain sentence for the engineer. */
  text: string
  /** The largest ISO scale that does fit that paper, when there is one: the
   *  control offers it as the next step. */
  fits: number | null
}

/**
 * A refusal in the engineer's words. A sheet that does not fit says by how
 * much and what would fit; any other refusal is the server's own sentence.
 */
export function sheetRefusal(message: string, details: SheetRefusalDetails | null | undefined): SheetRefusal {
  const d = details ?? {}
  if (d.gate !== 'sheet_does_not_fit') {
    return { text: message.replace(/^REFUSED:\s*/, ''), fits: null }
  }
  const over: string[] = []
  const mm = (v: number | undefined) => (typeof v === 'number' && v > 0 ? `${Math.ceil(v)} mm` : null)
  const wide = mm(d.overflow_x_mm)
  const tall = mm(d.overflow_y_mm)
  if (wide) over.push(`${wide} too wide`)
  if (tall) over.push(`${tall} too tall`)
  const at = typeof d.scale === 'number' ? ` at ${scaleLabel(d.scale)}` : ''
  const on = d.sheet ? ` for ${d.sheet}` : ''
  const fits = typeof d.largest_scale_that_fits === 'number' ? d.largest_scale_that_fits : null
  const how = over.length > 0 ? `: ${over.join(' and ')}` : ''
  const next =
    fits !== null
      ? ` The largest scale that fits is ${scaleLabel(fits)}.`
      : ' No ISO scale fits this paper; choose a larger sheet.'
  return { text: `The views${at} are too large${on}${how}.${next}`, fits }
}
