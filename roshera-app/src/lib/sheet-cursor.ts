/**
 * Where the cursor is on a drawing sheet, in the sheet's own millimetres.
 *
 * Measured from the sheet's bottom-left corner, X to the right and Y up --
 * the origin a drawing frame is set out from -- so the number does not
 * change with zoom or pan. Shown in the status bar: numbers live there.
 */

export interface SheetPoint {
  x: number
  y: number
}

/** The cursor (client pixels) as a point on the sheet, or null when it is
 *  off the paper. `rect` is the sheet's box on screen; `sheet` its paper
 *  size in millimetres. */
export function sheetPointMm(
  client: { x: number; y: number },
  rect: { left: number; top: number; width: number; height: number },
  sheet: { widthMm: number; heightMm: number },
): SheetPoint | null {
  if (!(rect.width > 0) || !(rect.height > 0)) return null
  const u = (client.x - rect.left) / rect.width
  const v = (client.y - rect.top) / rect.height
  if (u < 0 || u > 1 || v < 0 || v > 1) return null
  const tidy = (n: number) => Math.round(n * 1000) / 1000
  return { x: tidy(u * sheet.widthMm), y: tidy((1 - v) * sheet.heightMm) }
}

/** The status-bar readout: a tenth of a millimetre, with the unit. */
export function formatSheetPoint(p: SheetPoint): string {
  return `X ${p.x.toFixed(1)}  Y ${p.y.toFixed(1)} mm`
}
