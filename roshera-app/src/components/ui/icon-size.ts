/**
 * Which rendered icon size to use for an icon drawn at a given CSS size.
 * Icons are rendered natively at each of ICON_SIZES (never stretched): an
 * icon drawn at 14 px uses the 16 px render, and the 2x render (28 -> 32 px)
 * on a high-density screen.
 */
import { ICON_SIZES } from './icon-manifest.ts'

const TAILWIND_SIZE = /(?:^|\s)(?:size|h|w)-(\d+(?:\.\d+)?)(?=\s|$)/
const TAILWIND_PX = /(?:^|\s)(?:size|h|w)-\[(\d+(?:\.\d+)?)px\](?=\s|$)/

/** The CSS size an icon is drawn at: the `size` prop, else a Tailwind
 *  `size-*` / `h-*` / `w-*` class (4 px per unit, or an arbitrary `[Npx]`),
 *  else 24 -- the previous icon library's default. */
export function drawnSize(size: number | string | undefined, className: string | undefined): number {
  if (typeof size === 'number' && Number.isFinite(size) && size > 0) return size
  if (typeof size === 'string') {
    const n = Number.parseFloat(size)
    if (Number.isFinite(n) && n > 0) return n
  }
  if (className) {
    const px = TAILWIND_PX.exec(className)
    if (px) return Number(px[1])
    const tw = TAILWIND_SIZE.exec(className)
    if (tw) return Number(tw[1]) * 4
  }
  return 24
}

/** The smallest rendered size that is at least `px` (the largest if none is). */
export function renderedSize(px: number): number {
  for (const s of ICON_SIZES) if (s >= px) return s
  return ICON_SIZES[ICON_SIZES.length - 1]
}
